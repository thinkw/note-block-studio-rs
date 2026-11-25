//! D 模块的核心：超大滚动网格画布。
//!
//! 每帧只遍历 `[start_tick, start_tick + 可见列数)` × `[start_layer, +可见行数)`，
//! 音符查询走 [`nbs_core::model::Song::notes_in_view`]（`BTreeMap::range` + colfirst/collast
//! 早退），因此打开几十万 tick 的工程也不会因为「遍历全部音符」而卡死。
//! 底色与网格线先攒进 `Vec<Shape>` 一次 `Painter::extend` 提交，音符随后直接贴在它们上面
//! （原版 `control_draw` 就是这个顺序：线在下、方块在上）。

use egui::{
    pos2, Align2, Color32, CornerRadius, CursorIcon, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use nbs_core::model::{
    key_name, panning_velocity_to_short, Note, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN,
    PAN_CENTER, SOUND_STOPPER_NAME, TEMPO_CHANGER_NAME,
};

use crate::commands::Command;
use crate::fonts::{bold_font, info_big_font, info_med_font, main_font, small_font};
use crate::icons::BlockSet;
use crate::interaction::{DragMode, InputSnapshot};
use crate::layers_panel::time_str;
use crate::state::{ContextKind, ContextMenu, EditMode, EditorState};
use crate::theme::{Theme, ThemeKind};
use crate::tr;
use crate::viewport::{bar_thickness, timeline_band, Viewport, BAR_SCALE, RULER_SCALE};

/// 原版标尺刻度线的 `draw_set_alpha(0.6)`。
const RULER_A: u8 = 153;

/// 标尺秒标的候选档（秒）。原版 `c` 只按 tempo 在 0.25…30 里挑一档；端口把标签
/// 尺寸钉死后（#90），格距变小时还要顺这张表往上挑到放得下的那一档。
const SEC_STEPS: [f64; 11] = [
    0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
];

/// 秒标跳到哪一档：从原版按 tempo 挑出的 `c` 起，顺着 [`SEC_STEPS`] 找第一档
/// 「一格宽（`c × tempo × cell`）放得下一个标签」。跳到最大档仍放不下就用最大档。
/// 端口自加（用户要求 #90）：标签尺寸钉死后才需要它，原版连字一起缩放不会挤。
fn seconds_step(c: f64, tempo: f64, cell: f32, label_px: f64) -> f64 {
    let mut step = c;
    while step * tempo * (cell as f64) < label_px {
        let Some(next) = SEC_STEPS.iter().find(|s| **s > step) else {
            break;
        };
        step = *next;
    }
    step
}

/// tick 号的密度：原版每 4 格标一个，一格放不下就把步长按 4 的整数倍放宽。
/// 端口自加（用户要求 #90），同 [`seconds_step`]。
fn tick_label_step(label_px: f32, cell: f32) -> u32 {
    ((label_px / (cell * 4.0)).ceil().max(1.0) as u32) * 4
}

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
fn split(area: Rect, cell: f32, reference_row: bool) -> (Rect, Rect, Rect, Rect) {
    // 缩放只作用于格距；标尺带和滚动条各自钉死（见 `viewport::{BAR_SCALE, RULER_SCALE}`，
    // 偏离原版 #88 / #90：缩到 10% 时进度栏还是原来的高度、滚动条还抓得住）。
    let band = timeline_band();
    let bar = bar_thickness();
    let w = (area.width() - bar).max(64.0);
    let extra = if reference_row { cell } else { 0.0 };
    let h = (((area.height() - band - bar - extra) / cell)
        .floor()
        .max(1.0))
        * cell;
    let timeline = Rect::from_min_size(area.min, Vec2::new(w, band));
    let grid = Rect::from_min_size(pos2(area.min.x, area.min.y + band), Vec2::new(w, h));
    let hbar = Rect::from_min_size(
        pos2(area.min.x, area.min.y + band + h + extra),
        Vec2::new(w, bar),
    );
    let vbar = Rect::from_min_size(
        pos2(area.min.x + w, area.min.y + band),
        Vec2::new(bar, h + extra),
    );
    (timeline, grid, hbar, vbar)
}

pub fn show_grid(state: &mut EditorState, ui: &mut Ui, area: Rect) {
    // 原版 `control_draw.gml:308` 的巨型门：播放器（非下落模式）整块网格绘制与输入
    // 都不跑，画布只剩中央歌曲信息区（:2338-2420）。
    if state.player {
        show_player_info(state, ui, area);
        return;
    }
    let cell = state.viewport.cell();
    let (timeline_rect, grid_rect, hbar_rect, vbar_rect) =
        split(area, cell, state.flags.show_layers && !state.fullscreen);

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
    state.interaction.latest_pos = input.pointer;
    // 原版 `control_draw.gml:31` 每帧先把光标收回默认，再由命中的区域重新指定。
    // 少了这一步，上一帧网格给的光标会一直挂在整窗口上（#95）。拖拽中不收回：
    // 标尺手柄那几段每帧重申 `cr_size_we`，收了就永远画不出拉伸光标。
    if state.interaction.mode == DragMode::None {
        state.interaction.cursor = CursorIcon::Default;
    }

    handle_timeline_input(state, &input, timeline_rect, grid_rect);
    handle_grid_input(state, &input, grid_rect);
    handle_scrollbar_input(state, &input, hbar_rect, vbar_rect);

    draw_timeline(state, ui, timeline_rect);
    draw_grid(state, ui, grid_rect);
    if state.flags.show_layers && !state.fullscreen {
        let p = ui.painter();
        p.hline(
            grid_rect.x_range(),
            grid_rect.max.y - 1.0,
            egui::Stroke::new(1.0_f32, state.theme.grid_line),
        );
        let label = match state.reference.path.as_ref() {
            Some(path) => format!("{} {}", tr("Loaded file: ", "已加载音频: "), path.display()),
            None => tr("Click Reference to load audio", "点击「参考音频」打开文件").into(),
        };
        let font = crate::fonts::scaled(crate::fonts::main_font(), state.viewport.u());
        let label = crate::layers_panel::clip_label(p, &label, &font, grid_rect.width() - 30.0);
        p.text(
            pos2(
                grid_rect.min.x + 15.0,
                grid_rect.max.y - cell + 10.0 * state.viewport.u(),
            ),
            Align2::LEFT_TOP,
            label,
            font,
            state.theme.text,
        );
    }
    draw_scrollbars(state, ui, hbar_rect, vbar_rect);
    let idle = state.window.is_none() && state.context_menu.is_none();
    crate::layers_panel::macro_bar_over_grid(state, ui, area, idle);
    draw_context_menu(state, ui);

    // 只有指针真的落在中央画布上、且没有弹窗抢输入时，光标才归画布侧说。
    // 工具栏、状态栏、琴键和各个窗口里的控件自己会指定光标，这里无条件覆盖
    // 会把它们的手型全刷成画布那一个（#95）。
    let on_canvas = input.pointer.is_some_and(|p| area.contains(p))
        && state.window.is_none()
        && state.context_menu.is_none();
    if on_canvas {
        ui.output_mut(|o| o.cursor_icon = state.interaction.cursor);
    }
}

/// 原版 `get_seconds_from_tick`：沿变速点分段累计
/// （`entry[2] + (tick - entry[0]) / entry[1]`）。`points` 升序 `(tick, pitch)`，
/// 基速取歌曲 tempo；pitch≤0 的病态值按实时播放下限 0.25 t/s 兜底。
fn player_seconds_at(points: &[(i64, i16)], base_tempo: f64, tick: f64) -> f64 {
    let mut secs = 0.0;
    let mut prev = 0.0;
    let mut tps = base_tempo.max(0.01);
    for &(t, pitch) in points {
        if (t as f64) >= tick {
            break;
        }
        secs += (t as f64 - prev) / tps;
        prev = t as f64;
        tps = if pitch > 0 { pitch as f64 / 15.0 } else { 0.25 };
    }
    secs + (tick - prev) / tps
}

/// 原版播放器（非下落模式）的画布：`control_draw.gml:2338-2420`，外层条件
/// `!fullscreen && show_layers`。时间大字右对齐到 `centerx+70`（Fluent 左移 134）、
/// "/ 总长" 右对齐到 `centerx+70`（Fluent 左移 91），标题区左对齐到 `centerx-200`，
/// 进度拖条 `(centerx-200, centery+25)` 宽 400、上限 `enda + totalcols`。
/// 拖完 tempo 跟到落点段（`get_tempo_from_tick`）：端口的 seek 语义统一，
/// 播放推进遇到变速点自动改速，这里只挪播放头。
/// 原版 blackout 时文字换白（:286 draw_clear + isplayer 分支）——端口无此开关，省略。
fn show_player_info(state: &mut EditorState, ui: &mut Ui, area: Rect) {
    ui.allocate_rect(area, Sense::hover());
    if !state.flags.show_layers {
        return;
    }
    let th = state.theme.clone();
    let p = ui.painter().clone();
    let center = area.center();
    let fluent = th.kind == ThemeKind::Fluent;

    let tempo_events: Vec<(i64, i16)> = nbs_audio::build_event_table(&state.doc.song, false)
        .iter()
        .filter_map(|ev| match ev.kind {
            nbs_audio::EventKind::Tempo(pitch) => Some((ev.tick as i64, pitch)),
            _ => None,
        })
        .collect();
    let base = state.doc.song.meta.tempo as f64;
    let marker = state.marker();
    let enda = state.doc.song.enda as f64;
    // 原版 `totalcols` = 画布可见列数；拖条上限 = `enda + totalcols`（:2374）。
    let totalcols = (area.width() / state.viewport.cell()).floor() as f64;

    let big_x = center.x + if fluent { -64.0 } else { 70.0 };
    let med_x = center.x + if fluent { -21.0 } else { 70.0 };
    p.text(
        pos2(big_x, center.y - 50.0),
        Align2::RIGHT_TOP,
        time_str(player_seconds_at(&tempo_events, base, marker)),
        info_big_font(),
        th.text,
    );
    p.text(
        pos2(med_x, center.y - 20.0),
        Align2::RIGHT_TOP,
        format!(
            "/ {}",
            time_str(player_seconds_at(&tempo_events, base, enda))
        ),
        info_med_font(),
        th.text,
    );

    // 原版 title_str 链：歌名 → 下载名 → 文件名 → MIDI 名；端口只有歌名与文件名。
    let (title, author, orauthor) = {
        let song = &state.doc.song;
        let title = if song.meta.name.is_empty() {
            song.filename
                .as_ref()
                .and_then(|f| f.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            song.meta.name.clone()
        };
        (
            title,
            song.meta.author.clone(),
            song.meta.original_author.clone(),
        )
    };
    let mut offset = 0.0;
    if !author.is_empty() {
        offset += 20.0;
    }
    if !orauthor.is_empty() {
        offset += 15.0;
    }
    if !title.is_empty() {
        p.text(
            pos2(center.x - 200.0, center.y - 80.0 - offset),
            Align2::LEFT_TOP,
            title,
            info_med_font(),
            th.text,
        );
    }
    if !author.is_empty() {
        p.text(
            pos2(center.x - 200.0, center.y - 60.0 - offset),
            Align2::LEFT_TOP,
            author.clone(),
            bold_font(),
            th.text,
        );
    }
    if !orauthor.is_empty() {
        // 原版在画这一行前又对空作者补 15px（:2404）——间距 quirk，照抄。
        if author.is_empty() {
            offset += 15.0;
        }
        let prefix = tr("original by ", "原作者 ");
        p.text(
            pos2(center.x - 200.0, center.y - 45.0 - offset),
            Align2::LEFT_TOP,
            prefix,
            main_font(),
            Color32::from_gray(192),
        );
        let pw = p
            .layout_no_wrap(prefix.to_string(), main_font(), Color32::from_gray(192))
            .size()
            .x;
        p.text(
            pos2(center.x - 200.0 + pw, center.y - 45.0 - offset),
            Align2::LEFT_TOP,
            orauthor,
            bold_font(),
            th.text,
        );
    }

    player_dragbar(
        state,
        ui,
        &p,
        &th,
        center.x - 200.0,
        center.y + 25.0,
        400.0,
        marker,
        enda + totalcols,
        enda,
        &tempo_events,
        base,
    );
}

/// 原版 `draw_dragbar`（`control_draw.gml:2374`）：经典档轨道 2px + DRAG_KNOB 手柄，
/// Fluent 档轨道 3px + 圆点；按住拖动 seek，手柄热区 13×13。悬停提示
/// 「Song Position/当前位置」；拖动中的时间提示按 `enda` 域现算（原版 :2376）。
#[allow(clippy::too_many_arguments)]
fn player_dragbar(
    state: &mut EditorState,
    ui: &Ui,
    p: &Painter,
    th: &Theme,
    x: f32,
    y: f32,
    len: f32,
    value: f64,
    max: f64,
    enda: f64,
    tempo_events: &[(i64, i16)],
    base_tempo: f64,
) {
    let fluent = th.kind == ThemeKind::Fluent;
    let h = if fluent { 3.0 } else { 2.0 };
    let gray = Color32::from_gray(154);
    let progress = (value / max).clamp(0.0, 1.0);
    let knob = pos2(x + (progress * f64::from(len)) as f32, y + 1.0);
    p.rect_filled(
        Rect::from_min_size(pos2(x, y), Vec2::new(len, h)),
        0.0,
        gray,
    );
    if fluent {
        p.rect_filled(
            Rect::from_min_size(pos2(x, y), Vec2::new((progress * f64::from(len)) as f32, h)),
            0.0,
            th.accent,
        );
        p.circle_filled(knob, 10.0, Color32::WHITE);
        p.circle_filled(knob, 5.0, th.accent);
    } else if let Some(bank) = state.icons.bank() {
        // 经典主题的手柄是 `spr_icons` 第 9 帧（draw_dragbar 内 draw_sprite(..., 9, ...)）。
        bank.icon_glyph(
            p,
            knob - Vec2::new(12.0, 11.0),
            crate::icons::frame::DRAG_KNOB,
        );
    }
    let track = Rect::from_min_size(pos2(x - 3.0, y - 2.0), Vec2::new(len + 6.0, 6.0 + h - 2.0));
    let hot = Rect::from_center_size(knob, Vec2::splat(13.0));
    let resp = ui.interact(
        track.union(hot),
        egui::Id::new("player_position"),
        Sense::click_and_drag(),
    );
    if resp.dragged() || resp.clicked() {
        if let Some(pt) = resp.interact_pointer_pos() {
            // 原版出界直接钳到 0 / max。
            let frac = ((pt.x - x) / len).clamp(0.0, 1.0);
            state.set_marker(frac as f64 * max);
        }
    }
    let tip = if resp.dragged() {
        let t = resp
            .interact_pointer_pos()
            .map(|pt| f64::from(((pt.x - x) / len).clamp(0.0, 1.0)) * enda)
            .unwrap_or(0.0);
        time_str(player_seconds_at(tempo_events, base_tempo, t))
    } else {
        tr("Song Position", "当前位置").to_string()
    };
    resp.on_hover_text(tip);
}

// ---------------------------------------------------------------- 输入处理

/// 原版标尺/播放头共用的取列式：`starta + (mouse_x - (x1 + 2)) / 32`
/// （`control_draw.gml:171/175/177/1103/1113`）。不带 `note_offset` —— 那一项只在
/// 播放跟随到「按帧」时才非零，而这一整段命中判定要求停播（`:124`）。
fn ruler_tick(vp: &Viewport, x: f32) -> f64 {
    vp.start_tick as f64 + (x - vp.origin.x) as f64 / vp.cell() as f64
}

/// 播放头所在的原始横坐标（逻辑像素，可能带小数）：`starta + (marker_pos - starta) * 32`
/// 那条式子的端口写法。命中判定（`handle_timeline_input`）要的是这个连续值，
/// 画贴图不能用它 —— 见 [`marker_screen_x`]。
fn marker_x(state: &EditorState) -> f32 {
    let vp = &state.viewport;
    let m = state.marker().max(0.0);
    vp.tick_x(m.floor() as i64) + (m.fract() as f32) * vp.cell()
}

/// 把逻辑坐标吸附到整物理像素，和 #114 网格竖线（`draw_grid` 里那条 `snap`）同一个口径。
fn snap_px(v: f32, ppi: f32) -> f32 {
    (v * ppi).round() / ppi
}

/// 传给 `draw_sprite` 的横坐标：吸附到整物理像素后的 [`marker_x`]。
///
/// 原版是 `floor(current_song.marker_pos * 32 - current_song.starta * 32)`
/// （`control_draw.gml:1170`）—— 落到整数画布像素，上屏时才整体缩放，所以指针在任何
/// 缩放档下都只有贴图那两根实列宽。端口按物理坐标直接画，且播放中 `marker` 是连续
/// tick 数，落点带小数；`spr_marker` 的第 1 帧是 16×15、只有 x=7/x=8 两列不透明的位图
/// （原点 x=8，正好骑在落点两侧各 1 像素），而这条带用的是 LINEAR 采样，于是那两根
/// 实列会被摊成 3~4 列渐变（实测 f≈0.35 时三列 alpha 约 38/255/217）—— 用户看到的
/// 「指针后面跟着一条浅蓝色的线」就是被摊开的那半。吸附后每帧最多跳 1 物理像素、
/// 位置误差 ≤0.5 像素；原版是取整（截断），这里是四舍五入，方向一致、比它更准。
///
/// 时间轴带内和网格内两个绘制点必须调用同一个函数拿同一个值：箭头帽和竖线本来共用
/// 一个 `mx`，一边吸一边不吸就会错开 1 像素。
fn marker_screen_x(state: &EditorState, ppi: f32) -> f32 {
    snap_px(marker_x(state), ppi)
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

fn handle_timeline_input(state: &mut EditorState, input: &InputSnapshot, rect: Rect, grid: Rect) {
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
                    state.interaction.cursor = CursorIcon::ResizeHorizontal;
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
    // 三个手柄（`:132/141/148`）的命中框是 16px 宽 × **整列高** —— 标尺带之外，
    // 音符区里贴着播放头/区间线同样能抓住拖，抓住后音符编辑让位。
    let Some(pos) = input.pointer.filter(|p| rect.union(grid).contains(*p)) else {
        return;
    };
    if canvas_blocked(state) || state.is_playing() || state.interaction.mode != DragMode::None {
        return;
    }
    // 原版 `:132/141/148`：三个 16px 命中框，播放头优先、区间两端让位（`cursmarker`）。
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
        if (pos.x - x).abs() < 8.0 * RULER_SCALE {
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
    // 右键菜单和「按下记列」只认标尺带那 32px（原版 `:155/:166/:168` 的矩形高度都是 32）；
    // 音符区里的右键与按下交给网格侧处理。
    let Some(band) = input.pointer.filter(|p| rect.contains(*p)) else {
        return;
    };
    if input.secondary_pressed {
        state.context_menu = Some(ContextMenu {
            pos: band,
            cell: (state.viewport.x_tick(band.x).max(0) as u32, 0),
            kind: ContextKind::Timeline,
        });
        return;
    }
    if input.primary_pressed && !on_handle {
        // 原版 `:171`：按下只记下那一列（`timeline_pressa`），松手或拖开才有下文。
        state.interaction.mode = DragMode::PressTimeline {
            pressa: ruler_tick(&state.viewport, band.x).floor() as i64,
        };
    }
}

/// 原版用一个全局 `window` 变量表示「最前面有东西挡着」，画布侧的每一段输入都带
/// `window = 0` 这道门：滚轮换音色/换键在 `control_draw.gml:227`、悬停音符改属性在
/// :125 的总闸里、滚动条的滚轮和拖拽在 `draw_scrollbar.gml:22/51/161/188`、
/// 标尺手柄在 `:132-151`；工具栏音色框展开也写成 `window = w_insbox`（:1975）。
/// 端口把这几样拆成了四个字段，这里合成回同一个判断。
/// 第四条 `menubar_open` 是顶部菜单的下拉：原版走的是同一个 `window += w_menu`
/// （show_menu_ext.gml:21），egui 的菜单不认画布这套裸输入，只能自己补上这道门
/// （用户要求 #129）。
fn canvas_blocked(state: &EditorState) -> bool {
    state.window.is_some()
        || state.insbox_open
        || state.context_menu.is_some()
        || state.menubar_open
}

fn handle_grid_input(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    state.interaction.click_in_grid = false;

    let pointer_cell = input
        .pointer
        .filter(|p| rect.contains(*p))
        .and_then(|p| state.viewport.cell_at(p));
    state.hover_cell = pointer_cell.map(|(t, l)| (t, l));
    let hover = pointer_cell.map(|(t, l)| (t as i64, l as i64));

    let blocked = canvas_blocked(state);
    let interactive = hover.is_some() && !blocked;
    let pointer_in_grid = input.pointer.is_some_and(|p| rect.contains(p));

    // 滚轮：悬停音符调属性 / 按 `mousewheel` 档位决定是平移、换音色还是换键。
    // Ctrl+滚轮不在这里处理——原版把它归在键盘快捷键段（`control_draw.gml:736-751`），
    // 改的是整窗口缩放，见 `shortcuts.rs`。
    if !blocked
        && pointer_in_grid
        && !input.ctrl
        && (input.scroll.x.abs() > 0.0 || input.scroll.y.abs() > 0.0)
    {
        let delta = input.scroll.y + input.scroll.x;
        let d = if delta > 0.0 { 1 } else { -1 };
        // 原版条件 `exist && changepitch`：停在音符上且开了「滚轮改音符属性」才改那颗音。
        // 播放中额外禁掉：原版 `:211` 的 `playing = 0` 总闸只冻住 sela/selb 的刷新，
        // `:276` 的 `selbx = starta + sela` 却照用自动平移后的 starta，于是滚轮改的是
        // 「鼠标上一次指过的那一列」而非指针底下那颗 —— 端口按指针算，改到的确实是
        // 指针下那颗，但播放中滚轮本来就只该平移视口，碰音符是误伤。
        let note_cell = hover.filter(|(t, l)| {
            state.prefs.changepitch
                && !state.is_playing()
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
            scroll_bars(state, input, state.viewport.cell(), BarAxis::Both);
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
            // 原版框选进行中是 `cr_handpoint`（`control_draw.gml:904`），不是十字。
            state.interaction.cursor = CursorIcon::PointingHand;
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
            // 播放头/区间/标尺按下进行中：光标由标尺侧定（`cr_size_we`），网格别抢。
            if state.interaction.mode == DragMode::None {
                // 原版 `cr_cross` 全文没用过，网格上是 `cr_handpoint`
                // （`control_draw.gml:125-129`）；用户定案空格要箭头，
                // 只有落在音符或选区上才换手型 / 抓取。
                if state.doc.selection.contains(t, l) {
                    state.interaction.cursor = CursorIcon::Grab;
                } else if state.doc.song.has(cell.0, cell.1) {
                    state.interaction.cursor = CursorIcon::PointingHand;
                } else {
                    state.interaction.cursor = CursorIcon::Default;
                }
            }
        }
    }

    // ---- 松开 ----
    finish_drag(state, input, Some((t, l)));
}

/// 拖拽越过网格边界时自动滚动（原版 dragincxr/xl/yd/yu 的等价实现）。
fn edge_scroll(state: &mut EditorState, input: &InputSnapshot, rect: Rect) {
    let Some(pos) = input.pointer else { return };
    let edge = 28.0 * state.viewport.u();
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
    let mut moved_x = 0i64;
    let mut moved_y = 0i64;
    if ix != 0 {
        let before = state.viewport.start_tick as i64;
        state.viewport.start_tick = (before + ix).max(0) as u32;
        e.x -= ix as f32;
        moved_x = state.viewport.start_tick as i64 - before;
    }
    if iy != 0 {
        let before = state.viewport.start_layer as i64;
        state.viewport.start_layer = (before + iy).max(0) as u32;
        e.y -= iy as f32;
        moved_y = state.viewport.start_layer as i64 - before;
    }
    // 原版 `control_draw.gml:1092-1095`：拖框期间视口被边缘滚动推动时，
    // 按下像素跟着平移（`select_pressx += 32 * (a - starta)`），
    // 矩形才不会锚在屏幕上漂。滚轮滚动原版不补偿，这里也不补。
    if (moved_x != 0 || moved_y != 0)
        && matches!(state.interaction.mode, DragMode::BoxSelect { .. })
    {
        let cell = state.viewport.cell();
        if let Some(pos) = state.interaction.pressed_pos.as_mut() {
            pos.x -= moved_x as f32 * cell;
            pos.y -= moved_y as f32 * cell;
        }
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

/// 原版 `draw_scrollbar.gml:14-17` 的三个量。`track` = 滑轨像素长（原版 `mwh = swh * s`），
/// `visible` = 一屏可见槽数 `s`，`tail` = 最后一个含音符的槽号（原版第五参里多出来的那截，
/// 即 `enda` / `endb`）：
/// - 总槽 `ms = tail + visible`。原版调用点写的是 `enda + totalcols - 1` 配
///   `s = totalcols - 1`，两边各减 1 抵消，化简后值域上限就是 `enda`；
/// - 滑块长 `(visible / ms) * track`，下限用 #88 的恒定尺寸而不是原版的 10 像素；
/// - 上限 `tail`，就是原版那句 `median(0, sb_val, ms - s)`。
/// 返回 `(ms, 滑块长, 上限)`。
fn bar_metrics(track: f32, visible: f32, tail: f32) -> (f32, f32, f32) {
    let visible = visible.max(1.0);
    let ms = (tail + visible).max(1.0);
    let thumb = ((visible / ms) * track)
        .max(24.0 * BAR_SCALE)
        .min(track.max(0.0));
    (ms, thumb, tail)
}

/// 横向条几何，返回 `(ms, 上限, 滑块矩形)`。滑块位置用 `(val / ms) * 滑轨长`，
/// 分母是整条滑轨而不是「滑轨减滑块」—— 原版同款（`bpos = (sb_val / ms) * mwh`）。
/// 这两者必须一致，否则滑块每走一段就比指针少一点，从头拖到尾会偏出一截（#93.1）。
fn h_geometry(state: &EditorState, rect: Rect) -> (f32, f32, Rect) {
    let (ms, thumb_w, max_val) = bar_metrics(
        rect.width(),
        state.viewport.columns() as f32,
        state.doc.song.enda as f32,
    );
    let val = (state.viewport.start_tick as f32).clamp(0.0, max_val);
    (
        ms,
        max_val,
        Rect::from_min_size(
            pos2(
                rect.min.x + (val / ms) * rect.width(),
                rect.min.y + 2.0 * BAR_SCALE,
            ),
            Vec2::new(thumb_w, rect.height() - 4.0 * BAR_SCALE),
        ),
    )
}

/// 纵向条几何，同 `h_geometry`，只是轴换成上下、末尾槽号换成 `endb`。
/// 方向与原版一致：滑块越往下 `startb` 越大，也就是层号往上走
/// （`control_draw.gml:339` 的行号 `startb + b` 随屏幕 y 增大）。
fn v_geometry(state: &EditorState, rect: Rect) -> (f32, f32, Rect) {
    let reference_rows = if state.flags.show_layers && !state.fullscreen {
        1.0
    } else {
        0.0
    };
    let (ms, thumb_h, max_val) = bar_metrics(
        rect.height(),
        state.viewport.rows() as f32,
        state.doc.song.endb as f32 + reference_rows,
    );
    let val = (state.viewport.start_layer as f32).clamp(0.0, max_val);
    (
        ms,
        max_val,
        Rect::from_min_size(
            pos2(
                rect.min.x + 2.0 * BAR_SCALE,
                rect.min.y + (val / ms) * rect.height(),
            ),
            Vec2::new(rect.width() - 4.0 * BAR_SCALE, thumb_h),
        ),
    )
}

/// 滑轨上的位置比例 → 槽号：与滑块定位同一条式子（`ratio * ms`），所以指针拖多少
/// 滑块就走多少；再夹到原版的 `[0, ms - s]`，最后取整（原版 `starta` / `startb` 也是整数）。
fn bar_value(ratio: f32, ms: f32, max_val: f32) -> u32 {
    (ratio * ms).clamp(0.0, max_val).round() as u32
}

/// 这一帧的滚动归哪根条管。原版只有纵向滚轮（`sb_sel = i` 决定给哪根），
/// 触控板的双轴是 macOS 分支独有的。
#[derive(Clone, Copy, PartialEq)]
enum BarAxis {
    /// 网格区：x 给横向条、y 给纵向条。
    Both,
    Horizontal,
    Vertical,
}

/// 滚轮 / 触控板平移视口，三件事都在这一个函数里（#93.2 / #93.3 / #93.4）：
/// - **两轴各走各的条**：横向双指滑只动横向条，斜着滑两条一起动。原版
///   `control_draw.gml:1212-1224` 的触控板段就是两个并列的 `if`，`dx` 归 `starta`、
///   `dy` 归 `startb`。此前这里把 `x + y` 合成一个标量，横滑会顺带把纵向也推走。
/// - **方向取负**：winit/egui 的正值是「内容往下走、看到更早的内容」
///   （`winit/src/event.rs:957` 的 `LineDelta` 文档），而原版是
///   `sb_val += (mouse_wheel_down - mouse_wheel_up) * wheel_speed`
///   （向下滚 = 值增大，`draw_scrollbar.gml:24`），触控板段同样是 `start -= delta / 32`。
///   原来直接把 delta 加到 `start` 上，竖向就是反的。
/// - **夹到原版的 `enda` / `endb`**。若当前值已经越界（别的通路先推过去的），
///   只许往回收、不许再往外推，免得一个滚轮格把视口猛拽回去。
fn scroll_bars(state: &mut EditorState, input: &InputSnapshot, cell: f32, axis: BarAxis) {
    let wheel = input.scroll;
    let fine = input.scroll_fine;
    let speed = state.prefs.mousewheel_scroll_speed.clamp(1, 10) as f32;
    // egui 只把「Point 型、单事件不足 8px」的滚动填进 `smooth_scroll_delta`，
    // 所以它非零就是触控板细滑（像素），否则是鼠标滚轮（整格）。
    let (touch, src) = if fine == Vec2::ZERO {
        (false, wheel)
    } else {
        (true, fine)
    };
    if src == Vec2::ZERO {
        return;
    }
    // 触控板按原版的 `delta / 32` 换算成槽（这里的 32 就是当前格距），滚轮一格走
    // `wheel_speed` 个槽。
    let step = |d: f32| -> f32 {
        if d == 0.0 {
            0.0
        } else if touch {
            -d / cell.max(1.0)
        } else {
            -d.signum() * speed
        }
    };
    let (hx, vy) = match axis {
        // Shift + 滚轮让纵向那格去走横向条：端口既有约定，原版没有这一条。
        BarAxis::Both if input.shift => (step(src.y), 0.0),
        BarAxis::Both => (step(src.x), step(src.y)),
        BarAxis::Horizontal => (step(if src.x != 0.0 { src.x } else { src.y }), 0.0),
        BarAxis::Vertical => (0.0, step(if src.y != 0.0 { src.y } else { src.x })),
    };
    // 触控板一次几个像素、连一格都凑不满：原版用浮点累加器
    // `macos_scroll_temp_starta/b` 攒着，端口同样先攒后动，否则慢速滑动会被取整成 0。
    let carry = &mut state.interaction.scroll_carry;
    let drain = |c: &mut f32, d: f32| -> i64 {
        *c += d;
        let whole = c.trunc();
        *c -= whole;
        whole as i64
    };
    let (dx, dy) = (drain(&mut carry.x, hx), drain(&mut carry.y, vy));
    let cur = (
        state.viewport.start_tick as i64,
        state.viewport.start_layer as i64,
    );
    let hi = (
        (state.doc.song.enda as i64).max(cur.0),
        (state.doc.song.endb as i64).max(cur.1),
    );
    let got = (cur.0 + dx, cur.1 + dy);
    let clamped = (got.0.max(0).min(hi.0), got.1.max(0).min(hi.1));
    // 撞到边界就别再把小数攒着，否则反向时会先吐出一次超程。
    if clamped.0 != got.0 {
        carry.x = 0.0;
    }
    if clamped.1 != got.1 {
        carry.y = 0.0;
    }
    state.viewport.start_tick = clamped.0 as u32;
    state.viewport.start_layer = clamped.1 as u32;
}

fn handle_scrollbar_input(state: &mut EditorState, input: &InputSnapshot, hbar: Rect, vbar: Rect) {
    let Some(pos) = input.pointer else { return };
    // 原版 `draw_scrollbar.gml` 的滚轮段和拖拽段都带 `(window = 0 || win = 1)`，
    // 而主界面两根条传的 `win` 恒为 0，所以有窗口挡着时条既不能滚也不能点。
    if canvas_blocked(state) {
        return;
    }
    let (hm, hmax, ht) = h_geometry(state, hbar);
    let (vm, vmax, vt) = v_geometry(state, vbar);

    if input.primary_pressed {
        if hbar.contains(pos) {
            // 原版 `draw_scrollbar.gml:54`：按到哪根条就把 `sb_sel` 置成那条的号，之后
            // Home / End / PageUp / PageDown 一直翻这一根，直到碰到另一根。
            state.sb_sel = 0;
            if ht.contains(pos) {
                state.interaction.mode = DragMode::DragScrollbarH {
                    grab: pos.x - ht.min.x,
                };
            } else {
                let ratio = (pos.x - hbar.min.x) / hbar.width();
                state.viewport.start_tick = bar_value(ratio, hm, hmax);
            }
        } else if vbar.contains(pos) {
            state.sb_sel = 1;
            if vt.contains(pos) {
                state.interaction.mode = DragMode::DragScrollbarV {
                    grab: pos.y - vt.min.y,
                };
            } else {
                let ratio = (pos.y - vbar.min.y) / vbar.height();
                state.viewport.start_layer = bar_value(ratio, vm, vmax);
            }
        }
    }
    if input.primary_down {
        match state.interaction.mode.clone() {
            DragMode::DragScrollbarH { grab } => {
                let ratio = (pos.x - grab - hbar.min.x) / hbar.width();
                state.viewport.start_tick = bar_value(ratio, hm, hmax);
            }
            DragMode::DragScrollbarV { grab } => {
                let ratio = (pos.y - grab - vbar.min.y) / vbar.height();
                state.viewport.start_layer = bar_value(ratio, vm, vmax);
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

    // #93.3：指针停在哪根条上，这一帧的滚动就只走那一根。原版这个门是
    // `draw_scrollbar.gml:22-27` 的 `sb_sel = i`（只有刚碰过这根条才吃滚轮）；
    // Windows 侧本来没有横滚事件，端口把它做成「悬停横向条 = 横向条接住两轴」。
    if !input.primary_down {
        let cell = state.viewport.cell();
        if hbar.contains(pos) {
            scroll_bars(state, input, cell, BarAxis::Horizontal);
        } else if vbar.contains(pos) {
            scroll_bars(state, input, cell, BarAxis::Vertical);
        }
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
    shapes.push(Shape::rect_filled(ht, 3.0 * BAR_SCALE, th.accent_soft));
    shapes.push(Shape::rect_stroke(
        ht,
        3.0 * BAR_SCALE,
        Stroke::new(1.0_f32 * BAR_SCALE, th.accent),
        StrokeKind::Inside,
    ));
    shapes.push(Shape::rect_filled(vt, 3.0 * BAR_SCALE, th.accent_soft));
    shapes.push(Shape::rect_stroke(
        vt,
        3.0 * BAR_SCALE,
        Stroke::new(1.0_f32 * BAR_SCALE, th.accent),
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
    // 偏离原版（用户要求 #90）：带内的装饰尺寸——刻度数字、刻度线、区间帽、
    // 播放头的箭头帽和竖线宽度——钉在 100% 档，只有横向的 tick 位置跟格距走。
    // 原版整条标尺带连字一起乘 `window_scale`，缩到 10% 时指针不到 1px 就消失了。
    let u = RULER_SCALE;
    let meta = &state.doc.song.meta;
    let tempo = (meta.tempo.max(0.01)) as f64;
    let top = rect.min.y;
    // 背景必须当场画：下面两类文字（秒标、刻数）是 `p.text` 直接落笔的，
    // 留在 `shapes` 里到函数末尾才 `p.extend`，这块不透明背景就会把它们整个盖掉（#96）。
    p.rect_filled(rect, 0.0, th.ruler_bg);
    let mut shapes = Vec::new();

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
    // 端口自加（用户要求 #90）：秒标的字尺寸钉死后，界面一缩小平铺的「一秒一格」
    // 会把数字挤成一坨，所以按 `seconds_step` 把档位往上跳。
    let sec_gap = p
        .layout_no_wrap(
            time_str(600.0),
            crate::fonts::scaled(small_font(), u),
            th.text,
        )
        .size()
        .x as f64
        + 8.0;
    c = seconds_step(c, tempo, cell, sec_gap);
    let ticks_per_step = tempo * c;
    let px_per_step = (ticks_per_step * cell as f64) as f32;
    let starta = vp.start_tick as f64;
    let whole = (starta / ticks_per_step).ceil();
    let x0 = vp.tick_x(vp.start_tick as i64);
    let mut sec = whole * c;
    let mut xx = (whole - starta / ticks_per_step) as f32 * px_per_step;
    let mut first = true;
    while xx < rect.width() + 16.0 * u {
        let x = x0 + xx;
        p.text(
            pos2(x, top),
            if first {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            },
            time_str(sec),
            crate::fonts::scaled(small_font(), u),
            th.text,
        );
        first = false;
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(pos2(x, top + 12.0 * u), Vec2::new(1.0, 3.0 * u)),
            0.0,
            alpha(th.text, RULER_A),
        ));
        xx += px_per_step;
        sec += c;
    }

    // 同上（#90）：tick 号原样是「每 4 格标一个」，标签尺寸钉死后按 4 的整数倍放宽，
    // 免得数字在缩小的格距上叠成一团。刻度线本身跟格距走，密度不动。
    let num_w =
        p.layout_no_wrap(
            "0000".to_string(),
            crate::fonts::scaled(small_font(), u),
            th.text,
        )
        .size()
        .x + 4.0;
    let num_every = tick_label_step(num_w, cell);
    // 原版 `for (a = 0; a <= totalcols)`：刻度线 + 每 4 tick 一个的 tick 号。
    for a in 0..=vp.columns() {
        let tick = vp.start_tick + a;
        let x = vp.tick_x(tick as i64);
        if x < rect.min.x - 1.0 || x > rect.max.x + 1.0 {
            continue;
        }
        let by4 = tick % 4 == 0;
        let h = if by4 { 7.0 * u } else { 4.0 * u };
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(pos2(x, top + 31.0 * u - h), Vec2::new(1.0, h)),
            0.0,
            alpha(th.text, RULER_A),
        ));
        if tick % num_every == 0 {
            p.text(
                pos2(x, top + 15.0 * u),
                if a == 0 {
                    Align2::LEFT_TOP
                } else {
                    Align2::CENTER_TOP
                },
                tick.to_string(),
                crate::fonts::scaled(small_font(), u),
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
            let galley = p.layout_no_wrap(
                format!("BPM = {}", n.pitch.abs()),
                crate::fonts::scaled(main_font(), u),
                th.text,
            );
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
                    pos2(xx + 10.0 * u, rect.min.y + 4.0 * u),
                    pos2(xx + 10.0 * u + w, rect.min.y + 16.0 * u),
                ),
                0.0,
                fill,
            );
            bank.area(
                &p,
                Rect::from_min_max(
                    pos2(xx + 9.0 * u, rect.min.y + 3.0 * u),
                    pos2(xx + 11.0 * u + w, rect.min.y + 17.0 * u),
                ),
                th,
                u,
            );
            let dark_marker = th.kind == ThemeKind::Dark
                || state.flags.blackout
                || (th.kind == ThemeKind::Fluent && th.fdark);
            let f = 8 * dark_marker as usize;
            let at = pos2(xx, rect.min.y + 2.0 * u);
            bank.marker(&p, at, f, Vec2::new(16.0 * u, 15.0 * u), Color32::WHITE);
            bank.marker(
                &p,
                pos2(xx, rect.min.y + 17.0 * u),
                f + 1,
                Vec2::new(16.0 * u, 15.0 * u),
                Color32::WHITE,
            );
            // 原版 `draw_text(xx + 10, y1 + 2 + (theme != 3), …)`，对齐当时是 fa_left。
            p.galley(
                pos2(
                    xx + 10.0 * u,
                    rect.min.y
                        + if th.kind == ThemeKind::Fluent {
                            2.0 * u
                        } else {
                            3.0 * u
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
            Rect::from_min_max(pos2(x0.min(x1), top), pos2(x0.max(x1), top + 31.0 * u)),
            0.0,
            alpha(C_BLUE, 64),
        );
        if let Some(bank) = state.icons.bank() {
            for (x, cap) in [(x0, 2 + 2 * rev), (x1, 4 - 2 * rev)] {
                bank.marker(
                    &p,
                    pos2(x, top),
                    cap,
                    Vec2::new(16.0 * u, 15.0 * u),
                    Color32::WHITE,
                );
                bank.marker(
                    &p,
                    pos2(x, top),
                    cap + 1,
                    Vec2::new(16.0 * u, rect.max.y - top),
                    Color32::WHITE,
                );
            }
        }
    }

    // 播放头：原版在时间轴顶端画 spr_marker 的箭头帧，细身帧一路拉满整个网格
    // （贴图原点在线上，见 `IconBank::marker`）。网格那半截见 draw_grid。
    let mx = marker_screen_x(state, ui.ctx().pixels_per_point());
    if let Some(bank) = state.icons.bank() {
        if mx >= rect.min.x - 8.0 * u && mx <= rect.max.x + 8.0 * u {
            let tint = if th.kind == ThemeKind::Fluent {
                th.accent(3)
            } else {
                Color32::WHITE
            };
            let at = pos2(mx, rect.min.y + 2.0 * u);
            bank.marker(
                &p,
                at,
                marker_frame(th, state.flags.blackout),
                Vec2::new(16.0 * u, 15.0 * u),
                tint,
            );
            bank.marker(
                &p,
                at,
                marker_frame(th, state.flags.blackout) + 1,
                Vec2::new(16.0 * u, rect.max.y - at.y),
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
    let u = vp.u();
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

    // 竖线：原版 `control_draw.gml:311-321` 只有三种 —— 小节线 alpha 0.3、整 4 tick 0.25、
    // 其余 0.1，颜色一律是黑，画布像素宽分别 2 / 1 / 1。原版没有横线。
    if state.flags.show_grid_lines && !blackout {
        let ppi = ui.ctx().pixels_per_point();
        // 原版 `window_scale` 是「一个画布像素 = 屏幕上几个物理像素」。端口 #80 之后
        // 把它拆成了 `viewport.scale`（相对系统 DPI）× egui 的 `pixels_per_point`
        // （见 `state.rs:1750-1752`），这里合回去，缩放相关的判断一律按原版的口径走。
        let eff = u * ppi;
        let small = eff < 1.0 - 0.001;
        // 竖线的落点和宽度都量化到整数物理像素。原版所有线都画在整数逻辑坐标上
        // （`control_draw.gml:307` 的 `note_offset` 被 `floor` 过），整幅画在固定分辨率
        // 贴图上，上屏时才由 GPU 一次性缩放，所以每条线在屏幕上的落地宽度恒定；
        // 端口是直接按物理坐标画、且 `note_offset` 留了小数（按帧跟随时每帧都在变），
        // 1 逻辑像素的线在 150% 系统缩放下 = 1.5 物理像素，硬采样后按相位吃 1 个还是
        // 2 个像素列会逐帧变化 —— 用户实测的「播放时线条闪烁、粗细不均」就是这个。
        let snap = |v: f32| (v * ppi).round() / ppi;
        let line_w = |canvas_px: f32| (canvas_px * eff).round().max(1.0) / ppi;
        // 原版 `:314`：缩放小于 1 时小节线提到 alpha 0.5（Issue #254）。
        let bar = if small {
            th.grid_bar_small
        } else {
            th.grid_bar
        };
        // 原版 `:315-316`：播放中 + 按帧跟随 + 缩放小于 1 时小节线改画和拍线同粗的线。
        let thin_bar = small && state.follow_mode == 2 && state.is_playing();
        let w_line = line_w(1.0);
        let w_bar = line_w(2.0);
        // 分档丢弃（用户定案，刻意偏离原版）：缩放小于 50% 不画刻内细分线，
        // 小于等于 20% 连拍线也不画、只留小节线。原版到这些档位时最细的线本来就只剩
        // 半个屏幕像素、被过滤摊成一片灰；端口是硬边绘制，不丢会在密集处连成一片，
        // 反而比原版更抢眼。留 0.001 的余量：`viewport.scale` 是「绝对缩放 ÷ 系统 DPI」
        // 算出来的，150% 系统缩放下取 50% 档会折回 0.49999997，不能让它掉到下一档。
        let draw_beat = eff > 0.201;
        let draw_sub_beat = eff >= 0.5 - 0.001;
        for tick in t0..t1 {
            let x = snap(vp.tick_x(tick as i64));
            if x < rect.min.x - 1.0 || x > rect.max.x + 1.0 {
                continue;
            }
            let (w, c) = if tick % ts == 0 {
                (if thin_bar { w_line } else { w_bar }, bar)
            } else if tick % 4 == 0 {
                if !draw_beat {
                    continue;
                }
                (w_line, th.grid_beat)
            } else {
                if !draw_sub_beat {
                    continue;
                }
                (w_line, th.grid_line)
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
        push_note(
            state,
            &p,
            at,
            note,
            c,
            if in_box {
                state.prefs.selection_alpha
            } else {
                0.0
            },
        );
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
                        u,
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

    // 原版 `control_draw.gml:903-922`：拖框时画半透明矩形——左键加选=蓝
    // （Fluent 主题换成亮蓝 14120960=RGB(0,127,215)），右键减选=红；填充
    // α0.25、1px 描边。矩形从按下时的原始鼠标像素拉到当前鼠标（不吸附格子）；
    // starta/startb=0 时把远端夹回网格左/上缘（原版 `max(x1+2, mouse_x)`）。
    if let (DragMode::BoxSelect { subtract, .. }, Some(p0), Some(p1)) = (
        &state.interaction.mode,
        state.interaction.pressed_pos,
        state.interaction.latest_pos,
    ) {
        let mut x2 = p1.x;
        let mut y2 = p1.y;
        if state.viewport.start_tick == 0 {
            x2 = x2.max(rect.min.x);
        }
        if state.viewport.start_layer == 0 {
            y2 = y2.max(rect.min.y);
        }
        let far = pos2(x2, y2);
        let r = Rect::from_min_max(p0.min(far), p0.max(far));
        let col = if *subtract {
            C_RED
        } else if th.kind == ThemeKind::Fluent {
            Color32::from_rgb(0, 127, 215)
        } else {
            C_BLUE
        };
        p.rect_filled(r, 0.0, alpha(col, 64));
        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32 * u, col), StrokeKind::Inside);
    }

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
                    Vec2::new(16.0 * RULER_SCALE, rect.height()),
                    Color32::WHITE,
                );
            }
        }
    }

    // 播放头：原版 spr_marker 第 0 帧是箭头、第 1 帧是纵向拉满的细身（箭头画在时间轴那一格，
    // 见 draw_timeline），贴图原点在线上（`IconBank::marker`）。
    if let Some(bank) = state.icons.bank() {
        let mx = marker_screen_x(state, ui.ctx().pixels_per_point());
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
                // 竖线宽度恒定（#90）：这帧是 16px 宽的贴图，里面只有 x=7、x=8 两根
                // 实列（原点 8，落点两侧各占 1 像素），按 `cell` 缩放时缩到 10% 就成了
                // 0.2px，指针整个看不见。
                Vec2::new(16.0 * RULER_SCALE, rect.height()),
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
            pos2(
                base.x + 2.0 * u - e * 20.0 * u,
                base.y + 2.0 * u - e * 20.0 * u,
            ),
            pos2(
                base.x + 30.0 * u + e * 18.0 * u,
                base.y + 30.0 * u + e * 20.0 * u,
            ),
        );
        let col = th.accent(if th.fdark { 6 } else { 0 });
        let stroke = Stroke::new(1.0_f32 * u, alpha(col, (a * 255.0) as u8));
        let radius = CornerRadius::same((e * 20.0 * u).round().max(0.0) as u8);
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
    let u = state.viewport.u();
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
    // 其余走 `block_frame` —— 瀑布胶囊的底色取的是同一帧，两边必须同一个算式。
    let frame = if matches!(set, BlockSet::Plain) {
        1
    } else {
        crate::icons::block_frame(index, first_custom)
    };
    bank.block(p, at, set, frame, size, block_alpha);

    if flags.use_icons {
        let icon_at = pos2(at.x + 16.0 * u, at.y + 16.0 * u);
        let icon_size = Vec2::splat(16.0 * u);
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
    if flags.show_incompatible && note.is_out_of_range() {
        let r = Rect::from_min_max(
            pos2(at.x + 1.0 * u, at.y + 1.0 * u),
            pos2(at.x + 32.0 * u, at.y + 32.0 * u),
        );
        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32 * u, C_RED), StrokeKind::Inside);
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
    // 端口自加（用户要求 #92）：界面缩放低于 `note_text_min_percent` 时，方块上的两行字
    // 一起收掉，只留方块、图标和红框。原版没有这一档（它最低只到 50%，字照样画）。
    if state.hide_note_text {
        return;
    }
    let font = crate::fonts::scaled(main_font(), u);

    if flags.show_numbers {
        // 原版 `xx + 16 - 8 * use_icons`：勾了图标位就把数值往左挪半格。
        let x = at.x + (if flags.use_icons { 8.0 } else { 16.0 }) * u;
        p.text(
            pos2(x, at.y + 16.0 * u),
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
        pos2(at.x + 16.0 * u, at.y + 4.0 * u),
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

/// 原版 `get_mode_actions.gml` 的标签表：同一条动作在四种编辑模式下的叫法，
/// EN / ZH 两份逐字照抄（EN 的 "Set velocity..." 用三点，ZH 用「……」，原版就这样）。
/// 音高模式（m_key）只有 1~4 —— 原版 5、6 号返回空串，菜单外面还套着
/// `condstr((editmode != m_key), ...)`，所以这里是 `None`：整条不画，而不是画个灰的。
pub(crate) fn mode_action_label(mode: EditMode, slot: u8) -> Option<(&'static str, &'static str)> {
    match (mode, slot) {
        (EditMode::Key, 1) => Some(("Transpose one octave up", "升八度")),
        (EditMode::Key, 2) => Some(("Transpose one octave down", "降八度")),
        (EditMode::Key, 3) => Some(("Transpose up", "升半音")),
        (EditMode::Key, 4) => Some(("Transpose down", "降半音")),
        (EditMode::Velocity, 1) => Some(("Increase velocity by 10", "增加 10 音量")),
        (EditMode::Velocity, 2) => Some(("Decrease velocity by 10", "减小 10 音量")),
        (EditMode::Velocity, 3) => Some(("Increase velocity", "增加音量")),
        (EditMode::Velocity, 4) => Some(("Decrease velocity", "减小音量")),
        (EditMode::Velocity, 5) => Some(("Set velocity...", "设定音量……")),
        (EditMode::Velocity, 6) => Some(("Reset velocity", "重置音量")),
        (EditMode::Panning, 1) => Some(("Pan right by 10", "向右声道移动 10")),
        (EditMode::Panning, 2) => Some(("Pan left by 10", "向左声道移动 10")),
        (EditMode::Panning, 3) => Some(("Pan right", "向右声道移动")),
        (EditMode::Panning, 4) => Some(("Pan left", "向左声道移动")),
        (EditMode::Panning, 5) => Some(("Set panning...", "设定音符声道……")),
        (EditMode::Panning, 6) => Some(("Reset panning", "重置立体声")),
        (EditMode::Pitch, 1) => Some(("Detune +10 cents", "上调 10 微分")),
        (EditMode::Pitch, 2) => Some(("Detune -10 cents", "下调 10 微分")),
        (EditMode::Pitch, 3) => Some(("Detune +1 cent", "上调 1 微分")),
        (EditMode::Pitch, 4) => Some(("Detune -1 cent", "下调 1 微分")),
        (EditMode::Pitch, 5) => Some(("Set pitch...", "设定音高……")),
        (EditMode::Pitch, 6) => Some(("Reset pitch", "重置音高")),
        _ => None,
    }
}

/// 原版 `get_hotkey.gml:43-70` 的 action_1..6（非 macOS 那一支）：菜单里印在标签后面的
/// 那串键。#123 把快捷键做成可自定义之后，这里要换成读用户的键位表。
pub(crate) fn mode_action_hotkey(slot: u8) -> &'static str {
    match slot {
        1 => "Ctrl+E",
        2 => "Ctrl+D",
        3 => "Ctrl+R",
        4 => "Ctrl+F",
        5 => "Ctrl+T",
        _ => "Ctrl+G",
    }
}

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
                    // 原版 `control_draw.gml:550-555`：这六条的标签来自
                    // `get_mode_actions(editmode)`，跟着当前编辑模式走，音高模式下
                    // 第 5、6 项整条不画；六条共同的置灰条件是
                    // `inactive(current_song.selected = 0)`。
                    let has_sel = !state.doc.selection.is_empty();
                    for slot in 1..=6u8 {
                        let Some((en, zh)) = mode_action_label(state.tool, slot) else {
                            continue;
                        };
                        let label = format!("{}  {}", tr(en, zh), mode_action_hotkey(slot));
                        if ui.add_enabled(has_sel, egui::Button::new(&label)).clicked() {
                            cmds.push(Command::ModeAction(slot));
                            close = true;
                        }
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
    use crate::viewport::{SCROLLBAR_SIZE, TIMELINE_HEIGHT};

    /// 原版 `get_seconds_from_tick`：沿变速点分段累计。基速 10 t/s、tick 4 处
    /// 一枚 pitch=600 的 Tempo Changer（= 40 t/s）——前 4 刻 0.4s，之后加速。
    #[test]
    fn player_seconds_accumulates_across_tempo_changes() {
        let pts = vec![(4i64, 600i16)];
        let secs = |t: f64| player_seconds_at(&pts, 10.0, t);
        assert!((secs(0.0)).abs() < 1e-9);
        assert!((secs(4.0) - 0.4).abs() < 1e-9, "{}", secs(4.0));
        assert!((secs(8.0) - 0.5).abs() < 1e-9, "{}", secs(8.0));
        // pitch<=0 的病态值按 0.25 t/s 兜底（与实时播放同款下限）。
        let bad = vec![(2i64, 0i16)];
        assert!((player_seconds_at(&bad, 10.0, 4.0) - (0.2 + 2.0 / 0.25)).abs() < 1e-9);
    }

    /// 一帧滚轮（整格）：`scroll_fine` 为零 ⇒ 按鼠标滚轮处理。
    fn wheel(x: f32, y: f32) -> InputSnapshot {
        InputSnapshot {
            scroll: Vec2::new(x, y),
            ..Default::default()
        }
    }

    /// Shift + 滚轮（只有纵向分量）。
    fn wheel_shift(y: f32) -> InputSnapshot {
        InputSnapshot {
            scroll: Vec2::new(0.0, y),
            shift: true,
            ..Default::default()
        }
    }

    /// 一帧触控板（像素）：egui 会同时填 `raw` 和 `smooth`，这里按同样的值给两轴。
    fn pad(x: f32, y: f32) -> InputSnapshot {
        InputSnapshot {
            scroll: Vec2::new(x, y),
            scroll_fine: Vec2::new(x, y),
            ..Default::default()
        }
    }

    /// 一根滚得动的条：320×128 的网格（11 列 × 5 行）配 200 刻 / 40 层的歌。
    fn scroll_state() -> EditorState {
        let mut st = EditorState::for_test();
        // 出厂 `for_test` 带着欢迎窗，而窗口挡在最前面时画布不接输入（见
        // `canvas_blocked`）——这里要的是没遮挡的界面。
        st.window = None;
        st.doc.song.enda = 200;
        st.doc.song.endb = 40;
        st.viewport.origin = Pos2::ZERO;
        st.viewport.size = Vec2::new(320.0, 128.0);
        st.prefs.mousewheel_scroll_speed = 1;
        st
    }

    /// 原版 `draw_scrollbar.gml:24` 的
    /// `sb_val += (mouse_wheel_down - mouse_wheel_up) * wheel_speed`：一格走 `wheel_speed` 个槽，
    /// 向下滚 = 看更晚的内容。egui 的正值方向相反（`winit/src/event.rs:957`），
    /// 所以要取负，而且两轴不能再合成一个标量（#93.2）。
    #[test]
    fn wheel_steps_by_the_preference_in_the_original_direction() {
        let _prefs = crate::isolate_prefs();
        let mut st = scroll_state();

        st.viewport.start_layer = 20;
        scroll_bars(&mut st, &wheel(0.0, -1.0), 32.0, BarAxis::Both);
        assert_eq!(st.viewport.start_layer, 21, "滚轮向下 = 多看到一层");
        assert_eq!(st.viewport.start_tick, 0, "纵向那一格不许带动横向条");

        st.viewport.start_tick = 20;
        scroll_bars(&mut st, &wheel(1.0, 0.0), 32.0, BarAxis::Both);
        assert_eq!(st.viewport.start_tick, 19, "横向滚轮一格走一刻（原版槽位）");

        st.viewport.start_tick = 20;
        scroll_bars(&mut st, &wheel_shift(-1.0), 32.0, BarAxis::Both);
        assert_eq!(
            st.viewport.start_tick, 21,
            "Shift 把纵向那格交给横向条（端口约定）"
        );

        st.prefs.mousewheel_scroll_speed = 7;
        st.viewport.start_layer = 20;
        scroll_bars(&mut st, &wheel(0.0, -1.0), 32.0, BarAxis::Both);
        assert_eq!(st.viewport.start_layer, 27, "步长要按设置放大");

        // 配置里的脏值按原版 median(1, …, 10) 夹住，回退方向也不能退到负列。
        st.prefs.mousewheel_scroll_speed = 40;
        st.viewport.start_layer = 3;
        scroll_bars(&mut st, &wheel(0.0, 1.0), 32.0, BarAxis::Both);
        assert_eq!(st.viewport.start_layer, 0, "越界夹到 0，脏值夹到上限 10");
    }

    /// #93.3：触控板的两个分量各走各的条，斜着滑两条一起动；不足一格的位移先进
    /// 累加器，攒满一格才动（原版 `macos_scroll_temp_starta/b` 就是浮点累加）。
    #[test]
    fn trackpad_moves_each_axis_on_its_own_bar() {
        let _prefs = crate::isolate_prefs();
        let mut st = scroll_state();

        // 触控板正号 = 手指往右下走 = 内容跟着往右下 = 看到更早的东西（原版同款）。
        st.viewport.start_tick = 10;
        st.viewport.start_layer = 10;
        scroll_bars(&mut st, &pad(-32.0, 0.0), 32.0, BarAxis::Both);
        assert_eq!(
            (st.viewport.start_tick, st.viewport.start_layer),
            (11, 10),
            "双指往左滑只动横向条"
        );
        scroll_bars(&mut st, &pad(0.0, -64.0), 32.0, BarAxis::Both);
        assert_eq!(st.viewport.start_layer, 12, "双指往上滑 = 往高层走");

        // 半格一次：第一帧不动、两帧凑满一格。
        st.viewport.start_tick = 0;
        st.viewport.start_layer = 0;
        st.interaction.scroll_carry = Vec2::ZERO;
        scroll_bars(&mut st, &pad(-16.0, -16.0), 32.0, BarAxis::Both);
        assert_eq!(
            (st.viewport.start_tick, st.viewport.start_layer),
            (0, 0),
            "半格先攒着，不许直接取整成 0 丢掉"
        );
        scroll_bars(&mut st, &pad(-16.0, -16.0), 32.0, BarAxis::Both);
        assert_eq!(
            (st.viewport.start_tick, st.viewport.start_layer),
            (1, 1),
            "两帧凑满一格，两轴同时动"
        );

        // 悬停在哪根条上就只走那一根（原版 `sb_sel = i` 的门）。
        st.viewport.start_tick = 10;
        st.viewport.start_layer = 10;
        scroll_bars(&mut st, &pad(0.0, -32.0), 32.0, BarAxis::Horizontal);
        assert_eq!(
            (st.viewport.start_tick, st.viewport.start_layer),
            (11, 10),
            "指针在横向条上时，纵向滚动也归横向条"
        );

        // 撞到上限后不许继续攒小数，否则反向时会先吐一次超程。
        st.viewport.start_tick = st.doc.song.enda;
        st.interaction.scroll_carry = Vec2::ZERO;
        for _ in 0..20 {
            scroll_bars(&mut st, &pad(-8.0, 0.0), 32.0, BarAxis::Horizontal);
        }
        assert_eq!(st.viewport.start_tick, 200, "顶到上限就不再前进");
        assert_eq!(
            st.interaction.scroll_carry.x, 0.0,
            "每次被上限夹住都要清空累加器，20 帧攒不下 5 格"
        );
        for _ in 0..4 {
            scroll_bars(&mut st, &pad(8.0, 0.0), 32.0, BarAxis::Horizontal);
        }
        assert_eq!(
            st.viewport.start_tick,
            st.doc.song.enda - 1,
            "反向四帧（每帧 1/4 格）= 退一格，不能把攒着的 20 帧一次退光"
        );
    }

    /// 前台有东西挡着时，滚轮和滚动条点击都不许落到背后的画布。原版的门是
    /// `window = 0`：画布滚轮段 `control_draw.gml:227` 自带、音符改属性在 :125 的
    /// 总闸里、`draw_scrollbar.gml:22/51` 的滚轮和拖拽都带、音色框展开即
    /// `control_draw.gml:1975` 的 `window = w_insbox`。
    #[test]
    fn a_front_overlay_keeps_the_wheel_and_clicks_off_the_canvas() {
        let _prefs = crate::isolate_prefs();
        let grid = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 128.0));
        let hbar = Rect::from_min_size(pos2(0.0, 128.0), Vec2::new(320.0, 16.0));
        let vbar = Rect::from_min_size(pos2(320.0, 0.0), Vec2::new(16.0, 128.0));

        // 一帧滚轮（向下一格）：返回视口位置和这一帧新发了几条命令。
        let roll = |st: &mut EditorState, at: Pos2| -> (u32, u32, usize) {
            let mut input = wheel(0.0, -1.0);
            input.pointer = Some(at);
            input.hover = Some(at);
            let before = st.pending.len();
            handle_grid_input(st, &input, grid);
            handle_scrollbar_input(st, &input, hbar, vbar);
            (
                st.viewport.start_tick,
                st.viewport.start_layer,
                st.pending.len() - before,
            )
        };

        // 遮挡的三种形态：窗口在最上层、右键菜单、展开的音色框。
        let block = |st: &mut EditorState, which: u8| match which {
            0 => st.window = Some(crate::state::WindowKind::Preferences),
            1 => {
                st.context_menu = Some(ContextMenu {
                    pos: pos2(100.0, 64.0),
                    cell: (3, 2),
                    kind: ContextKind::Grid,
                })
            }
            _ => st.insbox_open = true,
        };

        // 没遮挡时这三条必须真的会动，否则下面的断言是空的。
        let mut st = scroll_state();
        assert_eq!(
            roll(&mut st, pos2(100.0, 64.0)),
            (0, 1, 0),
            "画布上的滚轮要平移视口"
        );
        let mut st = scroll_state();
        assert_eq!(
            roll(&mut st, pos2(328.0, 64.0)),
            (0, 1, 0),
            "滚动条上的滚轮要平移视口"
        );
        let mut st = scroll_state();
        st.prefs.changepitch = true;
        st.doc.song.set(3, 2, nbs_core::model::Note::new(0, 45));
        assert_eq!(
            roll(&mut st, pos2(100.0, 64.0)).2,
            1,
            "停在音符上要发改属性命令"
        );
        let mut st = scroll_state();
        st.prefs.mousewheel = 1;
        assert_eq!(
            roll(&mut st, pos2(100.0, 64.0)).2,
            1,
            "mousewheel=1 时要发换音色命令"
        );
        // 点在滚动条滑块上：把 start_layer 顶到某个值（下面遮挡版必须纹丝不动）。
        let mut st = scroll_state();
        st.viewport.start_layer = 12;
        let mut click = mouse_xy(328.0, 64.0, true, true, false);
        click.scroll = Vec2::ZERO;
        handle_scrollbar_input(&mut st, &click, hbar, vbar);
        assert_ne!(st.viewport.start_layer, 12, "点滑轨空白处要跳过去");

        for which in 0..3 {
            let mut st = scroll_state();
            block(&mut st, which);
            assert_eq!(
                roll(&mut st, pos2(100.0, 64.0)),
                (0, 0, 0),
                "第 {which} 种遮挡下，画布还吃了滚轮"
            );
            let mut st = scroll_state();
            block(&mut st, which);
            assert_eq!(
                roll(&mut st, pos2(328.0, 64.0)),
                (0, 0, 0),
                "第 {which} 种遮挡下，滚动条还吃了滚轮"
            );
            let mut st = scroll_state();
            block(&mut st, which);
            st.prefs.changepitch = true;
            st.doc.song.set(3, 2, nbs_core::model::Note::new(0, 45));
            assert_eq!(
                roll(&mut st, pos2(100.0, 64.0)).2,
                0,
                "第 {which} 种遮挡下，滚轮还能改到背后的音符"
            );
            let mut st = scroll_state();
            block(&mut st, which);
            let mut click = mouse_xy(328.0, 64.0, true, true, false);
            click.scroll = Vec2::ZERO;
            handle_scrollbar_input(&mut st, &click, hbar, vbar);
            assert_eq!(
                st.viewport.start_layer, 0,
                "第 {which} 种遮挡下，点背后的滚动条仍然生效"
            );
        }
    }

    /// 用户要求：播放中滚轮不许落到音符上。原版 `:211` 的 `playing = 0` 总闸只冻住
    /// sela/selb，`:276` 的 `selbx = starta + sela` 又拿自动平移后的 starta 去加，
    /// 所以原版改到的是「指针上次停过的那一列」；端口按实时光标算，改到的确实是
    /// 眼前这颗，误伤更直接，这道门只能自己加。
    #[test]
    fn the_wheel_skips_notes_while_playing() {
        let _prefs = crate::isolate_prefs();
        let grid = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 128.0));
        // 指针停在 (3,2) 那颗音符上滚一格；返回「那颗音符现在长什么样」和视口位置。
        let roll_on_note = |playing: bool| -> (nbs_core::model::Note, u32, usize) {
            let mut st = scroll_state();
            st.prefs.changepitch = true;
            st.doc.song.set(3, 2, nbs_core::model::Note::new(0, 45));
            if playing {
                st.audio.clock_mut().play();
            }
            let mut input = wheel(0.0, -1.0);
            input.pointer = Some(pos2(100.0, 64.0));
            input.hover = Some(pos2(100.0, 64.0));
            let before = st.pending.len();
            handle_grid_input(&mut st, &input, grid);
            let emitted = st.pending.len() - before;
            st.flush(&egui::Context::default());
            (
                st.doc.song.get(3, 2).unwrap(),
                st.viewport.start_layer,
                emitted,
            )
        };

        let untouched = roll_on_note(true);
        assert_eq!(
            untouched.0,
            nbs_core::model::Note::new(0, 45),
            "播放中那颗音符被改了"
        );
        assert_eq!(untouched.1, 1, "播放中滚轮要照常平移视口");
        assert_eq!(untouched.2, 0, "播放中还在发改音符的命令");

        // 停播时这一格必须照旧改得动，否则上面三条是空断言。
        let changed = roll_on_note(false);
        assert_ne!(
            changed.0,
            nbs_core::model::Note::new(0, 45),
            "停播时滚轮改不动音符了"
        );
        assert_eq!(changed.1, 0, "改音符这一格不平移视口");
    }

    /// #93.1 + #93.4：滑块按住哪儿就贴在哪儿（指针位移 = 滑块位移，一像素不差），
    /// 拖到尽头夹在 `enda`，此时滑块右边缘正好贴住滑轨末端。
    #[test]
    fn dragging_a_thumb_tracks_the_pointer_and_stops_at_the_song_end() {
        let _prefs = crate::isolate_prefs();
        let mut st = scroll_state();
        st.flags.show_layers = false;
        let hbar = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 16.0));
        let vbar = Rect::from_min_size(pos2(320.0, 0.0), Vec2::new(16.0, 128.0));

        let (_, max_val, thumb) = h_geometry(&st, hbar);
        assert_eq!(max_val, 200.0, "横向上限 = 原版 enda");
        // 一格 = 滑轨像素 / 总槽。`start_tick` 是整数，取整后滑块最多落后半格；
        // 要核的是这半格不随拖拽距离累积 —— 原先定位用「滑轨减滑块」、反解用「整条滑轨」，
        // 两个分母不一致，一路拖下来才会越偏越多（#93.1）。
        let tick_px = 320.0 / (200.0 + 11.0);
        let layer_px = 128.0 / (40.0 + 5.0);
        let start_center = thumb.center().x;
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(start_center, 8.0, true, true, false),
            hbar,
            vbar,
        );
        // 拖 100 像素。
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(start_center + 100.0, 8.0, false, true, false),
            hbar,
            vbar,
        );
        let dragged = h_geometry(&st, hbar).2;
        assert!(
            (dragged.center().x - (start_center + 100.0)).abs() <= tick_px / 2.0 + 0.01,
            "跟手：期望 {}，实测 {}（一格 {}）",
            start_center + 100.0,
            dragged.center().x,
            tick_px
        );
        // 再往界外拖：值停死在 enda，滑块右边缘贴住滑轨末端。
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(start_center + 4000.0, 8.0, false, true, false),
            hbar,
            vbar,
        );
        assert_eq!(st.viewport.start_tick, 200, "拖不出歌曲末尾");
        let end = h_geometry(&st, hbar).2;
        assert!(
            (end.max.x - 320.0).abs() < 1.0,
            "到头时滑块右边缘贴住滑轨，实测 {}",
            end.max.x
        );

        // 纵向同理：往下拖 = `startb` 增大（原版行号 `startb + b` 随屏幕 y 增大）。
        st.viewport.start_layer = 0;
        let (_, vmax, vt) = v_geometry(&st, vbar);
        assert_eq!(vmax, 40.0, "纵向上限 = 原版 endb");
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(328.0, vt.center().y, true, true, false),
            hbar,
            vbar,
        );
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(328.0, vt.center().y + 64.0, false, true, false),
            hbar,
            vbar,
        );
        assert!(st.viewport.start_layer > 0, "往下拖 = 往高层走");
        let moved = v_geometry(&st, vbar).2;
        assert!(
            (moved.center().y - (vt.center().y + 64.0)).abs() <= layer_px / 2.0 + 0.01,
            "纵向也跟手：期望 {}，实测 {}（一层 {}）",
            vt.center().y + 64.0,
            moved.center().y,
            layer_px
        );
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(328.0, vt.center().y + 9000.0, false, true, false),
            hbar,
            vbar,
        );
        assert_eq!(st.viewport.start_layer, 40, "拖不出最后一个含音符的层");

        // 点滑轨空白处（非滑块）也要夹在上限内。
        handle_scrollbar_input(
            &mut st,
            &mouse_xy(319.0, 8.0, true, true, false),
            hbar,
            vbar,
        );
        assert_eq!(st.viewport.start_tick, 200, "点最右边也点不出末尾");
    }

    /// #93 换过分母之后，滑块必须始终待在滑轨里，而且尺寸不随位置变
    /// （#88 定的恒定尺寸）。
    #[test]
    fn thumbs_stay_inside_their_track_at_every_scroll_position() {
        let hbar = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 16.0));
        let vbar = Rect::from_min_size(pos2(320.0, 0.0), Vec2::new(16.0, 128.0));
        let mut st = scroll_state();
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        for tick in (0..=201).step_by(17) {
            st.viewport.start_tick = tick as u32;
            st.viewport.start_layer = (tick / 5) as u32;
            let h = h_geometry(&st, hbar).2;
            let v = v_geometry(&st, vbar).2;
            assert!(hbar.contains_rect(h), "横向滑块 {h:?} 跑出滑轨 {hbar:?}");
            assert!(vbar.contains_rect(v), "纵向滑块 {v:?} 跑出滑轨 {vbar:?}");
            widths.push(h.width());
            heights.push(v.height());
        }
        assert!(
            widths.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001),
            "横向滑块长随位置变了：{widths:?}"
        );
        assert!(
            heights.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001),
            "纵向滑块长随位置变了：{heights:?}"
        );
    }

    // ---- 标尺状态机（区间 / 播放头手柄）------------------------------------

    /// 靶子谱：320×128 的网格 = 可见 10 列，空谱 `enda = 0`，
    /// 于是原版 `:1122` 的夹取上限正好是 `0 + 10 = 10`。
    fn ruler_state() -> EditorState {
        let mut st = EditorState::for_test();
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

    /// 原版手柄命中框的「整列高」：标尺带 + 全部网格行。
    fn grid_rect() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, TIMELINE_HEIGHT), Vec2::new(320.0, 128.0))
    }

    /// 一帧鼠标，`y = 16` 恒在标尺带内；按键位由参数给（`pressed` 与 `down`
    /// 在同一帧是两件事，原版 `mouse_check_button(_pressed/_released)` 也是三个独立查询）。
    fn mouse(x: f32, pressed: bool, down: bool, released: bool) -> InputSnapshot {
        mouse_xy(x, 16.0, pressed, down, released)
    }

    fn mouse_xy(x: f32, y: f32, pressed: bool, down: bool, released: bool) -> InputSnapshot {
        let p = pos2(x, y);
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
        let g = grid_rect();
        handle_timeline_input(&mut st, &mouse(100.0, true, true, false), r, g);
        assert!(
            matches!(st.interaction.mode, DragMode::PressTimeline { pressa: 3 }),
            "按下只记列：{:?}",
            st.interaction.mode
        );
        assert_eq!(st.marker(), 0.0, "按下本身不该动播放头");
        handle_timeline_input(&mut st, &mouse(100.0, false, false, true), r, g);
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
        let g = grid_rect();
        handle_timeline_input(&mut st, &mouse(100.0, true, true, false), r, g);
        handle_timeline_input(&mut st, &mouse(260.0, false, true, false), r, g);
        assert!(st.section_exists);
        assert_eq!((st.section_start, st.section_end), (3, 8), "新建用 floor");
        assert_eq!(st.interaction.mode, DragMode::DragSectionEnd);
        handle_timeline_input(&mut st, &mouse(260.0, false, false, true), r, g);
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
        let g = grid_rect();
        st.section_exists = true;
        st.section_start = 3;
        st.section_end = 8;
        // 右端手柄在 tick_x(8) = 256，命中框 ±8px。
        handle_timeline_input(&mut st, &mouse(253.0, true, true, false), r, g);
        assert_eq!(st.interaction.mode, DragMode::DragSectionEnd);
        handle_timeline_input(&mut st, &mouse(74.0, false, true, false), r, g);
        assert_eq!(
            (st.section_start, st.section_end),
            (3, 2),
            "2.3125 刻 round 成 2，且此时还不排序"
        );
        handle_timeline_input(&mut st, &mouse(74.0, false, false, true), r, g);
        assert_eq!((st.section_start, st.section_end), (2, 3), "松手排序");
        assert!(st.section_exists);
    }

    /// 原版 `:1105-1110`：松手时两端重合 = 直接把这段区间删掉。
    #[test]
    fn releasing_with_both_ends_on_the_same_tick_drops_the_section() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        let g = grid_rect();
        st.section_exists = true;
        st.section_start = 3;
        st.section_end = 8;
        handle_timeline_input(&mut st, &mouse(253.0, true, true, false), r, g);
        handle_timeline_input(&mut st, &mouse(96.0, false, true, false), r, g);
        assert_eq!(st.section_end, 3);
        handle_timeline_input(&mut st, &mouse(96.0, false, false, true), r, g);
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
        let g = grid_rect();
        handle_timeline_input(&mut st, &mouse(36.0, true, true, false), r, g);
        handle_timeline_input(&mut st, &mouse(964.0, false, true, false), r, g);
        assert_eq!(st.section_start, 1);
        assert_eq!(st.section_end, 30, "拖拽中按鼠标原值");
        handle_timeline_input(&mut st, &mouse(964.0, false, false, true), r, g);
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
        let g = grid_rect();
        st.set_marker(5.0);
        handle_timeline_input(&mut st, &mouse(164.0, true, true, false), r, g);
        assert_eq!(st.interaction.mode, DragMode::DragMarker);
        assert_eq!(st.marker(), 5.0, "按下本身不动播放头");
        handle_timeline_input(&mut st, &mouse(430.0, false, true, false), r, g);
        assert_eq!(st.marker(), 13.4375, "出格也要跟手");
        assert_eq!(st.interaction.cursor, CursorIcon::ResizeHorizontal);
        assert!(!st.section_exists, "抢到手柄就不该变成拖区间");
        handle_timeline_input(&mut st, &mouse(430.0, false, false, true), r, g);
        assert_eq!(st.interaction.mode, DragMode::None);
        assert_eq!(st.marker(), 13.4375);
    }

    /// 原版 `:156`：命中框只吃左键，右键落在手柄上照样开区间菜单。
    #[test]
    fn a_right_click_on_a_handle_still_opens_the_timeline_menu() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let r = ruler_rect();
        let g = grid_rect();
        st.set_marker(5.0);
        let mut snap = mouse(164.0, false, false, false);
        snap.secondary_pressed = true;
        handle_timeline_input(&mut st, &snap, r, g);
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
        let g = grid_rect();
        st.audio.clock_mut().play();
        handle_timeline_input(&mut st, &mouse(100.0, true, true, false), r, g);
        handle_timeline_input(&mut st, &mouse(260.0, false, true, false), r, g);
        handle_timeline_input(&mut st, &mouse(260.0, false, false, true), r, g);
        st.audio.clock_mut().pause();
        assert_eq!(st.marker(), 0.0);
        assert!(!st.section_exists);
        assert_eq!(st.interaction.mode, DragMode::None);
    }

    /// 原版 `:132`：播放头手柄的命中框是 16px 宽 × **整列高** —— 音符区里贴着
    /// 播放头线同样能抓住拖，抓住后出标尺带也一直跟手（按住左键不失控）。
    #[test]
    fn the_playhead_handle_grabs_from_inside_the_grid() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let (r, g) = (ruler_rect(), grid_rect());
        st.set_marker(5.0);
        // 手柄中心 x = tick_x(5) = 160，y = 96 落在网格第 3 行 —— 标尺带之外。
        handle_timeline_input(&mut st, &mouse_xy(160.0, 96.0, true, true, false), r, g);
        assert!(
            matches!(st.interaction.mode, DragMode::DragMarker),
            "网格里贴线要抓到手柄：{:?}",
            st.interaction.mode
        );
        assert_eq!(st.marker(), 5.0, "按下本身不动播放头");
        handle_timeline_input(&mut st, &mouse_xy(240.0, 96.0, false, true, false), r, g);
        assert_eq!(st.marker(), 7.5, "出标尺带也继续跟手");
        assert_eq!(st.interaction.cursor, CursorIcon::ResizeHorizontal);
        assert!(!st.section_exists, "抓住手柄不该变成拖区间");
        handle_timeline_input(&mut st, &mouse_xy(240.0, 96.0, false, false, true), r, g);
        assert_eq!(st.interaction.mode, DragMode::None);
        assert_eq!(st.marker(), 7.5);
    }

    /// 「按下记列」只认标尺带那 32px（原版 `:166` 的矩形高度是 32）：
    /// 音符区里的左键属于网格（放音符/框选），不得误开标尺状态机。
    #[test]
    fn the_timeline_press_stays_in_the_ruler_band() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let (r, g) = (ruler_rect(), grid_rect());
        handle_timeline_input(&mut st, &mouse_xy(100.0, 96.0, true, true, false), r, g);
        assert_eq!(
            st.interaction.mode,
            DragMode::None,
            "网格里的按下不进标尺状态机"
        );
        assert_eq!(st.marker(), 0.0);
        assert!(!st.section_exists);
    }

    /// 播放头拖拽经过网格时，网格侧不得抢光标、不得把按住中的左键当成框选起点。
    #[test]
    fn the_grid_defers_to_an_active_playhead_drag() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        let (r, g) = (ruler_rect(), grid_rect());
        st.set_marker(5.0);
        handle_timeline_input(&mut st, &mouse(164.0, true, true, false), r, g);
        assert!(matches!(st.interaction.mode, DragMode::DragMarker));
        // 指针移到网格里：光标保持 cr_size_we，拖拽状态不被覆盖。
        handle_grid_input(&mut st, &mouse_xy(170.0, 96.0, false, true, false), g);
        assert!(
            matches!(st.interaction.mode, DragMode::DragMarker),
            "网格输入不得打断拖拽"
        );
        assert_eq!(
            st.interaction.cursor,
            CursorIcon::ResizeHorizontal,
            "网格不得抢光标"
        );
        assert!(
            st.interaction.pressed_cell.is_none(),
            "网格不得把拖拽中的按下当成选音符的起点"
        );
        handle_timeline_input(&mut st, &mouse_xy(210.0, 96.0, false, true, false), r, g);
        assert_eq!(st.marker(), 6.5625, "210 ÷ 32 = 6.5625");
        handle_timeline_input(&mut st, &mouse_xy(210.0, 96.0, false, false, true), r, g);
        assert_eq!(st.interaction.mode, DragMode::None);
        assert_eq!(st.marker(), 6.5625, "松手不回跳");
    }

    /// 原版框选（`control_draw.gml:323-341` + `control_input`）：空白格按下只记起点，
    /// 拖开进入框选、hover 每帧跟手，松手整框落成浮动选区（音符抬出 song）。
    /// 用户验收 #83 反馈「没有框 / 选中外观不对」，先用真状态机钉住行为再谈外观。
    #[test]
    fn box_select_drag_lifts_notes_into_the_floating_selection() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        st.doc.song.set(0, 1, nbs_core::model::Note::new(0, 45));
        st.doc.song.set(1, 1, nbs_core::model::Note::new(0, 47));
        st.doc.song.set(2, 1, nbs_core::model::Note::new(0, 48));
        let g = grid_rect();
        let ctx = egui::Context::default();

        // 空白格 (0,3) 按下：只记起点，还不进框选。
        handle_grid_input(&mut st, &mouse_xy(16.0, 112.0, true, true, false), g);
        assert_eq!(st.interaction.mode, DragMode::None, "按下本身不该进框选");
        assert_eq!(
            st.interaction.pressed_cell,
            Some((0, 3)),
            "按下要记住框选锚点"
        );

        // 拖到 (5,1)：进入框选，hover 每帧跟手（原版 select=1 的 s*selection_alpha）。
        handle_grid_input(&mut st, &mouse_xy(170.0, 50.0, false, true, false), g);
        assert!(
            matches!(st.interaction.mode, DragMode::BoxSelect { .. }),
            "拖开应进框选: {:?}",
            st.interaction.mode
        );
        assert_eq!(st.hover_cell, Some((5, 1)), "框选中的 hover 要跟手");

        // 松手：整框（列 0..5、层 1..3）落成浮动选区，3 个音符抬出 song。
        handle_grid_input(&mut st, &mouse_xy(170.0, 50.0, false, false, true), g);
        for _ in 0..6 {
            if st.pending.is_empty() {
                break;
            }
            st.flush(&ctx);
        }
        assert_eq!(st.interaction.mode, DragMode::None, "松手要收掉拖拽状态");
        assert_eq!(st.doc.selection.len(), 3, "框内 3 个音符应抬进选区");
        assert!(
            !st.doc.song.has(0, 1) && !st.doc.song.has(1, 1) && !st.doc.song.has(2, 1),
            "音符应从谱面抬出（原版框选松手即清格）"
        );
    }

    /// #96：标尺背景是不透明矩形，必须排在秒标 / 刻数之前。端口原来把背景塞进
    /// `shapes` 到函数末尾才 `p.extend`，而两类文字是中途 `p.text` 直接落笔的，
    /// 结果背景压在文字上 —— 实测就是「标尺只剩刻度线，时间没了」。
    #[test]
    fn the_ruler_background_is_painted_before_its_labels() {
        let _prefs = crate::isolate_prefs();
        let st = ruler_state();
        let bg = st.theme.ruler_bg;
        let out = {
            let ctx = egui::Context::default();
            crate::fonts::install(&ctx);
            ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_timeline(&st, ui, ruler_rect());
                });
            })
        };
        let idx = out
            .shapes
            .iter()
            .position(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == bg))
            .expect("标尺背景矩形");
        let txt = out
            .shapes
            .iter()
            .position(|s| matches!(&s.shape, egui::Shape::Text(_)))
            .expect("标尺文字");
        assert!(idx < txt, "背景矩形 #{idx} 必须排在文字 #{txt} 之前");
    }

    /// #113：缩放小于 1 时竖线宽度不能跟着往 1 物理像素以下走。egui 无圆角填矩形的边
    /// 是硬采样，宽度不足 1px 的线只有相位撞上像素中心才落地 —— 实测 50% 档 83 条
    /// 只剩 5 条小节线。顺带钉住原版 `control_draw.gml:314` 的提亮和 `:315-316` 那条
    /// 「播放中 + 按帧跟随 + 缩放 <1 改画 1px」的分支。
    #[test]
    fn the_grid_vlines_stay_one_pixel_wide_when_scaled_down() {
        let _prefs = crate::isolate_prefs();

        // 取本帧所有竖线矩形：黑、又细（<8px）又顶满网格高，按 alpha 档认线。
        let vlines = |scale: f32, follow: u8, playing: bool| -> Vec<(f32, u8)> {
            let mut st = ruler_state();
            st.viewport.scale = scale;
            st.follow_mode = follow;
            if playing {
                st.begin_playback();
            }
            let out = {
                let ctx = egui::Context::default();
                crate::fonts::install(&ctx);
                ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_grid(&mut st, ui, grid_rect());
                    });
                })
            };
            if playing {
                st.audio.stop();
            }
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r)
                        if (r.fill.r(), r.fill.g(), r.fill.b()) == (0, 0, 0)
                            && r.rect.width() < 8.0
                            && r.rect.height() > 100.0
                            && matches!(r.fill.a(), 26 | 64 | 77 | 128) =>
                    {
                        Some((r.rect.width(), r.fill.a()))
                    }
                    _ => None,
                })
                .collect()
        };

        // 网格宽 320：条数 = floor(320 / 格距) + 1。
        let full = vlines(1.0, 0, false);
        assert_eq!(full.len(), 11, "100% 档每条竖线都要提交");
        assert!(full.iter().any(|&(_, a)| a == 77), "小节线用 alpha 0.3");

        let half = vlines(0.5, 0, false);
        assert_eq!(half.len(), 21, "50% 档每条竖线都要提交");
        assert!(
            half.iter().all(|&(w, _)| w >= 1.0),
            "50% 档有不足 1 像素的竖线：{half:?}"
        );
        assert!(half.iter().any(|&(_, a)| a == 128), "原版 `:314`");
        assert!(!half.iter().any(|&(_, a)| a == 77), "缩放 <1 不再用 0.3");

        // 0.75 档小节线本征宽 1.5px，播放 + 按帧跟随时按原版 `:315` 退回 1px。
        let still = vlines(0.75, 0, false);
        assert_eq!(still.len(), 14);
        assert!(
            still.iter().any(|&(w, a)| a == 128 && w > 1.001),
            "非播放态小节线保持加粗：{still:?}"
        );
        let played = vlines(0.75, 2, true);
        assert!(
            played.iter().all(|&(w, _)| w <= 1.001),
            "原版 `:315-316`：{played:?}"
        );
    }

    /// 本帧所有竖线的（左边 x、宽度、alpha），按系统缩放 `ppi` 渲染。
    /// `offset` 就是按帧跟随时每帧都在变的亚刻位移（原版 `control_draw.gml:307` 把它
    /// `floor` 成整数逻辑像素，端口 `viewport.rs:172-176` 留成小数）。
    fn vline_geometry(scale: f32, ppi: f32, offset: f32, playing: bool) -> Vec<(f32, f32, u8)> {
        let mut st = ruler_state();
        st.viewport.scale = scale;
        st.viewport.note_offset = offset;
        st.follow_mode = 2;
        if playing {
            st.begin_playback();
        }
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppi);
        crate::fonts::install(&ctx);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_grid(&mut st, ui, grid_rect());
            });
        });
        if playing {
            st.audio.stop();
        }
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r)
                    if (r.fill.r(), r.fill.g(), r.fill.b()) == (0, 0, 0)
                        && r.rect.width() < 8.0
                        && r.rect.height() > 100.0
                        && matches!(r.fill.a(), 26 | 64 | 77 | 128) =>
                {
                    Some((r.rect.min.x, r.rect.width(), r.fill.a()))
                }
                _ => None,
            })
            .collect()
    }

    fn whole_dev(v: f32) -> bool {
        (v - v.round()).abs() < 0.001
    }

    /// #114：竖线必须落在**物理像素**格上，否则播放时会闪、粗细还不均。
    /// 本机 Windows 缩放 150%（`AppliedDPI = 120`），1 逻辑像素 = 1.5 物理像素；
    /// #113 把宽度钳到「不低于 1 逻辑像素」在这台机器上等于 1.5 物理像素，而 egui
    /// 无圆角填矩形的边是硬采样 —— 同样宽的线，相位不同就吃掉 1 个或 2 个像素列，
    /// 按帧跟随时每帧相位都在变，于是线在 1px / 2px 之间来回跳。原版不抖是因为它
    /// 整幅画在整数逻辑坐标的贴图上，再由 GPU 一次性缩放，每条线的落地宽度恒定。
    #[test]
    fn the_grid_vlines_land_on_the_device_pixel_grid() {
        let _prefs = crate::isolate_prefs();
        for (ppi, scale) in [
            (1.5f32, 0.5f32),
            // 本机 Windows 缩放 150% 时「窗口缩放 50%」这一档的实际值（用户复现闪烁的档位）。
            (1.5, 0.5 / 1.5),
            (1.25, 0.5),
            (1.0, 0.5),
        ] {
            for playing in [true, false] {
                let mut base: Vec<(u8, i32)> = Vec::new();
                for (i, offset) in [0.0f32, 1.7, 5.1, 8.5, 11.9].into_iter().enumerate() {
                    let lines = vline_geometry(scale, ppi, offset, playing);
                    assert!(
                        !lines.is_empty(),
                        "ppi {ppi} 缩放 {scale} 位移 {offset}：一条竖线都没提交"
                    );
                    for (x, w, _) in &lines {
                        assert!(
                            whole_dev(x * ppi),
                            "ppi {ppi} 缩放 {scale} 位移 {offset}：线头 x={x:.3} \
                            不在物理像素格上（{:.3}）",
                            x * ppi
                        );
                        assert!(
                            whole_dev(w * ppi) && w * ppi > 0.999,
                            "ppi {ppi} 缩放 {scale} 位移 {offset}：宽 w={w:.3} \
                            不是整数个物理像素（{:.3}）",
                            w * ppi
                        );
                    }
                    // 同一档线在一帧里只能有一种物理宽度，而且各帧相同；否则画面上的线
                    // 就是粗细不均、并且随播放逐帧变化。
                    let mut tiers: Vec<(u8, i32)> = lines
                        .iter()
                        .map(|(_, w, a)| (*a, (w * ppi).round() as i32))
                        .collect();
                    tiers.sort_unstable();
                    tiers.dedup();
                    let alpha_only = tiers.iter().map(|(a, _)| *a).collect::<Vec<_>>();
                    assert_eq!(
                        alpha_only.len(),
                        tiers.len(),
                        "ppi {ppi} 缩放 {scale} 位移 {offset}：同一档线出现了多种物理宽度 {tiers:?}"
                    );
                    if i == 0 {
                        base = tiers;
                    } else {
                        assert_eq!(
                            tiers, base,
                            "ppi {ppi} 缩放 {scale}：位移 {offset} 时物理宽度变了（逐帧就会闪）"
                        );
                    }
                }
                // 非播放态（`playing = false`）才有原版 `draw_rectangle` 那条 2 画布像素
                // 宽的小节线，上面几条断言在播放态会被 1px 分支盖住。
                assert!(
                    base.iter().any(|(a, _)| *a == 128),
                    "ppi {ppi} 缩放 {scale}：小节线一档没提交，断言盖不到它"
                );
            }
        }
    }

    /// #114 附带的小缩放分档（用户定的口径，刻意偏离原版）：缩放**小于 50%** 时
    /// 刻内细分线整档不画（原版此时它们只有半像素、本来就糊成一片），只留拍线；
    /// **小于等于 20%** 连拍线也去掉，只留小节线。
    #[test]
    fn the_grid_drops_the_finer_line_tiers_at_small_zoom() {
        let _prefs = crate::isolate_prefs();
        let tiers = |scale: f32| -> Vec<u8> {
            let mut a: Vec<u8> = vline_geometry(scale, 1.0, 0.0, false)
                .into_iter()
                .map(|(_, _, a)| a)
                .collect();
            a.sort_unstable();
            a.dedup();
            a
        };
        assert_eq!(tiers(1.0), vec![26, 64, 77], "100% 三档都在");
        assert_eq!(
            tiers(0.5),
            vec![26, 64, 128],
            "50% 是「小于」的边界，三档都留"
        );
        assert_eq!(tiers(0.45), vec![64, 128], "45% 去掉刻内细分线");
        assert_eq!(tiers(0.25), vec![64, 128], "25% 仍留拍线");
        assert_eq!(tiers(0.2), vec![128], "20% 只剩小节线");
        assert_eq!(tiers(0.1), vec![128], "10% 只剩小节线");
    }

    /// #95：网格里空格是箭头（原版 `cr_cross` 全文没用过，`control_draw.gml:125-129`
    /// 是 `cr_handpoint`，用户定案要箭头），落在音符上换手型，框选拖拽中按 `:904`
    /// 也是手型。上一帧留下的十字必须被收回，否则整窗口都挂着一个十字。
    #[test]
    fn the_grid_cursor_is_an_arrow_except_over_notes_and_while_selecting() {
        let _prefs = crate::isolate_prefs();
        let mut st = ruler_state();
        st.doc.song.set(0, 1, nbs_core::model::Note::new(0, 45));
        let g = grid_rect();

        st.interaction.cursor = CursorIcon::Crosshair;
        handle_grid_input(&mut st, &mouse_xy(16.0, 112.0, false, false, false), g);
        assert_eq!(st.interaction.cursor, CursorIcon::Default, "空格是箭头");

        st.interaction.cursor = CursorIcon::Crosshair;
        handle_grid_input(&mut st, &mouse_xy(16.0, 48.0, false, false, false), g);
        assert_eq!(
            st.interaction.cursor,
            CursorIcon::PointingHand,
            "音符上是手型"
        );

        st.interaction.mode = DragMode::BoxSelect {
            anchor: (0, 3),
            subtract: false,
            alt_copy: false,
        };
        st.interaction.cursor = CursorIcon::Crosshair;
        handle_grid_input(&mut st, &mouse_xy(170.0, 50.0, false, true, false), g);
        assert_eq!(
            st.interaction.cursor,
            CursorIcon::PointingHand,
            "框选拖拽中是手型（原版 cr_handpoint）"
        );
    }

    /// 偏离原版（用户要求）：滚动条不随窗口缩放，恒定为 BAR_SCALE（0.67）档尺寸。
    /// 两种格距下 `split()` 分出的条带、`h/v_geometry()` 的滑块几何必须一致，
    /// 内边距钉在 `4 × BAR_SCALE`、最小滑块钉在 `24 × BAR_SCALE`。
    #[test]
    fn the_scrollbar_bands_do_not_scale_with_the_window() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let (_, _, h1, v1) = split(area, 32.0, false);
        let (_, _, h2, v2) = split(area, 32.0 * 0.4, false);
        assert_eq!(h1.height(), h2.height(), "横条带高不随缩放");
        assert_eq!(v1.width(), v2.width(), "竖条带宽不随缩放");
        assert!((h1.height() - SCROLLBAR_SIZE * BAR_SCALE).abs() < 0.01);
        assert!((v1.width() - SCROLLBAR_SIZE * BAR_SCALE).abs() < 0.01);

        let mut st = ruler_state();
        let (_, _, t1) = h_geometry(&st, h1);
        let (_, _, t2) = h_geometry(&st, h2);
        assert_eq!(t1.height(), t2.height(), "滑块高不随缩放");
        assert!(
            (t1.height() - (h1.height() - 4.0 * BAR_SCALE)).abs() < 0.01,
            "滑块上下内边距 = 4 × BAR_SCALE"
        );

        // 拉满谱长把比例压到最小，滑块钉在 24 × BAR_SCALE。
        let mut song = nbs_core::model::Song::new();
        song.enda = 100_000;
        st.doc.replace_song(song);
        let (_, _, t3) = h_geometry(&st, h1);
        assert!(
            (t3.width() - 24.0 * BAR_SCALE).abs() < 0.01,
            "最小滑块宽 = 24 × BAR_SCALE，实际 {}",
            t3.width()
        );
    }

    #[test]
    fn reference_row_takes_one_layer_row_and_keeps_scrollbar_geometry() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let (_, plain, h_plain, v_plain) = split(area, 32.0, false);
        let (_, reference, h_reference, v_reference) = split(area, 32.0, true);
        assert_eq!(plain.height() - reference.height(), 32.0);
        assert_eq!(h_plain, h_reference, "横条仍贴窗口底部");
        assert_eq!(v_plain, v_reference, "竖条尺寸不变，只多滚一格");
        let mut state = ruler_state();
        state.flags.show_layers = false;
        state.viewport.size = plain.size();
        let (_, max_plain, thumb_plain) = v_geometry(&state, v_plain);
        state.flags.show_layers = true;
        state.viewport.size = reference.size();
        let (_, max_reference, thumb_reference) = v_geometry(&state, v_reference);
        assert_eq!(max_reference, max_plain + 1.0);
        assert!(thumb_reference.height() <= thumb_plain.height());
    }

    /// 偏离原版（用户要求 #90）：时间轴带和播放指针都不随界面缩放。
    /// 带高恒为 `TIMELINE_HEIGHT`（100% 档），网格顶因此也不动；
    /// 指针竖线的贴图宽度按 `RULER_SCALE` 展开，缩到 10% 仍然是整 2px，不会消失。
    #[test]
    fn the_timeline_band_does_not_scale_with_the_window() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let (t1, g1, _, _) = split(area, 32.0, false);
        let (t2, g2, _, _) = split(area, 32.0 * 0.1, false);
        assert!(
            (t1.height() - TIMELINE_HEIGHT).abs() < 0.01,
            "标尺带恒为 100% 档"
        );
        assert!((t1.height() - t2.height()).abs() < 0.01, "带高不随缩放");
        assert_eq!(g1.min.y, g2.min.y, "网格顶不随缩放");

        // `IconBank::marker` 把 16px 宽的帧拉成 `size.x`；贴图里不透明的是 x=7、x=8
        // 两根实列（钉在 `the_marker_body_is_exactly_two_solid_columns`），所以屏幕上的
        // 线宽 = size.x ÷ 16 × 2。宽度钉死后恒 2px，不会随界面缩放变细。
        let line_w = (16.0 * RULER_SCALE) / 16.0 * 2.0;
        assert!((line_w - 2.0).abs() < 1e-6, "指针竖线恒 2px，实际 {line_w}");
        // 图层栏那边取的是同一个带高，两边算出的行数才对得上。
        assert!((timeline_band() - TIMELINE_HEIGHT).abs() < 0.01);
    }

    /// 用户实测「播放时指针后面跟着一条浅蓝色的线」的回归闸：贴图落点必须落在整物理
    /// 像素上。带内用的是 LINEAR 采样，落点一带小数，`spr_marker` 那两根实列就会被摊成
    /// 3~4 根渐变的半透明列（实测 f≈0.35 时三列 alpha ≈ 38/255/217），看着就像多出一条影。
    /// 原版是 `floor` 到整数画布像素，这里是四舍五入到物理像素格，误差上限 0.5 物理像素。
    #[test]
    fn the_marker_blit_lands_on_a_whole_physical_pixel() {
        let mut st = ruler_state();
        for (marker, ppi) in [
            (0.35, 1.0_f32),
            (3.37, 1.5),
            (3.5, 1.25),
            (12.9, 2.0),
            (7.0, 1.5),
        ] {
            st.set_marker(marker);
            let got = marker_screen_x(&st, ppi);
            assert!(
                (got * ppi).fract().abs() < 1e-3,
                "{marker} tick / ppi {ppi}：吸附后 {got}（×ppi = {}）不在物理像素格上",
                got * ppi
            );
            let err = (got - marker_x(&st)).abs() * ppi;
            assert!(err <= 0.5 + 1e-3, "{marker} tick：位置误差 {err} 物理像素");
        }
        // 反证：不吸附时播放中的落点确实是小数，正是被摊开的前提。
        st.set_marker(0.35);
        let raw = marker_x(&st);
        assert!(raw.fract().abs() > 0.1, "落点本该是小数，实际 {raw}");
    }

    /// 偏离原版（用户要求 #90）：标尺标签尺寸钉死，格距变小时靠跳档避免数字叠在一起。
    /// 一格宽 = `c × tempo × cell`（原版 `c` 是「几秒一格」）。
    #[test]
    fn ruler_labels_hop_to_a_wider_step_when_the_cells_get_narrow() {
        // 100%（cell 32）、tempo 15、0.5 秒一格 = 240px，早放得下 → 原样不动。
        assert!((seconds_step(0.5, 15.0, 32.0, 20.0) - 0.5).abs() < 1e-9);
        // 同样 tempo 缩到 10%（cell 3.2）：0.25 秒只有 12px，跳到 0.5 秒 = 24px。
        assert!((seconds_step(0.25, 15.0, 3.2, 20.0) - 0.5).abs() < 1e-9);
        // 慢速 + 小格距要连跳几档：0.5 t/s、cell 3.2 → 1 秒才 1.6px，一路跳到 30 秒（48px）。
        assert!((seconds_step(1.0, 0.5, 3.2, 20.0) - 30.0).abs() < 1e-9);
        // 最大档也放不下时停在最大档，不能越过 600 秒。
        assert!((seconds_step(1.0, 0.1, 0.1, 1e6) - 600.0).abs() < 1e-9);

        // tick 号：100% 时 4 格 = 128px，放得下 28px 的字 → 维持原版的每 4 格一个。
        assert_eq!(tick_label_step(28.0, 32.0), 4);
        // 缩到 10%：4 格只剩 12.8px，放宽到 3 个「4 格」= 12 格。
        assert_eq!(tick_label_step(28.0, 3.2), 12);
        assert_eq!(tick_label_step(28.0, 3.2) % 4, 0, "放宽后仍落在 4 的倍数上");
    }

    /// 原版 `control_draw.gml:1092-1095`：拖框期间边缘自动滚动推动视口时，
    /// 按下像素跟着平移（`select_pressx += 32 * (a - starta)`），矩形锚在格子上不漂。
    /// 移动浮动选区（锚在格子空间）不需要补偿，按下像素原样不动。
    #[test]
    fn box_select_edge_scroll_shifts_the_press_pixel_with_the_viewport() {
        let _prefs = crate::isolate_prefs();
        let g = grid_rect();
        let mut input = InputSnapshot::default();
        input.pointer = Some(pos2(1.0, 100.0)); // 压在左边缘 → dx = -1

        let mut st = ruler_state();
        st.viewport.start_tick = 10;
        st.interaction.mode = DragMode::BoxSelect {
            anchor: (0, 0),
            subtract: false,
            alt_copy: false,
        };
        st.interaction.pressed_pos = Some(pos2(100.0, 100.0));
        let from = st.viewport.start_tick;
        for _ in 0..8 {
            edge_scroll(&mut st, &input, g);
        }
        let moved = from as i64 - st.viewport.start_tick as i64;
        assert!(moved > 0, "左边缘滚动应把视口往左推，实际移动 {moved}");
        let press = st.interaction.pressed_pos.unwrap();
        assert!(
            (press.x - (100.0 + moved as f32 * st.viewport.cell())).abs() < 0.01,
            "按下像素应随视口右移 {moved} 格 × 格距，实际 x={}",
            press.x
        );

        let mut st = ruler_state();
        st.viewport.start_tick = 10;
        st.interaction.mode = DragMode::MoveSelection {
            grab: (0, 0),
            origin: (0, 0),
            before: Default::default(),
        };
        st.interaction.pressed_pos = Some(pos2(100.0, 100.0));
        for _ in 0..8 {
            edge_scroll(&mut st, &input, g);
        }
        assert_ne!(st.viewport.start_tick, 0, "对照组：滚视口确实发生了");
        assert!(
            (st.interaction.pressed_pos.unwrap().x - 100.0).abs() < 0.01,
            "移选区不补偿按下像素"
        );
    }
}
