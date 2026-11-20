//! UI 命令总线：菜单、工具栏、快捷键、上下文菜单、画布交互都只 emit 命令，
//! 由 `dispatch` 统一落地，避免状态散落在各个绘制函数里。

use std::path::PathBuf;

use egui::Context;
use nbs_audio::{export_wav, ExportOptions, DEFAULT_MAX_VOICES, EXTENDED_MAX_VOICES};
use nbs_core::model::{
    panning_velocity_to_short, short_to_panning_velocity, LockMode, Note, Song, SongMeta, KEY_MAX,
    PAN_MAX, PIT_MAX, VEL_MAX,
};
use nbs_core::Macro;

use crate::state::{
    DialogAction, EditMode, EditorState, FileDialog, Flag, Modal, ModalQueue, PrefsFolder,
    WindowKind,
};
use crate::theme::{Theme, ThemeKind};
use crate::tr;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    New,
    ConfirmNew,
    Open,
    Save,
    SaveAs,
    OpenPath(PathBuf),
    SavePath(PathBuf),
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    PasteText(String),
    Delete,
    SelectAll,
    Deselect,
    InvertSelection,
    SelectByInstrument {
        instrument: u8,
        invert: bool,
    },
    PlaceNote {
        tick: u32,
        layer: u32,
    },
    /// 速度变换编辑窗「确定」：pit = BPM（原版 `h_changeblock`）。
    SetEventTempo {
        tick: u32,
        layer: u32,
        bpm: i64,
    },
    /// 消音器编辑窗「确定」：pit = 起始层，终止层编码进声像 + 力度。
    SetEventStopper {
        tick: u32,
        layer: u32,
        start: i64,
        until: u64,
    },
    RemoveNote {
        tick: u32,
        layer: u32,
    },
    PickNote {
        tick: u32,
        layer: u32,
        all: bool,
    },
    SelectRect {
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        copy: bool,
        additive: bool,
        subtract: bool,
    },
    AddLayer,
    InsertLayer(u32),
    RemoveLayer(u32),
    MoveLayerUp(u32),
    MoveLayerDown(u32),
    RenameLayer(u32, String),
    SetLayerLock(u32, LockMode),
    SetLayerVolume(u32, u8),
    SetLayerStereo(u32, u8),
    SelectLayer(u32),
    SetInstrument(u8),
    SetEditMode(EditMode),
    SetKey(i32),
    /// 原版点琴键：`selected_key = c` 同时把力度 / 平衡 / 音高偏移复位成 100/100/0，并试听。
    SelectKey(u8),
    /// 原版 `draw_piano` 结尾的按键轮询：录音开着就直接往当前列写音符，否则只试听。
    KeyPress(u8),
    /// 原版工具栏「录制按键」：`playing = 0.25 record = !record`。
    ToggleRecord,
    SetVel(i32),
    SetPan(i32),
    SetPit(i32),
    /// 原版 `control_draw.gml:736-751`：Ctrl+滚轮 / Ctrl+± 沿固定档位改整窗口缩放
    /// （`window_scale`）。端口按用户要求只缩画布：倍率落到 `viewport.scale`
    /// （每格 `32 × scale` px），菜单 / 工具栏 / 状态栏 / 钢琴栏不动。
    WindowScaleUp,
    WindowScaleDown,
    /// 端口自加（用户要求）：琴键面板独立缩放档位，指针悬停在琴键面板上时
    /// Ctrl+滚轮 / Ctrl+± 改 `piano_scale`，不影响其他面板。
    PianoScaleUp,
    PianoScaleDown,
    /// 原版 `control_draw.gml:732-735`：Ctrl+0 回到出厂缩放 `get_default_window_scale()`。
    WindowScaleReset,
    /// 按页平移视口（Home/End/PageUp/PageDown）。
    PageHorizontal(i32),
    PageVertical(i32),
    PlayPause,
    Stop,
    SeekHome,
    SeekEnd,
    ToggleLoop,
    ToggleFollow,
    ShowWindow(WindowKind),
    CloseWindow,
    SetTheme(ThemeKind),
    ToggleFlag(Flag),
    /// 原版 Ctrl+Shift+F5（仅 theme 3）：切换界面强调色彩虹循环。
    ToggleRainbow,
    /// 原版 F11：`fullscreen = !fullscreen`（`control_draw.gml:122-134`）。
    ToggleFullscreen,
    Transpose(i32),
    SetAllInstrument(u8),
    BatchVelocity(u8),
    BatchPanning(u8),
    BatchPitch(i16),
    SetTempo(f32),
    SetTimeSignature(u8),
    /// 原版文件菜单「保存选项......」（`menu_click.gml:46-49`）：先把全局
    /// `save_options_version` 播种成歌曲版本，再开窗。
    OpenSaveOptions,
    /// 保存选项窗口的确定：`version` 是夹过之后的单选值。
    /// 和程序自身版本不同就先问一句（原版 `question(...)`），答否只把单选
    /// 退回歌曲版本、窗口不关；答是（或本来就相同）才写回歌曲并关窗。
    ApplySaveOptions {
        version: u8,
    },
    /// 上面那问的「是」分支（版本相同则直接走这条）：写回歌曲并关窗。
    SaveOptionsConfirm {
        version: u8,
    },
    ClearSong,
    /// 原版 Ctrl+Shift+ 字母开窗口那一族（`control_draw.gml:765-831`）：
    /// 片段编辑器 P、速度测量器 I、宏窗口 A/S/D/F/H、批量设置 R/T/Y——
    /// 每一行第一句都是 `playing = 0`。菜单和工具栏入口不一样
    /// （`menu_click.gml` 只写 `window`），播放继续，所以不能并进 `ShowWindow`。
    HotkeyWindow(WindowKind),
    /// 同上，但对应原版直接执行的那几个宏：Ctrl+Shift+G/J/K/Q/W/E/U
    /// （`control_draw.gml:792-831`），每条也都是先 `playing = 0`。
    HotkeyMacro(Macro),
    /// 原版右上角兼容性徽章被点（`control_draw.gml:2029-2038`）：停播放、关录制、
    /// 落回浮动选区，再打开 `w_minecraft`。
    ShowCompat,
    /// 兼容性窗口的两条速度阶梯，Schematic 页和数据包页各一条（`draw_window_minecraft.gml`）。
    FixTempoSchematic,
    FixTempoDatapack,
    /// 原版 `select_outside(lower, higher)`，按钮点完跟着 `windowclose = 1`。
    SelectOutside {
        lower: bool,
        higher: bool,
    },
    /// 原版 `select_custom()`，同样跟着 `windowclose = 1`。
    SelectCustom,
    /// 兼容性窗口「转换音符」：先 `question()`，答「是」才走下一条。
    TransposeToMinecraft,
    /// 上面那问的「是」分支：全选 → 折回两个八度 → 落回 → 按原版那句英文报数。
    TransposeToMinecraftApply,
    /// 「保存资源包」= 原版 `datapack_getextranotes()`，兼容性窗口和数据包导出窗口都有。
    SaveExtraNotesPack,
    /// 鼠标悬停在音符上滚轮调整其属性（对应原版 `changepitch`）。
    WheelOnNote {
        tick: u32,
        layer: u32,
        delta: i32,
        shift: bool,
    },
    /// 原版 `mousewheel = 1`：滚轮在音色表上前后挪一格，力度/声像/微调复位并试听。
    WheelInstrument(i32),
    /// 原版 `mousewheel = 2`：滚轮在琴键上前后挪一格，同样复位并试听。
    WheelKey(i32),
    /// 打开通用文件对话框（打开/保存/各类导入导出）。
    OpenDialog(DialogAction),
    /// 原版首选项目录行的「打开」：`open_url(songfolder)`，目录没了先弹「该路径不存在！」。
    OpenPrefsFolder(PrefsFolder),
    /// 原版首选项目录行的「还原默认」：退回 `songs_directory` / `pattern_directory`。
    ResetPrefsFolder(PrefsFolder),
    /// 原版首选项目录行的「修改」：先 `message()` 说明一句，再弹目录选择框。
    ChangePrefsFolder(PrefsFolder),
    /// 原版取色器「使用默认值」：`accent1/2/3 = 0,120,215` + `draw_accent_init()`
    /// + `resetcolor = true`（`draw_window_set_accent.gml:132-137`）。不落盘。
    AccentUseDefault,
    /// 原版 `menu_click` 的导出 Schematic 入口：先过 `calculate_size()` 的提示与提问。
    OpenSchematicExport,
    /// 原版「导出」按钮：两道闸门都过才弹保存框。
    ExportSchematicChecked,
    /// 原版 `menu_click` 的导出直轨入口：与 Schematic 共用同一个 `calculate_size()`
    /// 闸门（menu_click.gml:66-68），警告与兼容问句一样不少。
    OpenTrackExport,
    /// 原版直轨窗口「导出」按钮（draw_window_track_export.gml:270-278）。
    TrackExportChecked,
    /// 原版「使用默认值」= `reset_schematic_export(1)`。
    ResetSchematicOptions,
    /// 原版导入窗口的「导入」= `import_midi()`，随后 `save_settings()`。
    MidiImportRun,
    /// 原版导入窗口「使用默认值」答「是」之后那一大段：`midi_instruments()`
    /// 重查出厂 GM 表 + 恢复 `w_midi_*` + 恢复各声部的音符长度。
    MidiUseDefaults,
    /// 原版「取消」：`songs[song].midifile = ""` + 关窗，解析好的那份 MIDI 一起丢掉。
    MidiImportCancel,
    /// 回答当前模态框，`false` = 「否」（丢弃整个待弹队列）。
    AnswerModal(bool),
    /// 文件对话框确定。
    DialogConfirm(PathBuf),
    RunMacro(Macro),
    /// 琶音 / 滑音 / 错开三个原版宏窗口的「确定」。宏只改浮动选区，收尾原版是
    /// `changed = 1` + `if(!keyboard_check(vk_alt)) selection_place(false)` ——
    /// 没按 Alt 时把选区落回歌曲（那在原版里也是第二条历史）。
    MacroWindowOk {
        m: Macro,
        place: bool,
    },
    SetMasterVolume(f32),
    /// 端口自加（用户要求 #94）：切换输出设备，`None` = 跟随系统默认。
    SetOutput(Option<String>),
    SetPlaySpeed(f32),
    ToggleMetronome,
    ToggleExtendedVoices,
    ToggleMono,
    SetSoundsDir(String),
    ReloadSamples,
    SavePreferences,
    RestoreAutosave(PathBuf),
    /// 原版 `open_url(url)`：欢迎窗链接、捐赠横幅、更新弹窗的「去 releases 页」。
    OpenUrl(String),
    /// 首选项「检查开发版本」确认框答「是」之后的落盘开关。
    SetCheckPrerelease(bool),
    /// 原版设置菜单的「导入音效」入口：开窗前先 `update_asset_index_menu(0)`
    /// （menu_click.gml:196；实参那个 0 原版自己也没用上）。
    OpenSoundImport,
    /// 向导「从 Mojang 下载」勾选框：翻转 + 清列表 + 刷新（draw_window_sound_import.gml:56-59）。
    SoundImportToggleDownload,
    /// 向导「获取」按钮（下载模式第 1 步）：先过 EULA 确认框。
    SoundImportGetIndexes,
    /// EULA 答应后：起线程拉版本清单（原版 stage 1）。
    SoundImportStartVersionList,
    /// 版本下拉换选项：`sound_import_asset_index_select = sel` + 刷新
    /// （menu_click.gml:470-473）。
    SoundImportSelectIndex(usize),
    /// 向导「更改」按钮：`get_directory` 目录选择框。
    SoundImportChangeDir,
    /// 「使用默认设定」：`mc_install_path = mc_default_path` + 刷新。
    SoundImportUseDefaultPath,
    /// 「获取音效」按钮：本地走覆盖确认 + 拷贝；下载模式起线程（原版 stage 2..4 / 续传）。
    SoundImportGet,
    /// 本地覆盖确认答「是」后的实际拷贝。
    SoundImportCopyGo,
    /// 下载模式覆盖确认答「是」：唤醒后台线程继续。
    SoundImportReplaceAnswered,
    /// 「打开已获取音效目录」：`open_url(sounds_directory)`。
    SoundImportOpenSoundsFolder,
    /// 向导「确定」：`sound_import_status = 0` + 关窗（下载中按钮是锁着的，不会带线程走）。
    SoundImportOk,
    /// 原版文件菜单「Clear recent songs」（file 菜单与播放器 `filep` 都有，
    /// `control_draw.gml:1698/1715`）：清空最近打开列表。
    ClearRecent,
    /// 原版播放器菜单的 Exit（`control_draw.gml` 的 `game_end()`）。egui/eframe
    /// 没有直接退出 API，走 `ViewportCommand::Close`。
    Exit,
}

/// 原版 `selection_delete.gml:24-28`：删除选区前按当前视口收集格子，
/// 交给 `EditorState::spawn_remove_effects` 生成淡出边框（剪切 / 删除共用）。
fn spawn_selection_effects(state: &mut EditorState) {
    let sel = &state.doc.selection;
    let cells: Vec<(i64, i64)> = sel
        .notes
        .iter()
        .map(|n| (sel.x + n.dx as i64, sel.y + n.dy as i64))
        .collect();
    state.spawn_remove_effects(cells);
}

/// 原版键盘按下后实际用的那颗音：音色沿用当前的，键是**按下的键**，力度/声像/微调固定 100/100/0。
/// `select_lastpressed` 只决定选中的音要不要跟着走，不影响这颗音。
fn pressed_note(state: &EditorState, key: u8) -> Note {
    let mut note = state.current;
    note.key = key;
    note.velocity = 100;
    note.panning = 100;
    note.pitch = 0;
    note
}

/// 原版 `while (!add_block_manual(ceil(marker_pos), b, instrument, key, 100, 100, 0)) b += 1`：
/// 从第 0 层往上找当前列的第一个**空格**（`add_block_manual` 只看格子里有没有音符，
/// 不看图层锁定），全满就在最上面加一层。
fn record_note(state: &mut EditorState, key: u8) {
    let note = pressed_note(state, key);
    record_note_at(state, note);
}

/// 录音写块的共用入口：PC 键盘（[`record_note`]）和实时 MIDI
/// （[`crate::live_midi`]）都走 `add_block_manual` 的这一段。
pub(crate) fn record_note_at(state: &mut EditorState, note: Note) {
    let tick = state.marker().ceil().max(0.0) as u32;
    let layers = state.doc.song.layer_count();
    let mut layer = 0u32;
    while layer < layers && state.doc.song.has(tick, layer) {
        layer += 1;
    }
    if layer >= layers {
        state.doc.song.layers.push(Default::default());
    }
    state.doc.place_note(tick, layer, note);
    // 原版 add_block_manual 结尾 `if (ins.loaded) play_sound(...)`：写下去的同时出声。
    state.audio.preview_note(&note);
}

/// 供右键菜单/快捷键共用的入口。
pub fn dispatch(state: &mut EditorState, cmd: Command, ctx: &Context) {
    // 界面语言跟着 prefs 走：生产路径 dispatch 都在帧内（app::show 已同步过），
    // 但测试直接调 dispatch，不在这里同步的话消息文案会被并行用例的全局量串台。
    crate::set_language(state.prefs.language);
    match cmd {
        Command::New => {
            if state.doc.song.dirty {
                state.window = Some(WindowKind::ConfirmNew);
            } else {
                dispatch(state, Command::ConfirmNew, ctx);
            }
        }
        Command::ConfirmNew => {
            state.doc.replace_song(Song::new());
            // 原版换歌 = 新实例 id，声源分配方案随之失效。
            state.song_session += 1;
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.clear_section();
            state.window = None;
            state.set_message(tr("New song created", "已新建歌曲"));
        }
        Command::Open => {
            // 原版 `load_song.gml:11-12`：起始目录只看 `songfolder`，默认名是空串。
            dispatch(state, Command::OpenDialog(DialogAction::OpenSong), ctx);
        }
        Command::SaveAs => {
            // 原版 `save_song.gml:20`：播放器档位保存直接 `return 0`。
            if state.player {
                return;
            }
            // 原版 `save_song.gml:13-14`：目录同样是 `songfolder`，默认名是 `fsave + ".nbs"`。
            dispatch(state, Command::OpenDialog(DialogAction::SaveSong), ctx);
        }
        Command::Save => {
            if state.player {
                return;
            }
            if let Some(path) = state.doc.song.filename.clone() {
                dispatch(state, Command::SavePath(path), ctx);
            } else {
                dispatch(state, Command::SaveAs, ctx);
            }
        }
        Command::OpenPath(path) if is_schematic_path(&path) => {
            // 原版 `load_song.gml:27`：扩展名是 .schematic 就转 open_schematic() 并立刻返回。
            import_schematic(state, &path);
            state.window = None;
        }
        Command::OpenPath(path) => open_nbs_at(state, &path, false),
        Command::SavePath(path) => {
            if state.player {
                return;
            }
            let version = state.doc.song.meta.save_version;
            match nbs_core::nbs::save(&path, &state.doc.song, version) {
                Ok(()) => {
                    state.doc.song.filename = Some(path.clone());
                    state.doc.song.dirty = false;
                    state.add_recent(path);
                    state.window = None;
                    state.set_message(tr("Song saved", "歌曲已保存"));
                }
                Err(e) => state.set_message(if crate::lang() == 1 {
                    format!("保存失败: {e}")
                } else {
                    format!("Save failed: {e}")
                }),
            }
        }
        Command::Undo => {
            if state.doc.undo() {
                state.set_message(if crate::lang() == 1 {
                    format!("撤销: {}", state.doc.history.undo_label().unwrap_or(""))
                } else {
                    format!("Undo: {}", state.doc.history.undo_label().unwrap_or(""))
                });
            }
        }
        Command::Redo => {
            if state.doc.redo() {
                state.set_message(if crate::lang() == 1 {
                    format!("重做: {}", state.doc.history.redo_label().unwrap_or(""))
                } else {
                    format!("Redo: {}", state.doc.history.redo_label().unwrap_or(""))
                });
            }
        }
        Command::Copy => {
            state.doc.copy_selection();
            ctx.copy_text(state.doc.clipboard_text.clone());
            state.set_message(if crate::lang() == 1 {
                format!("已复制 {} 个音符", state.doc.clipboard.len())
            } else {
                format!("Copied {} notes", state.doc.clipboard.len())
            });
        }
        Command::Cut => {
            // 原版 `action_cut` 就是 `selection_delete(false)`，淡出边框同样要出。
            spawn_selection_effects(state);
            state.doc.cut_selection();
            ctx.copy_text(state.doc.clipboard_text.clone());
            state.set_message(tr("Cut", "已剪切"));
        }
        Command::Paste => {
            let (x, y) = state
                .hover_cell
                .map(|(t, l)| (t as i64, l as i64))
                .unwrap_or((
                    state.viewport.start_tick as i64,
                    state.viewport.start_layer as i64,
                ));
            state.doc.paste(x, y);
            state.set_message(tr(
                "Pasted (floating selection; drag to place)",
                "已粘贴（浮动选区，拖拽可定位）",
            ));
        }
        Command::PasteText(text) => {
            let (x, y) = state
                .hover_cell
                .map(|(t, l)| (t as i64, l as i64))
                .unwrap_or((
                    state.viewport.start_tick as i64,
                    state.viewport.start_layer as i64,
                ));
            if text.contains('|') {
                state.doc.paste_text(&text, x, y);
                state.set_message(tr("Pasted from clipboard text", "已从剪贴板文本粘贴"));
            } else {
                dispatch(state, Command::Paste, ctx);
            }
        }
        Command::Delete => {
            // 原版 `action_delete` → `selection_delete(false)`：逐格生成删除效果
            // （theme 3 且开关打开，只生成当前视口内的格子）。
            spawn_selection_effects(state);
            state.doc.delete_selection();
        }
        Command::SelectAll => state.doc.select_all(),
        Command::Deselect => state.doc.deselect(),
        Command::InvertSelection => state.doc.select_invert(),
        Command::SelectByInstrument { instrument, invert } => {
            state.doc.select_by_instrument(instrument, invert);
            state.set_message(if crate::lang() == 1 {
                format!("已选中 {} 个音符", state.doc.selection.len())
            } else {
                format!("Selected {} notes", state.doc.selection.len())
            });
        }
        Command::PlaceNote { tick, layer } => {
            if state.doc.song.is_locked(layer) {
                state.set_message(tr("Layer is locked", "图层已锁定"));
                return;
            }
            // 原版 add/change_block_manual 的结尾：落的是事件乐器就不直接吃画笔值，
            // 改写事件默认值并弹出编辑窗；落点已有事件乐器时是「再编辑」——草稿
            // 回填现有值，音符本身不动（原版 is_event 分支）。
            let brush = state.current;
            let name = state.doc.song.instrument_name(brush.instrument).to_string();
            let existing = state.doc.song.get(tick, layer);
            let existing_is_event = existing.as_ref().is_some_and(|n| {
                let n = state.doc.song.instrument_name(n.instrument);
                n == nbs_core::TEMPO_CHANGER_NAME || n == nbs_core::SOUND_STOPPER_NAME
            });
            match name.as_str() {
                nbs_core::TEMPO_CHANGER_NAME => {
                    let bpm = if existing_is_event {
                        existing.unwrap().pitch as i64
                    } else {
                        // 原版 `int64(song.tempo * 15)`。超过 i16 的值存不进音符模型
                        // （窗口校验也只收 < 32768），这里截断，普通落格路径不受影响。
                        let bpm = ((state.doc.song.meta.tempo as f64 * 15.0) as i64)
                            .clamp(0, i16::MAX as i64) as i16;
                        let note = Note {
                            pitch: bpm,
                            ..brush
                        };
                        if existing.is_some() {
                            state.doc.change_note_raw(tick, layer, note);
                        } else {
                            state.doc.place_note_raw(tick, layer, note);
                        }
                        bpm as i64
                    };
                    state.event_edit.tempo = bpm.to_string();
                    state.window = Some(WindowKind::EditTempoChanger(tick, layer));
                }
                nbs_core::SOUND_STOPPER_NAME => {
                    let (start, until) = if existing_is_event {
                        let n = existing.unwrap();
                        (
                            n.pitch as i64,
                            panning_velocity_to_short(n.panning, n.velocity) as i64,
                        )
                    } else {
                        // 原版 `start = until = yy + 1`，终止层编码进声像 + 力度。
                        let start = (layer as i64 + 1).clamp(0, i16::MAX as i64);
                        let (pan, vel) = short_to_panning_velocity(layer + 1);
                        let note = Note {
                            pitch: start as i16,
                            panning: pan,
                            velocity: vel,
                            ..brush
                        };
                        if existing.is_some() {
                            state.doc.change_note_raw(tick, layer, note);
                        } else {
                            state.doc.place_note_raw(tick, layer, note);
                        }
                        (start, layer as i64 + 1)
                    };
                    state.event_edit.start = start.to_string();
                    state.event_edit.until = until.to_string();
                    state.window = Some(WindowKind::EditSoundStopper(tick, layer));
                }
                _ => {
                    if existing.is_some() {
                        state.doc.change_note(tick, layer, brush);
                    } else {
                        state.doc.place_note(tick, layer, brush);
                    }
                }
            }
        }
        Command::SetEventTempo { tick, layer, bpm } => {
            state.doc.set_event_tempo(tick, layer, bpm);
        }
        Command::SetEventStopper {
            tick,
            layer,
            start,
            until,
        } => {
            state.doc.set_event_stopper(tick, layer, start, until);
        }
        Command::RemoveNote { tick, layer } => {
            if state.doc.song.is_locked(layer) {
                state.set_message(tr("Layer is locked", "图层已锁定"));
                return;
            }
            // 原版 `remove_block_manual.gml:64-68`：右键单删也出淡出边框。
            if state.doc.song.has(tick, layer) {
                state.spawn_remove_effects([(tick as i64, layer as i64)]);
            }
            state.doc.remove_note(tick, layer);
        }
        Command::PickNote { tick, layer, all } => {
            if let Some(n) = state.doc.song.get(tick, layer).or_else(|| {
                state
                    .doc
                    .selection
                    .contains(tick as i64, layer as i64)
                    .then(|| {
                        state.doc.selection.at(
                            (tick as i64 - state.doc.selection.x) as u32,
                            (layer as i64 - state.doc.selection.y) as u32,
                        )
                    })
                    .flatten()
            }) {
                state.current.instrument = n.instrument;
                state.current.key = n.key;
                if all {
                    state.current.velocity = n.velocity;
                    state.current.panning = n.panning;
                    state.current.pitch = n.pitch;
                } else {
                    state.current.velocity = VEL_MAX;
                    state.current.panning = 100;
                    state.current.pitch = 0;
                }
                let picked = format!(
                    "{} / {}",
                    nbs_core::instruments::instrument_name(n.instrument),
                    nbs_core::model::key_name(n.key, true, state.flags.flats)
                );
                state.set_message(if crate::lang() == 1 {
                    format!("吸管: {picked}")
                } else {
                    format!("Picked: {picked}")
                });
            }
        }
        Command::SelectRect {
            x0,
            y0,
            x1,
            y1,
            copy,
            additive,
            subtract,
        } => {
            if subtract {
                state.doc.selection_remove_rect(x0, y0, x1, y1);
            } else {
                state.doc.select_rect(x0, y0, x1, y1, copy, additive, false);
                state.set_message(if crate::lang() == 1 {
                    format!("已选中 {} 个音符", state.doc.selection.len())
                } else {
                    format!("Selected {} notes", state.doc.selection.len())
                });
            }
        }
        Command::AddLayer => state.doc.append_layer(),
        Command::InsertLayer(at) => state.doc.insert_layer(at),
        Command::RemoveLayer(i) => state.doc.remove_layer(i),
        Command::MoveLayerUp(i) => {
            if i > 0 {
                state.doc.swap_layers(i, i - 1);
            }
        }
        Command::MoveLayerDown(i) => state.doc.swap_layers(i, i + 1),
        Command::RenameLayer(i, name) => {
            state.doc.set_layer(i, |l| l.name = name.clone());
        }
        Command::SetLayerLock(i, lock) => {
            state.doc.set_layer(i, |l| l.lock = lock);
        }
        Command::SetLayerVolume(i, v) => {
            state.doc.set_layer(i, |l| l.volume = v.min(VEL_MAX));
        }
        Command::SetLayerStereo(i, v) => {
            state.doc.set_layer(i, |l| l.stereo = v.min(PAN_MAX));
        }
        Command::SelectLayer(i) => {
            // 原版按 `selected_layers` 判定这一层是否已经在选区里；我们没有那个列表，
            // 用「选区出现的层恰好只有这一层」等价判断：是 → 整行从选区减掉，
            // 否 → 把整行并进现有选区（原版 `selection_add` 本来就是累加）。
            let only_this = !state.doc.selection.is_empty()
                && state
                    .doc
                    .selection
                    .notes
                    .iter()
                    .all(|n| state.doc.selection.y + n.dy as i64 == i as i64);
            if only_this {
                let enda = state.doc.song.enda;
                state.doc.selection_remove_rect(0, i, enda, i);
            } else {
                let additive = !state.doc.selection.is_empty();
                let enda = state.doc.song.enda;
                state.doc.select_rect(0, i, enda, i, false, additive, false);
            }
        }
        // 原版每个选乐器的入口（数字快捷键 `control_draw.gml:822-843`、音色格子
        // `draw_icon_insbox` 的 `instrument = ins` 分支）都顺带把力度/声像/微调复位并发一声。
        Command::SetInstrument(i) => {
            state.current.instrument = i;
            state.current.velocity = 100;
            state.current.panning = 100;
            state.current.pitch = 0;
            state.audio.preview_note(&state.current);
        }
        Command::SetEditMode(m) => state.tool = m,
        Command::SetKey(d) => {
            state.current.key = (state.current.key as i32 + d).clamp(0, KEY_MAX as i32) as u8;
        }
        Command::SelectKey(k) => {
            state.current.key = k;
            state.current.velocity = 100;
            state.current.panning = 100;
            state.current.pitch = 0;
            state.audio.preview_note(&state.current);
        }
        Command::KeyPress(k) => {
            // 原版 `if (select_lastpressed) {selected_key = a; selected_vel = 100; …}`
            if state.prefs.select_lastpressed {
                state.current.key = k;
                state.current.velocity = 100;
                state.current.panning = 100;
                state.current.pitch = 0;
            }
            if state.record {
                // 原版：`if (playing = 0.25) toggle_playing()` —— 第一个按键才真正起播，
                // 这样第一个音符就落在播放起点上。
                if state.record_armed {
                    state.record_armed = false;
                    state.prepare_audio();
                    // 原版 draw_piano.gml:147：录音待命下按第一个键也是 toggle_playing。
                    state.begin_playback();
                }
                record_note(state, k);
            } else {
                // 原版 `if (record = 0) play_sound(instrument, a, 100, 100, 0)`。
                let note = pressed_note(state, k);
                state.audio.preview_note(&note);
            }
        }
        Command::WheelInstrument(dir) => {
            let list = state.instrument_list();
            let cur = list
                .iter()
                .position(|(id, _)| *id == state.current.instrument)
                .unwrap_or(0) as i32;
            // 原版只在还能走的时候动：`if (down && insindex > 0)` / `if (up && insindex < size-1)`。
            let next = (cur + dir).clamp(0, list.len() as i32 - 1);
            if next == cur {
                return;
            }
            state.current.instrument = list[next as usize].0;
            state.current.velocity = 100;
            state.current.panning = 100;
            state.current.pitch = 0;
            let (vol, stereo) = state.wheel_preview_vp();
            state.audio.preview_note_vp(&state.current, vol, stereo);
        }
        Command::WheelKey(dir) => {
            let cur = state.current.key as i32;
            let next = (cur + dir).clamp(0, KEY_MAX as i32);
            if next == cur {
                return;
            }
            state.current.key = next as u8;
            state.current.velocity = 100;
            state.current.panning = 100;
            state.current.pitch = 0;
            let (vol, stereo) = state.wheel_preview_vp();
            state.audio.preview_note_vp(&state.current, vol, stereo);
        }
        Command::ToggleRecord => {
            state.record = !state.record;
            state.record_armed = state.record && !state.is_playing();
        }
        Command::SetVel(d) => {
            state.current.velocity =
                (state.current.velocity as i32 + d).clamp(0, VEL_MAX as i32) as u8;
        }
        Command::SetPan(d) => {
            state.current.panning =
                (state.current.panning as i32 + d).clamp(0, PAN_MAX as i32) as u8;
        }
        Command::SetPit(d) => {
            state.current.pitch =
                (state.current.pitch as i32 + d).clamp(-PIT_MAX as i32, PIT_MAX as i32) as i16;
        }
        Command::WindowScaleUp => state.step_window_scale(ctx, true),
        Command::WindowScaleDown => state.step_window_scale(ctx, false),
        Command::WindowScaleReset => state.reset_window_scale(ctx),
        Command::PianoScaleUp => state.step_piano_scale(ctx, true),
        Command::PianoScaleDown => state.step_piano_scale(ctx, false),
        Command::PageHorizontal(d) => {
            let cols = state.viewport.columns() as i64;
            let max = state.doc.song.enda.max(64) as i64;
            state.viewport.start_tick =
                (state.viewport.start_tick as i64 + d as i64 * cols).clamp(0, max) as u32;
        }
        Command::PageVertical(d) => {
            let rows = state.viewport.rows() as i64;
            let max = state.doc.song.layer_count().max(8) as i64;
            state.viewport.start_layer =
                (state.viewport.start_layer as i64 + d as i64 * rows).clamp(0, max) as u32;
        }
        Command::PlayPause => {
            let playing = state.is_playing();
            state.prepare_audio();
            if playing {
                state.audio.clock_mut().pause();
            } else {
                state.begin_playback();
            }
        }
        Command::Stop => {
            state.audio.clock_mut().stop_and_rewind();
            state.viewport.start_tick = 0;
        }
        Command::SeekHome => state.set_marker(0.0),
        Command::SeekEnd => state.set_marker(state.doc.song.song_length() as f64),
        Command::ToggleLoop => {
            state.doc.edit_meta(tr("Toggle loop", "切换循环"), |m| {
                m.loop_enabled = !m.loop_enabled
            });
            state.sync_loop_params();
        }
        Command::ToggleFollow => state.marker_follow = !state.marker_follow,
        Command::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        Command::ShowWindow(kind) => {
            // 原版 `anim_window_open`：`window != window_last` 时窗口重新做打开动画，
            // 所有页签回到第一页；重复打开同一个窗口不重置。
            if state.window.as_ref() != Some(&kind) {
                state.datapack_tab = 0;
                state.schematic_tab = 0;
                state.prefs_tab = 0;
                state.mc_tab = 0;
            }
            if matches!(kind, WindowKind::DatapackExport) && state.datapack_options.name.is_empty()
            {
                // 原版在窗口第一次绘制时把 dat_name 填成歌曲名，其次是文件名去掉扩展名。
                let meta = state.doc.song.meta.name.trim().to_string();
                let from_file = state
                    .doc
                    .song
                    .filename
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                state.datapack_options.name = if meta.is_empty() { from_file } else { meta };
            }
            // 文件选择不是自绘窗口：走和工具栏同一套对话框请求，起始目录和默认名
            // 都按原版 `load_song.gml` / `save_song.gml` 的写法来。
            match kind {
                WindowKind::FileOpen => {
                    dispatch(state, Command::OpenDialog(DialogAction::OpenSong), ctx)
                }
                WindowKind::FileSave => {
                    dispatch(state, Command::OpenDialog(DialogAction::SaveSong), ctx)
                }
                // 原版没有「空白的 MIDI 导入窗口」：必须先选到文件、`open_midi()`
                // 解析成功才画得出来。
                WindowKind::MidiImport => {
                    dispatch(state, Command::OpenDialog(DialogAction::ImportMidi), ctx)
                }
                _ => state.window = Some(kind),
            }
            state.context_menu = None;
        }
        Command::CloseWindow => {
            if state.window == Some(WindowKind::MidiImport) {
                // 键盘 Escape 关窗和点「取消」一样，解析好的那份 MIDI 跟着丢。
                state.midi_data = None;
                state.midi_file.clear();
            }
            if state.window == Some(WindowKind::TempoTapper) {
                // 原版关窗清三条打点变量（OK / Cancel 写在窗口脚本里，Esc 写在
                // `draw_windows.gml:94-99`）；双倍速度的勾选是偏好，不跟着清。
                state.tapper.reset_for_close();
            }
            state.window = None;
            // 原版的 `obj_menu` 跟着窗口一起销毁。这里不清的话，下拉菜单开着时用
            // Escape / Ctrl+P 关窗，下次打开首选项会凭空冒出一个悬浮菜单。
            state.prefs_theme_menu = false;
            state.prefs_rate_menu = false;
        }
        // ---------------------------------------------- 导出 Schematic / 导出直轨
        // 原版 `menu_click`：两个入口都走 `if (calculate_size()) window = …`
        // （menu_click.gml:66-68），警告弹窗与兼容问句是 calculate_size() 的，
        // 所以直轨入口一样不少；差别只在最后开的窗口。
        Command::OpenSchematicExport | Command::OpenTrackExport => {
            let target = if cmd == Command::OpenTrackExport {
                WindowKind::TrackExport
            } else {
                WindowKind::SchematicExport
            };
            state.schematic_stats = nbs_io::schematic::calculate_stats(&state.doc.song);
            state.schematic_options.tempo_mode = state.schematic_stats.tempo_mode;
            let st = &state.schematic_stats;
            let mut items: std::collections::VecDeque<Modal> = std::collections::VecDeque::new();
            if !state.prefs.warning_schematic {
                items.push_back(Modal {
                    title: tr("Warning", "警告"),
                    text: if crate::lang() == 1 {
                        "导出的 schematic 目前之和 Minecraft: Java Edition 1.12 及以下兼容。\
如想在 1.13 和以上使用，请使用结构方块格式导出。\n\n此警告将不再提示。"
                            .into()
                    } else {
                        "The exported schematic is currently only compatible with Minecraft: Java Edition 1.12 and below. To import it in 1.13 and above, please use the export to structure block function.\n\nThis warning will never be shown again.".into()
                    },
                    yes_no: false,
                });
                // 原版在这里 `save_settings()`，把“已看过”落盘。
                state.prefs.warning_schematic = true;
                let _ = state.prefs.save();
            }
            for q in st.compat_questions() {
                items.push_back(Modal {
                    title: tr("Minecraft Compatibility", "Minecraft 兼容性"),
                    text: q,
                    yes_no: true,
                });
            }
            if let Some(t) = st.tempo_notice() {
                items.push_back(Modal {
                    title: tr("Tempo", "速度"),
                    text: t.into(),
                    yes_no: false,
                });
            }
            let guard = nbs_io::schematic::open_guard(st);
            if let Err(e) = &guard {
                items.push_back(Modal {
                    title: tr("Error", "错误"),
                    text: e.clone(),
                    yes_no: false,
                });
            }
            state.modals = ModalQueue {
                items,
                then: guard.ok().map(|_| Command::ShowWindow(target.clone())),
            };
            if state.modals.is_empty() {
                dispatch(state, Command::ShowWindow(target), ctx);
            }
        }
        Command::AnswerModal(yes) => {
            if !yes {
                // 下载模式的覆盖确认答「否/取消」也要告诉后台线程停下，否则它一直挂着。
                if state.sound_import.replace_asked {
                    crate::sound_import::answer_replace(&mut state.sound_import, false);
                }
                state.modals = ModalQueue::default();
            } else {
                state.modals.items.pop_front();
                if state.modals.is_empty() {
                    if let Some(cmd) = state.modals.then.take() {
                        dispatch(state, cmd, ctx);
                    }
                }
            }
        }
        Command::ResetSchematicOptions => {
            state.schematic_options.reset();
            // 原版 `reset_schematic_export.gml:70`：plan 与签名一并清空。
            state.invalidate_command_plan();
            state.schematic_command_signature.clear();
            state.set_message(tr(
                "Schematic export defaults restored",
                "导出 Schematic 的设定已还原",
            ));
        }
        Command::MidiImportRun => {
            // 原版 `import_midi()`：用窗口里那份已解析的 MIDI + 界面上的映射表落一首新歌，
            // 收尾按「记住我的更改」把映射写回 GM 表，否则只把 w_midi_* 恢复成出厂值，
            // 两种情况最后都会 `save_settings()`。
            let Some(midi) = state.midi_data.clone() else {
                state.window = None;
                return;
            };
            let (song, report) = if state.midi_enhanced.enabled {
                match nbs_io::midi_enhanced::import_midi_enhanced(
                    &midi,
                    &state.midi_enhanced_options,
                    &state.midi_enhanced,
                ) {
                    Ok((song, report)) => (song, Some(report)),
                    Err(error) => {
                        state.midi_enhanced_check = Some(Err(error.clone()));
                        state.set_message(error);
                        return;
                    }
                }
            } else {
                let song = nbs_io::midi::import_midi_data(&midi, &state.midi_options);
                state.midi_options.finish_import(&midi, &mut state.midi_gm);
                (song, None)
            };
            let n = song.total_blocks;
            state.midi_enhanced_check = None;
            state.window = None;
            state.midi_data = None;
            state.midi_file.clear();
            state.midi_tab = 0;
            state.doc.replace_song(song);
            state.song_session += 1;
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.clear_section();
            state.save_prefs();
            state.set_message(match report {
                Some(report) if crate::lang() == 1 => format!(
                    "增强 MIDI 导入完成：{n} 个音符，{} 个超出原生音域（key 33..57）",
                    report.out_of_range,
                ),
                Some(report) => format!(
                    "Enhanced MIDI import: {n} notes, {} outside native range (key 33..57)",
                    report.out_of_range,
                ),
                None if crate::lang() == 1 => format!("MIDI 导入完成：{n} 个音符"),
                None => format!("MIDI import finished: {n} notes"),
            });
        }
        Command::MidiUseDefaults => {
            // 原版「使用默认值」答「是」之后：先 `midi_instruments()` 把 GM 映射恢复出厂，
            // 再按这份出厂表重刷界面四张表，最后把 w_midi_* 设成出厂值。
            let Some(midi) = state.midi_data.clone() else {
                return;
            };
            state.midi_gm = nbs_io::MidiGmTable::default();
            state.midi_options.use_defaults(&midi, &state.midi_gm);
            state.set_message(crate::tr(
                "MIDI import options restored",
                "MIDI 导入设定已还原",
            ));
        }
        Command::MidiImportCancel => {
            // 原版「取消」：`songs[song].midifile = ""` 后关窗，解析好的那份 MIDI 一起丢掉，
            // 不写 settings.ini（界面上的改动留在内存里，等下次开文件时重刷）。
            state.window = None;
            state.midi_data = None;
            state.midi_file.clear();
            state.midi_tab = 0;
        }
        Command::ExportSchematicChecked => {
            // 原版按钮闸（draw_window_schematic_export.gml:323-331）：
            // 先查「没有方块」，尺寸闸只在非命令模式生效——命令模式的尺寸
            // 由声源分配方案决定，留给 schematic_export() 内部检查。
            let st = state.schematic_stats.clone();
            let o = state.schematic_options.clone();
            let blocked = if st.totalblocks[o.include_locked as usize] == 0 {
                Some(nbs_io::schematic::ExportCheck::NoBlocks.dialog().unwrap())
            } else if !(o.structure && o.command_block) {
                nbs_io::schematic::export_check(&st, &o).dialog()
            } else {
                None
            };
            match blocked {
                None => dispatch(
                    state,
                    Command::OpenDialog(DialogAction::ExportSchematic),
                    ctx,
                ),
                Some((title, msg)) => {
                    state.modals = ModalQueue::message(title, msg);
                }
            }
        }
        Command::TrackExportChecked => {
            // 原版按钮闸（draw_window_track_export.gml:270-278）：先查「没有方块」，
            // 尺寸闸只在非命令模式生效且只看 40+enda*2；CN 的太大文案比 Schematic
            // 窗口少一行。
            let st = state.schematic_stats.clone();
            let o = state.schematic_options.clone();
            let blocked = if st.totalblocks[o.include_locked as usize] == 0 {
                Some((
                    tr("Schematic export", "导出结构"),
                    tr("There are no blocks to export!", "没有方块可以导出！"),
                ))
            } else if !(o.structure && o.command_block)
                && 40u64 + state.doc.song.enda as u64 * 2 >= 2000
            {
                Some((
                    tr("Error", "错误"),
                    if crate::lang() == 1 {
                        "这个结构太大了。大小限制为 2000x2000x256。".into()
                    } else {
                        "The Schematic is too big. The maximum size is 2000x2000x256.\nTry changing the \"Repeaters per row\" value to decrease the size.".into()
                    },
                ))
            } else {
                None
            };
            match blocked {
                None => dispatch(state, Command::OpenDialog(DialogAction::ExportTrack), ctx),
                Some((title, msg)) => {
                    state.modals = ModalQueue::message(title, msg);
                }
            }
        }
        Command::SetTheme(kind) => {
            // 原版切主题不重置 accent1/2/3（draw_set_accent 的基色全局共享），
            // 事件乐器改的表倒是随新主题重算——所以只把基色带过去。
            let base = state.theme.accent_base;
            state.theme = Theme::new(kind);
            state.theme.set_accent_base(base);
            state.theme.apply(ctx);
            state.set_message(if crate::lang() == 1 {
                format!("主题 => {}", kind.name())
            } else {
                format!("Theme => {}", kind.name())
            });
        }
        Command::ToggleRainbow => {
            state.toggle_rainbow();
            state.theme.apply(ctx);
        }
        Command::ToggleFullscreen => {
            // 原版把全屏记在自己的 `fullscreen` 变量上，这里同理：egui 的
            // `ViewportCommand::Fullscreen` 只发不收，系统里退出全屏不会回传，
            // 所以镜像可能比实际慢一拍（此时第一次 F11 只是把镜像翻回来）。
            state.fullscreen = !state.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(state.fullscreen));
        }
        Command::ToggleFlag(flag) => {
            let f = &mut state.flags;
            match flag {
                Flag::ShowLayers => f.show_layers = !f.show_layers,
                Flag::ShowPiano => f.show_piano = !f.show_piano,
                Flag::UseColors => f.use_colors = !f.use_colors,
                Flag::UseShapes => f.use_shapes = !f.use_shapes,
                Flag::UseIcons => f.use_icons = !f.use_icons,
                Flag::ShowNumbers => f.show_numbers = !f.show_numbers,
                Flag::ShowOctaves => f.show_octaves = !f.show_octaves,
                Flag::ShowKeyNames => f.show_keynames = !f.show_keynames,
                Flag::ShowKeyNumbers => f.show_keynumbers = !f.show_keynumbers,
                Flag::ShowKeyboard => f.show_keyboard = !f.show_keyboard,
                Flag::ShowOutOfRange => f.show_outofrange = !f.show_outofrange,
                Flag::ShowIncompatible => f.show_incompatible = !f.show_incompatible,
                Flag::Fade => f.fade = !f.fade,
                Flag::ShowGridLines => f.show_grid_lines = !f.show_grid_lines,
                Flag::Flats => f.flats = !f.flats,
                Flag::Blackout => f.blackout = !f.blackout,
                Flag::RemoveEffect => f.remove_effect = !f.remove_effect,
                Flag::ShowNoteChart => f.show_notechart = !f.show_notechart,
            }
        }
        Command::Transpose(semi) => state.doc.transpose(semi),
        Command::SetAllInstrument(ins) => state.doc.set_instrument_all(ins),
        Command::BatchVelocity(v) => {
            let v = v.min(VEL_MAX);
            state
                .doc
                .batch_change(tr("Set velocity", "批量设置力度"), |n| n.velocity = v);
        }
        Command::BatchPanning(v) => {
            let v = v.min(PAN_MAX);
            state
                .doc
                .batch_change(tr("Set panning", "批量设置声像"), |n| n.panning = v);
        }
        Command::BatchPitch(v) => {
            let v = v.clamp(-PIT_MAX, PIT_MAX);
            state
                .doc
                .batch_change(tr("Set pitch", "批量设置微调"), |n| n.pitch = v);
        }
        Command::SetTempo(t) => {
            // 原版两条写速度的路径都夹在 `median(0.25, 值, 180)`
            // （`draw_window_set_tempo.gml:42`、`control_draw.gml:2532`），端口统一放在这里。
            state.doc.edit_meta(tr("Change tempo", "修改速度"), |m| {
                m.tempo = t.clamp(0.25, 180.0)
            });
            state.audio.clock_mut().set_tempo(state.doc.song.meta.tempo);
        }
        Command::SetTimeSignature(ts) => {
            state
                .doc
                .edit_meta(tr("Change time signature", "修改拍号"), |m| {
                    m.time_signature = ts.clamp(2, 8)
                });
        }
        Command::OpenSaveOptions => {
            state.save_options_version = state.doc.song.meta.save_version;
            dispatch(state, Command::ShowWindow(WindowKind::SaveOptions), ctx);
        }
        Command::ApplySaveOptions { version } => {
            if version != nbs_core::nbs::NBS_VERSION {
                // 原版 `question(...)` 的否分支：`save_options_version =
                // cursong.save_version` 然后 return，窗口留着。这里提前把单选退回
                // 歌曲版本，答「是」的分支再用手上的 version 覆盖回来。
                state.save_options_version = state.doc.song.meta.save_version;
                state.modals = ModalQueue::question(
                    tr("Confirm", "确定"),
                    tr(
                        "Some of the song's data will be lost if you save in a previous version! Are you sure?",
                        "如果你保存在旧版本的话一些歌曲信息将会丢失！你确定吗？",
                    ),
                    Command::SaveOptionsConfirm { version },
                );
            } else {
                dispatch(state, Command::SaveOptionsConfirm { version }, ctx);
            }
        }
        Command::SaveOptionsConfirm { version } => {
            state.save_options_version = version;
            state.doc.edit_meta(
                tr("Change save version", "修改保存版本"),
                |m: &mut SongMeta| {
                    m.save_version = version;
                },
            );
            dispatch(state, Command::CloseWindow, ctx);
        }
        Command::ClearRecent => {
            state.recent_files.clear();
        }
        Command::ClearSong => {
            state.doc.clear_all_notes();
            state.prepare_audio();
        }
        Command::HotkeyWindow(kind) => {
            // 原版这一族每一条都是 `playing = 0` 加一句 `window = ...`。
            // 播放中不重新起播，所以和 ShowCompat 一样直接 pause，而不是走 PlayPause。
            state.audio.clock_mut().pause();
            dispatch(state, Command::ShowWindow(kind), ctx);
        }
        Command::HotkeyMacro(m) => {
            state.audio.clock_mut().pause();
            dispatch(state, Command::RunMacro(m), ctx);
        }
        Command::ShowCompat => {
            // 原版这一段（`control_draw.gml:2032-2036`）：playing = 0、record = 0、
            // selection_place(0)，然后 window = w_minecraft。
            state.audio.clock_mut().pause();
            state.record = false;
            state.record_armed = false;
            state.doc.deselect();
            dispatch(state, Command::ShowWindow(WindowKind::MinecraftCompat), ctx);
        }
        Command::FixTempoSchematic => {
            // 原版 `draw_window_minecraft.gml` 的连续 `if`：后面的覆盖前面的，顺序照抄。
            let o = state.doc.song.meta.tempo;
            let mut t = o;
            if o > 10.0 {
                t = 10.0;
            }
            if o < 10.0 {
                t = 10.0;
            }
            if o < 7.5 {
                t = 5.0;
            }
            if o < 3.75 {
                t = 2.5;
            }
            if t != o {
                dispatch(state, Command::SetTempo(t), ctx);
            }
        }
        Command::FixTempoDatapack => {
            // 同上，数据包页那一串更细的阶梯；条件一律看改动前的原速度。
            let o = state.doc.song.meta.tempo;
            let mut t = o;
            if o >= 15.0 {
                t = 20.0;
            }
            if o < 15.0 {
                t = 10.0;
            }
            if o < 7.5 {
                t = 5.0;
            }
            if o < 4.5 {
                t = 4.0;
            }
            if o < 3.25 {
                t = 2.5;
            }
            if o < 2.25 {
                t = 2.0;
            }
            if o < 1.625 {
                t = 1.25;
            }
            if o < 1.125 {
                t = 1.0;
            }
            if o < 0.75 {
                t = 0.5;
            }
            if o < 0.375 {
                t = 0.25;
            }
            if t != o {
                dispatch(state, Command::SetTempo(t), ctx);
            }
        }
        Command::SelectOutside { lower, higher } => {
            state.doc.select_outside(lower, higher);
            state.set_message(if crate::lang() == 1 {
                format!("已选中 {} 个音符", state.doc.selection.len())
            } else {
                format!("Selected {} notes", state.doc.selection.len())
            });
            state.window = None;
        }
        Command::SelectCustom => {
            state.doc.select_custom();
            state.set_message(if crate::lang() == 1 {
                format!("已选中 {} 个音符", state.doc.selection.len())
            } else {
                format!("Selected {} notes", state.doc.selection.len())
            });
            state.window = None;
        }
        Command::TransposeToMinecraft => {
            state.modals = ModalQueue::question(
                tr("Transpose notes", "转换音符"),
                tr(
                    "Transpose notes so that they fall within Minecraft's 2 octaves?",
                    "转换音符使其在 Minecraft 的 2 八度限制内吗？",
                ),
                Command::TransposeToMinecraftApply,
            );
        }
        Command::TransposeToMinecraftApply => {
            // 原版这一段：select_all(-1, 0) → selection_transpose() → selection_place(0)。
            state.doc.select_all();
            let edited = state.doc.fold_keys_to_minecraft();
            state.doc.deselect();
            // 原版 `selection_transpose` 的报数是写死的英文串，语言开关影响不到它；
            // 空选区时它直接 return，连这句都不弹。
            if let Some(edited) = edited {
                state.modals =
                    ModalQueue::message("Transpose notes", &format!("{edited} notes were edited."));
            }
        }
        Command::SaveExtraNotesPack => {
            dispatch(
                state,
                Command::OpenDialog(DialogAction::SaveExtraNotes),
                ctx,
            );
        }
        Command::WheelOnNote {
            tick,
            layer,
            delta,
            shift,
        } => {
            if let Some(mut n) = state.doc.song.get(tick, layer) {
                match state.tool {
                    EditMode::Key => {
                        let step = if shift { 12 } else { 1 };
                        n.key = (n.key as i32 + delta * step).clamp(0, KEY_MAX as i32) as u8;
                    }
                    EditMode::Velocity => {
                        let step = if shift { 1 } else { 10 };
                        n.velocity =
                            (n.velocity as i32 + delta * step).clamp(0, VEL_MAX as i32) as u8;
                    }
                    EditMode::Panning => {
                        let step = if shift { 1 } else { 10 };
                        n.panning =
                            (n.panning as i32 + delta * step).clamp(0, PAN_MAX as i32) as u8;
                    }
                    EditMode::Pitch => {
                        let step = if shift { 10 } else { 100 };
                        n.pitch = (n.pitch as i32 + delta * step)
                            .clamp(-PIT_MAX as i32, PIT_MAX as i32)
                            as i16;
                    }
                }
                state.doc.change_note(tick, layer, n);
                state.current.instrument = n.instrument;
                state.current.key = n.key;
                state.current.velocity = n.velocity;
                state.current.panning = n.panning;
                state.current.pitch = n.pitch;
            }
        }
        // ------------------------------------------------------------ 导入导出 / 音频 / 宏
        Command::OpenPrefsFolder(folder) => {
            let dir = match folder {
                PrefsFolder::Song => state.prefs.song_folder_path(),
                PrefsFolder::Pattern => state.prefs.pattern_folder_path(),
            };
            // 原版 `if (!directory_exists_lib(…)) message("该路径不存在！", "错误") else open_url(…)`。
            if !dir.is_dir() {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    tr("The indicated folder doesn't exist!", "该路径不存在！"),
                );
            } else if open_in_file_manager(&dir).is_err() {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    tr("Could not open the file manager.", "无法打开文件管理器。"),
                );
            }
        }
        Command::ResetPrefsFolder(folder) => match folder {
            PrefsFolder::Song => state.prefs.song_folder = None,
            PrefsFolder::Pattern => state.prefs.pattern_folder = None,
        },
        Command::AccentUseDefault => {
            // :133-136：这支「默认」是 (0,120,215)，和 control_create.gml:202-204 的
            // 出厂值 (0,120,212) 不一样。原版两处本来就不同，照抄不改。
            state.theme.set_accent_base([0, 120, 215]);
            state.accent_pick = Some(crate::state::AccentPick::from_rgb([0, 120, 215]));
        }
        Command::ChangePrefsFolder(folder) => {
            // 原版按钮里先 `message(说明, "")`，再弹 `get_save_filename_ext`。
            state.modals = ModalQueue {
                items: [Modal {
                    title: "",
                    text: folder.tip().into(),
                    yes_no: false,
                }]
                .into_iter()
                .collect(),
                then: Some(Command::OpenDialog(DialogAction::PickFolder(folder))),
            };
        }
        Command::OpenDialog(action) => {
            let initial = if action.is_save() {
                state.doc.song.filename.clone()
            } else {
                None
            };
            // 原版每个对话框的起始目录都写死在调用处：歌曲看 `songfolder`、片段看
            // `patternfolder`（`load_song.gml:11`、`pattern_export.gml:9`），当前文件在哪个
            // 目录它们一概不看；只有音频导出用 `filename_path(filename)`。
            let start: Option<PathBuf> = match action {
                DialogAction::OpenSong
                | DialogAction::SaveSong
                | DialogAction::PickFolder(PrefsFolder::Song) => {
                    Some(state.prefs.song_folder_path())
                }
                DialogAction::ImportPattern
                | DialogAction::ExportPattern
                | DialogAction::PickFolder(PrefsFolder::Pattern) => {
                    Some(state.prefs.pattern_folder_path())
                }
                // 原版 `get_directory(mc_install_path)`：起始目录就是当前那个 .minecraft。
                DialogAction::PickMinecraftDir => {
                    let p = PathBuf::from(&state.sound_import.mc_install_path);
                    p.is_dir().then_some(p)
                }
                _ => None,
            };
            let mut dialog = FileDialog::for_action(action, initial.as_ref(), start.as_deref());
            if action == DialogAction::ExportAudio {
                // 原版把保存对话框的文件类型绑在所选导出格式上（.mp3/.wav/.aiff/.flac）。
                if let Some(f) = audio_format(state) {
                    dialog.set_ext(f.extension());
                }
            }
            if action == DialogAction::ExportDatapack {
                // 原版：开 ZIP 时对话框是 `ZIP archive (*.zip)`、默认名 `dat_name + ".zip"`；
                // 关掉 ZIP 时过滤器是空的“数据包目录”，默认名就是 `dat_name`（没有扩展名）。
                let zip = state.datapack_options.use_zip;
                dialog.set_ext(if zip { "zip" } else { "" });
                if !state.datapack_options.name.is_empty() {
                    dialog.name = if zip {
                        format!("{}.zip", state.datapack_options.name)
                    } else {
                        state.datapack_options.name.clone()
                    };
                    dialog.refresh();
                }
            }
            if action == DialogAction::ExportSchematic {
                // 原版：经典格式 `filename + ".schematic"`，结构方块把文件名转小写、
                // 空格换成下划线再加 ".nbt"。
                let structure = state.schematic_options.structure;
                dialog.set_ext(if structure { "nbt" } else { "schematic" });
                if structure {
                    let stem = std::path::Path::new(&dialog.name)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    dialog.name = format!("{}.nbt", stem.to_lowercase().replace(' ', "_"));
                    dialog.refresh();
                }
            }
            if action == DialogAction::ExportTrack {
                // 原版 track_export.gml:8-11：默认名规则与 Schematic 导出一致。
                let structure = state.schematic_options.structure;
                dialog.set_ext(if structure { "nbt" } else { "schematic" });
                if structure {
                    let stem = std::path::Path::new(&dialog.name)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    dialog.name = format!("{}.nbt", stem.to_lowercase().replace(' ', "_"));
                    dialog.refresh();
                }
            }
            if action == DialogAction::SaveExtraNotes {
                // 原版 `datapack_getextranotes.gml:6`：默认名写死 "Extra Note Blocks"（中文
                // 分支是「更多音符盒」），跟当前歌曲完全无关；扩展名交给对话框的 *.zip 过滤器补。
                dialog.name = tr("Extra Note Blocks", "更多音符盒").to_string();
                dialog.refresh();
            }
            state.request_file_dialog(dialog);
        }
        Command::DialogConfirm(path) => {
            let action = state
                .file_dialog
                .as_ref()
                .map(|d| d.action)
                .unwrap_or(DialogAction::OpenSong);
            match action {
                DialogAction::OpenSong => dispatch(state, Command::OpenPath(path), ctx),
                DialogAction::SaveSong => dispatch(state, Command::SavePath(path), ctx),
                DialogAction::PickFolder(folder) => {
                    // 原版 `songfolder = filename_dir(a)`：认的是对话框当时所在的那个目录。
                    // `windows.rs` 的目录模式直接把 `dialog.dir` 传过来，不带文件名。
                    match folder {
                        PrefsFolder::Song => {
                            state.prefs.song_folder = Some(path.to_string_lossy().into_owned())
                        }
                        PrefsFolder::Pattern => {
                            state.prefs.pattern_folder = Some(path.to_string_lossy().into_owned())
                        }
                    }
                    state.file_dialog = None;
                    // 原版的系统对话框叠在首选项窗口上；我们同时只开一个窗口，
                    // 选完把首选项放回去（页签存在 `state.prefs_tab`，不会丢）。
                    state.window = Some(WindowKind::Preferences);
                }
                DialogAction::PickMinecraftDir => {
                    // 原版：`mc_install_path = filename_dir(fn)`（再清掉泄漏的标题）；
                    // 目录选择器直接给目录，等价于清理后的结果。
                    state.sound_import.mc_install_path = path.to_string_lossy().into_owned();
                    state.sound_import.update_menu();
                    state.file_dialog = None;
                    state.window = Some(WindowKind::SoundImport);
                }
                DialogAction::ExportAudio => {
                    export_audio(state, &path);
                    state.window = None;
                }
                DialogAction::ExportMidi => {
                    match nbs_io::midi::export_midi(&state.doc.song, &path) {
                        Ok(()) => state.set_message(tr("MIDI exported", "MIDI 已导出")),
                        Err(e) => state.set_message(if crate::lang() == 1 {
                            format!("导出 MIDI 失败: {e}")
                        } else {
                            format!("MIDI export failed: {e}")
                        }),
                    }
                    state.window = None;
                }
                DialogAction::ExportPng => {
                    let opts = state.png_options.clone();
                    match nbs_io::image_export::export_png(&state.doc.song, &path, &opts) {
                        Ok((w, h)) => {
                            let msg = if crate::lang() == 1 {
                                format!("PNG 谱面已导出：{w}×{h}")
                            } else {
                                format!("PNG chart exported: {w}×{h}")
                            };
                            state.export_status = Some(msg.clone());
                            state.set_message(msg);
                        }
                        Err(e) => state.set_message(if crate::lang() == 1 {
                            format!("导出 PNG 失败: {e}")
                        } else {
                            format!("PNG export failed: {e}")
                        }),
                    }
                    state.window = None;
                }
                DialogAction::ExportSchematic => {
                    let opts = state.schematic_options.clone();
                    let path = if opts.structure {
                        path.with_extension("nbt")
                    } else {
                        path.with_extension("schematic")
                    };
                    state.window = None;
                    // 原版 `schematic_export.gml:16-17`：命令模式先以全预算重算
                    // 声源分配方案（get_schematic_plan(true)），校验通过才落盘。
                    let command_mode = opts.structure && opts.command_block && opts.layout < 2;
                    let plan = if command_mode {
                        // 克隆一份绕开与 song 的借用冲突；导出每次只做一次，代价可忽略。
                        Some(state.rebuild_command_plan_forced().clone())
                    } else {
                        None
                    };
                    match nbs_io::schematic::export_schematic(
                        &state.doc.song,
                        &path,
                        &opts,
                        plan.as_ref(),
                    ) {
                        // 原版：保存成功后 `message("Schematic 已保存！")`，
                        // 关掉提示再把导出窗口重新打开。
                        Ok(info) => {
                            // 原版 `schematic_export.gml:1107`：命令模式附声源分配报告。
                            if let Some(p) = plan.as_ref() {
                                for line in nbs_io::mc_export::log_report(
                                    p,
                                    &opts.command_source,
                                    "Standard command-block structure",
                                ) {
                                    state.log_strs.push(line);
                                }
                            }
                            state.set_message(if crate::lang() == 1 {
                                format!(
                                    "Schematic 已保存：{}×{}×{}，{} 个音符盒",
                                    info.length, info.width, info.height, info.note_blocks
                                )
                            } else {
                                format!(
                                    "Schematic saved: {}×{}×{}, {} note blocks",
                                    info.length, info.width, info.height, info.note_blocks
                                )
                            });
                            let mut q = ModalQueue::message(
                                crate::tr("Schematic Export", "导出 Schematic"),
                                crate::tr("Schematic saved!", "Schematic 已保存！"),
                            );
                            q.then = Some(Command::ShowWindow(WindowKind::SchematicExport));
                            state.modals = q;
                        }
                        Err(e) => state.set_message(if crate::lang() == 1 {
                            format!("导出 Schematic 失败: {e}")
                        } else {
                            format!("Schematic export failed: {e}")
                        }),
                    }
                }
                DialogAction::ExportTrack => {
                    // 原版 track_export()：对话框确认后才关窗、做命令模式闸门，再生成。
                    let opts = state.schematic_options.clone();
                    let structure = opts.structure;
                    let path = if structure {
                        path.with_extension("nbt")
                    } else {
                        path.with_extension("schematic")
                    };
                    state.window = None;
                    // 原版 track_export.gml:14-29：命令模式先取全预算方案并补 9 条
                    // 车道表，两道闸门都过才生成；失败只弹消息，窗口保持关闭。
                    let mut plan: Option<nbs_io::mc_export::Plan> = None;
                    let mut blocked: Option<ModalQueue> = None;
                    if structure && opts.command_block {
                        let mut p = state.rebuild_command_plan_forced().clone();
                        nbs_io::mc_export::prepare_track_plan(&mut p, &state.doc.song);
                        if p.rows.is_empty() {
                            blocked = Some(ModalQueue::message(
                                tr("Schematic export", "导出结构"),
                                tr(
                                    "There are no sound commands to export!",
                                    "没有声音命令可以导出！",
                                ),
                            ));
                        } else {
                            let planned_height =
                                20i64.max(((p.track_maximum_slots as i64 + 3) / 4) * 4 + 4);
                            let planned_length = 40 + p.last_command_grid * 2;
                            if planned_length > 2147483647 || planned_height > 2147483647 {
                                blocked = Some(ModalQueue::message(
                                    tr("Error", "错误"),
                                    tr(
                                        "The Track dimensions exceed the signed 32-bit Structure NBT limit.",
                                        "直轨尺寸超过了结构 NBT 的 32 位有符号整数限制。",
                                    ),
                                ));
                            } else {
                                plan = Some(p);
                            }
                        }
                    }
                    if let Some(q) = blocked {
                        state.modals = q;
                    } else if let Err(e) = nbs_io::schematic::export_track(
                        &state.doc.song,
                        &opts,
                        plan.as_ref(),
                        &path,
                    ) {
                        state.set_message(if crate::lang() == 1 {
                            format!("导出直轨失败: {e}")
                        } else {
                            format!("Track export failed: {e}")
                        });
                    } else {
                        // 原版 `track_export.gml:1401`：命令模式附声源分配报告。
                        if let Some(p) = plan.as_ref() {
                            for line in nbs_io::mc_export::log_report(
                                p,
                                &opts.command_source,
                                "Track command-block structure",
                            ) {
                                state.log_strs.push(line);
                            }
                        }
                        let mut q = ModalQueue::message(
                            crate::tr("Track Export", "导出直轨"),
                            if structure {
                                crate::tr("Structure saved!", "结构已保存！")
                            } else {
                                crate::tr("Schematic saved!", "Schematic 已保存！")
                            },
                        );
                        q.then = Some(Command::ShowWindow(WindowKind::TrackExport));
                        state.modals = q;
                    }
                }
                DialogAction::ExportDatapack => {
                    export_datapack(state, &path);
                    // 原版导出完把窗口重新打开（`window = w_datapack_export`）。
                    state.window = Some(WindowKind::DatapackExport);
                }
                DialogAction::ExportPattern => {
                    let sel = if state.doc.selection.is_empty() {
                        let items: Vec<_> = state.doc.song.iter_notes().collect();
                        nbs_core::Selection::from_absolute(0, 0, &items)
                    } else {
                        state.doc.selection.clone()
                    };
                    match nbs_io::pattern::export_pattern(&sel, &state.doc.song, &path) {
                        Ok(()) => state.set_message(if crate::lang() == 1 {
                            format!("片段已导出：{} 音符", sel.len())
                        } else {
                            format!("Pattern exported: {} notes", sel.len())
                        }),
                        Err(e) => state.set_message(if crate::lang() == 1 {
                            format!("导出片段失败: {e}")
                        } else {
                            format!("Pattern export failed: {e}")
                        }),
                    }
                    state.window = None;
                }
                DialogAction::ImportMidi => {
                    // 原版 `open_midi()`：先读盘解析，解析成功才开导入窗口，
                    // 并把当前 GM 映射表摊到界面四张表上（不落歌，等「导入」）。
                    match nbs_io::midi::parse_midi_path(&path) {
                        Ok(midi) => {
                            state.midi_options.reload_tables(&midi, &state.midi_gm);
                            state.midi_enhanced = nbs_io::midi_enhanced::EnhancedMidiOptions::for_midi(&midi);
                            state.midi_enhanced_options = state.midi_options.clone();
                            state.midi_enhanced_check = None;
                            state.midi_data = Some(midi);
                            state.midi_file = path
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            state.midi_tab = 0;
                            state.midi_scroll = [0.0; 4];
                            state.window = Some(WindowKind::MidiImport);
                        }
                        Err(e) => {
                            state.window = None;
                            state.set_message(if crate::lang() == 1 {
                                format!("导入 MIDI 失败: {e}")
                            } else {
                                format!("MIDI import failed: {e}")
                            });
                        }
                    }
                }
                DialogAction::ImportSchematic => {
                    import_schematic(state, &path);
                    state.window = None;
                }
                DialogAction::ImportPattern => {
                    match nbs_io::pattern::import_pattern(&path) {
                        Ok(pat) => {
                            // 原版 `pattern_import.gml:31-37`：按文件头的版本字节决定警告。
                            let warn = nbs_io::pattern::version_warning(
                                pat.version,
                                state.prefs.show_old_warning,
                            );
                            let sel = pat.selection();
                            let n = sel.len();
                            state.doc.clipboard_text = sel.encode_rle();
                            state.doc.clipboard = sel.notes;
                            let (x, y) = state
                                .hover_cell
                                .map(|(t, l)| (t as i64, l as i64))
                                .unwrap_or((
                                    state.viewport.start_tick as i64,
                                    state.viewport.start_layer as i64,
                                ));
                            state.doc.paste(x, y);
                            state.window = None;
                            if let Some(warn) = warn {
                                state.modals =
                                    ModalQueue::message(crate::tr("Warning", "警告"), &warn);
                            }
                            state.set_message(if crate::lang() == 1 {
                                format!("片段已导入：{n} 个音符（浮动选区）")
                            } else {
                                format!("Pattern imported: {n} notes (floating selection)")
                            });
                        }
                        Err(e) => state.set_message(if crate::lang() == 1 {
                            format!("导入片段失败: {e}")
                        } else {
                            format!("Pattern import failed: {e}")
                        }),
                    }
                }
                DialogAction::ImportBackground => {
                    // 原版 `menu_click.gml:57-58` → `wallpaper_init(temppath)`：读完下一次
                    // Draw 烘焙生效，不写最近列表、不进 .nbs、不落配置。
                    let baked = crate::background::bake(ctx, &path);
                    if let Err(e) = &baked {
                        // 原版读失败是静默的（`sprite_exists` 为假就什么都不画），
                        // 端口把原因写进状态栏，免得只剩「点了没反应」。
                        state.set_message(if crate::lang() == 1 {
                            format!("导入背景图片失败: {e}")
                        } else {
                            format!("Background image import failed: {e}")
                        });
                    }
                    // 原版是先删旧精灵再 `sprite_add`，读失败时旧背景也一起没了，这里同样整体覆盖。
                    state.wallpaper = baked.ok();
                    state.window = None;
                }
                DialogAction::SaveExtraNotes => {
                    // 原版 `datapack_getextranotes()`：复制完 `message("资源包保存成功！", "保存")`，
                    // 调用它的那个窗口原样留着（原生组件只是叠在上面）。走自绘兜底框时
                    // 那个窗口被顶掉过，用 `dialog_return` 放回去。
                    match nbs_io::datapack::save_extra_notes_pack(&path) {
                        Ok(()) => {
                            if let Some(w) = state.dialog_return.take() {
                                state.window = Some(w);
                            }
                            state.modals = ModalQueue::message(
                                crate::tr("Saved", "保存"),
                                crate::tr("Resource pack saved successfully!", "资源包保存成功！"),
                            );
                        }
                        Err(e) => state.set_message(if crate::lang() == 1 {
                            format!("保存资源包失败: {e}")
                        } else {
                            format!("Resource pack save failed: {e}")
                        }),
                    }
                }
            }
            state.file_dialog = None;
        }
        Command::RunMacro(m) => {
            let label = m.label().to_string();
            state.doc.run_macro(&m);
            state.prepare_audio();
            state.set_message(if crate::lang() == 1 {
                format!("已应用宏：{label}")
            } else {
                format!("Macro applied: {label}")
            });
        }
        Command::MacroWindowOk { m, place } => {
            dispatch(state, Command::RunMacro(m), ctx);
            if place {
                state.doc.place_selection();
            }
            dispatch(state, Command::CloseWindow, ctx);
        }
        Command::SetMasterVolume(v) => {
            state.prefs.master_volume = v.clamp(0.0, 2.0);
            state.apply_audio_prefs();
        }
        Command::SetOutput(wanted) => state.apply_output_choice(wanted),
        Command::SetPlaySpeed(v) => {
            state.prefs.play_speed = v.clamp(0.25, 4.0);
            state.apply_audio_prefs();
        }
        Command::ToggleMetronome => {
            state.prefs.metronome = !state.prefs.metronome;
            state.apply_audio_prefs();
        }
        Command::ToggleExtendedVoices => {
            state.prefs.extended_voices = !state.prefs.extended_voices;
            state
                .audio
                .clock_mut()
                .set_max_voices(if state.prefs.extended_voices {
                    EXTENDED_MAX_VOICES
                } else {
                    DEFAULT_MAX_VOICES
                });
        }
        Command::ToggleMono => {
            state.prefs.mono = !state.prefs.mono;
            state.apply_audio_prefs();
        }
        Command::SetSoundsDir(dir) => {
            state.prefs.sounds_dir = if dir.trim().is_empty() {
                None
            } else {
                Some(dir)
            };
            dispatch(state, Command::ReloadSamples, ctx);
        }
        Command::ReloadSamples => {
            let dir = state.prefs.sounds_dir_path();
            state.audio.reload_bank(dir.as_deref());
            state.prepare_audio();
            // 原版 `set_resourcepack.gml:462`：音色变化使声源分配方案失效。
            state.invalidate_command_plan();
            state.set_message(if crate::lang() == 1 {
                format!("采样已重新加载：{}", state.audio.sample_status())
            } else {
                format!("Samples reloaded: {}", state.audio.sample_status())
            });
        }
        Command::SavePreferences => {
            state.save_prefs();
            state.set_message(crate::tr("Preferences saved", "偏好设置已保存"));
        }
        // 原版 `load_song(fn, 1)` 那一档：还原自动备份既不弹歌曲信息，也不写最近列表。
        Command::RestoreAutosave(path) => open_nbs_at(state, &path, true),
        // 原版 `open_url(url)`：欢迎窗 / 关于窗 / 更新弹窗里所有跳浏览器的链接都走这里。
        // 单测里不真开浏览器，把 url 记进下面的线程局部槽，由用例 `take_open_url()` 取。
        Command::OpenUrl(url) => {
            #[cfg(test)]
            OPEN_URL.with(|slot| *slot.borrow_mut() = Some(url.clone()));
            #[cfg(not(test))]
            open_url(&url);
        }
        // 原版开发版确认框答「是」后的 `check_prerelease = !check_prerelease` +
        // 隐式落盘（原版在退出时统一 save_settings）。
        Command::SetCheckPrerelease(on) => {
            state.prefs.check_prerelease = on;
            dispatch(state, Command::SavePreferences, ctx);
        }
        Command::OpenSoundImport => {
            // 原版 menu_click.gml:196：`window = w_sound_import update_asset_index_menu(0)`。
            state.sound_import.update_menu();
            dispatch(state, Command::ShowWindow(WindowKind::SoundImport), ctx);
        }
        Command::SoundImportToggleDownload => {
            // 原版：翻转后 `sound_import_asset_indexes = []` + update_asset_index_menu()。
            let si = &mut state.sound_import;
            si.download_toggle = !si.download_toggle;
            si.indexes.clear();
            si.update_menu();
        }
        Command::SoundImportGetIndexes => {
            // 原版 Get 按钮：先 `question(EULA)`，答「是」才 stage = 1 起请求。
            state.modals = ModalQueue::question(
                tr("Import sounds", "导入音效"),
                tr(
                    "By pressing \"Yes\", you agree to the Minecraft EULA and the Microsoft Service Agreement:\nhttps://www.minecraft.net/eula\nhttps://www.microsoft.com/en-us/servicesagreement",
                    "如果你选择“是”，即代表你同意 Minecraft EULA 及 Microsoft 服务协议：\nhttps://www.minecraft.net/zh-hans/eula\nhttps://www.microsoft.com/zh-cn/servicesagreement",
                ),
                Command::SoundImportStartVersionList,
            );
        }
        Command::SoundImportStartVersionList => {
            let si = &mut state.sound_import;
            let Some(fetcher) = si.fetcher.clone() else {
                return;
            };
            if si
                .pending
                .as_ref()
                .is_some_and(|h| h.shared.lock().map(|g| !g.finished).unwrap_or(true))
            {
                // 原版允许重复点击叠加请求；这里有线程在跑就不叠加（偏差已记账）。
                return;
            }
            let handle = crate::sound_import::spawn_download(
                fetcher,
                crate::sound_import::DownloadMode::VersionList,
            );
            si.pending = Some(handle);
        }
        Command::SoundImportSelectIndex(i) => {
            let si = &mut state.sound_import;
            si.select = i;
            si.update_menu();
        }
        Command::SoundImportChangeDir => {
            dispatch(
                state,
                Command::OpenDialog(DialogAction::PickMinecraftDir),
                ctx,
            );
        }
        Command::SoundImportUseDefaultPath => {
            let si = &mut state.sound_import;
            si.mc_install_path = si.mc_default_path.clone();
            si.update_menu();
        }
        Command::SoundImportGet => {
            let si = &mut state.sound_import;
            if !si.download_toggle {
                // 原版 `load_asset_index(true)`：覆盖确认 → 删目录 → 拷贝，全程同步。
                si.sounds_root = crate::sound_import::sounds_dir(&state.prefs);
                match si.load_asset_index(true) {
                    crate::sound_import::LocateOutcome::MissingIndex => {
                        state.modals = ModalQueue::message(
                            tr("Note Block Studio", "Note Block Studio"),
                            tr(
                                "The file for the specified asset index could not be found!",
                                "未找到该索引所指向的文件！",
                            ),
                        );
                    }
                    crate::sound_import::LocateOutcome::NeedsReplace(matching) => {
                        state.modals = ModalQueue::question(
                            tr("Warning", "警告"),
                            tr(
                                "An existing folder with imported Minecraft sounds has been found in your Sounds folder. Would you like to replace it?",
                                "在您的Sounds文件夹中发现已经存在一个包含已导入Minecraft音效的文件夹，您想要覆盖它吗？",
                            ),
                            Command::SoundImportCopyGo,
                        );
                        state.sound_import.staged = matching;
                    }
                    crate::sound_import::LocateOutcome::None => {}
                }
            } else {
                let root = crate::sound_import::sounds_dir(&state.prefs);
                let si = &mut state.sound_import;
                let Some(fetcher) = si.fetcher.clone() else {
                    return;
                };
                if si.files_index == 0 {
                    // 原版：`files_index = 0` → stage 2 从版本 JSON 重新走。
                    let Some(url) = si.version_urls.get(si.select).cloned() else {
                        return;
                    };
                    if si
                        .pending
                        .as_ref()
                        .is_some_and(|h| h.shared.lock().map(|g| !g.finished).unwrap_or(true))
                    {
                        return;
                    }
                    let handle = crate::sound_import::spawn_download(
                        fetcher,
                        crate::sound_import::DownloadMode::Import {
                            version_url: url,
                            sounds_root: root,
                        },
                    );
                    si.pending = Some(handle);
                } else if si.files_index < si.files_total {
                    // 原版：`files_index < array_length(files_list)` → stage 4 续传。
                    let Some(handle) = si.pending.clone() else {
                        return;
                    };
                    {
                        let Ok(mut g) = handle.shared.lock() else {
                            return;
                        };
                        if !g.finished {
                            return;
                        }
                        g.failed = false;
                        g.finished = false;
                        g.stage = 4;
                        g.status = 1;
                    }
                    crate::sound_import::spawn_on(
                        handle,
                        fetcher,
                        crate::sound_import::DownloadMode::Resume,
                    );
                }
            }
        }
        Command::SoundImportCopyGo => {
            let root = crate::sound_import::sounds_dir(&state.prefs);
            state.sound_import.copy_staged(&root);
        }
        Command::SoundImportReplaceAnswered => {
            crate::sound_import::answer_replace(&mut state.sound_import, true);
        }
        Command::SoundImportOpenSoundsFolder => {
            let dir = crate::sound_import::sounds_dir(&state.prefs);
            dispatch(
                state,
                Command::OpenUrl(dir.to_string_lossy().into_owned()),
                ctx,
            );
        }
        Command::SoundImportOk => {
            state.sound_import.status = 0;
            dispatch(state, Command::CloseWindow, ctx);
        }
    }
}

// `Command::OpenUrl` 的落地槽：单测里线程局部记一条，用例各跑各的、不会串台。
#[cfg(test)]
thread_local! {
    static OPEN_URL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// 取走上一条被拦下的 `OpenUrl`（没有则 `None`），同时清空。
#[cfg(test)]
pub(crate) fn take_open_url() -> Option<String> {
    OPEN_URL.with(|slot| slot.borrow_mut().take())
}

/// 原版 `open_url(url)`（macros.gml:1-3）：用系统默认方式打开链接。
/// Windows 的 `start` 第一个带引号参数是窗口标题，所以要垫一个空串。
#[cfg(not(test))]
fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// 原版 `load_song.gml:38-70` 的 `.nbs`/`.zip` 落地段。`backup` 就是原版的第二个实参
/// （`download_song_from_url.gml:65` 那句注释写得很直白：load as backup file —
/// keep unsaved, don't add to recent etc.）。端口的自动备份还原窗是自加的，
/// 语义正对得上这一档，所以走 `backup = true`。
fn open_nbs_at(state: &mut EditorState, path: &std::path::Path, backup: bool) {
    match nbs_core::nbs::load(path) {
        Ok(song) => {
            let name = song.meta.name.clone();
            // 原版 `open_song_nbs.gml:12-13`：文件版本比 `nbs_version` 旧就先警告。
            // 比它新的文件根本进不来这一支（加载器直接报「不支持的 NBS 版本」，
            // 对应源码 :14-18 的报错并 return -1）。
            let old =
                song.meta.save_version < nbs_core::nbs::NBS_VERSION && state.prefs.show_old_warning;
            let text = nbs_io::prefs::old_file_warning("NBS");
            // 原版 `load_song.gml:64` 的 `window != w_instruments` 判的是换歌**之前**
            // 开着的那扇窗，所以要在 `replace_song` 前先记下来。
            let on_instruments = state.window == Some(WindowKind::Instruments);
            state.doc.replace_song(song);
            state.song_session += 1;
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.clear_section();
            if backup {
                // 原版 :68-70：备份档不写最近列表，反而把 `changed` 标成 1（还原出来算未保存）。
                state.doc.song.dirty = true;
                state.window = None;
            } else {
                state.add_recent(path.to_path_buf());
                // 原版 `load_song.gml:64`：歌名非空才弹歌曲信息；名字空的原版什么都不弹。
                // 弹的是 `w_songinfo`（点任意处关闭那一版），不是工具栏开的带按钮版。
                // 两个条件不满足时端口保持原有行为——把上一扇窗关掉，不留着一窗旧数据。
                state.window = if !on_instruments && !name.is_empty() {
                    Some(WindowKind::SongInfo)
                } else {
                    None
                };
            }
            if old {
                state.modals = ModalQueue::message(tr("Warning", "警告"), &text);
            }
            state.set_message(if crate::lang() == 1 {
                format!("已打开 {}（{} 个音符）", name, state.doc.song.total_blocks)
            } else {
                format!("Loaded {} ({} notes)", name, state.doc.song.total_blocks)
            });
        }
        Err(e) => state.set_message(if crate::lang() == 1 {
            format!("打开失败: {e}")
        } else {
            format!("Load failed: {e}")
        }),
    }
}

/// 原版 `filename_ext(fn) = ".schematic"`（GML 的字符串比较不分大小写）。
fn is_schematic_path(path: &std::path::Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase() == "schematic")
        .unwrap_or(false)
}

/// 原版 `open_schematic(fn)`：整个替换当前歌曲，失败弹模态框。
/// 和 MIDI 导入一样不进最近列表（`load_song.gml` 在 `add_to_recent()` 之前就 return 了），
/// 也不写 `song.filename`（原版把 `filename` 指到 `.schematic`，Ctrl+S 会用 NBS 覆盖图样文件）。
fn import_schematic(state: &mut EditorState, path: &std::path::Path) {
    match nbs_io::schematic::import_schematic(path) {
        Ok(info) => {
            let inputs = info.inputs;
            let n = info.added_notes;
            state.doc.replace_song(info.song);
            state.song_session += 1;
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.clear_section();
            state.set_message(if crate::lang() == 1 {
                format!("Schematic 导入完成：{n} 个音符，{inputs} 个输入")
            } else {
                format!("Schematic imported: {n} notes, {inputs} inputs")
            });
            // 原版：`inputam > 1` 时提示几段曲子被首尾接了起来（标题不经 `condstr`，保持英文）。
            if inputs > 1 {
                state.modals = ModalQueue::message(
                    "Schematic import",
                    "The Schematic contained more than one input.\n\n\
                     As a result, the songs of the inputs have been put after each other.",
                );
            }
        }
        // 读不到的错误在原版全是 message()（模态）；只有文件打不开才是静默 return。
        Err(e) => match e.dialog() {
            Some((title, text)) => state.modals = ModalQueue::message(title, text),
            None => state.set_message(if crate::lang() == 1 {
                format!("导入 Schematic 失败: {e}")
            } else {
                format!("Schematic import failed: {e}")
            }),
        },
    }
}

/// 原版 `open_url(folder)`：把这个目录交给系统文件管理器。
/// Windows 的 `explorer` 即使成功也返回非 0，所以只看 `spawn` 是否起来。
fn open_in_file_manager(dir: &std::path::Path) -> std::io::Result<std::process::Child> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(dir).spawn()
}

/// 当前选中的音频导出格式。解析失败时返回 None，让调用方报真实错误，
/// 绝不悄悄退回 WAV（那等于给用户一个内容为 MP3、名字叫 .wav 的产物）。
fn audio_format(state: &EditorState) -> Option<nbs_io::audio_export::AudioFormat> {
    nbs_io::audio_export::AudioFormat::parse(&state.prefs.export_format)
}

/// 音频导出 = 离线渲染 WAV →（非 WAV 格式时）ffmpeg 转码，与原版
/// `mp3_export()` 交给 nbswave/ffmpeg 的分工一致。中间 WAV 落在目标同目录，
/// 转码成功或失败都会删掉。
fn export_audio(state: &mut EditorState, path: &std::path::Path) {
    let Some(format) = audio_format(state) else {
        state.set_message(if crate::lang() == 1 {
            format!("导出音频失败：未知格式 {:?}", state.prefs.export_format)
        } else {
            format!(
                "Audio export failed: unknown format {:?}",
                state.prefs.export_format
            )
        });
        return;
    };
    let opts = ExportOptions {
        sample_rate: state.prefs.export_sample_rate,
        channels: state.prefs.export_channels,
        include_locked: state.prefs.include_locked_in_export,
        ..Default::default()
    };
    let tmp_wav = if format.needs_encoder() {
        Some(path.with_extension("nbs-export-tmp.wav"))
    } else {
        None
    };
    let wav_path = tmp_wav.as_deref().unwrap_or(path);
    let bank = state.audio.bank();
    let stats = match export_wav(&state.doc.song, &bank, wav_path, &opts) {
        Ok(s) => s,
        Err(e) => {
            state.export_status = None;
            state.set_message(if crate::lang() == 1 {
                format!("导出音频失败: {e}")
            } else {
                format!("Audio export failed: {e}")
            });
            return;
        }
    };
    let encoded = if format.needs_encoder() {
        let enc = nbs_io::audio_export::EncodeOptions {
            format,
            sample_rate: opts.sample_rate,
            channels: opts.channels,
        };
        let result = nbs_io::audio_export::encode_from_wav(wav_path, path, &enc);
        let _ = std::fs::remove_file(wav_path);
        result.map_err(|e| e.to_string())
    } else {
        Ok(())
    };
    match encoded {
        Ok(()) => {
            let status = if crate::lang() == 1 {
                format!(
                    "{} 已导出：{} 音符 / {:.1}s / {} 线程 / 峰值 {:.2} → {}",
                    format.name(),
                    stats.notes,
                    stats.duration_seconds,
                    stats.threads,
                    stats.peak,
                    path.display()
                )
            } else {
                format!(
                    "{} exported: {} notes / {:.1}s / {} threads / peak {:.2} → {}",
                    format.name(),
                    stats.notes,
                    stats.duration_seconds,
                    stats.threads,
                    stats.peak,
                    path.display()
                )
            };
            state.export_status = Some(status);
            state.set_message(if crate::lang() == 1 {
                format!("音频已导出到 {}", path.display())
            } else {
                format!("Audio exported to {}", path.display())
            });
        }
        Err(e) => {
            state.export_status = None;
            state.set_message(if crate::lang() == 1 {
                format!("导出音频失败: {e}")
            } else {
                format!("Audio export failed: {e}")
            });
        }
    }
}

/// 数据包导出。原版把 ZIP/目录两分支的扩展名规则写在 `datapack_export()` 里：
/// 开 ZIP 时若选中的文件名不是 `.zip` 就补上，关掉 ZIP 时目标是一个没有 `.zip`
/// 后缀的目录名。这里同样先把路径掰成与选项一致，再交给 `nbs_io::datapack::export`。
fn export_datapack(state: &mut EditorState, path: &std::path::Path) {
    let opts = state.datapack_options.clone();
    let is_zip_ext = path.extension().and_then(|e| e.to_str()) == Some("zip");
    let dest = if opts.use_zip {
        if is_zip_ext {
            path.to_path_buf()
        } else {
            path.with_extension("zip")
        }
    } else if is_zip_ext {
        path.with_extension("")
    } else {
        path.to_path_buf()
    };
    match nbs_io::datapack::export(&state.doc.song, &opts, &dest) {
        Ok(info) => {
            let fp = &info.functionpath;
            let status = if crate::lang() == 1 {
                format!(
                    "数据包已保存到 {}（{} 个文件，{}）\n\n如想在游戏内播放，使用命令：\n/function {}play\n/function {}pause\n/function {}stop\n\n如果你想从你的世界中卸载它，使用命令：\n/function {}uninstall\n\n然后从“datapacks”文件夹取出就行了。",
                    dest.display(),
                    info.files,
                    if info.zip { "ZIP" } else { "目录" },
                    fp,
                    fp,
                    fp,
                    fp
                )
            } else {
                format!(
                    "Data pack saved to {} ({} files, {})\n\nTo play the song in-game, use:\n/function {}play\n/function {}pause\n/function {}stop\n\nIf you wish to uninstall it from your world, use:\n/function {}uninstall\n\nand then remove it from the 'datapacks' folder.",
                    dest.display(),
                    info.files,
                    if info.zip { "ZIP" } else { "Folder" },
                    fp,
                    fp,
                    fp,
                    fp
                )
            };
            state.export_status = Some(status);
            state.set_message(if crate::lang() == 1 {
                format!("数据包已导出：{} 个文件", info.files)
            } else {
                format!("Data pack exported: {} files", info.files)
            });
        }
        Err(e) => {
            state.export_status = None;
            state.set_message(if crate::lang() == 1 {
                format!("导出数据包失败: {e}")
            } else {
                format!("Data pack export failed: {e}")
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Layer;
    use std::path::Path;

    fn enhanced_import_fixture(note: u8) -> nbs_io::midi::MidiData {
        use nbs_io::midi::{MidiData, MidiEvent, MidiPart, MidiTrack};
        MidiData {
            division: 480, minpos: 0, maxpos: 480, micsecqn: 500_000,
            tracks: vec![MidiTrack { name: "Piano".into(), events: vec![MidiEvent {
                pos: 0, channel: 0, note, velocity: 100, patch: 0, part: 0, until: 480,
            }] }],
            parts: vec![MidiPart { track: 0, channel: 0, patch: 0, note_duration: true }],
            ..MidiData::default()
        }
    }

    #[test]
    fn enhanced_import_replaces_song_without_changing_traditional_gm() {
        let _prefs = crate::isolate_prefs();
        let _lock = crate::frame_lock();
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        let midi = enhanced_import_fixture(60);
        state.midi_gm.set_patch(0, 6, -1);
        let gm_before = state.midi_gm.patch(0);
        state.midi_options = nbs_io::MidiImportOptions::for_midi(&midi, &state.midi_gm);
        state.midi_enhanced_options = state.midi_options.clone();
        state.midi_options.use_velocity = false;
        let options_before = state.midi_options.clone();
        state.midi_enhanced = nbs_io::midi_enhanced::EnhancedMidiOptions::for_midi(&midi);
        state.midi_enhanced.enabled = true;
        state.midi_data = Some(midi);
        state.window = Some(WindowKind::MidiImport);
        dispatch(&mut state, Command::MidiImportRun, &ctx);
        assert_eq!(state.doc.song.total_blocks, 2);
        assert!(state.doc.song.iter_notes().all(|(_, _, note)| note.velocity == 78));
        assert_eq!(state.midi_gm.patch(0), gm_before);
        assert_eq!(state.midi_options, options_before);
        assert!(!nbs_io::Preferences::load().midi_import.unwrap().use_velocity);
        assert!(state.midi_data.is_none());
        assert!(state.window.is_none());
    }

    #[test]
    fn enhanced_import_error_keeps_song_and_dialog() {
        let _prefs = crate::isolate_prefs();
        let _lock = crate::frame_lock();
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        let before = nbs_core::nbs::save_bytes(&state.doc.song, nbs_core::nbs::NBS_VERSION).unwrap();
        let midi = enhanced_import_fixture(0);
        state.midi_options = nbs_io::MidiImportOptions::for_midi(&midi, &state.midi_gm);
        state.midi_enhanced_options = state.midi_options.clone();
        state.midi_enhanced = nbs_io::midi_enhanced::EnhancedMidiOptions::for_midi(&midi);
        state.midi_enhanced.enabled = true;
        state.midi_enhanced.range_mode = nbs_io::midi_enhanced::RangeMode::Keep;
        state.midi_enhanced.channels[0].transpose = 0;
        state.midi_data = Some(midi);
        state.window = Some(WindowKind::MidiImport);
        dispatch(&mut state, Command::MidiImportRun, &ctx);
        assert_eq!(before, nbs_core::nbs::save_bytes(&state.doc.song, nbs_core::nbs::NBS_VERSION).unwrap());
        assert!(matches!(state.midi_enhanced_check, Some(Err(_))));
        assert!(state.midi_data.is_some());
        assert_eq!(state.window, Some(WindowKind::MidiImport));
    }

    /// 固定内容的歌曲：2 八度内一个、外一个、自定义音色一个、带微调一个，
    /// 正好让 `calculate_size()` 的三段提问全部出现且顺序固定。
    fn four_case_song(tempo: f32) -> Song {
        let mut s = Song {
            layers: vec![Layer::default(); 4],
            ..Song::new()
        };
        s.meta.tempo = tempo;
        s.set(0, 0, Note::new(0, 40));
        s.set(0, 1, Note::new(0, 30));
        // 用「第一个自定义索引」而不是写死 16：v6 起 16..19 是内置的铜块一族，
        // 拿它们当自定义音色就问不出「自定义音色」那一段提问了。
        s.set(1, 0, Note::new(nbs_core::model::FIRST_CUSTOM_INDEX, 45));
        let mut pitched = Note::new(0, 50);
        pitched.pitch = 50;
        s.set(2, 0, pitched);
        s
    }

    fn state_with(song: Song) -> EditorState {
        let mut s = EditorState::for_test();
        // 原版第一次走到这里会写 settings.ini；测试预先当作「已经提示过」，不碰用户配置。
        s.prefs.warning_schematic = true;
        // dispatch 按各自 state 的 prefs 同步全局语言；钉成中文让断言不跟本机/并行用例漂。
        s.prefs.language = 1;
        s.doc.replace_song(song);
        // 开机流程会开欢迎窗，命令测试的靶子是「欢迎窗已经关掉」的裸编辑器。
        s.window = None;
        s
    }

    fn titles(state: &EditorState) -> Vec<&'static str> {
        state.modals.items.iter().map(|m| m.title).collect()
    }

    fn texts(state: &EditorState) -> Vec<String> {
        state.modals.items.iter().map(|m| m.text.clone()).collect()
    }

    /// 打开导出窗口前，原版 `calculate_size()` 依次弹兼容性提问和速度提示，
    /// 全部答完才轮到窗口本身。
    #[test]
    fn player_mode_save_commands_are_noops() {
        let _lock = crate::frame_lock();
        let ctx = Context::default();
        let mut state = EditorState::for_test();
        state.window = None;
        state.player = true;
        state.doc.song.dirty = true;
        state.doc.song.filename = Some(std::path::PathBuf::from("C:/tmp/demo.nbs"));
        dispatch(&mut state, Command::Save, &ctx);
        assert!(state.doc.song.dirty, "播放器档 Save 不许真保存");
        assert_eq!(state.window, None, "播放器档 Save 不许弹窗");
        // 无文件名的变体走 SaveAs——同样被 save_song.gml:20 的门拦掉。
        state.doc.song.filename = None;
        dispatch(&mut state, Command::SaveAs, &ctx);
        assert_eq!(state.window, None);
        assert!(state.doc.song.dirty);
    }

    #[test]
    fn open_schematic_export_queues_the_original_dialogs_in_order() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let mut state = state_with(four_case_song(3.0));
        dispatch(&mut state, Command::OpenSchematicExport, &ctx);

        assert_eq!(state.window, None, "队列没走完不得打开导出窗口");
        assert_eq!(
            titles(&state),
            vec![
                "Minecraft 兼容性",
                "Minecraft 兼容性",
                "Minecraft 兼容性",
                "速度"
            ]
        );
        assert_eq!(
            texts(&state),
            vec![
                "一些方块在 Minecraft 的2八度范围外。你想无视这些方块导出 Schematic 吗？",
                "一些方块带有自定义音色。继续导出吗？",
                "一些方块带有音符盒不支持的微分音。继续导出吗？",
                "歌曲的速度与 Minecraft 不兼容。所以，在游戏内的播放会和软件内的有差别。",
            ]
        );
        assert_eq!(
            state
                .modals
                .items
                .iter()
                .map(|m| m.yes_no)
                .collect::<Vec<_>>(),
            vec![true, true, true, false],
            "只有提问会中断导出，速度提示是 message"
        );
        assert!(matches!(
            state.modals.then,
            Some(Command::ShowWindow(WindowKind::SchematicExport))
        ));

        for i in 0..3 {
            dispatch(&mut state, Command::AnswerModal(true), &ctx);
            assert_eq!(state.modals.items.len(), 3 - i);
            assert_eq!(state.window, None);
        }
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        assert!(state.modals.is_empty());
        assert_eq!(state.window, Some(WindowKind::SchematicExport));
        assert_eq!(
            state.schematic_options.tempo_mode, 2,
            "3.0 t/s 对应 2.5 红石刻档"
        );
    }

    /// 原版 add_block_manual 结尾：落速度变换写默认 BPM（tempo*15）并弹编辑窗；
    /// 落消音器写 start = until = 层号 + 1（终止层编码进声像 + 力度）并弹编辑窗。
    /// 事件乐器的 pit 远超 ±1200，必须绕过 `clamped()`。
    #[test]
    fn placing_event_instruments_writes_defaults_and_opens_edit_windows() {
        let ctx = Context::default();
        let tempo_ins = nbs_core::model::FIRST_CUSTOM_INDEX;
        let stopper_ins = nbs_core::model::FIRST_CUSTOM_INDEX + 1;
        let mut song = Song {
            layers: vec![Layer::default(); 3],
            ..Song::new()
        };
        song.meta.tempo = 10.0;
        song.custom_instruments = vec![
            nbs_core::model::CustomInstrument {
                name: nbs_core::TEMPO_CHANGER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: true,
            },
            nbs_core::model::CustomInstrument {
                name: nbs_core::SOUND_STOPPER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: true,
            },
        ];
        let mut state = state_with(song);

        state.current = Note::new(tempo_ins, 45);
        dispatch(&mut state, Command::PlaceNote { tick: 4, layer: 0 }, &ctx);
        assert_eq!(state.window, Some(WindowKind::EditTempoChanger(4, 0)));
        assert_eq!(state.event_edit.tempo, "150", "10 t/s * 15 = 150 BPM");
        let n = state.doc.song.get(4, 0).unwrap();
        assert_eq!(n.instrument, tempo_ins);
        assert_eq!(n.pitch, 150);

        state.current = Note::new(stopper_ins, 45);
        dispatch(&mut state, Command::PlaceNote { tick: 0, layer: 2 }, &ctx);
        assert_eq!(state.window, Some(WindowKind::EditSoundStopper(0, 2)));
        assert_eq!(state.event_edit.start, "3");
        assert_eq!(state.event_edit.until, "3");
        let n = state.doc.song.get(0, 2).unwrap();
        assert_eq!(n.pitch, 3);
        assert_eq!(n.panning, 103, "终止层 3 编进声像字节（(3+100)%256）");
        assert_eq!(n.velocity, 100);
    }

    /// 原版 change_block_manual 的 is_event 分支：落点已是事件乐器时不覆盖音符，
    /// 草稿回填现有值（消音器终止层从声像 + 力度解出来）。
    #[test]
    fn clicking_existing_event_note_reedits_without_overwriting() {
        let ctx = Context::default();
        let stopper_ins = nbs_core::model::FIRST_CUSTOM_INDEX;
        let tempo_ins = nbs_core::model::FIRST_CUSTOM_INDEX + 1;
        let mut song = Song {
            layers: vec![Layer::default(); 2],
            ..Song::new()
        };
        song.custom_instruments = vec![
            nbs_core::model::CustomInstrument {
                name: nbs_core::SOUND_STOPPER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: true,
            },
            nbs_core::model::CustomInstrument {
                name: nbs_core::TEMPO_CHANGER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: true,
            },
        ];
        let existing = Note {
            instrument: stopper_ins,
            key: 45,
            velocity: 100,
            panning: 157,
            pitch: 39,
        };
        song.set(7, 1, existing);
        let mut state = state_with(song);

        // 用消音器画笔点自己：再编辑，音符不动。
        state.current = Note::new(stopper_ins, 45);
        dispatch(&mut state, Command::PlaceNote { tick: 7, layer: 1 }, &ctx);
        assert_eq!(state.window, Some(WindowKind::EditSoundStopper(7, 1)));
        assert_eq!(state.event_edit.start, "39");
        assert_eq!(state.event_edit.until, "57", "pv_short(157, 100) = 57");
        assert_eq!(state.doc.song.get(7, 1), Some(existing));

        // 用速度变换画笔点它：一样不覆盖，草稿换成速度窗口的默认。
        state.window = None;
        state.current = Note::new(tempo_ins, 45);
        dispatch(&mut state, Command::PlaceNote { tick: 7, layer: 1 }, &ctx);
        assert_eq!(state.window, Some(WindowKind::EditTempoChanger(7, 1)));
        assert_eq!(state.event_edit.tempo, "39", "再编辑用现有 pit 当 BPM 草稿");
        assert_eq!(state.doc.song.get(7, 1), Some(existing));
    }

    /// 编辑窗「确定」的两条命令：速度变换只改 pit；消音器改 pit + 声像/力度，
    /// key 与乐器保持原样（原版 h_changeblock 只写这三个字段）。
    #[test]
    fn event_edit_ok_commands_write_only_event_fields() {
        let ctx = Context::default();
        let mut song = Song {
            layers: vec![Layer::default(); 2],
            ..Song::new()
        };
        let tempo_ins = nbs_core::model::FIRST_CUSTOM_INDEX;
        let stopper_ins = nbs_core::model::FIRST_CUSTOM_INDEX + 1;
        song.custom_instruments = vec![
            nbs_core::model::CustomInstrument {
                name: nbs_core::TEMPO_CHANGER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: true,
            },
            nbs_core::model::CustomInstrument {
                name: nbs_core::SOUND_STOPPER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: true,
            },
        ];
        song.set(
            2,
            0,
            Note {
                instrument: tempo_ins,
                key: 50,
                velocity: 80,
                panning: 60,
                pitch: 150,
            },
        );
        song.set(
            3,
            0,
            Note {
                instrument: stopper_ins,
                key: 40,
                velocity: 100,
                panning: 157,
                pitch: 39,
            },
        );
        let mut state = state_with(song);

        dispatch(
            &mut state,
            Command::SetEventTempo {
                tick: 2,
                layer: 0,
                bpm: 400,
            },
            &ctx,
        );
        let n = state.doc.song.get(2, 0).unwrap();
        assert_eq!(n.pitch, 400);
        assert_eq!((n.key, n.velocity, n.panning), (50, 80, 60));

        dispatch(
            &mut state,
            Command::SetEventStopper {
                tick: 3,
                layer: 0,
                start: 10,
                until: 57,
            },
            &ctx,
        );
        let n = state.doc.song.get(3, 0).unwrap();
        assert_eq!(n.pitch, 10);
        assert_eq!(n.panning, 157, "until 57 解码回 (157, 100)");
        assert_eq!(n.velocity, 100);
        assert_eq!(n.key, 40);
    }

    /// 选乐器顺带复位力度/声像/微调并发一声——原版每个入口都这么做
    /// （`control_draw.gml:826-830` 数字键、`1841-1848` 音色格子）。
    #[test]
    fn selecting_an_instrument_resets_and_previews() {
        let ctx = Context::default();
        let mut state = state_with(Song::demo());
        state.current.velocity = 33;
        state.current.panning = 7;
        state.current.pitch = -250;
        for ins in [0u8, 15, 16, 19] {
            dispatch(&mut state, Command::SetInstrument(ins), &ctx);
            assert_eq!(state.current.instrument, ins, "乐器 {ins}");
            assert_eq!(state.current.velocity, 100, "乐器 {ins} 的力度");
            assert_eq!(state.current.panning, 100, "乐器 {ins} 的声像");
            assert_eq!(state.current.pitch, 0, "乐器 {ins} 的微调");
        }
    }

    /// 提问只要答一个「否」，后面的提示连同窗口一起作废。
    #[test]
    fn answering_no_aborts_the_schematic_export() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(3.0));
        dispatch(&mut state, Command::OpenSchematicExport, &ctx);
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        dispatch(&mut state, Command::AnswerModal(false), &ctx);
        assert!(state.modals.is_empty(), "答否后队列必须清空");
        assert!(state.modals.then.is_none());
        assert_eq!(state.window, None, "答否不打开导出窗口");
    }

    /// 没有方块可导出时，原版只弹一个错误并到此为止。
    #[test]
    fn empty_song_shows_the_no_blocks_error_instead_of_the_window() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        dispatch(&mut state, Command::OpenSchematicExport, &ctx);
        assert_eq!(titles(&state), vec!["错误"]);
        assert_eq!(texts(&state), vec!["没有方块可以导出！"]);
        assert!(!state.modals.items[0].yes_no);
        assert!(state.modals.then.is_none(), "闸门没过就没有后续动作");
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        assert_eq!(state.window, None);
    }

    /// 「使用默认值」先问「你确定吗？」，答是才恢复，且不动原版默认值表里没有的三项。
    #[test]
    fn reset_defaults_asks_first_and_keeps_the_unlisted_options() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        dispatch(&mut state, Command::OpenSchematicExport, &ctx);
        while !state.modals.is_empty() {
            dispatch(&mut state, Command::AnswerModal(true), &ctx);
        }
        assert_eq!(state.window, Some(WindowKind::SchematicExport));

        state.schematic_options.layout = 0;
        state.schematic_options.notesperrow = 100;
        state.schematic_options.tempo_mode = 1;
        state.schematic_options.structure = true;
        state.schematic_options.command_block = true;

        state.modals = ModalQueue::question("确定", "你确定吗？", Command::ResetSchematicOptions);
        dispatch(&mut state, Command::AnswerModal(false), &ctx);
        assert_eq!(state.schematic_options.layout, 0, "答否不还原");
        assert_eq!(state.schematic_options.notesperrow, 100);

        state.modals = ModalQueue::question("确定", "你确定吗？", Command::ResetSchematicOptions);
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        let d = nbs_io::SchematicOptions::default();
        assert_eq!(state.schematic_options.layout, d.layout);
        assert_eq!(state.schematic_options.notesperrow, d.notesperrow);
        assert_eq!(
            state.schematic_options.tempo_mode, 1,
            "sch_exp_tempo 不在默认值表里"
        );
        assert!(
            state.schematic_options.structure,
            "structure 是全局变量，不被重置"
        );
        assert!(
            state.schematic_options.command_block,
            "command_block 同样不在表里"
        );
    }

    /// 「导出」按钮的两道闸门各有一个不同标题的 message，通过后才轮到保存对话框。
    #[test]
    fn export_button_gates_run_before_the_save_dialog() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        // 把 2 八度内的音符全锁在静音层里：totalblocks[不静音] = 0，但 open 闸门仍过。
        for l in state.doc.song.layers.iter_mut() {
            l.lock = LockMode::Locked;
        }
        dispatch(&mut state, Command::OpenSchematicExport, &ctx);
        while !state.modals.is_empty() {
            dispatch(&mut state, Command::AnswerModal(true), &ctx);
        }
        state.schematic_options.include_locked = false;
        dispatch(&mut state, Command::ExportSchematicChecked, &ctx);
        assert_eq!(
            titles(&state),
            vec!["导出结构"],
            "原版这条闸门的标题是「导出结构」（EN \"Schematic export\"），不是「错误」"
        );
        assert_eq!(texts(&state), vec!["没有方块可以导出！"]);
        assert_eq!(
            state.window,
            Some(WindowKind::SchematicExport),
            "闸门没过不开保存框"
        );

        // 原版这里是弹窗置顶：先把这条消息答掉，才能继续操作。
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        assert!(state.modals.is_empty());

        // 勾上「包含静音图层」后同一条曲子就能导出。
        state.schematic_options.include_locked = true;
        dispatch(&mut state, Command::ExportSchematicChecked, &ctx);
        assert!(state.modals.is_empty());
        assert_eq!(state.window, Some(WindowKind::FileSave));
    }

    /// 模态框置顶时背景界面完全失效，等价于原版 `message()` 冻结游戏循环。
    #[test]
    fn background_commands_are_dropped_while_a_modal_is_up() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.modals = ModalQueue::question("确定", "你确定吗？", Command::ResetSchematicOptions);
        state.schematic_options.layout = 0;
        state.pending.push(Command::CloseWindow);
        state.pending.push(Command::ToggleFlag(Flag::Blackout));
        state.flush(&ctx);
        assert_eq!(state.window, None, "CloseWindow 被丢弃，不该改变任何状态");
        assert!(!state.flags.blackout, "工具栏命令同样被丢弃");

        state.pending.push(Command::AnswerModal(true));
        state.flush(&ctx);
        assert_eq!(
            state.schematic_options.layout, 1,
            "答「是」后队列尾部的动作照常执行"
        );
    }

    /// 保存框的文件名跟原版一致：经典格式保留原名，结构方块转小写并加下划线。
    #[test]
    fn schematic_dialog_extension_follows_the_format_choice() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.doc.song.filename = Some(Path::new("Megalovania Boss.nbs").to_path_buf());

        state.schematic_options.structure = false;
        dispatch(
            &mut state,
            Command::OpenDialog(DialogAction::ExportSchematic),
            &ctx,
        );
        let d = state.file_dialog.clone().unwrap();
        assert_eq!(d.ext, "schematic");
        assert_eq!(d.name, "Megalovania Boss.schematic");

        state.schematic_options.structure = true;
        dispatch(
            &mut state,
            Command::OpenDialog(DialogAction::ExportSchematic),
            &ctx,
        );
        let d = state.file_dialog.clone().unwrap();
        assert_eq!(d.ext, "nbt");
        assert_eq!(d.name, "megalovania_boss.nbt");
    }

    /// 原版 `select_lastpressed`：关掉之后，按下的键照样出声，但选中的音不跟着跑。
    #[test]
    fn key_press_only_moves_selection_when_allowed() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.current.key = 30;
        state.current.velocity = 60;
        dispatch(&mut state, Command::KeyPress(50), &ctx);
        assert_eq!(state.current.key, 50, "默认（开）时选中音跟到按下的键");
        assert_eq!(state.current.velocity, 100, "选中时力度/声像/微调复位");

        state.prefs.select_lastpressed = false;
        state.current.key = 30;
        state.current.velocity = 60;
        dispatch(&mut state, Command::KeyPress(50), &ctx);
        assert_eq!(state.current.key, 30, "关掉后选中音不动");
        assert_eq!(state.current.velocity, 60);
    }

    /// 原版 `mousewheel = 1/2`：滚轮在音色表 / 琴键上挪一格并复位属性，走到头就不动。
    #[test]
    fn wheel_cycles_instrument_and_key() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.current.instrument = 0;
        state.current.velocity = 55;
        dispatch(&mut state, Command::WheelInstrument(1), &ctx);
        assert_eq!(
            state.current.instrument, 1,
            "原版 mouse_wheel_up 往后走一格"
        );
        assert_eq!(state.current.velocity, 100);
        dispatch(&mut state, Command::WheelInstrument(-1), &ctx);
        assert_eq!(state.current.instrument, 0);
        dispatch(&mut state, Command::WheelInstrument(-1), &ctx);
        assert_eq!(
            state.current.instrument, 0,
            "第 0 格再往下没有位置，保持不动"
        );

        state.current.key = 0;
        dispatch(&mut state, Command::WheelKey(-1), &ctx);
        assert_eq!(state.current.key, 0, "0 键再往下没有位置，保持不动");
        dispatch(&mut state, Command::WheelKey(1), &ctx);
        assert_eq!(state.current.key, 1);
        state.current.key = nbs_core::model::KEY_MAX;
        dispatch(&mut state, Command::WheelKey(1), &ctx);
        assert_eq!(state.current.key, nbs_core::model::KEY_MAX, "最高键不动");
    }

    /// 原版首选项「歌曲」组三个按钮的真实行为（`draw_window_preferences.gml:269-297`）。
    /// 「打开」的成功分支会真的开一个资源管理器窗口，测试里只验它「目录不存在要报错」这一支。
    #[test]
    fn prefs_folder_buttons_are_wired() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().into_owned();
        let mut state = state_with(Song::new());

        // 起始目录：原版歌曲对话框只看 songfolder，当前文件在哪它不看。
        state.prefs.song_folder = Some(tmp_str.clone());
        dispatch(
            &mut state,
            Command::OpenDialog(DialogAction::OpenSong),
            &ctx,
        );
        assert_eq!(state.file_dialog.clone().unwrap().dir, tmp);
        state.file_dialog = None;
        dispatch(
            &mut state,
            Command::OpenDialog(DialogAction::ExportPattern),
            &ctx,
        );
        assert_eq!(
            state.file_dialog.clone().unwrap().dir,
            nbs_io::prefs::Preferences::default_pattern_folder(),
            "片段对话框只看 patternfolder"
        );
        state.file_dialog = None;

        // 「还原默认」清掉自定义值，界面与对话框一起退回出厂目录。
        dispatch(
            &mut state,
            Command::ResetPrefsFolder(PrefsFolder::Song),
            &ctx,
        );
        assert_eq!(state.prefs.song_folder, None);
        assert_eq!(
            state.prefs.song_folder_path(),
            nbs_io::prefs::Preferences::default_song_folder()
        );

        // 「修改」：原版先 message 说明，答完才弹目录选择框。
        dispatch(
            &mut state,
            Command::ChangePrefsFolder(PrefsFolder::Pattern),
            &ctx,
        );
        assert_eq!(titles(&state), vec![""]);
        assert_eq!(texts(&state), vec!["选择导出和导入片段的文件夹"]);
        assert!(matches!(
            state.modals.then,
            Some(Command::OpenDialog(DialogAction::PickFolder(
                PrefsFolder::Pattern
            )))
        ));
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        let d = state.file_dialog.clone().unwrap();
        assert_eq!(d.action, DialogAction::PickFolder(PrefsFolder::Pattern));
        assert_eq!(d.ext, "", "原版第 1 个参数是空过滤器串");
        assert_eq!(
            d.name, "选择片段文件夹",
            "原版把占位串塞进文件名栏（`draw_window_preferences.gml:294`）"
        );
        assert!(
            d.save,
            "原版这条走的是 get_save_filename_ext，不是目录选择器"
        );

        // 确定：认的是对话框当时显示的目录，随后把首选项窗口放回去。
        dispatch(&mut state, Command::DialogConfirm(tmp.clone()), &ctx);
        assert_eq!(state.prefs.pattern_folder, Some(tmp_str));
        assert_eq!(state.window, Some(WindowKind::Preferences));

        // 「打开」：连出厂目录都不存在时，原版弹「该路径不存在！」。
        state.prefs.pattern_folder = Some("Z:\\nbs-rust 没有这个目录".into());
        dispatch(
            &mut state,
            Command::OpenPrefsFolder(PrefsFolder::Pattern),
            &ctx,
        );
        assert_eq!(titles(&state), vec!["错误"]);
        assert_eq!(texts(&state), vec!["该路径不存在！"]);
    }

    /// 原版 `open_song_nbs.gml:12-13`：文件版本比本程序旧才警告，
    /// 而且完全受首选项「打开旧版格式时提示」管。
    #[test]
    fn opening_an_older_nbs_warns_only_when_told_to() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let dir = std::env::temp_dir();
        let old = dir.join("nbs_rust_old_version.nbs");
        let current = dir.join("nbs_rust_current_version.nbs");
        let mut song = Song::new();
        song.set(0, 0, Note::new(0, 40));
        nbs_core::nbs::save(&old, &song, 3).unwrap();
        nbs_core::nbs::save(&current, &song, nbs_core::nbs::NBS_VERSION).unwrap();

        let mut state = state_with(Song::new());
        for show in [false, true] {
            state.prefs.show_old_warning = show;
            dispatch(&mut state, Command::OpenPath(old.clone()), &ctx);
            if show {
                assert_eq!(titles(&state), vec!["警告"]);
                assert_eq!(texts(&state), vec![nbs_io::prefs::old_file_warning("NBS")]);
            } else {
                assert!(state.modals.is_empty(), "开关关着不该弹旧版警告");
            }
            state.modals = ModalQueue::default();
            dispatch(&mut state, Command::OpenPath(current.clone()), &ctx);
            assert!(state.modals.is_empty(), "同版本的文件不该弹任何警告");
        }
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&current);
    }

    /// 原版 `obj_song/Create_0.gml:22-24`：区间属于「这一首歌」，每首开出来都是
    /// 无区间、两端归零。端口把它放在 `EditorState` 上，所以四条整首换歌的路线
    /// 都得清掉，否则上一首的区间会留在标尺上，还会继续当 `marker_end` 的停点。
    /// （MIDI 导入和图样导入两条走的是同样的三件套，这里不重复铺样本。）
    #[test]
    fn song_replacement_clears_the_section() {
        let _prefs = crate::isolate_prefs();
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        let arm = |state: &mut EditorState| {
            state.section_exists = true;
            state.section_start = 3;
            state.section_end = 8;
        };

        arm(&mut state);
        dispatch(&mut state, Command::ConfirmNew, &ctx);
        assert!(!state.section_exists, "新建歌曲要清掉区间");
        assert_eq!((state.section_start, state.section_end), (0, 0));

        let file = std::env::temp_dir().join("nbs_rust_section_reset.nbs");
        let mut song = Song::new();
        song.set(4, 0, Note::new(0, 48));
        nbs_core::nbs::save(&file, &song, nbs_core::nbs::NBS_VERSION).unwrap();
        arm(&mut state);
        dispatch(&mut state, Command::OpenPath(file.clone()), &ctx);
        assert!(!state.section_exists, "打开歌曲要清掉区间");
        assert_eq!((state.section_start, state.section_end), (0, 0));
        assert_eq!(state.marker(), 0.0, "原版换歌连播放头一起归零");
        let _ = std::fs::remove_file(&file);
    }

    /// `predicted_dt = 0` 是关键：egui 会把「 predicted 帧长」从延时里减掉防止超调
    /// （context.rs:187），留默认 1/60 秒的话 240 帧那一档会被减成 0，测不出档位。
    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 0.0,
            ..Default::default()
        }
    }

    /// 缩放语义按用户要求改过（#80）：原版 `window_scale` 连 chrome 一起缩
    /// （`control_draw.gml:11-12` 的 rw/rh 除以缩放），端口只缩画布 —— 倍率落到
    /// `viewport.scale`（每格 `32 × scale` px），egui 的 `pixels_per_point` 不再被动。
    #[test]
    fn window_scale_pref_drives_viewport_scale_not_zoom() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        state.prefs.window_scale = Some(2.0);
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        let out = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert_eq!(
            out.pixels_per_point, 1.0,
            "chrome 保持系统原生尺寸，不动 zoom"
        );
        assert_eq!(state.viewport.scale, 2.0, "倍率落到画布");
        assert_eq!(state.viewport.cell(), 64.0, "每格 32 × scale px");
        assert!(state.canvas_hires, "2.0 > 1.25 触发原版的 hires 贴图档");
        // 「重置」存回 None，必须真的退回系统倍率，不然按下去没反应。
        state.prefs.window_scale = None;
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        let out = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert_eq!(out.pixels_per_point, 1.0);
        assert_eq!(state.viewport.scale, 1.0, "重置后画布退回出厂缩放");
        assert!(!state.canvas_hires, "1.0 ≤ 1.25 退回普通贴图");
    }

    /// 端口自加（用户要求 #92）：缩放降到阈值以下时方块上的字整批不画。阈值默认 50%，
    /// 正好是原版能拖到的最低档 —— 在原版可达的缩放里这个开关永远不触发，行为和原版一致；
    /// 只有我们把下限放开到 10% 之后它才有意义。判据用首选项显示的那个百分数。
    #[test]
    fn low_window_scale_hides_the_note_block_text() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        state.prefs.note_text_min_percent = 50;
        state.prefs.window_scale = Some(0.5);
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert!(!state.hide_note_text, "50% 等于出厂阈值，字照画");
        state.prefs.window_scale = Some(0.4);
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert!(state.hide_note_text, "40% 低于阈值，格内文字收掉");
        state.prefs.note_text_min_percent = 0;
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert!(!state.hide_note_text, "阈值填 0 = 永不收字");
        state.prefs.window_scale = Some(2.0);
        state.prefs.note_text_min_percent = 100;
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert!(!state.hide_note_text, "放大到 200% 更不会触发");
        state.prefs.window_scale = Some(1.0);
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert!(!state.hide_note_text, "100% 正是阈值本身，照画");
    }

    /// 原版 `control_draw.gml:736-751` 的那串 `else if`：Ctrl+滚轮 / Ctrl+± 只能停在
    /// 离散档位上，两端封顶 0.5 和 4，中间按当前值就近跳下一档；Ctrl+0 回出厂值。
    #[test]
    fn window_scale_ladder_is_the_original_discrete_steps() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        let step = |up: bool, state: &mut EditorState| {
            dispatch(
                state,
                if up {
                    Command::WindowScaleUp
                } else {
                    Command::WindowScaleDown
                },
                &ctx,
            )
        };
        // 出厂是 None（= 系统倍率，测试环境里 1.0），往上第一档 1.25 而不是连续值。
        step(true, &mut state);
        assert_eq!(
            state.prefs.window_scale,
            Some(1.25),
            "1.0 的上一档应是 1.25"
        );
        // 再往下要一路退到 0.5，中间一档都不能漏。
        for want in [1.0, 0.9, 0.8, 0.75, 0.67, 0.5] {
            step(false, &mut state);
            assert_eq!(state.prefs.window_scale, Some(want), "往下漏了一档");
        }
        // 偏离原版（用户要求下界 10%）：0.5 之下自加 0.4/0.3/0.2/0.1 四档。
        for want in [0.4, 0.3, 0.2, 0.1] {
            step(false, &mut state);
            assert_eq!(state.prefs.window_scale, Some(want), "往下漏了一档");
        }
        step(false, &mut state);
        assert_eq!(
            state.prefs.window_scale,
            Some(0.1),
            "0.1 是当前下界（原版 0.5，用户要求放开），不能再小"
        );
        for want in [
            0.2, 0.3, 0.4, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0,
        ] {
            step(true, &mut state);
            assert_eq!(state.prefs.window_scale, Some(want), "往上漏了一档");
        }
        for _ in 0..5 {
            step(true, &mut state);
        }
        assert_eq!(state.prefs.window_scale, Some(4.0), "4 是原版上界");
        dispatch(&mut state, Command::WindowScaleReset, &ctx);
        assert_eq!(state.prefs.window_scale, None, "Ctrl+0 要存回出厂值");
    }

    /// 端口自加（用户要求）：琴键缩放和窗口缩放共用同一张档位表，但落到
    /// `prefs.piano_scale` 上、不碰 `window_scale`。
    #[test]
    fn piano_scale_ladder_is_the_same_discrete_steps() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        let step = |up: bool, state: &mut EditorState| {
            dispatch(
                state,
                if up {
                    Command::PianoScaleUp
                } else {
                    Command::PianoScaleDown
                },
                &ctx,
            )
        };
        // 出厂 1.0，往上一档 1.25；窗口缩放停在出厂值（测试态 Some(1.0)）不动。
        step(true, &mut state);
        assert_eq!(state.prefs.piano_scale, 1.25, "琴键 1.0 的上一档应是 1.25");
        assert_eq!(
            state.prefs.window_scale,
            Some(1.0),
            "琴键缩放不许影响窗口缩放"
        );
        for want in [1.0, 0.9, 0.8, 0.75, 0.67, 0.5, 0.4, 0.3, 0.2] {
            step(false, &mut state);
            assert_eq!(state.prefs.piano_scale, want, "往下漏了一档");
        }
        step(false, &mut state);
        assert_eq!(
            state.prefs.piano_scale, 0.1,
            "琴键与窗口共用下界 0.1，不能再小"
        );
        for want in [
            0.2, 0.3, 0.4, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0,
        ] {
            step(true, &mut state);
            assert_eq!(state.prefs.piano_scale, want, "往上漏了一档");
        }
        step(true, &mut state);
        assert_eq!(state.prefs.piano_scale, 4.0, "琴键上界与窗口一致为 4");
    }

    /// 原版 `game_set_speed(N, gamespeed_fps)`：帧数上限压的是整个程序的步进，
    /// 我们这边对应压住连续重绘的间隔。
    #[test]
    fn refresh_rate_caps_the_repaint_delay() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        state.scrub = 1; // 按住快进，和播放一样是连续重绘的来源
                         // 头两帧 egui 自己会补一次立即重绘（字体、outstanding 计数），先跑掉。
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        for (rate, fps) in [
            (0u8, 30.0f64),
            (1, 60.0),
            (2, 120.0),
            (3, 144.0),
            (4, 240.0),
        ] {
            state.prefs.refresh_rate = rate;
            let out = ctx.run(raw_input(), |c| state.tick_transport(c));
            let got = out
                .viewport_output
                .iter()
                .find(|(id, _)| **id == egui::ViewportId::ROOT)
                .map(|(_, v)| v.repaint_delay.as_secs_f64())
                .unwrap_or(0.0);
            let want = 1.0 / fps;
            assert!(
                (got - want).abs() < 0.002,
                "refreshrate {rate} 应把重绘压到 {fps} 帧，实际延后 {got} 秒"
            );
        }
    }

    /// 跑一帧，取这一帧交给平台层的窗口图标命令。
    fn icon_command(ctx: &Context, state: &mut EditorState) -> Option<egui::IconData> {
        let out = ctx.run(raw_input(), |c| state.tick_transport(c));
        root_commands(&out).iter().find_map(|cmd| match cmd {
            egui::ViewportCommand::Icon(Some(icon)) => Some((**icon).clone()),
            _ => None,
        })
    }

    /// 0.31 的窗口命令挂在 `viewport_output` 每个视口的 `commands` 里，不在 FullOutput 顶层。
    fn root_commands(out: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
        out.viewport_output
            .iter()
            .find(|(id, _)| **id == egui::ViewportId::ROOT)
            .map(|(_, v)| v.commands.clone())
            .unwrap_or_default()
    }

    /// 原版 `control_draw.gml:27-31`：`window_icon` 决定窗口图标是 `Data\icon.ico`
    /// 还是程序自带的那份；`control_step.gml:21-24` 只在它变化时才重新发一次。
    #[test]
    fn window_icon_pref_swaps_the_window_icon() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        state.prefs.window_icon = false;
        state.applied_window_icon = None;
        let new = icon_command(&ctx, &mut state).expect("出厂就该把自带的图标发给窗口");
        assert_eq!(
            (new.width, new.height),
            (256, 256),
            "原版那两个 .ico 只有 256 一档"
        );
        assert_eq!(new.rgba.len(), 256 * 256 * 4);
        // 同一档位不该每帧重发：原版只在值变化时打开重发窗口。
        let out = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert!(
            !root_commands(&out)
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Icon(_))),
            "没变化就不该再发图标命令"
        );
        state.prefs.window_icon = true;
        let old = icon_command(&ctx, &mut state).expect("勾上旧图标后要换成 Data\\icon.ico");
        assert_eq!((old.width, old.height), (256, 256));
        assert_ne!(old.rgba, new.rgba, "旧图标和新图标必须是两张不同的图");
    }

    /// 原版徽章命中那一段（`control_draw.gml:2032-2036`）：停播放、关录制、
    /// 浮动选区落回，再 `window = w_minecraft`。
    #[test]
    fn compat_badge_stops_everything_and_opens_the_window() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(3.0));
        state.record = true;
        state.record_armed = true;
        state.doc.select_all();
        assert!(!state.doc.selection.is_empty());

        dispatch(&mut state, Command::ShowCompat, &ctx);
        assert_eq!(state.window, Some(WindowKind::MinecraftCompat));
        assert!(!state.is_playing(), "原版 playing = 0");
        assert!(!state.record && !state.record_armed, "原版 record = 0");
        assert!(
            state.doc.selection.is_empty(),
            "原版 selection_place(0)：选区要落回歌曲"
        );
        assert_eq!(state.doc.song.total_blocks, 4);
        assert_eq!(state.mc_tab, 0, "原版开窗就从 Schematic 页开始");
    }

    /// 速度测量器的两条入口在原版里并不一样：Ctrl+Shift+I 前面有 `playing = 0`
    /// （`control_draw.gml:771-774`），标尺右键菜单只写 `window`
    /// （`menu_click.gml:390`）。关窗则清三条打点变量
    /// （`draw_window_tempo_tapper.gml:56-58` / `draw_windows.gml:94-99`）。
    #[test]
    fn tempo_tapper_hotkey_stops_playback_but_the_menu_entry_does_not() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.tapper.register_tap(0);
        state.tapper.register_tap(500);
        assert_eq!(state.tapper.measured(), 120.0);
        state.audio.clock_mut().seek(2.0);
        state.begin_playback();

        dispatch(
            &mut state,
            Command::ShowWindow(WindowKind::TempoTapper),
            &ctx,
        );
        assert_eq!(state.window, Some(WindowKind::TempoTapper));
        assert!(
            state.is_playing(),
            "菜单入口不该停播放（menu_click.gml:390 只赋值 window）"
        );

        dispatch(&mut state, Command::CloseWindow, &ctx);
        assert_eq!(state.window, None);
        assert_eq!(state.tapper.bpm, 0.0, "关窗清 taptempo");
        assert!(
            !state.tapper.tapping && state.tapper.last_ms == 0,
            "关窗清 tapping / ltime"
        );

        dispatch(
            &mut state,
            Command::HotkeyWindow(WindowKind::TempoTapper),
            &ctx,
        );
        assert_eq!(state.window, Some(WindowKind::TempoTapper));
        assert!(!state.is_playing(), "原版 playing = 0");
    }

    /// 原版 `control_draw.gml:765-831` 的 Ctrl+Shift+ 字母，每一条第一句都是
    /// `playing = 0`；同一批窗口从菜单/工具栏进来（`ShowWindow`）则不停。
    /// 端口此前只在速度测量器上补了这句，其余六个按键漏了。
    #[test]
    fn every_ctrl_shift_window_hotkey_stops_playback() {
        let ctx = Context::default();
        for kind in [
            WindowKind::ClipEditor,
            WindowKind::TempoTapper,
            WindowKind::Tremolo,
            WindowKind::Stereo,
            WindowKind::SetVelocity,
            WindowKind::SetPanning,
            WindowKind::SetPitch,
        ] {
            let mut state = state_with(four_case_song(10.0));
            state.begin_playback();
            assert!(state.is_playing());
            dispatch(&mut state, Command::HotkeyWindow(kind.clone()), &ctx);
            assert_eq!(state.window, Some(kind.clone()), "{kind:?} 要真的开起来");
            assert!(
                !state.is_playing(),
                "{kind:?} 走快捷键必须停播放（playing = 0）"
            );

            let mut state = state_with(four_case_song(10.0));
            state.begin_playback();
            dispatch(&mut state, Command::ShowWindow(kind), &ctx);
            assert!(state.is_playing(), "同一窗口的菜单入口原版不停播放");
        }
    }

    /// 快捷键族里「直接执行」的那 7 个宏也一样：原版每条都是先 `playing = 0`
    /// 再调宏函数（`control_draw.gml:792-834`）。端口此前一个都没接。
    #[test]
    fn hotkey_macro_stops_playback_and_runs() {
        let ctx = Context::default();
        // 选中的音符已经从 song.columns 抬进浮动选区（select_rect 的 copy=false），
        // 宏改的就是那一份，所以取速度要问 selection。
        let velocities = |state: &EditorState| -> Vec<u8> {
            state
                .doc
                .selection
                .notes
                .iter()
                .map(|n| n.note.velocity)
                .collect()
        };
        let mut state = state_with(four_case_song(10.0));
        state.doc.select_all();
        let before = velocities(&state);
        state.begin_playback();

        dispatch(&mut state, Command::HotkeyMacro(Macro::FadeIn), &ctx);
        assert!(!state.is_playing(), "原版 playing = 0");
        assert!(state.message.is_some(), "要真的走到 RunMacro");
        assert_eq!(before.len(), 4, "测试前提：全选要抬出 4 个音符");
        assert!(
            velocities(&state).iter().min() == Some(&1),
            "淡入要把首列压到最轻，实得 {:?}",
            velocities(&state)
        );
    }

    /// 原版宏窗口确定的收尾：`if(!keyboard_check(vk_alt)) selection_place(false)`。
    /// 没按 Alt 把改完的浮动选区落回歌曲；按着 Alt 只改选区，歌上一个字都不动。
    #[test]
    fn macro_window_ok_places_the_selection_unless_alt() {
        let ctx = Context::default();
        let key_at = |state: &EditorState, col: u32, layer: u32| {
            state.doc.song.get(col, layer).map(|n| n.key)
        };
        for place in [true, false] {
            let mut state = state_with(four_case_song(10.0));
            // four_case_song：列 0 两层（40 / 30）、列 1 一层 45、列 2 一层 50。
            // 全选把音符抬进浮动选区，歌上那三格此时是空的。
            state.doc.select_all();
            dispatch(
                &mut state,
                Command::MacroWindowOk {
                    m: Macro::Arpeggio {
                        pattern: vec![0, 12],
                    },
                    place,
                },
                &ctx,
            );
            assert_eq!(state.window, None, "确定后窗口要关掉");
            if place {
                assert_eq!(key_at(&state, 0, 0), Some(40), "第 1 列吃规律第 1 个值 +0");
                assert_eq!(key_at(&state, 1, 0), Some(57), "第 2 列吃 +12");
                assert_eq!(key_at(&state, 2, 0), Some(50), "第 3 列循环回 +0");
                assert!(state.doc.selection.is_empty(), "落回后没有残余选区");
            } else {
                assert_eq!(key_at(&state, 1, 0), None, "按 Alt 不落回，歌上还是空的");
                assert_eq!(
                    state.doc.selection.at(1, 0).map(|n| n.key),
                    Some(57),
                    "改的是浮动选区那一份"
                );
            }
        }
    }

    /// 原版 `toggle_playing` 起播第一行：播完停在歌尾（歌长 + 16 刻）再按
    /// 播放要回到 0 重新起播；中途位置起播原地不动。
    #[test]
    fn play_after_reaching_the_end_restarts_from_zero() {
        let mut state = state_with(four_case_song(10.0));
        let end_stop =
            state.doc.song.song_length() as f64 + nbs_audio::scheduler::END_STOP_MARGIN_TICKS;
        state.audio.clock_mut().seek(end_stop);
        state.begin_playback();
        assert!(state.is_playing());
        assert_eq!(
            state.audio.clock().position(),
            0.0,
            "停在结尾再按播放要回到 0（toggle_playing: marker_pos = enda + cols → 0）"
        );
        state.audio.clock_mut().pause();

        state.audio.clock_mut().seek(5.0);
        state.begin_playback();
        assert!(state.is_playing());
        assert_eq!(state.audio.clock().position(), 5.0, "中途起播不回 0");
    }

    /// 两条速度阶梯都是原版那一串连续 `if`：后面的覆盖前面的，条件一律看改动前的速度。
    #[test]
    fn tempo_fix_buttons_walk_the_original_ladders() {
        let ctx = Context::default();
        let schematic = [
            (23.45_f32, 10.0_f32),
            (12.0, 10.0),
            (8.0, 10.0),
            (6.0, 5.0),
            (4.0, 5.0),
            (3.0, 2.5),
            (1.0, 2.5),
        ];
        for (from, to) in schematic {
            let mut state = state_with(Song::demo());
            state.doc.edit_meta("改速度", |m| m.tempo = from);
            dispatch(&mut state, Command::FixTempoSchematic, &ctx);
            assert_eq!(state.doc.song.meta.tempo, to, "Schematic 页 {from} t/s");
        }
        let datapack = [
            (23.45_f32, 20.0_f32),
            (14.0, 10.0),
            (6.0, 5.0),
            (4.2, 4.0),
            (3.0, 2.5),
            (2.2, 2.0),
            (1.5, 1.25),
            (1.1, 1.0),
            (0.7, 0.5),
            (0.1, 0.25),
        ];
        for (from, to) in datapack {
            let mut state = state_with(Song::demo());
            state.doc.edit_meta("改速度", |m| m.tempo = from);
            dispatch(&mut state, Command::FixTempoDatapack, &ctx);
            assert_eq!(state.doc.song.meta.tempo, to, "数据包页 {from} t/s");
        }
    }

    /// 原版 `datapack_getextranotes()`：把 `Data\extranotes.zip` 原样复制过去，
    /// 弹「资源包保存成功！」，调用它的那个窗口不受影响。
    #[test]
    fn save_extra_notes_pack_copies_the_shipped_zip_and_keeps_the_window() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(3.0));
        dispatch(&mut state, Command::ShowCompat, &ctx);
        dispatch(&mut state, Command::SaveExtraNotesPack, &ctx);

        // 没装原生组件时走自绘兜底框：兼容性窗口被顶掉，但记在了 dialog_return 里。
        assert_eq!(state.window, Some(WindowKind::FileSave));
        assert_eq!(state.dialog_return, Some(WindowKind::MinecraftCompat));
        assert_eq!(state.file_dialog.as_ref().unwrap().name, "更多音符盒");

        let dir = std::env::temp_dir().join("nbs-rust-compat-test");
        let _ = std::fs::create_dir_all(&dir);
        let dst = dir.join("更多音符盒.zip");
        let _ = std::fs::remove_file(&dst);
        dispatch(&mut state, Command::DialogConfirm(dst.clone()), &ctx);

        let bytes = std::fs::read(&dst).expect("资源包应已写到所选路径");
        assert_eq!(&bytes[..2], b"PK", "复制的必须是那个 zip，不是别的文件");
        assert_eq!(bytes.len(), nbs_io::datapack::EXTRA_NOTES_ZIP.len());
        assert_eq!(
            state.window,
            Some(WindowKind::MinecraftCompat),
            "选完放回原窗口"
        );
        assert_eq!(titles(&state), vec!["保存"]);
        assert_eq!(texts(&state), vec!["资源包保存成功！"]);
        let _ = std::fs::remove_file(&dst);
    }

    /// 只带一个视觉事件乐器的歌，音符落在指定列。
    fn visual_instrument_song(name: &str, tick: u32) -> Song {
        let mut song = Song::new();
        song.custom_instruments = vec![nbs_core::model::CustomInstrument {
            name: name.into(),
            filename: String::new(),
            key: 45,
            press: false,
        }];
        song.set(tick, 0, Note::new(nbs_core::model::FIRST_CUSTOM_INDEX, 45));
        song.recalculate();
        song
    }

    /// 按播放键 + 排空一帧：视觉事件的派发在 start() 里，落地在 tick_transport。
    fn play_and_drain(state: &mut EditorState, tick: f64) {
        state.prepare_audio();
        state.audio.clock_mut().seek(tick);
        state.begin_playback();
        let ctx = Context::default();
        state.tick_transport(&ctx);
    }

    /// 原版 control_step.gml:289：Show Save Popup → set_msg("Song saved")（硬编码英文）。
    #[test]
    fn save_popup_instrument_shows_the_hardcoded_message() {
        let mut state = state_with(visual_instrument_song("Show Save Popup", 0));
        play_and_drain(&mut state, 0.0);
        assert_eq!(
            state.message.as_ref().map(|(m, _)| m.as_str()),
            Some("Song saved"),
            "原版不翻译这条消息"
        );
    }

    /// 用户要求 #91：演奏到带 `press` 的音符时，对应琴键要跟着弹下。
    /// 派发侧盖章（`VisualEvent::KeyPlayed`）→ 排空时写 `state.key_played`，
    /// 绘制侧 [`crate::piano::played_key`] 用 300ms 窗口决定染黄与下沉。
    #[test]
    fn played_note_stamps_the_piano_key() {
        let one_note = |tick: u32, instrument: u8, key: u8| {
            let mut song = Song::new();
            song.set(tick, 0, Note::new(instrument, key));
            song.recalculate();
            song
        };
        // 内置 0 = Harp（原版建表带 press）；内置 2 = Bass Drum（没带）。
        let mut state = state_with(one_note(0, 0, 45));
        assert_eq!(
            state.key_played[45],
            f64::NEG_INFINITY,
            "没播放过就不该有记录"
        );
        play_and_drain(&mut state, 0.0);
        assert!(
            state.key_played[45].is_finite(),
            "竖琴那一下要给键 45 盖上时间戳，实测 {}",
            state.key_played[45]
        );
        let mut drum = state_with(one_note(0, 2, 47));
        play_and_drain(&mut drum, 0.0);
        assert_eq!(
            drum.key_played[47],
            f64::NEG_INFINITY,
            "底鼓原版没带 press，琴键不该动"
        );
    }

    /// 原版 control_step.gml:290-293 + draw_set_accent：改色只重算 9 档表，
    /// 不写回持久化基色 accent1/2/3。
    #[test]
    fn change_color_instrument_resets_only_the_accent_table() {
        let mut state = state_with(visual_instrument_song("Change Color to #0A0B0C", 0));
        play_and_drain(&mut state, 0.0);
        assert_eq!(
            state.theme.accent_table[3],
            egui::Color32::from_rgb(0x0a, 0x0b, 0x0c),
            "整窗背景那一档应变新色"
        );
        assert_eq!(
            state.theme.accent, state.theme.accent_table[4],
            "派生档跟着表走"
        );
        assert_eq!(
            state.theme.accent_base,
            [0, 120, 212],
            "基色不被改色乐器覆盖"
        );
    }

    /// 原版 toggle_playing 的 Toggle Rainbow 分支：切换 + draw_accent_init（无消息）。
    /// 重置后的表立刻被彩虹动画按新色相覆盖，所以打开侧只验证标志与消息；
    /// 表的重置语义在关闭侧（toggle_rainbow → accent_init）可观察。
    #[test]
    fn rainbow_instrument_toggles_and_reinitializes_the_table() {
        let mut state = state_with(visual_instrument_song("Toggle Rainbow", 3));
        play_and_drain(&mut state, 3.0);
        assert!(state.rainbow, "跨过该列后彩虹应打开");
        assert!(state.rainbow_hue > 0.0, "彩虹动画每帧在走");
        assert_eq!(
            state.message, None,
            "事件乐器的切换不发消息（消息只归快捷键）"
        );
        state.toggle_rainbow();
        assert!(!state.rainbow);
        assert_eq!(
            state.theme.accent_table[3],
            egui::Color32::from_rgb(0, 120, 212),
            "关闭时 accent_init 把表重置回基色"
        );
    }

    /// 原版 control_step.gml:296：Toggle Background Accent 只翻标志，
    /// 铺色发生在每帧 draw_clear（theme 3 且标志开着）。
    #[test]
    fn background_accent_instrument_flips_the_flag() {
        let mut state = state_with(visual_instrument_song("Toggle Background Accent", 0));
        play_and_drain(&mut state, 0.0);
        assert!(state.background_accent);
        // 再来一次（先停再播）要翻回去。
        state.audio.stop();
        play_and_drain(&mut state, 0.0);
        assert!(!state.background_accent, "再落一枚应翻回关");
    }

    /// 原版 Ctrl+Shift+F5 仅 theme 3（control_draw.gml:720）。
    #[test]
    fn rainbow_shortcut_only_exists_on_fluent() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        dispatch(&mut state, Command::SetTheme(ThemeKind::Dark), &ctx);
        dispatch(&mut state, Command::ToggleRainbow, &ctx);
        // 命令本身不拦，闸门在快捷键层（原版 keyboard 判定里带 theme = 3）；
        // 这里验证命令效果可用，快捷键层的 theme 判定在 shortcuts::collect_shortcuts。
        assert!(state.rainbow);
        assert_eq!(
            state.message.as_ref().map(|(m, _)| m.as_str()),
            Some("炫彩模式 => ON")
        );
    }

    /// 原版 `selection_delete.gml:24-28`：删除选区时按视口内格子生成删除效果。
    #[test]
    fn delete_spawns_remove_effects_for_visible_cells() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        // 默认视口 800x400 → 26 列 14 行，(0..2, 0) 都在内。
        state.doc.select_rect(0, 0, 1, 0, false, false, false);
        dispatch(&mut state, Command::Delete, &ctx);
        assert_eq!(state.remove_effects.len(), 2, "两个可见音符各一个效果");
        assert_eq!(state.remove_effects[0].rel, (0, 0));
        assert_eq!(state.remove_effects[1].rel, (1, 0));
        assert!(
            (state.remove_effects[0].alpha() - 0.5).abs() < 1e-6,
            "刚生成时 alpha = 0.5（obj_removeeff Create）"
        );
        // 视口外的格子（tick 50 > 26 列）不生成。
        let mut state = state_with(four_case_song(10.0));
        state.doc.select_rect(50, 0, 50, 0, false, false, false);
        dispatch(&mut state, Command::Delete, &ctx);
        assert!(state.remove_effects.is_empty(), "视口外的删除不生成效果");
    }

    /// 原版闸门：`theme = 3 && remove_effect`（remove_block_manual.gml:64）。
    #[test]
    fn remove_effects_gate_on_theme_and_flag() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.flags.remove_effect = false;
        state.doc.select_rect(0, 0, 0, 0, false, false, false);
        dispatch(&mut state, Command::Delete, &ctx);
        assert!(state.remove_effects.is_empty(), "开关关闭不生成");

        let mut state = state_with(four_case_song(10.0));
        dispatch(&mut state, Command::SetTheme(ThemeKind::Dark), &ctx);
        state.doc.select_rect(0, 0, 0, 0, false, false, false);
        dispatch(&mut state, Command::Delete, &ctx);
        assert!(state.remove_effects.is_empty(), "非 Fluent 主题不生成");
    }

    /// 原版 `obj_removeeff`：alpha 每秒衰减 1.8（真实寿命 1/3.6 秒），衰减完销毁。
    #[test]
    fn remove_effects_decay_in_real_time_and_get_dropped() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.spawn_remove_effects([(0, 0)]);
        assert_eq!(state.remove_effects.len(), 1);
        // 刚生成的效果不该被清掉。
        state.tick_transport(&ctx);
        assert_eq!(state.remove_effects.len(), 1, "寿命没到不清除");
        // 已过寿命的效果（0.5/1.8 ≈ 0.278 秒后 alpha 归零）在下一帧被销毁。
        state.remove_effects[0].age = 1.0;
        state.tick_transport(&ctx);
        assert!(state.remove_effects.is_empty(), "寿命到了要销毁");
    }

    /// 原版 `remove_block_manual.gml:64-68`：右键单删也生成效果；锁定层不删也不生成。
    #[test]
    fn remove_note_spawns_effect_and_respects_lock() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        dispatch(&mut state, Command::RemoveNote { tick: 0, layer: 0 }, &ctx);
        assert_eq!(state.remove_effects.len(), 1);
        assert_eq!(state.remove_effects[0].rel, (0, 0));
        assert!(state.doc.song.get(0, 0).is_none(), "音符确实被删了");

        let mut state = state_with(four_case_song(10.0));
        state.doc.song.layers[0].lock = nbs_core::model::LockMode::Locked;
        dispatch(&mut state, Command::RemoveNote { tick: 0, layer: 0 }, &ctx);
        assert!(state.remove_effects.is_empty(), "锁定层拦截时不生成效果");
        assert!(state.doc.song.get(0, 0).is_some(), "音符还在");
    }

    /// 原版 `menu_click.gml:46-49`：文件菜单那一项先 `save_options_version =
    /// songs[song].save_version` 再开窗，单选草稿从歌曲版本起步。
    #[test]
    fn open_save_options_seeds_the_scratch_from_the_song() {
        let ctx = Context::default();
        let mut song = Song::new();
        song.meta.save_version = 4;
        let mut state = state_with(song);
        state.save_options_version = 0;
        dispatch(&mut state, Command::OpenSaveOptions, &ctx);
        assert_eq!(state.window, Some(WindowKind::SaveOptions));
        assert_eq!(
            state.save_options_version, 4,
            "开窗要把歌曲的保存版本播种给单选草稿"
        );
        assert!(state.modals.is_empty(), "开窗本身不问话");
    }

    /// 版本和当前写盘版本一致时原版不问直接改（`save_options_version !=
    /// nbs_version` 才 `question`），改完关窗；不一致先问，窗口留着。
    #[test]
    fn save_options_ok_asks_only_when_leaving_the_current_version() {
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        let ctx = Context::default();
        let current = nbs_core::nbs::NBS_VERSION;
        let mut state = state_with(Song::new());
        dispatch(
            &mut state,
            Command::ShowWindow(WindowKind::SaveOptions),
            &ctx,
        );
        dispatch(
            &mut state,
            Command::ApplySaveOptions { version: current },
            &ctx,
        );
        assert!(state.modals.is_empty(), "就是当前版本，不用问");
        assert_eq!(state.doc.song.meta.save_version, current);
        assert_eq!(state.window, None, "原版 windowclose = 1");
        assert!(state.doc.history.can_undo(), "原版 cursong.changed = 1");

        let mut state = state_with(Song::new());
        dispatch(
            &mut state,
            Command::ShowWindow(WindowKind::SaveOptions),
            &ctx,
        );
        dispatch(&mut state, Command::ApplySaveOptions { version: 0 }, &ctx);
        assert_eq!(titles(&state), vec!["确定"]);
        assert_eq!(
            texts(&state),
            vec!["如果你保存在旧版本的话一些歌曲信息将会丢失！你确定吗？"]
        );
        assert!(state.modals.then.is_some());
        assert_eq!(state.doc.song.meta.save_version, 6, "问完还没改");
        assert_eq!(
            state.save_options_version, 6,
            "原版否分支要 `save_options_version = cursong.save_version`，提前做掉"
        );
        assert_eq!(
            state.window,
            Some(WindowKind::SaveOptions),
            "问话时窗口还留着"
        );
    }

    /// 上面那问的答否：不改歌曲、窗口留着（原版是 `return`）。答是：走
    /// `SaveOptionsConfirm`，把手上的版本写回并关窗。
    #[test]
    fn save_options_question_branches_apply_or_keep_the_song_version() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        dispatch(
            &mut state,
            Command::ShowWindow(WindowKind::SaveOptions),
            &ctx,
        );
        dispatch(&mut state, Command::ApplySaveOptions { version: 3 }, &ctx);
        dispatch(&mut state, Command::AnswerModal(false), &ctx);
        assert!(state.modals.is_empty());
        assert_eq!(state.doc.song.meta.save_version, 6, "答否不改保存版本");
        assert_eq!(state.window, Some(WindowKind::SaveOptions));

        let mut state = state_with(Song::new());
        dispatch(
            &mut state,
            Command::ShowWindow(WindowKind::SaveOptions),
            &ctx,
        );
        dispatch(&mut state, Command::ApplySaveOptions { version: 3 }, &ctx);
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        assert_eq!(state.doc.song.meta.save_version, 3, "答是才写回");
        assert_eq!(state.save_options_version, 3);
        assert_eq!(state.window, None);
    }
}
