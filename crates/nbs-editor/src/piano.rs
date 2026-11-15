//! 底部钢琴键盘 —— 照原版 `draw_piano` / `draw_piano_key` 逐帧贴 `spr_piano`：
//! 白键 0、黑键背面 2 / 正面 1、边缘淡出蒙版 3/4/5；每列先画白键再画黑键（黑键压在上面）。
//! 白键固定 39px 步进、整排按窗口宽度居中，鼠标停在最左 / 最右白键上每 0.1 秒平移一个音
//! （原版 `d` + `delay`）。`spr_piano` 资源缺失时整排不画 —— 原版没有「无贴图键盘」，
//! 所以这里也不画一个假的出来。

use egui::{pos2, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Ui, Vec2};
use nbs_core::model::{key_name, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::commands::Command;
use crate::icons::{piano_frame, Bank};
use crate::state::EditorState;
use crate::theme::{Theme, ThemeKind};

/// 原版白键步进 `xx + 39 * a`。
const KEY_W: f32 = 39.0;
/// `spr_piano` 一帧 40×121。
const ART_H: f32 = 121.0;
/// 白键命中区 `mouse_rectangle(xx + 39a, yy, 39, 128)`。
const WHITE_HIT_H: f32 = 128.0;
/// 黑键画在白键左边 19px、上边 7px 处。
const BLACK_DRAW_X: f32 = -19.0;
const BLACK_LIFT: f32 = 7.0;
/// 黑键命中区 `mouse_rectangle(xx + 39a - 12, yy - 7, 25, 71)`：
/// 帧里不透明的是第 7..31 列，画在 `-19` 处正好落在白键左缘的 `-12..+13`。
const BLACK_HIT_X: f32 = -12.0;
const BLACK_HIT_W: f32 = 25.0;
const BLACK_HIT_H: f32 = 71.0;
/// 右边相邻黑键的命中区左缘（`xx + 39a + 27`，挡住本白键的右半边）。
const RIGHT_BLACK_HIT_X: f32 = 27.0;
/// 原版 `floor(7 * 0.75)`：按住时的下沉像素。
const PRESS_DOWN: f32 = 5.0;
/// 原版右移上限 `startkey + k < 52`。
const RIGHT_LIMIT: usize = 52;
/// 原版尾部淡出阈值 `min(5, 54 - (startkey + k))`。
const FADE_LIMIT: usize = 54;
/// 原版 8224255 = RGB(255,126,125)，超出 MC 音域的键。
const OUT_OF_RANGE: Color32 = Color32::from_rgb(255, 126, 125);
/// 原版 16753828 = RGB(164,164,255)，非 Fluent 主题下当前选中的键。
const SELECTED: Color32 = Color32::from_rgb(164, 164, 255);
/// 原版 `font_main` 在琴键上的字号（`fnt_main` 的 size = 9）。
const TEXT: FontId = FontId::proportional(9.0);
/// 原版 `draw_text` 在这里的有效对齐：`draw_text_edit` 把全局 valign 留在了 `fa_top`，
/// 琴键 / 方块上的文字都是「顶边贴 y」，量过的像素也是这个（键名顶边 = yy + 85）。
const TEXT_AT: Align2 = Align2::CENTER_TOP;

/// 原版 `init_keys()`：键号 33..=57 的默认按键绑定，也就是琴键上印的那个字符。
pub const KEYBINDS: [char; 25] = [
    '1', 'Q', '2', 'W', '3', 'E', 'R', '5', 'T', '6', 'Y', 'U', '8', 'I', '9', 'O', '0', 'P', 'Z',
    'S', 'X', 'D', 'C', 'V', 'G',
];

/// 原版 `key_press[0..=87]`：键号 → 该键的默认按键此刻是否被按住（琴键要画成按下态）。
pub type KeysHeld = [bool; KEY_MAX as usize + 1];
/// 一个键都没按住。
const NO_KEYS: KeysHeld = [false; KEY_MAX as usize + 1];
pub fn keybind(key: u8) -> Option<char> {    if key < MC_KEY_MIN || key > MC_KEY_MAX {
        return None;
    }
    KEYBINDS.get((key - MC_KEY_MIN) as usize).copied()
}

/// 按键字符 → egui 键位（原版 `keyboard_check(piano_key[a])`）。
pub fn keybind_key(c: char) -> Option<egui::Key> {
    use egui::Key;
    Some(match c {
        '1' => Key::Num1,
        '2' => Key::Num2,
        '3' => Key::Num3,
        '5' => Key::Num5,
        '6' => Key::Num6,
        '8' => Key::Num8,
        '9' => Key::Num9,
        '0' => Key::Num0,
        'Q' => Key::Q,
        'W' => Key::W,
        'E' => Key::E,
        'R' => Key::R,
        'T' => Key::T,
        'Y' => Key::Y,
        'U' => Key::U,
        'I' => Key::I,
        'O' => Key::O,
        'P' => Key::P,
        'Z' => Key::Z,
        'S' => Key::S,
        'X' => Key::X,
        'D' => Key::D,
        'C' => Key::C,
        'V' => Key::V,
        'G' => Key::G,
        _ => return None,
    })
}

/// 原版 `draw_piano_key` 的帧号换算：`sprite_offset = 6 * (theme == 3)`，
/// Fluent 深色再 `+6`；淡出蒙版只加 `sprite_offset`（深色那组只有 3 帧，蒙版是共用的）。
struct FrameMap {
    offset: usize,
    dark: usize,
}

impl FrameMap {
    fn new(th: &Theme) -> FrameMap {
        let fluent = th.kind == ThemeKind::Fluent;
        FrameMap {
            offset: if fluent { piano_frame::GROUP } else { 0 },
            dark: if fluent && th.fdark { piano_frame::GROUP } else { 0 },
        }
    }

    fn key(&self, frame: usize) -> usize {
        self.offset + self.dark + frame
    }

    fn fade(&self, frame: usize) -> usize {
        self.offset + frame
    }
}

/// 原版 `draw_sprite_ext(spr_piano, 3/4/5, …, window_background, 0.8 - alpha / 3)`。
fn fade_tint(th: &Theme, alpha: i32) -> Color32 {
    let a = (0.8 - alpha as f32 / 3.0).clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(
        th.window_bg.r(),
        th.window_bg.g(),
        th.window_bg.b(),
        (a * 255.0) as u8,
    )
}

/// 传给 `draw_piano_key` 的 `color`：底色 c_white，超范围染粉，
/// 选中染淡紫（Fluent 用 `accent[7]`）。
fn key_color(key: u8, o: &DrawOpts, th: &Theme) -> (Color32, bool) {
    let out = key < MC_KEY_MIN || key > MC_KEY_MAX;
    let mut col = Color32::WHITE;
    if o.out_of_range && out {
        col = OUT_OF_RANGE;
    }
    if !o.playing && o.current == key {
        col = if th.kind == ThemeKind::Fluent {
            Theme::accent_at(7)
        } else {
            SELECTED
        };
    }
    (col, out)
}

struct DrawOpts {
    names: bool,
    numbers: bool,
    keyboard: bool,
    out_of_range: bool,
    current: u8,
    playing: bool,
    flats: bool,
}

pub fn show_piano(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let bank = state.icons.bank();
    let top = ui.cursor().min;
    let area = Rect::from_min_size(
        top,
        Vec2::new(ui.available_width(), ui.available_height().max(ART_H)),
    );
    // 黑键高出面板顶 7px，画在横向滚动条下方那段空档里；面板默认会把它裁掉。
    ui.set_clip_rect(Rect::from_min_max(
        pos2(area.min.x, area.min.y - BLACK_LIFT),
        area.max,
    ));
    ui.allocate_rect(area, Sense::hover());

    let rw = area.width();
    let keysmax = state.prefs.keys_to_show.max(1) as usize;
    let keysshow = (((rw - 32.0) / KEY_W).floor() as usize).min(keysmax).max(1);
    if state.piano_keys_shown as usize != keysshow {
        // 原版：窗口宽度变了就把键盘重新以 27 号键为中心。
        state.piano_start = 27usize.saturating_sub(keysshow / 2) as u8;
        state.piano_keys_shown = keysshow as u8;
    }
    let startkey = state.piano_start as usize;
    // 原版 `sharpkeys`：startkey 之前有几个黑键（白键序号 mod 7 为 1、4 的没有右邻黑键）。
    let sharpkeys = (0..startkey).filter(|a| *a % 7 != 1 && *a % 7 != 4).count();
    let x0 = area.min.x + ((rw - keysshow as f32 * KEY_W) / 2.0).floor();
    let y0 = area.min.y;

    let pointer = ui.input(|i| i.pointer.hover_pos());
    let holding = ui.input(|i| i.pointer.any_down());
    let pressing = ui.input(|i| i.pointer.any_pressed());
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let opts = DrawOpts {
        names: state.flags.show_keynames,
        numbers: state.flags.show_keynumbers,
        keyboard: state.flags.show_keyboard,
        out_of_range: state.flags.show_outofrange,
        current: state.current.key,
        playing: state.audio.is_playing(),
        flats: state.flags.flats,
    };
    let inside = |r: Rect| pointer.map_or(false, |p| r.contains(p));
    // 键盘绑定当前按住的键（`collect_keybind_presses` 每帧刷新）。
    let keys_held = state.keys_held;
    let held = |k: u8| keys_held.get(k as usize).copied().unwrap_or(false);
    // 黑键命中区：`xx + 39a - 12`（`hit_x` 已经是这条线要用的绝对横坐标）。
    let hit = |hit_x: f32| Rect::from_min_size(pos2(hit_x, y0 - BLACK_LIFT), Vec2::new(BLACK_HIT_W, BLACK_HIT_H));

    let p = ui.painter();
    let frames = FrameMap::new(&th);
    let mut picked: Option<u8> = None;
    let mut b = 0usize;
    let mut alpha = (3 - startkey as i32).max(0);
    let mut edge = 0i8;

    for a in 0..keysshow {
        let c = (startkey + a) % 7;
        let x = x0 + a as f32 * KEY_W;
        // 原版 `a > 0 && c != 2 && c != 5`：最左一根白键不画它左边的黑键。
        let with_black = a > 0 && c != 2 && c != 5;
        let white = startkey + sharpkeys + b + if with_black { 1 } else { 0 };

        if white <= KEY_MAX as usize {
            let key = white as u8;
            let white_hit = Rect::from_min_size(pos2(x, y0), Vec2::new(KEY_W, WHITE_HIT_H));
            // 原版：白键被左右黑键挡住的部分不算命中；最右一根、以及右边没有黑键的白键除外。
            let left = with_black && inside(hit(x + BLACK_HIT_X));
            let right_exists = a + 1 < keysshow && c != 1 && c != 4;
            let right = right_exists && inside(hit(x + RIGHT_BLACK_HIT_X));
            let clickable = !left && !right;
            // 原版 `down = 0.75`：鼠标按住（key_click）或键盘绑定被按下（key_press）都算。
            let down = if (clickable && inside(white_hit) && holding) || held(key) {
                PRESS_DOWN
            } else {
                0.0
            };
            if let Some(bank) = bank.as_deref() {
                draw_white_key(bank, p, &th, &frames, pos2(x, y0 + down), key, alpha, &opts);
            }
            if clickable && inside(white_hit) {
                if a == 0 {
                    edge = -1;
                }
                if a == keysshow - 1 {
                    edge = 1;
                }
                if pressing {
                    picked = Some(key);
                }
            }
        }

        if with_black && white - 1 <= KEY_MAX as usize {
            let key = (white - 1) as u8;
            let black_hit = hit(x + BLACK_HIT_X);
            let down = if (inside(black_hit) && holding) || held(key) { PRESS_DOWN } else { 0.0 };
            // 原版：白键先画，黑键压在它上面。
            if let Some(bank) = bank.as_deref() {
                draw_black_key(
                    bank,
                    p,
                    &th,
                    &frames,
                    pos2(x + BLACK_DRAW_X, y0 - BLACK_LIFT),
                    key,
                    down,
                    alpha,
                    &opts,
                );
            }
            if inside(black_hit) && pressing {
                picked = Some(key);
            }
            b += 2;
        } else {
            b += 1;
        }

        if a < 3 && alpha < 3 {
            alpha += 1;
        }
        let tail = 5.min(FADE_LIMIT.saturating_sub(startkey + keysshow));
        if a > keysshow.saturating_sub(tail) {
            alpha -= 1;
        }
    }

    if let Some(key) = picked {
        state.emit(Command::SelectKey(key));
    }

    // 原版 `draw_piano` 末尾的按键轮询：没有输入框焦点、没按 Ctrl 时，
    // 按下 `init_keys` 表里的键就发一次音（`play_sound(instrument, a, 100, 100, 0)`）。
    collect_keybind_presses(state, ui);

    // 原版 `d` + `delay`：停在边缘白键上（没按住鼠标时）每 0.1 秒平移一个音。
    state.piano_scroll_delay = (state.piano_scroll_delay - dt).max(0.0);
    if edge != 0 && !holding && state.piano_scroll_delay == 0.0 {
        if edge < 0 && startkey > 0 {
            state.piano_start = (startkey - 1) as u8;
        } else if edge > 0 && startkey + keysshow < RIGHT_LIMIT {
            state.piano_start = (startkey + 1) as u8;
        }
        state.piano_scroll_delay = 0.1;
    }
}

/// 原版 `draw_piano` 结尾那段 `for (a = 0; a <= 87)` 的键盘轮询：
/// `key_press[a] = keyboard_check(piano_key[a])` 既用来触发（按下沿）也用来把琴键画成按下态。
fn collect_keybind_presses(state: &mut EditorState, ui: &mut Ui) {
    let blocked = ui.ctx().memory(|m| m.focused().is_some());
    let (held, pressed): (KeysHeld, KeysHeld) = if blocked {
        (NO_KEYS, NO_KEYS)
    } else {
        ui.input(|i| {
            let mut held = NO_KEYS;
            let mut pressed = NO_KEYS;
            if !crate::shortcuts::ctrl_chord(i) {
                for (idx, c) in KEYBINDS.iter().enumerate() {
                    if let Some(k) = keybind_key(*c) {
                        let key = MC_KEY_MIN as usize + idx;
                        held[key] = i.key_down(k);
                        pressed[key] = i.key_pressed(k);
                    }
                }
            }
            (held, pressed)
        })
    };
    state.keys_held = held;
    for (key, hit) in pressed.iter().enumerate() {
        if *hit {
            state.emit(Command::KeyPress(key as u8));
        }
    }
}

/// 原版白键：整帧（含文字）一起按 `floor(7 * down)` 下沉，所以 `at` 已带位移。
fn draw_white_key(
    bank: &Bank,
    p: &Painter,
    th: &Theme,
    frames: &FrameMap,
    at: Pos2,
    key: u8,
    alpha: i32,
    o: &DrawOpts,
) {
    let (col, out) = key_color(key, o, th);
    bank.piano_key(p, at, frames.key(piano_frame::WHITE), col);
    if alpha < 3 {
        bank.piano_key(p, at, frames.fade(piano_frame::WHITE_FADE), fade_tint(th, alpha));
    }

    let kb = o.keyboard && keybind(key).is_some();
    let clicks = o.numbers && !out;
    // 原版 `draw_set_alpha(alpha / 3)`，墨色在非 Fluent 深色下是 c_black。
    let ink = tint_alpha(
        if th.kind == ThemeKind::Fluent && th.fdark { col } else { Color32::BLACK },
        alpha,
    );
    let x = at.x + 20.0;
    if o.names {
        p.text(
            pos2(x, at.y + 85.0 - lift(kb, clicks)),
            TEXT_AT,
            key_name(key, true, o.flats),
            TEXT,
            ink,
        );
    }
    if clicks {
        p.text(
            pos2(x, at.y + 85.0 + 5.0 - 8.0 * kb as u8 as f32),
            TEXT_AT,
            (key as i32 - MC_KEY_MIN as i32).to_string(),
            TEXT,
            ink,
        );
    }
    if kb {
        if let Some(c) = keybind(key) {
            p.text(pos2(x, at.y + 95.0), TEXT_AT, c.to_string(), TEXT, ink);
        }
    }
}

/// 原版黑键：背面固定在 `at`，正面（和文字）随 `floor(7 * down)` 下沉。
fn draw_black_key(
    bank: &Bank,
    p: &Painter,
    th: &Theme,
    frames: &FrameMap,
    at: Pos2,
    key: u8,
    down: f32,
    alpha: i32,
    o: &DrawOpts,
) {
    let (col, out) = key_color(key, o, th);
    let face = at + Vec2::new(0.0, down);
    bank.piano_key(p, at, frames.key(piano_frame::BLACK_BACK), col);
    if alpha < 3 {
        bank.piano_key(p, at, frames.fade(piano_frame::BLACK_BACK_FADE), fade_tint(th, alpha));
    }
    bank.piano_key(p, face, frames.key(piano_frame::BLACK_FACE), col);
    if alpha < 3 {
        bank.piano_key(p, face, frames.fade(piano_frame::BLACK_FACE_FADE), fade_tint(th, alpha));
    }

    let kb = o.keyboard && keybind(key).is_some();
    let clicks = o.numbers && !out;
    let ink = tint_alpha(
        if th.kind == ThemeKind::Fluent && th.fdark { col } else { Color32::WHITE },
        alpha,
    );
    let x = at.x + 19.0;
    if o.names {
        p.text(
            pos2(x, face.y + 29.0 - lift(kb, clicks)),
            TEXT_AT,
            key_name(key, true, o.flats),
            TEXT,
            ink,
        );
    }
    if clicks {
        p.text(
            pos2(x, face.y + 34.0 - 8.0 * kb as u8 as f32),
            TEXT_AT,
            (key as i32 - MC_KEY_MIN as i32).to_string(),
            TEXT,
            ink,
        );
    }
    if kb {
        if let Some(c) = keybind(key) {
            p.text(pos2(x, face.y + 39.0), TEXT_AT, c.to_string(), TEXT, ink);
        }
    }
}

/// 原版 `- 4 * showkeybind - 4 * (showkeybind && showclicks) - 6 * showclicks`。
fn lift(keybind: bool, clicks: bool) -> f32 {
    4.0 * keybind as u8 as f32
        + 4.0 * (keybind && clicks) as u8 as f32
        + 6.0 * clicks as u8 as f32
}

/// 原版 `draw_set_alpha(alpha / 3)`：文字按淡出档位整体变透明。
fn tint_alpha(c: Color32, alpha: i32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (255.0 * alpha.max(0) as f32 / 3.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 原版 `init_keys()` 的 25 个绑定必须逐字符对上，琴键上印的字母才是真的。
    #[test]
    fn keybinds_match_init_keys() {
        // scripts/init_keys/init_keys.gml：从 piano_key[33] 起依次是 1 Q 2 W 3 E R 5 T 6 Y U 8 I 9 O 0 P Z S X D C V G
        assert_eq!(KEYBINDS.iter().collect::<String>(), "1Q2W3ER5T6YU8I9O0PZSXDCVG");
        assert_eq!(KEYBINDS.len(), 25);
        assert_eq!(keybind(MC_KEY_MIN), Some('1'));
        assert_eq!(keybind(MC_KEY_MAX), Some('G'));
        assert_eq!(keybind(MC_KEY_MIN - 1), None);
        assert_eq!(keybind(MC_KEY_MAX + 1), None);
        // 每个绑定都要能翻成 egui 键位，否则印出来的字母按了没反应。
        for c in KEYBINDS {
            assert!(keybind_key(c).is_some(), "按键 {c} 没有映射");
        }
    }

    /// 原版帧号：经典组 0..5，Fluent 浅色 6..11，Fluent 深色只有 12/13/14，蒙版仍用 9/10/11。
    #[test]
    fn piano_frame_groups() {
        let classic = FrameMap::new(&Theme::new(ThemeKind::Aqua));
        assert_eq!(classic.key(piano_frame::WHITE), 0);
        assert_eq!(classic.fade(piano_frame::WHITE_FADE), 3);
        let fluent = FrameMap::new(&Theme::new(ThemeKind::Fluent));
        assert_eq!(fluent.key(piano_frame::BLACK_BACK), 8);
        assert_eq!(fluent.fade(piano_frame::BLACK_BACK_FADE), 11);
        let mut dark = Theme::new(ThemeKind::Fluent);
        dark.fdark = true;
        let fd = FrameMap::new(&dark);
        assert_eq!(fd.key(piano_frame::BLACK_BACK), 14);
        assert_eq!(fd.fade(piano_frame::BLACK_BACK_FADE), 11);
    }
}
