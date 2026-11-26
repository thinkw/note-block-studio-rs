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
use crate::reference::ReferenceTrack;
use crate::samples::{SampleSource, SoundBank};

/// 播放到该列时要做的事。事件乐器不发声音——原版 control_step.gml:283-296
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
    /// Toggle Rainbow：切换界面强调色彩虹循环（control_step.gml:288）。
    Rainbow,
    /// Show Save Popup：弹「Song saved」消息（control_step.gml:289）。
    SavePopup,
    /// Change Color to #RRGGBB：重设界面强调色表（control_step.gml:290-293）。
    AccentColor([u8; 3]),
    /// Toggle Background Accent：切换窗口背景强调色（control_step.gml:296）。
    BackgroundAccent,
}

#[derive(Clone, Debug)]
pub struct NoteEvent {
    pub tick: u32,
    pub layer: u32,
    pub note: Note,
    pub layer_volume: u8,
    pub layer_stereo: u8,
    pub kind: EventKind,
    /// 原版 `song_ins[xx, b].press`：该列音符的乐器是否带「按下琴键」效果。
    pub press: bool,
}

/// 原版 `song_ins[xx, b].press`：内置乐器看建表时缺没缺第四个参数
/// （`control_create.gml:277-296`），自定义乐器用 .nbs 里那一字节。
fn instrument_press(song: &Song, index: u8) -> bool {
    match index.checked_sub(FIRST_CUSTOM_INDEX) {
        None => nbs_core::instruments::builtin_press(index),
        Some(i) => song
            .custom_instruments
            .get(i as usize)
            .is_some_and(|ci| ci.press),
    }
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
    /// 原版 `marker_end` + 区间末端：播放头自然越过这一刻就停在那里，负数 = 关。
    pub stop_at: f64,
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
            stop_at: -1.0,
        }
    }
}

/// 非循环播放越过歌尾多少刻后自动停（原版停在 `enda + cols`，一屏之后；
/// 这里取固定余量，停点即「播完」状态，再按播放键应回到 0）。
pub const END_STOP_MARGIN_TICKS: f64 = 16.0;

/// 事件乐器识别：只有自定义乐器（索引 >= 20）才可能是它，原版按名字精确比较
/// （control_step.gml:283-296）。这里必须用 checked_sub：saturating_sub 会把内置的
/// 0..19 全夹成 0 号自定义，于是每个普通音符都来变速，pitch=0 时 tempo 直接被
/// 钉死在 0.25 t/s（整首歌慢到听不出来）。
fn event_kind(song: &Song, note: &Note) -> EventKind {
    note.instrument
        .checked_sub(FIRST_CUSTOM_INDEX)
        .and_then(|i| song.custom_instruments.get(i as usize))
        .map_or(EventKind::Note, |ci| {
            // 链的顺序照抄原版：精确名在前，「Change Color to #」按小写包含
            // 且恰好一次才算（string_count(...) = 1），都未命中就是普通音符。
            if ci.name == TEMPO_CHANGER_NAME {
                EventKind::Tempo(note.pitch)
            } else if ci.name == RAINBOW_TOGGLE_NAME {
                EventKind::Rainbow
            } else if ci.name == SOUND_STOPPER_NAME {
                EventKind::Stopper(
                    i32::from(note.pitch),
                    nbs_core::model::panning_velocity_to_short(note.panning, note.velocity),
                )
            } else if ci.name == SAVE_POPUP_NAME {
                EventKind::SavePopup
            } else if let Some(rgb) = accent_change_rgb(&ci.name) {
                EventKind::AccentColor(rgb)
            } else if ci.name == BACKGROUND_ACCENT_NAME {
                EventKind::BackgroundAccent
            } else {
                EventKind::Note
            }
        })
}

/// 原版 `toggle_playing` / `control_step` else-if 链上的精确名（区分大小写）。
const TEMPO_CHANGER_NAME: &str = nbs_core::TEMPO_CHANGER_NAME;
const SOUND_STOPPER_NAME: &str = nbs_core::SOUND_STOPPER_NAME;
const RAINBOW_TOGGLE_NAME: &str = "Toggle Rainbow";
const SAVE_POPUP_NAME: &str = "Show Save Popup";
const BACKGROUND_ACCENT_NAME: &str = "Toggle Background Accent";

/// 「Change Color to #RRGGBB」变奏名：原版判定是
/// `string_count(string_lower("Change Color to #"), string_lower(name)) = 1`，
/// RGB 取原名的第 18/20/22 个字符起各两位（string_copy 1 基），按十六进制
/// `real("0x"+..)` 解析。这里缺位或非法按 0 处理（原版 real() 会报错，
/// 但报错对话框不是可移植行为）。
fn accent_change_rgb(name: &str) -> Option<[u8; 3]> {
    const PREFIX: &str = "change color to #";
    if name.to_lowercase().matches(PREFIX).count() != 1 {
        return None;
    }
    let chars: Vec<char> = name.chars().collect();
    let hex = |at: usize| -> u8 {
        let s: String = chars
            .get(at..at + 2)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        u8::from_str_radix(&s, 16).unwrap_or(0)
    };
    Some([hex(17), hex(19), hex(21)])
}

/// 调度侧「只改界面、不改声部」的副作用：事件乐器里那四种（原版在游戏进程里
/// 直接改全局变量），外加演奏到带 press 的音符时对琴键打的那一下时间戳。
/// 见 [`SharedState::take_visual_events`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualEvent {
    RainbowToggle,
    SavePopup,
    AccentColor([u8; 3]),
    BackgroundAccentToggle,
    /// 原版 `key_played[key] = current_time`（control_step.gml:295）：这枚音符的乐器
    /// 带 press，UI 侧据此让对应琴键在 300ms 内染黄并弹下。
    KeyPlayed(u8),
}

/// 防爆队列上限：正常人手写不出一秒几千枚事件乐器，超了就丢弃最旧的副作用。
const VISUAL_EVENT_CAP: usize = 4096;

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
            let press = instrument_press(song, note.instrument);
            NoteEvent {
                tick,
                layer,
                note,
                layer_volume,
                layer_stereo,
                press,
                kind,
            }
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
    /// 当前标签的参考音频轨；切标签时由编辑器重挂（用户要求 #131）。
    pub reference: ReferenceTrack,
    pub sample_rate: u32,
    /// 是否由真实音频回调推进（false 时 UI 侧内部时钟代理）。
    pub audio_active: bool,
    /// 输出流的错误回调记下的「这条流废了，请重建」。音频线程里只许记这一笔——
    /// 拆流建流都在界面线程下一帧做（在回调里动自己那条流会自己等自己）。
    /// 和 `audio_active` 分开是因为后端会投「抖一下、我自己在恢复」那一类事件，
    /// 那种既不该停表也不该重建。
    pub stream_needs_rebuild: bool,
    pub frames_rendered: u64,
    /// 改界面的事件乐器副作用，等 UI 线程每帧取走。
    pub visual_events: Vec<VisualEvent>,
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
            // 实时状态自带原版后端的不精确起点对齐；导出路径不走 Mixer，
            // 保持 nbswave 的精确时间轴。
            mixer: Mixer::new(sample_rate).with_tick_jitter(crate::mixer::REALTIME_TICK_JITTER_MS),
            bank,
            reference: ReferenceTrack::default(),
            sample_rate,
            audio_active: false,
            stream_needs_rebuild: false,
            frames_rendered: 0,
            visual_events: Vec::new(),
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

    /// 原版 `get_tempo_from_tick`：变速点表里 ≤ tick 的最后一个速度。
    /// 表 = 初始速度 + 每枚 Tempo Changer（同 tick 最后一枚生效），
    /// 每枚速度 = max(0.25, |pitch|/15)。锁定层的变速器照常参与
    /// （update_tempo_changes 不看层锁）。
    pub fn tempo_at_tick(&self, tick: i64) -> f32 {
        let mut tempo = self.params.tempo.max(0.25);
        for ev in &self.events {
            if ev.tick as i64 > tick {
                break;
            }
            if let EventKind::Tempo(pitch) = ev.kind {
                tempo = (pitch.unsigned_abs() as f32 / 15.0).max(0.25);
            }
        }
        tempo
    }

    /// 原版 `get_seconds_from_tick`：逐段累加变速器之间的 tick 时长。
    pub fn seconds_at(&self, tick: f64) -> f64 {
        let tick = tick.max(0.0);
        let mut segment_start = 0.0;
        let mut tempo = self.params.tempo.max(0.25) as f64;
        let mut seconds = 0.0;
        for ev in &self.events {
            let at = ev.tick as f64;
            if at > tick {
                break;
            }
            if let EventKind::Tempo(pitch) = ev.kind {
                seconds += (at - segment_start) / tempo;
                segment_start = at;
                tempo = (pitch.unsigned_abs() as f64 / 15.0).max(0.25);
            }
        }
        seconds + (tick - segment_start) / tempo
    }

    /// 原版 `toggle_playing` 的 PLAY COL：按播放键取落点列的速度，再把该列
    /// 整列派发一遍（变速器 / 消音器 / 音符）。播放中途大幅跳转或循环回绕时
    /// control_step 的 diff 分支（diff<0 或 diff>3）补播的也是这个列。
    /// 调用方需已 [`Self::seek`] 到落点；这里不改 cursor，后续跨列照常。
    pub fn dispatch_landing_column(&mut self, tick: f64) {
        let t = tick.floor();
        if t < 0.0 {
            return;
        }
        self.params.tempo = self.tempo_at_tick(t as i64);
        self.dispatch_column(t as i64);
    }

    /// 派发单列全部事件（共享 [`Self::trigger`] 的分流与闸门语义）。
    fn dispatch_column(&mut self, column: i64) {
        for index in 0..self.events.len() {
            let ev = self.events[index].clone();
            if ev.tick as i64 != column {
                continue;
            }
            self.apply_event(&ev);
        }
    }

    /// else-if 链的公共尾（control_step.gml:283-296）：改参数 / 消音 / 发声 /
    /// 发视觉事件。`trigger` 与落点列派发都走这里，语义只此一份。
    fn apply_event(&mut self, ev: &NoteEvent) {
        self.mixer.max_voices = self.params.max_voices;
        self.mixer.master = self.params.master;
        self.mixer.mono = self.params.mono;
        self.mixer.output_rate = self.sample_rate;
        match ev.kind {
            EventKind::Tempo(pitch) => {
                // pitch 是整数，floor(|pitch|)/15 就是 |pitch|/15
                self.params.tempo = (pitch.unsigned_abs() as f32 / 15.0).max(0.25);
            }
            EventKind::Stopper(first, last) => self.mixer.stop_range(first, last),
            EventKind::Rainbow => self.push_visual(VisualEvent::RainbowToggle),
            EventKind::SavePopup => self.push_visual(VisualEvent::SavePopup),
            EventKind::AccentColor(rgb) => self.push_visual(VisualEvent::AccentColor(rgb)),
            EventKind::BackgroundAccent => self.push_visual(VisualEvent::BackgroundAccentToggle),
            EventKind::Note => {
                // voice 层号 1 基（原版 play_sound(..., b + 1)）
                self.mixer.spawn(
                    &ev.note,
                    ev.layer_volume,
                    ev.layer_stereo,
                    &self.bank,
                    ev.layer + 1,
                    ev.tick as i64,
                );
                // 原版 control_step.gml:295：`if (song_ins.press || isplayer)
                // key_played[song_key] = current_time`。它和 play_sound 是两条独立
                // 的闸门——采样没加载、力度为 0 也照样给琴键打时间戳，所以这里不看
                // mixer 有没有真的出声。播放器模式（isplayer，#74）端口未接线，
                // 那个 `|| isplayer` 分支没有对应实现。
                if ev.press {
                    self.push_visual(VisualEvent::KeyPlayed(ev.note.key));
                }
            }
        }
    }

    /// 界面副作用入队；超上限丢新事件（不阻塞音频回调）。
    fn push_visual(&mut self, ev: VisualEvent) {
        if self.visual_events.len() < VISUAL_EVENT_CAP {
            self.visual_events.push(ev);
        }
    }

    /// UI 每帧取走积累的视觉事件（改色 / 彩虹 / 存档弹窗 / 背景强调）。
    pub fn take_visual_events(&mut self) -> Vec<VisualEvent> {
        std::mem::take(&mut self.visual_events)
    }

    /// 处理 tick 跨越时的事件派发。
    fn trigger(&mut self, from: i64, to: i64) {
        if to < from {
            return;
        }
        let diff = to - from;
        while self.cursor < self.events.len() {
            let ev = self.events[self.cursor].clone();
            let t = ev.tick as i64;
            if t > to {
                break;
            }
            // 卡顿/跳转跨过 >3 列时，只补播最后一列（原版行为）
            let should_play = (diff <= 3 && t >= from) || (diff > 3 && t == to);
            if should_play {
                self.apply_event(&ev);
            }
            self.cursor += 1;
        }
    }

    /// 原版 `control_draw.gml:991-1010` 的节拍器段：每 4 tick 一响，小节第一拍
    /// 换成 `soundmetronomeclick`，其余拍用 `soundmetronome`，两边都是
    /// `play_sound(ins, 45, 100, 100, 0)`；采样没载到就不响（`if (ins.loaded)`）。
    fn trigger_metronome(&mut self, position: f64) {
        if !self.params.metronome {
            return;
        }
        // 原版按 room_speed 挑偏移（>= 60 用 -0.1，30 帧用 -0.4）。端口的位置是
        // 按音频回调连续推进的，没有「一帧一跳」的量化，恒走 -0.1 那一支。
        let mut pos = (position - 0.1).floor() as i64;
        // 原版 `if (current_song.tempo = 30) pos -= 1`：30 t/s（= 450 BPM）时让开一列。
        if self.params.tempo == 30.0 {
            pos -= 1;
        }
        if pos < 0 || pos % 4 != 0 || self.metronome_played >= pos {
            return;
        }
        // 原版 `pos mod (4 * timesignature) == 0`：每小节第一拍。
        let bar = 4i64 * i64::from(self.params.time_signature.max(1));
        let accent = bar > 0 && pos.rem_euclid(bar) == 0;
        // 循环播放时掐掉歌尾那一拍的重音（原版 `if (!loop || pos < enda + 1)`）：
        // 回绕后紧接着又是一拍重音，两响会叠在一起。闸门只挡发声，
        // metronome_played 照旧推进 —— 原版它就写在 if 外面。
        let in_loop = self.params.loop_enabled;
        let at_song_end = pos >= self.params.song_length as i64;
        if !accent || !in_loop || !at_song_end {
            let source = if accent {
                SampleSource::MetronomeClick
            } else {
                SampleSource::Metronome
            };
            let mut click = Note::new(0, 45);
            click.velocity = 100;
            self.mixer.master = self.params.master;
            self.mixer
                .spawn_source(source, &click, 100, 100, &self.bank, 0, -1);
        }
        self.metronome_played = pos + 1;
    }

    /// 渲染一个输出采样帧（同时推进调度时钟）。
    pub fn render_frame(&mut self) -> (f32, f32) {
        if self.params.playing {
            let prev = self.position;
            let tick_per_sample = self.params.tempo as f64
                / self.params.speed.max(0.01) as f64
                / self.sample_rate.max(1) as f64;
            self.position += tick_per_sample;

            let from = prev.floor() as i64 + 1;
            let to = self.position.floor() as i64;
            // 原版 `control_draw.gml:1033-1036` 把播放头钉在区间末端并停下，
            // `control_step.gml:243` 那一大串或条件里唯一为假的情形就是「自然播放刚越过
            // 区间末端这一列」——那一列不发声。手动定位/拖播放头（原版 `forward <> 0`、
            // `window = w_dragmarker`）不在闸门里，照常响。
            let stop_at = self.params.stop_at;
            let hitting = stop_at >= 0.0 && prev < stop_at && self.position >= stop_at;
            let to_notes = if hitting {
                to.min(stop_at.floor() as i64 - 1)
            } else {
                to
            };
            if to_notes >= from {
                self.trigger(from, to_notes);
            }
            if to >= from {
                self.trigger_metronome(self.position);
            }
            if hitting {
                self.position = stop_at;
                self.params.playing = false;
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
                        // 原版回绕后 diff<0，control_step 补播落点列
                        self.dispatch_landing_column(self.position);
                    }
                }
            } else if self.position > self.params.song_length + END_STOP_MARGIN_TICKS {
                self.params.playing = false;
                self.position = self.params.song_length + END_STOP_MARGIN_TICKS;
            }
        }
        self.frames_rendered += 1;
        self.mixer.render_sample(&self.bank)
    }

    /// 渲染到交错输出 buffer（cpal 回调直接调用）。
    pub fn render_interleaved(&mut self, out: &mut [f32], channels: usize) {
        let frames = out.len() / channels.max(1);
        for f in 0..frames {
            let (mut l, mut r) = self.render_frame();
            let reference = self.reference.render_sample(self.sample_rate);
            if self.reference.is_solo() {
                l = 0.0;
                r = 0.0;
            }
            if !self.reference.is_muted() {
                l += reference;
                r += reference;
            }
            let base = f * channels;
            if self.reference.is_loaded() {
                if channels == 1 {
                    out[base] = ((l + r) * 0.5).clamp(-1.0, 1.0);
                } else {
                    out[base] = l.clamp(-1.0, 1.0);
                    out[base + 1] = r.clamp(-1.0, 1.0);
                    for c in 2..channels {
                        out[base + c] = ((l + r) * 0.5).clamp(-1.0, 1.0);
                    }
                }
            } else if channels == 1 {
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
    stop_at: f64,
}

/// 播放时钟的一份可跨线程快照。真实音频走 [`SharedState`] 的原子读取；
/// 无设备时浮窗用这份快照继续推进，主窗恢复显示后再灌回 [`TransportClock`]。
#[derive(Clone, Copy, Debug)]
pub struct TransportSnapshot {
    pub position: f64,
    pub tempo: f32,
    pub speed: f32,
    pub playing: bool,
    pub loop_enabled: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub stop_at: f64,
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
            stop_at: -1.0,
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

    pub fn snapshot(&self) -> TransportSnapshot {
        TransportSnapshot {
            position: self.position,
            tempo: self.tempo,
            speed: self.speed,
            playing: self.playing,
            loop_enabled: self.loop_enabled,
            loop_start: self.loop_start,
            loop_end: self.loop_end,
            stop_at: self.stop_at,
        }
    }

    /// 把浮窗在后台推进出来的播放头接回主时钟。只在无音频共享状态时用；
    /// 真实音频线程的位置不由 UI 覆盖。
    pub fn restore(&mut self, snapshot: TransportSnapshot) {
        self.position = snapshot.position.max(0.0);
        self.tempo = snapshot.tempo.max(0.01);
        self.speed = snapshot.speed.clamp(0.25, 4.0);
        self.playing = snapshot.playing;
        self.loop_enabled = snapshot.loop_enabled;
        self.loop_start = snapshot.loop_start;
        self.loop_end = snapshot.loop_end;
        self.stop_at = snapshot.stop_at;
        self.last = Instant::now();
        if let Some(shared) = &self.shared {
            let mut st = shared.lock();
            st.position = self.position;
            st.params.tempo = self.tempo;
            st.params.speed = self.speed;
            st.params.playing = self.playing;
            st.params.loop_enabled = self.loop_enabled;
            st.params.loop_start = self.loop_start;
            st.params.loop_end = self.loop_end;
            st.params.stop_at = self.stop_at;
        }
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
    /// 原版 `marker_end` 的那道闸：负数 = 关，非负 = 播放头自然越过这一刻就停。
    pub fn set_stop_at(&mut self, tick: f64) {
        self.stop_at = tick;
        if let Some(s) = &self.shared {
            s.lock().params.stop_at = tick;
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
        if self.playing {
            self.pause()
        } else {
            self.play()
        }
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
        let before = self.position;
        self.position += dt * self.tempo as f64 / self.speed as f64;
        if self.loop_enabled && self.loop_end > self.loop_start && self.position > self.loop_end {
            self.position = self.loop_start;
        }
        // 无音频设备时也要有原版 `marker_end` 的停点（这条路径不出声，只管播放头）。
        if self.stop_at >= 0.0 && before < self.stop_at && self.position >= self.stop_at {
            self.position = self.stop_at;
            self.playing = false;
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
    use crate::samples::Sample;
    use nbs_core::model::Note;

    fn song_ticks(ticks: u32) -> Song {
        let mut song = Song::new();
        for t in 0..ticks {
            song.set(
                t,
                0,
                Note {
                    instrument: 0,
                    key: 45,
                    velocity: 80,
                    panning: 100,
                    pitch: 0,
                },
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

    /// 原版 `marker_end` 那一档：播放头自然越过区间末端就钉在那里停下，
    /// 并且末端那一列不发声（`control_draw.gml:1033-1036` 定停点、
    /// `control_step.gml:243` 掐发声列）。
    #[test]
    fn stop_at_pins_the_playhead_and_mutes_that_column() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut song = Song::new();
        for t in [2u32, 4, 6] {
            song.set(t, 0, Note::new(0, 45));
        }
        song.recalculate();
        let mut st = SharedState::new(Arc::new(bank.clone()), 44100);
        st.prepare(&song, Arc::new(bank));
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        // 每采样 1 tick：2、4 两列先响，跨过 6 时命中停点。
        st.params.tempo = 44100.0;
        st.params.stop_at = 6.0;
        drive(&mut st, 20);
        assert_eq!(st.position, 6.0, "停点要钉在区间末端，实测 {}", st.position);
        assert!(!st.params.playing, "越过区间末端必须停止播放");
        assert_eq!(
            st.mixer.active(),
            2,
            "2、4 两列发声、末端那列不响，实测 {} 个 voice",
            st.mixer.active()
        );
    }

    /// 停点只挡「从下面越上来」：播放头已经停在区间末端再按播放，要从那里继续往下走，
    /// 否则一按播放就立刻自杀。
    #[test]
    fn stop_at_does_not_retrigger_from_the_stop_position() {
        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        let song = song_ticks(4);
        st.prepare(&song, Arc::new(SoundBank::default()));
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        st.params.tempo = 44100.0;
        st.params.stop_at = 6.0;
        st.seek(6.0);
        st.params.playing = true;
        drive(&mut st, 4);
        assert!(st.params.playing, "从停点原地起播不该立刻停");
        assert!(st.position > 6.0, "要能走过停点，实测 {}", st.position);
    }

    /// 带一个「Tempo Changer」自定义乐器的歌：内置那 20 个音符绝不能被当成它。
    fn song_with_tempo_changer() -> Song {
        let mut song = Song::new();
        song.meta.tempo = 41.33;
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Tempo Changer".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        for (t, ins) in [(0u32, 0u8), (1, 6), (2, 16), (3, 17), (4, 19), (5, 20)] {
            song.set(
                t,
                0,
                Note {
                    instrument: ins,
                    key: 45,
                    velocity: 80,
                    panning: 100,
                    pitch: if ins == 20 { 600 } else { 0 },
                },
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
        assert_eq!(
            tc,
            vec![5],
            "只有索引 20 起的自定义 0 号才是 Tempo Changer，实测 {tc:?}"
        );
    }

    /// 「Sound Stopper」自定义乐器：pitch/pan/vel 解码成层区间。
    /// pan=157, vel=100 编码出 end=57（ Living Millennium 第一枚消音器的真实编码）。
    #[test]
    fn sound_stopper_notes_decode_to_layer_range() {
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Sound Stopper".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        // start=39（pitch），end=57（pan/vel 编码）
        song.set(
            10,
            3,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 157,
                pitch: 39,
            },
        );
        song.set(
            20,
            4,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: -1,
            },
        );
        song.recalculate();
        let events = build_event_table(&song, false);
        assert_eq!(events[0].kind, EventKind::Stopper(39, 57));
        assert_eq!(
            events[1].kind,
            EventKind::Stopper(-1, 0),
            "负 pitch → layerid<0 → 全消音"
        );
    }

    /// 原版只有 play_sound 被 a 闸住：锁定层上的消音器/变速器照样进事件表。
    #[test]
    fn locked_layer_event_instruments_still_fire() {
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Sound Stopper".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(10, 0, Note::new(0, 45));
        song.set(
            20,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 157,
                pitch: 39,
            },
        );
        song.recalculate();
        song.layers[0].lock = nbs_core::model::LockMode::Locked;
        let events = build_event_table(&song, false);
        assert_eq!(events.len(), 1, "锁定层普通音符被剔除");
        assert_eq!(
            events[0].kind,
            EventKind::Stopper(39, 57),
            "锁定层消音器保留"
        );
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
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
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
        song.set(
            5,
            1,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 157,
                pitch: 2,
            },
        );
        song.recalculate();
        let mut st = SharedState::new(Arc::new(bank.clone()), 44100);
        st.prepare(&song, Arc::new(bank));
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        // tempo=44100 → 每采样 1 tick：6 个采样跨过 tick 1（发声）和 tick 5（消音）
        st.params.tempo = 44100.0;
        drive(&mut st, 6);
        let alive: Vec<u32> = st.mixer.voices.iter().map(|v| v.layer).collect();
        assert_eq!(
            alive,
            vec![1],
            "消音器只掐 voice 层 2/3，层 1 保留，实测 {alive:?}"
        );
    }

    /// 原版 toggle_playing：按播放键取落点列速度并整列派发——落点列的
    /// 变速器立刻生效、音符立刻发声，不必等跨过下一列。
    #[test]
    fn pressing_play_dispatches_landing_column() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Tempo Changer".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(
            3,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 600,
            },
        );
        song.set(3, 1, Note::new(0, 45));
        song.set(4, 0, Note::new(0, 45));
        song.recalculate();
        let mut st = SharedState::new(Arc::new(bank.clone()), 44100);
        st.prepare(&song, Arc::new(bank));
        st.seek(3.0);
        st.dispatch_landing_column(3.0);
        assert!(
            (st.params.tempo - 40.0).abs() < 1e-3,
            "落点列的变速器要立刻生效，实测 {}",
            st.params.tempo
        );
        assert_eq!(st.mixer.active(), 1, "落点列的音符要立刻发声");
        // 落点之后的列不能被误派发
        let layers: Vec<u32> = st.mixer.voices.iter().map(|v| v.layer).collect();
        assert_eq!(layers, vec![2], "只派发落点列（voice 层号 1 基）");
    }

    /// get_tempo_from_tick：变速点表里 ≤ tick 的最后一枚生效（同 tick 末枚
    /// 覆盖），锁定层变速器照常参与。
    #[test]
    fn tempo_at_tick_walks_the_changer_table() {
        let mut song = Song::new();
        song.meta.tempo = 41.33;
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Tempo Changer".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(
            2,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 600,
            },
        );
        song.set(
            5,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 300,
            },
        );
        song.layers.push(nbs_core::model::Layer::default());
        song.set(
            7,
            1,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 150,
            },
        );
        song.recalculate();
        song.layers[1].lock = nbs_core::model::LockMode::Locked;
        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        st.prepare(&song, Arc::new(SoundBank::default()));
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(
            near(st.tempo_at_tick(1), 41.33),
            "初始速度，实测 {}",
            st.tempo_at_tick(1)
        );
        assert!(near(st.tempo_at_tick(2), 40.0), "{}", st.tempo_at_tick(2));
        assert!(near(st.tempo_at_tick(4), 40.0), "{}", st.tempo_at_tick(4));
        assert!(near(st.tempo_at_tick(5), 20.0), "{}", st.tempo_at_tick(5));
        assert!(
            near(st.tempo_at_tick(7), 10.0),
            "锁定层变速器也进表，实测 {}",
            st.tempo_at_tick(7)
        );
        assert!(
            near(st.tempo_at_tick(100), 10.0),
            "{}",
            st.tempo_at_tick(100)
        );
        assert!((st.seconds_at(2.0) - 2.0 / 41.33).abs() < 1e-6);
        assert!((st.seconds_at(5.5) - (2.0 / 41.33 + 3.0 / 40.0 + 0.5 / 20.0)).abs() < 1e-6);
    }

    /// 循环回绕后 control_step 的 diff<0 分支补播落点列：回绕列上的
    /// 消音器要把上一轮残留的 voice 掐掉。
    #[test]
    fn loop_wrap_redispatches_landing_column() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Sound Stopper".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(
            0,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 157,
                pitch: 1,
            },
        );
        song.set(1, 0, Note::new(0, 45));
        song.set(1, 1, Note::new(0, 45));
        song.recalculate();
        let mut st = SharedState::new(Arc::new(bank.clone()), 44100);
        st.prepare(&song, Arc::new(bank));
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        st.params.tempo = 44100.0;
        st.params.loop_enabled = true;
        st.params.loop_start = 0.0;
        st.params.loop_end = 2.0;
        // 1 tick/采样：跨 tick 1 发声 2 枚 → 回绕到 0 → 落点列消音器掐光
        // （第 3 帧与第 6 帧各回绕一次，第 6 帧后应全灭）
        drive(&mut st, 6);
        assert_eq!(
            st.mixer.active(),
            0,
            "回绕后落点列的消音器必须掐掉残留 voice"
        );
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
        assert!(
            st.position > 40.0,
            "一秒该走 ~41 tick，实测 {:.2}",
            st.position
        );
        assert!(
            (st.params.tempo - 40.0).abs() < 1e-3,
            "只允许 tick 5 那枚真正的 Tempo Changer 把 tempo 从 41.33 改成 600/15=40，实测 {:.2}",
            st.params.tempo
        );
    }

    /// 四种改界面的事件乐器要按 else-if 链识别成对应 kind，且锁定层照常保留
    /// （原版只有 play_sound 被 a 闸住）。
    #[test]
    fn visual_instrument_names_parse_to_visual_kinds() {
        let mut song = Song::new();
        for name in [
            "Toggle Rainbow",
            "Show Save Popup",
            "Change Color to #FF8000",
            "TOGGLE RAINBOW",
            "Toggle Background Accent",
        ] {
            song.custom_instruments
                .push(nbs_core::model::CustomInstrument {
                    name: name.into(),
                    filename: String::new(),
                    key: 45,
                    press: false,
                });
        }
        song.set(
            0,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.set(
            1,
            1,
            Note {
                instrument: 21,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.set(
            2,
            2,
            Note {
                instrument: 22,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.set(
            3,
            3,
            Note {
                instrument: 23,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.set(
            4,
            4,
            Note {
                instrument: 24,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        while song.layers.len() < 5 {
            song.layers.push(nbs_core::model::Layer::default());
        }
        // 锁定层：只有「原版不被 a 闸住」的 kind 保留，普通音符（含变奏名）被滤掉。
        song.layers[1].lock = nbs_core::model::LockMode::Locked;
        song.layers[2].lock = nbs_core::model::LockMode::Locked;
        song.layers[3].lock = nbs_core::model::LockMode::Locked;
        song.recalculate();
        let events = build_event_table(&song, false);
        assert_eq!(
            events.len(),
            4,
            "锁定层上的视觉事件乐器保留，锁定层的普通音符滤掉"
        );
        assert_eq!(events[0].kind, EventKind::Rainbow);
        assert_eq!(events[1].kind, EventKind::SavePopup);
        assert_eq!(events[2].kind, EventKind::AccentColor([255, 128, 0]));
        assert_eq!(events[3].kind, EventKind::BackgroundAccent);
        assert!(
            events.iter().all(|e| e.kind != EventKind::Note),
            "TOGGLE RAINBOW 是变奏名不是精确名，按普通音符被锁定层闸门滤掉"
        );
    }

    /// 「Change Color to #」判定：小写包含且恰好一次；十六进制按**原名固定**
    /// 第 18/20/22 个字符起各取两位（string_copy 不看前缀在哪）；非法或缺位按 0。
    #[test]
    fn accent_change_rgb_parsing_rules() {
        assert_eq!(
            accent_change_rgb("Change Color to #FF8000"),
            Some([255, 128, 0])
        );
        // 前缀不在开头时照旧取固定位置——「please CHANGE…」的 18..24 字符是
        // 「OR TO 」，原版 real() 会报错，这里按 0 兜底。
        assert_eq!(
            accent_change_rgb("please CHANGE COLOR TO #ab12 now"),
            Some([0, 0, 0])
        );
        assert_eq!(
            accent_change_rgb("Change Color to #FF8000 and Change Color to #000000"),
            None,
            "包含两次不算"
        );
        assert_eq!(
            accent_change_rgb("Change Color to #zzzzzz"),
            Some([0, 0, 0])
        );
        assert_eq!(
            accent_change_rgb("Change Color to #"),
            Some([0, 0, 0]),
            "缺位按 0"
        );
        assert_eq!(accent_change_rgb("Toggle Rainbow"), None);
    }

    /// 播放头跨过视觉事件乐器所在列 → 副作用进共享队列，UI 侧 take 走后清空。
    #[test]
    fn visual_events_reach_the_queue_when_crossed() {
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Show Save Popup".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Toggle Rainbow".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(
            0,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.set(
            10,
            0,
            Note {
                instrument: 21,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.recalculate();
        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        st.prepare(&song, Arc::new(SoundBank::default()));
        // 按播放键：落点列 tick 0 整列过链（播放中 tick 0 永远不会被「跨过」）。
        st.dispatch_landing_column(0.0);
        st.params.playing = true;
        st.params.song_length = 100_000.0;
        st.params.tempo = 44100.0; // 1 tick/采样，快速跨列
        drive(&mut st, 15);
        assert_eq!(
            st.take_visual_events(),
            vec![VisualEvent::SavePopup, VisualEvent::RainbowToggle,],
            "跨过 tick 0 与 tick 10 各发一次"
        );
        assert!(st.take_visual_events().is_empty(), "take 之后必须清空");
    }

    /// 原版 `new_instrument` 的第四个参数：内置 20 种里只有 Bass Drum(2)、
    /// Snare Drum(3)、Click(4) 没传（`control_create.gml:279-281`），其余都是 true；
    /// 自定义乐器用 .nbs 里那一字节。只有带 press 的音符才让琴键跟着弹
    /// （`control_step.gml:295`）。
    #[test]
    fn press_flag_comes_from_the_instrument() {
        let mut song = Song::new();
        // 竖琴(0) 与长笛(6) 带 press；底鼓(2)、军鼓(3)、锤击(4) 不带。
        for (t, ins) in [(0u32, 0u8), (1, 2), (2, 3), (3, 4), (4, 6)] {
            song.set(
                t,
                0,
                Note {
                    instrument: ins,
                    key: 45 + t as u8,
                    velocity: 100,
                    panning: 100,
                    pitch: 0,
                },
            );
        }
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "My Synth".into(),
                filename: String::new(),
                key: 45,
                press: true,
            });
        song.set(
            5,
            0,
            Note {
                instrument: FIRST_CUSTOM_INDEX,
                key: 50,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.recalculate();
        let table = build_event_table(&song, false);
        let press: Vec<bool> = table.iter().map(|e| e.press).collect();
        assert_eq!(press, vec![true, false, false, false, true, true]);

        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        st.prepare(&song, Arc::new(SoundBank::default()));
        st.params.song_length = 100_000.0;
        st.params.tempo = 44100.0;
        st.params.playing = true;
        st.dispatch_landing_column(0.0);
        drive(&mut st, 8);
        // 落点列 + 跨过的 tick 1..=5：只有 0、4、5 这三列的键进队列。
        assert_eq!(
            st.take_visual_events(),
            vec![
                VisualEvent::KeyPlayed(45),
                VisualEvent::KeyPlayed(49),
                VisualEvent::KeyPlayed(50),
            ],
            "不带 press 的乐器不该让琴键动"
        );
    }

    /// 落点列（按播放键 / 循环回绕）的视觉事件乐器也要派发——原版 PLAY COL
    /// 把整列重新过一遍 else-if 链。
    #[test]
    fn landing_column_dispatches_visual_events() {
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Change Color to #0A0B0C".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(
            7,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 0,
            },
        );
        song.recalculate();
        let mut st = SharedState::new(Arc::new(SoundBank::default()), 44100);
        st.prepare(&song, Arc::new(SoundBank::default()));
        st.seek(7.0);
        st.dispatch_landing_column(7.0);
        assert_eq!(
            st.take_visual_events(),
            vec![VisualEvent::AccentColor([0x0a, 0x0b, 0x0c])]
        );
    }

    /// 原版节拍器两个采样是独立声音资产（`control_create.gml:316-327`），
    /// 两条采样给不同长度，好把「这一拍用了哪一个」读出来。
    fn metronome_bank() -> SoundBank {
        let mut bank = SoundBank::default();
        bank.samples.push(Sample {
            pcm: vec![0.0; 8],
            base_key: 45,
            name: "harp".into(),
        });
        bank.metronome = Some(Sample {
            pcm: vec![0.0; 16],
            base_key: 45,
            name: "metronome".into(),
        });
        bank.metronome_click = Some(Sample {
            pcm: vec![0.0; 32],
            base_key: 45,
            name: "metronome_click".into(),
        });
        bank
    }

    fn metronome_state(bank: SoundBank) -> SharedState {
        let mut st = SharedState::new(Arc::new(bank), 44100);
        st.params.metronome = true;
        st.params.time_signature = 4;
        st
    }

    /// 原版 `control_draw.gml:991-1010`：每 4 tick 一响，小节头（`pos mod
    /// 4*timesignature == 0`）用 metronome_click，其余用 metronome，
    /// 两边都是 `play_sound(ins, 45, 100, 100, 0)` = 力度 100。
    #[test]
    fn metronome_uses_its_own_samples_and_accents_the_bar_head() {
        let mut st = metronome_state(metronome_bank());
        st.trigger_metronome(0.5);
        assert_eq!(st.mixer.voices.len(), 1, "第 0 列是整小节，应有一记重音");
        let v = &st.mixer.voices[0];
        assert_eq!(v.source, SampleSource::MetronomeClick, "小节头用重音采样");
        // 力度 100、声道 100、master 1.0 → 左右各 0.5（旧的 60 力度会是 0.3）。
        assert_eq!(
            (v.gain_l, v.gain_r),
            (0.5, 0.5),
            "原版 play_sound 力度写死 100"
        );
        assert_eq!(v.layer, 0, "节拍器不带层号，消音器掐不到它");
        assert_eq!(v.pre, 0.0, "tick -1：不参与整列抖动");

        st.trigger_metronome(2.0);
        assert_eq!(
            st.mixer.voices.len(),
            1,
            "metronome_played 已推进，同一拍不双响"
        );

        st.trigger_metronome(4.5);
        assert_eq!(st.mixer.voices.len(), 2, "第 4 列该再响一记");
        assert_eq!(
            st.mixer.voices[1].source,
            SampleSource::Metronome,
            "小节内非第一拍用普通采样"
        );

        st.trigger_metronome(16.5);
        assert_eq!(
            st.mixer.voices.last().unwrap().source,
            SampleSource::MetronomeClick,
            "下一小节头又是重音"
        );

        st.trigger_metronome(5.5);
        assert_eq!(st.mixer.voices.len(), 3, "非 4 的倍数不响");
    }

    /// 原版 `if (current_song.tempo = 30) pos -= 1` 和循环末尾的重音闸门
    /// `if (!current_song.loop || pos < current_song.enda + 1)`。
    #[test]
    fn metronome_follows_the_tempo_30_shift_and_the_loop_guard() {
        // 30 t/s（= 450 BPM）时 pos 让开一列：4.5 这一帧本来落在第 4 列，
        // 让位之后是 3，不成拍 → 不响。换个 tempo 就不让。
        let mut st = metronome_state(metronome_bank());
        st.params.tempo = 30.0;
        st.trigger_metronome(4.5);
        assert!(st.mixer.voices.is_empty(), "tempo = 30 要按原版让开一列");

        let mut st = metronome_state(metronome_bank());
        st.trigger_metronome(4.5);
        assert_eq!(st.mixer.voices.len(), 1, "其余 tempo 不让位，同一拍照响");

        // 循环开着、这拍又是小节头重音、又到达歌尾（`pos >= enda + 1`）：
        // 原版把发声挡掉，但 metronome_played 写在 if 外面，照旧推进。
        let mut st = metronome_state(metronome_bank());
        st.params.loop_enabled = true;
        st.params.song_length = 4.0;
        st.trigger_metronome(16.5);
        assert!(st.mixer.voices.is_empty(), "循环中的歌尾重音被原版闸门挡掉");
        assert_eq!(
            st.metronome_played, 17,
            "挡掉的是发声，metronome_played 仍要推进（它在原版写在 if 外面）"
        );
        st.trigger_metronome(20.5);
        assert_eq!(st.mixer.voices.len(), 1, "闸门只挡重音，小节内普通拍照响");
        // 反向确认闸门确实来自 loop 开关：同一个 pos 关掉循环就响。
        let mut st = metronome_state(metronome_bank());
        st.params.loop_enabled = false;
        st.params.song_length = 4.0;
        st.trigger_metronome(16.5);
        assert_eq!(st.mixer.voices.len(), 1, "循环关掉后同一拍的重音照响");
    }
}
