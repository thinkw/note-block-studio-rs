//! 底部钢琴键盘 —— 照原版 `draw_piano` / `draw_piano_key` 的绝对像素：
//! 白键固定 39px 步进、整排水平居中、没有翻页按钮；鼠标停在最左 / 最右白键上
//! 每 0.1 秒平移一个音（原版 `d` + `delay`）。

use egui::{pos2, Align2, Color32, FontId, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use nbs_core::model::{key_name, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::commands::Command;
use crate::state::EditorState;
use crate::theme::Theme;

/// 原版白键步进 `xx + 39 * a`。
const KEY_W: f32 = 39.0;
/// `spr_piano` 帧 40×121：白键整帧可见，命中区 39×128（多出的 7px 是按下行程）。
const WHITE_ART_H: f32 = 121.0;
const WHITE_HIT_H: f32 = 128.0;
/// 白键帧底 21 行是 192 灰的立面（顶面 255 → 底面 192）。
const WHITE_FACE_H: f32 = 21.0;
/// 黑键在帧内的不透明部分是第 7..31 列（25px）、第 0..70 行（71px），
/// 帧画在 `白键x - 19`，所以可见部分正好压在两根白键的分界上。
const BLACK_IN_FRAME_X: f32 = 7.0;
const BLACK_W: f32 = 25.0;
const BLACK_H: f32 = 71.0;
/// 黑键正面是第 0..59 行，背面（第 60..70 行）在按下时露出来。
const BLACK_FACE_H: f32 = 60.0;
/// 黑键顶比白键顶高 7px。
const BLACK_LIFT: f32 = 7.0;
/// 原版 `floor(7 * 0.75)`：按住时的下沉像素。
const PRESS_DOWN: f32 = 5.0;
/// 原版右移上限 `startkey + k < 52`。
const RIGHT_LIMIT: usize = 52;
/// 原版尾部淡出阈值 `min(5, 54 - (startkey + k))`。
const FADE_LIMIT: usize = 54;
/// 原版 8224255 = RGB(255,126,125)，超出 MC 音域的键。
const OUT_OF_RANGE: Color32 = Color32::from_rgb(255, 126, 125);
/// 原版 16753828 = RGB(164,164,255)，当前选中的键。
const SELECTED: Color32 = Color32::from_rgb(164, 164, 255);

/// 原版 `merge_color` / `draw_sprite_ext(..., color, alpha)` 的按通道线性混合。
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8,
    )
}

/// 原版边缘键的淡出：用窗口底色以 `0.8 - alpha/3` 的不透明度盖一层遮罩帧。
fn fade(th: &Theme, alpha: i32) -> Option<Color32> {
    if alpha >= 3 {
        return None;
    }
    let a = (0.8 - alpha as f32 / 3.0).clamp(0.0, 1.0);
    Some(Color32::from_rgba_unmultiplied(
        th.bg.r(),
        th.bg.g(),
        th.bg.b(),
        (a * 255.0) as u8,
    ))
}

pub fn show_piano(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let top = ui.cursor().min;
    let area = Rect::from_min_size(
        top,
        Vec2::new(ui.available_width(), ui.available_height().max(WHITE_ART_H)),
    );
    // 黑键高出面板顶 7px，画在横向滚动条下方那段空档里；面板默认会把它裁掉。
    ui.set_clip_rect(Rect::from_min_max(
        pos2(area.min.x, area.min.y - BLACK_LIFT),
        area.max,
    ));
    ui.allocate_rect(area, Sense::hover());

    let rw = area.width();
    let keysmax = state.prefs.keys_to_show.max(1) as usize;
    let keysshow = (((rw - 32.0) / KEY_W).floor() as usize)
        .min(keysmax)
        .max(1);
    if state.piano_keys_shown as usize != keysshow {
        // 原版：窗口宽度变了就把键盘重新以 27 号键为中心。
        state.piano_start = 27usize.saturating_sub(keysshow / 2) as u8;
        state.piano_keys_shown = keysshow as u8;
    }
    let startkey = state.piano_start as usize;
    // 原版 `sharpkeys`：startkey 之前有几个黑键（白键序号 mod 7 为 1、4 的没有右邻黑键）。
    let sharpkeys = (0..startkey)
        .filter(|a| *a % 7 != 1 && *a % 7 != 4)
        .count();
    let x0 = area.min.x + ((rw - keysshow as f32 * KEY_W) / 2.0).floor();
    let y0 = area.min.y;

    let pointer = ui.input(|i| i.pointer.hover_pos());
    let holding = ui.input(|i| i.pointer.any_down());
    let pressing = ui.input(|i| i.pointer.any_pressed());
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let opts = DrawOpts {
        names: state.flags.show_keynames,
        numbers: state.flags.show_keynumbers,
        mark_out: state.flags.show_incompatible,
        flats: state.flags.flats,
        current: state.current.key,
        playing: state.audio.is_playing(),
    };
    let inside = |r: Rect| pointer.map_or(false, |p| r.contains(p));

    let mut shapes: Vec<Shape> = Vec::new();
    let mut texts: Vec<(String, f32, f32, Color32)> = Vec::new();
    let mut black_hits: Vec<(Rect, u8)> = Vec::new();
    let mut white_hits: Vec<(Rect, u8)> = Vec::new();
    let mut b = 0usize;
    let mut alpha = (3 - startkey as i32).max(0);
    let mut edge = 0i8;

    for a in 0..keysshow {
        let c = (startkey + a) % 7;
        let x = x0 + a as f32 * KEY_W;
        // 原版 `a > 0 && c != 2 && c != 5`：最左一根白键不画它左边的黑键。
        let with_black = a > 0 && c != 2 && c != 5;
        let white = startkey + sharpkeys + b + if with_black { 1 } else { 0 };

        if with_black && white - 1 <= KEY_MAX as usize {
            let key = (white - 1) as u8;
            let hit = Rect::from_min_size(
                pos2(x - BLACK_IN_FRAME_X, y0 - BLACK_LIFT),
                Vec2::new(BLACK_W, BLACK_H),
            );
            let down = if inside(hit) && holding { PRESS_DOWN } else { 0.0 };
            draw_black(&mut shapes, &mut texts, &th, hit, key, down, alpha, &opts);
            black_hits.push((hit, key));
            b += 1;
        }

        if white <= KEY_MAX as usize {
            let hit = Rect::from_min_size(pos2(x, y0), Vec2::new(KEY_W, WHITE_HIT_H));
            // 原版：白键被左右黑键挡住的部分不算命中；最右一根、以及右边没有黑键的白键除外。
            let left = with_black
                && inside(Rect::from_min_size(
                    pos2(x - BLACK_IN_FRAME_X, y0 - BLACK_LIFT),
                    Vec2::new(BLACK_W, BLACK_H),
                ));
            let right_exists = a + 1 < keysshow && c != 1 && c != 4;
            let right = right_exists
                && inside(Rect::from_min_size(
                    pos2(x + 27.0, y0 - BLACK_LIFT),
                    Vec2::new(BLACK_W, BLACK_H),
                ));
            let clickable = !left && !right;
            draw_white(
                &mut shapes,
                &mut texts,
                &th,
                hit,
                key_of(white),
                alpha,
                &opts,
                clickable && inside(hit) && holding,
            );
            if clickable {
                white_hits.push((hit, key_of(white)));
                // 原版 `d` 只在鼠标停在这根白键上时才置位，否则一帧空转就把键盘平移走了。
                if inside(hit) {
                    if a == 0 {
                        edge = -1;
                    }
                    if a == keysshow - 1 {
                        edge = 1;
                    }
                }
            }
        }
        b += 1;

        if a < 3 && alpha < 3 {
            alpha += 1;
        }
        let tail = 5.min(FADE_LIMIT.saturating_sub(startkey + keysshow));
        if a > keysshow.saturating_sub(tail + 1) {
            alpha -= 1;
        }
    }

    ui.painter().extend(shapes);
    let font = FontId::proportional(9.0);
    for (text, x, y, color) in texts {
        ui.painter()
            .text(pos2(x, y), Align2::CENTER_TOP, text, font.clone(), color);
    }

    // 原版按下即发声（`mouse_check_button` 的 0→1 跳变），黑键在上层先判。
    let mut picked: Option<u8> = None;
    if pressing {
        for (r, key) in black_hits.iter() {
            if inside(*r) {
                picked = Some(*key);
                break;
            }
        }
        if picked.is_none() {
            for (r, key) in white_hits.iter() {
                if inside(*r) {
                    picked = Some(*key);
                    break;
                }
            }
        }
    }
    if let Some(key) = picked {
        state.emit(Command::SelectKey(key));
    }

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

fn key_of(index: usize) -> u8 {
    index.min(KEY_MAX as usize) as u8
}

struct DrawOpts {
    names: bool,
    numbers: bool,
    mark_out: bool,
    flats: bool,
    current: u8,
    playing: bool,
}

/// 原版音名行在有序号时上移 6px（`- 6 * (showclicks)`）。
fn name_lift(showclicks: bool) -> f32 {
    if showclicks {
        6.0
    } else {
        0.0
    }
}

fn key_color(key: u8, base: Color32, o: &DrawOpts) -> (Color32, bool) {
    let out = key < MC_KEY_MIN || key > MC_KEY_MAX;
    let mut col = base;
    if o.mark_out && out {
        col = OUT_OF_RANGE;
    }
    if !o.playing && o.current == key {
        col = SELECTED;
    }
    (col, out)
}

fn draw_white(
    shapes: &mut Vec<Shape>,
    texts: &mut Vec<(String, f32, f32, Color32)>,
    th: &Theme,
    hit: Rect,
    key: u8,
    alpha: i32,
    o: &DrawOpts,
    held: bool,
) {
    let down = if held { PRESS_DOWN } else { 0.0 };
    let base = if th.dark {
        Color32::from_gray(232)
    } else {
        Color32::WHITE
    };
    let (col, out) = key_color(key, base, o);
    let showclicks = o.numbers && !out;
    let r = Rect::from_min_size(
        pos2(hit.min.x, hit.min.y + down),
        Vec2::new(KEY_W, WHITE_ART_H),
    );
    shapes.push(Shape::rect_filled(r, 0.0, col));
    shapes.push(Shape::rect_filled(
        Rect::from_min_size(
            pos2(r.min.x, r.max.y - WHITE_FACE_H),
            Vec2::new(KEY_W, WHITE_FACE_H),
        ),
        0.0,
        mix(col, Color32::BLACK, 0.25),
    ));
    shapes.push(Shape::rect_stroke(
        r,
        0.0,
        Stroke::new(1.0_f32, th.border),
        StrokeKind::Inside,
    ));
    if let Some(f) = fade(th, alpha) {
        shapes.push(Shape::rect_filled(r, 0.0, f));
    }
    let ink = if th.dark { col } else { Color32::BLACK };
    let cx = hit.min.x + 20.0;
    if o.names {
        texts.push((
            key_name(key, true, o.flats),
            cx,
            hit.min.y + 85.0 - name_lift(showclicks),
            ink,
        ));
    }
    if showclicks {
        texts.push((
            (key as i32 - MC_KEY_MIN as i32).to_string(),
            cx,
            hit.min.y + 90.0,
            ink,
        ));
    }
}

fn draw_black(
    shapes: &mut Vec<Shape>,
    texts: &mut Vec<(String, f32, f32, Color32)>,
    th: &Theme,
    hit: Rect,
    key: u8,
    down: f32,
    alpha: i32,
    o: &DrawOpts,
) {
    let base = if th.dark {
        Color32::from_gray(51)
    } else {
        Color32::from_gray(101)
    };
    let (col, out) = key_color(key, base, o);
    let showclicks = o.numbers && !out;
    shapes.push(Shape::rect_filled(
        hit,
        0.0,
        mix(col, Color32::BLACK, 0.45),
    ));
    shapes.push(Shape::rect_filled(
        Rect::from_min_size(pos2(hit.min.x, hit.min.y + down), Vec2::new(BLACK_W, BLACK_FACE_H)),
        0.0,
        col,
    ));
    if let Some(f) = fade(th, alpha) {
        shapes.push(Shape::rect_filled(hit, 0.0, f));
    }
    let ink = if th.dark { col } else { Color32::WHITE };
    let cx = hit.min.x + BLACK_W / 2.0;
    if o.names {
        texts.push((
            key_name(key, true, o.flats),
            cx,
            hit.min.y + 29.0 - name_lift(showclicks),
            ink,
        ));
    }
    if showclicks {
        texts.push((
            (key as i32 - MC_KEY_MIN as i32).to_string(),
            cx,
            hit.min.y + 34.0,
            ink,
        ));
    }
}
