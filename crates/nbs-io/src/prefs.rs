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
    /// 原版 accent1/2/3（`settings.ini [preferences]` 持久化，出厂 0,120,212，
    /// `load/save_settings.gml:63-67`）：界面强调色基色。改色乐器 / 彩虹不写这里。
    pub accent: [u8; 3],
    /// 原版 `keysmax` / `keys_to_show`：钢琴最多画几根白键，20..50。
    pub keys_to_show: u8,
    pub show_incompatible: bool,
    /// 原版 `fade`（出厂 0，`control_create.gml:131`）：勾上 = 音符盒不透明，
    /// 力度不再参与透明度，鼠标压格 / 选区也不再 +0.5。原版在 ini 里的键名是反着的
    /// `use_fading`（`load_settings.gml:38`），读写沿用同一个键才能和原版配置对得上。
    pub fade: bool,
    pub show_grid_lines: bool,
    pub flats: bool,
    pub blackout: bool,
    /// 原版 `remove_effect`（出厂 1，`control_create.gml:225`）：删除音符时的淡出边框。
    pub remove_effect: bool,
    /// 原版 `selection_alpha`（出厂 0.8，`control_create.gml:444`；落
    /// `settings.ini [preferences] selection_alpha`）：浮动选区精灵的透明度，
    /// 首选项里按百分数拖拽（10%..80%）。
    pub selection_alpha: f32,
    /// 原版 `show_notechart`（出厂 0，`control_create.gml:461`）：悬停琴键显示五线谱。
    pub show_notechart: bool,
    /// 原版 `refreshrate`（出厂 0，`control_create.gml:130`）：帧数上限档位。
    pub refresh_rate: u8,
    /// 原版 `window_scale`：界面缩放倍率。`None` = 跟随系统 DPI，
    /// 也就是原版的出厂值 `display_get_dpi_x() / 96`（`get_default_window_scale.gml`）。
    pub window_scale: Option<f32>,
    /// 原版 `window_icon`（出厂 0，`control_create.gml:59`）：真 = 用老图标换掉程序自带的图标。
    pub window_icon: bool,
    pub sounds_dir: Option<String>,
    /// 原版 `show_oldwarning`（出厂为 1）：打开旧版本存的文件时弹不弹警告。
    pub show_old_warning: bool,
    /// 原版 `songfolder`：打开 / 保存歌曲对话框的起始目录，`None` = 出厂的 `<用户配置目录>\songs\`。
    pub song_folder: Option<String>,
    /// 原版 `patternfolder`：导入 / 导出片段的起始目录，`None` = 出厂的 `<用户配置目录>\patterns\`。
    pub pattern_folder: Option<String>,
    pub export_sample_rate: u32,
    pub export_channels: u16,
    /// 音频导出容器，对齐原版 `audio_exp_format`：MP3 / WAV / AIFF / FLAC。
    /// 原版没有开关的参数（限幅、时间轴）不在首选项里，直接按 nbswave 的固定行为。
    pub export_format: String,
    /// 0 = English，1 = 简体中文。
    pub language: u8,
    pub auto_save_minutes: u8,
    /// 原版 `[schematic_export]` 段：整份导出选项无条件持久化。
    pub schematic: crate::schematic::SchematicOptions,
    /// 原版 `[midi_import]` 段：勾了「记住我的更改」后，导入窗口里改的通道/鼓映射
    /// 会写进 GM 表并持久化到这里。`None` = 没改过，用出厂表。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_gm: Option<crate::midi_gm::MidiGmPersist>,
    /// 原版 `[midi_import]` 段的 14 个 `w_midi_*` 标量：原版每次 `save_settings()`
    /// 都无条件写，因此导入过的用户下次看到的就是上次留下的勾。`None` = 出厂值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_import: Option<crate::midi::MidiImportPrefs>,
    /// 原版 `[warnings] schematics`：经典格式只兼容 1.12 以下的一次性警告是否已看过。
    pub warning_schematic: bool,
    /// 原版 `show_welcome`（出厂 1，`control_create.gml:175`）：开机显示欢迎窗。
    pub show_welcome: bool,
    /// 原版 `check_update`（Windows 直装出厂 1，`control_create.gml:172`）：开机检查更新。
    pub check_update: bool,
    /// 原版 `check_prerelease`（出厂 = is_prerelease，`control_create.gml:173`）：
    /// 除稳定版之外也检查开发版。我们的构建按稳定版算，出厂关。
    pub check_prerelease: bool,
    /// 原版 `[warnings] donate_banner_time`（出厂 −1，`control_create.gml:113`）：
    /// 捐赠横幅的静音到期日（unix 秒）。0 = 没静音过；当前日期晚于它才重新显示
    /// （`control_create.gml:728-733` 的 `date_compare_date(now, time) > 0`）。
    pub donate_banner_time: i64,
    /// 原版 `[preferences] last_version`（`save_settings.gml:8-9` 同段）：
    /// 上次运行时的版本号。与当前版本不同 → 开机走「更新成功」流程
    /// （`control_create.gml:747-753`），欢迎窗显示「更新成功！」并重新亮出捐赠横幅。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_version: Option<String>,
    /// 原版 `[recent]` 段的 11 条最近歌曲（`recent_song_N_path` / `recent_song_N_time`）。
    #[serde(default, deserialize_with = "recent_files_deser")]
    pub recent_files: Vec<RecentFile>,
}

/// 最近歌曲一条：路径 + 上次打开的时间（unix 秒，原版 `date_current_datetime()`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFile {
    pub path: String,
    pub time: i64,
}

/// 旧配置里 `recent_files` 是 `Vec<String>`（没时间戳）：照读，时间记 0
/// （欢迎窗会显示「0 秒前」，和原版 ini 读旧档的行为同级）。
fn recent_files_deser<'de, D>(d: D) -> Result<Vec<RecentFile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Paths(Vec<String>),
        Entries(Vec<RecentFile>),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Paths(v) => v.into_iter().map(|path| RecentFile { path, time: 0 }).collect(),
        Raw::Entries(v) => v,
    })
}

/// 当前时间的 unix 秒。原版的 `date_current_datetime()` 是天数浮点，我们统一用秒。
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 原版 `date_inc_month(now, 1)`：同一天下个月的今天，时间部分保持不变；
/// 目标月没有这一天时截到月末（1-31 → 2-28/29）。
pub fn inc_month(unix: i64) -> i64 {
    let days = unix.div_euclid(86400);
    let secs = unix.rem_euclid(86400);
    // 天序号 → 公历年月日（Howard Hinnant 的 civil_from_days）。
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let d = d.min(month_len(ny, nm));
    // 公历年月日 → 天序号（civil_from_days 的逆）。
    let yy = if nm <= 2 { ny - 1 } else { ny };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * (if nm > 2 { nm - 3 } else { nm + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + secs
}

fn month_len(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            theme: 3,
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
            accent: [0, 120, 212],
            keys_to_show: 30,
            show_incompatible: true,
            fade: false,
            show_grid_lines: true,
            flats: false,
            blackout: false,
            remove_effect: true,
            selection_alpha: 0.8,
            show_notechart: false,
            // 原版 `control_create.gml:130`：refreshrate = 0，即 30 帧。
            refresh_rate: 0,
            window_scale: None,
            // 原版 `control_create.gml:59`：window_icon = 0，即用程序自带的图标。
            window_icon: false,
            sounds_dir: None,
            // 原版 `control_create.gml:126`：show_oldwarning = 1。
            show_old_warning: true,
            song_folder: None,
            pattern_folder: None,
            export_sample_rate: 44100,
            export_channels: 2,
            export_format: "MP3".into(),
            language: detect_language(),
            auto_save_minutes: 10,
            schematic: crate::schematic::SchematicOptions::default(),
            midi_gm: None,
            midi_import: None,
            warning_schematic: false,
            // 原版 `control_create.gml:172-175`：check_update = 1（Windows）、
            // check_prerelease = is_prerelease（我们的构建按稳定版算 = 0）、
            // show_welcome = 1；donate_banner = 1、donate_banner_time = −1（:112-113）。
            show_welcome: true,
            check_update: true,
            check_prerelease: false,
            donate_banner_time: 0,
            last_version: None,
            recent_files: Vec::new(),
        }
    }
}

/// 原版 `control_create.gml:198`：
/// `language = 1 * (os_get_language() = "zh" && os_get_region() = "CN")`
/// ——首次开机按系统语言自动决定界面用英文还是简体中文。这里用
/// GetUserDefaultLocaleName 拿 BCP-47 语言标签，交给 parse_language 判定。
fn detect_language() -> u8 {
    #[cfg(windows)]
    {
        use winapi::um::winnls::GetUserDefaultLocaleName;
        // LOCALE_NAME_MAX_LENGTH = 85。
        let mut buf = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 {
            let s = String::from_utf16_lossy(&buf[..len as usize]);
            return parse_language(&s);
        }
    }
    0
}

/// 判定一个语言标签是不是「中文（中国大陆）」：`zh-cn`、`zh-hans-cn` 都算，
/// `zh-tw` / `zh` / 英文标签不算——原版要求语言 zh 且地区 CN 两者同时成立。
pub fn parse_language(locale: &str) -> u8 {
    let s = locale.trim().to_ascii_lowercase();
    if s.starts_with("zh-") && s.ends_with("-cn") {
        1
    } else {
        0
    }
}

/// 原版 `control_draw.gml:47-50` 的 `game_set_speed` 档位表，也是首选项下拉里的五项。
/// 0 档源码不调 `game_set_speed`，走的是房间默认的 30 帧，下拉里同样写「30帧」。
pub const REFRESH_RATES: [u32; 5] = [30, 60, 120, 144, 240];

impl Preferences {
    /// 原版 `if (autosave && …)`：开关关掉后间隔就不参与倒计时。
    pub fn autosave_interval(&self) -> u8 {
        if self.autosave_enabled {
            self.auto_save_minutes
        } else {
            0
        }
    }

    /// 当前档位的帧数上限（手改过的越界档位按最后一档算）。
    pub fn refresh_limit(&self) -> f64 {
        REFRESH_RATES[self.refresh_rate as usize % REFRESH_RATES.len()] as f64
    }

    /// 生效中的界面缩放倍率。`None` 就是原版的 `get_default_window_scale()`，
    /// Windows 上等于系统 DPI / 96，也就是传进来的 `native`。
    pub fn effective_scale(&self, native: f32) -> f32 {
        self.window_scale.unwrap_or(native)
    }

    /// 原版「重置」按钮的锁定条件 `window_scale == get_default_window_scale()`。
    /// 拖动只能落在整数百分比上，落在系统倍率上时和出厂值等价，按钮照样变灰。
    pub fn scale_is_default(&self, native: f32) -> bool {
        (self.effective_scale(native) - native).abs() < 0.005
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

    /// 原版 `add_to_recent.gml`：去重挪到最前，时间戳记打开时刻，11 条封顶。
    pub fn add_recent(&mut self, path: &Path) {
        let s = path.to_string_lossy().into_owned();
        self.recent_files.retain(|p| p.path != s);
        self.recent_files.insert(0, RecentFile { path: s, time: unix_now() });
        self.recent_files.truncate(11);
    }

    pub fn sounds_dir_path(&self) -> Option<PathBuf> {
        self.sounds_dir.as_ref().map(PathBuf::from).filter(|p| p.is_dir())
    }

    /// 原版 `macros.gml:35`：`songs_directory = file_directory + "songs\\"`，而
    /// `file_directory = game_save_id`，在 Windows 上就是 `%APPDATA%\Minecraft_Note_Block_Studio\`
    /// （装机版实测该目录里就有 `songs`、`patterns`、`data`、`settings_dev.ini`）。
    /// 所以默认目录跟着用户配置目录走，不是可执行文件旁边——那只是原版的
    /// `bundled_songs_directory`（随程序附带的示例曲），两者不能混。
    pub fn default_song_folder() -> PathBuf {
        Self::config_dir().join("songs")
    }

    /// 原版 `macros.gml:36`：`pattern_directory = file_directory + "patterns\\"`。
    pub fn default_pattern_folder() -> PathBuf {
        Self::config_dir().join("patterns")
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

    /// 原版 `os_get_language() = "zh" && os_get_region() = "CN"`：
    /// 语言 zh 且地区 CN 同时成立才算中文，zh-tw / 裸 zh / 英文都不算。
    #[test]
    fn parse_language_requires_zh_and_cn_region() {
        assert_eq!(parse_language("zh-CN"), 1);
        assert_eq!(parse_language("zh-cn"), 1);
        assert_eq!(parse_language("zh-Hans-CN"), 1);
        assert_eq!(parse_language(" zh-CN "), 1);
        assert_eq!(parse_language("zh-TW"), 0);
        assert_eq!(parse_language("zh-Hant-TW"), 0);
        assert_eq!(parse_language("zh"), 0);
        assert_eq!(parse_language("zh-Hans"), 0);
        assert_eq!(parse_language("en-US"), 0);
        assert_eq!(parse_language(""), 0);
        assert_eq!(parse_language("cn-zh"), 0);
    }

    #[test]
    fn prefs_roundtrip() {
        let mut p = Preferences::default();
        p.master_volume = 0.42;
        p.theme = 2;
        p.refresh_rate = 3;
        p.window_scale = Some(1.25);
        p.window_icon = true;
        p.show_welcome = false;
        p.check_update = false;
        p.check_prerelease = true;
        p.donate_banner_time = 1760000000;
        p.last_version = Some("0.1.0".into());
        p.recent_files.push(crate::prefs::RecentFile {
            path: "C:/songs/demo.nbs".into(),
            time: 1760000001,
        });
        let s = serde_json::to_string(&p).unwrap();
        let back: Preferences = serde_json::from_str(&s).unwrap();
        assert_eq!(back.theme, 2);
        assert!((back.master_volume - 0.42).abs() < 1e-6);
        assert_eq!(back.recent_files.len(), 1);
        assert_eq!(back.recent_files[0].path, "C:/songs/demo.nbs");
        assert_eq!(back.recent_files[0].time, 1760000001);
        assert_eq!(back.refresh_rate, 3);
        assert_eq!(back.window_scale, Some(1.25));
        assert!(back.window_icon, "旧图标开关要落盘（原版 save_settings.gml:73）");
        assert!(back.remove_effect, "删除效果出厂打开（control_create.gml:225）");
        assert!(!back.show_notechart, "五线谱出厂关闭（control_create.gml:461）");
        assert!(
            (back.selection_alpha - 0.8).abs() < 1e-6,
            "选区高亮强度出厂 0.8（control_create.gml:444）"
        );
        assert!(!back.show_welcome, "欢迎窗开关要落盘");
        assert!(!back.check_update, "检查更新开关要落盘");
        assert!(back.check_prerelease, "开发版检查要落盘");
        assert_eq!(back.donate_banner_time, 1760000000, "横幅静音到期日要落盘");
        assert_eq!(back.last_version.as_deref(), Some("0.1.0"), "上次版本号要落盘");
        // 旧配置文件里没有这几项时必须退回默认值，不能整个文件读不出来。
        let old = r#"{"version":1,"theme":3}"#;
        let back: Preferences = serde_json::from_str(old).unwrap();
        assert_eq!(back.refresh_rate, 0, "出厂就是 30 帧");
        assert_eq!(back.window_scale, None, "出厂跟随系统 DPI");
        assert!(!back.window_icon, "出厂用程序自带的图标");
        assert_eq!(back.accent, [0, 120, 212], "强调色基色出厂 0,120,212");
        assert!(back.remove_effect, "旧配置缺键时退回出厂打开");
        assert!(!back.show_notechart, "旧配置缺键时五线谱退回关闭");
        assert!(
            (back.selection_alpha - 0.8).abs() < 1e-6,
            "旧配置缺键时选区强度退回 0.8"
        );
        // 启动组出厂值（control_create.gml:172-175）。
        let fresh = Preferences::default();
        assert!(fresh.show_welcome, "出厂显示欢迎窗");
        assert!(fresh.check_update, "Windows 直装出厂检查更新");
        assert!(!fresh.check_prerelease, "稳定版出厂不查开发版");
        assert_eq!(fresh.donate_banner_time, 0, "出厂没静音过横幅（原版 −1）");
        assert_eq!(fresh.last_version, None);
        // 旧格式的 recent_files（纯字符串数组）也要能读回来，时间记 0。
        let legacy = r#"{"recent_files":["C:/a.nbs","C:/b.nbs"]}"#;
        let back: Preferences = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.recent_files.len(), 2);
        assert_eq!(back.recent_files[0].path, "C:/a.nbs");
        assert_eq!(back.recent_files[0].time, 0, "旧格式没有时间戳");
        let mut p = Preferences::default();
        p.remove_effect = false;
        p.selection_alpha = 0.35;
        p.show_notechart = true;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(!back.remove_effect, "删除效果开关要落盘");
        assert!((back.selection_alpha - 0.35).abs() < 1e-6, "选区强度要落盘");
        assert!(back.show_notechart, "五线谱开关要落盘");
        let mut p = Preferences::default();
        p.accent = [10, 20, 30];
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.accent, [10, 20, 30], "accent1/2/3 要落盘（save_settings.gml:65-67）");
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
        assert_eq!(p.recent_files[0].path, "a.nbs", "重复打开要挪到最前");
        assert!(p.recent_files[0].time > 0, "时间戳记打开时刻（原版 add_to_recent.gml:18）");
        assert!(p.recent_files[0].time >= p.recent_files[1].time);
    }

    /// 原版 `date_inc_month(now, 1)`：同日下月，月末截断，闰年单独看 2 月。
    #[test]
    fn inc_month_clamps_to_month_end() {
        /// 当月第 d 天 01:00:00Z 的 unix 秒（2024 闰年 / 2025、2026 平年）。
        let day = |days_since_epoch: i64| days_since_epoch * 86400 + 3600;
        let (d2026_01_01, d2024_01_01) = (20454, 19723);
        // 1-15 → 2-15。
        assert_eq!(
            inc_month(day(d2026_01_01 + 14)) / 86400,
            d2026_01_01 + 31 + 14,
            "2026-01-15 → 2026-02-15"
        );
        // 月末截断：2026-01-31 → 2026-02-28；2024-01-31 → 2024-02-29（闰年）。
        assert_eq!(inc_month(day(d2026_01_01 + 30)) / 86400, d2026_01_01 + 31 + 27);
        assert_eq!(inc_month(day(d2024_01_01 + 30)) / 86400, d2024_01_01 + 31 + 28);
        // 12 月滚下一年；时间部分（这里 1 小时）保持。
        let dec = 1767193261i64; // 2025-12-31T15:01:01Z
        let next = inc_month(dec);
        assert_eq!(next / 86400, 20484, "2025-12-31 → 2026-01-31");
        assert_eq!(next.rem_euclid(86400), dec.rem_euclid(86400), "时间部分不变");
    }

    /// 原版 `load_song.gml:11` / `save_song.gml:13`：没配过就用出厂目录，
    /// 配过但目录已经没了同样退回出厂目录。
    #[test]
    fn song_folder_falls_back_to_default_dir() {
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

    /// 原版 `control_draw.gml:47-50` + 首选项下拉：0..4 = 30 / 60 / 120 / 144 / 240。
    #[test]
    fn refresh_rate_maps_to_original_frames() {
        let mut p = Preferences::default();
        assert_eq!(
            p.refresh_limit(),
            30.0,
            "出厂 refreshrate = 0（control_create.gml:130），对应 30 帧"
        );
        for (rate, fps) in [(1u8, 60.0), (2, 120.0), (3, 144.0), (4, 240.0)] {
            p.refresh_rate = rate;
            assert_eq!(p.refresh_limit(), fps);
        }
    }

    /// 原版出厂的 `window_scale` 是 `dpi / 96`，「重置」按钮只在两者相等时锁住。
    #[test]
    fn window_scale_default_tracks_native_dpi() {
        let mut p = Preferences::default();
        assert_eq!(p.effective_scale(1.25), 1.25, "没配过就跟随系统 DPI");
        assert!(p.scale_is_default(1.25));
        p.window_scale = Some(1.25);
        assert!(p.scale_is_default(1.25), "拖回系统倍率后「重置」同样锁住");
        p.window_scale = Some(2.0);
        assert_eq!(p.effective_scale(1.25), 2.0);
        assert!(!p.scale_is_default(1.25));
    }
}
