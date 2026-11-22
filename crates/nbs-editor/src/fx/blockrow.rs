//! 特效：方块排（机体那种「屏幕下方一排乐器方块」）。
//!
//! 它是**叠加**表面：不接管中央面板，直接压在网格/钢琴栏之上，所以开着它还能照常
//! 编辑。每个还在响的音符在它**所按键的正上方**落一个方块，方块的样子就是那枚音符
//! 的乐器对应的方块（和导出 .nbt 用的同一张表），所以画面和机体里实际摆的方块对得上。
//! 一张密集的和弦会把同一条键道摞成一小塔（最新那枚在最下面、正对键道），
//! 不摞的话几十个音全盖在同一个像素上，看着就像只落了一块方块、也不随乐器变。
//! 导出表里没配方块的乐器（自定义乐器默认全是石头）改用按乐器名哈希出的颜色，
//! 让「现在响的是哪件乐器」在画面上看得见，并在设置区指一句去哪儿配。
//!
//! 「以初始位置为中心持续位移几个像素」这条要求位移必须是 `now` 的纯函数：
//! 每帧从「家」重新算，而不是「上帧位置 += 一点」，否则拖动播放头、或者切走再切回，
//! 方块会漂走或攒出一堆历史。`now` 用的是墙钟，活不活跃用的却是播放头 —— 前者决定
//! 摆动，后者决定哪些音符还在响，暂停时方块该安静地留在原处而不是继续换一批。

use serde::{Deserialize, Serialize};

use egui::{Color32, Pos2, Rect, Ui, Vec2};

use super::keyboard::{KeyboardLayout, WHITE_HIT_H};
use super::stage::Frame;
use super::tiles::tile_size;
use super::{fade, note_block, EffectInfo, FxContext, PlaybackEffect, Surface};
use crate::tr;
use nbs_core::model::KEY_MAX;

/// 一帧最多摆几个方块。密集段落一秒能进几十个音，超出的按「只留最新的」截。
pub const MAX_BLOCKS: usize = 240;

/// 同一枚键上一共叠几个方块封顶（出厂默认，可改见 [`Params::max_stack`]）：再多的音
/// 只留最新这一摞，不然塔会一路长出画面。
pub const MAX_STACK: usize = 12;

/// 方块边长占所在键道的比例。留的两成缝既是邻道之间的间隔，也是位移能摆的横向余量
/// （[`BlockRow::block_extent`] 据此夹住 `nudge` 的横分量）。
/// 用户报的「方块都叠在一起」就是方块宽过键道、位移又没有边界，两道以上的方块互相盖死。
pub const LANE_FILL: f32 = 0.8;

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
    /// 一条键道上最多摞几个方块（塔高上限）。密集颤音一屏能摞十几层，
    /// 想要「一眼只看到刚响过的那几枚」就往小调；调大只影响摞塔，不影响同时摆几枚。
    pub max_stack: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            life_seconds: 1.6,
            nudge_px: 3.0,
            nudge_hz: 1.5,
            tile_scale: 1.0,
            use_tiles: true,
            max_stack: MAX_STACK as u32,
        }
    }
}

/// 方块排。
#[derive(Default)]
pub struct BlockRow {
    params: Params,
    /// 上一帧有没有因为缺素材退成色块（用来提一句「记得扒贴图」）。
    tiles_missing: bool,
    /// 上一帧有没有因为乐器没配方块而改用乐器色（用来指一句「导出设置里能指定方块」）。
    unconfigured: bool,
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

    /// 方块的实际边长 + 键道里剩下的横向余量。
    ///
    /// 基准边长（[`tile_size`] × `tile_scale`）是像素，键道宽度是 `39 × scale`（黑键
    /// 更窄），两者不同源，所以基准值在键道窄的画面上会宽过道 —— 这里按本枚音符所在
    /// 键道夹一次。余量 `room` 就是位移的横向钳位：方块怎么摆都出不了自己那条道。
    pub fn block_extent(base: f32, lane: f32) -> (f32, f32) {
        let tile = base.min(lane * LANE_FILL);
        (tile, (lane - tile).max(0.0) * 0.5)
    }

    /// 同一枚键上叠着的第 `k` 个方块相对「家」的偏移：新的那枚正对键道中心，越早的
    /// 往上摞，层高一整格（`1.05 × tile`，只留一点缝免得看成一块）。
    /// 不摞的话一轮颤音全落在同一个像素上，看着就是一块方块，完全看不出「刚刚响过
    /// 这么多个音」；横着扇开又摆不下——一条键道只放得下一枚方块。
    pub fn stack_offset(k: usize, tile: f32) -> (f32, f32) {
        (0.0, -1.05 * k as f32 * tile)
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
        let mut cap = i64::from(p.max_stack);
        changed |= ui
            .add(
                egui::Slider::new(&mut cap, 1..=24)
                    .text(tr("Blocks per key", "每键摞上限")),
            )
            .changed();
        p.max_stack = cap.clamp(1, 24) as u32;
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
        } else if self.unconfigured {
            Some(
                tr(
                    "Some instruments have no block assigned in the schematic export, so they are shown in their own color.",
                    "有些乐器在导出表里还是默认的石头，这一帧改用各自的乐器色（导出设置 → 乐器方块 可以指定）。",
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
        let base_tile = tile_size(scale) * self.params.tile_scale.max(0.2);
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
        // 每道还剩几枚没摆：`live` 是按列排的正序，正着数下去序号一路减小，
        // 最新那枚拿到 0，正好压在键道正上方，更早的往上摞。
        let mut left = [0u16; KEY_MAX as usize + 1];
        for note in live {
            if let Some(n) = left.get_mut(note.key as usize) {
                *n += 1;
            }
        }
        let mut missing = false;
        let mut unconfigured = false;
        for note in live {
            let age = (now_seconds - cx.timeline.seconds_at(f64::from(note.tick)) as f64) as f32;
            if age < 0.0 || age > self.params.life_seconds {
                continue;
            }
            let Some(home) = layout.rect_of_key(note.key) else {
                // 这枚键本帧不在画面里（用户把钢琴栏拖到只露一半），方块跟着不画。
                continue;
            };
            // 边长和余量按本枚音符所在的键道算：黑键道比白键道窄，一刀切的像素边长
            // 会在窄道上盖住邻道，一整排就看成叠在一起。
            let (tile, room) = Self::block_extent(base_tile, layout.lane_width(note.key));
            // 「家」的底边压在键盘上缘之上，留出一点缝，别让方块盖住键面。
            let base_y = layout.origin.y - tile * 0.5 - 4.0 * scale;
            let stack = match left.get_mut(note.key as usize) {
                Some(n) => {
                    let v = n.saturating_sub(1);
                    *n = v;
                    v as usize
                }
                None => 0,
            };
            if stack >= self.params.max_stack.max(1) as usize {
                // 同一枚键上摞到顶了：再多只留最新那一摞，不然塔会一路长出画面。
                self.capped = true;
                continue;
            }
            let (swing, alpha) = Self::envelope(age, self.params.life_seconds);
            let phase = std::f32::consts::TAU * self.params.nudge_hz * cx.now as f32
                + note.key as f32 * PHASE_PER_SLOT;
            let (dx, dy) = Self::nudge(phase, amp * swing);
            let (ox, oy) = Self::stack_offset(stack, tile);
            // 横分量钳在键道余量里：方块可以晃，但出不了自己那条道。
            let center = Pos2::new(
                home.center().x + dx.clamp(-room, room) + ox,
                base_y + dy + oy,
            );
            let rect = Rect::from_center_size(center, Vec2::new(tile, tile));

            // 只有导出表里真配了方块的乐器才走贴图；没配过的（自定义乐器默认全是石头）
            // 用乐器色，否则一整排方块全同，看着就是「不随乐器变」。
            // 这一条和瀑布的「乐器方块」样式共用（[`super::note_block`]），两边必须同图同色。
            let blk = note_block(cx, note.instrument, self.params.use_tiles);
            unconfigured |= blk.unconfigured;
            match blk.texture {
                Some(tex) => out.image(rect, tex, Color32::from_white_alpha((255.0 * alpha) as u8)),
                None => {
                    missing |= blk.missing;
                    out.rect(rect, fade(blk.color, alpha), 2.0 * scale);
                    out.outlined(
                        rect,
                        Color32::TRANSPARENT,
                        (1.0 * scale, fade(Color32::from_gray(20), alpha)),
                        2.0 * scale,
                    );
                }
            }
        }
        self.tiles_missing = missing;
        self.unconfigured = unconfigured;
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::keyboard::KEY_W;
    use crate::fx::stage::Primitive;
    use crate::fx::tiles::{BlockTiles, InstrumentBlocks, NotePalette};
    use crate::fx::timeline::FxTimeline;
    use egui::Context;
    use nbs_core::model::{CustomInstrument, Layer, Note, Song};

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
        /// 方块排画的是方块贴图，不读音符配色表；空表摆在这里只为凑齐 `FxContext`。
        palette: NotePalette,
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
                palette: NotePalette::default(),
                layout: None,
            }
        }

        /// 挂了自定义乐器的曲子：20 号往后的名字得在曲子里存在，特效才拿得到名字。
        fn with_custom(notes: &[(u32, u32, u8, u8)], names: &[&str]) -> Self {
            let mut s = song(notes);
            for name in names {
                s.custom_instruments.push(CustomInstrument {
                    name: (*name).into(),
                    filename: String::new(),
                    key: 45,
                    press: false,
                });
            }
            let mut rig = Self::new(&[]);
            rig.timeline = FxTimeline::bake(&s, false);
            rig
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
                backdrop: crate::fx::STAGE_BACKDROP,
                area: Rect::from_min_size(Pos2::ZERO, Vec2::new(2200.0, 800.0)),
                keyboard: self.layout,
                timeline: &self.timeline,
                blocks: &self.blocks,
                tiles: Some(&self.tiles),
                palette: &self.palette,
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

    /// 色块那条路径里每个方块的颜色（描边那圈不算）。
    fn fills(frame: &Frame) -> Vec<Color32> {
        frame
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
            .collect()
    }

    /// 方块的「家」：那枚键的道中心，加上同键摞第 `stack` 个时让开的那点。
    /// 位移是围绕这个点算的，不是围绕键道中心 —— 否则同一列的方块会互相盖住。
    fn home(rig: &Rig, key: u8, stack: usize) -> f32 {
        let layout = rig.layout.unwrap();
        let base = tile_size(1.0) * Params::default().tile_scale;
        let (tile, _) = BlockRow::block_extent(base, layout.lane_width(key));
        let (ox, _) = BlockRow::stack_offset(stack, tile);
        layout.rect_of_key(key).unwrap().center().x + ox
    }

    /// 方块不许越到邻道：单音扫一整个摆动周期，横向始终待在自己那条键道里。
    /// 用户把「方块大小」拉到上限、「位移幅度」拉到上限时最容易挤过道，
    /// 一整排就会糊成「叠在一起」。
    #[test]
    fn a_block_stays_inside_its_lane() {
        for key in [48u8, 49, 52, 60, 61, 72] {
            let rig = Rig::new(&[(0, 0, 0, key)]).with_keyboard();
            let lane = rig.layout.unwrap().rect_of_key(key).unwrap();
            let mut fx = BlockRow::default();
            fx.set_params(&serde_json::json!({ "nudge_px": 14.0, "tile_scale": 2.5 }));
            for i in 0..32u32 {
                let mut frame = Frame::default();
                fx.draw(&rig.cx(0.5, i as f64 * 0.02), &mut frame);
                for rect in blocks(&frame) {
                    assert!(
                        rect.left() >= lane.left() - 0.01 && rect.right() <= lane.right() + 0.01,
                        "键 {key} 的方块 {rect:?} 越出了键道 {lane:?}"
                    );
                }
                assert!(!blocks(&frame).is_empty(), "键 {key} 一帧都没画着");
            }
        }
    }

    /// 摞起来的方块一层压一整格，不许在同一列上互相盖死。
    #[test]
    fn stacked_blocks_do_not_cover_each_other() {
        let notes: Vec<(u32, u32, u8, u8)> = (0..6u32).map(|t| (t, 0, 0, 60)).collect();
        let rig = Rig::new(&notes).with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(5.5, 0.0), &mut frame);
        let got = blocks(&frame);
        assert_eq!(got.len(), 6);
        for a in &got {
            for b in &got {
                if a.center().x == b.center().x && a != b {
                    assert!(
                        !a.intersects(*b),
                        "同一条键道上摞的两块互相盖住了：{a:?} {b:?}"
                    );
                }
            }
        }
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
        let fills = fills(&frame);
        assert_eq!(fills.len(), 2, "两个乐器方块");
        assert_ne!(fills[0], fills[1], "piano 和 bass 的方块要看得出区别");
        let msg = fx.note().unwrap();
        assert!(msg.contains("贴图") || msg.contains("textures"), "{msg}");
        // 同一帧再画一次颜色不该变（哈希稳定，不是随机色）。
        let mut again = Frame::default();
        fx.draw(&rig.cx(2.0, 0.0), &mut again);
        assert_eq!(blocks(&frame), blocks(&again));
    }

    /// 自定义乐器在导出方块表里全是默认的石头：光看方块分不出乐器，
    /// 得改用按乐器名的颜色，并指一句去哪儿配。
    #[test]
    fn instruments_without_a_block_get_their_own_color() {
        let rig = Rig::with_custom(&[(0, 0, 20, 48), (0, 1, 21, 60)], &["S080", "VGE"])
            .with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(2.0, 0.0), &mut frame);
        let fills = fills(&frame);
        assert_eq!(fills.len(), 2, "两件自定义乐器各一个方块");
        assert_ne!(fills[0], fills[1], "S080 和 VGE 都是石头，靠颜色才分得开");
        let msg = fx.note().unwrap();
        assert!(
            msg.contains("方块") || msg.contains("block"),
            "要指一句「导出设置里能配方块」：{msg}"
        );
        // 内置乐器配好了方块，不该被拖进这条说明里。
        let classic = Rig::new(&[(0, 0, 0, 48)]).with_keyboard();
        let mut other = BlockRow::default();
        let mut one = Frame::default();
        other.draw(&classic.cx(2.0, 0.0), &mut one);
        let classic_msg = other.note().unwrap_or_default();
        assert!(
            classic_msg.contains("贴图") || classic_msg.contains("textures"),
            "harp 有真方块，缺素材时只该提贴图：{classic_msg}"
        );
    }

    /// 同一条键道上连着几十个音：不能全盖在同一个像素上，看着像只落了一块方块。
    #[test]
    fn a_trill_on_one_key_spreads_upward() {
        let notes: Vec<(u32, u32, u8, u8)> = (0..20u32).map(|t| (t, 0, 0, 60)).collect();
        let rig = Rig::new(&notes).with_keyboard();
        let mut fx = BlockRow::default();
        let mut frame = Frame::default();
        // 10 t/s、寿命 1.6 s ⇒ 窗口 16 列；播放头 15.5 ⇒ 15 枚还活着。
        fx.draw(&rig.cx(15.5, 0.0), &mut frame);
        let got = blocks(&frame);
        assert!(got.len() >= 12, "摞到 {MAX_STACK} 封顶，剩下的算截断：{}", got.len());
        for (i, a) in got.iter().enumerate() {
            for b in &got[i + 1..] {
                assert!(
                    (a.center() - b.center()).length() > 2.0,
                    "同一条键道叠住了：{a:?} {b:?}"
                );
            }
        }
        // 最新那枚（第 15 列）压在键道正上方，只带着那点摆动。
        let want = home(&rig, 60, 0);
        assert!(
            got.iter()
                .any(|r| (r.center().x - want).abs() <= Params::default().nudge_px + 0.01),
            "总有一个方块正对键道 {want}：{:?}",
            got.iter().map(|r| r.center().x).collect::<Vec<_>>()
        );
    }

    /// 摞塔上限是个设置：往小调就只留刚响过的那几枚，多出来的算截断并说一句。
    #[test]
    fn the_stack_cap_is_a_setting() {
        let notes: Vec<(u32, u32, u8, u8)> = (0..20u32).map(|t| (t, 0, 0, 60)).collect();
        let rig = Rig::new(&notes).with_keyboard();
        let mut fx = BlockRow::default();
        fx.params.max_stack = 4;
        let mut frame = Frame::default();
        fx.draw(&rig.cx(15.5, 0.0), &mut frame);
        let got = blocks(&frame);
        assert_eq!(got.len(), 4, "上限 4 就该只摆最新 4 枚：{:?}", got.len());
        assert!(
            fx.note()
                .map(|m| m.contains("只画了") || m.contains("newest"))
                .unwrap_or(false),
            "摞到顶截断要有一句说明：{:?}",
            fx.note()
        );
        // 上限调到比窗口还大就等于不截断，塔跟着音数长。
        fx.params.max_stack = 24;
        let mut wide = Frame::default();
        fx.draw(&rig.cx(15.5, 0.0), &mut wide);
        assert_eq!(blocks(&wide).len(), 16, "1.6 秒窗口里 16 列全该摆出来");
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
        let got = blocks(&frame).len();
        assert!(got < 640, "640 枚在响不可能全摆出来：{got}");
        assert!(got <= MAX_BLOCKS, "全局上限 {MAX_BLOCKS}，实测 {got}");
        // 截到最新 240 枚后只剩 6 条键道，每道再按 `MAX_STACK` 摞封顶。
        assert_eq!(got, 6 * MAX_STACK, "每道摞到顶就停，塔不能长过画面");
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
        assert_eq!(
            value["max_stack"].as_u64(),
            Some(MAX_STACK as u64),
            "出厂摞塔上限就是原来写死那个数"
        );
        let mut back = BlockRow::default();
        back.set_params(&serde_json::json!({ "life_seconds": 2.5, "nudge_hz": 3.0 }));
        assert_eq!(back.params.life_seconds, 2.5);
        assert_eq!(back.params.nudge_hz, 3.0);
        assert_eq!(back.params.nudge_px, 3.0, "缺的字段走默认");
        back.set_params(&serde_json::json!({ "max_stack": 4 }));
        assert_eq!(back.params.max_stack, 4);
        let before = back.params();
        back.set_params(&serde_json::json!("nope"));
        assert_eq!(back.params(), before, "读不出来就不能把参数改掉");

        // 参数被拖成 0 也不能出界或除零。
        let (swing, alpha) = BlockRow::envelope(0.0, 0.0);
        assert!((0.35..=1.0).contains(&swing) && alpha <= 1.0);
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
