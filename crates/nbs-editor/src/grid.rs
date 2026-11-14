//! D 模块的核心：超大滚动网格画布。
//!
//! 每帧只遍历 `[start_tick, start_tick + 可见列数)` × `[start_layer, +可见行数)`，
//! 音符查询走 [`nbs_core::model::Song::notes_in_view`]（`BTreeMap::range` + colfirst/collast
//! 早退），因此打开几十万 tick 的工程也不会因为「遍历全部音符」而卡死。
//! 所有矩形先攒进 `Vec<Shape>` 一次 `Painter::extend` 提交，减少批次开销。

use egui::{
    pos2, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2,
};
use nbs_core::instruments::{instrument_color, instrument_shape};
use nbs_core::model::{key_name, key_offset, Note, MC_KEY_MAX, MC_KEY_MIN, PAN_CENTER};

use crate::commands::Command;
use crate::interaction::{DragMode, InputSnapshot};
use crate::state::{ContextKind, ContextMenu, EditMode, EditorState};
use crate::viewport::{SCROLLBAR_SIZE, TIMELINE_HEIGHT};

struct TextDraw {
    pos: Pos2,
    text: String,
    size: f32,
    color: Color32,
    align: Align2,
}

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// 切分布局：时间轴 / 网格 / 水平滚动条 / 垂直滚动条。
fn split(area: Rect) -> (Rect, Rect, Rect, Rect) {
    let w = (area.width() - SCROLLBAR_SIZE).max(64.0);
    let h = (area.height() - TIMELINE_HEIGHT - SCROLLBAR_SIZE).max(64.0);
    let timeline = Rect::from_min_size(area.min, Vec2::new(w, TIMELINE_HEIGHT));
    let grid = Rect::from_min_size(
        pos2(area.min.x, area.min.y + TIMELINE_HEIGHT),
        Vec2::new(w, h),
    );
    let hbar = Rect::from_min_size(
        pos2(area.min.x, area.min.y + TIMELINE_HEIGHT + h),
        Vec2::new(w, SCROLLBAR_SIZE),
    );
    let vbar = Rect::from_min_size(
        pos2(area.min.x + w, area.min.y + TIMELINE_HEIGHT),
        Vec2::new(SCROLLBAR_SIZE, h),
    );
    (timeline, grid, hbar, vbar)
}

pub fn show_grid(state: &mut EditorState, ui: &mut Ui, area: Rect) {
    let (timeline_rect, grid_rect, hbar_rect, vbar_rect) = split(area);

    state.viewport.origin = grid_rect.min;
    state.viewport.size = grid_rect.size();
    let (enda, layer_count) = (state.doc.song.enda, state.doc.song.layer_count());
    let max_start = enda.max(64);
    state.viewport.start_tick = state.viewport.start_tick.min(max_start);
    let max_lstart = layer_count.saturating_sub(1).max(8);
    state.viewport.start_layer = state.viewport.start_layer.min(max_lstart);

    // 占住整块区域，避免 egui 布局把它分配给别的 widget。
    let _response = ui.allocate_rect(area, Sense::click_and_drag());
    let input = InputSnapshot::read(ui);

    handle_timeline_input(state, &input, timeline_rect);
    handle_grid_input(state, &input, grid_rect);
    handle_scrollbar_input(state, &input, hbar_rect, vbar_rect);

    draw_timeline(state, ui, timeline_rect);
    draw_grid(state, ui, grid_rect);
    draw_scrollbars(state, ui, hbar_rect, vbar_rect);
    draw_context_menu(state, ui);

    ui.output_mut(|o| o.cursor_icon = state.interaction.cursor);
}

// ---------------------------------------------------------------- 输入处理

fn handle_timeline_input(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    let Some(pos) = input.pointer else { return };
    if !rect.contains(pos) || state.window.is_some() {
        return;
    }
    if input.secondary_pressed {
        let tick = state.viewport.x_tick(pos.x).max(0) as u32;
        state.context_menu = Some(ContextMenu {
            pos,
            cell: (tick, 0),
            kind: ContextKind::Timeline,
        });
        return;
    }
    if input.primary_pressed {
        state.interaction.mode = DragMode::DragMarker;
    }
    if matches!(state.interaction.mode, DragMode::DragMarker) && input.primary_down {
        let tick = state.viewport.x_tick(pos.x).max(0) as f64;
        state.set_marker(tick);
        state.interaction.cursor = CursorIcon::ResizeHorizontal;
    }
    if input.primary_released && matches!(state.interaction.mode, DragMode::DragMarker) {
        state.interaction.reset();
    }
}

fn handle_grid_input(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    state.interaction.cursor = CursorIcon::Default;
    state.interaction.click_in_grid = false;

    let pointer_cell = input
        .pointer
        .filter(|p| rect.contains(*p))
        .and_then(|p| state.viewport.cell_at(p));
    state.hover_cell = pointer_cell.map(|(t, l)| (t, l));
    let hover = pointer_cell.map(|(t, l)| (t as i64, l as i64));

    let interactive = hover.is_some() && state.window.is_none() && state.context_menu.is_none();
    let pointer_in_grid = input.pointer.is_some_and(|p| rect.contains(p));

    // 滚轮：Ctrl 缩放 / 悬停音符调属性 / 默认平移视口。
    if pointer_in_grid && (input.scroll.x.abs() > 0.0 || input.scroll.y.abs() > 0.0) {
        let delta = input.scroll.y + input.scroll.x;
        if input.ctrl {
            state.viewport.zoom_by(if delta > 0.0 { 1.1 } else { 0.9 });
        } else if let Some((t, l)) = hover {
            let on_note = state.doc.song.has(t as u32, l as u32)
                || state.doc.selection.contains(t, l);
            if on_note {
                let d = if delta > 0.0 { 1 } else { -1 };
                state.emit(Command::WheelOnNote {
                    tick: t as u32,
                    layer: l as u32,
                    delta: d,
                    shift: input.shift,
                });
            } else {
                scroll_viewport(state, delta, input.shift);
            }
        } else {
            scroll_viewport(state, delta, input.shift);
        }
    }

    if !interactive {
        if state.interaction.is_dragging() {
            finish_drag(state, input, hover);
        }
        return;
    }

    let (t, l) = (hover.unwrap().0, hover.unwrap().1);
    let cell = (t as u32, l as u32);

    // ---- 按下 ----
    if input.primary_pressed {
        state.interaction.click_in_grid = true;
        state.interaction.pressed_cell = Some((t, l));
        state.interaction.pressed_pos = input.pointer;
        state.interaction.ctrl_at_press = input.ctrl;
        state.interaction.alt_at_press = input.alt;
        state.interaction.secondary_at_press = false;

        if !input.ctrl && state.doc.selection.contains(t, l) {
            let sel = &state.doc.selection;
            state.interaction.mode = DragMode::MoveSelection {
                grab: (t, l),
                origin: (sel.x, sel.y),
                before: sel.clone(),
            };
            state.interaction.cursor = CursorIcon::Grabbing;
        }
    }
    if input.secondary_pressed {
        state.interaction.click_in_grid = true;
        state.interaction.pressed_cell = Some((t, l));
        state.interaction.pressed_pos = input.pointer;
        state.interaction.ctrl_at_press = input.ctrl;
        state.interaction.alt_at_press = input.alt;
        state.interaction.secondary_at_press = true;
    }
    if input.middle_pressed {
        // 中键单击 = 吸管；中键拖动 = 平移画布。松开时按位移判定。
        state.interaction.mode = DragMode::PanView {
            start: input.pointer.unwrap_or(pos2(0.0, 0.0)),
            start_tick: state.viewport.start_tick,
            start_layer: state.viewport.start_layer,
        };
        state.interaction.pressed_pos = input.pointer;
        state.interaction.pressed_cell = Some((t, l));
        state.interaction.ctrl_at_press = input.ctrl;
    }

    // ---- 拖动：判定框选 ----
    let pressed = state.interaction.pressed_cell;
    let pressed_pos = state.interaction.pressed_pos;
    if let (Some(anchor), Some(start_pos), Some(pos)) = (pressed, pressed_pos, input.pointer) {
        let moved = (pos - start_pos).length() > 5.0 || Some((t, l)) != Some(anchor);
        if moved {
            if input.primary_down && state.interaction.mode == DragMode::None {
                state.interaction.mode = DragMode::BoxSelect {
                    anchor,
                    subtract: false,
                    alt_copy: state.interaction.alt_at_press,
                };
            } else if input.secondary_down && state.interaction.mode == DragMode::None {
                state.interaction.mode = DragMode::BoxSelect {
                    anchor,
                    subtract: true,
                    alt_copy: false,
                };
            }
        }
    }

    // ---- 拖拽中：移动浮动选区 / 边缘自动滚动 ----
    match state.interaction.mode.clone() {
        DragMode::MoveSelection { grab, origin, .. } => {
            let nx = t - grab.0 + origin.0;
            let ny = l - grab.1 + origin.1;
            state.doc.set_selection_origin(nx, ny);
            state.interaction.cursor = CursorIcon::Grabbing;
            edge_scroll(state, input, rect);
        }
        DragMode::BoxSelect { .. } => {
            state.interaction.cursor = CursorIcon::Crosshair;
            edge_scroll(state, input, rect);
        }
        DragMode::PanView { start, start_tick, start_layer } => {
            if let Some(pos) = input.pointer {
                let c = state.viewport.cell();
                state.viewport.start_tick = (start_tick as f64 - (pos.x - start.x) as f64 / c as f64)
                    .max(0.0) as u32;
                state.viewport.start_layer = (start_layer as f64 - (pos.y - start.y) as f64 / c as f64)
                    .max(0.0) as u32;
            }
            state.interaction.cursor = CursorIcon::Grabbing;
        }
        _ => {
            if state.doc.selection.contains(t, l) {
                state.interaction.cursor = CursorIcon::Grab;
            } else if state.doc.song.has(cell.0, cell.1) {
                state.interaction.cursor = CursorIcon::PointingHand;
            } else {
                state.interaction.cursor = CursorIcon::Crosshair;
            }
        }
    }

    // ---- 松开 ----
    finish_drag(state, input, Some((t, l)));
}

fn scroll_viewport(state: &mut EditorState, delta: f32, shift: bool) {
    let step = if delta > 0.0 { 1 } else { -1 };
    if shift {
        let s = state.viewport.start_tick as i64 + step * 4;
        state.viewport.start_tick = s.max(0) as u32;
    } else {
        let s = state.viewport.start_layer as i64 + step;
        state.viewport.start_layer = s.max(0) as u32;
    }
}

/// 拖拽越过网格边界时自动滚动（原版 dragincxr/xl/yd/yu 的等价实现）。
fn edge_scroll(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    let Some(pos) = input.pointer else { return };
    let edge = 28.0;
    let dx = if pos.x < rect.min.x + edge {
        -1.0
    } else if pos.x > rect.max.x - edge {
        1.0
    } else {
        0.0
    };
    let dy = if pos.y < rect.min.y + edge {
        -1.0
    } else if pos.y > rect.max.y - edge {
        1.0
    } else {
        0.0
    };
    let e = &mut state.interaction.edge_scroll;
    e.x = (e.x + dx * 0.18).clamp(-4.0, 4.0);
    e.y = (e.y + dy * 0.18).clamp(-4.0, 4.0);
    let ix = e.x.trunc() as i64;
    let iy = e.y.trunc() as i64;
    if ix != 0 {
        state.viewport.start_tick = (state.viewport.start_tick as i64 + ix).max(0) as u32;
        e.x -= ix as f32;
    }
    if iy != 0 {
        state.viewport.start_layer = (state.viewport.start_layer as i64 + iy).max(0) as u32;
        e.y -= iy as f32;
    }
    if dx == 0.0 {
        e.x *= 0.8;
    }
    if dy == 0.0 {
        e.y *= 0.8;
    }
}

fn finish_drag(state: &mut EditorState, input: &InputSnapshot, hover: Option<(i64, i64)>) {
    let mode = state.interaction.mode.clone();
    let pressed = state.interaction.pressed_cell;
    let ctrl = state.interaction.ctrl_at_press;
    let secondary = state.interaction.secondary_at_press;

    if input.primary_released {
        match mode.clone() {
            DragMode::BoxSelect { anchor, subtract, alt_copy } => {
                let end = hover.unwrap_or(anchor);
                state.emit(Command::SelectRect {
                    x0: anchor.0.max(0) as u32,
                    y0: anchor.1.max(0) as u32,
                    x1: end.0.max(0) as u32,
                    y1: end.1.max(0) as u32,
                    copy: alt_copy,
                    additive: ctrl,
                    subtract,
                });
            }
            DragMode::MoveSelection { before, .. } => {
                state.doc.commit_selection_move(before);
            }
            DragMode::PanView { .. } | DragMode::DragMarker => {}
            DragMode::None => {
                if let Some(cell) = pressed {
                    if Some(cell) == hover && !secondary {
                        if ctrl {
                            state.emit(Command::SelectRect {
                                x0: cell.0.max(0) as u32,
                                y0: cell.1.max(0) as u32,
                                x1: cell.0.max(0) as u32,
                                y1: cell.1.max(0) as u32,
                                copy: false,
                                additive: true,
                                subtract: false,
                            });
                        } else {
                            // 有浮动选区时先落定，再放置/覆盖音符。
                            if !state.doc.selection.is_empty() {
                                state.doc.place_selection();
                            }
                            state.emit(Command::PlaceNote {
                                tick: cell.0.max(0) as u32,
                                layer: cell.1.max(0) as u32,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        state.interaction.reset();
    }

    if input.secondary_released {
        match mode {
            DragMode::BoxSelect { anchor, .. } => {
                let end = hover.unwrap_or(anchor);
                state.emit(Command::SelectRect {
                    x0: anchor.0.max(0) as u32,
                    y0: anchor.1.max(0) as u32,
                    x1: end.0.max(0) as u32,
                    y1: end.1.max(0) as u32,
                    copy: false,
                    additive: false,
                    subtract: true,
                });
            }
            DragMode::None => {
                if let Some(cell) = pressed {
                    if Some(cell) == hover {
                        let (t, l) = (cell.0.max(0) as u32, cell.1.max(0) as u32);
                        if ctrl {
                            state.emit(Command::SelectRect {
                                x0: t, y0: l, x1: t, y1: l,
                                copy: false, additive: false, subtract: true,
                            });
                        } else if state.doc.song.has(t, l) {
                            state.emit(Command::RemoveNote { tick: t, layer: l });
                        } else {
                            state.context_menu = Some(ContextMenu {
                                pos: input.pointer.unwrap_or(pos2(0.0, 0.0)),
                                cell: (t, l),
                                kind: ContextKind::Grid,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        state.interaction.reset();
    }

    // 中键：单击吸管 / 拖动平移画布
    if input.middle_released && matches!(&mode, DragMode::PanView { .. }) {
        let moved = match (state.interaction.pressed_pos, input.pointer) {
            (Some(a), Some(b)) => (a - b).length() > 5.0,
            _ => false,
        };
        if !moved {
            if let Some(c) = pressed {
                state.emit(Command::PickNote {
                    tick: c.0.max(0) as u32,
                    layer: c.1.max(0) as u32,
                    all: ctrl,
                });
            }
        }
        state.interaction.reset();
    }
}

// ---------------------------------------------------------------- 滚动条

fn h_geometry(state: &EditorState, rect: Rect) -> (f32, f32, Rect) {
    let visible = state.viewport.columns().max(1) as f32;
    let max_units = (state.doc.song.enda.max(state.viewport.start_tick) as f32 + visible + 8.0)
        .max(visible * 2.0);
    let thumb_w = ((visible / max_units) * rect.width()).max(24.0).min(rect.width());
    let thumb_x = rect.min.x
        + (state.viewport.start_tick as f32 / max_units.max(1.0)) * (rect.width() - thumb_w);
    (max_units, thumb_w, Rect::from_min_size(pos2(thumb_x, rect.min.y + 2.0), Vec2::new(thumb_w, rect.height() - 4.0)))
}

fn v_geometry(state: &EditorState, rect: Rect) -> (f32, f32, Rect) {
    let visible = state.viewport.rows().max(1) as f32;
    let max_units = (state.doc.song.layer_count().max(state.viewport.start_layer) as f32
        + visible + 4.0)
        .max(visible * 2.0);
    let thumb_h = ((visible / max_units) * rect.height()).max(24.0).min(rect.height());
    let thumb_y = rect.min.y
        + (state.viewport.start_layer as f32 / max_units.max(1.0)) * (rect.height() - thumb_h);
    (max_units, thumb_h, Rect::from_min_size(pos2(rect.min.x + 2.0, thumb_y), Vec2::new(rect.width() - 4.0, thumb_h)))
}

fn handle_scrollbar_input(state: &mut EditorState, input: &InputSnapshot, hbar: Rect, vbar: Rect) {
    let Some(pos) = input.pointer else { return };
    let (hm, _, ht) = h_geometry(state, hbar);
    let (vm, _, vt) = v_geometry(state, vbar);

    if input.primary_pressed {
        if hbar.contains(pos) {
            if ht.contains(pos) {
                state.interaction.mode = DragMode::DragScrollbarH { grab: pos.x - ht.min.x };
            } else {
                let ratio = ((pos.x - hbar.min.x) / hbar.width()).clamp(0.0, 1.0);
                state.viewport.start_tick = (ratio * hm) as u32;
            }
        } else if vbar.contains(pos) {
            if vt.contains(pos) {
                state.interaction.mode = DragMode::DragScrollbarV { grab: pos.y - vt.min.y };
            } else {
                let ratio = ((pos.y - vbar.min.y) / vbar.height()).clamp(0.0, 1.0);
                state.viewport.start_layer = (ratio * vm) as u32;
            }
        }
    }
    if input.primary_down {
        match state.interaction.mode.clone() {
            DragMode::DragScrollbarH { grab } => {
                let ratio = ((pos.x - grab - hbar.min.x) / hbar.width()).clamp(0.0, 1.0);
                state.viewport.start_tick = (ratio * hm) as u32;
            }
            DragMode::DragScrollbarV { grab } => {
                let ratio = ((pos.y - grab - vbar.min.y) / vbar.height()).clamp(0.0, 1.0);
                state.viewport.start_layer = (ratio * vm) as u32;
            }
            _ => {}
        }
    }
    if input.primary_released
        && matches!(
            state.interaction.mode,
            DragMode::DragScrollbarH { .. } | DragMode::DragScrollbarV { .. }
        )
    {
        state.interaction.reset();
    }
}

fn draw_scrollbars(state: &EditorState, ui: &Ui, hbar: Rect, vbar: Rect) {
    let p = ui.painter();
    let th = &state.theme;
    let mut shapes = Vec::new();
    shapes.push(Shape::rect_filled(hbar, 0.0, th.panel));
    shapes.push(Shape::rect_filled(vbar, 0.0, th.panel));
    let (_, _, ht) = h_geometry(state, hbar);
    let (_, _, vt) = v_geometry(state, vbar);
    shapes.push(Shape::rect_filled(ht, 3.0, th.accent_soft));
    shapes.push(Shape::rect_stroke(ht, 3.0, Stroke::new(1.0_f32, th.accent), StrokeKind::Inside));
    shapes.push(Shape::rect_filled(vt, 3.0, th.accent_soft));
    shapes.push(Shape::rect_stroke(vt, 3.0, Stroke::new(1.0_f32, th.accent), StrokeKind::Inside));
    p.extend(shapes);
}

// ---------------------------------------------------------------- 绘制

fn draw_timeline(state: &EditorState, ui: &Ui, rect: Rect) {
    let p = ui.painter().with_clip_rect(rect);
    let th = &state.theme;
    let vp = &state.viewport;
    let cell = vp.cell();
    let (t0, t1, _, _) = vp.visible_range();
    let per_bar = state.doc.song.meta.ticks_per_bar().max(1);
    let mut shapes = vec![Shape::rect_filled(rect, 0.0, th.panel)];
    let mut texts: Vec<TextDraw> = Vec::new();

    for tick in t0..t1 {
        let x = vp.tick_x(tick as i64);
        if x < rect.min.x - cell || x > rect.max.x + cell {
            continue;
        }
        let is_bar = tick % per_bar == 0;
        let is_beat = tick % 4 == 0;
        let h = if is_bar { 12.0 } else if is_beat { 7.0 } else { 4.0 };
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(pos2(x, rect.max.y - h), Vec2::new(1.0, h)),
            0.0,
            if is_bar { th.accent } else { th.text_dim },
        ));
        if (is_beat || (cell > 24.0 && tick % 2 == 0)) && cell >= 16.0 {
            texts.push(TextDraw {
                pos: pos2(x + 2.0, rect.min.y + 3.0),
                text: tick.to_string(),
                size: (cell * 0.30).clamp(8.0, 12.0),
                color: if is_bar { th.accent } else { th.text_dim },
                align: Align2::LEFT_TOP,
            });
        }
    }
    p.extend(shapes);
    for t in texts {
        p.text(t.pos, t.align, t.text, FontId::proportional(t.size), t.color);
    }

    // 播放头（时间轴刻度 + 三角形）
    let mx = vp.tick_x(state.marker().floor() as i64)
        + (state.marker().fract() as f32) * cell;
    if mx >= rect.min.x - 2.0 && mx <= rect.max.x + 2.0 {
        p.add(Shape::rect_filled(
            Rect::from_min_size(pos2(mx - 1.0, rect.min.y), Vec2::new(2.0, rect.height())),
            0.0,
            th.marker,
        ));
        p.add(Shape::convex_polygon(
            vec![
                pos2(mx - 6.0, rect.min.y),
                pos2(mx + 6.0, rect.min.y),
                pos2(mx, rect.min.y + 8.0),
            ],
            th.marker,
            Stroke::NONE,
        ));
    }
}

fn draw_grid(state: &mut EditorState, ui: &Ui, rect: Rect) {
    let p = ui.painter().with_clip_rect(rect);
    let th = &state.theme;
    let vp = &state.viewport;
    let cell = vp.cell();
    let (t0, t1, l0, l1) = vp.visible_range();
    let ts = state.doc.song.meta.ticks_per_bar().max(1);
    let locked = state.doc.song.locked_layers();
    let blackout = state.flags.blackout;

    let mut shapes: Vec<Shape> = Vec::with_capacity(4096);
    let mut texts: Vec<TextDraw> = Vec::with_capacity(512);

    // 背景
    shapes.push(Shape::rect_filled(
        rect,
        0.0,
        if blackout { Color32::BLACK } else { th.grid_bg },
    ));

    // 锁定层整行底纹
    if !blackout {
        for layer in l0..l1 {
            if locked.get(layer as usize).copied().unwrap_or(false) {
                let y = vp.layer_y(layer as i64);
                shapes.push(Shape::rect_filled(
                    Rect::from_min_size(pos2(rect.min.x, y), Vec2::new(rect.width(), cell)),
                    0.0,
                    th.locked,
                ));
            }
        }
    }

    // 竖线：小节 / 拍 / 普通 tick，只画可见列
    if state.flags.show_grid_lines && !blackout {
        for tick in t0..t1 {
            let x = vp.tick_x(tick as i64);
            if x < rect.min.x - 1.0 || x > rect.max.x + 1.0 {
                continue;
            }
            let is_bar = tick % ts == 0;
            let is_beat = tick % 4 == 0;
            let (w, c) = if is_bar {
                (2.0, alpha(th.grid_bar, 90))
            } else if is_beat {
                (1.0, alpha(th.grid_beat, 120))
            } else {
                (1.0, alpha(th.grid_line, 90))
            };
            shapes.push(Shape::rect_filled(
                Rect::from_min_size(pos2(x, rect.min.y), Vec2::new(w, rect.height())),
                0.0,
                c,
            ));
        }
        // 横向每 4 层一条淡分隔线
        for layer in (l0..l1).step_by(4) {
            let y = vp.layer_y(layer as i64);
            shapes.push(Shape::rect_filled(
                Rect::from_min_size(pos2(rect.min.x, y), Vec2::new(rect.width(), 1.0)),
                0.0,
                alpha(th.grid_line, 110),
            ));
        }
    }

    // 音符：唯一的查询入口，保证只处理可见区域
    let notes = state.doc.song.notes_in_view(t0, t1, l0, l1);
    let visible_count = notes.len();
    for (tick, layer, note) in notes {
        push_note_shapes(
            &mut shapes,
            &mut texts,
            state,
            tick as i64,
            layer as i64,
            note,
            false,
            &locked,
        );
    }

    // 浮动选区：同样先做整体视口剔除，再逐音符裁剪
    let sel = state.doc.selection.clone();
    if !sel.is_empty() {
        if let Some((bx0, by0, bx1, by1)) = sel.bounds() {
            if vp.rect_intersects(bx0, by0, bx1, by1) {
                for n in &sel.notes {
                    let at = sel.x + n.dx as i64;
                    let al = sel.y + n.dy as i64;
                    if at < t0 as i64 - 1
                        || at > t1 as i64
                        || al < l0 as i64 - 1
                        || al > l1 as i64
                    {
                        continue;
                    }
                    push_note_shapes(&mut shapes, &mut texts, state, at, al, n.note, true, &locked);
                }
            }
        }
    }
    state.interaction.visible_notes = visible_count + sel.len();

    // 框选矩形
    if let DragMode::BoxSelect { anchor, subtract, .. } = &state.interaction.mode {
        if let Some(hover) = state.hover_cell {
            let end = (hover.0 as i64, hover.1 as i64);
            let r = Rect::from_min_max(
                pos2(vp.tick_x(anchor.0.min(end.0)), vp.layer_y(anchor.1.min(end.1))),
                pos2(
                    vp.tick_x(anchor.0.max(end.0)) + cell,
                    vp.layer_y(anchor.1.max(end.1)) + cell,
                ),
            );
            let color = if *subtract { th.subtract } else { th.selection };
            shapes.push(Shape::rect_filled(r, 0.0, alpha(color, 55)));
            shapes.push(Shape::rect_stroke(
                r,
                0.0,
                Stroke::new(1.5_f32, color),
                StrokeKind::Inside,
            ));
        }
    }

    // 悬停格高亮
    if let Some((t, l)) = state.hover_cell {
        let r = vp.cell_rect(t as i64, l as i64);
        shapes.push(Shape::rect_stroke(
            r,
            0.0,
            Stroke::new(1.0_f32, alpha(th.accent, 150)),
            StrokeKind::Inside,
        ));
    }

    // 播放头
    if !blackout {
        let mx = vp.tick_x(state.marker().floor() as i64)
            + (state.marker().fract() as f32) * cell;
        if mx >= rect.min.x && mx <= rect.max.x {
            shapes.push(Shape::rect_filled(
                Rect::from_min_size(pos2(mx - 1.0, rect.min.y), Vec2::new(2.0, rect.height())),
                0.0,
                alpha(th.marker, 200),
            ));
        }
    }

    p.extend(shapes);
    for t in texts {
        p.text(t.pos, t.align, t.text, FontId::proportional(t.size), t.color);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_note_shapes(
    shapes: &mut Vec<Shape>,
    texts: &mut Vec<TextDraw>,
    state: &EditorState,
    tick: i64,
    layer: i64,
    note: Note,
    floating: bool,
    locked_layers: &[bool],
) {
    let vp = &state.viewport;
    let th = &state.theme;
    let flags = &state.flags;
    let cell = vp.cell();
    let r = vp.cell_rect(tick, layer);
    if r.max.x < -cell || r.max.y < -cell {
        return;
    }
    let inner = r.shrink((cell * 0.06).max(1.0));
    let center = inner.center();
    let radius = (cell * 0.08).clamp(1.0, 4.0);
    let vel_alpha = (0.55 + 0.45 * (note.velocity as f32 / 100.0)).clamp(0.4, 1.0);
    let base = if flags.use_colors {
        let (r8, g8, b8) = instrument_color(note.instrument);
        Color32::from_rgb(r8, g8, b8)
    } else {
        if th.dark {
            Color32::from_gray(110)
        } else {
            Color32::from_gray(140)
        }
    };
    let mut fill = alpha(base, (255.0 * vel_alpha) as u8);
    if floating {
        fill = alpha(fill, 210);
    }
    let locked_layer = locked_layers.get(layer.max(0) as usize).copied().unwrap_or(false);
    if locked_layer {
        fill = alpha(fill, 110);
    }

    let shape_kind = if flags.use_shapes {
        instrument_shape(note.instrument)
    } else {
        0
    };
    match shape_kind {
        1 => shapes.push(Shape::circle_filled(center, inner.width() * 0.42, fill)),
        2 => shapes.push(Shape::convex_polygon(
            vec![
                pos2(center.x, inner.min.y),
                pos2(inner.max.x, center.y),
                pos2(center.x, inner.max.y),
                pos2(inner.min.x, center.y),
            ],
            fill,
            Stroke::NONE,
        )),
        3 => shapes.push(Shape::convex_polygon(
            vec![inner.min, pos2(inner.max.x, inner.min.y), pos2(center.x, inner.max.y)],
            fill,
            Stroke::NONE,
        )),
        _ => shapes.push(Shape::rect_filled(inner, radius, fill)),
    }

    // 边框
    shapes.push(Shape::rect_stroke(
        inner,
        radius,
        Stroke::new(1.0_f32, alpha(th.border, 200)),
        StrokeKind::Inside,
    ));

    // 微调音分蒙版：负=红，正=蓝（与原版一致）
    if note.pitch != 0 {
        let tint = if note.pitch < 0 {
            Color32::from_rgb(230, 70, 70)
        } else {
            Color32::from_rgb(70, 120, 240)
        };
        shapes.push(Shape::rect_filled(
            inner,
            radius,
            alpha(tint, ((note.pitch.abs() as f32 / 1200.0) * 110.0) as u8),
        ));
    }

    // 不兼容（自定义乐器 / 超出 MC 音域）红框
    if flags.show_incompatible && (note.key < MC_KEY_MIN || note.key > MC_KEY_MAX) {
        shapes.push(Shape::rect_stroke(
            inner.shrink(1.0),
            0.0,
            Stroke::new(2.0_f32, Color32::from_rgb(230, 60, 60)),
            StrokeKind::Inside,
        ));
    }
    // 浮动选区高亮
    if floating {
        shapes.push(Shape::rect_stroke(
            r.shrink(0.5),
            0.0,
            Stroke::new(2.0_f32, th.selection),
            StrokeKind::Outside,
        ));
    }

    // 文本：音名 + 当前编辑模式对应的属性数值
    if cell >= 18.0 {
        let small = (cell * 0.26).clamp(7.0, 12.0);
        let big = (cell * 0.34).clamp(9.0, 16.0);
        if flags.show_keynames {
            texts.push(TextDraw {
                pos: pos2(center.x, inner.min.y + 1.0),
                text: key_name(note.key, flags.show_octaves, flags.flats),
                size: small,
                color: th.note_text,
                align: Align2::CENTER_TOP,
            });
        }
        if flags.show_numbers {
            let value = match state.tool {
                EditMode::Key => format!("{:+}", key_offset(note.key)),
                EditMode::Velocity => format!("{}", note.velocity),
                EditMode::Panning => {
                    let p = note.panning as i32 - PAN_CENTER as i32;
                    if p == 0 {
                        "C".into()
                    } else if p < 0 {
                        format!("L{}", -p)
                    } else {
                        format!("R{p}")
                    }
                }
                EditMode::Pitch => {
                    if note.pitch == 0 {
                        "0".into()
                    } else {
                        format!("{:+}", note.pitch / 100)
                    }
                }
            };
            texts.push(TextDraw {
                pos: pos2(center.x, center.y + cell * 0.10),
                text: value,
                size: big,
                color: th.note_text,
                align: Align2::CENTER_CENTER,
            });
        }
        if flags.use_icons && cell >= 28.0 {
            let short = nbs_core::instruments::instrument(note.instrument)
                .map(|i| i.short)
                .unwrap_or("CU");
            texts.push(TextDraw {
                pos: pos2(inner.min.x + 2.0, inner.max.y - 1.0),
                text: short.to_string(),
                size: small * 0.85,
                color: alpha(th.note_text, 210),
                align: Align2::LEFT_BOTTOM,
            });
        }
    }
}

// ---------------------------------------------------------------- 右键菜单

fn draw_context_menu(state: &mut EditorState, ui: &Ui) {
    let Some(menu) = state.context_menu.clone() else {
        return;
    };
    let mut cmds: Vec<Command> = Vec::new();
    let mut close = false;

    let area = egui::Area::new(egui::Id::new("grid_context_menu"))
        .fixed_pos(menu.pos)
        .order(egui::Order::Foreground)
        .interactable(true);
    area.show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(190.0);
            let item = |ui: &mut Ui, label: &str| -> bool { ui.button(label).clicked() };
            match menu.kind {
                ContextKind::Grid => {
                    if item(ui, "复制  Ctrl+C") { cmds.push(Command::Copy); close = true; }
                    if item(ui, "剪切  Ctrl+X") { cmds.push(Command::Cut); close = true; }
                    if item(ui, "粘贴  Ctrl+V") { cmds.push(Command::Paste); close = true; }
                    if item(ui, "删除  Delete") { cmds.push(Command::Delete); close = true; }
                    ui.separator();
                    if item(ui, "全选  Ctrl+A") { cmds.push(Command::SelectAll); close = true; }
                    if item(ui, "反选  Ctrl+I") { cmds.push(Command::InvertSelection); close = true; }
                    if item(ui, "取消选择") { cmds.push(Command::Deselect); close = true; }
                    ui.separator();
                    if item(ui, "升八度  +12") { cmds.push(Command::Transpose(12)); close = true; }
                    if item(ui, "降八度  -12") { cmds.push(Command::Transpose(-12)); close = true; }
                    if item(ui, "在此粘贴到此处") {
                        cmds.push(Command::Paste);
                        state.hover_cell = Some(menu.cell);
                        close = true;
                    }
                    ui.separator();
                    crate::toolbar::instrument_menus(state, ui);
                }
                ContextKind::Timeline => {
                    if item(ui, "跳到播放头") {
                        state.viewport.start_tick = state.marker().floor().max(0.0) as u32;
                        close = true;
                    }
                    if item(ui, "设为循环起点") {
                        state.doc.edit_meta("设置循环起点", |m| {
                            m.loop_start = menu.cell.0.min(u16::MAX as u32) as u16;
                            m.loop_enabled = true;
                        });
                        close = true;
                    }
                    if item(ui, "定位到此处") {
                        state.set_marker(menu.cell.0 as f64);
                        close = true;
                    }
                    if item(ui, "全选到此处") {
                        state.doc.select_rect(0, 0, menu.cell.0, state.doc.song.endb, false, false, true);
                        close = true;
                    }
                }
                ContextKind::Layer(layer) => {
                    if item(ui, "重命名图层") {
                        cmds.push(Command::ShowWindow(crate::state::WindowKind::LayerRename(layer)));
                        close = true;
                    }
                    if item(ui, "选中整层") { cmds.push(Command::SelectLayer(layer)); close = true; }
                    if item(ui, "在上方插入图层") { cmds.push(Command::InsertLayer(layer)); close = true; }
                    if item(ui, "在末尾添加图层") { cmds.push(Command::AddLayer); close = true; }
                    if item(ui, "上移图层") { cmds.push(Command::MoveLayerUp(layer)); close = true; }
                    if item(ui, "下移图层") { cmds.push(Command::MoveLayerDown(layer)); close = true; }
                    if item(ui, "删除图层") { cmds.push(Command::RemoveLayer(layer)); close = true; }
                }
            }
            ui.separator();
            if item(ui, "关闭菜单") { close = true; }
        });
    });

    for c in cmds {
        state.emit(c);
    }
    let menu_rect = Rect::from_min_size(menu.pos, egui::vec2(210.0, 360.0));
    let clicked_outside = ui.input(|i| {
        i.pointer.primary_pressed()
            && i.pointer
                .hover_pos()
                .is_some_and(|p| !menu_rect.contains(p))
    });
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if close || clicked_outside || escape {
        state.context_menu = None;
    }
}
