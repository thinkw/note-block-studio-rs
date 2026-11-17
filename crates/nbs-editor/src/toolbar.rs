//! 顶部两条：原版 4 个标签菜单（文件 / 编辑 / 设置 / 帮助）+ 一条 30px 图标条。
//!
//! 图标条严格照原版 `control_draw.gml` 的 `draw_icon` 序列排：
//! `xx = 6, yy = 23`（本条顶部即原版 y = 20，所以换成条内坐标），按钮 25×25、
//! 间距 25，段间 `xx += 4` → 竖线 → `xx += 4`，段尾按钮再多走 4px。
//! 右侧固定位置：主音量拖条（`xx += 8` 起，长 100）与 Minecraft 兼容性指示。
//! 窗口太窄时按原版的 `if (xx > rw - 312) break` 逐个丢弃。
//!
//! 原版有、我们还没有的功能不画按钮，免得给出一个点了没反应的假图标：
//! MIDI 设备管理器（`icons.MIDI_INPUT`）。因此这一段会比原版少 25px。

use std::sync::Arc;

use egui::{pos2, Align2, Color32, FontId, Id, OpenUrl, Pos2, Rect, Sense, Ui, Vec2};

use crate::commands::Command;
use crate::icons::{frame, Bank, IconHit};
use crate::state::{EditMode, EditorState, WindowKind};
use crate::theme::{Theme, ThemeKind};
use crate::tr;

/// 图标条里走动的绘制游标，对应原版的 `xx` / `yy`。
struct Bar {
    bank: Arc<Bank>,
    th: Theme,
    x: f32,
    y: f32,
}

impl Bar {
    /// 原版 `if (xx > rw - 312) break`。
    fn overflows(&self, rw: f32) -> bool {
        self.x > rw - 312.0
    }

    fn icon(
        &mut self,
        ui: &mut Ui,
        index: usize,
        tip: &str,
        locked: bool,
        pressed: bool,
    ) -> IconHit {
        let hit = self.bank.icon(
            ui,
            pos2(self.x, self.y),
            index,
            tip,
            locked,
            pressed,
            &self.th,
        );
        self.x += 25.0;
        hit
    }

    /// 段尾按钮：原版写成 `xx += 25 + 4`。
    fn icon_end(
        &mut self,
        ui: &mut Ui,
        index: usize,
        tip: &str,
        locked: bool,
        pressed: bool,
    ) -> IconHit {
        let hit = self.icon(ui, index, tip, locked, pressed);
        self.x += 4.0;
        hit
    }

    /// 紧挨段尾按钮的分隔线：段尾按钮已经多走了 4px，这里只 `draw_separator(xx) xx += 4`。
    fn separator(&mut self, ui: &Ui) {
        Bank::separator(ui.painter(), pos2(self.x - 1.0, self.y + 3.0), &self.th);
        self.x += 4.0;
    }

    /// 原版写成 `xx += 4 draw_separator(xx, yy + 3) xx += 4` 的那种（前面没有 +4 的段尾）。
    fn separator_gap(&mut self, ui: &Ui) {
        self.x += 4.0;
        self.separator(ui);
    }
}

pub fn show_menu_bar(state: &mut EditorState, ui: &mut Ui) {
    egui::menu::bar(ui, |ui| {
        // 标签和条目名照原版 control_draw.gml 的 `if (language != 1)` 英文分支；
        // 移植版自己加的条目（PNG 谱面 / 自动备份 / 清空所有音符）没有
        // 原版文案，英文按字面直译。
        ui.menu_button(tr("File", "文件"), |ui| {
            if ui.button(tr("New song  Ctrl+N", "新建  Ctrl+N")).clicked() {
                state.emit(Command::New);
                ui.close_menu();
            }
            if ui
                .button(tr("Open song...  Ctrl+O", "打开  Ctrl+O"))
                .clicked()
            {
                state.emit(Command::Open);
                ui.close_menu();
            }
            if ui.button(tr("Save song  Ctrl+S", "保存  Ctrl+S")).clicked() {
                state.emit(Command::Save);
                ui.close_menu();
            }
            if ui
                .button(tr("Save song as a new file...", "另存为"))
                .clicked()
            {
                state.emit(Command::SaveAs);
                ui.close_menu();
            }
            ui.separator();
            ui.menu_button(tr("Recent songs...", "最近打开"), |ui| {
                if state.recent_files.is_empty() {
                    ui.label(tr("No recent songs", "（无）"));
                }
                let recent = state.recent_files.clone();
                for path in recent {
                    let text = path.path.to_string_lossy().into_owned();
                    if ui.button(&text).clicked() {
                        state.emit(Command::OpenPath(path.path));
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            ui.menu_button(tr("Import...", "导入"), |ui| {
                if ui.button(tr("MIDI...", "从 MIDI 导入…")).clicked() {
                    state.emit(Command::OpenDialog(crate::state::DialogAction::ImportMidi));
                    ui.close_menu();
                }
                if ui
                    .button(tr("Schematic...", "从 Schematic 导入…"))
                    .clicked()
                {
                    state.emit(Command::OpenDialog(
                        crate::state::DialogAction::ImportSchematic,
                    ));
                    ui.close_menu();
                }
                if ui.button(tr("Pattern...", "导入片段 (.nbp)…")).clicked() {
                    state.emit(Command::OpenDialog(
                        crate::state::DialogAction::ImportPattern,
                    ));
                    ui.close_menu();
                }
            });
            ui.menu_button(tr("Export...", "导出"), |ui| {
                if ui.button(tr("Audio track...", "音频轨道…")).clicked() {
                    state.emit(Command::ShowWindow(WindowKind::AudioExport));
                    ui.close_menu();
                }
                if ui.button(tr("MIDI...", "MIDI…")).clicked() {
                    state.emit(Command::OpenDialog(crate::state::DialogAction::ExportMidi));
                    ui.close_menu();
                }
                if ui.button(tr("PNG sheet...", "PNG 谱面…")).clicked() {
                    state.emit(Command::ShowWindow(WindowKind::PngExport));
                    ui.close_menu();
                }
                if ui.button(tr("Schematic...", "Minecraft 原理图…")).clicked() {
                    state.emit(Command::OpenSchematicExport);
                    ui.close_menu();
                }
                if ui.button(tr("Data pack...", "Minecraft 数据包…")).clicked() {
                    state.emit(Command::ShowWindow(WindowKind::DatapackExport));
                    ui.close_menu();
                }
                if ui.button(tr("Pattern...", "片段 (.nbp)…")).clicked() {
                    state.emit(Command::OpenDialog(
                        crate::state::DialogAction::ExportPattern,
                    ));
                    ui.close_menu();
                }
            });
            if ui.button(tr("Save options...", "保存选项…")).clicked() {
                state.emit(Command::OpenSaveOptions);
                ui.close_menu();
            }
            if ui
                .button(tr("Restore unsaved files...", "自动保存 / 备份…"))
                .clicked()
            {
                state.emit(Command::ShowWindow(WindowKind::Autosaves));
                ui.close_menu();
            }
            ui.separator();
            if ui.button(tr("Clear all notes", "清空所有音符")).clicked() {
                state.emit(Command::ClearSong);
                ui.close_menu();
            }
        });

        ui.menu_button(tr("Edit", "编辑"), |ui| {
            let undo = state
                .doc
                .history
                .undo_label()
                .unwrap_or(tr("Undo", "撤销"))
                .to_string();
            let redo = state
                .doc
                .history
                .redo_label()
                .unwrap_or(tr("Redo", "重做"))
                .to_string();
            // 动作名出自 nbs-core 的历史记录（那边还没接入多语言），固定前缀走 tr。
            if ui
                .button(format!("{}: {undo}  Ctrl+Z", tr("Undo", "撤销")))
                .clicked()
            {
                state.emit(Command::Undo);
                ui.close_menu();
            }
            if ui
                .button(format!("{}: {redo}  Ctrl+Y", tr("Redo", "重做")))
                .clicked()
            {
                state.emit(Command::Redo);
                ui.close_menu();
            }
            ui.separator();
            if ui.button(tr("Copy  Ctrl+C", "复制  Ctrl+C")).clicked() {
                state.emit(Command::Copy);
                ui.close_menu();
            }
            if ui.button(tr("Cut  Ctrl+X", "剪切  Ctrl+X")).clicked() {
                state.emit(Command::Cut);
                ui.close_menu();
            }
            if ui.button(tr("Paste  Ctrl+V", "粘贴  Ctrl+V")).clicked() {
                state.emit(Command::Paste);
                ui.close_menu();
            }
            if ui.button(tr("Delete  Delete", "删除  Delete")).clicked() {
                state.emit(Command::Delete);
                ui.close_menu();
            }
            ui.separator();
            if ui
                .button(tr("Select all  Ctrl+A", "全选  Ctrl+A"))
                .clicked()
            {
                state.emit(Command::SelectAll);
                ui.close_menu();
            }
            if ui.button(tr("Deselect all", "取消选择")).clicked() {
                state.emit(Command::Deselect);
                ui.close_menu();
            }
            if ui
                .button(tr("Invert selection  Ctrl+I", "反选  Ctrl+I"))
                .clicked()
            {
                state.emit(Command::InvertSelection);
                ui.close_menu();
            }
            ui.separator();
            // 原版右键菜单里模式操作（升/降调）和「更改音色」「Macros...」都带
            // `inactive(current_song.selected = 0)`：没有选区就置灰。宏条目本身不再
            // 判空（宏窗口和宏本体各自在 `selected = 0` 时早退）。
            // 原版这里没有「移调…」窗口，也没有「批量设置」二级菜单——三个
            // Set velocity / panning / pitch... 都在 `Macros...` 子菜单里。
            let has_sel = !state.doc.selection.is_empty();
            let (up_oct, down_oct) = match state.tool {
                crate::state::EditMode::Key => (
                    tr("Transpose one octave up", "升八度"),
                    tr("Transpose one octave down", "降八度"),
                ),
                _ => (tr("Increase by 10", "升 10"), tr("Decrease by 10", "降 10")),
            };
            if ui
                .add_enabled(has_sel, egui::Button::new(format!("{up_oct} +12")))
                .clicked()
            {
                state.emit(Command::Transpose(12));
                ui.close_menu();
            }
            if ui
                .add_enabled(has_sel, egui::Button::new(format!("{down_oct} -12")))
                .clicked()
            {
                state.emit(Command::Transpose(-12));
                ui.close_menu();
            }
            ui.separator();
            instrument_menus(state, ui);
            ui.separator();
            // 原版 Edit 菜单的 `Macros...`（中文界面父项原文就写作「快捷键......」）
            // 是一条子菜单，条目见 `macros_menu_items`。父项在原版里带
            // `inactive(selected = 0 || selection_l = 0)`，也就是没有选区整条置灰；
            // 子项本身不再判空（宏窗口/宏本体各自早退）。
            // 点击走 menu_click.gml:115-129：这一族只开窗 / 只跑宏，
            // 不像图标条和 Ctrl+Shift 热键那样先停下播放。
            ui.add_enabled_ui(has_sel, |ui| {
                ui.menu_button(tr("Macros...", "快捷键......"), |ui| {
                    for (text, zh, key, cmd) in macros_menu_items() {
                        // 原版把热键右对齐单独画在行尾（`get_hotkey` 的返回串），
                        // egui 的菜单项没有第二列，这里按「文案 + 两个空格 + 显示串」拼成一条。
                        let label = format!("{}  Ctrl+Shift+{key}", tr(text, zh.unwrap_or(text)));
                        if ui.button(label).clicked() {
                            state.emit(cmd);
                        }
                    }
                });
            });
        });

        // 原版「设置」标签：音色列表 + 音色设置 / 歌曲信息 / 属性。显示选项与主题
        // 在原版里属于首选项窗口，这里也不再单列「视图」菜单。
        ui.menu_button(tr("Settings", "设置"), |ui| {
            let names = state.instrument_list();
            ui.menu_button(tr("Instrument", "音色"), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(340.0)
                    .show(ui, |ui| {
                        for (idx, name) in &names {
                            if ui
                                .selectable_label(state.current.instrument == *idx, name)
                                .clicked()
                            {
                                state.emit(Command::SetInstrument(*idx));
                            }
                        }
                    });
            });
            ui.separator();
            if ui
                .button(tr("Instrument settings...", "音色设置…"))
                .clicked()
            {
                state.emit(Command::ShowWindow(WindowKind::Instruments));
                ui.close_menu();
            }
            if ui.button(tr("Song info...", "歌曲信息…")).clicked() {
                state.emit(Command::ShowWindow(WindowKind::SongInfo));
                ui.close_menu();
            }
            if ui.button(tr("Song properties...", "歌曲属性…")).clicked() {
                state.emit(Command::ShowWindow(WindowKind::Properties));
                ui.close_menu();
            }
            if ui.button(tr("Song stats...", "歌曲统计…")).clicked() {
                state.emit(Command::ShowWindow(WindowKind::Statistics));
                ui.close_menu();
            }
            ui.separator();
            if ui
                .button(tr("Audio & sample settings...", "音频与采样设置…"))
                .clicked()
            {
                state.emit(Command::ShowWindow(WindowKind::AudioSettings));
                ui.close_menu();
            }
            if ui
                .button(tr("Preferences...  Ctrl+P", "首选项…  Ctrl+P"))
                .clicked()
            {
                state.emit(Command::ShowWindow(WindowKind::Preferences));
                ui.close_menu();
            }
        });

        ui.menu_button(tr("Help", "帮助"), |ui| {
            if ui
                .button(tr("Shortcut list  F1", "快捷键一览  F1"))
                .clicked()
            {
                state.emit(Command::ShowWindow(WindowKind::Shortcuts));
                ui.close_menu();
            }
            // 原版帮助菜单倒数第二条（`control_draw.gml:1783/1878` 的
            // `Changelist...` / 「更新历史......」），点的就是 `window = w_changelist`。
            if ui.button(tr("Changelist...", "更新历史…")).clicked() {
                state.emit(Command::ShowWindow(WindowKind::Changelist));
                ui.close_menu();
            }
            if ui.button(tr("About...", "关于…")).clicked() {
                state.emit(Command::ShowWindow(WindowKind::About));
                ui.close_menu();
            }
        });
    });
}

/// 原版 Edit → `Macros...` 子菜单的 15 条，顺序与文案逐条照抄
/// `control_draw.gml:562-577`（中文分支 :600-615：前 8 条宏名连中文界面都保持英文，
/// 所以中文文案留 `None`，后 7 条才给中文）。第三个字段是原版 `get_hotkey.gml:73-146`
/// 里这个宏的按键字母，画在行尾。
///
/// 前 8 条里 Vibrato/Chorus/Volume LFO 在原版里直接执行宏、其余开宏窗口；
/// 后 7 条按原版：淡入/淡出/替换音/重置所有属性直接执行，三个「设定…」开批量窗口。
/// 单独抽成表是为了能在没有鼠标的用例里核对「这条文案派发的是哪个命令」。
pub(crate) fn macros_menu_items() -> [(&'static str, Option<&'static str>, char, Command); 15] {
    use nbs_core::Macro as Mac;
    let open = Command::ShowWindow;
    let run = Command::RunMacro;
    // 这四条的命令先绑成局部值：整条写进表格那一格，这一行就超出 rustfmt 肯留在
    // 一行里的宽度，15 行的表会被拆成竖排，看不出顺序。
    let velocity = open(WindowKind::SetVelocity);
    let panning = open(WindowKind::SetPanning);
    let pitch = open(WindowKind::SetPitch);
    let reset = run(Mac::Reset);
    // (菜单文案, 中文界面改用这条, get_hotkey 的字母, 派发的命令)
    [
        ("Tremolo...", None, 'A', open(WindowKind::Tremolo)),
        ("Stereo...", None, 'S', open(WindowKind::Stereo)),
        ("Arpeggio...", None, 'D', open(WindowKind::Arpeggio)),
        ("Portamento...", None, 'F', open(WindowKind::Portamento)),
        ("Vibrato", None, 'G', run(Mac::Vibrato)),
        ("Stagger...", None, 'H', open(WindowKind::Stagger)),
        ("Chorus", None, 'J', run(Mac::Chorus { detune: 5 })),
        ("Volume LFO", None, 'K', run(Mac::VelocityLfo)),
        ("Fade in", Some("淡入"), 'Q', run(Mac::FadeIn)),
        ("Fade out", Some("淡出"), 'W', run(Mac::FadeOut)),
        ("Replace key", Some("替换音"), 'E', run(Mac::ReplaceKey)),
        ("Set velocity...", Some("设定音量......"), 'R', velocity),
        ("Set panning...", Some("设定声道......"), 'T', panning),
        ("Set pitch...", Some("设定音高......"), 'Y', pitch),
        ("Reset all properties", Some("重置所有属性"), 'U', reset),
    ]
}

/// 原版在 Edit 菜单和网格右键菜单里都出现的三段乐器列表：
/// 按乐器选择 / 选择除该乐器之外的全部 / 把音符改成该乐器。
pub fn instrument_menus(state: &mut EditorState, ui: &mut Ui) {
    let names = state.instrument_list();
    ui.menu_button(tr("Select all of instrument", "按乐器选择"), |ui| {
        egui::ScrollArea::vertical()
            .max_height(340.0)
            .show(ui, |ui| {
                for (idx, name) in &names {
                    if ui.button(format!("{} {name}", tr("Only", "仅"))).clicked() {
                        state.emit(Command::SelectByInstrument {
                            instrument: *idx,
                            invert: false,
                        });
                    }
                }
            });
    });
    ui.menu_button(tr("Select all but instrument", "选择除乐器…"), |ui| {
        egui::ScrollArea::vertical()
            .max_height(340.0)
            .show(ui, |ui| {
                for (idx, name) in &names {
                    if ui
                        .button(format!("{} {name}", tr("All but", "除…外")))
                        .clicked()
                    {
                        state.emit(Command::SelectByInstrument {
                            instrument: *idx,
                            invert: true,
                        });
                    }
                }
            });
    });
    // 原版「更改音色......」在无选区时置灰（`inactive(current_song.selected = 0)`）。
    let has_sel = !state.doc.selection.is_empty();
    ui.add_enabled_ui(has_sel, |ui| {
        ui.menu_button(tr("Change instrument to...", "更改乐器到…"), |ui| {
            egui::ScrollArea::vertical()
                .max_height(340.0)
                .show(ui, |ui| {
                    for (idx, name) in &names {
                        if ui.button(name).clicked() {
                            state.emit(Command::SetAllInstrument(*idx));
                        }
                    }
                });
        });
    });
}

pub fn show_toolbar(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let area = ui.max_rect();
    ui.painter().rect_filled(area, 0.0, th.panel);
    let Some(bank) = state.icons.bank() else {
        show_toolbar_text(state, ui);
        return;
    };
    // 原版 `if (theme != 3) draw_sprite(spr_iconbar, 0/1/2, …)`：经典主题用端帽 +
    // 横向拉伸的条做背景，Fluent 主题不画（露出面板底色）。
    // 条本身只有 30 高，面板下方 4px 是原版留给窗口底色的空带，不能被拉伸填满。
    if th.kind != ThemeKind::Fluent {
        let strip = Rect::from_min_size(
            area.min,
            Vec2::new(area.width(), crate::viewport::TOOLBAR_STRIP_HEIGHT),
        );
        bank.icon_strip_bg(ui.painter(), strip);
    }
    let rw = area.width();
    let y = area.min.y + 3.0;
    let compat = state.compatibility();
    let mut bar = Bar {
        bank,
        th,
        x: area.min.x + 6.0,
        y,
    };

    sequence(state, ui, &mut bar, rw);
    if let Some(v) = master_bar(state, ui, &mut bar, area) {
        state.emit(Command::SetMasterVolume(v));
    }
    compat_cluster(state, ui, &bar, compat, area);
}

/// 原版 `draw_icon` 序列。`state` / `ui` / `bar` 分开传，避免闭包借用打架。
fn sequence(state: &mut EditorState, ui: &mut Ui, bar: &mut Bar, rw: f32) {
    macro_rules! draw {
        ($index:expr, $tip:expr, $locked:expr, $pressed:expr) => {{
            let hit = bar.icon(ui, $index, $tip, $locked, $pressed);
            if bar.overflows(rw) {
                return;
            }
            hit
        }};
        // 段尾（原版 `xx += 25 + 4`）
        ($index:expr, $tip:expr, $locked:expr, $pressed:expr, end) => {{
            let hit = bar.icon_end(ui, $index, $tip, $locked, $pressed);
            if bar.overflows(rw) {
                return;
            }
            hit
        }};
    }

    let playing = state.is_playing();
    let loop_on = state.doc.song.meta.loop_enabled;
    let metronome_on = state.prefs.metronome;
    let marker = state.marker();
    let tool = state.tool;
    let can_undo = state.doc.history.can_undo();
    let can_redo = state.doc.history.can_redo();
    let no_selection = state.doc.selection.notes.is_empty();
    let no_clip = state.doc.clipboard_text.is_empty();

    if draw!(frame::NEW, tr("New song", "新文件"), false, false).clicked {
        state.emit(Command::New);
    }
    if draw!(frame::OPEN, tr("Open song", "打开歌曲"), false, false).clicked {
        state.emit(Command::Open);
    }
    if draw!(frame::SAVE, tr("Save song", "保存歌曲"), false, false, end).clicked {
        state.emit(Command::Save);
    }
    bar.separator(ui);

    let play_icon = if playing { frame::PAUSE } else { frame::PLAY };
    if draw!(
        play_icon,
        tr("Play / Pause song", "播放 / 暂停"),
        false,
        false
    )
    .clicked
    {
        state.emit(Command::PlayPause);
    }
    if draw!(frame::STOP, tr("Stop song", "停止歌曲"), false, false).clicked {
        state.emit(Command::Stop);
    }
    // 原版的快退 / 快进返回「按住」，所以是连续滚动而不是单击。
    let back = draw!(frame::BACK, tr("Rewind song", "快退"), false, false);
    let fwd = draw!(
        frame::FORWARD,
        tr("Fast-forward song", "快进"),
        false,
        false
    );
    // 原版 790-791 行：按住 ← / → 就是 `forward = ∓1`，和这两个按钮共用同一个变量。
    // 同样受「窗口开着 / 光标在输入框」闸门约束：那时 ←/→ 归窗口翻页签使用。
    let busy = state.window.is_some() || ui.memory(|m| m.focused().is_some());
    let (kbd_back, kbd_fwd) = if busy {
        (false, false)
    } else {
        ui.input(|i| {
            (
                i.key_down(egui::Key::ArrowLeft),
                i.key_down(egui::Key::ArrowRight),
            )
        })
    };
    state.scrub = if back.held || kbd_back {
        -1
    } else if fwd.held || kbd_fwd {
        1
    } else {
        0
    };
    if draw!(
        frame::RECORD,
        tr("Record key presses", "录制按键"),
        false,
        (playing || state.record_armed) && state.record
    )
    .clicked
    {
        state.emit(Command::ToggleRecord);
    }
    if draw!(
        frame::LOOP_INACTIVE + loop_on as usize,
        tr("Toggle looping", "开关循环"),
        false,
        loop_on
    )
    .clicked
    {
        state.emit(Command::ToggleLoop);
    }
    // 原版：节拍器响的那一拍用 METRONOME_1，其余用 METRONOME_2。
    let metricon = if !metronome_on {
        frame::METRONOME_INACTIVE
    } else if !playing || (marker.floor() as i64 - 1).rem_euclid(8) == 0 {
        frame::METRONOME_1
    } else {
        frame::METRONOME_2
    };
    if draw!(
        metricon,
        tr("Toggle metronome", "开关节拍器"),
        false,
        false,
        end
    )
    .clicked
    {
        state.emit(Command::ToggleMetronome);
    }
    bar.separator(ui);

    for (i, (icon, tip, mode)) in [
        (
            frame::EDITMODE_KEY,
            tr("Edit note key", "音调模式"),
            EditMode::Key,
        ),
        (
            frame::EDITMODE_VEL,
            tr("Edit note velocity", "音量模式"),
            EditMode::Velocity,
        ),
        (
            frame::EDITMODE_PAN,
            tr("Edit note panning", "声道模式"),
            EditMode::Panning,
        ),
        (
            frame::EDITMODE_PIT,
            tr("Edit note pitch", "音高模式"),
            EditMode::Pitch,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let pressed = tool == mode;
        if i == 3 {
            if draw!(icon, tip, false, pressed, end).clicked {
                state.emit(Command::SetEditMode(mode));
            }
        } else if draw!(icon, tip, false, pressed).clicked {
            state.emit(Command::SetEditMode(mode));
        }
    }
    bar.separator(ui);

    instrument_box(state, ui, bar, rw);
    bar.separator_gap(ui);

    if draw!(
        frame::UNDO,
        tr("Undo the last change", "撤销"),
        !can_undo,
        false
    )
    .clicked
    {
        state.emit(Command::Undo);
    }
    if draw!(
        frame::REDO,
        tr("Redo the last undo", "重做"),
        !can_redo,
        false
    )
    .clicked
    {
        state.emit(Command::Redo);
    }
    if draw!(
        frame::COPY,
        tr("Copy the selected note blocks", "复制"),
        no_selection,
        false
    )
    .clicked
    {
        state.emit(Command::Copy);
    }
    if draw!(
        frame::CUT,
        tr("Cut the selected note blocks", "剪切"),
        no_selection,
        false
    )
    .clicked
    {
        state.emit(Command::Cut);
    }
    if draw!(
        frame::PASTE,
        tr("Paste the copied note blocks", "粘贴"),
        no_clip,
        false
    )
    .clicked
    {
        state.emit(Command::Paste);
    }
    if draw!(
        frame::DELETE,
        tr("Delete the selected note blocks", "删除"),
        no_selection,
        false
    )
    .clicked
    {
        state.emit(Command::Delete);
    }
    bar.separator_gap(ui);

    if draw!(
        frame::INFORMATION,
        tr("View song info", "歌曲信息"),
        false,
        false
    )
    .clicked
    {
        state.emit(Command::ShowWindow(WindowKind::SongInfo));
    }
    if draw!(
        frame::PROPERTIES,
        tr("Edit song properties", "歌曲属性"),
        false,
        false
    )
    .clicked
    {
        state.emit(Command::ShowWindow(WindowKind::Properties));
    }
    if draw!(
        frame::INSTRUMENTS,
        tr("Edit instruments", "音色设置"),
        false,
        false
    )
    .clicked
    {
        state.emit(Command::ShowWindow(WindowKind::Instruments));
    }
    bar.separator_gap(ui);

    if draw!(
        frame::HELP,
        tr("Watch tutorial videos", "教程视频"),
        false,
        false
    )
    .clicked
    {
        open_link(ui, "https://www.bilibili.com/video/BV1Mx411a76p");
    }
    if draw!(
        frame::INTERNET,
        tr(
            "Visit the Note Block Studio website",
            "访问 Note Block Studio 官方网站"
        ),
        false,
        false
    )
    .clicked
    {
        open_link(ui, "https://opennbs.org/");
    }
}

fn open_link(ui: &Ui, url: &str) {
    ui.ctx().open_url(OpenUrl {
        url: url.into(),
        new_tab: true,
    });
}

/// 原版 `draw_icon_insbox` 的两种形态：收起时是图标条里的一行，
/// 展开时是盖住网格的窗口网格（点外面收回）。
fn instrument_box(state: &mut EditorState, ui: &mut Ui, bar: &mut Bar, rw: f32) {
    let ins = state.instrument_list();
    let count = ins.len();
    let mut ins_icons = (((rw - 920.0) / 25.0).ceil() as usize).clamp(5, count);
    if ins_icons + 1 == count {
        ins_icons += 1;
    }
    let ins_rows = count.div_ceil(ins_icons.max(1));
    let bank = Arc::clone(&bar.bank);
    let th = bar.th.clone();
    let first_custom = nbs_core::model::FIRST_CUSTOM_INDEX;
    if state.insbox_open {
        let at = pos2(bar.x, bar.y);
        draw_insbox_open(state, ui, &bank, &th, &ins, at, ins_icons, ins_rows);
        bar.x += ins_icons as f32 * 25.0;
        if bank.action_icon(
            ui,
            pos2(bar.x, bar.y),
            frame::INSBOX_COLLAPSE,
            tr("Less instruments", "收回音色"),
            false,
            &th,
        ) {
            state.insbox_open = false;
        }
        bar.x += 25.0;
        return;
    }
    // 保证当前音色落在这一行里（原版 `insbox_start`）。
    let cur = ins
        .iter()
        .position(|(i, _)| *i == state.current.instrument)
        .unwrap_or(0);
    if cur < state.insbox_start {
        state.insbox_start = state.insbox_start.saturating_sub(ins_icons) / ins_icons * ins_icons;
    } else if cur > state.insbox_start + ins_icons - 1 {
        state.insbox_start = (state.insbox_start + ins_icons) / ins_icons * ins_icons;
    }
    state.insbox_start = state.insbox_start.clamp(0, count.saturating_sub(ins_icons));
    for k in 0..ins_icons {
        let i = state.insbox_start + k;
        if i >= count {
            break;
        }
        let tip = format!("{}{}", tr("Change instrument to ", "更改音色为"), ins[i].1);
        let pressed = state.current.instrument == ins[i].0;
        if bank.instrument_icon(
            ui,
            pos2(bar.x, bar.y),
            i,
            first_custom,
            &ins[i].1,
            &tip,
            pressed,
            &th,
        ) {
            state.emit(Command::SetInstrument(ins[i].0));
        }
        bar.x += 25.0;
    }
    if ins_icons < count {
        if bank.action_icon(
            ui,
            pos2(bar.x, bar.y),
            frame::INSBOX_EXPAND,
            tr("More instruments...", "展开音色......"),
            false,
            &th,
        ) {
            state.insbox_open = true;
        }
        // 原版：只有画了展开按钮才走这 25px。
        bar.x += 25.0;
    }
}

/// 展开态：`draw_window` + rows×cols 网格，然后「点网格外面收回」
/// （原版顺序也是先画格子、最后判抬手）。
fn draw_insbox_open(
    state: &mut EditorState,
    ui: &Ui,
    bank: &Bank,
    th: &Theme,
    ins: &[(u8, String)],
    at: Pos2,
    cols: usize,
    rows: usize,
) {
    let rect = Rect::from_min_size(
        at - Vec2::new(2.0, 2.0),
        Vec2::new(cols as f32 * 25.0 + 4.0, rows as f32 * 25.0 + 4.0),
    );
    let ctx = ui.ctx();
    // 原版 `window = w_insbox`：展开期间网格不接输入，点哪儿都只用来收回。
    let clicked_away = egui::Area::new(Id::new("insbox_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::ZERO)
        .show(ctx, |ui| {
            ui.allocate_rect(ctx.screen_rect(), Sense::click())
                .clicked()
        })
        .inner;
    let first_custom = nbs_core::model::FIRST_CUSTOM_INDEX;
    let mut chosen: Option<usize> = None;
    egui::Area::new(Id::new("insbox_grid"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.painter().rect_filled(rect, 2.0, th.panel);
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0_f32, Color32::from_gray(128)),
                egui::StrokeKind::Outside,
            );
            for a in 0..rows {
                for b in 0..cols {
                    let i = a * cols + b;
                    if i >= ins.len() {
                        break;
                    }
                    let pos = rect.min + Vec2::new(2.0 + b as f32 * 25.0, 2.0 + a as f32 * 25.0);
                    let tip = format!("{}{}", tr("Change instrument to ", "更改音色为"), ins[i].1);
                    let pressed = state.current.instrument == ins[i].0;
                    if bank.instrument_icon(ui, pos, i, first_custom, &ins[i].1, &tip, pressed, th)
                    {
                        chosen = Some(i);
                    }
                }
            }
        });
    if let Some(i) = chosen {
        // 原版：改音色后把收起行的首格挪到这一组（`insbox_start`）。
        state.emit(Command::SetInstrument(ins[i].0));
        state.insbox_start = (i / cols * cols).min(ins.len().saturating_sub(1));
        state.insbox_open = false;
    } else if clicked_away {
        state.insbox_open = false;
    }
}

/// 原版 `draw_dragbar(mastervol, 1, xx, yy + 10, 100, 2, …)`，外加滚轮 ±0.02。
/// 返回新的音量（由调用方发命令）。
fn master_bar(state: &EditorState, ui: &mut Ui, bar: &mut Bar, area: Rect) -> Option<f32> {
    bar.x += 8.0;
    let value = state.prefs.master_volume.clamp(0.0, 1.0);
    let x = bar.x;
    let y = bar.y + 10.0;
    let len = (area.max.x - 210.0 - x).clamp(0.0, 100.0);
    let mut out = None;
    if len >= 20.0 {
        let th = &bar.th;
        let p = ui.painter();
        let fluent = th.kind == ThemeKind::Fluent;
        let end = x + value * len;
        let h = if fluent { 3.0 } else { 2.0 };
        p.rect_filled(
            Rect::from_min_size(pos2(x, y), Vec2::new(len, h)),
            0.0,
            if fluent {
                Color32::from_gray(154)
            } else {
                th.text
            },
        );
        if fluent {
            p.rect_filled(
                Rect::from_min_size(pos2(x, y), Vec2::new((end - x).max(0.0), h)),
                0.0,
                th.accent,
            );
        }
        let knob = pos2(end, y + 1.0);
        if fluent {
            p.circle_filled(knob, 10.0, Color32::WHITE);
            p.circle_filled(knob, 5.0, th.accent);
        } else {
            // 经典主题的手柄就是 `spr_icons` 的第 9 帧。
            bar.bank
                .icon_glyph(p, knob - Vec2::new(12.0, 11.0), frame::DRAG_KNOB);
        }
        let track = Rect::from_min_size(pos2(x - 3.0, y - 2.0), Vec2::new(len + 6.0, 6.0));
        let hot = Rect::from_center_size(knob, Vec2::splat(13.0));
        let resp = ui.interact(
            track.union(hot),
            Id::new("master_volume"),
            Sense::click_and_drag(),
        );
        if resp.dragged() || resp.clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                out = Some(((pt.x - x) / len).clamp(0.0, 1.0));
            }
        }
        if resp.hovered() {
            let wheel = ui.input(|i| i.raw_scroll_delta.y);
            if wheel != 0.0 {
                out = Some((value + wheel.signum() * 0.02).clamp(0.0, 1.0));
            }
        }
        resp.on_hover_text(format!(
            "{}{}",
            tr("Master Volume: ", "主音量："),
            (value * 100.0).floor()
        ));
    }
    out
}

/// 原版右上角：分隔线 + 两枚 `spr_minecraft` + 右对齐文字；可点区域宽 160。
fn compat_cluster(state: &mut EditorState, ui: &mut Ui, bar: &Bar, compat: u8, area: Rect) {
    let rw = area.max.x;
    let p = ui.painter();
    Bank::separator(p, pos2(rw - 35.0, area.min.y + 6.0), &bar.th);
    // 原版三档：c_green / c_orange / c_red，其中绿色在深色底（主题 2、Fluent fdark）换成 c_lime。
    let lime = bar.th.kind == ThemeKind::Dark || (bar.th.kind == ThemeKind::Fluent && bar.th.fdark);
    let (left, right, text, color) = match compat {
        1 => (
            0,
            0,
            tr("Fully compatible", "完全兼容"),
            if lime {
                Color32::from_rgb(0, 255, 0)
            } else {
                Color32::from_rgb(0, 176, 0)
            },
        ),
        2 => (
            1,
            0,
            tr("Data pack only", "仅限数据包"),
            Color32::from_rgb(255, 140, 0),
        ),
        _ => (
            1,
            2,
            tr("Resource pack only", "仅限资源包"),
            Color32::from_rgb(255, 0, 0),
        ),
    };
    bar.bank
        .compat_icon(p, pos2(rw - 59.0, area.min.y + 5.0), left, &bar.th);
    bar.bank
        .compat_icon(p, pos2(rw - 30.0, area.min.y + 5.0), right, &bar.th);
    p.text(
        pos2(rw - 65.0, area.min.y + 15.0),
        Align2::RIGHT_CENTER,
        text,
        FontId::proportional(12.0),
        color,
    );
    let hot = Rect::from_min_size(pos2(rw - 160.0, area.min.y + 4.0), Vec2::new(160.0, 25.0));
    // 原版 `popup_set` 三句中文文案（control_draw.gml:1976/2000/2024），第二行就是提示可点。
    let tip = match compat {
        1 => tr(
            "This song is compatible with both schematics and data packs.\n(Click for more info.)",
            "此歌曲兼容 schematic 和数据包。\n（点击查看更多）",
        ),
        2 => tr(
            "This song is only compatible with data packs.\n(Click for more info.)",
            "此歌曲仅兼容数据包。\n（点击查看更多）",
        ),
        _ => tr(
            "This song is compatible with data packs using a resource pack.\n(Click for more info.)",
            "此歌曲仅兼容带资源包的数据包。\n（点击查看更多）",
        ),
    };
    // 原版 `if (a && window = 0)`：已经有窗口开着时这颗不响应，光标也不变手型。
    let clickable = state.window.is_none();
    let resp = ui
        .interact(
            hot,
            Id::new("compat_badge"),
            if clickable {
                Sense::click()
            } else {
                Sense::hover()
            },
        )
        .on_hover_text(tip);
    // 原版命中时 `curs = cr_handpoint`。
    let resp = if clickable {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    if clickable && resp.clicked() {
        state.emit(Command::ShowCompat);
    }
}

/// 精灵图缺失时的兜底：顺序和功能完全一样，只是画成文字按钮。
fn show_toolbar_text(state: &mut EditorState, ui: &mut Ui) {
    egui::menu::bar(ui, |ui| {
        if ui.button(tr("New song", "新文件")).clicked() {
            state.emit(Command::New);
        }
        if ui.button(tr("Open song", "打开歌曲")).clicked() {
            state.emit(Command::Open);
        }
        if ui.button(tr("Save song", "保存歌曲")).clicked() {
            state.emit(Command::Save);
        }
        ui.separator();
        let playing = state.is_playing();
        if ui
            .button(if playing {
                tr("Pause", "暂停")
            } else {
                tr("Play", "播放")
            })
            .clicked()
        {
            state.emit(Command::PlayPause);
        }
        if ui.button(tr("Stop", "停止")).clicked() {
            state.emit(Command::Stop);
        }
        let loop_on = state.doc.song.meta.loop_enabled;
        if ui.selectable_label(loop_on, tr("Loop", "循环")).clicked() {
            state.emit(Command::ToggleLoop);
        }
        if ui
            .selectable_label(state.prefs.metronome, tr("Metronome", "节拍器"))
            .clicked()
        {
            state.emit(Command::ToggleMetronome);
        }
        ui.separator();
        for mode in EditMode::ALL {
            if ui
                .selectable_label(state.tool == mode, mode.name())
                .clicked()
            {
                state.emit(Command::SetEditMode(mode));
            }
        }
        ui.separator();
        let names = state.instrument_list();
        let current = state.current.instrument;
        egui::ComboBox::from_id_salt("ins_text")
            .selected_text(
                names
                    .iter()
                    .find(|(i, _)| *i == current)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_default(),
            )
            .show_ui(ui, |ui| {
                for (idx, name) in &names {
                    if ui
                        .selectable_value(&mut state.current.instrument, *idx, name)
                        .changed()
                    {
                        state.emit(Command::SetInstrument(*idx));
                    }
                }
            });
        ui.separator();
        if ui.button(tr("Undo", "撤销")).clicked() {
            state.emit(Command::Undo);
        }
        if ui.button(tr("Redo", "重做")).clicked() {
            state.emit(Command::Redo);
        }
        if ui.button(tr("Copy", "复制")).clicked() {
            state.emit(Command::Copy);
        }
        if ui.button(tr("Cut", "剪切")).clicked() {
            state.emit(Command::Cut);
        }
        if ui.button(tr("Paste", "粘贴")).clicked() {
            state.emit(Command::Paste);
        }
        if ui.button(tr("Delete", "删除")).clicked() {
            state.emit(Command::Delete);
        }
        ui.separator();
        if ui.button(tr("Song info", "歌曲信息")).clicked() {
            state.emit(Command::ShowWindow(WindowKind::SongInfo));
        }
        if ui.button(tr("Song properties", "歌曲属性")).clicked() {
            state.emit(Command::ShowWindow(WindowKind::Properties));
        }
        if ui.button(tr("Instruments", "音色设置")).clicked() {
            state.emit(Command::ShowWindow(WindowKind::Instruments));
        }
        ui.separator();
        let mut vol = state.prefs.master_volume.clamp(0.0, 1.0);
        let w = (ui.available_width() - 60.0).clamp(60.0, 100.0);
        if ui
            .add_sized(
                Vec2::new(w, 18.0),
                egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false),
            )
            .changed()
        {
            state.emit(Command::SetMasterVolume(vol));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 子菜单的顺序、文案和「这条文案派发哪个命令」都是原版的硬规格，抄在
    /// `macros_menu_items` 里；这里逐条钉住，防止后面接线时错位或漏改。
    /// 第三个字段 `Ctrl+Shift+字母` 的顺序（A S D F G H J K Q W E R T Y U）来自
    /// `get_hotkey.gml:73-146`，`shortcuts.rs` 里另有对应用例验按键真的生效。
    /// 拼到屏幕上之后的整串文案，由 `lib.rs` 的
    /// `edit_menu_macros_submenu_lists_the_original_fifteen_items` 对着真实绘制结果验。
    #[test]
    fn macros_menu_table_follows_the_original_order_and_targets() {
        use nbs_core::Macro as Mac;
        // 英文文案：原版 control_draw.gml:562-577 的 15 行。
        let want_text = [
            "Tremolo...",
            "Stereo...",
            "Arpeggio...",
            "Portamento...",
            "Vibrato",
            "Stagger...",
            "Chorus",
            "Volume LFO",
            "Fade in",
            "Fade out",
            "Replace key",
            "Set velocity...",
            "Set panning...",
            "Set pitch...",
            "Reset all properties",
        ];
        // 中文文案：原版 :600-615 的分支，前 8 条连中文界面都照抄英文，故为 None。
        let want_zh: [Option<&str>; 15] = [
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("淡入"),
            Some("淡出"),
            Some("替换音"),
            Some("设定音量......"),
            Some("设定声道......"),
            Some("设定音高......"),
            Some("重置所有属性"),
        ];
        let want_key = [
            'A', 'S', 'D', 'F', 'G', 'H', 'J', 'K', 'Q', 'W', 'E', 'R', 'T', 'Y', 'U',
        ];
        let want_cmd = [
            Command::ShowWindow(WindowKind::Tremolo),
            Command::ShowWindow(WindowKind::Stereo),
            Command::ShowWindow(WindowKind::Arpeggio),
            Command::ShowWindow(WindowKind::Portamento),
            Command::RunMacro(Mac::Vibrato),
            Command::ShowWindow(WindowKind::Stagger),
            Command::RunMacro(Mac::Chorus { detune: 5 }),
            Command::RunMacro(Mac::VelocityLfo),
            Command::RunMacro(Mac::FadeIn),
            Command::RunMacro(Mac::FadeOut),
            Command::RunMacro(Mac::ReplaceKey),
            Command::ShowWindow(WindowKind::SetVelocity),
            Command::ShowWindow(WindowKind::SetPanning),
            Command::ShowWindow(WindowKind::SetPitch),
            Command::RunMacro(Mac::Reset),
        ];
        for (i, (text, zh, key, cmd)) in macros_menu_items().iter().enumerate() {
            assert_eq!(*text, want_text[i], "第 {} 条英文文案", i + 1);
            assert_eq!(*zh, want_zh[i], "第 {} 条中文文案", i + 1);
            assert_eq!(*key, want_key[i], "第 {} 条热键字母", i + 1);
            assert_eq!(cmd, &want_cmd[i], "第 {} 条派发的命令", i + 1);
        }
    }
}
