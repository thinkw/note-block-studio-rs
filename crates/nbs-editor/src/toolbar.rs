//! 顶部菜单栏 + 图标工具栏（原版 4 个主菜单收敛成 egui 原生菜单）。

use egui::Ui;
use nbs_core::instruments::{instrument_name, INSTRUMENTS};

use crate::commands::{current_note_label, Command};
use crate::state::{DialogAction, EditMode, Flag, WindowKind};
use crate::theme::ThemeKind;
use crate::state::EditorState;

pub fn show_menu_bar(state: &mut EditorState, ui: &mut Ui) {
    egui::menu::bar(ui, |ui| {
        ui.menu_button("文件", |ui| {
            if ui.button("新建  Ctrl+N").clicked() { state.emit(Command::New); ui.close_menu(); }
            if ui.button("打开  Ctrl+O").clicked() { state.emit(Command::Open); ui.close_menu(); }
            if ui.button("保存  Ctrl+S").clicked() { state.emit(Command::Save); ui.close_menu(); }
            if ui.button("另存为").clicked() { state.emit(Command::SaveAs); ui.close_menu(); }
            ui.separator();
            ui.menu_button("最近打开", |ui| {
                if state.recent_files.is_empty() {
                    ui.label("（无）");
                }
                let recent = state.recent_files.clone();
                for path in recent {
                    let text = path.to_string_lossy().into_owned();
                    if ui.button(&text).clicked() {
                        state.emit(Command::OpenPath(path));
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            ui.menu_button("导入", |ui| {
                if ui.button("从 MIDI 导入…").clicked() { state.emit(Command::ShowWindow(WindowKind::MidiImport)); ui.close_menu(); }
                if ui.button("导入片段 (.nbp)…").clicked() { state.emit(Command::OpenDialog(DialogAction::ImportPattern)); ui.close_menu(); }
            });
            ui.menu_button("导出", |ui| {
                if ui.button("音频轨道…").clicked() { state.emit(Command::ShowWindow(WindowKind::AudioExport)); ui.close_menu(); }
                if ui.button("MIDI…").clicked() { state.emit(Command::OpenDialog(DialogAction::ExportMidi)); ui.close_menu(); }
                if ui.button("PNG 谱面…").clicked() { state.emit(Command::ShowWindow(WindowKind::PngExport)); ui.close_menu(); }
                if ui.button("Minecraft 原理图…").clicked() { state.emit(Command::ShowWindow(WindowKind::SchematicExport)); ui.close_menu(); }
                if ui.button("Minecraft 数据包…").clicked() { state.emit(Command::ShowWindow(WindowKind::DatapackExport)); ui.close_menu(); }
                if ui.button("片段 (.nbp)…").clicked() { state.emit(Command::OpenDialog(DialogAction::ExportPattern)); ui.close_menu(); }
            });
            if ui.button("自动保存 / 备份…").clicked() { state.emit(Command::ShowWindow(WindowKind::Autosaves)); ui.close_menu(); }
            ui.separator();
            if ui.button("清空所有音符").clicked() {
                state.emit(Command::ClearSong);
                ui.close_menu();
            }
        });

        ui.menu_button("编辑", |ui| {
            let undo = state.doc.history.undo_label().unwrap_or("撤销").to_string();
            let redo = state.doc.history.redo_label().unwrap_or("重做").to_string();
            if ui.button(format!("撤销: {undo}  Ctrl+Z")).clicked() { state.emit(Command::Undo); ui.close_menu(); }
            if ui.button(format!("重做: {redo}  Ctrl+Y")).clicked() { state.emit(Command::Redo); ui.close_menu(); }
            ui.separator();
            if ui.button("复制  Ctrl+C").clicked() { state.emit(Command::Copy); ui.close_menu(); }
            if ui.button("剪切  Ctrl+X").clicked() { state.emit(Command::Cut); ui.close_menu(); }
            if ui.button("粘贴  Ctrl+V").clicked() { state.emit(Command::Paste); ui.close_menu(); }
            if ui.button("删除  Delete").clicked() { state.emit(Command::Delete); ui.close_menu(); }
            ui.separator();
            if ui.button("全选  Ctrl+A").clicked() { state.emit(Command::SelectAll); ui.close_menu(); }
            if ui.button("取消选择").clicked() { state.emit(Command::Deselect); ui.close_menu(); }
            if ui.button("反选  Ctrl+I").clicked() { state.emit(Command::InvertSelection); ui.close_menu(); }
            ui.separator();
            if ui.button("升八度 +12").clicked() { state.emit(Command::Transpose(12)); ui.close_menu(); }
            if ui.button("降八度 -12").clicked() { state.emit(Command::Transpose(-12)); ui.close_menu(); }
            if ui.button("移调…").clicked() { state.emit(Command::ShowWindow(WindowKind::Transpose)); ui.close_menu(); }
            ui.menu_button("批量设置", |ui| {
                if ui.button("力度…").clicked() { state.emit(Command::ShowWindow(WindowKind::SetVelocity)); }
                if ui.button("声像…").clicked() { state.emit(Command::ShowWindow(WindowKind::SetPanning)); }
                if ui.button("微调…").clicked() { state.emit(Command::ShowWindow(WindowKind::SetPitch)); }
            });
            ui.separator();
            instrument_menus(state, ui);
            ui.separator();
            if ui.button("宏编辑器（15 种宏）…  Ctrl+M").clicked() { state.emit(Command::ShowWindow(WindowKind::MacroEditor)); ui.close_menu(); }
            ui.menu_button("快捷宏", |ui| {
                for m in [
                    nbs_core::Macro::FadeIn,
                    nbs_core::Macro::FadeOut,
                    nbs_core::Macro::Vibrato,
                    nbs_core::Macro::VelocityLfo,
                    nbs_core::Macro::Reset,
                    nbs_core::Macro::ReplaceKey,
                ] {
                    if ui.button(m.name()).clicked() { state.emit(Command::RunMacro(m)); }
                }
            });
        });

        ui.menu_button("视图 / 设置", |ui| {
            ui.menu_button("主题", |ui| {
                for kind in ThemeKind::ALL {
                    if ui.button(kind.name()).clicked() { state.emit(Command::SetTheme(kind)); }
                }
            });
            ui.menu_button("显示选项", |ui| {
                for flag in [
                    Flag::ShowLayers, Flag::ShowPiano, Flag::ShowInstruments,
                    Flag::UseColors, Flag::UseShapes, Flag::UseIcons,
                    Flag::ShowNumbers, Flag::ShowOctaves, Flag::ShowKeyNames,
                    Flag::ShowIncompatible, Flag::ShowGridLines, Flag::Flats, Flag::Blackout,
                ] {
                    let mut v = Flag::get(flag, &state.flags);
                    if ui.checkbox(&mut v, flag.label()).changed() {
                        state.emit(Command::ToggleFlag(flag));
                    }
                }
            });
            ui.separator();
            if ui.button("歌曲信息…").clicked() { state.emit(Command::ShowWindow(WindowKind::SongInfo)); ui.close_menu(); }
            if ui.button("歌曲属性…").clicked() { state.emit(Command::ShowWindow(WindowKind::Properties)); ui.close_menu(); }
            if ui.button("歌曲统计…").clicked() { state.emit(Command::ShowWindow(WindowKind::Statistics)); ui.close_menu(); }
            if ui.button("音色设置…").clicked() { state.emit(Command::ShowWindow(WindowKind::Instruments)); ui.close_menu(); }
            if ui.button("音频与采样设置…").clicked() { state.emit(Command::ShowWindow(WindowKind::AudioSettings)); ui.close_menu(); }
            if ui.button("音频导出设置…").clicked() { state.emit(Command::ShowWindow(WindowKind::AudioExport)); ui.close_menu(); }
            if ui.button("片段编辑器…").clicked() { state.emit(Command::ShowWindow(WindowKind::ClipEditor)); ui.close_menu(); }
            if ui.button("首选项…").clicked() { state.emit(Command::ShowWindow(WindowKind::Preferences)); ui.close_menu(); }
            if ui.button("保存偏好设置").clicked() { state.emit(Command::SavePreferences); ui.close_menu(); }
        });

        ui.menu_button("帮助", |ui| {
            if ui.button("快捷键一览  F1").clicked() { state.emit(Command::ShowWindow(WindowKind::Shortcuts)); ui.close_menu(); }
            if ui.button("关于 nbs-rust").clicked() { state.emit(Command::ShowWindow(WindowKind::About)); ui.close_menu(); }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(state.song_title()).weak().small());
        });
    });
}

/// 原版在 Edit 菜单和网格右键菜单里都出现的三段乐器列表：
/// 按乐器选择 / 选择除该乐器之外的全部 / 把音符改成该乐器。
pub fn instrument_menus(state: &mut EditorState, ui: &mut Ui) {
    let names = instrument_rows(state);
    ui.menu_button("按乐器选择", |ui| {
        egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
            for (idx, name) in &names {
                if ui.button(format!("仅 {name}")).clicked() {
                    state.emit(Command::SelectByInstrument { instrument: *idx, invert: false });
                }
            }
        });
    });
    ui.menu_button("选择除乐器…", |ui| {
        egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
            for (idx, name) in &names {
                if ui.button(format!("除 {name} 外")).clicked() {
                    state.emit(Command::SelectByInstrument { instrument: *idx, invert: true });
                }
            }
        });
    });
    ui.menu_button("更改乐器到…", |ui| {
        egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
            for (idx, name) in &names {
                if ui.button(name).clicked() {
                    state.emit(Command::SetAllInstrument(*idx));
                }
            }
        });
    });
}

/// 内置 + 当前歌曲引用的自定义乐器，索引即音符里的 instrument 值。
fn instrument_rows(state: &EditorState) -> Vec<(u8, String)> {
    let mut rows: Vec<(u8, String)> = nbs_core::instruments::INSTRUMENTS
        .iter()
        .enumerate()
        .map(|(i, ins)| (i as u8, ins.name.to_string()))
        .collect();
    for (j, ci) in state.doc.song.custom_instruments.iter().enumerate() {
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

pub fn show_toolbar(state: &mut EditorState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("新建").clicked() { state.emit(Command::New); }
        if ui.button("打开").clicked() { state.emit(Command::Open); }
        if ui.button("保存").clicked() { state.emit(Command::Save); }
        ui.separator();
        if ui.button(if state.is_playing() { "暂停" } else { "播放" }).clicked() {
            state.emit(Command::PlayPause);
        }
        if ui.button("停止").clicked() { state.emit(Command::Stop); }
        if ui.button("回退").clicked() { state.emit(Command::SeekHome); }
        let loop_on = state.doc.song.meta.loop_enabled;
        if ui.selectable_label(loop_on, "循环").clicked() { state.emit(Command::ToggleLoop); }
        if ui.selectable_label(state.marker_follow, "跟随").clicked() { state.emit(Command::ToggleFollow); }
        ui.separator();
        if ui.button("撤销").clicked() { state.emit(Command::Undo); }
        if ui.button("重做").clicked() { state.emit(Command::Redo); }
        ui.separator();

        for mode in EditMode::ALL {
            if ui.selectable_label(state.tool == mode, mode.name()).clicked() {
                state.emit(Command::SetEditMode(mode));
            }
        }
        ui.separator();

        // 当前画笔：音色 + 音高/力度/声像/微调
        egui::ComboBox::from_id_salt("current_instrument")
            .selected_text(instrument_name(state.current.instrument))
            .width(118.0)
            .show_ui(ui, |ui| {
                for (i, ins) in INSTRUMENTS.iter().enumerate() {
                    ui.selectable_value(&mut state.current.instrument, i as u8, ins.name);
                }
            });
        if ui.small_button("-").clicked() { state.emit(Command::SetKey(-1)); }
        ui.label(
            egui::RichText::new(nbs_core::model::key_name(
                state.current.key, true, state.flags.flats,
            ))
            .monospace(),
        );
        if ui.small_button("+").clicked() { state.emit(Command::SetKey(1)); }
        ui.label(
            egui::RichText::new(format!("V{} P{}", state.current.velocity, state.current.panning))
                .small()
                .weak(),
        );
        ui.separator();
        if ui.small_button("缩小").clicked() { state.emit(Command::ZoomOut); }
        ui.label(
            egui::RichText::new(format!("{}%", (state.viewport.zoom * 100.0) as i32))
                .small()
                .monospace(),
        );
        if ui.small_button("放大").clicked() { state.emit(Command::ZoomIn); }
        ui.separator();
        if ui.selectable_label(state.prefs.metronome, "节拍器").clicked() {
            state.emit(Command::ToggleMetronome);
        }
        let mut vol = state.prefs.master_volume;
        if ui.add_sized(egui::vec2(64.0, 18.0), egui::Slider::new(&mut vol, 0.0..=2.0).show_value(false)).changed() {
            state.emit(Command::SetMasterVolume(vol));
        }
        let mut speed = state.prefs.play_speed;
        if ui.add_sized(egui::vec2(56.0, 18.0), egui::Slider::new(&mut speed, 0.25..=4.0).show_value(false)).changed() {
            state.emit(Command::SetPlaySpeed(speed));
        }
        if ui.small_button("音频").clicked() { state.emit(Command::ShowWindow(WindowKind::AudioSettings)); }
        if ui.small_button("宏").clicked() { state.emit(Command::ShowWindow(WindowKind::MacroEditor)); }
    });
    // 第二行显示完整画笔信息，便于核对批量修改前的参数
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(current_note_label(state)).small().weak());
    });
}
