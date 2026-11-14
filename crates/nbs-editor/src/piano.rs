//! 底部钢琴键盘：点击选音高，左右按钮平移音域；黑键按音名自动判定。

use egui::{pos2, Align2, Color32, FontId, Id, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use nbs_core::model::{key_name, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::commands::Command;
use crate::state::EditorState;

const SPAN: u8 = 25;

pub fn show_piano(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let top = ui.cursor().min;
    let width = ui.available_width();
    let height = ui.available_height().max(60.0);
    let area = Rect::from_min_size(top, Vec2::new(width, height));
    ui.allocate_rect(area, Sense::hover());

    let btn_w = 34.0;
    let key_area = Rect::from_min_max(
        pos2(area.min.x + btn_w, area.min.y),
        pos2(area.max.x - btn_w, area.max.y),
    );
    if ui.put(
        Rect::from_min_size(area.min, Vec2::new(btn_w, height)),
        egui::Button::new("<"),
    ).clicked() {
        state.piano_start = state.piano_start.saturating_sub(12);
    }
    if ui.put(
        Rect::from_min_size(pos2(area.max.x - btn_w, area.min.y), Vec2::new(btn_w, height)),
        egui::Button::new(">"),
    ).clicked() {
        state.piano_start = (state.piano_start + 12).min(KEY_MAX.saturating_sub(SPAN));
    }

    let start = state.piano_start;
    let end = (start + SPAN).min(KEY_MAX);
    let whites: Vec<u8> = (start..=end)
        .filter(|k| !key_name(*k, false, false).contains('#'))
        .collect();
    if whites.is_empty() {
        return;
    }
    let w = key_area.width() / whites.len() as f32;
    let black_h = height * 0.62;
    let mut shapes = Vec::new();
    let mut texts = Vec::new();
    let mut white_hits: Vec<(Rect, u8)> = Vec::new();
    let mut black_hits: Vec<(Rect, u8)> = Vec::new();

    for (i, key) in whites.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(key_area.min.x + i as f32 * w, key_area.min.y),
            Vec2::new(w - 1.0, height),
        );
        let current = state.current.key == *key;
        let in_mc = *key >= MC_KEY_MIN && *key <= MC_KEY_MAX;
        let fill = if current {
            th.accent_soft
        } else if in_mc {
            if th.dark { Color32::from_gray(226) } else { Color32::WHITE }
        } else if th.dark {
            Color32::from_gray(150)
        } else {
            Color32::from_gray(224)
        };
        shapes.push(Shape::rect_filled(r, 2.0, fill));
        shapes.push(Shape::rect_stroke(
            r, 2.0, Stroke::new(1.0_f32, th.border), StrokeKind::Inside,
        ));
        texts.push((pos2(r.center().x, r.max.y - 8.0), key_name(*key, true, state.flags.flats)));
        white_hits.push((r, *key));
    }
    for key in start..=end {
        if !key_name(key, false, false).contains('#') {
            continue;
        }
        // 黑键定位在左邻白键的右边界
        let below = whites.iter().position(|w| *w == key - 1).unwrap_or(0);
        let x = key_area.min.x + below as f32 * w + w * 0.62;
        let bw = w * 0.66;
        let r = Rect::from_min_size(pos2(x, key_area.min.y), Vec2::new(bw, black_h));
        shapes.push(Shape::rect_filled(
            r.shrink(1.0),
            2.0,
            if state.current.key == key { th.accent } else if th.dark { Color32::from_gray(28) } else { Color32::from_gray(40) },
        ));
        black_hits.push((r, key));
    }
    ui.painter().extend(shapes);
    for (p, t) in texts {
        ui.painter().text(p, Align2::CENTER_CENTER, t, FontId::proportional(9.0), th.text);
    }

    // 黑键在上层，先处理它的点击
    for (r, key) in black_hits.iter().rev() {
        let resp = ui.interact(*r, Id::new(("piano_black", *key)), Sense::click());
        if resp.clicked() {
            state.emit(Command::SetKey(*key as i32 - state.current.key as i32));
        }
    }
    for (r, key) in &white_hits {
        let resp = ui.interact(*r, Id::new(("piano_white", *key)), Sense::click());
        if resp.clicked() {
            state.emit(Command::SetKey(*key as i32 - state.current.key as i32));
        }
    }
}
