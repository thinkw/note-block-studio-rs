//! 编辑器总状态。原版是 `obj_controller` 单实例的几百个实例变量；
//! 这里收敛成结构化字段，并保留 `window / select / select_press*` 的状态机语义。

use std::path::PathBuf;
use std::time::Instant;

use egui::{Context, Pos2};
use nbs_audio::{AudioEngine, CpalEngine, SoundBank, EXTENDED_MAX_VOICES, DEFAULT_MAX_VOICES};
use nbs_core::model::Note;
use nbs_core::{Document, Macro, TremoloFade};
use nbs_io::image_export::ImageOptions;
use nbs_io::midi::MidiImportOptions;
use nbs_io::prefs::Preferences;
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
    ShowInstruments,
    UseColors,
    UseShapes,
    UseIcons,
    ShowNumbers,
    ShowOctaves,
    ShowKeyNames,
    ShowIncompatible,
    ShowGridLines,
    Flats,
    Blackout,
}

impl Flag {
    pub fn label(self) -> &'static str {
        match self {
            Self::ShowLayers => "显示图层栏",
            Self::ShowPiano => "显示钢琴键盘",
            Self::ShowInstruments => "显示音色面板",
            Self::UseColors => "彩色方块",
            Self::UseShapes => "乐器形状",
            Self::UseIcons => "乐器图标",
            Self::ShowNumbers => "显示属性数值",
            Self::ShowOctaves => "显示八度",
            Self::ShowKeyNames => "显示音名",
            Self::ShowIncompatible => "标记不兼容音符",
            Self::ShowGridLines => "显示网格线",
            Self::Flats => "使用降号音名",
            Self::Blackout => "黑场模式",
        }
    }
    pub fn get(self, f: &UiFlags) -> bool {
        match self {
            Self::ShowLayers => f.show_layers,
            Self::ShowPiano => f.show_piano,
            Self::ShowInstruments => f.show_instruments,
            Self::UseColors => f.use_colors,
            Self::UseShapes => f.use_shapes,
            Self::UseIcons => f.use_icons,
            Self::ShowNumbers => f.show_numbers,
            Self::ShowOctaves => f.show_octaves,
            Self::ShowKeyNames => f.show_keynames,
            Self::ShowIncompatible => f.show_incompatible,
            Self::ShowGridLines => f.show_grid_lines,
            Self::Flats => f.flats,
            Self::Blackout => f.blackout,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiFlags {
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
}

impl Default for UiFlags {
    fn default() -> Self {
        Self {
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
        }
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
    MidiImport,
    SchematicExport,
    PngExport,
    MacroEditor,
    Autosaves,
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
            Self::AudioExport => "导出音频 (WAV)",
            Self::AudioSettings => "音频设置",
            Self::MidiImport => "从 MIDI 导入",
            Self::SchematicExport => "导出 Minecraft 原理图",
            Self::PngExport => "导出 PNG 谱面",
            Self::MacroEditor => "宏",
            Self::Autosaves => "自动保存 / 备份",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Grid,
    Timeline,
    Layer(u32),
}

#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub pos: Pos2,
    pub cell: (u32, u32),
    pub kind: ContextKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogAction {
    OpenSong,
    SaveSong,
    ExportAudio,
    ExportMidi,
    ExportPng,
    ExportSchematic,
    ExportPattern,
    ImportMidi,
    ImportPattern,
}

impl DialogAction {
    pub fn is_save(self) -> bool {
        matches!(
            self,
            Self::SaveSong | Self::ExportAudio | Self::ExportMidi | Self::ExportPng
                | Self::ExportSchematic | Self::ExportPattern
        )
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::OpenSong | Self::SaveSong => "nbs",
            Self::ExportAudio => "wav",
            Self::ExportMidi | Self::ImportMidi => "mid",
            Self::ExportPng => "png",
            Self::ExportSchematic => "schematic",
            Self::ExportPattern | Self::ImportPattern => "nbp",
        }
    }
    pub fn default_name(self, base: &str) -> String {
        format!("{}.{}", base, self.extension())
    }
}

#[derive(Clone, Debug)]
pub struct FileDialog {
    pub save: bool,
    pub action: DialogAction,
    pub dir: PathBuf,
    pub name: String,
    pub entries: Vec<PathBuf>,
    pub error: Option<String>,
}

impl FileDialog {
    pub fn new(save: bool, initial: Option<&PathBuf>) -> Self {
        Self::for_action(
            if save { DialogAction::SaveSong } else { DialogAction::OpenSong },
            initial,
        )
    }

    pub fn for_action(action: DialogAction, initial: Option<&PathBuf>) -> Self {
        let dir = initial
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .filter(|d| d.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let base = initial
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        let name = if action.is_save() {
            action.default_name(&base)
        } else {
            initial
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut d = Self { save: action.is_save(), action, dir, name, entries: Vec::new(), error: None };
        d.refresh();
        d
    }

    pub fn refresh(&mut self) {
        let ext = self.action.extension();
        self.entries = std::fs::read_dir(&self.dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                p.is_dir() || p.extension().and_then(|s| s.to_str()) == Some(ext)
            })
            .map(|e| e.path())
            .collect();
        self.entries.sort();
    }

    pub fn selected_path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayerDrag {
    pub index: u32,
    /// false = 音量, true = 声像
    pub stereo: bool,
    pub value: u8,
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
    pub context_menu: Option<ContextMenu>,
    pub message: Option<(String, Instant)>,
    /// 真实音频引擎（无音频设备时自动降级为静音时钟）。
    pub audio: Box<dyn AudioEngine>,
    pub prefs: Preferences,
    pub marker_follow: bool,
    pub follow_center: bool,
    pub hover_cell: Option<(u32, u32)>,
    pub pending: Vec<Command>,
    pub file_dialog: Option<FileDialog>,
    pub recent_files: Vec<PathBuf>,
    pub editing_layer_name: Option<(u32, String)>,
    /// 图层音量/声像拖拽预览（松开时才提交一条历史，避免拖拽产生几百条）。
    pub layer_drag: Option<LayerDrag>,
    pub macro_amount: f32,
    pub macro_index: usize,
    pub macro_pattern: String,
    pub macro_reverse: bool,
    pub midi_options: MidiImportOptions,
    pub schematic_options: SchematicOptions,
    pub png_options: ImageOptions,
    pub export_status: Option<String>,
    pub piano_start: u8,
    pub last_autosave: Instant,
    pub last_prefs_save: Instant,
    /// 已同步给音频引擎的 `Song::rev`，用于只在内容真变化时重建事件表。
    pub audio_rev: u64,
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorState {
    pub fn new() -> Self {
        let prefs = Preferences::load();
        let song = nbs_core::model::Song::demo();
        let bank = SoundBank::load_auto(prefs.sounds_dir_path().as_deref());
        let mut audio: Box<dyn AudioEngine> = Box::new(CpalEngine::with_bank(bank));
        audio.prepare(&song);
        apply_audio_prefs(&mut *audio, &prefs);

        let theme = Theme::new(match prefs.theme {
            0 => ThemeKind::Aqua,
            1 => ThemeKind::Retro,
            2 => ThemeKind::Dark,
            _ => ThemeKind::Fluent,
        });
        let mut flags = UiFlags::default();
        flags.show_layers = prefs.show_layers;
        flags.show_piano = prefs.show_piano;
        flags.show_instruments = prefs.show_instruments;
        flags.use_colors = prefs.use_colors;
        flags.use_shapes = prefs.use_shapes;
        flags.use_icons = prefs.use_icons;
        flags.show_numbers = prefs.show_numbers;
        flags.show_octaves = prefs.show_octaves;
        flags.show_keynames = prefs.show_keynames;
        flags.show_incompatible = prefs.show_incompatible;
        flags.show_grid_lines = prefs.show_grid_lines;
        flags.flats = prefs.flats;
        flags.blackout = prefs.blackout;

        let mut viewport = Viewport::default();
        viewport.zoom = prefs.zoom.clamp(0.5, 4.0);
        let recent_files = prefs.recent_files.iter().map(PathBuf::from).collect();
        let follow_marker = prefs.follow_marker;
        let follow_center = prefs.follow_center;
        let png_dark = theme.kind.is_dark();

        Self {
            doc: Document::with_song(song),
            viewport,
            interaction: Interaction::default(),
            tool: EditMode::Key,
            current: Note::default(),
            flags,
            theme,
            window: None,
            context_menu: None,
            message: None,
            audio,
            prefs,
            marker_follow: follow_marker,
            follow_center,
            hover_cell: None,
            pending: Vec::new(),
            file_dialog: None,
            recent_files,
            editing_layer_name: None,
            layer_drag: None,
            macro_amount: 1.0,
            macro_index: 0,
            macro_pattern: "0|4|7|12".into(),
            macro_reverse: false,
            midi_options: MidiImportOptions::default(),
            schematic_options: SchematicOptions::default(),
            png_options: ImageOptions { dark: png_dark, ..ImageOptions::default() },
            export_status: None,
            piano_start: 33,
            last_autosave: Instant::now(),
            last_prefs_save: Instant::now(),
            audio_rev: 0,
        }
    }

    pub fn emit(&mut self, cmd: Command) {
        self.pending.push(cmd);
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

    /// 每帧同步独立播放时钟。真实播放时位置由音频回调的采样计数推进，
    /// UI 只读；无音频设备时才由内部 `Instant` 时钟代理。
    /// **绝不在这里写 tempo/loop** —— 否则会覆盖 Tempo Changer 的运行时变速。
    pub fn tick_transport(&mut self, ctx: &Context) {
        let _crossed = self.audio.clock_mut().update();
        let playing = self.audio.clock().is_playing();
        let position = self.audio.clock().position();
        if playing && self.marker_follow {
            self.viewport.follow_marker(position, self.follow_center);
        }
        if playing {
            ctx.request_repaint();
        }
        // 定时自动保存（只写备份，不改动当前文件名与脏标记）
        let mins = self.prefs.auto_save_minutes as u64;
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

    pub fn add_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path.clone());
        self.recent_files.truncate(11);
        self.prefs.add_recent(&path);
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
        self.prefs.zoom = self.viewport.zoom;
        self.prefs.follow_marker = self.marker_follow;
        self.prefs.follow_center = self.follow_center;
        self.prefs.show_layers = self.flags.show_layers;
        self.prefs.show_piano = self.flags.show_piano;
        self.prefs.show_instruments = self.flags.show_instruments;
        self.prefs.use_colors = self.flags.use_colors;
        self.prefs.use_shapes = self.flags.use_shapes;
        self.prefs.use_icons = self.flags.use_icons;
        self.prefs.show_numbers = self.flags.show_numbers;
        self.prefs.show_octaves = self.flags.show_octaves;
        self.prefs.show_keynames = self.flags.show_keynames;
        self.prefs.show_incompatible = self.flags.show_incompatible;
        self.prefs.show_grid_lines = self.flags.show_grid_lines;
        self.prefs.flats = self.flags.flats;
        self.prefs.blackout = self.flags.blackout;
        self.prefs.recent_files = self.recent_files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
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
