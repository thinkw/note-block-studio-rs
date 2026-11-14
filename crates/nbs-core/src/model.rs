//! 歌曲数据模型：把原版 GML 的 8 个平行二维数组 `song_*[tick, layer]`
//! 收敛成一个稀疏的 `BTreeMap<tick, Column>`，并保留 `colfirst/collast/colamount`
//! 等价的缓存，让视口剔除可以做到 O(可见格) 而不是 O(全曲音符数)。

use std::collections::BTreeMap;
use std::path::PathBuf;

/// NBS 音高范围（88 键）。Minecraft 原生可播放区间是 33..=57。
pub const KEY_MIN: u8 = 0;
pub const KEY_MAX: u8 = 87;
pub const MC_KEY_MIN: u8 = 33;
pub const MC_KEY_MAX: u8 = 57;
pub const DEFAULT_KEY: u8 = 39;

/// 内置乐器数量；>= 此索引的均为自定义乐器。
pub const FIRST_CUSTOM_INDEX: u8 = 16;

/// 力度 / 声像 / 微调的取值上限（对应原版 median 夹取）。
pub const VEL_MAX: u8 = 100;
pub const PAN_CENTER: u8 = 100;
pub const PAN_MAX: u8 = 200;
pub const PIT_MAX: i16 = 1200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Note {
    /// 乐器索引（0..15 内置，>=16 自定义）。
    pub instrument: u8,
    /// 音高 0..=87。
    pub key: u8,
    /// 力度 0..=100。
    pub velocity: u8,
    /// 声像 0..=200，100 为中央。
    pub panning: u8,
    /// 微调音分 -1200..=1200。
    pub pitch: i16,
}

impl Note {
    pub const fn new(instrument: u8, key: u8) -> Self {
        Self {
            instrument,
            key,
            velocity: 100,
            panning: PAN_CENTER,
            pitch: 0,
        }
    }

    /// 夹取到 NBS 的合法范围。
    pub fn clamped(mut self) -> Self {
        self.key = self.key.min(KEY_MAX);
        self.velocity = self.velocity.min(VEL_MAX);
        self.panning = self.panning.min(PAN_MAX);
        self.pitch = self.pitch.clamp(-PIT_MAX, PIT_MAX);
        self
    }

    /// 是否超出 Minecraft 原生可播放范围（用于红框提示）。
    pub fn is_out_of_range(&self) -> bool {
        self.key < MC_KEY_MIN || self.key > MC_KEY_MAX || self.instrument >= FIRST_CUSTOM_INDEX
    }
}

impl Default for Note {
    fn default() -> Self {
        Self::new(0, DEFAULT_KEY)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LockMode {
    Normal = 0,
    Locked = 1,
    Solo = 2,
}

impl LockMode {
    pub fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Locked,
            2 => Self::Solo,
            _ => Self::Normal,
        }
    }
    pub fn as_byte(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Layer {
    pub name: String,
    pub lock: LockMode,
    /// 0..=100
    pub volume: u8,
    /// 0..=200，100 = 中央
    pub stereo: u8,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            name: String::new(),
            lock: LockMode::Normal,
            volume: 100,
            stereo: PAN_CENTER,
        }
    }
}

impl Layer {
    pub fn is_locked(&self, any_solo: bool) -> bool {
        match self.lock {
            LockMode::Locked => true,
            LockMode::Solo => false,
            LockMode::Normal => any_solo,
        }
    }
}

/// 一个 tick 列。按 layer 稀疏存放音符。
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Column {
    pub notes: BTreeMap<u32, Note>,
}

impl Column {
    /// 等价原版 `colfirst`；None 表示空列。
    pub fn first(&self) -> Option<u32> {
        self.notes.keys().next().copied()
    }
    /// 等价原版 `collast`。
    pub fn last(&self) -> Option<u32> {
        self.notes.keys().next_back().copied()
    }
    /// 等价原版 `colamount`。
    pub fn amount(&self) -> usize {
        self.notes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SongMeta {
    pub name: String,
    pub author: String,
    pub original_author: String,
    pub description: String,
    /// ticks / second，默认 10.0（= 150 BPM）。
    pub tempo: f32,
    /// 2..=8，每小节 = time_signature * 4 ticks。
    pub time_signature: u8,
    pub loop_enabled: bool,
    pub loop_max: u8,
    pub loop_start: u16,
    pub minutes_spent: i32,
    pub left_clicks: i32,
    pub right_clicks: i32,
    pub blocks_added: i32,
    pub blocks_removed: i32,
    pub midi_file: String,
    pub auto_save: bool,
    pub auto_save_mins: u8,
    /// 保存时使用的 NBS 版本号（0..=5）。
    pub save_version: u8,
}

impl Default for SongMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            author: String::new(),
            original_author: String::new(),
            description: String::new(),
            tempo: 10.0,
            time_signature: 4,
            loop_enabled: false,
            loop_max: 0,
            loop_start: 0,
            minutes_spent: 0,
            left_clicks: 0,
            right_clicks: 0,
            blocks_added: 0,
            blocks_removed: 0,
            midi_file: String::new(),
            auto_save: true,
            auto_save_mins: 10,
            save_version: 5,
        }
    }
}

impl SongMeta {
    pub fn bpm(&self) -> f32 {
        self.tempo * 15.0
    }
    pub fn ticks_per_bar(&self) -> u32 {
        self.time_signature as u32 * 4
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CustomInstrument {
    pub name: String,
    pub filename: String,
    /// 采样基准音高（默认 45）。
    pub key: u8,
    /// 按下时是否发声。
    pub press: bool,
}

#[derive(Clone, Debug)]
pub struct Song {
    /// tick -> 列。空 tick 不占条目，因此几十万 tick 的长曲子也不会爆炸。
    pub columns: BTreeMap<u32, Column>,
    pub layers: Vec<Layer>,
    pub meta: SongMeta,
    /// 最后一个含音符的 tick（缓存；空曲为 0）。
    pub enda: u32,
    /// 最后一个含音符的 layer（缓存；空曲为 0）。
    pub endb: u32,
    pub total_blocks: usize,
    pub custom_instruments: Vec<CustomInstrument>,
    pub filename: Option<PathBuf>,
    pub dirty: bool,
    /// 内容版本号：每次音符/图层/元数据变更递增。播放时据此决定要不要重建事件表，
    /// 空闲帧零开销，边播边改又能立刻发声。
    pub rev: u64,
}

impl Default for Song {
    fn default() -> Self {
        Self::new()
    }
}

impl Song {
    pub fn new() -> Self {
        Self {
            columns: BTreeMap::new(),
            layers: vec![Layer::default()],
            meta: SongMeta::default(),
            enda: 0,
            endb: 0,
            total_blocks: 0,
            custom_instruments: Vec::new(),
            filename: None,
            dirty: false,
            rev: 0,
        }
    }

    pub fn get(&self, tick: u32, layer: u32) -> Option<Note> {
        self.columns
            .get(&tick)
            .and_then(|c| c.notes.get(&layer))
            .copied()
    }

    pub fn has(&self, tick: u32, layer: u32) -> bool {
        self.get(tick, layer).is_some()
    }

    pub fn column(&self, tick: u32) -> Option<&Column> {
        self.columns.get(&tick)
    }

    /// 写入音符，返回被覆盖的旧音符（若有）。
    pub fn set(&mut self, tick: u32, layer: u32, note: Note) -> Option<Note> {
        let col = self.columns.entry(tick).or_default();
        let old = col.notes.insert(layer, note);
        if old.is_none() {
            self.total_blocks += 1;
        }
        self.enda = self.enda.max(tick);
        self.endb = self.endb.max(layer);
        self.dirty = true;
        old
    }

    pub fn remove(&mut self, tick: u32, layer: u32) -> Option<Note> {
        let mut old = None;
        let mut drop_col = false;
        if let Some(col) = self.columns.get_mut(&tick) {
            old = col.notes.remove(&layer);
            drop_col = col.notes.is_empty();
        }
        if old.is_some() {
            self.total_blocks = self.total_blocks.saturating_sub(1);
            self.dirty = true;
        }
        if drop_col {
            self.columns.remove(&tick);
        }
        old
    }

    /// 歌曲长度（最后一个音符 tick + 1）。
    pub fn song_length(&self) -> u32 {
        if self.total_blocks == 0 {
            0
        } else {
            self.enda + 1
        }
    }

    pub fn layer_count(&self) -> u32 {
        self.layers.len() as u32
    }

    /// 是否有任意层处于独奏态（对应原版 `solostr != ""`）。
    pub fn any_solo(&self) -> bool {
        self.layers.iter().any(|l| l.lock == LockMode::Solo)
    }

    /// 派生锁定表：有独奏层时，非独奏层全部锁定。
    pub fn locked_layers(&self) -> Vec<bool> {
        let solo = self.any_solo();
        self.layers.iter().map(|l| l.is_locked(solo)).collect()
    }

    pub fn is_locked(&self, layer: u32) -> bool {
        self.layers
            .get(layer as usize)
            .map(|l| l.is_locked(self.any_solo()))
            .unwrap_or(false)
    }

    /// 全量重算 enda/endb/total_blocks（批量修改或撤销图层操作后调用）。
    pub fn recalculate(&mut self) {
        self.enda = 0;
        self.endb = 0;
        self.total_blocks = 0;
        for (&tick, col) in &self.columns {
            self.enda = self.enda.max(tick);
            self.total_blocks += col.amount();
            if let Some(last) = col.last() {
                self.endb = self.endb.max(last);
            }
        }
        self.columns.retain(|_, c| !c.is_empty());
    }

    pub fn iter_notes(&self) -> impl Iterator<Item = (u32, u32, Note)> + '_ {
        self.columns.iter().flat_map(|(&t, c)| {
            c.notes
                .iter()
                .map(move |(&l, &n)| (t, l, n))
        })
    }

    /// 可见区间内的音符迭代器 —— 这是视口剔除的核心。
    /// 只遍历 `[tick_from, tick_to)` 的列，`BTreeMap::range` 是 O(log n + k)。
    pub fn notes_in_view(
        &self,
        tick_from: u32,
        tick_to: u32,
        layer_from: u32,
        layer_to: u32,
    ) -> Vec<(u32, u32, Note)> {
        let mut out = Vec::new();
        for (&tick, col) in self.columns.range(tick_from..tick_to) {
            if col.amount() == 0 {
                continue;
            }
            // colfirst / collast 三重早退，等价原版循环里的 break。
            if col.last().is_none_or(|last| last < layer_from) {
                continue;
            }
            if col.first().is_none_or(|first| first > layer_to) {
                continue;
            }
            for (&layer, &note) in col.notes.range(layer_from..=layer_to) {
                out.push((tick, layer, note));
            }
        }
        out
    }

    /// 把某一矩形区域内的音符收集为 `(tick, layer, Note)` 列表。
    pub fn notes_in_rect(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<(u32, u32, Note)> {
        if x1 < x0 || y1 < y0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (&tick, col) in self.columns.range(x0..=x1) {
            for (&layer, &note) in col.notes.range(y0..=y1) {
                out.push((tick, layer, note));
            }
        }
        out
    }

    /// 生成一首用于演示的测试曲（也方便打开程序就能看到效果）。
    pub fn demo() -> Self {
        let mut song = Self::new();
        song.meta.name = "Rust Rewrite Demo".into();
        song.meta.author = "nbs-rust".into();
        song.meta.description = "从 D 模块（UI + 网格编辑器）开始重写".into();
        song.meta.tempo = 10.0;
        song.layers[0].name = "Lead".into();
        song.layers.push(Layer { name: "Bass".into(), ..Default::default() });
        song.layers.push(Layer { name: "Drums".into(), ..Default::default() });

        // 一段简单的 C 大调音阶 + 低音 + 鼓点
        let scale = [45u8, 47, 49, 50, 52, 54, 56, 57];
        for (i, &key) in scale.iter().enumerate() {
            song.set((i * 2) as u32, 0, Note::new(0, key));
        }
        for bar in 0..4u32 {
            let t = bar * 16;
            song.set(t, 1, Note::new(1, 33 + (bar % 2) as u8 * 5));
            song.set(t + 8, 1, Note::new(1, 38));
            for beat in 0..4u32 {
                song.set(t + beat * 4, 2, Note::new(if beat % 2 == 0 { 2 } else { 3 }, 45));
            }
        }
        song.recalculate();
        song.dirty = false;
        song
    }
}

const KEY_NAMES_SHARP: [&str; 12] = [
    "A", "A#", "B", "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#",
];
const KEY_NAMES_FLAT: [&str; 12] = [
    "A", "Bb", "B", "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab",
];

/// 音名换算，与原版 `get_keyname` 一致：0 = 最低，33 = MC 最低，57 = MC 最高。
pub fn key_name(key: u8, show_octave: bool, flats: bool) -> String {
    let names = if flats { KEY_NAMES_FLAT } else { KEY_NAMES_SHARP };
    let name = names[(key as usize) % 12];
    if show_octave {
        let octave = (key as i32 - 3).div_euclid(12) + 1;
        format!("{name}{octave}")
    } else {
        name.to_string()
    }
}

/// 相对 MC 音域的半音数（状态栏显示用）。
pub fn key_offset(key: u8) -> i32 {
    key as i32 - 33
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_grid_insert_remove() {
        let mut s = Song::new();
        let n = Note::new(0, 45);
        assert!(s.set(10, 0, n).is_none());
        assert_eq!(s.total_blocks, 1);
        assert_eq!(s.enda, 10);
        assert_eq!(s.get(10, 0), Some(n));
        assert_eq!(s.remove(10, 0), Some(n));
        assert_eq!(s.total_blocks, 0);
        assert!(s.columns.is_empty());
    }

    #[test]
    fn culling_only_returns_visible_cells() {
        let mut s = Song::new();
        s.set(0, 0, Note::new(0, 45));
        s.set(100_000, 0, Note::new(0, 45));
        s.set(5, 9, Note::new(0, 45));
        let v = s.notes_in_view(0, 10, 0, 5);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, 0);
    }

    #[test]
    fn key_names_match_original() {
        assert_eq!(key_name(33, true, false), "F#3");
        assert_eq!(key_name(39, false, false), "C");
        // Minecraft 音符盒默认音高 key=45 = F#4；33 = F#3（最低），57 = F#5（最高）。
        assert_eq!(key_name(45, true, false), "F#4");
        assert_eq!(key_name(57, true, false), "F#5");
    }

    #[test]
    fn solo_locks_other_layers() {
        let mut s = Song::new();
        s.layers.push(Layer::default());
        s.layers[1].lock = LockMode::Solo;
        let locked = s.locked_layers();
        assert!(locked[0]);
        assert!(!locked[1]);
    }
}
