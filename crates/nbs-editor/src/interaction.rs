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
}

#[derive(Clone, Debug)]
pub struct Interaction {
    pub mode: DragMode,
    pub pressed_cell: Option<(i64, i64)>,
    pub pressed_pos: Option<Pos2>,
    pub hover: Option<(i64, i64)>,
    /// 拖拽越过网格边界时的自动滚动增量（每帧累加，上限 4 格）。
    pub edge_scroll: Vec2,
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
            hover: None,
            edge_scroll: Vec2::ZERO,
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
    pub fn is_dragging(&self) -> bool {
        self.mode != DragMode::None
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
}

impl InputSnapshot {
    pub fn read(ui: &Ui) -> Self {
        ui.input(|i| Self {
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
        })
    }
}
