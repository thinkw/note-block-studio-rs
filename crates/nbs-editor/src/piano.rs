//! 底部钢琴键盘 —— 照原版 `draw_piano` / `draw_piano_key` 逐帧贴 `spr_piano`：
//! 白键 0、黑键背面 2 / 正面 1；每列先画白键再画黑键（黑键压在上面）。
//! 白键固定 39px 步进、整排按窗口宽度居中，鼠标停在最左 / 最右白键上每 0.1 秒平移一个音
//! （原版 `d` + `delay`）。`spr_piano` 资源缺失时整排不画 —— 原版没有「无贴图键盘」，
//! 所以这里也不画一个假的出来。
//! 端口自加（用户要求）：① 面板独立缩放，`u = prefs.piano_scale` 乘所有原版像素常量；
//! ② 去掉原版 `draw_piano.gml:12/119-120` 的两侧渐隐蒙版（alpha 机制），恒全亮度
//! ——原版播放器模式（isplayer）本来就不渐隐。

use egui::{pos2, Align2, Color32, Context, LayerId, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use nbs_core::model::{key_name, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::commands::Command;
// 琴键像素常量与几何全部来自 `fx::keyboard`：特效的下落道要和这条键盘用同一套
// 横坐标，两边各留一份迟早会飘（见该模块的说明）。
use crate::fx::keyboard::{KeyboardLayout, BLACK_DRAW_X, BLACK_LIFT, KEY_W, WHITE_SLOTS, ART_H};
use crate::fonts::{bold_font, main_font};
use crate::icons::{piano_frame, Bank};
use crate::state::EditorState;
use crate::theme::{Theme, ThemeKind};
use crate::viewport::PIANO_HEIGHT;

/// 琴键面板高度：`viewport::PIANO_HEIGHT` 之外还要算上黑键高出的 `BLACK_LIFT`。
/// 中央面板（网格 + 横向滚动条）晚于琴键绘制，黑键越界画到面板外只会被滚动条底色
/// 盖住（用户报告 #109）；面板长高这 7px、白键顶边同步下移，键盘可见位置不变。
pub const PANEL_HEIGHT: f32 = PIANO_HEIGHT + BLACK_LIFT;
/// 原版 `if (key_midipress || key_press || key_click) down = 0.75`：实时按下时的下沉
/// 倍率，画出来是 `floor(7 * 0.75)` = 5px。
const PRESS_RATIO: f32 = 0.75;
/// 原版 `floor(7 * ispressed)` 里的 7：下沉倍率换成像素的系数。
const PRESS_PX: f32 = 7.0;
/// 原版 `key_played[c]` 的窗口：被演奏后 300 毫秒内染黄并弹一下。
const PLAYED_WINDOW: f64 = 0.3;
/// 原版 `c_yellow`（$00FFFF 按 BGR 存 = RGB(255,255,0)）。
const YELLOW: Color32 = Color32::from_rgb(255, 255, 0);
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
/// 原版 `key_played[0..=87]`：键号 → 最近一次被演奏的时刻（秒，`i.time`）。
/// 用 `NEG_INFINITY` 表示「从没响过」：原版默认 0 是靠 `current_time` 一直往后涨来
/// 保证过期的，而无头渲染里 `i.time` 恒为 0，写 0.0 会让整排键第一帧全亮。
pub type KeysPlayed = [f64; KEY_MAX as usize + 1];
/// 一个键都没被演奏过。
pub const NO_KEYS_PLAYED: KeysPlayed = [f64::NEG_INFINITY; KEY_MAX as usize + 1];

/// 原版 `draw_piano.gml:67/80/114` 那一段：`elapsed` 是该键被奏响后过了多少秒。
/// 窗口内返回 `(染黄比例, 下沉倍率)`，过期返回 `None`。
/// - 染黄 = `merge_color(col, c_yellow, 1 - elapsed/0.3)`：刚响完最黄，线性退回本色。
/// - 下沉 = `t*2` 超过 1 就折返成 `2-t*2`，再量化成 `floor(d*3)/3` —— 只有 0、1/3、
///   2/3、1 四档，配 `floor(7*d)` 就是 0、2、4、7 像素：键弹下去再回上来。
pub fn played_key(elapsed: f64) -> Option<(f32, f32)> {
    if !(0.0..PLAYED_WINDOW).contains(&elapsed) {
        return None;
    }
    let t = (elapsed / PLAYED_WINDOW) as f32;
    let mut down = t * 2.0;
    if down > 1.0 {
        down = 2.0 - down;
    }
    Some((1.0 - t, (down * 3.0).floor() / 3.0))
}

/// 原版 `merge_color(c1, c2, ratio)`：逐通道线性插值，ratio 0 取 c1、1 取 c2。
fn merge_color(a: Color32, b: Color32, ratio: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * ratio).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
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
/// `tint` 是 [`played_key`] 给的染黄比例（0 = 不在窗口内）：原版在超范围/选中之后
/// 才 `merge_color(col, c_yellow, ratio)`，所以刚演奏的键即便在音域外也是往黄里走。
fn key_color(key: u8, o: &DrawOpts, th: &Theme, tint: f32) -> (Color32, bool) {
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
    if tint > 0.0 {
        col = merge_color(col, YELLOW, tint);
    }
    (col, out)
}

/// 悬停琴键这一帧要画的五线谱参数；`show_piano` 只登记，真正落笔延到中央面板
/// 之后（#112，见 `paint_notechart`）。
#[derive(Clone, Copy)]
pub struct ChartRequest {
    /// 琴键面板宽（原版 `rw`），用来算面板左缘和「贴右缘」判定。
    rw: f32,
    /// 原版传给 `draw_notechart` 的横坐标：白键取列左缘 +18，黑键取列左缘。
    x1: f32,
    /// 白键顶边（原版 `yy` 由它减 32 得到）。
    y0: f32,
    /// 列号 `startkey + a`（黑键升号再减 1），不是实际键号 —— 原版如此。
    key: i32,
    accidental: bool,
    flats: bool,
    u: f32,
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

pub fn show_piano(state: &mut EditorState, ui: &mut Ui) -> Option<ChartRequest> {
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
    ui.allocate_rect(area, Sense::hover());

    let rw = area.width();
    // 偏离原版（用户要求）：琴键横向占满面板——去掉原版 `keysmax`（首选项
    // 「同屏显示的琴键数」，20..50）的截短与 32px 内缩，显示到放不下为止，
    // 上限 = 完整键盘 52 个白键槽（startkey 27-52/2=1，即 A0 起）。
    let keysshow = WHITE_SLOTS.min(((rw) / (KEY_W * u)).floor() as usize).max(1);
    if state.piano_keys_shown as usize != keysshow {
        // 原版：窗口宽度变了就把键盘重新以 27 号键为中心。
        // 偏离原版（#110）：中心位再夹进 `[0, 52 - keysshow]`。原式的 `27` 是给
        // `keysmax ≤ 50` 用的，端口去掉 keysmax 后 keysshow 能到 52（= 整台 88 键），
        // 算出 startkey = 1：槽 0 的 A0 永远画不出来，最右一列还落空（用户报告
        // 「缩到较小时低音区缺几个键，挪过去又少高音区几个」）。夹住之后
        // 52 槽时正好是槽 0..51 = MIDI 0..87，一个键都不缺，也无需再平移。
        state.piano_start = (27usize.saturating_sub(keysshow / 2)).min(WHITE_SLOTS - keysshow) as u8;
        state.piano_keys_shown = keysshow as u8;
    }
    let startkey = state.piano_start as usize;
    let x0 = area.min.x + ((rw - keysshow as f32 * KEY_W * u) / 2.0).floor();
    // 白键顶边下移黑键的那 7px：整排键（含黑键高出的一段）都留在面板内，
    // 琴键的可见位置与改之前一致（面板同步长高 7px，见 `PANEL_HEIGHT`）。
    let y0 = area.min.y + BLACK_LIFT * u;
    // 几何交给 `fx::keyboard`：这里的摆放（居中、startkey、缩放）保持原样，
    // 只把「槽 → 键号 / 矩形」的算式换成和特效共用的那一份。
    let layout = KeyboardLayout {
        origin: pos2(x0, y0),
        scale: u,
        first: startkey,
        slots: keysshow,
    };

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
    // 原版 `key_played[]` + 300ms 窗口：这一帧该键还在「刚被演奏」的窗口里就返回
    // (染黄比例, 下沉倍率)。时间戳由音频派发回传、UI 排空时盖章（见 state.rs）。
    let key_played = state.key_played;
    let now = ui.input(|i| i.time);
    let glow = |k: u8| {
        let at = key_played
            .get(k as usize)
            .copied()
            .unwrap_or(f64::NEG_INFINITY);
        played_key(now - at)
    };
    let p = ui.painter();
    let frames = FrameMap::new(&th);
    let mut picked: Option<u8> = None;
    let mut chart: Option<ChartRequest> = None;
    let mut edge = 0i8;

    // 偏离原版（用户要求）：去掉 `draw_piano.gml:12/119-120` 的两侧渐隐
    // （alpha 从 `3 - startkey` 起步、尾部递减），所有琴键恒全亮度——
    // 原版播放器模式（isplayer）本来就不渐隐。
    for a in 0..keysshow {
        let x = layout.slot_x(a);
        // 键号与矩形都由 `layout` 给出：黑键归属于它右侧那个白键的槽，
        // 最左一根白键不画它左边的黑键（原版 `a > 0 && c != 2 && c != 5`）。
        let Some(key) = layout.slot_white_key(a) else {
            continue;
        };
        let black = layout.slot_black_key(a);
        {
            let white_hit = layout.white_hit(a);
            // 原版：白键被左右黑键挡住的部分不算命中；最右一根、以及右边没有黑键的白键除外。
            let left = layout.black_rect(a).is_some_and(|r| inside(r));
            let right = layout.black_rect(a + 1).is_some_and(|r| inside(r));
            let hovering = !left && !right && inside(white_hit);
            // 原版 `draw_piano.gml:66-67`：先看 key_played 的 300ms 窗口（染黄 + 弹下），
            // 再被实时按下覆盖 —— 鼠标按住白键（key_click）或键盘绑定按住（key_press）
            // 把下沉倍率改成 0.75，但覆盖只动 down，染黄留着。像素数 = `floor(7 * down)`。
            let (tint, mut drop) = glow(key).unwrap_or((0.0, 0.0));
            if (hovering && holding) || held(key) {
                drop = PRESS_RATIO;
            }
            let down = (PRESS_PX * drop).floor() * u;
            // 原版在画这根键之前先画五线谱：门是 `c2 > 6 && c2 < 69 && playing = 0`
            // （C/F 分支用 c1，值相同），键号参数是列号 `startkey + a`。没有播放器模式，
            // `!isplayer` 恒真，C/F 分支少的那一个门可以合并。
            if hovering && show_chart && !opts.playing && key > 6 && key < 69 {
                chart = Some(ChartRequest {
                    rw,
                    x1: x + 18.0 * u,
                    y0,
                    key: (startkey + a) as i32,
                    accidental: false,
                    flats: opts.flats,
                    u,
                });
            }
            if let Some(bank) = bank.as_deref() {
                draw_white_key(
                    bank,
                    p,
                    &th,
                    &frames,
                    pos2(x, y0 + down),
                    key,
                    tint,
                    &opts,
                    u,
                );
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

        // 黑键骑在本槽左缘上：`black` 与 `black_rect(a)` 由同一个判定给出
        // （`layout` 只在「左边确有黑键」时返回它们）。
        if let (Some(key), Some(black_hit)) = (black, layout.black_rect(a)) {
            let (tint, mut drop) = glow(key).unwrap_or((0.0, 0.0));
            if (inside(black_hit) && holding) || held(key) {
                drop = PRESS_RATIO;
            }
            // 原版黑键：背面sprite 固定在 `yy - 7`，正面sprite 随 `floor(7 * down)` 下沉。
            let down = (PRESS_PX * drop).floor() * u;
            // 原版黑键五线谱：升号门 `c1 > 6 && c1 < 70`、降号门 `c1 > 5 && c1 < 69`，
            // 键号参数升号写 `startkey + a - 1`、降号写 `startkey + a`（同为列号空间）。
            if inside(black_hit) && show_chart && !opts.playing {
                let (ok, chart_key) = if opts.flats {
                    (key > 5 && key < 69, (startkey + a) as i32)
                } else {
                    (key > 6 && key < 70, (startkey + a) as i32 - 1)
                };
                if ok {
                    chart = Some(ChartRequest {
                        rw,
                        x1: x,
                        y0,
                        key: chart_key,
                        accidental: true,
                        flats: opts.flats,
                        u,
                    });
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
                    tint,
                    &opts,
                    u,
                );
            }
            if inside(black_hit) && pressing {
                picked = Some(key);
            }
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
    if edge != 0 && !holding {
        // 还能不能再往这一侧挪：左到头是槽 0，右到头是槽 `52 - keysshow`。
        let can_pan = (edge < 0 && startkey > 0)
            || (edge > 0 && startkey + keysshow < WHITE_SLOTS);
        if can_pan && state.piano_scroll_delay == 0.0 {
            state.piano_start = (startkey as i32 + if edge < 0 { -1 } else { 1 }) as u8;
            state.piano_scroll_delay = 0.1;
        }
        // 偏离原版（#111）：GameMaker 的 draw 事件每步都跑，egui 只在有输入或自己
        // 约过重绘时出帧。鼠标停在边缘不动就没有新事件，上面那个计时器不推进，
        // 琴键只挪一次卡住（用户报告「得鼠标持续位移才继续移动」）。这里在还能挪
        // 的时候按计时器剩余时间约一次重绘，频率与原版 0.1 秒一步一致。
        if can_pan {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    state.piano_scroll_delay as f64,
                ));
        }
    }
    chart
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
    tint: f32,
    o: &DrawOpts,
    u: f32,
) {
    let (col, out) = key_color(key, o, th, tint);
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
    tint: f32,
    o: &DrawOpts,
    u: f32,
) {
    let (col, out) = key_color(key, o, th, tint);
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

/// #112：悬停五线谱的真正落笔点，由 `app::show` 在中央面板画完之后调用。
/// 面板 / 指针 / 音符都进背景层（`LayerId::background()`），出队顺序晚于网格，
/// 所以五线谱盖在编辑区之上；弹窗是 Area（`Order::Medium`），仍压在五线谱之上
/// ——和原版「先画网格琴键、最后画窗口」的顺序一致。
pub fn paint_notechart(ctx: &Context, state: &EditorState, req: ChartRequest) {
    let Some(bank) = state.icons.bank() else {
        return;
    };
    draw_notechart(ctx, &bank, &state.theme, req);
}

/// 原版 `draw_notechart(x1, yy, key, accidental)`：悬停琴键上的五线谱面板。
/// 原版传入的 `key` 是列号 `startkey + a`（黑键升号再 -1），不是实际悬停键号，
/// 音符位置因此与键名差 `sharpkeys + b` —— 原版如此，逐字复刻。
fn draw_notechart(ctx: &Context, bank: &Bank, th: &Theme, req: ChartRequest) {
    let ChartRequest {
        rw,
        x1,
        y0,
        key,
        accidental,
        flats,
        u,
    } = req;
    let yy = y0 - 32.0 * u;
    let xx = chart_left(x1, rw, u);
    // 剪辑区向上扩到最高键位（startkey + keysshow ≤ 51）的上加线 / 音符顶。
    // 向下只到白键顶边为止：原版是先画五线谱再画这根键，键把越界的一小条压掉，
    // 现在五线谱晚于整排键出队，改成剪到同一条线才能保持同样的可见范围（#112）。
    let p = Painter::new(
        ctx.clone(),
        LayerId::background(),
        Rect::from_min_max(pos2(0.0, y0 - 660.0 * u), pos2(rw, y0)),
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

    /// 原版 `draw_piano.gml:67/80/114` 的 300ms 窗口：染黄从满线性退到本色，
    /// 下沉走「按下去再回上来」的三角波，而且量化成 `floor(d*3)/3` 四档。
    #[test]
    fn played_key_window_and_bounce() {
        assert_eq!(played_key(-0.01), None, "时间戳在未来不算刚演奏");
        assert_eq!(played_key(0.3), None, "正好 300ms 已过期（原版是 <300）");
        assert_eq!(played_key(f64::NEG_INFINITY), None, "从没响过的键");
        // 刚响完：最黄，但还没开始下沉
        let (tint, down) = played_key(0.0).unwrap();
        assert_eq!((tint, down), (1.0, 0.0));
        // 四档下沉：0 → 1/3 → 1（中点按到底）→ 1/3 → 0
        for (elapsed, want) in [
            (0.02_f64, 0.0_f32),
            (0.075, 1.0 / 3.0),
            (0.15, 1.0),
            (0.225, 1.0 / 3.0),
            (0.29, 0.0),
        ] {
            let got = played_key(elapsed).unwrap().1;
            assert!(
                (got - want).abs() < 1e-6,
                "{elapsed}s 的下沉应是 {want}，实测 {got}"
            );
        }
        // 染黄比例 = 1 - elapsed/0.3，单调退色
        let mut prev = 1.0_f32;
        for e in [0.05, 0.1, 0.15, 0.2, 0.25] {
            let tint = played_key(e).unwrap().0;
            assert!(tint < prev, "{e}s 要比上一帧更淡");
            prev = tint;
        }
        assert!(
            (played_key(0.15).unwrap().0 - 0.5).abs() < 1e-6,
            "150ms 染一半"
        );
        // `floor(7 * down)` 的四档位移
        for (down, px) in [
            (0.0f32, 0.0f32),
            (1.0 / 3.0, 2.0),
            (2.0 / 3.0, 4.0),
            (1.0, 7.0),
        ] {
            assert_eq!((7.0 * down).floor(), px);
        }
        // 实时按下覆盖成 0.75 → floor(7*0.75) = 5px
        assert_eq!((7.0 * PRESS_RATIO).floor(), 5.0);
    }

    /// 原版 `merge_color(c1, c2, ratio)`：逐通道线性插值，ratio=0 取 c1、1 取 c2。
    /// `c_yellow` 在 GameMaker 里按 BGR 存 $00FFFF，也就是 RGB(255,255,0)。
    #[test]
    fn merge_color_matches_game_maker() {
        assert_eq!(merge_color(Color32::WHITE, YELLOW, 0.0), Color32::WHITE);
        assert_eq!(merge_color(Color32::WHITE, YELLOW, 1.0), YELLOW);
        assert_eq!(
            merge_color(Color32::WHITE, YELLOW, 0.5),
            Color32::from_rgb(255, 255, 128)
        );
        assert_eq!(
            merge_color(Color32::BLACK, YELLOW, 0.5),
            Color32::from_rgb(128, 128, 0)
        );
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
