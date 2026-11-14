//! 左侧图层栏：名称、锁定/独奏、音量/声像拖拽、选中整层、增删与上下移动。
//! 行高严格取 `viewport.cell()`，与右侧网格逐行对齐。

use egui::{pos2, Align2, FontId, Id, Rect, Sense, Shape, Ui, Vec2};
use nbs_core::model::{LockMode, PAN_CENTER};

use crate::commands::Command;
use crate::state::{ContextKind, ContextMenu, EditorState, LayerDrag, WindowKind};
use crate::viewport::{LAYER_PANEL_WIDTH, TIMELINE_HEIGHT};

const BTN: f32 = 20.0;
const VAL_W: f32 = 24.0;

pub fn show_layers_panel(state: &mut EditorState, ui: &mut Ui) {
    let cell = state.viewport.cell().clamp(22.0, 64.0);
    let (_, _, l0, l1) = state.viewport.visible_range();
    let th = state.theme.clone();
    let width = ui.available_width().max(160.0);
    let start = ui.cursor().min;

    // 顶部与时间轴对齐的 32px：放「添加图层」按钮
    let header = Rect::from_min_size(start, Vec2::new(width, TIMELINE_HEIGHT));
    ui.allocate_rect(header, Sense::hover());
    let p = ui.painter();
    p.rect_filled(header, 0.0, th.panel);
    p.rect_filled(
        Rect::from_min_size(pos2(header.min.x, header.max.y - 1.0), Vec2::new(width, 1.0)),
        0.0,
        th.border,
    );
    if ui
        .put(
            Rect::from_min_max(
                pos2(header.min.x + 3.0, header.min.y + 3.0),
                pos2(header.max.x - 3.0, header.max.y - 3.0),
            ),
            egui::Button::new("+  添加图层"),
        )
        .clicked()
    {
        state.emit(Command::AddLayer);
    }

    let rows_start = pos2(start.x, start.y + TIMELINE_HEIGHT);
    let rows = (l1 - l0) as usize;
    let total = Rect::from_min_size(rows_start, Vec2::new(width, rows as f32 * cell));
    ui.allocate_rect(total, Sense::hover());

    let layers = state.doc.song.layers.clone();
    let locked = state.doc.song.locked_layers();
    let drag_preview = state.layer_drag;

    for (i, idx) in (l0..l1).enumerate() {
        if idx as usize >= layers.len() {
            break;
        }
        let y = rows_start.y + i as f32 * cell;
        let row = Rect::from_min_size(pos2(rows_start.x, y), Vec2::new(width, cell));
        let layer = &layers[idx as usize];
        let is_locked = locked.get(idx as usize).copied().unwrap_or(false);
        let selected_in_layer = state.doc.selection.notes.iter().any(|n| {
            state.doc.selection.y + n.dy as i64 == idx as i64
        });

        let mut shapes = Vec::new();
        shapes.push(Shape::rect_filled(
            row,
            0.0,
            if selected_in_layer {
                th.accent_soft
            } else if is_locked {
                th.locked
            } else {
                th.panel
            },
        ));
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(pos2(row.min.x, row.max.y - 1.0), Vec2::new(width, 1.0)),
            0.0,
            th.border,
        ));
        ui.painter().extend(shapes);

        // 名称区（点击重命名 / 右键菜单）
        let name_w = (width - VAL_W * 2.0 - BTN * 6.0 - 10.0).max(60.0);
        let name_rect = Rect::from_min_size(
            pos2(row.min.x + 4.0, y + 2.0),
            Vec2::new(name_w, cell - 4.0),
        );
        let name_resp = ui.interact(
            name_rect,
            Id::new(("layer_name", idx)),
            Sense::click(),
        );
        let label = if layer.name.is_empty() {
            format!("图层 {}", idx + 1)
        } else {
            layer.name.clone()
        };
        ui.painter().text(
            pos2(name_rect.min.x + 4.0, name_rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional((cell * 0.36).clamp(9.0, 14.0)),
            if is_locked { th.text_dim } else { th.text },
        );
        if name_resp.double_clicked() {
            state.editing_layer_name = Some((idx, layer.name.clone()));
            state.emit(Command::ShowWindow(WindowKind::LayerRename(idx)));
        }
        if name_resp.secondary_clicked() {
            state.context_menu = Some(ContextMenu {
                pos: name_resp.hover_pos().unwrap_or(name_rect.center()),
                cell: (0, idx),
                kind: ContextKind::Layer(idx),
            });
        }

        // 音量 / 声像小标签（水平拖拽调值）
        let vol = drag_preview
            .filter(|d| d.index == idx && !d.stereo)
            .map(|d| d.value)
            .unwrap_or(layer.volume);
        let stereo = drag_preview
            .filter(|d| d.index == idx && d.stereo)
            .map(|d| d.value)
            .unwrap_or(layer.stereo);
        let val_w = VAL_W;
        for (k, _value, text, is_stereo) in [
            (0, vol, format!("V{vol}"), false),
            (1, stereo, pan_label(stereo), true),
        ] {
            let r = Rect::from_min_size(
                pos2(name_rect.max.x + k as f32 * val_w, y + 2.0),
                Vec2::new(val_w - 2.0, cell - 4.0),
            );
            let resp = ui.interact(
                r,
                Id::new(("layer_val", idx, k)),
                Sense::drag(),
            );
            ui.painter().rect_filled(r, 2.0, th.button);
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional((cell * 0.28).clamp(8.0, 11.0)),
                th.text_dim,
            );
            if resp.dragged() {
                let delta = resp.drag_delta().x;
                let max = if is_stereo { 200 } else { 100 };
                let base = if is_stereo { layer.stereo } else { layer.volume } as i32;
                let next = (base as f32 + delta * 0.8).clamp(0.0, max as f32) as u8;
                state.layer_drag = Some(LayerDrag { index: idx, stereo: is_stereo, value: next });
            }
            if resp.drag_stopped() {
                if let Some(d) = state.layer_drag.take() {
                    state.emit(if d.stereo {
                        Command::SetLayerStereo(d.index, d.value)
                    } else {
                        Command::SetLayerVolume(d.index, d.value)
                    });
                }
            }
        }

        // 6 个功能按钮
        let bx = name_rect.max.x + val_w * 2.0 + 2.0;
        let buttons: [(&str, Command, bool); 6] = [
            ("L", Command::SetLayerLock(idx, if layer.lock == LockMode::Locked { LockMode::Normal } else { LockMode::Locked }), layer.lock == LockMode::Locked),
            ("S", Command::SetLayerLock(idx, if layer.lock == LockMode::Solo { LockMode::Normal } else { LockMode::Solo }), layer.lock == LockMode::Solo),
            ("A", Command::SelectLayer(idx), false),
            ("+", Command::AddLayer, false),
            ("−", Command::RemoveLayer(idx), false),
            ("⇅", Command::MoveLayerUp(idx), false),
        ];
        for (j, (text, cmd, active)) in buttons.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(bx + j as f32 * BTN, y + 2.0),
                Vec2::new(BTN - 1.0, cell - 4.0),
            );
            let mut button = egui::Button::new(*text).min_size(Vec2::new(BTN - 1.0, cell - 4.0));
            if *active {
                button = button.fill(th.accent_soft);
            }
            if ui.put(r, button).clicked() {
                state.emit(cmd.clone());
            }
        }
    }

}

fn pan_label(v: u8) -> String {
    let d = v as i32 - PAN_CENTER as i32;
    if d == 0 {
        "C".into()
    } else if d < 0 {
        format!("L{}", -d)
    } else {
        format!("R{d}")
    }
}

pub const PANEL_WIDTH: f32 = LAYER_PANEL_WIDTH;
