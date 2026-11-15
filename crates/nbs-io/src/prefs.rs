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
    /// 原版 `realvolume`：图层行里画不画音量小图标。
    pub show_layer_volumes: bool,
    /// 原版 `use_bpm`：速度框用 BPM 还是红石刻/秒。
    pub use_bpm: bool,
    pub extended_voices: bool,
    /// 原版 `show_soundcount`：状态栏要不要显示「声音数」。
    pub show_sound_count: bool,
    pub include_locked_in_export: bool,
    pub follow_marker: bool,
    /// 原版 `marker_pagebypage`：1 = 按页，0 = 按刻（居中），2 = 按帧（居中 + 亚刻位移）。
    pub follow_mode: u8,
    /// 原版 `autosave`：自动保存总开关，出厂是关的。间隔只在开启后才生效。
    pub autosave_enabled: bool,
    /// 原版 `mousewheel`：0 = 移动视口，1 = 换音色，2 = 换音调。
    pub mousewheel: u8,
    /// 原版 `changepitch`：滚轮停在音符上时改音符属性，而不是移动视口。
    pub changepitch: bool,
    /// 原版 `select_lastpressed`：键盘按下琴键后，选中的音跟到那个键。
    pub select_lastpressed: bool,
    pub show_layers: bool,
    pub show_piano: bool,
    pub use_colors: bool,
    pub use_shapes: bool,
    pub use_icons: bool,
    pub show_numbers: bool,
    pub show_octaves: bool,
    pub show_keynames: bool,
    /// 原版 `show_keynumbers`（默认关）：琴键上画 MC 音域内的序号。
    pub show_keynumbers: bool,
    /// 原版 `show_keyboard`（默认开）：琴键上画 `init_keys()` 绑定的按键字符。
    pub show_keyboard: bool,
    /// 原版 `show_outofrange`（默认开）：MC 两八度之外的琴键染粉红。
    pub show_outofrange: bool,
    /// 原版 `fdark`（默认关）：Fluent 主题的深色变体，只对 theme 3 生效。
    pub fdark: bool,
    /// 原版 `keysmax` / `keys_to_show`：钢琴最多画几根白键，20..50。
    pub keys_to_show: u8,
    pub show_incompatible: bool,
    pub show_grid_lines: bool,
    pub flats: bool,
    pub blackout: bool,
    pub sounds_dir: Option<String>,
    /// 原版 `show_oldwarning`（出厂为 1）：打开旧版本存的文件时弹不弹警告。
    pub show_old_warning: bool,
    /// 原版 `songfolder`：打开 / 保存歌曲对话框的起始目录，`None` = 出厂的 `工作目录\Songs\`。
    pub song_folder: Option<String>,
    /// 原版 `patternfolder`：导入 / 导出片段的起始目录，`None` = 出厂的 `工作目录\Patterns\`。
    pub pattern_folder: Option<String>,
    pub export_sample_rate: u32,
    pub export_channels: u16,
    /// 音频导出容器，对齐原版 `audio_exp_format`：MP3 / WAV / AIFF / FLAC。
    /// 归一化、尾部留白、余量这些原版没有开关的参数不在这里，用 `ExportOptions` 的默认值。
    pub export_format: String,
    /// 0 = English，1 = 简体中文。
    pub language: u8,
    pub auto_save_minutes: u8,
    /// 原版 `[schematic_export]` 段：整份导出选项无条件持久化。
    pub schematic: crate::schematic::SchematicOptions,
    /// 原版 `[warnings] schematics`：经典格式只兼容 1.12 以下的一次性警告是否已看过。
    pub warning_schematic: bool,
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
            show_layer_volumes: true,
            use_bpm: false,
            extended_voices: false,
            show_sound_count: false,
            include_locked_in_export: false,
            follow_marker: true,
            follow_mode: 1,
            autosave_enabled: false,
            mousewheel: 0,
            changepitch: true,
            select_lastpressed: true,
            show_layers: true,
            show_piano: true,
            use_colors: true,
            // 原版 `control_create.gml:246-248`：use_colors=1、use_icons=0、use_shapes=0。
            use_shapes: false,
            use_icons: false,
            show_numbers: true,
            show_octaves: false,
            show_keynames: true,
            show_keynumbers: false,
            show_keyboard: true,
            show_outofrange: true,
            fdark: false,
            keys_to_show: 30,
            show_incompatible: true,
            show_grid_lines: true,
            flats: false,
            blackout: false,
            sounds_dir: None,
            // 原版 `control_create.gml:126`：show_oldwarning = 1。
            show_old_warning: true,
            song_folder: None,
            pattern_folder: None,
            export_sample_rate: 44100,
            export_channels: 2,
            export_format: "MP3".into(),
            language: 0,
            auto_save_minutes: 10,
            schematic: crate::schematic::SchematicOptions::default(),
            warning_schematic: false,
            recent_files: Vec::new(),
        }
    }
}

impl Preferences {
    /// 原版 `if (autosave && …)`：开关关掉后间隔就不参与倒计时。
    pub fn autosave_interval(&self) -> u8 {
        if self.autosave_enabled {
            self.auto_save_minutes
        } else {
            0
        }
    }

    pub fn config_dir() -> PathBuf {
        // 无头校验（examples/uipng.rs）需要一份和真实用户配置隔离的设置，
        // 否则正在运行的 GUI 会把默认值覆盖回来。
        if let Ok(dir) = std::env::var("NBS_RUST_CONFIG_DIR") {
            return PathBuf::from(dir);
        }
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

    /// 原版宏的 `working_directory`：可执行文件所在目录。
    fn program_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 原版 `macros.gml:25`：`songs_directory = working_directory + "Songs\\"`。
    pub fn default_song_folder() -> PathBuf {
        Self::program_dir().join("Songs")
    }

    /// 原版 `macros.gml:26`：`pattern_directory = working_directory + "Patterns\\"`。
    pub fn default_pattern_folder() -> PathBuf {
        Self::program_dir().join("Patterns")
    }

    /// 原版 `load_song.gml:11` / `save_song.gml:13`：目录已经不在了就用出厂目录。
    pub fn song_folder_path(&self) -> PathBuf {
        existing_dir(self.song_folder.as_deref()).unwrap_or_else(Self::default_song_folder)
    }

    /// 原版 `pattern_import.gml:17` / `pattern_export.gml:9` 的同一句回退。
    pub fn pattern_folder_path(&self) -> PathBuf {
        existing_dir(self.pattern_folder.as_deref()).unwrap_or_else(Self::default_pattern_folder)
    }
}

fn existing_dir(s: Option<&str>) -> Option<PathBuf> {
    s.map(PathBuf::from).filter(|p| p.is_dir())
}

/// 原版 `open_song_nbs.gml:12-13` / `pattern_import.gml:31-32`：文件版本比本程序旧时的警告，
/// 受首选项「打开旧版格式时提示」管。文案照抄源码的中文分支，只换扩展名。
/// 比本程序新的文件不走这里：原版直接拒绝打开，我们的加载器已经报了错。
pub fn old_file_warning(ext: &str) -> String {
    format!("警告：你正在打开旧版的 {ext} 文件。保存此文件会使其与旧版 Note Block Studio 不兼容。")
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

    /// `[schematic_export]` 段：整份选项 + 「警告已看过」都要能落盘，
    /// 且旧文件里没有这两个键时必须退回默认值而不是报错。
    #[test]
    fn schematic_options_and_warning_flag_persist() {
        let mut p = Preferences::default();
        p.warning_schematic = true;
        p.schematic.layout = 2;
        p.schematic.structure = true;
        p.schematic.command_block = true;
        p.schematic.ins_block[0] = 99;
        p.schematic.ins_data[0] = 7;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(back.warning_schematic);
        assert_eq!(back.schematic.layout, 2);
        assert!(back.schematic.structure);
        assert!(back.schematic.command_block);
        assert_eq!(back.schematic.ins_block[0], 99);
        assert_eq!(back.schematic.ins_data[0], 7);

        let old: Preferences = serde_json::from_str(r#"{"theme":2}"#).unwrap();
        assert!(!old.warning_schematic);
        assert_eq!(old.schematic.layout, 1, "旧文件缺 [schematic_export] 时退回原版默认");
        assert_eq!(old.schematic.ins_block, crate::schematic::default_ins_block());
        assert_eq!(old.theme, 2, "已有的键照常读出来");
    }

    /// 原版 `control_create.gml` 的出厂值：autosave=0、autosavemins=10、mousewheel=0、
    /// changepitch=1、select_lastpressed=1、marker_pagebypage=1。
    #[test]
    fn startup_defaults_match_original() {
        let p = Preferences::default();
        assert!(!p.autosave_enabled, "原版 autosave = 0");
        assert_eq!(p.auto_save_minutes, 10);
        assert_eq!(p.autosave_interval(), 0, "开关关掉就不该有间隔");
        assert_eq!(p.mousewheel, 0);
        assert!(p.changepitch);
        assert!(p.select_lastpressed);
        assert_eq!(p.follow_mode, 1, "原版默认按页跟随");
        let on = Preferences { autosave_enabled: true, ..Default::default() };
        assert_eq!(on.autosave_interval(), 10);
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

    /// 原版 `load_song.gml:11` / `save_song.gml:13`：没配过就用出厂目录，
    /// 配过但目录已经没了同样退回出厂目录。
    #[test]
    fn song_folder_falls_back_to_program_dir() {
        let tmp = std::env::temp_dir();
        let mut p = Preferences::default();
        assert_eq!(p.song_folder_path(), Preferences::default_song_folder());
        assert_eq!(p.pattern_folder_path(), Preferences::default_pattern_folder());
        p.song_folder = Some(tmp.to_string_lossy().into_owned());
        assert_eq!(p.song_folder_path(), tmp);
        p.pattern_folder = Some("Z:\\nbs-rust 不存在的目录".into());
        assert_eq!(p.pattern_folder_path(), Preferences::default_pattern_folder());
    }

    /// 原版 `open_song_nbs.gml:13` / `pattern_import.gml:37` 的中文文案。
    #[test]
    fn old_file_warning_text_matches_source() {
        assert_eq!(
            old_file_warning("NBS"),
            "警告：你正在打开旧版的 NBS 文件。保存此文件会使其与旧版 Note Block Studio 不兼容。"
        );
        assert!(old_file_warning("NBP").contains("NBP"));
    }
}
