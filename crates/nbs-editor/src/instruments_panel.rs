//! 右侧音色面板：16 种内置乐器 + 当前画笔属性编辑。

use egui::{Color32, RichText, Ui};
use nbs_core::instruments::INSTRUMENTS;
use nbs_core::model::{PAN_MAX, VEL_MAX, PIT_MAX};

use crate::commands::Command;
use crate::state::EditorState;

pub fn show_instruments_panel(state: &mut EditorState, ui: &mut Ui) {
    ui.heading("音色");
    ui.label(RichText::new("点击选择当前画笔音色 (Ctrl+1..9)").small().weak());
    ui.separator();
    for (i, ins) in INSTRUMENTS.iter().enumerate() {
        let (r, g, b) = ins.color;
        let selected = state.current.instrument == i as u8;
        ui.horizontal(|ui| {
            let swatch = egui::Button::new("  ")
                .fill(Color32::from_rgb(r, g, b))
                .min_size(egui::vec2(18.0, 18.0));
            if ui.add(swatch).clicked() {
                state.emit(Command::SetInstrument(i as u8));
            }
            if ui.selectable_label(selected, ins.name).clicked() {
                state.emit(Command::SetInstrument(i as u8));
            }
        });
    }
    if !state.doc.song.custom_instruments.is_empty() {
        ui.separator();
        ui.label(RichText::new(format!(
            "自定义音色 {} 个（B 模块音频接入后可试听）",
            state.doc.song.custom_instruments.len()
        )).small().weak());
    }

    ui.separator();
    ui.heading("画笔属性");
    ui.horizontal(|ui| {
        ui.label("音高");
        if ui.small_button("-12").clicked() { state.emit(Command::SetKey(-12)); }
        if ui.small_button("-1").clicked() { state.emit(Command::SetKey(-1)); }
        ui.label(RichText::new(state.current.key.to_string()).monospace());
        if ui.small_button("+1").clicked() { state.emit(Command::SetKey(1)); }
        if ui.small_button("+12").clicked() { state.emit(Command::SetKey(12)); }
    });
    let mut vel = state.current.velocity as f32;
    if ui.add(egui::Slider::new(&mut vel, 0.0..=VEL_MAX as f32).text("力度")).changed() {
        state.current.velocity = vel as u8;
    }
    let mut pan = state.current.panning as f32;
    if ui.add(egui::Slider::new(&mut pan, 0.0..=PAN_MAX as f32).text("声像")).changed() {
        state.current.panning = pan as u8;
    }
    let mut pit = state.current.pitch as f32;
    if ui
        .add(egui::Slider::new(&mut pit, -(PIT_MAX as f32)..=PIT_MAX as f32).text("微调 (¢)"))
        .changed()
    {
        state.current.pitch = pit as i16;
    }
}
