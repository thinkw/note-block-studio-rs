//! 底部状态栏：画笔、悬停音符、坐标、选中数、性能、时间轴、BPM、消息。

use egui::{RichText, Ui};

use crate::state::EditorState;

pub fn show_status_bar(state: &EditorState, ui: &mut Ui) {
    let th = &state.theme;
    ui.horizontal(|ui| {
        let label = |ui: &mut Ui, s: String| {
            ui.label(RichText::new(s).small().color(th.text_dim));
        };

        if let Some(msg) = state.active_message() {
            ui.label(RichText::new(msg).small().strong().color(th.accent));
            ui.separator();
        }

        label(ui, format!(
            "画笔 {} {}",
            nbs_core::instruments::instrument_name(state.current.instrument),
            nbs_core::model::key_name(state.current.key, true, state.flags.flats)
        ));
        ui.separator();

        if let Some((t, l)) = state.hover_cell {
            if let Some(n) = state.doc.song.get(t, l) {
                label(ui, format!(
                    "悬停 [{}:{}] {} V{} L/R{} {}¢",
                    t, l,
                    nbs_core::model::key_name(n.key, true, state.flags.flats),
                    n.velocity,
                    n.panning as i32 - 100,
                    n.pitch
                ));
            } else {
                label(ui, format!("光标 tick={} layer={}", t, l));
            }
        } else {
            label(ui, "光标 —".into());
        }
        ui.separator();
        label(ui, format!("选中 {}", state.doc.selection.len()));
        label(ui, format!("音符 {}", state.doc.song.total_blocks));
        label(ui, format!("本帧绘制 {}", state.interaction.visible_notes));
        label(ui, format!("历史 {}/{}", state.doc.history.cursor(), state.doc.history.len()));
        ui.separator();
        label(ui, format!(
            "位置 {} / {}", state.marker().floor() as u32, state.doc.song.song_length()
        ));
        label(ui, format!(
            "{:.1} t/s  {:.0} BPM  {}/4",
            state.doc.song.meta.tempo,
            state.doc.song.meta.bpm(),
            state.doc.song.meta.time_signature
        ));
        label(ui, format!(
            "视口 tick {} 层 {} 缩放 {}%",
            state.viewport.start_tick,
            state.viewport.start_layer,
            (state.viewport.zoom * 100.0) as i32
        ));
        ui.separator();
        label(ui, format!(
            "{} | voice {} | 速度 {:.2}× | 主音量 {:.0}%",
            state.audio.name(),
            state.audio.active_voices(),
            state.prefs.play_speed,
            state.prefs.master_volume * 100.0
        ));
    });
}
