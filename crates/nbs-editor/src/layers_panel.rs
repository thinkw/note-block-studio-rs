//! 左侧图层栏：逐像素照原版 `control_draw.gml` 的图层列来画。
//!
//! 版式出处：行底 `spr_layerbox` 在 `x1 = 4`、`y1 = 54 + 32 + b * 32 - 1`；名称是
//! `draw_text_edit`（经典）/ `draw_textarea`（Fluent）的常驻输入框；音量、声像是
//! 竖向拖拽的小精灵（`spr_volume` / `spr_stereo`，悬停时上移 5px 并显出数值）；
//! 右侧 7 个 `draw_layericon`，横坐标 `x1 + 126…234 - !realvolume - realstereo * 10`；
//! 可视行底部再挂一条 15 个 `draw_macroicon` 的宏条（有选区且显示钢琴键盘时）。
//!
//! 列宽固定 264px（原版内容宽 256 + 左右各 4），只有行高跟着网格缩放走：
//! 横向偏移和图标尺寸保持原版像素，纵向按 `(cell - 32) / 2` 居中。

use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Id, Key, Layout, Pos2, Rect, Sense,
    TextEdit, Ui, UiBuilder,
};
use nbs_core::model::{Layer, LockMode, PAN_CENTER};

use crate::commands::Command;
use crate::icons::Bank;
use crate::state::{ContextKind, ContextMenu, EditorState, LayerDrag, WindowKind};
use crate::theme::{Theme, ThemeKind};
use crate::tr;
use crate::viewport::{BASE_CELL, LAYER_PANEL_WIDTH, SCROLLBAR_SIZE, TIMELINE_HEIGHT};

/// 原版图层列内容左边界 `x1`。
const X1: f32 = 4.0;
/// 原版行底比整行上移 1px：`spr_layerbox` 高 34，多出的 2px 是它自带的下沿压痕。
const ROW_LIFT: f32 = 1.0;

pub fn show_layers_panel(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let area = ui.max_rect();
    let Some(bank) = state.icons.bank() else {
        show_layers_text(state, ui);
        return;
    };
    let start = area.min;
    let cell = state.viewport.cell();
    let yo = (cell - BASE_CELL) * 0.5;
    let band = start.y;
    let rows_start = pos2(start.x, band + TIMELINE_HEIGHT);

    header(state, ui, &bank, &th, start, band, area.width());

    let l0 = {
        let (_, _, a, _) = state.viewport.visible_range();
        a
    };
    // 原版 `totalrows = floor((rh - rhval) / 32)`：只画整行，宏条占掉最后那点余量。
    // 网格那边用同一条公式（`grid.rs` 里的 `h`），两边画行数、宏条位置才对得上。
    // 行数只按本面板的区域算，不取 `viewport.rows()`：侧栏比网格先画，
    // 用视口那个值会慢一帧，第一帧少画几行、宏条也跟着错位。
    let rows = (((area.height() - TIMELINE_HEIGHT - SCROLLBAR_SIZE) / cell).floor() as u32).max(1);
    let layers = state.doc.song.layers.clone();
    let idle = state.window.is_none() && state.context_menu.is_none();
    let shift = ui.input(|i| i.modifiers.shift);
    let show_vol = state.prefs.show_layer_volumes;
    let mono = state.prefs.mono;
    let editing = state.editing_layer_name.clone();

    for idx in l0..l0 + rows {
        // 原版 `for (b = 0; b < totalrows; b += 1)` 把整列 19 行全画满（`control_draw.gml` 1298-1312）：
        // 超出图层数的行也画行框，只是数组越界的那几项被就地初始化成「空名称、音量 100、声道 100」。
        // 这些行上没有图层可改，所以只有「新建层」会真的响应，其余控件照画但不受理。
        let blank = Layer::default();
        let layer = layers.get(idx as usize).unwrap_or(&blank);
        let exists = (idx as usize) < layers.len();
        let y1 = rows_start.y + (idx - l0) as f32 * cell - ROW_LIFT;
        row(
            state,
            ui,
            &bank,
            &th,
            pos2(start.x, y1),
            cell,
            yo,
            idx,
            layer,
            &editing,
            idle,
            shift,
            show_vol,
            mono,
            exists,
        );
    }

    // 宏条贴在整行区底（原版 `y1 + 32`，y1 是最后一行行顶）。
    let bar_top = rows_start.y + rows as f32 * cell - ROW_LIFT;
    macro_bar(state, ui, &bank, &th, pos2(start.x, bar_top), idle, false);
}

/// 图层栏收起时，原版照样在左下角画宏条：`control_draw.gml` 1298-1308 那个行循环并不看
/// `show_layers`，`x1 = 4 / y1 = …` 照旧执行，只有行内容被跳过；到 1496 行时条子就落在
/// `y1 + 52`、宽 380。所以这里由网格面板调用一次，收起侧栏的人不会凭空丢掉 15 个宏。
pub(crate) fn macro_bar_over_grid(state: &mut EditorState, ui: &mut Ui, area: Rect, idle: bool) {
    if state.flags.show_layers {
        return;
    }
    let Some(bank) = state.icons.bank() else {
        return;
    };
    let th = state.theme.clone();
    let cell = state.viewport.cell();
    let full = (((area.height() - TIMELINE_HEIGHT - SCROLLBAR_SIZE) / cell).floor() as u32).max(1);
    let bar_top = area.min.y + TIMELINE_HEIGHT + full as f32 * cell - ROW_LIFT + 20.0;
    macro_bar(state, ui, &bank, &th, pos2(area.min.x, bar_top), idle, true);
}

/// 顶部与时间轴对齐的 32px：已用时间 / 总时长 / 速度框 / 小节·拍·十六分。
fn header(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    start: Pos2,
    band: f32,
    width: f32,
) {
    let p = ui.painter();
    p.rect_filled(
        Rect::from_min_size(start, vec2(width, TIMELINE_HEIGHT)),
        0.0,
        th.panel,
    );

    let song = &state.doc.song;
    let bpm_on = state.prefs.use_bpm;
    let tempo = state.doc.song.meta.tempo.max(0.25);
    // 拖速度框时先按 0.25 量化预览，松手才写进歌曲（原版 `tempo = floor(tempodrag * 4) / 4`）。
    let shown = state
        .tempo_drag
        .map(|raw| (raw * 4.0).floor() / 4.0)
        .unwrap_or(tempo) as f64;
    let marker = state.marker();
    let total = song.song_length() as f64;
    let fluent = th.kind == ThemeKind::Fluent;

    // 原版：经典主题右对齐到 x=93；Fluent 左对齐到 9 / 26。
    let elapsed = time_str(marker / shown);
    let length = format!("/ {}", time_str(total / shown));
    if fluent {
        p.text(
            pos2(start.x + 9.0, band - 2.0),
            Align2::LEFT_TOP,
            elapsed,
            FontId::proportional(11.0),
            th.text,
        );
        p.text(
            pos2(start.x + 26.0, band + 15.0),
            Align2::LEFT_TOP,
            length,
            FontId::proportional(8.0),
            th.text,
        );
    } else {
        p.text(
            pos2(start.x + 93.0, band - 2.0),
            Align2::RIGHT_TOP,
            elapsed,
            FontId::proportional(11.0),
            th.text,
        );
        p.text(
            pos2(start.x + 93.0, band + 15.0),
            Align2::RIGHT_TOP,
            length,
            FontId::proportional(8.0),
            th.text,
        );
    }

    // 小节·拍·十六分：原版只画，不给交互（悬停出说明）。
    let sig = song.meta.time_signature.max(1) as f64;
    bank.tempo_box(p, pos2(start.x + 184.0, band + 3.0), false, th);
    let digits = FontId::proportional(9.0);
    for (x, t) in [
        (215.0, ".".to_string()),
        (230.0, ".".to_string()),
        (
            210.0,
            ((marker / (sig * 4.0)).floor() as i64 + 1).to_string(),
        ),
        (
            225.0,
            (((marker / 4.0) % sig).floor() as i64 + 1).to_string(),
        ),
        (240.0, ((marker % 4.0).floor() as i64 + 1).to_string()),
    ] {
        p.text(
            pos2(start.x + x, band + 6.0),
            Align2::RIGHT_TOP,
            t,
            digits.clone(),
            th.text,
        );
    }

    // 速度框：精灵宽 74，原版判定和文字定位都用左边 108（BPM 时往左挪 7px 画框）。
    let box_x = if bpm_on { 101.0 } else { 108.0 };
    bank.tempo_box(p, pos2(start.x + box_x, band + 3.0), bpm_on, th);
    let value = if bpm_on {
        format!("{:.2} BPM", shown * 15.0)
    } else {
        format!("{:.2} t / s", shown)
    };
    p.text(
        pos2(start.x + 136.0, band + 6.0),
        Align2::CENTER_TOP,
        value,
        digits,
        th.text,
    );

    let idle = state.window.is_none() && state.context_menu.is_none();
    let bars_rect = Rect::from_min_size(pos2(start.x + 184.0, band + 3.0), vec2(64.0, 22.0));
    ui.interact(bars_rect, Id::new("bars_box"), Sense::hover())
        .on_hover_text(tr(
            "Position of the marker in bars, beats and sixteenths.",
            "歌曲的进度（全音、四分、十六分）",
        ));

    let hit = Rect::from_min_size(pos2(start.x + 108.0, band + 3.0), vec2(64.0, 22.0));
    let resp = ui.interact(hit, Id::new("tempo_box"), Sense::click_and_drag());
    let clicked = idle && resp.clicked();
    let started = idle && resp.drag_started();
    let rclicked = idle && resp.secondary_clicked();
    let (dragged, stopped, dy, hover_pos) = (
        resp.dragged(),
        resp.drag_stopped(),
        resp.drag_delta().y,
        resp.hover_pos(),
    );
    if clicked {
        state.emit(Command::ShowWindow(WindowKind::SetTempo));
    }
    if started {
        state.tempo_drag = Some(tempo);
    }
    if rclicked {
        state.context_menu = Some(ContextMenu {
            pos: hover_pos.unwrap_or(hit.center()),
            cell: (0, 0),
            kind: ContextKind::Tempo,
        });
    }
    if dragged {
        if let Some(raw) = state.tempo_drag {
            // 原版 `tempodrag += 0.25 * (mouse_yprev - mouse_y) / 3`，夹在 0.25..60。
            state.tempo_drag = Some((raw - dy * 0.25 / 3.0).clamp(0.25, 60.0));
        }
    }
    if stopped {
        if let Some(raw) = state.tempo_drag.take() {
            let q = (raw * 4.0).floor() / 4.0;
            if (q - tempo).abs() > f32::EPSILON {
                state.emit(Command::SetTempo(q));
            }
        }
    }
    // 原版两语言的括号句式不同：EN 是 "measured in …"，CN 是「（…）」。
    let tip = if crate::lang() == 1 {
        let unit = if bpm_on {
            "拍数/分钟"
        } else {
            "红石刻/秒"
        };
        format!("歌曲的速度（{unit}）\n拖拽来大致更改，点击来手动输入，右键查看更多选项。")
    } else if bpm_on {
        "Tempo of the song (measured in beats per minute).\nClick and drag to change. Click to enter value. Right-click for more options.".to_string()
    } else {
        "Tempo of the song (measured in ticks per second).\nClick and drag to change. Click to enter value. Right-click for more options.".to_string()
    };
    resp.on_hover_text(tip)
        .on_hover_cursor(if state.tempo_drag.is_some() {
            CursorIcon::ResizeNorth
        } else {
            CursorIcon::PointingHand
        });
}

/// 一行图层。`at` 是原版行底左上角（已含 `ROW_LIFT`），`yo` 是行高变化后的纵向居中量。
/// `exists` 为 false 表示原版那种「越界初始化出来的空白行」：框和图标照画，但没有图层可改。
#[allow(clippy::too_many_arguments)]
fn row(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    at: Pos2,
    cell: f32,
    yo: f32,
    idx: u32,
    layer: &Layer,
    editing: &Option<(u32, String)>,
    idle: bool,
    shift: bool,
    show_vol: bool,
    mono: bool,
    exists: bool,
) {
    let x1 = at.x + X1;
    let y1 = at.y;
    // 原版 `draw_sprite(spr_layerbox, …, x1, y1)`，行框同样从 x1 = 4 起。
    bank.layer_row(ui.painter(), pos2(x1, y1), vec2(256.0, cell + 2.0), th);

    // 原版 `x1 + 126 - !realvolume - realstereo * 10`：藏掉字形时按钮只左挪 1px / 10px。
    let bx = |base: f32| {
        x1 + base - (if show_vol { 0.0 } else { 1.0 }) - (if mono { 10.0 } else { 0.0 })
    };

    // ---- 名称：原版的常驻输入框 ----
    let fluent = th.kind == ThemeKind::Fluent;
    let font = FontId::proportional(if fluent { 9.0 } else { 8.0 });
    let name_rect = Rect::from_min_size(pos2(x1 + 10.0, y1 + 10.0 + yo), vec2(75.0, 13.0));
    let name_resp = ui.interact(name_rect, Id::new(("layername", idx)), Sense::click());
    let name_clicked = idle && name_resp.clicked();
    let name_right = idle && name_resp.secondary_clicked();
    let name_pos = name_resp.hover_pos();
    let buf = editing
        .as_ref()
        .filter(|(i, _)| *i == idx)
        .map(|(_, s)| s.clone());
    if let Some(text) = buf {
        let (nx, ny, nw, nh) = if fluent {
            (x1 + 9.0, y1 + 4.0 + yo, 72.0, 24.0)
        } else {
            (x1 + 11.0, y1 + 10.0 + yo, 72.0, 14.0)
        };
        let focus_id = Id::new(("layername_focus", idx));
        let had_focus = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(focus_id))
            .unwrap_or(false);
        let mut text = text;
        // 嵌套 ui 给一个跟滚动无关的稳定 id，否则逐行 id 会随滚动串到别的图层上。
        let nested = ui.allocate_new_ui(
            UiBuilder::new()
                .id_salt(("layername_edit", idx))
                .max_rect(Rect::from_min_size(pos2(nx, ny), vec2(nw, nh)))
                .layout(Layout::left_to_right(Align::Min)),
            |ui| {
                let re = ui.add(
                    TextEdit::singleline(&mut text)
                        .hint_text(format!(
                            "{}",
                            tr(&format!("Layer {}", idx + 1), &format!("第 {} 层", idx + 1))
                        ))
                        .desired_width(nw)
                        .font(font),
                );
                if !had_focus {
                    re.request_focus();
                }
                re
            },
        );
        let re = nested.inner;
        let enter = re.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let esc = re.has_focus() && ui.input(|i| i.key_pressed(Key::Escape));
        let changed = re.changed();
        let lost = re.lost_focus();
        if enter || lost {
            if text != layer.name {
                state.emit(Command::RenameLayer(idx, std::mem::take(&mut text)));
            }
            state.editing_layer_name = None;
        } else if esc {
            state.editing_layer_name = None;
        } else if changed {
            state.editing_layer_name = Some((idx, text));
        }
        let active = state
            .editing_layer_name
            .as_ref()
            .is_some_and(|(i, _)| *i == idx);
        ui.ctx().data_mut(|d| d.insert_temp(focus_id, active));
    } else {
        let (label, color) = if layer.name.is_empty() {
            (
                format!(
                    "{}",
                    tr(&format!("Layer {}", idx + 1), &format!("第 {} 层", idx + 1))
                ),
                Color32::from_gray(if th.dark { 160 } else { 128 }),
            )
        } else {
            (layer.name.clone(), th.text)
        };
        ui.painter().text(
            pos2(x1 + 11.0, y1 + 10.0 + yo),
            Align2::LEFT_TOP,
            label,
            font,
            color,
        );
        if exists && name_clicked {
            state.editing_layer_name = Some((idx, layer.name.clone()));
        }
    }
    if exists && name_right {
        state.context_menu = Some(ContextMenu {
            pos: name_pos.unwrap_or(name_rect.center()),
            cell: (0, idx),
            kind: ContextKind::Layer(idx),
        });
    }
    name_resp.on_hover_text(tr("The name of this layer.", "本层的名称。"));

    // ---- 音量 / 声像：竖向拖拽 ----
    let drag = state.layer_drag;
    let vol = drag
        .filter(|d| d.index == idx && !d.stereo)
        .map(|d| d.value)
        .unwrap_or(layer.volume);
    if show_vol {
        let hit = Rect::from_min_size(pos2(x1 + 88.0, y1 + 5.0 + yo), vec2(16.0, 25.0));
        let resp = ui.interact(hit, Id::new(("layervol", idx)), Sense::drag());
        let active = resp.hovered() || resp.dragged();
        let (started, dragged, stopped, delta) = (
            resp.drag_started(),
            resp.dragged(),
            resp.drag_stopped(),
            resp.drag_delta().y,
        );
        bank.volume_glyph(
            ui.painter(),
            pos2(x1 + 91.0, y1 + 11.0 + yo),
            vol,
            active,
            th,
        );
        if active {
            ui.painter().text(
                pos2(x1 + 98.0, y1 + 18.0 + yo),
                Align2::CENTER_TOP,
                format!("{vol}%"),
                FontId::proportional(8.0),
                th.text,
            );
        }
        if idle && exists && started {
            state.layer_drag = Some(LayerDrag {
                index: idx,
                stereo: false,
                value: vol,
                raw: vol as f32,
            });
        }
        if let Some(mut d) = state.layer_drag.filter(|d| d.index == idx && !d.stereo) {
            if dragged {
                d.raw = (d.raw - delta).clamp(0.0, 100.0);
                d.value = if shift {
                    d.raw.round() as u8
                } else {
                    (d.raw / 10.0).floor() as u8 * 10
                };
                state.layer_drag = Some(d);
            }
            if stopped {
                state.layer_drag = None;
                state.emit(Command::SetLayerVolume(idx, d.value));
            }
        }
        resp.on_hover_text(format!(
            "{}{}%\n{}",
            tr("Volume of this layer: ", "本层的音量: "),
            vol,
            tr("(Click and drag to change)", "（拖拽来修改）")
        ))
        .on_hover_cursor(CursorIcon::ResizeNorth);
    }

    // 原版 `realstereo` 是「关闭立体声」：为真时不画声道字形，播放也归到中央。
    if !mono {
        let pan = drag
            .filter(|d| d.index == idx && d.stereo)
            .map(|d| d.value)
            .unwrap_or(layer.stereo);
        let hit = Rect::from_min_size(pos2(x1 + 108.0, y1 + 5.0 + yo), vec2(16.0, 25.0));
        let resp = ui.interact(hit, Id::new(("layerpan", idx)), Sense::drag());
        let active = resp.hovered() || resp.dragged();
        let (started, dragged, stopped, delta) = (
            resp.drag_started(),
            resp.dragged(),
            resp.drag_stopped(),
            resp.drag_delta().y,
        );
        bank.stereo_glyph(
            ui.painter(),
            pos2(x1 + 110.0, y1 + 11.0 + yo),
            pan,
            active,
            th,
        );
        let (tip, label) = pan_label(pan);
        if active {
            ui.painter().text(
                pos2(x1 + 116.0, y1 + 18.0 + yo),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(8.0),
                th.text,
            );
        }
        if idle && exists && started {
            state.layer_drag = Some(LayerDrag {
                index: idx,
                stereo: true,
                value: pan,
                raw: pan as f32,
            });
        }
        if let Some(mut d) = state.layer_drag.filter(|d| d.index == idx && d.stereo) {
            if dragged {
                d.raw = (d.raw - delta).clamp(0.0, 200.0);
                d.value = if shift {
                    d.raw.round() as u8
                } else {
                    (d.raw / 10.0).floor() as u8 * 10
                };
                state.layer_drag = Some(d);
            }
            if stopped {
                state.layer_drag = None;
                state.emit(Command::SetLayerStereo(idx, d.value));
            }
        }
        resp.on_hover_text(format!(
            "{}{tip}\n{}",
            tr("Stereo pan: ", "立体声声道: "),
            tr("(Click and drag to change)", "（拖拽来修改）")
        ))
        .on_hover_cursor(CursorIcon::ResizeNorth);
    }

    // ---- 7 个功能按钮（原版下标 0..6）----
    // 原版在空白行上同样画全 7 个图标，点了却没有图层可改；只有「新建层」在空白行仍有意义。
    let by = y1 + 8.0 + yo;
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(bx(126.0), by),
            0,
            idx,
            tr("Lock this layer", "静音本层\n（拖拽可批量操作）"),
            layer.lock == LockMode::Locked,
            th,
        )
    {
        let next = if layer.lock == LockMode::Locked {
            LockMode::Normal
        } else {
            LockMode::Locked
        };
        state.emit(Command::SetLayerLock(idx, next));
    }
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(bx(144.0), by),
            1,
            idx,
            tr("Solo this layer", "独奏本层\n（拖拽可批量操作）"),
            layer.lock == LockMode::Solo,
            th,
        )
    {
        let next = if layer.lock == LockMode::Solo {
            LockMode::Normal
        } else {
            LockMode::Solo
        };
        state.emit(Command::SetLayerLock(idx, next));
    }
    if exists && idle && bank.layer_button(ui, pos2(bx(162.0), by), 2, idx, tr(
        "Select all note blocks in this layer\n(Click and drag to select multiple; click again to deselect)",
        "选择本层所有方块\n（拖拽可批量选择，再次点选取消选择）",
    ), false, th) {
        state.emit(Command::SelectLayer(idx));
    }
    if idle
        && bank.layer_button(
            ui,
            pos2(bx(180.0), by),
            3,
            idx,
            tr(
                "Add empty layer here\n(Click and drag to add multiple layers)",
                "新建层\n（拖拽可批量添加）",
            ),
            false,
            th,
        )
    {
        state.emit(Command::InsertLayer(idx));
    }
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(bx(198.0), by),
            4,
            idx,
            tr(
                "Remove this layer\n(Click and drag to remove multiple layers)",
                "删除层\n（拖拽可批量删除）",
            ),
            false,
            th,
        )
    {
        state.emit(Command::RemoveLayer(idx));
    }
    // 原版第 0 层不画上移按钮（`if ((startb + b > 0) && draw_layericon(...))`）。
    if exists
        && idx > 0
        && idle
        && bank.layer_button(
            ui,
            pos2(bx(216.0), by),
            5,
            idx,
            tr(
                "Shift layer up\n(Click and drag to shift multiple layers)",
                "上移本层\n（拖拽可上移多层）",
            ),
            false,
            th,
        )
    {
        state.emit(Command::MoveLayerUp(idx));
    }
    // 原版在最底层把下移按钮左挪 8px（少画的上移按钮位置就空在那儿）。
    let down_x = bx(234.0) - if idx == 0 { 8.0 } else { 0.0 };
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(down_x, by),
            6,
            idx,
            tr(
                "Shift layer down\n(Click and drag to shift multiple layers)",
                "下移本层\n（拖拽可下移多层）",
            ),
            false,
            th,
        )
    {
        state.emit(Command::MoveLayerDown(idx));
    }
}

/// 原版声道文字：悬停里说「中央」，行内显示「单声道」，其余都是 `L n` / `R n`。
fn pan_label(pan: u8) -> (String, String) {
    let d = pan as i32 - PAN_CENTER as i32;
    if d == 0 {
        (tr("Center", "中央").into(), tr("Mono", "单声道").into())
    } else {
        let s = format!("{} {}", if d > 0 { "R" } else { "L" }, d.abs());
        (s.clone(), s)
    }
}

/// 原版宏条：`spr_iconbar`（经典）或灰色圆角矩形（Fluent）+ 15 个 `draw_macroicon`。
/// `wide` = 图层栏收起时画在网格上的那种：矮一截、宽 380、按钮贴条顶 +3。
fn macro_bar(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    at: Pos2,
    idle: bool,
    wide: bool,
) {
    if !state.flags.show_piano || state.doc.selection.is_empty() {
        return;
    }
    let x1 = at.x + X1;
    let p = ui.painter();
    let fluent = th.kind == ThemeKind::Fluent;
    // 原版 1497-1511：show_layers 时 `y1 + 32`、高 1.3 × 30；收起时 `y1 + 52`、高 0.75 × 30。
    // Fluent 那两个圆角矩形是 38 和 21。宽度按 Fluent 的 255 / 380 取。
    let (w, h, dy) = if wide {
        (380.0, if fluent { 21.0 } else { 22.5 }, 3.0)
    } else {
        (255.0, if fluent { 38.0 } else { 39.0 }, 5.0)
    };
    let bg = Rect::from_min_size(pos2(x1, at.y), vec2(w, h));
    if fluent {
        p.rect_filled(bg, 2.0, Color32::from_gray(70));
    } else {
        bank.icon_strip_bg(p, bg);
    }
    let row1 = pos2(x1 + 6.0, at.y + dy);
    let row2 = pos2(row1.x, row1.y + 16.0);
    let step = 25.0;

    for btn in MACRO_BTNS {
        let i = btn.index;
        // 原版第一行画到第 9 个（淡出）为止，之后 `xx` 回到行首、`yy += 16`。
        let at = if i < 10 {
            pos2(row1.x + i as f32 * step, row1.y)
        } else {
            pos2(row2.x + (i - 10) as f32 * step, row2.y)
        };
        if idle && bank.macro_button(ui, at, i, tr(btn.tip_en, btn.tip_zh), th) {
            emit_macro(state, &btn.action);
        }
    }
}

/// 宏按钮的动作：原版三个（琶音 / 滑音 / 错开）有各自的对话框，我们没有，
/// 就打开统一的宏编辑器并预选好对应那一项。
fn emit_macro(state: &mut EditorState, action: &MacroAction) {
    match action {
        MacroAction::Window(kind) => state.emit(Command::ShowWindow(kind.clone())),
        MacroAction::Editor(mi) => {
            state.macro_index = *mi;
            state.emit(Command::ShowWindow(WindowKind::MacroEditor));
        }
        MacroAction::Run(m) => state.emit(Command::RunMacro(m.clone())),
    }
}

/// 原版 `time_str(sec)`：`hh:mm:ss;mmm`，各段补零。
pub(crate) fn time_str(sec: f64) -> String {
    let s = sec.max(0.0);
    let msec = (s * 1000.0).floor() as i64 % 1000;
    let mi = (s / 60.0).floor() as i64 % 60;
    let hr = (s / 3600.0).floor() as i64 % 60;
    let sec = s.floor() as i64 % 60;
    format!("{hr:02}:{mi:02}:{sec:02};{msec:03}")
}

#[derive(Clone)]
enum MacroAction {
    Window(WindowKind),
    Editor(usize),
    Run(nbs_core::Macro),
}

/// 宏条按钮：原版精灵下标、原版悬停词（前 8 个中文版也写英文，照抄）、
/// 宏按钮：`tip` 是悬停提示（原版 `draw_macroicon` 第 0..7 个两种语言都写英文，
/// 第 8..14 个才有中文分支），`label` 是精灵缺失时退路条上的按钮文字。
struct MacroBtn {
    index: usize,
    tip_en: &'static str,
    tip_zh: &'static str,
    label_en: &'static str,
    label_zh: &'static str,
    action: MacroAction,
}

use nbs_core::Macro as M;

const MACRO_BTNS: [MacroBtn; 15] = [
    MacroBtn {
        index: 0,
        tip_en: "Tremolo...",
        tip_zh: "Tremolo...",
        label_en: "Tremolo...",
        label_zh: "Tremolo…",
        action: MacroAction::Window(WindowKind::Tremolo),
    },
    MacroBtn {
        index: 1,
        tip_en: "Stereo...",
        tip_zh: "Stereo...",
        label_en: "Stereo...",
        label_zh: "Stereo…",
        action: MacroAction::Window(WindowKind::Stereo),
    },
    MacroBtn {
        index: 2,
        tip_en: "Arpeggio...",
        tip_zh: "Arpeggio...",
        label_en: "Arpeggio...",
        label_zh: "Arpeggio…",
        action: MacroAction::Editor(8),
    },
    MacroBtn {
        index: 3,
        tip_en: "Portamento...",
        tip_zh: "Portamento...",
        label_en: "Portamento...",
        label_zh: "Portamento…",
        action: MacroAction::Editor(9),
    },
    MacroBtn {
        index: 4,
        tip_en: "Vibrato",
        tip_zh: "Vibrato",
        label_en: "Vibrato",
        label_zh: "Vibrato",
        action: MacroAction::Run(M::Vibrato),
    },
    MacroBtn {
        index: 5,
        tip_en: "Stagger...",
        tip_zh: "Stagger...",
        label_en: "Stagger...",
        label_zh: "Stagger…",
        action: MacroAction::Editor(10),
    },
    MacroBtn {
        index: 6,
        tip_en: "Chorus",
        tip_zh: "Chorus",
        label_en: "Chorus",
        label_zh: "Chorus",
        action: MacroAction::Run(M::Chorus { detune: 5 }),
    },
    MacroBtn {
        index: 7,
        tip_en: "Volume LFO",
        tip_zh: "Volume LFO",
        label_en: "Volume LFO",
        label_zh: "Volume LFO",
        action: MacroAction::Run(M::VelocityLfo),
    },
    MacroBtn {
        index: 8,
        tip_en: "Fade in",
        tip_zh: "淡入",
        label_en: "Fade in",
        label_zh: "淡入",
        action: MacroAction::Run(M::FadeIn),
    },
    MacroBtn {
        index: 9,
        tip_en: "Fade out",
        tip_zh: "淡出",
        label_en: "Fade out",
        label_zh: "淡出",
        action: MacroAction::Run(M::FadeOut),
    },
    MacroBtn {
        index: 10,
        tip_en: "Replace key",
        tip_zh: "替换音",
        label_en: "Replace key",
        label_zh: "替换音",
        action: MacroAction::Run(M::ReplaceKey),
    },
    MacroBtn {
        index: 11,
        tip_en: "Set velocity...",
        tip_zh: "设定音量......",
        label_en: "Set velocity...",
        label_zh: "设定音量…",
        action: MacroAction::Window(WindowKind::SetVelocity),
    },
    MacroBtn {
        index: 12,
        tip_en: "Set panning...",
        tip_zh: "设定声道......",
        label_en: "Set panning...",
        label_zh: "设定声道…",
        action: MacroAction::Window(WindowKind::SetPanning),
    },
    MacroBtn {
        index: 13,
        tip_en: "Set pitch...",
        tip_zh: "设定音高......",
        label_en: "Set pitch...",
        label_zh: "设定音高…",
        action: MacroAction::Window(WindowKind::SetPitch),
    },
    MacroBtn {
        index: 14,
        tip_en: "Reset all properties",
        tip_zh: "重置所有属性",
        label_en: "Reset all properties",
        label_zh: "重置所有属性",
        action: MacroAction::Run(M::Reset),
    },
];

/// 精灵图缺失时的退路：同样的功能用普通控件摆一遍，不留不能用的按钮。
fn show_layers_text(state: &mut EditorState, ui: &mut Ui) {
    let song_len = state.doc.song.song_length() as f64;
    let tempo = state.doc.song.meta.tempo.max(0.25) as f64;
    let marker = state.marker();
    ui.horizontal(|ui| {
        ui.label(format!(
            "{} / {}",
            time_str(marker / tempo),
            time_str(song_len / tempo)
        ));
        if ui.button(tr("Tempo...", "速度…")).clicked() {
            state.emit(Command::ShowWindow(WindowKind::SetTempo));
        }
    });
    ui.separator();
    let layers = state.doc.song.layers.clone();
    let show_vol = state.prefs.show_layer_volumes;
    let mono = state.prefs.mono;
    for (i, layer) in layers.iter().enumerate() {
        let idx = i as u32;
        ui.horizontal(|ui| {
            let mut name = layer.name.clone();
            let re = ui.add(
                TextEdit::singleline(&mut name)
                    .desired_width(90.0)
                    .hint_text(format!(
                        "{}",
                        tr(&format!("Layer {}", idx + 1), &format!("第 {} 层", idx + 1))
                    )),
            );
            if re.lost_focus() && name != layer.name {
                state.emit(Command::RenameLayer(idx, name));
            }
            if ui
                .selectable_label(layer.lock == LockMode::Locked, tr("Mute", "静音"))
                .clicked()
            {
                let next = if layer.lock == LockMode::Locked {
                    LockMode::Normal
                } else {
                    LockMode::Locked
                };
                state.emit(Command::SetLayerLock(idx, next));
            }
            if ui
                .selectable_label(layer.lock == LockMode::Solo, tr("Solo", "独奏"))
                .clicked()
            {
                let next = if layer.lock == LockMode::Solo {
                    LockMode::Normal
                } else {
                    LockMode::Solo
                };
                state.emit(Command::SetLayerLock(idx, next));
            }
            if ui.button(tr("Select", "选层")).clicked() {
                state.emit(Command::SelectLayer(idx));
            }
            if ui.button(tr("Add", "加层")).clicked() {
                state.emit(Command::InsertLayer(idx));
            }
            if ui.button(tr("Remove", "删层")).clicked() {
                state.emit(Command::RemoveLayer(idx));
            }
            if idx > 0 && ui.button(tr("Up", "上移")).clicked() {
                state.emit(Command::MoveLayerUp(idx));
            }
            if ui.button(tr("Down", "下移")).clicked() {
                state.emit(Command::MoveLayerDown(idx));
            }
            if show_vol {
                text_drag_value(state, ui, idx, false, layer.volume, 100);
            }
            if !mono {
                text_drag_value(state, ui, idx, true, layer.stereo, 200);
            }
        });
    }
    if state.flags.show_piano && !state.doc.selection.is_empty() {
        ui.separator();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            for btn in MACRO_BTNS {
                if ui.button(tr(btn.label_en, btn.label_zh)).clicked() {
                    emit_macro(state, &btn.action);
                }
            }
        });
    }
}

/// 退路里的音量 / 声道框：拖拽期间只改预览，松手才提交一条历史。
fn text_drag_value(
    state: &mut EditorState,
    ui: &mut Ui,
    idx: u32,
    stereo: bool,
    current: u8,
    max: u8,
) {
    let mut value = state
        .layer_drag
        .filter(|d| d.index == idx && d.stereo == stereo)
        .map(|d| d.value)
        .unwrap_or(current);
    let resp = ui.add(
        egui::DragValue::new(&mut value)
            .range(0..=max)
            .suffix(if stereo { "" } else { "%" })
            .speed(1),
    );
    if resp.changed() {
        state.layer_drag = Some(LayerDrag {
            index: idx,
            stereo,
            value,
            raw: value as f32,
        });
    }
    if ui.input(|i| i.pointer.any_released())
        && state
            .layer_drag
            .filter(|d| d.index == idx && d.stereo == stereo)
            .is_some()
    {
        if let Some(d) = state.layer_drag.take() {
            state.emit(if d.stereo {
                Command::SetLayerStereo(d.index, d.value)
            } else {
                Command::SetLayerVolume(d.index, d.value)
            });
        }
    }
}

pub const PANEL_WIDTH: f32 = LAYER_PANEL_WIDTH;
