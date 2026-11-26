//! 偏好设置、最近文件、自动保存与原子写入。
//! 原版用 `settings.ini`，这里用 JSON（serde），跨平台且类型安全。

use std::path::{Path, PathBuf};

use nbs_core::model::Song;
use serde::{Deserialize, Serialize};

/// 图层栏出厂宽度 = 原版列宽 264（`x1 = 4` + `spr_layerbox` 宽 256）。
pub const LAYER_PANEL_DEFAULT_WIDTH: f32 = 264.0;
/// 图层栏最窄一档：再窄的话 7 个功能按钮（18px 一档、末档宽 16）就出栏了。
pub const LAYER_PANEL_MIN_WIDTH: f32 = 160.0;
/// 图层栏最宽一档：再宽就把网格挤得太狠。
pub const LAYER_PANEL_MAX_WIDTH: f32 = 420.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u8,
    pub theme: u8,
    pub master_volume: f32,
    pub play_speed: f32,
    pub metronome: bool,
    pub mono: bool,
    /// 端口自加（用户要求 #94）：点名的输出设备标识（cpal 的稳定 `DeviceId` 串，
    /// Windows 上是端点 ID，形如 `wasapi:{0.0.0.00000000}.{GUID}`），空 = 跟随系统默认。
    /// 原版把声音整个交给 GameMaker，只会落在系统默认设备上，没有这一项。
    /// 老版本存的是设备名（当时 cpal 0.15 不对外暴露稳定 ID），启动时按名字认一次换过来。
    pub audio_output: String,
    /// 原版 `realvolume`：图层行里画不画音量小图标。
    pub show_layer_volumes: bool,
    /// 原版 `use_bpm`：速度框用 BPM 还是红石刻/秒。
    pub use_bpm: bool,
    /// 原版 `tapdouble`（`control_create.gml:565` 出厂 0）：速度测量器里「双倍速度」
    /// 的勾选态。它和另外四条打点变量不一样，是写进 settings.ini 的
    /// （`save_settings.gml:71` / `load_settings.gml:78`），所以关窗、重开程序都还在。
    pub tapdouble: bool,
    pub extended_voices: bool,
    /// 原版 `show_soundcount`：状态栏要不要显示「声音数」。
    pub show_sound_count: bool,
    pub include_locked_in_export: bool,
    pub follow_marker: bool,
    /// 原版 `marker_pagebypage`：1 = 按页，0 = 按刻（居中），2 = 按帧（居中 + 亚刻位移）。
    pub follow_mode: u8,
    /// 原版 `marker_start`（`control_create.gml:381` 出厂开）：有区间时按播放从区间起点起播。
    pub marker_start: bool,
    /// 原版 `marker_end`（`control_create.gml:382` 出厂关）：播放头自然越过区间末端就停。
    pub marker_end: bool,
    /// 原版 `autosave`：自动保存总开关，出厂是关的。间隔只在开启后才生效。
    pub autosave_enabled: bool,
    /// 原版 `mousewheel`：0 = 移动视口，1 = 换音色，2 = 换音调。
    pub mousewheel: u8,
    /// 原版 `mousewheel_scroll_speed`（`control_create.gml:408` 出厂 = 1）：滚轮一格
    /// 走几层（纵向）/ 几刻（横向按住 Shift），1..=10，只在 `mousewheel = 0` 时可改。
    pub mousewheel_scroll_speed: u8,
    /// 原版 `changepitch`：滚轮停在音符上时改音符属性，而不是移动视口。
    pub changepitch: bool,
    /// 原版 `layerhov_vppreview`（`control_create.gml:410` 出厂 = 0）：滚轮换音色/换键
    /// 时的试听声用鼠标悬停那条层的音量和声道，而不是固定的 100/100。
    pub layerhov_vppreview: bool,
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
    /// 端口自加（用户要求 #119）：垂直同步。原版把呈现整个交给 GameMaker，
    /// 首选项里没有这一项；出厂值跟 eframe 自己的默认 `NativeOptions.vsync = true` 一致。
    /// 关掉后帧的节拍只剩上面那档「帧率限制」，不再被显示器刷新率压住。
    pub vsync: bool,
    /// 原版 `window_scale`：界面缩放倍率。`None` = 跟随系统 DPI，
    /// 也就是原版的出厂值 `display_get_dpi_x() / 96`（`get_default_window_scale.gml`）。
    pub window_scale: Option<f32>,
    /// 端口自加（用户要求）：琴键面板独立缩放倍率，出厂 1.0。原版钢琴恒 1:1，
    /// 不跟随 `window_scale`（工具栏 / 钢琴栏不随界面缩放）——这里同样独立，
    /// 只在鼠标悬停琴键面板时用 Ctrl+滚轮 / Ctrl+± 调整。
    pub piano_scale: f32,
    /// 端口自加（用户要求 #108）：图层栏宽度，屏幕像素，**不随 `window_scale`**。
    /// 原版整幅画面等比缩放，列宽 = 264 × `window_scale`，缩到 50% 时栏里只剩 132px，
    /// 头部那条时间、速度框、小节框全被挤出栏外压到标尺上。现在改成：网格照旧横竖一起缩，
    /// 侧栏只有纵轴（行高）跟缩放，横轴由用户拖右边界手动调，栏内版式按这个宽度自动布局。
    pub layer_panel_width: f32,
    /// 端口自加（用户要求 #92）：界面缩放低于这个百分比时，音符方块上的字（序号和键名）
    /// 一律不画。原版没有这一项 —— 原版缩放最低只到 50%，字挤成一坨也照样画；
    /// 我们把下限放开到 10% 后，用户要求低缩放时把字收掉只留方块。
    /// 默认 50：正好等于原版的最低档，所以在原版可达的缩放范围内行为和原版逐像素一致。
    pub note_text_min_percent: u8,
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
    /// 端口自加（用户要求接 MAESTRO 导出）：MAESTRO 机体节目单的导出选项。
    /// 原版没有这条导出，所以也没有对应的 `[..._export]` 段，键名用 `maestro`。
    pub maestro: crate::maestro::MaestroOptions,
    /// 端口自加（用户要求接 nbs-fractal 机体导出）：`.litematic` 机体投影的导出选项。
    pub fractal: crate::fractal::FractalOptions,
    /// 端口自加（用户要求接唱片工坊那一档）：编码唱片数据箱 `.litematic` 的导出选项。
    pub disc_chest: crate::disc_chest::DiscChestOptions,
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
    /// 端口自加：MAESTRO 那条「节目单要在已建好的机体里才出声」的一次性提示是否已看过。
    /// 语义同 `warning_schematic`（`false` = 还没提示过）。
    pub warning_maestro: bool,
    /// 端口自加：fractal 那条「投影里没有时钟也没有电源、喂电要自己接」的一次性提示。
    /// 键名沿用第一次加进来时的 `warning_fractal`；文案在 2026-11-23 从「贴下去就能响」改成了
    /// 实测口径，已经看过旧提示的人不会再见到的这句，需要时手动把这一项置回 `false`。
    pub warning_fractal: bool,
    /// 端口自加：编码唱片那条「只是数据箱，得贴在已建好的机体里才出声」的一次性提示。
    /// 语义同 `warning_maestro`（`false` = 还没提示过）。
    pub warning_disc_chest: bool,
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
    /// 外部控制（MCP 桥）：开着时窗口一起来就监听回环，不用命令行旗标。
    /// 原版没有对应物，这三条都是我们自己的开关。默认关：打开意味着本机多一个
    /// 回环端口和一份写着 token 的发现文件，得有人在设置页里点过同意才算。
    /// `--control` 那条命令行路径不看这个字段，给了旗标就照起。
    pub control_enabled: bool,
    /// 额外放行给外部读写的目录。词法前缀判断、不解析符号链接，所以这里每加一条
    /// 都是**安全决定**：那个目录底下所有子目录和文件都算放行。
    /// 配置目录、当前工程目录、启动时快照下来的最近文件目录本来就在白名单里，
    /// 这一栏只用于额外追加，不需要把默认那几条抄进来。
    pub control_dirs: Vec<String>,
    /// 危险动作（退出程序、替你答确认框、打开外链）是否可调。
    /// **独立**开关：勾「启用外部控制」绝不会把这条一起打开，取消勾选也不会清掉它。
    pub control_danger: bool,
    /// 用户要求 #131：允许外部控制不切标签就修改后台文档。默认关；
    /// 播放、文件、任务等全局操作仍只允许当前标签。
    #[serde(default)]
    pub control_allow_parked_write: bool,
    /// 播放特效：每个特效一份参数，键 = 特效的稳定 id（不是显示名，改名不冲存档）。
    /// 老存档没这条 → 空表，各特效由自己参数结构上的 `#[serde(default)]` 补出厂值。
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub fx: std::collections::HashMap<String, serde_json::Value>,
    /// 启用中的特效 id。出厂全关：没人要特效时播放路径不该多出任何绘制。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fx_active: Vec<String>,
    /// 叠加类特效的整体透明度（舞台类独占画面，不看这条）。
    pub fx_opacity: f32,
    /// 把播放特效分离到独立原生浮窗；开启时主窗只编辑，不画特效和琴键。
    pub fx_window: bool,
    /// 浮窗通用透明背景：不铺整块底色，音符、粒子、参考线和琴键照常画。
    pub fx_window_transparent: bool,
    /// 浮窗是否始终压在其他窗口前面。默认关闭，避免遮挡视线。
    pub fx_window_always_on_top: bool,
    /// 浮窗鼠标穿透：点击直接落到下方应用；默认关闭，保留正常窗口操作。
    pub fx_window_mouse_passthrough: bool,
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
            audio_output: String::new(),
            show_layer_volumes: true,
            use_bpm: false,
            tapdouble: false,
            // 原版 `control_create.gml:163-165`：channelstoggle=1 → audio_channel_num(1024)。
            extended_voices: true,
            show_sound_count: false,
            include_locked_in_export: false,
            follow_marker: true,
            follow_mode: 1,
            marker_start: true,
            marker_end: false,
            autosave_enabled: false,
            mousewheel: 0,
            mousewheel_scroll_speed: 1,
            changepitch: true,
            layerhov_vppreview: false,
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
            // 端口自加 #119：与 eframe 出厂的 NativeOptions.vsync = true 对齐。
            vsync: true,
            window_scale: None,
            piano_scale: 1.0,
            layer_panel_width: LAYER_PANEL_DEFAULT_WIDTH,
            note_text_min_percent: 50,
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
            maestro: crate::maestro::MaestroOptions::default(),
            fractal: crate::fractal::FractalOptions::default(),
            disc_chest: crate::disc_chest::DiscChestOptions::default(),
            midi_gm: None,
            midi_import: None,
            warning_schematic: false,
            warning_maestro: false,
            warning_fractal: false,
            warning_disc_chest: false,
            // 原版 `control_create.gml:172-175`：check_update = 1（Windows）、
            // check_prerelease = is_prerelease（我们的构建按稳定版算 = 0）、
            // show_welcome = 1；donate_banner = 1、donate_banner_time = −1（:112-113）。
            show_welcome: true,
            check_update: true,
            check_prerelease: false,
            donate_banner_time: 0,
            last_version: None,
            recent_files: Vec::new(),
            // 外部控制出厂全关：同意是人在设置页里给的，不是每次启动默认就给的。
            control_enabled: false,
            control_dirs: Vec::new(),
            control_danger: false,
            control_allow_parked_write: false,
            // 播放特效出厂全关、参数用各特效自己的默认值（这里留空表，
            // 设置文件里也就不多写这两段）。
            fx: std::collections::HashMap::new(),
            fx_active: Vec::new(),
            fx_opacity: 0.85,
            fx_window: false,
            fx_window_transparent: false,
            fx_window_always_on_top: false,
            fx_window_mouse_passthrough: false,
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

/// 原版 `control_draw.gml:47-50` 的 `game_set_speed` 档位表。
/// 0 档源码不调 `game_set_speed`，走的是房间默认的 30 帧，下拉里同样写「30帧」。
/// 下标就是原版的 `refreshrate`，也是写进 prefs.json 的那个数，所以新档位只往表尾加
/// （用户要求 #126 的 165 落在下标 5）：插进 144 和 240 之间会把老配置里存的 4 —— 也就是
/// 原版那一档 240 —— 静默换成 165，而选过 240 的恰好是群里最在意高刷的人。
pub const REFRESH_RATES: [u32; 6] = [30, 60, 120, 144, 240, 165];

/// 按帧数从小到大排的下标。首选项下拉和 F7 循环都走这个顺序，让「列表里的上下关系」跟
/// 数值一致（#126 偏离：代价是表尾的 165 排在下标 4 那档 240 前面，见 `REFRESH_RATES`）。
pub fn refresh_rate_order() -> Vec<usize> {
    let mut order: Vec<usize> = (0..REFRESH_RATES.len()).collect();
    order.sort_by_key(|&i| REFRESH_RATES[i]);
    order
}

/// 某一档（存的是 `refreshrate` 下标）在升序展示里排第几——下拉里要打勾的那一项、
/// 框里显示的那一行、F7 循环的下一步，都从这里换算。越界下标先取模，和 `refresh_limit` 一致。
pub fn refresh_rate_pos(rate: u8) -> usize {
    let order = refresh_rate_order();
    let cur = rate as usize % REFRESH_RATES.len();
    order
        .iter()
        .position(|&i| i == cur)
        .expect("order 覆盖 0..len 全部下标，取模后必命中")
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

    /// 端口自加 #92：某个生效缩放倍率（1.0 = 100%）下，格内文字是否收掉。
    /// 判据用的是首选项里显示的那个数（含系统 DPI），不是画布内部的倍率 ——
    /// 用户拖的是这个数，看到的字消失也必须发生在这个数上。
    /// 阈值本身是「低于」，所以正好等于阈值时文字照画（默认 50 ⇒ 50% 有字、49% 没字）。
    pub fn hides_note_text(&self, effective: f32) -> bool {
        (effective * 100.0).round() < self.note_text_min_percent as f32
    }

    pub fn config_dir() -> PathBuf {
        // 规则本体在 `nbs_core::paths::config_dir`：MCP 桥算的是同一处，
        // 不会出现「编辑器写在一个目录、桥在另一个目录找」这种两边都自洽的连不上。
        // 无头校验（examples/uipng.rs）靠 `NBS_RUST_CONFIG_DIR` 拿到一份和真实用户
        // 隔离的设置，否则正在运行的 GUI 会把默认值覆盖回来。
        nbs_core::paths::config_dir()
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("prefs.json")
    }

    pub fn backup_dir() -> PathBuf {
        Self::config_dir().join("Backups")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let mut p = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        // 手改过的宽度按最窄 / 最宽一档收回来，和拖动边界用的是同一条夹。
        p.clamp_layer_panel_width();
        p
    }

    /// 图层栏宽度夹进 #108 定的两档（拖动边界和读文件走同一条夹）。
    pub fn clamp_layer_panel_width(&mut self) {
        self.layer_panel_width = self
            .layer_panel_width
            .clamp(LAYER_PANEL_MIN_WIDTH, LAYER_PANEL_MAX_WIDTH);
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| {
            crate::tr(&format!("Failed to create config directory: {e}"), &format!("创建配置目录失败: {e}"))
                .to_string()
        })?;
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            crate::tr(&format!("Serialization failed: {e}"), &format!("序列化失败: {e}")).to_string()
        })?;
        // 原子写：先写 .tmp 再 rename，避免中断损坏配置
        let tmp = dir.join("prefs.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| {
            crate::tr(&format!("Failed to write config: {e}"), &format!("写入配置失败: {e}")).to_string()
        })?;
        std::fs::rename(&tmp, Self::config_path()).map_err(|e| {
            crate::tr(&format!("Failed to replace config: {e}"), &format!("替换配置失败: {e}")).to_string()
        })
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
    if crate::lang() == 1 {
        format!("警告：你正在打开旧版的 {ext} 文件。保存此文件会使其与旧版 Note Block Studio 不兼容。")
    } else {
        format!(
            "Warning: You are opening an older {ext} file. Saving this file will make it \
             incompatible with older Note Block Studio versions."
        )
    }
}

/// 原子保存歌曲：写临时文件再替换。
pub fn save_song_atomic(song: &Song, path: &Path) -> Result<(), String> {
    let version = song.meta.save_version;
    let bytes = nbs_core::nbs::save_bytes(song, version).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::tr(&format!("Failed to create directory: {e}"), &format!("创建目录失败: {e}")).to_string()
        })?;
    }
    let tmp = path.with_extension("nbs.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| {
        crate::tr(&format!("Failed to write temporary file: {e}"), &format!("写入临时文件失败: {e}"))
            .to_string()
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        crate::tr(&format!("Failed to replace song file: {e}"), &format!("替换歌曲文件失败: {e}")).to_string()
    })?;
    Ok(())
}

/// 当前端口自动备份的固定路径（`{safe}.autosave.nbs`）。
pub fn backup_path(song: &Song) -> PathBuf {
    let base = song
        .filename
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Preferences::backup_dir().join(format!("{safe}.autosave.nbs"))
}

/// 自动保存一份备份（不改动当前文件名/脏标记）。
pub fn autosave(song: &Song, prefs: &Preferences) -> Result<PathBuf, String> {
    let dir = Preferences::backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        crate::tr(&format!("Failed to create backup directory: {e}"), &format!("创建备份目录失败: {e}"))
            .to_string()
    })?;
    let path = backup_path(song);
    save_song_atomic(song, &path)?;
    let _ = prefs;
    Ok(path)
}

/// 原版 `save_unsaved()`：退出流程里另写一份恢复文件，不覆盖已有自动备份。
pub fn save_shutdown_recovery(song: &Song) -> Result<PathBuf, String> {
    let dir = Preferences::backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        crate::tr(
            &format!("Failed to create backup directory: {e}"),
            &format!("创建备份目录失败: {e}"),
        )
        .to_string()
    })?;
    let source = song
        .filename
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let stem: String = source.chars().take(48).collect();
    let base = format!("{stem} (unsaved on exit)");
    let mut path = dir.join(format!("{base}.nbs"));
    let mut number = 2;
    while path.exists() {
        path = dir.join(format!("{base} {number}.nbs"));
        number += 1;
    }
    save_song_atomic(song, &path)?;
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

    /// #108：图层栏宽度用户可拖，读文件时按同一条夹收回来；旧配置文件没这一项
    /// 也照样能读（`#[serde(default)]`），读进来就是出厂的 264。
    #[test]
    fn layer_panel_width_defaults_and_clamps() {
        assert_eq!(
            Preferences::default().layer_panel_width,
            LAYER_PANEL_DEFAULT_WIDTH
        );
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(p.layer_panel_width, LAYER_PANEL_DEFAULT_WIDTH);
        for (raw, want) in [
            ("264.0", LAYER_PANEL_DEFAULT_WIDTH),
            ("100.0", LAYER_PANEL_MIN_WIDTH),
            ("900.0", LAYER_PANEL_MAX_WIDTH),
        ] {
            let json = format!("{{\"layer_panel_width\": {raw}}}");
            let mut p: Preferences = serde_json::from_str(&json).unwrap();
            p.clamp_layer_panel_width();
            assert_eq!(p.layer_panel_width, want, "raw={raw}");
        }
    }
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
        p.vsync = false;
        p.window_scale = Some(1.25);
        p.window_icon = true;
        p.show_welcome = false;
        p.check_update = false;
        p.check_prerelease = true;
        p.donate_banner_time = 1760000000;
        p.last_version = Some("0.1.0".into());
        p.audio_output = "耳机 (Realtek(R) Audio)".into();
        p.note_text_min_percent = 35;
        p.fx_window = true;
        p.fx_window_transparent = true;
        p.fx_window_always_on_top = true;
        p.fx_window_mouse_passthrough = true;
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
        assert!(!back.vsync, "垂直同步开关要落盘（端口自加 #119）");
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
        assert_eq!(back.audio_output, "耳机 (Realtek(R) Audio)", "点名的输出设备要落盘");
        assert_eq!(back.note_text_min_percent, 35, "格内文字的隐藏阈值要落盘");
        assert!(back.fx_window, "分离浮窗开关要落盘");
        assert!(back.fx_window_transparent, "客户区背景透明开关要落盘");
        assert!(back.fx_window_always_on_top, "浮窗置顶开关要落盘");
        assert!(back.fx_window_mouse_passthrough, "浮窗鼠标穿透开关要落盘");
        // 旧配置文件里没有这几项时必须退回默认值，不能整个文件读不出来。
        let old = r#"{"version":1,"theme":3}"#;
        let back: Preferences = serde_json::from_str(old).unwrap();
        assert_eq!(back.refresh_rate, 0, "出厂就是 30 帧");
        assert!(
            back.vsync,
            "旧配置缺键时垂直同步退回出厂打开（eframe 默认同步）"
        );
        assert_eq!(back.window_scale, None, "出厂跟随系统 DPI");
        assert_eq!(
            back.note_text_min_percent, 50,
            "旧配置缺键时阈值退回出厂 50%（端口自加 #92）"
        );
        assert!(!back.window_icon, "出厂用程序自带的图标");
        assert!(!back.fx_window, "旧配置缺键时浮窗关闭");
        assert!(!back.fx_window_transparent, "旧配置缺键时浮窗不透明");
        assert!(!back.fx_window_always_on_top, "旧配置缺键时浮窗不置顶");
        assert!(!back.fx_window_mouse_passthrough, "旧配置缺键时不穿透");
        assert_eq!(back.accent, [0, 120, 212], "强调色基色出厂 0,120,212");
        assert!(back.remove_effect, "旧配置缺键时退回出厂打开");
        assert_eq!(
            back.mousewheel_scroll_speed, 1,
            "旧配置缺键时滚轮步长退回出厂 1（原版 median(1, ini_read_real(…)))"
        );
        assert!(!back.show_notechart, "旧配置缺键时五线谱退回关闭");
        assert!(
            (back.selection_alpha - 0.8).abs() < 1e-6,
            "旧配置缺键时选区强度退回 0.8"
        );
        assert!(back.audio_output.is_empty(), "旧配置缺键时跟随系统默认");
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
        p.mousewheel_scroll_speed = 7;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(!back.remove_effect, "删除效果开关要落盘");
        assert!((back.selection_alpha - 0.35).abs() < 1e-6, "选区强度要落盘");
        assert!(back.show_notechart, "五线谱开关要落盘");
        assert_eq!(back.mousewheel_scroll_speed, 7, "滚轮步长要落盘（save_settings.gml:49）");
        let mut p = Preferences::default();
        p.accent = [10, 20, 30];
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.accent, [10, 20, 30], "accent1/2/3 要落盘（save_settings.gml:65-67）");
    }

    /// 原版 `layerhov_vppreview`：`control_create.gml:410` 出厂 0，
    /// `save_settings.gml:51` 写盘、`load_settings.gml:58` 读回。
    #[test]
    fn layerhov_vppreview_defaults_off_and_persists() {
        assert!(
            !Preferences::default().layerhov_vppreview,
            "悬停层预览出厂关闭（control_create.gml:410）"
        );
        let old: Preferences = serde_json::from_str(r#"{"version":1,"theme":3}"#).unwrap();
        assert!(!old.layerhov_vppreview, "旧配置缺键时退回出厂关闭");
        let mut p = Preferences::default();
        p.layerhov_vppreview = true;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(back.layerhov_vppreview, "悬停层预览开关要落盘（save_settings.gml:51）");
    }

    /// 原版 `tapdouble`（速度测量器「双倍速度」）是写进 settings.ini 的
    /// （`save_settings.gml:71` / `load_settings.gml:78`），出厂 0（`control_create.gml:565`）。
    #[test]
    fn tapdouble_defaults_off_and_persists() {
        assert!(
            !Preferences::default().tapdouble,
            "双倍速度出厂关闭（control_create.gml:565）"
        );
        let old: Preferences = serde_json::from_str(r#"{"version":1,"theme":3}"#).unwrap();
        assert!(!old.tapdouble, "旧配置缺键时退回出厂关闭");
        let mut p = Preferences::default();
        p.tapdouble = true;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(back.tapdouble, "双倍速度勾选要落盘（save_settings.gml:71）");
    }

    /// `[schematic_export]` 段：整份选项 + 「警告已看过」都要能落盘，
    /// 且旧文件里没有这两个键时必须退回默认值而不是报错。
    #[test]
    fn schematic_options_and_warning_flag_persist() {
        let mut p = Preferences::default();
        p.warning_schematic = true;
        p.schematic.layout = 2;
        p.schematic.set_format(1);
        p.schematic.command_block = true;
        p.schematic.command_source = "music".into();
        p.schematic.command_allowed_sources[1] = false;
        p.schematic.command_tempo_grid = true;
        p.schematic.ins_block[0] = 99;
        p.schematic.ins_data[0] = 7;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(back.warning_schematic);
        assert_eq!(back.schematic.layout, 2);
        assert!(back.schematic.structure);
        assert_eq!(back.schematic.format, 1);
        assert!(back.schematic.command_block);
        assert_eq!(back.schematic.command_source, "music");
        assert!(!back.schematic.command_allowed_sources[1]);
        assert!(back.schematic.command_tempo_grid);
        assert_eq!(back.schematic.ins_block[0], 99);
        assert_eq!(back.schematic.ins_data[0], 7);

        let old: Preferences = serde_json::from_str(r#"{"theme":2}"#).unwrap();
        assert!(!old.warning_schematic);
        assert_eq!(old.schematic.layout, 1, "旧文件缺 [schematic_export] 时退回原版默认");
        assert_eq!(old.schematic.ins_block, crate::schematic::default_ins_block());
        // 原版 reset_schematic_export.gml:66-70：声源页四项的出厂值；format 出厂 0。
        assert_eq!(old.schematic.command_source, "block");
        assert_eq!(
            old.schematic.command_allowed_sources,
            crate::mc_export::default_allowed_sources()
        );
        assert!(!old.schematic.command_tempo_grid);
        assert_eq!(old.schematic.format, 0);
        assert_eq!(old.theme, 2, "已有的键照常读出来");
    }

    /// MAESTRO 那条导出（端口自加）：取整模式与一次性提示要能落盘，
    /// 旧配置里没有这两个键时退回默认（round / 未提示）。
    #[test]
    fn maestro_options_and_notice_flag_persist() {
        let mut p = Preferences::default();
        p.maestro.timing = crate::maestro::TimingMode::Flexible;
        p.warning_maestro = true;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.maestro.timing, crate::maestro::TimingMode::Flexible);
        assert!(back.warning_maestro);

        let old: Preferences = serde_json::from_str(r#"{"theme":2}"#).unwrap();
        assert_eq!(
            old.maestro.timing,
            crate::maestro::TimingMode::Round,
            "官方转换器默认 round，老配置缺键时必须退回它"
        );
        assert!(!old.warning_maestro);
    }

    /// fractal 那条导出（端口自加）：布局族、奇偶分道与一次性提示要能落盘，
    /// 老配置里没有这几个键时退回出厂档（Linear / 20 t/s / 未提示）。
    #[test]
    fn fractal_options_and_notice_flag_persist() {
        let mut p = Preferences::default();
        p.fractal.layout = crate::fractal::FractalLayout::Compact;
        p.fractal.twenty_hz = false;
        p.fractal.wrap = 16;
        p.warning_fractal = true;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.fractal.layout, crate::fractal::FractalLayout::Compact);
        assert!(!back.fractal.twenty_hz);
        assert_eq!(back.fractal.wrap, 16);
        assert!(back.warning_fractal);

        let old: Preferences = serde_json::from_str(r#"{"theme":2}"#).unwrap();
        assert_eq!(
            old.fractal.layout,
            crate::fractal::FractalLayout::Linear,
            "老配置缺键时得退回上游 CLI 的出厂档"
        );
        assert!(old.fractal.twenty_hz);
        assert!(!old.warning_fractal);
    }

    /// 编码唱片数据箱那条导出（端口自加）：机体档、时间处理方式、层高与一次性提示要能
    /// 落盘；老配置里没有这几个键时退回上游的出厂档（20 t/s / resample / 层高 8 / 未提示）。
    #[test]
    fn disc_chest_options_and_notice_flag_persist() {
        let mut p = Preferences::default();
        p.disc_chest.machine = crate::disc_chest::Machine::TenHz;
        p.disc_chest.timing = crate::disc_chest::Timing::KeepTicks;
        p.disc_chest.layer_height = 4;
        p.disc_chest.stackable_rests = false;
        p.warning_disc_chest = true;
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.disc_chest.machine, crate::disc_chest::Machine::TenHz);
        assert_eq!(back.disc_chest.timing, crate::disc_chest::Timing::KeepTicks);
        assert_eq!(back.disc_chest.layer_height, 4);
        assert!(!back.disc_chest.stackable_rests);
        assert!(back.warning_disc_chest);

        let old: Preferences = serde_json::from_str(r#"{"theme":2}"#).unwrap();
        assert_eq!(
            old.disc_chest.machine,
            crate::disc_chest::Machine::TwentyHz,
            "老配置缺键时得退回上游 `defaultConfig` 那台 20 音/秒"
        );
        assert_eq!(old.disc_chest.layer_height, 8);
        assert!(old.disc_chest.stackable_rests);
        assert!(!old.warning_disc_chest);
    }

    /// 外部控制这几条是我们自己加的（原版没有对应物），所以对旧文件的口径是「全关」：
    /// 装了老版本、prefs.json 里根本没这些键的用户，不该升级完就多出一个在听的端口。
    #[test]
    fn control_opt_in_defaults_to_off_in_old_files() {
        let old: Preferences = serde_json::from_str(r#"{"theme":2,"language":1}"#).unwrap();
        assert!(!old.control_enabled, "旧配置缺键时外部控制退回关闭");
        assert!(old.control_dirs.is_empty(), "旧配置缺键时不放行任何额外目录");
        assert!(!old.control_danger, "旧配置缺键时危险动作退回关闭");
        assert!(!old.control_allow_parked_write, "旧配置缺键时后台写退回关闭");
        assert_eq!(old.theme, 2, "已有的键照常读出来");

        let p = Preferences {
            control_enabled: true,
            control_dirs: vec!["D:\\乐谱".to_string(), "D:\\采样".to_string()],
            control_danger: true,
            control_allow_parked_write: true,
            ..Default::default()
        };
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(
            back.control_enabled && back.control_danger && back.control_allow_parked_write,
            "三条勾选都要存下来"
        );
        assert_eq!(back.control_dirs, p.control_dirs, "目录列表要按顺序存下来");
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
        assert_eq!(p.mousewheel_scroll_speed, 1, "滚轮步长出厂 1（control_create.gml:408）");
        assert!(p.changepitch);
        assert!(p.select_lastpressed);
        assert_eq!(p.follow_mode, 1, "原版默认按页跟随");
        assert!(p.marker_start, "原版 marker_start 出厂 1（control_create.gml:381）");
        assert!(!p.marker_end, "原版 marker_end 出厂 0（control_create.gml:382）");
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
    fn backup_path_uses_the_same_safe_stem_as_autosave() {
        let mut song = Song::new();
        song.filename = Some(PathBuf::from("C:/songs/我的 曲子!.nbs"));
        let path = backup_path(&song);
        assert_eq!(path.file_name().and_then(|s| s.to_str()), Some("我的_曲子_.autosave.nbs"));
        assert_eq!(path.parent(), Some(Preferences::backup_dir().as_path()));
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
        // 当月第 d 天 01:00:00Z 的 unix 秒（2024 闰年 / 2025、2026 平年）。
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
        let _cn = crate::TEST_LANG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        struct ResetLang(u8);
        impl Drop for ResetLang {
            fn drop(&mut self) {
                crate::set_language(self.0);
            }
        }
        let _lang = ResetLang(crate::lang());
        crate::set_language(1);
        assert_eq!(
            old_file_warning("NBS"),
            "警告：你正在打开旧版的 NBS 文件。保存此文件会使其与旧版 Note Block Studio 不兼容。"
        );
        assert!(old_file_warning("NBP").contains("NBP"));
        crate::set_language(0);
        assert_eq!(
            old_file_warning("NBS"),
            "Warning: You are opening an older NBS file. Saving this file will make it \
             incompatible with older Note Block Studio versions."
        );
    }

    /// 原版 `control_draw.gml:47-50`：0..4 = 30 / 60 / 120 / 144 / 240；#126 追加的
    /// 165 占下标 5，老下标的含义一个都没动。
    #[test]
    fn refresh_rate_maps_to_original_frames() {
        let mut p = Preferences::default();
        assert_eq!(
            p.refresh_limit(),
            30.0,
            "出厂 refreshrate = 0（control_create.gml:130），对应 30 帧"
        );
        for (rate, fps) in [(1u8, 60.0), (2, 120.0), (3, 144.0), (4, 240.0), (5, 165.0)] {
            p.refresh_rate = rate;
            assert_eq!(p.refresh_limit(), fps);
        }
    }

    /// #126：表尾追加的那一档在界面上要按帧数升序出现，而不是按存储下标。
    #[test]
    fn refresh_rate_display_order_is_ascending() {
        let order = refresh_rate_order();
        let shown: Vec<u32> = order.iter().map(|&i| REFRESH_RATES[i]).collect();
        assert_eq!(
            shown,
            vec![30, 60, 120, 144, 165, 240],
            "下拉与 F7 循环的展示顺序"
        );
        assert_eq!(
            order.iter().copied().collect::<Vec<usize>>(),
            vec![0, 1, 2, 3, 5, 4],
            "展示位次要能反查回存储下标，否则选中的不是自己看到的那一档"
        );
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

    /// 端口自加 #92：格内文字的隐藏阈值。判据是首选项里显示的那个百分数
    /// （`effective_scale` 含系统 DPI），四舍五入和界面显示用的是同一个式子，
    /// 所以「显示的数低于阈值」和「字消失」必须同时发生。
    #[test]
    fn note_text_threshold_uses_the_percent_the_user_sees() {
        let mut p = Preferences::default();
        assert_eq!(
            p.note_text_min_percent, 50,
            "出厂阈值 50% = 原版缩放的最低档"
        );
        assert!(!p.hides_note_text(0.50), "正好等于阈值时文字照画");
        assert!(p.hides_note_text(0.49), "低于阈值才收");
        assert!(!p.hides_note_text(1.0));
        assert!(!p.hides_note_text(4.0));
        p.note_text_min_percent = 0;
        assert!(!p.hides_note_text(0.10), "0 = 永不收字");
        p.note_text_min_percent = 100;
        assert!(p.hides_note_text(0.99), "100 = 只要往小拖就收");
        assert!(!p.hides_note_text(1.0), "但 100% 本身照画");
        p.note_text_min_percent = 50;
        p.window_scale = Some(0.5);
        assert!(
            !p.hides_note_text(p.effective_scale(1.25)),
            "系统 125% 下拖到 50%：显示的是 50，不该收字"
        );
    }
}
