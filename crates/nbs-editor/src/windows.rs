//! 弹窗集合：歌曲信息 / 属性 / 统计 / 首选项 / 音色 / 宏窗口 / 文件对话框 /
//! 图层重命名 / 快捷键帮助 / 关于。原版 47 个 `w_*` 窗口状态收敛成一个枚举。

use egui::{
    Color32, Context, CursorIcon, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};
use nbs_core::instruments::{instrument_name, INSTRUMENTS};
use nbs_core::model::{key_name, SongMeta};
use nbs_core::Macro;

use crate::commands::Command;
use crate::shortcuts::SHORTCUT_HELP;
use crate::state::{EditorState, Flag, ModalQueue, WindowKind};
use crate::theme::{Theme, ThemeKind};

pub fn show_windows(state: &mut EditorState, ctx: &Context) {
    show_modal(state, ctx);
    let Some(kind) = state.window.clone() else { return; };
    let mut open = true;
    let title = kind.title().to_string();
    let default_width = match kind {
        // 原版 `draw_window(x1, y1, x1 + 500, y1 + 510)`。
        WindowKind::Preferences => 500.0,
        WindowKind::Shortcuts => 460.0,
        WindowKind::FileOpen | WindowKind::FileSave => 520.0,
        WindowKind::ClipEditor => 500.0,
        // 原版数据包导出窗口 550×430，两列布局。
        WindowKind::DatapackExport => 560.0,
        // 原版 `draw_window_schematic_export` 是 550×400。
        WindowKind::SchematicExport => 560.0,
        _ => 360.0,
    };
    egui::Window::new(title)
        .id(egui::Id::new(format!("nbs_win_{kind:?}")))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(default_width)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| match &kind {
            WindowKind::SongInfo => song_info(state, ui),
            WindowKind::Properties => properties(state, ui),
            WindowKind::Statistics => statistics(state, ui),
            WindowKind::Preferences => preferences(state, ui),
            WindowKind::Instruments => instruments_window(state, ui),
            WindowKind::About => about(ui),
            WindowKind::SetTempo => numeric_window(state, ui, Numeric::Tempo),
            WindowKind::SetVelocity => numeric_window(state, ui, Numeric::Velocity),
            WindowKind::SetPanning => numeric_window(state, ui, Numeric::Panning),
            WindowKind::SetPitch => numeric_window(state, ui, Numeric::Pitch),
            WindowKind::Transpose => numeric_window(state, ui, Numeric::Transpose),
            WindowKind::Tremolo => numeric_window(state, ui, Numeric::Tremolo),
            WindowKind::Stereo => numeric_window(state, ui, Numeric::Stereo),
            WindowKind::ClipEditor => clip_editor(state, ui),
            WindowKind::Shortcuts => shortcuts_help(ui),
            WindowKind::FileOpen => file_dialog(state, ui, false),
            WindowKind::FileSave => file_dialog(state, ui, true),
            WindowKind::LayerRename(layer) => layer_rename(state, ui, *layer),
            WindowKind::ConfirmNew => confirm_new(state, ui),
            WindowKind::AudioExport => audio_export_window(state, ui),
            WindowKind::AudioSettings => audio_settings_window(state, ui),
            WindowKind::MidiImport => midi_import_window(state, ui),
            WindowKind::SchematicExport => schematic_export_window(state, ui),
            WindowKind::DatapackExport => datapack_export_window(state, ui),
            WindowKind::PngExport => png_export_window(state, ui),
            WindowKind::MacroEditor => macro_editor_window(state, ui),
            WindowKind::Autosaves => autosaves_window(state, ui),
        });
    if !open {
        state.window = None;
    }
}

/// 原版的 `message()` / `question()`：一次一个，答完才轮到下一个，
/// 中途答「否」丢掉整个队列。窗口右上角的叉等价于「确定」/「否」。
fn show_modal(state: &mut EditorState, ctx: &Context) {
    let Some(modal) = state.modals.items.front().cloned() else { return };
    let mut answer = None;
    let mut open = true;
    egui::Window::new(modal.title)
        .id(egui::Id::new("nbs_modal"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(&modal.text);
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if modal.yes_no {
                    if ui.button("否").clicked() {
                        answer = Some(false);
                    }
                    if ui.button("是").clicked() {
                        answer = Some(true);
                    }
                } else if ui.button("确定").clicked() {
                    answer = Some(true);
                }
            });
        });
    if answer.is_none() && !open {
        // 点右上角的叉：确认框算「确定」，提问框算「否」。
        answer = Some(!modal.yes_no);
    }
    if let Some(yes) = answer {
        state.emit(Command::AnswerModal(yes));
    }
}

fn footer(state: &mut EditorState, ui: &mut Ui) {
    ui.separator();
    if ui.button("关闭").clicked() {
        state.emit(Command::CloseWindow);
    }
}

fn song_info(state: &mut EditorState, ui: &mut Ui) {
    let mut changed = false;
    egui::Grid::new("song_info_grid").num_columns(2).show(ui, |ui| {
        let m: &mut SongMeta = &mut state.doc.song.meta;
        ui.label("名称");
        if ui.text_edit_singleline(&mut m.name).changed() { changed = true; }
        ui.end_row();
        ui.label("作者");
        if ui.text_edit_singleline(&mut m.author).changed() { changed = true; }
        ui.end_row();
        ui.label("原作者");
        if ui.text_edit_singleline(&mut m.original_author).changed() { changed = true; }
        ui.end_row();
        ui.label("描述");
        if ui.add(egui::TextEdit::multiline(&mut m.description).desired_rows(3)).changed() { changed = true; }
        ui.end_row();
    });
    if changed {
        state.doc.song.dirty = true;
    }
    ui.separator();
    ui.label(RichText::new("这些字段会随 .nbs 一起保存").small().weak());
    footer(state, ui);
}

fn properties(state: &mut EditorState, ui: &mut Ui) {
    let mut tempo = state.doc.song.meta.tempo as f64;
    if ui.add(egui::Slider::new(&mut tempo, 0.25..=60.0).text("速度 (tick/s)")).changed() {
        state.emit(Command::SetTempo(tempo as f32));
    }
    ui.label(RichText::new(format!("= {:.0} BPM", tempo * 15.0)).small().weak());
    let mut ts = state.doc.song.meta.time_signature as f32;
    if ui.add(egui::Slider::new(&mut ts, 2.0..=8.0).text("拍号")).changed() {
        state.emit(Command::SetTimeSignature(ts as u8));
    }
    let mut looping = state.doc.song.meta.loop_enabled;
    if ui.checkbox(&mut looping, "启用循环").changed() {
        state.emit(Command::ToggleLoop);
    }
    let loop_on = state.doc.song.meta.loop_enabled;
    ui.add_enabled_ui(loop_on, |ui| {
        ui.horizontal(|ui| {
            ui.label("循环起点 tick");
            let enda = state.doc.song.enda as f64;
            let mut start = state.doc.song.meta.loop_start as f64;
            if ui.add(egui::DragValue::new(&mut start).range(0.0..=enda)).changed() {
                state.doc.edit_meta("设置循环起点", |m| m.loop_start = start as u16);
                state.sync_loop_params();
            }
        });
        let len = state.doc.song.song_length();
        ui.horizontal(|ui| {
            ui.label("循环终点 tick");
            // 原版没有 loop_end 字段：循环终点就是歌曲长度，所以只能往上调，
            // 上调的效果是延长结尾。
            let mut end = len as f64;
            if ui.add(egui::DragValue::new(&mut end).range(len as f64..=(len + 16) as f64)).changed() {
                state.doc.extend_loop_end(end as u32);
                state.sync_loop_params();
            }
        });
        ui.label(
            RichText::new("超出歌曲长度时，结尾会自动加一个 0 音量音符来延长终点")
                .small()
                .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("循环次数");
            let mut max = state.doc.song.meta.loop_max as i32;
            if ui.add(egui::DragValue::new(&mut max).range(0..=10)).changed() {
                state.doc.edit_meta("设置循环次数", |m| m.loop_max = max as u8);
                state.sync_loop_params();
            }
            if state.doc.song.meta.loop_max == 0 {
                ui.label(RichText::new("（无限）").small().weak());
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label("保存格式版本");
        let mut v = state.doc.song.meta.save_version;
        egui::ComboBox::from_id_salt("save_version")
            .selected_text(format!("NBS v{v}"))
            .show_ui(ui, |ui| {
                for n in 0..=nbs_core::nbs::MAX_NBS_VERSION {
                    ui.selectable_value(&mut v, n, format!("NBS v{n}"));
                }
            });
        if v != state.doc.song.meta.save_version {
            state.emit(Command::SetSaveVersion(v));
        }
    });
    ui.separator();
    ui.label(RichText::new(format!(
        "v0..v{} 全部可读可写；v4/v5 才会保存力度/声像/微调与图层锁定，v6 只是把内置乐器从 16 种扩到 20 种（格式与 v5 相同）",
        nbs_core::nbs::MAX_NBS_VERSION
    ))
    .small()
    .weak());
    footer(state, ui);
}

fn statistics(state: &mut EditorState, ui: &mut Ui) {
    let song = &state.doc.song;
    // 自定义乐器的编号从 FIRST_CUSTOM_INDEX 起，所以直方图要覆盖到它们，
    // 否则大工程里最常见的恰恰是被漏掉的（原版统计就包含全部乐器行）。
    let slots = nbs_core::model::FIRST_CUSTOM_INDEX as usize
        + song.custom_instruments.len().max(1);
    let mut per_ins = vec![0usize; slots];
    let mut min_key = 87u8;
    let mut max_key = 0u8;
    for (_, _, n) in song.iter_notes() {
        let i = n.instrument as usize;
        if i < per_ins.len() {
            per_ins[i] += 1;
        }
        min_key = min_key.min(n.key);
        max_key = max_key.max(n.key);
    }
    let seconds = song.song_length() as f32 / song.meta.tempo.max(0.01);
    egui::Grid::new("stats_grid").num_columns(2).show(ui, |ui| {
        ui.label("音符总数"); ui.label(song.total_blocks.to_string()); ui.end_row();
        ui.label("歌曲长度"); ui.label(format!("{} ticks", song.song_length())); ui.end_row();
        ui.label("时长"); ui.label(format!("{seconds:.1} 秒")); ui.end_row();
        ui.label("图层数"); ui.label(song.layers.len().to_string()); ui.end_row();
        ui.label("已锁定图层"); ui.label(song.locked_layers().iter().filter(|x| **x).count().to_string()); ui.end_row();
        ui.label("音高范围"); ui.label(format!("{} ~ {}", key_name(min_key, true, false), key_name(max_key, true, false))); ui.end_row();
        ui.label("自定义音色"); ui.label(song.custom_instruments.len().to_string()); ui.end_row();
        ui.label("历史条目"); ui.label(state.doc.history.len().to_string()); ui.end_row();
    });
    ui.separator();
    ui.label(RichText::new("各乐器音符数").strong());
    let total = song.total_blocks.max(1);
    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
        for (i, c) in per_ins.iter().enumerate() {
            if *c > 0 {
                let pct = *c as f32 * 100.0 / total as f32;
                ui.label(format!("{:>6}  {:>5.1}%  {}", c, pct, instrument_name(i as u8)));
            }
        }
    });
    footer(state, ui);
}

/// 原版 `draw_window_preferences.gml`：500×510 的五页对话框
/// （通用 / 界面 / 外观 / 访问 / 播放）。
///
/// 原版全程按 `x1 + K` / `y1 + N` 的绝对坐标画，这里把 `(x1, y1)` 换成 egui 窗口的
/// 内容原点（= 原版页签条左上角），其余数字一比一照抄。源码里写成
/// `y1 + N + (theme = 3) * 22` 的行，Fluent 落在 `N - 8`、另外三套落在 `N - 26`，
/// 差的那 4 像素是原版页签条比 Fluent 高的部分，归 egui 的标题栏。
///
/// 只画真实接线的行。原版有、我们还没有对应行为的选项（启动页 / 检查更新 / 歌曲目录 /
/// Discord / 界面音效 / 透明效果 / 取色器 / 帧率 / 窗口缩放 / 语言 / 程序图标 /
/// 音符盒不透明 / 删除淡出 / 五线谱 / 任务栏进度 / 重置键位 / 在区间起止播放）
/// 整行不出现，免得留下点了没反应的假控件。
fn preferences(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    // 窗口外沿宽 500（原版 `draw_window(x1, y1, x1 + 500, y1 + 510)`），
    // 内容区扣掉 egui 的窗口内边距；把原点左移一个内边距，行内 x 就能直接用原版的 K。
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(500.0 - ml - mr, 472.0),
        egui::Sense::hover(),
    );
    // allocate_exact_size 之后 cursor 已经移到这块区域的下面，原点必须取分配到的矩形。
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };

    // 原版 `selected_tab += keyboard_check_pressed(vk_right) - keyboard_check_pressed(vk_left)`，
    // 出界绕到另一头。
    let (left, right) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),
        )
    });
    if left {
        state.prefs_tab = (state.prefs_tab + 4) % 5;
    }
    if right {
        state.prefs_tab = (state.prefs_tab + 1) % 5;
    }
    let tab = state.prefs_tab.min(4);

    let mut menu_at = Pos2::ZERO;
    {
        let mut pg = Page {
            ui: &mut *ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if fluent {
            // 原版只在 theme 3 画左上角返回按钮，抬手关窗。
            if pg.back(&state.icons) {
                state.emit(Command::CloseWindow);
            }
        }
        if !fluent {
            // 先铺面板再画页签：原版的选中页签会压掉面板顶边那一像素。
            pg.panel_box();
        }
        let tab = pg.tabs(if fluent { 40.0 } else { 8.0 }, tab);
        state.prefs_tab = tab;
        match tab {
            0 => prefs_general(state, &mut pg),
            1 => prefs_interface(state, &mut pg, &mut menu_at),
            2 => prefs_appearance(state, &mut pg),
            3 => prefs_usability(state, &mut pg),
            _ => prefs_playback(state, &mut pg),
        }
        // 原版 `draw_button2(x1 + 420, y1 + 478, 72, "确定")`：确定 / Escape / Ctrl+P 都关窗。
        if pg.button(420.0, 478.0, 72.0, "确定", false) {
            state.emit(Command::CloseWindow);
        }
        let ctrl_p = pg.ui.input(|i| {
            (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::P)
        });
        if ctrl_p {
            state.emit(Command::CloseWindow);
        }
    }
    if state.prefs_theme_menu {
        theme_menu(state, ui, menu_at);
    }
}

/// 原版页 0「通用」。源码这一页还有「启动」「歌曲」「Discord」三组，
/// 我们没有欢迎页、更新检查、歌曲/片段目录和 Discord 富在位，整组不画。
fn prefs_general(state: &mut EditorState, pg: &mut Page) {
    pg.header(278.0, 74.0, 200.0, 85.0, "自动保存");
    if pg.check(
        296.0,
        90.0,
        state.prefs.autosave_enabled,
        "启用自动保存",
        "歌曲是否每过一段时间自动保存一次。",
        true,
        false,
    ) {
        state.prefs.autosave_enabled = !state.prefs.autosave_enabled;
        state.emit(Command::SavePreferences);
    }
    // 原版 `if (autosave = 0)`：整行涂 c_gray(128)，数字退化成静态文本。
    let on = state.prefs.autosave_enabled;
    let ink = if on { pg.ink() } else { ggray(128) };
    pg.text(pg.row(326.0, 110.0), "间隔:            分钟", PREFS_FONT, ink);
    let tip = "自动保存中间的间隔。";
    pg.tip_area(326.0, 110.0, 180.0, 16.0, tip);
    let mins = state.prefs.auto_save_minutes as i32;
    let drag = pg.drag(375.0, 110.0, mins, 1.0, 1, 60, !on, tip, &mut state.prefs_drag);
    if let Some(mins) = drag {
        state.prefs.auto_save_minutes = mins as u8;
        state.emit(Command::SavePreferences);
    }
}

/// 原版页 1「界面」：主题下拉 + 暗色 / 全黑两个开关。
/// 同页的取色器（w_setaccent）、帧率限制、窗口缩放、语言、程序图标
/// 都没有对应实现，不画。
fn prefs_interface(
    state: &mut EditorState,
    pg: &mut Page,
    menu_at: &mut Pos2,
) {
    pg.header(22.0, 74.0, 218.0, 140.0, "主题");
    pg.frame_box(40.0, 95.0, 140.0, 115.0);
    let name = state.theme.kind.name().to_string();
    pg.text(pg.row(43.0, 98.0), &name, PREFS_FONT, pg.ink());
    // 原版 `show_menu_ext("theme", x1 + 40, y1 + 74 + 16 + 21 + (theme = 3) * 22 + 5, …)`。
    *menu_at = pg.row(40.0, 116.0);
    // 只有右边那个箭头能开下拉；菜单开着时原版由 obj_menu 吞掉点击，这里交给提示区收起。
    if pg.arrow(&state.icons) && !state.prefs_theme_menu {
        state.prefs_theme_menu = true;
    }
    if pg.check(
        40.0,
        135.0,
        state.theme.fdark,
        "暗色模式",
        "是否在界面上使用暗色调。\n（仅限 Fluent 主题）",
        true,
        state.theme.kind != ThemeKind::Fluent,
    ) {
        state.theme.fdark = !state.theme.fdark;
        state.emit(Command::SavePreferences);
    }
    if pg.check(
        40.0,
        195.0,
        state.flags.blackout,
        "全黑模式",
        "使背景变为纯黑色，可以用于剪辑时扣掉。",
        true,
        false,
    ) {
        state.emit(Command::ToggleFlag(Flag::Blackout));
    }
}

/// 原版页 2「外观」：音符盒与琴键。
/// 源码里的「音符盒不透明」(210)、「显示删除效果」(250)、
/// 「在琴键上时显示五线谱对应音符」(410) 我们没有这三项行为，整行不画。
fn prefs_appearance(state: &mut EditorState, pg: &mut Page) {
    pg.header(22.0, 74.0, 456.0, 196.0, "音符盒");
    let blocks = [
        (90.0, Flag::UseColors, "彩色音符盒", "用音符盒颜色区分音色。"),
        (110.0, Flag::UseIcons, "显示音色图标", "用音色图标区分音色。"),
        (130.0, Flag::UseShapes, "音符盒形状", "用音符盒形状区分音色。"),
        (150.0, Flag::ShowNumbers, "显示音符序号", "在音符盒上显示需要按右键次数。"),
        (170.0, Flag::ShowOctaves, "显示八度序号", "在音符盒上显示第几八度。"),
        (190.0, Flag::ShowIncompatible, "标记不兼容的音符盒", "在自定义音色或超出 2 八度范围的音符盒周围显示红框。"),
        (230.0, Flag::ShowLayers, "显示每层操作区", "显示在界面左侧操作每层设定的区域。"),
    ];
    for (n, flag, label, tip) in blocks {
        if pg.check(40.0, n, flag.get(&state.flags), label, tip, false, false) {
            state.emit(Command::ToggleFlag(flag));
        }
    }

    pg.header(22.0, 294.0, 456.0, 171.0, "琴键");
    let piano = state.flags.show_piano;
    if pg.check(40.0, 310.0, piano, "显示琴键", "钢琴键是否可见。", true, false) {
        state.emit(Command::ToggleFlag(Flag::ShowPiano));
    }
    // 「最多显示键数」：琴键隐藏时同样退化成灰色静态数字（原版 `if (!show_piano) draw_set_color(c_gray)`）。
    let ink = if piano { pg.ink() } else { ggray(128) };
    pg.text(pg.row(70.0, 328.0), "最多显示键数:", PREFS_FONT, ink);
    let tip = "同屏显示的琴键数。过多可能会在老电脑上造成卡顿。";
    pg.tip_area(70.0, 328.0, 150.0, 21.0, tip);
    let keys = state.prefs.keys_to_show as i32;
    let drag = pg.drag(150.0, 328.0, keys, 2.0, 20, 50, !piano, tip, &mut state.prefs_drag);
    if let Some(keys) = drag {
        state.prefs.keys_to_show = keys.clamp(20, 50) as u8;
        state.emit(Command::SavePreferences);
    }
    let keys_rows = [
        (350.0, Flag::ShowKeyNames, "显示键名", "显示琴键所对应音的名称。"),
        (370.0, Flag::ShowKeyNumbers, "显示音符序号", "在 2 八度内的琴键上显示需要按右键次数。"),
        (390.0, Flag::ShowKeyboard, "显示键盘键位", "在琴键上显示绑定的键盘键位。"),
        (430.0, Flag::ShowOutOfRange, "标记超范围琴键", "是否为在 2 八度之外的琴键上标红。"),
        (450.0, Flag::Flats, "使用降记号", "是否为键名使用降记号（b）而不是升记号（#）。"),
    ];
    for (n, flag, label, tip) in keys_rows {
        if pg.check(40.0, n, flag.get(&state.flags), label, tip, false, false) {
            state.emit(Command::ToggleFlag(flag));
        }
    }
}

/// 原版页 3「访问」：滚轮行为与琴键选中行为。
/// 源码末尾的「对琴键右键可更改键位。」和「重置键位」按钮依赖键位编辑功能，
/// 我们只有固定键位，所以这两行也不出现。
fn prefs_usability(state: &mut EditorState, pg: &mut Page) {
    pg.header(22.0, 74.0, 456.0, 120.0, "鼠标滚轮");
    let wheel_rows = [
        (90.0, 0u8, "使用鼠标滚轮移动位置", "使用鼠标滚轮横向或竖向移动编辑区域位置。"),
        (110.0, 1, "使用鼠标滚轮改变音色", "使用鼠标滚轮切换可用音色。"),
        (130.0, 2, "使用鼠标滚轮改变音调", "使用鼠标滚轮改变目前使用的音调。"),
    ];
    for (n, value, label, tip) in wheel_rows {
        if pg.radio(40.0, n, state.prefs.mousewheel == value, label, tip, false) {
            state.prefs.mousewheel = value;
            state.emit(Command::SavePreferences);
        }
    }
    if pg.check(
        40.0,
        158.0,
        state.prefs.changepitch,
        "当在音符上使用鼠标滚轮改变音符属性",
        "在音符上使用鼠标滚轮时是否根据目前模式改变其音调、音量、声道或音高。",
        false,
        false,
    ) {
        state.prefs.changepitch = !state.prefs.changepitch;
        state.emit(Command::SavePreferences);
    }

    pg.header(22.0, 220.0, 456.0, 105.0, "琴键");
    if pg.check(
        40.0,
        236.0,
        state.prefs.select_lastpressed,
        "选择按下的键",
        "是否选择按下的键所对应的音。",
        false,
        false,
    ) {
        state.prefs.select_lastpressed = !state.prefs.select_lastpressed;
        state.emit(Command::SavePreferences);
    }
}

/// 原版页 4「播放」：进度条跟随、播放开关、速度单位。
/// 「在区间时开始播放」/「在区间后停止播放」/「启用任务栏进度」三行
/// 我们没有区间起播和任务栏进度，整行不画。
fn prefs_playback(state: &mut EditorState, pg: &mut Page) {
    pg.header(22.0, 74.0, 456.0, 145.0, "进度条");
    if pg.check(
        40.0,
        90.0,
        state.marker_follow,
        "播放时跟随进度条",
        "播放歌曲时和进度条一起滚动。",
        false,
        false,
    ) {
        state.emit(Command::ToggleFollow);
    }
    // 原版 `marker_pagebypage`：1 按页 / 0 按刻 / 2 按帧，跟随关掉时三格都锁定。
    let lock = !state.marker_follow;
    let follow_rows = [
        (110.0, 1u8, "按页", "跟随进度条翻页。"),
        (130.0, 0, "按刻", "跟随进度条滚动。"),
        (150.0, 2, "按帧", "跟随进度条逐帧滚动。"),
    ];
    for (n, value, label, tip) in follow_rows {
        if pg.radio(70.0, n, state.follow_mode == value, label, tip, lock) {
            state.follow_mode = value;
            state.emit(Command::SavePreferences);
        }
    }

    pg.header(22.0, 244.0, 218.0, 120.0, "播放");
    if pg.check(
        32.0,
        260.0,
        state.prefs.show_layer_volumes,
        "显示每层音量",
        "是否显示每层的音量。",
        false,
        false,
    ) {
        state.prefs.show_layer_volumes = !state.prefs.show_layer_volumes;
        state.emit(Command::SavePreferences);
    }
    if pg.check(32.0, 280.0, state.prefs.mono, "关闭立体声", "关闭立体声音效。", false, false) {
        state.emit(Command::ToggleMono);
    }
    if pg.check(
        32.0,
        300.0,
        state.prefs.show_sound_count,
        "显示正在播放的声音数",
        "在状态栏里显示正在播放的声音数。",
        false,
        false,
    ) {
        state.prefs.show_sound_count = !state.prefs.show_sound_count;
        state.emit(Command::SavePreferences);
    }
    if pg.check(
        32.0,
        320.0,
        state.prefs.extended_voices,
        "提升声音限制",
        "提升可同时播放的声音数。",
        false,
        false,
    ) {
        state.emit(Command::ToggleExtendedVoices);
    }

    pg.header(255.0, 244.0, 223.0, 60.0, "速度单位");
    if pg.radio(265.0, 260.0, !state.prefs.use_bpm, "红石刻 / 秒 (t/s)", "使用每秒几刻显示速度。", false) {
        state.prefs.use_bpm = false;
        state.emit(Command::SavePreferences);
    }
    if pg.radio(265.0, 280.0, state.prefs.use_bpm, "拍数 / 分钟 (BPM)", "使用每分钟多少拍显示速度。", false) {
        state.prefs.use_bpm = true;
        state.emit(Command::SavePreferences);
    }
}

/// 原版 `show_menu_ext("theme", …)`：主题下拉展开的那四个条目，
/// 顺序照源码 `经典 | 暗黑 | 复古 | Fluent`。
const THEME_MENU_ORDER: [ThemeKind; 4] = [
    ThemeKind::Aqua,
    ThemeKind::Dark,
    ThemeKind::Retro,
    ThemeKind::Fluent,
];

fn theme_menu(state: &mut EditorState, ui: &Ui, at: Pos2) {
    let ctx = ui.ctx();
    // 原版 `obj_menu`：菜单开着时全屏吞输入，点哪儿都只用来收起。
    let dismissed = egui::Area::new(Id::new("prefs_menu_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::ZERO)
        .show(ctx, |ui| ui.allocate_rect(ctx.screen_rect(), Sense::click()).clicked())
        .inner;
    let mut picked: Option<ThemeKind> = None;
    egui::Area::new(Id::new("prefs_theme_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(98.0);
                for kind in THEME_MENU_ORDER {
                    let sel = state.theme.kind == kind;
                    let label = format!("{} {}", if sel { "\u{2713}" } else { " " }, kind.name());
                    if ui.selectable_label(sel, label).clicked() {
                        picked = Some(kind);
                    }
                }
            });
        });
    if let Some(kind) = picked {
        state.emit(Command::SetTheme(kind));
    }
    if picked.is_some() || dismissed {
        state.prefs_theme_menu = false;
    }
}

/// 原版 `fnt_main` 9 像素（正文）和 `fnt_info_med` 11 像素（Fluent 的分组标题与页签）。
const PREFS_FONT: f32 = 9.0;
const PREFS_FONT_MED: f32 = 11.0;

/// GameMaker 的灰阶常量 `make_color_rgb(v, v, v)`。
const fn ggray(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}

/// 首选项页面的绝对坐标绘制器。原版的 `draw_checkbox` / `draw_radiobox` /
/// `draw_button2` / `draw_areaheader` / `draw_area` / `draw_dragvalue` 各自只有
/// 一处调用形态，这里一一对应成方法，常量和圆角全部照抄源码。
struct Page<'a> {
    ui: &'a mut Ui,
    /// 内容原点，相当于原版的 `(x1, y1 + 30)`（Fluent）/ `(x1, y1 + 26)`（其余主题）。
    o: Pos2,
    /// 行号换算：原版 `y1 + N + (theme = 3) * 22` → `N + ry`。
    ry: f32,
    th: &'a Theme,
    fluent: bool,
}

impl<'a> Page<'a> {
    /// 不带主题行偏移的绝对位置（页签条、返回按钮这一类）。
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.o + Vec2::new(x, y)
    }

    /// 一行控件的左上角，`n` 是源码里 `y1 +` 后面的那个数。
    fn row(&self, x: f32, n: f32) -> Pos2 {
        self.at(x, n + self.ry)
    }

    /// 原版 GameMaker 的矩形写法：`(x1, y1)` 到 `(x2, y2)`，宽高取两端之差。
    fn row_rect(&self, x1: f32, n1: f32, x2: f32, n2: f32) -> Rect {
        Rect::from_min_max(self.row(x1, n1), self.row(x2, n2))
    }

    fn ink(&self) -> Color32 {
        self.th.text
    }

    /// 原版 `string_width_dynamic`。
    fn measure(&self, s: &str, font: f32) -> f32 {
        self.ui
            .painter()
            .layout_no_wrap(s.to_string(), FontId::proportional(font), self.ink())
            .size()
            .x
    }

    /// 原版 `draw_text_dynamic`：左上角对齐，返回宽度好接着排版。
    fn text(&self, at: Pos2, s: &str, font: f32, color: Color32) -> f32 {
        let p = self.ui.painter().clone();
        let g = p.layout_no_wrap(s.to_string(), FontId::proportional(font), color);
        let w = g.size().x;
        p.galley(at, g, Color32::BLACK);
        w
    }

    /// 原版 `mouse_rectangle` + `m += mouse_check_button(左)`：0 = 不在，1 = 悬停，
    /// 2 = 悬停且按住；`Response::clicked` 就是原版的「在本框内抬手」。
    /// 锁定行不给点击，只留悬停好挂提示。
    fn hit(&self, rect: Rect, id: Id, lock: bool) -> (egui::Response, usize) {
        let sense = if lock { Sense::hover() } else { Sense::click() };
        let resp = self.ui.interact(rect, id, sense);
        if lock {
            return (resp, 0);
        }
        let held = resp.hovered() && self.ui.input(|i| i.pointer.any_down());
        let m = if held {
            2
        } else if resp.hovered() {
            1
        } else {
            0
        };
        (resp, m)
    }

    /// 原版 `draw_areaheader(x, y, w, h, str)`。Fluent 只剩一行 11 像素文字
    /// （`case 3:` 只设了颜色，`draw_rectangle` 那行条件是 `theme != 3`，都不执行）；
    /// 主题 0 是 (213,223,229) 的实心圆角框，主题 1/2 是 `draw_frame` 立体框。
    /// 三套旧主题都在标题左上角挖一块底衬色（白 / (212,208,200) / c_dark）。
    /// `draw_frame` 依赖 `spr_interface4` 九宫格，这张图我们没导出，非 Fluent 用 1 像素边框近似。
    fn header(&self, x: f32, n: f32, w: f32, h: f32, label: &str) {
        let font = if self.fluent { PREFS_FONT_MED } else { PREFS_FONT };
        if !self.fluent {
            let p = self.ui.painter().clone();
            let rect = Rect::from_min_size(self.row(x, n), Vec2::new(w, h));
            match self.th.kind {
                // `draw_roundrect` 的第五个参数是「要不要圆角」，画的是填充，线宽不影响它。
                // 圆角半径取 GameMaker 的默认值 10（全工程没调过 `draw_set_roundrect_size`）。
                ThemeKind::Aqua => {
                    p.rect_filled(rect, 10.0, Color32::from_rgb(213, 223, 229));
                }
                ThemeKind::Retro | ThemeKind::Dark => {
                    p.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, self.th.border), StrokeKind::Inside);
                }
                ThemeKind::Fluent => {}
            }
            let notch = match self.th.kind {
                ThemeKind::Aqua => Color32::WHITE,
                ThemeKind::Retro => Color32::from_rgb(212, 208, 200),
                _ => self.th.window_bg,
            };
            let tw = self.measure(label, font);
            p.rect_filled(
                Rect::from_min_size(self.row(x + 12.0, n - 6.0), Vec2::new(tw + 6.0, 12.0)),
                0.0,
                notch,
            );
        }
        self.text(self.row(x + 16.0, n - 6.0), label, font, self.ink());
    }

    /// 原版 `draw_area(x1, y1, x2, y2)`：下拉框那一圈。Fluent 是 2 像素圆角描边
    /// （light 165 / fdark 182），其余主题是 `spr_interface1` 九宫格（同样没导出，用主题描边）。
    fn frame_box(&self, x1: f32, n1: f32, x2: f32, n2: f32) {
        let rect = self.row_rect(x1, n1, x2 - 2.0, n2 - 2.0);
        let (line, r) = if self.fluent {
            (if self.th.fdark { ggray(182) } else { ggray(165) }, 2.0)
        } else {
            (self.th.border, 0.0)
        };
        self.ui
            .painter()
            .rect_stroke(rect, r, Stroke::new(1.0_f32, line), StrokeKind::Inside);
    }

    /// 非 Fluent 主题那一整块面板：原版
    /// `draw_rectangle(x1+6, y1+46, x1+494, y1+472)` 铺底 + (137,140,149) 描边，
    /// 主题 0 再套一层 (213,223,229) 圆角框。
    fn panel_box(&self) {
        let p = self.ui.painter().clone();
        let rect = Rect::from_min_max(self.at(6.0, 20.0), self.at(494.0, 446.0));
        let fill = if self.th.kind == ThemeKind::Dark {
            self.th.window_bg
        } else {
            Color32::WHITE
        };
        p.rect_filled(rect, 0.0, fill);
        p.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
            StrokeKind::Inside,
        );
        if self.th.kind == ThemeKind::Aqua {
            p.rect_stroke(
                Rect::from_min_max(self.at(10.0, 24.0), self.at(490.0, 442.0)),
                10.0,
                Stroke::new(1.0_f32, Color32::from_rgb(213, 223, 229)),
                StrokeKind::Inside,
            );
        }
    }

    /// 原版页签条。Fluent：文字 `(b+6, 13)`，选中项下面画 `spr_tabsel`
    /// （实测 14×3 的圆条，水平居中于文字）；其余主题：`(b+6, 4)`，选中项是
    /// 一块压住面板线的矩形（原版 `spr_tabbuttons`，没导出，按同尺寸矩形近似）。
    fn tabs(&mut self, b0: f32, tab: u8) -> u8 {
        const LABELS: [&str; 5] = ["通用", "界面", "外观", "访问", "播放"];
        let font = if self.fluent { PREFS_FONT_MED } else { PREFS_FONT };
        let mut nsel = tab;
        let mut b = b0;
        for (i, name) in LABELS.iter().enumerate() {
            let tw = self.measure(name, font);
            let (hy, hh) = if self.fluent { (0.0, 39.0) } else { (2.0, 18.0) };
            let rect = Rect::from_min_size(self.at(b, hy), Vec2::new(tw + 12.0, hh));
            let resp = self.ui.interact(rect, Id::new(("prefs-tab", i)), Sense::click());
            let sel = i as u8 == tab;
            // 原版选中页签的文字比未选中高两像素（`y1 + 28` 对 `y1 + 30`）。
            let ty = match (self.fluent, sel) {
                (true, _) => 13.0,
                (false, true) => 2.0,
                (false, false) => 4.0,
            };
            let held = resp.hovered() && self.ui.input(|i| i.pointer.any_down());
            let ink = if !self.fluent {
                self.ink()
            } else if held || (resp.hovered() && sel) {
                match (sel, self.th.fdark) {
                    (true, false) => ggray(152),
                    (true, true) => ggray(163),
                    (false, false) => ggray(116),
                    (false, true) => ggray(176),
                }
            } else if self.th.fdark {
                Color32::WHITE
            } else {
                Color32::BLACK
            };
            if sel {
                let p = self.ui.painter().clone();
                if self.fluent {
                    p.rect_filled(
                        Rect::from_min_size(self.at(b + tw / 2.0 - 1.0, 33.0), Vec2::new(14.0, 3.0)),
                        1.5,
                        self.th.accent,
                    );
                } else {
                    // 原版：填一块 (stabx, 26)-(stabx+stabw, 46) 的白底（主题 2 用 c_dark），
                    // 描 (137,140,149) 一圈，再把面板顶线在页签正下方那一段擦掉，
                    // 让选中的页签和面板连成一片。
                    let fill = if self.th.kind == ThemeKind::Dark {
                        self.th.window_bg
                    } else {
                        Color32::WHITE
                    };
                    let body = Rect::from_min_size(self.at(b - 2.0, 0.0), Vec2::new(tw + 15.0, 20.0));
                    let line = Rect::from_min_size(self.at(b - 2.0, 0.0), Vec2::new(tw + 15.0, 21.0));
                    p.rect_filled(body, 0.0, fill);
                    p.rect_stroke(
                        line,
                        0.0,
                        Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                        StrokeKind::Inside,
                    );
                    p.rect_filled(
                        Rect::from_min_size(self.at(b - 1.0, 20.0), Vec2::new(tw + 13.0, 1.0)),
                        0.0,
                        fill,
                    );
                }
            }
            self.text(self.at(b + 6.0, ty), name, font, ink);
            if resp.clicked() {
                nsel = i as u8;
            }
            b += tw + 12.0;
        }
        nsel
    }

    /// 原版 `draw_checkbox(x, y, expr, str, tip[, lock[, is_switch]])`。
    /// Fluent 照 `draw_checkbox.gml` 的常量手绘方框 / 胶囊开关；
    /// 其余三套主题原版画的是 `spr_checkbox` / `spr_switch` 贴图（没导出），
    /// 所以退回 egui 原生复选框摆在同一行坐标上。
    fn check(&mut self, x: f32, n: f32, on: bool, label: &str, tip: &str, switch: bool, lock: bool) -> bool {
        let tw = self.measure(label, PREFS_FONT);
        let id = Id::new(("prefs-check", x as i32, n as i32, label));
        if !self.fluent {
            return self.legacy_check(x, n, on, label, lock, tw);
        }
        let at = self.row(x, n);
        let (resp, m) = self.hit(Rect::from_min_size(at, Vec2::new(26.0 + tw + 8.0, 13.0)), id, lock);
        let fdark = self.th.fdark;
        let acc = Theme::accent_at(5 + if m == 1 { 2 } else { 0 } + if m == 2 { 3 } else { 0 });
        let fill = if on && !lock {
            acc
        } else if on {
            if fdark {
                ggray(51)
            } else if switch {
                ggray(227)
            } else {
                ggray(136)
            }
        } else if lock {
            if fdark {
                ggray(0)
            } else {
                ggray(243)
            }
        } else if fdark {
            ggray([32, 42, 48][m])
        } else {
            ggray([243, 229, 220][m])
        };
        let p = self.ui.painter().clone();
        if switch {
            p.rect_filled(
                Rect::from_min_size(at + Vec2::new(-1.0, -1.0), Vec2::new(26.0, 13.0)),
                12.0,
                fill,
            );
            if !on {
                let line = if fdark {
                    if lock { ggray(56) } else { ggray(204) }
                } else if lock {
                    ggray(199)
                } else {
                    ggray(51)
                };
                p.rect_stroke(
                    Rect::from_min_size(at, Vec2::new(24.0, 11.0)),
                    6.0,
                    Stroke::new(1.0_f32, line),
                    StrokeKind::Inside,
                );
            }
            let knob_fill = match (on, lock, fdark) {
                (true, false, false) => ggray(243),
                (true, false, true) => ggray(32),
                (true, true, false) => ggray(204),
                (true, true, true) => ggray(76),
                (false, false, false) => ggray(51),
                (false, false, true) => ggray(204),
                (false, true, false) => ggray(199),
                (false, true, true) => ggray(56),
            };
            let (kx, ky) = if on {
                (at.x + 15.0 - if m == 2 && !lock { 1.0 } else { 0.0 }, at.y + if lock { 3.0 } else { 2.0 })
            } else {
                (at.x + 2.0, at.y + 2.0)
            };
            let kw = if on { 7.0 } else { 7.0 + if m == 2 && !lock { 1.0 } else { 0.0 } };
            p.rect_filled(Rect::from_min_size(Pos2::new(kx, ky), Vec2::new(kw, 7.0)), 12.0, knob_fill);
        } else {
            let box_rect = Rect::from_min_size(at + Vec2::new(-1.0, -1.0), Vec2::new(13.0, 13.0));
            p.rect_filled(box_rect, 4.0, fill);
            if !on {
                let line = if fdark {
                    if lock {
                        ggray(56)
                    } else {
                        ggray(if m == 2 { 76 } else { 153 })
                    }
                } else if lock {
                    ggray(204)
                } else {
                    ggray(if m == 2 { 178 } else { 136 })
                };
                p.rect_stroke(box_rect, 2.0, Stroke::new(1.0_f32, line), StrokeKind::Inside);
            } else {
                let mark = match (lock, fdark) {
                    (false, false) => ggray(243),
                    (false, true) => ggray(32),
                    (true, false) => Color32::WHITE,
                    (true, true) => Color32::BLACK,
                };
                let s = Stroke::new(1.0_f32, mark);
                p.line_segment([at + Vec2::new(2.0, 5.0), at + Vec2::new(4.0, 8.0)], s);
                p.line_segment([at + Vec2::new(4.0, 8.0), at + Vec2::new(9.0, 3.0)], s);
            }
        }
        self.text(
            at + Vec2::new(if switch { 31.0 } else { 18.0 }, -1.0),
            label,
            PREFS_FONT,
            if lock { ggray(204) } else { self.ink() },
        );
        resp.on_hover_text(tip).clicked()
    }

    /// 原版 `draw_radiobox(x, y, expr, str, tip[, lock])`，同上的 Fluent 手绘 /
    /// 其余主题退回原生单选框。
    fn radio(&mut self, x: f32, n: f32, on: bool, label: &str, tip: &str, lock: bool) -> bool {
        let tw = self.measure(label, PREFS_FONT);
        let id = Id::new(("prefs-radio", x as i32, n as i32, label));
        if !self.fluent {
            return self.legacy_radio(x, n, on, label, lock);
        }
        let at = self.row(x, n);
        let (resp, m) = self.hit(Rect::from_min_size(at, Vec2::new(12.0 + tw + 8.0, 12.0)), id, lock);
        let fdark = self.th.fdark;
        let c = at + Vec2::new(5.0, 6.0);
        let p = self.ui.painter().clone();
        let fill = match (lock, on) {
            (false, true) => Theme::accent_at(5),
            (false, false) if fdark => ggray([32, 42, 42][m]),
            (false, false) => ggray([243, 229, 229][m]),
            (true, true) if fdark => ggray(119),
            (true, true) => ggray(136),
            (true, false) if fdark => ggray(0),
            (true, false) => ggray(243),
        };
        p.circle_filled(c, 6.0, fill);
        if !on || lock {
            let line = match (lock, on, fdark) {
                (false, _, false) => ggray(136),
                (false, _, true) => ggray(119),
                (true, true, false) => ggray(136),
                (true, true, true) => ggray(119),
                (true, false, false) => ggray(204),
                (true, false, true) => ggray(51),
            };
            p.circle_stroke(c, 6.0, Stroke::new(1.0_f32, line));
        }
        if !lock && (m == 2 || on) || lock && on {
            // 锁定态的圆点是另一套色（源码在 `lock = 1` 分支里单独设 255 / 0）。
            let dot = if lock {
                if fdark {
                    ggray(0)
                } else {
                    Color32::WHITE
                }
            } else if on {
                if fdark {
                    ggray(32)
                } else {
                    ggray(229)
                }
            } else if fdark {
                ggray(0)
            } else {
                ggray(243)
            };
            let r = if lock { 3.0 } else { 3.0 + if m == 1 && on { 1.0 } else { 0.0 } };
            p.circle_filled(c, r, dot);
        }
        self.text(at + Vec2::new(18.0, -1.0), label, PREFS_FONT, if lock { ggray(204) } else { self.ink() });
        resp.on_hover_text(tip).clicked()
    }

    /// 原版 `draw_button2(x, y, w, str[, lock])`。Fluent 是强调色圆角矩形，
    /// 其余三套是 `spr_button` 九宫格（没导出），退回 egui 原生按钮。
    fn button(&mut self, x: f32, n: f32, w: f32, label: &str, lock: bool) -> bool {
        let id = Id::new(("prefs-btn", x as i32, n as i32, label));
        if !self.fluent {
            return self.legacy_row(x, n, w, id, lock, |ui| {
                ui.add_sized([w, 21.0], egui::Button::new(label)).clicked()
            });
        }
        let at = self.row(x, n);
        let (resp, m) = self.hit(Rect::from_min_size(at, Vec2::new(w, 23.0)), id, lock);
        let body = Rect::from_min_size(at, Vec2::new(w - 2.0, 21.0));
        let p = self.ui.painter().clone();
        let (fill, line) = if lock {
            if self.th.fdark {
                (ggray(72), ggray(80))
            } else {
                (ggray(248), ggray(238))
            }
        } else {
            (
                Theme::accent_at(5 - m),
                Theme::accent_at([4, 5, 3][m]),
            )
        };
        p.rect_filled(body, 4.0, fill);
        p.rect_stroke(body, 4.0, Stroke::new(1.0_f32, line), StrokeKind::Inside);
        let ink = if lock {
            ggray(127)
        } else if self.th.fdark {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        let push = if m == 2 { 1.0 } else { 0.0 };
        let g = p.layout_no_wrap(label.to_string(), FontId::proportional(PREFS_FONT), ink);
        let tx = body.center().x - g.size().x / 2.0 + push;
        p.galley(Pos2::new(tx, at.y + 4.0 + push), g, Color32::BLACK);
        resp.clicked()
    }

    /// 原版 `draw_dragvalue(id, x, y, val, sens)`：就是一行纯文本数字，
    /// 命中区放大一圈（`x-8, y-4, 宽+16, 高+4`），光标变上下箭头，
    /// 按住竖向拖：`累加器 += 0.1 * 向上位移 / sens`，满 1 才进一位。
    /// `lock` 对应原版调用点在开关关掉时改用 `draw_text_dynamic` 画静态数字：
    /// 数字和左边的标题一起涂成 c_gray(128)，并且不再响应拖动（提示仍然给）。
    fn drag(
        &mut self,
        x: f32,
        n: f32,
        val: i32,
        sens: f32,
        min: i32,
        max: i32,
        lock: bool,
        tip: &str,
        accum: &mut f32,
    ) -> Option<i32> {
        let at = self.row(x, n);
        let ink = if lock { ggray(128) } else { self.ink() };
        let p = self.ui.painter().clone();
        let g = p.layout_no_wrap(val.to_string(), FontId::proportional(PREFS_FONT), ink);
        let size = g.size();
        let rect = Rect::from_min_size(at + Vec2::new(-8.0, -4.0), Vec2::new(size.x + 16.0, size.y + 4.0));
        let sense = if lock { Sense::hover() } else { Sense::drag() };
        let resp = self
            .ui
            .interact(rect, Id::new(("prefs-drag", x as i32, n as i32)), sense)
            .on_hover_cursor(CursorIcon::ResizeNorth)
            .on_hover_text(tip);
        p.galley(at, g, Color32::BLACK);
        let mut out = None;
        if resp.drag_started() {
            *accum = 0.0;
        }
        if resp.dragged() {
            *accum += 0.1 * -resp.drag_delta().y / sens;
            let step = accum.floor();
            if step != 0.0 {
                *accum -= step;
                out = Some((val + step as i32).clamp(min, max));
            }
        }
        out
    }

    /// 原版 `popup_set_window(x, y, w, h, str)`：一块只用来挂悬停提示的透明命中区。
    /// 先注册它、再注册里面的控件，提示范围就是原版那一块，而控件仍拿得到自己的悬停。
    fn tip_area(&mut self, x: f32, n: f32, w: f32, h: f32, tip: &str) {
        if tip.is_empty() {
            return;
        }
        let rect = Rect::from_min_size(self.row(x, n), Vec2::new(w, h));
        // 0.31 里 `Ui::interact` 已标 deprecated，但它是唯一能「指定 Id + 只响应悬停 +
        // 不占用布局」的入口；`put` 要求闭包控件，会改掉命中区的 id 语义。
        #[allow(deprecated)]
        {
            let id = Id::new(("prefs-tip", x as i32, n as i32, tip));
            self.ui
                .interact(rect, id, Sense::hover())
                .on_hover_text(tip);
        }
    }

    /// 原版 `draw_abutton(x1 + 123, y1 + 96)`：主题下拉框右边的箭头（`spr_button_arrow`）。
    /// 精灵图找不到时画一个同位置的三角形，仍接同一个开关——不能留个看不见的热点。
    fn arrow(&mut self, icons: &crate::icons::Icons) -> bool {
        let at = self.row(123.0, 96.0);
        match icons.bank() {
            Some(b) => b.arrow_button(self.ui, at, self.th),
            None => {
                let rect = Rect::from_min_size(at, Vec2::new(17.0, 18.0));
                let ink = self.ink();
                let p = self.ui.painter().clone();
                p.add(egui::Shape::convex_polygon(
                    vec![at + Vec2::new(4.0, 7.0), at + Vec2::new(13.0, 7.0), at + Vec2::new(8.5, 12.5)],
                    ink,
                    Stroke::NONE,
                ));
                self.ui.allocate_rect(rect, Sense::click()).clicked()
            }
        }
    }

    /// 原版 Fluent 首选项左上角的返回按钮（`spr_back`，40×40 命中区，抬手关窗）。
    fn back(&mut self, icons: &crate::icons::Icons) -> bool {
        let at = self.at(0.0, 0.0);
        match icons.bank() {
            Some(b) => b.back_button(self.ui, at, self.th),
            None => {
                let rect = Rect::from_min_size(at, Vec2::new(40.0, 40.0));
                let ink = self.ink();
                let p = self.ui.painter().clone();
                p.add(egui::Shape::convex_polygon(
                    vec![at + Vec2::new(23.0, 13.0), at + Vec2::new(23.0, 27.0), at + Vec2::new(15.0, 20.0)],
                    ink,
                    Stroke::NONE,
                ));
                self.ui.allocate_rect(rect, Sense::click()).clicked()
            }
        }
    }

    /// 非 Fluent 主题的一行：原版这里画的是精灵图（`spr_checkbox` / `spr_radiobox` /
    /// `spr_button` 我们都没导出），所以交给 egui 原生控件，坐标仍是原版坐标。
    /// 锁定的行走 `set_enabled(false)`，灰掉且不回报改动。
    fn legacy_row(&mut self, x: f32, n: f32, w: f32, id: Id, lock: bool, add: impl FnOnce(&mut Ui) -> bool) -> bool {
        let rect = Rect::from_min_size(self.row(x, n), Vec2::new(w, 20.0));
        self.ui
            .allocate_new_ui(UiBuilder::new().id_salt(id).max_rect(rect), |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.add_enabled_ui(!lock, |ui| add(ui)).inner
            })
            .inner
    }

    fn legacy_check(&mut self, x: f32, n: f32, on: bool, label: &str, lock: bool, tw: f32) -> bool {
        let id = Id::new(("prefs-check", x as i32, n as i32, label));
        self.legacy_row(x, n, 18.0 + tw + 8.0, id, lock, |ui| {
            let mut v = on;
            ui.checkbox(&mut v, label).changed()
        })
    }

    fn legacy_radio(&mut self, x: f32, n: f32, on: bool, label: &str, lock: bool) -> bool {
        let id = Id::new(("prefs-radio", x as i32, n as i32, label));
        self.legacy_row(x, n - 3.0, 18.0 + self.measure(label, PREFS_FONT) + 24.0, id, lock, |ui| {
            ui.radio(on, label).clicked() && !on
        })
    }
}

fn instruments_window(state: &mut EditorState, ui: &mut Ui) {
    egui::Grid::new("instruments_grid").num_columns(2).show(ui, |ui| {
        for (i, ins) in INSTRUMENTS.iter().enumerate() {
            if ui.selectable_label(state.current.instrument == i as u8, ins.name).clicked() {
                state.emit(Command::SetInstrument(i as u8));
            }
            // 原版只给前 16 个乐器绑了数字键（`control_draw.gml:822-843`），
            // 铜块一族没有按键就留空，不能印一个按了没反应的组合键。
            let hint = if i < crate::shortcuts::INSTRUMENT_HOTKEYS {
                if i < 9 { format!("Ctrl+{}", i + 1) } else { format!("Ctrl+Shift+{}", i - 8) }
            } else {
                String::new()
            };
            ui.label(RichText::new(hint).small().weak());
            ui.end_row();
        }
    });
    ui.separator();
    ui.label(RichText::new(format!(
        "{} 种内置乐器；自定义乐器索引 >= {}，随 .nbs 文件读写",
        INSTRUMENTS.len(),
        nbs_core::model::FIRST_CUSTOM_INDEX
    ))
    .small()
    .weak());
    footer(state, ui);
}

#[derive(Clone, Copy)]
enum Numeric {
    Tempo, Velocity, Panning, Pitch, Transpose, Tremolo, Stereo,
}

fn numeric_window(state: &mut EditorState, ui: &mut Ui, which: Numeric) {
    let (label, hint) = match which {
        Numeric::Tempo => ("速度 (tick/s)", "10 t/s = 150 BPM"),
        Numeric::Velocity => ("力度 0..100", "应用到浮动选区；无选区时应用到全曲"),
        Numeric::Panning => ("声像 0..200", "100 = 中央，<100 偏左，>100 偏右"),
        Numeric::Pitch => ("微调 -1200..1200 ¢", "100 ¢ = 1 半音"),
        Numeric::Transpose => ("移调（半音）", "正数为升，负数为降"),
        Numeric::Tremolo => ("颤音强度", "按音高奇偶交替增减力度"),
        Numeric::Stereo => ("立体声展开量", "按音符顺序左右交替偏移声像"),
    };
    ui.label(label);
    ui.label(RichText::new(hint).small().weak());
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut state.macro_amount).speed(1.0));
        ui.label("数值");
    });
    let a = state.macro_amount;
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("应用").clicked() {
            state.emit(match which {
                Numeric::Tempo => Command::SetTempo(a),
                Numeric::Velocity => Command::BatchVelocity(a.clamp(0.0, 100.0) as u8),
                Numeric::Panning => Command::BatchPanning(a.clamp(0.0, 200.0) as u8),
                Numeric::Pitch => Command::BatchPitch(a.clamp(-1200.0, 1200.0) as i16),
                Numeric::Transpose => Command::Transpose(a as i32),
                Numeric::Tremolo => Command::Tremolo(a as i32),
                Numeric::Stereo => Command::Stereo(a as i16),
            });
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn clip_editor(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new("OpenNBS 兼容的选区 RLE 文本，可与原版剪贴板互通").small().weak());
    let mut text = state.doc.clipboard_text.clone();
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .desired_width(f32::INFINITY)
            .desired_rows(6)
            .code_editor(),
    );
    ui.horizontal(|ui| {
        if ui.button("从当前选区生成").clicked() {
            state.doc.copy_selection();
        }
        if ui.button("载入并粘贴").clicked() {
            state.emit(Command::PasteText(text.clone()));
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn shortcuts_help(ui: &mut Ui) {
    ui.label(RichText::new("快捷键对齐原版 OpenNBS，另增音频与导出相关组合键").small().weak());
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        egui::Grid::new("shortcut_grid").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in SHORTCUT_HELP {
                ui.label(RichText::new(*k).monospace().strong());
                ui.label(*v);
                ui.end_row();
            }
        });
    });
}

fn file_dialog(state: &mut EditorState, ui: &mut Ui, save: bool) {
    let mut cmd: Option<Command> = None;
    let mut error: Option<String> = None;
    let (action, ext) = state
        .file_dialog
        .as_ref()
        .map(|d| (d.action, d.ext))
        .unwrap_or((crate::state::DialogAction::OpenSong, "nbs"));
    ui.label(
        RichText::new(format!(
            "文件类型：*.{}   {}",
            ext,
            if action.is_save() { "（保存 / 导出）" } else { "（打开 / 导入）" }
        ))
        .small()
        .weak(),
    );
    if let Some(d) = state.file_dialog.as_mut() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(d.dir.to_string_lossy()).small().weak());
            if ui.small_button("上级").clicked() {
                if let Some(parent) = d.dir.parent() {
                    d.dir = parent.to_path_buf();
                    d.refresh();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("文件名");
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
        });
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            let entries = d.entries.clone();
            for entry in entries {
                let name = entry.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let is_dir = entry.is_dir();
                let label = if is_dir { format!("[目录] {name}") } else { name.clone() };
                if ui.selectable_label(false, label).clicked() {
                    if is_dir {
                        d.dir = entry.clone();
                        d.refresh();
                    } else {
                        d.name = name;
                    }
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(if save { "保存" } else { "打开" }).clicked() {
                let path = d.selected_path();
                let path = if path.extension().is_none() {
                    path.with_extension(ext)
                } else {
                    path
                };
                cmd = Some(Command::DialogConfirm(path));
            }
            if ui.button("取消").clicked() {
                cmd = Some(Command::CloseWindow);
            }
        });
        error = d.error.clone();
    } else {
        cmd = Some(Command::CloseWindow);
    }
    if let Some(e) = error {
        ui.label(RichText::new(e).color(egui::Color32::RED).small());
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

fn layer_rename(state: &mut EditorState, ui: &mut Ui, layer: u32) {
    if state.editing_layer_name.is_none() {
        let name = state.doc.song.layers.get(layer as usize).map(|l| l.name.clone()).unwrap_or_default();
        state.editing_layer_name = Some((layer, name));
    }
    let mut cmd: Option<Command> = None;
    if let Some((_, name)) = state.editing_layer_name.as_mut() {
        ui.label("图层名称");
        ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("确定").clicked() {
                cmd = Some(Command::RenameLayer(layer, name.clone()));
            }
            if ui.button("取消").clicked() {
                cmd = Some(Command::CloseWindow);
            }
        });
    }
    if let Some(c) = cmd {
        state.editing_layer_name = None;
        state.emit(c);
        state.emit(Command::CloseWindow);
    }
}

fn confirm_new(state: &mut EditorState, ui: &mut Ui) {
    ui.label("当前歌曲有未保存的修改，确定新建？");
    ui.horizontal(|ui| {
        if ui.button("确定").clicked() {
            state.emit(Command::ConfirmNew);
        }
        if ui.button("取消").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn about(ui: &mut Ui) {
    ui.heading("nbs-rust");
    ui.label("Open Note Block Studio 3.11.0 (GameMaker) 的 Rust 完整重写版");
    ui.separator();
    ui.label(RichText::new("A 数据层").strong());
    for line in [
        "稀疏 BTreeMap 网格、NBS v0–v5 完整读写、自定义乐器",
        "紧凑 diff 事务（撤销只记录变化的格子）",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.label(RichText::new("B 音频 / C 调度").strong());
    for line in [
        "原版 16 个 OGG 采样，44100/48000 统一重采样",
        "pitch = 0.5·2^((key−33+pitch/100)/12)，与原版逐字节对齐",
        "voice 池 256/1024、gain/pan、cpal 实时输出、独立采样时钟",
        "离线多线程混音导出 WAV，MP3/AIFF/FLAC 经 ffmpeg 转码",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.label(RichText::new("D 网格编辑器").strong());
    for line in [
        "视口剔除、批量形状绘制、缩放/平移/边缘自动滚动",
        "框选/减选/浮动选区拖拽/吸管/全量修饰键",
        "图层栏、乐器、时间轴、钢琴、菜单、弹窗、4 套主题、15 个宏",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.label(RichText::new("E 导入导出").strong());
    for line in [
        "MIDI SMF 导入/导出、.nbp 片段、.schematic / 结构 NBT、PNG 谱面",
        "JSON 偏好、最近文件、原子保存、自动备份",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.separator();
    ui.label(RichText::new("egui 0.31 · glow（可切 wgpu）· cpal · Rust 2021").small().weak());
}

// ---------------------------------------------------------------- B/C/E 弹窗

fn audio_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label("与原版相同：先离线渲染 PCM s16 WAV，MP3/AIFF/FLAC 再交给 ffmpeg 转码。");
    if let Some(s) = &state.export_status {
        ui.label(RichText::new(s).small().color(egui::Color32::from_rgb(20, 130, 60)));
    }
    ui.separator();
    egui::Grid::new("audio_export_grid").num_columns(2).show(ui, |ui| {
        // 原版四个选项：导出格式 / 采样率 / 通道 / 包括已静音的层。
        ui.label("导出格式");
        let mut fmt = state.prefs.export_format.clone();
        egui::ComboBox::from_id_salt("export_format")
            .selected_text(fmt.clone())
            .show_ui(ui, |ui| {
                for f in nbs_io::audio_export::AudioFormat::ALL {
                    ui.selectable_value(&mut fmt, f.name().into(), f.name());
                }
            });
        if fmt != state.prefs.export_format {
            state.prefs.export_format = fmt;
        }
        ui.end_row();
        ui.label("采样率");
        let mut rate = state.prefs.export_sample_rate;
        egui::ComboBox::from_id_salt("export_rate")
            .selected_text(hz_label(rate))
            .show_ui(ui, |ui| {
                for r in nbs_io::audio_export::SAMPLE_RATES {
                    ui.selectable_value(&mut rate, r, hz_label(r));
                }
            });
        if rate != state.prefs.export_sample_rate {
            state.prefs.export_sample_rate = rate;
        }
        ui.end_row();
        ui.label("通道");
        let mut ch = state.prefs.export_channels;
        ui.radio_value(&mut ch, 1, "1（单声道）").on_hover_text("导出单声道的音频。");
        ui.radio_value(&mut ch, 2, "2（立体声）").on_hover_text("导出立体声的音频。");
        if ch != state.prefs.export_channels {
            state.prefs.export_channels = ch;
        }
        ui.end_row();
    });
    let mut locked = state.prefs.include_locked_in_export;
    if ui
        .checkbox(&mut locked, "包括已静音的层")
        .on_hover_text("是否在音频内包含已静音的层。")
        .changed()
    {
        state.prefs.include_locked_in_export = locked;
    }
    let needs_encoder = nbs_io::audio_export::AudioFormat::parse(&state.prefs.export_format)
        .map(|f| f.needs_encoder())
        .unwrap_or(false);
    if needs_encoder {
        // 原版随程序附带 ffmpeg.exe；本程序按 audio_export::find_ffmpeg 的顺序找，
        // 这里显示真实结果，找不到就明说，避免「按了导出却没产物」。
        let line = match nbs_io::audio_export::ffmpeg_version() {
            Some(v) => v,
            None => "未找到 ffmpeg：把它放进程序目录、datafiles\\ 或 PATH，也可设 NBS_FFMPEG".into(),
        };
        ui.label(RichText::new(line).small().weak());
    }
    ui.label(RichText::new(format!(
        "本曲 {} 音符 / {:.1} 秒 / {:.2} t/s",
        state.doc.song.total_blocks,
        state.doc.song.song_length() as f32 / state.doc.song.meta.tempo,
        state.doc.song.meta.tempo
    )).small().weak());
    ui.separator();
    if ui.button("选择输出路径并导出…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ExportAudio));
    }
    footer(state, ui);
}

/// 原版 `format_sample_rate`：44100 → "44,100 Hz"。
fn hz_label(rate: u32) -> String {
    let digits: Vec<char> = rate.to_string().chars().collect();
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    out + " Hz"
}

fn audio_settings_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new(format!("引擎：{}", state.audio.name())).strong());
    ui.label(RichText::new(state.audio.device_info()).small().weak());
    ui.label(RichText::new(state.audio.sample_status()).small().weak());
    ui.label(RichText::new(format!("当前活跃 voice：{}", state.audio.active_voices())).small().weak());
    ui.separator();
    let mut master = state.prefs.master_volume;
    if ui.add(egui::Slider::new(&mut master, 0.0..=2.0).text("主音量")).changed() {
        state.emit(Command::SetMasterVolume(master));
    }
    let mut speed = state.prefs.play_speed;
    if ui.add(egui::Slider::new(&mut speed, 0.25..=4.0).text("播放速度")).changed() {
        state.emit(Command::SetPlaySpeed(speed));
    }
    let mut metronome = state.prefs.metronome;
    if ui.checkbox(&mut metronome, "节拍器（每 4 tick）").changed() {
        state.emit(Command::ToggleMetronome);
    }
    // 「播放」那四个开关和速度单位属于首选项的播放页，这里不重复一份。
    ui.separator();
    ui.label("内置采样目录（留空 = 自动搜索）");
    let mut dir = state.prefs.sounds_dir.clone().unwrap_or_default();
    ui.add(egui::TextEdit::singleline(&mut dir).desired_width(f32::INFINITY).hint_text("自动"));
    if ui.button("应用并重新加载采样").clicked() {
        state.emit(Command::SetSoundsDir(dir));
    }
    ui.separator();
    if ui.button("保存偏好").clicked() {
        state.emit(Command::SavePreferences);
    }
    footer(state, ui);
}

fn midi_import_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new("量化：tick = floor((midi_tick − 起始空白) / (PPQ / (4 × 精度倍率)))").small().weak());
    ui.separator();
    egui::Grid::new("midi_import_grid").num_columns(2).show(ui, |ui| {
        ui.label("时间精度");
        let mut p = state.midi_options.precision;
        egui::ComboBox::from_id_salt("midi_precision")
            .selected_text(format!("{}×", p + 1))
            .show_ui(ui, |ui| {
                for v in [0u8, 1, 3, 7] {
                    ui.selectable_value(&mut p, v, format!("{}×", v + 1));
                }
            });
        state.midi_options.precision = p;
        ui.end_row();
        ui.label("八度偏移");
        ui.add(egui::Slider::new(&mut state.midi_options.octave_shift, -24..=24).suffix(" 半音"));
        ui.end_row();
    });
    ui.checkbox(&mut state.midi_options.remove_silent, "去掉开头空白");
    ui.checkbox(&mut state.midi_options.use_velocity, "使用 MIDI 力度（上限 100）");
    ui.checkbox(&mut state.midi_options.fold_octaves, "折叠到 MC 音域 33..57");
    ui.checkbox(&mut state.midi_options.tempo_changer, "写入 Tempo Changer 分段速度");
    ui.checkbox(&mut state.midi_options.split_channels, "按 MIDI 通道分层");
    ui.separator();
    if ui.button("选择 MIDI 文件并导入…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ImportMidi));
    }
    footer(state, ui);
}

/// 原版 `draw_window_schematic_export`：设计 / 方块两页 + 记住我的更改 + 三个底部按钮。
fn schematic_export_window(state: &mut EditorState, ui: &mut Ui) {
    let mut tab = state.schematic_tab;
    ui.horizontal(|ui| {
        for (i, name) in ["设计", "方块"].iter().enumerate() {
            if ui.selectable_label(tab == i as u8, *name).clicked() {
                tab = i as u8;
            }
        }
    });
    state.schematic_tab = tab;
    ui.separator();

    let stats = state.schematic_stats.clone();
    let names = state.instrument_list();
    let opts = &mut state.schematic_options;
    if tab == 0 {
        design_tab(opts, &stats, ui);
    } else {
        blocks_tab(opts, &names, ui);
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.schematic_remember, "记住我的更改")
            .on_hover_text("下次导出 Schematic 时是否使用同样的设定。");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("使用默认值").clicked() {
                state.modals = ModalQueue::question("确定", "你确定吗？", Command::ResetSchematicOptions);
            }
            if ui.button("取消").clicked() {
                state.emit(Command::CloseWindow);
            }
            if ui.button("导出").clicked() {
                state.emit(Command::ExportSchematicChecked);
            }
        });
    });
}

/// 原版的方块候选菜单：分组标题 + 「id, data — 名称」，当前项高亮。
fn block_menu(
    ui: &mut Ui,
    id: &str,
    current: nbs_io::schematic::McBlock,
    sections: &[nbs_io::schematic::BlockMenuSection],
    label: String,
) -> Option<nbs_io::schematic::McBlock> {
    let mut out = None;
    egui::ComboBox::from_id_salt(id)
        .width(240.0)
        .selected_text(label)
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for sec in sections {
                    if let Some(t) = sec.title {
                        ui.label(RichText::new(t.to_string()).small().strong());
                    }
                    for m in &sec.items {
                        let text =
                            format!("{}, {}  {}", m.id, m.data, nbs_io::schematic::block_display_name(m.id, m.data));
                        if ui.selectable_label(*m == current, text).clicked() {
                            out = Some(*m);
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                }
            });
        });
    out
}

fn design_tab(o: &mut nbs_io::schematic::SchematicOptions, st: &nbs_io::schematic::SchematicStats, ui: &mut Ui) {
    use nbs_io::schematic::{calc_size, export_guard};
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label("分布:");
            if ui.radio(o.layout == 1, "普通过道").on_hover_text("生成过道延伸到歌曲末尾。").clicked() {
                o.layout = 1;
            }
            if ui.radio(o.layout == 0, "环形过道").on_hover_text("生成让玩家来回走的过道。").clicked() {
                o.layout = 0;
            }
            ui.label("导出为 Minecraft 版本:");
            if ui.radio(o.structure, "1.13+").on_hover_text("创建一个兼容 1.13+ 的结构方块文件。").clicked() {
                o.structure = true;
            }
            if ui
                .radio(!o.structure && !o.minecraft_old, "1.11-1.12")
                .on_hover_text("创建一个兼容 1.11 和 1.12 的 Schematic 文件。")
                .clicked()
            {
                o.minecraft_old = false;
                o.structure = false;
            }
            if ui
                .radio(!o.structure && o.minecraft_old, "pre 1.11")
                .on_hover_text("创建一个只兼容 1.11 之前旧版本的 Schematic 文件。")
                .clicked()
            {
                o.minecraft_old = true;
                o.structure = false;
            }
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            let mut n = o.notesperrow as f64;
            ui.horizontal(|ui| {
                ui.label("每行中继器个数:");
                if ui.add(egui::DragValue::new(&mut n).speed(1.0).range(5.0..=100.0)).changed() {
                    o.set_notesperrow(n as usize);
                }
            });
            ui.checkbox(&mut o.include_locked, "包括已静音的层")
                .on_hover_text("是否在 Schematic 内包括已静音的层。");
            ui.checkbox(&mut o.compress, "压缩每层").on_hover_text("压缩每层以节省竖向空间。");
            if o.layout < 2 {
                ui.checkbox(&mut o.minecart, "包括矿车轨道")
                    .on_hover_text("包括一个跟随歌曲进度的矿车轨道。");
                ui.indent("sch_chest", |ui| {
                    ui.add_enabled_ui(o.minecart, |ui| {
                        ui.checkbox(&mut o.chest, "添加矿车箱")
                            .on_hover_text("是否在歌曲开始处添加一个装满矿车的箱子。");
                    });
                });
                if o.layout == 0 {
                    ui.checkbox(&mut o.loop_option, "包括循环选项")
                        .on_hover_text("是否添加开关循环功能的拉杆。");
                }
                if o.structure {
                    ui.checkbox(&mut o.command_block, "使用命令方块")
                        .on_hover_text("是否使用命令方块代替音符盒以获得更广音域。\n（需要更多音符资源包）");
                }
            } else {
                // 原版这里连中文界面都没翻译，照抄。
                ui.checkbox(&mut o.glass, "Create glass floor")
                    .on_hover_text("Whether a glass floor should make all the note blocks visible.");
            }
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            let size = calc_size(st, o);
            let incl = o.include_locked as usize;
            egui::Grid::new("sch_stats").num_columns(2).show(ui, |ui| {
                ui.label("音符盒:");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(st.totalblocks[incl].to_string());
                });
                ui.end_row();
                ui.label("中继器:");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}x{}", st.repeaters[incl][o.tempo_mode as usize], size.layers));
                });
                ui.end_row();
                ui.label("大小:");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}x{}x{}", size.len, size.wid, size.hei));
                });
                ui.end_row();
            });
            ui.label("速度:");
            for (i, name) in ["10 红石刻 / 秒", "5 红石刻 / 秒", "2.5 红石刻 / 秒"].iter().enumerate() {
                if ui.radio(o.tempo_mode == i as u8, *name).on_hover_text("生成对应速度的歌曲").clicked() {
                    o.tempo_mode = i as u8;
                }
            }
            // 超限的红字提示与原版的报错同源。
            if let Err(e) = export_guard(st, o) {
                ui.label(RichText::new(e.replace('\n', " ")).small().color(egui::Color32::from_rgb(200, 90, 90)));
            }
        });
    });
}

fn blocks_tab(
    o: &mut nbs_io::schematic::SchematicOptions,
    names: &[(u8, String)],
    ui: &mut Ui,
) {
    use nbs_io::schematic::{block_display_name, instrument_block_menu, misc_block_menu, McBlock, INS_COUNT};
    let rows = names.len().min(INS_COUNT);
    let mut pick_ins: Option<(usize, McBlock)> = None;
    egui::Grid::new("sch_ins_table").num_columns(3).striped(true).show(ui, |ui| {
        ui.strong("音色");
        ui.strong("方块");
        ui.strong("方块名称");
        ui.end_row();
        for b in 0..rows {
            ui.label(&names[b].1);
            let cur = McBlock {
                id: o.ins_block.get(b).copied().unwrap_or(1),
                data: o.ins_data.get(b).copied().unwrap_or(0),
            };
            let menu = instrument_block_menu(b);
            if let Some(m) = block_menu(
                ui,
                &format!("schexport_insblock_{b}"),
                cur,
                &menu,
                format!("{}, {}", cur.id, cur.data),
            ) {
                pick_ins = Some((b, m));
            }
            ui.label(block_display_name(cur.id, cur.data));
            ui.end_row();
        }
    });
    if let Some((b, m)) = pick_ins {
        // 原版 `menu_click` 的 `schexport_insblock` 分支。
        if b < o.ins_block.len() {
            o.ins_block[b] = m.id;
        }
        if b < o.ins_data.len() {
            o.ins_data[b] = m.data;
        }
    }

    ui.separator();
    let slots = [
        ("过道所用方块:", &mut o.walkway, 0u8, "用于过道和除电路和地面的方块。"),
        ("电路所用方块:", &mut o.circuit, 1, "为电路用的方块。"),
        ("地面所用方块:", &mut o.ground, 2, "为地面用的方块。选草方块会刷一堆生物。"),
    ];
    for (label, slot, which, tip) in slots {
        ui.horizontal(|ui| {
            ui.label(label);
            let cur = *slot;
            let menu = misc_block_menu(which);
            if let Some(m) = block_menu(
                ui,
                &format!("schexport_block_{which}"),
                cur,
                &menu,
                block_display_name(cur.id, cur.data),
            ) {
                *slot = m;
            }
            ui.label(RichText::new(format!("{}, {}", cur.id, cur.data)).small().weak());
        });
        ui.label(RichText::new(tip).small().weak());
    }
}

/// 原版 `draw_window_datapack_export` 里 10 个声音类型的中文标签与悬停说明。
/// 元组第三项写进命令的必须是英文内部值，中文只是界面标签。
const SOUND_SOURCES_ZH: [(&str, &str, &str); 10] = [
    ("ambient", "环境", "由环境音量控制"),
    ("block", "方块", "由方块音量控制"),
    ("hostile", "敌对", "由敌对生物音量控制"),
    ("master", "主源", "由主音量控制"),
    ("music", "音乐", "由音乐音量控制"),
    ("neutral", "中立", "由友好生物音量控制"),
    ("player", "玩家", "由玩家音量控制"),
    ("record", "唱片", "由唱片机/音符盒音量控制"),
    ("voice", "语音", "由语音音量控制"),
    ("weather", "天气", "由天气音量控制"),
];

/// 与 `VisType::ALL` 同序的中文标签。
const VIS_TYPES_ZH: [(&str, &str); 7] = [
    ("弧形", "使用弧形效果器。"),
    ("下落", "使用下落效果器。"),
    ("上升", "使用上升效果器。"),
    ("弹跳", "使用弹跳效果器。"),
    ("钢琴", "使用钢琴式效果器。"),
    ("喷泉", "使用喷泉效果器。"),
    ("立体", "使用立体音效效果器。"),
];

fn datapack_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (i, tab) in ["设置", "效果"].iter().enumerate() {
            if ui.selectable_label(state.datapack_tab == i as u8, *tab).clicked() {
                state.datapack_tab = i as u8;
            }
        }
    });
    ui.separator();

    let tab = state.datapack_tab;
    let enda = state.doc.song.enda as f32;
    // 循环起点属于歌曲自身（原版的拖动条直接改 loopstart），先取到局部再写回。
    let mut loop_start = state.doc.song.meta.loop_start as f32;
    let o = &mut state.datapack_options;
    if tab == 0 {
        egui::Grid::new("datapack_grid").num_columns(2).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label("唯一名称:");
                ui.add(
                    egui::TextEdit::singleline(&mut o.name)
                        .desired_width(145.0)
                        .hint_text("<enter name>"),
                )
                .on_hover_text("该名称会作为在游戏里播放使用的命令名称。");
                ui.label("命名空间:");
                ui.add(
                    egui::TextEdit::singleline(&mut o.namespace)
                        .desired_width(145.0)
                        .hint_text("(可选)"),
                )
                .on_hover_text("（可选）将命令名称更改到自定义命名空间下。如为空，命名空间会设为歌曲名称。");
                ui.label("路径:");
                ui.add(
                    egui::TextEdit::singleline(&mut o.path)
                        .desired_width(145.0)
                        .hint_text("(可选)"),
                )
                .on_hover_text("（可选）从“functions”文件夹到歌曲的路径。用“/”来套文件夹。");
                ui.label("命令预览:");
                let preview = nbs_io::datapack::command_preview(&o.name, &o.namespace, &o.path);
                ui.add(
                    egui::Label::new(RichText::new(preview).monospace().strong())
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Truncate),
                );
                ui.label("声音类型");
                for (id, zh, tip) in &SOUND_SOURCES_ZH[..5] {
                    if ui.radio(o.source == *id, *zh).on_hover_text(*tip).clicked() {
                        o.source = (*id).to_string();
                    }
                }
                ui.label("Minecraft 版本");
                if ui
                    .radio(o.mc_version == 0, "1.13-1.20")
                    .on_hover_text("导出至 Minecraft: Java 版\n版本 1.13 至 1.20.6。")
                    .clicked()
                {
                    o.mc_version = 0;
                }
                if ui
                    .radio(o.mc_version == 1, "1.21+")
                    .on_hover_text("导出至 Minecraft: Java 版\n版本 1.21 及以上。")
                    .clicked()
                {
                    o.mc_version = 1;
                }
                ui.checkbox(&mut o.use_zip, "导出为 ZIP")
                    .on_hover_text("是否导出为 ZIP 格式的文件。如关闭，将导出为文件夹。");
            });
            ui.vertical(|ui| {
                ui.label(" ");
                ui.label(" ");
                ui.label(" ");
                ui.checkbox(&mut o.include_locked, "包括已静音的层")
                    .on_hover_text("是否在数据包内包括已静音的层。");
                ui.checkbox(&mut o.include_out_of_range, "包括超范围音符")
                    .on_hover_text("是否包括超过 Minecraft 的 2 八度限制的音符。此选项需要安装额外的数据包。");
                ui.checkbox(&mut o.enable_radius, "共享音乐")
                    .on_hover_text("是否共享音乐给一定范围内的玩家。");
                ui.horizontal(|ui| {
                    ui.label("范围（方块）:");
                    let mut r = o.radius as f32;
                    ui.add_enabled_ui(o.enable_radius, |ui| {
                        if ui
                            .add(egui::DragValue::new(&mut r).range(16.0..=100000.0).speed(2.0))
                            .changed()
                        {
                            o.radius = (r as i64).clamp(16, 100_000) as u32;
                        }
                    });
                });
                ui.label(RichText::new(format!("实际音量半径 = {:.2}", o.radius_value()))
                    .small()
                    .weak());
                ui.checkbox(&mut o.enable_looping, "启用循环")
                    .on_hover_text("如果开启，歌曲播放到最后将会循环。");
                ui.horizontal(|ui| {
                    ui.label("循环开始:");
                    ui.add_enabled_ui(o.enable_looping, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut loop_start)
                                .range(0.0..=enda)
                                .speed(0.5),
                        )
                        .on_hover_text("播放完循环时回到的时刻。");
                    });
                });
            });
            ui.end_row();
        });
    } else {
        ui.checkbox(&mut o.visualizer, "启用效果器")
            .on_hover_text("注意：请使用一个虚空地图不然方块就会堆起来！");
        ui.label("效果类型");
        for (vis, (zh, tip)) in nbs_io::datapack::VisType::ALL.iter().zip(VIS_TYPES_ZH) {
            if ui.radio(o.vis_type == *vis, zh).on_hover_text(tip).clicked() {
                o.vis_type = *vis;
            }
        }
        ui.checkbox(&mut o.glow, "发光效果")
            .on_hover_text("是否为每个音符盒添加发光效果。");
        ui.label("生成高度:");
        ui.add(egui::DragValue::new(&mut o.y_val).range(0.0..=255.0).speed(0.5));
        ui.label("地图中位置:");
        ui.horizontal(|ui| {
            ui.label("X值:");
            ui.add(egui::DragValue::new(&mut o.x_val).speed(0.5));
        });
        ui.horizontal(|ui| {
            ui.label("Z值:");
            ui.add(egui::DragValue::new(&mut o.z_val).speed(0.5));
        });
        if o.visualizer {
            ui.label(RichText::new(format!(
                "效果器类型：{}，音符盒生成在 y = {}",
                o.vis_type.gml_name(),
                o.y_val
            ))
            .small()
            .weak());
        }
    }

    if loop_start != state.doc.song.meta.loop_start as f32 {
        state.doc.edit_meta("设置循环起点", |m| m.loop_start = loop_start as u16);
        state.sync_loop_params();
    }

    if let Some(s) = &state.export_status {
        ui.separator();
        ui.label(RichText::new(s).small());
    }
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("导出").clicked() {
            state.emit(Command::OpenDialog(crate::state::DialogAction::ExportDatapack));
        }
        if ui.button("使用默认值").clicked() {
            // 原版是 dat_reset(1) + 一个确认框；这里直接重置并给出提示。
            state.datapack_options = nbs_io::datapack::DatapackOptions::default();
            state.export_status = None;
            state.set_message("已恢复数据包导出默认设置");
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn png_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label("把整首歌的网格画成 PNG 图片，颜色对应乐器，亮度对应力度。");
    ui.label(RichText::new("自动缩小格子以适配最大像素尺寸；不受屏幕视口限制。").small().weak());
    ui.separator();
    if let Some(s) = &state.export_status {
        ui.label(RichText::new(s).small().weak());
    }
    if ui.button("选择输出路径并导出…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ExportPng));
    }
    footer(state, ui);
}

fn macro_editor_window(state: &mut EditorState, ui: &mut Ui) {
    let macros = Macro::all();
    ui.label(RichText::new(format!(
        "作用对象：{}（无选区时对全曲生效）",
        if state.doc.selection.is_empty() {
            "全曲".to_string()
        } else {
            format!("浮动选区 {} 音符", state.doc.selection.len())
        }
    )).small().weak());
    ui.separator();
    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
        for (i, m) in macros.iter().enumerate() {
            if ui.selectable_label(state.macro_index == i, m.name()).clicked() {
                state.macro_index = i;
                state.macro_amount = match i {
                    6 => 2.0,   // Tremolo spacing
                    7 => 60.0,  // Stereo width
                    8 => 0.0,   // Arpeggio 用 pattern
                    9 => 300.0, // Portamento cents
                    12 => 100.0,
                    15 => 0.0,
                    _ => 20.0,
                };
            }
        }
    });
    ui.separator();
    ui.add(egui::Slider::new(&mut state.macro_amount, -1200.0..=1200.0).text("参数值"));
    ui.horizontal(|ui| {
        ui.label("序列 (| 分隔)");
        ui.add(egui::TextEdit::singleline(&mut state.macro_pattern).desired_width(160.0));
    });
    ui.checkbox(&mut state.macro_reverse, "反转 / 叠加模式");
    ui.label(RichText::new(format!("当前：{}", state.current_macro().name())).small().weak());
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("应用宏").clicked() {
            state.emit(Command::RunMacro(state.current_macro()));
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn autosaves_window(state: &mut EditorState, ui: &mut Ui) {
    let files = nbs_io::prefs::list_autosaves();
    ui.label(RichText::new(format!("备份目录：{}", nbs_io::prefs::Preferences::backup_dir().display())).small().weak());
    ui.separator();
    if files.is_empty() {
        ui.label("暂无自动备份");
    }
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
        for p in files {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if ui.button(&name).clicked() {
                state.emit(Command::RestoreAutosave(p.clone()));
            }
        }
    });
    ui.separator();
    ui.add(egui::Slider::new(&mut state.prefs.auto_save_minutes, 0..=60).text("自动保存间隔 (分钟，0=关闭)"));
    footer(state, ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_label_inserts_the_original_commas() {
        assert_eq!(hz_label(44100), "44,100 Hz");
        assert_eq!(hz_label(8000), "8,000 Hz");
        assert_eq!(hz_label(384000), "384,000 Hz");
        assert_eq!(hz_label(192000), "192,000 Hz");
    }
}
