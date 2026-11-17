//! D 模块的核心：超大滚动网格画布。
//!
//! 每帧只遍历 `[start_tick, start_tick + 可见列数)` × `[start_layer, +可见行数)`，
//! 音符查询走 [`nbs_core::model::Song::notes_in_view`]（`BTreeMap::range` + colfirst/collast
//! 早退），因此打开几十万 tick 的工程也不会因为「遍历全部音符」而卡死。
//! 底色与网格线先攒进 `Vec<Shape>` 一次 `Painter::extend` 提交，音符随后直接贴在它们上面
//! （原版 `control_draw` 就是这个顺序：线在下、方块在上）。

use egui::{
    pos2, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2,
};
use nbs_core::model::{
    key_name, panning_velocity_to_short, Note, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN,
    PAN_CENTER, SOUND_STOPPER_NAME, TEMPO_CHANGER_NAME,
};

use crate::commands::Command;
use crate::icons::BlockSet;
use crate::interaction::{DragMode, InputSnapshot};
use crate::layers_panel::time_str;
use crate::state::{ContextKind, ContextMenu, EditMode, EditorState};
use crate::theme::{Theme, ThemeKind};
use crate::tr;
use crate::viewport::{Viewport, SCROLLBAR_SIZE, TIMELINE_HEIGHT};

/// 原版标尺用的 `font_small`（fnt_small / fnt_wslui_small 的 size 都是 8）。
const RULER_FONT: FontId = FontId::proportional(8.0);
/// 原版 `font_main`，音符数值和标尺上的 BPM 盒都是它。
const MAIN_FONT: FontId = FontId::proportional(9.0);
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
    let h = (((area.height() - TIMELINE_HEIGHT - SCROLLBAR_SIZE) / cell)
        .floor()
        .max(1.0))
        * cell;
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

/// 原版标尺/播放头共用的取列式：`starta + (mouse_x - (x1 + 2)) / 32`
/// （`control_draw.gml:171/175/177/1103/1113`）。不带 `note_offset` —— 那一项只在
/// 播放跟随到「按帧」时才非零，而这一整段命中判定要求停播（`:124`）。
fn ruler_tick(vp: &Viewport, x: f32) -> f64 {
    vp.start_tick as f64 + (x - vp.origin.x) as f64 / vp.cell() as f64
}

/// 原版 `x1 + 2 + floor(marker_pos * 32 - starta * 32)`：播放头那条线所在的像素 x，
/// 也是 `spr_marker` 传给 `draw_sprite` 的横坐标（贴图原点就在线上）。
fn marker_x(state: &EditorState) -> f32 {
    let vp = &state.viewport;
    let m = state.marker().max(0.0);
    vp.tick_x(m.floor() as i64) + (m.fract() as f32) * vp.cell()
}

/// 原版 `control_draw.gml:1122-1127`：区间两端每帧夹进 `[0, enda + totalcols]`，
/// 夹完重合（且当前不在拖边界）就当作没有区间。
fn clamp_section(state: &mut EditorState) {
    if !state.section_exists {
        return;
    }
    let totalcols = (state.viewport.size.x / state.viewport.cell()).floor() as i64;
    let bound = state.doc.song.enda as i64 + totalcols;
    state.section_start = state.section_start.clamp(0, bound);
    state.section_end = state.section_end.clamp(0, bound);
    let dragging = matches!(
        state.interaction.mode,
        DragMode::DragSectionStart | DragMode::DragSectionEnd
    );
    if state.section_end == state.section_start && !dragging {
        state.section_exists = false;
    }
}

fn handle_timeline_input(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    // 每帧先夹取，和鼠标在哪、开没开窗都无关（原版这一段在 `if (!isplayer)` 里、
    // 命中判定之外，`control_draw.gml:1122`）。
    clamp_section(state);

    // ---- 进行中的拖拽：原版 `:1056-1120` 只认「左键还按着没」，鼠标出格也继续跟。
    let mode = state.interaction.mode.clone();
    if let Some(pos) = input.pointer.filter(|_| mode != DragMode::None) {
        let tick = ruler_tick(&state.viewport, pos.x);
        match mode {
            DragMode::DragMarker => {
                if input.primary_down {
                    state.set_marker(tick);
                    state.interaction.cursor = CursorIcon::ResizeHorizontal;
                } else {
                    state.interaction.reset();
                }
            }
            DragMode::PressTimeline { pressa } => {
                if !input.primary_down {
                    if input.primary_released {
                        // 原版 `:173-175`：没横向拖开就在原地松手 → 播放头跳到这一点（带小数）。
                        state.set_marker(tick);
                    }
                    state.interaction.reset();
                } else if tick.floor() as i64 != pressa {
                    // 原版 `:177-183`：一拖开就地建一段，右端接着跟鼠标，之后交给
                    // `w_dragsection_end`。
                    state.section_exists = true;
                    state.section_start = pressa;
                    state.section_end = tick.floor() as i64;
                    state.interaction.mode = DragMode::DragSectionEnd;
                }
            }
            DragMode::DragSectionStart | DragMode::DragSectionEnd => {
                if input.primary_down {
                    // 原版 `:1103/1113`：拖边界用 `round`，比新建区间时的 `floor` 进一步。
                    let t = tick.round() as i64;
                    if matches!(mode, DragMode::DragSectionStart) {
                        state.section_start = t;
                    } else {
                        state.section_end = t;
                    }
                } else {
                    // 原版 `:1105-1118`：松手时两端重合就删掉，否则把两端排好序。
                    if state.section_end == state.section_start {
                        state.section_exists = false;
                    }
                    let (a, b) = (state.section_start, state.section_end);
                    state.section_start = a.min(b);
                    state.section_end = a.max(b);
                    state.interaction.reset();
                }
            }
            _ => {}
        }
    }

    // ---- 起手判定：原版 `:124` 的总闸 —— 播放中、有窗口、已有别的拖拽状态都不响应。
    // `window = 0` 在原版里同时管着「没开窗」和「没在拖别的」（`window` 一个变量两用）。
    let Some(pos) = input.pointer.filter(|p| rect.contains(*p)) else {
        return;
    };
    if state.window.is_some() || state.is_playing() || state.interaction.mode != DragMode::None {
        return;
    }
    // 原版 `:132/141/148`：三个 16px 命中框，播放头优先、区间两端让位（`cursmarker`）。
    // 原版的高度是整列（`totalrows * 32 + 32`），端口只做标尺带那 32px，
    // 网格里的音符编辑因此不会被播放头/区间手柄抢走。
    let handles = [
        (marker_x(state), DragMode::DragMarker),
        (
            state.viewport.tick_x(state.section_start),
            DragMode::DragSectionStart,
        ),
        (
            state.viewport.tick_x(state.section_end),
            DragMode::DragSectionEnd,
        ),
    ];
    let mut on_handle = false;
    for (i, (x, mode)) in handles.iter().enumerate() {
        if i > 0 && !state.section_exists {
            continue;
        }
        if (pos.x - x).abs() < 8.0 {
            state.interaction.cursor = CursorIcon::ResizeHorizontal;
            // 原版 `:132/141/148` 的命中框只吃左键（按下即 `window = w_drag*`）；
            // 右键照样落到下面的区间菜单（`:156` 那道 `window = 0` 仍然成立）。
            if input.primary_pressed {
                state.interaction.mode = mode.clone();
                return;
            }
            on_handle = true;
            break;
        }
    }
    if input.secondary_pressed {
        state.context_menu = Some(ContextMenu {
            pos,
            cell: (state.viewport.x_tick(pos.x).max(0) as u32, 0),
            kind: ContextKind::Timeline,
        });
        return;
    }
    if input.primary_pressed && !on_handle {
        // 原版 `:171`：按下只记下那一列（`timeline_pressa`），松手或拖开才有下文。
        state.interaction.mode = DragMode::PressTimeline {
            pressa: ruler_tick(&state.viewport, pos.x).floor() as i64,
        };
    }
}

fn handle_grid_input(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    state.interaction.click_in_grid = false;

    let pointer_cell = input
        .pointer
        .filter(|p| rect.contains(*p))
        .and_then(|p| state.viewport.cell_at(p));
    state.hover_cell = pointer_cell.map(|(t, l)| (t, l));
    let hover = pointer_cell.map(|(t, l)| (t as i64, l as i64));

    let interactive = hover.is_some() && state.window.is_none() && state.context_menu.is_none();
    let pointer_in_grid = input.pointer.is_some_and(|p| rect.contains(p));
    // 光标只在指针真的进了网格时才收回默认：标尺上那三个 16px 手柄的
    // `cr_size_we` 是 `handle_timeline_input` 先设的（原版 `control_draw.gml:134/144/151`），
    // 无条件重置会让手柄永远画不出拉伸光标。
    if pointer_in_grid {
        state.interaction.cursor = CursorIcon::Default;
    }

    // 滚轮：悬停音符调属性 / 按 `mousewheel` 档位决定是平移、换音色还是换键。
    // Ctrl+滚轮不在这里处理——原版把它归在键盘快捷键段（`control_draw.gml:736-751`），
    // 改的是整窗口缩放，见 `shortcuts.rs`。
    if pointer_in_grid && !input.ctrl && (input.scroll.x.abs() > 0.0 || input.scroll.y.abs() > 0.0)
    {
        let delta = input.scroll.y + input.scroll.x;
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
        DragMode::PanView {
            start,
            start_tick,
            start_layer,
        } => {
            if let Some(pos) = input.pointer {
                let c = state.viewport.cell();
                state.viewport.start_tick =
                    (start_tick as f64 - (pos.x - start.x) as f64 / c as f64).max(0.0) as u32;
                state.viewport.start_layer =
                    (start_layer as f64 - (pos.y - start.y) as f64 / c as f64).max(0.0) as u32;
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

/// 原版 `draw_scrollbar(…, mousewheel_scroll_speed)`：滚轮一格走 `speed` 个槽位，
/// 横向一个槽 = 1 刻、纵向一个槽 = 1 层（`sb_val += 滚轮格数 * wheel_speed`）。
/// Shift 走横向（原版把它当作「另一根滚动条」），此前这里硬编码 4 刻一格，
/// 与原版 speed=1 时的一刻一格不符，改成走设置值。
fn scroll_viewport(state: &mut EditorState, delta: f32, shift: bool) {
    let step = if delta > 0.0 { 1 } else { -1 };
    let speed = state.prefs.mousewheel_scroll_speed.clamp(1, 10) as i64;
    if shift {
        let s = state.viewport.start_tick as i64 + step * speed;
        state.viewport.start_tick = s.max(0) as u32;
    } else {
        let s = state.viewport.start_layer as i64 + step * speed;
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
            DragMode::BoxSelect {
                anchor,
                subtract,
                alt_copy,
            } => {
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
                                x0: t,
                                y0: l,
                                x1: t,
                                y1: l,
                                copy: false,
                                additive: false,
                                subtract: true,
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
    let thumb_w = ((visible / max_units) * rect.width())
        .max(24.0)
        .min(rect.width());
    let thumb_x = rect.min.x
        + (state.viewport.start_tick as f32 / max_units.max(1.0)) * (rect.width() - thumb_w);
    (
        max_units,
        thumb_w,
        Rect::from_min_size(
            pos2(thumb_x, rect.min.y + 2.0),
            Vec2::new(thumb_w, rect.height() - 4.0),
        ),
    )
}

fn v_geometry(state: &EditorState, rect: Rect) -> (f32, f32, Rect) {
    let visible = state.viewport.rows().max(1) as f32;
    let max_units =
        (state.doc.song.layer_count().max(state.viewport.start_layer) as f32 + visible + 4.0)
            .max(visible * 2.0);
    let thumb_h = ((visible / max_units) * rect.height())
        .max(24.0)
        .min(rect.height());
    let thumb_y = rect.min.y
        + (state.viewport.start_layer as f32 / max_units.max(1.0)) * (rect.height() - thumb_h);
    (
        max_units,
        thumb_h,
        Rect::from_min_size(
            pos2(rect.min.x + 2.0, thumb_y),
            Vec2::new(rect.width() - 4.0, thumb_h),
        ),
    )
}

fn handle_scrollbar_input(state: &mut EditorState, input: &InputSnapshot, hbar: Rect, vbar: Rect) {
    let Some(pos) = input.pointer else { return };
    let (hm, _, ht) = h_geometry(state, hbar);
    let (vm, _, vt) = v_geometry(state, vbar);

    if input.primary_pressed {
        if hbar.contains(pos) {
            if ht.contains(pos) {
                state.interaction.mode = DragMode::DragScrollbarH {
                    grab: pos.x - ht.min.x,
                };
            } else {
                let ratio = ((pos.x - hbar.min.x) / hbar.width()).clamp(0.0, 1.0);
                state.viewport.start_tick = (ratio * hm) as u32;
            }
        } else if vbar.contains(pos) {
            if vt.contains(pos) {
                state.interaction.mode = DragMode::DragScrollbarV {
                    grab: pos.y - vt.min.y,
                };
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
    shapes.push(Shape::rect_stroke(
        ht,
        3.0,
        Stroke::new(1.0_f32, th.accent),
        StrokeKind::Inside,
    ));
    shapes.push(Shape::rect_filled(vt, 3.0, th.accent_soft));
    shapes.push(Shape::rect_stroke(
        vt,
        3.0,
        Stroke::new(1.0_f32, th.accent),
        StrokeKind::Inside,
    ));
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
            if first {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            },
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
                if a == 0 {
                    Align2::LEFT_TOP
                } else {
                    Align2::CENTER_TOP
                },
                tick.to_string(),
                RULER_FONT,
                th.text,
            );
        }
    }
    p.extend(shapes);

    // 原版 control_draw.gml:1147-1167：标尺上的「BPM = N」速度盒。队列是从最后一枚
    // （最靠下的）往回画的，后面的（更靠上的）过不了 `yy > last_tc_y`，
    // 所以每个 tick 列最终只留下最靠下的一枚 Tempo Changer。
    if let Some(bank) = state.icons.bank() {
        for a in 0..=vp.columns() {
            let tick = vp.start_tick + a;
            let Some(col) = state.doc.song.column(tick) else {
                continue;
            };
            let Some((_, n)) =
                col.notes.iter().rev().find(|(_, n)| {
                    state.doc.song.instrument_name(n.instrument) == TEMPO_CHANGER_NAME
                })
            else {
                continue;
            };
            let xx = vp.tick_x(tick as i64);
            let galley = p.layout_no_wrap(format!("BPM = {}", n.pitch.abs()), MAIN_FONT, th.text);
            let w = galley.size().x;
            let fill = if th.kind == ThemeKind::Dark {
                Color32::from_gray(54)
            } else if th.kind == ThemeKind::Fluent {
                if th.fdark {
                    Color32::from_gray(45)
                } else {
                    Color32::from_gray(252)
                }
            } else {
                Color32::WHITE
            };
            p.rect_filled(
                Rect::from_min_max(
                    pos2(xx + 10.0, rect.min.y + 4.0),
                    pos2(xx + 10.0 + w, rect.min.y + 16.0),
                ),
                0.0,
                fill,
            );
            bank.area(
                &p,
                Rect::from_min_max(
                    pos2(xx + 9.0, rect.min.y + 3.0),
                    pos2(xx + 11.0 + w, rect.min.y + 17.0),
                ),
                th,
            );
            let dark_marker = th.kind == ThemeKind::Dark
                || state.flags.blackout
                || (th.kind == ThemeKind::Fluent && th.fdark);
            let f = 8 * dark_marker as usize;
            let at = pos2(xx, rect.min.y + 2.0);
            bank.marker(&p, at, f, Vec2::new(16.0, 15.0), Color32::WHITE);
            bank.marker(
                &p,
                pos2(xx, rect.min.y + 17.0),
                f + 1,
                Vec2::new(16.0, 15.0),
                Color32::WHITE,
            );
            // 原版 `draw_text(xx + 10, y1 + 2 + (theme != 3), …)`，对齐当时是 fa_left。
            p.galley(
                pos2(
                    xx + 10.0,
                    rect.min.y
                        + if th.kind == ThemeKind::Fluent {
                            2.0
                        } else {
                            3.0
                        },
                ),
                galley,
                th.text,
            );
        }
    }

    // 时间轴区间：原版 `control_draw.gml:1128-1140`。0.25 不透明的纯蓝底加两端帽，
    // 帽和边界线共用 `spr_marker` 的 2..5 帧（区间反着拖时换成另一头的图形），
    // 线一路拉满网格的那半截在 `draw_grid` 里补，和播放头一样的拆法。
    if state.section_exists {
        let rev = (state.section_end < state.section_start) as usize;
        let x0 = vp.tick_x(state.section_start);
        let x1 = vp.tick_x(state.section_end);
        p.rect_filled(
            Rect::from_min_max(pos2(x0.min(x1), top), pos2(x0.max(x1), top + 31.0)),
            0.0,
            alpha(C_BLUE, 64),
        );
        if let Some(bank) = state.icons.bank() {
            for (x, cap) in [(x0, 2 + 2 * rev), (x1, 4 - 2 * rev)] {
                bank.marker(&p, pos2(x, top), cap, Vec2::new(16.0, 15.0), Color32::WHITE);
                bank.marker(
                    &p,
                    pos2(x, top),
                    cap + 1,
                    Vec2::new(16.0, rect.max.y - top),
                    Color32::WHITE,
                );
            }
        }
    }

    // 播放头：原版在时间轴顶端画 spr_marker 的箭头帧，细身帧一路拉满整个网格
    // （贴图原点在线上，见 `IconBank::marker`）。网格那半截见 draw_grid。
    let mx = marker_x(state);
    if let Some(bank) = state.icons.bank() {
        if mx >= rect.min.x - 8.0 && mx <= rect.max.x + 8.0 {
            let tint = if th.kind == ThemeKind::Fluent {
                th.accent(3)
            } else {
                Color32::WHITE
            };
            let at = pos2(mx, rect.min.y + 2.0);
            bank.marker(
                &p,
                at,
                marker_frame(th, state.flags.blackout),
                Vec2::new(16.0, 15.0),
                tint,
            );
            bank.marker(
                &p,
                at,
                marker_frame(th, state.flags.blackout) + 1,
                Vec2::new(16.0, rect.max.y - at.y),
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
    if let DragMode::BoxSelect {
        anchor, subtract, ..
    } = &state.interaction.mode
    {
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
        // 「音符盒不透明」(fade) 勾上时这两步都不做，`c` 恒为 1（`control_draw.gml:398-405`）。
        let in_box = !box_subtract && inside_box(tick, layer);
        let hovered = box_sel.is_none() && hover == Some((tick, layer));
        let mut c = if state.flags.fade {
            1.0
        } else {
            0.25 + 0.5 * (note.velocity as f32 / 100.0)
        };
        if !state.flags.fade && (in_box || hovered) {
            c += 0.5;
        }
        let at = vp.cell_rect(tick, layer).min;
        push_note(state, &p, at, note, c, if in_box { 0.8 } else { 0.0 });
    }

    // 消音器导引带：原版 `control_draw.gml:352-366` 也是逐列扫完整列（不限可见层），
    // 再交给 `draw_sound_stopper_guide` 裁到可见范围；闸门同样是「显示音符盒数值」。
    // 逐列画完再画下一列，等价于在这里一次性补上：带子只有 16px 宽，最多多压到
    // 前一列的右边缘 2px，而前一列的音符在原版里也是先画的。
    if state.flags.show_numbers {
        if let Some(bank) = state.icons.bank() {
            let dark = th.kind == ThemeKind::Dark || (th.kind == ThemeKind::Fluent && th.fdark);
            let visible_rows = (rect.height() / cell).floor() as i64;
            let visible_end = l0 as i64 + visible_rows - 1;
            for tick in t0..t1 {
                let Some(col) = state.doc.song.column(tick) else {
                    continue;
                };
                for (_, note) in &col.notes {
                    if state.doc.song.instrument_name(note.instrument) != SOUND_STOPPER_NAME {
                        continue;
                    }
                    // 原版存的是 1 基的层号（`song_pit = yy + 1`），我们内部一律 0 基。
                    let start = note.pitch.unsigned_abs() as i64 - 1;
                    let end = panning_velocity_to_short(note.panning, note.velocity) as i64 - 1;
                    let (ds, de) = (start.max(l0 as i64), end.min(visible_end));
                    if ds > de {
                        continue;
                    }
                    bank.stopper_wall(
                        &p,
                        vp.tick_x(tick as i64),
                        vp.layer_y(ds),
                        cell * (de - ds + 1) as f32,
                        dark,
                    );
                }
            }
        }
    }

    // 浮动选区：同样先做整体视口剔除，再逐音符裁剪。
    // 原版 `selection_draw.gml:79-99`：salpha 基值 = `selection_alpha`（首选项
    // 「选择高亮显示强度」，出厂 0.8）；悬停那格与拖动选区时乘 `0.72`（>0.4 时），
    // 减选框压住的那格固定 0.5*0.625。低强度（≤0.4）悬停时直接乘 0，照抄。
    let sel = state.doc.selection.clone();
    let sel_alpha = state.prefs.selection_alpha;
    if !sel.is_empty() {
        if let Some((bx0, by0, bx1, by1)) = sel.bounds() {
            if vp.rect_intersects(bx0, by0, bx1, by1) {
                for n in &sel.notes {
                    let at = sel.x + n.dx as i64;
                    let al = sel.y + n.dy as i64;
                    if at < t0 as i64 - 1 || at > t1 as i64 || al < l0 as i64 - 1 || al > l1 as i64
                    {
                        continue;
                    }
                    let mut salpha = sel_alpha;
                    if hover == Some((at, al)) {
                        salpha = if sel_alpha > 0.4 {
                            sel_alpha * 0.72
                        } else {
                            0.0
                        };
                    }
                    if box_subtract && inside_box(at, al) {
                        salpha = 0.3125;
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

    // 区间两条边界线在网格里的半截（原版 `control_draw.gml:1136/1139` 拉满
    // `(totalrows + 1) * 32 / 15` 高的那两笔）：音符之上、播放头之下，和原版同序。
    if state.section_exists {
        let rev = (state.section_end < state.section_start) as usize;
        if let Some(bank) = state.icons.bank() {
            for (x, cap) in [
                (vp.tick_x(state.section_start), 2 + 2 * rev),
                (vp.tick_x(state.section_end), 4 - 2 * rev),
            ] {
                bank.marker(
                    &p,
                    pos2(x, rect.min.y),
                    cap + 1,
                    Vec2::new(16.0, rect.height()),
                    Color32::WHITE,
                );
            }
        }
    }

    // 播放头：原版 spr_marker 第 0 帧是箭头、第 1 帧是纵向拉满的细身（箭头画在时间轴那一格，
    // 见 draw_timeline），贴图原点在线上（`IconBank::marker`）。
    if let Some(bank) = state.icons.bank() {
        let mx = marker_x(state);
        if mx >= rect.min.x && mx <= rect.max.x {
            let tint = if th.kind == ThemeKind::Fluent {
                th.accent(3)
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

    // 删除效果：原版 `obj_removeeff`（单独的 GML 对象，画在主界面之上）。
    // 基框 28×28（格子内缩 2px），随 alpha 淡出同时向外扩、圆角变大：
    // 左/上 −(0.5−alpha)*20、右 +(0.5−alpha)*18、下 +(0.5−alpha)*20、半径 (0.5−alpha)*20。
    // 颜色 accent[0]（非暗色）/ accent[6]（暗色），轮廓线宽 1。rel 不跟滚动（照抄原版对象）。
    for fx in &state.remove_effects {
        let a = fx.alpha();
        let e = 0.5 - a;
        let base = pos2(
            vp.origin.x + fx.rel.0 as f32 * cell,
            vp.origin.y + fx.rel.1 as f32 * cell,
        );
        let r = Rect::from_min_max(
            pos2(base.x + 2.0 - e * 20.0, base.y + 2.0 - e * 20.0),
            pos2(base.x + 30.0 + e * 18.0, base.y + 30.0 + e * 20.0),
        );
        let col = th.accent(if th.fdark { 6 } else { 0 });
        let stroke = Stroke::new(1.0_f32, alpha(col, (a * 255.0) as u8));
        let radius = CornerRadius::same((e * 20.0).round().max(0.0) as u8);
        p.rect_stroke(r, radius, stroke, StrokeKind::Inside);
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
/// 原版所有偏移都是写死的像素：方块 32、图标 16（左上角在 `xx + 16, yy + 16`）、
/// 数值在 `xx + 16 - 8 * use_icons, yy + 16`、键名在 `xx + 16, yy + 4`。
fn push_note(
    state: &EditorState,
    p: &Painter,
    at: Pos2,
    note: Note,
    block_alpha: f32,
    salpha: f32,
) {
    let Some(bank) = state.icons.bank() else {
        return;
    };
    let th = &state.theme;
    let flags = &state.flags;
    let size = Vec2::splat(state.viewport.cell());
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
        let icon_at = pos2(at.x + 16.0, at.y + 16.0);
        let icon_size = Vec2::splat(16.0);
        if iscustom {
            bank.custom_block_glyph(
                p,
                icon_at,
                index - first_custom,
                first_custom,
                state.doc.song.instrument_name(note.instrument),
                icon_size,
                block_alpha,
            );
        } else {
            bank.instrument_glyph(p, icon_at, index, icon_size, block_alpha);
        }
    }

    // 音高偏移蒙版：负 = c_red，正 = c_blue，透明度 min(0.5, |pit| / 100 * alpha * 0.25)。
    if note.pitch != 0 {
        let a = ((note.pitch.abs() as f32 / 100.0) * block_alpha * 0.25).min(0.5);
        let c = if note.pitch < 0 { C_RED } else { C_BLUE };
        p.rect_filled(
            Rect::from_min_size(at, size),
            0.0,
            alpha(c, (a * 255.0) as u8),
        );
    }

    // 不兼容：自定义乐器、或超出 MC 音域的键，画一圈红框（原版 draw_rectangle(..., 1)）。
    if flags.show_incompatible && (iscustom || note.key < MC_KEY_MIN || note.key > MC_KEY_MAX) {
        let r = Rect::from_min_max(pos2(at.x + 1.0, at.y + 1.0), pos2(at.x + 32.0, at.y + 32.0));
        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, C_RED), StrokeKind::Inside);
    }

    // 选中叠层用完后文字恢复不透明；否则文字比方块本身亮 0.25（原版 draw_set_alpha(alpha + 0.25)）。
    if salpha > 0.0 {
        bank.block_sel(p, at, size, salpha);
    }
    let text_alpha = if salpha > 0.0 {
        1.0
    } else {
        (block_alpha + 0.25).min(1.0)
    };
    let font = MAIN_FONT;

    if flags.show_numbers {
        // 原版 `xx + 16 - 8 * use_icons`：勾了图标位就把数值往左挪半格。
        let x = at.x + (if flags.use_icons { 8.0 } else { 16.0 });
        p.text(
            pos2(x, at.y + 16.0),
            Align2::CENTER_TOP,
            note_label(state, note),
            font.clone(),
            alpha(Color32::WHITE, (text_alpha * 255.0) as u8),
        );
    }
    // 键名不受 show_keynames 开关控制（那个开关只管琴键），原版恒画。
    let name_color = if th.kind == ThemeKind::Dark {
        Color32::WHITE
    } else {
        C_YELLOW
    };
    p.text(
        pos2(at.x + 16.0, at.y + 4.0),
        Align2::CENTER_TOP,
        key_name(note.key, flags.show_octaves, flags.flats),
        font,
        alpha(name_color, (text_alpha * 255.0) as u8),
    );
}

/// 原版 `draw_block` 的 `show_numbers` 分支：两个事件乐器各有专属文字，
/// 只有其余乐器才按 `editmode` 取那一串数字。
fn note_label(state: &EditorState, note: Note) -> String {
    let name = state.doc.song.instrument_name(note.instrument);
    if name == TEMPO_CHANGER_NAME {
        return format!("{} BPM", note.pitch.abs());
    }
    if name == SOUND_STOPPER_NAME {
        // 原版：起始层存在 pitch 里，结束层用声像 + 力度两个字节编码。
        let end = panning_velocity_to_short(note.panning, note.velocity);
        if note.pitch > 0 && end < note.pitch as u32 {
            // 原版 `condstr(language != 1, "Off", "停用")`。
            return tr("Off", "停用").to_string();
        }
        return format!("{}-{}", note.pitch.abs(), end);
    }
    note_value(state.tool, note)
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
                    if item(ui, tr("Copy  Ctrl+C", "复制  Ctrl+C")) {
                        cmds.push(Command::Copy);
                        close = true;
                    }
                    if item(ui, tr("Cut  Ctrl+X", "剪切  Ctrl+X")) {
                        cmds.push(Command::Cut);
                        close = true;
                    }
                    if item(ui, tr("Paste  Ctrl+V", "粘贴  Ctrl+V")) {
                        cmds.push(Command::Paste);
                        close = true;
                    }
                    if item(ui, tr("Delete  Delete", "删除  Delete")) {
                        cmds.push(Command::Delete);
                        close = true;
                    }
                    ui.separator();
                    if item(ui, tr("Select all  Ctrl+A", "全选  Ctrl+A")) {
                        cmds.push(Command::SelectAll);
                        close = true;
                    }
                    if item(ui, tr("Invert selection  Ctrl+I", "反选  Ctrl+I")) {
                        cmds.push(Command::InvertSelection);
                        close = true;
                    }
                    if item(ui, tr("Deselect all", "取消选择")) {
                        cmds.push(Command::Deselect);
                        close = true;
                    }
                    ui.separator();
                    if item(
                        ui,
                        &format!("{}  +12", tr("Transpose one octave up", "升八度")),
                    ) {
                        cmds.push(Command::Transpose(12));
                        close = true;
                    }
                    if item(
                        ui,
                        &format!("{}  -12", tr("Transpose one octave down", "降八度")),
                    ) {
                        cmds.push(Command::Transpose(-12));
                        close = true;
                    }
                    if item(ui, tr("Paste here", "在此粘贴到此处")) {
                        cmds.push(Command::Paste);
                        state.hover_cell = Some(menu.cell);
                        close = true;
                    }
                    ui.separator();
                    crate::toolbar::instrument_menus(state, ui);
                }
                ContextKind::Timeline => {
                    // 原版 `control_draw.gml:156-168` + `menu_click.gml:235-251`：
                    // 标尺右键菜单只有这六条，前四条没有区间就整条置灰。
                    let enda = state.doc.song.enda as i64;
                    let exists = state.section_exists;
                    let item_on = |ui: &mut Ui, label: &str, on: bool| -> bool {
                        ui.add_enabled(on, egui::Button::new(label)).clicked()
                    };
                    let tick = |t: i64| t.max(0) as u32;
                    if item_on(ui, tr("Remove section", "移除区间"), exists) {
                        state.clear_section();
                        close = true;
                    }
                    if item_on(
                        ui,
                        tr("Jump to beginning of section", "跳到区间开始"),
                        exists && state.section_start <= enda,
                    ) {
                        // 原版 `starta = section_start`（菜单项本身不管上下页，只管夹进 [0, enda]）。
                        state.viewport.start_tick = tick(state.section_start).min(enda as u32);
                        close = true;
                    }
                    if item_on(
                        ui,
                        tr("Jump to ending of section", "跳到区间结束"),
                        exists && state.section_end <= enda,
                    ) {
                        state.viewport.start_tick = tick(state.section_end - 1).min(enda as u32);
                        close = true;
                    }
                    ui.separator();
                    if item_on(
                        ui,
                        tr("Select all blocks in section", "选中区间内方块"),
                        exists,
                    ) {
                        // 原版 `selection_add(section_start, 0, section_end - 1, endb, 0, 0)`。
                        state.doc.select_rect(
                            tick(state.section_start),
                            0,
                            tick(state.section_end - 1),
                            state.doc.song.endb,
                            false,
                            false,
                            false,
                        );
                        close = true;
                    }
                    ui.separator();
                    // 原版 `show_menu_ext` 的 `check()` 前缀勾，等价于首选项「播放」页那两格。
                    let checked = |label: &str, on: bool| -> String {
                        if on {
                            format!("✓ {label}")
                        } else {
                            label.to_string()
                        }
                    };
                    let start_play = tr("Start playing in section", "从区间开始播放");
                    let stop_after = tr("Stop playing after section", "区间后结束播放");
                    if item(ui, &checked(start_play, state.prefs.marker_start)) {
                        state.prefs.marker_start = !state.prefs.marker_start;
                        state.emit(Command::SavePreferences);
                        close = true;
                    }
                    if item(ui, &checked(stop_after, state.prefs.marker_end)) {
                        state.prefs.marker_end = !state.prefs.marker_end;
                        state.emit(Command::SavePreferences);
                        close = true;
                    }
                }
                ContextKind::Layer(layer) => {
                    if item(ui, tr("Rename layer", "重命名图层")) {
                        cmds.push(Command::ShowWindow(crate::state::WindowKind::LayerRename(
                            layer,
                        )));
                        close = true;
                    }
                    if item(ui, tr("Select whole layer", "选中整层")) {
                        cmds.push(Command::SelectLayer(layer));
                        close = true;
                    }
                    if item(ui, tr("Insert layer above", "在上方插入图层")) {
                        cmds.push(Command::InsertLayer(layer));
                        close = true;
                    }
                    if item(ui, tr("Add layer at end", "在末尾添加图层")) {
                        cmds.push(Command::AddLayer);
                        close = true;
                    }
                    if item(ui, tr("Move layer up", "上移图层")) {
                        cmds.push(Command::MoveLayerUp(layer));
                        close = true;
                    }
                    if item(ui, tr("Move layer down", "下移图层")) {
                        cmds.push(Command::MoveLayerDown(layer));
                        close = true;
                    }
                    if item(ui, tr("Remove layer", "删除图层")) {
                        cmds.push(Command::RemoveLayer(layer));
                        close = true;
                    }
                }
                ContextKind::Tempo => {
                    let bpm_on = state.prefs.use_bpm;
                    if ui
                        .selectable_label(!bpm_on, tr("Ticks per second (t/s)", "红石刻/秒 (t/s)"))
                        .clicked()
                    {
                        state.prefs.use_bpm = false;
                        cmds.push(Command::SavePreferences);
                        close = true;
                    }
                    if ui
                        .selectable_label(bpm_on, tr("Beats per minute (BPM)", "拍数/分钟 (BPM)"))
                        .clicked()
                    {
                        state.prefs.use_bpm = true;
                        cmds.push(Command::SavePreferences);
                        close = true;
                    }
                    ui.separator();
                    for t in [10.0_f32, 12.0, 14.0, 16.0, 18.0, 20.0, 30.0, 60.0] {
                        let label = if bpm_on {
                            format!("{:.0} BPM", t * 15.0)
                        } else {
                            format!("{t:.0} t/s")
                        };
                        if item(ui, &label) {
                            cmds.push(Command::SetTempo(t));
                            close = true;
                        }
                    }
                    // 原版菜单最后一项，前面还有一条 `|-|` 分隔线
                    // （`control_draw.gml:2510` / `2521`，`menu_click` 的 `sel = 10`）：
                    // 打开速度测量器。这条入口不停播放。
                    ui.separator();
                    if item(ui, tr("Tempo tapper...", "速度测量器......")) {
                        cmds.push(Command::ShowWindow(crate::state::WindowKind::TempoTapper));
                        close = true;
                    }
                }
            }
            ui.separator();
            if item(ui, tr("Close menu", "关闭菜单")) {
                close = true;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 原版 `draw_scrollbar`：两根条每格都走 `wheel_speed` 个槽位，横向一个槽 = 1 刻、
    /// 纵向一个槽 = 1 层。方向（哪个符号算前进）沿用端口既有约定，这里只核每格走多远。
    #[test]
    fn wheel_scroll_steps_by_the_preference() {
        let _prefs = crate::isolate_prefs();
        let mut st = EditorState::new();
        let from = 20i64;
        st.prefs.mousewheel_scroll_speed = 1;

        st.viewport.start_layer = from as u32;
        scroll_viewport(&mut st, 1.0, false);
        let layers = st.viewport.start_layer as i64 - from;
        st.viewport.start_tick = from as u32;
        scroll_viewport(&mut st, 1.0, true);
        let ticks = st.viewport.start_tick as i64 - from;
        assert_eq!(layers.abs(), 1, "出厂纵向一格一层");
        assert_eq!(ticks.abs(), 1, "出厂横向一格一刻（原版槽位），不再是硬编码的 4");

        st.prefs.mousewheel_scroll_speed = 7;
        st.viewport.start_layer = from as u32;
        scroll_viewport(&mut st, 1.0, false);
        assert_eq!(
            (st.viewport.start_layer as i64 - from).abs(),
            7,
            "步长要按设置放大"
        );

        // 配置里的脏值按原版 median(1, …, 10) 夹住，回退方向也不能退到负列。
        let back = if layers > 0 { -1.0 } else { 1.0 };
        st.prefs.mousewheel_scroll_speed = 40;
        st.viewport.start_layer = 3;
        scroll_viewport(&mut st, back, false);
        assert_eq!(st.viewport.start_layer, 0, "越界夹到 0，脏值夹到上限 10");
    }

    // ---- 标尺状态机（区间 / 播放头手柄）------------------------------------

    /// 靶子谱：320×128 的网格 = 可见 10 列，空谱 `enda = 0`，
    /// 于是原版 `:1122` 的夹取上限正好是 `0 + 10 = 10`。
    fn ruler_state() -> EditorState {
        let mut st = EditorState::new();
        st.window = None;
        st.doc.replace_song(nbs_core::model::Song::new());
        st.viewport.origin = Pos2::ZERO;
        st.viewport.size = Vec2::new(320.0, 128.0);
        st.set_marker(0.0);
        st
    }

    fn ruler_rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, TIMELINE_HEIGHT))
    }

    /// 一帧鼠标，`y = 16` 恒在标尺带内；按键位由参数给（`pressed` 与 `down`
    /// 在同一帧是两件事，原版 `mouse_check_button(_pressed/_released)` 也是三个独立查询）。
    fn mouse(x: f32, pressed: bool, down: bool, released: bool) -> InputSnapshot {
        let p = pos2(x, 16.0);
        InputSnapshot {
            hover: Some(p),
            pointer: Some(p),
            primary_pressed: pressed,
            primary_down: down,
            primary_released: released,
            ..Default::default()
        }
    }

    /// 原版 `:171-175`：按下只记下那一列，原地松手才把播放头放到那「一点」上 ——
    /// 用不取整的 `starta + (mx - (x1 + 2)) / 32`，所以落在半刻就是半刻。
    #[test]
    fn a_still_press_on_the_ruler_jumps_the_playhead_to_that_point() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        handle_timeline_input(&mut st, &mouse(100.0, true, true, false), r);
        assert!(
            matches!(st.interaction.mode, DragMode::PressTimeline { pressa: 3 }),
            "按下只记列：{:?}",
            st.interaction.mode
        );
        assert_eq!(st.marker(), 0.0, "按下本身不该动播放头");
        handle_timeline_input(&mut st, &mouse(100.0, false, false, true), r);
        assert_eq!(st.marker(), 3.125, "100px ÷ 32 = 3.125 刻，取整就错了");
        assert_eq!(st.interaction.mode, DragMode::None);
        assert!(!st.section_exists, "没横向拖开就不该有区间");
    }

    /// 原版 `:177-183`：一拖开就地建段，右端用 `floor` 跟鼠标，随后转成
    /// 「拖右边界」，松手后区间留着。
    #[test]
    fn dragging_sideways_on_the_ruler_opens_a_section_and_keeps_it() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        handle_timeline_input(&mut st, &mouse(100.0, true, true, false), r);
        handle_timeline_input(&mut st, &mouse(260.0, false, true, false), r);
        assert!(st.section_exists);
        assert_eq!((st.section_start, st.section_end), (3, 8), "新建用 floor");
        assert_eq!(st.interaction.mode, DragMode::DragSectionEnd);
        handle_timeline_input(&mut st, &mouse(260.0, false, false, true), r);
        assert_eq!(st.interaction.mode, DragMode::None, "松手要收掉拖拽状态");
        assert!(st.section_exists, "两端不重合，区间留下");
        assert_eq!((st.section_start, st.section_end), (3, 8));
    }

    /// 原版 `:1103/1113` 用 `round`（比新建时的 `floor` 又进一步），且拖的过程中
    /// 不排序 —— `:1105-1118` 只在松手时排。
    #[test]
    fn dragging_a_boundary_rounds_and_sorts_only_on_release() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        st.section_exists = true;
        st.section_start = 3;
        st.section_end = 8;
        // 右端手柄在 tick_x(8) = 256，命中框 ±8px。
        handle_timeline_input(&mut st, &mouse(253.0, true, true, false), r);
        assert_eq!(st.interaction.mode, DragMode::DragSectionEnd);
        handle_timeline_input(&mut st, &mouse(74.0, false, true, false), r);
        assert_eq!(
            (st.section_start, st.section_end),
            (3, 2),
            "2.3125 刻 round 成 2，且此时还不排序"
        );
        handle_timeline_input(&mut st, &mouse(74.0, false, false, true), r);
        assert_eq!((st.section_start, st.section_end), (2, 3), "松手排序");
        assert!(st.section_exists);
    }

    /// 原版 `:1105-1110`：松手时两端重合 = 直接把这段区间删掉。
    #[test]
    fn releasing_with_both_ends_on_the_same_tick_drops_the_section() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        st.section_exists = true;
        st.section_start = 3;
        st.section_end = 8;
        handle_timeline_input(&mut st, &mouse(253.0, true, true, false), r);
        handle_timeline_input(&mut st, &mouse(96.0, false, true, false), r);
        assert_eq!(st.section_end, 3);
        handle_timeline_input(&mut st, &mouse(96.0, false, false, true), r);
        assert!(!st.section_exists, "两端重合就不该留一段零宽区间");
        assert_eq!(st.interaction.mode, DragMode::None);
    }

    /// 原版 `:1122-1127`：夹取在每帧开头跑，拖的时候鼠标说了算、夹完还会被覆写，
    /// 松手那一帧才留下夹好的值。空谱 `enda = 0` + 可见 10 列 → 上限 10。
    #[test]
    fn a_section_dragged_off_the_song_clamps_to_the_visible_page() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        handle_timeline_input(&mut st, &mouse(36.0, true, true, false), r);
        handle_timeline_input(&mut st, &mouse(964.0, false, true, false), r);
        assert_eq!(st.section_start, 1);
        assert_eq!(st.section_end, 30, "拖拽中按鼠标原值");
        handle_timeline_input(&mut st, &mouse(964.0, false, false, true), r);
        assert_eq!(st.section_end, 10, "夹进 enda + totalcols");
        assert!(st.section_exists);
    }

    /// 原版 `:132-151`：播放头手柄的 16px 命中框优先，按在上面不会落到「按下记列」；
    /// `:1056` 的拖拽只认左键还按着没，鼠标跑出标尺带也继续跟。
    #[test]
    fn the_playhead_handle_grabs_the_press_and_tracks_outside_the_ruler() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        st.set_marker(5.0);
        handle_timeline_input(&mut st, &mouse(164.0, true, true, false), r);
        assert_eq!(st.interaction.mode, DragMode::DragMarker);
        assert_eq!(st.marker(), 5.0, "按下本身不动播放头");
        handle_timeline_input(&mut st, &mouse(430.0, false, true, false), r);
        assert_eq!(st.marker(), 13.4375, "出格也要跟手");
        assert_eq!(st.interaction.cursor, CursorIcon::ResizeHorizontal);
        assert!(!st.section_exists, "抢到手柄就不该变成拖区间");
        handle_timeline_input(&mut st, &mouse(430.0, false, false, true), r);
        assert_eq!(st.interaction.mode, DragMode::None);
        assert_eq!(st.marker(), 13.4375);
    }

    /// 原版 `:156`：命中框只吃左键，右键落在手柄上照样开区间菜单。
    #[test]
    fn a_right_click_on_a_handle_still_opens_the_timeline_menu() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        st.set_marker(5.0);
        let mut snap = mouse(164.0, false, false, false);
        snap.secondary_pressed = true;
        handle_timeline_input(&mut st, &snap, r);
        let menu = st.context_menu.expect("手柄上的右键仍要开菜单");
        assert_eq!(menu.kind, ContextKind::Timeline);
        assert_eq!(st.interaction.mode, DragMode::None, "右键不进拖拽状态");
    }

    /// 原版 `:124` 的总闸：`playing` 非 0 时整段标尺命中都不跑 —— 播放中点标尺
    /// 既不跳播放头也不建区间。
    #[test]
    fn the_ruler_is_inert_while_playing() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        st.audio.clock_mut().play();
        handle_timeline_input(&mut st, &mouse(100.0, true, true, false), r);
        handle_timeline_input(&mut st, &mouse(260.0, false, true, false), r);
        handle_timeline_input(&mut st, &mouse(260.0, false, false, true), r);
        st.audio.clock_mut().pause();
        assert_eq!(st.marker(), 0.0);
        assert!(!st.section_exists);
        assert_eq!(st.interaction.mode, DragMode::None);
    }
}
