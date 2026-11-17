//! 全局快捷键。原版 `control_draw.gml:708-924` 那一整段键盘判定被
//! `window = 0 && text_focus = -1` 包着：窗口开着或光标停在输入框里时，编辑器按键
//! 一律不响应（窗口自己只认 ←/→ 翻页签、Escape、Ctrl+P）。其中 Ctrl+ 字母、Delete、
//! Home/End 又再被 `playing = 0` 包一层，空格 / Enter / F10 / F11 不受此限。
//!
//! 与原版的已知差异：PageUp/PageDown 原版是把播放头推到曲尾 / 曲首（要先用 Tab 把标尺
//! 焦点切过去 `sb_sel`），我们按视口翻页，标尺焦点没实现。
//! F1 原版是打开教学视频链接，这里指向本端口自建的「快捷键一览」窗口。
//! Escape 在原版只有三处：键位重设框取消（`control_draw.gml:87`）、首选项关窗
//! （`draw_window_preferences.gml:596`，取松开沿）、设置速度关窗
//! （`draw_window_set_tempo.gml:47`）——原版编辑器本身没有「按 Escape 取消选择」。
//! 这里的取消选择是跟着本端口「编辑 → 取消选择」菜单项一起加的额外行为，不是原版按键。

use egui::{Context, Event, Key};

use nbs_core::Macro;

use crate::commands::Command;
use crate::state::EditorState;
use crate::state::{EditMode, Flag, WindowKind};
use crate::theme::ThemeKind;
use crate::tr;

/// 原版 `control_draw.gml:822-843` 的数字快捷键只覆盖前 16 个乐器：
/// Ctrl+1..9 → 0..8，Ctrl+Shift+1..7 → 9..15。v6 新增的铜块一族（16..19）
/// 在原版里没有按键，所以这里不绑，`instruments_window` 也不给它们印提示。
pub const INSTRUMENT_HOTKEYS: usize = 16;

/// 本帧有没有「带着 Ctrl 按下某个键」。
///
/// 不能直接读 `i.modifiers`：那是 egui-winit 在帧首拍的键盘快照，Ctrl 的按下和松开挤在
/// 同一帧时（帧率上限 30，人手快速点一下就是这样）快照里 Ctrl 已经松开，快捷键会静默失灵。
/// 每个 `Event::Key` 自带按下那一刻的修饰位，所以取「本帧被按下的那些键」上的修饰位，
/// 没有按下事件时退回快照，按住不放连按的情况也照样成立。
pub fn ctrl_chord(i: &egui::InputState) -> bool {
    let mut ctrl = i.modifiers.ctrl || i.modifiers.command;
    for e in &i.events {
        if let Event::Key {
            pressed: true,
            modifiers,
            ..
        } = e
        {
            ctrl |= modifiers.ctrl || modifiers.command;
        }
    }
    ctrl
}

/// 同上，Shift 位。
pub fn shift_chord(i: &egui::InputState) -> bool {
    let mut shift = i.modifiers.shift;
    for e in &i.events {
        if let Event::Key {
            pressed: true,
            modifiers,
            ..
        } = e
        {
            shift |= modifiers.shift;
        }
    }
    shift
}

pub fn collect_shortcuts(state: &EditorState, ctx: &Context) -> Vec<Command> {
    // 有文本输入焦点或有窗口在最上层时，把键盘让给它们。
    let typing = ctx.memory(|m| m.focused().is_some());
    ctx.input(|i| {
        let mut out = Vec::new();
        if state.window.is_some() || typing {
            return out;
        }
        let ctrl = ctrl_chord(i);
        let shift = shift_chord(i);
        let p = |k: Key| i.key_pressed(k);

        // 系统剪贴板粘贴（可能来自其它程序的 OpenNBS selection_code 文本）
        for e in &i.events {
            if let Event::Paste(text) = e {
                out.push(Command::PasteText(text.clone()));
            }
        }

        if p(Key::Escape) {
            if !state.doc.selection.is_empty() {
                out.push(Command::Deselect);
            }
            return out;
        }

        // ---- 不受播放状态限制 ----
        if p(Key::Space) {
            out.push(Command::PlayPause);
        }
        // 原版 Enter：`playing = 0; marker_pos = 0`，等价于停止并回到开头。
        if p(Key::Enter) {
            out.push(Command::Stop);
        }
        if p(Key::F1) {
            out.push(Command::ShowWindow(WindowKind::Shortcuts));
        }
        if p(Key::F10) {
            out.push(Command::ToggleFlag(Flag::Blackout));
        }
        if p(Key::F11) {
            out.push(Command::ToggleFullscreen);
        }
        // 原版 720 行：Ctrl+Shift+F5 仅 theme 3，开/关彩虹。开只发消息，
        // 关再 draw_accent_init（消息在 toggle_rainbow 里，原版 language=0 的
        // 英文文案是 "Rainbow mode => ON/OFF"，界面沿用中文分支）。
        if p(Key::F5) && ctrl && shift && state.theme.kind == ThemeKind::Fluent {
            out.push(Command::ToggleRainbow);
        }

        // ---- 以下要求停止播放（原版 709 行 `if (playing = 0)`）----
        if !state.is_playing() {
            if p(Key::Delete) || p(Key::Backspace) {
                out.push(Command::Delete);
            }
            if p(Key::Home) {
                out.push(Command::SeekHome);
            }
            if p(Key::End) {
                out.push(Command::SeekEnd);
            }
            if p(Key::PageUp) {
                out.push(Command::PageHorizontal(-1));
                out.push(Command::PageVertical(-1));
            }
            if p(Key::PageDown) {
                out.push(Command::PageHorizontal(1));
                out.push(Command::PageVertical(1));
            }
            if ctrl && !shift {
                if p(Key::N) {
                    out.push(Command::New);
                }
                if p(Key::O) {
                    out.push(Command::Open);
                }
                if p(Key::S) {
                    out.push(Command::Save);
                }
                if p(Key::A) {
                    out.push(Command::SelectAll);
                }
                if p(Key::I) {
                    out.push(Command::InvertSelection);
                }
                if p(Key::C) {
                    out.push(Command::Copy);
                }
                if p(Key::X) {
                    out.push(Command::Cut);
                }
                if p(Key::V) {
                    out.push(Command::Paste);
                }
                if p(Key::Z) {
                    out.push(Command::Undo);
                }
                if p(Key::Y) {
                    out.push(Command::Redo);
                }
                if p(Key::P) {
                    out.push(Command::ShowWindow(WindowKind::Preferences));
                }
                if p(Key::Num0) {
                    out.push(Command::WindowScaleReset);
                }
                if p(Key::Plus) || p(Key::Equals) {
                    out.push(Command::WindowScaleUp);
                }
                if p(Key::Minus) {
                    out.push(Command::WindowScaleDown);
                }
                // 原版把滚轮和 187/189（+ / -）写在同一个条件里：`mouse_wheel_up()` 也算放大。
                if ctrl {
                    let wheel = i.raw_scroll_delta.y;
                    if wheel > 0.0 {
                        out.push(Command::WindowScaleUp);
                    } else if wheel < 0.0 {
                        out.push(Command::WindowScaleDown);
                    }
                }

                // `mode_action(1..4)`：E / D 走大步，R / F 走 ±1。音高模式的大步是一个
                // 八度（12 半音），力度 / 声像 / 微调三种模式源码里都写的是 10。
                let (big, small) = match state.tool {
                    EditMode::Key => (12, 1),
                    EditMode::Velocity | EditMode::Panning | EditMode::Pitch => (10, 1),
                };
                let step_cmd = |delta: i32| -> Command {
                    match state.tool {
                        EditMode::Key => Command::SetKey(delta),
                        EditMode::Velocity => Command::SetVel(delta),
                        EditMode::Panning => Command::SetPan(delta),
                        EditMode::Pitch => Command::SetPit(delta),
                    }
                };
                if p(Key::E) {
                    out.push(step_cmd(big));
                }
                if p(Key::D) {
                    out.push(step_cmd(-big));
                }
                if p(Key::R) {
                    out.push(step_cmd(small));
                }
                if p(Key::F) {
                    out.push(step_cmd(-small));
                }
                // `mode_action(5)`：按当前模式打开批量设置窗口。原版调用点写死
                // `editmode != m_key`：音高模式（Key）没有 T/G；微调模式有，
                // 打开的是 w_setpitch（v3.12 的 mode_action 已补齐 m_pit 分支）。
                let batch_window = match state.tool {
                    EditMode::Key => None,
                    EditMode::Velocity => Some(WindowKind::SetVelocity),
                    EditMode::Panning => Some(WindowKind::SetPanning),
                    EditMode::Pitch => Some(WindowKind::SetPitch),
                };
                if p(Key::T) {
                    if let Some(kind) = batch_window {
                        out.push(Command::ShowWindow(kind));
                    }
                }
                // `mode_action(6)`：把当前模式的属性恢复默认（力度 / 声像 100、微调 0）。
                // 空选区时 selection_change 直接 return，这里靠 batch_change 的
                // 空选区早退对齐。
                if p(Key::G) {
                    match state.tool {
                        EditMode::Key => {}
                        EditMode::Velocity => out.push(Command::BatchVelocity(100)),
                        EditMode::Panning => out.push(Command::BatchPanning(100)),
                        EditMode::Pitch => out.push(Command::BatchPitch(0)),
                    }
                }
            }
        }

        // ---- Ctrl + 数字 / Ctrl + Shift + 字母：原版不在 `playing = 0` 里 ----
        if ctrl {
            let digits = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            if !shift {
                for (idx, k) in digits.iter().enumerate() {
                    if p(*k) {
                        out.push(Command::SetInstrument(idx as u8));
                    }
                }
            } else {
                // 原版 `instrument_list[| a + 8]`，a 只到 7 → 索引 9..15。
                for (idx, k) in digits.iter().enumerate().take(7) {
                    if p(*k) {
                        out.push(Command::SetInstrument((9 + idx) as u8));
                    }
                }
            }
            // 原版的 Ctrl+Shift+ 字母：宏窗口、批量设置窗口和 7 个直接执行的宏。
            // 这一族（A/S/D/F/G/H/J/K/Q/W/E/R/T/Y/U）整段包在
            // `if current_song.selected != 0`（control_draw.gml:775）里，没有选区时
            // 按键无效果；片段编辑器 Ctrl+Shift+P（同文件 765-769）和速度测量器
            // Ctrl+Shift+I（770-773）在闸门外。
            // 每条第一句都是 `playing = 0`，所以走 HotkeyWindow / HotkeyMacro，
            // 不是 ShowWindow / RunMacro。
            // 琶音 D / 滑音 F / 交错 H 这三个窗口本端口还没实现，不占用按键。
            // 每一行原版都写死 `keyboard_check(vk_shift)`：这一族必须带 Shift，
            // 否则 Ctrl+P/S/A/R/Y 会和上面 `ctrl && !shift` 那批功能撞车。
            if shift {
                if p(Key::P) {
                    out.push(Command::HotkeyWindow(WindowKind::ClipEditor));
                }
                if p(Key::I) {
                    out.push(Command::HotkeyWindow(WindowKind::TempoTapper));
                }
                if !state.doc.selection.is_empty() {
                    if p(Key::A) {
                        out.push(Command::HotkeyWindow(WindowKind::Tremolo));
                    }
                    if p(Key::S) {
                        out.push(Command::HotkeyWindow(WindowKind::Stereo));
                    }
                    // 原版直接执行的 7 个宏（control_draw.gml:792-834）。同一段里
                    // 的 D 琶音 / F 滑音 / H 交错是带参数的窗口，本端口没有这两个
                    // 窗口，不占用按键。
                    if p(Key::G) {
                        out.push(Command::HotkeyMacro(Macro::Vibrato));
                    }
                    if p(Key::J) {
                        // 原版 macro_chorus() 里写死 +5 / -5。
                        out.push(Command::HotkeyMacro(Macro::Chorus { detune: 5 }));
                    }
                    if p(Key::K) {
                        out.push(Command::HotkeyMacro(Macro::VelocityLfo));
                    }
                    if p(Key::Q) {
                        out.push(Command::HotkeyMacro(Macro::FadeIn));
                    }
                    if p(Key::W) {
                        out.push(Command::HotkeyMacro(Macro::FadeOut));
                    }
                    if p(Key::E) {
                        out.push(Command::HotkeyMacro(Macro::ReplaceKey));
                    }
                    if p(Key::R) {
                        out.push(Command::HotkeyWindow(WindowKind::SetVelocity));
                    }
                    if p(Key::T) {
                        out.push(Command::HotkeyWindow(WindowKind::SetPanning));
                    }
                    if p(Key::Y) {
                        out.push(Command::HotkeyWindow(WindowKind::SetPitch));
                    }
                    if p(Key::U) {
                        out.push(Command::HotkeyMacro(Macro::Reset));
                    }
                }
            }
        }

        out
    })
}

/// 快捷键说明（帮助窗口用）。只列本端口真正接线的按键，鼠标行为同理。
/// 这是 port 自有的帮助页，EN 文案自拟。
pub fn shortcut_help() -> Vec<(&'static str, &'static str)> {
    [
        (
            "Left click",
            "左键单击",
            "Place / overwrite notes",
            "放置 / 覆盖音符",
        ),
        (
            "Left drag",
            "左键拖动",
            "Box-select (notes enter the floating selection)",
            "框选（音符进入浮动选区）",
        ),
        (
            "Drag selected notes",
            "左键拖选中音符",
            "Move the floating selection",
            "移动浮动选区",
        ),
        (
            "Right-click note",
            "右键单击音符",
            "Delete note",
            "删除音符",
        ),
        (
            "Right-click empty space",
            "右键单击空格",
            "Context menu",
            "上下文菜单",
        ),
        (
            "Right drag",
            "右键拖动",
            "Subtract from the selection",
            "从选区减选",
        ),
        (
            "Middle click / Ctrl+middle",
            "中键单击 / Ctrl+中键",
            "Eyedropper (key & instrument / all properties)",
            "吸管（音高音色 / 全部属性）",
        ),
        ("Middle drag", "中键拖动", "Pan the canvas", "平移画布"),
        (
            "Alt + box-select",
            "Alt + 框选",
            "Copy the boxed area without lifting the original notes",
            "复制框选，不摘出原音符",
        ),
        (
            "Ctrl + click",
            "Ctrl + 单击",
            "Add a single note to the selection",
            "单点加选",
        ),
        (
            "Ctrl + wheel",
            "Ctrl + 滚轮",
            "Whole-window zoom (original steps 0.5..4)",
            "整窗口缩放（原版档位 0.5..4）",
        ),
        (
            "Wheel",
            "滚轮",
            "Pan viewport / change hovered note's property",
            "平移视口 / 悬停音符时改属性",
        ),
        (
            "Shift + wheel",
            "Shift + 滚轮",
            "Pan horizontally",
            "横向平移",
        ),
        (
            "Ctrl+N/O/S",
            "Ctrl+N/O/S",
            "New / open / save",
            "新建 / 打开 / 保存",
        ),
        (
            "Ctrl+C/X/V",
            "Ctrl+C/X/V",
            "Copy / cut / paste",
            "复制 / 剪切 / 粘贴",
        ),
        (
            "Ctrl+Z / Ctrl+Y",
            "Ctrl+Z / Ctrl+Y",
            "Undo / redo",
            "撤销 / 重做",
        ),
        (
            "Ctrl+A / Ctrl+I",
            "Ctrl+A / Ctrl+I",
            "Select all / invert",
            "全选 / 反选",
        ),
        (
            "Ctrl+E / D",
            "Ctrl+E / D",
            "Current mode ±large step (pitch ±12, others ±10)",
            "按当前模式 ±大步（音高 ±12，其余 ±10）",
        ),
        (
            "Ctrl+R / F",
            "Ctrl+R / F",
            "Current mode ±1",
            "按当前模式 ±1",
        ),
        (
            "Ctrl+T / Ctrl+G",
            "Ctrl+T / Ctrl+G",
            "Batch-set window / reset current property (none in pitch mode; needs a selection)",
            "批量设置窗口 / 当前属性回默认（音高模式无，需选区才生效）",
        ),
        ("Ctrl+P", "Ctrl+P", "Preferences", "首选项"),
        (
            "Ctrl+1..9",
            "Ctrl+1..9",
            "Pick instruments 1-9 (velocity / panning / pitch reset + preview)",
            "选前 9 种乐器（力度 / 声像 / 微调复位并试听）",
        ),
        (
            "Ctrl+Shift+1..7",
            "Ctrl+Shift+1..7",
            "Pick instruments 10-16",
            "选第 10..16 种乐器",
        ),
        ("Ctrl+Shift+P", "Ctrl+Shift+P", "Clip editor", "片段编辑器"),
        (
            "Ctrl+Shift+I",
            "Ctrl+Shift+I",
            "Tempo tapper (tap T to measure the tempo)",
            "速度测量器（按 T 打点测速）",
        ),
        (
            "Ctrl+Shift+R / T / Y",
            "Ctrl+Shift+R / T / Y",
            "Batch-set velocity / panning / pitch (needs a selection, no-op without one)",
            "批量设置力度 / 声像 / 微调（需选区，无选区按键无效）",
        ),
        (
            "Ctrl+Shift+A / S",
            "Ctrl+Shift+A / S",
            "Tremolo / stereo macro windows (needs a selection, no-op without one)",
            "颤音 / 立体声宏窗口（需选区，无选区按键无效）",
        ),
        (
            "Ctrl+Shift+G / J / K / Q / W / E / U",
            "Ctrl+Shift+G / J / K / Q / W / E / U",
            "Run vibrato / chorus / volume LFO / fade in / fade out / replace key / reset \
             properties (needs a selection)",
            // 原版中文菜单前 8 个宏名也写英文（draw_macrobutton 的 language 分支），照抄。
            "Vibrato / Chorus / Volume LFO / 淡入 / 淡出 / 替换音 / 重置所有属性（需选区）",
        ),
        (
            "Ctrl+0 / Ctrl+±",
            "Ctrl+0 / Ctrl+±",
            "Reset / adjust whole-window zoom",
            "重置 / 调整整窗口缩放",
        ),
        (
            "Delete",
            "Delete",
            "Delete selection (while playback is stopped)",
            "删除选区（停止播放时）",
        ),
        (
            "Space / Enter",
            "Space / Enter",
            "Play-pause / stop and rewind to start",
            "播放暂停 / 停止并回到开头",
        ),
        (
            "Hold ← / →",
            "按住 ← / →",
            "Scroll left / right (same as the rewind / fast-forward buttons)",
            "向左 / 向右滚动（同底部快退 / 快进按钮）",
        ),
        (
            "Home / End",
            "Home / End",
            "Playhead to start / end",
            "播放头到开头 / 结尾",
        ),
        (
            "F10 / F11",
            "F10 / F11",
            "Blackout mode / fullscreen",
            "全黑模式 / 全屏",
        ),
        (
            "Ctrl+Shift+F5",
            "Ctrl+Shift+F5",
            "Rainbow mode toggle (Fluent theme only)",
            "炫彩模式开关（仅 Fluent 主题）",
        ),
        ("F1", "F1", "Open this page", "打开本页"),
        ("← / → in window", "窗口内 ← / →", "Switch tabs", "切换页签"),
        (
            "Escape",
            "Escape",
            "Close the preferences window / cancel selection",
            "关闭首选项窗口 / 取消选择",
        ),
    ]
    .into_iter()
    .map(|(k_en, k_zh, d_en, d_zh)| (tr(k_en, k_zh), tr(d_en, d_zh)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一次「Ctrl+P 点一下」在 egui 里长这样：四个事件全挤在同一帧，
    /// 而 `RawInput::modifiers` 是 egui-winit 帧首按当时键盘状态快照的，此刻 Ctrl 已松开。
    fn key(k: Key, pressed: bool, m: egui::Modifiers) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: m,
        }
    }

    fn prefs_open_cmd(c: &Command) -> bool {
        matches!(c, Command::ShowWindow(WindowKind::Preferences))
    }

    /// 出厂开机流程会打开欢迎窗，而窗口开着快捷键全被门槛拦掉；
    /// 快捷键测试的靶子是裸编辑器，这里把欢迎窗关掉。
    fn fresh_state() -> EditorState {
        let mut s = EditorState::new();
        s.window = None;
        s
    }

    fn cmds(events: Vec<Event>) -> Vec<Command> {
        let ctx = Context::default();
        let state = fresh_state();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 0.0,
            events,
            ..Default::default()
        };
        let mut out = Vec::new();
        let _ = ctx.run(raw, |c| out = collect_shortcuts(&state, c));
        out
    }

    /// 快速点按 Ctrl+P 也要开首选项 —— 帧率上限 30 时人手点快了就是这样一帧。
    #[test]
    fn fast_ctrl_tap_still_opens_preferences() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        // Ctrl 本身不产生 Key 事件（egui 的 Key 没有修饰键变体），修饰状态只体现在
        // 每个事件的 modifiers 和帧首的 RawInput::modifiers 快照上；快速点按时后者已经清零。
        let got = cmds(vec![key(Key::P, true, ctrl), key(Key::P, false, ctrl)]);
        assert!(
            got.iter().any(prefs_open_cmd),
            "Ctrl+P 快速点按被当成裸 P 丢掉了: {got:?}"
        );
    }

    /// 反过来不能松：裸按 P 不许触发 Ctrl+P。
    #[test]
    fn bare_key_does_not_match_a_ctrl_chord() {
        let got = cmds(vec![key(Key::P, true, egui::Modifiers::NONE)]);
        assert!(!got.iter().any(prefs_open_cmd), "裸 P 不该开首选项");
    }

    /// 原版 `control_draw.gml:770-773`：Ctrl+Shift+I 先 `playing = 0` 再开速度测量器，
    /// 和片段编辑器一样在选区闸门外；不带 Shift 的 Ctrl+I 不算这档。
    #[test]
    fn ctrl_shift_i_opens_the_tempo_tapper_without_a_selection() {
        let both = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let tapper = |c: &Command| {
            matches!(c, Command::HotkeyWindow(WindowKind::TempoTapper))
        };
        let got = cmds(vec![key(Key::I, true, both)]);
        assert!(got.iter().any(tapper), "Ctrl+Shift+I 应开速度测量器: {got:?}");
        let got = cmds(vec![key(Key::I, true, ctrl)]);
        assert!(!got.iter().any(tapper), "Ctrl+I 不该开速度测量器: {got:?}");
    }

    /// 原版 `control_draw.gml:765-831` 每一行都带 `keyboard_check(vk_shift)`：
    /// Ctrl+P 只开首选项，片段编辑器必须是 Ctrl+Shift+P。
    #[test]
    fn ctrl_window_keys_do_not_collide_with_plain_ctrl() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let both = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let clip = |c: &Command| matches!(c, Command::HotkeyWindow(WindowKind::ClipEditor));
        let stereo = |c: &Command| matches!(c, Command::HotkeyWindow(WindowKind::Stereo));
        let got = cmds(vec![key(Key::P, true, ctrl)]);
        assert!(got.iter().any(prefs_open_cmd), "Ctrl+P 应开首选项: {got:?}");
        assert!(!got.iter().any(clip), "Ctrl+P 不该同时开片段编辑器: {got:?}");
        // Ctrl+S 是保存，不能被立体声宏窗口抢走。
        let got = cmds(vec![key(Key::S, true, ctrl)]);
        assert!(got.iter().any(|c| matches!(c, Command::Save)), "Ctrl+S 应保存: {got:?}");
        assert!(!got.iter().any(stereo), "Ctrl+S 不该开立体声窗口: {got:?}");
        let got = cmds(vec![key(Key::P, true, both)]);
        assert!(got.iter().any(clip), "Ctrl+Shift+P 应开片段编辑器: {got:?}");
        assert!(
            !got.iter().any(prefs_open_cmd),
            "Ctrl+Shift+P 不该开首选项: {got:?}"
        );
    }

    /// 原版 `control_draw.gml:792-834`：同一段里的 7 个直接执行的宏各占一键
    /// （G 颤音 / J 合唱 / K 音量 LFO / Q 淡入 / W 淡出 / E 替换音 / U 重置属性），
    /// 同样在 `if current_song.selected != 0` 闸门里。端口此前一键都没接。
    #[test]
    fn ctrl_shift_direct_macro_hotkeys() {
        let both = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let want: [(Key, Macro); 7] = [
            (Key::G, Macro::Vibrato),
            (Key::J, Macro::Chorus { detune: 5 }),
            (Key::K, Macro::VelocityLfo),
            (Key::Q, Macro::FadeIn),
            (Key::W, Macro::FadeOut),
            (Key::E, Macro::ReplaceKey),
            (Key::U, Macro::Reset),
        ];
        let mut sel = fresh_state();
        sel.doc.place_note(0, 0, nbs_core::model::Note::new(0, 45));
        sel.doc.place_note(1, 0, nbs_core::model::Note::new(0, 47));
        sel.doc.select_rect(0, 0, 1, 0, false, false, false);
        assert!(!sel.doc.selection.is_empty(), "测试前提：框选要摘出音符");
        for (k, m) in &want {
            let got = cmds_with(vec![key(*k, true, both)], &sel);
            assert!(
                got.iter()
                    .any(|c| matches!(c, Command::HotkeyMacro(g) if *g == *m)),
                "Ctrl+Shift+{k:?} 应派发 {m:?}，实得 {got:?}"
            );
        }
        // 闸门另一侧：没有选区时这 7 个键一个都不该动。
        let empty = fresh_state();
        for (k, _) in &want {
            let got = cmds_with(vec![key(*k, true, both)], &empty);
            assert!(
                !got.iter().any(|c| matches!(c, Command::HotkeyMacro(_))),
                "无选区时 Ctrl+Shift+{k:?} 不该跑宏: {got:?}"
            );
        }
    }

    /// 原版 `control_draw.gml` 的 Macro Hotkeys 区整段包在
    /// `if current_song.selected != 0` 里：Ctrl+Shift+R/T/Y/A/S 没有选区时必须
    /// 无效果，而 Ctrl+Shift+P（片段编辑器）在闸门外，空选区照样打开。
    #[test]
    fn ctrl_shift_macro_keys_require_a_selection() {
        let both = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let empty = fresh_state();
        let got = cmds_with(
            vec![
                key(Key::R, true, both),
                key(Key::T, true, both),
                key(Key::Y, true, both),
                key(Key::A, true, both),
                key(Key::S, true, both),
            ],
            &empty,
        );
        assert!(
            !got.iter().any(|c| matches!(
                c,
                Command::HotkeyWindow(
                    WindowKind::SetVelocity
                        | WindowKind::SetPanning
                        | WindowKind::SetPitch
                        | WindowKind::Tremolo
                        | WindowKind::Stereo
                )
            )),
            "无选区时宏族按键必须全部无效: {got:?}"
        );
        // 片段编辑器不受闸门限制。
        let got = cmds_with(vec![key(Key::P, true, both)], &empty);
        assert!(
            got.iter()
                .any(|c| matches!(c, Command::HotkeyWindow(WindowKind::ClipEditor))),
            "Ctrl+Shift+P 不该受选区闸门限制: {got:?}"
        );

        // 框一个有音符的矩形后，同一批按键全部放行。
        let mut sel = fresh_state();
        sel.doc.place_note(0, 0, nbs_core::model::Note::new(0, 45));
        sel.doc.place_note(1, 0, nbs_core::model::Note::new(0, 47));
        sel.doc.select_rect(0, 0, 1, 0, false, false, false);
        assert!(!sel.doc.selection.is_empty(), "测试前提：框选要摘出音符");
        let got = cmds_with(
            vec![
                key(Key::R, true, both),
                key(Key::T, true, both),
                key(Key::Y, true, both),
                key(Key::A, true, both),
                key(Key::S, true, both),
            ],
            &sel,
        );
        for want in [
            WindowKind::SetVelocity,
            WindowKind::SetPanning,
            WindowKind::SetPitch,
            WindowKind::Tremolo,
            WindowKind::Stereo,
        ] {
            assert!(
                got.iter()
                    .any(|c| matches!(c, Command::HotkeyWindow(k) if *k == want)),
                "有选区时应打开 {want:?}: {got:?}"
            );
        }
    }

    /// 自带状态的 cmds 变体（theme 闸门要切主题才能测）。
    fn cmds_with(events: Vec<Event>, state: &EditorState) -> Vec<Command> {
        let ctx = Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 0.0,
            events,
            ..Default::default()
        };
        let mut out = Vec::new();
        let _ = ctx.run(raw, |c| out = collect_shortcuts(state, c));
        out
    }

    /// 原版 `control_draw.gml:720`：Ctrl+Shift+F5 的判定里带 `theme = 3`，
    /// 非 Fluent 主题这个组合必须无效。
    #[test]
    fn ctrl_shift_f5_toggles_rainbow_only_on_fluent() {
        let both = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let got = cmds(vec![key(Key::F5, true, both)]);
        assert!(
            got.iter().any(|c| matches!(c, Command::ToggleRainbow)),
            "Fluent（出厂主题）下应触发: {got:?}"
        );

        let mut state = fresh_state();
        state.theme = crate::theme::Theme::new(crate::theme::ThemeKind::Dark);
        let got = cmds_with(vec![key(Key::F5, true, both)], &state);
        assert!(
            !got.iter().any(|c| matches!(c, Command::ToggleRainbow)),
            "暗黑主题不该触发: {got:?}"
        );
        // 裸 F5 不算（原版要求 Ctrl+Shift 都按住）。
        let got = cmds(vec![key(Key::F5, true, egui::Modifiers::NONE)]);
        assert!(
            !got.iter().any(|c| matches!(c, Command::ToggleRainbow)),
            "{got:?}"
        );
    }
}
