//! 分离特效浮窗：同一进程里的 deferred 原生视口。
//!
//! 主窗与浮窗共享唯一一份 [`SharedFx`]，但绘制互斥。浮窗自己算舞台/琴键几何，
//! 真实音频直接读音频线程的 [`SharedState`]，无设备时用一份可推进的时钟快照，
//! 因此主窗最小化后浮窗仍能独立重绘。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use egui::{Color32, Context, LayerId, Painter, Pos2, Rect, Vec2, ViewportBuilder, ViewportClass, ViewportId, WindowLevel};
use nbs_audio::scheduler::{Shared, TransportClock, TransportSnapshot};
use nbs_core::model::Note;

use crate::icons::Bank;
use crate::piano::{paint_piano_keys, DrawOpts, KeysHeld, KeysPlayed, PianoDraw, PANEL_HEIGHT};
use crate::state::{EditorState, UiFlags};
use crate::theme::Theme;

use super::keyboard::KeyboardLayout;
use super::{FxInput, SharedFx};

static VIEWPORT_ID: OnceLock<ViewportId> = OnceLock::new();

pub fn viewport_id() -> ViewportId {
    *VIEWPORT_ID.get_or_init(|| ViewportId::from_hash_of("nbs_fx_playback"))
}
const DEFAULT_SIZE: Vec2 = Vec2::new(1280.0, 720.0);
const MIN_SIZE: Vec2 = Vec2::new(640.0, 360.0);
const ROOT_IDLE_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct Visuals {
    theme: Theme,
    flags: UiFlags,
    current: Note,
    keys_held: KeysHeld,
    key_played: KeysPlayed,
    bank: Option<std::sync::Arc<Bank>>,
    piano_scale: f32,
    transparent: bool,
    always_on_top: bool,
    refresh_limit: f64,
}

impl Visuals {
    fn from_state(state: &EditorState) -> Self {
        Self {
            theme: state.theme.clone(),
            flags: state.flags.clone(),
            current: state.current,
            keys_held: state.keys_held,
            key_played: state.key_played,
            bank: state.icons.bank(),
            piano_scale: state.prefs.piano_scale.max(0.1),
            transparent: state.prefs.fx_window_transparent,
            always_on_top: state.prefs.fx_window_always_on_top,
            refresh_limit: state.prefs.refresh_limit(),
        }
    }
}

#[derive(Clone, Copy)]
struct TransportView {
    position: f64,
    playing: bool,
    speed: f32,
}

struct FallbackClock {
    snapshot: TransportSnapshot,
    last: Instant,
    last_root: Instant,
}

impl FallbackClock {
    fn advance(&mut self, now: Instant) -> TransportView {
        let dt = now.duration_since(self.last).as_secs_f64().min(0.25);
        self.last = now;
        let snap = &mut self.snapshot;
        if snap.playing {
            snap.position += dt * snap.tempo.max(0.25) as f64 / snap.speed.max(0.25) as f64;
            if snap.loop_enabled && snap.loop_end > snap.loop_start && snap.position > snap.loop_end {
                snap.position = snap.loop_start;
            }
            if snap.stop_at >= 0.0 && snap.position >= snap.stop_at {
                snap.position = snap.stop_at;
                snap.playing = false;
            }
        }
        TransportView {
            position: snap.position,
            playing: snap.playing,
            speed: snap.speed,
        }
    }
}

pub struct FxWindowShared {
    fx: SharedFx,
    audio: Option<Shared>,
    visuals: Mutex<Visuals>,
    fallback: Mutex<FallbackClock>,
    size: Mutex<Option<Vec2>>,
    close_requested: AtomicBool,
}

impl FxWindowShared {
    pub fn new(fx: SharedFx, clock: &TransportClock, state: &EditorState) -> Self {
        let now = Instant::now();
        Self {
            fx,
            audio: clock.shared.clone(),
            visuals: Mutex::new(Visuals::from_state(state)),
            fallback: Mutex::new(FallbackClock {
                snapshot: clock.snapshot(),
                last: now,
                last_root: now,
            }),
            size: Mutex::new(None),
            close_requested: AtomicBool::new(false),
        }
    }

    fn audio_active(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|shared| shared.lock().audio_active)
    }

    /// 主窗恢复绘制时，把浮窗在最小化期间推进的静音时钟接回真实时钟。
    pub fn resume_state(&self, state: &mut EditorState) {
        if self.audio_active() {
            return;
        }
        let now = Instant::now();
        let snapshot = {
            let fallback = self.fallback.lock().unwrap_or_else(|e| e.into_inner());
            (now.duration_since(fallback.last_root) > ROOT_IDLE_GRACE).then_some(fallback.snapshot)
        };
        if let Some(snapshot) = snapshot {
            state.audio.clock_mut().restore(snapshot);
        }
    }

    pub fn sync_from_state(&self, state: &EditorState) {
        *self.visuals.lock().unwrap_or_else(|e| e.into_inner()) = Visuals::from_state(state);
        if !self.audio_active() {
            let now = Instant::now();
            let mut fallback = self.fallback.lock().unwrap_or_else(|e| e.into_inner());
            fallback.snapshot = state.audio.clock().snapshot();
            fallback.last = now;
            fallback.last_root = now;
        }
    }

    pub fn take_close_requested(&self) -> bool {
        self.close_requested.swap(false, Ordering::AcqRel)
    }

    pub fn builder(&self) -> ViewportBuilder {
        let visuals = self.visuals.lock().unwrap_or_else(|e| e.into_inner());
        let size = self
            .size
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(DEFAULT_SIZE);
        ViewportBuilder::default()
            .with_title(crate::tr("Playback effects", "播放特效"))
            .with_inner_size(size)
            .with_min_inner_size(MIN_SIZE)
            .with_decorations(true)
            .with_resizable(true)
            .with_window_level(if visuals.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_mouse_passthrough(false)
            // 子窗口保持普通装饰窗口；Windows 侧用 composition accent 给客户区
            // 加透明 gradient，不改变标题栏、任务栏和边框。
            .with_transparent(false)
            .with_taskbar(true)
    }

    fn transport(&self, now: Instant) -> TransportView {
        if self.audio_active() {
            let shared = self.audio.as_ref().expect("audio_active 为真时必有共享状态");
            let state = shared.lock();
            return TransportView {
                position: state.position,
                playing: state.params.playing,
                speed: state.params.speed,
            };
        }
        let mut fallback = self.fallback.lock().unwrap_or_else(|e| e.into_inner());
        fallback.advance(now)
    }

    pub fn paint(&self, ctx: &Context, class: ViewportClass) {
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_requested.store(true, Ordering::Release);
        }
        if class == ViewportClass::Embedded {
            egui::Window::new(crate::tr("Playback effects", "播放特效")).show(ctx, |ui| {
                ui.label(crate::tr(
                    "This backend cannot create a native floating window.",
                    "当前后端无法创建原生浮窗。",
                ));
            });
            return;
        }

        let area = ctx.screen_rect();
        if area.width() <= 0.0 || area.height() <= 0.0 {
            ctx.request_repaint();
            return;
        }
        if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
            let size = inner.size();
            if size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0 {
                *self.size.lock().unwrap_or_else(|e| e.into_inner()) = Some(size);
            }
        }

        let visuals = self.visuals.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // 琴键倍率再大也不能把整个浮窗吃光：至少给舞台留 120px。
        let scale = visuals
            .piano_scale
            .min(((area.height() - 120.0) / PANEL_HEIGHT).max(0.1))
            .max(0.1);
        let panel_height = PANEL_HEIGHT * scale;
        let panel = Rect::from_min_size(
            Pos2::new(area.min.x, area.max.y - panel_height),
            Vec2::new(area.width(), panel_height),
        );
        let stage_area = Rect::from_min_size(
            area.min,
            Vec2::new(area.width(), (area.height() - panel_height).max(1.0)),
        );
        let keyboard = KeyboardLayout::fit(panel, scale, 27);
        let transport = self.transport(Instant::now());
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let p = ctx.layer_painter(LayerId::background());

        let mut fx = self.fx.lock();
        let stage = fx.stage_active();
        let overlay = fx.overlay_active();
        if !visuals.transparent {
            p.rect_filled(area, 0.0, visuals.theme.window_bg);
            if stage {
                p.rect_filled(panel, 0.0, fx.stage_backdrop(visuals.theme.grid_bg));
            }
        }

        let mut key_played = visuals.key_played;
        let now_seconds = fx.timeline.seconds_at(transport.position) as f64;
        let from = fx.timeline.tick_at_seconds(now_seconds - 0.3);
        for note in fx.timeline.notes_in(from, transport.position + 0.001) {
            let age = now_seconds - fx.timeline.seconds_at(f64::from(note.tick)) as f64;
            if (0.0..0.3).contains(&age) {
                if let Some(at) = key_played.get_mut(note.key as usize) {
                    *at = now - age;
                }
            }
        }
        let opts = DrawOpts {
            names: visuals.flags.show_keynames,
            numbers: visuals.flags.show_keynumbers,
            keyboard: visuals.flags.show_keyboard,
            out_of_range: visuals.flags.show_outofrange,
            current: visuals.current.key,
            playing: transport.playing,
            flats: visuals.flags.flats,
            show_chart: false,
        };
        paint_piano_keys(
            &p,
            &visuals.theme,
            visuals.bank.as_deref(),
            &PianoDraw {
                layout: keyboard,
                width: area.width(),
                opts: &opts,
                keys_held: visuals.keys_held,
                key_played,
                now,
                pointer: None,
                holding: false,
                pressing: false,
            },
        );

        let backdrop = if visuals.transparent {
            Color32::TRANSPARENT
        } else if stage {
            fx.stage_backdrop(visuals.theme.grid_bg)
        } else {
            visuals.theme.grid_bg
        };
        let input = FxInput {
            now,
            dt,
            position: transport.position,
            playing: transport.playing,
            speed: transport.speed,
            scale,
            accent: visuals.theme.accent,
            backdrop,
            transparent: visuals.transparent,
            area: stage_area,
            keyboard: Some(keyboard),
        };
        if stage {
            let clip = stage_area.union(panel);
            let stage_painter = Painter::new(ctx.clone(), LayerId::background(), clip);
            fx.paint_stage(ctx, &input, &stage_painter);
        }
        if overlay {
            let overlay_input = FxInput {
                area,
                ..input
            };
            fx.paint_overlay(ctx, &overlay_input);
        }

        let refresh = visuals.refresh_limit.max(1.0);
        ctx.request_repaint_after(Duration::from_secs_f64(1.0 / refresh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallback(snapshot: TransportSnapshot) -> FallbackClock {
        let now = Instant::now();
        FallbackClock {
            snapshot,
            last: now,
            last_root: now,
        }
    }

    fn snapshot(playing: bool, position: f64) -> TransportSnapshot {
        TransportSnapshot {
            position,
            tempo: 20.0,
            speed: 2.0,
            playing,
            loop_enabled: false,
            loop_start: 0.0,
            loop_end: 0.0,
            stop_at: -1.0,
        }
    }

    #[test]
    fn fallback_clock_advances_only_while_playing() {
        let now = Instant::now();
        let mut paused = fallback(snapshot(false, 10.0));
        paused.last = now - Duration::from_millis(100);
        assert_eq!(paused.advance(now).position, 10.0);

        let mut playing = fallback(snapshot(true, 10.0));
        playing.last = now - Duration::from_millis(100);
        let got = playing.advance(now);
        assert!((got.position - 11.0).abs() < 1e-6, "20 t/s、2 倍速、0.1 秒 = 1 列");
        assert!(got.playing);
    }

    #[test]
    fn fallback_clock_honors_loop_and_stop() {
        let now = Instant::now();
        let mut looped = fallback(snapshot(true, 9.9));
        looped.snapshot.loop_enabled = true;
        looped.snapshot.loop_start = 2.0;
        looped.snapshot.loop_end = 10.0;
        looped.last = now - Duration::from_millis(100);
        assert_eq!(looped.advance(now).position, 2.0);

        let mut stopped = fallback(snapshot(true, 10.0));
        stopped.snapshot.stop_at = 10.5;
        stopped.last = now - Duration::from_millis(100);
        let got = stopped.advance(now);
        assert_eq!(got.position, 10.5);
        assert!(!got.playing, "到 marker_end 要停");
    }

    #[test]
    fn detached_layout_keeps_piano_outside_stage() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0));
        let scale = 1.0;
        let panel_height = PANEL_HEIGHT * scale;
        let panel = Rect::from_min_size(
            Pos2::new(area.min.x, area.max.y - panel_height),
            Vec2::new(area.width(), panel_height),
        );
        let stage = Rect::from_min_size(
            area.min,
            Vec2::new(area.width(), area.height() - panel_height),
        );
        let keyboard = KeyboardLayout::fit(panel, scale, 27);
        assert_eq!(stage.max.y, panel.min.y);
        assert_eq!(keyboard.origin.y, panel.min.y + super::super::keyboard::BLACK_LIFT);
        assert!(keyboard.slots > 0);
    }
}
