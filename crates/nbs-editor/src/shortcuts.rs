//! 全局快捷键。原版 `control_draw()` 里散落的键盘判定集中到这里，
//! 用 `consume` 无关的只读检查 + 焦点守卫，避免在输入框里打字误触发。

use egui::{Context, Event, Key};

use crate::commands::Command;
use crate::state::{EditMode, Flag, WindowKind};
use crate::state::EditorState;

/// 原版 `control_draw.gml:822-843` 的数字快捷键只覆盖前 16 个乐器：
/// Ctrl+1..9 → 0..8，Ctrl+Shift+1..7 → 9..15。v6 新增的铜块一族（16..19）
/// 在原版里没有按键，所以这里不绑，`instruments_window` 也不给它们印提示。
pub const INSTRUMENT_HOTKEYS: usize = 16;

pub fn collect_shortcuts(state: &EditorState, ctx: &Context) -> Vec<Command> {
    // 有文本输入焦点时，把键盘让给输入框。
    let typing = ctx.memory(|m| m.focused().is_some());
    ctx.input(|i| {
        let mut out = Vec::new();
        let ctrl = i.modifiers.ctrl || i.modifiers.command;
        let shift = i.modifiers.shift;
        let p = |k: Key| i.key_pressed(k);

        // 系统剪贴板粘贴（可能来自其它程序的 OpenNBS selection_code 文本）
        if !typing {
            for e in &i.events {
                if let Event::Paste(text) = e {
                    out.push(Command::PasteText(text.clone()));
                }
            }
        }

        if p(Key::Escape) {
            if state.window.is_some() {
                out.push(Command::CloseWindow);
            } else if !state.doc.selection.is_empty() {
                out.push(Command::Deselect);
            }
            return out;
        }
        if typing {
            return out;
        }

        // ---- Ctrl 组合 ----
        if ctrl {
            if p(Key::N) { out.push(Command::New); }
            if p(Key::O) { out.push(Command::Open); }
            if p(Key::S) { out.push(if shift { Command::SaveAs } else { Command::Save }); }
            if p(Key::A) { out.push(Command::SelectAll); }
            if p(Key::I) { out.push(if shift { Command::ShowWindow(WindowKind::Instruments) } else { Command::InvertSelection }); }
            if p(Key::C) { out.push(Command::Copy); }
            if p(Key::X) { out.push(Command::Cut); }
            if p(Key::V) { out.push(Command::Paste); }
            if p(Key::Z) { out.push(if shift { Command::Redo } else { Command::Undo }); }
            if p(Key::Y) { out.push(Command::Redo); }
            if p(Key::P) { out.push(Command::ShowWindow(WindowKind::Preferences)); }
            if p(Key::Num0) { out.push(Command::ZoomReset); }
            if p(Key::Plus) || p(Key::Equals) { out.push(Command::ZoomIn); }
            if p(Key::Minus) { out.push(Command::ZoomOut); }

            // Ctrl+E/D/R/F：按当前编辑模式升降属性（原版 mode_action 1..4）
            let (small, big) = match state.tool {
                EditMode::Key => (1, 12),
                EditMode::Velocity => (10, 20),
                EditMode::Panning => (10, 20),
                EditMode::Pitch => (100, 300),
            };
            let step_cmd = |delta: i32| -> Command {
                match state.tool {
                    EditMode::Key => Command::SetKey(delta),
                    EditMode::Velocity => Command::SetVel(delta),
                    EditMode::Panning => Command::SetPan(delta),
                    EditMode::Pitch => Command::SetPit(delta),
                }
            };
            if p(Key::E) { out.push(step_cmd(small)); }
            if p(Key::D) { out.push(step_cmd(-small)); }
            if p(Key::R) { out.push(step_cmd(big)); }
            if p(Key::F) { out.push(step_cmd(-big)); }
            if p(Key::T) { out.push(Command::ShowWindow(WindowKind::SetTempo)); }
            if p(Key::G) { out.push(Command::BatchPitch(0)); }
            if p(Key::M) && !shift { out.push(Command::ShowWindow(WindowKind::MacroEditor)); }
            if p(Key::B) { out.push(Command::ToggleMetronome); }
            if p(Key::OpenBracket) { out.push(Command::SetPlaySpeed(state.prefs.play_speed - 0.25)); }
            if p(Key::CloseBracket) { out.push(Command::SetPlaySpeed(state.prefs.play_speed + 0.25)); }

            // Ctrl+1..9 / Ctrl+Shift+1..7 选乐器
            let digits = [
                Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5,
                Key::Num6, Key::Num7, Key::Num8, Key::Num9,
            ];
            for (idx, k) in digits.iter().enumerate() {
                if p(*k) {
                    let ins = if shift { 9 + idx } else { idx };
                    if ins < INSTRUMENT_HOTKEYS {
                        out.push(Command::SetInstrument(ins as u8));
                    }
                }
            }

            if shift {
                if p(Key::V) { out.push(Command::ShowWindow(WindowKind::SetVelocity)); }
                if p(Key::P) { out.push(Command::ShowWindow(WindowKind::SetPanning)); }
                if p(Key::K) { out.push(Command::ShowWindow(WindowKind::ClipEditor)); }
                if p(Key::Q) { out.push(Command::ShowWindow(WindowKind::Tremolo)); }
                if p(Key::W) { out.push(Command::ShowWindow(WindowKind::Stereo)); }
                if p(Key::E) { out.push(Command::ShowWindow(WindowKind::AudioExport)); }
                if p(Key::M) { out.push(Command::ShowWindow(WindowKind::MidiImport)); }
            }
            return out;
        }

        // ---- 无修饰键 ----
        if p(Key::Space) { out.push(Command::PlayPause); }
        if p(Key::Enter) { out.push(Command::Stop); }
        if p(Key::Delete) || p(Key::Backspace) { out.push(Command::Delete); }
        if p(Key::Home) { out.push(Command::SeekHome); out.push(Command::PageHorizontal(-1000)); }
        if p(Key::End) { out.push(Command::SeekEnd); out.push(Command::PageHorizontal(1000)); }
        if p(Key::ArrowLeft) { out.push(Command::SetKey(-1)); }
        if p(Key::ArrowRight) { out.push(Command::SetKey(1)); }
        if p(Key::ArrowUp) { out.push(Command::SetKey(12)); }
        if p(Key::ArrowDown) { out.push(Command::SetKey(-12)); }
        if p(Key::PageUp) { out.push(Command::PageHorizontal(-1)); out.push(Command::PageVertical(-1)); }
        if p(Key::PageDown) { out.push(Command::PageHorizontal(1)); out.push(Command::PageVertical(1)); }
        if p(Key::F1) { out.push(Command::ShowWindow(WindowKind::Shortcuts)); }
        if p(Key::F10) { out.push(Command::ToggleFlag(Flag::Blackout)); }
        if p(Key::F11) { out.push(Command::ToggleFlag(Flag::ShowGridLines)); }
        if p(Key::F2) { out.push(Command::ShowWindow(WindowKind::SongInfo)); }
        if p(Key::F3) { out.push(Command::ShowWindow(WindowKind::Statistics)); }

        out
    })
}

/// 快捷键说明（帮助窗口用）。
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
    ("Ctrl+E/D/R/F", "按当前模式升降属性（小 / 大步）"),
    ("Ctrl+1..9", "选择前 9 种乐器"),
    ("Ctrl+0 / Ctrl+±", "重置 / 调整网格缩放"),
    ("Delete", "删除选区"),
    ("Space / Enter", "播放暂停 / 停止并回到开头"),
    ("方向键", "调整当前画笔音高"),
    ("Home / End", "播放头到开头 / 结尾"),
    ("F1 / F10 / F11", "快捷键 / 黑场 / 网格线"),
    ("Ctrl+M / Ctrl+B", "宏编辑器 / 节拍器"),
    ("Ctrl+[ / Ctrl+]", "播放速度 −/+ 0.25×"),
    ("Ctrl+Shift+E / +M", "音频导出 / MIDI 导入"),
    ("Ctrl+Shift+S", "另存为"),
    ("Ctrl+Page…", "更多命令见菜单"),
    ("Escape", "关闭窗口 / 取消选择"),
];
