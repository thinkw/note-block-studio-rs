//! D 模块的核心：超大滚动网格画布。
//!
//! 每帧只遍历 `[start_tick, start_tick + 可见列数)` × `[start_layer, +可见行数)`，
//! 音符查询走 [`nbs_core::model::Song::notes_in_view`]（`BTreeMap::range` + colfirst/collast
//! 早退），因此打开几十万 tick 的工程也不会因为「遍历全部音符」而卡死。
//! 底色与网格线先攒进 `Vec<Shape>` 一次 `Painter::extend` 提交，音符随后直接贴在它们上面
//! （原版 `control_draw` 就是这个顺序：线在下、方块在上）。

use egui::{
    pos2, Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use nbs_core::model::{key_name, Note, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN, PAN_CENTER};

use crate::commands::Command;
use crate::icons::BlockSet;
use crate::interaction::{DragMode, InputSnapshot};
use crate::layers_panel::time_str;
use crate::state::{ContextKind, ContextMenu, EditMode, EditorState};
use crate::theme::{Theme, ThemeKind};
use crate::viewport::{SCROLLBAR_SIZE, TIMELINE_HEIGHT};

/// 原版标尺用的 `font_small`（fnt_small / fnt_wslui_small 的 size 都是 8）。
const RULER_FONT: FontId = FontId::proportional(8.0);
/// 原版标尺刻度线的 `draw_set_alpha(0.6)`。
const RULER_A: u8 = 153;

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// 原版 `c_red` / `c_blue` / `c_yellow`。
const C_RED: Color32 = Color32::from_rgb(255, 0, 0);
const C_BLUE: Color32 = Color32::from_rgb(0, 0, 255);
const C_YELLOW: Color32 = Color32::from_rgb(255, 255, 0);

/// 切分布局：时间轴 / 网格 / 水平滚动条 / 垂直滚动条。
///
/// 行区只取整行（原版 `totalrows = floor((rh - rhval) / 32)`），水平滚动条紧贴行区底
/// （原版 `y1 + totalrows * 32 + 34`），而不是贴着窗口底边——这样图层栏的宏条
/// （`y1 + 32`）和网格最后一行才会落在同一条线上。
fn split(area: Rect, cell: f32) -> (Rect, Rect, Rect, Rect) {
    let w = (area.width() - SCROLLBAR_SIZE).max(64.0);
    let h = (((area.height() - TIMELINE_HEIGHT - SCROLLBAR_SIZE) / cell).floor().max(1.0)) * cell;
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
    let cell = state.viewport.cell();
    let (timeline_rect, grid_rect, hbar_rect, vbar_rect) = split(area, cell);

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
    let idle = state.window.is_none() && state.context_menu.is_none();
    crate::layers_panel::macro_bar_over_grid(state, ui, area, idle);
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

    // 滚轮：Ctrl 缩放 / 悬停音符调属性 / 按 `mousewheel` 档位决定是平移、换音色还是换键。
    if pointer_in_grid && (input.scroll.x.abs() > 0.0 || input.scroll.y.abs() > 0.0) {
        let delta = input.scroll.y + input.scroll.x;
        if input.ctrl {
            state.viewport.zoom_by(if delta > 0.0 { 1.1 } else { 0.9 });
        } else {
            let d = if delta > 0.0 { 1 } else { -1 };
            // 原版条件 `exist && changepitch`：停在音符上且开了「滚轮改音符属性」才改那颗音。
            let note_cell = hover.filter(|(t, l)| {
                state.prefs.changepitch
                    && (state.doc.song.has(*t as u32, *l as u32)
                        || state.doc.selection.contains(*t, *l))
            });
            if let Some((t, l)) = note_cell {
                state.emit(Command::WheelOnNote {
                    tick: t as u32,
                    layer: l as u32,
                    delta: d,
                    shift: input.shift,
                });
            } else if state.prefs.mousewheel == 1 {
                // 原版 `mousewheel = 1`：滚轮换音色，此时滚轮不再平移视口。
                state.emit(Command::WheelInstrument(d));
            } else if state.prefs.mousewheel == 2 {
                state.emit(Command::WheelKey(d));
            } else {
                scroll_viewport(state, delta, input.shift);
            }
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

/// 原版 `control_draw.gml:1000-1037` 的两行标尺：
/// 第一行是按 `tempo` 自适应步长的时间标签（`time_str`，顶边 y1+2，下方 3px 短刻度），
/// 第二行是每 4 tick 一个的 tick 号（顶边 y1+17）和贴网格上沿的刻度线（y1+30..33，整 4 tick 加长到 27）。
fn draw_timeline(state: &EditorState, ui: &Ui, rect: Rect) {
    let p = ui.painter().with_clip_rect(rect);
    let th = &state.theme;
    let vp = &state.viewport;
    let cell = vp.cell();
    let meta = &state.doc.song.meta;
    let tempo = (meta.tempo.max(0.01)) as f64;
    let top = rect.min.y;
    let mut shapes = vec![Shape::rect_filled(rect, 0.0, th.ruler_bg)];

    // 原版 `c`：一秒一格，太慢就 5/10/30 秒一格，太快就半秒 / 四分之一秒一格。
    let mut c = 1.0f64;
    if tempo < 3.5 {
        c = 5.0;
    }
    if tempo < 1.0 {
        c = 10.0;
    }
    if tempo < 0.5 {
        c = 30.0;
    }
    if tempo > 11.0 {
        c = 0.5;
    }
    if tempo > 18.0 {
        c = 0.25;
    }
    let ticks_per_step = tempo * c;
    let px_per_step = (ticks_per_step * cell as f64) as f32;
    let starta = vp.start_tick as f64;
    let whole = (starta / ticks_per_step).ceil();
    let x0 = vp.tick_x(vp.start_tick as i64);
    let mut sec = whole * c;
    let mut xx = (whole - starta / ticks_per_step) as f32 * px_per_step;
    let mut first = true;
    while xx < rect.width() + 16.0 {
        let x = x0 + xx;
        p.text(
            pos2(x, top),
            if first { Align2::LEFT_TOP } else { Align2::CENTER_TOP },
            time_str(sec),
            RULER_FONT,
            th.text,
        );
        first = false;
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(pos2(x, top + 12.0), Vec2::new(1.0, 3.0)),
            0.0,
            alpha(th.text, RULER_A),
        ));
        xx += px_per_step;
        sec += c;
    }

    // 原版 `for (a = 0; a <= totalcols)`：刻度线 + 每 4 tick 一个的 tick 号。
    for a in 0..=vp.columns() {
        let tick = vp.start_tick + a;
        let x = vp.tick_x(tick as i64);
        if x < rect.min.x - 1.0 || x > rect.max.x + 1.0 {
            continue;
        }
        let by4 = tick % 4 == 0;
        let h = if by4 { 7.0 } else { 4.0 };
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(pos2(x, top + 31.0 - h), Vec2::new(1.0, h)),
            0.0,
            alpha(th.text, RULER_A),
        ));
        if by4 {
            p.text(
                pos2(x, top + 15.0),
                if a == 0 { Align2::LEFT_TOP } else { Align2::CENTER_TOP },
                tick.to_string(),
                RULER_FONT,
                th.text,
            );
        }
    }
    p.extend(shapes);

    // 播放头：原版在时间轴顶端画 spr_marker 的箭头帧，细身帧一路拉满整个网格；
    // 精灵 xorigin = 8，贴图横坐标就是线所在的 x。网格那半截见 draw_grid。
    let mx = vp.tick_x(state.marker().floor() as i64)
        + (state.marker().fract() as f32) * cell;
    if let Some(bank) = state.icons.bank() {
        if mx >= rect.min.x - 8.0 && mx <= rect.max.x + 8.0 {
            let tint = if th.kind == ThemeKind::Fluent {
                Theme::accent_at(3)
            } else {
                Color32::WHITE
            };
            let k = cell / 32.0;
            let at = pos2(mx, rect.min.y + 2.0);
            let head = Vec2::new(16.0 * k, 15.0 * k);
            bank.marker(&p, at, marker_frame(th, state.flags.blackout), head, tint);
            bank.marker(
                &p,
                at,
                marker_frame(th, state.flags.blackout) + 1,
                Vec2::new(16.0 * k, rect.max.y - at.y),
                tint,
            );
        }
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

    // 背景
    shapes.push(Shape::rect_filled(
        rect,
        0.0,
        if blackout { Color32::BLACK } else { th.grid_bg },
    ));

    // 竖线：原版只有三种 —— 小节线（2px、alpha 0.3）、整 4 tick（1px、0.25）、其余（1px、0.1），
    // 颜色一律是黑。原版没有横线。
    if state.flags.show_grid_lines && !blackout {
        for tick in t0..t1 {
            let x = vp.tick_x(tick as i64);
            if x < rect.min.x - 1.0 || x > rect.max.x + 1.0 {
                continue;
            }
            let is_bar = tick % ts == 0;
            let is_beat = tick % 4 == 0;
            let (w, c) = if is_bar {
                (2.0, th.grid_bar)
            } else if is_beat {
                (1.0, th.grid_beat)
            } else {
                (1.0, th.grid_line)
            };
            shapes.push(Shape::rect_filled(
                Rect::from_min_size(pos2(x, rect.min.y), Vec2::new(w, rect.height())),
                0.0,
                c,
            ));
        }
    }

    // 原版顺序：底色与网格线 → 音符 → 浮动选区 → 锁定层蒙版 → 播放头。
    p.extend(shapes);

    // 拖框：原版 select = 1（左键加选）时才给框内音符加亮，select = 2（右键减选）什么都不画。
    let mut box_sel: Option<(i64, i64, i64, i64)> = None;
    let mut box_subtract = false;
    if let DragMode::BoxSelect { anchor, subtract, .. } = &state.interaction.mode {
        if let Some(hover) = state.hover_cell {
            let end = (hover.0 as i64, hover.1 as i64);
            box_sel = Some((
                anchor.0.min(end.0),
                anchor.0.max(end.0),
                anchor.1.min(end.1),
                anchor.1.max(end.1),
            ));
            box_subtract = *subtract;
        }
    }
    let inside_box = |tick: i64, layer: i64| {
        box_sel.map_or(false, |r| {
            tick >= r.0 && tick <= r.1 && layer >= r.2 && layer <= r.3
        })
    };
    let hover = state.hover_cell.map(|(t, l)| (t as i64, l as i64));

    // 音符：唯一的查询入口，保证只处理可见区域
    let notes = state.doc.song.notes_in_view(t0, t1, l0, l1);
    let visible_count = notes.len();
    for (tick, layer, note) in notes {
        let (tick, layer) = (tick as i64, layer as i64);
        // 原版 `c = 0.5 * (vel / 100) + 0.25`，鼠标压着（select = 0）或落在加选框内再 +0.5。
        let in_box = !box_subtract && inside_box(tick, layer);
        let hovered = box_sel.is_none() && hover == Some((tick, layer));
        let mut c = 0.25 + 0.5 * (note.velocity as f32 / 100.0);
        if in_box || hovered {
            c += 0.5;
        }
        let at = vp.cell_rect(tick, layer).min;
        push_note(state, &p, at, note, c, if in_box { 0.8 } else { 0.0 });
    }

    // 浮动选区：同样先做整体视口剔除，再逐音符裁剪。
    // 原版 selection_draw 用 alpha = 1、salpha = 0.8，鼠标压住那格降到 0.7，减选框内降到 0.5。
    let sel = state.doc.selection.clone();
    if !sel.is_empty() {
        if let Some((bx0, by0, bx1, by1)) = sel.bounds() {
            if vp.rect_intersects(bx0, by0, bx1, by1) {
                for n in &sel.notes {
                    let at = sel.x + n.dx as i64;
                    let al = sel.y + n.dy as i64;
                    if at < t0 as i64 - 1 || at > t1 as i64 || al < l0 as i64 - 1 || al > l1 as i64 {
                        continue;
                    }
                    let mut salpha = 0.8_f32;
                    if hover == Some((at, al)) {
                        salpha = 0.7;
                    }
                    if box_subtract && inside_box(at, al) {
                        salpha = 0.5;
                    }
                    let pos = vp.cell_rect(at, al).min;
                    push_note(state, &p, pos, n.note, 1.0, salpha);
                }
            }
        }
    }
    state.interaction.visible_notes = visible_count + sel.len();

    // 锁定层：原版把 32×32 的 spr_lock（整张纯黑）横向拉满整行，以 0.25 不透明度压在音符上面。
    for layer in l0..l1 {
        if locked.get(layer as usize).copied().unwrap_or(false) {
            let y = vp.layer_y(layer as i64);
            p.rect_filled(
                Rect::from_min_size(pos2(rect.min.x, y), Vec2::new(rect.width(), cell)),
                0.0,
                alpha(Color32::BLACK, 64),
            );
        }
    }

    // 播放头：原版 spr_marker 第 0 帧是箭头、第 1 帧是纵向拉满的细身（箭头画在时间轴那一格，
    // 见 draw_timeline），精灵 xorigin = 8，所以贴图横坐标就是播放头所在的 x。
    if let Some(bank) = state.icons.bank() {
        let mx = vp.tick_x(state.marker().floor() as i64)
            + (state.marker().fract() as f32) * cell;
        if mx >= rect.min.x && mx <= rect.max.x {
            let tint = if th.kind == ThemeKind::Fluent {
                Theme::accent_at(3)
            } else {
                Color32::WHITE
            };
            bank.marker(
                &p,
                pos2(mx, rect.min.y),
                marker_frame(th, blackout) + 1,
                Vec2::new(16.0 * cell / 32.0, rect.height()),
                tint,
            );
        }
    }
}

/// 原版 `0 + 6 * (theme = 2 || (blackout && theme != 3)) + 8 * (theme = 3)`。
fn marker_frame(th: &Theme, blackout: bool) -> usize {
    if th.kind == ThemeKind::Fluent {
        8
    } else if th.kind == ThemeKind::Dark || blackout {
        6
    } else {
        0
    }
}

/// 原版 `draw_block(x, y, ins, key, pan, vel, pit, alpha, selalpha)`：
/// 方块精灵 → 乐器图标 → 音高偏移蒙版 → 不兼容红框 → 选中叠层 → 数值 / 键名两行文字。
/// 精灵库缺失时整格不画（原版没有「无贴图的音符」）。
/// 原版方块固定 32px，缩放时所有偏移按 `k = cell / 32` 等比。
fn push_note(
    state: &EditorState,
    p: &Painter,
    at: Pos2,
    note: Note,
    block_alpha: f32,
    salpha: f32,
) {
    let Some(bank) = state.icons.bank() else { return };
    let th = &state.theme;
    let flags = &state.flags;
    let cell = state.viewport.cell();
    let k = cell / 32.0;
    let size = Vec2::splat(cell);
    let index = note.instrument as usize;
    let first_custom = FIRST_CUSTOM_INDEX as usize;
    let iscustom = index >= first_custom;
    let set = match (flags.use_colors, flags.use_shapes) {
        (true, true) => BlockSet::ColorShape,
        (true, false) => BlockSet::Color,
        (false, true) => BlockSet::Shape,
        (false, false) => BlockSet::Plain,
    };
    // 原版：`!use_colors && !use_shapes` 时帧号恒为 1（spr_block 只有一帧）；
    // 自定义乐器按 `(index - first_custom) % 18 + first_custom` 循环取帧。
    let frame = if matches!(set, BlockSet::Plain) {
        1
    } else if iscustom {
        (index - first_custom) % 18 + first_custom
    } else {
        index
    };
    bank.block(p, at, set, frame, size, block_alpha);

    if flags.use_icons {
        let icon_at = at + Vec2::splat(16.0 * k);
        let icon_size = Vec2::splat(16.0 * k);
        if iscustom {
            bank.custom_block_glyph(p, icon_at, index - first_custom, first_custom, icon_size, block_alpha);
        } else {
            bank.instrument_glyph(p, icon_at, index, icon_size, block_alpha);
        }
    }

    // 音高偏移蒙版：负 = c_red，正 = c_blue，透明度 min(0.5, |pit| / 100 * alpha * 0.25)。
    if note.pitch != 0 {
        let a = ((note.pitch.abs() as f32 / 100.0) * block_alpha * 0.25).min(0.5);
        let c = if note.pitch < 0 { C_RED } else { C_BLUE };
        p.rect_filled(Rect::from_min_size(at, size), 0.0, alpha(c, (a * 255.0) as u8));
    }

    // 不兼容：自定义乐器、或超出 MC 音域的键，画一圈红框（原版 draw_rectangle(..., 1)）。
    if flags.show_incompatible
        && (iscustom || note.key < MC_KEY_MIN || note.key > MC_KEY_MAX)
    {
        let r = Rect::from_min_max(at + Vec2::splat(k), at + Vec2::splat(32.0 * k));
        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, C_RED), StrokeKind::Inside);
    }

    // 选中叠层用完后文字恢复不透明；否则文字比方块本身亮 0.25（原版 draw_set_alpha(alpha + 0.25)）。
    if salpha > 0.0 {
        bank.block_sel(p, at, size, salpha);
    }
    let text_alpha = if salpha > 0.0 { 1.0 } else { (block_alpha + 0.25).min(1.0) };
    let font = FontId::proportional(9.0 * k);

    if flags.show_numbers {
        // 原版 `xx + 16 - 8 * use_icons`：勾了图标位就把数值往左挪半格。
        let x = at.x + (if flags.use_icons { 8.0 } else { 16.0 }) * k;
        p.text(
            pos2(x, at.y + 16.0 * k),
            Align2::CENTER_TOP,
            note_value(state.tool, note),
            font.clone(),
            alpha(Color32::WHITE, (text_alpha * 255.0) as u8),
        );
    }
    // 键名不受 show_keynames 开关控制（那个开关只管琴键），原版恒画。
    let name_color = if th.kind == ThemeKind::Dark { Color32::WHITE } else { C_YELLOW };
    p.text(
        pos2(at.x + 16.0 * k, at.y + 4.0 * k),
        Align2::CENTER_TOP,
        key_name(note.key, flags.show_octaves, flags.flats),
        font,
        alpha(name_color, (text_alpha * 255.0) as u8),
    );
}

/// 原版 `draw_block` 里按 `editmode` 选的那串数字。
fn note_value(tool: EditMode, note: Note) -> String {
    let mut s = String::new();
    match tool {
        EditMode::Key => {
            if note.key < MC_KEY_MIN {
                s.push_str(" < ");
            }
            if note.key > MC_KEY_MAX {
                s.push_str(" > ");
            }
            if note.key >= MC_KEY_MIN && note.key <= MC_KEY_MAX {
                s.push_str(&(note.key as i32 - MC_KEY_MIN as i32).to_string());
            }
        }
        EditMode::Velocity => s.push_str(&note.velocity.to_string()),
        EditMode::Panning => {
            let p = note.panning as i32 - PAN_CENTER as i32;
            if p < 0 {
                s.push_str("L ");
            } else if p > 0 {
                s.push_str("R ");
            }
            s.push_str(&p.abs().to_string());
        }
        EditMode::Pitch => {
            if note.pitch > 0 {
                s.push('+');
            }
            s.push_str(&note.pitch.to_string());
        }
    }
    s
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
    let area_response = area.show(ui.ctx(), |ui| {
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
                ContextKind::Tempo => {
                    let bpm_on = state.prefs.use_bpm;
                    if ui.selectable_label(!bpm_on, "红石刻/秒 (t/s)").clicked() {
                        state.prefs.use_bpm = false;
                        cmds.push(Command::SavePreferences);
                        close = true;
                    }
                    if ui.selectable_label(bpm_on, "拍数/分钟 (BPM)").clicked() {
                        state.prefs.use_bpm = true;
                        cmds.push(Command::SavePreferences);
                        close = true;
                    }
                    ui.separator();
                    // 原版末尾还有一项「速度测量器」，我们没有打点测速功能，就不画它。
                    for t in [10.0_f32, 12.0, 14.0, 16.0, 18.0, 20.0, 30.0, 60.0] {
                        let label = if bpm_on { format!("{:.0} BPM", t * 15.0) } else { format!("{t:.0} t/s") };
                        if item(ui, &label) {
                            cmds.push(Command::SetTempo(t));
                            close = true;
                        }
                    }
                }
            }
            ui.separator();
            if item(ui, "关闭菜单") { close = true; }
        });
    });

    for c in cmds {
        state.emit(c);
    }
    // 判定「点到菜单外」要用菜单自己的实际尺寸：各分支菜单高度不同。
    let menu_rect = area_response.response.rect;
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
