//! 视口与坐标换算 —— 视口剔除（culling）的所在地。
//!
//! 原版每格固定 32px（`draw_block(x1 + 2 + 32 * a - note_offset, y1 + 34 + 32 * b, …)`），
//! 没有网格缩放，只有整窗口的 `window_scale`。所有绘制只遍历
//! `[start_tick, start_tick+cols)`，与歌曲总长度无关。

use egui::{Pos2, Rect, Vec2};

/// 原版每格 32px。
pub const BASE_CELL: f32 = 32.0;
/// 原版图层栏：x1 = 4，`spr_layerbox` 宽 256 → 列宽 264。
pub const LAYER_PANEL_WIDTH: f32 = 264.0;
/// 原版时间轴带：y1+2 .. y1+34。
pub const TIMELINE_HEIGHT: f32 = 32.0;
/// 原版滚动条厚度 32。
pub const SCROLLBAR_SIZE: f32 = 32.0;
/// 原版白键高 128，上方留 2px。
pub const PIANO_HEIGHT: f32 = 130.0;
/// 原版状态栏分隔线在 `rh - 24`。
pub const STATUS_HEIGHT: f32 = 24.0;
/// 原版菜单标签 y = 1，高 18 → 整条 20。
pub const MENU_HEIGHT: f32 = 20.0;
/// 原版图标条 y = 20..49（`spr_iconbar` 高 30），下方 4px 是窗口底色空带：
/// 网格带顶在 `y1 + 2 = 52`，图层面板头部带顶在 `54`。
/// 整条取 34 是为了让面板高度之和 = 54，时间轴带和第一行正好落在原版像素上。
pub const TOOLBAR_HEIGHT: f32 = 34.0;
/// 原版图标条自身的可见高度。
pub const TOOLBAR_STRIP_HEIGHT: f32 = 30.0;

#[derive(Clone, Debug)]
pub struct Viewport {
    /// 左上角可见 tick（原版 `starta`）。
    pub start_tick: u32,
    /// 左上角可见 layer（原版 `startb`）。
    pub start_layer: u32,
    /// 网格区域左上角（屏幕坐标），每帧由布局计算。
    pub origin: Pos2,
    /// 网格区域尺寸。
    pub size: Vec2,
    /// 播放跟随时的亚 tick 平滑偏移（原版 `note_offset`）。
    pub note_offset: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            start_tick: 0,
            start_layer: 0,
            origin: Pos2::ZERO,
            size: egui::vec2(800.0, 400.0),
            note_offset: 0.0,
        }
    }
}

impl Viewport {
    /// 原版网格恒 32px，没有缩放。
    pub fn cell(&self) -> f32 {
        BASE_CELL
    }

    /// 可见列数（多算 1 列给平滑滚动留余量）。
    pub fn columns(&self) -> u32 {
        (self.size.x / self.cell()).ceil() as u32 + 1
    }

    pub fn rows(&self) -> u32 {
        (self.size.y / self.cell()).ceil() as u32 + 1
    }

    pub fn tick_x(&self, tick: i64) -> f32 {
        self.origin.x + (tick - self.start_tick as i64) as f32 * self.cell() - self.note_offset
    }

    pub fn layer_y(&self, layer: i64) -> f32 {
        self.origin.y + (layer - self.start_layer as i64) as f32 * self.cell()
    }

    pub fn x_tick(&self, x: f32) -> i64 {
        self.start_tick as i64
            + ((x + self.note_offset - self.origin.x) / self.cell()).floor() as i64
    }

    pub fn y_layer(&self, y: f32) -> i64 {
        self.start_layer as i64 + ((y - self.origin.y) / self.cell()).floor() as i64
    }

    /// 屏幕坐标 → 绝对格坐标，超出网格返回 None。
    pub fn cell_at(&self, pos: Pos2) -> Option<(u32, u32)> {
        if !Rect::from_min_size(self.origin, self.size).contains(pos) {
            return None;
        }
        let t = self.x_tick(pos.x);
        let l = self.y_layer(pos.y);
        if t < 0 || l < 0 {
            return None;
        }
        Some((t as u32, l as u32))
    }

    pub fn cell_rect(&self, tick: i64, layer: i64) -> Rect {
        let x = self.tick_x(tick);
        let y = self.layer_y(layer);
        Rect::from_min_size(Pos2::new(x, y), Vec2::splat(self.cell()))
    }

    /// 可见范围（半开区间），供 `Song::notes_in_view` 使用。
    /// **绝不会返回整首歌的范围** —— 这是 culling 的硬约束。
    pub fn visible_range(&self) -> (u32, u32, u32, u32) {
        let t0 = self.start_tick;
        let t1 = t0 + self.columns();
        let l0 = self.start_layer;
        let l1 = l0 + self.rows();
        (t0, t1, l0, l1)
    }

    /// 是否落在可见矩形内（选区/播放头的整体剔除）。
    pub fn rect_intersects(&self, x0: i64, y0: i64, x1: i64, y1: i64) -> bool {
        let (t0, t1, l0, l1) = self.visible_range();
        x1 >= t0 as i64 && x0 <= t1 as i64 && y1 >= l0 as i64 && y0 <= l1 as i64
    }

    /// 播放头滚动跟随，对应原版 `marker_pagebypage` 的三档
    /// （`control_draw.gml:1090-1106` + `:375`）。
    /// `backward` 就是原版的 `forward = -1`（按住 ← 或快退按钮）。
    pub fn follow_marker(&mut self, marker: f64, mode: u8, backward: bool) {
        // 原版用的是 `totalcols`（完整可见的列数）；`columns()` 为平滑滚动多算了 1 列，
        // 用它翻页会晚一列触发、居中也会偏一列。
        let cols = (self.size.x / self.cell()).floor().max(1.0) as f64;
        let m = marker.max(0.0);
        if mode == 1 {
            let start = self.start_tick as f64;
            // 按页：只有播放头跑出可视页才翻页，翻完播放头落在第 2 列（`starta = marker_pos - 1`）。
            if m.floor() >= start + cols - 1.0 {
                self.start_tick = (m - 1.0).max(0.0) as u32;
            } else if m.floor() < start + 1.0 && backward {
                // 原版 1098 行：往回翻页只在按住 ← / 快进快退的「退」那一侧时才发生。
                self.start_tick = (m.floor() - cols + 1.0).max(0.0) as u32;
            }
            self.note_offset = 0.0;
        } else {
            let target = (m - (cols / 2.0).ceil()).max(0.0);
            self.start_tick = target.floor() as u32;
            // 原版 `note_offset` 只在按帧（=2）时才有亚刻位移；按刻（=0）整刻对齐。
            self.note_offset = if mode == 2 {
                (target.fract() * self.cell() as f64) as f32
            } else {
                0.0
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Song;

    #[test]
    fn coordinate_roundtrip() {
        let mut v = Viewport::default();
        v.origin = Pos2::new(100.0, 60.0);
        v.size = egui::vec2(640.0, 320.0);
        v.start_tick = 37;
        v.start_layer = 4;
        assert_eq!(v.x_tick(v.tick_x(52)), 52);
        assert_eq!(v.y_layer(v.layer_y(9)), 9);
        assert_eq!(v.cell_at(Pos2::new(100.0, 60.0)), Some((37, 4)));
        assert_eq!(v.cell_at(Pos2::new(10.0, 10.0)), None);
    }

    #[test]
    fn visible_range_is_bounded_by_screen() {
        let mut v = Viewport::default();
        v.size = egui::vec2(640.0, 320.0);
        let (t0, t1, l0, l1) = v.visible_range();
        assert!(t1 - t0 <= 21);
        assert!(l1 - l0 <= 11);
        let song = Song::demo();
        assert_eq!(song.notes_in_view(t0, t1, l0, l1).len() <= 22 * 12, true);
    }

    /// 原版 `marker_pagebypage` 的三档必须互相有真实差别。
    #[test]
    fn follow_marker_has_three_modes() {
        let mut v = Viewport::default();
        v.size = egui::vec2(640.0, 320.0); // 原版 totalcols = 640 / 32 = 20
        v.start_tick = 0;
        v.follow_marker(10.0, 1, false);
        assert_eq!((v.start_tick, v.note_offset), (0, 0.0), "按页：没跑出可视页就不动");
        v.follow_marker(20.0, 1, false);
        assert_eq!((v.start_tick, v.note_offset), (19, 0.0), "按页：翻过后播放头落在第 2 列");
        v.follow_marker(5.0, 1, false);
        assert_eq!(v.start_tick, 19, "按页：不按住 ← 不会自己往回翻页");
        v.follow_marker(5.0, 1, true);
        assert_eq!(v.start_tick, 0, "按页：按住 ← 倒回左边界外时回到它自己那页");
        v.start_tick = 0;
        v.follow_marker(24.5, 0, false);
        assert_eq!((v.start_tick, v.note_offset), (14, 0.0), "按刻：居中但整刻对齐");
        v.start_tick = 0;
        v.follow_marker(24.5, 2, false);
        assert_eq!((v.start_tick, v.note_offset), (14, 16.0), "按帧：居中且带半刻位移");
    }
}
