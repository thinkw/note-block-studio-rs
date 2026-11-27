//! 交互状态机（对应原版 `window` + `select` + `select_press*` 那一组变量）。
//!
//! 原版把输入与绘制混在 `control_draw()` 的一帧里；这里拆成
//! `InputSnapshot → 状态迁移 → 绘制`，但保留同一套语义：
//! 框选(1) / 减选(2)、浮动选区拖拽、修饰键、边缘自动滚动。

use egui::{CursorIcon, Pos2, Ui, Vec2};
use nbs_core::selection::SelectionSnapshot;

#[derive(Clone, Debug, PartialEq)]
pub enum DragMode {
    None,
    /// 左键拖框 = 加选，右键拖框 = 减选。
    BoxSelect {
        anchor: (i64, i64),
        subtract: bool,
        alt_copy: bool,
    },
    /// 按住浮动选区里的音符拖动。
    MoveSelection {
        grab: (i64, i64),
        origin: (i64, i64),
        before: SelectionSnapshot,
    },
    /// 中键 / 空格拖动画布。
    PanView {
        start: Pos2,
        start_tick: u32,
        start_layer: u32,
    },
    DragScrollbarH { grab: f32 },
    DragScrollbarV { grab: f32 },
    DragMarker,
    /// 原版 `timeline_pressa >= 0`：在标尺空白处按下、还没横向拖开。
    /// 拖开 → 画一段区间（[`DragMode::DragSectionEnd`]）；原地松开 → 播放头跳过去。
    PressTimeline {
        pressa: i64,
    },
    /// 原版 `w_dragsection_start`：拖区间左边界（`control_draw.gml:1102-1110`）。
    DragSectionStart,
    /// 原版 `w_dragsection_end`：拖区间右边界（`control_draw.gml:1112-1120`），
    /// 在标尺上横向拖出新段时也用它。
    DragSectionEnd,
}

#[derive(Clone, Debug)]
pub struct Interaction {
    pub mode: DragMode,
    pub pressed_cell: Option<(i64, i64)>,
    pub pressed_pos: Option<Pos2>,
    /// 本帧指针最新位置（`InputSnapshot::pointer` 的镜像），
    /// 供绘制阶段画框选矩形用 —— `draw_grid` 不直接读输入。
    pub latest_pos: Option<Pos2>,
    pub hover: Option<(i64, i64)>,
    /// 拖拽越过网格边界时的自动滚动增量（每帧累加，上限 4 格）。
    pub edge_scroll: Vec2,
    /// 触控板滚动的浮点余量（原版 `macos_scroll_temp_starta/b`）：x 管刻、y 管层，
    /// 不足一格的滑动先进这里，攒够一整格才动 `start_tick` / `start_layer`。
    pub scroll_carry: Vec2,
    /// 正在进行的这一轮滚动（见 `grid::scroll_gesture`）。`None` = 轮与轮之间。
    pub scroll_gesture: Option<ScrollGesture>,
    pub cursor: CursorIcon,
    /// 本帧点击是否发生在网格区域（阻止弹窗误吞输入）。
    pub click_in_grid: bool,
    /// 右键已弹出上下文菜单，松开时不要再删除音符。
    pub suppress_next_click: bool,
    pub ctrl_at_press: bool,
    pub alt_at_press: bool,
    pub secondary_at_press: bool,
    /// 本帧绘制的可见音符数（状态栏性能显示）。
    pub visible_notes: usize,
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            mode: DragMode::None,
            pressed_cell: None,
            pressed_pos: None,
            latest_pos: None,
            hover: None,
            edge_scroll: Vec2::ZERO,
            scroll_carry: Vec2::ZERO,
            scroll_gesture: None,
            cursor: CursorIcon::Default,
            click_in_grid: false,
            suppress_next_click: false,
            ctrl_at_press: false,
            alt_at_press: false,
            secondary_at_press: false,
            visible_notes: 0,
        }
    }
}

impl Interaction {
    /// 网格式拖拽（框选 / 移选区 / 平移）是否在进行中。标尺那三种状态
    /// （按下待判定、拖区间两端）由 `grid::handle_timeline_input` 自己收尾，
    /// 不归这里 —— 原版 `w_dragsection_*` 也只在松开左键时才结束，
    /// 和鼠标跑没跑出编辑区无关。
    pub fn is_dragging(&self) -> bool {
        !matches!(
            self.mode,
            DragMode::None
                | DragMode::PressTimeline { .. }
                | DragMode::DragSectionStart
                | DragMode::DragSectionEnd
        )
    }
    pub fn reset(&mut self) {
        self.mode = DragMode::None;
        self.pressed_cell = None;
        self.pressed_pos = None;
        self.edge_scroll = Vec2::ZERO;
        self.cursor = CursorIcon::Default;
        self.ctrl_at_press = false;
        self.alt_at_press = false;
        self.secondary_at_press = false;
    }
}

/// 一轮连续滚动开始时定下的动作，整轮沿用，中途不改道（见 `grid::scroll_gesture`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAction {
    /// 平移视口。
    Pan,
    /// 改这颗音符的属性（原版 `exist && changepitch`）。
    Note(i64, i64),
    /// 原版 `mousewheel = 1`：轮换音色。
    Instrument,
    /// 原版 `mousewheel = 2`：轮换键。
    Key,
}

/// 正在进行的这一轮滚动。触控板一轮 = 手指按下到抬起后的漏桶放完，可能几十帧；
/// 期间指针底下的格子会随画面移动而变，所以判定必须在轮开始时钉住。
#[derive(Clone, Copy, Debug)]
pub struct ScrollGesture {
    /// true = 触控板细滑：整格之外的漏桶余量（`scroll_fine`）也要放行。
    /// false = 鼠标滚轮：只认带事件的帧，漏桶尾巴不平移（否则一格会变 1.25 格、拖 200ms）。
    pub touch: bool,
    pub action: ScrollAction,
    /// 最近一次看到这轮输入的 egui 时间（秒），超过 `grid::SCROLL_STREAM_GAP` 就算新一轮。
    pub seen_at: f64,
}

/// 一帧输入快照，避免在绘制过程中反复借用 `InputState`。
#[derive(Clone, Debug, Default)]
pub struct InputSnapshot {
    pub hover: Option<Pos2>,
    pub pointer: Option<Pos2>,
    pub primary_pressed: bool,
    pub primary_down: bool,
    pub primary_released: bool,
    pub secondary_pressed: bool,
    pub secondary_down: bool,
    pub secondary_released: bool,
    pub middle_pressed: bool,
    pub middle_down: bool,
    pub middle_released: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub scroll: Vec2,
    /// `smooth_scroll_delta`：egui 把滚轮量过一遍漏桶（`input_state/mod.rs:369-385`，
    /// 每帧放 `1 − 0.1^(dt/0.1)` ≈ 32%）后本帧放出来的那部分。
    /// **注意**：这个桶对鼠标滚轮同样生效，所以它非零并不代表来自触控板 ——
    /// 本机实测（Precision Touchpad YML1176）鼠标一格也会给出 `fine ≈ 12.8`。
    /// 真正的来源判据是 `wheel_trackpad`。
    pub scroll_fine: Vec2,
    /// 本帧的滚轮事件像触控板：出现过分数的格（Windows 一格 = 120 的整数倍，
    /// 精确触控板给的是小数），或单事件不足一整格。
    pub wheel_trackpad: bool,
    /// `InputState::time`：给滚轮流定超时用（反应式渲染下帧数不能当时间用）。
    pub time: f64,
}

impl InputSnapshot {
    pub fn read(ui: &Ui) -> Self {
        ui.input(|i| {
            // 判来源：Windows 的鼠标滚轮每事件是 120 的整数倍（winit 除以 120 后是
            // 1.0 / 3.0 这种整数格），精确触控板给的是小数、且常常不足一格。
            // 本机 900 条事件实测：91% 是小数格，鼠标那一段全是整数格。
            let sub_notch = |v: f32| -> bool {
                let a = v.abs();
                a > 0.0 && (a < 1.0 || (a - a.round()).abs() >= 0.011)
            };
            let mut trackpad = false;
            for event in i.events.iter() {
                if let egui::Event::MouseWheel { unit, delta, .. } = event {
                    let fine = match unit {
                        egui::MouseWheelUnit::Point => true,
                        egui::MouseWheelUnit::Page => false,
                        egui::MouseWheelUnit::Line => sub_notch(delta.x) || sub_notch(delta.y),
                    };
                    if fine {
                        trackpad = true;
                        break;
                    }
                }
            }
            Self {
                hover: i.pointer.hover_pos(),
                pointer: i.pointer.latest_pos(),
                primary_pressed: i.pointer.button_pressed(egui::PointerButton::Primary),
                primary_down: i.pointer.primary_down(),
                primary_released: i.pointer.button_released(egui::PointerButton::Primary),
                secondary_pressed: i.pointer.button_pressed(egui::PointerButton::Secondary),
                secondary_down: i.pointer.secondary_down(),
                secondary_released: i.pointer.button_released(egui::PointerButton::Secondary),
                middle_pressed: i.pointer.button_pressed(egui::PointerButton::Middle),
                middle_down: i.pointer.middle_down(),
                middle_released: i.pointer.button_released(egui::PointerButton::Middle),
                ctrl: i.modifiers.ctrl,
                shift: i.modifiers.shift,
                alt: i.modifiers.alt,
                scroll: i.raw_scroll_delta,
                scroll_fine: i.smooth_scroll_delta,
                wheel_trackpad: trackpad,
                time: i.time,
            }
        })
    }
}
