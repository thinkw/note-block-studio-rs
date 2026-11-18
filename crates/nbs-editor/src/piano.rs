//! 底部钢琴键盘 —— 照原版 `draw_piano` / `draw_piano_key` 逐帧贴 `spr_piano`：
//! 白键 0、黑键背面 2 / 正面 1；每列先画白键再画黑键（黑键压在上面）。
//! 白键固定 39px 步进、整排按窗口宽度居中，鼠标停在最左 / 最右白键上每 0.1 秒平移一个音
//! （原版 `d` + `delay`）。`spr_piano` 资源缺失时整排不画 —— 原版没有「无贴图键盘」，
//! 所以这里也不画一个假的出来。
//! 端口自加（用户要求）：① 面板独立缩放，`u = prefs.piano_scale` 乘所有原版像素常量；
//! ② 去掉原版 `draw_piano.gml:12/119-120` 的两侧渐隐蒙版（alpha 机制），恒全亮度
//! ——原版播放器模式（isplayer）本来就不渐隐。

use egui::{pos2, Align2, Color32, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use nbs_core::model::{key_name, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::commands::Command;
use crate::fonts::{bold_font, main_font};
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
/// 原版 8224255 = RGB(255,126,125)，超出 MC 音域的键。
const OUT_OF_RANGE: Color32 = Color32::from_rgb(255, 126, 125);
/// 原版 16753828 = RGB(164,164,255)，非 Fluent 主题下当前选中的键。
const SELECTED: Color32 = Color32::from_rgb(164, 164, 255);
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
            th.accent(7)
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
    // 端口自加（用户要求）：`u` = 琴键面板独立缩放，乘所有原版像素常量。
    let u = state.prefs.piano_scale.max(0.1);
    let top = ui.cursor().min;
    let area = Rect::from_min_size(
        top,
        Vec2::new(ui.available_width(), ui.available_height().max(ART_H * u)),
    );
    // 缩放快捷键按这块矩形判断指针是否悬停在琴键上（先记下来，再谈画不画得成）。
    state.piano_rect = Some(area);
    // 黑键高出面板顶 7px，画在横向滚动条下方那段空档里；面板默认会把它裁掉。
    ui.set_clip_rect(Rect::from_min_max(
        pos2(area.min.x, area.min.y - BLACK_LIFT * u),
        area.max,
    ));
    ui.allocate_rect(area, Sense::hover());

    let rw = area.width();
    // 偏离原版（用户要求）：琴键横向占满面板——去掉原版 `keysmax`（首选项
    // 「同屏显示的琴键数」，20..50）的截短与 32px 内缩，显示到放不下为止，
    // 上限 = 完整键盘 52 个白键槽（startkey 27-52/2=1，即 A0 起）。
    let keysshow = 52usize.min(((rw) / (KEY_W * u)).floor() as usize).max(1);
    if state.piano_keys_shown as usize != keysshow {
        // 原版：窗口宽度变了就把键盘重新以 27 号键为中心。
        state.piano_start = 27usize.saturating_sub(keysshow / 2) as u8;
        state.piano_keys_shown = keysshow as u8;
    }
    let startkey = state.piano_start as usize;
    // 原版 `sharpkeys`：startkey 之前有几个黑键（白键序号 mod 7 为 1、4 的没有右邻黑键）。
    let sharpkeys = (0..startkey).filter(|a| *a % 7 != 1 && *a % 7 != 4).count();
    let x0 = area.min.x + ((rw - keysshow as f32 * KEY_W * u) / 2.0).floor();
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
    let show_chart = state.flags.show_notechart;
    // 键盘绑定当前按住的键（`collect_keybind_presses` 每帧刷新）。
    let keys_held = state.keys_held;
    let held = |k: u8| keys_held.get(k as usize).copied().unwrap_or(false);
    // 黑键命中区：`xx + 39a - 12`（`hit_x` 已经是这条线要用的绝对横坐标）。
    let hit = |hit_x: f32| {
        Rect::from_min_size(
            pos2(hit_x, y0 - BLACK_LIFT * u),
            Vec2::new(BLACK_HIT_W * u, BLACK_HIT_H * u),
        )
    };

    let p = ui.painter();
    let frames = FrameMap::new(&th);
    let mut picked: Option<u8> = None;
    let mut b = 0usize;
    let mut edge = 0i8;

    // 偏离原版（用户要求）：去掉 `draw_piano.gml:12/119-120` 的两侧渐隐
    // （alpha 从 `3 - startkey` 起步、尾部递减），所有琴键恒全亮度——
    // 原版播放器模式（isplayer）本来就不渐隐。
    for a in 0..keysshow {
        let c = (startkey + a) % 7;
        let x = x0 + a as f32 * KEY_W * u;
        // 原版 `a > 0 && c != 2 && c != 5`：最左一根白键不画它左边的黑键。
        let with_black = a > 0 && c != 2 && c != 5;
        let white = startkey + sharpkeys + b + if with_black { 1 } else { 0 };

        if white <= KEY_MAX as usize {
            let key = white as u8;
            let white_hit =
                Rect::from_min_size(pos2(x, y0), Vec2::new(KEY_W * u, WHITE_HIT_H * u));
            // 原版：白键被左右黑键挡住的部分不算命中；最右一根、以及右边没有黑键的白键除外。
            let left = with_black && inside(hit(x + BLACK_HIT_X * u));
            let right_exists = a + 1 < keysshow && c != 1 && c != 4;
            let right = right_exists && inside(hit(x + RIGHT_BLACK_HIT_X * u));
            let hovering = !left && !right && inside(white_hit);
            // 原版 `down = 0.75`：鼠标按住（key_click）或键盘绑定被按下（key_press）都算。
            let down = if (hovering && holding) || held(key) {
                PRESS_DOWN * u
            } else {
                0.0
            };
            // 原版在画这根键之前先画五线谱：门是 `c2 > 6 && c2 < 69 && playing = 0`
            // （C/F 分支用 c1，值相同），键号参数是列号 `startkey + a`。没有播放器模式，
            // `!isplayer` 恒真，C/F 分支少的那一个门可以合并。
            if hovering && show_chart && !opts.playing && white > 6 && white < 69 {
                if let Some(bank) = bank.as_deref() {
                    draw_notechart(
                        ui,
                        bank,
                        &th,
                        rw,
                        x + 18.0 * u,
                        y0,
                        (startkey + a) as i32,
                        false,
                        opts.flats,
                        u,
                    );
                }
            }
            if let Some(bank) = bank.as_deref() {
                draw_white_key(bank, p, &th, &frames, pos2(x, y0 + down), key, &opts, u);
            }
            if hovering {
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
            let black_hit = hit(x + BLACK_HIT_X * u);
            let down = if (inside(black_hit) && holding) || held(key) {
                PRESS_DOWN * u
            } else {
                0.0
            };
            // 原版黑键五线谱：升号门 `c1 > 6 && c1 < 70`、降号门 `c1 > 5 && c1 < 69`，
            // 键号参数升号写 `startkey + a - 1`、降号写 `startkey + a`（同为列号空间）。
            if inside(black_hit) && show_chart && !opts.playing {
                let (ok, chart_key) = if opts.flats {
                    (key > 5 && key < 69, (startkey + a) as i32)
                } else {
                    (key > 6 && key < 70, (startkey + a) as i32 - 1)
                };
                if ok {
                    if let Some(bank) = bank.as_deref() {
                        draw_notechart(
                            ui,
                            bank,
                            &th,
                            rw,
                            x,
                            y0,
                            chart_key,
                            true,
                            opts.flats,
                            u,
                        );
                    }
                }
            }
            // 原版：白键先画，黑键压在它上面。
            if let Some(bank) = bank.as_deref() {
                draw_black_key(
                    bank,
                    p,
                    &th,
                    &frames,
                    pos2(x + BLACK_DRAW_X * u, y0 - BLACK_LIFT * u),
                    key,
                    down,
                    &opts,
                    u,
                );
            }
            if inside(black_hit) && pressing {
                picked = Some(key);
            }
            b += 2;
        } else {
            b += 1;
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
/// 实时 MIDI 在同一道闸门里逐键过 `midi_input_set_key`（draw_piano.gml:152），
/// 帧尾清空按下/松开队列。
fn collect_keybind_presses(state: &mut EditorState, ui: &mut Ui) {
    // 原版 `window = 0 && text_focus = -1 && key_edit = -1 && !check_ctrl()`：
    // 窗口开着 / 输入框有焦点 / 按 Ctrl 时整段轮询跳过，按住态保持原样。
    let blocked = state.window.is_some() || ui.ctx().memory(|m| m.focused().is_some());
    // 没被键盘覆盖的键保留现值：MIDI 按住的键也在 keys_held 里（原版只对
    // `piano_key[a] > 0` 的键重写 `key_press[a]`）。
    let mut held = state.keys_held;
    let mut pressed: KeysHeld = NO_KEYS;
    if !blocked {
        ui.input(|i| {
            if !crate::shortcuts::ctrl_chord(i) {
                for (idx, c) in KEYBINDS.iter().enumerate() {
                    if let Some(k) = keybind_key(*c) {
                        let key = MC_KEY_MIN as usize + idx;
                        held[key] = i.key_down(k);
                        pressed[key] = i.key_pressed(k);
                    }
                }
            }
        });
    }
    state.keys_held = held;
    if blocked {
        return;
    }
    for (key, hit) in pressed.iter().enumerate() {
        if *hit {
            state.emit(Command::KeyPress(key as u8));
        }
    }
    // 原版 control_step 的泵 + `midi_input_set_key(a, totalcols)`：0..=87 逐键
    // 过按下/松开队列，帧尾清空（draw_piano.gml:161-162）。
    state.live_midi.pump();
    for key in 0..=KEY_MAX {
        state.midi_input_set_key(key);
    }
    state.live_midi.presses.clear();
    state.live_midi.releases.clear();
}

/// 原版白键：整帧（含文字）一起按 `floor(7 * down)` 下沉，所以 `at` 已带位移。
fn draw_white_key(
    bank: &Bank,
    p: &Painter,
    th: &Theme,
    frames: &FrameMap,
    at: Pos2,
    key: u8,
    o: &DrawOpts,
    u: f32,
) {
    let (col, out) = key_color(key, o, th);
    bank.piano_key(p, at, frames.key(piano_frame::WHITE), u, col);

    let kb = o.keyboard && keybind(key).is_some();
    let clicks = o.numbers && !out;
    // 原版墨色在非 Fluent 深色下是 c_black；Fluent 深色用键色反衬
    // （渐隐去掉后不再套 alpha，恒全亮）。
    let ink = if th.kind == ThemeKind::Fluent && th.fdark {
        col
    } else {
        Color32::BLACK
    };
    let x = at.x + 20.0 * u;
    let font = crate::fonts::scaled(main_font(), u);
    if o.names {
        p.text(
            pos2(x, at.y + (85.0 - lift(kb, clicks)) * u),
            TEXT_AT,
            key_name(key, true, o.flats),
            font.clone(),
            ink,
        );
    }
    if clicks {
        p.text(
            pos2(x, at.y + (85.0 + 5.0 - 8.0 * kb as u8 as f32) * u),
            TEXT_AT,
            (key as i32 - MC_KEY_MIN as i32).to_string(),
            font.clone(),
            ink,
        );
    }
    if kb {
        if let Some(c) = keybind(key) {
            p.text(pos2(x, at.y + 95.0 * u), TEXT_AT, c.to_string(), font, ink);
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
    o: &DrawOpts,
    u: f32,
) {
    let (col, out) = key_color(key, o, th);
    let face = at + Vec2::new(0.0, down);
    bank.piano_key(p, at, frames.key(piano_frame::BLACK_BACK), u, col);
    bank.piano_key(p, face, frames.key(piano_frame::BLACK_FACE), u, col);

    let kb = o.keyboard && keybind(key).is_some();
    let clicks = o.numbers && !out;
    let ink = if th.kind == ThemeKind::Fluent && th.fdark {
        col
    } else {
        Color32::WHITE
    };
    let x = at.x + 19.0 * u;
    let font = crate::fonts::scaled(bold_font(), u);
    if o.names {
        p.text(
            pos2(x, face.y + (29.0 - lift(kb, clicks)) * u),
            TEXT_AT,
            key_name(key, true, o.flats),
            font.clone(),
            ink,
        );
    }
    if clicks {
        p.text(
            pos2(x, face.y + (34.0 - 8.0 * kb as u8 as f32) * u),
            TEXT_AT,
            (key as i32 - MC_KEY_MIN as i32).to_string(),
            font.clone(),
            ink,
        );
    }
    if kb {
        if let Some(c) = keybind(key) {
            p.text(pos2(x, face.y + 39.0 * u), TEXT_AT, c.to_string(), font, ink);
        }
    }
}

/// 原版 `draw_notechart(x1, yy, key, accidental)`：悬停琴键上的五线谱面板。
/// 画在与琴键同层、剪辑区向上扩展的 painter 里：面板 / 指针 / 音符都在这根键的
/// 精灵之前出队（原版同序），而整层钢琴晚于网格，所以面板叠在网格之上。
/// 原版传入的 `key` 是列号 `startkey + a`（黑键升号再 -1），不是实际悬停键号，
/// 音符位置因此与键名差 `sharpkeys + b` —— 原版如此，逐字复刻。
fn draw_notechart(
    ui: &Ui,
    bank: &Bank,
    th: &Theme,
    rw: f32,
    x1: f32,
    y0: f32,
    key: i32,
    accidental: bool,
    flats: bool,
    u: f32,
) {
    let yy = y0 - 32.0 * u;
    let xx = chart_left(x1, rw, u);
    // 剪辑区向上扩到最高键位（startkey + keysshow ≤ 51）的上加线 / 音符顶，
    // 向下盖过白键命中区（越界部分会被之后画的琴键压住，与原版一致）。
    let p = Painter::new(
        ui.ctx().clone(),
        ui.layer_id(),
        Rect::from_min_max(pos2(0.0, y0 - 660.0 * u), pos2(rw, y0 + WHITE_HIT_H * u)),
    );
    let right = x1 > rw - 250.0 * u;
    bank.notechart_panel(&p, pos2(xx, yy - 319.0 * u), th.kind, u);
    bank.notechart_pointer(
        &p,
        pos2(x1 - 16.0 * u * right as u8 as f32, yy),
        th.kind,
        right,
        u,
    );

    // 五线谱横线（跳过第 5 条给中加线留位）与左括号两条竖线。
    let fade = Stroke::new(1.0 * u, Color32::from_black_alpha(64));
    let solid = Stroke::new(1.0 * u, Color32::BLACK);
    for a in 0..11i32 {
        if a != 5 {
            let y = yy - 250.0 * u + 16.0 * a as f32 * u;
            p.line_segment(
                [pos2(xx + 32.0 * u, y), pos2(xx + 288.0 * u, y)],
                fade,
            );
        }
    }
    p.line_segment(
        [
            pos2(xx + 32.0 * u, yy - 250.0 * u),
            pos2(xx + 32.0 * u, yy - 250.0 * u + 64.0 * u),
        ],
        solid,
    );
    p.line_segment(
        [
            pos2(xx + 32.0 * u, yy - 250.0 * u + 96.0 * u),
            pos2(xx + 32.0 * u, yy - 250.0 * u + 160.0 * u),
        ],
        solid,
    );
    bank.gclef(
        &p,
        pos2(xx + 58.0 * u, yy - 250.0 * u + 48.0 * u),
        u,
    );
    bank.fclef(
        &p,
        pos2(xx + 48.0 * u, yy - 250.0 * u + 112.0 * u),
        u,
    );
    bank.note_accidental(
        &p,
        pos2(xx + 152.0 * u, yy - 250.0 * u - 8.0 * (key - 33) as f32 * u),
        flats,
        accidental,
        key > 22,
        u,
    );
    // 中加线（键 23/24 一条）与上 / 下加线，都是 0.25 黑线。
    if key > 22 && key < 25 {
        let y = yy - 250.0 * u + 80.0 * u;
        p.line_segment([pos2(xx + 122.0 * u, y), pos2(xx + 180.0 * u, y)], fade);
    }
    for a in 1..upper_ledger_bound(key) {
        let y = yy - 250.0 * u - 16.0 * a as f32 * u;
        p.line_segment([pos2(xx + 122.0 * u, y), pos2(xx + 180.0 * u, y)], fade);
    }
    for a in 0..lower_ledger_bound(key) {
        let y = yy - 250.0 * u + 176.0 * u + 16.0 * a as f32 * u;
        p.line_segment([pos2(xx + 122.0 * u, y), pos2(xx + 180.0 * u, y)], fade);
    }
}

/// 原版 `median(0, x1 - 32, rw - 320)`：三数取中，面板左缘。
fn chart_left(x1: f32, rw: f32, u: f32) -> f32 {
    let mut v = [0.0, x1 - 32.0 * u, rw - 320.0 * u];
    v.sort_by(|a, b| a.total_cmp(b));
    v[1]
}

/// 原版上加线循环上界 `floor((key - 31) / 2)`（`for a = 1; a < bound`）。
fn upper_ledger_bound(key: i32) -> i32 {
    ((key - 31) as f32 / 2.0).floor() as i32
}

/// 原版下加线循环上界 `7 - ceil(key / 2)`（`for a = 0; a < bound`）。
fn lower_ledger_bound(key: i32) -> i32 {
    (7.0 - (key as f32 / 2.0).ceil()) as i32
}

/// 原版 `- 4 * showkeybind - 4 * (showkeybind && showclicks) - 6 * showclicks`。
fn lift(keybind: bool, clicks: bool) -> f32 {
    4.0 * keybind as u8 as f32
        + 4.0 * (keybind && clicks) as u8 as f32
        + 6.0 * clicks as u8 as f32
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

    /// 原版帧号：经典组 0..5，Fluent 浅色 6..11，Fluent 深色只有 12/13/14。
    #[test]
    fn piano_frame_groups() {
        let classic = FrameMap::new(&Theme::new(ThemeKind::Aqua));
        assert_eq!(classic.key(piano_frame::WHITE), 0);
        let fluent = FrameMap::new(&Theme::new(ThemeKind::Fluent));
        assert_eq!(fluent.key(piano_frame::BLACK_BACK), 8);
        let mut dark = Theme::new(ThemeKind::Fluent);
        dark.fdark = true;
        let fd = FrameMap::new(&dark);
        assert_eq!(fd.key(piano_frame::BLACK_BACK), 14);
    }

    /// 原版 `median(0, x1 - 32, rw - 320)` 与上 / 下加线循环上界。
    #[test]
    fn notechart_geometry() {
        assert_eq!(chart_left(500.0, 800.0, 1.0), 468.0);
        assert_eq!(chart_left(10.0, 800.0, 1.0), 0.0);
        assert_eq!(chart_left(2000.0, 800.0, 1.0), 480.0);
        // 缩放后 32/320 两个原版常量跟着 u 走。
        assert_eq!(chart_left(500.0, 2000.0, 2.0), 436.0);
        assert_eq!(chart_left(3000.0, 800.0, 2.0), 160.0);
        // 上加线：key ≤ 34 循环空转，35 起每 2 个键多一条。
        assert_eq!(upper_ledger_bound(33), 1);
        assert_eq!(upper_ledger_bound(34), 1);
        assert_eq!(upper_ledger_bound(35), 2);
        assert_eq!(upper_ledger_bound(51), 10);
        // 下加线：key ≥ 13 就没有；key 0 画满 7 条。
        assert_eq!(lower_ledger_bound(0), 7);
        assert_eq!(lower_ledger_bound(11), 1);
        assert_eq!(lower_ledger_bound(12), 1);
        assert_eq!(lower_ledger_bound(13), 0);
        assert_eq!(lower_ledger_bound(33), -10);
    }
}
