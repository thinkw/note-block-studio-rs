//! 视口与坐标换算 —— 视口剔除（culling）的所在地。
//!
//! 原版的 32px 固定格在这里抽成 `BASE_CELL * zoom`，并新增了独立的网格缩放
//! （原版只有整窗口 UI 缩放）。所有绘制只遍历 `[start_tick, start_tick+cols)`，
//! 与歌曲总长度无关。

use egui::{Pos2, Rect, Vec2};
use nbs_core::model::Song;

/// 原版每格 32px。
pub const BASE_CELL: f32 = 32.0;
pub const LAYER_PANEL_WIDTH: f32 = 284.0;
pub const TIMELINE_HEIGHT: f32 = 32.0;
pub const SCROLLBAR_SIZE: f32 = 16.0;
pub const PIANO_HEIGHT: f32 = 132.0;
pub const STATUS_HEIGHT: f32 = 26.0;
pub const MENU_HEIGHT: f32 = 26.0;
pub const TOOLBAR_HEIGHT: f32 = 38.0;

#[derive(Clone, Debug)]
pub struct Viewport {
    /// 左上角可见 tick（原版 `starta`）。
    pub start_tick: u32,
    /// 左上角可见 layer（原版 `startb`）。
    pub start_layer: u32,
    /// 网格独立缩放 0.5..=4.0。
    pub zoom: f32,
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
            zoom: 1.0,
            origin: Pos2::ZERO,
            size: egui::vec2(800.0, 400.0),
            note_offset: 0.0,
        }
    }
}

impl Viewport {
    pub fn cell(&self) -> f32 {
        BASE_CELL * self.zoom
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

    /// 夹取滚动位置，避免滚出歌曲边界太远。
    pub fn clamp_scroll(&mut self, song: &Song) {
        let max_tick = song.enda.max(song.song_length().saturating_sub(1));
        let max_start = max_tick.saturating_sub(self.columns().saturating_sub(2));
        self.start_tick = self.start_tick.min(max_start.max(64));
        let max_layer = song.layer_count().saturating_sub(1);
        let max_lstart = max_layer.saturating_sub(self.rows().saturating_sub(2));
        self.start_layer = self.start_layer.min(max_lstart.max(8));
        self.note_offset = self.note_offset.clamp(-self.cell(), self.cell());
    }

    /// 播放头滚动跟随：逐页 / 居中两种模式。
    pub fn follow_marker(&mut self, marker: f64, center: bool) {
        let cols = self.columns() as f64;
        let m = marker.max(0.0);
        if center {
            let target = (m - cols / 2.0).max(0.0);
            self.start_tick = target.floor() as u32;
            self.note_offset = ((target.fract()) * self.cell() as f64) as f32;
        } else {
            let page = (m / cols).floor() * cols;
            self.start_tick = page.max(0.0) as u32;
            self.note_offset = 0.0;
        }
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.5, 4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
