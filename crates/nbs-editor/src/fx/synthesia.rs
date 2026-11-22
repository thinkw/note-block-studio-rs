//! 特效：钢琴瀑布（Synthesia 那种下落胶囊 + 击中处发光）。
//!
//! 它是**舞台**表面：接管中央面板，键盘由自己画（扁平款，不是底部那条精灵键盘），
//! 因为瀑布的轨道必须和这块键盘严格对齐 —— 借用编辑区那条键盘的话，网格、滚动条
//! 和琴键面板的高度关系一变，轨道就对不上。
//!
//! 时间一律走 [`super::timeline::FxTimeline`]：胶囊位置由「这枚音符还有几秒落到
//! 击中线」决定，所以 Tempo Changer 的变速自动跟上，这里不再算一遍速度。

use serde::{Deserialize, Serialize};

use egui::{Color32, Pos2, Rect, Ui, Vec2};

use super::keyboard::{is_white_key, KeyboardLayout, KEY_W, WHITE_HIT_H};
use super::stage::Frame;
use super::{EffectInfo, FxContext, PlaybackEffect, Surface};
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
    /// 键盘高度倍率：1.0 就是原版琴键那 128px。
    pub keyboard_scale: f32,
    /// 光晕向外扩散的像素。
    pub glow_px: f32,
    /// 画不画轨道分隔线。
    pub lanes: bool,
    /// 击中时冒不冒粒子。
    pub particles: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            fall_seconds: 2.4,
            keyboard_scale: 1.0,
            glow_px: 12.0,
            lanes: true,
            particles: true,
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
pub struct Synthesia {
    params: Params,
    particles: Vec<Particle>,
    /// 上一帧的播放头（tick）：靠它认出「这一帧跨过了音符」，也认出回绕/拖动。
    last_position: f64,
}

impl Default for Synthesia {
    fn default() -> Self {
        Self {
            params: Params::default(),
            particles: Vec::new(),
            last_position: 0.0,
        }
    }
}

impl Synthesia {
    pub const ID: &'static str = "synthesia";

    pub fn new_with(params: Params) -> Self {
        Self {
            params,
            ..Default::default()
        }
    }

    /// 舞台底部那条键盘的摆放：贴着舞台下缘，高度按参数缩放。
    pub fn keyboard(area: Rect, scale: f32, keyboard_scale: f32) -> KeyboardLayout {
        let height = WHITE_HIT_H * scale * keyboard_scale.max(0.2);
        let strip = Rect::from_min_size(
            Pos2::new(area.min.x, area.max.y - height),
            Vec2::new(area.width(), height),
        );
        // `fit` 把白键顶边放在 strip 顶 + 黑键上探的那段；这里要的正是「整块键盘
        // 贴在舞台下缘」，所以按它的原始语义排，顶边留出的下探区由面板底色吸收。
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

    /// 一枚音符击中时撒的那一束（角度按序号摊开：同一帧撒出来要对称，
    /// 纯随机看着像噪声）。
    fn spawn(&mut self, center_x: f32, hit_y: f32, velocity: u8, now: f64) {
        let accent = self.hit_color(velocity);
        for i in 0..PARTICLES_PER_HIT {
            let angle = std::f32::consts::PI * (0.12 + 0.76 * (i as f32 / PARTICLES_PER_HIT as f32));
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
            wants_piano: false,
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
            .add(
                egui::Slider::new(&mut p.keyboard_scale, 0.5..=2.0).text(tr("Keyboard height", "键盘高度")),
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
        changed
    }

    fn draw(&mut self, cx: &FxContext, out: &mut Frame) {
        let scale = cx.scale;
        let layout = Self::keyboard(cx.area, scale, self.params.keyboard_scale);
        let hit_y = layout.origin.y;
        let top_y = cx.area.min.y;

        if self.params.lanes {
            // 轨道线从键盘上缘拉到屏幕顶，间距就是白键宽。
            for a in 0..=layout.slots {
                let x = layout.slot_x(a);
                out.line(
                    Pos2::new(x, top_y),
                    Pos2::new(x, hit_y),
                    1.0 * scale,
                    Color32::from_white_alpha(14),
                );
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
        if cx.position + 0.5 < self.last_position {
            self.particles.clear();
        }
        if self.params.particles && cx.playing {
            let crossed = cx.timeline.notes_in(self.last_position.max(0.0), cx.position);
            let mut spawned = 0;
            for note in crossed {
                if layout.rect_of_key(note.key).is_some() && spawned < 40 {
                    let x = layout.lane_center(note.key).unwrap_or(cx.area.center().x);
                    self.spawn(x, hit_y, note.velocity, cx.now);
                    spawned += 1;
                }
            }
        }
        Self::step_particles(&mut self.particles, cx.dt * cx.speed, cx.now);
        self.last_position = cx.position;

        for note in cx.timeline.notes_in(from, to) {
            let Some(rect) = layout.rect_of_key(note.key) else {
                continue;
            };
            let ahead = (cx.timeline.seconds_at(note.tick as f64) as f64 - now_seconds) as f32;
            let center_y = Self::capsule_y(top_y, hit_y, ahead, self.params.fall_seconds);
            let capsule_w = rect.width() * 0.82;
            let capsule_h = layout.lane_width(note.key) * 1.15;
            let body = Rect::from_center_size(
                Pos2::new(rect.center().x, center_y),
                Vec2::new(capsule_w, capsule_h),
            );
            let color = capsule_color(cx.accent, note.key, note.velocity, ahead);
            out.rect(body, color, capsule_w / 2.0);
            if ahead <= 0.0 {
                // 刚落下的几枚：在击中线上发一束，并把对应琴键点亮。
                let fade = (1.0 + ahead / TAIL_SECONDS).clamp(0.0, 1.0);
                out.glow(body, self.params.glow_px * scale * fade, color);
                if let Some(key_rect) = layout.rect_of_key(note.key) {
                    out.glow(
                        key_rect,
                        self.params.glow_px * scale * 0.6 * fade,
                        Color32::from_white_alpha((140.0 * fade) as u8),
                    );
                }
            }
        }

        draw_keyboard(&layout, self.params, cx.accent, out);

        // 击中线：瀑布的「地面」。
        out.line(
            Pos2::new(cx.area.min.x, hit_y),
            Pos2::new(cx.area.max.x, hit_y),
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

fn capsule_color(accent: Color32, key: u8, velocity: u8, ahead: f32) -> Color32 {
    // 黑键道稍暗：原版 Synthesia 用左右手两色，这里改成黑白键两种明度，
    // 一眼能看出音符落在哪一族键上。
    let base = if is_white_key(key) {
        accent
    } else {
        accent.linear_multiply(0.72)
    };
    let v = 0.45 + 0.55 * (velocity.min(100) as f32 / 100.0);
    let fade = if ahead <= 0.0 {
        (1.0 + ahead / TAIL_SECONDS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), (255.0 * v * fade) as u8)
}

/// 舞台自己那条扁平键盘：白键亮、黑键暗，选中/击中靠瀑布的发光体现。
fn draw_keyboard(layout: &KeyboardLayout, params: Params, accent: Color32, out: &mut Frame) {
    let scale = layout.scale;
    for a in 0..layout.slots {
        let white = layout.white_rect(a);
        out.rect(white, Color32::from_gray(235), 0.0);
        out.outlined(
            white,
            Color32::TRANSPARENT,
            (1.0 * scale, Color32::from_gray(70)),
            0.0,
        );
        if let Some(black) = layout.black_rect(a) {
            out.rect(black, Color32::from_gray(26), 2.0 * scale);
        }
    }
    // 键盘下缘一条强调色边，把「地面」和键盘本体分开。
    let bottom = layout.origin.y + WHITE_HIT_H * scale * params.keyboard_scale;
    out.line(
        Pos2::new(layout.origin.x, bottom),
        Pos2::new(layout.origin.x + layout.slots as f32 * KEY_W * scale, bottom),
        2.0 * scale,
        accent,
    );
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::keyboard::{BLACK_LIFT, WHITE_SLOTS};
    use crate::fx::tiles::InstrumentBlocks;
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
    }

    impl Rig {
        fn new(notes: &[(u32, u8, u8)]) -> Self {
            Self {
                ctx: Context::default(),
                timeline: FxTimeline::bake(&song(notes), false),
                blocks: InstrumentBlocks::default(),
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
                area: stage(),
                keyboard: None,
                timeline: &self.timeline,
                blocks: &self.blocks,
                tiles: None,
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

    #[test]
    fn keyboard_sits_on_the_stage_floor_with_every_lane() {
        let layout = Synthesia::keyboard(stage(), 1.0, 1.0);
        assert_eq!(layout.slots, WHITE_SLOTS, "2200px 宽放得下整台键盘");
        assert_eq!(layout.origin.y, 800.0 - WHITE_HIT_H + BLACK_LIFT);
        // 88 枚键都得有下落道，否则低音区的音符凭空消失。
        for key in 0..=KEY_MAX {
            assert!(layout.rect_of_key(key).is_some(), "键 {key} 没有轨道");
        }
        // 键盘缩放只改高度，不改横向摆放；击中线就是键面上沿，键盘长高它跟着往上抬。
        let tall = Synthesia::keyboard(stage(), 1.0, 2.0);
        assert_eq!(tall.origin.x, layout.origin.x);
        assert!(tall.origin.y < layout.origin.y, "键盘变高 → 击中线往上抬");

        let narrow = Rect::from_min_size(Pos2::ZERO, Vec2::new(20.0 * KEY_W, 400.0));
        let layout = Synthesia::keyboard(narrow, 1.0, 1.0);
        assert_eq!(layout.slots, 20);
        assert!(layout.origin.x >= narrow.min.x);
        assert!(layout.origin.y + WHITE_HIT_H <= narrow.max.y + BLACK_LIFT);
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

        // 空曲子剩下的就是键盘 + 轨道 + 击中线那些静态图元。
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
        assert!(!info.wants_piano, "舞台自画键盘，不该再要底部琴键栏");
        assert!(!info.name.is_empty());
    }
}
