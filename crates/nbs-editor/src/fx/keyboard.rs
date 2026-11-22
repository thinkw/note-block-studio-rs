//! 琴键几何的唯一来源。
//!
//! 编辑器底部那条钢琴栏（`piano.rs`）和接管整屏的特效（Synthesia 的下落道、
//! 方块排的落点）必须用同一套横坐标，否则「音符落在哪条道上」和「哪个键在亮」
//! 会对不上。原版只有一份 `draw_piano.gml`，端口原先把算式写在 `show_piano` 的
//! 循环里，特效要用就得再抄一遍 —— 这里把算式抽出来，两边都调它。
//!
//! 键号沿用原版的绝对编号（0..=87，见 `nbs_core::model::KEY_MAX`）：0 是 A0，
//! 87 是 C8。白键序号 `w`（第几个白键）到键号是 `key = w + (w 之前的黑键数)`，
//! 而「白键 w 右边有没有黑键」取决于 `w % 7` 是否落在 1、4 上（B-C、E-F 那两处
//! 半音）—— 原版的 `sharpkeys` 就是这个数。

use egui::{Pos2, Rect, Vec2};
use nbs_core::model::KEY_MAX;

/// 原版白键步进 `xx + 39 * a`。
pub const KEY_W: f32 = 39.0;
/// `spr_piano` 一帧 40×121：白键可见高度。
pub const ART_H: f32 = 121.0;
/// 白键命中区 `mouse_rectangle(xx + 39a, yy, 39, 128)`。
pub const WHITE_HIT_H: f32 = 128.0;
/// 黑键画在白键左边 19px、上边 7px 处。
pub const BLACK_DRAW_X: f32 = -19.0;
pub const BLACK_LIFT: f32 = 7.0;
/// 黑键命中区 `mouse_rectangle(xx + 39a - 12, yy - 7, 25, 71)`：
/// sprite 不透明的第 7..31 列画在 `-19` 处，正好落在白键左缘的 `-12..+13`。
pub const BLACK_HIT_X: f32 = -12.0;
pub const BLACK_HIT_W: f32 = 25.0;
pub const BLACK_HIT_H: f32 = 71.0;
/// 整台键盘的白键数（52 白 + 36 黑 = 88 键）。原版右移上限 `startkey + k < 52`。
pub const WHITE_SLOTS: usize = 52;

/// 键号是白键吗。以 A0 为 0 号键，所以半音阶里的白键是 A、B、C、D、E、F、G。
pub fn is_white_key(key: u8) -> bool {
    matches!(key % 12, 0 | 2 | 3 | 5 | 7 | 8 | 10)
}

/// 白键序号 `w` 之前有多少黑键 —— 原版 `sharpkeys`。
pub fn blacks_before(w: usize) -> usize {
    (0..w)
        .filter(|a| !matches!(a % 7, 1 | 4))
        .count()
}

/// 第 `w` 个白键的键号；越过 87 返回 `None`（原版 `RIGHT_LIMIT` 之后就是空槽）。
pub fn white_key(w: usize) -> Option<u8> {
    let key = w + blacks_before(w);
    (key <= KEY_MAX as usize).then_some(key as u8)
}

/// 第 `w` 个白键右边那颗黑键的键号；半音交界处或越过键盘末端返回 `None`。
pub fn black_key_right_of(w: usize) -> Option<u8> {
    if matches!(w % 7, 1 | 4) {
        return None;
    }
    let key = white_key(w)? + 1;
    (key <= KEY_MAX).then_some(key)
}

/// 一帧的键盘摆放：横坐标原点 + 白键槽范围。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyboardLayout {
    /// 白键顶边的左上角（已含黑键上探的 `BLACK_LIFT`）。
    pub origin: Pos2,
    /// 界面缩放（`piano_scale` 那套像素倍率）。
    pub scale: f32,
    /// 第一个白键槽（原版 `startkey`）。
    pub first: usize,
    /// 显示的白键槽数（原版 `keysshow`）。
    pub slots: usize,
}

impl KeyboardLayout {
    /// 按可用宽度摆放键盘：放不下多少白键就显示多少，居中取整（原版 `floor`）。
    /// `centered_on` 是重新排布时想让它居中的白键序号（原版用 27，即 A0 起第 27 个白键）。
    pub fn fit(area: Rect, scale: f32, centered_on: usize) -> Self {
        let u = scale.max(f32::EPSILON);
        let slots = WHITE_SLOTS
            .min((area.width() / (KEY_W * u)).floor() as usize)
            .max(1);
        // 中心位夹进 `[0, WHITE_SLOTS - slots]`：不夹的话整台键盘时 first=1，
        // 槽 0 的 A0 永远画不出来（#110）。
        let first = centered_on.saturating_sub(slots / 2).min(WHITE_SLOTS - slots);
        let x0 = area.min.x + ((area.width() - slots as f32 * KEY_W * u) / 2.0).floor();
        Self {
            origin: Pos2::new(x0, area.min.y + BLACK_LIFT * u),
            scale: u,
            first,
            slots,
        }
    }

    /// 槽 `a`（`0..slots`）左缘的横坐标。
    pub fn slot_x(&self, a: usize) -> f32 {
        self.origin.x + a as f32 * KEY_W * self.scale
    }

    /// 槽 `a` 的白键键号。
    pub fn slot_white_key(&self, a: usize) -> Option<u8> {
        if a >= self.slots {
            return None;
        }
        white_key(self.first + a)
    }

    /// 槽 `a` 左边那颗黑键的键号（也就是前一个槽的右邻）。
    pub fn slot_black_key(&self, a: usize) -> Option<u8> {
        if a == 0 || a >= self.slots {
            return None;
        }
        black_key_right_of(self.first + a - 1)
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w * self.scale, h * self.scale))
    }

    /// 白键可见矩形（不含原版 128 高的命中下探）。
    pub fn white_rect(&self, a: usize) -> Rect {
        self.rect(self.slot_x(a), self.origin.y, KEY_W, ART_H)
    }

    /// 白键命中矩形（原版比可见区高 7px，够到面板顶）。
    pub fn white_hit(&self, a: usize) -> Rect {
        self.rect(self.slot_x(a), self.origin.y, KEY_W, WHITE_HIT_H)
    }

    /// 槽 `a` 的黑键矩形 = 它的命中矩形（sprite 不透明部分正好铺满这 25px）。
    pub fn black_rect(&self, a: usize) -> Option<Rect> {
        self.slot_black_key(a)?;
        // 偏移量是「未缩放像素」，得和宽高一样乘倍率，否则缩放一开黑白键就错位。
        Some(self.rect(
            self.slot_x(a) + BLACK_HIT_X * self.scale,
            self.origin.y - BLACK_LIFT * self.scale,
            BLACK_HIT_W,
            BLACK_HIT_H,
        ))
    }

    /// 键号 → 可见矩形。不在本帧显示范围内的键返回 `None`，
    /// 特效据此把音符挡在键盘之外，而不是画到面板外面。
    pub fn rect_of_key(&self, key: u8) -> Option<Rect> {
        for a in 0..self.slots {
            let w = self.first + a;
            if white_key(w) == Some(key) {
                return Some(self.white_rect(a));
            }
            if black_key_right_of(w) == Some(key) {
                return self.black_rect(a + 1);
            }
        }
        None
    }

    /// 键号所在的下落道中心（横坐标）。
    pub fn lane_center(&self, key: u8) -> Option<f32> {
        self.rect_of_key(key).map(|r| r.center().x)
    }

    /// 键号所在道的宽度：白键道 39px、黑键道 25px（都乘缩放）。
    pub fn lane_width(&self, key: u8) -> f32 {
        if is_white_key(key) {
            KEY_W * self.scale
        } else {
            BLACK_HIT_W * self.scale
        }
    }

    /// 本帧显示到的键号区间（特效据此把音符夹进可见键盘）。
    pub fn visible_keys(&self) -> (u8, u8) {
        let lo = self.slot_white_key(0).unwrap_or(0);
        let hi = self
            .slot_white_key(self.slots.saturating_sub(1))
            .unwrap_or(KEY_MAX);
        (lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn full() -> KeyboardLayout {
        KeyboardLayout {
            origin: pos2(100.0, 50.0),
            scale: 1.0,
            first: 0,
            slots: WHITE_SLOTS,
        }
    }

    /// 从某个白键序号起把整台键盘排满，用来比对原 `show_piano` 的累加式。
    fn from(first: usize) -> KeyboardLayout {
        KeyboardLayout {
            first,
            slots: WHITE_SLOTS - first,
            ..full()
        }
    }

    /// 原版 `startkey = 0` 时那一串白键键号（A0 起，B-C、E-F 处只差一个半音）。
    #[test]
    fn white_key_numbers_match_the_original_step() {
        let keys: Vec<u8> = (0..8).map(|w| white_key(w).unwrap()).collect();
        assert_eq!(keys, vec![0, 2, 3, 5, 7, 8, 10, 12]);
        assert_eq!(white_key(51), Some(87), "最后一个白键槽是 C8");
        assert_eq!(white_key(52), None, "52 个白键之后是空槽");
    }

    /// 抽出来的闭式必须和 `piano.rs` 原来的逐步累加一致，否则键盘会走形。
    #[test]
    fn closed_form_equals_the_piano_loop() {
        for first in [0usize, 1, 14, 27, 51] {
            let layout = from(first);
            // 原循环：b 从 0 起，每个槽 +1 或 +2，带黑键的那个槽再多算一格。
            let sharpkeys = (0..first).filter(|a| *a % 7 != 1 && *a % 7 != 4).count();
            let mut b = 0usize;
            for a in 0..(WHITE_SLOTS - first) {
                let c = (first + a) % 7;
                let with_black = a > 0 && c != 2 && c != 5;
                let white = first + sharpkeys + b + if with_black { 1 } else { 0 };
                if white <= KEY_MAX as usize {
                    assert_eq!(
                        layout.slot_white_key(a),
                        Some(white as u8),
                        "first={first} a={a} 的白键"
                    );
                    assert_eq!(
                        layout.slot_black_key(a),
                        with_black.then(|| white as u8 - 1),
                        "first={first} a={a} 的黑键"
                    );
                }
                b += if with_black { 2 } else { 1 };
            }
        }
    }

    #[test]
    fn black_keys_land_on_the_semitone_gaps() {
        let blacks: Vec<Option<u8>> = (0..8).map(black_key_right_of).collect();
        assert_eq!(
            blacks,
            vec![Some(1), None, Some(4), Some(6), None, Some(9), Some(11), Some(13)]
        );
        assert_eq!(black_key_right_of(51), None, "C8 右边没有键");
    }

    #[test]
    fn every_key_has_a_slot() {
        let l = full();
        for key in 0..=KEY_MAX {
            let r = l.rect_of_key(key).unwrap_or_else(|| panic!("键 {key} 落到了显示范围外"));
            assert!(r.width() > 0.0 && r.height() > 0.0);
        }
        // 白键 39px、黑键 25px，都从各自的可见矩形读出来；黑键顶边高 7px。
        assert_eq!(l.rect_of_key(0).unwrap().width(), KEY_W);
        assert_eq!(l.rect_of_key(0).unwrap().min.y, 50.0);
        assert_eq!(l.rect_of_key(1).unwrap().width(), BLACK_HIT_W);
        assert_eq!(l.rect_of_key(1).unwrap().min.y, 50.0 - BLACK_LIFT);
    }

    #[test]
    fn keys_outside_the_keyboard_have_no_slot() {
        let l = full();
        // 88 号根本不存在（键号只到 C8 = 87）。
        assert_eq!(l.rect_of_key(88), None);
        // 只显示中间一段时，两头的键拿不到矩形 —— 特效据此不画这些音符。
        let l = KeyboardLayout {
            first: 20,
            slots: 20,
            ..full()
        };
        assert_eq!(l.rect_of_key(0), None, "A0 被挤出了显示范围");
        assert_eq!(l.rect_of_key(KEY_MAX), None, "C8 也在范围外");
        assert!(l.rect_of_key(l.slot_white_key(0).unwrap()).is_some());
        assert!(l.rect_of_key(l.slot_white_key(19).unwrap()).is_some());
    }

    #[test]
    fn key_rects_do_not_overlap_their_neighbours() {
        let l = full();
        for a in 1..l.slots {
            assert_eq!(
                l.white_rect(a - 1).max.x,
                l.white_rect(a).min.x,
                "白键槽 {a} 之间漏了缝"
            );
        }
        let mut blacks: Vec<Rect> = (0..l.slots).filter_map(|a| l.black_rect(a)).collect();
        blacks.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        assert_eq!(blacks.len(), 36, "整台键盘 36 颗黑键");
        for pair in blacks.windows(2) {
            assert!(
                pair[0].max.x <= pair[1].min.x + 0.01,
                "黑键 {:?} 与 {:?} 重叠",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn fit_clamps_centering_so_a0_is_never_dropped() {
        // 88 键正好放得下：first 必须夹到 0，槽 0 是 A0。
        let area = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(52.0 * KEY_W, 128.0));
        let l = KeyboardLayout::fit(area, 1.0, 27);
        assert_eq!((l.first, l.slots), (0, WHITE_SLOTS));
        assert_eq!(l.slot_white_key(0), Some(0));
        assert_eq!(l.visible_keys(), (0, KEY_MAX));
        // 一半宽：26 槽、以 27 号白键为中心 ⇒ 第一个白键序号 14 ⇒ 键号 24。
        let area = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(26.0 * KEY_W, 128.0));
        let l = KeyboardLayout::fit(area, 1.0, 27);
        assert_eq!((l.first, l.slots), (14, 26));
        assert_eq!(l.slot_white_key(0), Some(24));
        assert_eq!(l.slot_white_key(25), Some(67));
        // 再窄也只留一个槽，别出现 0 槽导致的除不尽。
        let l = KeyboardLayout::fit(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1.0, 100.0)),
            1.0,
            27,
        );
        assert_eq!((l.first, l.slots), (27, 1));
    }

    #[test]
    fn scale_moves_everything_together() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(52.0 * KEY_W * 2.0, 300.0));
        let l = KeyboardLayout::fit(area, 2.0, 27);
        assert_eq!(l.origin.y, BLACK_LIFT * 2.0, "黑键上探那段也跟着缩放");
        assert_eq!(l.white_rect(1).min.x, KEY_W * 2.0);
        assert_eq!(l.lane_width(0), KEY_W * 2.0);
        assert_eq!(l.lane_width(1), BLACK_HIT_W * 2.0);
        // 道宽决定胶囊宽：白键道比黑键道宽，Synthesia 才看得出黑白。
        // 这里 x0 落在 0，A0 那一条道的中心就是 39*2/2。
        assert_eq!(l.lane_center(0), Some(KEY_W));
        assert_eq!(l.lane_center(1), Some(79.0), "黑键道中心骑在两条白键之间");
    }

    #[test]
    fn white_and_black_are_told_apart_by_pitch_class() {
        assert!(is_white_key(0) && is_white_key(3) && is_white_key(87));
        // 0=A0 ⇒ 音级 1 是 A#、4 是 C#、85(F#6) 也是黑键；86 = B6 是白键。
        assert!(!is_white_key(1) && !is_white_key(4) && !is_white_key(85));
        assert!(is_white_key(86));
        // 键盘上 52 个白键、36 个黑键。
        assert_eq!((0..=KEY_MAX).filter(|k| is_white_key(*k)).count(), 52);
        assert_eq!((0..=KEY_MAX).filter(|k| !is_white_key(*k)).count(), 36);
    }
}
