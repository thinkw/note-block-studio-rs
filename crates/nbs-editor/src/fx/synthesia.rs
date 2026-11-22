//! 特效：钢琴瀑布（Synthesia 那种下落胶囊 + 击中处发光）。
//!
//! 它是**舞台**表面：接管中央面板，但键盘不自己画 —— 用底部那条真的琴键面板
//! （`wants_piano`），胶囊就落在真键面上，键位高亮盖在面板上（画布的裁剪框由
//! 调用方扩到琴键栏，见 `app::show`）。轨道因此和琴键栏同一套摆放，
//! 不会出现两条键盘或对不齐。
//!
//! 时间一律走 [`super::timeline::FxTimeline`]：胶囊的上下缘由「这枚音符还有几秒
//! 落到击中线」决定，所以 Tempo Changer 的变速自动跟上，这里不再算一遍速度。
//!
//! 数值口径抄的是 Neothesia（GPLv3，只借规则和常量，没有搬它一行代码）：
//! 胶囊高度 = 时值、宽 = 道宽 −1px、圆角 = 道宽 ×0.2、竖线只在 C 与 F 两处且双色、
//! 按下的键整面填色、发光按墙钟呼吸而 α 恒定、时值算不出时垫 0.1 秒并留 0.01 秒连打缝。

use serde::{Deserialize, Serialize};

use egui::{Color32, Pos2, Rect, Ui, Vec2};

use super::keyboard::{is_white_key, KeyboardLayout, BLACK_LIFT, WHITE_HIT_H};
use super::stage::Frame;
use super::tiles::instrument_color;
use super::timeline::FxNote;
use super::{EffectInfo, FxContext, PlaybackEffect, Surface};
use crate::tr;

/// 一屏最多留多少粒子。真曲子的密集段落一秒能进上百枚音符，每枚 6 粒，
/// 不设上限会在长颤音那里把帧率拖没。
pub const MAX_PARTICLES: usize = 1500;
/// 每枚音符击中时冒几粒。
const PARTICLES_PER_HIT: usize = 6;
/// 粒子寿命（秒）。
const PARTICLE_TTL: f32 = 0.55;
/// 最短时值（秒）。NBS 一列在 20 t/s 时只有 0.05 秒，不垫一脚，密集的十六分音符
/// 会塌成看不见的缝。抄上游 `waterfall/mod.rs` 的 `h.max(0.1)`。
pub const MIN_DURATION: f32 = 0.1;
/// 同音连打之间留的缝（秒），上游那句 `h - 0.01`：不留就看着像一记长音。
pub const SUCCESSIVE_GAP: f32 = 0.01;
/// 胶囊圆角占道宽的比例（上游 `radius: key.width() * 0.2`）。
pub const CAPSULE_ROUNDING: f32 = 0.2;
/// 「按声道」那档的 5 色，数值抄上游 `config/model.rs` 的默认调色板。
/// 每个声道取 (亮, 暗) 一对：白键道用亮档，黑键道用暗档（上游的 `.dark`）。
const LAYER_COLORS: [([u8; 3], [u8; 3]); 5] = [
    ([210, 89, 222], [125, 69, 134]),
    ([93, 188, 255], [48, 124, 255]),
    ([255, 126, 51], [192, 73, 0]),
    ([51, 255, 102], [0, 168, 2]),
    ([255, 51, 129], [192, 0, 77]),
];
/// 八度参考线画在哪两个音级：键号以 A0 为 0（见 `keyboard.rs`），所以 C 是 3、F 是 8。
/// 上游按 MIDI 音号取 0 和 5，同一个意思。
const GUIDE_C: u8 = 3;
const GUIDE_F: u8 = 8;
/// 两条参考线的灰度：C 亮、F 暗。52 条等权线在密集段会糊成一张网，反而看不出八度起点。
const GUIDE_C_GRAY: u8 = 51;
const GUIDE_F_GRAY: u8 = 13;
/// 小节的粗横线越粗越淡（上游 `intensity = 0.05 / 线宽`），倍率大时不会画出一道白墙。
const BAR_BASE_GRAY: f32 = 13.0;
/// 发光的 α 恒定 0.2（上游 `glow/renderer.rs`）：呼吸靠尺寸和提白，不靠淡出。
const GLOW_ALPHA: u8 = 51;
/// 光晕尺寸的呼吸幅度（上游 150 ± 10px ⇒ ±7%）。
const GLOW_BREATH: f32 = 0.07;

/// 瀑布上音符的上色来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    /// 按乐器：取主界面画那枚音符用的方块精灵平均色 —— 编辑区什么色，这里就什么色。
    /// 素材没入库时退回按乐器名哈希（`tiles::instrument_color`）。
    Instrument,
    /// 按声道：一层一色，用 [`LAYER_COLORS`] 那 5 档。
    Layer,
    /// 单色：主题强调色，Synthesia 原版那种瀑布。
    Accent,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// 一枚音符从屏幕顶落到击中线要几秒（越大越慢）。
    pub fall_seconds: f32,
    /// 光晕向外扩散的像素。
    pub glow_px: f32,
    /// 八度参考线（只在 C 和 F 两处竖线）。
    pub lanes: bool,
    /// 小节横线。
    pub bar_lines: bool,
    /// 按住的键整面填上胶囊色（关掉只剩发光那一层）。
    pub key_fill: bool,
    /// 击中时冒不冒粒子。
    pub particles: bool,
    /// 上色来源。
    pub color_mode: ColorMode,
    /// 延迟补偿（秒）：听到的比画面慢就把数值调大，让胶囊晚一点落线。
    pub animation_offset: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            fall_seconds: 2.4,
            glow_px: 12.0,
            lanes: true,
            bar_lines: true,
            key_fill: true,
            particles: true,
            color_mode: ColorMode::Instrument,
            animation_offset: 0.0,
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

    /// 胶囊底色，三条上色路都从这里出；按住的键面填色也用同一个返回值 ——
    /// 「落在哪条道」和「哪个键亮」必须同色。黑键那档深浅：只有乐器精灵色或强调色
    /// 这一支颜色时，深浅是黑白键唯一的区分手段；「按声道」那档不用它，
    /// 上游本来就配了明暗两套。
    fn capsule_base(&self, cx: &FxContext, note: &FxNote) -> Color32 {
        let dark = !is_white_key(note.key);
        match self.params.color_mode {
            ColorMode::Accent => shade(cx.accent, dark),
            ColorMode::Layer => {
                let (light, deep) = LAYER_COLORS[note.layer as usize % LAYER_COLORS.len()];
                if dark {
                    rgb(deep)
                } else {
                    rgb(light)
                }
            }
            ColorMode::Instrument => shade(
                cx.palette.color(note.instrument).unwrap_or_else(|| {
                    instrument_color(cx.timeline.instrument_name(note.instrument), note.instrument)
                }),
                dark,
            ),
        }
    }

    /// 某一时刻在这一帧落在屏幕的哪一行。`ahead` = 距击中还剩几秒：
    /// 顶上 = `fall_seconds`，地面（击中线）= 0；越界都夹住 —— 已经落过去的
    /// 不再往键盘里钻，还没进屏的留在顶外。
    pub fn capsule_y(top: f32, hit: f32, ahead: f32, fall_seconds: f32) -> f32 {
        let fall = fall_seconds.max(0.1);
        let t = 1.0 - (ahead / fall).clamp(0.0, 1.0);
        top + (hit - top) * t
    }

    /// 画出来的时长（秒）：先垫到 `MIN_DURATION`，再留出连打缝。
    /// NBS 的时值是烘表时从「同层同键下一枚」推出来的，推不出就是 1 列。
    pub fn drawn_seconds(duration: f32) -> f32 {
        (duration.max(MIN_DURATION) - SUCCESSIVE_GAP).max(0.005)
    }

    /// 胶囊在这一帧占的纵向区间（上缘, 下缘）。落过去之后下缘钉在击中线，
    /// 上缘继续往下走 —— 胶囊被线「吃掉」，比整条淡出更像按住再松手。
    pub fn capsule_span(top: f32, hit: f32, ahead: f32, dur: f32, fall_seconds: f32) -> (f32, f32) {
        (
            Self::capsule_y(top, hit, ahead + dur, fall_seconds),
            Self::capsule_y(top, hit, ahead, fall_seconds),
        )
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
        // 首选项里存的是旧字段 `color_by_instrument`（那时只有「按乐器 / 单色」两档），
        // 新字段带 `#[serde(default)]` 会被补成默认值「按乐器」——直接信它就把
        // 用户当初关掉的单色悄悄改回去了。只有旧键真写着 false 才认成「单色」，
        // 旧键 absent（新写的档色是三态）时不动 `color_mode`。
        let legacy_only_accent = value.get("color_by_instrument").is_some_and(|v| {
            v.as_bool() == Some(false) && value.get("color_mode").is_none()
        });
        if legacy_only_accent {
            self.params.color_mode = ColorMode::Accent;
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
                egui::Slider::new(&mut p.animation_offset, -0.2..=0.2)
                    .suffix(" s")
                    .text(tr("Latency offset", "延迟补偿")),
            )
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut p.glow_px, 0.0..=40.0).text(tr("Glow spread", "光晕范围")))
            .changed();
        ui.horizontal(|ui| {
            ui.label(tr("Note color", "胶囊上色"));
            changed |= ui
                .radio_value(
                    &mut p.color_mode,
                    ColorMode::Instrument,
                    tr("By instrument", "按乐器"),
                )
                .changed();
            changed |= ui
                .radio_value(&mut p.color_mode, ColorMode::Layer, tr("By track", "按声道"))
                .changed();
            changed |= ui
                .radio_value(&mut p.color_mode, ColorMode::Accent, tr("Single", "单色"))
                .changed();
        });
        changed |= ui
            .checkbox(&mut p.lanes, tr("Octave guides", "八度参考线"))
            .changed();
        changed |= ui
            .checkbox(&mut p.bar_lines, tr("Bar lines", "小节线"))
            .changed();
        changed |= ui
            .checkbox(&mut p.key_fill, tr("Fill pressed key", "按下的键填色"))
            .changed();
        changed |= ui
            .checkbox(&mut p.particles, tr("Hit sparks", "击中粒子"))
            .changed();
        changed
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

        // 整像素吸附：1px 的线压在半个像素上会散成两条淡线，一动就闪。
        let ppp = cx.tex.pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;

        if self.params.lanes {
            // 八度参考线只在 C 和 F 起笔，一根亮一根暗。等权画满 52 条的话，
            // 密集段会糊成一张网，反而看不出八度从哪儿起。
            for a in 0..layout.slots {
                let Some(key) = layout.slot_white_key(a) else {
                    continue;
                };
                let gray = match key % 12 {
                    GUIDE_C => GUIDE_C_GRAY,
                    GUIDE_F => GUIDE_F_GRAY,
                    _ => continue,
                };
                let x = snap(layout.slot_x(a));
                out.line(
                    Pos2::new(x, top_y),
                    Pos2::new(x, hit_y),
                    scale.max(1.0),
                    Color32::from_gray(gray),
                );
            }
        }

        let fall = self.params.fall_seconds.max(0.1);
        // 延迟补偿的语义是「让画面晚一点落线」，所以从播放头的秒数里减掉：
        // 同一时刻对应的胶囊整体退回上方，声音追上来。
        let now_seconds =
            cx.timeline.seconds_at(cx.position) as f64 - self.params.animation_offset as f64;

        // 可见窗口：上缘是落进屏幕前的最后一秒；下缘按烘表里最长那枚音往前多取，
        // 否则长音的尾还没落到底、整条胶囊就先消失了。
        let from = cx
            .timeline
            .tick_at_seconds(now_seconds - cx.timeline.max_span_seconds() as f64);
        let to = cx
            .timeline
            .tick_at_seconds(now_seconds + fall as f64);

        if self.params.bar_lines {
            // 小节横线：一小节一条，从上方落下来。从「播放头的下一小节」起算，
            // 已经过掉的小节不会在线上留残影。
            let per_bar = cx.timeline.ticks_per_bar().max(1) as f64;
            let first = (cx.timeline.tick_at_seconds(now_seconds) / per_bar).floor() as i64 + 1;
            let width = (2.0 * scale).max(1.0);
            // 越粗越淡（上游 `intensity = 0.05 / 线宽`），倍率大时才不是一道白墙。
            let gray = (BAR_BASE_GRAY / width).clamp(4.0, BAR_BASE_GRAY) as u8;
            // 按列（而不是按秒）走到窗口上缘就停：`seconds_at` 会把越出表尾的 tick
            // 夹在最后一秒，曲子快放完时按秒判断会留下一叠重叠在同一条线上的横线。
            // `to` 是 `seconds_at` 的反函数，本来就不会超过表尾。
            for bar in first.max(0)..first + 64 {
                let bar_tick = cx.timeline.tick_of_bar(bar as u32) as f64;
                if bar_tick > to {
                    break;
                }
                let ahead = (cx.timeline.seconds_at(bar_tick) as f64 - now_seconds) as f32;
                let y = snap(Self::capsule_y(top_y, hit_y, ahead, fall));
                out.line(
                    Pos2::new(cx.area.min.x, y),
                    Pos2::new(cx.area.max.x, y),
                    width,
                    Color32::from_gray(gray),
                );
            }
        }


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
            let start = cx.timeline.seconds_at(note.tick as f64);
            let ahead = (start as f64 - now_seconds) as f32;
            let held_seconds = cx.timeline.seconds_at((note.tick + note.duration) as f64);
            let dur = Self::drawn_seconds(held_seconds - start);
            let (up, bottom) = Self::capsule_span(top_y, hit_y, ahead, dur, fall);
            let base = self.capsule_base(cx, note);
            // ⑤ 宽 = 道宽 −1px、圆角 = 道宽 ×0.2（上游那两个数），① 高 = 时值。
            let body_w = (rect.width() - 1.0 * scale).max(1.0);
            let body = Rect::from_min_size(
                Pos2::new(rect.center().x - body_w / 2.0, up),
                Vec2::new(body_w, (bottom - up).max(0.0)),
            );
            if body.height() > 0.5 {
                out.rect(
                    body,
                    capsule_color(base, note.velocity),
                    rect.width() * CAPSULE_ROUNDING,
                );
            }
            // 还在响（起手已过、尾还没到）＝ 这一键正被按住。
            if ahead > 0.0 || ahead + dur <= 0.0 {
                continue;
            }
            if self.params.key_fill {
                // ③ 按住的键不是描一圈光，是整面填上胶囊那档色（上游 key_state.rs）。
                // 直角、往里收 1px：面板画的黑白键本是方角，压着边填会糊到邻键。
                out.rect(
                    rect.shrink(1.0 * scale),
                    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 235),
                    0.0,
                );
            }
            // ⑥ 发光按墙钟呼吸：α 恒定，尺寸 ±7%、亮度随 |cos| 提白。
            // 相位按键号岔开，整片键盘才不是一起亮一起暗。
            let phase = cx.now as f32 * 5.0 + note.key as f32 * 0.37;
            let spread = self.params.glow_px * scale * (1.0 + GLOW_BREATH * phase.sin());
            out.glow(rect, spread.max(0.0), glow_color(base, phase.cos().abs()));
        }

        // 击中线：瀑布的「地面」，压在琴键面板的上沿。
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

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// 黑键那一档压暗 0.72：只有一支底色（乐器精灵色或强调色）时，深浅是黑白键唯一的
/// 区分手段。「按声道」那档用不着它，上游本来就配了明暗两套色。
fn shade(base: Color32, dark: bool) -> Color32 {
    if dark {
        base.linear_multiply(0.72)
    } else {
        base
    }
}

/// 胶囊的最终色：底色不动，不透明度由力度决定 —— 密集的弱拍才不糊成一团。
/// 落下之后不再靠 α 淡出，收尾改由「胶囊被击中线吃掉」表达。
fn capsule_color(base: Color32, velocity: u8) -> Color32 {
    let v = 0.45 + 0.55 * (velocity.min(100) as f32 / 100.0);
    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), (255.0 * v) as u8)
}

/// 发光的颜色：α 恒定 [`GLOW_ALPHA`]（上游 0.2），提白由 `lighten`（|cos|）给 ——
/// 呼吸的是尺寸和亮度，不是剩余时间。
fn glow_color(base: Color32, lighten: f32) -> Color32 {
    let add = (GLOW_ALPHA as f32 * lighten.clamp(0.0, 1.0)) as u8;
    Color32::from_rgba_unmultiplied(
        base.r().saturating_add(add),
        base.g().saturating_add(add),
        base.b().saturating_add(add),
        GLOW_ALPHA,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::keyboard::{BLACK_HIT_W, KEY_W, WHITE_SLOTS};
    use crate::fx::stage::Primitive;
    use crate::fx::tiles::{instrument_color, InstrumentBlocks, NotePalette};
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
                area: stage(),
                keyboard: None,
                timeline: &self.timeline,
                blocks: &self.blocks,
                tiles: None,
                palette: &self.palette,
            }
        }
    }

    /// 胶囊 = 带圆角的那批矩形（键面填色是直角，粒子是小方块）。
    fn capsules(frame: &Frame) -> Vec<(Rect, f32)> {
        frame
            .prims
            .iter()
            .filter_map(|prim| match *prim {
                Primitive::Rect {
                    rect, rounding, ..
                } if rounding > 0.0 => Some((rect, rounding)),
                _ => None,
            })
            .collect()
    }

    fn glow(frame: &Frame) -> (f32, Color32) {
        frame
            .prims
            .iter()
            .find_map(|prim| match *prim {
                Primitive::Glow { spread, color, .. } => Some((spread, color)),
                _ => None,
            })
            .expect("这一帧该有一枚发光")
    }

    /// 键面按住填色 = 直角那批矩形（胶囊和粒子都带圆角）。
    fn key_fills(frame: &Frame) -> Vec<Rect> {
        frame
            .prims
            .iter()
            .filter_map(|prim| match *prim {
                Primitive::Rect {
                    rect,
                    rounding: 0.0,
                    ..
                } => Some(rect),
                _ => None,
            })
            .collect()
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

    /// ① 胶囊高度就是时值：同一枚键按住 4 列，胶囊就该是 1 列那枚的四倍来高。
    /// 这首测试曲 10 t/s ⇒ 一列 0.1 秒；瀑布 2.4 秒走完 800px ⇒ 333.3px/秒。
    #[test]
    fn capsule_height_is_the_note_duration() {
        let rig = Rig::new(&[(10, 0, 60), (14, 0, 60)]);
        let notes = rig.timeline.notes();
        assert_eq!((notes[0].duration, notes[1].duration), (4, 1));
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.0, 0.0, false), &mut frame);
        let list = capsules(&frame);
        assert_eq!(list.len(), 2);
        // 4 列 = 0.4 秒，垫完留缝后 0.39 秒 ⇒ 130px；1 列那枚垫到 0.1 − 0.01 ⇒ 30px。
        assert!((list[0].0.height() - 130.0).abs() < 0.5, "{:?}", list[0]);
        assert!((list[1].0.height() - 30.0).abs() < 0.5, "{:?}", list[1]);
        // 下缘 = 起手那一行，上缘 = 收尾那一行：长音是从上往下被吃掉的。
        assert!(list[0].0.max.y > list[1].0.max.y, "起手早的那枚该更靠下");
    }

    /// ⑤ 宽 = 道宽 −1px、圆角 = 道宽 ×0.2（上游那两个数）。黑键道比白键道窄，
    /// 胶囊也跟着窄，一眼看得出音符落在哪一族键上。
    #[test]
    fn capsule_width_and_rounding_follow_the_lane() {
        let rig = Rig::new(&[(10, 0, 60), (14, 0, 61)]);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.0, 0.0, false), &mut frame);
        let list = capsules(&frame);
        assert_eq!(list.len(), 2);
        let (white, black) = (&list[0], &list[1]);
        assert!((white.0.width() - (KEY_W - 1.0)).abs() < 0.01, "{white:?}");
        assert!((black.0.width() - (BLACK_HIT_W - 1.0)).abs() < 0.01, "{black:?}");
        assert!((white.1 - KEY_W * CAPSULE_ROUNDING).abs() < 0.01);
        assert!((black.1 - BLACK_HIT_W * CAPSULE_ROUNDING).abs() < 0.01);
        // 胶囊骑在道的正中，两侧各留半像素，不压到邻道的线上。
        let layout = Synthesia::fallback_keyboard(stage(), 1.0);
        let center = layout.lane_center(60).unwrap();
        assert!((white.0.center().x - center).abs() < 0.01);
    }

    /// ⑤ 延迟补偿：调大 = 整条瀑布退回上方，胶囊晚一点落线，声音才追得上。
    #[test]
    fn animation_offset_pushes_the_whole_waterfall_up() {
        let rig = Rig::new(&[(10, 0, 60)]);
        let mut plain = Synthesia::default();
        let mut delayed = Synthesia::new_with(Params {
            animation_offset: 0.5,
            ..Params::default()
        });
        let mut a = Frame::default();
        let mut b = Frame::default();
        plain.draw(&rig.cx(0.0, 0.0, false), &mut a);
        delayed.draw(&rig.cx(0.0, 0.0, false), &mut b);
        let shift = capsules(&b)[0].0.max.y - capsules(&a)[0].0.max.y;
        // 补 0.5 秒 ⇒ 往上挪 0.5 × 333.3 ≈ 167px。
        assert!(
            (shift + 166.67).abs() < 1.0,
            "补偿没推动画面？shift={shift}"
        );
        assert_eq!(
            Synthesia::default().params.animation_offset,
            0.0,
            "默认不补偿：音频链路本来就该对齐"
        );
    }

    /// ② 竖线只在 C 和 F 两处、一明一暗；小节横线整幅压过去，从上方落下来。
    #[test]
    fn only_c_and_f_get_a_vertical_and_bars_fall_from_above() {
        // 曲子跨过两小节，第二小节那条线才有「拍速」可依。
        let rig = Rig::new(&[(10, 0, 60), (24, 0, 64)]);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.0, 0.0, false), &mut frame);
        let layout = Synthesia::fallback_keyboard(stage(), 1.0);

        let verticals: Vec<(f32, u8)> = frame
            .prims
            .iter()
            .filter_map(|prim| match *prim {
                Primitive::Line { from, to, color, .. } if (from.x - to.x).abs() < 0.01 => {
                    Some((from.x, color.r()))
                }
                _ => None,
            })
            .collect();
        // 只画八度的头：52 个槽里挑出 C 和 F，比全画少一大截。
        let want = (0..layout.slots)
            .filter(|a| {
                let class = layout.slot_white_key(*a).unwrap_or(99) % 12;
                class == GUIDE_C || class == GUIDE_F
            })
            .count();
        assert_eq!(verticals.len(), want, "竖线该正好落在 C 和 F 上");
        assert!(want < layout.slots, "不该每条道都画一根");
        let grays: Vec<u8> = verticals.iter().map(|v| v.1).collect();
        assert!(grays.contains(&GUIDE_C_GRAY) && grays.contains(&GUIDE_F_GRAY), "两档灰度都要出现");
        assert!(grays
            .iter()
            .all(|g| *g == GUIDE_C_GRAY || *g == GUIDE_F_GRAY));

        // 4/4、10 t/s ⇒ 一小节 16 列 = 1.6 秒，2.4 秒的窗口里正好落下第 16 列那条。
        let hit_y = layout.origin.y;
        let bars: Vec<f32> = frame
            .prims
            .iter()
            .filter_map(|prim| match *prim {
                Primitive::Line { from, to, .. }
                    if (from.y - to.y).abs() < 0.01
                        && (to.y - hit_y).abs() > 0.01
                        && (from.x - 0.0).abs() < 0.01 =>
                {
                    Some(from.y)
                }
                _ => None,
            })
            .collect();
        assert_eq!(bars.len(), 1, "该只有一条小节线落在窗口里：{bars:?}");
        assert!(bars[0] < hit_y, "小节线在击中线上方，越往下越接近「现在」");
    }

    /// ③ 按住的键整面填色（不是描一圈光），⑥ 发光 α 恒定、只有尺寸在呼吸。
    #[test]
    fn a_held_key_is_filled_and_breathes_at_constant_alpha() {
        let rig = Rig::new(&[(0, 0, 60)]);
        let mut fx = Synthesia::default();
        let mut frame = Frame::default();
        // 播放头压在这枚音上（起手已过、尾还没到）＝ 正被按住。
        fx.draw(&rig.cx(0.0, 0.0, false), &mut frame);
        let fills = key_fills(&frame);
        assert_eq!(fills.len(), 1, "按下的键该有一整面填色：{fills:?}");
        // 直角、往里收 1px：面板画的黑白键本是方角，压着边填会糊到邻键。
        assert!(
            (fills[0].width() - (KEY_W - 2.0)).abs() < 0.01,
            "{:?}",
            fills[0]
        );
        let (spread, color) = glow(&frame);
        assert_eq!(color.a(), GLOW_ALPHA, "α 恒定，不随剩余时间衰减");
        // 呼吸的是尺寸：在参数给的锚点上下 ±7%，锚点本身不动。
        assert!(
            (spread - 12.0).abs() <= 12.0 * GLOW_BREATH + 1e-4,
            "{spread}"
        );

        // 墙钟往前走一点：α 不动，尺寸起伏 —— 呼吸而不是淡出。
        let mut later = Frame::default();
        fx.draw(&rig.cx(0.0, 0.4, false), &mut later);
        let (spread2, color2) = glow(&later);
        assert_eq!(color2.a(), GLOW_ALPHA);
        assert_ne!(spread, spread2, "尺寸该随墙钟呼吸");

        // 尾也落过去了：不填色、不发光，胶囊已被击中线吃掉。
        let mut gone = Frame::default();
        fx.draw(&rig.cx(10.0, 0.8, false), &mut gone);
        assert!(capsules(&gone).is_empty(), "收尾之后不该剩半截胶囊");
        assert!(
            !gone
                .prims
                .iter()
                .any(|p| matches!(p, Primitive::Glow { .. })),
            "松手就该停掉发光"
        );
    }

    /// ④ 按声道：一层一色、只有五档（第六层循环回第一档），黑键道用同族暗档。
    #[test]
    fn layer_mode_gives_each_track_its_own_colour() {
        let rig = Rig::new(&[]);
        let cx = rig.cx(0.0, 0.0, false);
        let fx = Synthesia::new_with(Params {
            color_mode: ColorMode::Layer,
            ..Params::default()
        });
        let note = |layer: u32, key: u8| FxNote {
            tick: 0,
            layer,
            key,
            instrument: 0,
            velocity: 100,
            duration: 1,
        };
        // 60 = C（白键），61 = C#（黑键）。
        assert_eq!(
            fx.capsule_base(&cx, &note(0, 60)),
            Color32::from_rgb(210, 89, 222)
        );
        assert_eq!(
            fx.capsule_base(&cx, &note(0, 61)),
            Color32::from_rgb(125, 69, 134),
            "黑键道用暗档"
        );
        assert_ne!(
            fx.capsule_base(&cx, &note(1, 60)),
            fx.capsule_base(&cx, &note(2, 60))
        );
        assert_eq!(
            fx.capsule_base(&cx, &note(0, 60)),
            fx.capsule_base(&cx, &note(5, 60)),
            "五档之后循环"
        );
    }

    /// 配色对齐主界面：精灵素材在库里时，胶囊底色 = 主界面画那格音符的色；
    /// 素材没入库才退回按乐器名哈希。
    #[test]
    fn instrument_mode_paints_the_colour_the_grid_draws() {
        let rig = Rig::new(&[(10, 3, 60)]);
        let fx = Synthesia::default();
        let note = FxNote {
            tick: 10,
            layer: 0,
            key: 60,
            instrument: 3,
            velocity: 100,
            duration: 1,
        };
        // 空表（Rig::new）走哈希兜底，和以前一样分得开乐器。
        let name = rig.timeline.instrument_name(3);
        assert_eq!(
            fx.capsule_base(&rig.cx(0.0, 0.0, false), &note),
            instrument_color(name, 3)
        );

        let palette = NotePalette::open();
        if palette.is_empty() {
            eprintln!("跳过：assets/icons/block_color 没素材");
            return;
        }
        let rig = Rig::with_palette(&[(10, 3, 60), (11, 3, 61)], palette.clone());
        let cx = rig.cx(0.0, 0.0, false);
        let white = fx.capsule_base(&cx, &note);
        assert_eq!(
            white,
            palette.color(3).unwrap(),
            "白键道就该等于主界面那格精灵的平均色"
        );
        let black = FxNote { key: 61, ..note };
        assert_eq!(
            fx.capsule_base(&cx, &black),
            palette.color(3).unwrap().linear_multiply(0.72),
            "黑键道在同一支精灵色上压暗一档"
        );
        assert_ne!(white, instrument_color(name, 3), "有素材时不该还走哈希");
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

        // 击中线压在面板的白键面沿（700 + BLACK_LIFT），不是舞台下缘 800。
        let hit = panel_layout.origin.y;
        assert!(
            frame.prims.iter().any(|prim| matches!(prim,
                Primitive::Line { from, to, .. } if (from.y - hit).abs() < 0.01 && (to.y - hit).abs() < 0.01)),
            "没有一条线压在琴键面沿上（击中线 y 应为 {hit}）"
        );
        // 面板放的是白键槽 10..30，即键号 17..51：45 号在栏里，60 号不在，只剩一枚胶囊
        //（按住的键面填色是直角，`capsules` 只数带圆角的那批）。
        let list = capsules(&frame);
        assert_eq!(list.len(), 1, "琴键栏放不下的音符不该有轨道");
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

    /// 一屏图元只覆盖可见窗口：轨道线现在只有 C/F 两处，静态图元比原来那一版
    /// 「每条道一根」少了 40 来条。
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

        // 空曲子剩下的只有八度参考线、一小节一条的横线和击中线
        //（键盘由琴键面板画，不在这一帧里）。
        let quiet = Rig::new(&[]);
        let mut bare = Synthesia::default();
        let mut empty = Frame::default();
        bare.draw(&quiet.cx(12.0, 1.2, false), &mut empty);
        assert!(empty.len() < frame.len(), "有音符的帧该比空曲子多出胶囊");
        assert!(empty.len() < 60, "静态图元就该这么几条：{}（C/F 竖线 + 小节线 + 击中线）", empty.len());

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
        assert!(last.len() < 60, "曲尾的帧也该只比静态多一点（{}）", last.len());
    }

    /// 长音的尾还在响，胶囊就必须留在窗口里：下缘按烘表记下的最长时值往前多取，
    /// 不然长音会「尾还没落到底就整条消失」。
    #[test]
    fn a_long_note_stays_on_screen_until_its_tail_lands() {
        // 第 10 列一枚，28 秒后才在同键再来一枚 ⇒ 这枚按住了 280 列。
        let rig = Rig::new(&[(10, 0, 60), (290, 0, 60)]);
        assert_eq!(rig.timeline.notes()[0].duration, 280);
        let mut fx = Synthesia::default();
        // 播放头走到起手之后 3 秒：胶囊的尾还早得很。
        let mut frame = Frame::default();
        fx.draw(&rig.cx(40.0, 3.0, false), &mut frame);
        let list = capsules(&frame);
        assert_eq!(list.len(), 1, "起手过了、尾没到底 —— 该还挂着");
        assert!(
            list[0].0.height() > 100.0,
            "长音的胶囊该很高，不是垫底的那 30px：{:?}",
            list[0]
        );
    }

    #[test]
    fn params_round_trip_through_json() {
        let fx = Synthesia::new_with(Params {
            fall_seconds: 3.5,
            lanes: false,
            color_mode: ColorMode::Layer,
            ..Params::default()
        });
        let value = fx.params();
        assert_eq!(value["fall_seconds"].as_f64(), Some(3.5));
        assert_eq!(value["lanes"].as_bool(), Some(false));
        assert_eq!(value["color_mode"].as_str(), Some("layer"));
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

    /// 老首选项里只有 `color_by_instrument`（那时只有「按乐器 / 单色」两档）：
    /// 写着 false 是用户亲手关掉的，得认成「单色」，不能被新默认值「按乐器」改回去。
    #[test]
    fn an_old_single_colour_preference_stays_single() {
        let mut fx = Synthesia::default();
        fx.set_params(&serde_json::json!({ "color_by_instrument": false }));
        assert_eq!(fx.params.color_mode, ColorMode::Accent);
        fx.set_params(&serde_json::json!({ "color_by_instrument": true }));
        assert_eq!(fx.params.color_mode, ColorMode::Instrument);
        // 新字段自己写着档位时，以新字段为准，旧键不再说话。
        fx.set_params(&serde_json::json!({ "color_mode": "layer", "color_by_instrument": false }));
        assert_eq!(fx.params.color_mode, ColorMode::Layer);
        // 两个键都没有（第一次用这版）不该被误伤。
        let mut fresh = Synthesia::default();
        fresh.set_params(&serde_json::json!({ "fall_seconds": 3.0 }));
        assert_eq!(fresh.params.color_mode, ColorMode::Instrument);
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
        // 三态里的「单色」档回到主题强调色（原版 Synthesia 那种一色瀑布）。
        let plain = Synthesia::new_with(Params {
            color_mode: ColorMode::Accent,
            ..Params::default()
        });
        assert_eq!(plain.capsule_base(&cx, &notes[0]), cx.accent);
        // 强调色那一档也分黑白键深浅，否则一整片同色看不出落在哪族键上。
        assert!(plain.capsule_base(&cx, &notes[2]).r() < cx.accent.r());
    }

    /// 深浅由 [`shade`] 给（黑键那档压暗 0.72），力度只动不透明度 ——
    /// 落下之后不再靠 α 淡出，收尾靠「胶囊被击中线吃掉」。
    #[test]
    fn black_keys_are_darker_and_velocity_sets_alpha() {
        let accent = Color32::from_rgb(200, 220, 240);
        assert_eq!(shade(accent, false), accent);
        let dark = shade(accent, true);
        assert!(dark.r() < accent.r() && dark.g() < accent.g(), "{dark:?}");
        let white = capsule_color(accent, 100);
        assert_eq!(white.a(), 255);
        // 弱音符更透明，密集的弱拍才不糊成一团。
        assert!(capsule_color(accent, 20).a() < white.a());
        // 力度不影响色相，只影响 α。
        assert_eq!([white.r(), white.g(), white.b()], [200, 220, 240]);
        // 发光：α 钉死，提白随 |cos| 走，最亮时也不越过 255。
        assert_eq!(glow_color(accent, 0.0).a(), GLOW_ALPHA);
        assert_eq!(glow_color(accent, 1.0).a(), GLOW_ALPHA);
        assert_eq!(
            glow_color(Color32::WHITE, 1.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, GLOW_ALPHA)
        );
        // 比的是同一档 α 下的两帧，不是直接和 `accent` 比：egui 的 `Color32` 存的是
        // 预乘色，α=51 的发光 `.r()` 本来就只有原色的五分之一。
        assert!(
            glow_color(accent, 1.0).r() > glow_color(accent, 0.0).r(),
            "{:?} {:?}",
            glow_color(accent, 0.0),
            glow_color(accent, 1.0)
        );
    }

    #[test]
    fn it_declares_itself_as_a_stage() {
        let info = Synthesia::default().info();
        assert_eq!(info.id, Synthesia::ID);
        assert_eq!(info.surface, Surface::Stage);
        assert!(info.wants_piano, "瀑布的轨道按底部琴键栏放下，得把面板顶出来");
        assert!(!info.name.is_empty());
    }
}
