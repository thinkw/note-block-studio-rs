//! 编辑器总状态。原版是 `obj_controller` 单实例的几百个实例变量；
//! 这里收敛成结构化字段，并保留 `window / select / select_press*` 的状态机语义。

use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{Context, Pos2};
use nbs_audio::{AudioEngine, CpalEngine, SoundBank, DEFAULT_MAX_VOICES, EXTENDED_MAX_VOICES};
use nbs_core::model::Note;
use nbs_core::Document;
use nbs_io::datapack::DatapackOptions;
use nbs_io::image_export::ImageOptions;
use nbs_io::midi::MidiImportOptions;
use nbs_io::prefs::{Preferences, RecentFile, REFRESH_RATES};
use nbs_io::schematic::SchematicOptions;

use crate::commands::Command;
use crate::interaction::Interaction;
use crate::theme::{Theme, ThemeKind};
use crate::tr;
use crate::viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditMode {
    #[default]
    Key,
    Velocity,
    Panning,
    Pitch,
}

impl EditMode {
    pub const ALL: [EditMode; 4] = [Self::Key, Self::Velocity, Self::Panning, Self::Pitch];
    pub fn name(self) -> &'static str {
        match self {
            Self::Key => tr("Key", "音高"),
            Self::Velocity => tr("Velocity", "力度"),
            Self::Panning => tr("Panning", "声像"),
            Self::Pitch => tr("Pitch", "微调"),
        }
    }
}

/// 原版 Home / End / PageUp / PageDown 对「最后碰过的那根滚动条」做的三种动作
/// （`control_draw.gml:684-704`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisScroll {
    Home,
    End,
    /// ±多少个可见页；原版 PageUp = +1，PageDown = -1。
    Page(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    ShowLayers,
    ShowPiano,
    UseColors,
    UseShapes,
    UseIcons,
    ShowNumbers,
    ShowOctaves,
    ShowKeyNames,
    ShowKeyNumbers,
    ShowKeyboard,
    ShowOutOfRange,
    ShowIncompatible,
    Fade,
    ShowGridLines,
    Flats,
    Blackout,
    RemoveEffect,
    ShowNoteChart,
}

impl Flag {
    pub fn get(self, f: &UiFlags) -> bool {
        match self {
            Self::ShowLayers => f.show_layers,
            Self::ShowPiano => f.show_piano,
            Self::UseColors => f.use_colors,
            Self::UseShapes => f.use_shapes,
            Self::UseIcons => f.use_icons,
            Self::ShowNumbers => f.show_numbers,
            Self::ShowOctaves => f.show_octaves,
            Self::ShowKeyNames => f.show_keynames,
            Self::ShowKeyNumbers => f.show_keynumbers,
            Self::ShowKeyboard => f.show_keyboard,
            Self::ShowOutOfRange => f.show_outofrange,
            Self::ShowIncompatible => f.show_incompatible,
            Self::Fade => f.fade,
            Self::ShowGridLines => f.show_grid_lines,
            Self::Flats => f.flats,
            Self::Blackout => f.blackout,
            Self::RemoveEffect => f.remove_effect,
            Self::ShowNoteChart => f.show_notechart,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiFlags {
    pub show_layers: bool,
    pub show_piano: bool,
    pub use_colors: bool,
    pub use_shapes: bool,
    pub use_icons: bool,
    pub show_numbers: bool,
    pub show_octaves: bool,
    pub show_keynames: bool,
    /// 原版 `show_keynumbers`：在 MC 音域内的琴键上画「要按几次右键才够到音高」的序号。
    pub show_keynumbers: bool,
    /// 原版 `show_keyboard`：在琴键上画 `init_keys()` 绑定的按键字符。
    pub show_keyboard: bool,
    /// 原版 `show_outofrange`：把 MC 两八度（33..=57）之外的琴键染成粉红。
    pub show_outofrange: bool,
    pub show_incompatible: bool,
    /// 原版 `fade`：真 = 音符盒不透明（力度与选区都不再影响透明度）。
    pub fade: bool,
    pub show_grid_lines: bool,
    pub flats: bool,
    pub blackout: bool,
    /// 原版 `remove_effect`（`control_create.gml:225` 出厂 = 1）：
    /// 删除音符时在格子上画一个淡出扩大的圆角边框（theme 3 才生效）。
    pub remove_effect: bool,
    /// 原版 `show_notechart`（`control_create.gml:461` 出厂 = 0）：
    /// 悬停琴键时在其上方画五线谱面板（draw_notechart）。
    pub show_notechart: bool,
}

impl Default for UiFlags {
    fn default() -> Self {
        Self {
            show_layers: true,
            show_piano: true,
            // 原版 `control_create.gml:244-248,314-318` 的出厂值：
            // show_numbers=1、show_octaves=0、use_colors=1、use_icons=0、use_shapes=0、
            // show_keynames=1、show_keynumbers=0、show_keyboard=1、show_outofrange=1。
            use_colors: true,
            use_shapes: false,
            use_icons: false,
            show_numbers: true,
            show_octaves: false,
            show_keynames: true,
            show_keynumbers: false,
            show_keyboard: true,
            show_outofrange: true,
            show_incompatible: true,
            // 原版 `control_create.gml:131`：fade = 0，即默认按力度做透明。
            fade: false,
            show_grid_lines: true,
            flats: false,
            blackout: false,
            remove_effect: true,
            show_notechart: false,
        }
    }
}

/// 原版 `obj_removeeff`：删除音符时格子上的淡出边框动画。
/// 原版 alpha 从 0.5 每帧减 `0.06 * 30 / (room_speed * currspeed)`，
/// 换算成真实时间寿命恒为 1/3.6 秒（衰减速率 1.8/秒），与播放速度无关。
#[derive(Clone, Debug)]
pub struct RemoveFx {
    /// 相对视口左上格的格坐标（原版 `xn = xx - starta`）。
    pub rel: (i64, i64),
    /// 已存活的真实秒数。
    pub age: f32,
}

impl RemoveFx {
    pub fn alpha(&self) -> f32 {
        (0.5 - 1.8 * self.age).max(0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowKind {
    /// 原版 `w_songinfo`（`macros.gml:96`）：只读卡片，脚注是「点击任意位置以关闭」，
    /// 没有按钮，按左键即关（`draw_window_songinfo.gml:110/138-143`）。
    /// 打开文件后自动弹的就是这一版（`load_song.gml:64`），播放器档位的工具栏图标
    /// （`control_draw.gml:2042`）和 `settingsp` 菜单（`menu_click.gml:374`）也开这一版。
    SongInfo,
    /// 原版 `w_songinfoedit`（`macros.gml:95`）：同一张卡片，脚注换成
    /// 「此信息在打开文件时显示。」并多一排「修改」「确定」（`:129-136`）。
    /// 编辑器档位的工具栏信息图标和 `settings` 菜单开的是这一版。
    SongInfoEdit,
    Properties,
    Statistics,
    Preferences,
    /// 「外部控制（MCP）」这一栏。原版首选项是 500×510 的五页像素稿，
    /// 塞第六个页签会把页签条顶出窗口，而且这一页要显示端口、发现文件全路径、
    /// 目录列表这些长字符串——所以单开一窗，不冒充原版的排版。
    ExternalControl,
    /// 端口自加（用户要求「要有个底座能扩展各种播放效果并可以切换」）：特效开关、
    /// 各自的调参面板、叠加透明度。原版没有这一窗，排版用普通 egui 控件。
    PlaybackEffects,
    Instruments,
    About,
    /// 原版 `w_tempotapper`（`draw_window_tempo_tapper.gml`）：按 T 打点测速。
    TempoTapper,
    /// 原版 `w_saveoptions`（`draw_window_save_options.gml`）：文件菜单里的「保存选项」，
    /// 用单选框选存盘用的 NBS 版本。打开时把 `save_options_version` 播种成歌曲当前版本。
    SaveOptions,
    SetVelocity,
    SetPanning,
    SetPitch,
    Tremolo,
    Stereo,
    ClipEditor,
    Shortcuts,
    FileOpen,
    FileSave,
    LayerRename(u32),
    AudioExport,
    AudioSettings,
    DatapackExport,
    MidiImport,
    Transcription,
    /// 原版 `w_mididevices`（`draw_window_mididevices.gml`）：实时 MIDI 输入的
    /// 设备管理器，500×350，设备名 + 指派音色两列。
    MidiDevices,
    /// 原版 `w_sound_import`（`draw_window_sound_import.gml`）：导入音效向导，
    /// 415×310，本地 `.minecraft` 与 Mojang 下载两条路。
    SoundImport,
    SchematicExport,
    /// 端口自加（用户要求「导出那一栏多加个这个格式」）：MAESTRO 音乐机的节目单导出。
    /// 产物是给已建好的机体读的 `.schem`，本机没有声音。
    MaestroExport,
    /// 端口自加（用户要求接第二条 20Hz 路子）：nbs-fractal 的整机投影导出。
    /// 与 MAESTRO 相反，这份 `.litematic` 自带红石，贴下去就能响。
    FractalExport,
    /// 端口自加（用户要求接第二条 20Hz 路子）：唱片工坊那一档的编码唱片数据箱导出。
    /// 和 MAESTRO 同族——这份 `.litematic` 只有箱子，要贴在已建好的机体里才出声。
    DiscChestExport,
    /// 原版 `w_track_export`（`draw_window_track_export.gml`）：550×400，
    /// 「普通过道」直轨布局的结构/schematic 导出。
    TrackExport,
    PngExport,
    /// 原版 `w_arpeggio`（`draw_window_macro_arpeggio.gml`）：140×130，只有一个规律框。
    Arpeggio,
    /// 原版 `w_portamento`（`draw_window_macro_portamento.gml`）：140×130，
    /// 反转勾选 + cents 拖拽。
    Portamento,
    /// 原版 `w_stagger`（`draw_window_macro_stagger.gml`）：150×160，规律框 + 警示文字。
    Stagger,
    Autosaves,
    /// 原版 `w_minecraft`（`macros.gml:69`）：右上角兼容性图标点开的窗口。
    MinecraftCompat,
    /// 原版 `w_edit_tempo_changer`（`add/change_block_manual` 落速度变换时弹出），
    /// 参数是目标格 (tick, layer)。
    EditTempoChanger(u32, u32),
    /// 原版 `w_edit_sound_stopper`：落消音器时弹出，参数同上。
    EditSoundStopper(u32, u32),
    /// 原版 `w_setaccent`（`macros.gml:118`）：首选项「主题」组里那个 17x17 色块
    /// 点开的取色器（`draw_window_preferences.gml:313-322`），只有 Fluent 主题画得出这一行。
    SetAccent,
    /// 原版 `w_changelist`（`macros.gml:86`）：帮助菜单「更新历史……」打开的那扇，
    /// 和 `w_update` 共用 `draw_window_update.gml`，只有标题、「谢词」和「确认」的去处不同。
    Changelist,
    /// 原版 `w_update`（`macros.gml:87`）：开机发现上次记录的版本和本版不同时，
    /// 顶掉欢迎窗打开的那一版（`control_create.gml:750`）。
    Update,
    /// 原版 `w_greeting`（`macros.gml:105`）：开机欢迎窗（`show_welcome` 时打开）。
    /// 原版 `draw_window` 没有标题文字，这里也画成无标题栏的 700x430 窗。
    Greeting,
}

impl WindowKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::SongInfo | Self::SongInfoEdit => tr("Song Info", "歌曲信息"),
            Self::Properties => tr("Song Properties", "歌曲属性"),
            // 原版 `draw_window_stats`：EN "Song Stats" / CN "歌曲数据"。
            Self::Statistics => tr("Song Stats", "歌曲数据"),
            Self::Preferences => tr("Preferences", "首选项"),
            // 我们自己的一页，标题就是菜单上那句。
            Self::ExternalControl => tr("External control (MCP)", "外部控制（MCP）"),
            Self::PlaybackEffects => tr("Playback effects", "播放特效"),
            Self::Instruments => tr("Instruments", "音色设置"),
            // 原版 `draw_window_about.gml:10-11`：框内标题就一个「About」/「关于」。
            Self::About => tr("About", "关于"),
            // 原版 `draw_window_update.gml:23-37` 的粗体标题：`window = w_update` 才是
            // 「更新」，从帮助菜单进的「更新历史」走 else 那支。
            Self::Changelist => tr("Changelist", "更新历史"),
            Self::Update => tr("Update", "更新"),
            // 原版 `draw_window_set_accent.gml:13-14` 的粗体标题。
            Self::SetAccent => tr("Set accent color", "设置主题颜色"),
            // 原版 `draw_window_tempo_tapper` 的标题（`condstr(language != 1, "Tempo tapper", "速度测量器")`）。
            Self::TempoTapper => tr("Tempo tapper", "速度测量器"),
            // 原版 `draw_window_save_options:13-14` 的正文粗体标题。
            Self::SaveOptions => tr("Save options", "保存选项"),
            Self::SetVelocity => tr("Set note velocity", "设置音量"),
            Self::SetPanning => tr("Set note panning", "设置声道"),
            Self::SetPitch => tr("Set note pitch", "设置音高"),
            // 原版这两个窗口的粗体标题没有语言分支（`draw_window_macro_tremolo:16`、
            // `draw_window_macro_stereo:16` 都是直接写英文），中文界面也显示英文。
            Self::Tremolo => tr("Tremolo", "Tremolo"),
            Self::Stereo => tr("Stereo", "Stereo"),
            Self::ClipEditor => tr("Clip Editor", "片段编辑器"),
            Self::Shortcuts => tr("Shortcuts", "快捷键一览"),
            Self::FileOpen => tr("Open Song", "打开歌曲"),
            Self::FileSave => tr("Save Song", "保存歌曲"),
            Self::LayerRename(_) => tr("Rename layer", "重命名图层"),
            Self::AudioExport => tr("Audio Export", "音频导出"),
            Self::AudioSettings => tr("Audio Settings", "音频设置"),
            // 原版 `draw_window_datapack_export.gml:13`：CN "导出数据包"。
            Self::DatapackExport => tr("Data Pack Export", "导出数据包"),
            // 原版 `draw_window_midi_import.gml` 中文分支画的是 `"导入MIDI"`。
            Self::MidiImport => tr("MIDI Import", "导入MIDI"),
            Self::Transcription => tr("Audio to MIDI / preview", "音频转 MIDI／预览"),
            // 原版 draw_window_mididevices.gml:11-12：窗口标题就在框里。
            Self::MidiDevices => tr("MIDI device manager", "MIDI 设备管理器"),
            Self::SoundImport => tr("Import sounds", "导入音效"),
            // 原版 `draw_window_schematic_export.gml:14`：CN "导出结构"。
            Self::SchematicExport => tr("Schematic Export", "导出结构"),
            // 端口自加，原版没有对应文案；「节目单」沿用机体文档对 sheet 的叫法。
            Self::MaestroExport => tr("MAESTRO Machine Sheet", "MAESTRO 机体节目单"),
            // 端口自加，原版没有对应文案。
            Self::FractalExport => tr("Fractal Machine Projection", "Fractal 机体投影"),
            // 端口自加，原版没有对应文案。
            Self::DiscChestExport => tr("Encoded Disc Chest", "编码唱片数据箱"),
            // 原版 `draw_window_track_export.gml:13`：CN "导出直轨"。
            Self::TrackExport => tr("Track Export", "导出直轨"),
            Self::PngExport => tr("Export PNG chart", "导出 PNG 谱面"),
            // 原版三个宏窗口的正文标题都是 `draw_text_dynamic(x1 + 8, y1 + 8, "Arpeggio")`
            // 这种不分语言的写法，中文界面里也保持英文，照抄。
            Self::Arpeggio => "Arpeggio",
            Self::Portamento => "Portamento",
            Self::Stagger => "Stagger",
            Self::Autosaves => tr("Autosaves / Backups", "自动保存 / 备份"),
            // 原版 `draw_window_minecraft.gml:21` 的标题文字。
            Self::MinecraftCompat => tr("Minecraft Compatibility", "Minecraft 兼容性"),
            // 原版两个编辑窗没有画标题文字（w_settempo 只有输入框），
            // 这里是 port 标题栏的用词，与窗口内部报错标题保持一致。
            Self::EditTempoChanger(..) => tr("Set Tempo", "设置速度"),
            Self::EditSoundStopper(..) => tr("Set Stopping Range", "设置抑制范围"),
            // 原版欢迎窗没有标题文字（draw_window 只画边框），这里仅供窗口 id 用。
            Self::Greeting => "Note Block Studio",
        }
    }
}

/// 原版 `w_setaccent` 的编辑态：全局 `hsv[0..3]`、`rr`/`gg`/`bb`、`text_str[60..62]`。
/// 原版另有一组 `hsdrag`/`vdrag`/`nocdrag` 拖拽标志（在某一块上按下就认死这一块，
/// 滑出判定区也继续跟，松手才清零），egui 的 `Sense::drag` 天生就是这个语义，不再存。
/// 原版的这些全局是常驻的，但每次从首选项点色块进来都会置 `resetcolor = true`
/// （`draw_window_preferences.gml:320`），下一帧就按当前 accent1/2/3 全部复位，
/// 所以「开窗即初始化」与原版的观感等价。
#[derive(Clone, Debug, PartialEq)]
pub struct AccentPick {
    /// 原版 `hsv`：色相 0..=360、饱和和明度 0..=100；复位时是整数，拖动中是小数。
    pub hsv: [f32; 3],
    /// 原版 `rr`/`gg`/`bb`：正在挑的这支颜色。
    pub rgb: [u8; 3],
    /// 原版 `text_str[60..62]`：RGB 调色区那三个文本框，拖动时实时跟着刷新。
    pub text: [String; 3],
}

impl AccentPick {
    /// 原版 `resetcolor` 那一段：`rr/gg/bb = accent1/2/3`，再反推 hsv 和三个文本框。
    pub fn from_rgb(rgb: [u8; 3]) -> Self {
        Self {
            hsv: crate::theme::Theme::rgb_to_hsb(rgb[0], rgb[1], rgb[2]),
            rgb,
            text: std::array::from_fn(|i| rgb[i].to_string()),
        }
    }

    /// 原版每拖一步的收尾：`dhsv = scr_HSBtoRGB(hsv[0], hsv[1], hsv[2])`，
    /// 把 rr/gg/bb 和三个文本框一起刷新。
    pub fn apply_hsv(&mut self) {
        self.rgb = crate::theme::Theme::hsb_to_rgb(self.hsv[0], self.hsv[1], self.hsv[2]);
        self.text = std::array::from_fn(|i| self.rgb[i].to_string());
    }
}

/// 原版 `menu_shown` 在 MIDI 导入窗口里的四种下拉：括号内是那一行的行号
/// （`menu.menub`），点箭头打开、点条目或菜单外任意处关闭。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMenu {
    /// 「乐器」页的音色列。
    ChannelIns(usize),
    /// 「乐器」页的八度调整列。
    ChannelOct(usize),
    /// 「打击乐」页的音色列。
    PercIns(usize),
    /// 「打击乐」页的音高列。
    PercPitch(usize),
}

/// 原版三种弹窗的按钮组：`message()` 只有确定、`question()` 是/否、
/// `message_yesnocancel()` 是/否/取消。第三键的返回值是从出货二进制里反汇编
/// 确认的（是 = 1 / 否 = 0 / 取消 = -1，见
/// `.work/songtabs-20261123/gml/message_yesnocancel_返回值.md`），所以「取消」
/// 和「保存失败」在原版里是同一个值，都表示这条路作废。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalButtons {
    Ok,
    YesNo,
    YesNoCancel,
}

/// 原版的 `message()` / `question()` / `message_yesnocancel()` 模态框。
/// `YesNo` 答「否」会丢掉整个待弹队列（即 `calculate_size()` 返回 0，不打开导出窗口）。
#[derive(Clone, Debug)]
pub struct Modal {
    pub title: &'static str,
    pub text: String,
    pub buttons: ModalButtons,
}

/// 一组待弹的模态框，队列走完后执行 `then`。
#[derive(Clone, Debug, Default)]
pub struct ModalQueue {
    pub items: std::collections::VecDeque<Modal>,
    pub then: Option<Command>,
    /// 只有带「否」按钮的问句用得上：答「否」也要继续走，但走的是另一条路。
    /// 原版 `close_song` 里「否」= 不保存、照关（`confirm` 返回 0，不 < 0），
    /// 和答「是」的「先保存再关」不是一件事，所以得分开挂（用户要求 #131）。
    pub then_no: Option<Command>,
}

/// 关标签进行中的挂起状态（用户要求 #131）。
///
/// 原版 `close_song(sid)` 从头到尾是阻塞的：先把要关的那首摆上台，问完再切回
/// 原来那首，最后才真正销毁。端口里「先保存」这一步可能弹原生文件选择框，
/// 一帧完不了，所以得把「正在关哪首 / 关完切回哪首」记下来，等对话框回来接着走。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingClose {
    /// 正在关闭的那首。
    pub id: crate::tabs::SongId,
    /// 问之前站在台前的那一首（原版 `previous_song`）：问完先切回它，再销毁。
    /// 要关的就是它时两者相同，切回去是空操作。
    pub previous: Option<crate::tabs::SongId>,
}

/// 欢迎窗 / 启动文件的 `load_song(..., replace = 1)`：先替换第 1 个标签，再载入新歌。
#[derive(Clone, Debug)]
pub struct PendingReplace {
    pub target: crate::tabs::SongId,
    pub previous: Option<crate::tabs::SongId>,
    /// `None` = 还在等用户从打开框选文件。
    pub path: Option<PathBuf>,
}

/// 标签条右键菜单的现场（用户要求 #131）。
#[derive(Clone, Copy, Debug)]
pub struct TabMenu {
    pub id: crate::tabs::SongId,
    pub pos: Pos2,
}

/// 标签拖拽现场（原版 `tabdrag` / `draggingtab` / `tabdest`）。
#[derive(Clone, Copy, Debug)]
pub struct TabDrag {
    pub id: crate::tabs::SongId,
    pub press_x: f32,
    pub grab_offset: f32,
    pub x: f32,
    pub target: usize,
    pub dragging: bool,
    pub span: f32,
}

/// 原版两条退出链：`request_quit()` 可取消；原生窗口关闭走 `control_end()`，不能取消。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitMode {
    Request,
    NativeClose,
}

/// 逐标签退出流程的现场（用户要求 #131）。
#[derive(Clone, Debug)]
pub struct QuitState {
    pub mode: QuitMode,
    /// 原版从 `array_length(songs) - 1` 往下问，快照保证中途换标签也不丢顺序。
    pub tabs: Vec<crate::tabs::SongId>,
    pub next: usize,
    pub previous: Option<crate::tabs::SongId>,
    /// 原版 `clear_backups`：只要有一首需要恢复，就保留所有标签的已有备份。
    pub clear_backups: bool,
    /// 开始退出前按每首当时的文件名记下备份路径；另存为改文件名后仍要删原备份。
    pub backup_paths: Vec<PathBuf>,
    /// 无文件名或重试另存为时，文件框还没回来。
    pub awaiting_save: Option<crate::tabs::SongId>,
}

/// 最近歌曲一条（原版 `recent_song[a]` + `recent_song_time[a]`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentPath {
    pub path: PathBuf,
    /// 上次打开时间的 unix 秒（原版 `date_current_datetime()`），欢迎窗显示相对时间。
    pub time: i64,
}

/// 开机更新检查的共享结果：后台线程写，UI 线程读。`status` 语义同原版
/// 全局变量 `update`（check_updates.gml）：-1 检查失败 / 0 检查中 / 1 有新版本 / 2 已是最新。
#[derive(Clone, Debug, Default)]
pub struct UpdateCheck {
    pub status: i8,
    /// `status == 1` 时的远端版本号（原版 `new_version`，用于询问框文案）。
    pub new_version: String,
}

pub type SharedUpdateCheck = std::sync::Arc<std::sync::Mutex<UpdateCheck>>;

impl ModalQueue {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 单个原版 `question(正文, 标题)`：答「是」才执行 `then`。
    pub fn question(title: &'static str, text: &str, then: Command) -> Self {
        Self {
            items: [Modal {
                title,
                text: text.into(),
                buttons: ModalButtons::YesNo,
            }]
            .into_iter()
            .collect(),
            then: Some(then),
            then_no: None,
        }
    }

    /// 两键问句，但「否」也要继续走另一条路（原生关闭时「不保存」仍会退出）。
    pub fn yes_no(title: &'static str, text: &str, then: Command, then_no: Command) -> Self {
        Self {
            items: [Modal {
                title,
                text: text.into(),
                buttons: ModalButtons::YesNo,
            }]
            .into_iter()
            .collect(),
            then: Some(then),
            then_no: Some(then_no),
        }
    }

    /// 单个原版 `message_yesnocancel(正文, 标题)`：三键问句。
    /// 「是」走 `then`、「否」走 `then_no`（不保存也要办完的那条路）、
    /// 「取消」两条都不走 —— 原版的 `close_song` 只在 `decision < 0` 时中止，
    /// 而取消键返回的就是 -1。
    pub fn yes_no_cancel(title: &'static str, text: &str, then: Command, then_no: Command) -> Self {
        Self {
            items: [Modal {
                title,
                text: text.into(),
                buttons: ModalButtons::YesNoCancel,
            }]
            .into_iter()
            .collect(),
            then: Some(then),
            then_no: Some(then_no),
        }
    }

    /// 单个原版 `message(正文, 标题)`。
    pub fn message(title: &'static str, text: &str) -> Self {
        Self {
            items: [Modal {
                title,
                text: text.into(),
                buttons: ModalButtons::Ok,
            }]
            .into_iter()
            .collect(),
            then: None,
            then_no: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Grid,
    Timeline,
    Layer(u32),
    /// 图层栏顶部的速度框（原版右键：单位切换 + 8 个预设速度）。
    Tempo,
}

#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub pos: Pos2,
    pub cell: (u32, u32),
    pub kind: ContextKind,
}

/// 原版首选项「歌曲」组里那两个目录：`songfolder` 和 `patternfolder`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefsFolder {
    Song,
    Pattern,
}

impl PrefsFolder {
    /// 原版 `draw_text_dynamic` 那行的前缀（冒号和空格由源码写在一起）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Song => tr("Song folder: ", "歌曲路径: "),
            Self::Pattern => tr("Pattern folder: ", "片段路径: "),
        }
    }

    /// 原版「修改」按钮里先弹的那句 `message(…, "")`。
    pub fn tip(self) -> &'static str {
        match self {
            Self::Song => tr(
                "Select the directory where saving/loading should be opened in.",
                "选择保存和打开的文件夹",
            ),
            Self::Pattern => tr(
                "Select the directory where patterns can be imported/exported to.",
                "选择导出和导入片段的文件夹",
            ),
        }
    }

    /// 原版系统对话框的标题（`draw_window_preferences.gml:278/294` 的第 4 个参数）。
    pub fn title(self) -> &'static str {
        match self {
            Self::Song => tr("Song folder", "歌曲文件夹"),
            Self::Pattern => tr("Pattern folder", "片段文件夹"),
        }
    }

    /// 原版传给 `get_save_filename_ext` 的默认文件名（第 2 个参数）。
    /// 原版并不真去创建这个文件：它只要用户按下「保存」那一刻对话框所在的目录
    /// （`songfolder = filename_dir(a)`），所以这个名字纯粹是占位 ——
    /// 但它确实显示在文件名栏里，照抄。
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Song => tr("Select song folder", "选择歌曲文件夹"),
            Self::Pattern => tr("Select patterns folder", "选择片段文件夹"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogAction {
    OpenSong,
    SaveSong,
    ExportAudio,
    ExportDatapack,
    ExportMidi,
    ExportPng,
    ExportSchematic,
    /// 原版 `track_export()` 的保存对话框：structure 选 .nbt，classic 选 .schematic。
    ExportTrack,
    ExportPattern,
    /// 端口自加：MAESTRO 机体节目单的保存框，扩展名 `.schem`（WorldEdit gzip 格式）。
    ExportMaestro,
    /// 端口自加：nbs-fractal 整机投影的保存框，扩展名 `.litematic`。
    ExportFractal,
    /// 端口自加：编码唱片数据箱的保存框，扩展名同样 `.litematic`，但内容只有箱子。
    ExportDiscChest,
    ImportMidi,
    TranscriptionAudio,
    TranscriptionMidi,
    TranscriptionModel,
    TranscriptionSoundFont,
    SaveTranscriptionMidi,
    ImportPattern,
    ImportSchematic,
    /// 原版「导入背景图片」：`menu_click.gml:57-58` / `:367-368`，选完直接 `wallpaper_init(temppath)`。
    ImportBackground,
    /// 原版 `datapack_getextranotes()`：保存随程序那份「更多音符盒」资源包。
    SaveExtraNotes,
    /// 原版「修改」按钮：`get_save_filename_ext("", "选择歌曲文件夹", songfolder, "歌曲文件夹")`。
    /// 原版取的是那个假文件名所在的目录，所以这里确定后认的是当前目录本身。
    PickFolder(PrefsFolder),
    /// 音色导入向导「更改」：原版走的是真正的目录选择器 `get_directory(mc_install_path)`
    /// （`draw_window_sound_import.gml:71`），与首选项那条占位文件名路子不同。
    PickMinecraftDir,
    /// 外部控制面板「添加目录…」：同一种真目录选择器，但结果写的是
    /// `prefs.control_dirs`（白名单），不是某个工程的安装路径。
    /// 单列一个动作而不是复用 `PickMinecraftDir`，是因为回填的目标窗口和落点都不同——
    /// 共用的话「添加白名单」会把 `.minecraft` 改掉。
    PickControlDir,
}

impl DialogAction {
    pub fn is_save(self) -> bool {
        matches!(
            self,
            Self::SaveSong | Self::ExportAudio | Self::ExportDatapack | Self::ExportMidi
                | Self::ExportPng | Self::ExportSchematic | Self::ExportTrack
                | Self::ExportPattern
                | Self::ExportMaestro
                | Self::ExportFractal
                | Self::ExportDiscChest
                | Self::SaveExtraNotes
                | Self::SaveTranscriptionMidi
                // 原版选文件夹走的是 `get_save_filename_ext`（见 `picker_spec` 里那条），
                // 不是目录选择器，所以这里也算「保存」式对话框。
                | Self::PickFolder(_)
        )
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::OpenSong | Self::SaveSong => "nbs",
            // 音频导出的扩展名跟着所选格式走，`FileDialog::set_ext` 会覆盖这里。
            Self::ExportAudio => "mp3",
            // 原版 dat_usezip 默认开，对话框就是 *.zip；关掉时改成选目录（见 set_ext("")）。
            Self::ExportDatapack => "zip",
            Self::ExportMidi | Self::ImportMidi | Self::TranscriptionMidi | Self::SaveTranscriptionMidi => "mid",
            Self::TranscriptionAudio => "wav",
            Self::TranscriptionModel => "safetensors",
            Self::TranscriptionSoundFont => "sf2",
            Self::ExportPng => "png",
            // 原版：get_open_filename_ext("Minecraft Schematics (*.schematic)|*.schematic", …)。
            Self::ExportSchematic | Self::ImportSchematic => "schematic",
            // 直轨导出 structure 用 .nbt、classic 用 .schematic，
            // 对话框打开前的默认名已按模式选好扩展名（commands.rs 的 ExportTrack 分支）。
            Self::ExportTrack => "nbt",
            Self::ExportPattern | Self::ImportPattern => "nbp",
            // 端口自加：MAESTRO 机体吃的是 WorldEdit 的 .schem（gzip），不是原版的 .schematic。
            Self::ExportMaestro => "schem",
            // 端口自加：fractal 出的是 Litematica 投影（gzip NBT，扩展名 .litematic）。
            Self::ExportFractal | Self::ExportDiscChest => "litematic",
            // 原版背景图对话框一次放行 png/jpg/jpeg（`menu_click.gml:57`）；
            // 这里的单扩展名只用于兜底列表和默认名，多个扩展名在 `accepts` 里放行。
            Self::ImportBackground => "png",
            // 原版：get_save_filename_ext("Resource pack (*.zip)|*.zip", "更多音符盒", "", …)。
            Self::SaveExtraNotes => "zip",
            // 原版这两个对话框的过滤器是空字符串：不做过滤，选的是目录。
            Self::PickFolder(_) => "",
            // 原版 get_directory 是目录选择器，没有过滤器。
            Self::PickMinecraftDir | Self::PickControlDir => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileDialog {
    pub save: bool,
    pub action: DialogAction,
    pub dir: PathBuf,
    /// 原版调用处有没有显式给起始目录。给了才让系统组件跳过去；
    /// 原版传 `""` 的那些（MIDI/Schematic 导入导出、数据包导出）本来就交给系统自己记。
    pub start_dir: bool,
    pub name: String,
    /// 实际生效的扩展名：文件类型过滤、补全文件名都用它，而不是 `action` 的默认值。
    pub ext: &'static str,
    pub entries: Vec<PathBuf>,
    pub error: Option<String>,
}

impl FileDialog {
    /// `start` = 原版调用处写死的起始目录（歌曲用 `songfolder`、片段用 `patternfolder`）；
    /// 给了就不看当前文件所在的目录，这一点和原版一致。
    pub fn for_action(
        action: DialogAction,
        initial: Option<&PathBuf>,
        start: Option<&Path>,
    ) -> Self {
        let dir = start.map(|d| d.to_path_buf()).unwrap_or_else(|| {
            initial
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .filter(|d| d.is_dir())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        });
        let base = initial
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        let ext = action.extension();
        let name = if let DialogAction::PickFolder(f) = action {
            f.placeholder().to_string()
        } else if matches!(action, DialogAction::PickMinecraftDir | DialogAction::PickControlDir) {
            // 目录选择器没有文件名栏。
            String::new()
        } else if action.is_save() {
            if ext.is_empty() {
                base
            } else {
                format!("{base}.{ext}")
            }
        } else {
            initial
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut d = Self {
            save: action.is_save(),
            action,
            dir,
            start_dir: start.is_some(),
            name,
            ext,
            entries: Vec::new(),
            error: None,
        };
        d.refresh();
        d
    }

    /// 换用另一个扩展名（音频导出按所选格式；数据包关掉 ZIP 时按原版空过滤器处理，
    /// 传 `""` 表示目标是文件夹）。同时改写默认文件名并重新过滤目录。
    pub fn set_ext(&mut self, ext: &'static str) {
        if self.ext == ext {
            return;
        }
        self.ext = ext;
        let p = Path::new(&self.name);
        if p.extension().is_some() {
            let renamed = if ext.is_empty() {
                p.file_stem().map(|s| s.to_string_lossy().into_owned())
            } else {
                p.with_extension(ext)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
            };
            if let Some(f) = renamed {
                self.name = f;
            }
        }
        self.refresh();
    }

    pub fn refresh(&mut self) {
        let ext = self.ext;
        let image_exts = matches!(self.action, DialogAction::ImportBackground);
        let audio_exts = matches!(self.action, DialogAction::TranscriptionAudio);
        let midi_exts = matches!(self.action, DialogAction::TranscriptionMidi);
        let transcription_file = matches!(self.action, DialogAction::TranscriptionModel | DialogAction::TranscriptionSoundFont);
        self.entries = std::fs::read_dir(&self.dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                let got = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                // 空扩展名 = 原版导出目录时的空过滤器：什么都列出来，名字当作目录。
                ext.is_empty()
                    || p.is_dir()
                    || got == ext
                    || (image_exts && matches!(got, "png" | "jpg" | "jpeg"))
                    || (audio_exts && matches!(got.to_ascii_lowercase().as_str(), "wav" | "flac" | "mp3" | "ogg" | "m4a" | "aac" | "aif" | "aiff" | "opus" | "webm"))
                    || (midi_exts && matches!(got.to_ascii_lowercase().as_str(), "mid" | "midi"))
                    || (transcription_file && got.eq_ignore_ascii_case(ext))
            })
            .map(|e| e.path())
            .collect();
        self.entries.sort();
    }

    pub fn selected_path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }

    /// 原生选择器要用的标题与文件类型。
    pub fn picker_spec(&self) -> PickerSpec {
        let ext = self.ext;
        let (title, filters) = match self.action {
            // 原版 `load_song.gml:20` 的过滤器还有 `Zipped Files (*.zip)` 和
            // `MIDI Sequences (*.mid)`；这两条路我们还没接通（带自定义音色的 .nbs 压缩包
            // 读不了，.mid 要先开 MIDI 导入窗口），列出来只会让人选了以后报错，所以先不列。
            DialogAction::OpenSong => (
                tr("Load song", "打开歌曲"),
                vec![
                    filter("Note Block Songs (*.nbs)", "*.nbs"),
                    filter("Minecraft Schematics (*.schematic)", "*.schematic"),
                ],
            ),
            // 原版 `save_song.gml:27`。
            DialogAction::SaveSong => (
                tr("Save song", "保存歌曲"),
                vec![filter("Note Block Songs (*.nbs)", "*.nbs")],
            ),
            // 原版 `audio_export.gml:24`：`audio_exp_format + " files (*" + output_ext + ")"`。
            DialogAction::ExportAudio => (
                tr("Export audio track", "导出音频文件"),
                vec![filter(
                    &format!("{} files (*.{ext})", ext.to_uppercase()),
                    &format!("*.{ext}"),
                )],
            ),
            DialogAction::ExportDatapack => {
                if ext.is_empty() {
                    // 原版 `datapack_export.gml:29-32`：目录选择框标题；过滤器串只有一个词、
                    // 没有 `|通配符`，传进系统组件就是一个空白模式；导出的目录名来自文件名栏，
                    // 不是选中的目录。
                    (
                        tr(
                            "Choose where to create the data pack folder",
                            "选择数据包文件夹的保存位置",
                        ),
                        vec![filter(tr("Data pack folder", "数据包目录"), "")],
                    )
                } else {
                    // 原版 `datapack_export.gml:21/24`。
                    (
                        tr("Data Pack Export", "导出数据包"),
                        vec![filter("ZIP archive (*.zip)", "*.zip")],
                    )
                }
            }
            // 原版 `open_midi.gml:14/15`。
            DialogAction::ExportMidi => (
                tr("Export MIDI", "导出 MIDI"),
                vec![filter("MIDI Sequences (*.mid)", "*.mid")],
            ),
            DialogAction::ExportPng => (
                tr("Export image", "导出图片"),
                vec![filter("PNG image (*.png)", "*.png")],
            ),
            DialogAction::ExportSchematic => {
                // 原版 `schematic_export.gml:7/8`：结构方块存的是 .nbt，过滤器名也不同；
                // 标题原文两种语言都是 "Export Schematic"（原版没翻译这句）。
                if ext == "nbt" {
                    (
                        "Export Schematic",
                        vec![filter("Minecraft Structures (*.nbt)", "*.nbt")],
                    )
                } else {
                    (
                        "Export Schematic",
                        vec![filter("Minecraft Schematics (*.schematic)", "*.schematic")],
                    )
                }
            }
            DialogAction::ExportTrack => {
                // 原版 `track_export.gml:7-11`：标题两种语言都是 "Export Track"，
                // 过滤器跟 Schematic 导出同款（结构方块 .nbt / 经典 .schematic）。
                if ext == "nbt" {
                    (
                        "Export Track",
                        vec![filter("Minecraft Structures (*.nbt)", "*.nbt")],
                    )
                } else {
                    (
                        "Export Track",
                        vec![filter("Minecraft Schematics (*.schematic)", "*.schematic")],
                    )
                }
            }
            // 原版 `pattern_export.gml:12`。
            DialogAction::ExportPattern => (
                tr("Save pattern", "保存分段"),
                vec![filter("Note Block Pattern (*.nbp)", "*.nbp")],
            ),
            // 端口自加：fractal 整机投影。Litematica 认 .litematic（gzip NBT）。
            DialogAction::ExportFractal => (
                tr("Export Fractal machine", "导出 Fractal 机体投影"),
                vec![filter("Litematica Projection (*.litematic)", "*.litematic")],
            ),
            // 端口自加：编码唱片数据箱。同一份 Litematica 容器，只是里面只有箱子没有红石。
            DialogAction::ExportDiscChest => (
                tr("Export encoded disc chests", "导出编码唱片数据箱"),
                vec![filter("Litematica Projection (*.litematic)", "*.litematic")],
            ),
            // 端口自加：MAESTRO 节目单。扩展名用机体那边认的 .schem。
            DialogAction::ExportMaestro => (
                tr("Export MAESTRO sheet", "导出 MAESTRO 节目单"),
                vec![filter("Minecraft Schematic (*.schem)", "*.schem")],
            ),
            // 原版 `open_midi.gml:14/15`。
            DialogAction::ImportMidi => (
                tr("Import from MIDI", "从 MIDI 导入"),
                vec![filter("MIDI Sequences (*.mid)", "*.midi;*.mid")],
            ),
            DialogAction::TranscriptionAudio => (
                tr("Choose audio to transcribe", "选择待识别的音频"),
                vec![filter("Audio files", "*.wav;*.flac;*.mp3;*.ogg;*.m4a;*.aac;*.aif;*.aiff;*.opus;*.webm")],
            ),
            DialogAction::TranscriptionMidi => (
                tr("Preview original MIDI", "预览原始 MIDI"),
                vec![filter("MIDI Sequences", "*.mid;*.midi")],
            ),
            DialogAction::TranscriptionModel => (
                tr("Choose local model weights", "选择本地模型权重"),
                vec![filter("SafeTensors weights", "*.safetensors")],
            ),
            DialogAction::TranscriptionSoundFont => (
                tr("Choose GM SoundFont", "选择 GM 音色库"),
                vec![filter("SoundFont 2", "*.sf2")],
            ),
            DialogAction::SaveTranscriptionMidi => (
                tr("Save original MIDI", "保存原始 MIDI"),
                vec![filter("MIDI Sequences", "*.mid")],
            ),
            // 原版 `pattern_import.gml:23`。
            DialogAction::ImportPattern => (
                tr("Load pattern", "打开分段"),
                vec![filter("Note Block Pattern (*.nbp)", "*.nbp")],
            ),
            // 原版 `open_schematic.gml:11`：标题原文两种语言都是 "Import from Schematic"。
            DialogAction::ImportSchematic => (
                "Import from Schematic",
                vec![filter("Minecraft Schematics (*.schematic)", "*.schematic")],
            ),
            // 原版 `menu_click.gml:57/367`：过滤器和标题都是照抄的这一句。
            DialogAction::ImportBackground => (
                tr("Open background image", "打开背景图片"),
                vec![filter(
                    "Image Files (*.png, *.jpg, *.jpeg)",
                    "*.png;*.jpg;*.jpeg",
                )],
            ),
            // 原版 `datapack_getextranotes.gml:6`：过滤器名和标题都是照抄的这一句。
            DialogAction::SaveExtraNotes => (
                tr("Save extra notes for data pack", "为数据包保存音符盒资源包"),
                vec![filter("Resource pack (*.zip)", "*.zip")],
            ),
            // 原版 `draw_window_preferences.gml:278/294` 是 `get_save_filename_ext("", …)`：
            // 过滤器整串为空，文件名栏塞的是占位串，按下保存后取的是它所在的目录
            // （换算回来见 `EditorState::run_pending_dialog`）。不是目录选择器。
            DialogAction::PickFolder(f) => (f.title(), Vec::new()),
            // 原版 `draw_window_sound_import.gml:70-71`：`widget_set_caption(...)` +
            // `get_directory(mc_install_path)`，真目录选择器，标题即 caption。
            DialogAction::PickMinecraftDir => (
                tr(
                    "Select Minecraft installation directory",
                    "选择Minecraft安装目录",
                ),
                Vec::new(),
            ),
            DialogAction::PickControlDir => (
                tr(
                    "Add a directory external control may read and write",
                    "添加允许外部控制读写的目录",
                ),
                Vec::new(),
            ),
        };
        PickerSpec {
            title: title.to_string(),
            filters,
            pick_folder: matches!(
                self.action,
                DialogAction::PickMinecraftDir | DialogAction::PickControlDir
            ),
        }
    }
}

/// 宿主注入的原生文件选择器的返回结果。
#[derive(Clone, Debug)]
pub enum PickResult {
    /// 用户选定了一个路径。
    Picked(PathBuf),
    /// 用户取消了。
    Cancelled,
    /// 这台机器上没有可用的原生对话框（COM 起不来等），调用方回落到自绘选框。
    Unavailable,
}

/// 操作系统的文件选择对话框。编辑器自己不碰 Win32，由 `nbs-app` 在启动时装进来；
/// 没装（非 Windows、或单测）就继续用自绘的那套。
pub trait FilePicker: Send + Sync {
    fn pick(&self, dlg: &FileDialog) -> PickResult;
}

/// 原生下拉框里的一个文件类型。`patterns` 是分号分隔的通配符，和原版过滤器串的后半段一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterSpec {
    pub name: String,
    pub patterns: String,
}

/// 原生对话框需要的文案：标题 + 文件类型。
/// 全部照抄原版各调用点传给 `get_save_filename_ext` / `get_open_filename_ext` 的参数
/// （中文分支，即 `language == 1`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerSpec {
    pub title: String,
    pub filters: Vec<FilterSpec>,
    /// 真目录选择器（原版 `get_directory`）：原生组件加 `FOS_PICKFOLDERS`，
    /// 不设文件类型与文件名。
    pub pick_folder: bool,
}

fn filter(name: &str, patterns: &str) -> FilterSpec {
    FilterSpec {
        name: name.to_string(),
        patterns: patterns.to_string(),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayerDrag {
    pub index: u32,
    /// false = 音量, true = 声像
    pub stereo: bool,
    /// 量化后显示 / 提交用的值（原版不带 Shift 时按 10 取整）。
    pub value: u8,
    /// 未量化的累加值，对应原版 `dragvol` / `dragstereo`：
    /// 慢慢拖也能越过 10 的台阶，量化只影响落到模型上的那一份。
    pub raw: f32,
}

/// 原版 `tempo_changer_set_tempo` / `sound_stopper_set_start` / `sound_stopper_set_until`：
/// 两个事件乐器编辑窗里 `draw_textarea` 的原始字符串（能整成 int64 才算合法）。
/// 一次只开一个窗，三个草稿共用一个结构体。
#[derive(Clone, Debug, Default)]
pub struct EventEditDraft {
    pub tempo: String,
    pub start: String,
    pub until: String,
}

/// 原版 `w_tempotapper`（速度测量器）的会话状态：`taptempo` / `tapping` / `ltime` /
/// `taps`（`control_create.gml:561-564`）。这四个是纯会话变量，不进 settings.ini，
/// 窗口一关就清零（OK / Cancel 在 `draw_window_tempo_tapper.gml:56-58` 与 `68-70`，
/// Esc 在 `draw_windows.gml:94-99`）。
/// 勾选项 `tapdouble` 不在这个结构里：原版把它当偏好存盘（`save_settings.gml:71` /
/// `load_settings.gml:78`），所以它住 `prefs.tapdouble`，关窗不清零。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TempoTapper {
    /// `taptempo`：目前测到的 BPM。
    pub bpm: f64,
    /// `tapping`：已经打过至少一下。
    pub tapping: bool,
    /// `ltime`：上一次打点的毫秒时刻。
    pub last_ms: i64,
    /// `taps`：已计入平均的间隔数。
    pub taps: u32,
}

impl TempoTapper {
    /// 原版 `draw_window_tempo_tapper` 里按「T」那一段：
    /// `taptempo = (60 / dt秒 + taptempo * taps) / (taps + 1)`，
    /// 间隔短到「新一拍比现有测量值快一倍」时视为重新起打，清零重来。
    /// 返回是否要发一声打点音（原版 `if (ins.loaded) play_sound(ins, 45, 100, 100, 0)`）。
    /// 原版同一毫秒连按会得到 `60 / 0 = inf` 直接写进 `taptempo`，端口不照抄：
    /// 那样界面要显示 inf、确认时还会把 inf 送进速度，这里只更新时间与打点数。
    pub fn register_tap(&mut self, now_ms: i64) -> bool {
        let dt_ms = now_ms - self.last_ms;
        if dt_ms > 0 {
            let instant = 60.0 / (dt_ms as f64 / 1000.0);
            let ltempo = self.bpm;
            if self.tapping {
                let denom = if ltempo != 0.0 {
                    self.taps as f64 + 1.0
                } else {
                    1.0
                };
                self.bpm = (instant + ltempo * self.taps as f64) / denom;
            }
            if (instant + 0.5).floor() <= ltempo / 2.0 && ltempo != 0.0 {
                self.bpm = 0.0;
                self.taps = 0;
            }
        }
        self.tapping = true;
        self.last_ms = now_ms;
        self.taps += 1;
        true
    }

    /// 原版 `ctempo = floor(taptempo + 0.5)`：界面显示和确认都用它。
    pub fn measured(&self) -> f64 {
        (self.bpm + 0.5).floor()
    }

    /// 原版关窗时的三条复位（OK / Cancel 在 `draw_window_tempo_tapper.gml:56-58`
    /// 和 `68-70`，Esc 在 `draw_windows.gml:94-99`）。`taps` 照原版不清：那一项只在
    /// `(taps + 1) * (ltempo != 0) + (!ltempo)` 里参与，而 `taptempo` 已经归零，
    /// 分母塌成 1，上一轮残留的计数值传不到新一轮来。
    pub fn reset_for_close(&mut self) {
        self.bpm = 0.0;
        self.tapping = false;
        self.last_ms = 0;
    }
}

/// 打点用的毫秒时钟：原版用 `current_time`（进程启动以来的毫秒），端口拿
/// 首次调用作原点，够单调且不受系统时间调整影响。
pub fn tap_clock_ms() -> i64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as i64
}

pub struct EditorState {
    /// 台前标签的文档：真身住在 [`Self::store`] 的槽位里，这份是切标签时交换过来的
    /// （见 [`Self::switch_tab`]）。现有 658 处 `state.doc` 引用照旧直接用。
    pub doc: Document,
    /// 原版 `global.songs[]` + `global.song` 的替身：歌曲标签槽位表。
    pub store: crate::tabs::SongStore,
    /// 原版台前 `song.song_backup_path`：切标签时与文档一起收装。
    pub song_backup_path: Option<PathBuf>,
    pub viewport: Viewport,
    /// 原版 `hires`（`control_create.gml:110`：`window_scale > 1.25`，绝对倍率含
    /// DPI）：Fluent 主题的画布精灵换 4 倍源图。每帧随 `apply_ui_scale` 更新。
    pub canvas_hires: bool,
    /// 端口自加（用户要求 #92）：界面缩放低于 `prefs.note_text_min_percent` 时，
    /// 音符方块上的字（序号 / 键名）整批不画。和 `canvas_hires` 一样每帧随
    /// `apply_ui_scale` 更新，渲染侧只读这个现成的布尔，不参与换算。
    pub hide_note_text: bool,
    pub interaction: Interaction,
    pub tool: EditMode,
    /// 当前画笔（放置音符时的 instrument/key/vel/pan/pit）。
    pub current: Note,
    pub flags: UiFlags,
    pub theme: Theme,
    pub window: Option<WindowKind>,
    /// 上一帧在最上层的窗口，对应原版全局变量 `prevwindow`。原版首选项的 Ctrl+P
    /// 关窗条件是 `prevwindow == window`：开窗那一帧按键事件仍然有效，不加这道闸
    /// 就会被「Ctrl+P 开」和「Ctrl+P 关」在同一帧里连着执行，窗口一闪就没。
    pub window_prev: Option<WindowKind>,
    /// 自绘选框顶掉的那个窗口。原版的系统对话框是叠在原窗口上的，选完原窗口还在；
    /// 我们同时只开一个窗口，兜底路径得记住被顶掉的是哪个（走原生组件时不会被顶掉，这里就是 None）。
    pub dialog_return: Option<WindowKind>,
    /// 原版的全屏开关（F11）。
    pub fullscreen: bool,
    /// 原版 `sb_sel`（初值 0，`draw_scrollbar.gml` 里按到哪根条就置成那条的号）：
    /// 最后一次碰过的滚动条，0 = 横向、1 = 纵向。原版 Home / End / PageUp / PageDown
    /// 只翻这一根指向的轴，且按下后不复位。端口只有主界面两根条参与（窗口内的列表
    /// 条是 egui 自绘的，原版那些条被按下时会把 `sb_sel` 抢走、导致编辑器按键空转，
    /// 这个副作用不复刻）。
    pub sb_sel: u8,
    /// 原版 `isplayer`（`control_create.gml:28-32`）：`-player` / `--protocol-launcher`
    /// 启动或欢迎窗按钮进入的只读播放档位。运行时标志不进 prefs。
    pub player: bool,
    pub context_menu: Option<ContextMenu>,
    pub message: Option<(String, Instant)>,
    /// 真实音频引擎（无音频设备时自动降级为静音时钟）。
    pub audio: Box<dyn AudioEngine>,
    pub prefs: Preferences,
    /// 字体表是否已装进本窗口的 `Context`（`fonts::install` 里 `set_fonts` 会整张
    /// 表做字节比较，代价不小，只装一次）。
    pub fonts_installed: bool,
    pub marker_follow: bool,
    /// 原版 `marker_pagebypage`：1 = 按页，0 = 按刻，2 = 按帧。
    pub follow_mode: u8,
    /// 原版 `section_exists` / `section_start` / `section_end`
    /// （`obj_song/Create_0.gml:22-24`）：时间轴上的一段区间。原版挂在歌曲对象上、
    /// **不写进 .nbs**（`save_song.gml:119` 只写 `loopstart`），所以端口把它当会话状态，
    /// 换歌、新建即清空。拖拽途中允许 `section_end < section_start`，松手才排序
    /// （`control_draw.gml:1108-1118`）。
    pub section_exists: bool,
    pub section_start: i64,
    pub section_end: i64,
    /// 上一帧推给播放时钟的 `marker_end` 停点，只在它变化时才推。
    stop_at_pushed: f64,
    pub hover_cell: Option<(u32, u32)>,
    /// 原版 `w_tempotapper`（速度测量器）的会话状态。
    pub tapper: TempoTapper,
    /// 原版全局 `save_options_version`：保存选项窗口里的单选草稿，
    /// 打开时播种成歌曲的 `save_version`，点确定才写回歌曲。不落盘。
    pub save_options_version: u8,
    pub pending: Vec<Command>,
    pub file_dialog: Option<FileDialog>,
    /// 宿主装进来的原生选择器（Windows 上是 `IFileOpenDialog` / `IFileSaveDialog`）。
    pub picker: Option<std::sync::Arc<dyn FilePicker>>,
    /// 外部控制链路（MCP 桥 ↔ 本进程）。`None` = 这个进程没开外部控制，
    /// 每帧的 `control::drain` 因此是零成本。收件箱与任务表都挂在这里，
    /// 服务端句柄由 nbs-app 持有（它要在退出时停服务、删发现文件）。
    pub control: Option<crate::control::ControlChannel>,
    /// 外部控制面板这一帧提的生命周期要求，宿主处理完置回 `None`。
    /// 服务端句柄在 nbs-app（它要在退出时停服务、删发现文件），这里只放「要它做什么」。
    pub control_change: Option<crate::control::ControlChange>,
    /// 服务开着时才有的可显示信息（端口 / 发现文件绝对路径 / 来源 / danger）。
    /// 由宿主在起停之后填；`None` = 现在没监听，面板就照着写。
    pub control_info: Option<crate::control::ControlInfo>,
    /// 等着被原生选择器处理的请求。在帧外（`raw_input_hook`）取走，避免在 egui 帧内
    /// 起一个会泵消息的模态框。
    pub pending_dialog: Option<FileDialog>,
    /// 关标签走到「先保存」这一步时挂住的现场（用户要求 #131）。
    pub pending_close: Option<PendingClose>,
    /// 欢迎窗 / 启动文件替换首标签的现场（用户要求 #131）。
    pub pending_replace: Option<PendingReplace>,
    /// 标签条右键菜单；点外或选完一条即清。
    pub tab_menu: Option<TabMenu>,
    /// 标签拖拽；起拖后屏蔽其它输入，松手才重排。
    pub tab_drag: Option<TabDrag>,
    /// 「关闭其他/右侧歌曲」的逐首确认队列；取消则整组作废。
    pub pending_close_set: Option<Vec<crate::tabs::SongId>>,
    /// 原版 `quit_prompting` + 逐标签确认现场；`None` 表示当前没在退出确认。
    pub quit: Option<QuitState>,
    /// 原版 `quit_confirmed`：确认全部完成后，`control_end` 不再重复问。
    pub quit_confirmed: bool,
    /// 原版 `recent_song[0..10]`（路径 + 打开时间）。
    pub recent_files: Vec<RecentPath>,
    /// 开机更新检查的共享结果。没开「检查更新」时状态一直是 0（原版 `update_http = -1`）。
    pub update_check: SharedUpdateCheck,
    /// 原版在 check_updates 的回调里 `question(下载？)` 只问一次：这里就是那道闸。
    pub update_question: bool,
    /// 原版 `update_success`（`control_create.gml:747-753`）：上次运行版本和当前不同时
    /// 置 1，欢迎窗显示「更新成功！」并重新亮出捐赠横幅。
    pub update_success: bool,
    /// 原版全局 `changelogstr`（`control_create.gml:754`：
    /// `load_text(bundled_data_directory + "changelog.txt")`）。读不到是 `None`，
    /// 对应原版的 `changelogstr = 0`——打开更新历史时弹「找不到更新历史！」并把窗收掉。
    pub changelog: Option<String>,
    /// 原版 `sb_val[update_scrollbar]`：更新历史那根竖滚动条的读数，单位是行。
    pub update_scroll: f32,
    /// 原版全局 `creditsstr`（`control_create.gml:755`：
    /// `load_text(bundled_data_directory + "credits.txt")`）：关于窗右侧那栏谢词。
    /// 读不到是 `None`。原版没有兜底分支（`string_count("\n", 0)` 直接报错），
    /// 端口按「一栏都不画」处理，文件本身随程序一起发。
    pub credits: Option<String>,
    /// 原版 `sb_val[credits_scrollbar]`：谢词栏那根竖滚动条的读数，单位是行。
    pub credits_scroll: f32,
    /// 原版 `w_setaccent` 的编辑态（`hsv` / `rr,gg,bb` / `text_str[60..62]`）。
    /// 只有取色器开着才有值，关窗就丢掉——原版靠 `resetcolor` 每次复位，效果一样。
    pub accent_pick: Option<AccentPick>,
    /// 原版 `donate_banner`：捐赠横幅是否显示（跟着欢迎窗走）。
    pub donate_banner: bool,
    pub editing_layer_name: Option<(u32, String)>,
    /// 事件乐器编辑窗的输入草稿（见 [`EventEditDraft`]），开窗时由落格逻辑回填。
    pub event_edit: EventEditDraft,
    /// 图层音量/声像拖拽预览（松开时才提交一条历史，避免拖拽产生几百条）。
    pub layer_drag: Option<LayerDrag>,
    /// 原版 `obj_removeeff` 实例：删除音符时格子上的淡出边框。
    /// `rel` 是相对当前视口左上格的格坐标（原版存 `xx - starta`，不随滚动移动）。
    pub remove_effects: Vec<RemoveFx>,
    /// 原版 `draw_window_macro_arpeggio` 的 57 号文本框（琶音规律）。原版把它存在
    /// `draw_text_edit` 的会话缓冲里：跨开关窗保留，但不落 settings.ini。
    pub arpeggio_pattern: String,
    /// 同上，`draw_window_macro_stagger` 的 58 号文本框（错开规律）。
    pub stagger_pattern: String,
    /// 原版会话全局 `porta_reverse` / `port_cent`（`control_create.gml:658-659`），
    /// 滑音窗口的勾选与偏移量；同样只在会话里活着。
    /// 原版 `port_cent` 是浮点（拖拽灵敏度 0.1），端口沿用了 `Page::drag` 的整数走查。
    pub porta_reverse: bool,
    pub port_cent: i32,
    /// 原版颤音窗口的五枚会话参数（`control_create.gml:653-657`：
    /// `tremolotype = 0`、`trem_spacing = 1`、`fade_auto = 1`、`leg_dec = 20`、
    /// `leg_sus = 20`）。`tremolotype` 是 0 无音量变化 / 1 淡入 / 2 淡出，
    /// 决定确定时调 `macro_tremolo` / `_fadein` / `_fadeout` 哪一个。
    /// 原版「音符间隔」那一格钳在 `median(1, 拖拽值, 2)`，也就是只有 1 和 2。
    pub tremolotype: i32,
    pub fade_auto: bool,
    pub leg_dec: i32,
    pub leg_sus: i32,
    pub trem_spacing: i32,
    /// 原版会话全局 `setvel` / `percentvel` / `setpan` / `setpit` / `addpitch` /
    /// `stereo_width` / `stereo_reverse`（`control_create.gml:566-663`，出厂
    /// 100 / 0 / 0 / 0 / 0 / 50 / 0），四个批量窗口的草稿值，同样不落 settings.ini。
    /// 原版 `setvel` 在百分比模式下能拖到 1000（`100 + percentvel * 900`），
    /// 所以这里用 `i32` 而不是 `u8`。
    pub set_vel: i32,
    pub vel_percent: bool,
    pub set_pan: i32,
    pub set_pit: i32,
    pub pitch_additive: bool,
    pub stereo_width: i32,
    pub stereo_reverse: bool,
    pub midi_options: MidiImportOptions,
    pub midi_enhanced: nbs_io::midi_enhanced::EnhancedMidiOptions,
    pub midi_enhanced_options: MidiImportOptions,
    pub midi_enhanced_check: Option<Result<nbs_io::midi_enhanced::EnhancedImportReport, String>>,
    pub transcription: crate::transcription::TranscriptionState,
    /// 原版 `midi_*` 那一堆全局：解析好的 MIDI。导入窗口只在这有值时才打开——
    /// 原版 `open_midi()` 是先解析、再 `window = w_midi`，没有「先开空窗口」这条路。
    pub midi_data: Option<nbs_io::midi::MidiData>,
    /// 原版 `songs[song].midifile`：窗口右上角显示的文件名。
    pub midi_file: String,
    /// 原版 `w_midi_tab`：0 乐器 / 1 打击乐 / 2 轨道 / 3 音符长度。
    pub midi_tab: usize,
    /// 原版 `sb_val[midi_sb1..midi_sb4]`：四页各自的滚动行数（拖拽时可以是小数，
    /// 取行号时才 `floor`，和原版一样）。
    pub midi_scroll: [f32; 4],
    /// 原版 `menu_shown`：导入窗口一次只允许开一个下拉菜单，下标是那一行的行号。
    pub midi_menu: Option<MidiMenu>,
    /// 原版 `obj_menu.menu_show[]`：打击乐音高那种带子菜单的下拉，当前展开的是第几行。
    pub midi_submenu: Option<usize>,
    /// 原版 `draw_dragvalue` 的亚像素累加器（通道最高层数 / 尾音起始 / 尾音结束）。
    pub midi_drags: [f32; 3],
    /// 原版 `midi_ins[*, 1..2]` / `midi_drum[*, 1..2]` 的生效副本：出厂表 +
    /// 「记住我的更改」写回的改动。
    pub midi_gm: nbs_io::MidiGmTable,
    pub schematic_options: SchematicOptions,
    /// 端口自加：MAESTRO 机体节目单的导出选项（只有取整模式），随首选项落盘。
    pub maestro_options: nbs_io::MaestroOptions,
    /// 端口自加：nbs-fractal 机体投影的导出选项（布局族 + 奇偶分道 + 几个几何参数），随首选项落盘。
    pub fractal_options: nbs_io::FractalOptions,
    /// 上次/当前预览算出的改写清单。窗口每次开、每次换模式都会重算，
    /// 导出成功后也用它给出「实际改写了什么」。
    pub maestro_report: Option<nbs_io::MaestroReport>,
    /// 预览失败的原因（空歌、超出箱子容量等），窗口用它替代清单显示。
    pub maestro_error: Option<String>,
    /// 端口自加：fractal 投影的预览清单与失败原因，用法同上面那对 maestro 字段。
    pub fractal_report: Option<nbs_io::FractalReport>,
    pub fractal_error: Option<String>,
    /// 端口自加：编码唱片数据箱的导出选项（机体档 + 时间处理方式 + 层高 + 留白/空行两条开关），
    /// 随首选项落盘。
    pub disc_chest_options: nbs_io::DiscChestOptions,
    /// 端口自加：编码唱片的预览清单与失败原因，用法同上面那对 fractal 字段。
    pub disc_chest_report: Option<nbs_io::DiscChestReport>,
    pub disc_chest_error: Option<String>,
    /// 原版 `calculate_size()` 每次打开导出窗口时重算的歌曲统计。
    pub schematic_stats: nbs_io::schematic::SchematicStats,
    /// 原版 `selected_tab_sch`：0 = 设计，1 = 方块。关窗时回到 0。
    pub schematic_tab: u8,
    /// 原版 `sch_exp_remember`。
    pub schematic_remember: bool,
    /// 原版 `sch_command_plan`：结构导出「声源分配」的缓存方案（有界预算）。
    pub schematic_command_plan: Option<nbs_io::mc_export::Plan>,
    /// 原版 `sch_command_signature`：方案缓存签名，歌/选项任一变化即失效。
    pub schematic_command_signature: String,
    /// 原版 `string(o.songs[o.song])` 的替代：台前标签的缓存身份，号由
    /// [`crate::tabs::SongStore`] 发，开档 / 新建 / 整首替换才变，内容编辑不变
    /// （与原版实例 id 语义一致）。
    pub song_session: u64,
    /// 原版 `obj_controller.log_strs`：导出报告等日志行（暂不落盘文件）。
    pub log_strs: Vec<String>,
    /// 原版 `message()` / `question()` 的待弹队列。
    pub modals: ModalQueue,
    pub datapack_options: DatapackOptions,
    /// 原版 `selected_tab_dat`：0 = 设置，1 = 效果器。
    pub datapack_tab: u8,
    /// 原版 `selected_tab_mc`：兼容性窗口的两页，0 = Schematic，1 = 数据包。
    pub mc_tab: u8,
    /// 原版 `selected_tab`：首选项的五页，0 通用 / 1 界面 / 2 外观 / 3 访问 / 4 播放。
    /// 原版是全局变量（窗口间共享），我们只在这里用，关窗不清零也不影响别的窗口。
    pub prefs_tab: u8,
    /// 原版 `draw_abutton` 展开的 `obj_menu`：首选项里主题下拉框是否开着。
    pub prefs_theme_menu: bool,
    /// 同上，页 1「帧率限制」的下拉框。原版的 `wmenu` 一次只允许开一个，
    /// 菜单自带的整屏拦截区会先收掉已开的那个，所以两个开关不会同时为真。
    pub prefs_rate_menu: bool,
    /// 同上，页 1「语言」的下拉框（draw_window_preferences.gml:412-421）。
    pub prefs_language_menu: bool,
    /// 端口自加（用户要求 #94）：首选项「播放」页输出设备下拉是否开着。
    pub prefs_output_menu: bool,
    /// 端口自加 #94：顶栏音量条右侧那个「输出」下拉是否开着。
    pub output_menu: bool,
    /// 端口自加 #94：输出设备表。cpal 每次枚举都要过一遍系统，只在打开下拉时刷新。
    pub output_list: Vec<nbs_audio::OutputDevice>,
    /// 原版 `w_dragvalue`：`draw_dragvalue` 的亚步进累加器。
    pub prefs_drag: f32,
    pub png_options: ImageOptions,
    pub export_status: Option<String>,
    /// 原版 `startkey`：最左白键的白键序号（不是键号，键号还要加上左边的黑键数）。
    pub piano_start: u8,
    /// 原版 `keysshow`：上一帧画了几根白键，宽度变了要重新居中。
    pub piano_keys_shown: u8,
    /// 原版 `delay`：边缘自动平移的节流计时（秒）。
    pub piano_scroll_delay: f32,
    /// 端口自加：上一帧琴键面板的屏幕矩形，缩放快捷键靠它判断指针是否悬停在
    /// 琴键上（悬停时 Ctrl+滚轮 / ± 调 `piano_scale` 而不是 `window_scale`）。
    pub piano_rect: Option<egui::Rect>,
    pub last_autosave: Instant,
    pub last_prefs_save: Instant,
    /// 已同步给音频引擎的 `Song::rev`，用于只在内容真变化时重建事件表。
    pub audio_rev: u64,
    /// 已经交给窗口管理器的图标档位（原版 `window_icon`）。`None` = 还没发过，
    /// 对应原版 `icon_display` 初值为 1：第一帧必发一次。
    pub applied_window_icon: Option<bool>,
    /// 端口自加（用户要求 #119）：这一帧用户改了「垂直同步」，`Some(新值)` 留给宿主
    /// （nbs-app）在帧外真切一次呈现层——编辑器够不着 GL，只能提要求。
    pub vsync_request: Option<bool>,
    /// 原版精灵图（工具栏 / 图层栏 / 宏条的图标按钮）。
    pub icons: crate::icons::Icons,
    /// 原版 `showinsbox`：工具栏音色框是否展开成整窗网格。
    pub insbox_open: bool,
    /// 原版 `window += w_menu`（show_menu_ext.gml:21）：顶栏下拉在这一帧里开过。
    /// 三条输入闸门都读它——画布（grid.rs 的 `canvas_blocked`）、琴键（piano.rs）、
    /// egui 控件（app.rs 那层挡板）。用户要求 #129：关菜单的那一次点击不许穿透到别处。
    /// 口径是「本帧开过」而不是「帧末还开着」：收菜单那一下的判定就发生在本帧的
    /// `menu_button` 里，只认帧末会让闸门同一帧落下，那一下点击照样穿透
    /// （`a_dismiss_click_on_the_canvas_only_closes_the_menu` 实测到过）。
    pub menubar_open: bool,
    /// 上一帧帧末的原始开合，只用来算出本帧的 `menubar_open`（见 `show_menu_bar`）。
    pub menubar_open_end: bool,
    /// 原版 `insbox_start`：收起状态下这一行从第几个音色开始画。
    pub insbox_start: usize,
    /// 原版 `forward`：按住快退 / 快进时的滚动方向（-1 / 0 / +1）。
    pub scrub: i8,
    /// 原版 `record`：工具栏「录制按键」开关。开着时按键不 preview，而是直接写进当前列。
    pub record: bool,
    /// 原版 `playing = 0.25`：按下了录音但还没出声，等第一个按键来启动播放。
    pub record_armed: bool,
    /// 原版 `key_press[a]`：当前按住的按键绑定，琴键要画成按下态。
    pub keys_held: crate::piano::KeysHeld,
    /// 原版 `key_played[a]`：每个键最近一次被演奏的时刻（当帧的 `i.time`，秒）。
    /// 琴键在它之后 300 毫秒内染黄并弹下（`draw_piano.gml:67/80/114`）。
    pub key_played: crate::piano::KeysPlayed,
    /// 原版实时 MIDI 那一堆全局：设备表、指派、本帧按下/松开队列，见
    /// [`crate::live_midi`]。
    pub live_midi: crate::live_midi::LiveMidi,
    /// 原版「导入音效」向导的一组全局（`sound_import_*` + `mc_install_path`）。
    pub sound_import: crate::sound_import::SoundImport,
    /// 原版 `tempodrag`：拖速度框时的未量化累加值。
    pub tempo_drag: Option<f32>,
    /// 原版 `window = w_settempo` + `text_value[64]`：标尺速度格上就地弹出的输入框，
    /// `Some` 表示正开着，内容就是框里那份还没提交的文字。
    pub tempo_input: Option<String>,
    /// 原版会话全局 `settempo`：最近一次左键按下是否落在这格输入框里。
    /// 原版只有「按下不在框里、抬手也在框外」才提交（`draw_window_set_tempo.gml:26-31`）。
    pub tempo_press_in: bool,
    /// 兼容性指示（原版 `compatible`），随 `song.rev` 重算；拖速度框时板上的数还没写进
    /// 歌曲，所以判定用的那个速度也得进缓存键，否则徽标要等松手才改口。
    pub compat_rev: u64,
    pub compat_tempo: f32,
    pub compat: u8,
    /// 原版 `block_outside` / `block_custom`：兼容性窗口那两条计数，
    /// 和 `compat` 同一次遍历算出、同一个 `compat_rev` 缓存。
    pub compat_outside: usize,
    pub compat_custom: usize,
    /// 原版 `rainbowtoggle`：界面强调色的彩虹循环。开关只有两处——
    /// theme 3 的 Ctrl+Shift+F5（发消息）和 Toggle Rainbow 事件乐器（不发）。
    pub rainbow: bool,
    /// 原版 `rainbow`：色相 0..360（`control_create.gml:154` 初值 0，回绕归 1）。
    pub rainbow_hue: f32,
    /// 原版 `backgroundaccent`：Fluent 主题把整窗先铺一层 `accent[3]`
    /// （`control_draw.gml:69`），只有事件乐器能切，原版没有快捷键也不持久化。
    pub background_accent: bool,
    /// 原版 `wpaper` + `wpaperexist`：导入的背景图（糊好的贴图）。
    /// 原版既不写进 .nbs 也不写进 settings.ini，重启即失，端口一致。
    pub wallpaper: Option<crate::background::Wallpaper>,
    /// 播放特效那一坨：烘出来的时间轴、乐器方块表、贴图、启用中的特效。
    /// 缓存键是 `song.rev`（和 [`Self::compat_rev`]、[`Self::audio_rev`] 同一套路）。
    pub fx: crate::fx::FxState,
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

/// 原版 Schematic 只认这三档速度（`compatible = 1` 的前半个条件，也是兼容性窗口
/// Schematic 页第一行的判据——那一行不看音符微调）。
pub fn tempo_schematic_ok(tempo: f32) -> bool {
    [10.0_f32, 5.0, 2.5]
        .iter()
        .any(|v| (tempo - v).abs() < 1e-3)
}

/// 原版数据包页第一行说「更佳」的那十档。数据包本身任意速度都能导，
/// 所以差档只给橙色图标，不给红叉。
pub fn tempo_datapack_good(tempo: f32) -> bool {
    [20.0_f32, 10.0, 5.0, 4.0, 2.5, 2.0, 1.25, 1.0, 0.5, 0.25]
        .iter()
        .any(|v| (tempo - v).abs() < 1e-3)
}

/// 原版 `bundled_data_directory + <name>`（`control_create.gml:754-755`：
/// `changelog.txt` 与 `credits.txt`）。搜索顺序跟 `icons::asset_root` 一致：
/// 先可执行文件旁逐级往上，再工作目录，不把编译期的绝对路径写进二进制。
/// 找到后只把 CRLF 归一成 LF——原版逐行 `draw_text` 时那个 CR 是不显示的，
/// egui 却会把它当成一个字形画出来。
fn load_bundled_text(name: &str) -> Option<String> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for up in ["", "..", "../..", "../../.."] {
                dirs.push(dir.join(up).join("assets"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for up in ["", "..", "../.."] {
            dirs.push(cwd.join(up).join("assets"));
        }
    }
    let path = dirs
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())?;
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.replace("\r\n", "\n"))
}

impl EditorState {
    pub fn new() -> Self {
        let mut prefs = Preferences::load();
        // 演示曲的说明是「生成那一刻」定型的文案，按 prefs 的语言显式传进去；
        // 这里不翻全局开关——new() 会加载采样、开音频设备，占着全局会把并行跑的
        // 单测串台（其余状态串都是绘制时现翻的）。
        let song = nbs_core::model::Song::demo_in(prefs.language);
        let bank = SoundBank::load_auto(prefs.sounds_dir_path().as_deref());
        // 端口自加（用户要求 #94）：老版本首选项存的是设备名，这里认一次并换成系统给的
        // 稳定 ID（改名、重插、换驱动都还认得同一台）；那台已经不在了就清空、跟着系统
        // 默认走，稍后在状态栏说一声。这一趟枚举顺带当作下拉的初始列表。
        let devices = nbs_audio::list_output_devices();
        let stored_output = prefs.audio_output.clone();
        let output_gone = migrate_output_pref(&mut prefs, &devices);
        if prefs.audio_output != stored_output {
            let _ = prefs.save();
        }
        // 上次点名的输出设备直接交给引擎当起始设备。引擎那一趟拿不到就自己回落系统
        // 默认，结果只进设备那一格、不进状态栏（那会儿窗口还没画起来）；
        // 「上次那台不在了」这一句是上面那趟迁移说的，见 new() 结尾。
        let mut audio: Box<dyn AudioEngine> = Box::new(CpalEngine::with_bank_output(
            bank,
            Some(prefs.audio_output.clone()),
        ));
        audio.prepare(&song);
        apply_audio_prefs(&mut *audio, &prefs);

        let mut theme = Theme::new(match prefs.theme {
            0 => ThemeKind::Aqua,
            1 => ThemeKind::Retro,
            2 => ThemeKind::Dark,
            _ => ThemeKind::Fluent,
        });
        // 原版 `fdark` 只对 theme 3 生效（`control_create.gml:614` 在切到 theme 2 时也会置 1）。
        theme.fdark = prefs.fdark && theme.kind == ThemeKind::Fluent;
        // 原版 load_settings 读回 accent1/2/3 后走 draw_accent_init 推 9 档表。
        theme.set_accent_base(prefs.accent);
        let mut flags = UiFlags::default();
        flags.show_layers = prefs.show_layers;
        flags.show_piano = prefs.show_piano;
        flags.use_colors = prefs.use_colors;
        flags.use_shapes = prefs.use_shapes;
        flags.use_icons = prefs.use_icons;
        flags.show_numbers = prefs.show_numbers;
        flags.show_octaves = prefs.show_octaves;
        flags.show_keynames = prefs.show_keynames;
        flags.show_keynumbers = prefs.show_keynumbers;
        flags.show_keyboard = prefs.show_keyboard;
        flags.show_outofrange = prefs.show_outofrange;
        flags.show_incompatible = prefs.show_incompatible;
        flags.fade = prefs.fade;
        flags.show_grid_lines = prefs.show_grid_lines;
        flags.flats = prefs.flats;
        flags.blackout = prefs.blackout;
        flags.remove_effect = prefs.remove_effect;
        flags.show_notechart = prefs.show_notechart;

        let viewport = Viewport::default();
        let recent_files = prefs
            .recent_files
            .iter()
            .map(|r| RecentPath {
                path: PathBuf::from(&r.path),
                time: r.time,
            })
            .collect();
        let follow_marker = prefs.follow_marker;
        let follow_mode = prefs.follow_mode;
        let png_dark = theme.kind.is_dark();
        // 原版 `load_settings` 的 `[schematic_export]` 段。
        let schematic_options = prefs.schematic.clone();
        // 端口自加：MAESTRO 导出选项（`[maestro]`），出厂 round。
        let maestro_options = prefs.maestro.clone();
        // 端口自加：fractal 投影导出选项（`[fractal]`），出厂 Linear + 奇偶分道。
        let fractal_options = prefs.fractal;
        // 端口自加：编码唱片数据箱的导出选项，出厂照上游 `defaultConfig`（20 t/s、resample、层高 8）。
        let disc_chest_options = prefs.disc_chest;
        // 原版 `load_settings` 的 `[midi_import]` 段：14 个 `w_midi_*` 标量 +
        // 「记住我的更改」写回的 GM 表。通道/鼓映射不在这里恢复，那是
        // `open_midi` 每次解析完按 GM 表现算的（我们的 `MidiImportOptions::for_midi`）。
        let mut midi_options = MidiImportOptions::default();
        if let Some(p) = prefs.midi_import.as_ref() {
            p.apply(&mut midi_options);
        }
        let midi_gm = prefs
            .midi_gm
            .as_ref()
            .map(nbs_io::MidiGmTable::from_persist)
            .unwrap_or_default();

        // 刀 0：文档住进标签槽位表，台前再取一份出来。`state.doc` 于是是「台前镜像」，
        // 缓存身份 `song_session` 由槽位表发号（原版每首曲子是一个独立 obj_song 实例）。
        let mut store = crate::tabs::SongStore::new();
        let first_tab = store.insert(Document::with_song(song));
        let doc = store.lift(first_tab).expect("刚入槽的标签一定取得到文档");
        let song_session = store.active_session();

        let mut state = Self {
            doc,
            store,
            song_backup_path: None,
            viewport,
            canvas_hires: false,
            hide_note_text: false,
            interaction: Interaction::default(),
            tool: EditMode::Key,
            current: Note::default(),
            flags,
            theme,
            fullscreen: false,
            sb_sel: 0,
            player: false,
            window: None,
            window_prev: None,
            dialog_return: None,
            context_menu: None,
            message: None,
            audio,
            prefs,
            fonts_installed: false,
            marker_follow: follow_marker,
            follow_mode,
            section_exists: false,
            section_start: 0,
            section_end: 0,
            stop_at_pushed: -1.0,
            hover_cell: None,
            tapper: TempoTapper::default(),
            // 原版这个全局没有初值（只有 menu_click.gml:47 在开窗前播种）。
            // 端口需要一个 Default，取程序自身的版本；从菜单进来必然被覆盖，看不到。
            save_options_version: nbs_core::nbs::NBS_VERSION,
            pending: Vec::new(),
            file_dialog: None,
            picker: None,
            control: None,
            control_change: None,
            control_info: None,
            pending_dialog: None,
            pending_close: None,
            pending_replace: None,
            tab_menu: None,
            tab_drag: None,
            pending_close_set: None,
            quit: None,
            quit_confirmed: false,
            recent_files,
            update_check: std::sync::Arc::new(std::sync::Mutex::new(UpdateCheck::default())),
            update_question: false,
            update_success: false,
            changelog: load_bundled_text("changelog.txt"),
            update_scroll: 0.0,
            credits: load_bundled_text("credits.txt"),
            credits_scroll: 0.0,
            accent_pick: None,
            donate_banner: false,
            editing_layer_name: None,
            event_edit: EventEditDraft::default(),
            layer_drag: None,
            remove_effects: Vec::new(),
            arpeggio_pattern: String::new(),
            stagger_pattern: String::new(),
            tremolotype: 0,
            fade_auto: true,
            leg_dec: 20,
            leg_sus: 20,
            trem_spacing: 1,
            porta_reverse: false,
            port_cent: 0,
            set_vel: 100,
            vel_percent: false,
            set_pan: 0,
            set_pit: 0,
            pitch_additive: false,
            stereo_width: 50,
            stereo_reverse: false,
            midi_enhanced_options: midi_options.clone(),
            midi_options,
            midi_enhanced: nbs_io::midi_enhanced::EnhancedMidiOptions::default(),
            midi_enhanced_check: None,
            transcription: crate::transcription::TranscriptionState::default(),
            midi_data: None,
            midi_file: String::new(),
            midi_tab: 0,
            midi_scroll: [0.0; 4],
            midi_menu: None,
            midi_submenu: None,
            midi_drags: [0.0; 3],
            midi_gm,
            schematic_options,
            maestro_options,
            fractal_options,
            disc_chest_options,
            maestro_report: None,
            maestro_error: None,
            fractal_report: None,
            fractal_error: None,
            disc_chest_report: None,
            disc_chest_error: None,
            schematic_command_plan: None,
            schematic_command_signature: String::new(),
            song_session,
            log_strs: Vec::new(),
            schematic_stats: nbs_io::schematic::SchematicStats::default(),
            schematic_tab: 0,
            schematic_remember: true,
            modals: ModalQueue::default(),
            datapack_options: DatapackOptions::default(),
            datapack_tab: 0,
            mc_tab: 0,
            prefs_tab: 0,
            prefs_theme_menu: false,
            prefs_rate_menu: false,
            prefs_language_menu: false,
            prefs_output_menu: false,
            output_menu: false,
            output_list: devices,
            prefs_drag: 0.0,
            png_options: ImageOptions {
                dark: png_dark,
                ..ImageOptions::default()
            },
            export_status: None,
            piano_start: 12,
            piano_keys_shown: 0,
            piano_scroll_delay: 0.0,
            piano_rect: None,
            last_autosave: Instant::now(),
            last_prefs_save: Instant::now(),
            audio_rev: 0,
            applied_window_icon: None,
            vsync_request: None,
            icons: crate::icons::Icons::default(),
            insbox_open: false,
            menubar_open: false,
            menubar_open_end: false,
            insbox_start: 0,
            scrub: 0,
            record: false,
            record_armed: false,
            keys_held: [false; nbs_core::model::KEY_MAX as usize + 1],
            key_played: crate::piano::NO_KEYS_PLAYED,
            live_midi: crate::live_midi::LiveMidi::new(),
            sound_import: crate::sound_import::SoundImport::default(),
            tempo_drag: None,
            tempo_input: None,
            tempo_press_in: false,
            compat_rev: u64::MAX,
            compat_tempo: f32::NAN,
            compat: 1,
            compat_outside: 0,
            compat_custom: 0,
            rainbow: false,
            rainbow_hue: 0.0,
            background_accent: false,
            wallpaper: None,
            fx: crate::fx::FxState::default(),
        };
        // 特效：存档里留过痕迹（启用列表或参数快照）才把快照灌回各特效，
        // 出厂那两份空表不动，设置文件也就不多写这两段。
        if !state.prefs.fx_active.is_empty() || !state.prefs.fx.is_empty() {
            state.fx.stack.sync_prefs(&mut state.prefs);
        }
        state.enter_startup();
        // 迁移那趟在设备表里认不出的老名字：在状态栏说一句，别让人以为还在自己
        // 点名的那台出声。引擎这时候已经跟着系统默认建好流了。
        if let Some(stale) = output_gone {
            state.set_message(format!(
                "{}{}{}",
                nbs_core::tr("Output device \"", "上次那台输出设备「"),
                stale,
                nbs_core::tr(
                    "\" is no longer available; following the system default instead.",
                    "」已经不在了，已改回跟随系统默认。"
                )
            ));
        }
        state
    }

    /// 切到某个歌曲标签（原版 `set_song.gml`）：台前这份的界面状态收回自己槽位，
    /// 目标标签的那份装上台，播放与交互层按原版归零，音色库重挂。
    /// 标签不存在、或它本来就在台前时返回 false，且什么都不动。
    pub fn switch_tab(&mut self, id: crate::tabs::SongId) -> bool {
        self.enter_tab(id)
    }

    /// 装/收 [`crate::tabs::TabUi`]：台前这一份界面状态 ↔ 标签槽里那一份。
    ///
    /// 两端都写成解构赋值，以后往 [`crate::tabs::TabUi`] 加字段会同时在这两个
    /// 函数编译不过，防止只收不装（用户要求 #131）。
    fn grab_tab_ui(&self) -> crate::tabs::TabUi {
        crate::tabs::TabUi {
            brush: self.current,
            scroll_tick: self.viewport.start_tick,
            scroll_layer: self.viewport.start_layer,
            playhead: self.audio.clock().position(),
            section_exists: self.section_exists,
            section_start: self.section_start,
            section_end: self.section_end,
            insbox_start: self.insbox_start,
            midi_file: self.midi_file.clone(),
        }
    }

    fn apply_tab_ui(&mut self, ui: crate::tabs::TabUi) {
        let crate::tabs::TabUi {
            brush,
            scroll_tick,
            scroll_layer,
            playhead,
            section_exists,
            section_start,
            section_end,
            insbox_start,
            midi_file,
        } = ui;
        self.current = brush;
        self.viewport.start_tick = scroll_tick;
        self.viewport.start_layer = scroll_layer;
        // 原版 `marker_pos` 是每首一份的播放头。走 set_marker 而不是直接写，
        // 顺带把 `metronome_played = -1` 一起复位（set_song.gml:8）。
        self.set_marker(playhead);
        self.section_exists = section_exists;
        self.section_start = section_start;
        self.section_end = section_end;
        self.insbox_start = insbox_start;
        self.midi_file = midi_file;
    }

    /// 原版 `set_song.gml:6-19` 落在播放器/交互层的那几行。
    ///
    /// `playing = 0` 用 [`crate::audio::AudioEngine::stop`]（它就是 `clock.pause()`，
    /// 保留播放头，位置由 [`EditorState::apply_tab_ui`] 另行装回）。
    /// `fade` / `metronome` / `realstereo` 在原版是全局量、每次切标签都归零：
    /// 端口对应 `flags.fade`、`prefs.metronome`、`prefs.mono`，照原样清掉。
    /// 这三个开关落盘的是各自的 `use_fading` / `use_stereo`（`load_settings.gml:45/69`），
    /// 本次归零不会改掉下次开机的默认值；节拍器原版根本不持久化。
    fn clear_tab_transients(&mut self) {
        self.audio.stop();
        self.tempo_drag = None; // tempodrag = 10（出厂值 = 没在拖）
        self.tempo_input = None;
        self.flags.fade = false;
        self.prefs.metronome = false;
        self.prefs.mono = false;
        self.interaction.mode = crate::interaction::DragMode::None; // timeline_pressa = -1
        self.layer_drag = None; // dragvolb = 0
    }

    /// 切标签的完整动作：文档、缓存身份、界面状态、播放/交互归零、音色库重挂。
    /// 台前已经站着它时返回 false；台上空着（启动、刚关空）走 `lift` 那条。
    fn enter_tab(&mut self, id: crate::tabs::SongId) -> bool {
        if self.store.active() == Some(id) {
            return false;
        }
        let outgoing = self.store.active();
        let ui_out = self.grab_tab_ui();
        let moved = match outgoing {
            Some(_) => self.store.swap_active(id, &mut self.doc),
            None => match self.store.lift(id) {
                Some(doc) => {
                    self.doc = doc;
                    true
                }
                None => false,
            },
        };
        if !moved {
            return false;
        }
        if let Some(prev) = outgoing {
            self.store.set_ui(prev, ui_out);
            self.store
                .set_backup_path(prev, self.song_backup_path.clone());
        }
        self.song_backup_path = self.store.backup_path(id).cloned();
        self.song_session = self.store.active_session();
        self.clear_tab_transients();
        self.invalidate_derived_caches();
        let ui_in = self.store.ui(id).cloned().unwrap_or_default();
        self.apply_tab_ui(ui_in);
        self.prepare_audio();
        self.apply_audio_prefs();
        self.sync_loop_params();
        self.sync_section_stop();
        true
    }

    /// 新建一个标签并切过去（原版 `new_song` / `open_song_nbs` / `open_midi` /
    /// `open_schematic` 四条入口都是「建实例压进 `songs[]` + `reset_add` + `set_song」）。
    ///
    /// `reset_add.gml:24-33` 里端口有对应物的那几条一并做掉：关前台窗
    /// （`window = 0`）、收右键菜单（`global.popup = 0`）、钢琴滚动节流
    /// （`delay = 0`）、图层选中项（`sb_sel = 0`）。`text_exists` / `insselect` /
    /// `insedit` / `key_edit` / `editline` 端口没有对应字段（无歌词功能），不复刻。
    pub fn open_tab(&mut self, doc: Document) -> crate::tabs::SongId {
        self.open_tab_at(self.store.len(), doc)
    }

    /// 在原版 `array_insert(songs, index, create(obj_song))` 的位置开标签并切过去。
    pub fn open_tab_at(&mut self, index: usize, doc: Document) -> crate::tabs::SongId {
        let id = self.store.insert_at(index, doc);
        let opened = self.enter_tab(id);
        debug_assert!(opened, "刚入槽的标签一定切得过去");
        self.window = None;
        self.window_prev = None;
        self.context_menu = None;
        self.piano_scroll_delay = 0.0;
        self.sb_sel = 0;
        id
    }

    /// 关掉一个标签：状态搬移（原版 `close_song.gml` 的后半段）。
    ///
    /// 保存确认、备份文件删除都不在这里——那是调用方（`Command::CloseTab`
    /// 与退出流程）的事，对应原版 `close_song` 前半段的 `confirm` 和
    /// `backup_delete_own_tab`。原版关任何一首都要经过两次 `set_song`
    /// （`close_song.gml:15/27`），所以**关后台标签同样停播**、清 fade/metronome/mono；
    /// 台前的文档和它的界面状态则一格不动。
    /// 标签不存在时返回 `was_active: false` 且不动任何状态。
    pub fn close_tab(&mut self, id: crate::tabs::SongId) -> crate::tabs::Closed {
        let out = self.store.close(id, &mut self.doc);
        if !out.was_active {
            self.clear_tab_transients();
            self.apply_audio_prefs();
            return out;
        }
        self.song_session = self.store.active_session();
        self.clear_tab_transients();
        self.invalidate_derived_caches();
        match out.active {
            Some(next) => {
                self.song_backup_path = self.store.backup_path(next).cloned();
                let ui = self.store.ui(next).cloned().unwrap_or_default();
                self.apply_tab_ui(ui);
            }
            // 表关空了：`store.close` 已经把台前换成一首空曲，界面状态跟着回出厂值。
            None => {
                self.song_backup_path = None;
                let ui = crate::tabs::TabUi::default();
                self.apply_tab_ui(ui);
            }
        }
        self.prepare_audio();
        self.apply_audio_prefs();
        self.sync_loop_params();
        self.sync_section_stop();
        out
    }

    /// 最后一个标签关掉后补一首空曲（原版 `close_song.gml:27-29`：表空了就
    /// `new_song()`）。闸门是 `!nonew || noconfirm`：Ctrl+W / 菜单关标签（`close_song(sid)`）
    /// 和退出流程（`close_song(i, 1, 1)`）都成立，只有欢迎窗和启动文件那条
    /// 「替换第 1 个标签」的路（`close_song(0, 1)`）不补——那条路紧接着就载入新歌。
    /// [`crate::tabs::SongStore::close`] 已经换上一首空曲在台前（不在表里，
    /// `active_tab()` 是 `None`），这里把它搬进槽位表、台上再放一首。
    pub fn refill_tab_after_last_close(&mut self) -> Option<crate::tabs::SongId> {
        if !self.store.is_empty() {
            return self.store.active();
        }
        let blank = std::mem::replace(&mut self.doc, Document::new());
        Some(self.open_tab(blank))
    }

    /// 新开一个标签，放在标签条末尾、**不在台前**（原版 `open_song_nbs.gml:50-53`
    /// 就是建实例压进 `songs[]`，切过去是另一步 `set_song`）。
    pub fn push_tab(&mut self, song: nbs_core::model::Song) -> crate::tabs::SongId {
        self.store.insert(Document::with_song(song))
    }

    /// 台前标签（原版 `global.song`）。
    pub fn active_tab(&self) -> Option<crate::tabs::SongId> {
        self.store.active()
    }

    /// 读后台标签的文档（刀 5 的 MCP 跨标签读走这里）；台前那份就是 `self.doc`。
    pub fn parked_doc(&self, id: crate::tabs::SongId) -> Option<&Document> {
        self.store.parked_doc(id)
    }

    /// 台前这一首被整份换掉了（新建 / 载入 / 导入）：换一个新的缓存身份。
    /// 替掉原来散在五处的 `state.song_session += 1`。
    pub fn reissue_session(&mut self) {
        if let Some(session) = self.store.reissue_session() {
            self.song_session = session;
        }
    }

    /// 作废所有按 `doc.song.rev` 缓存的派生态，下一次比较必然落空重算。
    ///
    /// 缓存不随标签走是实测定的：整表重算兼容性只要 0.71 ms（10 万音符），而事件表
    /// 和特效烘焙本来就是「下次播放才建」——切标签即停播，常驻只会把内存乘以标签数
    /// （特效时间轴 + 事件表实测 4.0 + 1.5 MiB/首）。理由与代价见
    /// `.work/songtabs-20261123/正式方案.md` 3.2 / 3.4（用户要求 #131）。
    fn invalidate_derived_caches(&mut self) {
        // u64::MAX 撞不上：`rev` 只会从当前值往上加，加不到 2^64。
        self.audio_rev = u64::MAX;
        self.compat_rev = u64::MAX;
        // NAN 和任何数比都不相等，`refresh_compat` 的速度档同样落空。
        self.compat_tempo = f32::NAN;
        self.schematic_command_signature.clear();
        self.fx.invalidate_bake();
    }

    /// 原版 `control_create.gml` 结尾的开机流程：捐赠横幅按静音到期日重新亮出、
    /// 版本变化走「更新成功」、`show_welcome` 时打开欢迎窗。
    pub fn enter_startup(&mut self) {
        // `date_compare_date(date_current_datetime(), donate_banner_time) > 0`（:728-733）：
        // 当前日期晚于静音到期日才显示。donate_banner_time = 0 等价原版出厂的 −1（没静音过）。
        self.donate_banner = nbs_io::prefs::unix_now() > self.prefs.donate_banner_time;
        // :747-753 的 `vers_tmp != version` 分支：置 update_success、强制亮横幅、
        // 把运行时主题切成 Fluent，并打开 w_update。
        let last = self.prefs.last_version.clone();
        let updated = last.is_some() && last.as_deref() != Some(crate::VERSION);
        if updated {
            self.update_success = true;
            self.donate_banner = true;
            // :748 `if (theme = 2) fdark = 1`：原来存的是深色主题，切过去要带 fdark，
            // 得到的是「深色 Fluent」。prefs.theme 不动——原版要等「确认」里的
            // save_settings 才写回设置文件。
            if self.theme.kind == ThemeKind::Dark {
                self.theme.fdark = true;
            }
            let fdark = self.theme.fdark;
            let base = self.theme.accent_base;
            self.theme = Theme::new(ThemeKind::Fluent);
            self.theme.fdark = fdark;
            self.theme.set_accent_base(base);
        }
        // :707 的 `if (show_welcome) window = w_greeting`。
        if self.prefs.show_welcome {
            self.window = Some(WindowKind::Greeting);
        }
        // :750 的 `window = w_update` 排在 :707 之后，欢迎窗会被顶掉。
        if updated {
            self.window = Some(WindowKind::Update);
        }
    }

    /// 原版 `check_updates.gml` 回调里「有新版本 → question(下载？)」的那一步：
    /// 后台线程把结果写进共享结构，UI 线程在帧首轮询，问一次就不再问。
    /// 答「是」执行 `Command::OpenUrl`——原版此处直接 `http_get_file` 下载安装包，
    /// 我们改成打开 releases 页（已记录的偏差）。
    pub fn poll_update_check(&mut self) {
        let (status, ver) = {
            let g = self.update_check.lock().unwrap();
            (g.status, g.new_version.clone())
        };
        if status == 1 && !self.update_question {
            self.update_question = true;
            self.modals = ModalQueue::question(
                crate::tr("Update available!", "更新可用！"),
                &format!(
                    "{}",
                    crate::tr(
                        &format!("Version {ver} is available! Do you want to download it?"),
                        &format!("版本 {ver} 可用！是否现在下载？"),
                    )
                ),
                Command::OpenUrl(crate::RELEASES_PAGE.to_string()),
            );
        }
    }

    pub fn emit(&mut self, cmd: Command) {
        self.pending.push(cmd);
    }

    /// 要一个文件路径。装了原生选择器时把请求挂到帧外去执行（原版的选框就是系统模态的，
    /// 期间主窗口不重绘）；没装则打开自绘的 `WindowKind::FileOpen` / `FileSave`。
    /// 两条路都会把 `file_dialog` 留下，因为 `Command::DialogConfirm` 靠它认操作类型。
    pub fn request_file_dialog(&mut self, dialog: FileDialog) {
        self.dialog_return = None;
        if self.picker.is_some() {
            self.pending_dialog = Some(dialog.clone());
        } else {
            let save = dialog.save;
            self.dialog_return = self.window.clone();
            self.window = Some(if save {
                WindowKind::FileSave
            } else {
                WindowKind::FileOpen
            });
        }
        self.file_dialog = Some(dialog);
        self.context_menu = None;
    }

    /// 执行挂起的原生选择请求。宿主在帧外（`raw_input_hook`）调，不要在 `update` 里调：
    /// 原生对话框会泵自己的消息循环，塞在 egui 帧中间不安全。
    /// 返回 true 表示真的处理了一个请求（调用方据此催一次重绘）。
    /// COM 起不来时回落到自绘选框，功能不断。
    pub fn run_pending_dialog(&mut self) -> bool {
        let Some(dialog) = self.pending_dialog.take() else {
            return false;
        };
        let Some(picker) = self.picker.clone() else {
            return false;
        };
        match picker.pick(&dialog) {
            PickResult::Picked(path) => {
                // 原版选文件夹取的是 `filename_dir(a)`（`draw_window_preferences.gml:279`）：
                // 对话框里那个占位名不算数，要的只是它所在的目录。系统组件交回来的是完整
                // 路径，这里先换算成同一个目录，`DialogConfirm` 两条路看到的形状才一致。
                let path = match dialog.action {
                    DialogAction::PickFolder(_) => {
                        path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
                    }
                    // 目录选择器交回来的本身就是目录（原版 get_directory 语义）。
                    DialogAction::PickMinecraftDir => path,
                    _ => path,
                };
                // `Command::DialogConfirm` 靠 `file_dialog.action` 认是哪一种导出。
                self.file_dialog = Some(dialog);
                self.emit(Command::DialogConfirm(path));
            }
            PickResult::Cancelled => {
                self.file_dialog = None;
                // 关标签正等在这个框上时，取消 = 原版 `save_song` 返回 0 = `confirm`
                // 返回 -1：这一首不关，切回原来那首（用户要求 #131）。
                // 只有 `Command::CloseTab` 会置挂起记录，别的请求取消时这条是空操作。
                self.emit(Command::DialogCancelled);
            }
            PickResult::Unavailable => {
                // 自绘兜底框会把原窗口顶掉，记住顶掉的是哪个，选完放回去。
                self.dialog_return = self.window.clone();
                self.window = Some(if dialog.save {
                    WindowKind::FileSave
                } else {
                    WindowKind::FileOpen
                });
                self.file_dialog = Some(dialog);
            }
        }
        true
    }

    /// 原版 `draw_windows.gml:31-38`：Escape 先把展开的下拉收掉，并且把
    /// `window_escape_pressed` 清成 0 —— 同一次按键不许既收下拉又关窗。
    /// 返回这一次有没有落到下拉上。
    ///
    /// 顶部菜单栏是 egui 自己的 popup（egui 在 `menu.rs:366` 就地把 Escape 吃掉了），
    /// 这里只负责「看见它开着」；首选项里那四个下拉是端口自绘的 Area，
    /// 状态记在开关上，得由这里关掉。MIDI 窗的两个菜单归 #122b：那个窗口
    /// 至今没有任何 Escape 路径，先不凭空加行为。
    pub fn close_prefs_dropdowns(&mut self, ctx: &Context) -> bool {
        let egui_popup = ctx.memory(|m| m.any_popup_open());
        // 按位或：四个开关都要被清一遍，不能被短路跳过。
        let mine = std::mem::take(&mut self.prefs_theme_menu)
            | std::mem::take(&mut self.prefs_rate_menu)
            | std::mem::take(&mut self.prefs_language_menu)
            | std::mem::take(&mut self.prefs_output_menu);
        egui_popup | mine
    }

    pub fn set_message(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    pub fn active_message(&self) -> Option<&str> {
        self.message
            .as_ref()
            .filter(|(_, t)| t.elapsed().as_secs_f64() < 4.0)
            .map(|(s, _)| s.as_str())
    }

    /// 处理本帧累积的命令。
    pub fn flush(&mut self, ctx: &Context) {
        let cmds = std::mem::take(&mut self.pending);
        for cmd in cmds {
            // 原版 `message()` / `question()` 会冻结整个游戏循环：模态队列没答完之前，
            // 背景界面产生的命令一律丢弃，只接受回答当前模态框（是 / 否 / 取消）。
            if !self.modals.is_empty()
                && !matches!(
                    cmd,
                    crate::commands::Command::AnswerModal(_)
                        | crate::commands::Command::CancelModal
                )
            {
                continue;
            }
            crate::commands::dispatch(self, cmd, ctx);
        }
        self.sync_audio_events();
    }

    /// 歌曲内容变化后把事件表同步给音频引擎。空闲帧只做一次整数比较，
    /// 因此边播放边画音符能立即发声，而不会每帧重建上万条事件。
    pub fn sync_audio_events(&mut self) {
        let rev = self.doc.song.rev;
        if rev == self.audio_rev {
            return;
        }
        self.audio_rev = rev;
        let include_locked = self.prefs.include_locked_in_export;
        let clock = self.audio.clock_mut();
        clock.set_include_locked(include_locked, &self.doc.song);
    }

    /// 把循环元数据推给播放时钟：开关、起点、终点（= 歌曲长度，原版用
    /// 结尾静音音符表达）与循环次数。改完 meta 必须显式调用一次。
    pub fn sync_loop_params(&mut self) {
        let s = &self.doc.song;
        let end = s.song_length().max(1) as f64;
        self.audio.clock_mut().set_loop(
            s.meta.loop_enabled,
            s.meta.loop_start as f64,
            end,
            s.meta.loop_max,
        );
    }

    /// 缩放语义按用户要求改过：原版 `control_draw.gml:11-12` 的 `window_scale`
    /// 连 chrome 一起缩（GM 画面整体放大），端口只缩画布 —— 倍率落到
    /// `viewport.scale`（每格 `32 × scale` px），egui 的 zoom_factor 不再被动，
    /// 菜单 / 工具栏 / 状态栏 / 钢琴栏保持系统原生尺寸。
    fn apply_ui_scale(&mut self, ctx: &Context) {
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let effective = self.prefs.effective_scale(native);
        self.viewport.scale = effective / native;
        self.canvas_hires = effective > 1.25;
        self.hide_note_text = self.prefs.hides_note_text(effective);
    }

    /// 原版 `control_draw.gml:736-751`：Ctrl+滚轮 / Ctrl+± 把 `window_scale` 推到
    /// 下一个（上一个）固定档位，档位表就是那串 `else if` 里的数，两端是 0.5 和 4。
    /// 网格本身恒 32px，缩放的是整块界面。
    /// 偏离原版：用户要求下界放开到 10%，低档 0.1–0.4 是自加的（原版止步 0.5）。
    /// 原版 `control_draw.gml:684-704`：翻的是 `sb_sel` 那一根，夹在 `0..=enda`
    /// （纵向 `0..=endb`）之间，播放头不动。
    pub fn scroll_axis(&mut self, op: AxisScroll) {
        let vertical = self.sb_sel == 1;
        // 上限取 enda / endb：和那两根条自己的上限（bar_metrics 的 tail）是同一个数，
        // 原版也是夹到这两个值（median(0, starta, enda)）。
        let (total, max) = if vertical {
            (self.viewport.rows() as i64, self.doc.song.endb as i64)
        } else {
            (self.viewport.columns() as i64, self.doc.song.enda as i64)
        };
        let cur = if vertical {
            self.viewport.start_layer as i64
        } else {
            self.viewport.start_tick as i64
        };
        let next = match op {
            AxisScroll::Home => 0,
            AxisScroll::End => max,
            // 原版 PageUp 是 `starta += totalcols`（往曲尾走），PageDown 反向。
            AxisScroll::Page(d) => (cur + i64::from(d) * total).clamp(0, max),
        } as u32;
        if vertical {
            self.viewport.start_layer = next;
        } else {
            self.viewport.start_tick = next;
        }
    }

    /// 原版 F7（`control_draw.gml:840-867`）：档位 +1 取模循环，每档回显一句
    /// 「帧数上限 => N FPS」，且只改运行值、不写 settings.ini —— 落盘留给首选项「确定」，
    /// `f7_cycles_the_frame_cap_and_reports_it` 就是钉这一条的。
    /// 循环走 `refresh_rate_order` 的升序而不是存储下标（#126 偏离）：165 只能追加在表尾，
    /// 按下标循环会变成「240 → 165」，和下拉里看到的上下顺序对不上。
    pub fn cycle_refresh_rate(&mut self) {
        let order = nbs_io::prefs::refresh_rate_order();
        let pos = nbs_io::prefs::refresh_rate_pos(self.prefs.refresh_rate);
        let next = order[(pos + 1) % order.len()];
        self.prefs.refresh_rate = next as u8;
        self.set_message(format!(
            "{} => {} FPS",
            tr("Max framerate", "帧数上限"),
            REFRESH_RATES[next]
        ));
    }

    pub fn step_window_scale(&mut self, ctx: &Context, up: bool) {
        let cur = self
            .prefs
            .effective_scale(ctx.native_pixels_per_point().unwrap_or(1.0));
        let Some(next) = step_in_ladder(cur, up) else {
            return;
        };
        self.prefs.window_scale = Some(next);
        // 原版 `control_draw.gml:666`：`condstr(language = 1, "窗口缩放", "Window scale") + " => " + … + "%"`。
        self.set_message(format!(
            "{} => {}%",
            crate::tr("Window scale", "窗口缩放"),
            (next * 100.0).round()
        ));
    }

    /// 端口自加（用户要求）：琴键面板独立缩放，与 `window_scale` 各走各的档位表。
    pub fn step_piano_scale(&mut self, _ctx: &Context, up: bool) {
        let next = step_in_ladder(self.prefs.piano_scale, up);
        let Some(next) = next else { return };
        self.prefs.piano_scale = next;
        self.set_message(format!(
            "{} => {}%",
            crate::tr("Piano scale", "琴键缩放"),
            (next * 100.0).round()
        ));
    }

    /// 原版 `control_draw.gml:732-735`：Ctrl+0 回到 `get_default_window_scale()`（= 系统 DPI / 96）。
    pub fn reset_window_scale(&mut self, ctx: &Context) {
        self.prefs.window_scale = None;
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        self.set_message(format!(
            "{} => {}%",
            crate::tr("Window scale", "窗口缩放"),
            (native * 100.0).round()
        ));
    }

    /// 原版 `control_draw.gml:27-31` + `control_step.gml:21-29`：`window_icon` 一变就重设窗口图标。
    /// 原版为此留了 1 秒的重发窗口（`icon_display`），因为 GM 的 `window_set_icon_raw`
    /// 不保证第一次就生效；egui 的窗口命令是排队给平台层的，一次就够，所以只在档位变化时发。
    /// 图标文件读不到时不发命令，窗口保持系统默认 —— 而不是换一张凑数。
    fn apply_window_icon(&mut self, ctx: &Context) {
        let want = self.prefs.window_icon;
        if self.applied_window_icon == Some(want) {
            return;
        }
        let Some(icon) = self.icons.window_icon(want) else {
            return;
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(icon)));
        self.applied_window_icon = Some(want);
    }

    /// 原版 Ctrl+Shift+F5（仅 theme 3，`control_draw.gml:710-740`）：手动切换彩虹。
    /// 与 Toggle Rainbow 事件乐器的差异：这里要发消息，且只在关掉时重置表
    /// （原版 `rainbowtoggle = !rainbowtoggle` → 开：只 set_msg，关：set_msg + draw_accent_init）。
    pub fn toggle_rainbow(&mut self) {
        self.rainbow = !self.rainbow;
        if self.rainbow {
            self.set_message(format!("{} => ON", crate::tr("Rainbow mode", "炫彩模式")));
        } else {
            self.set_message(format!("{} => OFF", crate::tr("Rainbow mode", "炫彩模式")));
            self.theme.accent_init();
        }
    }

    /// 原版删除效果的生成（`remove_block_manual.gml:64-68`、`selection_delete.gml:24-28`）：
    /// theme 3 且「显示删除效果」开着才生成，批量删除只生成当前视口内的格子。
    /// rel 记的是生成那一刻相对视口左上格的格坐标 —— 原版对象不跟滚动，
    /// 动画期间滚动页面时边框会留在屏幕原地，这里照抄。
    pub fn spawn_remove_effects(&mut self, cells: impl IntoIterator<Item = (i64, i64)>) {
        if self.theme.kind != ThemeKind::Fluent || !self.flags.remove_effect {
            return;
        }
        let (t0, t1, l0, l1) = self.viewport.visible_range();
        let (t0, t1, l0, l1) = (t0 as i64, t1 as i64, l0 as i64, l1 as i64);
        for (tick, layer) in cells {
            if tick >= t0 && tick < t1 && layer >= l0 && layer < l1 {
                self.remove_effects.push(RemoveFx {
                    rel: (tick - t0, layer - l0),
                    age: 0.0,
                });
            }
        }
    }

    /// 每帧同步独立播放时钟。真实播放时位置由音频回调的采样计数推进，
    /// UI 只读；无音频设备时才由内部 `Instant` 时钟代理。
    /// **绝不在这里写 tempo/loop** —— 否则会覆盖 Tempo Changer 的运行时变速。
    pub fn tick_transport(&mut self, ctx: &Context) {
        self.apply_ui_scale(ctx);
        self.apply_window_icon(ctx);
        // 端口自加（用户要求 #94）：输出流出事（拔设备、换驱动、默认被禁用）时 cpal 会在
        // 音频线程上记一笔，这里在界面线程取走并当场重建一条，顺带把换到的设备报给状态栏。
        // 有流又没出事时这一趟只是读一个标志位，不碰系统（设备表也只在打开下拉时才枚举）。
        self.poll_output_report();
        // 原版 else-if 链里「改界面」的事件乐器副作用（control_step.gml:283-296）
        // 由音频调度发回来，这里统一落地。彩虹开/关都重置表且不发消息，照抄原版。
        let mut accent_changed = false;
        // 原版在派发那一列的当下盖 `current_time`；这里是 UI 排空时才盖，最多差一帧。
        let now = ctx.input(|i| i.time);
        for ev in self.audio.take_visual_events() {
            match ev {
                nbs_audio::VisualEvent::KeyPlayed(key) => {
                    if let Some(slot) = self.key_played.get_mut(key as usize) {
                        *slot = now;
                    }
                }
                nbs_audio::VisualEvent::RainbowToggle => {
                    self.rainbow = !self.rainbow;
                    self.theme.accent_init();
                    accent_changed = true;
                }
                nbs_audio::VisualEvent::SavePopup => self.set_message("Song saved"),
                nbs_audio::VisualEvent::AccentColor(rgb) => {
                    // 原版 draw_set_accent：只重算 9 档表，不写基色、不落盘。
                    self.theme.set_accent_table(rgb);
                    accent_changed = true;
                }
                nbs_audio::VisualEvent::BackgroundAccentToggle => {
                    self.background_accent = !self.background_accent;
                }
            }
        }
        // 原版 `marker_end` 的停点每帧从区间现算（`control_draw.gml:1033` 当场读
        // `section_*`），推给时钟要在推进时钟之前。
        self.sync_section_stop();
        let _crossed = self.audio.clock_mut().update();
        let playing = self.audio.clock().is_playing();
        // 原版 `if (playing = 0) record = 0`：播放一停，录音自动退出。
        // 0.25（按下录音、等第一个键起播）不算「在播」，所以 armed 时不清。
        if playing {
            self.record_armed = false;
        } else if !self.record_armed {
            self.record = false;
        }
        let position = self.audio.clock().position();
        // 原版 `control_draw.gml:1037`：`marker_follow = 1 || isplayer`，播放器强制跟随。
        if playing && (self.marker_follow || self.player) {
            self.viewport
                .follow_marker(position, self.follow_mode, self.scrub < 0);
        }
        if playing {
            // 原版 `game_set_speed(N, gamespeed_fps)`：帧数上限管的是整个程序的步进，
            // 播放画面就是靠它推进的。这里对应改成定时重绘，而不是每帧立即重绘。
            let frame = std::time::Duration::from_secs_f64(1.0 / self.prefs.refresh_limit());
            ctx.request_repaint_after(frame);
        }
        // 原版 `control_draw.gml:2585`：彩虹开着就每帧 draw_accent_rainbow()。
        // hue 每帧走 `4°×(30/room_speed)×(1/currspeed)`，currspeed = targetspeed/delta_time
        // 做帧率补偿，净效果恒为 120°/秒；回绕判定里的 `- refreshrate` 是原稿手误
        // （最多 4° 相位差，肉眼不可见），不复刻。
        if self.rainbow {
            let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) as f32;
            let advanced = self.rainbow_hue + 120.0 * dt;
            self.rainbow_hue = if advanced < 360.0 { advanced } else { 1.0 };
            self.theme
                .set_accent_table(Theme::hsb_to_rgb(self.rainbow_hue, 100.0, 100.0));
            accent_changed = true;
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                1.0 / self.prefs.refresh_limit(),
            ));
        }
        if accent_changed {
            // egui 原生控件（菜单 / 按钮 / 选中色）读的是 apply 时的快照，
            // accent 表每帧在变时必须跟着刷新，否则只有自绘面板在变色。
            self.theme.apply(ctx);
        }
        // 原版 `obj_removeeff` 每帧 alpha -= 0.06*(30/(room_speed*currspeed))，
        // 折算成真实时间衰减恒为 1.8/秒（寿命 1/3.6 秒），与播放速度无关。
        if !self.remove_effects.is_empty() {
            let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) as f32;
            for fx in &mut self.remove_effects {
                fx.age += dt;
            }
            self.remove_effects.retain(|fx| fx.alpha() > 0.0);
            if !self.remove_effects.is_empty() {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    1.0 / self.prefs.refresh_limit(),
                ));
            }
        }
        // 原版快退 / 快进：按住时按歌曲速度滚动；播放中倒退是双倍速
        // （`forward - (forward < 0 && playing = 1)`）。
        if self.scrub != 0 {
            let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) as f64;
            let factor = if self.scrub < 0 && playing { 2.0 } else { 1.0 };
            let delta = self.audio.clock().tempo() as f64 * dt * factor * self.scrub as f64;
            let pos = (self.audio.clock().position() + delta).max(0.0);
            self.audio.clock_mut().seek(pos);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                1.0 / self.prefs.refresh_limit(),
            ));
        }
        // 定时自动保存（只写备份，不改动当前文件名与脏标记）。
        // 播放器档位是只读的（原版备份链在 save_song/backup_save 里被 isplayer 挡住）。
        let mins = self.prefs.autosave_interval() as u64;
        if !self.player
            && mins > 0
            && self.doc.song.dirty
            && self.last_autosave.elapsed().as_secs() >= mins * 60
        {
            self.last_autosave = Instant::now();
            if let Ok(p) = nbs_io::prefs::autosave(&self.doc.song, &self.prefs) {
                self.song_backup_path = Some(p.clone());
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.set_message(if crate::lang() == 1 {
                    format!("已自动备份 {name}")
                } else {
                    format!("Autosaved {name}")
                });
            }
        }
    }

    pub fn marker(&self) -> f64 {
        self.audio.clock().position()
    }

    /// 原版 `compatible`（0 = 仅资源包，1 = 完全兼容，2 = 仅数据包）。
    /// 判定条件与原版逐字一致：有选择时不刷新，保留上一次的值。
    pub fn compatibility(&mut self) -> u8 {
        self.refresh_compat();
        self.compat
    }

    /// 原版兼容性窗口那两条「有 N 个方块…」用的 `block_outside` / `block_custom`。
    /// 原版这两个数是 `add_block*` 一路累加、`calculate_size` 归零重算的，
    /// 这里和 `compatibility` 共用同一份缓存，刷新条件也一致。
    pub fn mc_stats(&mut self) -> (usize, usize) {
        self.refresh_compat();
        (self.compat_outside, self.compat_custom)
    }

    /// 速度框上此刻显示的那个数：没在拖就是歌曲自己的速度。原版在拖动过程中就把量化值
    /// 写回 `real_tempo`（`control_draw.gml:2534`），所以框上的字和右上角徽标是同步动的；
    /// 端口把拖动值暂存在 `tempo_drag`、松手才提交，判定这一侧也必须拿它，否则徽标会等松手。
    pub fn shown_tempo(&self) -> f32 {
        self.tempo_drag
            .map(|raw| (raw * 4.0).floor() / 4.0)
            .unwrap_or(self.doc.song.meta.tempo.max(0.25))
    }

    fn refresh_compat(&mut self) {
        let tempo = self.shown_tempo();
        if !self.doc.selection.notes.is_empty()
            || (self.doc.song.rev == self.compat_rev
                && (tempo - self.compat_tempo).abs() < f32::EPSILON)
        {
            return;
        }
        self.compat_rev = self.doc.song.rev;
        self.compat_tempo = tempo;
        use nbs_core::model::{MC_KEY_MAX, MC_KEY_MIN};
        let (mut outside, mut custom, mut pitched) = (false, false, false);
        let (mut n_outside, mut n_custom) = (0usize, 0usize);
        for (_, _, n) in self.doc.song.iter_notes() {
            if n.key < MC_KEY_MIN || n.key > MC_KEY_MAX {
                outside = true;
                n_outside += 1;
            }
            if n.instrument >= nbs_core::model::FIRST_CUSTOM_INDEX {
                custom = true;
                n_custom += 1;
            }
            pitched |= n.pitch != 0;
        }
        self.compat_outside = n_outside;
        self.compat_custom = n_custom;
        self.compat = if outside || custom {
            0
        } else {
            if tempo_schematic_ok(tempo) && !pitched {
                1
            } else {
                2
            }
        };
    }

    pub fn set_marker(&mut self, tick: f64) {
        self.audio.clock_mut().seek(tick);
    }

    /// 原版 `marker_end` 认的那道闸（`control_draw.gml:1033`）：开关开着、区间存在、
    /// 终点确实在起点之后，才有停点。
    pub fn section_stop_tick(&self) -> Option<i64> {
        (self.prefs.marker_end && self.section_exists && self.section_end > self.section_start)
            .then_some(self.section_end)
    }

    /// 把 `marker_end` 的停点推给播放时钟。原版每帧现算，这里只在值变了时才推。
    pub fn sync_section_stop(&mut self) {
        let stop = self.section_stop_tick().map_or(-1.0, |t| t as f64);
        if stop != self.stop_at_pushed {
            self.stop_at_pushed = stop;
            self.audio.clock_mut().set_stop_at(stop);
        }
    }

    /// 换歌 / 新建歌曲：原版每首歌自带一组 `section_exists = 0`、
    /// `section_start = section_end = 0`（`obj_song/Create_0.gml:22-24`）。
    pub fn clear_section(&mut self) {
        self.section_exists = false;
        self.section_start = 0;
        self.section_end = 0;
    }

    pub fn is_playing(&self) -> bool {
        self.audio.clock().is_playing()
    }

    pub fn prepare_audio(&mut self) {
        self.audio.prepare(&self.doc.song);
        self.audio_rev = self.doc.song.rev;
    }

    /// 原版 `toggle_playing` 的起播语义：上一轮播完停在歌尾（非循环的停点 =
    /// 歌长 + [`nbs_audio::scheduler::END_STOP_MARGIN_TICKS`]）时先回到 0，
    /// 再按 `marker_start` 跳到区间起点（`:9`，只认「区间存在且终点在起点之后」），
    /// 最后按落点列起播（取落点速度并整列派发，见 [`AudioEngine::start`]）。
    /// 「播放 / 暂停」与录音待命首键两条起播路径都必须走这里。
    pub fn begin_playback(&mut self) {
        let end_stop =
            self.doc.song.song_length() as f64 + nbs_audio::scheduler::END_STOP_MARGIN_TICKS;
        if self.audio.clock().position() >= end_stop {
            self.audio.clock_mut().seek(0.0);
        }
        if self.prefs.marker_start && self.section_exists && self.section_end > self.section_start {
            self.audio
                .clock_mut()
                .seek(self.section_start.max(0) as f64);
        }
        self.sync_section_stop();
        let pos = self.audio.clock().position();
        self.audio.start(pos);
    }

    /// 原版 `instrument_list`：内置乐器（v6 起 20 个）+ 当前歌曲的自定义乐器，索引即音符里的
    /// instrument 值。自定义乐器没填名字时回落到「自定义音色 N」。
    pub fn instrument_list(&self) -> Vec<(u8, String)> {
        let mut rows: Vec<(u8, String)> = nbs_core::instruments::INSTRUMENTS
            .iter()
            .enumerate()
            .map(|(i, ins)| (i as u8, ins.name.to_string()))
            .collect();
        for (j, ci) in self.doc.song.custom_instruments.iter().enumerate() {
            let idx = nbs_core::model::FIRST_CUSTOM_INDEX + j as u8;
            let name = if ci.name.trim().is_empty() {
                tr(
                    &format!("Custom instrument {}", j + 1),
                    &format!("自定义音色 {}", j + 1),
                )
                .to_string()
            } else {
                ci.name.clone()
            };
            rows.push((idx, name));
        }
        rows
    }

    /// 原版 `control_draw.gml:256-263`：滚轮换音色 / 换键的试听声，在
    /// `layerhov_vppreview` 打开、鼠标确实悬在一条已有层上（`selby <= endb`）时
    /// 用该层的音量和声道；此时 `selected_vel = selected_pan = 100`，所以原式的
    /// `(layervol / 100) * 100` 与 `(layerstereo + 100) / 2` 就是层自身的两个值。
    /// 其余情况回落到中性 100/100。
    pub fn wheel_preview_vp(&self) -> (u8, u8) {
        let Some((_, layer)) = self.hover_cell else {
            return (100, 100);
        };
        if !self.prefs.layerhov_vppreview || layer > self.doc.song.endb {
            return (100, 100);
        }
        match self.doc.song.layers.get(layer as usize) {
            Some(l) => (l.volume, l.stereo),
            None => (100, 100),
        }
    }

    pub fn add_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p.path != path);
        self.recent_files.insert(
            0,
            RecentPath {
                path: path.clone(),
                time: nbs_io::prefs::unix_now(),
            },
        );
        self.recent_files.truncate(11);
        self.prefs.add_recent(&path);
    }

    /// 原版 draw_window_greeting.gml:219-231：点开最近歌曲时文件不在了，
    /// 弹「找不到文件」并把同一路径的条目整串移走（含重复项）。
    /// 落盘跟着 SavePreferences 走（sync_prefs 会把 recent_files 一起写）。
    pub fn remove_recent(&mut self, path: &Path) {
        self.recent_files.retain(|p| p.path != path);
    }

    /// 把偏好推给音频引擎（主音量、速度、节拍器、voice 上限、单声道、锁定层）。
    /// 端口自加（用户要求 #94）：列一遍可选的输出设备。cpal 0.18 在本机实测这一趟
    /// 2.6~4.0 毫秒（0.15 要 1.3 秒，当年得挪去后台线程），打开下拉时当场拿就行。
    pub fn refresh_outputs(&mut self) {
        self.output_list = self.audio.output_devices();
    }

    /// 「输出」下拉的条目：第 0 项是跟随系统默认，其余按枚举顺序列出设备名。
    /// 设备名由系统给（Windows 会按系统语言给「耳机 (Realtek(R) Audio)」这类名字），
    /// 不由我们翻译；只有后缀文案翻。
    pub fn output_items(&self) -> Vec<String> {
        let mut items = vec![nbs_core::tr("Follow system default", "跟随系统默认").to_string()];
        items.extend(self.output_list.iter().map(|d| {
            if d.is_default {
                format!(
                    "{} {}",
                    d.name,
                    nbs_core::tr("(system default)", "（系统默认）")
                )
            } else {
                d.name.clone()
            }
        }));
        items
    }

    /// 一台设备在首选项里应当怎么被记下来：优先用系统给的稳定 ID（改名、重插、
    /// 换驱动都还认得同一台），后端没给 ID 时退回名字。
    fn output_key(device: &nbs_audio::OutputDevice) -> String {
        if device.id.is_empty() {
            device.name.clone()
        } else {
            device.id.clone()
        }
    }

    /// 把首选项里那个标识（稳定 ID，或没迁移前的老设备名）翻回给人看的设备名。
    /// 拿手边的设备表翻，不额外枚举（这一格每帧都要画）。
    fn output_display_name(&self, id_or_name: &str) -> String {
        self.output_list
            .iter()
            .find(|d| d.id == id_or_name || d.name == id_or_name)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| id_or_name.to_string())
    }

    /// 当前选择落在下拉的第几项；点名的设备已经不在了返回 `usize::MAX`（菜单里不打勾）。
    pub fn output_selection(&self) -> usize {
        if self.prefs.audio_output.is_empty() {
            return 0;
        }
        self.output_list
            .iter()
            .position(|d| d.id == self.prefs.audio_output || d.name == self.prefs.audio_output)
            .map_or(usize::MAX, |i| i + 1)
    }

    /// 下拉里选中第 `i` 项（0 = 跟随系统默认），返回该存进首选项的那个标识。
    pub fn output_choice(&self, i: usize) -> Option<String> {
        if i == 0 {
            None
        } else {
            self.output_list.get(i - 1).map(Self::output_key)
        }
    }

    /// 「输出」那一格显示的字：正在出声的设备名。还没建成流时按偏好退回「系统默认」或
    /// 「<点名设备>（不可用）」，绝不显示一个根本没在出声的设备糊弄人。
    pub fn output_label(&self) -> String {
        if let Some(name) = self.audio.output_name() {
            return name;
        }
        if self.prefs.audio_output.is_empty() {
            nbs_core::tr("System default", "系统默认").to_string()
        } else {
            format!(
                "{}{}",
                self.output_display_name(&self.prefs.audio_output),
                nbs_core::tr(" (unavailable)", "（不可用）")
            )
        }
    }

    /// 端口自加 #94：切输出设备，`None` = 跟随系统默认。当场拆旧流、当场建新流
    /// （本机实测连拆带建约 0.1 秒，只在用户点那一下付），所以结果就在这一次里报。
    pub fn apply_output_choice(&mut self, wanted: Option<String>) {
        let err = self.audio.set_output(wanted).err();
        // 引擎那句「点名的没拿到，已经落到哪台」比 Err 里的原因更具体，优先报它。
        let fallback = self.audio.take_output_error();
        self.sync_output_prefs();
        match err.or(fallback) {
            Some(e) => self.set_message(format!(
                "{}{e}",
                nbs_core::tr("Failed to switch output device: ", "切换输出设备失败：")
            )),
            None => self.set_message(format!(
                "{}{}",
                nbs_core::tr("Output device: ", "输出设备："),
                self.audio.output_name().unwrap_or_default()
            )),
        }
    }

    /// 把引擎手上的实情写进偏好并落盘：点名的那台没拿到时引擎已经把它退回「跟随系统
    /// 默认」，存一个没生效的选择，下次启动还要再撞一次。
    fn sync_output_prefs(&mut self) {
        let wanted = self.audio.wanted_output().unwrap_or_default().to_string();
        if self.prefs.audio_output != wanted {
            self.prefs.audio_output = wanted;
            self.save_prefs();
        }
    }

    /// 自动跟随 / 补建那几趟的结果：接上了报设备名，没接上就报原因，两种都把偏好按
    /// 引擎的实情写回去。`tick_transport` 每帧调一次；引擎内部限流。
    pub fn poll_output_report(&mut self) {
        let name = self.audio.poll_output();
        let error = self.audio.take_output_error();
        if name.is_none() && error.is_none() {
            return;
        }
        self.sync_output_prefs();
        if let Some(e) = error {
            self.set_message(format!(
                "{}{e}",
                nbs_core::tr("Failed to switch output device: ", "切换输出设备失败：")
            ));
        } else if let Some(name) = name {
            self.set_message(format!(
                "{}{name}",
                nbs_core::tr("Output device: ", "输出设备：")
            ));
        }
    }

    pub fn apply_audio_prefs(&mut self) {
        apply_audio_prefs(&mut *self.audio, &self.prefs);
        // 实时播放此前从不遵守"包含锁定层"，只有导出遵守；这里补齐。
        let include = self.prefs.include_locked_in_export;
        self.audio
            .clock_mut()
            .set_include_locked(include, &self.doc.song);
        self.audio_rev = self.doc.song.rev;
    }

    /// 帧首刷新播放特效：曲子一改（`song.rev` 变了）、或「是否播放锁定层」一改，
    /// 时间轴重烘；导出选项里的乐器方块表一改，方块名表跟着重建；贴图目录只认一次。
    /// 没有启用中的特效时这三条各是一次比较，不铺表。
    pub fn refresh_fx(&mut self) {
        let include_locked = self.prefs.include_locked_in_export;
        self.fx
            .refresh(&self.doc.song, include_locked, &self.schematic_options);
    }

    /// 把当前会话状态同步回偏好对象。
    pub fn sync_prefs(&mut self) {
        self.prefs.theme = match self.theme.kind {
            ThemeKind::Aqua => 0,
            ThemeKind::Retro => 1,
            ThemeKind::Dark => 2,
            ThemeKind::Fluent => 3,
        };
        self.prefs.fdark = self.theme.fdark;
        // 原版 accent1/2/3 落 settings.ini（save_settings.gml:65-67）；改色乐器
        // 只改表不改基色，所以这里同步出去的永远是基色。
        self.prefs.accent = self.theme.accent_base;
        self.prefs.follow_marker = self.marker_follow;
        self.prefs.follow_mode = self.follow_mode;
        self.prefs.show_layers = self.flags.show_layers;
        self.prefs.show_piano = self.flags.show_piano;
        self.prefs.use_colors = self.flags.use_colors;
        self.prefs.use_shapes = self.flags.use_shapes;
        self.prefs.use_icons = self.flags.use_icons;
        self.prefs.show_numbers = self.flags.show_numbers;
        self.prefs.show_octaves = self.flags.show_octaves;
        self.prefs.show_keynames = self.flags.show_keynames;
        self.prefs.show_keynumbers = self.flags.show_keynumbers;
        self.prefs.show_keyboard = self.flags.show_keyboard;
        self.prefs.show_outofrange = self.flags.show_outofrange;
        self.prefs.show_incompatible = self.flags.show_incompatible;
        self.prefs.fade = self.flags.fade;
        self.prefs.show_grid_lines = self.flags.show_grid_lines;
        self.prefs.flats = self.flags.flats;
        self.prefs.blackout = self.flags.blackout;
        self.prefs.remove_effect = self.flags.remove_effect;
        self.prefs.show_notechart = self.flags.show_notechart;
        // 原版把整份 sch_exp_* 无条件写进 settings.ini，「记住我的更改」并不门控它。
        self.prefs.schematic = self.schematic_options.clone();
        self.prefs.maestro = self.maestro_options.clone();
        self.prefs.fractal = self.fractal_options;
        self.prefs.disc_chest = self.disc_chest_options;
        // MIDI 导入同理：`import_midi()` 结尾的 `save_settings()` 每次都把
        // 14 个 w_midi_* 和 128 组 ins_N / drum_N 全量写盘。
        self.prefs.midi_import = Some(nbs_io::MidiImportPrefs::from_options(&self.midi_options));
        self.prefs.midi_gm = Some(self.midi_gm.to_persist());
        // 原版 save_settings.gml 每次都写 last_version（save_settings.gml:8 附近的
        // `[preferences] last_version`），下次开机据此判断是不是刚更新过。
        self.prefs.last_version = Some(crate::VERSION.to_string());
        self.prefs.recent_files = self
            .recent_files
            .iter()
            .map(|r| RecentFile {
                path: r.path.to_string_lossy().into_owned(),
                time: r.time,
            })
            .collect();
        // 特效：只写不读。用 `sync_prefs` 会把调参面板刚滑出来的值覆盖回旧快照。
        self.fx.stack.store(&mut self.prefs);
    }

    pub fn save_prefs(&mut self) {
        self.sync_prefs();
        self.last_prefs_save = Instant::now();
        if let Err(e) = self.prefs.save() {
            self.set_message(if crate::lang() == 1 {
                format!("保存偏好失败: {e}")
            } else {
                format!("Failed to save preferences: {e}")
            });
        }
    }

    /// 端口自加：按当前 `maestro_options` 重算一遍「导出会发生哪些改写」。
    /// 只算不落盘，代价是一遍编码，开窗和换取整模式时调用。
    pub fn refresh_maestro_preview(&mut self) {
        match nbs_io::preview_maestro(&self.doc.song, &self.maestro_options) {
            Ok(r) => {
                self.maestro_report = Some(r);
                self.maestro_error = None;
            }
            Err(e) => {
                self.maestro_report = None;
                self.maestro_error = Some(e);
            }
        }
    }

    /// 端口自加：按当前 `fractal_options` 重算一遍「导出会发生哪些改写 + 机体多大」。
    /// 这条要把几何真摆一遍才能知道包围盒，比 maestro 的预览贵，只在开窗和改选项时调。
    pub fn refresh_fractal_preview(&mut self) {
        match nbs_io::preview_fractal(&self.doc.song, &self.fractal_options) {
            Ok(r) => {
                self.fractal_report = Some(r);
                self.fractal_error = None;
            }
            Err(e) => {
                self.fractal_report = None;
                self.fractal_error = Some(e);
            }
        }
    }

    /// 端口自加：按当前 `disc_chest_options` 重算一遍「编码唱片会发生哪些改写 + 数据箱多大」。
    /// 同 fractal：要把盒子真摆一遍才出得了包围盒，只在开窗和改选项时调。
    pub fn refresh_disc_chest_preview(&mut self) {
        match nbs_io::preview_disc_chest(&self.doc.song, &self.disc_chest_options) {
            Ok(r) => {
                self.disc_chest_report = Some(r);
                self.disc_chest_error = None;
            }
            Err(e) => {
                self.disc_chest_report = None;
                self.disc_chest_error = Some(e);
            }
        }
    }

    /// 原版 `minecraft_export_get_schematic_plan` 的签名
    /// （minecraft_export.gml:1030）：歌实例 + 会影响方案的那六个 sch_exp_* 项。
    fn command_plan_signature(&self) -> String {
        let o = &self.schematic_options;
        let allowed: String = o
            .command_allowed_sources
            .iter()
            .map(|&b| if b { "1" } else { "0" })
            .collect();
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.song_session,
            o.include_locked,
            o.compress,
            o.command_tempo_grid,
            o.tempo_mode,
            o.command_source,
            allowed
        )
    }

    /// 原版 `minecraft_export_command_tps`：速度档 0/1/其它 → 10/5/2.5。
    fn command_tps(&self) -> f64 {
        match self.schematic_options.tempo_mode {
            0 => 10.0,
            1 => 5.0,
            _ => 2.5,
        }
    }

    /// 有界预算的方案构建（原版 `full_search = false`）。原版把带预算的那一档用在
    /// 窗口计数上（`schematic_height/length` 走 `get_schematic_plan(false)` 复用缓存），
    /// 但命令方块模式下窗口画的是「导出时计算」（`draw_window_schematic_export.gml:138-140`），
    /// 所以端口只有导出这一条路，走 [`Self::rebuild_command_plan_forced`]（全预算）。
    fn build_command_plan(&self, full_search: bool) -> nbs_io::mc_export::Plan {
        use nbs_io::mc_export;
        let o = &self.schematic_options;
        let bank = self.audio.bank();
        let radius = 1.0 + (self.datapack_options.radius.saturating_sub(16)) as f64 * 0.06;
        mc_export::build_plan(
            &self.doc.song,
            "command",
            &o.command_source,
            &o.command_allowed_sources,
            o.include_locked,
            true,
            true,
            o.command_tempo_grid,
            self.command_tps(),
            o.layout == 0 && o.loop_option,
            o.compress,
            full_search,
            radius,
            &|_| None,
            &|i: usize| {
                bank.samples
                    .get(i)
                    .map(|s| s.duration())
                    .filter(|d| *d > 0.0)
            },
        )
    }

    /// 原版 `minecraft_export_get_schematic_plan(true)`：导出时全预算重建并替换缓存。
    pub fn rebuild_command_plan_forced(&mut self) -> &nbs_io::mc_export::Plan {
        let signature = self.command_plan_signature();
        let plan = self.build_command_plan(true);
        self.schematic_command_signature = signature;
        self.schematic_command_plan = Some(plan);
        self.schematic_command_plan.as_ref().unwrap()
    }

    /// 原版在乐器编辑/音色重载时 `sch_command_plan = undefined`。
    pub fn invalidate_command_plan(&mut self) {
        self.schematic_command_plan = None;
    }

    /// 每 30 秒静默持久化一次，避免意外退出丢失主题/音频设置。
    pub fn maybe_save_prefs(&mut self) {
        if self.last_prefs_save.elapsed().as_secs() >= 30 {
            self.sync_prefs();
            self.last_prefs_save = Instant::now();
            let _ = self.prefs.save();
        }
    }

    pub fn song_title(&self) -> String {
        // 原版 `update_window_caption.gml`：播放器只显示歌名加 " - Player Mode" 后缀
        // （不能编辑，也没有脏标记）。
        if self.player {
            let name = if self.doc.song.meta.name.is_empty() {
                self.doc
                    .song
                    .filename
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| tr("Untitled song", "无名歌曲").to_string())
            } else {
                self.doc.song.meta.name.clone()
            };
            return format!("{name} - Player Mode");
        }
        let name = if self.doc.song.meta.name.is_empty() {
            self.doc
                .song
                .filename
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| tr("Untitled song", "无名歌曲").to_string())
        } else {
            self.doc.song.meta.name.clone()
        };
        let mark = if self.doc.song.dirty { " *" } else { "" };
        format!("{name}{mark} — nbs-rust")
    }
}

/// 缩放档位表：原版 `control_draw.gml:736-751` 的 `else if` 链（两端 0.5/4），
/// 低档 0.1–0.4 是自加偏离（用户要求下界 10%）。窗口与琴键共用同一张表。
const SCALE_STEPS: [f32; 18] = [
    0.1, 0.2, 0.3, 0.4, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0,
];

fn step_in_ladder(cur: f32, up: bool) -> Option<f32> {
    if up {
        SCALE_STEPS.iter().find(|s| **s > cur + 1e-4).copied()
    } else {
        SCALE_STEPS.iter().rev().find(|s| cur > **s + 1e-4).copied()
    }
}

/// 端口自加（用户要求 #94、#103 阶段 1）：首选项里那个输出设备标识只认一次——
/// 老版本存的是设备名，在当前设备表里认得出来就换成系统给的稳定 ID（改名、重插、
/// 换驱动都还认得同一台）；认不出来（那台已经不在了）就清空，让引擎跟着系统默认走，
/// 并把旧名字还回去，由调用方在状态栏说一声。
fn migrate_output_pref(
    prefs: &mut Preferences,
    devices: &[nbs_audio::OutputDevice],
) -> Option<String> {
    if prefs.audio_output.is_empty() {
        return None;
    }
    let known = devices
        .iter()
        .find(|d| d.id == prefs.audio_output || d.name == prefs.audio_output)
        .map(EditorState::output_key);
    match known {
        Some(key) => {
            prefs.audio_output = key;
            None
        }
        None => Some(std::mem::take(&mut prefs.audio_output)),
    }
}

fn apply_audio_prefs(audio: &mut dyn AudioEngine, p: &Preferences) {
    let clock = audio.clock_mut();
    clock.set_master(p.master_volume);
    clock.set_speed(p.play_speed);
    clock.set_metronome(p.metronome);
    clock.set_max_voices(if p.extended_voices {
        EXTENDED_MAX_VOICES
    } else {
        DEFAULT_MAX_VOICES
    });
    clock.set_mono(p.mono);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 刀 0 不变量（用户要求 #131）：切走再切回，台前那份文档整份还原——内容、
    /// `rev`、撤销历史、脏标记、缓存身份都跟着自己的标签走，不串到隔壁那一首。
    #[test]
    fn switch_tab_roundtrips_the_whole_document() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let home = st.active_tab().expect("启动就有台前标签");
        let home_session = st.song_session;
        st.doc.place_note(7, 0, Note::new(0, 45));
        st.doc.song.dirty = true;
        let home_rev = st.doc.song.rev;
        let home_history = st.doc.history.len();

        let other = st.push_tab(nbs_core::model::Song::new());
        let other_rev = st.parked_doc(other).expect("后台标签读得到文档").song.rev;
        assert_ne!(other, home);
        assert_ne!(st.store.session(other), Some(home_session));

        assert!(st.switch_tab(other));
        assert_eq!(st.song_session, st.store.session(other).unwrap());
        assert_eq!(st.doc.song.rev, other_rev);
        assert_eq!(st.doc.history.len(), 0);
        assert!(!st.doc.song.dirty);
        assert!(st.doc.song.get(7, 0).is_none());
        // 台前这一首的槽位是空的（文档在台前），槽里只剩后台那一首。
        assert!(st.parked_doc(other).is_none());

        assert!(st.switch_tab(home));
        assert_eq!(st.song_session, home_session);
        assert_eq!(st.doc.song.rev, home_rev);
        assert_eq!(st.doc.history.len(), home_history);
        assert!(st.doc.song.dirty);
        assert_eq!(st.doc.song.get(7, 0), Some(Note::new(0, 45)));
        assert!(st.parked_doc(home).is_none());
        assert!(st.parked_doc(other).unwrap().song.get(7, 0).is_none());
        // 同一首歌切回来不重演一遍：标签条顺序不变、身份不变。
        assert_eq!(st.store.order(), &[home, other]);
        // 台前那一首再切一次是被拒绝的（原版 `set_song` 对同一首直接返回）。
        assert!(!st.switch_tab(home));
        assert_eq!(st.doc.song.rev, home_rev);
    }

    /// 刀 0 不变量：两首曲子算出**同一个 `rev`** 时也不许复用上一首的派生态。
    /// 只拿 `rev` 当键的下游有四处（右上角徽标、音频事件表、特效烘焙、声源分配方案），
    /// 串台就是 #116 那个徽标滞后的多标签版本。
    #[test]
    fn switch_invalidates_caches_even_when_revs_collide() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        // 台前换成一首音域内的曲子（MC 键域 33..=57）。
        let mut ok_song = nbs_core::model::Song::new();
        ok_song.set(2, 0, Note::new(0, 50));
        ok_song.recalculate();
        st.doc.replace_song(ok_song);
        // 后台压一首越界的，且让它和台前撞在同一个 rev 上。
        let mut bad_song = nbs_core::model::Song::new();
        bad_song.set(2, 0, Note::new(0, 60));
        bad_song.set(3, 0, Note::new(0, 61));
        bad_song.recalculate();
        let mut bad_doc = Document::new();
        bad_doc.replace_song(bad_song);
        assert_eq!(bad_doc.song.rev, st.doc.song.rev);
        let other = st.store.insert(bad_doc);

        let ok_compat = st.compatibility();
        st.sync_audio_events();
        // 关着特效时 refresh 一帧都不烘，这里要验的是「切标签后重烘」，先打开瀑布。
        st.fx
            .stack
            .set_active(crate::fx::synthesia::Synthesia::ID, true);
        st.refresh_fx();
        assert_eq!(st.audio_rev, st.doc.song.rev);
        assert_eq!(st.compat_rev, st.doc.song.rev);
        assert_eq!(st.fx.baked_for, (st.doc.song.rev, false));
        assert_eq!(st.fx.timeline.note_count(), 1);

        assert!(st.switch_tab(other));
        // 三处缓存打回哨兵值，而不是「rev 相等就算命中」；音频事件表是当场
        // 重挂的（刀 1：原版 `set_song` 之后新那首要能立刻出声）。
        assert_eq!(st.compat_rev, u64::MAX);
        assert_eq!(st.fx.baked_for, (u64::MAX, false));
        assert!(st.schematic_command_signature.is_empty());
        assert_eq!(st.audio_rev, st.doc.song.rev, "切标签没重挂到新那首");
        // 重算拿到的是新那首的结论。
        assert_eq!(st.doc.song.iter_notes().count(), 2, "换过来的文档");
        st.refresh_fx();
        assert_eq!(
            st.fx.timeline.note_count(),
            2,
            "切标签之后特效还在画上一首的音符"
        );
        assert_ne!(st.compatibility(), ok_compat, "徽标沿用了上一首的判定");
        assert_eq!(st.compat, 0);
    }

    /// 刀 1（用户要求 #131）：每首曲子那份界面状态跟着自己的标签走。
    /// 画笔、滚动位置、区间、乐器面板滚动、MIDI 文件名、播放头——切走要收回槽里，
    /// 切回要一格不差装回来；中间那一首拿到的是出厂值，不是上一首的。
    #[test]
    fn switch_tab_roundtrips_the_per_tab_ui_state() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let home = st.active_tab().unwrap();
        st.current = Note::new(7, 60);
        st.viewport.start_tick = 40;
        st.viewport.start_layer = 3;
        st.section_exists = true;
        st.section_start = 4;
        st.section_end = 9;
        st.insbox_start = 21;
        st.midi_file = "demo.mid".into();
        st.set_marker(12.5);

        let other = st.push_tab(nbs_core::model::Song::new());
        assert!(st.switch_tab(other));
        // 台上换成出厂值（原版新歌曲就是刚 create 出来的 obj_song）。
        assert_eq!(st.current, Note::default());
        assert_eq!((st.viewport.start_tick, st.viewport.start_layer), (0, 0));
        assert!(!st.section_exists);
        assert_eq!((st.section_start, st.section_end), (0, 0));
        assert_eq!(st.insbox_start, 0);
        assert!(st.midi_file.is_empty());
        assert_eq!(st.audio.clock().position(), 0.0);
        // home 那一份是收进它自己的槽位，不是塞给刚上台那一首。
        let parked = st.store.ui(home).unwrap().clone();
        assert_eq!(parked.brush, Note::new(7, 60));
        assert_eq!(parked.scroll_tick, 40);
        assert_eq!(parked.midi_file, "demo.mid");
        assert_eq!(st.store.ui(other).unwrap().playhead, 0.0);

        assert!(st.switch_tab(home));
        assert_eq!(st.current, Note::new(7, 60));
        assert_eq!((st.viewport.start_tick, st.viewport.start_layer), (40, 3));
        assert_eq!(
            (st.section_exists, st.section_start, st.section_end),
            (true, 4, 9)
        );
        assert_eq!(st.insbox_start, 21);
        assert_eq!(st.midi_file, "demo.mid");
        assert_eq!(st.audio.clock().position(), 12.5);
    }

    /// 刀 1：切标签 = 原版 `set_song.gml:6-19`。播放停、播放头保留，
    /// 速度拖拽 / 淡出 / 节拍器 / 单声道 / 标尺按住 / 层音量拖拽全部归零，
    /// 音色库换成新那首的（用 `clock().tempo()` 观察，prepare 会跟着 song 重设）。
    #[test]
    fn switch_tab_stops_playback_and_rehooks_audio() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let home = st.active_tab().unwrap();
        st.doc.song.meta.tempo = 1.0;
        // 摆个靠后的音，歌曲长度盖过下面的播放头，免得起播时先被弹回 0 列。
        st.doc.place_note(30, 0, Note::new(0, 45));
        st.flags.fade = true;
        st.prefs.metronome = true;
        st.prefs.mono = true;
        st.tempo_drag = Some(2.0);
        st.tempo_input = Some("2".into());
        st.interaction.mode = crate::interaction::DragMode::PressTimeline { pressa: 3 };
        st.layer_drag = Some(LayerDrag {
            index: 0,
            stereo: false,
            value: 100,
            raw: 100.0,
        });
        let other = {
            let mut s = nbs_core::model::Song::new();
            s.meta.tempo = 2.5;
            s.set(1, 0, Note::new(0, 45));
            s.recalculate();
            st.push_tab(s)
        };
        let home_pos = 5.0;
        st.set_marker(home_pos);
        st.begin_playback();
        assert!(st.is_playing());

        assert!(st.switch_tab(other));
        assert!(!st.is_playing(), "原版切标签必停播");
        assert_eq!(st.audio.clock().tempo(), 2.5, "音色库还挂在上一首");
        assert_eq!(st.audio.clock().position(), 0.0, "另一首的播放头是自己的");
        assert!(!st.flags.fade);
        assert!(!st.prefs.metronome);
        assert!(!st.prefs.mono);
        assert_eq!(st.tempo_drag, None);
        assert_eq!(st.tempo_input, None);
        assert!(matches!(
            st.interaction.mode,
            crate::interaction::DragMode::None
        ));
        assert!(st.layer_drag.is_none());

        // 切回上一首：停播不变（原版如此），但播放头回到离开时那一格。
        assert!(st.switch_tab(home));
        assert!(!st.is_playing());
        assert_eq!(st.audio.clock().position(), home_pos);
        assert_eq!(st.audio.clock().tempo(), 1.0);
    }

    /// 刀 1：关后台标签——台前那一份文档和它的界面状态一格不动，
    /// 但原版经过两次 `set_song`，所以播放照旧停、fade/节拍器/单声道照旧清。
    #[test]
    fn closing_a_background_tab_stops_playback_but_keeps_the_stage() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let home = st.active_tab().unwrap();
        st.doc.place_note(4, 0, Note::new(0, 45));
        st.current = Note::new(9, 50);
        st.viewport.start_tick = 12;
        st.midi_file = "keep.mid".into();
        let back = st.push_tab(nbs_core::model::Song::new());
        st.flags.fade = true;
        st.prefs.metronome = true;
        st.begin_playback();

        let out = st.close_tab(back);
        assert_eq!(
            out,
            crate::tabs::Closed {
                active: Some(home),
                was_active: false
            }
        );
        assert_eq!(st.active_tab(), Some(home));
        assert_eq!(st.doc.song.get(4, 0), Some(Note::new(0, 45)));
        assert_eq!(st.current, Note::new(9, 50));
        assert_eq!(st.viewport.start_tick, 12);
        assert_eq!(st.midi_file, "keep.mid");
        assert_eq!(st.song_session, st.store.session(home).unwrap());
        assert!(!st.is_playing());
        assert!(!st.flags.fade);
        assert!(!st.prefs.metronome);
        // 关掉的号再也读不出来，标签条顺序里也没有了。
        assert!(st.parked_doc(back).is_none());
        assert_eq!(st.store.order(), &[home]);
    }

    /// 刀 1：关台前标签落在原版那条下标规则选出的邻居身上，装的是邻居那份界面
    /// 状态；关到最后一首时台前换成一首空曲、界面回出厂值（补不补标签由调用方定）。
    #[test]
    fn closing_the_active_tab_restores_the_neighbour() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let a = st.active_tab().unwrap();
        // 台前是开机那首演示曲，给它一枚好认的标记，下面用它认「台上摆的是哪一首」。
        st.doc.place_note(40, 7, Note::new(0, 45));
        st.current = Note::new(1, 40);
        let b = {
            let mut s = nbs_core::model::Song::new();
            s.set(41, 7, Note::new(0, 45));
            s.recalculate();
            st.push_tab(s)
        };
        assert!(st.switch_tab(b));
        st.current = Note::new(2, 50);
        st.viewport.start_tick = 7;

        let out = st.close_tab(b);
        assert_eq!(
            out,
            crate::tabs::Closed {
                active: Some(a),
                was_active: true
            }
        );
        assert_eq!(st.active_tab(), Some(a));
        assert_eq!(st.song_session, st.store.session(a).unwrap());
        assert_eq!(st.current, Note::new(1, 40), "装回来的是邻居那份画笔");
        assert_eq!(st.viewport.start_tick, 0);
        assert_eq!(st.doc.song.get(40, 7), Some(Note::new(0, 45)));
        assert_eq!(st.doc.song.get(41, 7), None, "被关那首的文档该整份消失");
        assert_eq!(st.audio_rev, st.doc.song.rev);
        assert!(!st.is_playing());

        // 关到没有标签：台前是一首空曲，标签条空、身份归零。
        let out = st.close_tab(a);
        assert_eq!(
            out,
            crate::tabs::Closed {
                active: None,
                was_active: true
            }
        );
        assert!(st.store.is_empty());
        assert_eq!(st.active_tab(), None);
        assert_eq!(st.song_session, 0);
        assert_eq!(st.doc.song.total_blocks, 0);
        assert_eq!(st.current, Note::default());
        // 关一个不存在的号不动任何状态。
        let out = st.close_tab(b);
        assert_eq!(
            out,
            crate::tabs::Closed {
                active: None,
                was_active: false
            }
        );
    }

    /// 刀 1：`open_tab` = 原版四条入口共用的「建实例 + `reset_add` + `set_song`」。
    /// 新标签上台、进标签条末尾，前台窗和右键菜单被 `reset_add` 关掉。
    #[test]
    fn open_tab_stages_the_new_song_and_clears_transient_ui() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let home = st.active_tab().unwrap();
        st.window = Some(WindowKind::Properties);
        st.window_prev = Some(WindowKind::Properties);
        st.sb_sel = 3;
        st.piano_scroll_delay = 0.5;
        st.viewport.start_tick = 12;
        let home_session = st.song_session;
        let mut song = nbs_core::model::Song::new();
        song.set(6, 0, Note::new(0, 45));
        song.recalculate();
        let opened = st.open_tab(Document::with_song(song));

        assert_eq!(st.active_tab(), Some(opened));
        assert_eq!(st.store.order(), &[home, opened]);
        assert_eq!(st.song_session, st.store.session(opened).unwrap());
        assert_ne!(st.song_session, home_session, "新标签要和上一首分清身份");
        assert_eq!(st.doc.song.get(6, 0), Some(Note::new(0, 45)));
        assert_eq!(st.audio_rev, st.doc.song.rev);
        assert_eq!(st.window, None);
        assert_eq!(st.window_prev, None);
        assert!(st.context_menu.is_none());
        assert_eq!(st.piano_scroll_delay, 0.0);
        assert_eq!(st.sb_sel, 0);
        // 上一首的界面状态留在自己槽里，没有跟着上新台。
        assert_eq!(st.store.ui(home).unwrap().scroll_tick, 12);
        assert_eq!(st.viewport.start_tick, 0);
        assert_eq!(st.current, Note::default());
    }

    /// 原版 `update_window_caption.gml`：播放器标题 = 歌名 + " - Player Mode"，
    /// 无歌名退回文件名主干。
    #[test]
    fn player_mode_title_carries_the_suffix() {
        let mut st = EditorState::for_test();
        st.player = true;
        st.doc.song.meta.name = "Demo".into();
        assert_eq!(st.song_title(), "Demo - Player Mode");
        st.doc.song.meta.name.clear();
        st.doc.song.filename = Some(std::path::PathBuf::from("C:/songs/my song.nbs"));
        assert_eq!(st.song_title(), "my song - Player Mode");
    }

    /// 播放器档 autosave 门：脏歌 + 备份间隔到了也不刷新计时器（不写备份）。
    #[test]
    fn player_mode_skips_autosave() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        st.player = true;
        st.doc.song.dirty = true;
        st.prefs.auto_save_minutes = 1;
        st.last_autosave = Instant::now() - std::time::Duration::from_secs(3600);
        let ctx = Context::default();
        st.tick_transport(&ctx);
        assert!(
            st.last_autosave.elapsed().as_secs() >= 3600,
            "播放器档不该重置自动备份计时"
        );
    }

    /// 音频导出的文件类型必须跟着所选格式走（原版 `get_save_filename_ext`）。
    #[test]
    fn audio_dialog_ext_follows_chosen_format() {
        let mut d = FileDialog::for_action(DialogAction::ExportAudio, None, None);
        assert_eq!(d.ext, "mp3", "原版默认 audio_exp_format = MP3");
        assert!(d.name.ends_with(".mp3"), "{}", d.name);
        d.set_ext("flac");
        assert_eq!(d.ext, "flac");
        assert!(d.name.ends_with(".flac"), "{}", d.name);
        // 其它动作用默认扩展名，且 set_ext 只对当前对话框生效
        let song = FileDialog::for_action(DialogAction::SaveSong, None, None);
        assert_eq!(song.ext, "nbs");
    }

    /// 端口自加 #94：「输出」下拉第 0 项固定是跟随系统默认，设备从第 1 项开始；
    /// 选中项存的是稳定 ID，点名设备掉线后不再打勾，也不能凭空造出一个选择。
    #[test]
    fn output_menu_items_map_back_to_a_device_name() {
        let mut st = EditorState::for_test();
        st.output_list = vec![
            nbs_audio::OutputDevice {
                id: "wasapi:{SPK}".into(),
                name: "扬声器".into(),
                is_default: true,
            },
            nbs_audio::OutputDevice {
                id: "wasapi:{HP}".into(),
                name: "耳机".into(),
                is_default: false,
            },
        ];
        assert_eq!(st.output_items().len(), 3, "1 个跟随默认 + 2 台设备");
        assert_eq!(st.output_selection(), 0, "出厂跟随系统默认 → 勾第 0 项");
        assert_eq!(st.output_choice(0), None);
        assert_eq!(st.output_choice(1).as_deref(), Some("wasapi:{SPK}"));
        assert_eq!(st.output_choice(2).as_deref(), Some("wasapi:{HP}"));
        assert_eq!(st.output_choice(3), None, "越界不编造设备名");
        st.prefs.audio_output = "wasapi:{HP}".into();
        assert_eq!(st.output_selection(), 2, "存 ID 要能认回第 2 项");
        st.prefs.audio_output = "耳机".into();
        assert_eq!(st.output_selection(), 2, "老名字也认得，别判成掉线");
        st.prefs.audio_output = "已经拔掉的声卡".into();
        assert_eq!(st.output_selection(), usize::MAX, "不在了就不打勾");
    }

    /// 端口自加 #103 阶段 1：老首选项存的设备名只认一次换成稳定 ID；后端没给 ID 的
    /// 设备退回名字（别把它清空）；那台已经不在了就清空回落「跟随系统默认」，
    /// 并把旧名字交回调用方去状态栏说一声。
    #[test]
    fn an_old_output_device_name_is_migrated_to_its_id_once() {
        let devices = vec![
            nbs_audio::OutputDevice {
                id: "wasapi:{SPK}".into(),
                name: "扬声器".into(),
                is_default: true,
            },
            nbs_audio::OutputDevice {
                id: String::new(),
                name: "没有 ID 的声卡".into(),
                is_default: false,
            },
        ];
        let mut prefs = Preferences::default();
        // 出厂空值：什么都不动
        assert_eq!(migrate_output_pref(&mut prefs, &devices), None);
        assert!(prefs.audio_output.is_empty());
        // 名字认得出来 → 换成 ID，不需要提示
        prefs.audio_output = "扬声器".into();
        assert_eq!(migrate_output_pref(&mut prefs, &devices), None);
        assert_eq!(prefs.audio_output, "wasapi:{SPK}");
        // 已经是 ID：再跑一次不该改动，也不该报「不在了」
        prefs.audio_output = "wasapi:{HP}".into();
        let devices_with_hp = {
            let mut d = devices.clone();
            d.push(nbs_audio::OutputDevice {
                id: "wasapi:{HP}".into(),
                name: "耳机".into(),
                is_default: false,
            });
            d
        };
        assert_eq!(migrate_output_pref(&mut prefs, &devices_with_hp), None);
        assert_eq!(prefs.audio_output, "wasapi:{HP}");
        // 后端没给 ID：存名字就是它能认的全部，别清掉
        prefs.audio_output = "没有 ID 的声卡".into();
        assert_eq!(migrate_output_pref(&mut prefs, &devices), None);
        assert_eq!(prefs.audio_output, "没有 ID 的声卡");
        // 那台不在了：清空 + 把旧名字交回去
        prefs.audio_output = "拔掉的外置声卡".into();
        assert_eq!(
            migrate_output_pref(&mut prefs, &devices).as_deref(),
            Some("拔掉的外置声卡")
        );
        assert!(prefs.audio_output.is_empty(), "回落跟随系统默认");
    }

    /// 端口自加 #103 阶段 1：整条启动链路（`EditorState::new()`）——盘上存的那台已经不
    /// 在了，要清空回落「跟随系统默认」、落盘，并在状态栏说一声是哪台。
    /// 设备名用一句这台机器上不可能存在的，用例才不依赖具体硬件。
    #[test]
    fn a_gone_output_device_is_cleared_at_startup_and_said_aloud() {
        let _prefs = crate::isolate_prefs();
        let mut seed = Preferences::default();
        seed.audio_output = "nbs-rust 这台机器上不存在的输出设备".into();
        seed.save().unwrap();
        let _cn = crate::LangGuard::set(1);
        let st = EditorState::new();
        assert!(st.prefs.audio_output.is_empty(), "认不出来的老名字要清空");
        assert!(
            Preferences::load().audio_output.is_empty(),
            "清空要落盘，别每次启动都撞一次同一堵墙"
        );
        let m = st.active_message().unwrap_or_default().to_string();
        assert!(
            m.contains("已经不在了") && m.contains("跟随系统默认"),
            "状态栏要说清回落了：{m:?}"
        );
        assert!(m.contains("不存在的输出设备"), "要说清是哪台不在了：{m:?}");
    }

    /// 端口自加 #94：点名的设备拿不到 → 当场回落系统默认，落盘的选择必须跟着变空，
    /// 别存一个没生效的下标（否则下次启动还要再撞一次）。
    #[test]
    fn dead_output_choice_does_not_stick_in_prefs() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::for_test();
        let missing = "nbs-rust 这台机器上不存在的输出设备".to_string();
        st.apply_output_choice(Some(missing.clone()));
        assert!(
            st.prefs.audio_output.is_empty(),
            "没生效的点选不该留下：{}",
            st.prefs.audio_output
        );
        let m = st.active_message().unwrap_or_default().to_string();
        assert!(
            m.contains(&missing) || m.starts_with("输出设备：") || m.starts_with("Output device: "),
            "点下去当场就该报出结果，实际消息 {m:?}"
        );
        st.apply_output_choice(None);
        assert!(st.prefs.audio_output.is_empty());
    }

    /// 端口自加 #94：还没建成流时，那一格显示的是「按偏好的退回文案」，
    /// 不能显示一个根本没在出声的设备名。
    #[test]
    fn output_label_falls_back_without_a_stream() {
        let _cn = crate::LangGuard::set(1);
        let mut st = EditorState::for_test();
        st.audio = Box::new(nbs_audio::SilentEngine::new());
        assert_eq!(st.output_label(), "系统默认");
        st.prefs.audio_output = "拔掉的耳机".into();
        assert_eq!(st.output_label(), "拔掉的耳机（不可用）");
    }

    /// 原生选择器的文案必须和原版传给 `get_*_filename_ext` 的参数一致。
    /// 断言写的是中文分支，测试里把语言钉回 1。
    #[test]
    fn picker_spec_carries_the_original_titles_and_filters() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let open = FileDialog::for_action(DialogAction::OpenSong, None, None).picker_spec();
        assert_eq!(open.title, "打开歌曲");
        assert_eq!(
            open.filters
                .iter()
                .map(|f| (f.name.as_str(), f.patterns.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Note Block Songs (*.nbs)", "*.nbs"),
                ("Minecraft Schematics (*.schematic)", "*.schematic"),
            ],
        );

        // 原版 `audio_export.gml:21-24`：显示名用的就是 `audio_exp_format` 本身。
        let mut audio = FileDialog::for_action(DialogAction::ExportAudio, None, None);
        audio.set_ext("flac");
        let spec = audio.picker_spec();
        assert_eq!(spec.title, "导出音频文件");
        assert_eq!(spec.filters[0].name, "FLAC files (*.flac)");
        assert_eq!(spec.filters[0].patterns, "*.flac");

        // 数据包关掉 ZIP 时原版给的是 `get_save_filename_ext("数据包目录", …)`：
        // 过滤器只有一个词、没有通配符（`datapack_export.gml:12`），不是目录选择器。
        let mut dat = FileDialog::for_action(DialogAction::ExportDatapack, None, None);
        assert_eq!(dat.picker_spec().filters[0].name, "ZIP archive (*.zip)");
        dat.set_ext("");
        let spec = dat.picker_spec();
        assert_eq!(spec.filters[0].name, "数据包目录");
        assert_eq!(spec.filters[0].patterns, "");

        // 结构方块导出的不是 schematic 而是 .nbt（`schematic_export.gml:6`）。
        let mut sch = FileDialog::for_action(DialogAction::ExportSchematic, None, None);
        sch.set_ext("nbt");
        assert_eq!(
            sch.picker_spec().filters[0].name,
            "Minecraft Structures (*.nbt)"
        );

        // 首选项里的「修改」：原版是过滤器为空串的保存框，文件名栏塞占位串
        // （`draw_window_preferences.gml:278`），选完由调用处取它所在的目录。
        let dlg = FileDialog::for_action(DialogAction::PickFolder(PrefsFolder::Song), None, None);
        let spec = dlg.picker_spec();
        assert_eq!(spec.title, "歌曲文件夹");
        assert!(spec.filters.is_empty(), "原版第 1 个参数是空串");
        assert!(dlg.save, "原版走的是 get_save_filename_ext");
        assert_eq!(dlg.name, "选择歌曲文件夹");

        // 兼容性窗口的「保存资源包」：`datapack_getextranotes.gml:6`。
        let pack = FileDialog::for_action(DialogAction::SaveExtraNotes, None, None);
        let spec = pack.picker_spec();
        assert_eq!(spec.title, "为数据包保存音符盒资源包");
        assert_eq!(spec.filters[0].name, "Resource pack (*.zip)");
        assert_eq!(spec.filters[0].patterns, "*.zip");
        assert!(pack.save, "原版是 get_save_filename_ext");
        assert!(!pack.start_dir, "原版第 3 个参数是空串，起始目录交给系统记");
    }

    /// 兼容性窗口那两行的判据：档位表必须和原版一模一样，
    /// 否则「调整速度」按钮该出现的时候不出现、绿灯该亮的时候不亮。
    #[test]
    fn compat_tempo_sets_are_exactly_the_originals() {
        for t in [10.0_f32, 5.0, 2.5] {
            assert!(tempo_schematic_ok(t), "{t} 应能导 Schematic");
            assert!(tempo_datapack_good(t), "{t} 应是数据包的好速度");
        }
        for t in [20.0_f32, 4.0, 2.0, 1.25, 1.0, 0.5, 0.25] {
            assert!(
                !tempo_schematic_ok(t),
                "数据包的好速度不等于能导 Schematic: {t}"
            );
            assert!(tempo_datapack_good(t), "{t} 应是数据包的好速度");
        }
        for t in [23.45_f32, 9.0, 3.0, 1.5, 0.3] {
            assert!(!tempo_schematic_ok(t), "{t} 不该被判成能导 Schematic");
            assert!(!tempo_datapack_good(t), "{t} 不该被判成好速度");
        }
    }

    /// 右上角那颗徽标不能落后于歌曲本身：换歌时 `Document::replace_song` 会把
    /// `rev` 归到载入值（新载入的都是同一个数），缓存必须跟着作废。
    #[test]
    fn the_compat_badge_follows_a_swapped_song() {
        use nbs_core::model::{Note, Song};
        let mut st = EditorState::for_test();
        let mut loud = Song::new();
        loud.set(0, 0, Note::new(0, 100)); // 超出 MC 音域 → 只走数据包
        st.doc.replace_song(loud);
        assert_eq!(st.compatibility(), 0, "载入超音域的歌，徽标当帧就该改口");
        st.doc.replace_song(Song::new());
        assert_eq!(st.compatibility(), 1, "换回空歌不能把上一首的判定留下");
    }

    /// 原版拖速度框时把量化值实时写进 `real_tempo`（`control_draw.gml:2534`），
    /// 徽标是跟着手走的；端口把拖动值暂存在 `tempo_drag`，判定也要读它。
    #[test]
    fn the_compat_badge_follows_a_live_tempo_drag() {
        let mut st = EditorState::for_test();
        assert_eq!(
            st.doc.song.meta.tempo, 10.0,
            "默认速度该是能导 Schematic 的那档"
        );
        assert_eq!(st.compatibility(), 1);
        st.tempo_drag = Some(6.3); // 量化成 6.25，既不是 10/5/2.5 也没到数据包好速度
        assert_eq!(
            st.compatibility(),
            2,
            "拖动过程中徽标就该改成「仅限数据包」，不能等松手"
        );
        st.tempo_drag = Some(10.0);
        assert_eq!(st.compatibility(), 1, "拖回 10 又该变回完全兼容");
        st.tempo_drag = None;
        assert_eq!(st.compatibility(), 1, "松手没提交（速度没变）就维持原判定");
    }

    /// 假选择器：按预设结果回话，顺便记下它收到了哪一次请求。
    struct FakePicker {
        answer: PickResult,
        seen: std::sync::Mutex<Vec<String>>,
    }
    impl FilePicker for FakePicker {
        fn pick(&self, dlg: &FileDialog) -> PickResult {
            self.seen.lock().unwrap().push(dlg.picker_spec().title);
            self.answer.clone()
        }
    }

    fn state_with(answer: PickResult) -> (EditorState, std::sync::Arc<FakePicker>) {
        let picker = std::sync::Arc::new(FakePicker {
            answer,
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let mut st = EditorState::for_test();
        // dispatch 按各自 state 的 prefs 同步全局语言；钉中文让「保存歌曲」断言稳定。
        st.prefs.language = 1;
        st.picker = Some(picker.clone());
        // 开机流程会开欢迎窗；选框测试的靶子是「欢迎窗已关」的裸编辑器。
        st.window = None;
        (st, picker)
    }

    /// 装了原生选择器：请求挂到帧外，回来后直接走 `DialogConfirm`，全程不开自绘窗口。
    #[test]
    fn native_picker_answers_without_the_in_app_dialog() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let dir = std::env::temp_dir().join("nbs-rust-pick-test");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("picked.nbs");
        let _ = std::fs::remove_file(&out);

        let (mut st, picker) = state_with(PickResult::Picked(out.clone()));
        let ctx = egui::Context::default();
        st.request_file_dialog(FileDialog::for_action(DialogAction::SaveSong, None, None));
        assert!(st.pending_dialog.is_some(), "请求要留给宿主在帧外执行");
        assert!(st.window.is_none(), "装了原生选择器就不该开自绘选框");

        st.run_pending_dialog();
        assert!(st.pending_dialog.is_none());
        assert_eq!(
            picker.seen.lock().unwrap().as_slice(),
            &["保存歌曲".to_string()]
        );
        // DialogConfirm(SaveSong) → SavePath：真的把歌曲写到选定的路径上。
        st.flush(&ctx);
        assert!(out.is_file(), "选完路径必须真的保存，实测 {:?}", out);
        assert!(st.window.is_none());
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn cancelled_native_picker_leaves_nothing_behind() {
        let (mut st, _picker) = state_with(PickResult::Cancelled);
        let ctx = egui::Context::default();
        st.request_file_dialog(FileDialog::for_action(DialogAction::OpenSong, None, None));
        st.run_pending_dialog();
        assert!(st.pending_dialog.is_none());
        assert!(st.file_dialog.is_none(), "取消后不该留着上一次请求");
        assert!(st.window.is_none());
        st.flush(&ctx);
        assert!(st.doc.song.filename.is_none(), "取消不该改动文档");
    }

    /// COM 起不来（非 Windows、或系统组件出错）时回落自绘选框，功能不断。
    #[test]
    fn unavailable_native_picker_falls_back_to_the_drawn_one() {
        let (mut st, _picker) = state_with(PickResult::Unavailable);
        st.request_file_dialog(FileDialog::for_action(DialogAction::OpenSong, None, None));
        assert!(st.pending_dialog.is_some());
        st.run_pending_dialog();
        assert_eq!(st.window, Some(WindowKind::FileOpen));
        assert!(st.file_dialog.is_some(), "自绘选框要靠它显示当前目录");
    }

    /// 首选项「修改」：系统组件交回来的是「<目录>\\选择歌曲文件夹」这一整串，
    /// 原版记的却是 `filename_dir(a)` —— 也就是那个目录本身，占位名不许漏进设置里。
    #[test]
    fn folder_request_stores_the_directory_not_the_placeholder_name() {
        let dir = std::env::temp_dir().join("nbs-rust-pick-test");
        let _ = std::fs::create_dir_all(&dir);
        let picked = dir.join(PrefsFolder::Song.placeholder());
        let (mut st, _picker) = state_with(PickResult::Picked(picked.clone()));
        let ctx = egui::Context::default();
        st.request_file_dialog(FileDialog::for_action(
            DialogAction::PickFolder(PrefsFolder::Song),
            None,
            Some(&dir),
        ));
        st.run_pending_dialog();
        st.flush(&ctx);
        assert_eq!(
            st.prefs.song_folder.as_deref(),
            Some(dir.to_str().unwrap()),
            "实测 {picked:?}"
        );
        assert_eq!(st.window, Some(WindowKind::Preferences));
    }

    /// 原版 `control_draw.gml:256-263`：只有 `layerhov_vppreview` 打开、且鼠标悬在
    /// `0..=endb` 的层上时，滚轮换音色/换键的试听才用那条层的音量和声道，
    /// 其余情况一律按中性 100/100（原版 `selected_vel = selected_pan = 100`）。
    #[test]
    fn wheel_preview_uses_the_hovered_layer_only_when_the_pref_is_on() {
        use nbs_core::model::Layer;
        let mut st = EditorState::for_test();
        // 清掉新建模板自带的音符，好让 endb 由本用例说了算。
        st.doc.song.columns.clear();
        st.doc.song.layers = vec![
            Layer::default(),
            Layer {
                volume: 60,
                stereo: 30,
                ..Layer::default()
            },
        ];
        st.doc.song.set(0, 1, Note::default());
        st.doc.song.recalculate();
        assert_eq!(st.doc.song.endb, 1, "用例前提：最后一条有音符的层是 1");
        st.hover_cell = Some((0, 1));
        // 出厂值不靠磁盘：临时配置目录里可能正好留着别的用例写下的 true。
        // 「缺键退回出厂关闭」由 nbs-io/prefs.rs 的持久化用例钉，这里只管两种取值的行为。
        st.prefs.layerhov_vppreview = false;
        assert_eq!(
            st.wheel_preview_vp(),
            (100, 100),
            "出厂关闭时试听按中性 100/100"
        );
        st.prefs.layerhov_vppreview = true;
        assert_eq!(
            st.wheel_preview_vp(),
            (60, 30),
            "打开后取悬停层的 volume/stereo"
        );
        // 原版条件 `selby <= current_song.endb`：endb 现在是 1，2 号层不算。
        st.hover_cell = Some((0, 2));
        assert_eq!(
            st.wheel_preview_vp(),
            (100, 100),
            "超出 endb 的层不算（原版 selby <= endb）"
        );
        st.hover_cell = None;
        assert_eq!(
            st.wheel_preview_vp(),
            (100, 100),
            "鼠标不在格子上时按 100/100"
        );
    }

    /// 原版 `draw_window_tempo_tapper` 的打点算法：滑动平均 + 「间隔突然拉长一倍
    /// 就当重新起打」。这里钉的是三条：第一下不出值、等间隔值稳定、慢一半的清零。
    #[test]
    fn tempo_tapper_averages_taps_and_restarts_on_a_double_interval() {
        let mut t = TempoTapper::default();
        // 原版 `if (tapping)`：第一下只记时刻，不进平均。
        assert!(t.register_tap(1000), "每一下都要发打点音");
        assert_eq!(t.bpm, 0.0, "第一下没有间隔可测");
        assert_eq!(t.taps, 1);
        // 之后每 500 ms 一下 = 120 BPM，再多点也不变。
        t.register_tap(1500);
        assert_eq!(t.measured(), 120.0, "60 / 0.5 秒");
        t.register_tap(2000);
        t.register_tap(2500);
        assert_eq!(t.measured(), 120.0, "等间隔打点平均不漂移");
        assert_eq!(t.taps, 4);
        // 间隔拉长到 2.5 秒（24 BPM）：floor(24 + 0.5) <= 120 / 2 成立 → 清零重新起打。
        t.register_tap(5000);
        assert_eq!(t.bpm, 0.0, "间隔翻倍要当作重新起打");
        assert_eq!(t.taps, 1, "清零后这一下算第一个点");
        t.register_tap(6000);
        assert_eq!(t.measured(), 60.0, "重新起打后按新的间隔测");
        // 同一毫秒连两下：原版 `60 / 0` 会得到 inf 并写进 taptempo，端口不照抄。
        let before = t.bpm;
        t.register_tap(6000);
        assert_eq!(t.bpm, before, "同一毫秒不得把 inf 写进测量值");
        assert!(t.bpm.is_finite());
        // 关窗复位：原版清三条，`taps` 不动。
        let taps_before = t.taps;
        t.reset_for_close();
        assert_eq!(t.bpm, 0.0);
        assert!(!t.tapping);
        assert_eq!(t.last_ms, 0);
        assert_eq!(t.taps, taps_before, "原版没有清 taps");
        // 残留的 taps 不会污染新一轮：第一下只记时刻，第二下照样按新间隔测。
        t.register_tap(6500);
        assert_eq!(t.bpm, 0.0, "新一轮第一下没有间隔可测");
        t.register_tap(7000);
        assert_eq!(t.measured(), 120.0, "上一轮残留的计数不得把新一轮平均带偏");
    }

    /// 区间起播/停播的靶子谱：空谱、没有窗口、播放头归零。
    fn section_state() -> EditorState {
        let mut st = EditorState::for_test();
        st.window = None;
        st.doc.replace_song(nbs_core::model::Song::new());
        st.prepare_audio();
        st.set_marker(0.0);
        st
    }

    /// 原版 `control_draw.gml:1033` 的停点判据是三个条件与：`marker_end` 开关、
    /// 区间存在、终点在起点之后。零宽和反向区间都不算。
    #[test]
    fn section_stop_needs_the_switch_a_section_and_a_real_end() {
        let _prefs = crate::isolate_prefs();
        let mut st = section_state();
        st.section_exists = true;
        st.section_start = 4;
        st.section_end = 9;
        assert_eq!(st.section_stop_tick(), None, "出厂 marker_end = 关");
        st.prefs.marker_end = true;
        assert_eq!(st.section_stop_tick(), Some(9));
        st.section_exists = false;
        assert_eq!(st.section_stop_tick(), None, "没有区间就没有停点");
        st.section_exists = true;
        st.section_end = 4;
        assert_eq!(st.section_stop_tick(), None, "零宽区间不算停点");
    }

    /// 原版每帧现算停点，端口把它推给播放时钟（`TransportClock::set_stop_at`），
    /// 关掉开关或删掉区间要推回「没有停点」的哨兵值 -1。
    #[test]
    fn sync_section_stop_pushes_the_stop_tick_into_the_clock() {
        let _prefs = crate::isolate_prefs();
        let mut st = section_state();
        assert_eq!(st.stop_at_pushed, -1.0, "空状态没有停点");
        st.section_exists = true;
        st.section_start = 4;
        st.section_end = 9;
        st.prefs.marker_end = true;
        st.sync_section_stop();
        assert_eq!(st.stop_at_pushed, 9.0);
        st.prefs.marker_end = false;
        st.sync_section_stop();
        assert_eq!(st.stop_at_pushed, -1.0, "关开关要把哨兵值推回去");
        st.prefs.marker_end = true;
        st.sync_section_stop();
        st.clear_section();
        st.sync_section_stop();
        assert_eq!(st.stop_at_pushed, -1.0, "删区间同样要撤掉停点");
    }

    /// 原版 `toggle_playing:9`：按播放键时 `marker_start` 开着就从区间起点起播，
    /// 判据和停点那条一样只看 `end > start`；开关关掉则仍按播放头。
    #[test]
    fn begin_playback_honors_marker_start() {
        let _prefs = crate::isolate_prefs();
        let mut st = section_state();
        st.section_exists = true;
        st.section_start = 4;
        st.section_end = 9;
        st.prefs.marker_start = true;
        st.begin_playback();
        assert!(st.is_playing());
        assert_eq!(st.marker(), 4.0, "开关开着要落在区间起点");
        st.audio.stop();
        st.set_marker(0.0);
        st.prefs.marker_start = false;
        st.begin_playback();
        assert_eq!(st.marker(), 0.0, "关掉开关就该按播放头起播");
        st.audio.stop();
        st.prefs.marker_start = true;
        st.section_start = 9;
        st.begin_playback();
        assert_eq!(st.marker(), 0.0, "零宽区间不跳（原版 end > start 那道闸）");
        st.audio.stop();
    }

    /// 原版区间是「每首歌自带一组」的运行时状态（`obj_song/Create_0.gml:22-24`），
    /// 既不存盘也不跨歌；端口把它挂在 `EditorState` 上，换歌处调用的就是这里
    /// （见 commands.rs 的 `song_replacement_clears_the_section`），三件一起归零。
    #[test]
    fn clear_section_resets_all_three_fields() {
        let _prefs = crate::isolate_prefs();
        let mut st = section_state();
        st.section_exists = true;
        st.section_start = 7;
        st.section_end = 12;
        st.prefs.marker_end = true;
        st.sync_section_stop();
        st.clear_section();
        assert!(!st.section_exists);
        assert_eq!((st.section_start, st.section_end), (0, 0));
        assert_eq!(st.section_stop_tick(), None);
    }
}
