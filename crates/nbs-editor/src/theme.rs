//! 四套主题，对齐原版 `theme = 0..3`（经典 Aqua / 复古 90s / 暗黑 Dark / Fluent）。
//!
//! 颜色不是自己配的：`window_bg` 一律取原版 `control_draw.gml:147-152` 的
//! `window_background`（也就是 `draw_clear(window_background)` 的ClearColor），
//! Fluent 一栏的其余颜色取装机版自己窗口的实测像素，网格线取
//! `control_draw.gml:376-389` 的「黑色 + alpha 0.1 / 0.25 / 0.3」。

use egui::{Color32, Context, Style, Visuals};

use crate::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeKind {
    Aqua,
    Retro,
    Dark,
    #[default]
    Fluent,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 4] = [Self::Aqua, Self::Retro, Self::Dark, Self::Fluent];
    /// 原版首选项「主题」下拉框里印的名字（`draw_window_preferences.gml:274-276`）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Aqua => tr("Aqua", "经典"),
            Self::Retro => tr("90s", "复古"),
            Self::Dark => tr("Dark", "暗黑"),
            Self::Fluent => "Fluent",
        }
    }
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

/// GM 的 `$BBGGRR` 颜色常量转成 RGB。
const fn gm(c: u32) -> Color32 {
    Color32::from_rgb((c & 0xff) as u8, ((c >> 8) & 0xff) as u8, ((c >> 16) & 0xff) as u8)
}

/// 按 `t` 把 `b` 混进 `a`（`t` 取 0~1），用来从原版常量推出派生色。
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| ((x as f32) + ((y as f32) - (x as f32)) * t) as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub kind: ThemeKind,
    /// 原版 `fdark`：Fluent 主题的深色变体（外观页的「暗色模式」，仅 theme 3 生效）。
    pub fdark: bool,
    /// 原版 `window_background`：清屏色，也是所有面板的底色。
    pub window_bg: Color32,
    /// 图层行底（原版 Fluent 实测）。非 Fluent 主题原版没有单独的行色，等于 `window_bg`。
    pub row_bg: Color32,
    /// 菜单条 / 工具栏条（原版 Fluent 实测）。
    pub menu_bg: Color32,
    /// 尺子（拍号那一横条）底色，原版 Fluent 实测是纯白。
    pub ruler_bg: Color32,
    /// 音符格子上方的音名颜色：原版 `draw_block` 除 theme 2 用白，其余一律 c_yellow。
    pub note_name: Color32,
    pub bg: Color32,
    pub panel: Color32,
    pub grid_bg: Color32,
    pub grid_line: Color32,
    pub grid_beat: Color32,
    pub grid_bar: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// 原版 `accent1/2/3`：出厂 (0,120,212)，settings.ini [preferences] 持久化。
    /// 改色乐器只重算 9 档表，不碰这里。
    pub accent_base: [u8; 3],
    /// 原版 `accent[0..8]`：当前生效的 9 档强调色表。`draw_set_accent` /
    /// 彩虹循环 / Change Color 乐器都会重算它。
    pub accent_table: [Color32; 9],
    /// 界面实际用的那档（= `accent_table[4]`，Fluent 图标染色 / 按下划线）。
    pub accent: Color32,
    pub accent_soft: Color32,
    pub selection: Color32,
    pub subtract: Color32,
    pub locked: Color32,
    pub marker: Color32,
    pub border: Color32,
    pub button: Color32,
    pub button_hover: Color32,
    pub note_text: Color32,
    pub dark: bool,
}

impl Theme {
    /// 原版网格线：`draw_set_alpha(0.1)`，每 4 拍 `0.25`，小节线 `0.3`，颜色是黑。
    const LINE_A: u8 = 26;
    const BEAT_A: u8 = 64;
    const BAR_A: u8 = 77;

    /// 原版 `control_create.gml:143-145` 的 accent1/2/3 出厂值（RGB 0,120,212）。
    const FACTORY_ACCENT: [u8; 3] = [0, 120, 212];

    /// 原版 `draw_accent_init()` / `draw_set_accent()`：由一组 RGB 和其中的最大值
    /// `hiacc` 推 9 档强调色。逐条照抄，包括原稿 `accent[6]` 中间项只乘
    /// `0.1 * 2` 的写法。
    fn build_accent_table(rgb: [u8; 3]) -> [Color32; 9] {
        let [a, b, c] = [rgb[0] as f32, rgb[1] as f32, rgb[2] as f32];
        let hi = a.max(b).max(c);
        let at = |t: (f32, f32, f32)| {
            Color32::from_rgb(t.0 as u8, t.1 as u8, t.2 as u8)
        };
        [
            at((a * 0.7, b * 0.7, c * 0.7)),
            at((a * 0.8, b * 0.8, c * 0.8)),
            at((a * 0.9, b * 0.9, c * 0.9)),
            at((a, b, c)),
            at((a + (hi - a) * 0.1, b + (hi - b) * 0.1, c + (hi - c) * 0.1)),
            at((a + (hi - a) * 0.2, b + (hi - b) * 0.2, c + (hi - c) * 0.2)),
            at((a + (hi - a) * 0.3, b + (hi - b) * 0.2, c + (hi - c) * 0.3)),
            at((a + (hi - a) * 0.5, b + (hi - b) * 0.5, c + (hi - c) * 0.5)),
            at((a * 0.5, b * 0.5, c * 0.5)),
        ]
    }

    /// 原版 `scr_HSBtoRGB`（h 0..360，s/b 0..100）的逐行移植。
    /// 彩虹循环每帧喂 `hsb_to_rgb(hue, 100, 100)`（draw_accent_rainbow）。
    pub fn hsb_to_rgb(h: f32, s: f32, b: f32) -> [u8; 3] {
        let s = s / 100.0;
        let b = b / 100.0;
        let fl = |x: f32| (x * 255.0).floor() as u8;
        if s == 0.0 {
            let v = fl(b);
            return [v, v, v];
        }
        let mut temp_h = h / 360.0 * 6.0;
        if temp_h == 6.0 {
            temp_h = 0.0;
        }
        let i = temp_h.floor();
        let temp1 = b * (1.0 - s);
        let temp2 = b * (1.0 - s * (temp_h - i));
        let temp3 = b * (1.0 - s * (1.0 - (temp_h - i)));
        let rgb = match i as i32 {
            0 => [b, temp3, temp1],
            1 => [temp2, b, temp1],
            2 => [temp1, b, temp3],
            3 => [temp1, temp2, b],
            4 => [temp3, temp1, b],
            _ => [b, temp1, temp2],
        };
        [fl(rgb[0]), fl(rgb[1]), fl(rgb[2])]
    }

    /// 第 i 档强调色（原版界面各处直接读 `accent[i]`）。
    pub fn accent(&self, i: usize) -> Color32 {
        self.accent_table[i.min(8)]
    }

    /// 原版 `draw_accent_init()`：把表重置回持久化基色 accent1/2/3 推出的 9 档
    /// （彩虹关闭、事件乐器改色后复位都走它）。改完顺手同步派生色。
    pub fn accent_init(&mut self) {
        self.set_accent_table(self.accent_base);
    }

    /// 原版 `draw_set_accent(rgb)`：只重算表，不写回基色，也不持久化
    /// （改色乐器 / 彩虹每帧的入口；下次启动仍从 settings.ini 的基色起步）。
    pub fn set_accent_table(&mut self, rgb: [u8; 3]) {
        self.accent_table = Self::build_accent_table(rgb);
        self.sync_accent();
    }

    /// 载入持久化基色（settings.ini 的 accent1/2/3）：先写回 base 再重算表。
    pub fn set_accent_base(&mut self, rgb: [u8; 3]) {
        self.accent_base = rgb;
        self.accent_init();
    }

    /// accent 相关的派生色都从表第 4 档来，重算表后必须跟着刷新。
    fn sync_accent(&mut self) {
        self.accent = self.accent_table[4];
        self.accent_soft = mix(self.window_bg, self.accent, 0.12);
        self.selection = self.accent;
    }

    /// 原版 `accent[6 - 2 * !fdark]`：Fluent 图标染色 / 按下划线用的那一档，
    /// 浅色是 `accent[4]`，`fdark` 打开后升到 `accent[6]`。
    pub fn accent_icon(&self) -> Color32 {
        self.accent_table[if self.fdark { 6 } else { 4 }]
    }

    pub fn new(kind: ThemeKind) -> Self {
        // 三个浅色主题的 window_background 直接抄原版常量。
        let (window_bg, grid_bg) = match kind {
            ThemeKind::Aqua => (gm(0x00F0F0F0), gm(0x00F0F0F0)),
            ThemeKind::Retro => (gm(0x00C8D0D4), gm(0x00C8D0D4)),
            ThemeKind::Dark => (gm(0x00292929), gm(0x00292929)),
            // 原版只对 theme 3 额外铺一层 (249,249,249) 的网格底。
            ThemeKind::Fluent => (gm(0x00F3F3F3), gm(0x00F9F9F9)),
        };
        let dark = kind.is_dark();
        // 原版只有 theme 3 单独铺界面底色，其余主题界面和窗口同色。
        let (row_bg, menu_bg, ruler_bg) = match kind {
            ThemeKind::Fluent => (
                gm(0x00F7F6F1), // 实测 (241,246,247)
                gm(0x00F8F7F2), // 实测 (242,247,248)
                Color32::WHITE,
            ),
            _ => (window_bg, window_bg, window_bg),
        };
        // 装机版 Fluent 界面实测 (21,129,212)，正是原版 `accent[4]`（图标染色、按下划线用的那档）。
        let accent_table = Self::build_accent_table(Self::FACTORY_ACCENT);
        let accent = accent_table[4];
        Self {
            kind,
            fdark: false,
            window_bg,
            row_bg,
            menu_bg,
            ruler_bg,
            // 原版 `draw_block`：theme 2 用 c_white 写音名，其余用 c_yellow。
            note_name: if dark { Color32::WHITE } else { gm(0x0000FFFF) },
            bg: window_bg,
            panel: window_bg,
            grid_bg,
            grid_line: Color32::from_rgba_premultiplied(0, 0, 0, Self::LINE_A),
            grid_beat: Color32::from_rgba_premultiplied(0, 0, 0, Self::BEAT_A),
            grid_bar: Color32::from_rgba_premultiplied(0, 0, 0, Self::BAR_A),
            text: if dark { Color32::WHITE } else { Color32::BLACK },
            text_dim: if dark {
                Color32::from_gray(160)
            } else {
                Color32::from_gray(120)
            },
            accent_base: Self::FACTORY_ACCENT,
            accent_table,
            accent,
            // 以下都是上面几个原版常量的混合，没有新配的颜色：选中/悬停 = 主色或文字
            // 按 12%~24% 压到底色上，未选中按钮 = 底色提亮。
            accent_soft: mix(window_bg, accent, 0.12),
            selection: accent,
            subtract: gm(0x000000FF),
            locked: mix(window_bg, Color32::GRAY, 0.2),
            marker: gm(0x000000FF),
            border: mix(window_bg, Color32::BLACK, 0.24),
            button: mix(window_bg, Color32::WHITE, if dark { 0.0 } else { 0.5 }),
            button_hover: mix(window_bg, if dark { Color32::WHITE } else { Color32::BLACK }, 0.1),
            note_text: Color32::WHITE,
            dark,
        }
    }

    /// 让 egui 原生控件（菜单、按钮、输入框）也跟随主题。
    pub fn apply(&self, ctx: &Context) {
        let mut style = Style::default();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        ctx.set_style(style);

        let mut visuals = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        visuals.window_fill = self.panel;
        visuals.panel_fill = self.panel;
        visuals.extreme_bg_color = self.grid_bg;
        visuals.override_text_color = Some(self.text);
        visuals.widgets.noninteractive.bg_fill = self.panel;
        visuals.widgets.noninteractive.fg_stroke.color = self.text;
        visuals.widgets.inactive.bg_fill = self.button;
        visuals.widgets.inactive.fg_stroke.color = self.text;
        visuals.widgets.hovered.bg_fill = self.button_hover;
        visuals.widgets.active.bg_fill = self.accent_soft;
        visuals.widgets.active.fg_stroke.color = self.text;
        visuals.selection.bg_fill = self.accent_soft;
        visuals.window_stroke.color = self.border;
        ctx.set_visuals(visuals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 装机版 Fluent 界面实测 (21,129,212) = 出厂基色推出的 accent[4]。
    #[test]
    fn factory_table_matches_measured_fluent_accent() {
        let t = Theme::new(ThemeKind::Fluent);
        assert_eq!(t.accent_table[3], Color32::from_rgb(0, 120, 212), "accent[3] = 基色");
        assert_eq!(t.accent_table[4], Color32::from_rgb(21, 129, 212));
        assert_eq!(t.accent_table[0], Color32::from_rgb(0, 84, 148), "0.7 倍截断档");
        assert_eq!(t.accent_table[6], Color32::from_rgb(63, 138, 212), "原稿 0.1*2 的怪癖档");
        assert_eq!(t.accent, t.accent_table[4]);
        assert_eq!(t.selection, t.accent);
    }

    /// 原版 draw_set_accent：改表不改基色；draw_accent_init：表回基色。
    #[test]
    fn accent_table_resets_only_from_base() {
        let mut t = Theme::new(ThemeKind::Fluent);
        t.set_accent_table([255, 0, 0]);
        assert_eq!(t.accent_table[3], Color32::from_rgb(255, 0, 0));
        assert_eq!(t.accent, t.accent_table[4]);
        assert_eq!(t.selection, t.accent);
        assert_eq!(t.accent_soft, mix(t.window_bg, t.accent, 0.12));
        assert_eq!(t.accent_base, [0, 120, 212], "改色乐器不写基色");
        t.accent_init();
        assert_eq!(t.accent_table[3], Color32::from_rgb(0, 120, 212), "accent_init 回基色");
        t.set_accent_base([10, 20, 30]);
        assert_eq!(t.accent_table[3], Color32::from_rgb(10, 20, 30));
        assert_eq!(t.accent_base, [10, 20, 30], "set_accent_base 才写基色");
    }

    /// 原版 `scr_HSBtoRGB` 的整档抽样（避开浮点敏感的中间值）。
    #[test]
    fn hsb_to_rgb_matches_original_samples() {
        assert_eq!(Theme::hsb_to_rgb(0.0, 100.0, 100.0), [255, 0, 0], "hue 0 红");
        assert_eq!(Theme::hsb_to_rgb(60.0, 100.0, 100.0), [255, 255, 0]);
        assert_eq!(Theme::hsb_to_rgb(120.0, 100.0, 100.0), [0, 255, 0]);
        assert_eq!(Theme::hsb_to_rgb(240.0, 100.0, 100.0), [0, 0, 255]);
        assert_eq!(Theme::hsb_to_rgb(359.0, 100.0, 100.0), [255, 0, 4], "i=5 档");
        assert_eq!(
            Theme::hsb_to_rgb(360.0, 100.0, 100.0),
            [255, 0, 0],
            "tempH==6 要折回 0（原版第 15 行）"
        );
        assert_eq!(Theme::hsb_to_rgb(200.0, 0.0, 100.0), [255, 255, 255], "S=0 全灰");
        assert_eq!(Theme::hsb_to_rgb(90.0, 100.0, 50.0), [63, 127, 0], "b 折半，floor(127.5)=127");
    }
}
