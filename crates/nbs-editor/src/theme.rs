//! 四套主题，对齐原版 `theme = 0..3`（经典 Aqua / 90s / Dark / Fluent）。

use egui::{Color32, Context, Style, Visuals};

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
    pub fn name(self) -> &'static str {
        match self {
            Self::Aqua => "经典 Aqua",
            Self::Retro => "90 年代",
            Self::Dark => "暗黑",
            Self::Fluent => "Fluent",
        }
    }
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub kind: ThemeKind,
    pub bg: Color32,
    pub panel: Color32,
    pub grid_bg: Color32,
    pub grid_line: Color32,
    pub grid_beat: Color32,
    pub grid_bar: Color32,
    pub text: Color32,
    pub text_dim: Color32,
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
    pub fn new(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Aqua => Self {
                kind,
                bg: Color32::from_rgb(236, 244, 250),
                panel: Color32::from_rgb(214, 230, 244),
                grid_bg: Color32::from_rgb(249, 252, 255),
                grid_line: Color32::from_rgb(214, 226, 238),
                grid_beat: Color32::from_rgb(178, 200, 222),
                grid_bar: Color32::from_rgb(120, 160, 205),
                text: Color32::from_rgb(24, 44, 66),
                text_dim: Color32::from_rgb(96, 122, 150),
                accent: Color32::from_rgb(40, 116, 190),
                accent_soft: Color32::from_rgb(180, 214, 244),
                selection: Color32::from_rgb(60, 130, 220),
                subtract: Color32::from_rgb(214, 70, 70),
                locked: Color32::from_rgb(214, 218, 224),
                marker: Color32::from_rgb(220, 70, 60),
                border: Color32::from_rgb(160, 186, 210),
                button: Color32::from_rgb(226, 238, 250),
                button_hover: Color32::from_rgb(196, 222, 246),
                note_text: Color32::from_rgb(255, 255, 255),
                dark: false,
            },
            ThemeKind::Retro => Self {
                kind,
                bg: Color32::from_rgb(214, 214, 206),
                panel: Color32::from_rgb(196, 196, 188),
                grid_bg: Color32::from_rgb(238, 238, 228),
                grid_line: Color32::from_rgb(198, 198, 188),
                grid_beat: Color32::from_rgb(160, 160, 150),
                grid_bar: Color32::from_rgb(80, 80, 120),
                text: Color32::from_rgb(20, 20, 20),
                text_dim: Color32::from_rgb(90, 90, 90),
                accent: Color32::from_rgb(0, 0, 140),
                accent_soft: Color32::from_rgb(170, 170, 210),
                selection: Color32::from_rgb(0, 60, 180),
                subtract: Color32::from_rgb(170, 20, 20),
                locked: Color32::from_rgb(190, 190, 182),
                marker: Color32::from_rgb(190, 20, 20),
                border: Color32::from_rgb(120, 120, 120),
                button: Color32::from_rgb(224, 224, 214),
                button_hover: Color32::from_rgb(200, 200, 220),
                note_text: Color32::from_rgb(255, 255, 255),
                dark: false,
            },
            ThemeKind::Dark => Self {
                kind,
                bg: Color32::from_rgb(30, 33, 40),
                panel: Color32::from_rgb(40, 44, 52),
                grid_bg: Color32::from_rgb(25, 27, 33),
                grid_line: Color32::from_rgb(50, 55, 64),
                grid_beat: Color32::from_rgb(72, 80, 92),
                grid_bar: Color32::from_rgb(110, 130, 160),
                text: Color32::from_rgb(226, 232, 240),
                text_dim: Color32::from_rgb(140, 150, 166),
                accent: Color32::from_rgb(96, 170, 250),
                accent_soft: Color32::from_rgb(48, 76, 112),
                selection: Color32::from_rgb(100, 170, 255),
                subtract: Color32::from_rgb(230, 96, 96),
                locked: Color32::from_rgb(44, 47, 54),
                marker: Color32::from_rgb(250, 110, 96),
                border: Color32::from_rgb(72, 80, 92),
                button: Color32::from_rgb(52, 58, 68),
                button_hover: Color32::from_rgb(70, 80, 94),
                note_text: Color32::from_rgb(255, 255, 255),
                dark: true,
            },
            ThemeKind::Fluent => Self {
                kind,
                bg: Color32::from_rgb(243, 243, 245),
                panel: Color32::from_rgb(249, 249, 251),
                grid_bg: Color32::from_rgb(250, 250, 252),
                grid_line: Color32::from_rgb(226, 226, 230),
                grid_beat: Color32::from_rgb(196, 196, 204),
                grid_bar: Color32::from_rgb(0, 120, 212),
                text: Color32::from_rgb(26, 26, 30),
                text_dim: Color32::from_rgb(110, 110, 122),
                accent: Color32::from_rgb(0, 120, 212),
                accent_soft: Color32::from_rgb(214, 232, 250),
                selection: Color32::from_rgb(0, 120, 212),
                subtract: Color32::from_rgb(196, 60, 60),
                locked: Color32::from_rgb(232, 232, 236),
                marker: Color32::from_rgb(232, 72, 64),
                border: Color32::from_rgb(206, 206, 214),
                button: Color32::from_rgb(252, 252, 254),
                button_hover: Color32::from_rgb(228, 240, 252),
                note_text: Color32::from_rgb(255, 255, 255),
                dark: false,
            },
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
