//! 特效：方块排（机体那种「屏幕下方一排乐器方块」）。
//!
//! 它是**叠加**表面：不接管中央面板，直接压在网格/钢琴栏之上，所以开着它还能照常
//! 编辑。每个还在响的音符在它**所按键的正上方**落一个方块，方块的样子就是那枚音符
//! 的乐器对应的方块（和导出 .nbt 用的同一张表），所以画面和机体里实际摆的方块对得上。
//!
//! 「以初始位置为中心持续位移几个像素」这条要求位移必须是 `now` 的纯函数：
//! 每帧从「家」重新算，而不是「上帧位置 += 一点」，否则拖动播放头、或者切走再切回，
//! 方块会漂走或攒出一堆历史。`now` 用的是墙钟，活不活跃用的却是播放头 —— 前者决定
//! 摆动，后者决定哪些音符还在响，暂停时方块该安静地留在原处而不是继续换一批。

use serde::{Deserialize, Serialize};

use egui::{Color32, Pos2, Rect, Ui, Vec2};

use super::keyboard::{KeyboardLayout, WHITE_HIT_H};
use super::stage::Frame;
use super::tiles::{fallback_color, tile_size};
use super::{EffectInfo, FxContext, PlaybackEffect, Surface};
use crate::tr;

/// 一帧最多摆几个方块。密集段落一秒能进几十个音，超出的按「只留最新的」截。
pub const MAX_BLOCKS: usize = 240;

/// 方块在「家」周围摆的相位种子步进：相邻声道错开一点，一整排才不像一块板。
const PHASE_PER_SLOT: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// 一个音过了之后方块还留几秒（按播放头算，不按墙钟）。
    pub life_seconds: f32,
    /// 摆动的幅度（像素，已乘界面缩放前）。
    pub nudge_px: f32,
    /// 每秒摆几次。
    pub nudge_hz: f32,
    /// 方块边长相对 [`tile_size`] 的倍率。
    pub tile_scale: f32,
    /// 用方块贴图；关掉或素材没扒到时用稳定色块。
    pub use_tiles: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            life_seconds: 1.6,
            nudge_px: 3.0,
            nudge_hz: 1.5,
            tile_scale: 1.0,
            use_tiles: true,
        }
    }
}

/// 方块排。
#[derive(Default)]
pub struct BlockRow {
    params: Params,
    /// 上一帧有没有因为缺素材退成色块（用来提一句「记得扒贴图」）。
    tiles_missing: bool,
    /// 上一帧是否截断过方块数。
    capped: bool,
}

impl BlockRow {
    pub const ID: &'static str = "block_row";

    /// 方块落在哪副键的上方：优先用底部钢琴栏的摆放，没有就自己在下缘铺一条。
    pub fn layout(cx: &FxContext) -> KeyboardLayout {
        if let Some(layout) = cx.keyboard {
            return layout;
        }
        let scale = cx.scale.max(f32::EPSILON);
        let height = WHITE_HIT_H * scale;
        KeyboardLayout::fit(
            Rect::from_min_size(
                Pos2::new(cx.area.min.x, cx.area.max.y - height),
                Vec2::new(cx.area.width(), height),
            ),
            scale,
            27,
        )
    }

    /// 围绕「家」的偏移量：两个方向都是正弦，一个周期内均值 0（真的以初始位置为中心）。
    pub fn nudge(phase: f32, amp: f32) -> (f32, f32) {
        // 纵向用 2 倍频 + 相位差，轨迹是个小 8 字，比纯左右晃更像机体里的抖动。
        (amp * phase.sin(), amp * 0.4 * (2.0 * phase + 0.8).sin())
    }

    /// `(摆动包络, 透明度包络)`：刚击中时摆最大，之后线性收；最后三成寿命淡出。
    /// 包络下限 0.35 —— 「持续位移」要的是它一直在动，不是抖一下就停。
    pub fn envelope(age: f32, life: f32) -> (f32, f32) {
        let life = life.max(0.1);
        let r = (age / life).clamp(0.0, 1.0);
        (0.35 + 0.65 * (1.0 - r), ((1.0 - r) / 0.3).clamp(0.0, 1.0))
    }

    /// 同一枚键上多个声道叠在一起时的小偏移：一个 2×2 的子格，按声道号取。
    fn slot_offset(layer: u32, tile: f32) -> (f32, f32) {
        let s = layer % 4;
        let dx = if s % 2 == 0 { -0.28 } else { 0.28 };
        let dy = -0.42 * (s / 2) as f32;
        (dx * tile, dy * tile)
    }

    fn fade(color: Color32, k: f32) -> Color32 {
        // 只压 alpha：`from_rgba_premultiplied` 就是「按这 4 个数存」，
        // RGB 不动才不会淡出的同时把方块颜色也压暗。
        Color32::from_rgba_premultiplied(
            color.r(),
            color.g(),
            color.b(),
            (color.a() as f32 * k.clamp(0.0, 1.0)) as u8,
        )
    }
}

impl PlaybackEffect for BlockRow {
    fn info(&self) -> EffectInfo {
        EffectInfo {
            id: Self::ID,
            name: tr("Instrument blocks", "乐器方块排"),
            surface: Surface::Overlay,
            z: 10,
            // 叠加在有钢琴栏的画面上最好看（方块正好压在键上方），
            // 用户把钢琴栏关掉时底座会补一条，不然方块没地方摆。
            wants_piano: true,
        }
    }

    fn params(&self) -> serde_json::Value {
        serde_json::to_value(self.params).unwrap_or_default()
    }

    fn set_params(&mut self, value: &serde_json::Value) {
        if let Ok(parsed) = serde_json::from_value::<Params>(value.clone()) {
            self.params = parsed;
        }
    }

    fn settings_ui(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let p = &mut self.params;
        changed |= ui
            .add(
                egui::Slider::new(&mut p.life_seconds, 0.3..=6.0)
                    .suffix(" s")
                    .text(tr("Block lifetime", "方块停留")),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut p.nudge_px, 0.0..=14.0)
                    .suffix(" px")
                    .text(tr("Shift distance", "位移幅度")),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut p.nudge_hz, 0.2..=6.0)
                    .suffix(" Hz")
                    .text(tr("Shift speed", "位移频率")),
            )
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut p.tile_scale, 0.5..=2.5).text(tr("Block size", "方块大小")))
            .changed();
        changed |= ui
            .checkbox(&mut p.use_tiles, tr("Block textures", "方块贴图"))
            .changed();
        changed
    }

    fn note(&self) -> Option<String> {
        if self.capped {
            Some(
                tr(
                    "Too many live notes, only the newest blocks are drawn.",
                    "同时发声的音符太多，只画了最新的一批方块。",
                )
                .to_string(),
            )
        } else if self.tiles_missing {
            Some(
                tr(
                    "Block textures not found, using flat colors (run tools/fetch_block_textures.py).",
                    "没找到方块贴图，改用色块代替（跑一次 tools/fetch_block_textures.py 再重启）。",
                )
                .to_string(),
            )
        } else {
            None
        }
    }

    fn draw(&mut self, cx: &FxContext, out: &mut Frame) {
        let scale = cx.scale.max(f32::EPSILON);
        let layout = Self::layout(cx);
        let tile = tile_size(scale) * self.params.tile_scale.max(0.2);
        // 「家」的底边压在键盘上缘之上，留出一点缝，别让方块盖住键面。
        let base_y = layout.origin.y - tile * 0.5 - 4.0 * scale;
        let now_seconds = cx.timeline.seconds_at(cx.position) as f64;
        let from = cx
            .timeline
            .tick_at_seconds(now_seconds - self.params.life_seconds as f64);
        let mut live = cx.timeline.notes_in(from, cx.position + 0.001);
        self.capped = live.len() > MAX_BLOCKS;
        if self.capped {
            live = &live[live.len() - MAX_BLOCKS..];
        }

        let amp = self.params.nudge_px * scale;
        let mut missing = false;
        for note in live {
            let age = (now_seconds - cx.timeline.seconds_at(f64::from(note.tick)) as f64) as f32;
            if age < 0.0 || age > self.params.life_seconds {
                continue;
            }
            let Some(home) = layout.rect_of_key(note.key) else {
                // 这枚键本帧不在画面里（用户把钢琴栏拖到只露一半），方块跟着不画。
                continue;
            };
            let (swing, alpha) = Self::envelope(age, self.params.life_seconds);
            let phase = std::f32::consts::TAU * self.params.nudge_hz * cx.now as f32
                + note.key as f32 * PHASE_PER_SLOT;
            let (dx, dy) = Self::nudge(phase, amp * swing);
            let (ox, oy) = Self::slot_offset(note.layer, tile);
            let center = Pos2::new(
                home.center().x + dx + ox,
                base_y + dy + oy - home.height() * 0.0,
            );
            let rect = Rect::from_center_size(center, Vec2::new(tile, tile));

            let texture = self
                .params
                .use_tiles
                .then(|| cx.tiles?.texture_for(cx.tex, cx.blocks, note.instrument))
                .flatten();
            match texture {
                Some(tex) => out.image(rect, tex, Color32::from_white_alpha((255.0 * alpha) as u8)),
                None => {
                    missing = true;
                    let color = fallback_color(cx.blocks.name(note.instrument));
                    out.rect(rect, Self::fade(color, alpha), 2.0 * scale);
                    out.outlined(
                        rect,
                        Color32::TRANSPARENT,
                        (1.0 * scale, Self::fade(Color32::from_gray(20), alpha)),
                        2.0 * scale,
                    );
                }
            }
        }
        self.tiles_missing = missing;
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::keyboard::KEY_W;
    use crate::fx::stage::Primitive;
    use crate::fx::tiles::{BlockTiles, InstrumentBlocks};
    use crate::fx::timeline::FxTimeline;
    use egui::Context;
    use nbs_core::model::{Layer, Note, Song};

    /// `(tick, layer, instrument, key)`。
    fn song(notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::new();
        s.meta.tempo = 10.0;
        for (tick, layer, instrument, key) in notes {
            while s.layers.len() <= *layer as usize {
                s.layers.push(Layer::default());
            }
            s.set(*tick, *layer, Note::new(*instrument, *key));
        }
        s.recalculate();
        s
    }

    struct Rig {
        ctx: Context,
        timeline: FxTimeline,
        blocks: InstrumentBlocks,
        tiles: BlockTiles,
        layout: Option<KeyboardLayout>,
    }

    impl Rig {
        /// `open_in(None)` 不读盘：素材缺失那条退化路径才是可预期的，
        /// 测试不该跟着本机有没有扒过贴图一起变绿变红。
        fn new(notes: &[(u32, u32, u8, u8)]) -> Self {
            Self {
                ctx: Context::default(),
                timeline: FxTimeline::bake(&song(notes), false),
                blocks: InstrumentBlocks::default(),
                tiles: BlockTiles::open_in(None),
                layout: None,
            }
        }

        /// 底部钢琴栏在位的那种画面（叠加表面的正常情形）。
        fn with_keyboard(mut self) -> Self {
            self.layout = Some(KeyboardLayout::fit(
                Rect::from_min_size(Pos2::new(20.0, 600.0), Vec2::new(52.0 * KEY_W, WHITE_HIT_H)),
                1.0,
                0,
            ));
            self
        }

        fn cx(&self, position: f64, now: f64) -> FxContext<'_> {
            FxContext {
                tex: &self.ctx,
                now,
                dt: 0.05,
                position,
                playing: true,
                speed: 1.0,
                scale: 1.0,
                accent: Color32::from_rgb(80, 160, 255),
                area: Rect::from_min_size(Pos2::ZERO, Vec2::new(2200.0, 800.0)),
                keyboard: self.layout,
                timeline: &self.timeline,
                blocks: &self.blocks,
                tiles: Some(&self.tiles),
            }
        }
    }

    /// 色块那条路径里，一个方块 = 一个无描边的填充矩形（描边那圈另算）。
    fn blocks(frame: &Frame) -> Vec<Rect> {
        frame
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect {
                    rect,
                    stroke: None,
                    ..
                } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    fn centers(frame: &Frame) -> Vec<Pos2> {
        blocks(frame).iter().map(|r| r.center()).collect()
    }

    /// 方块的「家」：那枚键的道中心，加上同键多声道时让开的那半格。
    /// 位移是围绕这个点算的，不是围绕键道中心 —— 否则同一列的方块会互相盖住。
    fn home(rig: &Rig, key: u8, layer: u32) -> f32 {
        let layout = rig.layout.unwrap();
        let tile = tile_size(1.0) * Params::default().tile_scale;
        let (ox, _) = BlockRow::slot_offset(layer, tile);
        layout.rect_of_key(key).unwrap().center().x + ox
    }

    #[test]
    fn a_block_lands_over_the_key_it_plays() {
        let rig = Rig::new(&[(10, 0, 0, 60)]).with_keyboard();
        let layout = rig.layout.unwrap();
        let want = home(&rig, 60, 0);
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(11.0, 0.0), &mut frame);
        assert_eq!(centers(&frame).len(), 1, "一个还在响的音符 → 一个方块");
        let got = centers(&frame)[0];
        assert!(
            (got.x - want).abs() <= Params::default().nudge_px + 0.01,
            "{got:?} 该贴着键 {want}"
        );
        assert!(got.y < layout.origin.y, "方块压在键盘上缘之上");
        // 一个方块就是一格填充 + 一圈描边，不该画成两格。
        assert_eq!(frame.prims.len(), 2);
    }

    #[test]
    fn future_and_finished_notes_get_nothing() {
        // 音符在 2 列，另一枚在 60 列把曲子撑长（否则播放头会被夹到曲尾，看不出「早就响完了」）。
        let rig = Rig::new(&[(2, 0, 0, 60), (60, 0, 7, 62)]).with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        frame.clear();
        fx.draw(&rig.cx(0.0, 0.0), &mut frame);
        assert!(blocks(&frame).is_empty(), "还没响的音符不该有方块");
        frame.clear();
        fx.draw(&rig.cx(2.5, 0.0), &mut frame);
        assert_eq!(blocks(&frame).len(), 1, "刚响过的该有");
        frame.clear();
        fx.draw(&rig.cx(30.0, 0.0), &mut frame);
        assert!(blocks(&frame).is_empty(), "过了寿命就该消失");
    }

    /// 「以初始位置为中心持续位移」：一个完整周期里位移均值 0，两个方向都到得了边界。
    #[test]
    fn the_nudge_oscillates_around_home() {
        let rig = Rig::new(&[(0, 0, 0, 60)]).with_keyboard();
        let want = home(&rig, 60, 0);
        let mut fx = BlockRow::default();
        // 32 个采样正好一个周期（`nudge_hz` = 1.5）。
        let step = 1.0 / (Params::default().nudge_hz * 32.0);
        let mut dxs = Vec::new();
        for i in 0..64u32 {
            let mut frame = Frame::default();
            fx.draw(&rig.cx(0.5, i as f64 * step as f64), &mut frame);
            dxs.push(centers(&frame).first().copied().map(|c| c.x - want));
        }
        let dxs: Vec<f32> = dxs.into_iter().flatten().collect();
        assert_eq!(dxs.len(), 64, "每个采样都该有方块");
        let amp = Params::default().nudge_px;
        assert!(dxs.iter().all(|d| d.abs() <= amp + 0.01), "{dxs:?}");
        assert!(dxs.iter().any(|d| *d > amp * 0.8), "要摆得到 +幅度");
        assert!(dxs.iter().any(|d| *d < -amp * 0.8), "要摆得到 -幅度");
        let mean = dxs.iter().sum::<f32>() / dxs.len() as f32;
        assert!(mean.abs() < amp * 0.1, "位移均值要贴着 0，实际 {mean}");
    }

    /// 位移是 `now` 的纯函数：画过别的帧不该影响这一帧摆在哪。
    #[test]
    fn the_row_ignores_its_own_history() {
        let rig = Rig::new(&[(4, 0, 0, 48), (5, 1, 3, 55)]).with_keyboard();
        let mut once = BlockRow::default();
        let mut twice = BlockRow::default();
        let mut a = Frame::default();
        let mut b = Frame::default();
        once.draw(&rig.cx(12.0, 3.25), &mut a);
        twice.draw(&rig.cx(6.0, 1.0), &mut b);
        twice.draw(&rig.cx(9.0, 2.0), &mut b);
        b.clear();
        twice.draw(&rig.cx(12.0, 3.25), &mut b);
        assert_eq!(blocks(&a), blocks(&b));
        assert_eq!(a.images.len(), b.images.len());
    }

    #[test]
    fn missing_tiles_fall_back_to_stable_colors() {
        let rig = Rig::new(&[(0, 0, 0, 48), (0, 1, 7, 60)]).with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(2.0, 0.0), &mut frame);
        let fills: Vec<Color32> = frame
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect {
                    fill,
                    stroke: None,
                    ..
                } => Some(*fill),
                _ => None,
            })
            .collect();
        assert_eq!(fills.len(), 2, "两个乐器方块");
        assert_ne!(fills[0], fills[1], "piano 和 bass 的方块要看得出区别");
        let msg = fx.note().unwrap();
        assert!(msg.contains("贴图") || msg.contains("textures"), "{msg}");
        // 同一帧再画一次颜色不该变（哈希稳定，不是随机色）。
        let mut again = Frame::default();
        fx.draw(&rig.cx(2.0, 0.0), &mut again);
        assert_eq!(blocks(&frame), blocks(&again));
    }

    #[test]
    fn same_key_on_two_layers_stays_visible() {
        let rig = Rig::new(&[(8, 0, 0, 60), (8, 1, 7, 60), (8, 2, 3, 60)]).with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(12.0, 0.0), &mut frame);
        let got = blocks(&frame);
        assert_eq!(got.len(), 3);
        for (i, a) in got.iter().enumerate() {
            for b in &got[i + 1..] {
                assert!(
                    (a.center() - b.center()).length() > 2.0,
                    "同键同列的方块叠住了：{a:?} {b:?}"
                );
            }
        }
    }

    #[test]
    fn a_dense_section_is_capped_not_exploded() {
        // 16 列（= 1.6 秒）里塞 40 个声道，同时有 640 枚音符在响。
        let notes: Vec<(u32, u32, u8, u8)> = (760..820u32)
            .flat_map(|t| (0..40u32).map(move |l| (t, l, (t % 16) as u8, 21 + (t % 60) as u8)))
            .collect();
        let rig = Rig::new(&notes).with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(800.0, 0.0), &mut frame);
        assert_eq!(
            blocks(&frame).len(),
            MAX_BLOCKS,
            "超出上限要截到上限，不是无限堆"
        );
        let msg = fx.note().unwrap();
        assert!(
            msg.contains("只画了") || msg.contains("newest"),
            "截断要有一句说明：{msg}"
        );
    }

    #[test]
    fn without_the_piano_it_still_fits_a_row() {
        // 钢琴栏关掉：`cx.keyboard` 是 None，特效自己在下缘铺一条键盘当参照。
        let rig = Rig::new(&[(0, 0, 0, 0)]);
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(2.0, 0.0), &mut frame);
        assert_eq!(centers(&frame).len(), 1, "低音 A0 也得有方块");
        let got = centers(&frame)[0];
        assert!(got.y > 600.0 && got.y < 800.0, "{got:?} 要贴着画面下缘");
        assert_eq!(
            BlockRow::layout(&rig.cx(0.0, 0.0)).slots,
            crate::fx::keyboard::WHITE_SLOTS,
            "自己铺的那条键盘要放得下 88 键"
        );
    }

    #[test]
    fn params_round_trip_and_the_math_stays_in_range() {
        let fx = BlockRow::default();
        let value = fx.params();
        assert_eq!(value["nudge_px"].as_f64(), Some(3.0));
        assert_eq!(value["use_tiles"].as_bool(), Some(true));
        let mut back = BlockRow::default();
        back.set_params(&serde_json::json!({ "life_seconds": 2.5, "nudge_hz": 3.0 }));
        assert_eq!(back.params.life_seconds, 2.5);
        assert_eq!(back.params.nudge_hz, 3.0);
        assert_eq!(back.params.nudge_px, 3.0, "缺的字段走默认");
        let before = back.params();
        back.set_params(&serde_json::json!("nope"));
        assert_eq!(back.params(), before, "读不出来就不能把参数改掉");

        // 参数被拖成 0 也不能出界或除零。
        let (swing, alpha) = BlockRow::envelope(0.0, 0.0);
        assert!(swing >= 0.35 && swing <= 1.0 && alpha <= 1.0);
        let (swing, _) = BlockRow::envelope(99.0, 1.6);
        assert_eq!(swing, 0.35, "老方块只剩底噪那点动");
        let (dx, dy) = BlockRow::nudge(0.0, 3.0);
        assert_eq!(dx, 0.0);
        assert!(dy.abs() <= 3.0 * 0.4 + 0.01, "纵向幅度是横向的 0.4 倍");
    }

    #[test]
    fn it_declares_itself_as_an_overlay() {
        let info = BlockRow::default().info();
        assert_eq!(info.id, BlockRow::ID);
        assert_eq!(
            info.surface,
            Surface::Overlay,
            "方块排叠在界面上，不接管中央面板"
        );
        assert!(info.wants_piano, "方块压在键上方，得让钢琴栏跟着显示");
        // 舞台和叠加各排各的 z，方块排的 z 只在叠加层里比较有意义。
        assert!(info.z > 0);
    }
}
