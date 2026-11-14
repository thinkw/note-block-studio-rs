//! 偏好设置、最近文件、自动保存与原子写入。
//! 原版用 `settings.ini`，这里用 JSON（serde），跨平台且类型安全。

use std::path::{Path, PathBuf};

use nbs_core::model::Song;
use serde::{Deserialize, Serialize};

const APP_DIR: &str = "nbs-rust";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u8,
    pub theme: u8,
    pub zoom: f32,
    pub master_volume: f32,
    pub play_speed: f32,
    pub metronome: bool,
    pub mono: bool,
    pub extended_voices: bool,
    pub include_locked_in_export: bool,
    pub follow_marker: bool,
    pub follow_center: bool,
    pub show_layers: bool,
    pub show_piano: bool,
    pub show_instruments: bool,
    pub use_colors: bool,
    pub use_shapes: bool,
    pub use_icons: bool,
    pub show_numbers: bool,
    pub show_octaves: bool,
    pub show_keynames: bool,
    pub show_incompatible: bool,
    pub show_grid_lines: bool,
    pub flats: bool,
    pub blackout: bool,
    pub sounds_dir: Option<String>,
    pub export_sample_rate: u32,
    pub export_channels: u16,
    /// 音频导出容器，对齐原版 `audio_exp_format`：MP3 / WAV / AIFF / FLAC。
    /// 归一化、尾部留白、余量这些原版没有开关的参数不在这里，用 `ExportOptions` 的默认值。
    pub export_format: String,
    /// 0 = English，1 = 简体中文。
    pub language: u8,
    pub auto_save_minutes: u8,
    pub recent_files: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            theme: 3,
            zoom: 1.0,
            master_volume: 1.0,
            play_speed: 1.0,
            metronome: false,
            mono: false,
            extended_voices: false,
            include_locked_in_export: false,
            follow_marker: true,
            follow_center: false,
            show_layers: true,
            show_piano: true,
            show_instruments: false,
            use_colors: true,
            use_shapes: true,
            use_icons: true,
            show_numbers: true,
            show_octaves: false,
            show_keynames: true,
            show_incompatible: true,
            show_grid_lines: true,
            flats: false,
            blackout: false,
            sounds_dir: None,
            export_sample_rate: 44100,
            export_channels: 2,
            export_format: "MP3".into(),
            language: 0,
            auto_save_minutes: 10,
            recent_files: Vec::new(),
        }
    }
}

impl Preferences {
    pub fn config_dir() -> PathBuf {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR);
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join(APP_DIR);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config").join(APP_DIR);
        }
        PathBuf::from(".").join(".nbs-rust")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("prefs.json")
    }

    pub fn backup_dir() -> PathBuf {
        Self::config_dir().join("Backups")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
        let json = serde_json::to_string_pretty(self).map_err(|e| format!("序列化失败: {e}"))?;
        // 原子写：先写 .tmp 再 rename，避免中断损坏配置
        let tmp = dir.join("prefs.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写入配置失败: {e}"))?;
        std::fs::rename(&tmp, Self::config_path())
            .map_err(|e| format!("替换配置失败: {e}"))
    }

    pub fn add_recent(&mut self, path: &Path) {
        let s = path.to_string_lossy().into_owned();
        self.recent_files.retain(|p| p != &s);
        self.recent_files.insert(0, s);
        self.recent_files.truncate(11);
    }

    pub fn sounds_dir_path(&self) -> Option<PathBuf> {
        self.sounds_dir.as_ref().map(PathBuf::from).filter(|p| p.is_dir())
    }
}

/// 原子保存歌曲：写临时文件再替换。
pub fn save_song_atomic(song: &Song, path: &Path) -> Result<(), String> {
    let version = song.meta.save_version;
    let bytes = nbs_core::nbs::save_bytes(song, version).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let tmp = path.with_extension("nbs.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换歌曲文件失败: {e}"))?;
    Ok(())
}

/// 自动保存一份备份（不改动当前文件名/脏标记）。
pub fn autosave(song: &Song, prefs: &Preferences) -> Result<PathBuf, String> {
    let dir = Preferences::backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    let base = song
        .filename
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let safe: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let path = dir.join(format!("{safe}.autosave.nbs"));
    save_song_atomic(song, &path)?;
    let _ = prefs;
    Ok(path)
}

/// 列出备份目录里的自动保存文件（恢复用）。
pub fn list_autosaves() -> Vec<PathBuf> {
    let dir = Preferences::backup_dir();
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("nbs"))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Note;

    #[test]
    fn prefs_roundtrip() {
        let mut p = Preferences::default();
        p.master_volume = 0.42;
        p.theme = 2;
        p.recent_files.push("C:/songs/demo.nbs".into());
        let s = serde_json::to_string(&p).unwrap();
        let back: Preferences = serde_json::from_str(&s).unwrap();
        assert_eq!(back.theme, 2);
        assert!((back.master_volume - 0.42).abs() < 1e-6);
        assert_eq!(back.recent_files.len(), 1);
    }

    #[test]
    fn atomic_save_roundtrips() {
        let mut song = Song::new();
        song.set(4, 0, Note::new(0, 45));
        let p = std::env::temp_dir().join("nbs_rust_atomic.nbs");
        save_song_atomic(&song, &p).unwrap();
        let back = nbs_core::nbs::load(&p).unwrap();
        assert_eq!(back.total_blocks, 1);
        assert!(!p.with_extension("nbs.tmp").exists());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn recent_files_deduplicates() {
        let mut p = Preferences::default();
        p.add_recent(Path::new("a.nbs"));
        p.add_recent(Path::new("b.nbs"));
        p.add_recent(Path::new("a.nbs"));
        assert_eq!(p.recent_files.len(), 2);
        assert_eq!(p.recent_files[0], "a.nbs");
    }
}
