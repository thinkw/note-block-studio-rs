//! 左侧图层栏：逐像素照原版 `control_draw.gml` 的图层列来画。
//!
//! 版式出处：行底 `spr_layerbox` 在 `x1 = 4`、`y1 = 54 + 32 + b * 32 - 1`；名称是
//! `draw_text_edit`（经典）/ `draw_textarea`（Fluent）的常驻输入框；音量、声像是
//! 竖向拖拽的小精灵（`spr_volume` / `spr_stereo`，悬停时上移 5px 并显出数值）；
//! 右侧 7 个 `draw_layericon`，横坐标 `x1 + 126…234 - !realvolume - realstereo * 10`；
//! 可视行底部再挂一条 15 个 `draw_macroicon` 的宏条（有选区且显示钢琴键盘时）。
//!
//! 原版整个画面按 window_scale 等比放大，图层列也在其中；缩放改造（#80）照抄这一
//! 语义：面板宽 264×u，内部所有坐标、字号、精灵尺寸一律乘 `u = viewport.u()`。
//! 拖拽增量同理：原版鼠标坐标在「除以缩放」的房间坐标系里（`control_draw.gml:15-16`），
//! 这里的 `drag_delta`（屏幕点）先 `/u` 再参与数值。

use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, Id, Key, Layout, PointerButton, Pos2, Rect,
    Sense, TextEdit, Ui, UiBuilder,
};
use nbs_core::model::{Layer, LockMode, PAN_CENTER};

use crate::commands::Command;
use crate::fonts::{info_med_bold_font, main_font, small_font};
use crate::icons::{layer as layer_icon, macro_icon, Bank};
use crate::state::{ContextKind, ContextMenu, EditorState, LayerDrag, WindowKind};
use crate::theme::{Theme, ThemeKind};
use crate::tr;
use crate::viewport::{bar_thickness, timeline_band, RULER_SCALE};

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
    let u = state.viewport.u();
    let band = start.y;
    // 时间轴带和滚动条的厚度都不随缩放（#88 / #90），和网格的 `split()` 取同一个数，
    // 两边算出的行数、宏条位置才对得上。
    let rows_start = pos2(start.x, band + timeline_band());

    header(state, ui, &bank, &th, start, band, area.width());

    let l0 = {
        let (_, _, a, _) = state.viewport.visible_range();
        a
    };
    // 原版 `totalrows = floor((rh - rhval) / 32)`：只画整行，宏条占掉最后那点余量。
    // 网格那边用同一条公式（`grid.rs` 里的 `split`），两边画行数、宏条位置才对得上。
    // 行数只按本面板的区域算，不取 `viewport.rows()`：侧栏比网格先画，
    // 用视口那个值会慢一帧，第一帧少画几行、宏条也跟着错位。
    let rows = (((area.height() - timeline_band() - bar_thickness()) / cell).floor() as u32).max(1);
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
        let y1 = rows_start.y + (idx - l0) as f32 * cell - ROW_LIFT * u;
        row(
            state,
            ui,
            &bank,
            &th,
            pos2(start.x, y1),
            cell,
            u,
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
    let bar_top = rows_start.y + rows as f32 * cell - ROW_LIFT * u;
    macro_bar(
        state,
        ui,
        &bank,
        &th,
        pos2(start.x, bar_top),
        idle,
        false,
        u,
    );
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
    let u = state.viewport.u();
    let full = (((area.height() - timeline_band() - bar_thickness()) / cell).floor() as u32).max(1);
    let bar_top = area.min.y + timeline_band() + full as f32 * cell - ROW_LIFT * u + 20.0 * u;
    macro_bar(
        state,
        ui,
        &bank,
        &th,
        pos2(area.min.x, bar_top),
        idle,
        true,
        u,
    );
}

/// 顶部与时间轴对齐的一条：已用时间 / 总时长 / 速度框 / 小节·拍·十六分。
/// 高度跟时间轴带一样（`timeline_band()`，不随缩放，见 viewport::RULER_SCALE），
/// 内部坐标统一按 `RULER_SCALE` 换算，和带内的刻度、指针保持同一档尺寸。
fn header(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    start: Pos2,
    band: f32,
    width: f32,
) {
    let u = RULER_SCALE;
    let p = ui.painter();
    p.rect_filled(
        Rect::from_min_size(start, vec2(width, timeline_band())),
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
            pos2(start.x + 9.0 * u, band - 2.0 * u),
            Align2::LEFT_TOP,
            elapsed,
            crate::fonts::scaled(info_med_bold_font(), u),
            th.text,
        );
        p.text(
            pos2(start.x + 26.0 * u, band + 15.0 * u),
            Align2::LEFT_TOP,
            length,
            crate::fonts::scaled(small_font(), u),
            th.text,
        );
    } else {
        p.text(
            pos2(start.x + 93.0 * u, band - 2.0 * u),
            Align2::RIGHT_TOP,
            elapsed,
            crate::fonts::scaled(info_med_bold_font(), u),
            th.text,
        );
        p.text(
            pos2(start.x + 93.0 * u, band + 15.0 * u),
            Align2::RIGHT_TOP,
            length,
            crate::fonts::scaled(small_font(), u),
            th.text,
        );
    }

    // 小节·拍·十六分：原版只画，不给交互（悬停出说明）。
    let sig = song.meta.time_signature.max(1) as f64;
    bank.tempo_box(
        p,
        pos2(start.x + 184.0 * u, band + 3.0 * u),
        false,
        th,
        u,
        state.canvas_hires,
    );
    let digits = crate::fonts::scaled(main_font(), u);
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
            pos2(start.x + x * u, band + 6.0 * u),
            Align2::RIGHT_TOP,
            t,
            digits.clone(),
            th.text,
        );
    }

    // 速度框：精灵宽 74，原版判定和文字定位都用左边 108（BPM 时往左挪 7px 画框）。
    // 就地输入框按单位换几何：t/s 用 (108, 57, 64)，BPM 用 (101, 57, 74)
    // （`draw_window_set_tempo.gml:14-21`）；点击 / 拖拽的判定框固定 (108, 57, 64, 22)。
    let (box_x, box_w) = if bpm_on { (101.0, 74.0) } else { (108.0, 64.0) };
    let mult = if bpm_on { 15.0 } else { 1.0 };
    bank.tempo_box(
        p,
        pos2(start.x + box_x * u, band + 3.0 * u),
        bpm_on,
        th,
        u,
        state.canvas_hires,
    );
    if state.tempo_input.is_none() {
        // 原版把数值文字整段包在 `if (window != w_settempo)` 里（`control_draw.gml:2453`）：
        // 就地框开着时不画数值，输入框直接盖在同一格上。
        let value = if bpm_on {
            format!("{:.2} BPM", shown * 15.0)
        } else {
            format!("{:.2} t / s", shown)
        };
        p.text(
            pos2(start.x + 136.0 * u, band + 6.0 * u),
            Align2::CENTER_TOP,
            value,
            digits.clone(),
            th.text,
        );
    }

    let idle = state.window.is_none() && state.context_menu.is_none();
    let bars_rect = Rect::from_min_size(
        pos2(start.x + 184.0 * u, band + 3.0 * u),
        vec2(64.0 * u, 22.0 * u),
    );
    ui.interact(bars_rect, Id::new("bars_box"), Sense::hover())
        .on_hover_text(tr(
            "Position of the marker in bars, beats and sixteenths.",
            "歌曲的进度（全音、四分、十六分）",
        ));

    let inline = state.tempo_input.take();
    if let Some(mut text) = inline {
        // 原版 `draw_inputbox(64, xx, 57 + song_tab_offset, w, …)`：框体高 21，
        // 而「上一次按下 / 这次抬手在不在框里」用的是同一条高 22 的矩形。
        let at = pos2(start.x + box_x * u, band + 3.0 * u);
        let gate = Rect::from_min_size(at, vec2(box_w * u, 22.0 * u));
        let (pressed, released, esc, hover) = ui.input(|i| {
            (
                i.pointer.button_pressed(PointerButton::Primary),
                i.pointer.button_released(PointerButton::Primary),
                i.key_pressed(Key::Escape),
                i.pointer.hover_pos(),
            )
        });
        let inside = hover.is_some_and(|h| gate.contains(h));
        if pressed {
            // 原版 `settempo = mouse_rectangle(xx, 57 + song_tab_offset, w, 22)`。
            state.tempo_press_in = inside;
        }
        let focus_id = Id::new("tempo_input_focus");
        let had_focus = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(focus_id))
            .unwrap_or(false);
        let nested = ui.allocate_new_ui(
            UiBuilder::new()
                .id_salt("tempo_input")
                .max_rect(Rect::from_min_size(at, vec2(box_w * u, 21.0 * u)))
                .layout(Layout::left_to_right(Align::Min)),
            |ui| {
                let re = ui.add(
                    TextEdit::singleline(&mut text)
                        .desired_width(box_w * u - 6.0 * u)
                        .font(digits.clone()),
                );
                if !had_focus {
                    re.request_focus();
                }
                re
            },
        );
        let re = nested.inner;
        // 提示走原版 `draw_inputbox` 里的 `popup_set_window(xx, yy, w, 21, str)`，
        // 两种语言的括号和斜杠两侧空格都不一样。
        let tip = if crate::lang() == 1 {
            format!(
                "输入新速度（{}）",
                if bpm_on {
                    "拍数 / 分钟"
                } else {
                    "红石刻 / 秒"
                }
            )
        } else {
            format!(
                "Enter the new tempo (in {})",
                if bpm_on {
                    "beats per minute"
                } else {
                    "ticks per second"
                }
            )
        };
        let re = re.on_hover_text(tip);
        // 原版的收口条件只有一条：这次按下不在框里、这次抬手也在框外
        // （`draw_window_set_tempo.gml:31`）。Enter 原样留给文本框、不算提交，
        // Esc 只丢弃不改速度（`:47-51`），两条都顺带把 `settempo` 清回 0。
        let finish_commit = released && !state.tempo_press_in && !inside;
        let focused = re.has_focus();
        // 关框时顺手清掉「已经抢过焦点」的暂存，下次开框才会重新去抢。
        ui.ctx()
            .data_mut(|d| d.insert_temp(focus_id, !finish_commit && !esc && focused));
        if finish_commit {
            state.tempo_press_in = false;
            commit_tempo_input(state, &text, mult);
        } else if esc {
            state.tempo_press_in = false;
        } else {
            state.tempo_input = Some(text);
        }
        return;
    }
    let hit = Rect::from_min_size(
        pos2(start.x + 108.0 * u, band + 3.0 * u),
        vec2(64.0 * u, 22.0 * u),
    );
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
        open_tempo_input(state, bpm_on);
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
            // 原版 `tempodrag += 0.25 * (mouse_yprev - mouse_y) / 3`，夹在 0.25..180
            // （`control_draw.gml:2531-2532`）。原版鼠标增量在「除以缩放」的房间坐标系里，
            // 屏幕 delta 先 `/u`。
            state.tempo_drag = Some((raw - dy / u * 0.25 / 3.0).clamp(0.25, 180.0));
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

/// 打开原版 `w_settempo` 的那一格就地输入框：初值就是
/// `string_format(real_tempo * 倍数, 0, 2)`（`draw_window_set_tempo.gml:4`）。
/// 原版每次开窗重算一遍 `var input`，靠 `text_exists[64] = 0` 才把新值灌进
/// `text_value[64]`，所以这里同样在开窗时重新播种。`settempo` 保持 0：它是全局量，
/// 开窗那一刻还没被更新过，挡住同帧提交的是「抬手落在框内」那条判定。
fn open_tempo_input(state: &mut EditorState, bpm_on: bool) {
    let mult = if bpm_on { 15.0 } else { 1.0 };
    state.tempo_input = Some(format!("{:.2}", state.doc.song.meta.tempo * mult));
    state.tempo_press_in = false;
}

/// 原版 `draw_window_set_tempo.gml:29-46` 的提交：逗号当小数点、其余只留数字和小数点，
/// 除回红石刻/秒，`>= 1000` 先除以 100，再夹进 `median(0.25, 值, 180)`。
/// 一个数字都不剩时原版的 `real()` 抛错走 catch 回滚，这里同样按原速度不动；
/// 原版只在值真的变了才置 `changed`，所以相等时连命令都不发。
fn commit_tempo_input(state: &mut EditorState, text: &str, mult: f32) {
    let old = state.doc.song.meta.tempo;
    let digits: String = text
        .replace(',', ".")
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut next = digits.parse::<f32>().map(|v| v / mult).unwrap_or(old);
    if next >= 1000.0 {
        next /= 100.0;
    }
    next = next.clamp(0.25, 180.0);
    if (next - old).abs() > f32::EPSILON {
        state.emit(Command::SetTempo(next));
    }
}

/// 原版图层名是常驻 `draw_text_edit` / `draw_textarea`（内容宽 72）：超框的字符断行
/// 到下一行，而框高只够一行，视觉上就是「只显示放得下的前缀」。非编辑态直接画文本
/// 没有框会溢出压到音量/声像图标，这里按同样的可视宽度截断。
fn clip_label(p: &egui::Painter, label: &str, font: &egui::FontId, max_w: f32) -> String {
    if p.layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x
        <= max_w
    {
        return label.to_owned();
    }
    let chars: Vec<char> = label.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cand: String = chars[..mid].iter().collect();
        if p.layout_no_wrap(cand, font.clone(), Color32::WHITE)
            .size()
            .x
            <= max_w
        {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect()
}

/// 一行图层。`at` 是原版行底左上角（已含 `ROW_LIFT`），内部坐标一律乘 `u`
/// （`u = cell / BASE_CELL`，与原版「整个画面等比缩放」同构）。
/// `exists` 为 false 表示原版那种「越界初始化出来的空白行」：框和图标照画，但没有图层可改。
#[allow(clippy::too_many_arguments)]
fn row(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    at: Pos2,
    cell: f32,
    u: f32,
    idx: u32,
    layer: &Layer,
    editing: &Option<(u32, String)>,
    idle: bool,
    shift: bool,
    show_vol: bool,
    mono: bool,
    exists: bool,
) {
    let x1 = at.x + X1 * u;
    let y1 = at.y;
    // 原版 `draw_sprite(spr_layerbox, …, x1, y1)`，行框同样从 x1 = 4 起。
    bank.layer_row(
        ui.painter(),
        pos2(x1, y1),
        vec2(256.0 * u, cell + 2.0 * u),
        th,
    );

    // 原版 `x1 + 126 - !realvolume - realstereo * 10`：藏掉字形时按钮只左挪 1px / 10px。
    let bx = |base: f32| {
        x1 + base * u - (if show_vol { 0.0 } else { 1.0 * u }) - (if mono { 0.0 } else { 10.0 * u })
    };

    // ---- 名称：原版的常驻输入框 ----
    let fluent = th.kind == ThemeKind::Fluent;
    let font = crate::fonts::scaled(if fluent { main_font() } else { small_font() }, u);
    let name_rect =
        Rect::from_min_size(pos2(x1 + 10.0 * u, y1 + 10.0 * u), vec2(75.0 * u, 13.0 * u));
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
            (x1 + 9.0 * u, y1 + 4.0 * u, 72.0 * u, 24.0 * u)
        } else {
            (x1 + 11.0 * u, y1 + 10.0 * u, 72.0 * u, 14.0 * u)
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
                        .font(font.clone()),
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
        let label = clip_label(ui.painter(), &label, &font, 72.0 * u);
        ui.painter().text(
            pos2(x1 + 11.0 * u, y1 + 10.0 * u),
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
        let hit = Rect::from_min_size(pos2(x1 + 88.0 * u, y1 + 5.0 * u), vec2(16.0 * u, 25.0 * u));
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
            pos2(x1 + 91.0 * u, y1 + 11.0 * u),
            vol,
            active,
            th,
            u,
            state.canvas_hires,
        );
        if active {
            ui.painter().text(
                pos2(x1 + 98.0 * u, y1 + 18.0 * u),
                Align2::CENTER_TOP,
                format!("{vol}%"),
                crate::fonts::scaled(small_font(), u),
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
                // 原版鼠标增量在「除以缩放」的房间坐标系里，屏幕 delta 先 `/u`。
                d.raw = (d.raw - delta / u).clamp(0.0, 100.0);
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
        let hit = Rect::from_min_size(pos2(x1 + 108.0 * u, y1 + 5.0 * u), vec2(16.0 * u, 25.0 * u));
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
            pos2(x1 + 110.0 * u, y1 + 11.0 * u),
            pan,
            active,
            th,
            u,
            state.canvas_hires,
        );
        let (tip, label) = pan_label(pan);
        if active {
            ui.painter().text(
                pos2(x1 + 116.0 * u, y1 + 18.0 * u),
                Align2::CENTER_TOP,
                label,
                crate::fonts::scaled(small_font(), u),
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
                d.raw = (d.raw - delta / u).clamp(0.0, 200.0);
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
    let by = y1 + 8.0 * u;
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(bx(126.0), by),
            layer_icon::LOCK,
            idx,
            tr("Lock this layer", "静音本层\n（拖拽可批量操作）"),
            layer.lock == LockMode::Locked,
            th,
            u,
            state.canvas_hires,
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
            layer_icon::SOLO,
            idx,
            tr("Solo this layer", "独奏本层\n（拖拽可批量操作）"),
            layer.lock == LockMode::Solo,
            th,
            u,
            state.canvas_hires,
        )
    {
        let next = if layer.lock == LockMode::Solo {
            LockMode::Normal
        } else {
            LockMode::Solo
        };
        state.emit(Command::SetLayerLock(idx, next));
    }
    if exists && idle && bank.layer_button(ui, pos2(bx(162.0), by), layer_icon::SELECT, idx, tr(
        "Select all note blocks in this layer\n(Click and drag to select multiple; click again to deselect)",
        "选择本层所有方块\n（拖拽可批量选择，再次点选取消选择）",
    ), false, th, u, state.canvas_hires) {
        state.emit(Command::SelectLayer(idx));
    }
    if idle
        && bank.layer_button(
            ui,
            pos2(bx(180.0), by),
            layer_icon::ADD,
            idx,
            tr(
                "Add empty layer here\n(Click and drag to add multiple layers)",
                "新建层\n（拖拽可批量添加）",
            ),
            false,
            th,
            u,
            state.canvas_hires,
        )
    {
        state.emit(Command::InsertLayer(idx));
    }
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(bx(198.0), by),
            layer_icon::REMOVE,
            idx,
            tr(
                "Remove this layer\n(Click and drag to remove multiple layers)",
                "删除层\n（拖拽可批量删除）",
            ),
            false,
            th,
            u,
            state.canvas_hires,
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
            layer_icon::UP,
            idx,
            tr(
                "Shift layer up\n(Click and drag to shift multiple layers)",
                "上移本层\n（拖拽可上移多层）",
            ),
            false,
            th,
            u,
            state.canvas_hires,
        )
    {
        state.emit(Command::MoveLayerUp(idx));
    }
    // 原版在最底层把下移按钮左挪 8px（少画的上移按钮位置就空在那儿）。
    let down_x = bx(234.0) - if idx == 0 { 8.0 * u } else { 0.0 };
    if exists
        && idle
        && bank.layer_button(
            ui,
            pos2(down_x, by),
            layer_icon::DOWN,
            idx,
            tr(
                "Shift layer down\n(Click and drag to shift multiple layers)",
                "下移本层\n（拖拽可下移多层）",
            ),
            false,
            th,
            u,
            state.canvas_hires,
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
#[allow(clippy::too_many_arguments)]
fn macro_bar(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    at: Pos2,
    idle: bool,
    wide: bool,
    u: f32,
) {
    if !state.flags.show_piano || state.doc.selection.is_empty() {
        return;
    }
    let x1 = at.x + X1 * u;
    let p = ui.painter();
    let fluent = th.kind == ThemeKind::Fluent;
    // 原版 1497-1511：show_layers 时 `y1 + 32`、高 1.3 × 30；收起时 `y1 + 52`、高 0.75 × 30。
    // Fluent 那两个圆角矩形是 38 和 21。宽度按 Fluent 的 255 / 380 取。
    let (w, h, dy) = if wide {
        (380.0 * u, if fluent { 21.0 * u } else { 22.5 * u }, 3.0 * u)
    } else {
        (255.0 * u, if fluent { 38.0 * u } else { 39.0 * u }, 5.0 * u)
    };
    let bg = Rect::from_min_size(pos2(x1, at.y), vec2(w, h));
    if fluent {
        p.rect_filled(bg, 2.0 * u, Color32::from_gray(70));
    } else {
        bank.icon_strip_bg(p, bg);
    }
    let row1 = pos2(x1 + 6.0 * u, at.y + dy);
    let row2 = pos2(row1.x, row1.y + 16.0 * u);
    let step = 25.0 * u;

    for btn in MACRO_BTNS {
        let i = btn.index;
        // 原版第一行画到第 9 个（淡出）为止，之后 `xx` 回到行首、`yy += 16`。
        let at = if i < 10 {
            pos2(row1.x + i as f32 * step, row1.y)
        } else {
            pos2(row2.x + (i - 10) as f32 * step, row2.y)
        };
        if idle
            && bank.macro_button(
                ui,
                at,
                i,
                tr(btn.tip_en, btn.tip_zh),
                th,
                u,
                state.canvas_hires,
            )
        {
            emit_macro(state, &btn.action);
        }
    }
}

/// 宏条按钮的动作分发。原版 `control_draw.gml:1640-1658` 每条都是
/// `{playing = 0 …}`：窗口走 `HotkeyWindow`、直接执行的宏走 `HotkeyMacro`，
/// 两者都会先停下播放，再开窗 / 跑宏。
fn emit_macro(state: &mut EditorState, action: &MacroAction) {
    match action {
        MacroAction::Window(kind) => state.emit(Command::HotkeyWindow(kind.clone())),
        MacroAction::Run(m) => state.emit(Command::HotkeyMacro(m.clone())),
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
        index: macro_icon::TREMOLO,
        tip_en: "Tremolo...",
        tip_zh: "Tremolo...",
        label_en: "Tremolo...",
        label_zh: "Tremolo…",
        action: MacroAction::Window(WindowKind::Tremolo),
    },
    MacroBtn {
        index: macro_icon::STEREO,
        tip_en: "Stereo...",
        tip_zh: "Stereo...",
        label_en: "Stereo...",
        label_zh: "Stereo…",
        action: MacroAction::Window(WindowKind::Stereo),
    },
    MacroBtn {
        index: macro_icon::ARPEGGIO,
        tip_en: "Arpeggio...",
        tip_zh: "Arpeggio...",
        label_en: "Arpeggio...",
        label_zh: "Arpeggio…",
        action: MacroAction::Window(WindowKind::Arpeggio),
    },
    MacroBtn {
        index: macro_icon::PORTAMENTO,
        tip_en: "Portamento...",
        tip_zh: "Portamento...",
        label_en: "Portamento...",
        label_zh: "Portamento…",
        action: MacroAction::Window(WindowKind::Portamento),
    },
    MacroBtn {
        index: macro_icon::VIBRATO,
        tip_en: "Vibrato",
        tip_zh: "Vibrato",
        label_en: "Vibrato",
        label_zh: "Vibrato",
        action: MacroAction::Run(M::Vibrato),
    },
    MacroBtn {
        index: macro_icon::STAGGER,
        tip_en: "Stagger...",
        tip_zh: "Stagger...",
        label_en: "Stagger...",
        label_zh: "Stagger…",
        action: MacroAction::Window(WindowKind::Stagger),
    },
    MacroBtn {
        index: macro_icon::CHORUS,
        tip_en: "Chorus",
        tip_zh: "Chorus",
        label_en: "Chorus",
        label_zh: "Chorus",
        action: MacroAction::Run(M::Chorus { detune: 5 }),
    },
    MacroBtn {
        index: macro_icon::VELOCITY_LFO,
        tip_en: "Volume LFO",
        tip_zh: "Volume LFO",
        label_en: "Volume LFO",
        label_zh: "Volume LFO",
        action: MacroAction::Run(M::VelocityLfo),
    },
    MacroBtn {
        index: macro_icon::FADE_IN,
        tip_en: "Fade in",
        tip_zh: "淡入",
        label_en: "Fade in",
        label_zh: "淡入",
        action: MacroAction::Run(M::FadeIn),
    },
    MacroBtn {
        index: macro_icon::FADE_OUT,
        tip_en: "Fade out",
        tip_zh: "淡出",
        label_en: "Fade out",
        label_zh: "淡出",
        action: MacroAction::Run(M::FadeOut),
    },
    MacroBtn {
        index: macro_icon::REPLACE_KEY,
        tip_en: "Replace key",
        tip_zh: "替换音",
        label_en: "Replace key",
        label_zh: "替换音",
        action: MacroAction::Run(M::ReplaceKey),
    },
    MacroBtn {
        index: macro_icon::SET_VELOCITY,
        tip_en: "Set velocity...",
        tip_zh: "设定音量......",
        label_en: "Set velocity...",
        label_zh: "设定音量…",
        action: MacroAction::Window(WindowKind::SetVelocity),
    },
    MacroBtn {
        index: macro_icon::SET_PANNING,
        tip_en: "Set panning...",
        tip_zh: "设定声道......",
        label_en: "Set panning...",
        label_zh: "设定声道…",
        action: MacroAction::Window(WindowKind::SetPanning),
    },
    MacroBtn {
        index: macro_icon::SET_PITCH,
        tip_en: "Set pitch...",
        tip_zh: "设定音高......",
        label_en: "Set pitch...",
        label_zh: "设定音高…",
        action: MacroAction::Window(WindowKind::SetPitch),
    },
    MacroBtn {
        index: macro_icon::RESET,
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
            let bpm_on = state.prefs.use_bpm;
            open_tempo_input(state, bpm_on);
        }
        // 退路没有标尺那条 32px 带，就地输入框只好画在按钮右边，提交条件也从
        // 原版的「抬手在框外」换成「失去焦点」；Esc 仍然是直接丢弃。
        let inline = state.tempo_input.take();
        if let Some(mut text) = inline {
            let re = ui.add(TextEdit::singleline(&mut text).desired_width(64.0));
            // 只在刚开框的那一帧抢焦点，否则「失焦即提交」根本不会被触发。
            let focus_id = Id::new("tempo_input_focus");
            let had_focus = ui
                .ctx()
                .data(|d| d.get_temp::<bool>(focus_id))
                .unwrap_or(false);
            if !had_focus {
                re.request_focus();
            }
            let focused = re.has_focus();
            ui.ctx().data_mut(|d| d.insert_temp(focus_id, focused));
            let bpm_on = state.prefs.use_bpm;
            let mult = if bpm_on { 15.0 } else { 1.0 };
            let esc = ui.input(|i| i.key_pressed(Key::Escape));
            if esc {
                state.tempo_press_in = false;
            } else if re.lost_focus() {
                state.tempo_press_in = false;
                commit_tempo_input(state, &text, mult);
            } else {
                state.tempo_input = Some(text);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::dispatch;

    /// 原版就地速度框那一次提交的规则（`draw_window_set_tempo.gml:29-46`）：
    /// 直接喂字符串进去，看最后写进歌曲的速度。`start` 是提交前的速度。
    fn committed(text: &str, mult: f32, start: f32) -> f32 {
        let mut state = EditorState::for_test();
        state.doc.song.meta.tempo = start;
        commit_tempo_input(&mut state, text, mult);
        for cmd in std::mem::take(&mut state.pending) {
            dispatch(&mut state, cmd, &egui::Context::default());
        }
        state.doc.song.meta.tempo
    }

    #[test]
    fn inline_tempo_commit_follows_the_original_parse_rules() {
        assert_eq!(committed("12.5", 1.0, 10.0), 12.5);
        // 逗号当小数点（`string_replace(input, ",", ".")`）。
        assert_eq!(committed("1,25", 1.0, 10.0), 1.25);
        // 先除回红石刻/秒：BPM 单位下输 150 就是 10 t/s。
        assert_eq!(committed("150", 15.0, 3.0), 10.0);
        // 一个数字都不剩：原版 `real()` 抛错走 catch，速度回到改之前。
        assert_eq!(committed("abc", 1.0, 10.0), 10.0);
        // 单位混在数字里也照旧取值（`string_digits_symbol(…, ".")`）。
        assert_eq!(committed("t8.5/s", 1.0, 10.0), 8.5);
        // 除完之后 >= 1000 先除以 100，再夹进 median(0.25, 值, 180)。
        assert_eq!(committed("1200", 1.0, 10.0), 12.0);
        assert_eq!(committed("800", 1.0, 10.0), 180.0);
        assert_eq!(committed("0", 1.0, 10.0), 0.25);
        // 和原值相等时原版不置 changed，端口连命令都不发。
        assert_eq!(committed("10", 1.0, 10.0), 10.0);
    }
}
