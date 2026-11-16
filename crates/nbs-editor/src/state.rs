//! 编辑器总状态。原版是 `obj_controller` 单实例的几百个实例变量；
//! 这里收敛成结构化字段，并保留 `window / select / select_press*` 的状态机语义。

use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{Context, Pos2};
use nbs_audio::{AudioEngine, CpalEngine, SoundBank, EXTENDED_MAX_VOICES, DEFAULT_MAX_VOICES};
use nbs_core::model::Note;
use nbs_core::{Document, Macro, TremoloFade};
use nbs_io::datapack::DatapackOptions;
use nbs_io::image_export::ImageOptions;
use nbs_io::midi::MidiImportOptions;
use nbs_io::prefs::{Preferences, RecentFile};
use nbs_io::schematic::SchematicOptions;

use crate::commands::Command;
use crate::interaction::Interaction;
use crate::theme::{Theme, ThemeKind};
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
            Self::Key => "音高",
            Self::Velocity => "力度",
            Self::Panning => "声像",
            Self::Pitch => "微调",
        }
    }
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
    /// 原版首选项里印的中文标签（`draw_window_preferences.gml` 外观页）。
    /// `ShowGridLines` 是原版没有的我们自己加的开关，所以名字留着。
    pub fn label(self) -> &'static str {
        match self {
            Self::ShowLayers => "显示每层操作区",
            Self::ShowPiano => "显示琴键",
            Self::UseColors => "彩色音符盒",
            Self::UseShapes => "音符盒形状",
            Self::UseIcons => "显示音色图标",
            Self::ShowNumbers => "显示音符序号",
            Self::ShowOctaves => "显示八度序号",
            Self::ShowKeyNames => "显示键名",
            Self::ShowKeyNumbers => "显示音符序号",
            Self::ShowKeyboard => "显示键盘键位",
            Self::ShowOutOfRange => "标记超范围琴键",
            Self::ShowIncompatible => "标记不兼容的音符盒",
            Self::Fade => "音符盒不透明",
            Self::ShowGridLines => "显示网格线",
            Self::Flats => "使用降记号",
            Self::Blackout => "全黑模式",
            Self::RemoveEffect => "显示删除效果",
            Self::ShowNoteChart => "在琴键上时显示五线谱对应音符",
        }
    }
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
    SongInfo,
    Properties,
    Statistics,
    Preferences,
    Instruments,
    About,
    SetTempo,
    SetVelocity,
    SetPanning,
    SetPitch,
    Transpose,
    Tremolo,
    Stereo,
    ClipEditor,
    Shortcuts,
    FileOpen,
    FileSave,
    LayerRename(u32),
    ConfirmNew,
    AudioExport,
    AudioSettings,
    DatapackExport,
    MidiImport,
    SchematicExport,
    PngExport,
    MacroEditor,
    Autosaves,
    /// 原版 `w_minecraft`（`macros.gml:69`）：右上角兼容性图标点开的窗口。
    MinecraftCompat,
    /// 原版 `w_edit_tempo_changer`（`add/change_block_manual` 落速度变换时弹出），
    /// 参数是目标格 (tick, layer)。
    EditTempoChanger(u32, u32),
    /// 原版 `w_edit_sound_stopper`：落消音器时弹出，参数同上。
    EditSoundStopper(u32, u32),
    /// 原版 `w_greeting`（`macros.gml:105`）：开机欢迎窗（`show_welcome` 时打开）。
    /// 原版 `draw_window` 没有标题文字，这里也画成无标题栏的 700x430 窗。
    Greeting,
}

impl WindowKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::SongInfo => "歌曲信息",
            Self::Properties => "歌曲属性",
            Self::Statistics => "歌曲统计",
            Self::Preferences => "首选项",
            Self::Instruments => "音色设置",
            Self::About => "关于 nbs-rust",
            Self::SetTempo => "设置速度",
            Self::SetVelocity => "批量设置力度",
            Self::SetPanning => "批量设置声像",
            Self::SetPitch => "批量设置微调",
            Self::Transpose => "移调",
            Self::Tremolo => "颤音 / Tremolo",
            Self::Stereo => "立体声 / Stereo",
            Self::ClipEditor => "片段编辑器",
            Self::Shortcuts => "快捷键一览",
            Self::FileOpen => "打开歌曲",
            Self::FileSave => "保存歌曲",
            Self::LayerRename(_) => "重命名图层",
            Self::ConfirmNew => "新建歌曲",
            Self::AudioExport => "音频导出",
            Self::AudioSettings => "音频设置",
            Self::DatapackExport => "导出为数据包",
            // 原版 `draw_window_midi_import.gml` 中文分支画的是 `"导入MIDI"`。
            Self::MidiImport => "导入MIDI",
            Self::SchematicExport => "导出 Minecraft 原理图",
            Self::PngExport => "导出 PNG 谱面",
            Self::MacroEditor => "宏",
            Self::Autosaves => "自动保存 / 备份",
            // 原版 `draw_window_minecraft.gml:24` 的标题文字。
            Self::MinecraftCompat => "Minecraft 兼容性",
            // 原版两个编辑窗在标题带写的文字（语言 1 = 中文分支）。
            Self::EditTempoChanger(..) => "设置速度",
            Self::EditSoundStopper(..) => "设置抑制范围",
            // 原版欢迎窗没有标题文字（draw_window 只画边框），这里仅供窗口 id 用。
            Self::Greeting => "Note Block Studio",
        }
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

/// 原版的 `message()` / `question()` 模态框。`yes_no` 对应 `question()`，
/// 答「否」会丢掉整个待弹队列（即 `calculate_size()` 返回 0，不打开导出窗口）。
#[derive(Clone, Debug)]
pub struct Modal {
    pub title: &'static str,
    pub text: String,
    pub yes_no: bool,
}

/// 一组待弹的模态框，队列走完后执行 `then`。
#[derive(Clone, Debug, Default)]
pub struct ModalQueue {
    pub items: std::collections::VecDeque<Modal>,
    pub then: Option<Command>,
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
            items: [Modal { title, text: text.into(), yes_no: true }].into_iter().collect(),
            then: Some(then),
        }
    }

    /// 单个原版 `message(正文, 标题)`。
    pub fn message(title: &'static str, text: &str) -> Self {
        Self {
            items: [Modal { title, text: text.into(), yes_no: false }].into_iter().collect(),
            then: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {    Grid,
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
            Self::Song => "歌曲路径: ",
            Self::Pattern => "片段路径: ",
        }
    }

    /// 原版「修改」按钮里先弹的那句 `message(…, "")`。
    pub fn tip(self) -> &'static str {
        match self {
            Self::Song => "选择保存和打开的文件夹",
            Self::Pattern => "选择导出和导入片段的文件夹",
        }
    }

    /// 原版系统对话框的标题（`draw_window_preferences.gml:278/294` 的第 4 个参数）。
    pub fn title(self) -> &'static str {
        match self {
            Self::Song => "歌曲文件夹",
            Self::Pattern => "片段文件夹",
        }
    }

    /// 原版传给 `get_save_filename_ext` 的默认文件名（第 2 个参数）。
    /// 原版并不真去创建这个文件：它只要用户按下「保存」那一刻对话框所在的目录
    /// （`songfolder = filename_dir(a)`），所以这个名字纯粹是占位 ——
    /// 但它确实显示在文件名栏里，照抄。
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Song => "选择歌曲文件夹",
            Self::Pattern => "选择片段文件夹",
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
    ExportPattern,
    ImportMidi,
    ImportPattern,
    ImportSchematic,
    /// 原版 `datapack_getextranotes()`：保存随程序那份「更多音符盒」资源包。
    SaveExtraNotes,
    /// 原版「修改」按钮：`get_save_filename_ext("", "选择歌曲文件夹", songfolder, "歌曲文件夹")`。
    /// 原版取的是那个假文件名所在的目录，所以这里确定后认的是当前目录本身。
    PickFolder(PrefsFolder),
}

impl DialogAction {
    pub fn is_save(self) -> bool {
        matches!(
            self,
            Self::SaveSong | Self::ExportAudio | Self::ExportDatapack | Self::ExportMidi
                | Self::ExportPng | Self::ExportSchematic | Self::ExportPattern
                | Self::SaveExtraNotes
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
            Self::ExportMidi | Self::ImportMidi => "mid",
            Self::ExportPng => "png",
            // 原版：get_open_filename_ext("Minecraft Schematics (*.schematic)|*.schematic", …)。
            Self::ExportSchematic | Self::ImportSchematic => "schematic",
            Self::ExportPattern | Self::ImportPattern => "nbp",
            // 原版：get_save_filename_ext("Resource pack (*.zip)|*.zip", "更多音符盒", "", …)。
            Self::SaveExtraNotes => "zip",
            // 原版这两个对话框的过滤器是空字符串：不做过滤，选的是目录。
            Self::PickFolder(_) => "",
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
                p.with_extension(ext).file_name().map(|s| s.to_string_lossy().into_owned())
            };
            if let Some(f) = renamed {
                self.name = f;
            }
        }
        self.refresh();
    }

    pub fn refresh(&mut self) {
        let ext = self.ext;
        self.entries = std::fs::read_dir(&self.dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                // 空扩展名 = 原版导出目录时的空过滤器：什么都列出来，名字当作目录。
                ext.is_empty() || p.is_dir() || p.extension().and_then(|s| s.to_str()) == Some(ext)
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
            // 原版 `load_song.gml:12` 的过滤器还有 `Zipped Files (*.zip)` 和
            // `MIDI Sequences (*.mid)`；这两条路我们还没接通（带自定义音色的 .nbs 压缩包
            // 读不了，.mid 要先开 MIDI 导入窗口），列出来只会让人选了以后报错，所以先不列。
            DialogAction::OpenSong => ("打开歌曲", vec![
                filter("Note Block Songs (*.nbs)", "*.nbs"),
                filter("Minecraft Schematics (*.schematic)", "*.schematic"),
            ]),
            DialogAction::SaveSong => ("保存歌曲", vec![
                filter("Note Block Songs (*.nbs)", "*.nbs"),
            ]),
            // 原版 `audio_export.gml:24`：`audio_exp_format + " files (*" + output_ext + ")"`。
            DialogAction::ExportAudio => ("导出音频文件", vec![
                filter(&format!("{} files (*.{ext})", ext.to_uppercase()), &format!("*.{ext}")),
            ]),
            DialogAction::ExportDatapack => {
                if ext.is_empty() {
                    // 原版 `datapack_export.gml:12` 的过滤器串只有一个词、没有 `|通配符`，
                    // 传进系统组件就是一个空白模式；导出的目录名来自文件名栏，不是选中的目录。
                    ("导出数据包", vec![filter("数据包目录", "")])
                } else {
                    ("导出数据包", vec![filter("ZIP archive (*.zip)", "*.zip")])
                }
            }
            DialogAction::ExportMidi => ("导出 MIDI", vec![
                filter("MIDI Sequences (*.mid)", "*.mid"),
            ]),
            DialogAction::ExportPng => ("导出图片", vec![
                filter("PNG image (*.png)", "*.png"),
            ]),
            DialogAction::ExportSchematic => {
                // 原版 `schematic_export.gml:5-6`：结构方块存的是 .nbt，过滤器名也不同。
                if ext == "nbt" {
                    ("导出 Schematic", vec![filter("Minecraft Structures (*.nbt)", "*.nbt")])
                } else {
                    ("导出 Schematic", vec![filter("Minecraft Schematics (*.schematic)", "*.schematic")])
                }
            }
            DialogAction::ExportPattern => ("保存分段", vec![
                filter("Note Block Pattern (*.nbp)", "*.nbp"),
            ]),
            DialogAction::ImportMidi => ("从 MIDI 导入", vec![
                filter("MIDI Sequences (*.mid)", "*.midi;*.mid"),
            ]),
            DialogAction::ImportPattern => ("打开分段", vec![
                filter("Note Block Pattern (*.nbp)", "*.nbp"),
            ]),
            DialogAction::ImportSchematic => ("从 Schematic 导入", vec![
                filter("Minecraft Schematics (*.schematic)", "*.schematic"),
            ]),
            // 原版 `datapack_getextranotes.gml:6`：过滤器名和标题都是照抄的这一句。
            DialogAction::SaveExtraNotes => ("为数据包保存音符盒资源包", vec![
                filter("Resource pack (*.zip)", "*.zip"),
            ]),
            // 原版 `draw_window_preferences.gml:278/294` 是 `get_save_filename_ext("", …)`：
            // 过滤器整串为空，文件名栏塞的是占位串，按下保存后取的是它所在的目录
            // （换算回来见 `EditorState::run_pending_dialog`）。不是目录选择器。
            DialogAction::PickFolder(f) => (f.title(), Vec::new()),
        };
        PickerSpec { title: title.to_string(), filters }
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
}

fn filter(name: &str, patterns: &str) -> FilterSpec {
    FilterSpec { name: name.to_string(), patterns: patterns.to_string() }
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

pub struct EditorState {
    pub doc: Document,
    pub viewport: Viewport,
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
    pub context_menu: Option<ContextMenu>,
    pub message: Option<(String, Instant)>,
    /// 真实音频引擎（无音频设备时自动降级为静音时钟）。
    pub audio: Box<dyn AudioEngine>,
    pub prefs: Preferences,
    pub marker_follow: bool,
    /// 原版 `marker_pagebypage`：1 = 按页，0 = 按刻，2 = 按帧。
    pub follow_mode: u8,
    pub hover_cell: Option<(u32, u32)>,
    pub pending: Vec<Command>,
    pub file_dialog: Option<FileDialog>,
    /// 宿主装进来的原生选择器（Windows 上是 `IFileOpenDialog` / `IFileSaveDialog`）。
    pub picker: Option<std::sync::Arc<dyn FilePicker>>,
    /// 等着被原生选择器处理的请求。在帧外（`raw_input_hook`）取走，避免在 egui 帧内
    /// 起一个会泵消息的模态框。
    pub pending_dialog: Option<FileDialog>,
    /// 原版 `recent_song[0..10]`（路径 + 打开时间）。
    pub recent_files: Vec<RecentPath>,
    /// 开机更新检查的共享结果。没开「检查更新」时状态一直是 0（原版 `update_http = -1`）。
    pub update_check: SharedUpdateCheck,
    /// 原版在 check_updates 的回调里 `question(下载？)` 只问一次：这里就是那道闸。
    pub update_question: bool,
    /// 原版 `update_success`（`control_create.gml:747-753`）：上次运行版本和当前不同时
    /// 置 1，欢迎窗显示「更新成功！」并重新亮出捐赠横幅。
    pub update_success: bool,
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
    pub macro_amount: f32,
    pub macro_index: usize,
    pub macro_pattern: String,
    pub macro_reverse: bool,
    pub midi_options: MidiImportOptions,
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
    /// 原版 `calculate_size()` 每次打开导出窗口时重算的歌曲统计。
    pub schematic_stats: nbs_io::schematic::SchematicStats,
    /// 原版 `selected_tab_sch`：0 = 设计，1 = 方块。关窗时回到 0。
    pub schematic_tab: u8,
    /// 原版 `sch_exp_remember`。
    pub schematic_remember: bool,
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
    pub last_autosave: Instant,
    pub last_prefs_save: Instant,
    /// 已同步给音频引擎的 `Song::rev`，用于只在内容真变化时重建事件表。
    pub audio_rev: u64,
    /// 已经交给窗口管理器的图标档位（原版 `window_icon`）。`None` = 还没发过，
    /// 对应原版 `icon_display` 初值为 1：第一帧必发一次。
    pub applied_window_icon: Option<bool>,
    /// 原版精灵图（工具栏 / 图层栏 / 宏条的图标按钮）。
    pub icons: crate::icons::Icons,
    /// 原版 `showinsbox`：工具栏音色框是否展开成整窗网格。
    pub insbox_open: bool,
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
    /// 原版 `tempodrag`：拖速度框时的未量化累加值。
    pub tempo_drag: Option<f32>,
    /// 兼容性指示（原版 `compatible`），随 `song.rev` 重算。
    pub compat_rev: u64,
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
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

/// 原版 Schematic 只认这三档速度（`compatible = 1` 的前半个条件，也是兼容性窗口
/// Schematic 页第一行的判据——那一行不看音符微调）。
pub fn tempo_schematic_ok(tempo: f32) -> bool {
    [10.0_f32, 5.0, 2.5].iter().any(|v| (tempo - v).abs() < 1e-3)
}

/// 原版数据包页第一行说「更佳」的那十档。数据包本身任意速度都能导，
/// 所以差档只给橙色图标，不给红叉。
pub fn tempo_datapack_good(tempo: f32) -> bool {
    [20.0_f32, 10.0, 5.0, 4.0, 2.5, 2.0, 1.25, 1.0, 0.5, 0.25]
        .iter()
        .any(|v| (tempo - v).abs() < 1e-3)
}

impl EditorState {
    pub fn new() -> Self {
        let prefs = Preferences::load();
        let song = nbs_core::model::Song::demo();
        let bank = SoundBank::load_auto(prefs.sounds_dir_path().as_deref());
        let mut audio: Box<dyn AudioEngine> = Box::new(CpalEngine::with_bank(bank));
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
            .map(|r| RecentPath { path: PathBuf::from(&r.path), time: r.time })
            .collect();
        let follow_marker = prefs.follow_marker;
        let follow_mode = prefs.follow_mode;
        let png_dark = theme.kind.is_dark();
        // 原版 `load_settings` 的 `[schematic_export]` 段。
        let schematic_options = prefs.schematic.clone();
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

        let mut state = Self {
            doc: Document::with_song(song),
            viewport,
            interaction: Interaction::default(),
            tool: EditMode::Key,
            current: Note::default(),
            flags,
            theme,
            fullscreen: false,
            window: None,
            window_prev: None,
            dialog_return: None,
            context_menu: None,
            message: None,
            audio,
            prefs,
            marker_follow: follow_marker,
            follow_mode,
            hover_cell: None,
            pending: Vec::new(),
            file_dialog: None,
            picker: None,
            pending_dialog: None,
            recent_files,
            update_check: std::sync::Arc::new(std::sync::Mutex::new(UpdateCheck::default())),
            update_question: false,
            update_success: false,
            donate_banner: false,
            editing_layer_name: None,
            event_edit: EventEditDraft::default(),
            layer_drag: None,
            remove_effects: Vec::new(),
            macro_amount: 1.0,
            macro_index: 0,
            macro_pattern: "0|4|7|12".into(),
            macro_reverse: false,
            midi_options,
            midi_data: None,
            midi_file: String::new(),
            midi_tab: 0,
            midi_scroll: [0.0; 4],
            midi_menu: None,
            midi_submenu: None,
            midi_drags: [0.0; 3],
            midi_gm,
            schematic_options,
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
            prefs_drag: 0.0,
            png_options: ImageOptions { dark: png_dark, ..ImageOptions::default() },
            export_status: None,
            piano_start: 12,
            piano_keys_shown: 0,
            piano_scroll_delay: 0.0,
            last_autosave: Instant::now(),
            last_prefs_save: Instant::now(),
            audio_rev: 0,
            applied_window_icon: None,
            icons: crate::icons::Icons::default(),
            insbox_open: false,
            insbox_start: 0,
            scrub: 0,
            record: false,
            record_armed: false,
            keys_held: [false; nbs_core::model::KEY_MAX as usize + 1],
            tempo_drag: None,
            compat_rev: u64::MAX,
            compat: 1,
            compat_outside: 0,
            compat_custom: 0,
            rainbow: false,
            rainbow_hue: 0.0,
            background_accent: false,
        };
        state.enter_startup();
        state
    }

    /// 原版 `control_create.gml` 结尾的开机流程：捐赠横幅按静音到期日重新亮出、
    /// 版本变化走「更新成功」、`show_welcome` 时打开欢迎窗。
    pub fn enter_startup(&mut self) {
        // `date_compare_date(date_current_datetime(), donate_banner_time) > 0`（:728-733）：
        // 当前日期晚于静音到期日才显示。donate_banner_time = 0 等价原版出厂的 −1（没静音过）。
        self.donate_banner = nbs_io::prefs::unix_now() > self.prefs.donate_banner_time;
        // :747-753 的 `vers_tmp != version` 分支：置 update_success 并强制亮横幅。
        // 原版还会切回 Fluent 主题并打开 w_update 窗口——我们没有更新历史文本，
        // 状态行放进欢迎窗（已记录的偏差）。
        let last = self.prefs.last_version.clone();
        if last.is_some() && last.as_deref() != Some(crate::VERSION) {
            self.update_success = true;
            self.donate_banner = true;
        }
        // :707 的 `if (show_welcome) window = w_greeting`。
        // 原版在版本变化时会用 w_update 顶掉欢迎窗；这里没有 w_update，欢迎窗保留。
        if self.prefs.show_welcome {
            self.window = Some(WindowKind::Greeting);
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
            self.window = Some(if save { WindowKind::FileSave } else { WindowKind::FileOpen });
        }
        self.file_dialog = Some(dialog);
        self.context_menu = None;
    }

    /// 执行挂起的原生选择请求。宿主在帧外（`raw_input_hook`）调，不要在 `update` 里调：
    /// 原生对话框会泵自己的消息循环，塞在 egui 帧中间不安全。
    /// 返回 true 表示真的处理了一个请求（调用方据此催一次重绘）。
    /// COM 起不来时回落到自绘选框，功能不断。
    pub fn run_pending_dialog(&mut self) -> bool {
        let Some(dialog) = self.pending_dialog.take() else { return false };
        let Some(picker) = self.picker.clone() else { return false };
        match picker.pick(&dialog) {
            PickResult::Picked(path) => {
                // 原版选文件夹取的是 `filename_dir(a)`（`draw_window_preferences.gml:279`）：
                // 对话框里那个占位名不算数，要的只是它所在的目录。系统组件交回来的是完整
                // 路径，这里先换算成同一个目录，`DialogConfirm` 两条路看到的形状才一致。
                let path = match dialog.action {
                    DialogAction::PickFolder(_) => {
                        path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
                    }
                    _ => path,
                };
                // `Command::DialogConfirm` 靠 `file_dialog.action` 认是哪一种导出。
                self.file_dialog = Some(dialog);
                self.emit(Command::DialogConfirm(path));
            }
            PickResult::Cancelled => {
                self.file_dialog = None;
            }
            PickResult::Unavailable => {
                // 自绘兜底框会把原窗口顶掉，记住顶掉的是哪个，选完放回去。
                self.dialog_return = self.window.clone();
                self.window = Some(if dialog.save { WindowKind::FileSave } else { WindowKind::FileOpen });
                self.file_dialog = Some(dialog);
            }
        }
        true
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
            // 背景界面产生的命令一律丢弃，只接受「回答当前模态框」这一条。
            if !self.modals.is_empty()
                && !matches!(cmd, crate::commands::Command::AnswerModal(_))
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
        self.audio
            .clock_mut()
            .set_loop(s.meta.loop_enabled, s.meta.loop_start as f64, end, s.meta.loop_max);
    }

    /// 原版 `control_draw.gml:11-12`：`rw = 窗口物理宽 / window_scale` —— 窗口尺寸不动，
    /// 逻辑分辨率变小，等于整块界面变大。egui 的 `pixels_per_point` 就是同一个数，
    /// 而 `zoom_factor` 是「相对系统 DPI 的倍率」，所以出厂值（`None`）对应 1.0。
    fn apply_ui_scale(&self, ctx: &Context) {
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let want = self.prefs.effective_scale(native) / native;
        // 拖动落点是整数百分比，和系统倍率比对时留一点余量，避免每帧都重设。
        if (ctx.zoom_factor() - want).abs() > 0.001 {
            ctx.set_zoom_factor(want);
        }
    }

    /// 原版 `control_draw.gml:736-751`：Ctrl+滚轮 / Ctrl+± 把 `window_scale` 推到
    /// 下一个（上一个）固定档位，档位表就是那串 `else if` 里的数，两端是 0.5 和 4。
    /// 网格本身恒 32px，缩放的是整块界面。
    pub fn step_window_scale(&mut self, ctx: &Context, up: bool) {
        const STEPS: [f32; 14] = [
            0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0,
        ];
        let cur = self.prefs.effective_scale(ctx.native_pixels_per_point().unwrap_or(1.0));
        let next = if up {
            STEPS.iter().find(|s| **s > cur + 1e-4).copied()
        } else {
            STEPS.iter().rev().find(|s| cur > **s + 1e-4).copied()
        };
        let Some(next) = next else { return };
        self.prefs.window_scale = Some(next);
        self.set_message(format!("窗口缩放 => {}%", (next * 100.0).round()));
    }

    /// 原版 `control_draw.gml:732-735`：Ctrl+0 回到 `get_default_window_scale()`（= 系统 DPI / 96）。
    pub fn reset_window_scale(&mut self, ctx: &Context) {
        self.prefs.window_scale = None;
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        self.set_message(format!("窗口缩放 => {}%", (native * 100.0).round()));
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
        let Some(icon) = self.icons.window_icon(want) else { return };
        ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(icon)));
        self.applied_window_icon = Some(want);
    }

    /// 原版 Ctrl+Shift+F5（仅 theme 3，`control_draw.gml:710-740`）：手动切换彩虹。
    /// 与 Toggle Rainbow 事件乐器的差异：这里要发消息，且只在关掉时重置表
    /// （原版 `rainbowtoggle = !rainbowtoggle` → 开：只 set_msg，关：set_msg + draw_accent_init）。
    pub fn toggle_rainbow(&mut self) {
        self.rainbow = !self.rainbow;
        if self.rainbow {
            self.set_message("炫彩模式 => ON");
        } else {
            self.set_message("炫彩模式 => OFF");
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
        // 原版 else-if 链里「改界面」的事件乐器副作用（control_step.gml:283-296）
        // 由音频调度发回来，这里统一落地。彩虹开/关都重置表且不发消息，照抄原版。
        let mut accent_changed = false;
        for ev in self.audio.take_visual_events() {
            match ev {
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
        if playing && self.marker_follow {
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
            self.theme.set_accent_table(Theme::hsb_to_rgb(self.rainbow_hue, 100.0, 100.0));
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
        // 定时自动保存（只写备份，不改动当前文件名与脏标记）
        let mins = self.prefs.autosave_interval() as u64;
        if mins > 0
            && self.doc.song.dirty
            && self.last_autosave.elapsed().as_secs() >= mins * 60
        {
            self.last_autosave = Instant::now();
            if let Ok(p) = nbs_io::prefs::autosave(&self.doc.song, &self.prefs) {
                self.set_message(format!("已自动备份 {}", p.file_name().unwrap_or_default().to_string_lossy()));
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

    fn refresh_compat(&mut self) {
        if !self.doc.selection.notes.is_empty() || self.doc.song.rev == self.compat_rev {
            return;
        }
        self.compat_rev = self.doc.song.rev;
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
            let t = self.doc.song.meta.tempo;
            if tempo_schematic_ok(t) && !pitched {
                1
            } else {
                2
            }
        };
    }

    pub fn set_marker(&mut self, tick: f64) {
        self.audio.clock_mut().seek(tick);
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
    /// 再按落点列起播（取落点速度并整列派发，见 [`AudioEngine::start`]）。
    /// 「播放 / 暂停」与录音待命首键两条起播路径都必须走这里。
    pub fn begin_playback(&mut self) {
        let end_stop = self.doc.song.song_length() as f64
            + nbs_audio::scheduler::END_STOP_MARGIN_TICKS;
        if self.audio.clock().position() >= end_stop {
            self.audio.clock_mut().seek(0.0);
        }
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
                format!("自定义音色 {}", j + 1)
            } else {
                ci.name.clone()
            };
            rows.push((idx, name));
        }
        rows
    }

    pub fn add_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p.path != path);
        self.recent_files.insert(0, RecentPath { path: path.clone(), time: nbs_io::prefs::unix_now() });
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
    pub fn apply_audio_prefs(&mut self) {
        apply_audio_prefs(&mut *self.audio, &self.prefs);
        // 实时播放此前从不遵守"包含锁定层"，只有导出遵守；这里补齐。
        let include = self.prefs.include_locked_in_export;
        self.audio.clock_mut().set_include_locked(include, &self.doc.song);
        self.audio_rev = self.doc.song.rev;
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
            .map(|r| RecentFile { path: r.path.to_string_lossy().into_owned(), time: r.time })
            .collect();
    }

    pub fn save_prefs(&mut self) {
        self.sync_prefs();
        self.last_prefs_save = Instant::now();
        if let Err(e) = self.prefs.save() {
            self.set_message(format!("保存偏好失败: {e}"));
        }
    }

    /// 每 30 秒静默持久化一次，避免意外退出丢失主题/音频设置。
    pub fn maybe_save_prefs(&mut self) {
        if self.last_prefs_save.elapsed().as_secs() >= 30 {
            self.sync_prefs();
            self.last_prefs_save = Instant::now();
            let _ = self.prefs.save();
        }
    }

    /// 按宏窗口当前参数构造一个宏。
    pub fn current_macro(&self) -> Macro {
        let a = self.macro_amount;
        let pattern: Vec<i32> = self
            .macro_pattern
            .split('|')
            .filter_map(|s| s.trim().parse::<i32>().ok())
            .collect();
        match self.macro_index {
            0 => Macro::FadeIn,
            1 => Macro::FadeOut,
            2 => Macro::VelocityLfo,
            3 => Macro::Vibrato,
            4 => Macro::Reset,
            5 => Macro::ReplaceKey,
            6 => Macro::Tremolo {
                spacing: a.clamp(1.0, 8.0) as u32,
                fade: if self.macro_reverse { TremoloFade::Out } else { TremoloFade::None },
                amount: a as i32,
            },
            7 => Macro::Stereo { width: a.clamp(0.0, 100.0) as u8, reverse: self.macro_reverse },
            8 => Macro::Arpeggio { pattern },
            9 => Macro::Portamento { cents: a as i32, reverse: self.macro_reverse },
            10 => Macro::Stagger { pattern: pattern.iter().map(|&x| x.max(0) as u32).collect() },
            11 => Macro::Chorus { detune: a as i32 },
            12 => Macro::SetVelocity { value: a.clamp(0.0, 100.0) as u8, percent: self.macro_reverse },
            13 => Macro::SetPanning { value: a.clamp(-100.0, 100.0) as i16 },
            14 => Macro::SetPitch { value: a as i16, additive: self.macro_reverse },
            15 => Macro::SetInstrument { value: a.clamp(0.0, 15.0) as u8 },
            _ => Macro::Reset,
        }
    }

    pub fn song_title(&self) -> String {
        let name = if self.doc.song.meta.name.is_empty() {
            self.doc
                .song
                .filename
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "未命名".to_string())
        } else {
            self.doc.song.meta.name.clone()
        };
        let mark = if self.doc.song.dirty { " *" } else { "" };
        format!("{name}{mark} — nbs-rust")
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

    /// 原生选择器的文案必须和原版传给 `get_*_filename_ext` 的参数一致。
    #[test]
    fn picker_spec_carries_the_original_titles_and_filters() {
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
        assert_eq!(sch.picker_spec().filters[0].name, "Minecraft Structures (*.nbt)");

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
            assert!(!tempo_schematic_ok(t), "数据包的好速度不等于能导 Schematic: {t}");
            assert!(tempo_datapack_good(t), "{t} 应是数据包的好速度");
        }
        for t in [23.45_f32, 9.0, 3.0, 1.5, 0.3] {
            assert!(!tempo_schematic_ok(t), "{t} 不该被判成能导 Schematic");
            assert!(!tempo_datapack_good(t), "{t} 不该被判成好速度");
        }
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
        let picker = std::sync::Arc::new(FakePicker { answer, seen: std::sync::Mutex::new(Vec::new()) });
        let mut st = EditorState::new();
        st.picker = Some(picker.clone());
        // 开机流程会开欢迎窗；选框测试的靶子是「欢迎窗已关」的裸编辑器。
        st.window = None;
        (st, picker)
    }

    /// 装了原生选择器：请求挂到帧外，回来后直接走 `DialogConfirm`，全程不开自绘窗口。
    #[test]
    fn native_picker_answers_without_the_in_app_dialog() {
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
        assert_eq!(picker.seen.lock().unwrap().as_slice(), &["保存歌曲".to_string()]);
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
        assert_eq!(st.prefs.song_folder.as_deref(), Some(dir.to_str().unwrap()), "实测 {picked:?}");
        assert_eq!(st.window, Some(WindowKind::Preferences));
    }
}
