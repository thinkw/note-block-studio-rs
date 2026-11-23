//! 特效：钢琴瀑布（Synthesia 那种下落胶囊 + 击中处发光）。
//!
//! 它是**舞台**表面：接管中央面板，但键盘不自己画 —— 用底部那条真的琴键面板
//! （`wants_piano`），胶囊就落在真键面上，键位高亮盖在面板上（画布的裁剪框由
//! 调用方扩到琴键栏，见 `app::show`）。轨道因此和琴键栏同一套摆放，
//! 不会出现两条键盘或对不齐。
//!
//! 时间一律走 [`super::timeline::FxTimeline`]：胶囊位置由「这枚音符还有几秒落到
//! 击中线」决定，所以 Tempo Changer 的变速自动跟上，这里不再算一遍速度。

use serde::{Deserialize, Serialize};

use egui::{Color32, Pos2, Rect, Ui, Vec2};

use super::blockrow::BlockRow;
use super::keyboard::{is_white_key, KeyboardLayout, BLACK_LIFT, WHITE_HIT_H};
use super::stage::Frame;
use super::tiles::{instrument_color, tile_size};
use super::timeline::FxNote;
use super::{
    fade, is_light, note_block, EffectInfo, FxContext, PlaybackEffect, Surface, STAGE_BACKDROP,
};
use crate::tr;

/// 一屏最多留多少粒子。真曲子的密集段落一秒能进上百枚音符，每枚 6 粒，
/// 不设上限会在长颤音那里把帧率拖没。
pub const MAX_PARTICLES: usize = 1500;
/// 每枚音符击中时冒几粒。
const PARTICLES_PER_HIT: usize = 6;
/// 击中后胶囊在击中线以下残留的时长（秒）：不给这点余量，音符会在落线那一帧凭空消失。
pub const TAIL_SECONDS: f32 = 0.18;
/// 粒子寿命（秒）。
const PARTICLE_TTL: f32 = 0.55;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// 一枚音符从屏幕顶落到击中线要几秒（越大越慢）。
    pub fall_seconds: f32,
    /// 光晕向外扩散的像素。
    pub glow_px: f32,
    /// 画不画轨道分隔线。
    pub lanes: bool,
    /// 击中时冒不冒粒子。
    pub particles: bool,
    /// 胶囊按乐器上色（关掉就全用主题强调色，回到 Synthesia 那种单色瀑布）。
    pub color_by_instrument: bool,
    /// 轨道里摆乐器方块而不是胶囊：方块排那套画法搬进瀑布（同一条键道、同一个落点，
    /// 只是形状换成方块 + 贴图/乐器色）。
    pub blocks: bool,
    /// 舞台底色用黑色（`STAGE_BACKDROP`）；关掉就用主界面编辑区那块底色（浅色主题是白）。
    pub dark_background: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            fall_seconds: 2.4,
            glow_px: 12.0,
            lanes: true,
            particles: true,
            color_by_instrument: true,
            blocks: false,
            dark_background: false,
        }
    }
}

/// 一粒火花。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub pos: Pos2,
    pub vel: Vec2,
    pub born: f64,
    pub color: Color32,
}

/// 钢琴瀑布。
#[derive(Default)]
pub struct Synthesia {
    params: Params,
    particles: Vec<Particle>,
    /// 上一帧的播放头（tick）：靠它认出「这一帧跨过了音符」，也认出回绕/拖动。
    /// `None` = 这个特效还没画过一帧：中途开启时播放头可能已在半路，那一帧只
    /// 登记位置，不把之前「跨过」的音符一口气补撒出来。
    last_position: Option<f64>,
}

impl Synthesia {
    pub const ID: &'static str = "synthesia";

    pub fn new_with(params: Params) -> Self {
        Self {
            params,
            ..Default::default()
        }
    }

    /// 键盘几何的兜底摆放：只在「已经要到琴键面板、但面板还没画出来」的头一帧用
    /// （`piano_rect` 由面板每帧写回，第一帧是空的）。口径同 `panel_keyboard`——
    /// 白键面顶边压在舞台上缘往下 = 区域下缘，击中线就是瀑布的地面。
    fn fallback_keyboard(area: Rect, scale: f32) -> KeyboardLayout {
        let strip = Rect::from_min_size(
            Pos2::new(area.min.x, area.max.y - BLACK_LIFT * scale),
            Vec2::new(area.width(), WHITE_HIT_H * scale),
        );
        KeyboardLayout::fit(strip, scale, 27)
    }

    /// 粒子推进 + 清理。`dt` 是已按倍速折算的步长。
    pub fn step_particles(particles: &mut Vec<Particle>, dt: f32, now: f64) {
        for p in particles.iter_mut() {
            p.pos += p.vel * dt;
            // 轻微向上飘的火花靠重力落回来，比直线飞出去像粒子。
            p.vel.y += 220.0 * dt;
        }
        particles.retain(|p| now - p.born < PARTICLE_TTL as f64 && now >= p.born);
        let overflow = particles.len().saturating_sub(MAX_PARTICLES);
        if overflow > 0 {
            particles.drain(0..overflow);
        }
    }

    /// 一枚音符击中时撒的那一束。角度围绕正上方对称摊开（同一帧撒出来要对称，
    /// 纯随机看着像噪声），且两端等距：旧口径取 `0.12 + 0.76 × i/N`，六粒只摊到
    /// 0.753π，中点落在 0.437π —— 右半边的水平射程比左半边长一截，看起来
    /// 「左边少了一点」。端点含入的等分才绕着 0.5π 对称。
    fn spawn(&mut self, center_x: f32, hit_y: f32, velocity: u8, now: f64) {
        let accent = self.hit_color(velocity);
        for i in 0..PARTICLES_PER_HIT {
            let t = i as f32 / (PARTICLES_PER_HIT - 1) as f32 - 0.5;
            let angle = std::f32::consts::PI * (0.5 + 0.76 * t);
            let speed = 90.0 + 130.0 * (velocity.min(100) as f32 / 100.0);
            self.particles.push(Particle {
                pos: Pos2::new(center_x, hit_y),
                vel: Vec2::new(angle.cos() * speed * 0.7, -angle.sin() * speed),
                born: now,
                color: accent,
            });
        }
    }

    fn hit_color(&self, velocity: u8) -> Color32 {
        let v = 0.5 + 0.5 * (velocity.min(100) as f32 / 100.0);
        Color32::from_rgba_unmultiplied(255, 250, 190, (230.0 * v) as u8)
    }

    /// 胶囊底色：按乐器上色这一档取的是**主界面画这枚音符用的同一帧精灵的平均色**
    /// （[`FxContext::palette`]，帧号口径和 `grid.rs` 共用 [`crate::icons::block_frame`]），
    /// 素材没入库才退回按乐器名哈希。关掉则退回主题强调色。
    /// 原版 Synthesia 只分「左右手」两档，看不出这枚音是哪件乐器弹的；
    /// 自定义乐器在导出方块表里又全是同一块石头，所以这里按乐器上色。
    fn capsule_base(&self, cx: &FxContext, note: &FxNote) -> Color32 {
        if self.params.color_by_instrument {
            cx.palette.color(note.instrument).unwrap_or_else(|| {
                instrument_color(
                    cx.timeline.instrument_name(note.instrument),
                    note.instrument,
                )
            })
        } else {
            cx.accent
        }
    }

    /// 胶囊在这一帧的纵向中心。`ahead` = 距击中还剩几秒。
    pub fn capsule_y(top: f32, hit: f32, ahead: f32, fall_seconds: f32) -> f32 {
        let fall = fall_seconds.max(0.1);
        let t = 1.0 - (ahead / fall).clamp(0.0, 1.0);
        top + (hit - top) * t
    }
}

impl PlaybackEffect for Synthesia {
    fn info(&self) -> EffectInfo {
        EffectInfo {
            id: Self::ID,
            name: tr("Piano waterfall", "钢琴瀑布"),
            surface: Surface::Stage,
            z: 0,
            // 键盘改用底部那条真琴键栏：瀑布的轨道、击中高亮都按它的摆放算。
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
                egui::Slider::new(&mut p.fall_seconds, 0.6..=6.0)
                    .suffix(" s")
                    .text(tr("Fall time", "下落时间")),
            )
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut p.glow_px, 0.0..=40.0).text(tr("Glow spread", "光晕范围")))
            .changed();
        changed |= ui
            .checkbox(&mut p.lanes, tr("Lane guides", "轨道分隔线"))
            .changed();
        changed |= ui
            .checkbox(&mut p.particles, tr("Hit sparks", "击中粒子"))
            .changed();
        changed |= ui
            .checkbox(
                &mut p.color_by_instrument,
                tr("Color by instrument", "按乐器上色"),
            )
            .changed();
        changed |= ui
            .checkbox(&mut p.blocks, tr("Instrument blocks", "乐器方块"))
            .changed();
        changed |= ui
            .checkbox(&mut p.dark_background, tr("Black background", "黑色背景"))
            .changed();
        changed
    }

    /// 舞台底色这条口径也交给琴键面板用：勾了「黑色背景」面板一起黑，
    /// 没勾就跟编辑区同色 —— 面板上探的那 7px 不再露出窗口底色。
    fn backdrop(&self, grid_bg: Color32) -> Color32 {
        if self.params.dark_background {
            STAGE_BACKDROP
        } else {
            grid_bg
        }
    }

    fn draw(&mut self, cx: &FxContext, out: &mut Frame) {
        let scale = cx.scale;
        // 轨道按底部琴键面板的摆放下：面板那一帧的矩形由 `fx_input` 换算成
        // `KeyboardLayout` 传进来，`hit_y` 于是正好压在白键面沿上。
        let layout = cx
            .keyboard
            .unwrap_or_else(|| Self::fallback_keyboard(cx.area, scale));
        let hit_y = layout.origin.y;
        let top_y = cx.area.min.y;
        // 键盘剪影的顶：黑键比白键高出 BLACK_LIFT，击中线压在这条沿上才不会
        // 横穿黑键。胶囊仍落在白键面沿（`hit_y`），线只是「地面」的记号。
        let keyline_y = hit_y - BLACK_LIFT * scale;

        // 舞台底色：默认跟主界面编辑区同色（浅色主题就是那块白），勾上「黑色背景」
        // 才回到原来那块。参考线和键位高亮的明暗跟着它翻，否则白底上整条线看不见。
        let back = if self.params.dark_background {
            STAGE_BACKDROP
        } else {
            cx.backdrop
        };
        let light = is_light(back);
        out.rect(cx.area, back, 0.0);

        if self.params.lanes {
            // 轨道线从键盘上缘拉到屏幕顶，间距就是白键宽；下端同样停在键盘剪影
            // 的顶，不然会戳进黑键上探的那 7px。
            let guide = if light {
                Color32::from_black_alpha(20)
            } else {
                Color32::from_white_alpha(14)
            };
            for a in 0..=layout.slots {
                let x = layout.slot_x(a);
                out.line(Pos2::new(x, top_y), Pos2::new(x, keyline_y), 1.0 * scale, guide);
            }
        }

        let now_seconds = cx.timeline.seconds_at(cx.position) as f64;
        let from = cx
            .timeline
            .tick_at_seconds(now_seconds - TAIL_SECONDS as f64);
        let to = cx
            .timeline
            .tick_at_seconds(now_seconds + self.params.fall_seconds as f64);

        // 播放头倒退（拖动、循环回绕）时丢掉旧粒子，不然会在两处同时冒。
        // 击中判定用精确窗口（`notes_crossed`）：画胶囊那份 `notes_in` 两端放宽了
        // 一列，播放头在同一列里走的每一帧都会把那枚音符再当一次「刚跨过」，
        // 一帧补一束——连续击中时粒子翻几倍，把上限挤爆后更早的束会被中途抽走。
        if let Some(last) = self.last_position {
            if cx.position + 0.5 < last {
                self.particles.clear();
            }
            if self.params.particles && cx.playing {
                let crossed = cx.timeline.notes_crossed(last, cx.position);
                let mut spawned = 0;
                for note in crossed {
                    if layout.rect_of_key(note.key).is_some() && spawned < 40 {
                        let x = layout.lane_center(note.key).unwrap_or(cx.area.center().x);
                        self.spawn(x, hit_y, note.velocity, cx.now);
                        spawned += 1;
                    }
                }
            }
        }
        Self::step_particles(&mut self.particles, cx.dt * cx.speed, cx.now);
        self.last_position = Some(cx.position);

        for note in cx.timeline.notes_in(from, to) {
            let Some(rect) = layout.rect_of_key(note.key) else {
                continue;
            };
            let ahead = (cx.timeline.seconds_at(note.tick as f64) as f64 - now_seconds) as f32;
            let center_y = Self::capsule_y(top_y, hit_y, ahead, self.params.fall_seconds);
            let capsule_w = rect.width() * 0.82;
            let capsule_h = layout.lane_width(note.key) * 1.15;
            let center = Pos2::new(rect.center().x, center_y);
            let color = capsule_color(
                self.capsule_base(cx, note),
                note.key,
                note.velocity,
                ahead,
            );
            // 两种画法摆在同一条轨道、同一个落点：胶囊（默认），或把方块排那套乐器方块
            // 搬进来。方块边长照 `BlockRow::block_extent` 按本枚键道夹，和方块排一致，
            // 免得同一条道上两种特效给出两个宽度互相盖住邻道。
            let body = if self.params.blocks {
                let (side, _) = BlockRow::block_extent(tile_size(scale), layout.lane_width(note.key));
                Rect::from_center_size(center, Vec2::new(side, side))
            } else {
                Rect::from_center_size(center, Vec2::new(capsule_w, capsule_h))
            };
            if self.params.blocks {
                let blk = note_block(cx, note.instrument, true);
                let alpha = body_alpha(note.velocity, ahead);
                match blk.texture {
                    Some(tex) => {
                        out.image(body, tex, Color32::from_white_alpha((255.0 * alpha) as u8))
                    }
                    None => {
                        let tint = if self.params.color_by_instrument {
                            blk.color
                        } else {
                            cx.accent
                        };
                        out.rect(body, fade(tint, alpha), 2.0 * scale);
                        out.outlined(
                            body,
                            Color32::TRANSPARENT,
                            (1.0 * scale, fade(Color32::from_gray(20), alpha)),
                            2.0 * scale,
                        );
                    }
                }
            } else {
                out.rect(body, color, capsule_w / 2.0);
            }
            if ahead <= 0.0 {
                // 刚落下的几枚：在击中线上发一束，并把对应琴键点亮。
                let lit = (1.0 + ahead / TAIL_SECONDS).clamp(0.0, 1.0);
                out.glow(body, self.params.glow_px * scale * lit, color);
                if let Some(key_rect) = layout.rect_of_key(note.key) {
                    // 白底上白 alpha 等于没画，浅底改用这枚音自己的颜色点亮。
                    let key_glow = if light {
                        fade(color, lit)
                    } else {
                        Color32::from_white_alpha((140.0 * lit) as u8)
                    };
                    out.glow(key_rect, self.params.glow_px * scale * 0.6 * lit, key_glow);
                }
            }
        }

        // 击中线：瀑布的「地面」，压在键盘剪影的顶（面板上沿）——黑键就顶在这条线上。
        out.line(
            Pos2::new(cx.area.min.x, keyline_y),
            Pos2::new(cx.area.max.x, keyline_y),
            2.0 * scale,
            Color32::from_rgba_unmultiplied(cx.accent.r(), cx.accent.g(), cx.accent.b(), 210),
        );

        for p in &self.particles {
            let size = 3.0 * scale;
            out.rect(
                Rect::from_center_size(p.pos, Vec2::new(size, size)),
                p.color,
                size / 2.0,
            );
        }
    }
}

/// 力度 + 击中后淡出决定的不透明度：胶囊和「乐器方块」两种画法共用这一条，
/// 换形状不该换淡出速度。
fn body_alpha(velocity: u8, ahead: f32) -> f32 {
    let v = 0.45 + 0.55 * (velocity.min(100) as f32 / 100.0);
    let fade = if ahead <= 0.0 {
        (1.0 + ahead / TAIL_SECONDS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    v * fade
}

fn capsule_color(base: Color32, key: u8, velocity: u8, ahead: f32) -> Color32 {
    // 黑键道稍暗：底色（乐器色或强调色）之上再分黑白键两档明度，
    // 一眼能看出音符落在哪一族键上。
    let base = if is_white_key(key) {
        base
    } else {
        base.linear_multiply(0.72)
    };
    Color32::from_rgba_unmultiplied(
        base.r(),
        base.g(),
        base.b(),
        (255.0 * body_alpha(velocity, ahead)) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::keyboard::{KEY_W, WHITE_SLOTS};
    use crate::fx::stage::Primitive;
    use crate::fx::tiles::{InstrumentBlocks, NotePalette};
    use crate::fx::timeline::FxTimeline;
    use egui::Context;
    use nbs_core::model::{Note, Song, KEY_MAX};

    fn song(notes: &[(u32, u8, u8)]) -> Song {
        let mut s = Song::new();
        s.meta.tempo = 10.0;
        for (tick, instrument, key) in notes {
            s.set(*tick, 0, Note::new(*instrument, *key));
        }
        s.recalculate();
        s
    }

    /// 一台放得下整副键盘的舞台（52 × 39px 再加左右的居中余量）。
    fn stage() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(2200.0, 800.0))
    }

    struct Rig {
        ctx: Context,
        timeline: FxTimeline,
        blocks: InstrumentBlocks,
        /// 默认给空表（走按名哈希那条），要看「和主界面同色」的测试自己换掉。
        palette: NotePalette,
    }

    impl Rig {
        fn new(notes: &[(u32, u8, u8)]) -> Self {
            Self::with_palette(notes, NotePalette::default())
        }

        /// 用装机素材里那批方块精灵平均色当配色表：单测不碰磁盘，`NotePalette::open()`
        /// 只在专门那条测试里调。
        fn with_palette(notes: &[(u32, u8, u8)], palette: NotePalette) -> Self {
            Self {
                ctx: Context::default(),
                timeline: FxTimeline::bake(&song(notes), false),
                blocks: InstrumentBlocks::default(),
                palette,
            }
        }

        fn cx(&self, position: f64, now: f64, playing: bool) -> FxContext<'_> {
            FxContext {
                tex: &self.ctx,
                now,
                dt: 0.05,
                position,
                playing,
                speed: 1.0,
                scale: 1.0,
                accent: Color32::from_rgb(80, 160, 255),
                backdrop: Color32::WHITE,
                area: stage(),
                keyboard: None,
                timeline: &self.timeline,
                blocks: &self.blocks,
                tiles: None,
                palette: &self.palette,
            }
        }
    }

    /// 胶囊位置只由「还剩几秒落到击中线」决定：顶上 = `fall_seconds`，地面 = 0。
    #[test]
    fn capsule_maps_remaining_seconds_onto_the_stage() {
        assert_eq!(Synthesia::capsule_y(0.0, 600.0, 2.4, 2.4), 0.0);
        assert_eq!(Synthesia::capsule_y(0.0, 600.0, 1.2, 2.4), 300.0);
        assert_eq!(Synthesia::capsule_y(0.0, 600.0, 0.0, 2.4), 600.0);
        // 已经过去的音符停在击中线，不再往键盘里钻。
        assert_eq!(Synthesia::capsule_y(0.0, 600.0, -0.5, 2.4), 600.0);
        // 下落时间被拖成 0 也不能除出 inf。
        assert!(Synthesia::capsule_y(0.0, 600.0, 1.0, 0.0).is_finite());
    }

    /// 兜底摆放（琴键面板还没画出来的头一帧）：整副键盘都得有下落道，
    /// 击中线压在舞台下缘，也就是后面那条琴键栏的面沿。
    #[test]
    fn the_fallback_keyboard_lands_on_the_stage_floor() {
        let layout = Synthesia::fallback_keyboard(stage(), 1.0);
        assert_eq!(layout.slots, WHITE_SLOTS, "2200px 宽放得下整台键盘");
        assert_eq!(layout.origin.y, 800.0, "白键面顶边 = 舞台下缘");
        // 88 枚键都得有下落道，否则低音区的音符凭空消失。
        for key in 0..=KEY_MAX {
            assert!(layout.rect_of_key(key).is_some(), "键 {key} 没有轨道");
        }
        // 倍率只改横向尺寸，不改「贴下缘」这条。
        let small = Synthesia::fallback_keyboard(stage(), 0.5);
        assert_eq!(small.origin.y, layout.origin.y);
        assert_eq!(small.lane_width(0), layout.lane_width(0) * 0.5);

        let narrow = Rect::from_min_size(Pos2::ZERO, Vec2::new(20.0 * KEY_W, 400.0));
        let layout = Synthesia::fallback_keyboard(narrow, 1.0);
        assert_eq!(layout.slots, 20, "放不下多少白键就显示多少");
        assert!(layout.origin.x >= narrow.min.x);
    }

    /// 键盘改用底部那条真琴键栏：轨道、击中高亮、粒子都按面板传来的摆放下，
    /// 面板只显示一部分键时，栏外的键不再有轨道。
    #[test]
    fn capsules_follow_the_piano_panel_layout() {
        let rig = Rig::new(&[(0, 0, 45), (1, 0, 60)]);
        let panel = Rect::from_min_size(Pos2::new(100.0, 700.0), Vec2::new(20.0 * KEY_W, 128.0));
        let panel_layout = crate::fx::panel_keyboard(Some(panel), 1.0, 10, 20).unwrap();
        let mut cx = rig.cx(0.0, 0.0, false);
        cx.keyboard = Some(panel_layout);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&cx, &mut frame);

        // 击中线压在键盘剪影的顶（白键面沿 707 再上探 BLACK_LIFT = 700），不是舞台下缘 800。
        let hit = panel_layout.origin.y - BLACK_LIFT;
        assert!(
            frame.prims.iter().any(|prim| matches!(prim,
                Primitive::Line { from, to, .. } if (from.y - hit).abs() < 0.01 && (to.y - hit).abs() < 0.01)),
            "没有一条线压在黑键顶上（击中线 y 应为 {hit}）"
        );
        // 面板放的是白键槽 10..30，即键号 17..51：45 号在栏里，60 号不在，只剩一枚胶囊。
        let capsules = frame
            .prims
            .iter()
            .filter(|prim| matches!(prim, Primitive::Rect { rounding, .. } if *rounding > 0.0))
            .count();
        assert_eq!(capsules, 1, "琴键栏放不下的音符不该有轨道");
    }

    #[test]
    fn particles_fall_then_expire_and_stay_capped() {
        let mut fx = Synthesia::default();
        fx.spawn(100.0, 600.0, 100, 0.0);
        assert_eq!(fx.particles.len(), PARTICLES_PER_HIT);
        Synthesia::step_particles(&mut fx.particles, 0.1, 0.1);
        assert!(fx.particles[0].pos.y < 600.0, "先向上飘");
        Synthesia::step_particles(&mut fx.particles, 0.4, 0.5);
        assert!(fx.particles[0].vel.y > 0.0, "之后靠重力落回来");
        assert_eq!(fx.particles.len(), PARTICLES_PER_HIT, "还在寿命里就得留着");
        Synthesia::step_particles(&mut fx.particles, 0.0, PARTICLE_TTL as f64 + 0.01);
        assert!(fx.particles.is_empty(), "过寿的粒子必须清掉");

        let mut many: Vec<Particle> = (0..MAX_PARTICLES * 2)
            .map(|i| Particle {
                pos: Pos2::new(i as f32, 0.0),
                vel: Vec2::ZERO,
                born: 0.0,
                color: Color32::WHITE,
            })
            .collect();
        Synthesia::step_particles(&mut many, 0.0, 0.0);
        assert_eq!(many.len(), MAX_PARTICLES, "超上限截到上限，不是无限堆");
        // 截的是最旧的：长颤音的最新火花要留在屏幕上。
        assert_eq!(many.first().unwrap().pos.x, MAX_PARTICLES as f32);
    }

    #[test]
    fn crossing_a_note_sparks_and_rewinding_clears() {
        let rig = Rig::new(&[(3, 0, 60)]);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.0, 0.0, true), &mut frame);
        assert!(fx.particles.is_empty(), "没跨过音符不该有粒子");
        fx.draw(&rig.cx(4.0, 4.0, true), &mut frame);
        assert_eq!(fx.particles.len(), PARTICLES_PER_HIT, "跨过第 3 列该撒一束");
        fx.draw(&rig.cx(1.0, 5.0, true), &mut frame);
        assert!(fx.particles.is_empty(), "拖动/回绕要整批丢掉，否则两处同时冒");
        // 暂停时拖播放头不算「击中」。
        fx.draw(&rig.cx(0.0, 6.0, false), &mut frame);
        fx.draw(&rig.cx(4.0, 7.0, false), &mut frame);
        assert!(fx.particles.is_empty(), "没在播就不撒火花");
    }

    /// 同一枚音符只撒一次：击中判定用精确窗口（`from ≤ tick ≤ to`）。旧的
    /// `notes_in` 两端放宽一列，播放头在同一列里走的每帧都把那枚音符再报一次，
    /// 一帧补一束连撒六七帧 —— 连续击中时粒子翻几倍，1500 上限从队头抽走
    /// 更早的束（用户看到的「连续击中只播一部分」），40 束/帧的预算也被吃掉。
    #[test]
    fn a_note_sparks_once_no_matter_how_long_the_playhead_lingers() {
        let rig = Rig::new(&[(3, 0, 60)]);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.0, 0.0, true), &mut frame);
        fx.draw(&rig.cx(3.1, 0.1, true), &mut frame);
        assert_eq!(fx.particles.len(), PARTICLES_PER_HIT, "跨过第 3 列撒一束");
        // 播放头还在第 3 列里走（10 t/s 下一列要走好几帧）：不许再补。
        fx.draw(&rig.cx(3.5, 0.2, true), &mut frame);
        fx.draw(&rig.cx(3.9, 0.3, true), &mut frame);
        fx.draw(&rig.cx(4.3, 0.4, true), &mut frame);
        assert_eq!(
            fx.particles.len(),
            PARTICLES_PER_HIT,
            "同一列里走的后续帧不许再补"
        );
    }

    /// 中途开启特效的第一帧只登记播放头：不把之前「跨过」的音符补撒出来。
    /// 旧代码从 last=0 起算，播放到一半才开特效的话一开就把半首歌各撒一束。
    #[test]
    fn enabling_mid_song_only_registers_the_playhead() {
        let rig = Rig::new(&(0..40u32).map(|t| (t, 0, 60)).collect::<Vec<_>>());
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(150.0, 1.0, true), &mut frame);
        assert!(fx.particles.is_empty(), "第一帧只登记位置");
        fx.draw(&rig.cx(150.3, 1.2, true), &mut frame);
        assert!(fx.particles.is_empty(), "第 150 列没有音符");
    }

    /// 一束火花左右对称：角度绕正上方等分摊开，最左和最右的水平初速一样大。
    /// 旧口径的六粒摊在 0.12π..0.753π，中点 0.437π，右半边射程长一截。
    #[test]
    fn a_burst_spreads_symmetrically_around_up() {
        let mut fx = Synthesia::default();
        fx.spawn(0.0, 0.0, 100, 0.0);
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for p in &fx.particles {
            assert!(p.vel.y < 0.0, "出生时全部向上");
            if p.vel.x < 0.0 {
                left = left.min(p.vel.x);
            } else {
                right = right.max(p.vel.x);
            }
        }
        assert!(left < 0.0 && right > 0.0, "两边都得有粒子，不能挤在一边");
        assert!(
            (left + right).abs() < 1e-3,
            "最左 {left} 和最右 {right} 的水平射程要一样远"
        );
    }

    #[test]
    fn a_frame_is_bounded_and_only_covers_the_visible_window() {
        let rig = Rig::new(&(0..40u32).map(|t| (t, (t as u8) % 16, 30 + (t as u8) % 50)).collect::<Vec<_>>());
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(12.0, 1.2, false), &mut frame);
        assert!(!frame.is_empty());
        assert!(
            frame.len() < 600,
            "每帧图元数失控（{}）—— 说明窗口剔除没生效",
            frame.len()
        );

        // 空曲子剩下的只有轨道线和击中线（键盘由琴键面板画，不在这一帧里）。
        let quiet = Rig::new(&[]);
        let mut bare = Synthesia::default();
        let mut empty = Frame::default();
        bare.draw(&quiet.cx(12.0, 1.2, false), &mut empty);
        assert!(empty.len() < frame.len(), "有音符的帧该比空曲子多出胶囊");
        assert!(empty.len() < 220, "静态图元本身也得有界（{}）", empty.len());

        // 播放头落到曲尾之外：窗口只剩最后几枚，不会跟着曲子长度涨。
        let mut tail = Synthesia::default();
        let mut last = Frame::default();
        tail.draw(&rig.cx(10_000.0, 1_000.0, false), &mut last);
        assert!(
            last.len() < frame.len(),
            "曲尾之外只剩末尾几枚胶囊：{}/{}/{}",
            last.len(),
            frame.len(),
            empty.len()
        );
        assert!(last.len() < 260, "曲尾的帧也该只比静态多一点（{}）", last.len());
    }

    #[test]
    fn params_round_trip_through_json() {
        let fx = Synthesia::new_with(Params {
            fall_seconds: 3.5,
            lanes: false,
            ..Params::default()
        });
        let value = fx.params();
        assert_eq!(value["fall_seconds"].as_f64(), Some(3.5));
        assert_eq!(value["lanes"].as_bool(), Some(false));
        // 两条新档出厂都是「原来的样子」：方块样式关着、底色跟主题走。
        assert_eq!(value["blocks"].as_bool(), Some(false));
        assert_eq!(value["dark_background"].as_bool(), Some(false));
        let mut back = Synthesia::default();
        back.set_params(&value);
        assert_eq!(back.params(), fx.params());
        // 老存档缺字段由 `#[serde(default)]` 补，不该整个读不出来。
        back.set_params(&serde_json::json!({ "glow_px": 20.0 }));
        assert_eq!(back.params.glow_px, 20.0);
        assert_eq!(back.params.fall_seconds, Params::default().fall_seconds);
        // 完全不按形状的值不能把参数改成乱七八糟的东西，也不能把已设的改掉。
        let before = back.params();
        back.set_params(&serde_json::json!(42));
        assert_eq!(back.params(), before);
    }

    /// 第一条竖线就是轨道参考线（击中线是横的，粒子是小方块）。
    fn guide_color(frame: &Frame) -> Color32 {
        frame
            .prims
            .iter()
            .find_map(|p| match p {
                Primitive::Line { from, to, color, .. } if (from.x - to.x).abs() < 0.01 => {
                    Some(*color)
                }
                _ => None,
            })
            .expect("一条竖线都没有（轨道参考线没画）")
    }

    /// 底色默认跟主界面编辑区同色（浅色主题就是那块白），勾上「黑色背景」才回到原来
    /// 那块。参考线必须跟着翻：`Color32` 的白 alpha 铺在白底上等于没画。
    #[test]
    fn the_backdrop_follows_the_theme_and_the_guides_flip_with_it() {
        let rig = Rig::new(&[(3, 0, 60)]);
        let cx = rig.cx(0.0, 0.0, false);
        assert_eq!(cx.backdrop, Color32::WHITE, "Rig 摆的是浅色主题那块底");

        let mut fx = Synthesia::default();
        let mut light = Frame::default();
        fx.draw(&cx, &mut light);
        let back = match &light.prims[0] {
            Primitive::Rect {
                rect,
                fill,
                rounding,
                ..
            } => {
                assert_eq!(*rect, stage(), "底色要铺满整块舞台");
                assert_eq!(*rounding, 0.0);
                *fill
            }
            other => panic!("第一笔该是底色，实际是 {other:?}"),
        };
        assert_eq!(back, cx.backdrop, "默认就该是主界面那种白");
        assert_eq!(guide_color(&light).to_srgba_unmultiplied()[0], 0);

        let mut dark = Synthesia::default();
        dark.params.dark_background = true;
        let mut black = Frame::default();
        dark.draw(&cx, &mut black);
        let back = match &black.prims[0] {
            Primitive::Rect { fill, .. } => *fill,
            other => panic!("第一笔该是底色，实际是 {other:?}"),
        };
        assert_eq!(back, STAGE_BACKDROP, "勾了黑色背景才回到原来那块");
        // 反预乘会有 1 的舍入（白 alpha 14 解出来是 254），只看是不是那一族颜色。
        assert!(guide_color(&black).to_srgba_unmultiplied()[0] >= 250);
    }

    /// 「乐器方块」样式就是把方块排那套搬进轨道：形状是正方形、边长照同一口径夹键道，
    /// 颜色查的是同一张表（[`note_block`]），不是胶囊那条配色。
    #[test]
    fn the_block_style_drops_instrument_blocks_into_the_lanes() {
        let rig = Rig::new(&[(3, 0, 60), (4, 13, 62)]);
        let cx = rig.cx(0.0, 0.0, false);
        let mut fx = Synthesia::default();
        let mut soft = Frame::default();
        fx.draw(&cx, &mut soft);
        let capsules = soft
            .prims
            .iter()
            .filter(|p| {
                matches!(p, Primitive::Rect { rounding, rect, fill, .. }
                    if *fill != Color32::TRANSPARENT && (*rounding - rect.width() / 2.0).abs() < 0.01)
            })
            .count();
        assert_eq!(capsules, 2, "出厂还是两枚胶囊");

        fx.params.blocks = true;
        let mut frame = Frame::default();
        fx.draw(&cx, &mut frame);
        let blocks: Vec<(Rect, Color32)> = frame
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect {
                    rect,
                    fill,
                    rounding,
                    stroke,
                } if *fill != Color32::TRANSPARENT
                    && stroke.is_none()
                    && *rounding > 0.0
                    && *rounding < rect.width() / 3.0 =>
                {
                    Some((*rect, *fill))
                }
                _ => None,
            })
            .collect();
        assert_eq!(blocks.len(), 2, "两枚音符该摆两个方块：{}", blocks.len());
        // 边长按本枚键道夹，口径和方块排同一条（`BlockRow::block_extent`）：
        // 宽过道就会盖住邻道，那正是用户报过的「都叠在一起」。
        let layout = Synthesia::fallback_keyboard(stage(), cx.scale);
        for (note, (r, _)) in cx.timeline.notes().iter().zip(&blocks) {
            let (side, _) = BlockRow::block_extent(tile_size(cx.scale), layout.lane_width(note.key));
            assert!(
                (r.width() - side).abs() < 0.01 && (r.height() - side).abs() < 0.01,
                "方块该是 {side} 见方，实测 {r:?}"
            );
        }
        // 查表同源：方块样式用的色 = 方块排给同一件乐器算出的色。
        let want = note_block(&cx, cx.timeline.notes()[0].instrument, true).color;
        assert_eq!(blocks[0].1, want, "两条特效对同一件乐器必须同色");
    }

    /// 瀑布原本只按黑白键分两档，看不出这枚音是哪件乐器弹的：底色要跟着乐器走。
    #[test]
    fn capsules_are_tinted_by_instrument() {
        let rig = Rig::new(&[(3, 0, 60), (4, 13, 62), (5, 20, 64)]);
        let fx = Synthesia::default();
        let cx = rig.cx(0.0, 0.0, false);
        let notes = cx.timeline.notes();
        assert_eq!(notes.len(), 3);
        let harp = fx.capsule_base(&cx, &notes[0]);
        let chime = fx.capsule_base(&cx, &notes[1]);
        let custom = fx.capsule_base(&cx, &notes[2]);
        assert_ne!(harp, chime, "harp 和 chime 的胶囊该不同色");
        assert_ne!(harp, custom, "自定义乐器也要有自己的色");
        assert_eq!(harp, fx.capsule_base(&cx, &notes[0]), "同一件乐器每次同色");
        // 关掉「按乐器上色」回到主题强调色。
        let mut plain = Synthesia::default();
        plain.params.color_by_instrument = false;
        assert_eq!(plain.capsule_base(&cx, &notes[0]), cx.accent);
        assert_eq!(plain.capsule_base(&cx, &notes[1]), cx.accent);
    }

    /// 瀑布胶囊的底色必须和主界面这枚音符刷的那格精灵同一个色：两边都只经
    /// [`crate::icons::block_frame`] 取帧号，这里钉住「取到色就等于色板」。
    /// 素材没装全时跳过（和 `fx::tiles` 那条同一口径），退回按名哈希那一档。
    #[test]
    fn capsule_colour_comes_from_the_main_ui_sprites() {
        let palette = NotePalette::open();
        let rig = Rig::with_palette(&[(3, 0, 60), (4, 13, 62), (5, 20, 64)], palette);
        let fx = Synthesia::default();
        let cx = rig.cx(0.0, 0.0, false);
        let mut checked = 0;
        for note in cx.timeline.notes() {
            let Some(want) = cx.palette.color(note.instrument) else {
                continue;
            };
            assert_eq!(
                fx.capsule_base(&cx, note),
                want,
                "{} 号乐器的胶囊和主界面不同色",
                note.instrument
            );
            checked += 1;
        }
        if checked == 0 {
            eprintln!("assets/icons/block_color 读不出色板，本例只验了下面的兜底");
        }
        // 素材缺席时不是没色，而是退回按乐器名哈希 —— 和加色板之前完全一致。
        let bare = Rig::new(&[(3, 0, 60), (4, 13, 62)]);
        let bx = bare.cx(0.0, 0.0, false);
        for note in bx.timeline.notes() {
            assert_eq!(
                fx.capsule_base(&bx, note),
                instrument_color(bx.timeline.instrument_name(note.instrument), note.instrument)
            );
        }
    }

    #[test]
    fn black_keys_are_darker_and_velocity_sets_alpha() {
        let accent = Color32::from_rgb(200, 220, 240);
        let white = capsule_color(accent, 0, 100, 1.0);
        let black = capsule_color(accent, 1, 100, 1.0);
        assert!(white.r() > black.r(), "{white:?} vs {black:?}");
        // 弱音符更透明，密集的弱拍才不糊成一团。
        assert!(capsule_color(accent, 0, 20, 1.0).a() < white.a());
        // 击中之后按 TAIL 淡出到全透明。
        assert!(capsule_color(accent, 0, 100, -0.09).a() < white.a());
        assert_eq!(capsule_color(accent, 0, 100, -TAIL_SECONDS).a(), 0);
    }

    #[test]
    fn it_declares_itself_as_a_stage() {
        let info = Synthesia::default().info();
        assert_eq!(info.id, Synthesia::ID);
        assert_eq!(info.surface, Surface::Stage);
        assert!(info.wants_piano, "瀑布的轨道按底部琴键栏放下，得把面板顶出来");
        assert!(!info.name.is_empty());
    }

    /// 击中线不能横穿黑键：黑键比白键高出 BLACK_LIFT，线必须压在键盘剪影的顶，
    /// 白键面沿那条 y 上不再有任何横线。
    #[test]
    fn the_hit_line_tops_the_keyboard_silhouette() {
        let rig = Rig::new(&[]);
        let panel = Rect::from_min_size(Pos2::new(100.0, 700.0), Vec2::new(20.0 * KEY_W, 128.0));
        let layout = crate::fx::panel_keyboard(Some(panel), 1.0, 10, 20).unwrap();
        let mut cx = rig.cx(0.0, 0.0, false);
        cx.keyboard = Some(layout);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&cx, &mut frame);

        let top = layout.origin.y - BLACK_LIFT;
        let horizontals: Vec<f32> = frame
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Line { from, to, .. }
                    if (from.y - to.y).abs() < 0.01 && to.x - from.x > 100.0 =>
                {
                    Some(from.y)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            horizontals,
            vec![top],
            "击中线该压在黑键顶 {top}（只有这一条横线）"
        );
        assert_ne!(
            layout.origin.y,
            top,
            "测试摆放里白键顶和黑键顶必须是两个高度"
        );
    }

    /// 琴键面板的底色口径跟着「黑色背景」走：勾了就一起黑，没勾就跟编辑区同色。
    #[test]
    fn the_panel_backdrop_follows_the_dark_background_switch() {
        let grid_bg = Color32::from_rgb(249, 249, 249);
        let fx = Synthesia::default();
        assert_eq!(
            fx.backdrop(grid_bg),
            grid_bg,
            "默认跟编辑区同色，面板不另起一套"
        );
        let mut dark = Synthesia::default();
        dark.params.dark_background = true;
        assert_eq!(dark.backdrop(grid_bg), STAGE_BACKDROP);
    }
}
