//! 播放调度（C 模块）与 UI/音频线程共享状态。
//!
//! 原版把 `marker_pos += tempo/room_speed` 放在 Draw 帧里（虽有 delta_time 补偿，
//! 仍与 UI 帧绑定）。这里改成**音频回调按采样推进**：每个输出采样推进
//! `tempo / speed / sample_rate` 个 tick，跨过整数 tick 时派发该列音符。
//! 掉帧、窗口拖拽、后台渲染都不会让节奏跑偏。

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use nbs_core::model::{Note, Song};

use crate::mixer::{Mixer, DEFAULT_MAX_VOICES};
use crate::samples::SoundBank;

#[derive(Clone, Debug)]
pub struct NoteEvent {
    pub tick: u32,
    pub layer: u32,
    pub note: Note,
    pub layer_volume: u8,
    pub layer_stereo: u8,
    /// Tempo Changer 自定义乐器：pitch 字段即 BPM，播放到此处改速度。
    pub tempo_change_bpm: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct PlayParams {
    /// ticks/second（原版 `tempo`）。
    pub tempo: f32,
    /// 播放速度倍率。
    pub speed: f32,
    /// 主音量 0..1。
    pub master: f32,
    pub loop_enabled: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub loop_max: u8,
    pub loop_count: i32,
    pub metronome: bool,
    pub playing: bool,
    pub song_length: f64,
    pub max_voices: usize,
    pub mono: bool,
    pub time_signature: u8,
    pub include_locked: bool,
}

impl Default for PlayParams {
    fn default() -> Self {
        Self {
            tempo: 10.0,
            speed: 1.0,
            master: 1.0,
            loop_enabled: false,
            loop_start: 0.0,
            loop_end: 0.0,
            loop_max: 0,
            loop_count: 0,
            metronome: false,
            playing: false,
            song_length: 0.0,
            max_voices: DEFAULT_MAX_VOICES,
            mono: false,
            time_signature: 4,
            include_locked: false,
        }
    }
}

/// 把整首歌展开成按 tick 排序的事件表（实时与离线共用）。
/// 锁定/独奏屏蔽的层默认排除（对齐原版 `control_draw.gml:480-495`）。
pub fn build_event_table(song: &Song, include_locked: bool) -> Vec<NoteEvent> {
    let locked = song.locked_layers();
    let mut events: Vec<NoteEvent> = song
        .iter_notes()
        .filter(|(_, layer, _)| include_locked || !locked.get(*layer as usize).copied().unwrap_or(false))
        .map(|(tick, layer, note)| {
            let (layer_volume, layer_stereo) = song
                .layers
                .get(layer as usize)
                .map(|l| (l.volume, l.stereo))
                .unwrap_or((100, 100));
            // Tempo Changer：自定义乐器名匹配，pitch 字段存 BPM
            let tempo_change_bpm = song
                .custom_instruments
                .get(note.instrument.saturating_sub(16) as usize)
                .filter(|ci| ci.name.eq_ignore_ascii_case("Tempo Changer"))
                .map(|_| note.pitch as f32);
            NoteEvent { tick, layer, note, layer_volume, layer_stereo, tempo_change_bpm }
        })
        .collect();
    events.sort_by_key(|e| (e.tick, e.layer));
    events
}

pub struct SharedState {
    pub params: PlayParams,
    pub events: Vec<NoteEvent>,
    pub cursor: usize,
    /// 浮点 tick 位置（播放头）。
    pub position: f64,
    pub mixer: Mixer,
    pub bank: Arc<SoundBank>,
    pub sample_rate: u32,
    /// 是否由真实音频回调推进（false 时 UI 侧内部时钟代理）。
    pub audio_active: bool,
    pub frames_rendered: u64,
    metronome_played: i64,
}

pub type Shared = Arc<Mutex<SharedState>>;

impl SharedState {
    pub fn new(bank: Arc<SoundBank>, sample_rate: u32) -> Self {
        Self {
            params: PlayParams::default(),
            events: Vec::new(),
            cursor: 0,
            position: 0.0,
            mixer: Mixer::new(sample_rate),
            bank,
            sample_rate,
            audio_active: false,
            frames_rendered: 0,
            metronome_played: -1,
        }
    }

    /// 载入/重新载入歌曲。
    pub fn prepare(&mut self, song: &Song, bank: Arc<SoundBank>) {
        self.bank = bank;
        self.events = build_event_table(song, self.params.include_locked);
        self.params.tempo = song.meta.tempo;
        self.params.time_signature = song.meta.time_signature;
        self.params.loop_enabled = song.meta.loop_enabled;
        self.params.loop_start = song.meta.loop_start as f64;
        self.params.loop_end = song.song_length().max(1) as f64;
        self.params.loop_max = song.meta.loop_max;
        self.params.loop_count = song.meta.loop_max as i32;
        self.params.song_length = song.song_length() as f64;
        self.cursor = 0;
        self.mixer.clear();
    }

    pub fn seek(&mut self, tick: f64) {
        self.position = tick.max(0.0);
        self.cursor = self
            .events
            .partition_point(|e| (e.tick as f64) < self.position);
        self.metronome_played = -1;
    }

    pub fn refresh_events(&mut self, song: &Song) {
        self.events = build_event_table(song, self.params.include_locked);
        self.params.song_length = song.song_length() as f64;
        self.seek(self.position);
    }

    /// 处理 tick 跨越时的事件派发。
    fn trigger(&mut self, from: i64, to: i64) {
        if to < from {
            return;
        }
        let diff = to - from;
        while self.cursor < self.events.len() {
            let ev = &self.events[self.cursor];
            let t = ev.tick as i64;
            if t > to {
                break;
            }
            // 卡顿/跳转跨过 >3 列时，只补播最后一列（原版行为）
            let should_play = (diff <= 3 && t >= from) || (diff > 3 && t == to);
            if should_play {
                if let Some(bpm) = ev.tempo_change_bpm {
                    self.params.tempo = (bpm.abs() / 15.0).max(0.25);
                }
                self.mixer.max_voices = self.params.max_voices;
                self.mixer.master = self.params.master;
                self.mixer.mono = self.params.mono;
                self.mixer.output_rate = self.sample_rate;
                self.mixer
                    .spawn(&ev.note, ev.layer_volume, ev.layer_stereo, &self.bank);
            }
            self.cursor += 1;
        }
    }

    fn trigger_metronome(&mut self, position: f64) {
        if !self.params.metronome || self.bank.metronome.is_none() {
            return;
        }
        // 原版：pos = floor(marker_pos - 0.1)，每 4 tick 一响
        let pos = (position - 0.1).floor() as i64;
        if pos >= 0 && pos % 4 == 0 && self.metronome_played < pos {
            let mut click = Note::new(4, 45);
            click.velocity = 60;
            self.mixer.master = self.params.master;
            self.mixer.spawn(&click, 100, 100, &self.bank);
            self.metronome_played = pos + 1;
        }
    }

    /// 渲染一个输出采样帧（同时推进调度时钟）。
    pub fn render_frame(&mut self) -> (f32, f32) {
        if self.params.playing {
            let prev = self.position;
            let tick_per_sample =
                self.params.tempo as f64 / self.params.speed.max(0.01) as f64
                    / self.sample_rate.max(1) as f64;
            self.position += tick_per_sample;

            let from = prev.floor() as i64 + 1;
            let to = self.position.floor() as i64;
            if to >= from {
                self.trigger(from, to);
                self.trigger_metronome(self.position);
            }

            if self.params.loop_enabled && self.params.loop_end > self.params.loop_start {
                if self.position > self.params.loop_end {
                    self.params.loop_count -= 1;
                    if self.params.loop_max != 0 && self.params.loop_count < 0 {
                        self.params.playing = false;
                        self.position = 0.0;
                        self.seek(0.0);
                        self.params.loop_count = self.params.loop_max as i32;
                    } else {
                        self.position = self.params.loop_start;
                        self.seek(self.position);
                        self.metronome_played = -1;
                    }
                }
            } else if self.position > self.params.song_length + 16.0 {
                self.params.playing = false;
                self.position = self.params.song_length + 16.0;
            }
        }
        self.frames_rendered += 1;
        self.mixer.render_sample(&self.bank)
    }

    /// 渲染到交错输出 buffer（cpal 回调直接调用）。
    pub fn render_interleaved(&mut self, out: &mut [f32], channels: usize) {
        let frames = out.len() / channels.max(1);
        for f in 0..frames {
            let (l, r) = self.render_frame();
            let base = f * channels;
            if channels == 1 {
                out[base] = (l + r) * 0.5;
            } else {
                out[base] = l;
                out[base + 1] = r;
                for c in 2..channels {
                    out[base + c] = (l + r) * 0.5;
                }
            }
        }
    }
}

/// UI 侧看到的播放时钟。所有写操作都代理到共享状态，读操作每帧 `update()` 同步。
#[derive(Clone)]
pub struct TransportClock {
    pub shared: Option<Shared>,
    last: Instant,
    position: f64,
    tempo: f32,
    speed: f32,
    playing: bool,
    loop_enabled: bool,
    loop_start: f64,
    loop_end: f64,
}

impl Default for TransportClock {
    fn default() -> Self {
        Self {
            shared: None,
            last: Instant::now(),
            position: 0.0,
            tempo: 10.0,
            speed: 1.0,
            playing: false,
            loop_enabled: false,
            loop_start: 0.0,
            loop_end: 0.0,
        }
    }
}

impl TransportClock {
    pub fn with_shared(shared: Shared) -> Self {
        let mut s = Self::default();
        s.shared = Some(shared);
        s
    }

    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }
    pub fn tempo(&self) -> f32 {
        self.tempo
    }

    pub fn set_tempo(&mut self, tempo: f32) {
        self.tempo = tempo.max(0.01);
        if let Some(s) = &self.shared {
            s.lock().params.tempo = self.tempo;
        }
    }
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 4.0);
        if let Some(s) = &self.shared {
            s.lock().params.speed = self.speed;
        }
    }
    pub fn set_master(&mut self, v: f32) {
        if let Some(s) = &self.shared {
            s.lock().params.master = v.clamp(0.0, 2.0);
        }
    }
    pub fn set_metronome(&mut self, on: bool) {
        if let Some(s) = &self.shared {
            s.lock().params.metronome = on;
        }
    }
    pub fn set_max_voices(&mut self, n: usize) {
        if let Some(s) = &self.shared {
            let mut st = s.lock();
            st.params.max_voices = n;
            st.mixer.set_max_voices(n);
        }
    }
    pub fn set_mono(&mut self, mono: bool) {
        if let Some(s) = &self.shared {
            let mut st = s.lock();
            st.params.mono = mono;
            st.mixer.mono = mono;
        }
    }
    pub fn set_include_locked(&mut self, include: bool, song: &Song) {
        if let Some(s) = &self.shared {
            let mut st = s.lock();
            st.params.include_locked = include;
            st.refresh_events(song);
        }
    }
    pub fn set_loop(&mut self, enabled: bool, start: f64, end: f64) {
        self.loop_enabled = enabled;
        self.loop_start = start;
        self.loop_end = end;
        if let Some(s) = &self.shared {
            let mut st = s.lock();
            st.params.loop_enabled = enabled;
            if start >= 0.0 {
                st.params.loop_start = start;
            }
            if end > 0.0 {
                st.params.loop_end = end;
            }
        }
    }
    pub fn seek(&mut self, tick: f64) {
        self.position = tick.max(0.0);
        self.last = Instant::now();
        if let Some(s) = &self.shared {
            s.lock().seek(self.position);
        }
    }
    pub fn play(&mut self) {
        self.playing = true;
        self.last = Instant::now();
        if let Some(s) = &self.shared {
            let mut st = s.lock();
            st.params.playing = true;
            st.seek(self.position);
        }
    }
    pub fn pause(&mut self) {
        self.playing = false;
        if let Some(s) = &self.shared {
            s.lock().params.playing = false;
        }
    }
    pub fn toggle(&mut self) {
        if self.playing { self.pause() } else { self.play() }
    }
    pub fn stop_and_rewind(&mut self) {
        self.playing = false;
        self.position = 0.0;
        if let Some(s) = &self.shared {
            let mut st = s.lock();
            st.params.playing = false;
            st.seek(0.0);
            st.mixer.clear();
        }
    }

    /// 每帧调用一次：同步音频线程的位置，或在无音频设备时用内部时钟推进。
    pub fn update(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().min(0.25);
        self.last = now;

        if let Some(shared) = &self.shared {
            let audio_active = shared.lock().audio_active;
            if audio_active {
                let st = shared.lock();
                let prev = self.position.floor() as i64;
                self.position = st.position;
                self.playing = st.params.playing;
                self.tempo = st.params.tempo;
                self.speed = st.params.speed;
                return self.position.floor() as i64 != prev;
            }
        }

        if !self.playing {
            return false;
        }
        let prev = self.position.floor() as i64;
        self.position += dt * self.tempo as f64 / self.speed as f64;
        if self.loop_enabled && self.loop_end > self.loop_start && self.position > self.loop_end {
            self.position = self.loop_start;
        }
        if let Some(shared) = &self.shared {
            shared.lock().position = self.position;
        }
        self.position.floor() as i64 != prev
    }
}

/// 便捷：创建一个共享状态。
pub fn make_shared(bank: SoundBank, sample_rate: u32) -> Shared {
    Arc::new(Mutex::new(SharedState::new(Arc::new(bank), sample_rate)))
}
