//! 全局快捷键。原版 `control_draw.gml:708-924` 那一整段键盘判定被
//! `window = 0 && text_focus = -1` 包着：窗口开着或光标停在输入框里时，编辑器按键
//! 一律不响应（窗口自己只认 ←/→ 翻页签、Escape、Ctrl+P）。其中 Ctrl+ 字母、Delete、
//! Home/End 又再被 `playing = 0` 包一层，空格 / Enter / F10 / F11 不受此限。
//!
//! 与原版的已知差异：原版只有窗口缩放（`window_scale`），没有网格缩放，这里的
//! Ctrl+0 / Ctrl+± / Ctrl+滚轮 走的是本端口的网格缩放；PageUp/PageDown 原版是把播放头
//! 推到曲尾 / 曲首（要先用 Tab 把标尺焦点切过去 `sb_sel`），我们按视口翻页，标尺焦点没实现。
//! F1 原版是打开教学视频链接，这里指向本端口自建的「快捷键一览」窗口。
//! Escape 在原版只有三处：键位重设框取消（`control_draw.gml:87`）、首选项关窗
//! （`draw_window_preferences.gml:596`，取松开沿）、设置速度关窗
//! （`draw_window_set_tempo.gml:47`）——原版编辑器本身没有「按 Escape 取消选择」。
//! 这里的取消选择是跟着本端口「编辑 → 取消选择」菜单项一起加的额外行为，不是原版按键。

use egui::{Context, Event, Key};

use crate::commands::Command;
use crate::state::{EditMode, Flag, WindowKind};
use crate::state::EditorState;

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
        if let Event::Key { pressed: true, modifiers, .. } = e {
            ctrl |= modifiers.ctrl || modifiers.command;
        }
    }
    ctrl
}

/// 同上，Shift 位。
pub fn shift_chord(i: &egui::InputState) -> bool {
    let mut shift = i.modifiers.shift;
    for e in &i.events {
        if let Event::Key { pressed: true, modifiers, .. } = e {
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
                    out.push(Command::ZoomReset);
                }
                if p(Key::Plus) || p(Key::Equals) {
                    out.push(Command::ZoomIn);
                }
                if p(Key::Minus) {
                    out.push(Command::ZoomOut);
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
                // `mode_action(5)`：按当前模式打开批量设置窗口，音高模式没有这一步。
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
                Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5,
                Key::Num6, Key::Num7, Key::Num8, Key::Num9,
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
            // 原版的 Ctrl+Shift+ 字母：宏窗口和批量设置窗口（都要求有选区，
            // 空选区由窗口自己给出提示）。Tempo Tapper / 琶音 / 滑音 / 交错这几个
            // 窗口本端口还没实现，不占用按键。
            // 每一行原版都写死 `keyboard_check(vk_shift)`：这一族必须带 Shift，
            // 否则 Ctrl+P/S/A/R/Y 会和上面 `ctrl && !shift` 那批功能撞车。
            if shift {
                if p(Key::P) {
                    out.push(Command::ShowWindow(WindowKind::ClipEditor));
                }
                if p(Key::R) {
                    out.push(Command::ShowWindow(WindowKind::SetVelocity));
                }
                if p(Key::T) {
                    out.push(Command::ShowWindow(WindowKind::SetPanning));
                }
                if p(Key::Y) {
                    out.push(Command::ShowWindow(WindowKind::SetPitch));
                }
                if p(Key::A) {
                    out.push(Command::ShowWindow(WindowKind::Tremolo));
                }
                if p(Key::S) {
                    out.push(Command::ShowWindow(WindowKind::Stereo));
                }
            }
        }

        out
    })
}

/// 快捷键说明（帮助窗口用）。只列本端口真正接线的按键，鼠标行为同理。
pub const SHORTCUT_HELP: &[(&str, &str)] = &[
    ("左键单击", "放置 / 覆盖音符"),
    ("左键拖动", "框选（音符进入浮动选区）"),
    ("左键拖选中音符", "移动浮动选区"),
    ("右键单击音符", "删除音符"),
    ("右键单击空格", "上下文菜单"),
    ("右键拖动", "从选区减选"),
    ("中键单击 / Ctrl+中键", "吸管（音高音色 / 全部属性）"),
    ("中键拖动", "平移画布"),
    ("Alt + 框选", "复制框选，不摘出原音符"),
    ("Ctrl + 单击", "单点加选"),
    ("Ctrl + 滚轮", "缩放网格"),
    ("滚轮", "平移视口 / 悬停音符时改属性"),
    ("Shift + 滚轮", "横向平移"),
    ("Ctrl+N/O/S", "新建 / 打开 / 保存"),
    ("Ctrl+C/X/V", "复制 / 剪切 / 粘贴"),
    ("Ctrl+Z / Ctrl+Y", "撤销 / 重做"),
    ("Ctrl+A / Ctrl+I", "全选 / 反选"),
    ("Ctrl+E / D", "按当前模式 ±大步（音高 ±12，其余 ±10）"),
    ("Ctrl+R / F", "按当前模式 ±1"),
    ("Ctrl+T / Ctrl+G", "批量设置窗口 / 当前属性回默认（音高模式无）"),
    ("Ctrl+P", "首选项"),
    ("Ctrl+1..9", "选前 9 种乐器（力度 / 声像 / 微调复位并试听）"),
    ("Ctrl+Shift+1..7", "选第 10..16 种乐器"),
    ("Ctrl+Shift+P", "片段编辑器"),
    ("Ctrl+Shift+R / T / Y", "批量设置力度 / 声像 / 微调"),
    ("Ctrl+Shift+A / S", "颤音 / 立体声宏窗口"),
    ("Ctrl+0 / Ctrl+±", "重置 / 调整网格缩放"),
    ("Delete", "删除选区（停止播放时）"),
    ("Space / Enter", "播放暂停 / 停止并回到开头"),
    ("按住 ← / →", "向左 / 向右滚动（同底部快退 / 快进按钮）"),
    ("Home / End", "播放头到开头 / 结尾"),
    ("F10 / F11", "全黑模式 / 全屏"),
    ("F1", "打开本页"),
    ("窗口内 ← / →", "切换页签"),
    ("Escape", "关闭首选项窗口 / 取消选择"),
];


#[cfg(test)]
mod tests {
    use super::*;

    /// 一次「Ctrl+P 点一下」在 egui 里长这样：四个事件全挤在同一帧，
    /// 而 `RawInput::modifiers` 是 egui-winit 帧首按当时键盘状态快照的，此刻 Ctrl 已松开。
    fn key(k: Key, pressed: bool, m: egui::Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: m }
    }

    fn prefs_open_cmd(c: &Command) -> bool {
        matches!(c, Command::ShowWindow(WindowKind::Preferences))
    }

    fn cmds(events: Vec<Event>) -> Vec<Command> {
        let ctx = Context::default();
        let state = EditorState::new();
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
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
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

    /// 原版 `control_draw.gml:849-924` 每一行都带 `keyboard_check(vk_shift)`：
    /// Ctrl+P 只开首选项，片段编辑器必须是 Ctrl+Shift+P。
    #[test]
    fn ctrl_window_keys_do_not_collide_with_plain_ctrl() {
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
        let both = egui::Modifiers { ctrl: true, shift: true, ..Default::default() };
        let got = cmds(vec![key(Key::P, true, ctrl)]);
        assert!(got.iter().any(prefs_open_cmd), "Ctrl+P 应开首选项: {got:?}");
        assert!(
            !got.iter().any(|c| matches!(c, Command::ShowWindow(WindowKind::ClipEditor))),
            "Ctrl+P 不该同时开片段编辑器: {got:?}"
        );
        // Ctrl+S 是保存，不能被立体声宏窗口抢走。
        let got = cmds(vec![key(Key::S, true, ctrl)]);
        assert!(got.iter().any(|c| matches!(c, Command::Save)), "Ctrl+S 应保存: {got:?}");
        assert!(
            !got.iter().any(|c| matches!(c, Command::ShowWindow(WindowKind::Stereo))),
            "Ctrl+S 不该开立体声窗口: {got:?}"
        );
        let got = cmds(vec![key(Key::P, true, both)]);
        assert!(
            got.iter().any(|c| matches!(c, Command::ShowWindow(WindowKind::ClipEditor))),
            "Ctrl+Shift+P 应开片段编辑器: {got:?}"
        );
        assert!(!got.iter().any(prefs_open_cmd), "Ctrl+Shift+P 不该开首选项: {got:?}");
    }
}
