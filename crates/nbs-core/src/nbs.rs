//! `.nbs` 二进制编解码（v0..v5）。
//!
//! 虽然本轮以「D. UI + 网格编辑器」为起点，但编辑器必须能真正打开/保存文件，
//! 因此这里先落地一个与 OpenNBS 3.11.0 (`nbs_version = 5`) 完全互通的最小 A 模块。
//! 文件结构：header + tick/layer 双 RLE 音符区 + 图层属性 + 自定义乐器。

use std::fmt;
use std::path::Path;

use crate::model::{
    CustomInstrument, Layer, LockMode, Note, Song, SongMeta, FIRST_CUSTOM_INDEX, KEY_MAX,
};

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
            Self::UnsupportedVersion(v) => write!(f, "不支持的 NBS 版本: {v}（最高支持 5）"),
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
        if version > 5 {
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
            // 运行时统一按 16 偏移，做一次重映射。
            // v0 老格式没有该字段（只有内置乐器），不能重映射。
            if version > 0 && ins >= first_custom && first_custom < FIRST_CUSTOM_INDEX {
                ins = FIRST_CUSTOM_INDEX + (ins - first_custom);
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
    let version = version.min(5);
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

    w.u8(song.custom_instruments.len().min(255) as u8);
    for ins in song.custom_instruments.iter().take(255) {
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
        for v in 0..=5 {
            roundtrip(v);
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
