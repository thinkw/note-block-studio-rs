//! 原版精灵图（`spr_icons` / `spr_layericons` / `spr_macroicons` / `spr_frame1` / `spr_frame4`
//! / `spr_volume` / `spr_stereo` / `spr_tempobox` / `spr_layerbox` / `spr_iconbar`
//! / `spr_minecraft` / `spr_numbers`）
//! 的加载与绘制，对应原版的 `draw_icon` / `draw_icon_insbox` / `draw_layericon` /
//! `draw_macroicon`。
//!
//! 帧号就是原版 `enum icons` 的下标：精灵的 PNG 按 `.yy` 里 `frames` 的书写顺序
//! 落成 `assets/icons/<集合>/<序号>.png`，所以这里可以直接按序号取。
//!
//! 资源缺失时整个精灵库为 `None`，调用方退回文字按钮 —— 不画假图标。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{pos2, Color32, ColorImage, Context, Id, Pos2, Rect, Sense, TextureHandle, Vec2};

use crate::theme::{Theme, ThemeKind};

/// 原版 `enum icons` 的帧号。
pub mod frame {
    pub const NEW: usize = 0;
    pub const OPEN: usize = 1;
    pub const SAVE: usize = 2;
    pub const SAVE_AS: usize = 3;
    pub const PLAY: usize = 4;
    /// 播放中显示的暂停图标（原版 `icons.PLAY + playing`）。
    pub const PAUSE: usize = 5;
    pub const STOP: usize = 6;
    pub const BACK: usize = 7;
    pub const FORWARD: usize = 8;
    /// 原版这一帧既是「键盘录音」按钮，也是所有拖条的圆形手柄
    /// （`draw_dragbar` 里 `draw_sprite(spr_icons, 9, …)`）。我们没有录音功能，
    /// 所以只当手柄用。
    pub const DRAG_KNOB: usize = 9;
    pub const LOOP_INACTIVE: usize = 49;
    pub const LOOP: usize = 50;
    pub const METRONOME_INACTIVE: usize = 51;
    pub const METRONOME_1: usize = 52;
    pub const METRONOME_2: usize = 53;
    pub const EDITMODE_KEY: usize = 54;
    pub const EDITMODE_VEL: usize = 55;
    pub const EDITMODE_PAN: usize = 56;
    pub const EDITMODE_PIT: usize = 57;
    pub const INSBOX_EXPAND: usize = 58;
    pub const INSBOX_COLLAPSE: usize = 59;
    /// 自定义音色的角标底。
    pub const CUSTOM_INS_OVERLAY: usize = 27;
    /// 原版给「没有专属图标的乐器」用的通用音色图标（`icons.INS_1 + first_custom_index`）。
    pub const CUSTOM_INS_BASE: usize = 26;
    pub const INS_1: usize = 10;
    /// 本仓库贴图表里内置乐器图标的个数：`spr_icons` 帧 10..25。装机版 3.12 的表已经有
    /// 20 帧（10..29 是铜块一族），我们没有那 4 帧，所以只能退回通用图标。
    pub const BUILTIN_ICON_FRAMES: usize = 16;
    // 原版 28/30/32/34/36/38 是同一批图标的灰态；`icon(..., locked = true)` 会减 1 取到它们。
    pub const UNDO: usize = 29;
    pub const REDO: usize = 31;
    pub const COPY: usize = 33;
    pub const CUT: usize = 35;
    pub const PASTE: usize = 37;
    pub const DELETE: usize = 39;
    pub const INFORMATION: usize = 40;
    pub const PROPERTIES: usize = 41;
    pub const INSTRUMENTS: usize = 42;
    pub const HELP: usize = 44;
    pub const INTERNET: usize = 45;
}

/// 原版 `spr_layericons` 的帧号（7 个图层按钮 + 全屏两态）。
pub mod layer {
    pub const LOCK: usize = 0;
    pub const SOLO: usize = 1;
    pub const SELECT: usize = 2;
    pub const ADD: usize = 3;
    pub const REMOVE: usize = 4;
    pub const UP: usize = 5;
    pub const DOWN: usize = 6;
}

/// 原版宏条的 15 个图标，间距 25、图标 16×16。
pub mod macro_icon {
    pub const TREMOLO: usize = 0;
    pub const STEREO: usize = 1;
    pub const ARPEGGIO: usize = 2;
    pub const PORTAMENTO: usize = 3;
    pub const VIBRATO: usize = 4;
    pub const STAGGER: usize = 5;
    pub const CHORUS: usize = 6;
    pub const VELOCITY_LFO: usize = 7;
    pub const FADE_IN: usize = 8;
    pub const FADE_OUT: usize = 9;
    pub const REPLACE_KEY: usize = 10;
    pub const SET_VELOCITY: usize = 11;
    pub const SET_PANNING: usize = 12;
    pub const SET_PITCH: usize = 13;
    pub const RESET: usize = 14;
}

/// 一行摆完所有帧的小图集。
#[derive(Clone)]
struct Strip {
    tex: TextureHandle,
    fw: f32,
    fh: f32,
    count: usize,
}

impl Strip {
    fn load(ctx: &Context, dir: &Path, fw: u32, fh: u32) -> Option<Strip> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
            .collect();
        paths.sort();
        if paths.is_empty() {
            return None;
        }
        let mut img = ColorImage::new([paths.len() * fw as usize, fh as usize], Color32::TRANSPARENT);
        let row = img.width();
        for (i, path) in paths.iter().enumerate() {
            let Ok(decoded) = image::open(path) else { continue };
            let rgba = decoded.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            if w > fw || h > fh {
                eprintln!("nbs-editor: 图标 {path:?} 尺寸 {w}x{h} 超出 {fw}x{fh}，已跳过");
                continue;
            }
            let x0 = i * fw as usize;
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let p = rgba.get_pixel(x as u32, y as u32).0;
                    img.pixels[y * row + x0 + x] = Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]);
                }
            }
        }
        let name = dir.file_name()?.to_string_lossy().into_owned();
        Some(Strip {
            tex: ctx.load_texture(name, img, Default::default()),
            fw: fw as f32,
            fh: fh as f32,
            count: paths.len(),
        })
    }

    fn blit(&self, p: &egui::Painter, at: Pos2, index: usize, size: Vec2, tint: Color32) {
        let index = index.min(self.count.saturating_sub(1));
        let total = self.count as f32 * self.fw;
        let u0 = index as f32 * self.fw / total;
        let u1 = u0 + self.fw / total;
        p.image(
            self.tex.id(),
            Rect::from_min_size(at, size),
            Rect::from_min_max(egui::pos2(u0, 0.0), egui::pos2(u1, 1.0)),
            tint,
        );
    }

    /// 原生尺寸。
    fn draw(&self, p: &egui::Painter, at: Pos2, index: usize, tint: Color32) {
        self.blit(p, at, index, Vec2::new(self.fw, self.fh), tint);
    }
}

/// 一次图标点击的结果。原版 `draw_icon` 对快进 / 快退返回「按住」，
/// 其余返回「抬手」，所以两个都要暴露给调用方。
#[derive(Clone, Copy, Default)]
pub struct IconHit {
    pub clicked: bool,
    /// 悬停且按住（原版 `a = 2`）。
    pub held: bool,
}

/// 已加载的精灵集合。
#[derive(Clone)]
pub struct Bank {
    icons: Strip,
    icons_light: Strip,
    icons_accent: Strip,
    layer: Strip,
    layer_fluent: Strip,
    macro_ikon: Strip,
    frame1: Strip,
    frame4: Strip,
    numbers: Strip,
    minecraft: Strip,
    minecraft_fluent: Strip,
    volume: Strip,
    volume_fluent: Strip,
    stereo: Strip,
    stereo_fluent: Strip,
    tempobox: Strip,
    layerbox: Strip,
    iconbar: Strip,
}

/// 三态：0 常态、1 悬停、2 悬停且按住。
fn pointer_state(hovered: bool, held: bool) -> usize {
    match (hovered, held) {
        (true, true) => 2,
        (true, false) => 1,
        _ => 0,
    }
}

/// 原版 `spr_frame4` / `draw_layericon` / `draw_macroicon` 的按钮边长。
const LAYER_ICON: f32 = 16.0;
/// 原版 `spr_volume` / `spr_stereo` 的字形尺寸。
const GLYPH_W: f32 = 12.0;
const GLYPH_H: f32 = 10.0;

fn theme_index(kind: ThemeKind) -> usize {
    match kind {
        ThemeKind::Aqua => 0,
        ThemeKind::Retro => 1,
        ThemeKind::Dark => 2,
        ThemeKind::Fluent => 3,
    }
}

/// 原版 `make_color_hsv(h, s, v)`，三个分量都是 0..=255。
fn hsv(h: u8, s: u8, v: u8) -> Color32 {
    let hf = h as f32 / 255.0 * 6.0;
    let sf = s as f32 / 255.0;
    let vf = v as f32 / 255.0;
    let i = hf.floor() as i32;
    let f = hf - hf.floor();
    let p = vf * (1.0 - sf);
    let q = vf * (1.0 - sf * f);
    let t = vf * (1.0 - sf * (1.0 - f));
    let (r, g, b) = match i % 6 {
        0 => (vf, t, p),
        1 => (q, vf, p),
        2 => (p, vf, t),
        3 => (p, q, vf),
        4 => (t, p, vf),
        _ => (vf, p, q),
    };
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// 原版 `draw_sprite_ext(..., iconcolor, alpha)`：字形按主题染色（暗黑用白，其余用黑），
/// 再乘上精灵自身的透明度。
fn glyph_tint(th: &Theme, alpha: f32) -> Color32 {
    let base = if th.dark { Color32::from_gray(230) } else { Color32::BLACK };
    let (r, g, b, a) = base.to_tuple();
    Color32::from_rgba_unmultiplied(r, g, b, (a as f32 * alpha) as u8)
}

/// 乐器格子用哪一帧图标：原版 `icons.INS_1 + min(i, first_custom_index)`，
/// 即「内置乐器按序号、自定义乐器一律用通用图标」。我们的贴图表只有 16 帧内置图标
/// （装机版 3.12 有 20 帧，多出的 4 帧是铜块一族），所以再夹一次到 `BUILTIN_ICON_FRAMES`，
/// 让 16..19 和自定义乐器一样用通用图标，而不是画到无关的 UI 帧上。
fn instrument_frame(list_index: usize, first_custom: usize) -> usize {
    frame::INS_1 + list_index.min(first_custom).min(frame::BUILTIN_ICON_FRAMES)
}

impl Bank {
    fn load(ctx: &Context, root: &Path) -> Option<Bank> {
        let strip = |dir: &str, w: u32, h: u32| Strip::load(ctx, &root.join(dir), w, h);
        Some(Bank {
            icons: strip("icons_classic", 25, 25)?,
            icons_light: strip("icons_light", 25, 25)?,
            icons_accent: strip("icons_accent", 25, 25)?,
            layer: strip("layer_classic", 16, 16)?,
            layer_fluent: strip("layer_fluent", 16, 16)?,
            macro_ikon: strip("macro", 16, 16)?,
            frame1: strip("frame1", 25, 23)?,
            frame4: strip("frame4", 16, 16)?,
            numbers: strip("numbers", 3, 5)?,
            minecraft: strip("minecraft", 20, 20)?,
            minecraft_fluent: strip("minecraft_fluent", 20, 20)?,
            volume: strip("volume", 12, 10)?,
            volume_fluent: strip("volume_fluent", 12, 10)?,
            stereo: strip("stereo", 12, 10)?,
            stereo_fluent: strip("stereo_fluent", 12, 10)?,
            tempobox: strip("tempobox", 74, 22)?,
            layerbox: strip("layerbox", 256, 34)?,
            iconbar: strip("iconbar", 2, 30)?,
        })
    }

    /// 原版 `spr_frame1` / `spr_frame4` 每套主题 3 帧（常态 / 悬停 / 按下）。
    fn base(kind: ThemeKind) -> usize {
        3 * theme_index(kind)
    }

    fn ptr(ui: &egui::Ui, rect: Rect, id: Id) -> (egui::Response, bool) {
        let resp = ui.interact(rect, id, Sense::click());
        let held = resp.hovered() && ui.input(|i| i.pointer.any_down());
        (resp, held)
    }

    /// 原版 `draw_icon(i, x, y, str, locked, pressed)`。
    pub fn icon(&self, ui: &mut egui::Ui, at: Pos2, index: usize, tooltip: &str, locked: bool, pressed: bool, th: &Theme) -> IconHit {
        let rect = Rect::from_min_size(at, Vec2::new(25.0, 25.0));
        let (resp, holding) = Self::ptr(ui, rect, Id::new(("icon", index, tooltip)));
        let state = pointer_state(resp.hovered(), holding);
        let p = ui.painter();
        self.frame1.blit(p, at, Self::base(th.kind) + if pressed { 2 } else { state }, Vec2::new(25.0, 25.0), Color32::WHITE);
        let idx = index.saturating_sub(locked as usize);
        let fluent = th.kind == ThemeKind::Fluent;
        if fluent {
            // Fluent：悬停按下时字形原地缩小 20%，并叠一层强调色蒙版。
            let scale = if state == 2 { 0.8 } else { 1.0 };
            let off = Vec2::splat(2.5 * (1.0 - scale));
            let size = Vec2::splat(25.0 * scale);
            self.icons_light.blit(p, at + off, idx, size, Color32::WHITE);
            self.icons_accent.blit(p, at + off, idx, size, th.accent);
        } else {
            let push = if state == 2 || pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
            self.icons.draw(p, at + push, idx, Color32::WHITE);
        }
        if pressed && fluent {
            // 原版按下时用 spr_tabsel 画一条强调色下划线。
            p.rect_filled(Rect::from_min_size(at + Vec2::new(5.0, 21.0), Vec2::new(16.0, 3.0)), 1.5, th.accent);
        }
        let resp = resp.on_hover_text(tooltip);
        IconHit { clicked: resp.clicked(), held: state == 2 }
    }

    /// 原版 `draw_icon_insbox()`：音色框里的按钮。自定义音色在原版里是
    /// 「内置图标（截到第 16 个）+ 角标 + 两位序号」。
    pub fn instrument_icon(
        &self,
        ui: &mut egui::Ui,
        at: Pos2,
        list_index: usize,
        first_custom: u8,
        tooltip: &str,
        pressed: bool,
        th: &Theme,
    ) -> bool {
        let rect = Rect::from_min_size(at, Vec2::new(25.0, 25.0));
        let (resp, holding) = Self::ptr(ui, rect, Id::new(("insicon", list_index, tooltip)));
        let state = pointer_state(resp.hovered(), holding);
        let p = ui.painter();
        self.frame1.blit(p, at, Self::base(th.kind) + if pressed { 2 } else { state }, Vec2::new(25.0, 25.0), Color32::WHITE);
        // 原版 `sprite = icons.INS_1 + min(i, first_custom_index)`：没有专属图标的乐器
        // 一律退回通用音色图标。本仓库的贴图表只画到 16 个内置乐器，铜块一族（16..19）
        // 没有帧，所以同样退回通用图标，而不是拿 UNDO/COPY 那类无关的 UI 帧凑数。
        let sprite = instrument_frame(list_index, first_custom as usize);
        let fluent = th.kind == ThemeKind::Fluent;
        if fluent {
            let scale = if state == 2 { 0.8 } else { 1.0 };
            let off = Vec2::splat(2.5 * (1.0 - scale));
            let size = Vec2::splat(25.0 * scale);
            self.icons_light.blit(p, at + off, sprite, size, Color32::WHITE);
            self.icons_accent.blit(p, at + off, sprite, size, th.accent);
        } else {
            let push = if state == 2 || pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
            self.icons.draw(p, at + push, sprite, Color32::WHITE);
            if list_index >= first_custom as usize {
                self.custom_badge(p, at + push, list_index - first_custom as usize);
            }
        }
        if pressed && fluent {
            p.rect_filled(Rect::from_min_size(at + Vec2::new(5.0, 21.0), Vec2::new(16.0, 3.0)), 1.5, th.accent);
        }
        resp.on_hover_text(tooltip).clicked()
    }

    /// 动作类图标（展开 / 收起音色框），用 `icons` 里的高帧号。
    pub fn action_icon(&self, ui: &mut egui::Ui, at: Pos2, index: usize, tooltip: &str, pressed: bool, th: &Theme) -> bool {
        let rect = Rect::from_min_size(at, Vec2::new(25.0, 25.0));
        let (resp, holding) = Self::ptr(ui, rect, Id::new(("actionicon", index, tooltip)));
        let state = pointer_state(resp.hovered(), holding);
        let p = ui.painter();
        self.frame1.blit(p, at, Self::base(th.kind) + if pressed { 2 } else { state }, Vec2::new(25.0, 25.0), Color32::WHITE);
        let fluent = th.kind == ThemeKind::Fluent;
        if fluent {
            self.icons_light.blit(p, at, index, Vec2::splat(25.0), Color32::WHITE);
            self.icons_accent.blit(p, at, index, Vec2::splat(25.0), th.accent);
        } else {
            let push = if state == 2 || pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
            self.icons.draw(p, at + push, index, Color32::WHITE);
        }
        if pressed && fluent {
            p.rect_filled(Rect::from_min_size(at + Vec2::new(5.0, 21.0), Vec2::new(16.0, 3.0)), 1.5, th.accent);
        }
        resp.on_hover_text(tooltip).clicked()
    }

    /// 自定义音色角标：`spr_icons[27]` 染色 + 两位数字。
    fn custom_badge(&self, p: &egui::Painter, at: Pos2, ins: usize) {
        let digits = (ins + 1) % 100;
        let (num1, num2) = (digits / 10, digits % 10);
        let c1 = hsv(((ins * 16) % 256) as u8, 192, 255);
        let c2 = hsv(((ins * 16) % 256) as u8, 255, 128);
        self.icons.draw(p, at, frame::CUSTOM_INS_OVERLAY, c1);
        for (d, dx) in [(num1, 11.0), (num2, 15.0)] {
            self.numbers.draw(p, at + Vec2::new(dx + 1.0, 2.0), d, c2);
            self.numbers.draw(p, at + Vec2::new(dx, 1.0), d, Color32::WHITE);
        }
    }

    /// 原版 `draw_layericon()`：图层行里的 16×16 按钮。
    /// `row` 用来给每行的同一个按钮生成不同的 widget id。
    /// 返回值对应原版的 `mouse_entered || (a && mouse_check_button_pressed)`：
    /// 按下那一刻、或按住左键从别的行划进来都算一次触发（原版靠这个跨行批量操作）。
    pub fn layer_button(
        &self,
        ui: &mut egui::Ui,
        at: Pos2,
        index: usize,
        row: u32,
        tooltip: &str,
        pressed: bool,
        th: &Theme,
    ) -> bool {
        let rect = Rect::from_min_size(at, Vec2::splat(LAYER_ICON));
        let id = Id::new(("layericon", index, row));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let hovered = resp.hovered();
        let down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
        let press_now = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
        // 划进来：上一帧划过的是别的按钮，这一帧在本按钮上且按着左键。
        let prev = ui.ctx().data(|d| d.get_temp::<Id>(Id::new("layericon_prev_hover")));
        let entered = hovered && down && prev != Some(id);
        if hovered {
            ui.ctx()
                .data_mut(|d| d.insert_temp(Id::new("layericon_prev_hover"), id));
        }
        let state = pointer_state(hovered, hovered && down);
        let p = ui.painter();
        self.frame4.blit(p, at, Self::base(th.kind) + if pressed { 2 } else { state }, Vec2::splat(LAYER_ICON), Color32::WHITE);
        let push = if state == 2 || pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
        let at = at + push;
        if th.kind == ThemeKind::Fluent {
            self.layer_fluent.blit(p, at, index + 9, Vec2::splat(LAYER_ICON), Color32::WHITE);
            self.layer_fluent.blit(p, at, index + 18, Vec2::splat(LAYER_ICON), th.accent);
        } else {
            // 原版：非 Fluent 用 c_black，暗黑主题用 c_white，透明度 0.7。
            self.layer.blit(p, at, index, Vec2::splat(LAYER_ICON), glyph_tint(th, 0.7));
        }
        resp.on_hover_text(tooltip);
        entered || (hovered && press_now)
    }

    /// 原版 `draw_macroicon()`：宏条里的 16×16 按钮（间距 25）。
    pub fn macro_button(&self, ui: &mut egui::Ui, at: Pos2, index: usize, tooltip: &str, th: &Theme) -> bool {
        let rect = Rect::from_min_size(at, Vec2::splat(LAYER_ICON));
        let (resp, holding) = Self::ptr(ui, rect, Id::new(("macroicon", index, tooltip)));
        let state = pointer_state(resp.hovered(), holding);
        let p = ui.painter();
        self.frame4.blit(p, at, Self::base(th.kind) + state, Vec2::splat(LAYER_ICON), Color32::WHITE);
        let push = if state == 2 { Vec2::splat(1.0) } else { Vec2::ZERO };
        self.macro_ikon.blit(p, at + push, index, Vec2::splat(LAYER_ICON), glyph_tint(th, 0.7));
        resp.on_hover_text(tooltip).clicked()
    }

    /// 原版 `spr_volume[a / 30]`（12×10），悬停时字形上移 5px。
    pub fn volume_glyph(&self, p: &egui::Painter, at: Pos2, value: u8, hovered: bool, th: &Theme) {
        let at = at - Vec2::new(0.0, if hovered { 5.0 } else { 0.0 });
        let size = Vec2::new(GLYPH_W, GLYPH_H);
        if th.kind == ThemeKind::Fluent {
            // 原版 Fluent 用 `!fdark` 那一帧当底，再叠一格强调色进度。
            self.volume_fluent.blit(p, at, if th.dark { 0 } else { 1 }, size, Color32::WHITE);
            self.volume_fluent.blit(p, at, (value / 30) as usize + 2, size, th.accent);
        } else {
            self.volume.blit(p, at, (value / 30) as usize, size, glyph_tint(th, 0.7));
        }
    }

    /// 原版 `spr_stereo[a / 50]`（12×10），悬停时字形上移 5px。
    pub fn stereo_glyph(&self, p: &egui::Painter, at: Pos2, value: u8, hovered: bool, th: &Theme) {
        let at = at - Vec2::new(0.0, if hovered { 5.0 } else { 0.0 });
        let size = Vec2::new(GLYPH_W, GLYPH_H);
        if th.kind == ThemeKind::Fluent {
            self.stereo_fluent.blit(p, at, if th.dark { 0 } else { 1 }, size, Color32::WHITE);
            self.stereo_fluent.blit(p, at, (value / 50) as usize + 2, size, th.accent);
        } else {
            self.stereo.blit(p, at, (value / 50) as usize, size, glyph_tint(th, 0.7));
        }
    }

    /// 原版 `spr_tempobox`：0/1 = 经典 t/s / BPM，2/3 = Fluent t/s / BPM，4/5 = Fluent 暗黑。
    /// 框体固定 74×22，不跟网格缩放。
    pub fn tempo_box(&self, p: &egui::Painter, at: Pos2, bpm: bool, th: &Theme) {
        let base = match th.kind {
            ThemeKind::Fluent => 2 + if th.dark { 2 } else { 0 },
            _ => 0,
        };
        self.tempobox.draw(p, at, base + bpm as usize, Color32::WHITE);
    }

    /// 原版 `spr_layerbox` 的行底：0 经典浅色、1 经典暗黑、2/3 Fluent 浅/深。
    /// 原始图 256×34，比行高多出的 2px 是原版自带的下边压痕。
    pub fn layer_row(&self, p: &egui::Painter, at: Pos2, size: Vec2, th: &Theme) {
        let index = match th.kind {
            ThemeKind::Dark => 1,
            ThemeKind::Fluent => 2 + th.dark as usize,
            _ => 0,
        };
        self.layerbox.blit(p, at, index, size, Color32::WHITE);
    }

    /// 原版 `spr_iconbar`：工具栏底的左右端帽 + 横向拉伸的中间条
    /// （`draw_sprite(0, 0, 20)` / `draw_sprite_ext(1, 2, 20, rw - 4, 1)` / `draw_sprite(2, rw - 2, 20)`）。
    /// Fluent 主题原版不画这条，调用方自己判断。
    pub fn icon_strip_bg(&self, p: &egui::Painter, rect: Rect) {
        let h = rect.height();
        self.iconbar.blit(p, rect.min, 0, Vec2::new(2.0, h), Color32::WHITE);
        self.iconbar.blit(
            p,
            pos2(rect.min.x + 2.0, rect.min.y),
            1,
            Vec2::new((rect.width() - 4.0).max(0.0), h),
            Color32::WHITE,
        );
        self.iconbar.blit(
            p,
            pos2(rect.max.x - 2.0, rect.min.y),
            2,
            Vec2::new(2.0, h),
            Color32::WHITE,
        );
    }

    /// 原版右上角的兼容性图标：0 = 通过，1 = 仅数据包，2 = 不通过。
    pub fn compat_icon(&self, p: &egui::Painter, at: Pos2, state: usize, th: &Theme) {
        if th.kind == ThemeKind::Fluent {
            self.minecraft_fluent.draw(p, at, state, Color32::WHITE);
            self.minecraft_fluent.draw(p, at, state + 6, th.accent);
        } else {
            self.minecraft.draw(p, at, state, Color32::WHITE);
        }
    }

    /// 原版 `draw_separator(xx, yy + 3)`：1px 竖线，高 18，灰色。
    /// （原版只在 Fluent 深色模式下换成白线，我们没有那个开关。）
    pub fn separator(p: &egui::Painter, at: Pos2, _th: &Theme) {
        p.rect_filled(Rect::from_min_size(at, Vec2::new(1.0, 18.0)), 0.0, Color32::from_gray(128));
    }

    /// 只画字形、不画按钮框：原版拖条手柄就是 `draw_sprite(spr_icons, 9, …)`。
    pub fn icon_glyph(&self, p: &egui::Painter, at: Pos2, index: usize) {
        self.icons.draw(p, at, index, Color32::WHITE);
    }
}

/// 挂在 `EditorState` 上的懒加载精灵缓存。
#[derive(Clone, Default)]
pub struct Icons {
    bank: Option<Arc<Bank>>,
    /// 已经找过一遍但资源缺失：不再每帧重新读盘。
    missing: bool,
}

impl Icons {
    pub fn ensure(&mut self, ctx: &Context) {
        if self.bank.is_some() || self.missing {
            return;
        }
        let Some(root) = asset_root() else {
            self.missing = true;
            return;
        };
        match Bank::load(ctx, &root) {
            Some(bank) => self.bank = Some(Arc::new(bank)),
            None => {
                self.missing = true;
                eprintln!("nbs-editor: 图标资源 {root:?} 不完整，工具栏退回文字按钮");
            }
        }
    }

    pub fn bank(&self) -> Option<Arc<Bank>> {
        self.bank.clone()
    }

    /// 资源确实找不到（而不是还没加载）时为真，调用方据此退回文字按钮。
    pub fn unavailable(&self) -> bool {
        self.bank.is_none() && self.missing
    }
}

/// 与 `nbs-audio` 找采样同样的搜索顺序：可执行文件旁 → 工作目录 → cargo 清单目录。
fn asset_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for up in ["", "..", "../..", "../../.."] {
                candidates.push(dir.join(up).join("assets/icons"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for up in ["", "..", "../.."] {
            candidates.push(cwd.join(up).join("assets/icons"));
        }
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("../../assets/icons"));
    candidates.into_iter().find(|p| p.join("icons_classic").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图标帧只能指到「16 帧内置乐器图标 + 通用图标」，不能滑到 26..29 那些无关的 UI 帧。
    #[test]
    fn instrument_frames_never_point_at_unrelated_icons() {
        for i in 0..16 {
            assert_eq!(instrument_frame(i, 20), frame::INS_1 + i, "内置乐器 {i}");
        }
        // 铜块一族和自定义乐器一样：通用图标帧，不借用 UNDO/COPY 的帧。
        for i in 16..40 {
            assert_eq!(instrument_frame(i, 20), frame::INS_1 + frame::BUILTIN_ICON_FRAMES, "索引 {i}");
        }
        assert_eq!(
            instrument_frame(16, 16),
            frame::INS_1 + 16,
            "旧格式（first_custom = 16）仍然是通用图标"
        );
        // 用到的帧必须真的在贴图表里（00..59）。
        for i in 0..=255u8 {
            let f = instrument_frame(i as usize, 20);
            assert!(f <= frame::INSBOX_COLLAPSE, "索引 {i} 取到不存在的帧 {f}");
        }
    }
}
