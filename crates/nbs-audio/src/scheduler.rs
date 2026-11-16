//! 播放调度（C 模块）与 UI/音频线程共享状态。
//!
//! 原版把 `marker_pos += tempo/room_speed` 放在 Draw 帧里（虽有 delta_time 补偿，
//! 仍与 UI 帧绑定）。这里改成**音频回调按采样推进**：每个输出采样推进
//! `tempo / speed / sample_rate` 个 tick，跨过整数 tick 时派发该列音符。
//! 掉帧、窗口拖拽、后台渲染都不会让节奏跑偏。

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use nbs_core::model::{Note, Song, FIRST_CUSTOM_INDEX};

use crate::mixer::{Mixer, DEFAULT_MAX_VOICES};
use crate::samples::SoundBank;

/// 播放到该列时要做的事。事件乐器不发声音——原版 control_step.gml:283-291
/// 的 else-if 链在 `play_sound` 之前就把它们拦截了。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Note,
    /// Tempo Changer：pitch 即 BPM。派发时 `tempo = floor(|pitch|)/15`
    /// （control_step.gml:284；0.25 t/s 下限来自 control_draw.gml:427）。
    Tempo(i16),
    /// Sound Stopper：`floor(pitch)` 起、`panning_velocity_to_short(pan, vel)` 止的
    /// 1 基层号闭区间消音（control_step.gml:286）。pitch<=0 全部消音；
    /// end<start 是「停用」消音器，不动任何发声。
    Stopper(i32, u32),
}

#[derive(Clone, Debug)]
pub struct NoteEvent {
    pub tick: u32,
    pub layer: u32,
    pub note: Note,
    pub layer_volume: u8,
    pub layer_stereo: u8,
    pub kind: EventKind,
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

/// 事件乐器识别：只有自定义乐器（索引 >= 20）才可能是它，原版按名字精确比较
/// （control_step.gml:283-291）。这里必须用 checked_sub：saturating_sub 会把内置的
/// 0..19 全夹成 0 号自定义，于是每个普通音符都来变速，pitch=0 时 tempo 直接被
/// 钉死在 0.25 t/s（整首歌慢到听不出来）。
fn event_kind(song: &Song, note: &Note) -> EventKind {
    note.instrument
        .checked_sub(FIRST_CUSTOM_INDEX)
        .and_then(|i| song.custom_instruments.get(i as usize))
        .map_or(EventKind::Note, |ci| {
            if ci.name == nbs_core::TEMPO_CHANGER_NAME {
                EventKind::Tempo(note.pitch)
            } else if ci.name == nbs_core::SOUND_STOPPER_NAME {
                EventKind::Stopper(
                    i32::from(note.pitch),
                    nbs_core::model::panning_velocity_to_short(note.panning, note.velocity),
                )
            } else {
                EventKind::Note
            }
        })
}

/// 把整首歌展开成按 tick 排序的事件表（实时与离线共用）。
/// 锁定/独奏屏蔽的层默认排除，但**事件乐器例外**：原版只有最后的
/// `play_sound` 被 a 闸住（control_step.gml:249-281），变速/消音分支在链上
/// 先于它，锁定层的 Tempo Changer / Sound Stopper 照样生效。
pub fn build_event_table(song: &Song, include_locked: bool) -> Vec<NoteEvent> {
    let locked = song.locked_layers();
    let mut events: Vec<NoteEvent> = song
        .iter_notes()
        .filter(|&(_, layer, note)| {
            include_locked
                || !locked.get(layer as usize).copied().unwrap_or(false)
                || event_kind(song, &note) != EventKind::Note
        })
        .map(|(tick, layer, note)| {
            let (layer_volume, layer_stereo) = song
                .layers
                .get(layer as usize)
                .map(|l| (l.volume, l.stereo))
                .unwrap_or((100, 100));
            let kind = event_kind(song, &note);
            NoteEvent { tick, layer, note, layer_volume, layer_stereo, kind }
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
                self.mixer.max_voices = self.params.max_voices;
                self.mixer.master = self.params.master;
                self.mixer.mono = self.params.mono;
                self.mixer.output_rate = self.sample_rate;
                match ev.kind {
                    EventKind::Tempo(pitch) => {
                        // pitch 是整数，floor(|pitch|)/15 就是 |pitch|/15
                        self.params.tempo =
                            (pitch.unsigned_abs() as f32 / 15.0).max(0.25);
                    }
                    EventKind::Stopper(first, last) => self.mixer.stop_range(first, last),
                    EventKind::Note => {
                        // voice 层号 1 基（原版 play_sound(..., b + 1)）
                        self.mixer.spawn(
                            &ev.note,
                            ev.layer_volume,
                            ev.layer_stereo,
                            &self.bank,
                            ev.layer + 1,
                        );
                    }
                }
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
            self.mixer.spawn(&click, 100, 100, &self.bank, 0);
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
    /// 内容变化后重建事件表；保留播放头位置，也不清空正在发声的 voice。
    pub fn refresh_events(&mut self, song: &Song) {
        if let Some(s) = &self.shared {
            s.lock().refresh_events(song);
        }
    }
    pub fn set_loop(&mut self, enabled: bool, start: f64, end: f64, max: u8) {
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
            st.params.loop_max = max;
            st.params.loop_count = max as i32;
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

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Note;

    fn song_ticks(ticks: u32) -> Song {
        let mut song = Song::new();
        for t in 0..ticks {
            song.set(
                t,
                0,
                Note { instrument: 0, key: 45, velocity: 80, panning: 100, pitch: 0 },
            );
        }
        song.recalculate();
        song
    }

    /// 逐采样推进；返回「真正回绕到循环起点的次数」和结束时是否仍在播放。
    fn drive(st: &mut SharedState, frames: usize) -> (usize, bool) {
        let mut wraps = 0usize;
        let mut prev = st.position;
        for _ in 0..frames {
            st.render_frame();
            if st.position < prev && st.params.playing {
                wraps += 1;
            }
            prev = st.position;
            if !st.params.playing {
                break;
            }
        }
        (wraps, st.params.playing)
    }

    fn looping_state(loop_max: u8) -> SharedState {
        let song = song_ticks(4);
        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        st.prepare(&song, Arc::new(SoundBank::default()));
        // 1 tick / sample：4 tick 的歌每轮 4 个采样。
        st.params.tempo = 44100.0;
        st.params.loop_enabled = true;
        st.params.playing = true;
        st.params.loop_max = loop_max;
        st.params.loop_count = loop_max as i32;
        st
    }

    #[test]
    fn loop_max_counts_extra_passes() {
        let mut st = looping_state(1);
        let (wraps, playing) = drive(&mut st, 200);
        assert_eq!(wraps, 1, "循环次数 1 = 多播一轮后停止");
        assert!(!playing, "用完循环次数必须停止播放");
    }

    #[test]
    fn loop_max_zero_loops_forever() {
        let mut st = looping_state(0);
        let (wraps, playing) = drive(&mut st, 200);
        assert!(wraps >= 10, "无限循环应不断回绕，实测 {wraps} 次");
        assert!(playing, "无限循环不该停止");
    }

    #[test]
    fn set_loop_pushes_loop_max_to_shared_params() {
        let shared = make_shared(SoundBank::default(), 44100);
        let mut clock = TransportClock::with_shared(shared.clone());
        clock.set_loop(true, 1.0, 9.0, 3);
        let st = shared.lock();
        assert!(st.params.loop_enabled);
        assert_eq!(st.params.loop_start, 1.0);
        assert_eq!(st.params.loop_end, 9.0);
        assert_eq!(st.params.loop_max, 3);
        assert_eq!(st.params.loop_count, 3);
    }

    /// 带一个「Tempo Changer」自定义乐器的歌：内置那 20 个音符绝不能被当成它。
    fn song_with_tempo_changer() -> Song {
        let mut song = Song::new();
        song.meta.tempo = 41.33;
        song.custom_instruments.push(nbs_core::model::CustomInstrument {
            name: "Tempo Changer".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        for (t, ins) in [(0u32, 0u8), (1, 6), (2, 16), (3, 17), (4, 19), (5, 20)] {
            song.set(
                t,
                0,
                Note { instrument: ins, key: 45, velocity: 80, panning: 100, pitch: if ins == 20 { 600 } else { 0 } },
            );
        }
        song.recalculate();
        song
    }

    #[test]
    fn only_real_tempo_changer_notes_carry_a_tempo_change() {
        let events = build_event_table(&song_with_tempo_changer(), false);
        let tc: Vec<u32> = events
            .iter()
            .filter(|e| e.kind == EventKind::Tempo(600))
            .map(|e| e.tick)
            .collect();
        assert_eq!(tc, vec![5], "只有索引 20 起的自定义 0 号才是 Tempo Changer，实测 {tc:?}");
    }

    /// 「Sound Stopper」自定义乐器：pitch/pan/vel 解码成层区间。
    /// pan=157, vel=100 编码出 end=57（ Living Millennium 第一枚消音器的真实编码）。
    #[test]
    fn sound_stopper_notes_decode_to_layer_range() {
        let mut song = Song::new();
        song.custom_instruments.push(nbs_core::model::CustomInstrument {
            name: "Sound Stopper".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        // start=39（pitch），end=57（pan/vel 编码）
        song.set(10, 3, Note { instrument: 20, key: 45, velocity: 100, panning: 157, pitch: 39 });
        song.set(20, 4, Note { instrument: 20, key: 45, velocity: 100, panning: 100, pitch: -1 });
        song.recalculate();
        let events = build_event_table(&song, false);
        assert_eq!(events[0].kind, EventKind::Stopper(39, 57));
        assert_eq!(events[1].kind, EventKind::Stopper(-1, 0), "负 pitch → layerid<0 → 全消音");
    }

    /// 原版只有 play_sound 被 a 闸住：锁定层上的消音器/变速器照样进事件表。
    #[test]
    fn locked_layer_event_instruments_still_fire() {
        let mut song = Song::new();
        song.custom_instruments.push(nbs_core::model::CustomInstrument {
            name: "Sound Stopper".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        song.set(10, 0, Note::new(0, 45));
        song.set(20, 0, Note { instrument: 20, key: 45, velocity: 100, panning: 157, pitch: 39 });
        song.recalculate();
        song.layers[0].lock = nbs_core::model::LockMode::Locked;
        let events = build_event_table(&song, false);
        assert_eq!(events.len(), 1, "锁定层普通音符被剔除");
        assert_eq!(events[0].kind, EventKind::Stopper(39, 57), "锁定层消音器保留");
        // include_locked 时全部保留
        assert_eq!(build_event_table(&song, true).len(), 2);
    }

    /// 端到端：播放跨过消音器列后，区间内更早发声的 voice 被掐，
    /// 区间外的 voice 保留。
    #[test]
    fn stopper_silences_spawned_voices_when_playhead_crosses() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut song = Song::new();
        song.custom_instruments.push(nbs_core::model::CustomInstrument {
            name: "Sound Stopper".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        // 层 0/1/2 在 tick 1 发声（voice 层号 1/2/3），tick 5 层 1 上消音器掐 [2,57]。
        // 注：调度窗口从 floor(prev)+1 起，tick 0 不会派发（起始列偏差，另案）。
        song.set(1, 0, Note::new(0, 45));
        song.set(1, 1, Note::new(0, 45));
        song.set(1, 2, Note::new(0, 45));
        song.set(5, 1, Note { instrument: 20, key: 45, velocity: 100, panning: 157, pitch: 2 });
        song.recalculate();
        let mut st = SharedState::new(Arc::new(bank.clone()), 44100);
        st.prepare(&song, Arc::new(bank));
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        // tempo=44100 → 每采样 1 tick：6 个采样跨过 tick 1（发声）和 tick 5（消音）
        st.params.tempo = 44100.0;
        drive(&mut st, 6);
        let alive: Vec<u32> = st.mixer.voices.iter().map(|v| v.layer).collect();
        assert_eq!(alive, vec![1], "消音器只掐 voice 层 2/3，层 1 保留，实测 {alive:?}");
    }

    /// 回归：这行曾经把内置音符的 pitch=0 当 BPM 用，tempo 被钉在 0.25 t/s，
    /// 「约 41 t/s 的歌放起来极慢」就是这么来的。
    #[test]
    fn builtin_notes_do_not_collapse_the_playback_tempo() {
        let song = song_with_tempo_changer();
        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        st.prepare(&song, Arc::new(SoundBank::default()));
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        // 一秒音频 = 44100 个采样，播放头应走到约 41.33 tick。
        drive(&mut st, 44100);
        assert!(st.position > 40.0, "一秒该走 ~41 tick，实测 {:.2}", st.position);
        assert!(
            (st.params.tempo - 40.0).abs() < 1e-3,
            "只允许 tick 5 那枚真正的 Tempo Changer 把 tempo 从 41.33 改成 600/15=40，实测 {:.2}",
            st.params.tempo
        );
    }
}
