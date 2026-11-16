//! UI 命令总线：菜单、工具栏、快捷键、上下文菜单、画布交互都只 emit 命令，
//! 由 `dispatch` 统一落地，避免状态散落在各个绘制函数里。

use std::path::PathBuf;

use egui::Context;
use nbs_audio::{export_wav, ExportOptions, EXTENDED_MAX_VOICES, DEFAULT_MAX_VOICES};
use nbs_core::model::{
    panning_velocity_to_short, short_to_panning_velocity, LockMode, Note, Song, SongMeta, KEY_MAX,
    PAN_MAX, VEL_MAX, PIT_MAX,
};
use nbs_core::Macro;

use crate::state::{DialogAction, EditMode, EditorState, FileDialog, Flag, Modal, ModalQueue, PrefsFolder, WindowKind};
use crate::theme::{Theme, ThemeKind};

#[derive(Clone, Debug)]
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
    SelectByInstrument { instrument: u8, invert: bool },
    PlaceNote { tick: u32, layer: u32 },
    /// 速度变换编辑窗「确定」：pit = BPM（原版 `h_changeblock`）。
    SetEventTempo { tick: u32, layer: u32, bpm: i64 },
    /// 消音器编辑窗「确定」：pit = 起始层，终止层编码进声像 + 力度。
    SetEventStopper { tick: u32, layer: u32, start: i64, until: u64 },
    RemoveNote { tick: u32, layer: u32 },
    PickNote { tick: u32, layer: u32, all: bool },
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
    /// （`window_scale`），网格本身恒 32px。
    WindowScaleUp,
    WindowScaleDown,
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
    Tremolo(i32),
    Stereo(i16),
    SetTempo(f32),
    SetTimeSignature(u8),
    SetSaveVersion(u8),
    ClearSong,
    /// 原版右上角兼容性徽章被点（`control_draw.gml:2029-2038`）：停播放、关录制、
    /// 落回浮动选区，再打开 `w_minecraft`。
    ShowCompat,
    /// 兼容性窗口的两条速度阶梯，Schematic 页和数据包页各一条（`draw_window_minecraft.gml`）。
    FixTempoSchematic,
    FixTempoDatapack,
    /// 原版 `select_outside(lower, higher)`，按钮点完跟着 `windowclose = 1`。
    SelectOutside { lower: bool, higher: bool },
    /// 原版 `select_custom()`，同样跟着 `windowclose = 1`。
    SelectCustom,
    /// 兼容性窗口「转换音符」：先 `question()`，答「是」才走下一条。
    TransposeToMinecraft,
    /// 上面那问的「是」分支：全选 → 折回两个八度 → 落回 → 按原版那句英文报数。
    TransposeToMinecraftApply,
    /// 「保存资源包」= 原版 `datapack_getextranotes()`，兼容性窗口和数据包导出窗口都有。
    SaveExtraNotesPack,
    /// 鼠标悬停在音符上滚轮调整其属性（对应原版 `changepitch`）。
    WheelOnNote { tick: u32, layer: u32, delta: i32, shift: bool },
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
    /// 原版 `menu_click` 的导出 Schematic 入口：先过 `calculate_size()` 的提示与提问。
    OpenSchematicExport,
    /// 原版「导出」按钮：两道闸门都过才弹保存框。
    ExportSchematicChecked,
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
    SetMasterVolume(f32),
    SetPlaySpeed(f32),
    ToggleMetronome,
    ToggleExtendedVoices,
    ToggleMono,
    SetSoundsDir(String),
    ReloadSamples,
    SavePreferences,
    RestoreAutosave(PathBuf),
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
    let tick = state.marker().ceil().max(0.0) as u32;
    let layers = state.doc.song.layer_count();
    let mut layer = 0u32;
    while layer < layers && state.doc.song.has(tick, layer) {
        layer += 1;
    }
    if layer >= layers {
        state.doc.song.layers.push(Default::default());
    }
    let note = pressed_note(state, key);
    state.doc.place_note(tick, layer, note);
    // 原版 add_block_manual 结尾 `if (ins.loaded) play_sound(...)`：写下去的同时出声。
    state.audio.preview_note(&note);
}

/// 供右键菜单/快捷键共用的入口。
pub fn dispatch(state: &mut EditorState, cmd: Command, ctx: &Context) {
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
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.window = None;
            state.set_message("已新建歌曲");
        }
        Command::Open => {
            // 原版 `load_song.gml:11-12`：起始目录只看 `songfolder`，默认名是空串。
            dispatch(state, Command::OpenDialog(DialogAction::OpenSong), ctx);
        }
        Command::SaveAs => {
            // 原版 `save_song.gml:13-14`：目录同样是 `songfolder`，默认名是 `fsave + ".nbs"`。
            dispatch(state, Command::OpenDialog(DialogAction::SaveSong), ctx);
        }
        Command::Save => {
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
        Command::OpenPath(path) => match nbs_core::nbs::load(&path) {
            Ok(song) => {
                let name = song.meta.name.clone();
                // 原版 `open_song_nbs.gml:12-13`：文件版本比 `nbs_version` 旧就先警告。
                // 比它新的文件根本进不来这一支（加载器直接报「不支持的 NBS 版本」，
                // 对应源码 :14-18 的报错并 return -1）。
                let old = song.meta.save_version < nbs_core::nbs::NBS_VERSION
                    && state.prefs.show_old_warning;
                let text = nbs_io::prefs::old_file_warning("NBS");
                state.doc.replace_song(song);
                state.prepare_audio();
                state.viewport.start_tick = 0;
                state.viewport.start_layer = 0;
                state.set_marker(0.0);
                state.add_recent(path.clone());
                state.window = None;
                if old {
                    state.modals = ModalQueue::message("警告", &text);
                }
                state.set_message(format!(
                    "已打开 {}（{} 个音符）",
                    name,
                    state.doc.song.total_blocks
                ));
            }
            Err(e) => state.set_message(format!("打开失败: {e}")),
        },
        Command::SavePath(path) => {
            let version = state.doc.song.meta.save_version;
            match nbs_core::nbs::save(&path, &state.doc.song, version) {
                Ok(()) => {
                    state.doc.song.filename = Some(path.clone());
                    state.doc.song.dirty = false;
                    state.add_recent(path);
                    state.window = None;
                    state.set_message("歌曲已保存");
                }
                Err(e) => state.set_message(format!("保存失败: {e}")),
            }
        }
        Command::Undo => {
            if state.doc.undo() {
                state.set_message(format!(
                    "撤销: {}",
                    state.doc.history.undo_label().unwrap_or("")
                ));
            }
        }
        Command::Redo => {
            if state.doc.redo() {
                state.set_message(format!(
                    "重做: {}",
                    state.doc.history.redo_label().unwrap_or("")
                ));
            }
        }
        Command::Copy => {
            state.doc.copy_selection();
            ctx.copy_text(state.doc.clipboard_text.clone());
            state.set_message(format!("已复制 {} 个音符", state.doc.clipboard.len()));
        }
        Command::Cut => {
            state.doc.cut_selection();
            ctx.copy_text(state.doc.clipboard_text.clone());
            state.set_message("已剪切");
        }
        Command::Paste => {
            let (x, y) = state
                .hover_cell
                .map(|(t, l)| (t as i64, l as i64))
                .unwrap_or((state.viewport.start_tick as i64, state.viewport.start_layer as i64));
            state.doc.paste(x, y);
            state.set_message("已粘贴（浮动选区，拖拽可定位）");
        }
        Command::PasteText(text) => {
            let (x, y) = state
                .hover_cell
                .map(|(t, l)| (t as i64, l as i64))
                .unwrap_or((state.viewport.start_tick as i64, state.viewport.start_layer as i64));
            if text.contains('|') {
                state.doc.paste_text(&text, x, y);
                state.set_message("已从剪贴板文本粘贴");
            } else {
                dispatch(state, Command::Paste, ctx);
            }
        }
        Command::Delete => state.doc.delete_selection(),
        Command::SelectAll => state.doc.select_all(),
        Command::Deselect => state.doc.deselect(),
        Command::InvertSelection => state.doc.select_invert(),
        Command::SelectByInstrument { instrument, invert } => {
            state.doc.select_by_instrument(instrument, invert);
            state.set_message(format!("已选中 {} 个音符", state.doc.selection.len()));
        }
        Command::PlaceNote { tick, layer } => {
            if state.doc.song.is_locked(layer) {
                state.set_message("图层已锁定");
                return;
            }
            // 原版 add/change_block_manual 的结尾：落的是事件乐器就不直接吃画笔值，
            // 改写事件默认值并弹出编辑窗；落点已有事件乐器时是「再编辑」——草稿
            // 回填现有值，音符本身不动（原版 is_event 分支）。
            let brush = state.current;
            let name = state.doc.song.instrument_name(brush.instrument).to_string();
            let existing = state.doc.song.get(tick, layer);
            let existing_is_event = existing
                .as_ref()
                .is_some_and(|n| {
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
                        let note = Note { pitch: bpm, ..brush };
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
        Command::SetEventStopper { tick, layer, start, until } => {
            state.doc.set_event_stopper(tick, layer, start, until);
        }
        Command::RemoveNote { tick, layer } => {
            if state.doc.song.is_locked(layer) {
                state.set_message("图层已锁定");
                return;
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
                        state
                            .doc
                            .selection
                            .at(
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
                state.set_message(format!(
                    "吸管: {} / {}",
                    nbs_core::instruments::instrument_name(n.instrument),
                    nbs_core::model::key_name(n.key, true, state.flags.flats)
                ));
            }
        }
        Command::SelectRect { x0, y0, x1, y1, copy, additive, subtract } => {
            if subtract {
                state.doc.selection_remove_rect(x0, y0, x1, y1);
            } else {
                state.doc.select_rect(x0, y0, x1, y1, copy, additive, false);
                state.set_message(format!("已选中 {} 个音符", state.doc.selection.len()));
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
            state.audio.preview_note(&state.current);
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
            state.audio.preview_note(&state.current);
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
            state.doc.edit_meta("切换循环", |m| m.loop_enabled = !m.loop_enabled);
            state.sync_loop_params();
        }
        Command::ToggleFollow => state.marker_follow = !state.marker_follow,
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
            state.window = None;
            // 原版的 `obj_menu` 跟着窗口一起销毁。这里不清的话，下拉菜单开着时用
            // Escape / Ctrl+P 关窗，下次打开首选项会凭空冒出一个悬浮菜单。
            state.prefs_theme_menu = false;
            state.prefs_rate_menu = false;
        }
        // ------------------------------------------------------------ 导出 Schematic
        // 原版 `menu_click`：`if (calculate_size()) window = w_schematic_export`。
        Command::OpenSchematicExport => {
            state.schematic_stats = nbs_io::schematic::calculate_stats(&state.doc.song);
            state.schematic_options.tempo_mode = state.schematic_stats.tempo_mode;
            let st = &state.schematic_stats;
            let mut items: std::collections::VecDeque<Modal> = std::collections::VecDeque::new();
            if !state.prefs.warning_schematic {
                items.push_back(Modal {
                    title: "警告",
                    text: "导出的 schematic 目前之和 Minecraft: Java Edition 1.12 及以下兼容。\
如想在 1.13 和以上使用，请使用结构方块格式导出。\n\n此警告将不再提示。"
                        .into(),
                    yes_no: false,
                });
                // 原版在这里 `save_settings()`，把“已看过”落盘。
                state.prefs.warning_schematic = true;
                let _ = state.prefs.save();
            }
            for q in st.compat_questions() {
                items.push_back(Modal { title: "Minecraft 兼容性", text: q, yes_no: true });
            }
            if let Some(t) = st.tempo_notice() {
                items.push_back(Modal { title: "速度", text: t.into(), yes_no: false });
            }
            let guard = nbs_io::schematic::open_guard(st);
            if let Err(e) = &guard {
                items.push_back(Modal { title: "错误", text: e.clone(), yes_no: false });
            }
            state.modals = ModalQueue {
                items,
                then: guard.ok().map(|_| Command::ShowWindow(WindowKind::SchematicExport)),
            };
            if state.modals.is_empty() {
                dispatch(state, Command::ShowWindow(WindowKind::SchematicExport), ctx);
            }
        }
        Command::AnswerModal(yes) => {
            if !yes {
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
            state.set_message("导出 Schematic 的设定已还原");
        }
        Command::MidiImportRun => {
            // 原版 `import_midi()`：用窗口里那份已解析的 MIDI + 界面上的映射表落一首新歌，
            // 收尾按「记住我的更改」把映射写回 GM 表，否则只把 w_midi_* 恢复成出厂值，
            // 两种情况最后都会 `save_settings()`。
            let Some(midi) = state.midi_data.clone() else {
                state.window = None;
                return;
            };
            let song = nbs_io::midi::import_midi_data(&midi, &state.midi_options);
            let n = song.total_blocks;
            state.midi_options.finish_import(&midi, &mut state.midi_gm);
            state.window = None;
            state.midi_data = None;
            state.midi_file.clear();
            state.midi_tab = 0;
            state.doc.replace_song(song);
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.save_prefs();
            state.set_message(format!("MIDI 导入完成：{n} 个音符"));
        }
        Command::MidiUseDefaults => {
            // 原版「使用默认值」答「是」之后：先 `midi_instruments()` 把 GM 映射恢复出厂，
            // 再按这份出厂表重刷界面四张表，最后把 w_midi_* 设成出厂值。
            let Some(midi) = state.midi_data.clone() else { return };
            state.midi_gm = nbs_io::MidiGmTable::default();
            state.midi_options.use_defaults(&midi, &state.midi_gm);
            state.set_message("MIDI 导入设定已还原");
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
            let st = state.schematic_stats.clone();
            match nbs_io::schematic::export_check(&st, &state.schematic_options).dialog() {
                None => dispatch(state, Command::OpenDialog(DialogAction::ExportSchematic), ctx),
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
            state.set_message(format!("主题 => {}", kind.name()));
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
            }
        }
        Command::Transpose(semi) => state.doc.transpose(semi),
        Command::SetAllInstrument(ins) => state.doc.set_instrument_all(ins),
        Command::BatchVelocity(v) => {
            let v = v.min(VEL_MAX);
            state.doc.batch_change("批量设置力度", |n| n.velocity = v);
        }
        Command::BatchPanning(v) => {
            let v = v.min(PAN_MAX);
            state.doc.batch_change("批量设置声像", |n| n.panning = v);
        }
        Command::BatchPitch(v) => {
            let v = v.clamp(-PIT_MAX, PIT_MAX);
            state.doc.batch_change("批量设置微调", |n| n.pitch = v);
        }
        Command::Tremolo(amount) => {
            // 按列交替升降力度，等价于原版 Tremolo 宏的简化版。
            state.doc.batch_change("颤音", |n| {
                let d = if n.key % 2 == 0 { amount } else { -amount };
                n.velocity = (n.velocity as i32 + d).clamp(0, VEL_MAX as i32) as u8;
            });
        }
        Command::Stereo(amount) => {
            let mut i = 0i32;
            state.doc.batch_change("立体声", |n| {
                let d = if i % 2 == 0 { amount as i32 } else { -(amount as i32) };
                n.panning = (n.panning as i32 + d).clamp(0, PAN_MAX as i32) as u8;
                i += 1;
            });
        }
        Command::SetTempo(t) => {
            state.doc.edit_meta("修改速度", |m| m.tempo = t.clamp(0.25, 60.0));
            state.audio.clock_mut().set_tempo(state.doc.song.meta.tempo);
        }
        Command::SetTimeSignature(ts) => {
            state.doc.edit_meta("修改拍号", |m| m.time_signature = ts.clamp(2, 8));
        }
        Command::SetSaveVersion(v) => {
            state.doc.edit_meta("修改保存版本", |m: &mut SongMeta| m.save_version = v.min(nbs_core::nbs::MAX_NBS_VERSION));
        }
        Command::ClearSong => {
            state.doc.clear_all_notes();
            state.prepare_audio();
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
            state.set_message(format!("已选中 {} 个音符", state.doc.selection.len()));
            state.window = None;
        }
        Command::SelectCustom => {
            state.doc.select_custom();
            state.set_message(format!("已选中 {} 个音符", state.doc.selection.len()));
            state.window = None;
        }
        Command::TransposeToMinecraft => {
            state.modals = ModalQueue::question(
                "转换音符",
                "转换音符使其在 Minecraft 的 2 八度限制内吗？",
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
            dispatch(state, Command::OpenDialog(DialogAction::SaveExtraNotes), ctx);
        }
        Command::WheelOnNote { tick, layer, delta, shift } => {
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
                state.modals = ModalQueue::message("错误", "该路径不存在！");
            } else if open_in_file_manager(&dir).is_err() {
                state.modals = ModalQueue::message("错误", "无法打开文件管理器。");
            }
        }
        Command::ResetPrefsFolder(folder) => match folder {
            PrefsFolder::Song => state.prefs.song_folder = None,
            PrefsFolder::Pattern => state.prefs.pattern_folder = None,
        },
        Command::ChangePrefsFolder(folder) => {
            // 原版按钮里先 `message(说明, "")`，再弹 `get_save_filename_ext`。
            state.modals = ModalQueue {
                items: [Modal { title: "", text: folder.tip().into(), yes_no: false }]
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
                | DialogAction::PickFolder(PrefsFolder::Song) => Some(state.prefs.song_folder_path()),
                DialogAction::ImportPattern
                | DialogAction::ExportPattern
                | DialogAction::PickFolder(PrefsFolder::Pattern) => {
                    Some(state.prefs.pattern_folder_path())
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
            if action == DialogAction::SaveExtraNotes {
                // 原版 `datapack_getextranotes.gml:6`：默认名写死「更多音符盒」，
                // 跟当前歌曲完全无关；扩展名交给对话框的 *.zip 过滤器补。
                dialog.name = "更多音符盒".to_string();
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
                        PrefsFolder::Song => state.prefs.song_folder = Some(path.to_string_lossy().into_owned()),
                        PrefsFolder::Pattern => {
                            state.prefs.pattern_folder = Some(path.to_string_lossy().into_owned())
                        }
                    }
                    state.file_dialog = None;
                    // 原版的系统对话框叠在首选项窗口上；我们同时只开一个窗口，
                    // 选完把首选项放回去（页签存在 `state.prefs_tab`，不会丢）。
                    state.window = Some(WindowKind::Preferences);
                }
                DialogAction::ExportAudio => {
                    export_audio(state, &path);
                    state.window = None;
                }
                DialogAction::ExportMidi => {
                    match nbs_io::midi::export_midi(&state.doc.song, &path) {
                        Ok(()) => state.set_message("MIDI 已导出"),
                        Err(e) => state.set_message(format!("导出 MIDI 失败: {e}")),
                    }
                    state.window = None;
                }
                DialogAction::ExportPng => {
                    let opts = state.png_options.clone();
                    match nbs_io::image_export::export_png(&state.doc.song, &path, &opts) {
                        Ok((w, h)) => {
                            state.export_status = Some(format!("PNG 谱面已导出：{w}×{h}"));
                            state.set_message(format!("PNG 谱面已导出：{w}×{h}"));
                        }
                        Err(e) => state.set_message(format!("导出 PNG 失败: {e}")),
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
                    match nbs_io::schematic::export_schematic(&state.doc.song, &path, &opts) {
                        // 原版：保存成功后 `message("Schematic 已保存！")`，
                        // 关掉提示再把导出窗口重新打开。
                        Ok(info) => {
                            state.set_message(format!(
                                "Schematic 已保存：{}×{}×{}，{} 个音符盒",
                                info.length, info.width, info.height, info.note_blocks
                            ));
                            let mut q = ModalQueue::message("导出 Schematic", "Schematic 已保存！");
                            q.then = Some(Command::ShowWindow(WindowKind::SchematicExport));
                            state.modals = q;
                        }
                        Err(e) => state.set_message(format!("导出 Schematic 失败: {e}")),
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
                        Ok(()) => state.set_message(format!("片段已导出：{} 音符", sel.len())),
                        Err(e) => state.set_message(format!("导出片段失败: {e}")),
                    }
                    state.window = None;
                }
                DialogAction::ImportMidi => {
                    // 原版 `open_midi()`：先读盘解析，解析成功才开导入窗口，
                    // 并把当前 GM 映射表摊到界面四张表上（不落歌，等「导入」）。
                    match nbs_io::midi::parse_midi_path(&path) {
                        Ok(midi) => {
                            state.midi_options.reload_tables(&midi, &state.midi_gm);
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
                            state.set_message(format!("导入 MIDI 失败: {e}"));
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
                                .unwrap_or((state.viewport.start_tick as i64, state.viewport.start_layer as i64));
                            state.doc.paste(x, y);
                            state.window = None;
                            if let Some(warn) = warn {
                                state.modals = ModalQueue::message("警告", &warn);
                            }
                            state.set_message(format!("片段已导入：{n} 个音符（浮动选区）"));
                        }
                        Err(e) => state.set_message(format!("导入片段失败: {e}")),
                    }
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
                            state.modals = ModalQueue::message("保存", "资源包保存成功！");
                        }
                        Err(e) => state.set_message(format!("保存资源包失败: {e}")),
                    }
                }
            }
            state.file_dialog = None;
        }
        Command::RunMacro(m) => {
            let label = m.name().to_string();
            state.doc.run_macro(&m);
            state.prepare_audio();
            state.set_message(format!("已应用宏：{label}"));
        }
        Command::SetMasterVolume(v) => {
            state.prefs.master_volume = v.clamp(0.0, 2.0);
            state.apply_audio_prefs();
        }
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
            state.audio.clock_mut().set_max_voices(if state.prefs.extended_voices {
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
            state.prefs.sounds_dir = if dir.trim().is_empty() { None } else { Some(dir) };
            dispatch(state, Command::ReloadSamples, ctx);
        }
        Command::ReloadSamples => {
            let dir = state.prefs.sounds_dir_path();
            state.audio.reload_bank(dir.as_deref());
            state.prepare_audio();
            state.set_message(format!("采样已重新加载：{}", state.audio.sample_status()));
        }
        Command::SavePreferences => {
            state.save_prefs();
            state.set_message("偏好设置已保存");
        }
        Command::RestoreAutosave(path) => dispatch(state, Command::OpenPath(path), ctx),
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
            state.prepare_audio();
            state.viewport.start_tick = 0;
            state.viewport.start_layer = 0;
            state.set_marker(0.0);
            state.set_message(format!("Schematic 导入完成：{n} 个音符，{inputs} 个输入"));
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
            None => state.set_message(format!("导入 Schematic 失败: {e}")),
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
        state.set_message(format!("导出音频失败：未知格式 {:?}", state.prefs.export_format));
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
            state.set_message(format!("导出音频失败: {e}"));
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
            state.export_status = Some(format!(
                "{} 已导出：{} 音符 / {:.1}s / {} 线程 / 峰值 {:.2} → {}",
                format.name(),
                stats.notes,
                stats.duration_seconds,
                stats.threads,
                stats.peak,
                path.display()
            ));
            state.set_message(format!("音频已导出到 {}", path.display()));
        }
        Err(e) => {
            state.export_status = None;
            state.set_message(format!("导出音频失败: {e}"));
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
            state.export_status = Some(format!(
                "数据包已保存到 {}（{} 个文件，{}）\n\n如想在游戏内播放，使用命令：\n/function {}play\n/function {}pause\n/function {}stop\n\n如果你想从你的世界中卸载它，使用命令：\n/function {}uninstall\n\n然后从“datapacks”文件夹取出就行了。",
                dest.display(),
                info.files,
                if info.zip { "ZIP" } else { "目录" },
                fp,
                fp,
                fp,
                fp
            ));
            state.set_message(format!("数据包已导出：{} 个文件", info.files));
        }
        Err(e) => {
            state.export_status = None;
            state.set_message(format!("导出数据包失败: {e}"));
        }
    }
}

/// 供菜单与状态栏显示的当前画笔描述。
pub fn current_note_label(state: &EditorState) -> String {
    let c: Note = state.current;
    format!(
        "{} | {} | V{} | P{} | {}¢",
        nbs_core::instruments::instrument_name(c.instrument),
        nbs_core::model::key_name(c.key, true, state.flags.flats),
        c.velocity,
        c.panning,
        c.pitch
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Layer;
    use std::path::Path;

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
        let mut s = EditorState::new();
        // 原版第一次走到这里会写 settings.ini；测试预先当作「已经提示过」，不碰用户配置。
        s.prefs.warning_schematic = true;
        s.doc.replace_song(song);
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
    fn open_schematic_export_queues_the_original_dialogs_in_order() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(3.0));
        dispatch(&mut state, Command::OpenSchematicExport, &ctx);

        assert_eq!(state.window, None, "队列没走完不得打开导出窗口");
        assert_eq!(
            titles(&state),
            vec!["Minecraft 兼容性", "Minecraft 兼容性", "Minecraft 兼容性", "速度"]
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
            state.modals.items.iter().map(|m| m.yes_no).collect::<Vec<_>>(),
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
        assert_eq!(state.schematic_options.tempo_mode, 2, "3.0 t/s 对应 2.5 红石刻档");
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
        song.set(2, 0, Note { instrument: tempo_ins, key: 50, velocity: 80, panning: 60, pitch: 150 });
        song.set(3, 0, Note { instrument: stopper_ins, key: 40, velocity: 100, panning: 157, pitch: 39 });
        let mut state = state_with(song);

        dispatch(
            &mut state,
            Command::SetEventTempo { tick: 2, layer: 0, bpm: 400 },
            &ctx,
        );
        let n = state.doc.song.get(2, 0).unwrap();
        assert_eq!(n.pitch, 400);
        assert_eq!((n.key, n.velocity, n.panning), (50, 80, 60));

        dispatch(
            &mut state,
            Command::SetEventStopper { tick: 3, layer: 0, start: 10, until: 57 },
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
        assert_eq!(state.schematic_options.tempo_mode, 1, "sch_exp_tempo 不在默认值表里");
        assert!(state.schematic_options.structure, "structure 是全局变量，不被重置");
        assert!(state.schematic_options.command_block, "command_block 同样不在表里");
    }

    /// 「导出」按钮的两道闸门各有一个不同标题的 message，通过后才轮到保存对话框。
    #[test]
    fn export_button_gates_run_before_the_save_dialog() {
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
            vec!["导出 Schematic"],
            "原版这里的标题是「导出 Schematic」，不是「错误」"
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
        assert_eq!(state.schematic_options.layout, 1, "答「是」后队列尾部的动作照常执行");
    }

    /// 保存框的文件名跟原版一致：经典格式保留原名，结构方块转小写并加下划线。
    #[test]
    fn schematic_dialog_extension_follows_the_format_choice() {
        let ctx = Context::default();
        let mut state = state_with(four_case_song(10.0));
        state.doc.song.filename = Some(Path::new("Megalovania Boss.nbs").to_path_buf());

        state.schematic_options.structure = false;
        dispatch(&mut state, Command::OpenDialog(DialogAction::ExportSchematic), &ctx);
        let d = state.file_dialog.clone().unwrap();
        assert_eq!(d.ext, "schematic");
        assert_eq!(d.name, "Megalovania Boss.schematic");

        state.schematic_options.structure = true;
        dispatch(&mut state, Command::OpenDialog(DialogAction::ExportSchematic), &ctx);
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
        assert_eq!(state.current.instrument, 1, "原版 mouse_wheel_up 往后走一格");
        assert_eq!(state.current.velocity, 100);
        dispatch(&mut state, Command::WheelInstrument(-1), &ctx);
        assert_eq!(state.current.instrument, 0);
        dispatch(&mut state, Command::WheelInstrument(-1), &ctx);
        assert_eq!(state.current.instrument, 0, "第 0 格再往下没有位置，保持不动");

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
        let ctx = Context::default();
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().into_owned();
        let mut state = state_with(Song::new());

        // 起始目录：原版歌曲对话框只看 songfolder，当前文件在哪它不看。
        state.prefs.song_folder = Some(tmp_str.clone());
        dispatch(&mut state, Command::OpenDialog(DialogAction::OpenSong), &ctx);
        assert_eq!(state.file_dialog.clone().unwrap().dir, tmp);
        state.file_dialog = None;
        dispatch(&mut state, Command::OpenDialog(DialogAction::ExportPattern), &ctx);
        assert_eq!(
            state.file_dialog.clone().unwrap().dir,
            nbs_io::prefs::Preferences::default_pattern_folder(),
            "片段对话框只看 patternfolder"
        );
        state.file_dialog = None;

        // 「还原默认」清掉自定义值，界面与对话框一起退回出厂目录。
        dispatch(&mut state, Command::ResetPrefsFolder(PrefsFolder::Song), &ctx);
        assert_eq!(state.prefs.song_folder, None);
        assert_eq!(state.prefs.song_folder_path(), nbs_io::prefs::Preferences::default_song_folder());

        // 「修改」：原版先 message 说明，答完才弹目录选择框。
        dispatch(&mut state, Command::ChangePrefsFolder(PrefsFolder::Pattern), &ctx);
        assert_eq!(titles(&state), vec![""]);
        assert_eq!(texts(&state), vec!["选择导出和导入片段的文件夹"]);
        assert!(matches!(
            state.modals.then,
            Some(Command::OpenDialog(DialogAction::PickFolder(PrefsFolder::Pattern)))
        ));
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        let d = state.file_dialog.clone().unwrap();
        assert_eq!(d.action, DialogAction::PickFolder(PrefsFolder::Pattern));
        assert_eq!(d.ext, "", "原版第 1 个参数是空过滤器串");
        assert_eq!(d.name, "选择片段文件夹", "原版把占位串塞进文件名栏（`draw_window_preferences.gml:294`）");
        assert!(d.save, "原版这条走的是 get_save_filename_ext，不是目录选择器");

        // 确定：认的是对话框当时显示的目录，随后把首选项窗口放回去。
        dispatch(&mut state, Command::DialogConfirm(tmp.clone()), &ctx);
        assert_eq!(state.prefs.pattern_folder, Some(tmp_str));
        assert_eq!(state.window, Some(WindowKind::Preferences));

        // 「打开」：连出厂目录都不存在时，原版弹「该路径不存在！」。
        state.prefs.pattern_folder = Some("Z:\\nbs-rust 没有这个目录".into());
        dispatch(&mut state, Command::OpenPrefsFolder(PrefsFolder::Pattern), &ctx);
        assert_eq!(titles(&state), vec!["错误"]);
        assert_eq!(texts(&state), vec!["该路径不存在！"]);
    }

    /// 原版 `open_song_nbs.gml:12-13`：文件版本比本程序旧才警告，
    /// 而且完全受首选项「打开旧版格式时提示」管。
    #[test]
    fn opening_an_older_nbs_warns_only_when_told_to() {
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

    /// 原版 `control_draw.gml:11-12`：窗口物理尺寸不变、逻辑分辨率除以 `window_scale`，
    /// 等价于 egui 的 `pixels_per_point`。`None`（出厂值 = 系统 DPI / 96）就是 1.0。
    #[test]
    fn window_scale_pref_drives_pixels_per_point() {
        let ctx = Context::default();
        let mut state = state_with(Song::new());
        state.prefs.window_scale = Some(2.0);
        // 缩放要下一帧才生效（egui 在 begin_pass 里消费 new_zoom_factor），原版也是下一帧重算 rw/rh。
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        let out = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert_eq!(out.pixels_per_point, 2.0, "200% 就是每点 2 物理像素");
        // 「重置」存回 None，必须真的退回系统倍率，不然按下去没反应。
        state.prefs.window_scale = None;
        let _ = ctx.run(raw_input(), |c| state.tick_transport(c));
        let out = ctx.run(raw_input(), |c| state.tick_transport(c));
        assert_eq!(out.pixels_per_point, 1.0, "重置后要退回出厂缩放");
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
                if up { Command::WindowScaleUp } else { Command::WindowScaleDown },
                &ctx,
            )
        };
        // 出厂是 None（= 系统倍率，测试环境里 1.0），往上第一档 1.25 而不是连续值。
        step(true, &mut state);
        assert_eq!(state.prefs.window_scale, Some(1.25), "1.0 的上一档应是 1.25");
        // 再往下要一路退到 0.5，中间一档都不能漏。
        for want in [1.0, 0.9, 0.8, 0.75, 0.67, 0.5] {
            step(false, &mut state);
            assert_eq!(state.prefs.window_scale, Some(want), "往下漏了一档");
        }
        step(false, &mut state);
        assert_eq!(state.prefs.window_scale, Some(0.5), "0.5 是原版下界，不能再小");
        for want in [0.67, 0.75, 0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0] {
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
        for (rate, fps) in [(0u8, 30.0f64), (1, 60.0), (2, 120.0), (3, 144.0), (4, 240.0)] {
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
        assert_eq!((new.width, new.height), (256, 256), "原版那两个 .ico 只有 256 一档");
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
        assert!(state.doc.selection.is_empty(), "原版 selection_place(0)：选区要落回歌曲");
        assert_eq!(state.doc.song.total_blocks, 4);
        assert_eq!(state.mc_tab, 0, "原版开窗就从 Schematic 页开始");
    }

    /// 原版 `toggle_playing` 起播第一行：播完停在歌尾（歌长 + 16 刻）再按
    /// 播放要回到 0 重新起播；中途位置起播原地不动。
    #[test]
    fn play_after_reaching_the_end_restarts_from_zero() {
        let mut state = state_with(four_case_song(10.0));
        let end_stop = state.doc.song.song_length() as f64
            + nbs_audio::scheduler::END_STOP_MARGIN_TICKS;
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
        assert_eq!(state.window, Some(WindowKind::MinecraftCompat), "选完放回原窗口");
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
        assert_eq!(state.theme.accent, state.theme.accent_table[4], "派生档跟着表走");
        assert_eq!(state.theme.accent_base, [0, 120, 212], "基色不被改色乐器覆盖");
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
        assert_eq!(state.message, None, "事件乐器的切换不发消息（消息只归快捷键）");
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
}
