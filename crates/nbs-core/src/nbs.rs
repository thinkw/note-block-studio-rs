//! `.nbs` 二进制编解码（v0..v6）。
//!
//! 文件结构：header + tick/layer 双 RLE 音符区 + 图层属性 + 自定义乐器。
//! v6（装机版 NBS 3.12）相对 v5 没有增删任何字段，只是内置乐器 16 → 20，
//! 官方格式说明：<https://noteblock.studio/nbs>。

use std::fmt;
use std::path::Path;

use crate::model::{
    CustomInstrument, Layer, LockMode, Note, Song, SongMeta, FIRST_CUSTOM_INDEX, KEY_MAX,
};

/// 本程序自身的 NBS 版本（原版 `nbs_version`）：新建曲默认写到这个版本。
pub const NBS_VERSION: u8 = 6;
/// 能读取的最高版本。
pub const MAX_NBS_VERSION: u8 = 6;

#[derive(Debug)]
pub enum NbsError {
    Io(std::io::Error),
    UnexpectedEof(&'static str),
    UnsupportedVersion(u8),
}

impl fmt::Display for NbsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO 错误: {e}"),
            Self::UnexpectedEof(what) => write!(f, "文件意外结束（读取 {what} 时）"),
            Self::UnsupportedVersion(v) => {
                write!(f, "不支持的 NBS 版本: {v}（最高支持 {MAX_NBS_VERSION}）")
            }
        }
    }
}

impl std::error::Error for NbsError {}

impl From<std::io::Error> for NbsError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], NbsError> {
        if self.remaining() < n {
            return Err(NbsError::UnexpectedEof(what));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, NbsError> {
        Ok(self.take(1, what)?[0])
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, NbsError> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self, what: &'static str) -> Result<i16, NbsError> {
        Ok(self.u16(what)? as i16)
    }
    fn i32(&mut self, what: &'static str) -> Result<i32, NbsError> {
        let b = self.take(4, what)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Result<String, NbsError> {
        let len = self.i32("字符串长度")?.max(0) as usize;
        let b = self.take(len, "字符串内容")?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }
}

struct Writer {
    data: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { data: Vec::with_capacity(4096) }
    }
    fn u8(&mut self, v: u8) {
        self.data.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }
    fn i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }
    fn string(&mut self, s: &str) {
        let b = s.as_bytes();
        self.i32(b.len() as i32);
        self.data.extend_from_slice(b);
    }
}

/// 从字节解析一首歌。
pub fn load_bytes(bytes: &[u8]) -> Result<Song, NbsError> {
    let mut r = Reader::new(bytes);
    let mut song = Song::new();

    // 前两个字节为 0 表示带版本号的新格式；否则是 v0 老格式（首 u16 = 歌曲长度）。
    let first = r.u16("格式标志")?;
    let version: u8;
    let first_custom: u8;
    if first == 0 {
        version = r.u8("NBS 版本")?;
        if version > MAX_NBS_VERSION {
            return Err(NbsError::UnsupportedVersion(version));
        }
        first_custom = r.u8("首个自定义乐器索引")?;
        if version >= 3 {
            let _length = r.u16("歌曲长度")?;
        }
    } else {
        version = 0;
        first_custom = 0;
    }

    let layer_count = r.u16("图层数")? as usize;
    let mut meta = SongMeta::default();
    meta.name = r.string()?;
    meta.author = r.string()?;
    meta.original_author = r.string()?;
    meta.description = r.string()?;
    meta.tempo = (r.u16("速度")? as f32 / 100.0).max(0.01);
    meta.auto_save = r.u8("自动保存")? != 0;
    meta.auto_save_mins = r.u8("自动保存间隔")?;
    meta.time_signature = r.u8("拍号")?.clamp(2, 8);
    meta.minutes_spent = r.i32("工作时间")?.max(0);
    meta.left_clicks = r.i32("左键次数")?.max(0);
    meta.right_clicks = r.i32("右键次数")?.max(0);
    meta.blocks_added = r.i32("添加音符数")?.max(0);
    meta.blocks_removed = r.i32("删除音符数")?.max(0);
    meta.midi_file = r.string()?;
    if version >= 4 {
        meta.loop_enabled = r.u8("循环开关")? != 0;
        meta.loop_max = r.u8("循环次数")?;
        meta.loop_start = r.u16("循环起点")?;
    }
    meta.save_version = version;
    song.meta = meta;

    // 音符区：tick 与 layer 都是相对跳跃 RLE。
    let mut tick: i64 = -1;
    loop {
        let jump = r.u16("tick 跳跃")?;
        if jump == 0 {
            break;
        }
        tick += jump as i64;
        let mut layer: i64 = -1;
        loop {
            let ljump = r.u16("layer 跳跃")?;
            if ljump == 0 {
                break;
            }
            layer += ljump as i64;
            let mut ins = r.u8("乐器")?;
            let key = r.u8("音高")?.min(KEY_MAX);
            let (velocity, panning, pitch) = if version >= 4 {
                (
                    r.u8("力度")?,
                    r.u8("声像")?,
                    r.i16("微调")?,
                )
            } else {
                (100, 100, 0)
            };
            // 文件里的自定义乐器索引基于文件自身的 first_custom_index，
            // 运行时统一按 FIRST_CUSTOM_INDEX（v6 起是 20）偏移，做一次重映射。
            // v0 老格式没有该字段（只有内置乐器），不能重映射。
            // 旧文件最多可带 240 个自定义乐器，索引 16 起算会越过 u8 的上限，
            // 所以整体换算到 usize 再夹回 255，不能让算术溢出。
            if version > 0 && ins >= first_custom && first_custom < FIRST_CUSTOM_INDEX {
                let shifted = FIRST_CUSTOM_INDEX as usize + (ins - first_custom) as usize;
                ins = shifted.min(u8::MAX as usize) as u8;
            }
            if tick >= 0 && layer >= 0 {
                song.set(
                    tick as u32,
                    layer as u32,
                    Note { instrument: ins, key, velocity, panning, pitch },
                );
            }
        }
    }

    // 图层属性。
    song.layers.clear();
    for _ in 0..layer_count {
        let mut layer = Layer::default();
        layer.name = r.string()?;
        if version >= 4 {
            layer.lock = LockMode::from_byte(r.u8("图层锁定")?);
        }
        let vol = r.u8("图层音量")?;
        layer.volume = if vol == 255 { 100 } else { vol.min(100) };
        if version >= 2 {
            layer.stereo = r.u8("图层声像")?;
        }
        song.layers.push(layer);
    }
    if song.layers.is_empty() {
        song.layers.push(Layer::default());
    }

    // 自定义乐器（可选尾部）。
    if !r.eof() {
        let count = r.u8("自定义乐器数")?;
        for _ in 0..count {
            song.custom_instruments.push(CustomInstrument {
                name: r.string()?,
                filename: r.string()?,
                key: r.u8("采样基准音高")?,
                press: r.u8("按键发声")? != 0,
            });
        }
    }

    song.recalculate();
    song.dirty = false;
    Ok(song)
}

pub fn load(path: &Path) -> Result<Song, NbsError> {
    let bytes = std::fs::read(path)?;
    let mut song = load_bytes(&bytes)?;
    song.filename = Some(path.to_path_buf());
    Ok(song)
}

/// 序列化为 NBS 字节流。
pub fn save_bytes(song: &Song, version: u8) -> Result<Vec<u8>, NbsError> {
    let version = version.min(MAX_NBS_VERSION);
    // 自定义上限只由「写出去的首个自定义索引」决定：索引是 u8，从 FIRST_CUSTOM_INDEX
    // 起算就只剩 256-20 格（v6 只是把内置乐器从 16 扩到 20，没有新增字段）。
    // 原版 `save_song.gml:33,108` 也是直接写全局 first_custom_index 和 user_instruments。
    let max_custom = 256 - FIRST_CUSTOM_INDEX as usize;
    let mut w = Writer::new();
    let m = &song.meta;

    if version >= 1 {
        w.u16(0);
        w.u8(version);
        w.u8(FIRST_CUSTOM_INDEX);
    }
    if version == 0 || version >= 3 {
        w.u16(song.enda.min(u16::MAX as u32) as u16);
    }
    w.u16(song.layers.len().min(u16::MAX as usize) as u16);
    w.string(&m.name);
    w.string(&m.author);
    w.string(&m.original_author);
    w.string(&m.description);
    w.u16(((m.tempo * 100.0).round() as i32).clamp(0, u16::MAX as i32) as u16);
    w.u8(m.auto_save as u8);
    w.u8(m.auto_save_mins);
    w.u8(m.time_signature.clamp(2, 8));
    w.i32(m.minutes_spent);
    w.i32(m.left_clicks);
    w.i32(m.right_clicks);
    w.i32(m.blocks_added);
    w.i32(m.blocks_removed);
    w.string(&m.midi_file);
    if version >= 4 {
        w.u8(m.loop_enabled as u8);
        w.u8(m.loop_max);
        w.u16(m.loop_start);
    }

    // tick / layer 双 RLE。
    let mut prev_tick: i64 = -1;
    for (&tick, col) in &song.columns {
        if col.is_empty() {
            continue;
        }
        let mut gap = tick as i64 - prev_tick;
        // u16 跳跃上限保护：插入若干空列继续推进 tick。
        while gap > u16::MAX as i64 {
            w.u16(u16::MAX);
            w.u16(0); // 该列无音符，立即结束
            gap -= u16::MAX as i64;
        }
        w.u16(gap as u16);
        prev_tick = tick as i64;

        let mut prev_layer: i64 = -1;
        for (&layer, &note) in &col.notes {
            w.u16((layer as i64 - prev_layer) as u16);
            prev_layer = layer as i64;
            w.u8(note.instrument);
            w.u8(note.key.min(KEY_MAX));
            if version >= 4 {
                w.u8(note.velocity);
                w.u8(note.panning);
                w.i16(note.pitch);
            }
        }
        w.u16(0); // 列结束
    }
    w.u16(0); // 音符区结束

    for layer in &song.layers {
        w.string(&layer.name);
        if version >= 4 {
            w.u8(layer.lock.as_byte());
        }
        w.u8(layer.volume);
        if version >= 2 {
            w.u8(layer.stereo);
        }
    }

    w.u8(song.custom_instruments.len().min(max_custom) as u8);
    for ins in song.custom_instruments.iter().take(max_custom) {
        w.string(&ins.name);
        w.string(&ins.filename);
        w.u8(ins.key);
        w.u8(ins.press as u8);
    }
    Ok(w.data)
}

pub fn save(path: &Path, song: &Song, version: u8) -> Result<(), NbsError> {
    let bytes = save_bytes(song, version)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(version: u8) {
        let mut song = Song::demo();
        song.layers[0].name = "Lead 层".into();
        song.layers[2].lock = LockMode::Solo;
        let bytes = save_bytes(&song, version).unwrap();
        let back = load_bytes(&bytes).unwrap();
        assert_eq!(back.total_blocks, song.total_blocks);
        assert_eq!(back.layers.len(), song.layers.len());
        assert_eq!(back.meta.name, song.meta.name);
        assert_eq!((back.meta.tempo * 100.0).round(), (song.meta.tempo * 100.0).round());
        assert_eq!(back.get(0, 0), song.get(0, 0));
        assert_eq!(back.get(63, 2), song.get(63, 2));
    }

    #[test]
    fn roundtrip_all_versions() {
        for v in 0..=MAX_NBS_VERSION {
            roundtrip(v);
        }
    }

    /// v6 只改了「内置乐器 16 → 20」这一件事：字段布局和 v5 完全一致，
    /// 所以同一个歌曲存成 v5/v6 读回来必须一模一样，且首个自定义索引写的是 20。
    #[test]
    fn v6_only_moves_the_custom_instrument_boundary() {
        let song = Song::demo();
        for v in [5, 6] {
            let bytes = save_bytes(&song, v).unwrap();
            assert_eq!(bytes[2], v, "版本字节");
            assert_eq!(bytes[3], FIRST_CUSTOM_INDEX, "首个自定义乐器索引");
            let back = load_bytes(&bytes).unwrap();
            assert_eq!(back.total_blocks, song.total_blocks, "v{v} 音符数");
            assert_eq!(back.get(63, 2), song.get(63, 2), "v{v} 音符内容");
            assert_eq!(back.meta.save_version, v, "v{v} 记住来源版本");
        }
    }

    /// 旧的 v1..v5 文件把自定义乐器排在 16 之后，读进来要整体平移到 20 之后；
    /// 原版 `open_song_nbs.gml:99` 用的是同一个 `custom_index_diff` 加法。
    /// 索引越过 u8 时夹在 255，不能让加法 panic。
    #[test]
    fn pre_v6_custom_indices_are_shifted_by_four() {
        let mut song = Song::new();
        song.set(0, 0, Note::new(16, 45)); // 旧文件里的第 1 个自定义乐器
        song.set(1, 0, Note::new(255, 45)); // 越界索引
        let mut bytes = save_bytes(&song, 5).unwrap();
        bytes[3] = 16; // 伪装成 v5 时代的 first_custom_index
        let back = load_bytes(&bytes).unwrap();
        assert_eq!(back.get(0, 0).unwrap().instrument, 20, "旧自定义 0 号 → 20");
        assert_eq!(back.get(1, 0).unwrap().instrument, 255, "越界索引夹在 255");
    }

    /// 原版 open_song_nbs.gml:71 `if (song_nbs_version >= 4)` 才读循环三兄弟，
    /// save_song.gml 无条件写 —— 所以 v0..v3 读回来必须是默认值，不能串位。
    #[test]
    fn loop_metadata_roundtrips_only_from_v4() {
        let mut song = Song::demo();
        song.meta.loop_enabled = true;
        song.meta.loop_max = 3;
        song.meta.loop_start = 7;
        song.meta.minutes_spent = 42;
        song.meta.left_clicks = 11;
        song.meta.right_clicks = 5;
        song.meta.blocks_added = 99;
        song.meta.blocks_removed = 7;
        song.meta.midi_file = "a.mid".into();
        for v in 0..=MAX_NBS_VERSION {
            let back = load_bytes(&save_bytes(&song, v).unwrap()).unwrap();
            assert_eq!(back.meta.minutes_spent, 42, "v{v} 耗时");
            assert_eq!(back.meta.left_clicks, 11, "v{v} 左键");
            assert_eq!(back.meta.blocks_added, 99, "v{v} 添加数");
            assert_eq!(back.meta.midi_file, "a.mid", "v{v} MIDI 文件");
            if v >= 4 {
                assert!(back.meta.loop_enabled, "v{v} 循环开关应保留");
                assert_eq!(back.meta.loop_max, 3, "v{v} 循环次数");
                assert_eq!(back.meta.loop_start, 7, "v{v} 循环起点");
            } else {
                assert!(!back.meta.loop_enabled, "v{v} 不该读出循环开关");
                assert_eq!(back.meta.loop_max, 0, "v{v} 不该读出循环次数");
                assert_eq!(back.meta.loop_start, 0, "v{v} 不该读出循环起点");
            }
        }
    }

    /// 用原版仓库自带的真实歌曲验证互通（目录不存在时自动跳过）。
    #[test]
    fn opens_real_opennbs_songs() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../NoteBlockStudio/datafiles/Songs");
        if !dir.is_dir() {
            return;
        }
        let mut count = 0;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("nbs") {
                continue;
            }
            let song = load(&path).unwrap_or_else(|e| panic!("打开 {:?} 失败: {e}", path));
            assert!(song.total_blocks > 0, "{:?} 解析后没有音符", path);
            assert!(!song.layers.is_empty());
            // 用读到的版本重新序列化，再读回来，音符数与图层数必须一致
            let bytes = save_bytes(&song, song.meta.save_version).unwrap();
            let back = load_bytes(&bytes).unwrap();
            assert_eq!(back.total_blocks, song.total_blocks, "{:?} 音符数不一致", path);
            assert_eq!(back.layers.len(), song.layers.len(), "{:?} 图层数不一致", path);
            println!(
                "  {:?}: v{}, {} 音符, {} 图层, {:.2} t/s, {} ticks",
                path.file_name().unwrap_or_default(),
                song.meta.save_version,
                song.total_blocks,
                song.layers.len(),
                song.meta.tempo,
                song.song_length()
            );
            count += 1;
        }
        assert!(count > 0, "目录里没有找到 .nbs 文件");
    }

    #[test]
    fn large_tick_gap_survives() {
        // 超过 u16 跳跃上限的长曲子也要能正确保存。
        let mut song = Song::new();
        song.set(0, 0, Note::new(0, 45));
        song.set(140_000, 0, Note::new(1, 50));
        let bytes = save_bytes(&song, 5).unwrap();
        let back = load_bytes(&bytes).unwrap();
        assert_eq!(back.get(140_000, 0), Some(Note::new(1, 50)));
    }
}
