//! Standard MIDI File (SMF) 导入 / 导出。
//!
//! 解析对齐装机版（tag `v3.12.0-rc.1`）的 `open_midi.gml` + `midi_add_note.gml` +
//! `midi_set_until.gml`：逐轨道顺序读事件、支持 running status、program change 覆盖
//! `midi_channelpatch`、通道 9 出现过的音按首次顺序进 `midi_percnote`、tempo meta 按**文件
//! 顺序**（不排序）进 `midi_tempo_changer_*`。与旧版 `main` 的三处关键差别：
//! 1. note-off 不再直接丢掉，它会回填最近一个同音同通道的 `until`，并且**参与
//!    `midi_minpos/midi_maxpos`**（打击乐通道除外）——右上角时长和速度都因此不同；
//! 2. 每个 note-on 还记下当时的 patch 与「声部」（`midi_eventpatch` / `midi_eventpart`），
//!    声部按 `(track, channel, patch)` 首次出现编号，供「音符长度」页逐声部开关；
//! 3. 时长改用 `cache_midi_durations()` 的四格缓存（× 速度变化 × 去头静音），
//!    有速度变化时按时间轴分段积分，而不是拿首个 tempo 一路算到底。
//!
//! 导入对齐同版本的 `import_midi.gml`：量化 `pos = floor((midi_tick − minpos·去头) / deltapertick)`，
//! `deltapertick = (PPQ & 0x7FFF) / 4 / (精度 + 1)`（原版浮点除）；第一遍按通道做声部分配
//! （区间贪心，见 `assign_layers`），第二遍按 `eventlayer + 前面各通道的 channelheight之和`
//! 落块，开「音符长度」时把长音摊成左右交替的尾音。
//!
//! 导出为 format 1（每条 layer 一个 track），PPQ=480，1 NBS tick = 120 MIDI tick。

use std::collections::HashMap;
use std::path::Path;

use nbs_core::model::{
    CustomInstrument, Layer, LockMode, Note, Song, FIRST_CUSTOM_INDEX, KEY_MAX, MC_KEY_MAX,
    MC_KEY_MIN,
};

use crate::midi_gm::{is_note_fade, MidiGmTable};

const PPQ: u16 = 480;
const MIDI_TICKS_PER_NBS_TICK: u32 = PPQ as u32 / 4;

/// 每轨道事件上限，原版 `midi_add_note` 里 `if (n = 31999) return 0`。
const EVENTS_PER_TRACK_LIMIT: usize = 31_999;

// ---------------------------------------------------------------- 解析结果

/// 原版 `midi_eventx/note/channel/vel/patch/part/until[track, e]` 的一条 note-on。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    /// 原版 `midi_eventx`：文件内的绝对 tick。
    pub pos: i64,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    /// 原版 `midi_eventpatch`：note-on **当时**该通道的 program change，之后改.patch 也不跟着变。
    /// 用 u16 是因为原版界面留了 `>= 128 → 未知` 那条分支。
    pub patch: u16,
    /// 原版 `midi_eventpart`：`MidiData::parts` 的下标。
    pub part: usize,
    /// 原版 `midi_eventuntil`：配对到的 note-off 的 tick，没配对到为 -1。
    pub until: i64,
}

/// 原版 `midi_parts` 的一条：同一轨道、同一通道、同一 patch 算一个声部。
/// `note_duration` 只是出厂初始值（`channel != 9`）；界面上改的是
/// `MidiImportOptions::part_durations`，导入门槛也看那份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiPart {
    pub track: usize,
    pub channel: u8,
    pub patch: u16,
    /// 初始值 = `channel != 9`（原版 `new_instrument` 那句）。
    pub note_duration: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MidiTrack {
    /// 原版 `midi_trackname[t]`（meta 0x03）。
    pub name: String,
    /// 原版 `midi_trackamount[t]`，即 `events.len()`。
    pub events: Vec<MidiEvent>,
}

#[derive(Clone, Debug)]
pub struct MidiData {
    pub format: u16,
    /// 原版 `midi_tempo`：头部的分频字段，`& 0x7FFF` 才是每四分音符 tick 数，
    /// 置了高位（`& 0x8000`）表示 SMPTE 分频，原版据此跳过速度变化那套。
    pub division: u16,
    pub tracks: Vec<MidiTrack>,
    /// 原版 `midi_channelpatch`：最后一次 program change，跨轨道全局生效。
    pub channel_patch: [u16; 16],
    /// 原版 `midi_channels`：见过的最大通道号（不是通道个数），界面和循环都用 `0..=它`。
    pub channels: u8,
    /// 原版 `midi_percnote`：通道 9 用到的音号，按首次出现顺序（旧版会过滤 24..84，装机版不过滤）。
    pub perc_notes: Vec<u8>,
    /// 原版 `midi_parts`，按 `(track, channel, patch)` 首次出现编号。
    pub parts: Vec<MidiPart>,
    /// 原版 `midi_minpos`，没有任何事件时保持 -1。note-off 也算进来。
    pub minpos: i64,
    pub maxpos: i64,
    /// 原版 `midi_micsecqn`：第一个 tempo meta 的微秒数，没有则 0。
    pub micsecqn: u32,
    /// 原版 `midi_tempo_changer_x` / `midi_tempo_changer_tempo`（后者是 BPM = 6e7/微秒数）。
    pub tempo_changers: Vec<(i64, f32)>,
    pub copyright: String,
}

impl Default for MidiData {
    fn default() -> Self {
        Self {
            format: 1,
            division: 0,
            tracks: Vec::new(),
            channel_patch: [0; 16],
            channels: 0,
            perc_notes: Vec::new(),
            parts: Vec::new(),
            minpos: -1,
            maxpos: 0,
            micsecqn: 0,
            tempo_changers: Vec::new(),
            copyright: String::new(),
        }
    }
}

impl MidiData {
    pub fn ppq(&self) -> u16 {
        self.division & 0x7FFF
    }

    /// 原版 `cache_midi_durations()`：`[速度变化 off/on][去头静音 keep/strip]` 四格秒数。
    pub fn duration_cache(&self) -> [[f64; 2]; 2] {
        let mut cache = [[0.0, 0.0], [0.0, 0.0]];
        let division = self.ppq() as f64;
        if division <= 0.0 || self.maxpos <= 0 || self.micsecqn == 0 {
            return cache;
        }
        let trim_start = self.minpos.max(0);
        let mut seconds_per_tick = self.micsecqn as f64 / division / 1_000_000.0;
        let full = self.maxpos as f64 * seconds_per_tick;
        let trimmed = (self.maxpos - trim_start).max(0) as f64 * seconds_per_tick;
        cache = [[full, trimmed], [full, trimmed]];
        if self.tempo_changers.is_empty() || self.division & 0x8000 != 0 {
            return cache;
        }

        // 各轨道是分开读的，tempo 事件的 tick 未必递增；原版按 (位置, 文件顺序) 排，
        // 同位置保留文件顺序，和导入那边「后写的覆盖」一致。
        let mut changes: Vec<(i64, f64, usize)> = self
            .tempo_changers
            .iter()
            .enumerate()
            .filter(|(_, (_, bpm))| *bpm > 0.0)
            .map(|(i, (x, bpm))| (*x, 60.0 / (*bpm as f64) / division, i))
            .collect();
        changes.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));

        let mut full = 0.0;
        let mut trimmed = 0.0;
        let mut previous_tick = 0i64;
        for i in 0..=changes.len() {
            let mut next_tick = self.maxpos;
            if i < changes.len() {
                next_tick = changes[i].0.clamp(0, self.maxpos);
            }
            full += (next_tick - previous_tick) as f64 * seconds_per_tick;
            trimmed += (next_tick - previous_tick.max(trim_start)).max(0) as f64 * seconds_per_tick;
            previous_tick = next_tick;
            if next_tick >= self.maxpos {
                break;
            }
            seconds_per_tick = changes[i].1;
        }
        cache[1] = [full, trimmed];
        cache
    }

    /// 原版 `midi_songlength`：界面右上角的时长，也是导入时算速度的依据。
    pub fn song_length(&self, tempo_changer: bool, remove_silent: bool) -> f64 {
        self.duration_cache()[tempo_changer as usize][remove_silent as usize]
    }

    /// 原版 `deltapertick = (midi_tempo & $7FFF) / 4 / (w_midi_precision + 1)`，浮点。
    pub fn deltapertick(&self, precision: u8) -> f64 {
        self.ppq() as f64 / 4.0 / (precision as f64 + 1.0)
    }

    /// 原版 `floor((midi_eventx − midi_minpos * w_midi_removesilent) / deltapertick)`。
    pub fn quantize(&self, pos: i64, remove_silent: bool, deltapertick: f64) -> i64 {
        let minpos = if remove_silent { self.minpos } else { 0 };
        ((pos - minpos) as f64 / deltapertick).floor() as i64
    }
}

// ---------------------------------------------------------------- 导入选项

/// 原版 `w_midi_*` 一套 + 界面可改的通道/打击乐映射（`midi_channelins` 等）。
#[derive(Clone, Debug, PartialEq)]
pub struct MidiImportOptions {
    /// `w_midi_remember`：导入后把改动写回 GM 表并保存。
    pub remember: bool,
    pub remove_silent: bool,
    /// `w_midi_name`：给每层命名。
    pub name_layers: bool,
    /// `w_midi_name_patch`：按乐器名命名（否则按层号）。
    pub name_after_patch: bool,
    /// `w_midi_tempo`：速度取文件里的。
    pub tempo_from_file: bool,
    pub tempo_changer: bool,
    /// `w_midi_maxheight`：1..20，20 表示不限。
    pub max_height: u8,
    /// `w_midi_octave`：保持八度范围（折进 33..57）。
    pub keep_octave_range: bool,
    pub use_velocity: bool,
    /// `w_midi_precision`：0/1/3/7 → 1/2/4/8 倍。
    pub precision: u8,
    /// `w_midi_note_duration`：把长音摊成尾音。
    pub note_duration: bool,
    /// `w_midi_note_duration_fade`：尾音音量按起止百分比线性变化。
    pub note_duration_fade: bool,
    pub note_duration_fade_start: u8,
    pub note_duration_fade_end: u8,
    /// `midi_channelins` / `midi_channeloctave`，-1 = 无视该通道。
    pub channel_ins: [i8; 16],
    pub channel_octave: [i8; 16],
    /// `midi_percins` / `midi_percpitch`，与 `MidiData::perc_notes` 下标对齐；pitch 已含 +33。
    pub perc_ins: Vec<i8>,
    pub perc_pitch: Vec<i8>,
    /// 界面上的「音符长度」勾选状态，与 `MidiData::parts` 下标对齐。
    /// 原版直接改 `midi_parts[a].note_duration`；这里把「解析结果」和「界面上的改动」分开，
    /// 初始值取 `MidiPart::note_duration`（= `channel != 9`），「使用默认值」再恢复成它。
    pub part_durations: Vec<bool>,
}

impl Default for MidiImportOptions {
    /// 原版 `control_create.gml`（tag v3.12.0-rc.1）的 `w_midi_*` 默认值。
    fn default() -> Self {
        Self {
            remember: true,
            remove_silent: true,
            name_layers: true,
            name_after_patch: true,
            tempo_from_file: true,
            tempo_changer: false,
            max_height: 20,
            keep_octave_range: false,
            use_velocity: true,
            precision: 1,
            note_duration: false,
            note_duration_fade: false,
            note_duration_fade_start: 50,
            note_duration_fade_end: 50,
            channel_ins: [0; 16],
            channel_octave: [0; 16],
            perc_ins: Vec::new(),
            perc_pitch: Vec::new(),
            part_durations: Vec::new(),
        }
    }
}

impl MidiImportOptions {
    /// 原版 `open_midi.gml:225-244`：解析完一个文件后，按 GM 表现算通道音色/八度、
    /// 鼓映射和声部勾选。`w_midi_*` 那几个标量**不动**——原版它们是全局，
    /// 上一次导入留下的勾下次还在（只有「使用默认值」和「不记住」那两条路会改）。
    pub fn reload_tables(&mut self, midi: &MidiData, gm: &MidiGmTable) {
        for c in 0..16 {
            let patch = midi.channel_patch[c] as usize;
            // 原版 midi_ins[patch, 1] 越界（patch >= 128）时 try/catch 落到 -1 / 0。
            let (ins, oct) = if patch < 128 { gm.patch(patch as u8) } else { (-1, 0) };
            self.channel_ins[c] = ins;
            self.channel_octave[c] = oct;
        }
        self.perc_ins.clear();
        self.perc_pitch.clear();
        for &note in &midi.perc_notes {
            let (ins, key) = gm.drum(note);
            self.perc_ins.push(ins);
            self.perc_pitch.push(key + 33);
        }
        self.part_durations = midi.parts.iter().map(|p| p.note_duration).collect();
    }

    /// 出厂标量 + 这个文件的映射表，给只做计算、没有界面状态的调用方用
    /// （`nbs-app --import-midi`、对照工具、测试）。
    pub fn for_midi(midi: &MidiData, gm: &MidiGmTable) -> Self {
        let mut o = Self::default();
        o.reload_tables(midi, gm);
        o
    }

    /// 原版「使用默认值」按钮（`draw_window_midi_import.gml:109-137`）：
    /// 重查 GM 表 + 恢复 `w_midi_*`，并把每个声部的「音符长度」开关恢复成 `channel != 9`。
    pub fn use_defaults(&mut self, midi: &MidiData, gm: &MidiGmTable) {
        self.reload_tables(midi, gm);
        self.remove_silent = true;
        self.name_layers = true;
        self.name_after_patch = true;
        self.max_height = 20;
        self.tempo_from_file = true;
        self.keep_octave_range = false;
        self.use_velocity = true;
        self.precision = 1;
        self.tempo_changer = false;
        self.note_duration = false;
        self.note_duration_fade = false;
        self.note_duration_fade_start = 50;
        self.note_duration_fade_end = 50;
    }

    /// 原版 `import_midi.gml:197-222` 的收尾。
    ///
    /// 记住：把界面上的通道/鼓映射写回 GM 表。原版的循环是 `a < midi_channels`
    /// （不含编号最大的那个通道），照抄——写回范围不一样，下次开窗口预填的值就
    /// 不一样，装机版实测如此。
    ///
    /// 不记住：原版并不只是「不写回」，而是把界面上的这几个开关恢复成另一组值，
    /// 然后 `save_settings()`。这组和 `control_create` 的出厂值不同（层数上限 2、
    /// 精准度 1 倍、八度范围开），所以「不记住」的下一次导入确实会长这样。
    /// `note_duration` 本身不在恢复清单里，只有它下面的三个渐变项被清零/复位。
    pub fn finish_import(&mut self, midi: &MidiData, gm: &mut MidiGmTable) {
        if self.remember {
            for c in 0..midi.channels as usize {
                let patch = midi.channel_patch[c as usize];
                if patch < 128 {
                    gm.set_patch(patch as u8, self.channel_ins[c], self.channel_octave[c]);
                }
            }
            for (i, &note) in midi.perc_notes.iter().enumerate() {
                if let (Some(ins), Some(pitch)) = (self.perc_ins.get(i), self.perc_pitch.get(i)) {
                    gm.set_drum(note, *ins, *pitch - 33);
                }
            }
            return;
        }
        self.remove_silent = true;
        self.name_layers = true;
        self.name_after_patch = true;
        self.max_height = 2;
        self.tempo_from_file = true;
        self.keep_octave_range = true;
        self.precision = 0;
        self.note_duration_fade = false;
        self.note_duration_fade_start = 50;
        self.note_duration_fade_end = 50;
    }
}

// ---------------------------------------------------------------- 偏好

/// 原版 `settings.ini` 的 `[midi_import]` 里那 14 个 `w_midi_*` 标量：每次
/// `save_settings()` 无条件写、启动时 `load_settings()` 无条件读回，所以导入窗口
/// 打开时看到的就是上次导入留下的状态。通道/鼓映射那 512 个数另存 `MidiGmPersist`；
/// 声部勾选属于当前文件，和 per-file 映射一样不落盘。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MidiImportPrefs {
    pub remember: bool,
    pub remove_silent: bool,
    pub name_layers: bool,
    pub name_after_patch: bool,
    pub tempo_from_file: bool,
    pub tempo_changer: bool,
    pub max_height: u8,
    pub keep_octave_range: bool,
    pub use_velocity: bool,
    pub precision: u8,
    pub note_duration: bool,
    pub note_duration_fade: bool,
    pub note_duration_fade_start: u8,
    pub note_duration_fade_end: u8,
}

impl MidiImportPrefs {
    /// 出厂值 = `control_create.gml`（tag v3.12.0-rc.1）那套 `w_midi_*`。
    pub fn factory() -> Self {
        Self::from_options(&MidiImportOptions::default())
    }

    pub fn from_options(o: &MidiImportOptions) -> Self {
        Self {
            remember: o.remember,
            remove_silent: o.remove_silent,
            name_layers: o.name_layers,
            name_after_patch: o.name_after_patch,
            tempo_from_file: o.tempo_from_file,
            tempo_changer: o.tempo_changer,
            max_height: o.max_height,
            keep_octave_range: o.keep_octave_range,
            use_velocity: o.use_velocity,
            precision: o.precision,
            note_duration: o.note_duration,
            note_duration_fade: o.note_duration_fade,
            note_duration_fade_start: o.note_duration_fade_start,
            note_duration_fade_end: o.note_duration_fade_end,
        }
    }

    /// `load_settings`：只覆盖这几个标量；界面上的映射表由 `for_midi` 按 GM 表现算。
    pub fn apply(&self, o: &mut MidiImportOptions) {
        o.remember = self.remember;
        o.remove_silent = self.remove_silent;
        o.name_layers = self.name_layers;
        o.name_after_patch = self.name_after_patch;
        o.tempo_from_file = self.tempo_from_file;
        o.tempo_changer = self.tempo_changer;
        o.max_height = self.max_height;
        o.keep_octave_range = self.keep_octave_range;
        o.use_velocity = self.use_velocity;
        o.precision = self.precision;
        o.note_duration = self.note_duration;
        o.note_duration_fade = self.note_duration_fade;
        o.note_duration_fade_start = self.note_duration_fade_start;
        o.note_duration_fade_end = self.note_duration_fade_end;
    }
}

// ---------------------------------------------------------------- 解析

fn be_u16(b: &[u8], p: usize) -> Result<u16, String> {
    if p + 2 > b.len() {
        return Err(crate::tr("Unexpected end of file (u16)", "文件意外结束 (u16)").into());
    }
    Ok(u16::from_be_bytes([b[p], b[p + 1]]))
}

fn be_u32(b: &[u8], p: usize) -> Result<u32, String> {
    if p + 4 > b.len() {
        return Err(crate::tr("Unexpected end of file (u32)", "文件意外结束 (u32)").into());
    }
    Ok(u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]))
}

fn read_varlen(data: &[u8], pos: &mut usize) -> Result<u32, String> {
    let mut value: u32 = 0;
    for _ in 0..4 {
        if *pos >= data.len() {
            return Err(crate::tr("Unexpected end of file (varlen)", "文件意外结束 (varlen)").into());
        }
        let b = data[*pos];
        *pos += 1;
        value = (value << 7) | (b & 0x7F) as u32;
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(crate::tr("Invalid variable-length integer", "非法的变长整数").into())
}

fn read_text(data: &[u8], pos: &mut usize, len: usize) -> String {
    let end = (*pos + len).min(data.len());
    let s = String::from_utf8_lossy(&data[*pos..end]).into_owned();
    *pos = end;
    s
}

/// 原版散在 `midi_minpos/maxpos/channels/percnote/percamount` 上的全局统计，
/// 加上 `midi_part_lookup/parts`。拆出来是因为 `add_note` 要同时改统计和当前轨道的事件表。
#[derive(Default)]
struct GlobalStats {
    minpos: i64,
    maxpos: i64,
    channels: u8,
    perc_notes: Vec<u8>,
    parts: Vec<MidiPart>,
    part_lookup: HashMap<String, usize>,
}

impl GlobalStats {
    fn new() -> Self {
        Self { minpos: -1, ..Default::default() }
    }

    /// 原版 `if (pos < midi_minpos || midi_minpos = -1) midi_minpos = pos; midi_maxpos = max(...)`。
    fn span(&mut self, pos: i64) {
        if self.minpos == -1 || pos < self.minpos {
            self.minpos = pos;
        }
        if pos > self.maxpos {
            self.maxpos = pos;
        }
    }
}

/// 原版 `midi_add_note`：登记 note-on，记当时 patch、所属声部，until 先置 -1。
/// 每轨道上限 31999 条；装机版对通道 9 **不再**过滤 24..84，越界的鼓音靠 GM 表里的
/// `ins = -1`（原版写的是 "----"）在导入阶段静音。
fn add_note(
    g: &mut GlobalStats,
    events: &mut Vec<MidiEvent>,
    track: usize,
    pos: i64,
    channel: u8,
    note: u8,
    velocity: u8,
    channel_patch: u16,
) {
    if events.len() == EVENTS_PER_TRACK_LIMIT {
        return;
    }
    g.span(pos);
    let patch = channel_patch;
    let key = format!("{}_{}_{}", track, channel, patch);
    let part = match g.part_lookup.get(&key) {
        Some(&i) => i,
        None => {
            let i = g.parts.len();
            g.parts.push(MidiPart { track, channel, patch, note_duration: channel != 9 });
            g.part_lookup.insert(key, i);
            i
        }
    };
    events.push(MidiEvent { pos, channel, note, velocity, patch, part, until: -1 });
    g.channels = g.channels.max(channel);
    if channel == 9 && !g.perc_notes.contains(&note) {
        g.perc_notes.push(note);
    }
}

/// 原版 `midi_set_until`：note-off（或力度 0 的 note-on）回填最近一个同音同通道、
/// 还没配对过的 note-on，并参与 minpos/maxpos。通道 9 直接返回。
fn set_until(
    g: &mut GlobalStats,
    events: &mut [MidiEvent],
    pos: i64,
    channel: u8,
    note: u8,
) {
    if channel == 9 {
        return;
    }
    g.span(pos);
    for ev in events.iter_mut().rev() {
        if ev.note == note && ev.channel == channel {
            if ev.until == -1 {
                ev.until = pos;
            }
            break;
        }
    }
}

/// 解析 SMF 字节流，逐条对应 `open_midi.gml`。
pub fn parse_midi(data: &[u8]) -> Result<MidiData, String> {
    if data.len() < 14 || &data[0..4] != b"MThd" {
        return Err(crate::tr("Not a valid MIDI file (missing MThd)", "不是有效的 MIDI 文件（缺少 MThd）").into());
    }
    let header_len = be_u32(data, 4)? as usize;
    if be_u32(data, 4)? != 6 {
        return Err(crate::tr("MIDI header length must be 00 00 00 06", "MIDI 头部长度必须是 00 00 00 06").into());
    }
    let format = be_u16(data, 8)?;
    if format > 2 {
        return Err(crate::tr("Unsupported MIDI format", "不支持的 MIDI 格式").into());
    }
    let tracks = be_u16(data, 10)? as usize;
    let mut midi = MidiData {
        format,
        division: be_u16(data, 12)?,
        tracks: vec![MidiTrack::default(); tracks],
        ..MidiData::default()
    };
    let mut pos = 8 + header_len;
    // 原版 lasteventtype / lastchannel 是函数级变量，跨轨道保留。
    let mut running: u8 = 0;
    let mut stats = GlobalStats::new();

    for t in 0..tracks {
        if pos + 8 > data.len() {
            break;
        }
        if &data[pos..pos + 4] != b"MTrk" {
            return Err(crate::tr("MIDI track chunk must start with MTrk", "MIDI 轨道块必须以 MTrk 开头").into());
        }
        let len = be_u32(data, pos + 4)? as usize;
        let mut p = pos + 8;
        let end = (p + len).min(data.len());
        pos = end;
        let mut tick: i64 = 0;
        while p < end {
            tick += read_varlen(data, &mut p)? as i64;
            if p >= end {
                break;
            }
            let event = data[p];
            if event == 0xFF {
                // meta
                p += 1;
                if p >= end {
                    break;
                }
                let meta = data[p];
                p += 1;
                let mlen = read_varlen(data, &mut p)? as usize;
                match meta {
                    0x02 => midi.copyright = read_text(data, &mut p, mlen),
                    0x03 => midi.tracks[t].name = read_text(data, &mut p, mlen),
                    0x51 => {
                        // 原版读完长度字节后固定取 3 个字节；这里 p 已经指向第一个数据字节。
                        let v = [
                            data.get(p).copied().unwrap_or(0),
                            data.get(p + 1).copied().unwrap_or(0),
                            data.get(p + 2).copied().unwrap_or(0),
                        ];
                        let uspq = (v[0] as u32) << 16 | (v[1] as u32) << 8 | v[2] as u32;
                        p += mlen;
                        midi.tempo_changers.push((tick, 60_000_000.0 / uspq as f32));
                        if midi.micsecqn == 0 {
                            midi.micsecqn = uspq;
                        }
                        continue;
                    }
                    _ => p = (p + mlen).min(end),
                }
                continue;
            }
            if event == 0xF0 || event == 0xF7 {
                p += 1;
                let slen = read_varlen(data, &mut p)? as usize;
                p = (p + slen).min(end);
                continue;
            }
            if matches!(event, 0xF8 | 0xFA | 0xFB | 0xFC) {
                // 实时信息：原版读完 status 就什么都不做
                p += 1;
                continue;
            }
            let (kind, channel) = if event & 0x80 != 0 {
                p += 1;
                running = event;
                (event >> 4, event & 0x0F)
            } else {
                // running status：不前进，原位重读数据字节
                (running >> 4, running & 0x0F)
            };
            match kind {
                0x8 => {
                    if p >= end {
                        break;
                    }
                    let note = data[p];
                    // 原版 midi_set_until 收了 vel 但从来没用。
                    p += 2;
                    set_until(&mut stats, &mut midi.tracks[t].events, tick, channel, note);
                }
                0x9 => {
                    if p + 2 > end {
                        break;
                    }
                    let (note, vel) = (data[p], data[p + 1]);
                    p += 2;
                    if vel > 0 {
                        add_note(
                            &mut stats,
                            &mut midi.tracks[t].events,
                            t,
                            tick,
                            channel,
                            note,
                            vel,
                            midi.channel_patch[channel as usize],
                        );
                    } else {
                        set_until(&mut stats, &mut midi.tracks[t].events, tick, channel, note);
                    }
                }
                0xA | 0xB | 0xE => p = (p + 2).min(end),
                0xC => {
                    if p >= end {
                        break;
                    }
                    midi.channel_patch[channel as usize] = data[p] as u16;
                    p += 1;
                }
                0xD => p = (p + 1).min(end),
                _ => return Err(crate::tr("Unknown MIDI event", "未知的 MIDI 事件").into()),
            }
        }
    }
    midi.minpos = stats.minpos;
    midi.maxpos = stats.maxpos;
    midi.channels = stats.channels;
    midi.perc_notes = stats.perc_notes;
    midi.parts = stats.parts;
    Ok(midi)
}

/// 原版 `open_midi()`：只读盘解析，不落歌。落歌是导入窗口点「导入」之后的事。
pub fn parse_midi_path(path: &Path) -> Result<MidiData, String> {
    let data = std::fs::read(path).map_err(|e| {
        crate::tr(&format!("Failed to read MIDI: {e}"), &format!("读取 MIDI 失败: {e}")).to_string()
    })?;
    parse_midi(&data)
}

// ---------------------------------------------------------------- 导入

pub fn import_midi_bytes(data: &[u8], opts: &MidiImportOptions) -> Result<Song, String> {
    let midi = parse_midi(data)?;
    Ok(import_midi_data(&midi, opts))
}

pub fn import_midi_path(path: &Path, opts: &MidiImportOptions) -> Result<Song, String> {
    let data = std::fs::read(path).map_err(|e| {
        crate::tr(&format!("Failed to read MIDI: {e}"), &format!("读取 MIDI 失败: {e}")).to_string()
    })?;
    import_midi_bytes(&data, opts)
}

/// 原版一个 note-on 在本轮量化后的全部派生值，两遍扫描各算一次（原版也是各算一次）。
struct Resolved {
    channel: usize,
    /// 量化后的起始 tick，可能为负（原版第二遍不拦，交给 `add_block` 的 `xx < 0` 忽略）。
    pos: i64,
    /// 量化后的结束 tick（未配对到 note-off 时为 -1 量化出来的负数）。
    until: i64,
    note: i32,
    ins: i8,
    /// 通道 9 且该鼓音被设成静音 / 通道被设成无视。
    stop: bool,
    include_duration: bool,
    vel: u8,
}

fn resolve(midi: &MidiData, opts: &MidiImportOptions, ev: &MidiEvent, dpt: f64) -> Resolved {
    let channel = ev.channel as usize;
    let pos = midi.quantize(ev.pos, opts.remove_silent, dpt);
    let until = midi.quantize(ev.until, opts.remove_silent, dpt);
    let patch_ok = ev.patch < 128;
    let include_duration = opts.note_duration
        && channel != 9
        && patch_ok
        && opts.part_durations.get(ev.part).copied().unwrap_or(false)
        && ev.until != -1
        && until > pos;
    let mut vel = if opts.use_velocity { ev.velocity } else { 100 };
    if vel >= 100 {
        vel = 100;
    }
    let mut note = ev.note as i32 - 21;
    let (ins, stop) = if channel == 9 {
        // 原版那圈「找不到就停在 midi_percamount」：解析阶段每个鼓音都进过 perc_notes，
        // 找不到只可能来自手改的选项表，此时按静音处理。
        match midi
            .perc_notes
            .iter()
            .position(|&n| n == ev.note)
            .and_then(|i| Some((opts.perc_ins.get(i).copied()?, opts.perc_pitch.get(i).copied()?)))
        {
            Some((perc_ins, pitch)) => {
                note = (pitch as i32).clamp(0, KEY_MAX as i32);
                (perc_ins, perc_ins == -1)
            }
            None => (-1, true),
        }
    } else {
        let ins = opts.channel_ins[channel];
        note = (note + 12 * opts.channel_octave[channel] as i32).clamp(0, KEY_MAX as i32);
        (ins, ins == -1)
    };
    if opts.keep_octave_range {
        while note < MC_KEY_MIN as i32 {
            note += 12;
        }
        while note > MC_KEY_MAX as i32 {
            note -= 12;
        }
    }
    Resolved { channel, pos, until, note, ins, stop, include_duration, vel }
}

/// 原版第一遍扫描里 `channel_events[channel]` 的一项。
struct EvRef {
    track: usize,
    index: usize,
    start: i64,
    /// 原版 `finish`：不延长时是 `pos + 1`，延长时是量化后的 note-off 位置。
    finish: i64,
    /// 原版 `order`：入队顺序，用于和 start 相同的项定先后。
    order: usize,
}

/// 原版 `import_midi.gml:59-75` 的声部分配：按 start 排序后，贪心找第一个
/// 「上一个音还没结束」之后的空声部；`layer_ends` 的长度就是该通道占的层数。
/// 返回每个事件落在的层号（-1 = 被「通道最高层数」拦掉）。
fn assign_layers(
    per_channel: &mut [Vec<EvRef>],
    eventlayer: &mut [Vec<i32>],
    max_height: u8,
) -> Vec<usize> {
    let unlimited = max_height >= 20;
    let mut channel_height = vec![0usize; per_channel.len()];
    for (channel, events) in per_channel.iter_mut().enumerate() {
        events.sort_by_key(|e| (e.start, e.order));
        let mut layer_ends: Vec<i64> = Vec::new();
        for entry in events.iter() {
            let mut slot = 0usize;
            while slot < layer_ends.len() && layer_ends[slot] > entry.start {
                slot += 1;
            }
            if !unlimited && slot >= max_height as usize {
                continue;
            }
            if slot == layer_ends.len() {
                layer_ends.push(entry.finish);
            } else {
                layer_ends[slot] = entry.finish;
            }
            eventlayer[entry.track][entry.index] = slot as i32;
        }
        channel_height[channel] = layer_ends.len();
    }
    channel_height
}

/// 逐条对应装机版的 `import_midi.gml`。
pub fn import_midi_data(midi: &MidiData, opts: &MidiImportOptions) -> Song {
    let mut song = Song::new();
    let dpt = midi.deltapertick(opts.precision).max(1e-9);
    let seconds = midi.song_length(opts.tempo_changer, opts.remove_silent) as f32;

    if opts.tempo_changer {
        song.custom_instruments.push(CustomInstrument {
            name: "Tempo Changer".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
    }

    let last_channel = midi.channels as usize;
    let mut per_channel: Vec<Vec<EvRef>> = (0..last_channel + 1).map(|_| Vec::new()).collect();
    let mut eventlayer: Vec<Vec<i32>> =
        midi.tracks.iter().map(|t| vec![-1; t.events.len()]).collect();

    // 第一遍：只登记事件并算出每个通道要占几层。
    for (t, track) in midi.tracks.iter().enumerate() {
        for (e, ev) in track.events.iter().enumerate() {
            let r = resolve(midi, opts, ev, dpt);
            if r.channel > last_channel || r.stop || r.pos < 0 {
                continue;
            }
            let finish = if r.include_duration { r.until } else { r.pos + 1 };
            let order = per_channel[r.channel].len();
            per_channel[r.channel].push(EvRef { track: t, index: e, start: r.pos, finish, order });
        }
    }
    let channel_height = assign_layers(&mut per_channel, &mut eventlayer, opts.max_height);

    // 第二遍：落块。层号 = 该事件在通道内的声部 + 前面所有通道的层数之和（开速度变化再 +1）。
    for (t, track) in midi.tracks.iter().enumerate() {
        for (e, ev) in track.events.iter().enumerate() {
            let r = resolve(midi, opts, ev, dpt);
            if r.channel > last_channel || r.ins < 0 || eventlayer[t][e] < 0 {
                continue;
            }
            let mut yy = eventlayer[t][e] as usize;
            yy += channel_height[..r.channel].iter().sum::<usize>();
            if opts.tempo_changer {
                yy += 1;
            }
            let forvalue = if r.include_duration { r.until } else { r.pos + 1 };

            // 尾音的音量曲线：勾选「尾音渐变」走自定义线性，否则查渐入/渐出名单，都不在就固定音量。
            let mut fade_mode: i8 = -1;
            if r.include_duration {
                if opts.note_duration_fade {
                    fade_mode = 0;
                } else if is_note_fade(MidiGmTable::patch_name(ev.patch as u8), false) {
                    fade_mode = 1;
                } else if is_note_fade(MidiGmTable::patch_name(ev.patch as u8), true) {
                    fade_mode = 2;
                }
            }

            let mut i = r.pos;
            while i < forvalue {
                let at = i - r.pos;
                let mut tempvel = r.vel as i32;
                let mut temppan: u8 = 100;
                let span = (r.until - r.pos) as f64;
                if fade_mode == 1 {
                    tempvel = (r.vel as f64 * (at as f64 / span)).floor() as i32;
                } else if fade_mode == 2 {
                    tempvel = (r.vel as f64 * ((r.until - i) as f64 / span)).floor() as i32;
                } else if at != 0 {
                    // 音头保持原音量，只让生成的尾音渐变。
                    let mut fadepercent = 50.0;
                    if fade_mode == 0 {
                        let length = (forvalue - r.pos - 2) as f64;
                        fadepercent = opts.note_duration_fade_start as f64;
                        if length > 0.0 {
                            fadepercent += (opts.note_duration_fade_end as f64
                                - opts.note_duration_fade_start as f64)
                                * ((at - 1) as f64 / length);
                        }
                    }
                    tempvel = (r.vel as f64 * fadepercent / 100.0).floor() as i32;
                    // 原版靠左右交替让长音听起来像「持续」，而不是同一只喇叭反复敲。
                    temppan = if at % 2 == 0 { 150 } else { 50 };
                }
                if i >= 0 && tempvel >= 0 && !song.has(i as u32, yy as u32) {
                    // 对应原版 add_block：格子已被占就什么都不做（返回 0）。
                    song.set(
                        i as u32,
                        yy as u32,
                        Note {
                            instrument: r.ins as u8,
                            key: r.note.clamp(0, KEY_MAX as i32) as u8,
                            velocity: tempvel.min(u8::MAX as i32) as u8,
                            panning: temppan,
                            pitch: 0,
                        },
                    );
                }
                i += 1;
            }
        }
    }

    if opts.tempo_changer {
        let tc = FIRST_CUSTOM_INDEX;
        for (x, bpm) in &midi.tempo_changers {
            let pos = midi.quantize(*x, opts.remove_silent, dpt).max(0);
            if pos >= song.song_length() as i64 {
                continue;
            }
            let pos = pos as u32;
            if !song.has(pos, 0) {
                song.set(
                    pos,
                    0,
                    Note {
                        instrument: tc,
                        key: 39,
                        velocity: 100,
                        panning: 100,
                        pitch: (bpm * (opts.precision as f32 + 1.0)) as i16,
                    },
                );
            } else {
                // 原版这里走 change_block，即覆盖音高以外的全部字段。
                if let Some(col) = song.columns.get_mut(&pos) {
                    if let Some(n) = col.notes.get_mut(&0) {
                        *n = Note {
                            instrument: tc,
                            key: 39,
                            velocity: 100,
                            panning: 100,
                            pitch: (bpm * (opts.precision as f32 + 1.0)) as i16,
                        };
                    }
                }
            }
        }
    }

    // 速度：原版两个分支，用的秒数来自时长缓存。
    if !opts.tempo_changer || midi.tempo_changers.is_empty() {
        if opts.tempo_from_file && song.enda > 0 && seconds > 0.0 {
            song.meta.tempo =
                (10.0 / (seconds / (song.enda as f32 / 10.0))).clamp(0.25, 1000.0);
        }
    } else {
        song.meta.tempo = (midi.tempo_changers[0].1 * (opts.precision as f32 + 1.0)) / 15.0;
    }

    // 层名：原版 `w_midi_name` 分支，每个通道按它占的层数重复同一个名字。
    if opts.name_layers {
        let mut yy: usize = 0;
        if opts.tempo_changer {
            grow_layers(&mut song, 1);
            song.layers[0].name = "TempoChgr".into();
            yy = 1;
        }
        for a in 0..=last_channel {
            for _ in 0..channel_height[a] {
                grow_layers(&mut song, yy + 1);
                let mut name = format!("Channel {}", a + 1);
                if opts.name_after_patch {
                    let patch = midi.channel_patch[a] as usize;
                    name = if patch >= 128 {
                        "Unknown".into()
                    } else {
                        let short = MidiGmTable::patch_short(patch as u8);
                        if short.is_empty() {
                            MidiGmTable::patch_name(patch as u8).into()
                        } else {
                            short.into()
                        }
                    };
                    if a == 9 {
                        name = "Percussion".into();
                    }
                }
                song.layers[yy].name = name;
                song.layers[yy].lock = LockMode::Normal;
                song.layers[yy].volume = 100;
                song.layers[yy].stereo = 100;
                yy += 1;
            }
        }
        grow_layers(&mut song, yy);
    }

    song.recalculate();
    song.dirty = true;
    song
}

// ---------------------------------------------------------------- 导出
fn write_varlen(out: &mut Vec<u8>, mut value: u32) {
    let mut bytes = [0u8; 4];
    let mut i = 4;
    bytes[i - 1] = (value & 0x7F) as u8;
    i -= 1;
    value >>= 7;
    while value > 0 {
        bytes[i - 1] = ((value & 0x7F) | 0x80) as u8;
        i -= 1;
        value >>= 7;
    }
    out.extend_from_slice(&bytes[i..]);
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
}

/// 导出为 SMF format 1：track 0 是速度/拍号，之后每条 layer 一个 track。
pub fn export_midi_bytes(song: &Song) -> Vec<u8> {
    let bpm = (song.meta.tempo * 15.0).round().clamp(1.0, 1000.0);
    let uspq = (60_000_000.0 / bpm as f64).round() as u32;
    let mut t0 = Vec::new();
    write_varlen(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x51, 0x03]);
    t0.extend_from_slice(&[(uspq >> 16) as u8, (uspq >> 8) as u8, uspq as u8]);
    write_varlen(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x58, 0x04, song.meta.time_signature, 2, 24, 8]);
    write_varlen(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    let mut out = Vec::new();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes()); // format 1
    header.extend_from_slice(&((song.layers.len() as u16) + 1).to_be_bytes());
    header.extend_from_slice(&PPQ.to_be_bytes());
    write_chunk(&mut out, b"MThd", &header);
    write_chunk(&mut out, b"MTrk", &t0);

    for layer in 0..song.layers.len() as u32 {
        let mut events: Vec<(u32, Vec<u8>)> = Vec::new();
        for (tick, l, note) in song.iter_notes() {
            if l != layer {
                continue;
            }
            let midi_tick = tick * MIDI_TICKS_PER_NBS_TICK;
            let midi_note = (note.key as i32 + 21).clamp(0, 127) as u8;
            let vel = note.velocity.min(127).max(1);
            events.push((midi_tick, vec![0x90, midi_note, vel]));
            events.push((
                midi_tick + (MIDI_TICKS_PER_NBS_TICK / 2).max(1),
                vec![0x80, midi_note, 0],
            ));
        }
        events.sort_by_key(|e| e.0);
        let mut track = Vec::new();
        let mut prev = 0u32;
        for (tick, ev) in &events {
            write_varlen(&mut track, tick - prev);
            track.extend_from_slice(ev);
            prev = *tick;
        }
        write_varlen(&mut track, 0);
        track.extend_from_slice(&[0xFF, 0x2F, 0x00]);
        write_chunk(&mut out, b"MTrk", &track);
    }
    out
}

pub fn export_midi(song: &Song, path: &Path) -> Result<(), String> {
    std::fs::write(path, export_midi_bytes(song)).map_err(|e| {
        crate::tr(&format!("Failed to write MIDI: {e}"), &format!("写入 MIDI 失败: {e}")).to_string()
    })?;
    Ok(())
}

/// 原版 `endb2 = yy` 之后界面上的层数就是这么多，这里保证 `layers` 至少长到 n。
fn grow_layers(song: &mut Song, n: usize) {
    while song.layers.len() < n {
        song.layers.push(Layer::default());
    }
}
