//! 左侧图层栏：逐像素照原版 `control_draw.gml` 的图层列来画。
//!
//! 版式出处：行底 `spr_layerbox` 在 `x1 = 4`、`y1 = 54 + 32 + b * 32 - 1`；名称是
//! `draw_text_edit`（经典）/ `draw_textarea`（Fluent）的常驻输入框；音量、声像是
//! 竖向拖拽的小精灵（`spr_volume` / `spr_stereo`，悬停时上移 5px 并显出数值）；
//! 右侧 7 个 `draw_layericon`，横坐标 `x1 + 126…234 - !realvolume - realstereo * 10`；
//! 可视行底部再挂一条 15 个 `draw_macroicon` 的宏条（有选区且显示钢琴键盘时）。
//!
//! 原版整个画面按 window_scale 等比放大，图层列也在其中。**#108 把这一条改掉了**：
//! 侧栏只有纵轴跟缩放（行高 = 32×`u`，`u = viewport.u()`），横轴脱离缩放，宽度由用户拖
//! 面板右边界调，落在 `prefs.layer_panel_width`（出厂 264 = 原版列宽）。栏内版式一律屏幕
//! 像素、按宽度自动布局：控件簇贴右边界走，名称区吃剩下的空间，放不下就依次藏掉声像、
//! 音量。出厂宽度 264 这一档，下面每个坐标都还原成上面那些原版数字。
//! 拖拽增量：原版鼠标坐标在「除以缩放」的房间坐标系里（`control_draw.gml:15-16`），
//! 纵向的 `drag_delta`（屏幕点）先 `/u` 再参与数值；横向（拖面板边界）不除。

use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, Id, Key, Layout, PointerButton, Pos2, Rect,
    Sense, TextEdit, Ui, UiBuilder,
};
use nbs_core::model::{Layer, LockMode, PAN_CENTER};

use crate::commands::Command;
use crate::fonts::{info_med_bold_font, main_font, small_font};
use crate::icons::{layer as layer_icon, macro_icon, Bank};
use crate::state::{
    ContextKind, ContextMenu, DialogAction, EditorState, FileDialog, LayerDrag, ReferenceDrag,
    WindowKind,
};
use crate::theme::{Theme, ThemeKind};
use crate::tr;
use crate::viewport::{bar_thickness, timeline_band, RULER_SCALE};
use nbs_io::prefs::{LAYER_PANEL_DEFAULT_WIDTH, LAYER_PANEL_MAX_WIDTH, LAYER_PANEL_MIN_WIDTH};

/// 原版图层列内容左边界 `x1`。
const X1: f32 = 4.0;
/// 原版行底比整行上移 1px：`spr_layerbox` 高 34，多出的 2px 是它自带的下沿压痕。
const ROW_LIFT: f32 = 1.0;

/// 原版功能按钮：18px 一档，图标本体 16px（`spr_layericons` 帧宽）。
const BTN_PITCH: f32 = 18.0;
const BTN_SIZE: f32 = 16.0;
/// 7 个按钮占的总宽：`x1 + 126` 到末按钮右沿 `x1 + 250`。
const CLUSTER_W: f32 = BTN_PITCH * 6.0 + BTN_SIZE;
/// 原版音量 / 声像判定框：`mouse_rectangle(x1 + 88 / 108, y1 + 5, 16, 25)`。
const GLYPH_HIT_W: f32 = 16.0;
const GLYPH_HIT_H: f32 = 25.0;
/// 原版 `spr_volume` / `spr_stereo` 的帧尺寸（12×10）。
const GLYPH_W: f32 = 12.0;
const GLYPH_H: f32 = 10.0;
/// 原版判定框之间的空隙：声像框右 124 → 首按钮左 126；音量框右 104 → 声像框左 108。
const PAN_GAP: f32 = 2.0;
const VOL_GAP: f32 = 4.0;
/// 原版末按钮右沿 254，列宽 264 → 右边空 10。
const CLUSTER_PAD: f32 = 10.0;
/// 右边界拖拽条宽度（#108）：吃的是上面那 10px 右边空，不压任何控件。
const HANDLE_W: f32 = 5.0;
/// 原版按偏好藏字形时按钮的左挪补偿：`- !realvolume - realstereo * 10`。
const VOL_SHIFT: f32 = 1.0;
const PAN_SHIFT: f32 = 10.0;
/// 名称文字的左沿（原版 `x1 + 11` = 15）。
const NAME_TEXT_X: f32 = X1 + 11.0;
/// 最左控件的判定框到行底名称框右边线的空隙（原版 92 → 89）。
const NAME_BOX_GAP: f32 = 3.0;
/// 名称框右边线到文字截断处的内边距（原版 89 → 87，正好是原版的 72 内容宽）。
const NAME_INNER_PAD: f32 = 2.0;
/// 名称区窄到这个数就不值得再画字形（依次藏声像、藏音量，按钮永远保留）。
const NAME_MIN: f32 = 30.0;
/// 原版宏条：`spr_iconbar` 宽 255，从 `x1` 起，右边空 5。
const MACRO_W: f32 = 255.0;
/// 原版头部：小节框 184..248（右边空 16）、速度框判定 (108, 57, 64, 22)、
/// 时间右对齐 93（都是面板左起、不加 x1）。
const BARS_RIGHT_PAD: f32 = 16.0;
const BARS_W: f32 = 64.0;
const TEMPO_GAP: f32 = 76.0;
const TIME_GAP: f32 = 15.0;
const TEMPO_HIT_W: f32 = 64.0;
/// Fluent 主题的时间左沿（原版 9 / 26，两行错开）。
const FLUENT_TIME_X: f32 = 9.0;
const FLUENT_LENGTH_X: f32 = 26.0;

/// 侧栏内部的横向版式（#108）：一律屏幕像素、不随缩放，按面板宽度自动布局。
/// 出厂宽度 264 下每个数都还原成原版 `control_draw.gml` 的坐标。
#[derive(Clone, Copy, Debug, PartialEq)]
struct PanelLayout {
    /// 面板宽度。
    pw: f32,
    /// 画不画音量字形（原版 `realvolume`）。
    volume: bool,
    /// 画不画声像字形（原版 `!realstereo`）。
    pan: bool,
    /// 按偏好藏字形时按钮的左挪补偿，照原版；拥挤降级藏字形不补偿（那是把空间让给名称）。
    shift: f32,
}

impl PanelLayout {
    /// 拥挤降级：名称放不下就依次藏掉音量、声像。顺序按「藏哪个能腾出宽度」定：
    /// 音量在声像左边，只藏声像并不会让名称区变宽，所以先藏音量。结果只由宽度和两个
    /// 偏好决定，同一个宽度只有一个解，拖动边界时不会在两种排法之间来回抖。
    fn new(pw: f32, show_vol: bool, stereo_on: bool) -> Self {
        let mut l = Self {
            pw,
            volume: show_vol,
            pan: stereo_on,
            shift: (if show_vol { 0.0 } else { VOL_SHIFT })
                + (if stereo_on { 0.0 } else { PAN_SHIFT }),
        };
        for drop_volume in [true, false] {
            if l.name_w() >= NAME_MIN {
                break;
            }
            if drop_volume {
                l.volume = false;
            } else {
                l.pan = false;
            }
        }
        l
    }

    /// 首个功能按钮左沿。原版 `x1 + 126` = 130。
    fn buttons_x(&self) -> f32 {
        self.pw - CLUSTER_PAD - CLUSTER_W - self.shift
    }

    /// 声像判定框左沿。原版 `x1 + 108` = 112。
    fn pan_x(&self) -> f32 {
        self.pw - CLUSTER_PAD - CLUSTER_W - PAN_GAP - GLYPH_HIT_W
    }

    /// 音量判定框左沿。原版 `x1 + 88` = 92。
    fn vol_x(&self) -> f32 {
        self.pan_x() - VOL_GAP - GLYPH_HIT_W
    }

    /// 还在画的最左那个控件的左沿，名称区就到它为止。
    fn left_edge(&self) -> f32 {
        if self.volume {
            self.vol_x()
        } else if self.pan {
            self.pan_x()
        } else {
            self.buttons_x()
        }
    }

    /// 行底名称框的右沿（`spr_layerbox` 里那一格的右边线）。原版 89 = `x1 + 85`。
    fn name_box_right(&self) -> f32 {
        self.left_edge() - NAME_BOX_GAP
    }

    /// 名称可画的宽度。原版 72。
    fn name_w(&self) -> f32 {
        (self.name_box_right() - NAME_TEXT_X - NAME_INNER_PAD).max(0.0)
    }

    /// 行底 `spr_layerbox` 的宽：原版 256，从 `x1 = 4` 画到 260，右边同样留 4px 窗底。
    /// 宽度可拖之后这条也跟着左右各留 4（264 这一档正好还原成 256）。
    fn strip_w(&self) -> f32 {
        (self.pw - X1 * 2.0).max(0.0)
    }

    /// 宏条的等比缩放：照旧跟 `u`，但整条不许超出面板 —— 放不下就缩，15 个宏不折行。
    /// 原版 255 宽从 `x1` 起、右边空 5，所以 264 这一档正好是 1.0。
    fn macro_scale(&self, u: f32) -> f32 {
        u.min((self.pw - X1 - 5.0) / MACRO_W).max(0.25)
    }
}

/// 行内文字的纵向落点（相对行顶）。原版是 `base × u`；字号恒定不跟着缩，
/// 所以行高不够时先贴到行内可放的最下位置，再挤不下就不画。
/// `base × u` 与居中取小，保证 100% 缩放时就是原版的数。
fn text_y(cell: f32, u: f32, base: f32, text_h: f32) -> Option<f32> {
    if text_h > cell {
        return None;
    }
    Some((base * u).min(cell - text_h))
}

/// 精灵尺寸：100% 缩放就是原版的 1，行高压不到 16px 才跟着缩，免得压到上下行。
fn glyph_scale(cell: f32) -> f32 {
    (1.0f32).min(cell / BTN_SIZE)
}

/// 头部那一排（时间 / 速度框 / 小节框）的横向版式（#108）：整簇贴右边界走，
/// 出厂宽度 264 下还原成原版的 184 / 108 / 右对齐 93。时间放不下先换短格式
/// （去掉小时段），再收起小节框（那格只有悬停说明、没有交互），最后按可视宽度截断。
/// 时间始终留在侧栏里，不挪到标尺带上。
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeaderLayout {
    /// 速度框判定框左沿。原版 108。
    tempo_x: f32,
    /// 小节框左沿。原版 184。
    bars_x: f32,
    /// 时间文字的右沿（经典主题右对齐到这里）。原版 93。
    time_right: f32,
    /// 画不画小节框。
    bars: bool,
    /// 时间用短格式。
    compact_time: bool,
}

impl HeaderLayout {
    /// `full_w` / `compact_w` 是长、短两种格式里较那行的实测宽度。
    fn new(pw: f32, fluent: bool, full_w: f32, compact_w: f32) -> Self {
        let bars_x = pw - BARS_RIGHT_PAD - BARS_W;
        let mut tempo_x = bars_x - TEMPO_GAP;
        let mut bars = true;
        let mut compact = false;
        if full_w > Self::room(tempo_x, fluent) {
            if compact_w <= Self::room(tempo_x, fluent) {
                compact = true;
            } else {
                bars = false;
                // 速度框挪到小节框那一格上（右边空 16 照旧），时间区随之变宽。
                tempo_x += TEMPO_GAP;
                compact = full_w > Self::room(tempo_x, fluent);
            }
        }
        Self {
            tempo_x,
            bars_x,
            time_right: tempo_x - TIME_GAP,
            bars,
            compact_time: compact,
        }
    }

    /// 时间能占的宽度：Fluent 左对齐在 9、往右挤到速度框前；经典右对齐在速度框前 15，
    /// 文字可以一直铺到面板左沿 0（原版 `draw_text_dynamic(93, …)` 的 `fa_right` 正是这样，
    /// 264 宽 + 原版字体下「00:00:00;000」实测 93px，刚好贴着栏的左边）。
    fn room(tempo_x: f32, fluent: bool) -> f32 {
        if fluent {
            tempo_x - 4.0 - FLUENT_TIME_X
        } else {
            tempo_x - TIME_GAP
        }
    }
}

/// 侧栏当前宽度（屏幕像素，#108）：`prefs.layer_panel_width` 夹进最窄 / 最宽一档。
/// `app.rs` 的 SidePanel 和栏内版式都取这一个数，两边不会打架。
pub fn panel_width(state: &EditorState) -> f32 {
    state
        .prefs
        .layer_panel_width
        .clamp(LAYER_PANEL_MIN_WIDTH, LAYER_PANEL_MAX_WIDTH)
}

/// 面板右边界的拖拽条（#108）：鼠标移到侧栏右边界出现左右箭头，按住拖动改宽度，
/// 双击回出厂宽度。原版整幅画面等比缩放、没有这一条，是横轴脱离缩放后才需要的。
/// 宽度只写 `prefs`，松手 / 双击才落盘，避免每帧写配置文件。
fn panel_resize_handle(state: &mut EditorState, ui: &mut Ui) {
    let area = ui.max_rect();
    let rect = Rect::from_min_size(
        pos2(area.right() - HANDLE_W, area.min.y),
        vec2(HANDLE_W, area.height()),
    );
    let resp = ui.interact(rect, Id::new("layer_panel_resize"), Sense::click_and_drag());
    let stopped = resp.drag_stopped();
    // 松手那一帧 egui 已经把 `dragged` 清空（`interaction.rs` 的 Released 分支），指针位置
    // 却还在。只认 `dragged` 就会丢掉最后一段移动，边界比光标窄几个像素。
    let live = resp.dragged() || stopped;
    if resp.double_clicked() {
        state.prefs.layer_panel_width = LAYER_PANEL_DEFAULT_WIDTH;
        state.emit(Command::SavePreferences);
    } else if let (true, Some(p)) = (live, resp.interact_pointer_pos()) {
        state.prefs.layer_panel_width =
            (p.x - area.min.x).clamp(LAYER_PANEL_MIN_WIDTH, LAYER_PANEL_MAX_WIDTH);
    }
    if stopped {
        state.emit(Command::SavePreferences);
    }
    resp.on_hover_cursor(CursorIcon::ResizeHorizontal)
        .on_hover_text(tr(
            "Drag to resize this panel (double-click resets it).",
            "左右拖动改宽度，双击回默认宽度。",
        ));
}

pub fn show_layers_panel(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let area = ui.max_rect();
    let Some(bank) = state.icons.bank() else {
        show_layers_text(state, ui);
        panel_resize_handle(state, ui);
        return;
    };
    let start = area.min;
    let cell = state.viewport.cell();
    let u = state.viewport.u();
    let band = start.y;
    // 时间轴带和滚动条的厚度都不随缩放（#88 / #90），和网格的 `split()` 取同一个数，
    // 两边算出的行数、宏条位置才对得上。
    let rows_start = pos2(start.x, band + timeline_band());
    let show_vol = state.prefs.show_layer_volumes;
    // 原版 `realstereo` 存的是「关闭立体声」那个勾选框（`draw_window_preferences.gml:606`），
    // 为真时不画声像字形、播放也归到中央。端口里同一个语义是 `prefs.mono`。
    let stereo_on = !state.prefs.mono;
    let layout = PanelLayout::new(area.width(), show_vol, stereo_on);

    header(state, ui, &bank, &th, start, band, area.width());

    let l0 = {
        let (_, _, a, _) = state.viewport.visible_range();
        a
    };
    // 原版 `totalrows = floor((rh - rhval) / 32)`：只画整行，宏条占掉最后那点余量。
    // 网格那边用同一条公式（`grid.rs` 里的 `split`），两边画行数、宏条位置才对得上。
    // 行数只按本面板的区域算，不取 `viewport.rows()`：侧栏比网格先画，
    // 用视口那个值会慢一帧，第一帧少画几行、宏条也跟着错位。
    let show_reference_row = !state.fullscreen;
    let rows = (((area.height() - timeline_band() - bar_thickness()) / cell).floor() as u32)
        .saturating_sub(u32::from(show_reference_row))
        .max(1);
    let layers = state.doc.song.layers.clone();
    let idle = state.window.is_none() && state.context_menu.is_none();
    let shift = ui.input(|i| i.modifiers.shift);
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
            &layout,
            exists,
        );
    }

    if show_reference_row {
        reference_row(
            state,
            ui,
            &bank,
            &th,
            pos2(start.x, rows_start.y + rows as f32 * cell - ROW_LIFT * u),
            cell,
            u,
            idle,
            &layout,
        );
    }

    // 宏条贴在参考行下面（原版 `y1 + 32`，y1 是最后一行行顶）。
    let bar_top =
        rows_start.y + (rows + u32::from(show_reference_row)) as f32 * cell - ROW_LIFT * u;
    macro_bar(
        state,
        ui,
        &bank,
        &th,
        pos2(start.x, bar_top),
        idle,
        false,
        layout.macro_scale(u),
    );

    // 把手要最后注册：egui 取「同一个点上最后注册的那个控件」，先注册会被行里的
    // 交互区盖掉，边界就拖不动。
    panel_resize_handle(state, ui);
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
    // 右上角的兼容徽标拿的是同一个数（`EditorState::shown_tempo`），框和徽标一起动。
    let shown = state.shown_tempo() as f64;
    let marker = state.marker();
    let total = song.song_length() as f64;
    let fluent = th.kind == ThemeKind::Fluent;

    // 原版：经典主题右对齐到 x=93；Fluent 左对齐到 9 / 26。宽度不够时按
    // `HeaderLayout` 的降级阶梯走（#108）。
    let elapsed_font = crate::fonts::scaled(info_med_bold_font(), u);
    let length_font = crate::fonts::scaled(small_font(), u);
    let text_w = |text: &str, font: &egui::FontId| {
        p.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    let full_elapsed = time_str(marker / shown);
    let full_length = format!("/ {}", time_str(total / shown));
    let short_elapsed = time_str_short(marker / shown);
    let short_length = format!("/ {}", time_str_short(total / shown));
    let full_w = text_w(&full_elapsed, &elapsed_font).max(text_w(&full_length, &length_font));
    let compact_w = text_w(&short_elapsed, &elapsed_font).max(text_w(&short_length, &length_font));
    let hl = HeaderLayout::new(width, fluent, full_w, compact_w);
    let room = HeaderLayout::room(hl.tempo_x, fluent);
    let (elapsed, length) = if hl.compact_time {
        (
            clip_label(p, &short_elapsed, &elapsed_font, room),
            clip_label(p, &short_length, &length_font, room),
        )
    } else {
        (
            clip_label(p, &full_elapsed, &elapsed_font, room),
            clip_label(p, &full_length, &length_font, room),
        )
    };
    if fluent {
        p.text(
            pos2(start.x + FLUENT_TIME_X, band - 2.0 * u),
            Align2::LEFT_TOP,
            elapsed,
            elapsed_font,
            th.text,
        );
        p.text(
            pos2(start.x + FLUENT_LENGTH_X, band + 15.0 * u),
            Align2::LEFT_TOP,
            length,
            length_font,
            th.text,
        );
    } else {
        p.text(
            pos2(start.x + hl.time_right, band - 2.0 * u),
            Align2::RIGHT_TOP,
            elapsed,
            elapsed_font,
            th.text,
        );
        p.text(
            pos2(start.x + hl.time_right, band + 15.0 * u),
            Align2::RIGHT_TOP,
            length,
            length_font,
            th.text,
        );
    }

    // 小节·拍·十六分：原版只画，不给交互（悬停出说明）。窄到时间放不下时这一格先让位。
    let sig = song.meta.time_signature.max(1) as f64;
    let digits = crate::fonts::scaled(main_font(), u);
    if hl.bars {
        bank.tempo_box(
            p,
            pos2(start.x + hl.bars_x, band + 3.0 * u),
            false,
            th,
            u,
            state.canvas_hires,
        );
        // 原版数字右对齐在 210 / 215 / 225 / 230 / 240，也就是这一格左边 +26 起。
        for (dx, t) in [
            (31.0, ".".to_string()),
            (46.0, ".".to_string()),
            (
                26.0,
                ((marker / (sig * 4.0)).floor() as i64 + 1).to_string(),
            ),
            (
                41.0,
                (((marker / 4.0) % sig).floor() as i64 + 1).to_string(),
            ),
            (56.0, ((marker % 4.0).floor() as i64 + 1).to_string()),
        ] {
            p.text(
                pos2(start.x + hl.bars_x + dx, band + 6.0 * u),
                Align2::RIGHT_TOP,
                t,
                digits.clone(),
                th.text,
            );
        }
    }

    // 速度框：精灵宽 74，原版判定和文字定位都用左边 108（BPM 时往左挪 7px 画框）。
    // 就地输入框按单位换几何：t/s 用 (108, 57, 64)，BPM 用 (101, 57, 74)
    // （`draw_window_set_tempo.gml:14-21`）；点击 / 拖拽的判定框固定 (108, 57, 64, 22)。
    let box_x = hl.tempo_x + if bpm_on { -7.0 } else { 0.0 };
    let box_w = if bpm_on { 74.0 } else { 64.0 };
    let mult = if bpm_on { 15.0 } else { 1.0 };
    bank.tempo_box(
        p,
        pos2(start.x + box_x, band + 3.0 * u),
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
            pos2(start.x + hl.tempo_x + 28.0, band + 6.0 * u),
            Align2::CENTER_TOP,
            value,
            digits.clone(),
            th.text,
        );
    }

    let idle = state.window.is_none() && state.context_menu.is_none();
    if hl.bars {
        let bars_rect = Rect::from_min_size(
            pos2(start.x + hl.bars_x, band + 3.0 * u),
            vec2(BARS_W * u, 22.0 * u),
        );
        ui.interact(bars_rect, Id::new("bars_box"), Sense::hover())
            .on_hover_text(tr(
                "Position of the marker in bars, beats and sixteenths.",
                "歌曲的进度（全音、四分、十六分）",
            ));
    }

    let inline = state.tempo_input.take();
    if let Some(mut text) = inline {
        // 原版 `draw_inputbox(64, xx, 57 + song_tab_offset, w, …)`：框体高 21，
        // 而「上一次按下 / 这次抬手在不在框里」用的是同一条高 22 的矩形。
        let at = pos2(start.x + box_x, band + 3.0 * u);
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
        // （`draw_window_set_tempo.gml:31`）；Enter 原样留给文本框、不算提交（`:29`
        // 那行注释），Esc 只丢弃不改速度（`:47-51`），两条都顺带把 `settempo` 清回 0。
        // 刻意偏离（用户要求 #118）：回车也算收口，填完数字按回车当场生效。
        // 单行 TextEdit 自己会在回车时让出焦点（egui `text_edit/builder.rs:1000`），
        // 所以判据取「这一帧失焦 + 这一帧按了回车」，和图层名那条输入框同一个写法。
        let enter = re.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let finish_commit = (released && !state.tempo_press_in && !inside) || enter;
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
        pos2(start.x + hl.tempo_x, band + 3.0 * u),
        vec2(TEMPO_HIT_W * u, 22.0 * u),
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
pub(crate) fn clip_label(
    p: &egui::Painter,
    label: &str,
    font: &egui::FontId,
    max_w: f32,
) -> String {
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

/// 原版 `control_draw.gml:1456-1566` 的参考音频行。
/// 打包版把工具栏上的加载按钮注释掉了，所以名称格同时承担打开文件框的入口。
fn reference_row(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    at: Pos2,
    cell: f32,
    u: f32,
    idle: bool,
    layout: &PanelLayout,
) {
    let config = state.reference.clone();
    let loaded = config.path.is_some();
    let x1 = at.x + X1;
    let y1 = at.y;
    let gs = glyph_scale(cell);
    bank.layer_row(
        ui.painter(),
        pos2(x1, y1),
        layout.strip_w(),
        layout.name_box_right() - X1,
        cell + 2.0 * u,
        th,
    );
    let font = crate::fonts::scaled(
        if th.kind == ThemeKind::Fluent {
            main_font()
        } else {
            small_font()
        },
        u,
    );
    let name_rect = Rect::from_min_size(
        pos2(x1 + 10.0, y1 + 10.0 * u),
        vec2(
            (at.x + layout.name_box_right() - x1 - 10.0).max(0.0),
            13.0 * u,
        ),
    );
    let name = ui.interact(name_rect, Id::new("reference_name"), Sense::click());
    let h = ui
        .painter()
        .layout_no_wrap("Reference".to_owned(), font.clone(), Color32::WHITE)
        .size()
        .y;
    if let Some(dy) = text_y(cell, u, 10.0, h) {
        ui.painter().text(
            pos2(x1 + 11.0, y1 + dy),
            Align2::LEFT_TOP,
            tr("Reference", "参考音频"),
            font,
            th.text,
        );
    }
    if idle && name.clicked() {
        let dialog =
            FileDialog::for_action(DialogAction::LoadReference, config.path.as_ref(), None);
        state.request_file_dialog(dialog);
    }
    name.on_hover_text(tr("Load or replace reference audio", "打开或更换参考音频"));

    let bx = |base: f32| at.x + layout.buttons_x() + (base - 126.0);
    let by = y1 + (cell - BTN_SIZE * gs) * 0.5;
    let drag = state.reference_drag;
    for (offset, x, current) in [
        (false, layout.vol_x(), config.volume as f32),
        (true, layout.pan_x(), config.offset_ms as f32),
    ] {
        if (offset && !layout.pan) || (!offset && !layout.volume) {
            continue;
        }
        let hit = Rect::from_min_size(
            pos2(at.x + x, y1 + 5.0 * u),
            vec2(GLYPH_HIT_W, GLYPH_HIT_H * u),
        );
        let resp = ui.interact(hit, Id::new(("reference_drag", offset)), Sense::drag());
        let active = resp.hovered() || resp.dragged();
        let value = drag
            .filter(|d| d.offset == offset)
            .map(|d| d.raw)
            .unwrap_or(current);
        let gy = y1 + (cell - GLYPH_H * gs) * 0.5;
        if offset {
            bank.stereo_glyph(
                ui.painter(),
                pos2(at.x + x + 3.0 + GLYPH_W * (1.0 - gs) * 0.5, gy),
                PAN_CENTER,
                active,
                th,
                gs,
                state.canvas_hires,
            );
        } else {
            bank.volume_glyph(
                ui.painter(),
                pos2(at.x + x + 3.0 + GLYPH_W * (1.0 - gs) * 0.5, gy),
                value.clamp(0.0, 100.0) as u8,
                active,
                th,
                gs,
                state.canvas_hires,
            );
        }
        if active {
            let text = if offset {
                format!("{} ms", value.round() as i64)
            } else {
                format!("{}%", value.floor() as u8)
            };
            let small = crate::fonts::scaled(small_font(), u);
            let h = ui
                .painter()
                .layout_no_wrap(text.clone(), small.clone(), Color32::WHITE)
                .size()
                .y;
            if let Some(dy) = text_y(cell, u, 18.0, h) {
                ui.painter().text(
                    pos2(at.x + x + 10.0, y1 + dy),
                    Align2::CENTER_TOP,
                    text,
                    small,
                    th.text,
                );
            }
        }
        if idle && loaded && resp.drag_started() {
            state.reference_drag = Some(ReferenceDrag { offset, raw: value });
        }
        if let Some(mut d) = state.reference_drag.filter(|d| d.offset == offset) {
            if resp.dragged() && !resp.drag_started() {
                d.raw -= resp.drag_delta().y / u;
                if d.offset {
                    state.reference.offset_ms = d.raw.round() as i64;
                } else {
                    d.raw = d.raw.clamp(0.0, 100.0);
                    state.reference.volume = if ui.input(|i| i.modifiers.shift) {
                        d.raw.round() as u8
                    } else {
                        (d.raw / 10.0).floor() as u8 * 10
                    };
                }
                state.reference_drag = Some(d);
            }
            if resp.drag_stopped() {
                state.reference_drag = None;
            }
        }
        resp.clone().on_hover_cursor(CursorIcon::ResizeNorth);
        if offset {
            resp.on_hover_text(tr(
                "Reference offset (click and drag to change)",
                "参考音频偏移（拖拽来修改）",
            ));
        } else {
            resp.on_hover_text(tr(
                "Reference volume (click and drag to change)",
                "参考音频音量（拖拽来修改）",
            ));
        }
    }

    if idle
        && loaded
        && bank.layer_button(
            ui,
            pos2(bx(126.0), by),
            layer_icon::LOCK,
            u32::MAX,
            tr("Mute reference audio", "静音参考音频"),
            config.option == 0,
            th,
            gs,
            state.canvas_hires,
        )
    {
        state.emit(Command::SetReferenceMode(if config.option == 0 {
            2
        } else {
            0
        }));
    }
    if idle
        && loaded
        && bank.layer_button(
            ui,
            pos2(bx(144.0), by),
            layer_icon::SOLO,
            u32::MAX,
            tr("Solo reference audio", "独奏参考音频"),
            config.option == 1,
            th,
            gs,
            state.canvas_hires,
        )
    {
        state.emit(Command::SetReferenceMode(if config.option == 1 {
            2
        } else {
            1
        }));
    }
    if idle
        && loaded
        && bank.layer_button(
            ui,
            pos2(bx(162.0), by),
            layer_icon::REMOVE,
            u32::MAX,
            tr("Remove reference audio", "删除参考音频"),
            false,
            th,
            gs,
            state.canvas_hires,
        )
    {
        state.emit(Command::RemoveReferenceAudio);
    }
}

/// 一行图层。`at` 是原版行底左上角（已含 `ROW_LIFT`）。
/// **横向坐标一律屏幕像素、由 `layout` 按面板宽度算（#108），不乘 `u`**；
/// 纵向（行内 y、文字高度、判定框高）仍乘 `u`，跟网格行高一起缩。
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
    layout: &PanelLayout,
    exists: bool,
) {
    let ox = at.x;
    let x1 = ox + X1;
    let y1 = at.y;
    // 精灵尺寸：100% 缩放就是原版的 1，行高压到 16px 以下才跟着缩，不压上下两行。
    let gs = glyph_scale(cell);
    // 原版 `draw_sprite(spr_layerbox, …, x1, y1)`，行框同样从 x1 = 4 起；
    // 名称那一格的右边线按版式走，其余部分按宽度摊开（见 icons::Bank::layer_row）。
    bank.layer_row(
        ui.painter(),
        pos2(x1, y1),
        layout.strip_w(),
        layout.name_box_right() - X1,
        cell + 2.0 * u,
        th,
    );

    // 原版 `x1 + 126 - !realvolume - realstereo * 10`：按偏好藏字形时按钮只左挪 1px / 10px。
    let bx = |base: f32| ox + layout.buttons_x() + (base - 126.0);

    // ---- 名称：原版的常驻输入框 ----
    let fluent = th.kind == ThemeKind::Fluent;
    let font = crate::fonts::scaled(if fluent { main_font() } else { small_font() }, u);
    // 行底名称框右边线的绝对坐标（版式里的数一律从面板左沿量）。
    let box_right = ox + layout.name_box_right();
    let name_w = layout.name_w();
    let name_rect = Rect::from_min_size(
        pos2(x1 + 10.0, y1 + 10.0 * u),
        vec2((box_right - x1 - 10.0).max(0.0), 13.0 * u),
    );
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
            (x1 + 9.0, y1 + 4.0 * u, name_w, 24.0 * u)
        } else {
            (x1 + 11.0, y1 + 10.0 * u, name_w, 14.0 * u)
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
        // 栏宽压到下限附近时名称区不足 NAME_MIN：一个字母既读不出名字，又会在行里
        // 留下无意义的碎片，索性不画。
        let label = if name_w < NAME_MIN {
            String::new()
        } else {
            clip_label(ui.painter(), &label, &font, name_w)
        };
        let h = ui
            .painter()
            .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
            .size()
            .y;
        if let Some(dy) = text_y(cell, u, 10.0, h) {
            ui.painter().text(
                pos2(x1 + 11.0, y1 + dy),
                Align2::LEFT_TOP,
                label,
                font,
                color,
            );
        }
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
    if layout.volume {
        let hit = Rect::from_min_size(
            pos2(ox + layout.vol_x(), y1 + 5.0 * u),
            vec2(GLYPH_HIT_W, GLYPH_HIT_H * u),
        );
        let resp = ui.interact(hit, Id::new(("layervol", idx)), Sense::drag());
        let active = resp.hovered() || resp.dragged();
        let (started, dragged, stopped, delta) = (
            resp.drag_started(),
            resp.dragged(),
            resp.drag_stopped(),
            resp.drag_delta().y,
        );
        let gy = y1 + (cell - GLYPH_H * gs) * 0.5;
        bank.volume_glyph(
            ui.painter(),
            // 原版字形左边 x1 + 91（判定框左 +3）；缩小后按中心对齐，不偏走。
            pos2(ox + layout.vol_x() + 3.0 + GLYPH_W * (1.0 - gs) * 0.5, gy),
            vol,
            active,
            th,
            gs,
            state.canvas_hires,
        );
        if active {
            let text = format!("{vol}%");
            let font = crate::fonts::scaled(small_font(), u);
            let h = ui
                .painter()
                .layout_no_wrap(text.clone(), font.clone(), Color32::WHITE)
                .size()
                .y;
            if let Some(dy) = text_y(cell, u, 18.0, h) {
                ui.painter().text(
                    pos2(ox + layout.vol_x() + 10.0, y1 + dy),
                    Align2::CENTER_TOP,
                    text,
                    font,
                    th.text,
                );
            }
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
    if layout.pan {
        let pan = drag
            .filter(|d| d.index == idx && d.stereo)
            .map(|d| d.value)
            .unwrap_or(layer.stereo);
        let hit = Rect::from_min_size(
            pos2(ox + layout.pan_x(), y1 + 5.0 * u),
            vec2(GLYPH_HIT_W, GLYPH_HIT_H * u),
        );
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
            // 原版字形左边 x1 + 110（判定框左 +2，正好居中）；缩小后仍按中心对齐。
            pos2(
                ox + layout.pan_x() + (GLYPH_HIT_W - GLYPH_W * gs) * 0.5,
                y1 + (cell - GLYPH_H * gs) * 0.5,
            ),
            pan,
            active,
            th,
            gs,
            state.canvas_hires,
        );
        let (tip, label) = pan_label(pan);
        if active {
            let font = crate::fonts::scaled(small_font(), u);
            let h = ui
                .painter()
                .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
                .size()
                .y;
            if let Some(dy) = text_y(cell, u, 18.0, h) {
                ui.painter().text(
                    pos2(ox + layout.pan_x() + 8.0, y1 + dy),
                    Align2::CENTER_TOP,
                    label,
                    font,
                    th.text,
                );
            }
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
    // 纵向在行内居中：100% 缩放时 (32 - 16) / 2 = 8，正是原版的 `y1 + 8`。
    let by = y1 + (cell - BTN_SIZE * gs) * 0.5;
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
            gs,
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
            gs,
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
    ), false, th, gs, state.canvas_hires) {
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
            gs,
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
            gs,
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
            gs,
            state.canvas_hires,
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
            layer_icon::DOWN,
            idx,
            tr(
                "Shift layer down\n(Click and drag to shift multiple layers)",
                "下移本层\n（拖拽可下移多层）",
            ),
            false,
            th,
            gs,
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
/// `scale` = 条子整体的缩放：图层栏里传 `PanelLayout::macro_scale`，也就是
/// 「照旧跟 `u`，但 15 个宏不许出栏」的较小那档（#108，不折行）。
#[allow(clippy::too_many_arguments)]
fn macro_bar(
    state: &mut EditorState,
    ui: &mut Ui,
    bank: &Bank,
    th: &Theme,
    at: Pos2,
    idle: bool,
    wide: bool,
    scale: f32,
) {
    if !state.flags.show_piano || state.doc.selection.is_empty() {
        return;
    }
    let x1 = at.x + X1 * scale;
    let p = ui.painter();
    let fluent = th.kind == ThemeKind::Fluent;
    // 原版 1497-1511：show_layers 时 `y1 + 32`、高 1.3 × 30；收起时 `y1 + 52`、高 0.75 × 30。
    // Fluent 那两个圆角矩形是 38 和 21。宽度按 Fluent 的 255 / 380 取。
    let (w, h, dy) = if wide {
        (
            380.0 * scale,
            if fluent { 21.0 * scale } else { 22.5 * scale },
            3.0 * scale,
        )
    } else {
        (
            255.0 * scale,
            if fluent { 38.0 * scale } else { 39.0 * scale },
            5.0 * scale,
        )
    };
    let bg = Rect::from_min_size(pos2(x1, at.y), vec2(w, h));
    if fluent {
        p.rect_filled(bg, 2.0 * scale, Color32::from_gray(70));
    } else {
        bank.icon_strip_bg(p, bg);
    }
    let row1 = pos2(x1 + 6.0 * scale, at.y + dy);
    let row2 = pos2(row1.x, row1.y + 16.0 * scale);
    let step = 25.0 * scale;

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
                scale,
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

/// 侧栏窄到放不下 `time_str` 时的短格式（#108 的降级第一档）：小时并进分钟，
/// 只剩 `mm:ss;mmm`。原版不需要这一档（整幅画面等比缩放，从不缺宽）。
pub(crate) fn time_str_short(sec: f64) -> String {
    let s = sec.max(0.0);
    let msec = (s * 1000.0).floor() as i64 % 1000;
    let min = (s / 60.0).floor() as i64;
    let sec = s.floor() as i64 % 60;
    format!("{min:02}:{sec:02};{msec:03}")
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
    if !state.fullscreen {
        ui.horizontal(|ui| {
            if ui.button(tr("Reference", "参考音频")).clicked() {
                let dialog = FileDialog::for_action(
                    DialogAction::LoadReference,
                    state.reference.path.as_ref(),
                    None,
                );
                state.request_file_dialog(dialog);
            }
            if let Some(path) = &state.reference.path {
                ui.label(path.display().to_string());
                let mut volume = state.reference.volume as u8;
                if ui
                    .add(egui::DragValue::new(&mut volume).range(0..=100))
                    .changed()
                {
                    state.reference.volume = volume;
                }
                let mut offset = state.reference.offset_ms;
                if ui.add(egui::DragValue::new(&mut offset)).changed() {
                    state.reference.offset_ms = offset;
                }
                if ui
                    .selectable_label(state.reference.option == 0, tr("Mute", "静音"))
                    .clicked()
                {
                    state.emit(Command::SetReferenceMode(if state.reference.option == 0 {
                        2
                    } else {
                        0
                    }));
                }
                if ui
                    .selectable_label(state.reference.option == 1, tr("Solo", "独奏"))
                    .clicked()
                {
                    state.emit(Command::SetReferenceMode(if state.reference.option == 1 {
                        2
                    } else {
                        1
                    }));
                }
                if ui.button(tr("Remove", "删除")).clicked() {
                    state.emit(Command::RemoveReferenceAudio);
                }
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

    /// #108 的第一条闸门：出厂宽度 264 这一档，栏内版式必须还原成
    /// `control_draw.gml` 里那些裸数字（`x1 = 4`，所以 126 → 130）。
    #[test]
    fn panel_layout_is_the_original_at_default_width() {
        let l = PanelLayout::new(264.0, true, true);
        assert_eq!((l.volume, l.pan), (true, true));
        assert_eq!(l.buttons_x(), 130.0); // x1 + 126
        assert_eq!(l.pan_x(), 112.0); // x1 + 108
        assert_eq!(l.vol_x(), 92.0); // x1 + 88
        assert_eq!(l.name_box_right(), 89.0); // spr_layerbox 名称格右边线 x1 + 85
        assert_eq!(l.name_w(), 72.0); // 原版常驻输入框的内容宽
        assert_eq!(l.strip_w(), 256.0); // spr_layerbox 原宽（渲染转储里量到的行底图正是 256×34）
        assert_eq!(l.macro_scale(1.0), 1.0); // 宏条 255 宽 + 右边空 5
    }

    /// 原版 `x1 + 126 - !realvolume - realstereo * 10`：按偏好藏字形时按钮只左挪
    /// 1px / 10px。端口里 `prefs.mono` 就是那个「关闭立体声」勾选框。
    #[test]
    fn panel_layout_keeps_the_original_glyph_shift() {
        assert_eq!(PanelLayout::new(264.0, false, true).buttons_x(), 129.0);
        assert_eq!(PanelLayout::new(264.0, true, false).buttons_x(), 120.0);
        assert_eq!(PanelLayout::new(264.0, false, false).buttons_x(), 119.0);
    }

    /// 拥挤降级的三档边界（名称区留不住 30px 才动字形），且按钮永远出栏不了。
    #[test]
    fn crowded_panel_drops_glyphs_before_buttons() {
        let l = PanelLayout::new(222.0, true, true);
        assert_eq!((l.volume, l.pan, l.name_w()), (true, true, 30.0));
        let l = PanelLayout::new(221.0, true, true);
        assert_eq!((l.volume, l.pan), (false, true));
        assert_eq!(l.name_w(), 49.0);
        let l = PanelLayout::new(201.0, true, true);
        assert_eq!((l.volume, l.pan), (false, false));
        assert_eq!(l.name_w(), 47.0);
        let l = PanelLayout::new(LAYER_PANEL_MIN_WIDTH, true, true);
        assert!(l.buttons_x() + CLUSTER_W <= LAYER_PANEL_MIN_WIDTH - CLUSTER_PAD);
        assert!(l.name_w() > 0.0);
    }

    /// 宽度改的是自动布局，不是把整栏等比拉伸：末按钮右沿始终贴着 `pw - 10`。
    #[test]
    fn wider_panel_only_moves_the_name_area() {
        let l = PanelLayout::new(420.0, true, true);
        assert_eq!(l.buttons_x() + CLUSTER_W, 410.0);
        assert_eq!(l.name_box_right(), l.vol_x() - NAME_BOX_GAP);
        assert_eq!(l.macro_scale(1.0), 1.0); // 富余宽度下宏条不再缩
    }

    /// 头部那一排同样在 264 还原成原版的 108 / 184 / 右沿 93。
    #[test]
    fn header_layout_is_the_original_at_default_width() {
        let h = HeaderLayout::new(264.0, false, 60.0, 45.0);
        assert_eq!((h.tempo_x, h.bars_x, h.time_right), (108.0, 184.0, 93.0));
        assert_eq!((h.bars, h.compact_time), (true, false));
        assert_eq!(HeaderLayout::room(108.0, false), 93.0);
        assert_eq!(HeaderLayout::room(108.0, true), 95.0);
    }

    /// 显示不全的降级阶梯：先换短格式，再收掉只有悬停说明的小节框，最后按可视宽截断；
    /// 时间始终留在侧栏里（不挪去标尺带），速度框挪进空出来的那一格。
    #[test]
    fn header_layout_degrades_in_order() {
        let h = HeaderLayout::new(264.0, false, 95.0, 80.0);
        assert_eq!((h.bars, h.compact_time, h.tempo_x), (true, true, 108.0));
        let h = HeaderLayout::new(264.0, false, 200.0, 150.0);
        assert_eq!((h.bars, h.compact_time), (false, true));
        assert_eq!((h.tempo_x, h.time_right), (184.0, 169.0));
        // Fluent 是左对齐，可用宽从 9 起算；收掉小节框后同样挪到 184。
        let h = HeaderLayout::new(200.0, true, 300.0, 300.0);
        assert_eq!((h.bars, h.tempo_x, h.time_right), (false, 120.0, 105.0));
    }

    /// 行内文字：100% 缩放就是原版的 `y1 + 10` / `y1 + 18`；行高不够先贴到行底，
    /// 连一行都塞不下就不画（宁可少字，不让它压到上下两行）。
    #[test]
    fn row_text_follows_the_row_height() {
        assert_eq!(text_y(32.0, 1.0, 10.0, 9.0), Some(10.0));
        assert_eq!(text_y(32.0, 1.0, 18.0, 9.0), Some(18.0));
        assert_eq!(text_y(16.0, 0.5, 10.0, 9.0), Some(5.0));
        assert_eq!(text_y(8.0, 0.25, 10.0, 9.0), None);
        assert_eq!(glyph_scale(32.0), 1.0);
        assert_eq!(glyph_scale(8.0), 0.5);
    }

    #[test]
    fn short_time_format_drops_the_hours() {
        assert_eq!(time_str(3723.241), "01:02:03;241");
        assert_eq!(time_str_short(3723.241), "62:03;241");
        assert_eq!(time_str_short(9.05), "00:09;050");
        assert_eq!(time_str_short(-1.0), "00:00;000");
    }
}
