//! `.nbp`（Note Block Pattern）选区片段格式，与 OpenNBS 互通。
//!
//! 布局（原版 `pattern_export.gml`）：
//! ```text
//! u8   pat_version (=1)
//! u16  enda        歌曲长度
//! u16  endb        歌曲高度
//! u16  selection_l 选区列数
//! cstr selection_code（明文 RLE，NUL 结尾）
//! selection_l × (i8 colfirst, i8 collast)   // -1/0xFF 表示空列
//! ```
//! 剪贴板里的 `selection_code` 可能是 zlib + base64 压缩形式，这里也提供兼容工具。

use std::io::{Read, Write};
use std::path::Path;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use nbs_core::model::Song;
use nbs_core::selection::Selection;

pub const PAT_VERSION: u8 = 1;

#[derive(Clone, Debug)]
pub struct Pattern {
    pub version: u8,
    pub enda: u16,
    pub endb: u16,
    pub selection_l: u16,
    pub code: String,
    pub col_bounds: Vec<(i8, i8)>,
}

impl Pattern {
    pub fn selection(&self) -> Selection {
        Selection::decode_rle(&self.code)
    }
}

pub fn export_pattern_bytes(sel: &Selection, song: &Song) -> Vec<u8> {
    let code = sel.encode_rle();
    let cols = sel.width.max(1) as usize;
    let mut bounds = vec![(-1i8, -1i8); cols];
    for n in &sel.notes {
        let dx = n.dx as usize;
        if dx < cols {
            let (first, last) = &mut bounds[dx];
            let dy = n.dy.min(127) as i8;
            if *first < 0 || dy < *first {
                *first = dy;
            }
            if dy > *last {
                *last = dy;
            }
        }
    }

    let mut out = Vec::new();
    out.push(PAT_VERSION);
    out.extend_from_slice(&(song.enda.min(u16::MAX as u32) as u16).to_le_bytes());
    out.extend_from_slice(&(song.endb.min(u16::MAX as u32) as u16).to_le_bytes());
    out.extend_from_slice(&(cols.min(u16::MAX as usize) as u16).to_le_bytes());
    out.extend_from_slice(code.as_bytes());
    out.push(0);
    for (a, b) in bounds {
        out.push(a as u8);
        out.push(b as u8);
    }
    out
}

pub fn export_pattern(sel: &Selection, song: &Song, path: &Path) -> Result<(), String> {
    std::fs::write(path, export_pattern_bytes(sel, song)).map_err(|e| {
        crate::tr(&format!("Failed to write pattern: {e}"), &format!("写入片段失败: {e}")).to_string()
    })
}

pub fn parse_pattern(data: &[u8]) -> Result<Pattern, String> {
    if data.len() < 8 {
        return Err(crate::tr("Pattern file too short", "片段文件过短").into());
    }
    let version = data[0];
    let enda = u16::from_le_bytes([data[1], data[2]]);
    let endb = u16::from_le_bytes([data[3], data[4]]);
    let selection_l = u16::from_le_bytes([data[5], data[6]]);
    let mut p = 7;
    while p < data.len() && data[p] != 0 {
        p += 1;
    }
    if p >= data.len() {
        return Err(crate::tr("Pattern missing string terminator", "片段缺少字符串结束符").into());
    }
    let code = String::from_utf8_lossy(&data[7..p]).into_owned();
    p += 1;
    let mut col_bounds = Vec::with_capacity(selection_l as usize);
    for _ in 0..selection_l {
        if p + 1 >= data.len() {
            break;
        }
        col_bounds.push((data[p] as i8, data[p + 1] as i8));
        p += 2;
    }
    Ok(Pattern { version, enda, endb, selection_l, code, col_bounds })
}

/// 原版 `pattern_import.gml:31-37`：读到的片段版本和本程序 `pat_version` 不一致时弹什么。
/// 旧版警告受首选项「打开旧版格式时提示」管，新版那句不受（源码就没管）。
/// 新版文案照抄 v3.12.0-rc.1（EN "This pattern was created in a newer version…"）。
pub fn version_warning(version: u8, show_old_warning: bool) -> Option<String> {
    if version < PAT_VERSION && show_old_warning {
        Some(crate::prefs::old_file_warning("NBP"))
    } else if version > PAT_VERSION {
        Some(
            crate::tr(
                "This pattern was created in a newer version of Note Block Studio and cannot be opened safely.",
                "此分段文件由新版 Note Block Studio 创建，无法安全打开。",
            )
            .into(),
        )
    } else {
        None
    }
}

/// 给回整个 `Pattern`（含 `version`），原版 `pattern_import` 要用版本号决定弹不弹警告。
pub fn import_pattern(path: &Path) -> Result<Pattern, String> {
    let data = std::fs::read(path).map_err(|e| {
        crate::tr(&format!("Failed to read pattern: {e}"), &format!("读取片段失败: {e}")).to_string()
    })?;
    parse_pattern(&data)
}

// ---------------------------------------------------------------- 压缩文本

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let v = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((v >> 18) & 63) as usize] as char);
        out.push(B64[((v >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64[((v >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[(v & 63) as usize] as char } else { '=' });
    }
    out
}

fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

/// 原版 `try_compress_selection`：长度 ≥16 时 zlib + base64。
pub fn compress_code(code: &str) -> String {
    if code.len() < 16 {
        return code.to_string();
    }
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    if enc.write_all(code.as_bytes()).is_err() {
        return code.to_string();
    }
    match enc.finish() {
        Ok(z) => base64_encode(&z),
        Err(_) => code.to_string(),
    }
}

/// 原版 `try_decompress_selection`：含 `|` 视为明文，否则 base64+zlib。
pub fn decompress_code(code: &str) -> String {
    if code.contains('|') || code.is_empty() {
        return code.to_string();
    }
    let z = base64_decode(code.trim());
    let mut dec = ZlibDecoder::new(&z[..]);
    let mut s = String::new();
    match dec.read_to_string(&mut s) {
        Ok(_) => s,
        Err(_) => code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Note;

    #[test]
    fn nbp_roundtrip() {
        let mut song = Song::new();
        song.set(3, 0, Note::new(0, 45));
        let sel = Selection::from_absolute(0, 0, &[
            (0, 0, Note::new(0, 45)),
            (0, 3, Note::new(1, 40)),
            (5, 1, Note::new(2, 50)),
        ]);
        let bytes = export_pattern_bytes(&sel, &song);
        assert_eq!(bytes[0], PAT_VERSION);
        let pat = parse_pattern(&bytes).unwrap();
        assert_eq!(pat.selection_l, 6);
        let back = pat.selection();
        assert_eq!(back.len(), 3);
        assert_eq!(back.at(0, 3), Some(Note::new(1, 40)));
        assert_eq!(back.at(5, 1), Some(Note::new(2, 50)));
    }

    #[test]
    fn compressed_code_roundtrip() {
        let code = "0|0|0|45|100|100|0|-1|3|1|1|40|100|100|0|-1|";
        let z = compress_code(code);
        assert!(!z.contains('|'));
        assert_eq!(decompress_code(&z), code);
    }

    #[test]
    fn reads_original_drum_example_nbp() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../NoteBlockStudio/datafiles/Patterns/Drum Example.nbp");
        if !path.exists() {
            return;
        }
        let data = std::fs::read(&path).unwrap();
        let pat = parse_pattern(&data).unwrap();
        assert_eq!(pat.version, 1);
        assert!(!pat.selection().is_empty(), "应能还原原版示例片段的音符");
    }

    /// 原版 `pattern_import.gml:31-37`：版本比 `pat_version` 旧的警告受首选项开关管，
    /// 比它新的那句「可能会崩溃」不受开关管。
    #[test]
    fn nbp_version_warnings() {
        let _lock = crate::TEST_LANG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        struct ResetLang(u8);
        impl Drop for ResetLang {
            fn drop(&mut self) {
                crate::set_language(self.0);
            }
        }
        let _lang = ResetLang(crate::lang());
        crate::set_language(1);
        assert_eq!(version_warning(1, true), None);
        assert_eq!(version_warning(1, false), None);
        assert_eq!(
            version_warning(0, true).as_deref(),
            Some(crate::prefs::old_file_warning("NBP").as_str())
        );
        assert_eq!(version_warning(0, false), None, "旧版警告要看「打开旧版格式时提示」");
        assert_eq!(
            version_warning(2, false).as_deref(),
            Some(crate::tr(
                "This pattern was created in a newer version of Note Block Studio and cannot be opened safely.",
                "此分段文件由新版 Note Block Studio 创建，无法安全打开。",
            ))
        );
    }
}
