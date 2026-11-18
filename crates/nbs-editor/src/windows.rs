//! 弹窗集合：歌曲信息 / 属性 / 统计 / 首选项 / 音色 / 宏窗口 / 文件对话框 /
//! 图层重命名 / 快捷键帮助 / 关于。原版 47 个 `w_*` 窗口状态收敛成一个枚举。

use egui::{
    Color32, Context, CursorIcon, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};
use nbs_core::instruments::{instrument_name, INSTRUMENTS};
use nbs_core::model::{key_name, SongMeta};
use nbs_core::{Macro, TremoloFade};

use crate::commands::Command;
use crate::state::{
    AccentPick, DialogAction, EditorState, Flag, ModalQueue, PrefsFolder, WindowKind,
};
use crate::theme::{Theme, ThemeKind};
use crate::tr;

pub fn show_windows(state: &mut EditorState, ctx: &Context) {
    show_modal(state, ctx);
    let Some(kind) = state.window.clone() else {
        return;
    };
    if kind == WindowKind::Greeting {
        // 原版 `draw_window(x1, y1, x1 + 700, y1 + 430)`（draw_window_greeting.gml:38）
        // 只画边框不写标题，欢迎窗不要标题栏。
        egui::Window::new("Note Block Studio")
            .id(egui::Id::new("nbs_win_Greeting"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_width(700.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| greeting(state, ui));
        // 捐赠横幅在欢迎窗右侧窗外（原版 :10-36），egui 窗口内容裁剪到自己的矩形，
        // 只能再用一张无边框小窗承载；精灵缺失时不画——没有图的横幅只剩看不见的热点。
        if state.donate_banner && state.icons.bank().is_some() {
            donate_banner_window(state, ctx);
        }
        return;
    }
    let mut open = true;
    let title = match &kind {
        // 原版在标题带左边写「导入MIDI」、右边右对齐写文件名
        // （`draw_text_dynamic(x1 + 590, y1 + 6, songs[song].midifile, true)`）。
        // egui 的标题带归 `Window` 自己，文件名只能并到标题文字里。
        WindowKind::MidiImport if !state.midi_file.is_empty() => {
            format!("{} · {}", kind.title(), state.midi_file)
        }
        _ => kind.title().to_string(),
    };
    let default_width = match kind {
        // 原版 `draw_window(x1, y1, x1 + 500, y1 + 510)`。
        WindowKind::Preferences => 500.0,
        // 原版 `draw_window(x1, y1, x1 + 500 + theme_offset, y1 + 400)`：
        // `theme_offset` 只有非 Fluent 才 +20（那三套字体更宽）。
        WindowKind::Changelist | WindowKind::Update => {
            if state.theme.kind == ThemeKind::Fluent {
                500.0
            } else {
                520.0
            }
        }
        // 原版 `draw_window(x1, y1, x1 + 580, y1 + 410)`，两个主题组都是这个尺寸。
        WindowKind::About => 580.0,
        WindowKind::Shortcuts => 460.0,
        // 原版 `draw_window(x1, y1, x1 + 280, y1 + 440)`（draw_window_set_accent.gml:10）。
        WindowKind::SetAccent => 280.0,
        WindowKind::FileOpen | WindowKind::FileSave => 520.0,
        WindowKind::ClipEditor => 500.0,
        // 原版数据包导出窗口 550×430，两列布局。
        WindowKind::DatapackExport => 560.0,
        // 原版 `draw_window_schematic_export` 是 550×400。
        WindowKind::SchematicExport => 560.0,
        // 原版 `draw_window(x1, y1, x1 + 490, y1 + 450)`。
        WindowKind::MinecraftCompat => 490.0,
        // 原版 `draw_window(x1, y1, x1 + 600, y1 + 400 + navigation_space)`。
        WindowKind::MidiImport => 600.0,
        // 原版 `draw_window(x1, y1, x1 + 500, y1 + 350)`。
        WindowKind::MidiDevices => 500.0,
        // 原版 `draw_window(x1, y1, x1 + 415, y1 + 310)`。
        WindowKind::SoundImport => 415.0,
        // 原版 `draw_window(x1, y1, x1 + 140, y1 + 130/162/180)`。
        WindowKind::TempoTapper
        | WindowKind::SaveOptions
        | WindowKind::Arpeggio
        | WindowKind::Portamento
        | WindowKind::SetVelocity
        | WindowKind::SetPanning
        | WindowKind::SetPitch
        | WindowKind::Stereo
        | WindowKind::EditTempoChanger(..)
        | WindowKind::EditSoundStopper(..) => 140.0,
        // 原版 `draw_window_macro_stagger`：`draw_window(x1, y1, x1 + 150, y1 + 160)`；
        // `draw_window_macro_tremolo` 同宽，240 高。
        WindowKind::Stagger | WindowKind::Tremolo => 150.0,
        _ => 360.0,
    };
    egui::Window::new(title)
        .id(egui::Id::new(format!("nbs_win_{kind:?}")))
        .open(&mut open)
        .collapsible(false)
        .resizable(!matches!(
            kind,
            WindowKind::TempoTapper
                | WindowKind::SaveOptions
                | WindowKind::About
                | WindowKind::Arpeggio
                | WindowKind::Portamento
                | WindowKind::Stagger
                | WindowKind::SetVelocity
                | WindowKind::SetPanning
                | WindowKind::SetPitch
                | WindowKind::Stereo
                | WindowKind::Tremolo
                | WindowKind::SetAccent
                | WindowKind::EditTempoChanger(..)
                | WindowKind::EditSoundStopper(..)
        ))
        .default_width(default_width)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| match &kind {
            WindowKind::SongInfo => song_info(state, ui),
            WindowKind::Properties => properties(state, ui),
            WindowKind::Statistics => statistics(state, ui),
            WindowKind::Preferences => preferences(state, ui),
            WindowKind::Instruments => instruments_window(state, ui),
            WindowKind::About => about(state, ui),
            // 原版 `draw_windows.gml:56-57`：`w_update` 和 `w_changelist` 都进
            // `draw_window_update()`，函数里再看 `window` 是哪个。
            WindowKind::Changelist | WindowKind::Update => changelog_window(state, ui),
            WindowKind::TempoTapper => tempo_tapper_window(state, ui),
            WindowKind::SaveOptions => save_options_window(state, ui),
            WindowKind::Arpeggio => arpeggio_window(state, ui),
            WindowKind::Portamento => portamento_window(state, ui),
            WindowKind::Stagger => stagger_window(state, ui),
            WindowKind::SetVelocity => batch_attr_window(state, ui, BatchAttr::Velocity),
            WindowKind::SetPanning => batch_attr_window(state, ui, BatchAttr::Panning),
            WindowKind::SetPitch => batch_attr_window(state, ui, BatchAttr::Pitch),
            WindowKind::Tremolo => tremolo_window(state, ui),
            WindowKind::Stereo => stereo_window(state, ui),
            WindowKind::ClipEditor => clip_editor(state, ui),
            WindowKind::Shortcuts => shortcuts_help(ui),
            WindowKind::FileOpen => file_dialog(state, ui, false),
            WindowKind::FileSave => file_dialog(state, ui, true),
            WindowKind::LayerRename(layer) => layer_rename(state, ui, *layer),
            WindowKind::ConfirmNew => confirm_new(state, ui),
            WindowKind::AudioExport => audio_export_window(state, ui),
            WindowKind::AudioSettings => audio_settings_window(state, ui),
            WindowKind::MidiImport => midi_import_window(state, ui),
            WindowKind::MidiDevices => midi_devices_window(state, ui),
            WindowKind::SoundImport => sound_import_window(state, ui),
            WindowKind::SchematicExport => schematic_export_window(state, ui),
            WindowKind::DatapackExport => datapack_export_window(state, ui),
            WindowKind::PngExport => png_export_window(state, ui),
            WindowKind::Autosaves => autosaves_window(state, ui),
            WindowKind::MinecraftCompat => minecraft_compat_window(state, ui),
            WindowKind::EditTempoChanger(tick, layer) => {
                event_tempo_window(state, ui, *tick, *layer)
            }
            WindowKind::EditSoundStopper(tick, layer) => {
                event_stopper_window(state, ui, *tick, *layer)
            }
            WindowKind::SetAccent => set_accent_window(state, ui),
            // Greeting 在上面已提前 return，这个分支实际到不了。
            WindowKind::Greeting => {}
        });
    if !open {
        state.window = None;
    }
}

/// 原版的 `message()` / `question()`：一次一个，答完才轮到下一个，
/// 中途答「否」丢掉整个队列。窗口右上角的叉等价于「确定」/「否」。
fn show_modal(state: &mut EditorState, ctx: &Context) {
    let Some(modal) = state.modals.items.front().cloned() else {
        return;
    };
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
                    if ui.button(tr("No", "否")).clicked() {
                        answer = Some(false);
                    }
                    if ui.button(tr("Yes", "是")).clicked() {
                        answer = Some(true);
                    }
                } else if ui.button(tr("OK", "确定")).clicked() {
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
    if ui.button(tr("Close", "关闭")).clicked() {
        state.emit(Command::CloseWindow);
    }
}

/// 原版 `mouse_rectangle` + `m += mouse_check_button(左)`：0 = 不在、1 = 悬停、
/// 2 = 悬停且按住。与 `Page::hit` 同一套，只是欢迎窗不走 Page。
fn greet_hit(ui: &mut Ui, rect: Rect, id: Id) -> (egui::Response, usize) {
    let resp = ui.interact(rect, id, Sense::click());
    let held = resp.hovered() && ui.input(|i| i.pointer.any_down());
    let m = if held {
        2
    } else if resp.hovered() {
        1
    } else {
        0
    };
    (resp, m)
}

/// 原版 `draw_text_center`（x 为中心）。重写版没有粗体字型，原版的
/// `font_main_bold` / `font_info_med_bold` 用 0.5px 重叠双绘近似。
fn greet_text_center(
    p: &egui::Painter,
    center_x: f32,
    y: f32,
    s: &str,
    font: f32,
    color: Color32,
    bold: bool,
) {
    let g = p.layout_no_wrap(s.to_string(), FontId::proportional(font), color);
    let px = center_x - g.size().x / 2.0;
    p.galley(egui::pos2(px, y), g.clone(), Color32::BLACK);
    if bold {
        p.galley(egui::pos2(px + 0.5, y), g, Color32::BLACK);
    }
}

/// 原版 `draw_text_dynamic`（左上角对齐）。
fn greet_text_left(p: &egui::Painter, at: Pos2, s: &str, font: f32, color: Color32, bold: bool) {
    let g = p.layout_no_wrap(s.to_string(), FontId::proportional(font), color);
    p.galley(at, g.clone(), Color32::BLACK);
    if bold {
        p.galley(at + Vec2::new(0.5, 0.0), g, Color32::BLACK);
    }
}

/// 原版 `draw_text_dynamic` 的居中多行形态（关于窗那一大块声明）。英文分支直接丢给
/// 引擎的 `draw_text`，中文分支逐字排版、每遇到换行 `line_y += 16`
/// （`draw_text_dynamic.gml`）——两套字体的行距都是 16 像素，这里统一按 16 走一行。
fn greet_text_center_lines(
    p: &egui::Painter,
    center_x: f32,
    y: f32,
    s: &str,
    font: f32,
    color: Color32,
) {
    for (i, line) in s.split('\n').enumerate() {
        greet_text_center(p, center_x, y + 16.0 * i as f32, line, font, color, false);
    }
}

/// 原版 `string_truncate(str, maxwidth)`：逐字累加宽度，放不下补 `...`。
fn greet_truncate(p: &egui::Painter, s: &str, max_w: f32, font: f32) -> String {
    let width = |t: &str| {
        p.layout_no_wrap(t.to_string(), FontId::proportional(font), Color32::WHITE)
            .size()
            .x
    };
    if width(s) <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0.0;
    for c in s.chars() {
        let cw = width(&c.to_string());
        if w + cw > max_w {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push_str("...");
    out
}

/// 原版 `draw_text_url(x, y, text, url)`：文字以 x 为中心，y+12 处画同色下划线，
/// 点击打开链接。Fluent 用 accent[6]（悬停 (137,137,137)、按下 (181,181,181)），
/// 其余主题固定 rgb(62,144,255)。
fn greet_link(
    state: &mut EditorState,
    ui: &mut Ui,
    p: &egui::Painter,
    th: &Theme,
    center_x: f32,
    y: f32,
    s: &str,
    url: &str,
) {
    let w = p
        .layout_no_wrap(
            s.to_string(),
            FontId::proportional(PREFS_FONT),
            Color32::WHITE,
        )
        .size()
        .x;
    // 原版 `mouse_rectangle(xx - width/2, yy, width, 12)`：命中区以 x 为中心、高 12。
    let rect = Rect::from_min_size(egui::pos2(center_x - w / 2.0, y), Vec2::new(w, 12.0));
    let resp = ui.interact(
        rect,
        Id::new(("greet-link", center_x as i32, y as i32)),
        Sense::click(),
    );
    let color = if th.kind == ThemeKind::Fluent {
        if resp.is_pointer_button_down_on() {
            ggray(181)
        } else if resp.hovered() {
            ggray(137)
        } else {
            th.accent(6)
        }
    } else {
        Color32::from_rgb(62, 144, 255)
    };
    let g = p.layout_no_wrap(s.to_string(), FontId::proportional(PREFS_FONT), color);
    let w = g.size().x;
    let px = center_x - w / 2.0;
    p.galley(egui::pos2(px, y), g, Color32::BLACK);
    // 原版 `draw_line(xx - floor(width/2) - 1, yy + 12, xx + ceil(width/2) - 1, yy + 12)`。
    p.line_segment(
        [
            egui::pos2(center_x - w / 2.0 - 1.0, y + 12.0),
            egui::pos2(center_x + (w / 2.0).ceil() - 1.0, y + 12.0),
        ],
        Stroke::new(1.0_f32, color),
    );
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if resp.clicked() {
        state.emit(Command::OpenUrl(url.to_string()));
    }
}

/// 原版 `seconds_to_str`：最近歌曲的相对时间。阈值两语言完全一样，只是文案不同；
/// 英文的复数按整除之后的值判（原版先 `c = floor(c / n)` 再拼 `condstr(c != 1, "s")`）。
fn seconds_to_str(secs: i64) -> String {
    let c = secs;
    if crate::lang() != 1 {
        let unit = |c: i64, word: &str| {
            if c == 1 {
                format!("{c} {word} ago")
            } else {
                format!("{c} {word}s ago")
            }
        };
        if c < 60 {
            unit(c, "second")
        } else if c < 60 * 60 {
            unit(c / 60, "minute")
        } else if c < 60 * 60 * 24 {
            unit(c / (60 * 60), "hour")
        } else if c < 60 * 60 * 24 * 7 {
            unit(c / (60 * 60 * 24), "day")
        } else if c < 60 * 60 * 24 * 31 {
            unit(c / (60 * 60 * 24 * 7), "week")
        } else if c < 60 * 60 * 24 * 31 * 12 {
            unit(c / (60 * 60 * 24 * 31), "month")
        } else {
            unit(c / (60 * 60 * 24 * 31 * 12), "year")
        }
    } else if c < 60 {
        format!("{c} 秒前")
    } else if c < 60 * 60 {
        format!("{} 分钟前", c / 60)
    } else if c < 60 * 60 * 24 {
        format!("{} 小时前", c / (60 * 60))
    } else if c < 60 * 60 * 24 * 7 {
        format!("{} 天前", c / (60 * 60 * 24))
    } else if c < 60 * 60 * 24 * 31 {
        format!("{} 星期前", c / (60 * 60 * 24 * 7))
    } else if c < 60 * 60 * 24 * 31 * 12 {
        format!("{} 月前", c / (60 * 60 * 24 * 31))
    } else {
        format!("{} 年前", c / (60 * 60 * 24 * 31 * 12))
    }
}

/// 原版 `draw_window_greeting.gml`。窗口 700×430 无标题栏，内容原点 o = 原版 (x1, y1)。
/// `is_prerelease` 恒为假 → dev_label_offset = 0、开发版本行不画；播放器模式按钮没有
/// 对应功能，不画（原版 :323-329）。
fn greeting(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let margin = ui.spacing().window_margin;
    let (ml, mr, mt, mb) = (
        margin.left as f32,
        margin.right as f32,
        margin.top as f32,
        margin.bottom as f32,
    );
    let body = ui.allocate_exact_size(
        egui::vec2(700.0 - ml - mr, 430.0 - mt - mb),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, mt);
    let p = ui.painter().clone();
    let bank = state.icons.bank();
    let x = |dx: f32| o.x + dx;
    let y = |dy: f32| o.y + dy;

    // :39-41 logo：帧 = window_icon + (window_icon && fdark)，也就是「旧图标」开关
    // 开着时欢迎窗左上角那张也跟着换。
    if let Some(bk) = &bank {
        bk.window_logo(
            &p,
            egui::pos2(x(64.0), y(50.0)),
            state.prefs.window_icon,
            th.fdark,
        );
    }
    // :43 标题（font_info_med_bold）。
    greet_text_center(
        &p,
        x(132.0),
        y(213.0),
        "Note Block Studio",
        PREFS_FONT_MED,
        th.text,
        true,
    );
    // :68-100 更新状态行（font_main_bold）。update=0 那支原版没设颜色，用主题色。
    let status = state.update_check.lock().unwrap().status;
    let (line, color) = if state.prefs.check_update {
        if state.update_success {
            (
                tr("Successfully updated!", "更新成功！"),
                Color32::from_rgb(0, 255, 0),
            )
        } else {
            match status {
                -1 => (
                    tr("Could not check for updates", "检查更新失败"),
                    Color32::from_rgb(255, 0, 0),
                ),
                0 => (tr("Checking for updates...", "正在检查更新……"), th.text),
                1 => (
                    tr("There is an update available!", "有新版本！"),
                    Color32::from_rgb(255, 128, 0),
                ),
                _ => (
                    tr("You are using the latest version!", "已为最新版本！"),
                    if th.kind == ThemeKind::Dark {
                        Color32::from_rgb(0, 255, 0)
                    } else {
                        Color32::from_rgb(0, 128, 0)
                    },
                ),
            }
        }
    } else {
        (
            tr("Update checking disabled by user.", "检查更新未开启"),
            Color32::from_rgb(255, 0, 0),
        )
    };
    greet_text_center(&p, x(132.0), y(248.0), line, PREFS_FONT, color, true);
    // :107-108 版本行（font_main_bold）。
    let ver_line = format!(
        "{} {} - {} {}",
        tr("Version", "版本"),
        crate::VERSION,
        tr("Released", "发布于"),
        crate::VERSION_DATE
    );
    greet_text_center(&p, x(132.0), y(233.0), &ver_line, PREFS_FONT, th.text, true);
    // :114-115 宣传语。
    greet_text_center(
        &p,
        x(132.0),
        y(280.0),
        tr(
            "An open-source Minecraft music maker",
            "一个开源的 Minecraft 音乐制作软件",
        ),
        PREFS_FONT,
        th.text,
        false,
    );
    // :117 官网链接。
    greet_link(
        state,
        ui,
        &p,
        &th,
        x(132.0),
        y(296.0),
        "noteblock.studio",
        crate::LINK_WEBSITE,
    );
    // :122-136 社区链接两行。竖线是 draw_text_dynamic（左对齐）。第二行两种语言
    // 布的点不一样：英文 GitHub 108 / 竖线 131 / Donate 156，中文 GitHub 88 /
    // 竖线 109 / QQ 群 133 / 竖线 153 / 捐赠 171，全部照原式
    // `108-(language=1)*20`、`131-(language=1)*22`、`156+15*(language=1)` 拆开写。
    greet_text_center(
        &p,
        x(132.0),
        y(340.0),
        tr("Follow OpenNBS:", "关注 OpenNBS："),
        PREFS_FONT,
        th.text,
        false,
    );
    let off = if th.kind == ThemeKind::Fluent {
        0.0
    } else {
        2.0
    };
    greet_link(
        state,
        ui,
        &p,
        &th,
        x(80.0),
        y(356.0),
        "Discord",
        crate::LINK_DISCORD,
    );
    greet_text_left(
        &p,
        egui::pos2(x(104.0 - off), y(356.0)),
        "|",
        PREFS_FONT,
        th.text,
        false,
    );
    greet_link(
        state,
        ui,
        &p,
        &th,
        x(128.0),
        y(356.0),
        "Twitter",
        crate::LINK_TWITTER,
    );
    greet_text_left(
        &p,
        egui::pos2(x(150.0 - off), y(356.0)),
        "|",
        PREFS_FONT,
        th.text,
        false,
    );
    greet_link(
        state,
        ui,
        &p,
        &th,
        x(180.0),
        y(356.0),
        "YouTube",
        crate::LINK_YOUTUBE,
    );
    if crate::lang() == 1 {
        greet_link(
            state,
            ui,
            &p,
            &th,
            x(88.0),
            y(372.0),
            "GitHub",
            crate::LINK_GITHUB,
        );
        greet_text_left(
            &p,
            egui::pos2(x(109.0 - off), y(372.0)),
            "|",
            PREFS_FONT,
            th.text,
            false,
        );
        greet_link(
            state,
            ui,
            &p,
            &th,
            x(133.0),
            y(372.0),
            "QQ 群",
            crate::LINK_QQ_GROUP,
        );
        greet_text_left(
            &p,
            egui::pos2(x(153.0 - off), y(372.0)),
            "|",
            PREFS_FONT,
            th.text,
            false,
        );
        greet_link(
            state,
            ui,
            &p,
            &th,
            x(171.0),
            y(372.0),
            "捐赠",
            crate::LINK_DONATE,
        );
    } else {
        greet_link(
            state,
            ui,
            &p,
            &th,
            x(108.0),
            y(372.0),
            "GitHub",
            crate::LINK_GITHUB,
        );
        greet_text_left(
            &p,
            egui::pos2(x(131.0 - off), y(372.0)),
            "|",
            PREFS_FONT,
            th.text,
            false,
        );
        greet_link(
            state,
            ui,
            &p,
            &th,
            x(156.0),
            y(372.0),
            "Donate",
            crate::LINK_DONATE,
        );
    }
    // :139-146 分隔线：先一条 1px 白线（Fluent 深色换黑），再一条 25% 主题色线。
    let line1 = if th.fdark && th.kind == ThemeKind::Fluent {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    p.line_segment(
        [
            egui::pos2(x(270.0), y(24.0)),
            egui::pos2(x(270.0), y(396.0)),
        ],
        Stroke::new(1.0_f32, line1),
    );
    let line2 = Color32::from_rgba_unmultiplied(th.text.r(), th.text.g(), th.text.b(), 64);
    p.line_segment(
        [
            egui::pos2(x(269.0), y(24.0)),
            egui::pos2(x(269.0), y(396.0)),
        ],
        Stroke::new(1.0_f32, line2),
    );
    // :148-149 右栏标题。
    greet_text_left(
        &p,
        egui::pos2(x(290.0), y(20.0)),
        tr("What do you want to do?", "要做什么？"),
        PREFS_FONT,
        th.text,
        false,
    );

    // :152-175 创建歌曲。
    let mut c = 48.0;
    {
        let at = egui::pos2(x(300.0), y(c));
        let (resp, m) = greet_hit(
            ui,
            Rect::from_min_size(at, Vec2::new(224.0, 32.0)),
            Id::new("greet-create"),
        );
        let pressed = m == 2;
        if let Some(bk) = &bank {
            bk.frame2_button(&p, at, &th, m);
            bk.big_icon(&p, at, &th, 0, pressed);
        }
        let push = if pressed { 1.0 } else { 0.0 };
        greet_text_left(
            &p,
            at + Vec2::new(48.0 + push, 9.0 + push),
            tr("Create a new song", "创建歌曲"),
            PREFS_FONT,
            th.text,
            false,
        );
        if resp.clicked() {
            state.emit(Command::CloseWindow);
        }
    }
    c += 44.0;
    // :178-203 打开歌曲（load_song("", 0, 1) = 系统选框）。
    {
        let at = egui::pos2(x(300.0), y(c));
        let (resp, m) = greet_hit(
            ui,
            Rect::from_min_size(at, Vec2::new(224.0, 32.0)),
            Id::new("greet-open"),
        );
        let pressed = m == 2;
        if let Some(bk) = &bank {
            bk.frame2_button(&p, at, &th, m);
            bk.big_icon(&p, at, &th, 1, pressed);
        }
        let push = if pressed { 1.0 } else { 0.0 };
        greet_text_left(
            &p,
            at + Vec2::new(48.0 + push, 9.0 + push),
            tr("Load a song", "打开歌曲"),
            PREFS_FONT,
            th.text,
            false,
        );
        if resp.clicked() {
            state.emit(Command::OpenDialog(DialogAction::OpenSong));
        }
    }
    // :204-249 最近歌曲。b 换到 x1+320，行高 16，表头只随第一行画一次。
    let b = 320.0;
    for (i, r) in state.recent_files.clone().iter().enumerate() {
        if i == 0 {
            c += 36.0;
            greet_text_left(
                &p,
                egui::pos2(x(b - 20.0), y(c)),
                tr("Recent songs:", "最近歌曲："),
                PREFS_FONT,
                th.text,
                false,
            );
            c += 16.0;
        }
        let at = egui::pos2(x(b), y(c));
        let (resp, m) = greet_hit(
            ui,
            Rect::from_min_size(at, Vec2::new(320.0, 16.0)),
            Id::new(("greet-recent", i)),
        );
        let pressed = m == 2;
        if let Some(bk) = &bank {
            bk.frame5_row(&p, at, &th, m);
        }
        let push = if pressed { 1.0 } else { 0.0 };
        let name = r
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = greet_truncate(&p, &name, 220.0, PREFS_FONT);
        greet_text_left(
            &p,
            at + Vec2::new(2.0 + push, 1.0 + push),
            &name,
            PREFS_FONT,
            th.text,
            false,
        );
        let ago = seconds_to_str(nbs_io::prefs::unix_now() - r.time);
        let tw = p
            .layout_no_wrap(ago.clone(), FontId::proportional(PREFS_FONT), th.text)
            .size()
            .x;
        // 原版 `draw_set_halign(fa_right)`：x 是右缘。
        greet_text_left(
            &p,
            egui::pos2(at.x + 316.0 - tw + push, at.y + 1.0 + push),
            &ago,
            PREFS_FONT,
            th.text,
            false,
        );
        if resp.clicked() {
            if r.path.is_file() {
                state.emit(Command::OpenPath(r.path.clone()));
            } else {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    &format!(
                        "{}\n{}",
                        tr("Could not find file:", "找不到文件："),
                        r.path.display()
                    ),
                );
                state.remove_recent(&r.path);
                state.emit(Command::SavePreferences);
            }
        }
        c += 16.0;
    }
    c += 10.0;
    if state.recent_files.is_empty() {
        c += 34.0;
    }
    // :250-291 Note Block World：底图横向拉伸 1.5 倍，NEW! 徽标和灰字都按原版坐标。
    {
        let at = egui::pos2(x(300.0), y(c));
        let (resp, m) = greet_hit(
            ui,
            Rect::from_min_size(at, Vec2::new(336.0, 32.0)),
            Id::new("greet-world"),
        );
        let pressed = m == 2;
        if let Some(bk) = &bank {
            bk.frame2_button_wide(&p, at, &th, m);
            bk.big_icon(&p, at, &th, 6, pressed);
        }
        let push = if pressed { 1.0 } else { 0.0 };
        greet_text_left(
            &p,
            at + Vec2::new(48.0 + push, 9.0 + push - 7.0),
            tr("Browse songs made by the community", "浏览社区制作的歌曲"),
            PREFS_FONT,
            th.text,
            false,
        );
        // :271-281 NEW! 徽标：accent[4] 圆角胶囊（(566, c+1)-(604, c+16)，r=15）+ 白字。
        p.rect_filled(
            Rect::from_min_max(
                egui::pos2(x(566.0), y(c + 1.0)),
                egui::pos2(x(604.0), y(c + 16.0)),
            ),
            15.0,
            th.accent(4),
        );
        greet_text_left(
            &p,
            egui::pos2(x(570.0), y(c + 2.0)),
            "NEW!",
            PREFS_FONT,
            Color32::WHITE,
            true,
        );
        // :283-285 灰字。
        greet_text_left(
            &p,
            at + Vec2::new(48.0 + push, 9.0 + push + 7.0),
            tr("Go to noteblock.world", "前往 noteblock.world"),
            PREFS_FONT,
            ggray(128),
            false,
        );
        if resp.clicked() {
            state.emit(Command::OpenUrl("https://noteblock.world/".to_string()));
        }
    }
    c += 44.0;
    // :294-317 从 MIDI 文件生成（open_midi("", 1) = 系统选框）。
    {
        let at = egui::pos2(x(300.0), y(c));
        let (resp, m) = greet_hit(
            ui,
            Rect::from_min_size(at, Vec2::new(224.0, 32.0)),
            Id::new("greet-midi"),
        );
        let pressed = m == 2;
        if let Some(bk) = &bank {
            bk.frame2_button(&p, at, &th, m);
            bk.big_icon(&p, at, &th, 2, pressed);
        }
        let push = if pressed { 1.0 } else { 0.0 };
        greet_text_left(
            &p,
            at + Vec2::new(48.0 + push, 9.0 + push),
            tr("Generate song out of MIDI file", "从 MIDI 文件生成"),
            PREFS_FONT,
            th.text,
            false,
        );
        if resp.clicked() {
            state.emit(Command::OpenDialog(DialogAction::ImportMidi));
        }
    }
    // Escape 关窗（原版 windowclose 默认值）。
    if ui.input(|i| i.key_released(egui::Key::Escape)) {
        state.emit(Command::CloseWindow);
    }
}

/// 原版 draw_window_greeting.gml:10-36 的捐赠横幅：画在欢迎窗右侧窗外
/// （(x1+700, y1+110) = 屏幕中心 + (350, −100)）。文字/X 都在贴图里，
/// 这张窗不画任何边框和底色，透明处透出的就是原版的「直接画在屏幕上」。
fn donate_banner_window(state: &mut EditorState, ctx: &Context) {
    let mut close = false;
    let mut open_page = false;
    egui::Window::new("donate")
        .id(egui::Id::new("nbs_donate_banner"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(egui::Frame::NONE)
        .fixed_size(egui::vec2(150.0, 120.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(350.0, -100.0))
        .show(ctx, |ui| {
            let o = ui.cursor().min;
            ui.allocate_exact_size(egui::vec2(150.0, 120.0), egui::Sense::hover());
            let p = ui.painter().clone();
            let x_rect = Rect::from_min_size(o + Vec2::new(120.0, 3.0), Vec2::new(16.0, 16.0));
            let banner = Rect::from_min_size(o, Vec2::new(150.0, 120.0));
            let (rx, _) = greet_hit(ui, x_rect, Id::new("donate_x"));
            let (rb, _) = greet_hit(ui, banner, Id::new("donate_badge"));
            let rx = rx.on_hover_cursor(CursorIcon::PointingHand);
            let rb = rb.on_hover_cursor(CursorIcon::PointingHand);
            if let Some(bk) = state.icons.bank() {
                bk.donate_banner(&p, o, rx.hovered(), crate::lang() == 1);
            }
            // 原版 :29：横幅上抬手、悬停不在 X 区、按下起点也不在 X 区才开捐赠页。
            let press_in_x = ui
                .input(|i| i.pointer.press_origin())
                .map(|po| x_rect.contains(po))
                .unwrap_or(false);
            close = rx.clicked();
            open_page = rb.clicked() && !rx.hovered() && !press_in_x;
        });
    if close {
        state.modals = ModalQueue::message(
            "Note Block Studio",
            tr(
                "Developing Note Block Studio takes a lot of unpaid volunteering time. If you can, please consider supporting us in the future! =)\n\n(You can find that option at any time in Help > Donate.)",
                "开发Note Block Studio完全基于我们用爱发电。如果情况允许，请考虑在以后小小的支持我们一下！(～￣▽￣)～\n\n（您可以随时在帮助 > 捐赠中找到该选项。）",
            ),
        );
        state.prefs.donate_banner_time = nbs_io::prefs::inc_month(nbs_io::prefs::unix_now());
        state.donate_banner = false;
        state.emit(Command::SavePreferences);
    }
    if open_page {
        state.emit(Command::OpenUrl(crate::LINK_DONATE.to_string()));
    }
}

fn song_info(state: &mut EditorState, ui: &mut Ui) {
    let mut changed = false;
    egui::Grid::new("song_info_grid")
        .num_columns(2)
        .show(ui, |ui| {
            let m: &mut SongMeta = &mut state.doc.song.meta;
            ui.label(tr("Name", "名称"));
            if ui.text_edit_singleline(&mut m.name).changed() {
                changed = true;
            }
            ui.end_row();
            ui.label(tr("Author", "作者"));
            if ui.text_edit_singleline(&mut m.author).changed() {
                changed = true;
            }
            ui.end_row();
            ui.label(tr("Original author", "原作者"));
            if ui.text_edit_singleline(&mut m.original_author).changed() {
                changed = true;
            }
            ui.end_row();
            ui.label(tr("Description", "描述"));
            if ui
                .add(egui::TextEdit::multiline(&mut m.description).desired_rows(3))
                .changed()
            {
                changed = true;
            }
            ui.end_row();
        });
    if changed {
        state.doc.song.dirty = true;
    }
    ui.separator();
    ui.label(
        RichText::new(tr(
            "These fields are saved with the .nbs file.",
            "这些字段会随 .nbs 一起保存",
        ))
        .small()
        .weak(),
    );
    footer(state, ui);
}

fn properties(state: &mut EditorState, ui: &mut Ui) {
    let mut tempo = state.doc.song.meta.tempo as f64;
    if ui
        .add(egui::Slider::new(&mut tempo, 0.25..=60.0).text(tr("Tempo (tick/s)", "速度 (tick/s)")))
        .changed()
    {
        state.emit(Command::SetTempo(tempo as f32));
    }
    ui.label(
        RichText::new(format!("= {:.0} BPM", tempo * 15.0))
            .small()
            .weak(),
    );
    let mut ts = state.doc.song.meta.time_signature as f32;
    if ui
        .add(egui::Slider::new(&mut ts, 2.0..=8.0).text(tr("Time signature", "拍号")))
        .changed()
    {
        state.emit(Command::SetTimeSignature(ts as u8));
    }
    let mut looping = state.doc.song.meta.loop_enabled;
    if ui
        .checkbox(&mut looping, tr("Enable looping", "启用循环"))
        .changed()
    {
        state.emit(Command::ToggleLoop);
    }
    let loop_on = state.doc.song.meta.loop_enabled;
    ui.add_enabled_ui(loop_on, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Loop start tick", "循环起点 tick"));
            let enda = state.doc.song.enda as f64;
            let mut start = state.doc.song.meta.loop_start as f64;
            if ui
                .add(egui::DragValue::new(&mut start).range(0.0..=enda))
                .changed()
            {
                state
                    .doc
                    .edit_meta(tr("Set loop start", "设置循环起点"), |m| {
                        m.loop_start = start as u16
                    });
                state.sync_loop_params();
            }
        });
        let len = state.doc.song.song_length();
        ui.horizontal(|ui| {
            ui.label(tr("Loop end tick", "循环终点 tick"));
            // 原版没有 loop_end 字段：循环终点就是歌曲长度，所以只能往上调，
            // 上调的效果是延长结尾。
            let mut end = len as f64;
            if ui
                .add(egui::DragValue::new(&mut end).range(len as f64..=(len + 16) as f64))
                .changed()
            {
                state.doc.extend_loop_end(end as u32);
                state.sync_loop_params();
            }
        });
        ui.label(
            RichText::new(tr(
                "A silent note will be added to extend the length of the song.",
                "超出歌曲长度时，结尾会自动加一个 0 音量音符来延长终点",
            ))
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label(tr("Times to loop", "循环次数"));
            let mut max = state.doc.song.meta.loop_max as i32;
            if ui
                .add(egui::DragValue::new(&mut max).range(0..=10))
                .changed()
            {
                state
                    .doc
                    .edit_meta(tr("Set loop count", "设置循环次数"), |m| {
                        m.loop_max = max as u8
                    });
                state.sync_loop_params();
            }
            if state.doc.song.meta.loop_max == 0 {
                ui.label(RichText::new(tr("(infinite)", "（无限）")).small().weak());
            }
        });
    });
    // 原版属性窗到「循环次数」就结束了；保存版本另有文件菜单的「保存选项」窗口，
    // 不再在这里塞一个下拉框和一段格式说明。
    footer(state, ui);
}

fn statistics(state: &mut EditorState, ui: &mut Ui) {
    let song = &state.doc.song;
    // 自定义乐器的编号从 FIRST_CUSTOM_INDEX 起，所以直方图要覆盖到它们，
    // 否则大工程里最常见的恰恰是被漏掉的（原版统计就包含全部乐器行）。
    let slots = nbs_core::model::FIRST_CUSTOM_INDEX as usize + song.custom_instruments.len().max(1);
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
        ui.label(tr("Total notes", "音符总数"));
        ui.label(song.total_blocks.to_string());
        ui.end_row();
        ui.label(tr("Song length", "歌曲长度"));
        ui.label(format!("{} ticks", song.song_length()));
        ui.end_row();
        ui.label(tr("Duration", "时长"));
        ui.label(if crate::lang() == 1 {
            format!("{seconds:.1} 秒")
        } else {
            format!("{seconds:.1} s")
        });
        ui.end_row();
        ui.label(tr("Layers", "图层数"));
        ui.label(song.layers.len().to_string());
        ui.end_row();
        ui.label(tr("Locked layers", "已锁定图层"));
        ui.label(
            song.locked_layers()
                .iter()
                .filter(|x| **x)
                .count()
                .to_string(),
        );
        ui.end_row();
        ui.label(tr("Pitch range", "音高范围"));
        ui.label(format!(
            "{} ~ {}",
            key_name(min_key, true, false),
            key_name(max_key, true, false)
        ));
        ui.end_row();
        ui.label(tr("Custom instruments", "自定义音色"));
        ui.label(song.custom_instruments.len().to_string());
        ui.end_row();
        ui.label(tr("History entries", "历史条目"));
        ui.label(state.doc.history.len().to_string());
        ui.end_row();
    });
    ui.separator();
    ui.label(RichText::new(tr("Notes per instrument", "各乐器音符数")).strong());
    let total = song.total_blocks.max(1);
    egui::ScrollArea::vertical()
        .max_height(160.0)
        .show(ui, |ui| {
            for (i, c) in per_ins.iter().enumerate() {
                if *c > 0 {
                    let pct = *c as f32 * 100.0 / total as f32;
                    ui.label(format!(
                        "{:>6}  {:>5.1}%  {}",
                        c,
                        pct,
                        instrument_name(i as u8)
                    ));
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
/// 只画真实接线的行。原版有、我们还没有对应行为的选项（Discord 富在位 / 界面音效 /
/// 透明效果 / 取色器 / 高分辨率纹理 / 语言 / 程序图标 /
/// 任务栏进度 / 重置键位 / 在区间起止播放）
/// 整行不出现，免得留下点了没反应的假控件。
fn preferences(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    // 窗口外沿宽 500（原版 `draw_window(x1, y1, x1 + 500, y1 + 510)`），
    // 内容区扣掉 egui 的窗口内边距；把原点左移一个内边距，行内 x 就能直接用原版的 K。
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    // 内容原点 o 相当于原版的 (x1, y1 + 30)（Fluent）/ (x1, y1 + 26)（其余），
    // 所以原版窗口底边 y1 + 510 落到 o 系就是 480 / 484。
    let body_h = if fluent { 480.0 } else { 484.0 };
    let body = ui.allocate_exact_size(egui::vec2(500.0 - ml - mr, body_h), egui::Sense::hover());
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
    let mut rate_at = Pos2::ZERO;
    let mut lang_at = Pos2::ZERO;
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
        let tab = pg.tabs(if fluent { 40.0 } else { 8.0 }, 0.0, &prefs_tabs(), tab);
        state.prefs_tab = tab;
        match tab {
            0 => prefs_general(state, &mut pg),
            1 => prefs_interface(state, &mut pg, &mut menu_at, &mut rate_at, &mut lang_at),
            2 => prefs_appearance(state, &mut pg),
            3 => prefs_usability(state, &mut pg),
            _ => prefs_playback(state, &mut pg),
        }
        // 原版 `draw_button2(x1 + 420, y1 + 478, 72, "确定")`：确定 / Escape / Ctrl+P 都关窗。
        // 这一行的 y 没有 `(theme = 3) * 22`，所以 Fluent 要把那 22 减回去，
        // 否则按钮会掉到窗口底边外面（原版底边在 y1 + 510，按钮占到 y1 + 499）。
        let ok_n = 478.0 - if fluent { 22.0 } else { 0.0 };
        if pg.button(420.0, ok_n, 72.0, tr("OK", "确定"), false) {
            state.emit(Command::CloseWindow);
        }
        // Escape 取「松开」沿（原版 `keyboard_check_released(vk_escape)`）；Ctrl+P 的 Ctrl 位
        // 要从按下事件上读，见 `shortcuts::ctrl_chord`。
        // Ctrl+P 还多一道原版才有的闸 `prevwindow == window`（596 行）：用 Ctrl+P 开窗的那一帧
        // 按键事件依然为真，不挡一下就是同帧「开 → 关」，窗口一闪即消失。
        let (ctrl_p, esc) = pg.ui.input(|i| {
            (
                crate::shortcuts::ctrl_chord(i) && i.key_pressed(egui::Key::P),
                i.key_released(egui::Key::Escape),
            )
        });
        if esc || (ctrl_p && state.window_prev == state.window) {
            state.emit(Command::CloseWindow);
        }
    }
    if state.prefs_theme_menu {
        theme_menu(state, ui, menu_at);
    }
    if state.prefs_rate_menu {
        rate_menu(state, ui, rate_at);
    }
    if state.prefs_language_menu {
        language_menu(state, ui, lang_at);
    }
}

/// 原版页 0「通用」。源码这一页还有「Discord」组，没有对应行为，整组不画。
fn prefs_general(state: &mut EditorState, pg: &mut Page) {
    // 原版 draw_window_preferences.gml:101-138 的「启动」组。macOS 渠道闸
    // （:109）在 Windows 恒为真，不搬。
    pg.header(22.0, 74.0, 243.0, 85.0, tr("Startup", "启动"));
    if pg.check(
        40.0,
        90.0,
        state.prefs.show_welcome,
        tr("Show greeting window", "显示欢迎界面"),
        tr(
            "Whether to show the greeting window\nwhen the program is opened.",
            "打开软件时是否显示欢迎界面。",
        ),
        true,
        false,
    ) {
        state.prefs.show_welcome = !state.prefs.show_welcome;
        state.emit(Command::SavePreferences);
    }
    if pg.check(
        40.0,
        110.0,
        state.prefs.check_update,
        tr("Check for updates", "检查更新"),
        tr(
            "Whether to check for any updates\nwhen the program is opened.",
            "打开软件时是否检查更新。",
        ),
        true,
        false,
    ) {
        state.prefs.check_update = !state.prefs.check_update;
        state.emit(Command::SavePreferences);
    }
    // 原版 :112-137：翻转后按运行版分支弹警告，答否/取消就翻回去。重写版不是
    // 开发版（is_prerelease 恒为假），只有开启方向的警告分支。
    if pg.check(
        60.0,
        130.0,
        state.prefs.check_prerelease,
        tr("Check for development versions", "检查开发版本"),
        tr(
            "Whether to check for development versions in addition to stable releases.\nDevelopment versions contain the latest improvements, but may be potentially unstable.",
            "是否在稳定版之外额外检测开发版。\n开发版本包含最新的内容，但可能不稳定。",
        ),
        false,
        !state.prefs.check_update,
    ) {
        let enable = !state.prefs.check_prerelease;
        if enable {
            state.modals = ModalQueue::question(
                tr("Warning", "警告"),
                &tr(
                    "Development versions are experimental and may be unstable. To avoid data corruption, it's highly recommended that you install Note Block Studio to a separate directory before enabling this option.\n\nProceed?",
                    "开发版本仍在测试中，可能会不稳定。为了防止数据损坏，强烈建议您在开启该选项前将 Note Block Studio 安装到一个单独的目录。\n\n继续吗？",
                ).to_string(),
                Command::SetCheckPrerelease(true),
            );
        } else {
            state.prefs.check_prerelease = false;
            state.emit(Command::SavePreferences);
        }
    }

    pg.header(278.0, 74.0, 200.0, 85.0, tr("Auto-saving", "自动保存"));
    if pg.check(
        296.0,
        90.0,
        state.prefs.autosave_enabled,
        tr("Enable auto-saving", "启用自动保存"),
        tr(
            "Whether the song should automatically\nbe saved every now and then.",
            "歌曲是否每过一段时间自动保存一次。",
        ),
        true,
        false,
    ) {
        state.prefs.autosave_enabled = !state.prefs.autosave_enabled;
        state.emit(Command::SavePreferences);
    }
    // 原版 `if (autosave = 0)`：整行涂 c_gray(128)，数字退化成静态文本。
    let on = state.prefs.autosave_enabled;
    let ink = if on { pg.ink() } else { ggray(128) };
    // 和「缩放」同一套：原版把 `"间隔:            分钟"` 画成一整串，数字叠在空格上，
    // 我们的空格更窄，照抄个数会让数字压住「分钟」，所以单位按最宽的数字（60）定死。
    pg.text(
        pg.row(326.0, 110.0),
        tr("Interval:", "间隔:"),
        PREFS_FONT,
        ink,
    );
    let tip = tr(
        "The amount of minutes between each auto-save.",
        "自动保存中间的间隔。",
    );
    pg.tip_area(326.0, 110.0, 180.0, 16.0, tip);
    let mins = state.prefs.auto_save_minutes as i32;
    let drag = pg.drag(
        375.0,
        110.0,
        mins,
        1.0,
        1,
        60,
        !on,
        tip,
        &mut state.prefs_drag,
    );
    let unit_x = 375.0 + pg.measure("60", PREFS_FONT) + 4.0;
    // 原版 `condstr(autosavemins > 1, "s")`。
    let unit = if mins > 1 {
        tr("minutes", "分钟")
    } else {
        tr("minute", "分钟")
    };
    pg.text(pg.row(unit_x, 110.0), unit, PREFS_FONT, ink);
    if let Some(mins) = drag {
        state.prefs.auto_save_minutes = mins as u8;
        state.emit(Command::SavePreferences);
    }

    // 原版 `draw_areaheader(x1 + 22, y1 + 184, 456, 145, "歌曲")`。
    pg.header(22.0, 184.0, 456.0, 145.0, tr("Songs", "歌曲"));
    let warn = state.prefs.show_old_warning;
    if pg.check(
        40.0,
        200.0,
        warn,
        tr("Show warning when opening older songs", "打开旧版格式时提示"),
        tr(
            "Whether to show a warning when opening a song\nsaved in an older version of Note Block Studio.",
            "打开在旧版 Note Block Studio 中保存的文件时是否显示警告。",
        ),
        true,
        false,
    ) {
        state.prefs.show_old_warning = !warn;
        state.emit(Command::SavePreferences);
    }
    folder_row(state, pg, PrefsFolder::Song, 230.0, 246.0);
    folder_row(state, pg, PrefsFolder::Pattern, 280.0, 296.0);
}

/// 原版「歌曲」组里的那两行目录：
/// `draw_text_dynamic(x1 + 40, y1 + N, "歌曲路径: " + string_truncate(songfolder, 340))`
/// 加 `draw_button2` 打开 76 / 修改 76 / 还原默认 96，起点 40、间隔 84。
/// 「修改」用我们自己的目录选择框（原版是系统对话框），确定后取对话框当前所在目录。
fn folder_row(state: &mut EditorState, pg: &mut Page, which: PrefsFolder, text_n: f32, btn_n: f32) {
    let dir = match which {
        PrefsFolder::Song => state.prefs.song_folder_path(),
        PrefsFolder::Pattern => state.prefs.pattern_folder_path(),
    };
    let full = dir.to_string_lossy().into_owned();
    let line = format!("{}{}", which.label(), pg.truncate(&full, 340.0));
    pg.text(pg.row(40.0, text_n), &line, PREFS_FONT, pg.ink());
    // 原版 `popup_set_window(x1 + 40, y1 + N, 430, 18, songfolder)`：悬停看完整路径。
    pg.tip_area(40.0, text_n, 430.0, 18.0, &full);
    if pg.button(40.0, btn_n, 76.0, tr("Open", "打开"), false) {
        state.emit(Command::OpenPrefsFolder(which));
    }
    if pg.button(124.0, btn_n, 76.0, tr("Change", "修改"), false) {
        state.emit(Command::ChangePrefsFolder(which));
    }
    if pg.button(208.0, btn_n, 96.0, tr("Use default", "还原默认"), false) {
        state.emit(Command::ResetPrefsFolder(which));
    }
}

/// 原版页 1「界面」：主题下拉 + 暗色 / 全黑两个开关 + Fluent 那一支主题色块（点开
/// 取色器）+ 帧率限制 + 窗口缩放 + 旧图标 + 语言下拉。同页的「为高分辨率优化」
/// （要 `*_hires` 那套纹理）没有对应实现，整行不画。
fn prefs_interface(
    state: &mut EditorState,
    pg: &mut Page,
    menu_at: &mut Pos2,
    rate_at: &mut Pos2,
    lang_at: &mut Pos2,
) {
    pg.header(22.0, 74.0, 218.0, 140.0, tr("Theme", "主题"));
    pg.frame_box(40.0, 95.0, 140.0, 115.0);
    let name = state.theme.kind.name().to_string();
    pg.text(pg.row(43.0, 98.0), &name, PREFS_FONT, pg.ink());
    // 原版 `show_menu_ext("theme", x1 + 40, y1 + 74 + 16 + 21 + (theme = 3) * 22 + 5, …)`。
    *menu_at = pg.row(40.0, 116.0);
    // 只有右边那个箭头能开下拉；菜单开着时原版由 obj_menu 吞掉点击，这里交给提示区收起。
    if pg.arrow(&state.icons, 123.0, 96.0) && !state.prefs_theme_menu {
        state.prefs_theme_menu = true;
    }
    if pg.check(
        40.0,
        135.0,
        state.theme.fdark,
        tr("Dark mode", "暗色模式"),
        tr(
            "Whether to use darker colors on the interface.\n(Only applies to the Fluent theme.)",
            "是否在界面上使用暗色调。\n（仅限 Fluent 主题）",
        ),
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
        tr("Blackout mode", "全黑模式"),
        tr(
            "Makes the workspace background solid black, so you can\nremove in your video editor when recording the screen.",
            "使背景变为纯黑色，可以用于剪辑时扣掉。",
        ),
        true,
        false,
    ) {
        state.emit(Command::ToggleFlag(Flag::Blackout));
    }

    // 原版 :299-323「主题色取色器」：整块的条件是 `theme == 3`，只有 Fluent 画得出。
    // 位置 `xx = x1 + 160`、`yy = y1 + 98 + (theme = 3) * 22`，压在主题下拉右边。外层
    // 17x17 先用主题底色填一遍再描一圈（圆角半径 4），内层 13x13 才是那支色，档位是
    // `accent[3 + (accentclick = 1) * 3 - (accentclick = 2) * 3]`：常态 accent[3]、
    // 悬停 accent[6]、按下 accent[0]。抬手开 `w_setaccent` 并置 `resetcolor = true`；
    // 紧跟的那句 `play_sound(soundinvoke)` 属于「界面音效」，那个开关端口没实现，不响。
    if state.theme.kind == ThemeKind::Fluent {
        let at = pg.row(160.0, 98.0);
        let box_rect = Rect::from_min_size(at - Vec2::splat(2.0), Vec2::new(17.0, 17.0));
        let (resp, click) = pg.hit(box_rect, Id::new("accent-click"), false);
        let resp = resp.on_hover_text(tr(
            "Click to change the theme's accent color.",
            "点击更改此主题的主题色。",
        ));
        let p = pg.ui.painter().clone();
        p.rect_filled(
            box_rect,
            4.0,
            if state.theme.fdark {
                ggray(32)
            } else {
                ggray(243)
            },
        );
        p.rect_stroke(
            box_rect,
            4.0,
            Stroke::new(1.0_f32, pg.ink()),
            StrokeKind::Inside,
        );
        let tier = match click {
            1 => 6,
            2 => 0,
            _ => 3,
        };
        p.rect_filled(
            Rect::from_min_size(at, Vec2::new(13.0, 13.0)),
            4.0,
            state.theme.accent(tier),
        );
        if resp.clicked() {
            state.accent_pick = Some(AccentPick::from_rgb(state.theme.accent_base));
            state.emit(Command::ShowWindow(WindowKind::SetAccent));
        }
    }

    // 原版 `draw_window_preferences.gml:412-421`：左列下方的语言下拉。
    // 组标题跟着语言切换（"Language" / "语言"），但框里的值和菜单两项是
    // 语言本身的专名（"English" / "简体中文"），两种界面语言下都原样印。
    pg.header(22.0, 239.0, 220.0, 65.0, tr("Language", "语言"));
    pg.frame_box(40.0, 260.0, 140.0, 280.0);
    pg.text(
        pg.row(43.0, 263.0),
        LANGUAGE_ITEMS[state.prefs.language as usize % LANGUAGE_ITEMS.len()],
        PREFS_FONT,
        pg.ink(),
    );
    *lang_at = pg.row(40.0, 281.0);
    if pg.arrow(&state.icons, 123.0, 261.0) && !state.prefs_language_menu {
        state.prefs_language_menu = true;
    }

    // 原版 `draw_areaheader(x1 + 258, y1 + 74, 220, 60, "帧率限制")`。
    pg.header(258.0, 74.0, 220.0, 60.0, tr("Max. frame rate", "帧率限制"));
    pg.frame_box(274.0, 95.0, 374.0, 115.0);
    let rate = state.prefs.refresh_rate as usize % rate_items().len();
    pg.text(
        pg.row(277.0, 98.0),
        rate_items()[rate],
        PREFS_FONT,
        pg.ink(),
    );
    *rate_at = pg.row(274.0, 116.0);
    if pg.arrow(&state.icons, 357.0, 96.0) && !state.prefs_rate_menu {
        state.prefs_rate_menu = true;
    }

    // 原版 `draw_areaheader(x1 + 258, y1 + 150, 220, 64, "窗口")`。
    let native = pg.ui.ctx().native_pixels_per_point().unwrap_or(1.0);
    pg.header(258.0, 150.0, 220.0, 64.0, tr("Window", "窗口"));
    // 原版把 `"缩放:              %"` 整串画在 (276, 166)，数字用 `draw_dragvalue`
    // 叠在中间那段空格上（14 个空格是源码里的原始个数）。我们的字体度量和
    // GameMaker 的 fnt_main 不一样，照抄空格数会让数字压住百分号，所以 % 按
    // 最宽的那个数字（400）定在数字右边，位置同样是固定的、不随数值跳动。
    pg.text(
        pg.row(276.0, 166.0),
        tr("Scale:", "缩放:"),
        PREFS_FONT,
        pg.ink(),
    );
    let pct = (state.prefs.effective_scale(native) * 100.0).round() as i32;
    // 原版灵敏度 `1 / window_scale^2`，而 `draw_dragvalue` 是拿它当除数用的：
    // 100% 时约 10 像素进 1 格，400% 时不到 1 像素就进 1 格，倍率越大拖得越快。
    let sens = 10000.0 / (pct * pct) as f32;
    let drag = pg.drag(
        322.0,
        166.0,
        pct,
        sens,
        50,
        400,
        false,
        "",
        &mut state.prefs_drag,
    );
    let sign_x = 322.0 + pg.measure("400", PREFS_FONT) + 4.0;
    pg.text(pg.row(sign_x, 166.0), "%", PREFS_FONT, pg.ink());
    if let Some(pct) = drag {
        state.prefs.window_scale = Some(pct as f32 / 100.0);
        state.emit(Command::SavePreferences);
    }
    // 原版 `draw_button2(x1 + 394, y1 + 161, 72, "重置", window_scale == get_default_window_scale())`。
    if pg.button(
        394.0,
        161.0,
        72.0,
        tr("Reset", "重置"),
        state.prefs.scale_is_default(native),
    ) {
        state.prefs.window_scale = None;
        state.emit(Command::SavePreferences);
    }

    // 原版 `draw_areaheader(x1 + 258, y1 + 239, 220, 65, "图标")` +
    // `draw_checkbox(x1 + 276, y1 + 239 + 16 + 5 + 4, window_icon, "旧图标", …, false, true)`。
    // 开关本身只改设置，真正换图标由 `EditorState::apply_window_icon` 在下一帧发给窗口。
    pg.header(258.0, 239.0, 220.0, 65.0, tr("Icon", "图标"));
    // 中文「旧图标」是重写版的叫法；原版那颗旧图标是 macOS 风格，EN 照原版印 "Fruity icon"。
    if pg.check(
        276.0,
        264.0,
        state.prefs.window_icon,
        tr("Fruity icon", "旧图标"),
        tr(
            "Whether to use the a fruity themed icon instead of the regular icon.",
            "使用旧图标替换新图标。",
        ),
        true,
        false,
    ) {
        state.prefs.window_icon = !state.prefs.window_icon;
        state.emit(Command::SavePreferences);
    }
}

/// 原版页 2「外观」：音符盒与琴键。
fn prefs_appearance(state: &mut EditorState, pg: &mut Page) {
    pg.header(22.0, 74.0, 456.0, 208.0, tr("Note blocks", "音符盒"));
    let blocks = [
        (90.0, Flag::UseColors, "Use colored note blocks", "彩色音符盒", "If the instruments should be identified with\ndifferent colors.", "用音符盒颜色区分音色。"),
        (110.0, Flag::UseIcons, "Show instrument icons", "显示音色图标", "If the instruments should be identified with\ntheir respective icons.", "用音色图标区分音色。"),
        (130.0, Flag::UseShapes, "Use different shapes for each instrument", "音符盒形状", "If the instruments should be identified\nwith different shapes.", "用音符盒形状区分音色。"),
        (150.0, Flag::ShowNumbers, "Show key numbers", "显示音符序号", "Whether to show the amount of right-clicks required\nfor each note block.", "在音符盒上显示需要按右键次数。"),
        (170.0, Flag::ShowOctaves, "Show octave numbers", "显示八度序号", "Whether the number of the octave the note block\nis in should be shown.", "在音符盒上显示第几八度。"),
        (190.0, Flag::ShowIncompatible, "Highlight incompatible note blocks", "标记不兼容的音符盒", "Whether to show a red outline on note blocks with\ncustom instruments or outside the 2 octave range.", "在自定义音色或超出 2 八度范围的音符盒周围显示红框。"),
        (210.0, Flag::Fade, "No fading", "音符盒不透明", "Disables transparency animations on note block sprites", "关闭音符盒上的透明效果。"),
        (230.0, Flag::ShowLayers, "Show layer boxes", "显示每层操作区", "Whether the layer boxes should be shown\non the left side of the workspace.", "显示在界面左侧操作每层设定的区域。"),
        (250.0, Flag::RemoveEffect, "Show effect on removal", "显示删除效果", "Whether to show a fading border\nwhen removing a note block.", "删除音符时显示淡出边框效果。"),
    ];
    for (n, flag, label_en, label_zh, tip_en, tip_zh) in blocks {
        if pg.check(
            40.0,
            n,
            flag.get(&state.flags),
            tr(label_en, label_zh),
            tr(tip_en, tip_zh),
            false,
            false,
        ) {
            state.emit(Command::ToggleFlag(flag));
        }
    }

    // 原版 `draw_window_preferences.gml:492`：强度按百分数拖拽，10%..80%。
    // 原版这一行没有 popup_set_window，所以不给提示。
    // 拖拽 x 原版两种语言不一样（EN 218 / CN 172）。
    pg.text(
        pg.row(60.0, 268.0),
        tr(
            "Selection highlight intensity:         %",
            "选择高亮显示强度:         %",
        ),
        PREFS_FONT,
        pg.ink(),
    );
    let strength = (state.prefs.selection_alpha * 100.0).round() as i32;
    let strength_x = if crate::lang() == 1 { 172.0 } else { 218.0 };
    if let Some(v) = pg.drag(
        strength_x,
        268.0,
        strength,
        2.0,
        10,
        80,
        false,
        "",
        &mut state.prefs_drag,
    ) {
        state.prefs.selection_alpha = v as f32 / 100.0;
        state.emit(Command::SavePreferences);
    }

    pg.header(22.0, 294.0, 456.0, 171.0, tr("Piano", "琴键"));
    let piano = state.flags.show_piano;
    if pg.check(
        40.0,
        310.0,
        piano,
        tr("Show piano", "显示琴键"),
        tr("Whether the piano should be visible.", "钢琴键是否可见。"),
        true,
        false,
    ) {
        state.emit(Command::ToggleFlag(Flag::ShowPiano));
    }
    // 「最多显示键数」：琴键隐藏时同样退化成灰色静态数字（原版 `if (!show_piano) draw_set_color(c_gray)`）。
    let ink = if piano { pg.ink() } else { ggray(128) };
    pg.text(
        pg.row(70.0, 328.0),
        tr("Keys to show:", "最多显示键数:"),
        PREFS_FONT,
        ink,
    );
    let tip = tr(
        "The amount of keys to show. A high number may\nslow down the program on old computers.",
        "同屏显示的琴键数。过多可能会在老电脑上造成卡顿。",
    );
    pg.tip_area(70.0, 328.0, 150.0, 21.0, tip);
    let keys = state.prefs.keys_to_show as i32;
    let drag = pg.drag(
        150.0,
        328.0,
        keys,
        2.0,
        20,
        50,
        !piano,
        tip,
        &mut state.prefs_drag,
    );
    if let Some(keys) = drag {
        state.prefs.keys_to_show = keys.clamp(20, 50) as u8;
        state.emit(Command::SavePreferences);
    }
    let keys_rows = [
        (350.0, Flag::ShowKeyNames, "Show key names", "显示键名", "If the names of the keys should be shown.", "显示琴键所对应音的名称。"),
        (370.0, Flag::ShowKeyNumbers, "Show key numbers", "显示音符序号", "Whether to show the amount of right-clicks required\non each key inside the 2 octave range.", "在 2 八度内的琴键上显示需要按右键次数。"),
        (390.0, Flag::ShowKeyboard, "Show keyboard shortcuts", "显示键盘键位", "Show the keyboard shortcuts of the keys.", "在琴键上显示绑定的键盘键位。"),
        (410.0, Flag::ShowNoteChart, "Show note chart when hovering over keys", "在琴键上时显示五线谱对应音符", "Whether to show a note chart\nwhen hovering over the keys.", "在鼠标停留在琴键上时是否显示五线谱。"),
        (430.0, Flag::ShowOutOfRange, "Highlight out-of-range keys", "标记超范围琴键", "Whether to show a red tint on keys\noutside of the 2 octave range.", "是否为在 2 八度之外的琴键上标红。"),
        (450.0, Flag::Flats, "Use flat symbol", "使用降记号", "Whether to use the flat symbol (b) instead of\nthe sharp symbol (#) on keynames.", "是否为键名使用降记号（b）而不是升记号（#）。"),
    ];
    for (n, flag, label_en, label_zh, tip_en, tip_zh) in keys_rows {
        if pg.check(
            40.0,
            n,
            flag.get(&state.flags),
            tr(label_en, label_zh),
            tr(tip_en, tip_zh),
            false,
            false,
        ) {
            state.emit(Command::ToggleFlag(flag));
        }
    }
}

/// 原版页 3「访问」：滚轮行为（三个档位 + 滚动速度 + 悬停改属性 + 悬停层预览）
/// 与琴键选中行为。
/// 源码末尾的「对琴键右键可更改键位。」和「重置键位」按钮依赖键位编辑功能，
/// 我们只有固定键位，所以这两行也不出现。
fn prefs_usability(state: &mut EditorState, pg: &mut Page) {
    // 原版 `draw_areaheader(x1 + 22, y1 + 74, 456, 140, "鼠标滚轮")`：组里除了三个档位
    // 还有「滚动速度」这一行，只有档位 = 移动视口时可拖，其余情况数字照画但变灰。
    pg.header(22.0, 74.0, 456.0, 140.0, tr("Mouse wheel", "鼠标滚轮"));
    let wheel_rows = [
        (
            90.0,
            0u8,
            "Use mouse wheel to scroll through the song",
            "使用鼠标滚轮移动位置",
            "Use the mouse wheel to scroll through\nthe song horizontally or vertically.",
            "使用鼠标滚轮横向或竖向移动编辑区域位置。",
        ),
        (
            110.0,
            1,
            "Use mouse wheel to change instrument",
            "使用鼠标滚轮改变音色",
            "Use the mouse wheel to toggle between\nthe available instruments.",
            "使用鼠标滚轮切换可用音色。",
        ),
        (
            130.0,
            2,
            "Use mouse wheel to change key",
            "使用鼠标滚轮改变音调",
            "Use the mouse wheel to toggle\nbetween the keys on the piano.",
            "使用鼠标滚轮改变目前使用的音调。",
        ),
    ];
    for (n, value, label_en, label_zh, tip_en, tip_zh) in wheel_rows {
        if pg.radio(
            40.0,
            n,
            state.prefs.mousewheel == value,
            tr(label_en, label_zh),
            tr(tip_en, tip_zh),
            false,
        ) {
            state.prefs.mousewheel = value;
            state.emit(Command::SavePreferences);
        }
    }
    // 原版 `draw_text_dynamic(x1 + 70, y1 + 150, "滚动速度：")` +
    // `draw_dragvalue(22, x1 + 150, y1 + 150, mousewheel_scroll_speed, 1)`（夹 1..=10），
    // 档位不是「移动视口」时数字变灰且拖不动。提示挂在整块 180×16 上（popup_set_window）。
    let speed_on = state.prefs.mousewheel == 0;
    let speed_tip = tr(
        "The number of ticks or layers scrolled with each mouse wheel step.\nThis setting does not affect macOS trackpads.",
        "每滚动一次鼠标滚轮所移动的刻或层数。\n此设置不影响 macOS 触控板。",
    );
    pg.text(
        pg.row(70.0, 150.0),
        tr("Scroll speed:", "滚动速度："),
        PREFS_FONT,
        if speed_on { pg.ink() } else { ggray(128) },
    );
    pg.tip_area(70.0, 150.0, 180.0, 16.0, speed_tip);
    let speed = state.prefs.mousewheel_scroll_speed as i32;
    if let Some(v) = pg.drag(
        150.0,
        150.0,
        speed,
        2.0,
        1,
        10,
        !speed_on,
        "",
        &mut state.prefs_drag,
    ) {
        state.prefs.mousewheel_scroll_speed = v.clamp(1, 10) as u8;
        state.emit(Command::SavePreferences);
    }
    if pg.check(
        40.0,
        178.0,
        state.prefs.changepitch,
        tr("Change note properties when scrolling over notes", "当在音符上使用鼠标滚轮改变音符属性"),
        tr(
            "Whether scrolling when hovering over a note should change its key,\nvelocity, panning or pitch, according to the currently selected edit mode.",
            "在音符上使用鼠标滚轮时是否根据目前模式改变其音调、音量、声道或音高。",
        ),
        false,
        false,
    ) {
        state.prefs.changepitch = !state.prefs.changepitch;
        state.emit(Command::SavePreferences);
    }
    // 原版 `draw_checkbox(x1 + 40, y1 + 198, layerhov_vppreview, …)`。
    if pg.check(
        40.0,
        198.0,
        state.prefs.layerhov_vppreview,
        tr(
            "Apply hovered layer's volume and panning to note preview sounds",
            "将悬停层的音量和声道调整应用于音符预览声音",
        ),
        tr(
            "Whether the preview sound's volume and panning should be\nrelative to the layer the mouse is hovering over.",
            "预览声音的音量和声道是否应与鼠标悬停的层相关。",
        ),
        false,
        false,
    ) {
        state.prefs.layerhov_vppreview = !state.prefs.layerhov_vppreview;
        state.emit(Command::SavePreferences);
    }

    // 原版 `draw_areaheader(x1 + 22, y1 + 240, 456, 105, "琴键与层")`；
    // 组里剩下的「对琴键右键可更改键位。」+「重置键位」按钮依赖键位编辑，端口没有，整行不画。
    pg.header(
        22.0,
        240.0,
        456.0,
        105.0,
        tr("Piano and layers", "琴键与层"),
    );
    if pg.check(
        40.0,
        256.0,
        state.prefs.select_lastpressed,
        tr("Set selected key to pressed one", "选择按下的键"),
        tr(
            "If the selected key should be set\nto the one pressed using the keyboard.",
            "是否选择按下的键所对应的音。",
        ),
        false,
        false,
    ) {
        state.prefs.select_lastpressed = !state.prefs.select_lastpressed;
        state.emit(Command::SavePreferences);
    }
}

/// 原版页 4「播放」：进度条跟随、区间起播/停播、播放开关、速度单位。
/// 原版 `taskbar`（任务栏进度）那一行在上游自己就被注释掉了（`:612`），两边一致。
fn prefs_playback(state: &mut EditorState, pg: &mut Page) {
    pg.header(22.0, 74.0, 456.0, 145.0, tr("Marker", "进度条"));
    if pg.check(
        40.0,
        90.0,
        state.marker_follow,
        tr("Follow marker when playing", "播放时跟随进度条"),
        tr(
            "Automatically scroll along with the\nmarker when playing the song.",
            "播放歌曲时和进度条一起滚动。",
        ),
        false,
        false,
    ) {
        state.emit(Command::ToggleFollow);
    }
    // 原版 `marker_pagebypage`：1 按页 / 0 按刻 / 2 按帧，跟随关掉时三格都锁定。
    let lock = !state.marker_follow;
    let follow_rows = [
        (
            110.0,
            1u8,
            "Page by page",
            "按页",
            "Scroll with the marker every page.",
            "跟随进度条翻页。",
        ),
        (
            130.0,
            0,
            "Tick by tick",
            "按刻",
            "Scroll with the marker every tick.",
            "跟随进度条滚动。",
        ),
        (
            150.0,
            2,
            "Frame by frame",
            "按帧",
            "Scroll with the marker every frame.",
            "跟随进度条逐帧滚动。",
        ),
    ];
    for (n, value, label_en, label_zh, tip_en, tip_zh) in follow_rows {
        if pg.radio(
            70.0,
            n,
            state.follow_mode == value,
            tr(label_en, label_zh),
            tr(tip_en, tip_zh),
            lock,
        ) {
            state.follow_mode = value;
            state.emit(Command::SavePreferences);
        }
    }

    // 原版 `:600-601`（中文 `:652-653`）：区间起播/停播两格，挂在「进度条」这个框里，
    // 和标尺右键菜单后两条是同一对全局量。
    if pg.check(
        40.0,
        170.0,
        state.prefs.marker_start,
        tr("Start playing in section", "在区间时开始播放"),
        tr(
            "Whether to always start playing\nat the start of the active section.",
            "有区间存在时是否在区间起始处开始播放。",
        ),
        false,
        false,
    ) {
        state.prefs.marker_start = !state.prefs.marker_start;
        state.emit(Command::SavePreferences);
    }
    if pg.check(
        40.0,
        190.0,
        state.prefs.marker_end,
        tr("Stop playing after section", "在区间后停止播放"),
        tr(
            "Whether to stop playing when the\nmarker passes the active section.",
            "有区间存在时是否在区间结束处停止播放。",
        ),
        false,
        false,
    ) {
        state.prefs.marker_end = !state.prefs.marker_end;
        state.emit(Command::SavePreferences);
    }

    pg.header(22.0, 244.0, 218.0, 120.0, tr("Playing", "播放"));
    if pg.check(
        32.0,
        260.0,
        state.prefs.show_layer_volumes,
        tr("Show layer volumes", "显示每层音量"),
        tr(
            "Whether to show the volume of layers.",
            "是否显示每层的音量。",
        ),
        false,
        false,
    ) {
        state.prefs.show_layer_volumes = !state.prefs.show_layer_volumes;
        state.emit(Command::SavePreferences);
    }
    if pg.check(
        32.0,
        280.0,
        state.prefs.mono,
        tr("Disable stereo", "关闭立体声"),
        tr("Disables stereo playback.", "关闭立体声音效。"),
        false,
        false,
    ) {
        state.emit(Command::ToggleMono);
    }
    if pg.check(
        32.0,
        300.0,
        state.prefs.show_sound_count,
        tr("Show number of active sounds", "显示正在播放的声音数"),
        tr(
            "Displays the number of sounds that are\ncurrently playing in the status bar.",
            "在状态栏里显示正在播放的声音数。",
        ),
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
        tr("Use more audio channels", "提升声音限制"),
        tr(
            "Whether to increase the number\nof sounds that can play at once.",
            "提升可同时播放的声音数。",
        ),
        false,
        false,
    ) {
        state.emit(Command::ToggleExtendedVoices);
    }

    pg.header(255.0, 244.0, 223.0, 60.0, tr("Tempo unit", "速度单位"));
    if pg.radio(
        265.0,
        260.0,
        !state.prefs.use_bpm,
        tr("Ticks per second (t/s)", "红石刻 / 秒 (t/s)"),
        tr(
            "Display song tempos in ticks per second.",
            "使用每秒几刻显示速度。",
        ),
        false,
    ) {
        state.prefs.use_bpm = false;
        state.emit(Command::SavePreferences);
    }
    if pg.radio(
        265.0,
        280.0,
        state.prefs.use_bpm,
        tr("Beats per minute (BPM)", "拍数 / 分钟 (BPM)"),
        tr(
            "Display song tempos in beats per minute.",
            "使用每分钟多少拍显示速度。",
        ),
        false,
    ) {
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

/// 原版 `show_menu_ext` + `obj_menu`：首选项「主题 / 帧率限制」那两个下拉和 MIDI 导入
/// 窗口用的是同一个菜单对象，所以直接复用 `midi_popup` 的板子、勾和屏幕内收边。
/// 返回（选中的下标，是否被点掉）。
fn popup_list(
    ui: &Ui,
    id: &str,
    at: Pos2,
    items: &[&str],
    sel: usize,
    th: &Theme,
    bank: Option<&crate::icons::Bank>,
) -> (Option<usize>, bool) {
    let rows: Vec<MidiMenuItem> = items
        .iter()
        .enumerate()
        .map(|(i, s)| MidiMenuItem::item(*s, i as i16, i == sel))
        .collect();
    // 这两项都没有子菜单，展开状态只是走个形式。
    let mut open = None;
    let (picked, dismissed) = midi_popup(ui, id, at, &rows, &mut open, th, bank);
    (picked.map(|v| v as usize), dismissed)
}

/// 原版 `show_menu_ext("theme", …)`：四套主题，当前项打勾。
fn theme_menu(state: &mut EditorState, ui: &Ui, at: Pos2) {
    let names: Vec<&str> = THEME_MENU_ORDER.iter().map(|k| k.name()).collect();
    let sel = THEME_MENU_ORDER
        .iter()
        .position(|k| *k == state.theme.kind)
        .unwrap_or(usize::MAX);
    let th = state.theme.clone();
    let bank = state.icons.bank();
    let (picked, dismissed) = popup_list(
        ui,
        "prefs_theme_menu",
        at,
        &names,
        sel,
        &th,
        bank.as_deref(),
    );
    if let Some(i) = picked {
        state.emit(Command::SetTheme(THEME_MENU_ORDER[i]));
    }
    if picked.is_some() || dismissed {
        state.prefs_theme_menu = false;
    }
}

/// 原版 `show_menu_ext("refreshrate", …)`：框里显示的那一行和菜单五项是同一串字。
fn rate_menu(state: &mut EditorState, ui: &Ui, at: Pos2) {
    let th = state.theme.clone();
    let bank = state.icons.bank();
    let items = rate_items();
    // 原版五项都有 `check(refreshrate = i)`，当前帧率那一项前面要打勾。
    let sel = state.prefs.refresh_rate as usize;
    let (picked, dismissed) =
        popup_list(ui, "prefs_rate_menu", at, &items, sel, &th, bank.as_deref());
    if let Some(i) = picked {
        state.prefs.refresh_rate = i as u8;
        state.emit(Command::SavePreferences);
    }
    if picked.is_some() || dismissed {
        state.prefs_rate_menu = false;
    }
}

/// 原版 `show_menu_ext("language", …)`（draw_window_preferences.gml:421）：
/// `"English|简体中文"` 两项，当前语言那项打勾。切换走 prefs.language，
/// 下一帧 app::show 同步进 tr() 的全局开关，全界面跟着换。
const LANGUAGE_ITEMS: [&str; 2] = ["English", "简体中文"];
fn language_menu(state: &mut EditorState, ui: &Ui, at: Pos2) {
    let th = state.theme.clone();
    let bank = state.icons.bank();
    let sel = state.prefs.language as usize;
    let (picked, dismissed) = popup_list(
        ui,
        "prefs_language_menu",
        at,
        &LANGUAGE_ITEMS,
        sel,
        &th,
        bank.as_deref(),
    );
    if let Some(i) = picked {
        state.prefs.language = i as u8;
        state.emit(Command::SavePreferences);
    }
    if picked.is_some() || dismissed {
        state.prefs_language_menu = false;
    }
}

/// 首选项正文的字号。原版是 `fnt_main` 9 像素、菜单栏是 `fnt_small` 8 像素，
/// 但重写版其余界面（菜单栏、面板、状态栏）都走 egui 默认档，9 像素夹在里面明显偏小，
/// 所以按用户要求取菜单栏同一档：`TextStyle::Button`，下面那条测试盯着这个等式。
const PREFS_FONT: f32 = 12.5;
/// 分组标题与页签：原版 `fnt_info_med` 11 对 `fnt_main` 9（约 1.22 倍），这里同比例放大一档。
const PREFS_FONT_MED: f32 = 14.0;

/// 原版「帧率限制」下拉的五项，下标就是 `refreshrate`。
fn rate_items() -> [&'static str; 5] {
    if crate::lang() == 1 {
        ["30帧", "60帧", "120帧", "144帧", "240帧"]
    } else {
        ["30FPS", "60FPS", "120FPS", "144FPS", "240FPS"]
    }
}

/// 原版首选项的五个页签（`draw_window_preferences.gml` 的 `str` 数组）。
fn prefs_tabs() -> [&'static str; 5] {
    if crate::lang() == 1 {
        ["通用", "界面", "外观", "访问", "播放"]
    } else {
        [
            "General",
            "Interface",
            "Appearance",
            "Usability",
            "Playback",
        ]
    }
}

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

    /// 原版 `string_truncate(str, maxwidth)` + `string_maxwidth`：逐字累加宽度，
    /// 放不下就截断并在末尾补 `...`（省略号本身不给预留宽度，源码就是这么算的）。
    fn truncate(&self, s: &str, max_w: f32) -> String {
        if self.measure(s, PREFS_FONT) <= max_w {
            return s.to_string();
        }
        let mut out = String::new();
        let mut w = 0.0;
        for c in s.chars() {
            let cw = self.measure(&c.to_string(), PREFS_FONT);
            if w + cw > max_w {
                break;
            }
            w += cw;
            out.push(c);
        }
        out.push_str("...");
        out
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
        let font = if self.fluent {
            PREFS_FONT_MED
        } else {
            PREFS_FONT
        };
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
                    p.rect_stroke(
                        rect,
                        2.0,
                        Stroke::new(1.0_f32, self.th.border),
                        StrokeKind::Inside,
                    );
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
            (
                if self.th.fdark {
                    ggray(182)
                } else {
                    ggray(165)
                },
                2.0,
            )
        } else {
            (self.th.border, 0.0)
        };
        self.ui
            .painter()
            .rect_stroke(rect, r, Stroke::new(1.0_f32, line), StrokeKind::Inside);
    }

    /// 原版 `draw_textarea(focus, x, y, w, h, val, hint)`。先铺一层内衬（默认白，
    /// 主题 2 是 (54,54,54)，Fluent 用次按钮那组灰阶），再画框：主题 0 是
    /// `spr_textbox` 九宫格、主题 1/2 是 `draw_area`、Fluent 是 `spr_textbox_f`
    /// 九宫格（悬停换亮档、聚焦时底边吃强调色）。九宫格都没导出，统一近似成
    /// 主题描边；文字编辑区在 (x+3, y+3) 大小 (w-6, h-6)，交给 egui 的 TextEdit。
    /// 返回值是本帧文本真的被改过（对应原版的 `text_focus` 那一档）。
    fn textarea(
        &mut self,
        x: f32,
        n: f32,
        w: f32,
        h: f32,
        buf: &mut String,
        hint: &str,
        salt: &str,
    ) -> bool {
        let at = self.row(x, n);
        let rect = Rect::from_min_size(at, Vec2::new(w, h));
        let on = self
            .ui
            .interact(rect, Id::new(("textarea-hover", salt)), Sense::hover())
            .hovered();
        let p = self.ui.painter().clone();
        let fill = if self.fluent {
            match (self.th.fdark, on) {
                (false, false) => ggray(252),
                (false, true) => ggray(246),
                (true, false) => ggray(45),
                (true, true) => ggray(50),
            }
        } else if self.th.kind == ThemeKind::Dark {
            ggray(54)
        } else {
            Color32::WHITE
        };
        p.rect_filled(rect.shrink(1.0), 0.0, fill);
        let (line, r) = if self.fluent {
            (if on { self.th.accent } else { self.th.border }, 4.0)
        } else {
            (self.th.border, 0.0)
        };
        p.rect_stroke(rect, r, Stroke::new(1.0_f32, line), StrokeKind::Inside);
        let edit = Rect::from_min_size(at + Vec2::new(3.0, 3.0), Vec2::new(w - 6.0, h - 6.0));
        let resp = self.ui.put(
            edit,
            egui::TextEdit::singleline(buf)
                .hint_text(hint)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .font(FontId::proportional(PREFS_FONT)),
        );
        let typed = resp.changed();
        if self.fluent && resp.has_focus() {
            // `spr_textbox_f` 聚焦时底边 2 像素换成强调色（`accent[6] * focus`）。
            p.rect_filled(
                Rect::from_min_max(at + Vec2::new(2.0, h - 2.0), at + Vec2::new(w - 2.0, h)),
                0.0,
                self.th.accent,
            );
        }
        typed
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
    fn tabs(&mut self, b0: f32, base: f32, labels: &[&str], tab: u8) -> u8 {
        let font = if self.fluent {
            PREFS_FONT_MED
        } else {
            PREFS_FONT
        };
        let mut nsel = tab;
        let mut b = b0;
        for (i, name) in labels.iter().enumerate() {
            let tw = self.measure(name, font);
            let (hy, hh) = if self.fluent {
                (0.0, 39.0)
            } else {
                (2.0, 18.0)
            };
            let rect = Rect::from_min_size(self.at(b, base + hy), Vec2::new(tw + 12.0, hh));
            let resp = self.ui.interact(rect, Id::new(("tabs", i)), Sense::click());
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
                        Rect::from_min_size(
                            self.at(b + tw / 2.0 - 1.0, base + 33.0),
                            Vec2::new(14.0, 3.0),
                        ),
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
                    let body =
                        Rect::from_min_size(self.at(b - 2.0, base), Vec2::new(tw + 15.0, 20.0));
                    let line =
                        Rect::from_min_size(self.at(b - 2.0, base), Vec2::new(tw + 15.0, 21.0));
                    p.rect_filled(body, 0.0, fill);
                    p.rect_stroke(
                        line,
                        0.0,
                        Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                        StrokeKind::Inside,
                    );
                    p.rect_filled(
                        Rect::from_min_size(
                            self.at(b - 1.0, base + 20.0),
                            Vec2::new(tw + 13.0, 1.0),
                        ),
                        0.0,
                        fill,
                    );
                }
            }
            self.text(self.at(b + 6.0, base + ty), name, font, ink);
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
    fn check(
        &mut self,
        x: f32,
        n: f32,
        on: bool,
        label: &str,
        tip: &str,
        switch: bool,
        lock: bool,
    ) -> bool {
        let tw = self.measure(label, PREFS_FONT);
        let id = Id::new(("prefs-check", x as i32, n as i32, label));
        if !self.fluent {
            return self.legacy_check(x, n, on, label, lock, tw);
        }
        let at = self.row(x, n);
        let (resp, m) = self.hit(
            Rect::from_min_size(at, Vec2::new(26.0 + tw + 8.0, 13.0)),
            id,
            lock,
        );
        let fdark = self.th.fdark;
        let acc = self
            .th
            .accent(5 + if m == 1 { 2 } else { 0 } + if m == 2 { 3 } else { 0 });
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
                    if lock {
                        ggray(56)
                    } else {
                        ggray(204)
                    }
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
                (
                    at.x + 15.0 - if m == 2 && !lock { 1.0 } else { 0.0 },
                    at.y + if lock { 3.0 } else { 2.0 },
                )
            } else {
                (at.x + 2.0, at.y + 2.0)
            };
            let kw = if on {
                7.0
            } else {
                7.0 + if m == 2 && !lock { 1.0 } else { 0.0 }
            };
            p.rect_filled(
                Rect::from_min_size(Pos2::new(kx, ky), Vec2::new(kw, 7.0)),
                12.0,
                knob_fill,
            );
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
                p.rect_stroke(
                    box_rect,
                    2.0,
                    Stroke::new(1.0_f32, line),
                    StrokeKind::Inside,
                );
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
        let (resp, m) = self.hit(
            Rect::from_min_size(at, Vec2::new(12.0 + tw + 8.0, 12.0)),
            id,
            lock,
        );
        let fdark = self.th.fdark;
        let c = at + Vec2::new(5.0, 6.0);
        let p = self.ui.painter().clone();
        let fill = match (lock, on) {
            (false, true) => self.th.accent(5),
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
            let r = if lock {
                3.0
            } else {
                3.0 + if m == 1 && on { 1.0 } else { 0.0 }
            };
            p.circle_filled(c, r, dot);
        }
        self.text(
            at + Vec2::new(18.0, -1.0),
            label,
            PREFS_FONT,
            if lock { ggray(204) } else { self.ink() },
        );
        resp.on_hover_text(tip).clicked()
    }

    /// 原版 `draw_button2(x, y, w, str, lock)`：没有第 6 个参数就是主按钮。
    fn button(&mut self, x: f32, n: f32, w: f32, label: &str, lock: bool) -> bool {
        self.button2(x, n, w, label, lock, false)
    }

    /// 原版 `draw_button2(x, y, w, str[, lock[, secondary]])`。Fluent 是圆角矩形，
    /// 主按钮填强调色、次按钮填 `secondary` 那组灰阶（取消 / 使用默认值都是次按钮，
    /// 而且只有主按钮吃回车，见源码的 `keyboard_invoke`）；
    /// 其余三套是 `spr_button` 九宫格（没导出），退回 egui 原生按钮。
    fn button2(
        &mut self,
        x: f32,
        n: f32,
        w: f32,
        label: &str,
        lock: bool,
        secondary: bool,
    ) -> bool {
        let id = Id::new(("prefs-btn", x as i32, n as i32, label));
        if !self.fluent {
            return self.legacy_row(x, n, w, id, lock, |ui| {
                ui.add_sized(
                    [w, 21.0],
                    egui::Button::new(RichText::new(label).size(PREFS_FONT)),
                )
                .clicked()
            });
        }
        let at = self.row(x, n);
        let (resp, m) = self.hit(Rect::from_min_size(at, Vec2::new(w, 23.0)), id, lock);
        let body = Rect::from_min_size(at, Vec2::new(w - 2.0, 21.0));
        let p = self.ui.painter().clone();
        // 次按钮的三组灰阶：`16579836 / 16185078 / 16185078` 填、
        // `15592941 / 15066597 / 15066597` 描边；fdark 是 `45 / 50 / 38` 与 `54 / 86 / 54`。
        let (fill, line) = if lock {
            if self.th.fdark {
                (ggray(72), ggray(80))
            } else {
                (ggray(248), ggray(238))
            }
        } else if secondary {
            if self.th.fdark {
                (ggray([45, 50, 38][m]), ggray([54, 86, 54][m]))
            } else {
                (ggray([252, 246, 246][m]), ggray([238, 229, 229][m]))
            }
        } else {
            (self.th.accent(5 - m), self.th.accent([4, 5, 3][m]))
        };
        p.rect_filled(body, 4.0, fill);
        p.rect_stroke(body, 4.0, Stroke::new(1.0_f32, line), StrokeKind::Inside);
        // 主按钮是强调色底，文字才用白 / 黑；次按钮沿用主题文字色（`draw_theme_color()`）。
        let ink = if lock {
            ggray(127)
        } else if secondary {
            self.ink()
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
        let rect = Rect::from_min_size(
            at + Vec2::new(-8.0, -4.0),
            Vec2::new(size.x + 16.0, size.y + 4.0),
        );
        let sense = if lock { Sense::hover() } else { Sense::drag() };
        let resp = self
            .ui
            .interact(rect, Id::new(("prefs-drag", x as i32, n as i32)), sense)
            .on_hover_cursor(CursorIcon::ResizeNorth);
        // 原版 `draw_dragvalue` 没有 `popup_set_window`，这类行就是没有提示。
        let resp = if tip.is_empty() {
            resp
        } else {
            resp.on_hover_text(tip)
        };
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

    /// 原版 `draw_abutton(x2 - 17, y1 + 96)`：下拉框右边的箭头（`spr_button_arrow`）。
    /// 精灵图找不到时画一个同位置的三角形，仍接同一个开关——不能留个看不见的热点。
    fn arrow(&mut self, icons: &crate::icons::Icons, x: f32, n: f32) -> bool {
        let at = self.row(x, n);
        match icons.bank() {
            Some(b) => b.arrow_button(self.ui, at, self.th),
            None => {
                let rect = Rect::from_min_size(at, Vec2::new(17.0, 18.0));
                let ink = self.ink();
                let p = self.ui.painter().clone();
                p.add(egui::Shape::convex_polygon(
                    vec![
                        at + Vec2::new(4.0, 7.0),
                        at + Vec2::new(13.0, 7.0),
                        at + Vec2::new(8.5, 12.5),
                    ],
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
                    vec![
                        at + Vec2::new(23.0, 13.0),
                        at + Vec2::new(23.0, 27.0),
                        at + Vec2::new(15.0, 20.0),
                    ],
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
    /// 控件文字一律显式带 `PREFS_FONT`：默认字号是主题的 body 字号，比原版大一号，
    /// 长标签会在这 20 像素的一行里被折成两行。
    fn legacy_row(
        &mut self,
        x: f32,
        n: f32,
        w: f32,
        id: Id,
        lock: bool,
        add: impl FnOnce(&mut Ui) -> bool,
    ) -> bool {
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
            ui.checkbox(&mut v, RichText::new(label).size(PREFS_FONT))
                .changed()
        })
    }

    fn legacy_radio(&mut self, x: f32, n: f32, on: bool, label: &str, lock: bool) -> bool {
        let id = Id::new(("prefs-radio", x as i32, n as i32, label));
        self.legacy_row(
            x,
            n - 3.0,
            18.0 + self.measure(label, PREFS_FONT) + 24.0,
            id,
            lock,
            |ui| {
                ui.radio(on, RichText::new(label).size(PREFS_FONT))
                    .clicked()
                    && !on
            },
        )
    }
}

fn instruments_window(state: &mut EditorState, ui: &mut Ui) {
    egui::Grid::new("instruments_grid")
        .num_columns(2)
        .show(ui, |ui| {
            for (i, ins) in INSTRUMENTS.iter().enumerate() {
                if ui
                    .selectable_label(state.current.instrument == i as u8, ins.name)
                    .clicked()
                {
                    state.emit(Command::SetInstrument(i as u8));
                }
                // 原版只给前 16 个乐器绑了数字键（`control_draw.gml:822-843`），
                // 铜块一族没有按键就留空，不能印一个按了没反应的组合键。
                let hint = if i < crate::shortcuts::INSTRUMENT_HOTKEYS {
                    if i < 9 {
                        format!("Ctrl+{}", i + 1)
                    } else {
                        format!("Ctrl+Shift+{}", i - 8)
                    }
                } else {
                    String::new()
                };
                ui.label(RichText::new(hint).small().weak());
                ui.end_row();
            }
        });
    ui.separator();
    ui.label(RichText::new(tr(
        &format!(
            "{} built-in instruments; custom instrument indices >= {}, read and written with .nbs files",
            INSTRUMENTS.len(),
            nbs_core::model::FIRST_CUSTOM_INDEX
        ),
        &format!(
            "{} 种内置乐器；自定义乐器索引 >= {}，随 .nbs 文件读写",
            INSTRUMENTS.len(),
            nbs_core::model::FIRST_CUSTOM_INDEX
        ),
    ))
    .small()
    .weak());
    footer(state, ui);
}

/// 原版 `draw_set_halign(fa_center) + draw_text_dynamic(x, y, str)`：`cx` 是文字的中心。
fn center_text(pg: &Page, cx: f32, n: f32, s: &str, color: Color32) {
    let at = pg.row(cx - pg.measure(s, PREFS_FONT) / 2.0, n);
    pg.text(at, s, PREFS_FONT, color);
}

/// 取色器里的 HSV 转 RGB。原版这三处用的是内建 `make_color_hsv`（三个分量都 0..255），
/// 端口统一走 `scr_HSBtoRGB` 那条换算（色相 0..360、饱和明度 0..100），同一套 HSV 模型。
fn hsv_color(h: f32, s: f32, v: f32) -> Color32 {
    let [r, g, b] = Theme::hsb_to_rgb(h, s, v);
    Color32::from_rgb(r, g, b)
}

/// 原版 `draw_window_set_accent.gml`：Fluent 首选项里点主题色块开进来的取色器。
/// 上面 `spr_hsv`（187x225）挑色相/饱和，紧挨着的 `spr_value`（240x8）挑明度（整条按
/// 当前色相/饱和调制），两个游标是 `spr_radiobox` 的第 18、21 帧；右侧两片圆角色块是
/// 「正在挑的色 / 当前生效的 accent[3]」，下面四格是这支色铺在深浅底上的预览，再往下
/// 是 RGB 三个文本框和「使用默认值 / 取消 / 确定」。
/// 原版把鼠标判定放在绘制之后（:44-73），所以方块、色块、游标用的是上一帧的值，
/// 预览格用的是本帧的——端口按同样顺序排，不提前刷新。
fn set_accent_window(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    // :14-22：原版靠 `resetcolor` 在开窗第一帧把 rr/gg/bb 复位成 accent1/2/3；
    // 端口用「还没有编辑态」当同一个条件（首选项点色块时新建、用默认值时重建）。
    let mut pick = state
        .accent_pick
        .clone()
        .unwrap_or_else(|| AccentPick::from_rgb(state.theme.accent_base));
    // :8-10：`draw_window(x1, y1, x1 + 280, y1 + 440)`。
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(280.0 - ml - mr, if fluent { 432.0 } else { 414.0 }),
        Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let mut pg = Page {
        ui,
        o,
        ry,
        th: &th,
        fluent,
    };
    let p = pg.ui.painter().clone();
    let sq = pg.row_rect(20.0, 43.0, 207.0, 268.0);
    let bar = pg.row_rect(20.0, 278.0, 260.0, 286.0);
    // :27-32 那块白底面板的条件是 `theme = 0`，而这扇只有 Fluent 才开得到（色块只画在
    // `draw_window_preferences.gml:313` 的 theme 3 分支里），原版那段到不了，不跟。
    if let Some(bk) = state.icons.bank() {
        // :33-34：方块原图不上色；明度条按 `make_color_hsv(h / 360 * 255, s * 2.55, 255)`
        // 调制，也就是「这支色相/饱和的满明度版本」当色带。
        bk.hsv_square(&p, sq.min);
        bk.value_bar(&p, bar.min, hsv_color(pick.hsv[0], pick.hsv[1], 100.0));
    }
    // :35-39：右侧两片色块，上面是正在挑的 rr/gg/bb，下面是当前生效的 accent[3]。
    // 原版走 rectircle 着色器画连续圆角（半径取默认 16），egui 只有普通圆角，近似。
    let picked = Color32::from_rgb(pick.rgb[0], pick.rgb[1], pick.rgb[2]);
    p.rect_filled(
        Rect::from_min_max(pg.row(217.0, 42.0), pg.row(260.0, 154.0)),
        16.0,
        picked,
    );
    p.rect_filled(
        Rect::from_min_max(pg.row(217.0, 155.0), pg.row(260.0, 267.0)),
        16.0,
        th.accent(3),
    );
    if let Some(bk) = state.icons.bank() {
        // :42 方块上的游标：横 `floor(187 * h / 360)`、纵从下往上 `floor(225 * s / 100)`，
        // 精灵是 12x12，各减 5 让它居中在取到的点上。
        bk.picker_cursor(
            &p,
            pg.row(
                20.0 + (187.0 * pick.hsv[0] / 360.0).floor() - 5.0,
                43.0 + 225.0 - (225.0 * pick.hsv[1] / 100.0).floor() - 5.0,
            ),
            18,
        );
        // :43 明度条上的竖标：横向行程原版写的是 232，而判定区宽 240（:56）——
        // 原版自己的不一致，拖到最右端游标离右边缘还差 8 像素，照抄不改。
        bk.picker_cursor(
            &p,
            pg.row(20.0 + (232.0 * pick.hsv[2] / 100.0).floor(), 276.0),
            21,
        );
    }
    // :44-73：两块判定区各认一条，按下落在哪块就认死那块，滑出判定区也继续按那块算，
    // 松手才清零；两块都没沾上是 `nocdrag`，什么都不做。egui 的 `Sense::drag` 只有
    // 「按下 originating 的那个控件才拖得动」这一种语义，跟原版完全对得上。
    let hs = pg.ui.interact(sq, Id::new("accent-hs"), Sense::drag());
    let vv = pg.ui.interact(bar, Id::new("accent-v"), Sense::drag());
    if let Some(pt) = pg.ui.input(|i| i.pointer.interact_pos()) {
        if hs.dragged() {
            // :46-47 那两串的括号展开后就是「夹进方块再换算」：色相看横向，饱和看纵向取反。
            let dx = (pt.x - sq.min.x).clamp(0.0, 187.0);
            let dy = (pt.y - sq.min.y).clamp(0.0, 225.0);
            pick.hsv[0] = dx / 187.0 * 360.0;
            pick.hsv[1] = (1.0 - dy / 225.0) * 100.0;
            // :48-54：拖一步就重算 rr/gg/bb，并把三个文本框刷成新的数字。
            pick.apply_hsv();
        } else if vv.dragged() {
            // :57-64：明度只看横向 0..240。
            let dx = (pt.x - bar.min.x).clamp(0.0, 240.0);
            pick.hsv[2] = dx / 240.0 * 100.0;
            pick.apply_hsv();
        }
    }
    // :74-98：四个预览格。整段用 fa_center，给的横坐标都是格心。第一格铺 rr/gg/bb、
    // 字用白；第二格铺「明度按饱和折一档」的那支、字用 rr/gg/bb；第三、四格铺近黑
    // (9,9,9) 和近白 (221,221,221)，字都用 rr/gg/bb。
    // :82 那条公式换成 0..100 就是「饱和过半取半明度，不足取双明度」，超过 255 的
    // 部分原版交给 make_color_hsv 夹住；端口换算没有输入夹取，先 `min(100)` 再转。
    let col = Color32::from_rgb(pick.rgb[0], pick.rgb[1], pick.rgb[2]);
    let bright = if pick.hsv[1] >= 50.0 {
        pick.hsv[2] * 0.5
    } else {
        (pick.hsv[2] * 2.0).min(100.0)
    };
    p.rect_filled(pg.row_rect(20.0, 298.0, 141.0, 318.0), 0.0, col);
    p.rect_filled(
        pg.row_rect(142.0, 298.0, 260.0, 318.0),
        0.0,
        hsv_color(pick.hsv[0], pick.hsv[1], bright),
    );
    p.rect_filled(pg.row_rect(20.0, 319.0, 141.0, 339.0), 0.0, ggray(9));
    p.rect_filled(pg.row_rect(142.0, 319.0, 260.0, 339.0), 0.0, ggray(221));
    let label = tr("Preview", "预览");
    center_text(&pg, 80.0, 302.0, label, Color32::WHITE);
    center_text(&pg, 202.0, 302.0, label, col);
    center_text(&pg, 80.0, 323.0, label, col);
    center_text(&pg, 202.0, 323.0, label, col);
    // :99-100 分组标题，:102-110 三个 71x25 的文本框（`x1 + 20`、`+85`、`+170`）。
    pg.header(10.0, 353.0, 220.0, 35.0, tr("RGB color", "RGB 调色"));
    let t0 = pg.textarea(
        20.0,
        360.0,
        71.0,
        25.0,
        &mut pick.text[0],
        tr("Set red amount.", "设置红色值。"),
        "setaccent-60",
    );
    let t1 = pg.textarea(
        105.0,
        360.0,
        71.0,
        25.0,
        &mut pick.text[1],
        tr("Set green amount.", "设置绿色值。"),
        "setaccent-61",
    );
    let t2 = pg.textarea(
        190.0,
        360.0,
        71.0,
        25.0,
        &mut pick.text[2],
        tr("Set blue amount.", "设置蓝色值。"),
        "setaccent-62",
    );
    // :111-127：三个框都有内容才整批转数字，任何一个转不了就这批作废（`err = 1`，
    // 原版这里不弹提示）。端口把 rr/gg/bb 存成 0..=255 的整数，超范围和小数都按原版
    // `make_color_rgb` 的取整方式夹住、截断。:125 那句 `if (text_focus)` 只在焦点还在
    // 文本框上时才反推 hsv；端口用「这一帧文本真的被改过」当同一条件，否则每帧从文本
    // 往返一次颜色， hsv 会被整数化，游标跟着跳一格。
    if pick.text.iter().all(|t| !t.is_empty()) {
        let nums = std::array::from_fn(|i| pick.text[i].parse::<f32>().ok());
        if let [Some(dr), Some(dg), Some(db)] = nums {
            let to = |v: f32| v.clamp(0.0, 255.0).floor() as u8;
            pick.rgb = [to(dr), to(dg), to(db)];
            if t0 || t1 || t2 {
                pick.hsv = Theme::rgb_to_hsb(pick.rgb[0], pick.rgb[1], pick.rgb[2]);
            }
        }
    }
    // :131-139「使用默认值」：次按钮，先问一句，点头才把基色写成 (0,120,215)、重算 9 档
    // 表并复位取色器，全在 `Command::AccentUseDefault` 里。那句 `&& wmenu = 0` 是「没有
    // 别的弹出菜单开着」，端口没有对应全局，不跟。
    if pg.button2(
        10.0,
        408.0,
        72.0,
        tr("Use default", "使用默认值"),
        false,
        true,
    ) {
        state.modals = ModalQueue::question(
            tr("Confirm", "确定"),
            tr("Are you sure?", "你确定吗？"),
            Command::AccentUseDefault,
        );
    }
    // :140 取消：回首选项，不改基色。
    if pg.button2(104.0, 408.0, 72.0, tr("Cancel", "取消"), false, true) {
        state.window = Some(WindowKind::Preferences);
    }
    // :141-153 确定（主按钮）：把正在挑的色写成 accent1/2/3、重算 9 档表，回首选项。
    // 原版那句 `&& (windowopen = 1)`：GML 表达式里的 `=` 是比较，这条是「开窗动画放完
    // 了才认这次点击」（端口没有开窗动画，等价于恒真），不跟着复刻。外面那个 catch 只在
    // rr/gg/bb 不是数字时才弹「请输入一个有效的数字！」，端口的 rr/gg/bb 恒是整数，
    // 那条走不到。原版也只改运行时基色，落盘要等首选项自己按「确定」。
    if pg.button2(198.0, 408.0, 72.0, tr("OK", "确定"), false, false) {
        state.theme.set_accent_base(pick.rgb);
        state.window = Some(WindowKind::Preferences);
    }
    // `draw_windows.gml:100-101`：这扇的 Escape 不是关窗，是退回首选项。
    if pg.ui.input(|i| i.key_released(egui::Key::Escape)) {
        state.window = Some(WindowKind::Preferences);
    }
    state.accent_pick = Some(pick);
}

/// 原版 `draw_window_update.gml`，`w_changelist` 与 `w_update` 共用一份绘制：
/// 正文是随程序的 `changelog.txt` 逐行列出，`w_update` 多一句谢词，「确认」的
/// 去处也不同（前者回欢迎窗并 `save_settings()`，后者直接关窗）。
fn changelog_window(state: &mut EditorState, ui: &mut Ui) {
    let is_update = state.window == Some(WindowKind::Update);
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    // 原版 `theme_offset = (theme == 3 ? 0 : 20)`，右缘的框线、滚动条、确认按钮跟着右移。
    let off = if fluent { 0.0 } else { 20.0 };
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(500.0 + off - ml - mr, if fluent { 392.0 } else { 374.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    // :7-16：`changelogstr = 0` 就 `message()` 一句然后收窗，`w_update` 退回欢迎窗。
    let Some(src) = state.changelog.clone() else {
        state.modals = ModalQueue::message(
            "Note Block Studio",
            tr("Changelist not found!", "找不到更新历史！"),
        );
        state.window = if is_update {
            Some(WindowKind::Greeting)
        } else {
            None
        };
        return;
    };
    let lines = changelog_lines(&src);
    let total = lines.len();
    let mut pg = Page {
        ui,
        o,
        ry,
        th: &th,
        fluent,
    };
    // :52-59：只有 `w_update` 有这句，坐标就是源文的 (x1 + 32, y1 + 32)。
    if is_update {
        let thanks = if crate::lang() == 1 {
            format!("感谢你更新到 {}版本！", crate::VERSION)
        } else {
            format!("Thank you for upgrading to version {}!", crate::VERSION)
        };
        pg.text(pg.row(32.0, 32.0), &thanks, PREFS_FONT, pg.ink());
    }
    // 「观看更新视频」原版画在 `y1 + 8` 那一横带里，也就是 GameMaker 自绘的标题带上；
    // 端口这扇用的是 egui 标题栏，这一条只能落在内容区顶上，横坐标照抄。
    if pg.button2(
        500.0 - 120.0 - 8.0,
        8.0,
        120.0,
        tr("Watch release video", "观看更新视频"),
        false,
        true,
    ) {
        // :60-64：两种语言连站点都不一样。
        state.emit(Command::OpenUrl(if crate::lang() == 1 {
            "https://www.bilibili.com/video/BV1qWq9Y2ESX/".to_string()
        } else {
            "https://youtu.be/gya4NAQunEc".to_string()
        }));
    }
    // :68 的正文框 `draw_area(x1 + 16, y1 + 58, x1 + 487 + theme_offset, y1 + 357)`。
    pg.frame_box(16.0, 58.0, 487.0 + off, 357.0);
    // 一页 18 行（:76 `a < sb_val[update_scrollbar] + 18`），可滚动条是按 22 槽给的
    // （:84 第 5 个参数是 22）——这个不一致是原版自己的，照抄。
    let start = state.update_scroll.floor().max(0.0) as usize;
    let p = pg.ui.painter().clone();
    let ink = pg.ink();
    for a in start..(start + 18).min(total) {
        let (line, bold) = &lines[a];
        greet_text_left(
            &p,
            pg.row(32.0, 64.0 + 16.0 * (a - start) as f32),
            line,
            PREFS_FONT,
            ink,
            *bold,
        );
    }
    list_scrollbar(
        &mut pg,
        "upd",
        470.0 + off,
        60.0,
        12.0,
        22.0,
        total,
        &mut state.update_scroll,
    );
    // :85-87「查看更旧版本……」开 `link_changelog`。
    if pg.button2(
        16.0,
        365.0,
        96.0,
        tr("Older versions...", "查看更旧版本…"),
        false,
        true,
    ) {
        state.emit(Command::OpenUrl(crate::LINK_CHANGELOG.to_string()));
    }
    // :88-95：`w_update` 的确认是「回欢迎窗 + 把新版本号写回设置」，
    // 帮助菜单进来的那版只是关窗。原版那句 `&& (windowopen = 1 || theme != 3)` 里的
    // `=` 是比较（Fluent 要等开窗动画放完才认点击），端口没有开窗动画，等价于恒真，
    // 所以不跟着复刻。
    if pg.button2(
        487.0 - 72.0 + off,
        365.0,
        72.0,
        tr("OK", "确认"),
        false,
        false,
    ) {
        if is_update {
            state.prefs.last_version = Some(crate::VERSION.to_string());
            state.save_prefs();
            state.window = Some(WindowKind::Greeting);
        } else {
            state.emit(Command::CloseWindow);
        }
    }
}

/// 原版 :69-75 的分行：一次取到下一个 `\n` 之前，行首字符是 `C` 的那行走粗体
/// （`if (string_char_at(str[a], 1) = "C") strb[a] = 1`）。末尾没有换行符的那段
/// 原版按 `string_count("\n")` 计数，压根不会进表，这里同样丢掉。
pub(crate) fn changelog_lines(src: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find('\n') {
        let line = &rest[..i];
        out.push((line.to_string(), line.starts_with('C')));
        rest = &rest[i + 1..];
    }
    out
}

/// 原版 `draw_scrollbar(id, x, y, slotwh, slots, 行数, 0, 1)` 的竖直分支：
/// 轨道高 `slots * slotwh`，滑块 `max(10, slots / 行数 * 轨道)`，
/// 出界一律 `median(0, 值, 行数 - slots)`。
/// `update_scrollbar` 给的是槽高 12、22 槽（`draw_window_update.gml:84`），
/// 关于窗谢词栏是 16、18（`draw_window_about.gml:78`）——一屏可见几行是各窗口
/// 自己的循环上限，跟这里的槽数不一定相等（两处都是原版自己的不一致）。
///
/// 算式跟 `midi_scrollbar` 是同一条，但没去复用：那条把 MIDI 窗自己的 17/9 两个常数和
/// 17 像素的画宽写死在函数里，改它要把已经逐项核对过的 MIDI 窗口再动一遍。
fn list_scrollbar(
    pg: &mut Page,
    id: &str,
    x: f32,
    n: f32,
    slotwh: f32,
    slots: f32,
    total: usize,
    scroll: &mut f32,
) {
    const DRAW_W: f32 = 16.0;
    let track = slotwh * slots;
    let max_scroll = (total as f32 - slots).max(0.0);
    *scroll = scroll.clamp(0.0, max_scroll);
    let locked = total as f32 <= slots;
    let bwh = if locked {
        0.0
    } else {
        (slots / total as f32 * track).max(10.0)
    };
    let bpos = if locked {
        0.0
    } else {
        (*scroll / total as f32 * track).floor()
    };
    let p = pg.ui.painter().clone();
    p.rect_filled(
        Rect::from_min_size(pg.row(x, n), Vec2::new(DRAW_W, track + 32.0)),
        3.0,
        pg.th.panel,
    );
    if !locked {
        let thumb = Rect::from_min_size(
            pg.row(x, n + 20.0 + bpos),
            Vec2::new(DRAW_W, bwh.floor() - 6.0),
        );
        p.rect_filled(thumb, 3.0, pg.th.accent_soft);
        p.rect_stroke(
            thumb,
            3.0,
            Stroke::new(1.0_f32, pg.th.accent),
            StrokeKind::Inside,
        );
    }
    // 上下箭头：原版是 `spr_scrollbar_v_button_up/down`，精灵图没导出，画同位置的三角形。
    let ink = pg.ink();
    for (down, dy) in [(false, 0.0), (true, track + 16.0)] {
        let rect = Rect::from_min_size(pg.row(x, n + dy), Vec2::new(DRAW_W, 16.0));
        let (resp, _) = pg.hit(rect, Id::new((id, "sb-btn", dy as i32)), locked);
        let c = rect.center();
        let pts = if down {
            vec![
                c + Vec2::new(-3.5, -1.5),
                c + Vec2::new(3.5, -1.5),
                c + Vec2::new(0.0, 3.0),
            ]
        } else {
            vec![
                c + Vec2::new(-3.5, 1.5),
                c + Vec2::new(3.5, 1.5),
                c + Vec2::new(0.0, -3.0),
            ]
        };
        p.add(egui::Shape::convex_polygon(pts, ink, Stroke::NONE));
        if resp.clicked() {
            *scroll = (*scroll + if down { 1.0 } else { -1.0 }).clamp(0.0, max_scroll);
        }
    }
    // 槽底（滑块上/下那两段）按整页翻，一页是 `slots` 槽。
    for above in [true, false] {
        if locked {
            break;
        }
        let y = if above {
            n + 16.0
        } else {
            n + 16.0 + bpos + bwh
        };
        let h = if above { bpos } else { track - (bpos + bwh) };
        if h <= 0.0 {
            continue;
        }
        let rect = Rect::from_min_size(pg.row(x, y), Vec2::new(DRAW_W, h));
        let (resp, _) = pg.hit(rect, Id::new((id, "sb-track", above)), false);
        if resp.clicked() {
            *scroll = (*scroll + if above { -slots } else { slots }).clamp(0.0, max_scroll);
        }
    }
    // 拖滑块：原版 `sb_val += dy / (mwh / ms)`，松手取整。
    if !locked {
        let grip = Rect::from_min_size(pg.row(x, n + 16.0 + bpos), Vec2::new(DRAW_W, bwh));
        let resp = pg
            .ui
            .interact(grip, Id::new((id, "sb-thumb")), Sense::drag());
        if resp.dragged() {
            *scroll =
                (*scroll + resp.drag_delta().y / (track / total as f32)).clamp(0.0, max_scroll);
        } else if resp.drag_stopped() {
            *scroll = scroll.round().clamp(0.0, max_scroll);
        }
    }
    // 滚轮：原版要 `sb_sel = i`（点过这条）才吃滚轮，这里跟 MIDI 窗一样按悬停就算。
    let zone = Rect::from_min_size(pg.row(x, n), Vec2::new(DRAW_W, track + 32.0));
    let hover = pg
        .ui
        .interact(zone, Id::new((id, "sb-wheel")), Sense::hover());
    if hover.hovered() {
        let d = pg.ui.input(|i| i.raw_scroll_delta.y);
        if d != 0.0 {
            *scroll = (*scroll - d).clamp(0.0, max_scroll);
        }
    }
}

fn clip_editor(state: &mut EditorState, ui: &mut Ui) {
    ui.label(
        RichText::new(tr(
            "OpenNBS-compatible selection RLE text, interchangeable with the original clipboard",
            "OpenNBS 兼容的选区 RLE 文本，可与原版剪贴板互通",
        ))
        .small()
        .weak(),
    );
    let mut text = state.doc.clipboard_text.clone();
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .desired_width(f32::INFINITY)
            .desired_rows(6)
            .code_editor(),
    );
    ui.horizontal(|ui| {
        if ui
            .button(tr("Generate from current selection", "从当前选区生成"))
            .clicked()
        {
            state.doc.copy_selection();
        }
        if ui.button(tr("Load and paste", "载入并粘贴")).clicked() {
            state.emit(Command::PasteText(text.clone()));
        }
        if ui.button(tr("Close", "关闭")).clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn shortcuts_help(ui: &mut Ui) {
    ui.label(
        RichText::new(tr(
            "Shortcuts match the original OpenNBS, plus audio and export combinations",
            "快捷键对齐原版 OpenNBS，另增音频与导出相关组合键",
        ))
        .small()
        .weak(),
    );
    egui::ScrollArea::vertical()
        .max_height(420.0)
        .show(ui, |ui| {
            egui::Grid::new("shortcut_grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (k, v) in crate::shortcuts::shortcut_help() {
                        ui.label(RichText::new(k).monospace().strong());
                        ui.label(v);
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
        RichText::new(match action {
            // 原版这两个框的过滤器是空字符串，选的是目录不是文件。
            crate::state::DialogAction::PickFolder(f) => f.tip().to_string(),
            crate::state::DialogAction::PickMinecraftDir => tr(
                "Select Minecraft installation directory",
                "选择Minecraft安装目录",
            )
            .to_string(),
            _ => format!(
                "{}: *.{}   {}",
                tr("File type", "文件类型"),
                ext,
                if action.is_save() {
                    tr("(Save / Export)", "（保存 / 导出）")
                } else {
                    tr("(Open / Import)", "（打开 / 导入）")
                }
            ),
        })
        .small()
        .weak(),
    );
    if let Some(d) = state.file_dialog.as_mut() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(d.dir.to_string_lossy()).small().weak());
            if ui.small_button(tr("Up", "上级")).clicked() {
                if let Some(parent) = d.dir.parent() {
                    d.dir = parent.to_path_buf();
                    d.refresh();
                }
            }
        });
        let pick_folder = matches!(
            action,
            crate::state::DialogAction::PickFolder(_)
                | crate::state::DialogAction::PickMinecraftDir
        );
        if !pick_folder {
            ui.horizontal(|ui| {
                ui.label(tr("File name", "文件名"));
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
            });
        }
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                let entries = d.entries.clone();
                for entry in entries {
                    let name = entry
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let is_dir = entry.is_dir();
                    let label = if is_dir {
                        format!("[{}] {}", tr("dir", "目录"), name)
                    } else {
                        name.clone()
                    };
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
            if ui
                .button(if pick_folder {
                    tr("Select", "选择")
                } else if save {
                    tr("Save", "保存")
                } else {
                    tr("Open", "打开")
                })
                .clicked()
            {
                // 原版「修改」拿的是 `filename_dir(a)`，即对话框当前显示的目录本身。
                let path = if pick_folder {
                    d.dir.clone()
                } else {
                    d.selected_path()
                };
                let path = if path.extension().is_none() && !pick_folder {
                    path.with_extension(ext)
                } else {
                    path
                };
                cmd = Some(Command::DialogConfirm(path));
            }
            if ui.button(tr("Cancel", "取消")).clicked() {
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
        let name = state
            .doc
            .song
            .layers
            .get(layer as usize)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        state.editing_layer_name = Some((layer, name));
    }
    let mut cmd: Option<Command> = None;
    if let Some((_, name)) = state.editing_layer_name.as_mut() {
        ui.label(tr("Layer name", "图层名称"));
        ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button(tr("OK", "确定")).clicked() {
                cmd = Some(Command::RenameLayer(layer, name.clone()));
            }
            if ui.button(tr("Cancel", "取消")).clicked() {
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

/// 原版 `draw_window_save_options`：140×162，七行版本单选 + 一个确定。
/// 每行的悬停词就是该版本相对上一版新增了什么；锁住的行表示曲子里的自定义
/// 乐器数超过那个版本的上限（原版 `min_version` / `v6_instrument_limit`）。
fn save_options_window(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 138.0 } else { 136.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let user_instruments = state.doc.song.custom_instruments.len();
    let opt = nbs_core::nbs::save_version_options(state.save_options_version, user_instruments);
    state.save_options_version = opt.selected;
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 132.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        // (版本, 标签, 悬停词, 原版 `y1 +` 后面的行号)，中英逐字照抄。
        let rows: [(u8, &str, &str, f32); 7] = [
            (
                6,
                "v6",
                tr(
                    "Added the new instruments introduced in Minecraft 26",
                    "增加在 Minecraft 26 中推出的新音色",
                ),
                23.0,
            ),
            (
                5,
                "v5",
                tr(
                    "Increases custom instrument limit\nAllows custom sounds in subfolders",
                    "增加自定义音色限制、允许自定义文件在子目录",
                ),
                38.0,
            ),
            (
                4,
                "v4",
                tr(
                    "Includes note velocity/pan/pitch and looping",
                    "包含音符音量、声道、音高和循环",
                ),
                53.0,
            ),
            (3, "v3", tr("Includes song length", "包含歌曲长度"), 68.0),
            (
                2,
                "v2",
                tr("Includes layer panning", "包含层声道数据"),
                83.0,
            ),
            (
                1,
                "v1",
                tr("Includes custom instrument index", "包含自定义乐器序号"),
                98.0,
            ),
            (
                0,
                tr("Classic", "经典"),
                tr(
                    "Doesn't have any of the above, but works on all versions.",
                    "上面的都没有，但所有版本都兼容。",
                ),
                113.0,
            ),
        ];
        for (v, label, tip, n) in rows {
            // 原版 v6 的条件是 `min_version > 6 || 乐器数超上限`，而 min_version
            // 只可能取 0 / 5，前半永远不成立，所以这里只问上限。
            let lock = match v {
                6 => user_instruments > opt.v6_limit,
                _ => opt.floor > v,
            };
            if pg.radio(15.0, n, opt.selected == v, label, tip, lock) {
                state.save_options_version = v;
            }
        }
        if pg.button2(40.0, 135.0, 60.0, tr("OK", "确定"), false, false) {
            cmd = Some(Command::ApplySaveOptions {
                version: state.save_options_version,
            });
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版三个带参数宏窗口开头都有 `if (songs[song].selected == 0) { window = 0; return 0 }`：
/// 没有选区就不给用，选区在窗口开着的时候没了窗口自己关掉，绝不退化成「对全曲生效」。
/// 返回 `false` 表示已经安排上关窗，调用方直接 return。
fn macro_gate(state: &mut EditorState) -> bool {
    if state.doc.selection.is_empty() {
        state.emit(Command::CloseWindow);
        return false;
    }
    true
}

/// 原版 `draw_window_macro_arpeggio`：140×130，一个规律框 + 确定/取消。
/// 规律只认数字和 `|`，所以原版确定时只校验「有没有 `|`」，没有就弹错误并留着窗口。
fn arpeggio_window(state: &mut EditorState, ui: &mut Ui) {
    if !macro_gate(state) {
        return;
    }
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 106.0 } else { 104.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    // 原版 OK 收尾 `if(!keyboard_check(vk_alt)) selection_place(false)`：
    // 按着 Alt 确定就只改浮动选区，不落回歌曲。
    let place = !ui.input(|i| i.modifiers.alt);
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 92.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        pg.header(10.0, 43.0, 120.0, 35.0, tr("Pattern", "规律"));
        pg.textarea(
            15.0,
            50.0,
            113.0,
            25.0,
            &mut state.arpeggio_pattern,
            tr(
                "Must separate relative keys with pipes.",
                "必须用“|”分隔相联系的音。",
            ),
            "macro-arpeggio",
        );
        if pg.button2(10.0, 98.0, 60.0, tr("OK", "确定"), false, false) {
            if state.arpeggio_pattern.contains('|') {
                cmd = Some(Command::MacroWindowOk {
                    m: Macro::Arpeggio {
                        pattern: nbs_core::parse_pattern(&state.arpeggio_pattern),
                    },
                    place,
                });
            } else {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    tr(
                        "Please add pipes ( | ) to separate values!",
                        "请使用“|”分隔值！",
                    ),
                );
            }
        }
        if pg.button2(70.0, 98.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版 `draw_window_macro_portamento`：140×130，反转勾选 + cents 拖拽。
/// 两个参数都是原版会话全局（`porta_reverse` / `port_cent`），不落 settings.ini。
fn portamento_window(state: &mut EditorState, ui: &mut Ui) {
    if !macro_gate(state) {
        return;
    }
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 106.0 } else { 104.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let place = !ui.input(|i| i.modifiers.alt);
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 92.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        if pg.check(
            10.0,
            30.0,
            state.porta_reverse,
            tr("Reversed", "反转"),
            tr(
                "Portamento is done in the inverse direction.",
                "反方向应用 Portamento。",
            ),
            false,
            false,
        ) {
            state.porta_reverse = !state.porta_reverse;
        }
        pg.header(10.0, 53.0, 120.0, 35.0, tr("Cent shift", "偏移"));
        // 原版 `median(-1200, draw_dragvalue(11, x1 + 55, y1 + 65, port_cent, 0.1), 1200)`。
        if let Some(v) = pg.drag(
            55.0,
            65.0,
            state.port_cent,
            0.1,
            -1200,
            1200,
            false,
            "",
            &mut state.prefs_drag,
        ) {
            state.port_cent = v;
        }
        if pg.button2(10.0, 98.0, 60.0, tr("OK", "确定"), false, false) {
            cmd = Some(Command::MacroWindowOk {
                m: Macro::Portamento {
                    cents: state.port_cent,
                    reverse: state.porta_reverse,
                },
                place,
            });
        }
        if pg.button2(70.0, 98.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版 `draw_window_macro_stagger`：150×160，规律框加一句警示。
/// 确定时原版校验两条：没有 `|` 报错；出现 `-` 报「只能向下延伸」
/// （就算放行，`string_digits_symbol` 也会把负号吃掉变成正数，所以那条检查是唯一的拦法）。
fn stagger_window(state: &mut EditorState, ui: &mut Ui) {
    if !macro_gate(state) {
        return;
    }
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(150.0 - ml - mr, if fluent { 136.0 } else { 134.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let place = !ui.input(|i| i.modifiers.alt);
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(11.0, 26.0, 154.0, 102.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        pg.header(15.0, 53.0, 120.0, 35.0, tr("Pattern", "规律"));
        pg.textarea(
            20.0,
            60.0,
            113.0,
            25.0,
            &mut state.stagger_pattern,
            tr(
                "Must separate relative keys with pipes.",
                "必须用“|”分割值。",
            ),
            "macro-stagger",
        );
        // 原版 `draw_text_dynamic` 的 `\n` 是真断行；`Page::text` 走不分行的排版，
        // 所以按行各画一次，行距取第一行排版出来的人字高。
        let warn = tr(
            "This may replace \nlower note blocks!",
            "这可能会替换低的音符！",
        );
        let ink = pg.ink();
        let p = pg.ui.painter().clone();
        let mut line_n = 92.0;
        for part in warn.split('\n') {
            let g = p.layout_no_wrap(part.to_string(), FontId::proportional(PREFS_FONT), ink);
            let h = g.size().y;
            pg.text(pg.row(25.0, line_n), part, PREFS_FONT, ink);
            line_n += h;
        }
        if pg.button2(15.0, 128.0, 60.0, tr("OK", "确定"), false, false) {
            if !state.stagger_pattern.contains('|') {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    tr(
                        "Please add pipes ( | ) to separate values!",
                        "请使用“|”分隔值！",
                    ),
                );
            } else if state.stagger_pattern.contains('-') {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    tr(
                        "Stagger can only extend downwards!",
                        "Stagger 只能向下延伸！",
                    ),
                );
            } else {
                cmd = Some(Command::MacroWindowOk {
                    m: Macro::Stagger {
                        pattern: nbs_core::parse_pattern(&state.stagger_pattern)
                            .into_iter()
                            .map(|d| d as u32)
                            .collect(),
                    },
                    place,
                });
            }
        }
        if pg.button2(75.0, 128.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版 `draw_window_macro_stereo`：140×130，一个「反转」勾选 + 立体程度拖拽。
/// 确定时按列交替写声道（第一列偏右、第二列偏左，勾选后对调），
/// 并且照例 Alt 确定不落位。
fn stereo_window(state: &mut EditorState, ui: &mut Ui) {
    if !macro_gate(state) {
        return;
    }
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 106.0 } else { 104.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let place = !ui.input(|i| i.modifiers.alt);
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 92.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        if pg.check(
            10.0,
            30.0,
            state.stereo_reverse,
            tr("Reversed", "反转"),
            tr(
                "Stereo is done in the inverse direction.",
                "反方向应用立体效果。",
            ),
            false,
            false,
        ) {
            state.stereo_reverse = !state.stereo_reverse;
        }
        pg.header(10.0, 53.0, 120.0, 35.0, tr("Stereo width", "立体程度"));
        // 原版 `median(0, draw_dragvalue(11, x1 + 55, y1 + 65, stereo_width, 0.5), 100)`。
        // 那个 0.5 是原版的浮点步长，端口沿用 `Page::drag` 的整数走查。
        if let Some(v) = pg.drag(
            55.0,
            65.0,
            state.stereo_width,
            0.5,
            0,
            100,
            false,
            "",
            &mut state.prefs_drag,
        ) {
            state.stereo_width = v;
        }
        if pg.button2(10.0, 98.0, 60.0, tr("OK", "确定"), false, false) {
            cmd = Some(Command::MacroWindowOk {
                m: Macro::Stereo {
                    width: state.stereo_width.clamp(0, 100) as u8,
                    reverse: state.stereo_reverse,
                },
                place,
            });
        }
        if pg.button2(70.0, 98.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版三扇批量属性窗口里那一格拖拽的值分别写到哪个会话字段上。
enum BatchAttr {
    Velocity,
    Panning,
    Pitch,
}

/// 原版 `draw_window_macro_setvelocity` / `_setpanning` / `_setpitch` 是同一副壳：
/// 140×130，白底框 `(6,26)-(134,92)`，`draw_areaheader(10, 53, 120, 35)` 里放一个
/// 拖拽值 `(55,65)`（步长都是 0.1），确定/取消在 `(10,98)` / `(70,98)`。
/// 三者只差标题文案、拖拽的钳位上下限，以及 `(15,80)` 那一格有没有勾选框：
/// 音量配「百分比」（钳位上限跟着变成 `100 + 900`，所以原版这个值最大到 1000）、
/// 音高配「加法模式」、声道什么都不配。
/// 确定一律走 `MacroWindowOk`：Alt 按着就只改浮动选区，不落回歌曲。
fn batch_attr_window(state: &mut EditorState, ui: &mut Ui, which: BatchAttr) {
    if !macro_gate(state) {
        return;
    }
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 106.0 } else { 104.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let place = !ui.input(|i| i.modifiers.alt);
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 92.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        let (header, (min, max), val) = match which {
            BatchAttr::Velocity => (
                tr("Velocity", "音量"),
                (0, 100 + 900 * i32::from(state.vel_percent)),
                state.set_vel,
            ),
            BatchAttr::Panning => (tr("Panning", "声道"), (-100, 100), state.set_pan),
            BatchAttr::Pitch => (tr("Pitch", "音高"), (-1200, 1200), state.set_pit),
        };
        pg.header(10.0, 53.0, 120.0, 35.0, header);
        if let Some(v) = pg.drag(
            55.0,
            65.0,
            val,
            0.1,
            min,
            max,
            false,
            "",
            &mut state.prefs_drag,
        ) {
            match which {
                BatchAttr::Velocity => state.set_vel = v,
                BatchAttr::Panning => state.set_pan = v,
                BatchAttr::Pitch => state.set_pit = v,
            }
        }
        match which {
            BatchAttr::Velocity => {
                if pg.check(
                    15.0,
                    80.0,
                    state.vel_percent,
                    tr("Percent", "百分比"),
                    tr(
                        "Apply the velocity as a percentage\nof each note's current velocity.",
                        "按百分比应用至每个音符。",
                    ),
                    false,
                    false,
                ) {
                    state.vel_percent = !state.vel_percent;
                }
            }
            BatchAttr::Pitch => {
                if pg.check(
                    15.0,
                    80.0,
                    state.pitch_additive,
                    tr("Add mode", "加法模式"),
                    tr(
                        "Add the pitch amount to each note,\ninstead of setting it.",
                        "将数值叠加至音符上，而不是直接设置。",
                    ),
                    false,
                    false,
                ) {
                    state.pitch_additive = !state.pitch_additive;
                }
            }
            BatchAttr::Panning => {}
        }
        // 原版设定声道那扇窗的确定按钮没走 `condstr`：`draw_button2(..., "OK")`，
        // 中文界面也显示 "OK"（`draw_window_macro_setpanning:31`）。如实照抄；
        // 同一扇窗的取消按钮反倒是翻译过的。
        let ok = match which {
            BatchAttr::Panning => "OK",
            _ => tr("OK", "确定"),
        };
        if pg.button2(10.0, 98.0, 60.0, ok, false, false) {
            let m = match which {
                BatchAttr::Velocity => Macro::SetVelocity {
                    value: state.set_vel,
                    percent: state.vel_percent,
                },
                BatchAttr::Panning => Macro::SetPanning {
                    value: state.set_pan,
                },
                BatchAttr::Pitch => Macro::SetPitch {
                    value: state.set_pit,
                    additive: state.pitch_additive,
                },
            };
            cmd = Some(Command::MacroWindowOk { m, place });
        }
        if pg.button2(70.0, 98.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版 `draw_window_macro_tremolo`：150×240，三枚淡入类型单选 + 两枚自动/手动单选
/// + 触发/延长、延长时间、音符间隔三行拖拽。
/// 五枚参数都是原版会话全局（`control_create.gml:653-657`：`tremolotype = 0`、
/// `fade_auto = 1`、`leg_dec = 20`、`leg_sus = 20`、`trem_spacing = 1`），不落 settings.ini。
/// 两处原版只改颜色的地方照搬不升级：`tremolotype = 0` 时下面两枚单选的标签被涂成
/// c_gray，但 `draw_radiobox` 画字前自己 `draw_theme_color()`，Fluent 下这层灰根本盖不上；
/// 自动音量时「触发/延长:」「延长时间:」两行标签涂灰，数字仍是主题色、照样能拖——
/// 原版没锁控件，这里也不锁。
fn tremolo_window(state: &mut EditorState, ui: &mut Ui) {
    if !macro_gate(state) {
        return;
    }
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(150.0 - ml - mr, if fluent { 216.0 } else { 214.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let place = !ui.input(|i| i.modifiers.alt);
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 208.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        let kinds = [
            (
                0,
                tr("No fading", "无音量变化"),
                tr("Just repeats notes.", "只放置音符。"),
            ),
            (
                1,
                tr("Fade in", "淡入"),
                tr("Fades in until note changes.", "音符改变之前淡入。"),
            ),
            (
                2,
                tr("Fade out", "淡出"),
                tr("Fades out until note changes.", "音符改变之前淡出。"),
            ),
        ];
        for (i, (kind, label, tip)) in kinds.iter().enumerate() {
            if pg.radio(
                15.0,
                43.0 + 15.0 * i as f32,
                state.tremolotype == *kind,
                label,
                tip,
                false,
            ) {
                state.tremolotype = *kind;
            }
        }
        let modes = [
            (
                true,
                tr("Automatic fade", "自动音量"),
                tr(
                    "Fade is calculated relative to tick duration of note.",
                    "音量由音符长度决定。",
                ),
            ),
            (
                false,
                tr("Manual fade", "手动音量"),
                tr(
                    "Specify decay and sustain of fade.",
                    "手动指定音符的触发和延长。",
                ),
            ),
        ];
        for (i, (mode, label, tip)) in modes.iter().enumerate() {
            if pg.radio(
                15.0,
                88.0 + 15.0 * i as f32,
                state.fade_auto == *mode,
                label,
                tip,
                false,
            ) {
                state.fade_auto = *mode;
            }
        }
        let dim = if state.fade_auto {
            ggray(128)
        } else {
            pg.ink()
        };
        let attack = tr("Attack/Decay:", "触发/延长:");
        pg.text(pg.row(15.0, 123.0), &attack, PREFS_FONT, dim);
        if let Some(v) = pg.drag(
            15.0,
            138.0,
            state.leg_dec,
            0.5,
            1,
            100,
            false,
            "",
            &mut state.prefs_drag,
        ) {
            state.leg_dec = v;
        }
        let sustain = tr("Sustain:", "延长时间:");
        pg.text(pg.row(15.0, 156.0), &sustain, PREFS_FONT, dim);
        if let Some(v) = pg.drag(
            15.0,
            170.0,
            state.leg_sus,
            0.5,
            1,
            100,
            false,
            "",
            &mut state.prefs_drag,
        ) {
            state.leg_sus = v;
        }
        let spacing = tr("Spacing:", "音符间隔:");
        pg.text(pg.row(15.0, 188.0), &spacing, PREFS_FONT, pg.ink());
        let spacing_tip = tr(
            "The amount of ticks between repetitions.",
            "重复音符中间的间隔。",
        );
        pg.tip_area(40.0, 183.0, 180.0, 21.0, &spacing_tip);
        // 原版 `median(1, draw_dragvalue(3, x1 + 80, y1 + 188, trem_spacing, 1), 2)`：
        // 间隔只可能是 1 或 2。
        if let Some(v) = pg.drag(
            80.0,
            188.0,
            state.trem_spacing,
            1.0,
            1,
            2,
            false,
            "",
            &mut state.prefs_drag,
        ) {
            state.trem_spacing = v;
        }
        if pg.button2(10.0, 215.0, 60.0, tr("OK", "确定"), false, false) {
            // 原版只校验这两格（拖拽本身钳在 1..100，这条分支实际进不去），报错后
            // 留着窗口不提交。
            if !(0..=100).contains(&state.leg_dec) || !(0..=100).contains(&state.leg_sus) {
                state.modals = ModalQueue::message(
                    tr("Error", "错误"),
                    tr(
                        "Values must be between 1 and 100.",
                        "所有值必须在 1 到 100 之间。",
                    ),
                );
            } else {
                cmd = Some(Command::MacroWindowOk {
                    m: Macro::Tremolo {
                        spacing: state.trem_spacing,
                        fade: match state.tremolotype {
                            0 => TremoloFade::None,
                            1 => TremoloFade::In,
                            _ => TremoloFade::Out,
                        },
                        auto: state.fade_auto,
                        dec: state.leg_dec,
                        sus: state.leg_sus,
                    },
                    place,
                });
            }
        }
        if pg.button2(80.0, 215.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

/// 原版 `draw_window_tempo_tapper`：140×130，只有一颗「按 T 打点」和结果数字。
/// 按 T 采间隔 → `taptempo` 取全部间隔的滑动平均；数字只有在打过点且测出值时才画
/// （`if (tapping && taptempo != 0)`），显示单位跟首选项的速度单位走
/// （BPM 直接画 `floor(taptempo + 0.5)`，t/s 画它除以 15）。
/// 确认：`real_tempo = (ctempo * (1 + tapdouble)) / 15`，然后关窗。
/// 原版即使一次没打也照算（`0 / 15 = 0`），端口沿用了 `SetTempo` 的 0.25..=60 夹取。
fn tempo_tapper_window(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 106.0 } else { 104.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    // 原版 `keyboard_check_pressed(ord("T"))`：只在这个窗开着时收点，每点发一声打点音。
    let tapped = ui.input(|i| i.key_pressed(egui::Key::T))
        && state.tapper.register_tap(crate::state::tap_clock_ms());
    if tapped {
        // 原版 `play_sound(soundmetronome, 45, 100, 100, 0)`：100 力度、居中、无微调。
        state.audio.preview_metronome(45);
    }
    let mut apply: Option<Command> = None;
    let mut close = false;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 92.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        // 原版组名跟着速度单位走：`if (use_bpm) "BPM (T to tap)" else "t/s (T to tap)"`。
        let header = if state.prefs.use_bpm {
            tr("BPM (T to tap)", "BPM (按 T 打速度)").to_string()
        } else {
            tr("t/s (T to tap)", "t/s (按 T 打速度)").to_string()
        };
        pg.header(10.0, 43.0, 120.0, 35.0, &header);
        if state.tapper.tapping && state.tapper.bpm != 0.0 {
            let ctempo = state.tapper.measured();
            let shown = if state.prefs.use_bpm {
                ctempo
            } else {
                ctempo / 15.0
            };
            let text = gml_number(shown);
            pg.text(pg.row(60.0, 55.0), &text, PREFS_FONT, pg.ink());
        }
        // 双倍速度：原版把它存进 settings.ini（`save_settings.gml:71`），
        // 关窗不会跟着清零，所以直接读写 prefs。
        if pg.check(
            15.0,
            80.0,
            state.prefs.tapdouble,
            tr("Double tempo", "双倍速度"),
            tr("Double the tempo to apply.", "应用时使速度 x2。"),
            true,
            false,
        ) {
            state.prefs.tapdouble = !state.prefs.tapdouble;
        }
        if pg.button2(10.0, 98.0, 60.0, tr("OK", "确认"), false, false) {
            let doubled = state.tapper.measured() * (1.0 + state.prefs.tapdouble as i32 as f64);
            apply = Some(Command::SetTempo((doubled / 15.0) as f32));
            close = true;
        }
        if pg.button2(70.0, 98.0, 60.0, tr("Cancel", "取消"), false, true) {
            close = true;
        }
    }
    if let Some(c) = apply {
        state.emit(c);
    }
    if close {
        state.emit(Command::CloseWindow);
    }
}

/// 原版 `string()`：整数值不带小数点，其余最多 6 位有效数字并去掉尾零。
fn gml_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.5}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// 原版 `draw_window_edit_tempo_changer`：140×130 小窗，落速度变换 / 点已有速度变换
/// 时由落格逻辑打开。主题 0 才铺那块白面板（`if (theme = 0)`），OK 校验
/// `int64 > 0 && < 32768`，非法弹「非法数值！」且窗口留着。
fn event_tempo_window(state: &mut EditorState, ui: &mut Ui, tick: u32, layer: u32) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 106.0 } else { 104.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 92.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        pg.header(10.0, 40.0, 120.0, 35.0, tr("Tempo (BPM)", "速度（BPM）"));
        pg.textarea(
            15.0,
            50.0,
            113.0,
            25.0,
            &mut state.event_edit.tempo,
            tr("Must be an integer larger than zero.", "必须是一个正整数。"),
            "ev-tempo",
        );
        if pg.button2(10.0, 98.0, 60.0, tr("OK", "确定"), false, false) {
            cmd = match state.event_edit.tempo.parse::<i64>() {
                Ok(bpm) if bpm > 0 && bpm < 32768 => {
                    Some(Command::SetEventTempo { tick, layer, bpm })
                }
                _ => {
                    state.modals = ModalQueue::message(
                        tr("Set Tempo", "设置速度"),
                        tr("Invalid value!", "非法数值！"),
                    );
                    None
                }
            };
        }
        if pg.button2(70.0, 98.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
        state.emit(Command::CloseWindow);
    }
}

/// 原版 `draw_window_edit_sound_stopper`：140×180，两个层号输入行。
/// OK 校验 `start/until 都在 0..32768`（0 合法，虽然提示语说正整数——原版如此）。
fn event_stopper_window(state: &mut EditorState, ui: &mut Ui, tick: u32, layer: u32) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(140.0 - ml - mr, if fluent { 156.0 } else { 154.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let mut cmd = None;
    {
        let mut pg = Page {
            ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        if th.kind == ThemeKind::Aqua {
            let p = pg.ui.painter().clone();
            let rect = pg.row_rect(6.0, 26.0, 134.0, 142.0);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
                StrokeKind::Inside,
            );
        }
        pg.header(
            10.0,
            40.0,
            120.0,
            35.0,
            tr("Start Layer (inclusive)", "起始层（包含）"),
        );
        pg.textarea(
            15.0,
            50.0,
            113.0,
            25.0,
            &mut state.event_edit.start,
            tr("Must be an integer larger than zero.", "必须是一个正整数。"),
            "ev-start",
        );
        pg.header(
            10.0,
            90.0,
            120.0,
            35.0,
            tr("End Layer (inclusive)", "终止层（包含）"),
        );
        pg.textarea(
            15.0,
            100.0,
            113.0,
            25.0,
            &mut state.event_edit.until,
            tr("Must be an integer larger than zero.", "必须是一个正整数。"),
            "ev-until",
        );
        if pg.button2(10.0, 148.0, 60.0, tr("OK", "确定"), false, false) {
            cmd = match (
                state.event_edit.start.parse::<i64>(),
                state.event_edit.until.parse::<i64>(),
            ) {
                (Ok(s), Ok(u)) if (0..32768).contains(&s) && (0..32768).contains(&u) => {
                    Some(Command::SetEventStopper {
                        tick,
                        layer,
                        start: s,
                        until: u as u64,
                    })
                }
                _ => {
                    state.modals = ModalQueue::message(
                        tr("Set Stopping Range", "设置抑制范围"),
                        tr("Invalid value!", "非法数值！"),
                    );
                    None
                }
            };
        }
        if pg.button2(70.0, 148.0, 60.0, tr("Cancel", "取消"), false, true) {
            cmd = Some(Command::CloseWindow);
        }
    }
    if let Some(c) = cmd {
        state.emit(c);
        state.emit(Command::CloseWindow);
    }
}

fn confirm_new(state: &mut EditorState, ui: &mut Ui) {
    ui.label(tr(
        "The current song has unsaved changes. Create a new song anyway?",
        "当前歌曲有未保存的修改，确定新建？",
    ));
    ui.horizontal(|ui| {
        if ui.button(tr("OK", "确定")).clicked() {
            state.emit(Command::ConfirmNew);
        }
        if ui.button(tr("Cancel", "取消")).clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

/// 原版 `draw_window_about.gml`：580×410。左半是 logo + 「Note Block Studio」+ 版本
/// 与延续声明，右半是随程序发的 `credits.txt` 那一栏（行首 `*` 是标题，`|` 后面是链接），
/// 底下一排六个站点按钮加一个「确定」。
fn about(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    // 原版 `draw_window(x1, y1, x1 + 580, y1 + 410)`，这扇不带 `theme_offset`；
    // 高度照 `changelog_window` 那一套减掉标题带。
    let body = ui.allocate_exact_size(
        egui::vec2(580.0 - ml - mr, if fluent { 402.0 } else { 384.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let bank = state.icons.bank();
    let mut pg = Page {
        ui,
        o,
        ry,
        th: &th,
        fluent,
    };
    // :15-17 logo：帧号跟欢迎窗是同一条算式（`window_icon + (window_icon && fdark)`），
    // 只是画在 `x1 + 80` 而不是欢迎窗的 `x1 + 64`。
    if let Some(bk) = &bank {
        let p = pg.ui.painter().clone();
        bk.window_logo(&p, pg.row(80.0, 50.0), state.prefs.window_icon, th.fdark);
    }
    let p = pg.ui.painter().clone();
    // :19-20 程序名（font_info_med_bold），居中于 `x1 + 150`。
    greet_text_center(
        &p,
        pg.row(150.0, 0.0).x,
        pg.row(0.0, 200.0).y,
        "Note Block Studio",
        PREFS_FONT_MED,
        th.text,
        true,
    );
    // :22-28 版本 + 延续声明（font_main）。英文三段正是语言表的
    // `text_window_about_version` / `_released` / `_description`（`lang_en_us.gml:480-482`，
    // 「Version 」的尾空格和声明里两处行尾空格都留着）；中文照源码字面，
    // 「版本」「发布于」后面不带空格。原版 `NOT_RUN_FROM_IDE` 那一支（「Version Local
    // Release」/「本地版本」）只有在 GameMaker IDE 里直接跑才走得到，端口没有这一支。
    let head = format!(
        "{}{}{}{}",
        tr("Version ", "版本"),
        crate::VERSION,
        tr(" - Released ", " - 发布于"),
        crate::VERSION_DATE
    );
    let desc = if crate::lang() == 1 {
        "\n\n本程序是 Minecraft Note Block Studio 的延续版本\n原作者 David Andrei\n\n本产品与 Mojang Studios、\nMicrosoft Studios 以及\n游戏 Minecraft 无任何关联。"
    } else {
        "\n\nThis program is a continuation of\n Minecraft Note Block Studio, \ncreated by David Andrei. \n\nThis product is not affiliated with\nMojang Studios, Microsoft Studios\nor the game Minecraft."
    };
    greet_text_center_lines(
        &p,
        pg.row(150.0, 0.0).x,
        pg.row(0.0, 217.0).y,
        &(head + desc),
        PREFS_FONT,
        th.text,
    );
    // :35-79 谢词栏。原版读不到 `credits.txt` 时 `string_count` 当场报错，端口按
    // 「一栏都不画」处理：框照旧、滚动条因为不满 18 槽锁死。
    let lines = state
        .credits
        .as_deref()
        .map(credits_lines)
        .unwrap_or_default();
    let total = lines.len();
    pg.frame_box(295.0, 35.0, 555.0, 357.0);
    // 一屏 19 行（:60 `a < sb_val + 19`），滚动条按 18 槽给（:78）——原版自己的不一致。
    let start = state.credits_scroll.floor().max(0.0) as usize;
    let ink = pg.ink();
    let cx = pg.row(425.0, 0.0).x;
    for a in start..(start + 19).min(total) {
        let (text, bold, url) = &lines[a];
        let row = pg.row(0.0, 44.0 + 16.0 * (a - start) as f32);
        if let Some(u) = url {
            greet_link(state, &mut pg.ui, &p, &th, cx, row.y, text, u);
        } else {
            greet_text_center(&p, cx, row.y, text, PREFS_FONT, ink, *bold);
        }
    }
    list_scrollbar(
        &mut pg,
        "credits",
        538.0,
        36.0,
        16.0,
        18.0,
        total,
        &mut state.credits_scroll,
    );
    // :82-104 六个站点按钮；GitHub / Discord / Twitter / YouTube 四个
    // `condstr(language != 1, X, X)` 两边同词，直接写英文。
    for (x, label, url) in [
        (8.0, tr("Website", "官方网站"), crate::LINK_WEBSITE),
        (84.0, "GitHub", crate::LINK_GITHUB),
        (160.0, "Discord", crate::LINK_DISCORD),
        (236.0, "Twitter", crate::LINK_TWITTER),
        (312.0, "YouTube", crate::LINK_YOUTUBE),
        (388.0, tr("Donate", "捐赠"), crate::LINK_DONATE),
    ] {
        if pg.button2(x, 380.0, 72.0, label, false, true) {
            state.emit(Command::OpenUrl(url.to_string()));
        }
    }
    // :106-107「确定」= `windowclose = 1`，位置 `x2 - 72 - 8`。原版那句 `&& (windowopen = 1 || theme != 3)`
    // 是等开窗动画放完才认点击，端口没有开窗动画，恒真，不跟着复刻。
    if pg.button2(492.0, 380.0, 72.0, tr("OK", "确定"), false, false) {
        state.emit(Command::CloseWindow);
    }
}

/// 原版 :44-58 的谢词分行：行首 `*` 是标题（吃掉 `*`、走 `font_main_bold`），
/// 含 `|` 的那行在竖线处切开、后半当链接交给 `draw_text_url`。跟 `changelog_lines`
/// 一样，末尾换行之后的那段原版按 `string_count("\n")` 计数进不了表，这里同样丢掉。
pub(crate) fn credits_lines(src: &str) -> Vec<(String, bool, Option<String>)> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find('\n') {
        let mut line = &rest[..i];
        let bold = line.starts_with('*');
        if bold {
            line = &line[1..];
        }
        let url = match line.find('|') {
            Some(k) => {
                let u = line[k + 1..].to_string();
                line = &line[..k];
                Some(u)
            }
            None => None,
        };
        out.push((line.to_string(), bold, url));
        rest = &rest[i + 1..];
    }
    out
}

// ---------------------------------------------------------------- B/C/E 弹窗

fn audio_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(tr("Same as the original: renders PCM s16 WAV offline first, then transcodes MP3/AIFF/FLAC via ffmpeg.", "与原版相同：先离线渲染 PCM s16 WAV，MP3/AIFF/FLAC 再交给 ffmpeg 转码。"));
    if let Some(s) = &state.export_status {
        ui.label(
            RichText::new(s)
                .small()
                .color(egui::Color32::from_rgb(20, 130, 60)),
        );
    }
    ui.separator();
    egui::Grid::new("audio_export_grid")
        .num_columns(2)
        .show(ui, |ui| {
            // 原版四个选项：导出格式 / 采样率 / 通道 / 包括已静音的层。
            ui.label(tr("Output format", "导出格式"));
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
            ui.label(tr("Sampling rate", "采样率"));
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
            ui.label(tr("Channels", "通道"));
            let mut ch = state.prefs.export_channels;
            ui.radio_value(&mut ch, 1, tr("1 (mono)", "1（单声道）"))
                .on_hover_text(tr(
                    "Export the track with a single channel.",
                    "导出单声道的音频。",
                ));
            ui.radio_value(&mut ch, 2, tr("2 (stereo)", "2（立体声）"))
                .on_hover_text(tr(
                    "Export the track with two channels.",
                    "导出立体声的音频。",
                ));
            if ch != state.prefs.export_channels {
                state.prefs.export_channels = ch;
            }
            ui.end_row();
        });
    let mut locked = state.prefs.include_locked_in_export;
    if ui
        .checkbox(&mut locked, tr("Include locked layers", "包括已静音的层"))
        .on_hover_text(tr(
            "Whether to export locked layers in the audio track.",
            "是否在音频内包含已静音的层。",
        ))
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
            None => tr("ffmpeg not found: put it in the program directory, datafiles\\ or PATH, or set NBS_FFMPEG", "未找到 ffmpeg：把它放进程序目录、datafiles\\ 或 PATH，也可设 NBS_FFMPEG").into(),
        };
        ui.label(RichText::new(line).small().weak());
    }
    let info = if crate::lang() == 1 {
        format!(
            "本曲 {} 音符 / {:.1} 秒 / {:.2} t/s",
            state.doc.song.total_blocks,
            state.doc.song.song_length() as f32 / state.doc.song.meta.tempo,
            state.doc.song.meta.tempo
        )
    } else {
        format!(
            "This song: {} notes / {:.1} s / {:.2} t/s",
            state.doc.song.total_blocks,
            state.doc.song.song_length() as f32 / state.doc.song.meta.tempo,
            state.doc.song.meta.tempo
        )
    };
    ui.label(RichText::new(info).small().weak());
    ui.separator();
    if ui.button(tr("Export…", "选择输出路径并导出…")).clicked() {
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
    ui.label(
        RichText::new(format!(
            "{} {}",
            tr("Engine:", "引擎："),
            state.audio.name()
        ))
        .strong(),
    );
    ui.label(RichText::new(state.audio.device_info()).small().weak());
    ui.label(RichText::new(state.audio.sample_status()).small().weak());
    ui.label(
        RichText::new(format!(
            "{} {}",
            tr("Active voices:", "当前活跃 voice："),
            state.audio.active_voices()
        ))
        .small()
        .weak(),
    );
    ui.separator();
    let mut master = state.prefs.master_volume;
    if ui
        .add(egui::Slider::new(&mut master, 0.0..=2.0).text(tr("Master volume", "主音量")))
        .changed()
    {
        state.emit(Command::SetMasterVolume(master));
    }
    let mut speed = state.prefs.play_speed;
    if ui
        .add(egui::Slider::new(&mut speed, 0.25..=4.0).text(tr("Playback speed", "播放速度")))
        .changed()
    {
        state.emit(Command::SetPlaySpeed(speed));
    }
    let mut metronome = state.prefs.metronome;
    if ui
        .checkbox(
            &mut metronome,
            tr("Metronome (every 4 ticks)", "节拍器（每 4 tick）"),
        )
        .changed()
    {
        state.emit(Command::ToggleMetronome);
    }
    // 「播放」那四个开关和速度单位属于首选项的播放页，这里不重复一份。
    ui.separator();
    ui.label(tr(
        "Built-in sounds directory (empty = auto-search)",
        "内置采样目录（留空 = 自动搜索）",
    ));
    let mut dir = state.prefs.sounds_dir.clone().unwrap_or_default();
    ui.add(
        egui::TextEdit::singleline(&mut dir)
            .desired_width(f32::INFINITY)
            .hint_text(tr("auto", "自动")),
    );
    if ui
        .button(tr("Apply and reload sounds", "应用并重新加载采样"))
        .clicked()
    {
        state.emit(Command::SetSoundsDir(dir));
    }
    ui.separator();
    if ui.button(tr("Save preferences", "保存偏好")).clicked() {
        state.emit(Command::SavePreferences);
    }
    footer(state, ui);
}

// ---------------------------------------------------------------- MIDI 导入窗口
//
// 逐行对照 `target/v312/draw_window_midi_import.gml` 的中文分支。原版所有坐标都是窗口
// 左上角 `(x1, y1)` 的偏移，而 `Page::row(x, n)` 落在 `y1 + n + 22`（Fluent）/`y1 + n`
// （其余主题），所以按原版 `y1 + N` 摆放的行还要补上原版自己加的那一截：
//   * 设定区 `settings_y = y1 + (theme == 3) * 48` → `set_dy`（26 / 0）
//   * 表格里 `y1 += navigation_space`（只有 Fluent 加 32）→ `body_dy`（10 / 0）
// 22 是 `Page::row` 自带的 Fluent 偏移，所以下面这两个数才是「原版多出来的部分」。

/// 原版一页九行（`draw_scrollbar(id, x, y, 17, 9, 总数, 0, 1)` 里的那个 slots），行高 20。
const MIDI_SLOTS: usize = 9;
const MIDI_ROW_H: f32 = 20.0;

/// 原版横竖分隔线的颜色 `12632256`。
const MIDI_LINE: Color32 = ggray(192);

/// 原版页签（双语分支）；第四页只在「音符长度」打开时出现
/// （`tab_count = 3 + (w_midi_note_duration != 0)`）。
fn midi_tabs() -> [&'static str; 4] {
    [
        tr("Instruments", "乐器"),
        tr("Percussion", "打击乐"),
        tr("Tracks", "轨道"),
        tr("Note Duration", "音符长度"),
    ]
}

/// 原版各页 `tabstr` / `tabtip` / `tabw` 的双语分支：`(列宽, 表头, 悬停提示)`。
fn midi_cols(page: usize) -> &'static [(f32, &'static str, &'static str)] {
    const EN0: &[(f32, &str, &str)] = &[
        (50.0, "Channel", "The number of the channel."),
        (41.0, "Patch", "The patch (instrument) of the channel."),
        (
            260.0,
            "Patch name",
            "The name of the patch (instrument) of the channel.",
        ),
        (
            130.0,
            "Instrument",
            "The instrument the notes in the channel\nshould be translated to.",
        ),
        (
            100.0,
            "Octave modifier",
            "The octave modifier for all sounds in this channel.",
        ),
    ];
    const CN0: &[(f32, &str, &str)] = &[
        (50.0, "通道", "通道的个数。"),
        (41.0, "乐器", "通道所用的乐器。"),
        (260.0, "乐器名", "通道所用乐器的名称。"),
        (130.0, "音色", "通道内的音符应被转换成的音色。"),
        (100.0, "八度调整", "该通道的八度偏移设定。"),
    ];
    const EN1: &[(f32, &str, &str)] = &[
        (50.0, "Note", "The note which represents the sound."),
        (220.0, "Sound name", "The name of the sound."),
        (
            210.0,
            "Instrument",
            "The instrument the sound\nshould be translated to.",
        ),
        (100.0, "Pitch", "The pitch of the output."),
    ];
    const CN1: &[(f32, &str, &str)] = &[
        (50.0, "音调", "代表声音的音调。"),
        (220.0, "声音名", "该声音的名称。"),
        (210.0, "音色", "声音应被转换成的音色。"),
        (100.0, "音高", "输出的音高设定。"),
    ];
    const EN2: &[(f32, &str, &str)] = &[
        (79.0, "Track number", "The number of the track."),
        (300.0, "Track name", "The name of the track."),
        (
            200.0,
            "Amount of events",
            "The amount of events in the track.",
        ),
    ];
    const CN2: &[(f32, &str, &str)] = &[
        (79.0, "轨道序号", "轨道的序号。"),
        (300.0, "轨道名", "轨道的名称。"),
        (200.0, "事件数", "轨道上的事件数量。"),
    ];
    const EN3: &[(f32, &str, &str)] = &[
        (50.0, "Track", "The track containing this part.\nHover over a track number to see its name."),
        (60.0, "Channel", "The MIDI channel used by this part.\nParts on different tracks have separate duration settings."),
        (320.0, "Patch name", "The MIDI instrument used by this part."),
        (151.0, "Note Duration", "Include duration for this part in the current file.\nNew MIDI files start with all melodic parts enabled."),
    ];
    const CN3: &[(f32, &str, &str)] = &[
        (
            50.0,
            "轨道",
            "声部所在的轨道。\n将鼠标移到轨道序号上可查看名称。",
        ),
        (
            60.0,
            "通道",
            "声部使用的 MIDI 通道。\n不同轨道上的声部可分别设置音符长度。",
        ),
        (320.0, "乐器名", "声部使用的 MIDI 乐器。"),
        (
            151.0,
            "音符长度",
            "为当前文件中的此声部导入音符长度。\n新 MIDI 文件默认启用所有非打击乐声部。",
        ),
    ];
    match page {
        0 => {
            if crate::lang() == 1 {
                CN0
            } else {
                EN0
            }
        }
        1 => {
            if crate::lang() == 1 {
                CN1
            } else {
                EN1
            }
        }
        2 => {
            if crate::lang() == 1 {
                CN2
            } else {
                EN2
            }
        }
        _ => {
            if crate::lang() == 1 {
                CN3
            } else {
                EN3
            }
        }
    }
}

/// 一帧里四张表共用的上下文：界面上的映射表、解析结果、可选音色、图标册、待播的试听
/// 音符、当前打开的下拉（原版 `menu_shown`，一次只允许一个），以及表格区的主题偏移。
struct MidiCtx<'a> {
    opt: &'a mut nbs_io::midi::MidiImportOptions,
    midi: &'a nbs_io::midi::MidiData,
    names: &'a [(u8, String)],
    icons: &'a crate::icons::Icons,
    flats: bool,
    body_dy: f32,
    previews: &'a mut Vec<nbs_core::model::Note>,
    menu: &'a mut Option<crate::state::MidiMenu>,
    /// 原版 `menu_show[]`：带子菜单那一层展开的是第几行，每次重开菜单都要清零。
    sub: &'a mut Option<usize>,
}

impl MidiCtx<'_> {
    /// 原版 `show_menu_ext`：新开一个菜单，同时把子菜单的展开状态清掉
    /// （新实例的 `menu_show[]` 全是 0）。
    fn open_menu(&mut self, m: crate::state::MidiMenu) {
        *self.menu = Some(m);
        *self.sub = None;
    }
}

/// 原版 `draw_window_mididevices.gml`：500×350，设备名 + 指派音色两列，
/// 每行一枚指派下拉（禁用设备 / 无 / 使用选择的 / 音色表），底部
/// OK + 刷新设备。下拉项和行间距照原版 `show_menu_ext` 的清单
/// （`:76-84`）：三个固定项后一条分隔线接音色表，内置和自定义音色之间
/// 还有一条分隔线；分隔线不占菜单序号。
/// 原版 `draw_window_sound_import`（415×310）：导入音效向导，本地 `.minecraft`
/// 与 Mojang 下载两条路。每帧先 `pump` 抄后台进度、排覆盖确认/失败模态框。
fn sound_import_window(state: &mut EditorState, ui: &mut Ui) {
    crate::sound_import::pump(state);

    // 原版 info 文案（非 macOS 才有 Mojang 分支；我们是 Windows 版）。
    ui.label(tr(
        "This assistant will help you get sound files from your Minecraft:\nJava Edition installation or from the Mojang server.",
        "本向导将帮助您从您的 Minecraft：Java 版或 Mojang 服务器获取音效。",
    ));
    ui.add_space(6.0);

    // 「从 Mojang 下载」勾选框：下载中锁住（原版 draw_checkbox 的 lock 参数）。
    // 展示态拷贝到本地，点击走命令翻转真实状态（emit 里同步派发）。
    let busy = state.sound_import.download_toggle && state.sound_import.status == 1;
    let mut shown = state.sound_import.download_toggle;
    if ui
        .add_enabled(
            !busy,
            egui::Checkbox::new(&mut shown, tr("Download from Mojang", "从 Mojang 下载")),
        )
        .clicked()
    {
        state.emit(Command::SoundImportToggleDownload);
    }
    ui.add_space(4.0);

    if !state.sound_import.download_toggle {
        ui.label(tr(
            "Before continuing, ensure you have launched the version you want from\nthe Minecraft Launcher.\nSelect a version from the list, and then click 'Get sounds' to import the\nsounds from that version!",
            "在开始之前，请确保您从您的启动器至少启动过一次您想要选择的版本。\n从列表中选择一个游戏版本，然后点击“获取音效”来导入该版本的音效！",
        ));
        ui.add_space(10.0);
        ui.label(tr(
            "1. Set your Minecraft installation path:",
            "1. 设定您的.minecraft目录：",
        ));
        ui.add_space(4.0);
        // 原版 string_truncate(mc_install_path, 280, true)；「更改」按钮与路径同一行。
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(state.sound_import.mc_install_path.clone()).weak(),
                )
                .truncate(),
            );
            if ui
                .add(egui::Button::new(tr("Change", "更改")).min_size(egui::vec2(72.0, 0.0)))
                .clicked()
            {
                state.emit(Command::SoundImportChangeDir);
            }
        });
        ui.add_space(4.0);
        if ui
            .add(
                egui::Button::new(tr("Use default", "使用默认设定"))
                    .min_size(egui::vec2(72.0, 0.0)),
            )
            .clicked()
        {
            state.emit(Command::SoundImportUseDefaultPath);
        }
    } else {
        ui.label(tr(
            "Before continuing, you will need to fetch the asset indexes of Minecraft\nversions from Mojang.\nSelect a version from the list, and then click 'Get sounds' to import the\nsounds from that version!",
            "在开始之前，请从 Mojang 获取 Minecraft 版本的资产索引。\n从列表中选择一个游戏版本，然后点击“获取音效”来导入该版本的音效！",
        ));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(tr(
                "1. Get asset indexes from Mojang",
                "1. 从 Mojang 获取资产索引",
            ));
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(tr("Get", "获取")).min_size(egui::vec2(72.0, 0.0)),
                )
                .clicked()
            {
                state.emit(Command::SoundImportGetIndexes);
            }
        });
    }
    ui.add_space(8.0);

    // 第 2 步：版本下拉 + 状态文字。
    ui.label(tr(
        "2. Select the asset list to copy sounds from:",
        "2. 选择想从中提取音效的版本：",
    ));
    ui.add_space(4.0);
    let combo_locked = state.sound_import.indexes.is_empty() || state.sound_import.files_index > 0;
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let (selected, download) = (
            state.sound_import.selected.clone(),
            state.sound_import.download_toggle,
        );
        let shown = if download {
            selected
        } else {
            crate::sound_import::friendly_name(&selected)
        };
        egui::ComboBox::from_id_salt("sound_import_asset_index")
            .selected_text(shown)
            .width(86.0)
            .show_ui(ui, |ui| {
                let (indexes, select, download) = (
                    state.sound_import.indexes.clone(),
                    state.sound_import.select,
                    state.sound_import.download_toggle,
                );
                for (i, id) in indexes.iter().enumerate() {
                    let label = if download {
                        id.clone()
                    } else {
                        crate::sound_import::friendly_name(id)
                    };
                    if ui
                        .add_enabled(
                            !combo_locked,
                            egui::SelectableLabel::new(select == i, label),
                        )
                        .clicked()
                    {
                        state.emit(Command::SoundImportSelectIndex(i));
                    }
                }
            });
        // 状态文字（原版在 x+86+8 处）。
        let (status, count, files_index, files_total, download) = (
            state.sound_import.status,
            state.sound_import.count,
            state.sound_import.files_index,
            state.sound_import.files_total,
            state.sound_import.download_toggle,
        );
        if status == 2 {
            ui.label(
                egui::RichText::new(format!(
                    "{}{}",
                    crate::sound_import::format_thousands(count),
                    tr(" sounds have been copied!", " 个音效获取成功！")
                ))
                .color(egui::Color32::from_rgb(60, 170, 60)),
            );
        } else if status == 1 {
            ui.label(tr(
                "Copying sounds... please wait!",
                "正在获取音效...... 请稍候！",
            ));
            if download {
                ui.label(format!(
                    "{}/{}{}",
                    files_index.max(1) - 1,
                    files_total,
                    tr(" sounds downloaded", " 个音效已下载")
                ));
            }
        } else if count > 0 {
            ui.label(format!(
                "{}{}",
                crate::sound_import::format_thousands(count),
                tr(" sounds located!", " 个音效已被找到！")
            ));
        } else if !download {
            ui.label(
                egui::RichText::new(tr(
                    "No sounds located! Check your\nMinecraft installation path.",
                    "未找到音效！请检查您的 Minecraft\n安装目录。",
                ))
                .color(egui::Color32::RED),
            );
        }
    });
    ui.add_space(6.0);

    // 获取音效（右对齐 86 宽）。
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(12.0);
        let get_locked = state.sound_import.selected.is_empty() || state.sound_import.status == 1;
        if ui
            .add_enabled(
                !get_locked,
                egui::Button::new(tr("Get sounds", "获取音效")).min_size(egui::vec2(86.0, 0.0)),
            )
            .clicked()
        {
            state.emit(Command::SoundImportGet);
        }
    });
    ui.add_space(8.0);

    // 底行：音色设置... | 打开已获取音效目录 | ... | 确定。
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(tr("Instrument settings...", "音色设置…"))
                    .min_size(egui::vec2(120.0, 0.0)),
            )
            .clicked()
        {
            state.emit(Command::ShowWindow(WindowKind::Instruments));
        }
        if ui
            .add(
                egui::Button::new(tr("Open Sounds folder", "打开已获取音效目录"))
                    .min_size(egui::vec2(120.0, 0.0)),
            )
            .clicked()
        {
            state.emit(Command::SoundImportOpenSoundsFolder);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let busy = state.sound_import.download_toggle && state.sound_import.status == 1;
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(tr("OK", "确定")).min_size(egui::vec2(72.0, 0.0)),
                )
                .clicked()
            {
                state.emit(Command::SoundImportOk);
            }
        });
    });
}

fn midi_devices_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(tr(
        "Here you can see and select the MIDI devices connected via USB.",
        "你可以查看和选择通过 USB 连接的 MIDI 设备。",
    ));
    ui.add_space(4.0);
    let devices = state.live_midi.devices.clone();
    let rows = devices.len().min(crate::live_midi::DEVICE_SLOTS);
    egui::Grid::new("midi_devices_grid")
        .num_columns(2)
        .min_col_width(325.0)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            // 原版两个 20px 高的表头格，鼠标悬停有说明（popup_set_window）。
            ui.label(tr("Name", "名称"))
                .on_hover_text(tr("The name of the device.", "该设备的名称。"));
            ui.label(tr("Instrument", "音色")).on_hover_text(tr(
                "The instrument that note block added by this device\nwhen recording should have.",
                "为该 MIDI 设备指定的音色。",
            ));
            ui.end_row();
            for d in 0..rows {
                ui.label(&devices[d]);
                let assignment = state.live_midi.assignments[d];
                let list = state.instrument_list();
                let current = midi_assignment_label(assignment, &list);
                egui::ComboBox::from_id_salt(format!("midi_device_{d}"))
                    .selected_text(current)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for (value, en, zh) in [
                            (-3, "Disable device", "禁用设备"),
                            (-2, "None", "无"),
                            (-1, "Use selected", "使用选择的"),
                        ] {
                            if ui
                                .selectable_label(assignment == value, tr(en, zh))
                                .clicked()
                            {
                                assign_midi_device(state, d, value);
                            }
                        }
                        ui.separator();
                        for (i, (id, name)) in list.iter().enumerate() {
                            if i as u8 == nbs_core::model::FIRST_CUSTOM_INDEX {
                                ui.separator();
                            }
                            if ui.selectable_label(assignment == *id as i8, name).clicked() {
                                assign_midi_device(state, d, *id as i8);
                            }
                        }
                    });
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        // 原版两枚按钮：OK 在右（x+408），刷新设备在左（x+12）。
        if ui.button(tr("OK", "确定")).clicked() {
            state.emit(Command::CloseWindow);
        }
        if ui.button(tr("Refresh", "刷新设备")).clicked() {
            state.live_midi.refresh_devices();
        }
    });
}

/// 设备行第二列的显示名：-3/-2/-1 三个固定文案，其余用音色名。
fn midi_assignment_label(assignment: i8, list: &[(u8, String)]) -> String {
    match assignment {
        -3 => tr("Disable device", "禁用设备").to_string(),
        -2 => tr("None", "无").to_string(),
        -1 => tr("Use selected", "使用选择的").to_string(),
        _ => list
            .iter()
            .find(|(id, _)| *id == assignment as u8)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| {
                let n = assignment as usize - nbs_core::model::FIRST_CUSTOM_INDEX as usize + 1;
                tr(
                    &format!("Custom instrument {n}"),
                    &format!("自定义音色 {n}"),
                )
                .to_string()
            }),
    }
}

/// 原版 menu_click.gml:310-314：每次改指派都先 `midi_set_device`（重开设备口，
/// 顺带把全部指派清成 -3）——所以给一台设备指派音色会抹掉其他设备的指派，
/// 这是原版行为；设备口开不出就刷新设备表并提示。
fn assign_midi_device(state: &mut EditorState, device: usize, value: i8) {
    if !state.live_midi.set_device(device) {
        state.live_midi.refresh_devices();
        state.set_message(tr(
            "That device no longer exists, refreshing...",
            "所指定的设备不存在，刷新中……",
        ));
        return;
    }
    state.live_midi.assignments[device] = value;
}

fn midi_import_window(state: &mut EditorState, ui: &mut Ui) {
    // 原版没有「空白的 MIDI 导入窗口」：`command_open_midi` 一定先解析、解析成功才开窗，
    // 所以这里只在数据被清掉的边缘情况下退回一句说明。
    let Some(midi) = state.midi_data.as_ref() else {
        ui.label(RichText::new(tr("No parsed MIDI file.", "没有已解析的 MIDI 文件。")).weak());
        return;
    };
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let names = state.instrument_list();
    let flats = state.flags.flats;
    let margin = ui.spacing().window_margin;
    // 原版窗口外沿 600×(400 + Fluent 的 32 像素导航条)，内容区要扣掉 egui 的窗口内边距；
    // 把原点左移一个内边距，行内 x 就能直接写原版的数字。
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    let body = ui.allocate_exact_size(
        egui::vec2(600.0 - ml - mr, if fluent { 402.0 } else { 374.0 }),
        egui::Sense::hover(),
    );
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };
    let set_dy = if fluent { 26.0 } else { 0.0 };
    let body_dy = if fluent { 10.0 } else { 0.0 };
    // `row()` 里已经含了 Fluent 那 22 像素的行偏移，凡是原版直接写 `y1 + N`
    // （不带 `settings_y`、也不在 `y1 += navigation_space` 之后）的矩形都要减回去。
    let abs_dy = if fluent { -22.0 } else { 0.0 };

    let mut previews: Vec<nbs_core::model::Note> = Vec::new();
    let mut do_import = false;
    let mut do_cancel = false;
    let mut do_defaults = false;
    {
        let mut pg = Page {
            ui: &mut *ui,
            o,
            ry,
            th: &th,
            fluent,
        };
        let opt = &mut state.midi_options;
        let tab = &mut state.midi_tab;
        let scroll = &mut state.midi_scroll;
        let menu = &mut state.midi_menu;
        let sub = &mut state.midi_submenu;
        let drags = &mut state.midi_drags;
        let icons = &state.icons;

        // ---- 上半部设定区。Fluent 先铺一块 0xCCCCCC（fdark 0x333333）的圆角卡片。
        if fluent {
            pg.ui.painter().clone().rect_filled(
                pg.row_rect(8.0, 74.0 + abs_dy, 592.0, 176.0 + abs_dy),
                16.0,
                if th.fdark { ggray(51) } else { ggray(204) },
            );
        }
        if pg.check(
            32.0,
            32.0 + set_dy,
            opt.remove_silent,
            tr("Remove silent parts at beginning", "去除开始时空部分"),
            tr(
                "Whether to remove any silent parts\nat the beginning of the song.",
                "是否移除歌曲开始时无音符的部分。",
            ),
            false,
            false,
        ) {
            opt.remove_silent = !opt.remove_silent;
        }
        if pg.check(
            32.0,
            52.0 + set_dy,
            opt.name_layers,
            tr("Name layers...", "给每层命名......"),
            tr(
                "If the layers should be given names\ndepending on the data in the MIDI file.",
                "是否根据 MIDI 内数据为每层命名。",
            ),
            false,
            false,
        ) {
            opt.name_layers = !opt.name_layers;
        }
        if pg.radio(
            52.0,
            72.0 + set_dy,
            opt.name_after_patch,
            tr("...after patches", "......根据乐器"),
            tr(
                "If the layers should be named\nafter the instruments in the MIDI file.",
                "是否根据 MIDI 中的乐器为每层命名。",
            ),
            !opt.name_layers,
        ) {
            opt.name_after_patch = true;
        }
        if pg.radio(
            52.0,
            92.0 + set_dy,
            !opt.name_after_patch,
            tr("...channel numbers", "......根据层号"),
            tr(
                "If the layers should be named\nafter the channels in the MIDI file.",
                "是否根据 MIDI 中的层序号为每层命名。",
            ),
            !opt.name_layers,
        ) {
            opt.name_after_patch = false;
        }
        // EN 与 CN 的右半列起点不同：EN 260 / CN 300。
        let col2_x = if crate::lang() == 1 { 300.0 } else { 260.0 };
        if pg.check(
            col2_x,
            32.0 + set_dy,
            opt.tempo_from_file,
            tr("Same tempo as in file", "导入速度"),
            tr(
                "Set the song's tempo to match\nthe one of the MIDI file.",
                "是否将速度设定为与 MIDI 文件中一样。",
            ),
            false,
            false,
        ) {
            opt.tempo_from_file = !opt.tempo_from_file;
        }
        if pg.check(
            410.0,
            32.0 + set_dy,
            opt.tempo_changer,
            tr("Tempo changes", "速度变化"),
            tr("Whether to add tempo changes found in the MIDI file.\nTempo changes are not supported in-game and in most NBS-compatible tools.", "是否添加 MIDI 文件中的速度变化。\n速度调节器（Tempo Changer）在游戏中及大多数NBS兼容软件中不被支持。"),
            false,
            false,
        ) {
            opt.tempo_changer = !opt.tempo_changer;
        }
        // 打开「音符长度」会让第四页出现，并立刻跳过去（原版 `w_midi_tab = 3`）。
        if pg.check(
            col2_x,
            52.0 + set_dy,
            opt.note_duration,
            tr("Note Duration", "音符长度"),
            tr("Whether to include the note duration as a stereo trail of notes.\nChoose individual parts in the Note Duration tab.", "是否将长音以震荡式的排列方式表示。\n可在“音符长度”页面为各声部分别设置。"),
            false,
            false,
        ) {
            opt.note_duration = !opt.note_duration;
            if opt.note_duration {
                *tab = 3;
            }
        }
        pg.text(
            pg.row(col2_x, 72.0 + set_dy),
            tr("Max. channel height:", "通道最高层数:"),
            PREFS_FONT,
            pg.ink(),
        );
        pg.tip_area(
            col2_x,
            72.0 + set_dy,
            140.0,
            16.0,
            tr(
                "The maximum allowed layers per channel.\nClick and drag to adjust.",
                "每个通道所允许使用的最多层数。拖拽来更改。",
            ),
        );
        if let Some(v) = pg.drag(
            if crate::lang() == 1 { 420.0 } else { 380.0 },
            72.0 + set_dy,
            opt.max_height as i32,
            1.0,
            1,
            20,
            false,
            "",
            &mut drags[0],
        ) {
            opt.max_height = v as u8;
        }
        if pg.check(
            col2_x,
            92.0 + set_dy,
            opt.keep_octave_range,
            tr("Keep within octave range", "保持八度范围"),
            tr("Whether to automatically transpose the notes\nto keep them within the 2 octave range.", "是否自动将音符转换到 2 八度限制内。"),
            false,
            false,
        ) {
            opt.keep_octave_range = !opt.keep_octave_range;
        }
        if pg.check(
            32.0,
            112.0 + set_dy,
            opt.use_velocity,
            tr("Read note velocity", "导入音符音量"),
            tr(
                "Whether to copy the volume data found\nin each MIDI note.",
                "是否将 MIDI 文件中音符音量应用到音符上。",
            ),
            false,
            false,
        ) {
            opt.use_velocity = !opt.use_velocity;
        }
        if pg.check(
            col2_x,
            112.0 + set_dy,
            opt.note_duration_fade,
            tr("Fade tail velocity", "尾音渐变"),
            tr("Linearly change the generated tail notes from the start percentage to the end percentage.\nThe original note head keeps its imported velocity.", "将生成的尾音从起始百分比线性变化到结束百分比。\n原音符起始音的音量保持不变。"),
            false,
            !opt.note_duration,
        ) {
            opt.note_duration_fade = !opt.note_duration_fade;
        }
        // 「音符长度」或「尾音渐变」没开时，原版把这一整行涂成 c_gray 并改成静态数字。
        let fade_locked = !opt.note_duration || !opt.note_duration_fade;
        let dim = ggray(128);
        let head_ink = if fade_locked { dim } else { pg.ink() };
        pg.text(
            pg.row(
                if crate::lang() == 1 { 405.0 } else { 410.0 },
                111.0 + set_dy,
            ),
            tr("Start:", "起始:"),
            PREFS_FONT,
            head_ink,
        );
        let start_n = if fade_locked { 111.0 } else { 112.0 };
        if let Some(v) = pg.drag(
            445.0,
            start_n + set_dy,
            opt.note_duration_fade_start as i32,
            1.0,
            0,
            100,
            fade_locked,
            "",
            &mut drags[1],
        ) {
            opt.note_duration_fade_start = v as u8;
        }
        pg.text(pg.row(466.0, 111.0 + set_dy), "%", PREFS_FONT, head_ink);
        pg.text(
            pg.row(485.0, 111.0 + set_dy),
            tr("End:", "结束:"),
            PREFS_FONT,
            head_ink,
        );
        if let Some(v) = pg.drag(
            if crate::lang() == 1 { 525.0 } else { 513.0 },
            start_n + set_dy,
            opt.note_duration_fade_end as i32,
            1.0,
            0,
            100,
            fade_locked,
            "",
            &mut drags[2],
        ) {
            opt.note_duration_fade_end = v as u8;
        }
        pg.text(
            pg.row(
                if crate::lang() == 1 { 546.0 } else { 534.0 },
                111.0 + set_dy,
            ),
            "%",
            PREFS_FONT,
            head_ink,
        );
        pg.tip_area(
            if crate::lang() == 1 { 403.0 } else { 408.0 },
            108.0 + set_dy,
            if crate::lang() == 1 { 72.0 } else { 67.0 },
            20.0,
            tr("Velocity of the first generated tail note,\nas a percentage of the MIDI note velocity.\nClick and drag to adjust.", "第一个尾音相对于 MIDI 音符音量的百分比。\n拖拽来更改。"),
        );
        pg.tip_area(
            483.0,
            108.0 + set_dy,
            if crate::lang() == 1 { 72.0 } else { 60.0 },
            20.0,
            tr("Velocity of the last generated tail note,\nas a percentage of the MIDI note velocity.\nClick and drag to adjust.", "最后一个尾音相对于 MIDI 音符音量的百分比。\n拖拽来更改。"),
        );
        pg.text(
            pg.row(470.0, 52.0 + set_dy),
            tr("Time precision", "精准度"),
            PREFS_FONT,
            pg.ink(),
        );
        pg.tip_area(
            470.0,
            52.0 + set_dy,
            100.0,
            20.0,
            tr("How much to increase the spacing between each note,\nso that more notes can be placed in between.", "调整音符间的距离，以在中间放下更多音符。"),
        );
        let prec = [
            (470.0, 72.0, 0u8, tr("1x", "1 倍"), tr("Keep the same spacing found in the MIDI file.", "与 MIDI 文件中音符间距一致。")),
            (470.0, 92.0, 1, tr("2x", "2 倍"), tr("Add twice as much space between each note.", "在音符间使用两倍间距。")),
            (520.0, 72.0, 3, tr("4x", "4 倍"), tr("Add four times as much space between each note.", "在音符间使用四倍间距。")),
            (
                520.0,
                92.0,
                7,
                tr("8x", "8 倍"),
                tr("Add eight times as much space between each note.\n(This will create a very long song!)", "在音符间使用八倍间距。\n（歌曲将会很长！）"),
            ),
        ];
        for (x, n, value, label, tip) in prec {
            if pg.radio(x, n + set_dy, opt.precision == value, label, tip, false) {
                opt.precision = value;
            }
        }

        // ---- 页签条。原版 `tab_count = 3 + (w_midi_note_duration != 0)`，越界回到 0。
        let tab_count = 3 + usize::from(opt.note_duration);
        if *tab >= tab_count {
            *tab = 0;
        }
        if fluent {
            // `draw_navigationview` 自带左上角返回按钮（原版在 `(x1, y1 + 30)` 的 40×40 那块）。
            if pg.back(icons) {
                do_cancel = true;
            }
        } else {
            // 非 Fluent 先铺面板再画页签：选中的页签要压掉面板顶边那一像素。
            midi_panel_box(&mut pg);
        }
        let nsel = pg.tabs(
            if fluent { 40.0 } else { 8.0 },
            if fluent { 0.0 } else { 102.0 },
            &midi_tabs()[..tab_count],
            *tab as u8,
        ) as usize;
        if nsel != *tab {
            *tab = nsel;
        }
        // 原版 `w_midi_tab += keyboard_check_pressed(vk_right) - keyboard_check_pressed(vk_left)`，
        // 出界绕到另一头。
        let (left, right) = pg.ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
            )
        });
        if left {
            *tab = (*tab + tab_count - 1) % tab_count;
        }
        if right {
            *tab = (*tab + 1) % tab_count;
        }

        // 原版把文件名和总时长画在标题带里（右对齐到 `x1 + 590`）。egui 的标题带归
        // `Window` 自己，所以文件名并进了窗口标题，这里只留时长那一行。
        let secs = midi.song_length(opt.tempo_changer, opt.remove_silent);
        let len_str = if secs > 0.0 {
            crate::layers_panel::time_str(secs)
        } else {
            tr("Song tempo never defined", "速度未定义").to_string()
        };
        let lw = pg.measure(&len_str, PREFS_FONT);
        pg.text(pg.at(590.0 - lw, 2.0), &len_str, PREFS_FONT, pg.ink());

        // ---- 四张表。原版每页九行，行号从 `floor(sb_val) ` 起，超出总数就截断。
        let mut ctx = MidiCtx {
            opt,
            midi,
            names: &names,
            icons,
            flats,
            body_dy,
            previews: &mut previews,
            menu,
            sub,
        };
        match *tab {
            // 乐器页的总行数含「10 号通道 = 打击乐」那一行，所以是 channels + 1。
            0 => midi_table(
                &mut pg,
                &mut ctx,
                midi_cols(0),
                midi.channels as usize + 1,
                586.0,
                MIDI_VLINES[0],
                &mut scroll[0],
                midi_row_ins,
            ),
            1 => midi_table(
                &mut pg,
                &mut ctx,
                midi_cols(1),
                midi.perc_notes.len(),
                586.0,
                MIDI_VLINES[1],
                &mut scroll[1],
                midi_row_perc,
            ),
            2 => midi_table(
                &mut pg,
                &mut ctx,
                midi_cols(2),
                midi.tracks.len(),
                586.0,
                MIDI_VLINES[2],
                &mut scroll[2],
                midi_row_track,
            ),
            _ => {
                let rows = midi.parts.len();
                midi_table(
                    &mut pg,
                    &mut ctx,
                    midi_cols(3),
                    rows,
                    570.0,
                    MIDI_VLINES[3],
                    &mut scroll[3],
                    midi_row_part,
                );
                if rows == 0 {
                    pg.text(
                        pg.row(16.0, 178.0 + body_dy),
                        tr("No notes found in this file.", "此文件中没有音符。"),
                        PREFS_FONT,
                        pg.ink(),
                    );
                }
            }
        }

        // ---- 底部：原版 `draw_checkbox(..., false, true)` 的「记住我的更改」是胶囊开关。
        if pg.check(
            12.0,
            374.0 + body_dy,
            ctx.opt.remember,
            tr("Remember changes", "记住我的更改"),
            tr(
                "Whether to use these settings the\nnext time you import a MIDI file.",
                "下次导入 MIDI 文件时是否使用同样的设定。",
            ),
            true,
            false,
        ) {
            ctx.opt.remember = !ctx.opt.remember;
        }
        if pg.button(520.0, 368.0 + body_dy, 72.0, tr("Import", "导入"), false) {
            do_import = true;
        }
        if pg.button2(
            440.0,
            368.0 + body_dy,
            72.0,
            tr("Cancel", "取消"),
            false,
            true,
        ) {
            do_cancel = true;
        }
        if pg.button2(
            360.0,
            368.0 + body_dy,
            72.0,
            tr("Use default", "使用默认值"),
            false,
            true,
        ) {
            do_defaults = true;
        }
    }

    for note in previews {
        state.audio.preview_note(&note);
    }
    if do_import {
        state.emit(Command::MidiImportRun);
        return;
    }
    if do_cancel {
        state.emit(Command::MidiImportCancel);
        return;
    }
    if do_defaults {
        // 原版 `question("你确定吗？", "确定")`：确认后才重查 GM 表并恢复 `w_midi_*`。
        state.modals = ModalQueue::question(
            tr("Confirm", "确定"),
            tr("Are you sure?", "你确定吗？"),
            Command::MidiUseDefaults,
        );
    }
    if let Some(kind) = state.midi_menu {
        use crate::state::MidiMenu as M;
        let items = midi_menu_items(&names, flats, &state.midi_options, kind);
        let (x, n) = midi_menu_pos(kind, &state.midi_scroll, body_dy);
        let bank = state.icons.bank();
        let (picked, dismissed) = midi_popup(
            ui,
            "midi_menu",
            o + Vec2::new(x, n + ry),
            &items,
            &mut state.midi_submenu,
            &th,
            bank.as_deref(),
        );
        if let Some(v) = picked {
            let v = v as i8;
            let opt = &mut state.midi_options;
            match kind {
                M::ChannelIns(b) if b < 16 => opt.channel_ins[b] = v,
                M::ChannelOct(b) if b < 16 => opt.channel_octave[b] = v,
                M::PercIns(b) => {
                    if let Some(s) = opt.perc_ins.get_mut(b) {
                        *s = v;
                    }
                }
                M::PercPitch(b) => {
                    if let Some(s) = opt.perc_pitch.get_mut(b) {
                        *s = v;
                    }
                }
                _ => {}
            }
            state.midi_menu = None;
        }
        if dismissed {
            state.midi_menu = None;
        }
    }
}

/// 原版非 Fluent 的表格底板。主题 1 先在外面再套一层小窗口
/// （`draw_window(x1 + 4, y1 + 145, x1 + 596, y1 + 364)`，`spr_interface2` 没导出，
/// 用同色底 + 主题描边近似），然后在 `(6, 146)-(594, 362)` 铺一块白
/// （主题 2 是 c_dark）并描 (137,140,149) 一圈；主题 1 再在里面补
/// `(9, 149)-(589, 358)` 的白底加 `draw_area` 那一圈。Fluent 没有这块
/// （源码那两行 `draw_rectangle` 的条件都是 `theme != 3`）。
fn midi_panel_box(pg: &mut Page) {
    let retro = pg.th.kind == ThemeKind::Retro;
    let p = pg.ui.painter().clone();
    if retro {
        let outer = pg.row_rect(4.0, 145.0, 596.0, 364.0);
        p.rect_filled(outer, 0.0, ggray(200));
        p.rect_stroke(
            outer,
            0.0,
            Stroke::new(1.0_f32, pg.th.border),
            StrokeKind::Inside,
        );
    }
    let rect = if retro {
        pg.row_rect(9.0, 149.0, 589.0, 358.0)
    } else {
        pg.row_rect(6.0, 146.0, 594.0, 362.0)
    };
    let fill = if pg.th.kind == ThemeKind::Dark {
        ggray(64)
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
    if retro {
        pg.frame_box(9.0, 149.0, 590.0, 360.0);
    }
}

/// 原版第 `k` 列的右边界 `x1 + 8 + Σ tabw[..k]`：表头、试听热区和下拉菜单都以它为准。
fn midi_col_end(cols: &[(f32, &str, &str)], k: usize) -> f32 {
    8.0 + cols[..k.min(cols.len())].iter().map(|c| c.0).sum::<f32>()
}

/// 原版格子里文字的起点：`x1 + 12 + 4 + Σ tabw[..k]`。
fn midi_col_x(cols: &[(f32, &str, &str)], k: usize) -> f32 {
    midi_col_end(cols, k) + 8.0
}

/// 原版每页表格的竖分隔线，逐条照该页 `draw_line` 那几行抄。多数页符合
/// `x1 + 11 - k + Σ tabw[..k]`（每往右一列就多退一像素），只有「轨道」页最后一条
/// 写成 `x1 + 9 + Σ`，比公式多一像素——原版的笔误，这里照抄不修。
const MIDI_VLINES: [&[f32]; 4] = [
    &[12.0, 60.0, 100.0, 359.0, 488.0],
    &[12.0, 60.0, 279.0, 488.0],
    &[12.0, 89.0, 388.0, 588.0],
    &[12.0, 66.0, 127.0, 438.0],
];

/// 原版试听热区：`(列左边界, y1 + 170 + 20a, 列宽 - 20, 20)`。
fn midi_preview_rect(
    pg: &Page,
    cols: &[(f32, &str, &str)],
    k: usize,
    a: usize,
    body_dy: f32,
) -> Rect {
    let at = pg.row(
        midi_col_end(cols, k),
        170.0 + a as f32 * MIDI_ROW_H + body_dy,
    );
    Rect::from_min_size(at, Vec2::new(cols[k].0 - 20.0, MIDI_ROW_H))
}

fn midi_hline(pg: &Page, x1: f32, x2: f32, n: f32) {
    let p = pg.ui.painter().clone();
    p.line_segment(
        [pg.row(x1, n), pg.row(x2, n)],
        Stroke::new(1.0_f32, MIDI_LINE),
    );
}

fn midi_vline(pg: &Page, x: f32, n1: f32, n2: f32) {
    let p = pg.ui.painter().clone();
    p.line_segment(
        [pg.row(x, n1), pg.row(x, n2)],
        Stroke::new(1.0_f32, MIDI_LINE),
    );
}

/// 原版表格区：九行内容 + 分隔线 + 从右往左铺的表头 + 右侧那根 17×185 的滚动条。
/// `rows` 是这一页的总条数，`line_right` 是每行下面那条横线伸到哪儿（原版「音符长度」
/// 页只到 570，其余到 586），`vlines` 是这一页的竖线 x（见 `MIDI_VLINES`），
/// `row_fn(pg, ctx, 行号, 可见序号)` 负责画一整行。
fn midi_table(
    pg: &mut Page,
    ctx: &mut MidiCtx,
    cols: &[(f32, &str, &str)],
    rows: usize,
    line_right: f32,
    vlines: &[f32],
    scroll: &mut f32,
    row_fn: fn(&mut Page, &mut MidiCtx, usize, usize),
) {
    let base = scroll.floor().max(0.0) as usize;
    let mut drawn = 0usize;
    for a in 0..MIDI_SLOTS {
        let b = base + a;
        if b >= rows {
            break;
        }
        row_fn(pg, ctx, b, a);
        drawn += 1;
        midi_hline(
            pg,
            12.0,
            line_right,
            190.0 + a as f32 * MIDI_ROW_H + ctx.body_dy,
        );
    }
    // 竖线的下沿按实际画出的行数收（原版用的是循环结束后的那个 `a`）；一行都没画时
    // 原版根本不进这个分支（「打击乐」页），所以这时整组竖线都不画。
    let top = 170.0 + ctx.body_dy;
    if drawn > 0 {
        let bottom = top + drawn as f32 * MIDI_ROW_H;
        for x in vlines {
            midi_vline(pg, *x, top, bottom);
        }
    }
    // 表头：`xx = x1 + 588` 起从最后一列往左铺，相邻列重叠一像素，画在内容之上。
    let mut xx = 588.0;
    for (w, name, tip) in cols.iter().rev() {
        midi_header_cell(pg, xx - w, *w, name, tip);
        xx -= w - 1.0;
    }
    midi_scrollbar(pg, 171.0 + ctx.body_dy, rows.max(1), scroll);
}

/// 原版 `draw_window(xx - tabw, y1 + 151, xx, y1 + 151 + 20, 1)` 的那一格表头。
/// `sml = 1` 只是不画阴影；底色按主题分别是 240 / 200 / c_dark / 243（fdark 32），
/// 九宫格边框 `spr_interface2` 没导出，用主题描边近似（同 `Page::frame_box`）。
fn midi_header_cell(pg: &mut Page, x: f32, w: f32, name: &str, tip: &str) {
    let top = 151.0 + if pg.fluent { 10.0 } else { 0.0 };
    let rect = pg.row_rect(x, top, x + w, top + 20.0);
    let fill = match pg.th.kind {
        ThemeKind::Aqua => ggray(240),
        ThemeKind::Retro => ggray(200),
        ThemeKind::Dark => ggray(64),
        ThemeKind::Fluent if pg.th.fdark => ggray(32),
        ThemeKind::Fluent => ggray(243),
    };
    let p = pg.ui.painter().clone();
    p.rect_filled(rect, 0.0, fill);
    p.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0_f32, pg.th.border),
        StrokeKind::Inside,
    );
    pg.tip_area(x, top, w, 20.0, tip);
    pg.text(pg.row(x + 4.0, top + 3.0), name, PREFS_FONT, pg.ink());
}

/// 原版 `draw_scrollbar(id, x1 + 572, y1 + 171, 17, 9, 总数, 0, 1)` 的竖直分支：
/// 上下各一个 16×16 箭头，中间 153 像素的槽；滑块高 `max(10, 9/总数 × 153)`，
/// 顶边在 `滚动量 / 总数 × 153`（取整）。滚轮 ±1 行，拖拽按 `dy ÷ (153/总数)` 折算，
/// 松手时 `round` 取整；总数不到一页时整条锁定（滑块不画，点了也不动）。
/// 两处和原版不一样，都是有意的：①滚轮在指针悬停于滚动条上时即生效，原版要先点一下
/// 让 `sb_sel` 指到它；②按住箭头或槽底不做按帧连发（原版的 `sb_press` 计时），
/// 只响应单次按下——连发要跨帧计时，功能上和单击没有区别。
fn midi_scrollbar(pg: &mut Page, n: f32, total: usize, scroll: &mut f32) {
    const TRACK: f32 = 17.0 * 9.0;
    let slots = MIDI_SLOTS as f32;
    let x = 572.0;
    let max_scroll = (total as f32 - slots).max(0.0);
    *scroll = scroll.clamp(0.0, max_scroll);
    let locked = total as f32 <= slots;
    let bwh = if locked {
        0.0
    } else {
        (slots / total as f32 * TRACK).max(10.0)
    };
    let bpos = if locked {
        0.0
    } else {
        (*scroll / total as f32 * TRACK).floor()
    };
    let p = pg.ui.painter().clone();
    p.rect_filled(
        Rect::from_min_size(pg.row(x, n), Vec2::new(17.0, TRACK + 32.0)),
        3.0,
        pg.th.panel,
    );
    if !locked {
        let thumb = Rect::from_min_size(
            pg.row(x, n + 20.0 + bpos),
            Vec2::new(17.0, bwh.floor() - 6.0),
        );
        p.rect_filled(thumb, 3.0, pg.th.accent_soft);
        p.rect_stroke(
            thumb,
            3.0,
            Stroke::new(1.0_f32, pg.th.accent),
            StrokeKind::Inside,
        );
    }
    // 上下箭头。原版是 `spr_scrollbar_v_button_up/down`，精灵图没导出，画同位置的三角形。
    let ink = pg.ink();
    for (down, dy) in [(false, 0.0), (true, TRACK + 16.0)] {
        let rect = Rect::from_min_size(pg.row(x, n + dy), Vec2::new(16.0, 16.0));
        let (resp, _) = pg.hit(rect, Id::new(("midi-sb-btn", n as i32, dy as i32)), locked);
        let c = rect.center();
        let pts = if down {
            vec![
                c + Vec2::new(-3.5, -1.5),
                c + Vec2::new(3.5, -1.5),
                c + Vec2::new(0.0, 3.0),
            ]
        } else {
            vec![
                c + Vec2::new(-3.5, 1.5),
                c + Vec2::new(3.5, 1.5),
                c + Vec2::new(0.0, -3.0),
            ]
        };
        p.add(egui::Shape::convex_polygon(pts, ink, Stroke::NONE));
        if resp.clicked() {
            *scroll = (*scroll + if down { 1.0 } else { -1.0 }).clamp(0.0, max_scroll);
        }
    }
    // 槽底（滑块上/下那两段）按整页翻。
    for above in [true, false] {
        if locked {
            break;
        }
        let y = if above {
            n + 16.0
        } else {
            n + 16.0 + bpos + bwh
        };
        let h = if above { bpos } else { TRACK - (bpos + bwh) };
        if h <= 0.0 {
            continue;
        }
        let rect = Rect::from_min_size(pg.row(x, y), Vec2::new(16.0, h));
        let (resp, _) = pg.hit(rect, Id::new(("midi-sb-track", n as i32, above)), false);
        if resp.clicked() {
            *scroll = (*scroll + if above { -slots } else { slots }).clamp(0.0, max_scroll);
        }
    }
    // 拖滑块：原版 `sb_val += dy / (mwh / ms)`，松手取整。
    if !locked {
        let grip = Rect::from_min_size(pg.row(x, n + 16.0 + bpos), Vec2::new(16.0, bwh));
        let resp = pg
            .ui
            .interact(grip, Id::new(("midi-sb-thumb", n as i32)), Sense::drag());
        if resp.dragged() {
            *scroll =
                (*scroll + resp.drag_delta().y / (TRACK / total as f32)).clamp(0.0, max_scroll);
        } else if resp.drag_stopped() {
            *scroll = scroll.round().clamp(0.0, max_scroll);
        }
    }
    // 滚轮（悬停在整条上就算，见函数头第 ① 条）。
    let zone = Rect::from_min_size(pg.row(x, n), Vec2::new(17.0, TRACK + 32.0));
    let hover = pg
        .ui
        .interact(zone, Id::new(("midi-sb-wheel", n as i32)), Sense::hover());
    if hover.hovered() {
        let d = pg.ui.input(|i| i.raw_scroll_delta.y);
        if d != 0.0 {
            *scroll = (*scroll - d).clamp(0.0, max_scroll);
        }
    }
}

/// 原版「乐器」页的一行：通道号、GM 乐器号与名、映射到的音色（点名字试听）、八度调整。
/// 下标 9（10 号通道）是打击乐，整行涂 rgb(120,120,120)，后三格换成固定说明。
fn midi_row_ins(pg: &mut Page, ctx: &mut MidiCtx, b: usize, a: usize) {
    use crate::state::MidiMenu;
    let cols = midi_cols(0);
    let dy = ctx.body_dy;
    let n = 174.0 + a as f32 * MIDI_ROW_H + dy;
    let n_box = 170.0 + a as f32 * MIDI_ROW_H + dy;
    let n_arrow = 172.0 + a as f32 * MIDI_ROW_H + dy;
    let ink = if b == 9 { ggray(120) } else { pg.ink() };
    pg.text(
        pg.row(midi_col_x(cols, 0), n),
        &format!("{}", b + 1),
        PREFS_FONT,
        ink,
    );
    if b == 9 {
        for (k, s) in [
            " - ",
            tr("Percussion", "打击乐"),
            tr("See \"Percussion\" tab", "请查看“打击乐”页面"),
            "-",
        ]
        .iter()
        .enumerate()
        {
            pg.text(pg.row(midi_col_x(cols, k + 1), n), s, PREFS_FONT, ink);
        }
        return;
    }
    let c = b.min(15);
    let patch = ctx.midi.channel_patch[c] as usize;
    pg.text(
        pg.row(midi_col_x(cols, 1), n),
        &format!("{}", patch + 1),
        PREFS_FONT,
        ink,
    );
    let pname = if patch < 128 {
        nbs_io::MidiGmTable::patch_name(patch as u8)
    } else {
        tr("Unknown", "未知")
    };
    pg.text(pg.row(midi_col_x(cols, 2), n), pname, PREFS_FONT, ink);
    let ins = ctx.opt.channel_ins[c];
    if ins < 0 {
        // 原版只把这一格涂成 c_gray，右边那列仍是主题色。
        pg.text(
            pg.row(midi_col_x(cols, 3), n),
            tr("Ignore", "无"),
            PREFS_FONT,
            ggray(128),
        );
    } else {
        let name = ctx
            .names
            .get(ins as usize)
            .map(|(_, s)| s.as_str())
            .unwrap_or("");
        let shown = pg.truncate(name, cols[3].0 - 20.0);
        let (resp, _) = pg.hit(
            Rect::from_min_size(
                pg.row(midi_col_end(cols, 3), n_box),
                Vec2::new(cols[3].0 - 20.0, MIDI_ROW_H),
            ),
            Id::new(("midi-ins-play", b)),
            false,
        );
        pg.text(pg.row(midi_col_x(cols, 3), n), &shown, PREFS_FONT, pg.ink());
        let clicked = resp
            .on_hover_text(tr("Click to play this sound", "点击预览声音"))
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked();
        if clicked {
            // 原版 `play_sound(instrument_list[| ins], 45 + midi_channeloctave[b] * 12, 100, 100, 0)`。
            let key = (45 + 12 * i32::from(ctx.opt.channel_octave[c])).clamp(0, 87) as u8;
            ctx.previews
                .push(nbs_core::model::Note::new(ins as u8, key));
        }
    }
    if pg.arrow(ctx.icons, midi_col_end(cols, 4) - 20.0, n_arrow) {
        ctx.open_menu(MidiMenu::ChannelIns(b));
    }
    // 八度调整：0 显示「无」，正数带 `+`（原版 `condstr(oct > 0, " + ") + string(oct)`）。
    let oct = ctx.opt.channel_octave[c];
    let label = match oct {
        0 => tr("None", "无").to_string(),
        v if v > 0 => format!(" + {v}"),
        v => format!("{v}"),
    };
    pg.text(pg.row(midi_col_x(cols, 4), n), &label, PREFS_FONT, ink);
    if pg.arrow(ctx.icons, midi_col_end(cols, 5) - 36.0, n_arrow) {
        ctx.open_menu(MidiMenu::ChannelOct(b));
    }
}

/// 原版「打击乐」页的一行：音调是原始音号，声音名查 GM 表，音色可点试听并带下拉，
/// 音高是 0..=87 的下拉。
fn midi_row_perc(pg: &mut Page, ctx: &mut MidiCtx, b: usize, a: usize) {
    use crate::state::MidiMenu;
    let cols = midi_cols(1);
    let dy = ctx.body_dy;
    let n = 174.0 + a as f32 * MIDI_ROW_H + dy;
    let note = ctx.midi.perc_notes[b];
    pg.text(
        pg.row(midi_col_x(cols, 0), n),
        &format!("{note}"),
        PREFS_FONT,
        pg.ink(),
    );
    pg.text(
        pg.row(midi_col_x(cols, 1), n),
        nbs_io::MidiGmTable::drum_name(note),
        PREFS_FONT,
        pg.ink(),
    );
    let ins = ctx.opt.perc_ins.get(b).copied().unwrap_or(-1);
    if ins < 0 {
        pg.text(
            pg.row(midi_col_x(cols, 2), n),
            tr("Ignore", "无视"),
            PREFS_FONT,
            ggray(128),
        );
    } else {
        let name = ctx
            .names
            .get(ins as usize)
            .map(|(_, s)| s.as_str())
            .unwrap_or("");
        let shown = pg.truncate(name, cols[2].0 - 20.0);
        let (resp, _) = pg.hit(
            midi_preview_rect(pg, cols, 2, a, dy),
            Id::new(("midi-perc-play", b)),
            false,
        );
        pg.text(pg.row(midi_col_x(cols, 2), n), &shown, PREFS_FONT, pg.ink());
        let clicked = resp
            .on_hover_text(tr("Click to play this sound", "点击预览声音"))
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked();
        if clicked {
            // 原版直接拿界面上设定的音高来试听：`play_sound(ins, midi_percpitch[b], 100, 100, 0)`。
            let key = ctx.opt.perc_pitch.get(b).copied().unwrap_or(0).clamp(0, 87) as u8;
            ctx.previews
                .push(nbs_core::model::Note::new(ins as u8, key));
        }
    }
    if pg.arrow(
        ctx.icons,
        midi_col_end(cols, 3) - 19.0,
        172.0 + a as f32 * MIDI_ROW_H + dy,
    ) {
        ctx.open_menu(MidiMenu::PercIns(b));
    }
    let pitch = ctx.opt.perc_pitch.get(b).copied().unwrap_or(0);
    pg.text(
        pg.row(midi_col_x(cols, 3), n),
        &key_name(pitch.clamp(0, 87) as u8, true, ctx.flats),
        PREFS_FONT,
        pg.ink(),
    );
    if pg.arrow(
        ctx.icons,
        midi_col_end(cols, 4) - 35.0,
        172.0 + a as f32 * MIDI_ROW_H + dy,
    ) {
        ctx.open_menu(MidiMenu::PercPitch(b));
    }
}

/// 原版「轨道」页的一行，三列纯文字：序号、轨道名、事件数。
fn midi_row_track(pg: &mut Page, ctx: &mut MidiCtx, b: usize, a: usize) {
    let cols = midi_cols(2);
    let n = 174.0 + a as f32 * MIDI_ROW_H + ctx.body_dy;
    let track = &ctx.midi.tracks[b];
    pg.text(
        pg.row(midi_col_x(cols, 0), n),
        &format!("{}", b + 1),
        PREFS_FONT,
        pg.ink(),
    );
    pg.text(
        pg.row(midi_col_x(cols, 1), n),
        &pg.truncate(&track.name, cols[1].0),
        PREFS_FONT,
        pg.ink(),
    );
    pg.text(
        pg.row(midi_col_x(cols, 2), n),
        &format!("{}", track.events.len()),
        PREFS_FONT,
        pg.ink(),
    );
}

/// 原版「音符长度」页的一行：轨道号（悬停出轨道名）、通道号、乐器名，最后一个是
/// 该声部要不要导入尾音的勾选框；10 号通道的格子是锁住的（打击乐按单音导入）。
fn midi_row_part(pg: &mut Page, ctx: &mut MidiCtx, b: usize, a: usize) {
    let cols = midi_cols(3);
    let dy = ctx.body_dy;
    let n = 174.0 + a as f32 * MIDI_ROW_H + dy;
    let part = &ctx.midi.parts[b];
    pg.text(
        pg.row(midi_col_x(cols, 0), n),
        &format!("{}", part.track + 1),
        PREFS_FONT,
        pg.ink(),
    );
    let name = ctx
        .midi
        .tracks
        .get(part.track)
        .map(|t| t.name.as_str())
        .unwrap_or("");
    pg.tip_area(
        12.0,
        172.0 + a as f32 * MIDI_ROW_H + dy,
        cols[0].0 - 4.0,
        18.0,
        name,
    );
    pg.text(
        pg.row(midi_col_x(cols, 1), n),
        &format!("{}", part.channel + 1),
        PREFS_FONT,
        pg.ink(),
    );
    let label = if part.channel == 9 {
        tr("Percussion", "打击乐").to_string()
    } else if (part.patch as usize) < 128 {
        format!(
            "{}. {}",
            part.patch + 1,
            nbs_io::MidiGmTable::patch_name(part.patch as u8)
        )
    } else {
        tr("Unknown", "未知").to_string()
    };
    pg.text(
        pg.row(midi_col_x(cols, 2), n),
        &pg.truncate(&label, cols[2].0),
        PREFS_FONT,
        pg.ink(),
    );
    let tip = if part.channel == 9 {
        tr(
            "Percussion is imported as single notes.",
            "打击乐以单个音符导入。",
        )
    } else {
        cols[3].2
    };
    let on = ctx
        .opt
        .part_durations
        .get(b)
        .copied()
        .unwrap_or(part.note_duration);
    if pg.check(
        midi_col_end(cols, 3) + 16.0,
        175.0 + a as f32 * MIDI_ROW_H + dy,
        on,
        "",
        tip,
        false,
        part.channel == 9,
    ) {
        if let Some(v) = ctx.opt.part_durations.get_mut(b) {
            *v = !*v;
        }
    }
}

/// 原版 `show_menu_ext` 的一行。`children` 非空就是原版用 `\` `/` 包出来的子菜单
/// （只有打击乐音高的「N 八度」用到）：悬停展开、点标题行什么都不选也不收起；
/// `shortcut` 是右对齐的那列字（原版 `x$` 前缀的 `(outside)`）；`checked` 在左边
/// 36 像素的图标槽里画一个勾（原版 `check()` 给的 `icons.CHECK`）。
struct MidiMenuItem {
    label: String,
    shortcut: &'static str,
    value: i16,
    checked: bool,
    children: Vec<MidiMenuItem>,
}

impl MidiMenuItem {
    fn item(label: impl Into<String>, value: i16, checked: bool) -> Self {
        Self {
            label: label.into(),
            shortcut: "",
            value,
            checked,
            children: Vec::new(),
        }
    }

    fn with_shortcut(mut self, s: &'static str) -> Self {
        self.shortcut = s;
        self
    }

    fn head(label: impl Into<String>, children: Vec<MidiMenuItem>) -> Self {
        Self {
            label: label.into(),
            shortcut: "",
            value: -1,
            checked: false,
            children,
        }
    }

    fn is_head(&self) -> bool {
        !self.children.is_empty()
    }
}

/// 「无」+ 整张音色表（原版遍历 `instrument_list`）。界面上这两个映射存的是 `i8`，
/// 所以下标超过 127 的自定义音色进不了菜单——导入器也只认 0..=127。
fn midi_ins_items(names: &[(u8, String)], cur: i8) -> Vec<MidiMenuItem> {
    let mut out = Vec::with_capacity(names.len().min(128) + 1);
    out.push(MidiMenuItem::item(tr("Ignore", "无"), -1, cur == -1));
    for (i, (_, name)) in names.iter().enumerate().take(128) {
        out.push(MidiMenuItem::item(name.clone(), i as i16, cur == i as i8));
    }
    out
}

/// 四个下拉各自的条目，全部照 `show_menu_ext` 传进去那串 `str` 拼。
fn midi_menu_items(
    names: &[(u8, String)],
    flats: bool,
    opt: &nbs_io::midi::MidiImportOptions,
    kind: crate::state::MidiMenu,
) -> Vec<MidiMenuItem> {
    use crate::state::MidiMenu as M;
    match kind {
        M::ChannelIns(b) => {
            midi_ins_items(names, opt.channel_ins.get(b.min(15)).copied().unwrap_or(-1))
        }
        M::PercIns(b) => midi_ins_items(names, opt.perc_ins.get(b).copied().unwrap_or(-1)),
        M::ChannelOct(b) => {
            let cur = opt.channel_octave.get(b.min(15)).copied().unwrap_or(0);
            (-3..=3i16)
                .map(|v| {
                    MidiMenuItem::item(
                        if v > 0 {
                            format!("+{v}")
                        } else {
                            v.to_string()
                        },
                        v,
                        v == i16::from(cur),
                    )
                })
                .collect()
        }
        M::PercPitch(b) => {
            let cur = i32::from(opt.perc_pitch.get(b).copied().unwrap_or(0));
            // 原版每遇到 `key ≡ 3 (mod 12)` 就 `/|N 八度|\|` 换一节子菜单，且这个判断排在
            // `key = 88` 的收尾之后——所以第 0 节只有 0..2 三个音，中间每节十二个，
            // 最后 key 87 自己单独剩一节「8 八度」。父菜单就只有这些标题行。
            let mut out = Vec::new();
            let (mut key, mut oct) = (0i32, 0i32);
            loop {
                let take = if oct == 0 { 3 } else { (88 - key).min(12) };
                let children = (0..take)
                    .map(|i| {
                        let k = key + i;
                        MidiMenuItem::item(key_name(k as u8, true, flats), k as i16, k == cur)
                            .with_shortcut(if k < 33 || k > 57 { "(outside)" } else { "" })
                    })
                    .collect();
                out.push(MidiMenuItem::head(
                    if crate::lang() == 1 {
                        format!("{oct} 八度")
                    } else {
                        format!("Octave {oct}")
                    },
                    children,
                ));
                key += take;
                if key >= 88 {
                    break;
                }
                oct += 1;
            }
            out
        }
    }
}

/// 原版菜单的锚点：`(该列左边界, y1 + 190 + 20 × 可见行号)`，
/// 可见行号 = 行号 - `floor(滚动量)`，和画那一行时用的是同一个数。
fn midi_menu_pos(kind: crate::state::MidiMenu, scroll: &[f32; 4], body_dy: f32) -> (f32, f32) {
    use crate::state::MidiMenu as M;
    let (cols, page, b, k) = match kind {
        M::ChannelIns(b) => (midi_cols(0), 0, b, 3),
        M::ChannelOct(b) => (midi_cols(0), 0, b, 4),
        M::PercIns(b) => (midi_cols(1), 1, b, 2),
        M::PercPitch(b) => (midi_cols(1), 1, b, 3),
    };
    let a = (b as f32 - scroll[page].floor()).max(0.0);
    (midi_col_end(cols, k), 190.0 + a * MIDI_ROW_H + body_dy)
}

/// 原版 `show_menu_ext` 的 `menu_wid[a]`：最长的一条（文字 + 快捷字 + 七个空格）再加 56。
fn midi_menu_wid(ui: &Ui, th: &Theme, items: &[MidiMenuItem]) -> f32 {
    items
        .iter()
        .map(|it| {
            let s = if it.shortcut.is_empty() {
                it.label.clone()
            } else {
                format!("{}{}       ", it.label, it.shortcut)
            };
            ui.painter()
                .layout_no_wrap(s, FontId::proportional(PREFS_FONT), th.text)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        + 56.0
}

/// 原版 `menu_draw` 的 `black_text`：选中底色按 sRGB 线性化求亮度，浅色 Fluent 比的是
/// 「跟黑比划算还是跟白比划算」，深色 Fluent 直接看亮度有没有过半；非 Fluent 恒为白。
fn menu_sel_text(th: &Theme, bg: Color32) -> Color32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let lum = 0.2126 * lin(bg.r()) + 0.7152 * lin(bg.g()) + 0.0722 * lin(bg.b());
    let black = if th.fdark {
        lum >= 0.5
    } else {
        (lum + 0.05) / 0.05 >= 1.05 / (lum + 0.05)
    };
    if black {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// 原版 `menu_draw` 里那块板：底色非 Fluent 是 `window_background`，Fluent 浅色
/// rgb(249,249,249)、深色 rgb(41,41,41)（`liquid_glass_settings("menu")` 的 tint），
/// Fluent 圆角 16；描边一律 `draw_theme_color()`（= 主题文字色）叠 25%。非 Fluent 还在
/// `+29/+30` 画一条图标槽分隔线（25% 主题色压一条、白描一条）。原版的投影
/// `spr_shadowext` 和 Fluent 的亚克力模糊都没有，只铺平色。
fn midi_menu_board(p: &egui::Painter, th: &Theme, rect: Rect) {
    let fluent = th.kind == ThemeKind::Fluent;
    let radius = if fluent { 16.0 } else { 0.0 };
    let fill = if fluent {
        if th.fdark {
            ggray(41)
        } else {
            ggray(249)
        }
    } else {
        th.window_bg
    };
    let dim = th.text.linear_multiply(0.25);
    p.rect_filled(rect, radius, fill);
    p.rect_stroke(rect, radius, Stroke::new(1.0_f32, dim), StrokeKind::Inside);
    if !fluent {
        for (dx, col) in [(29.0, dim), (30.0, Color32::WHITE)] {
            p.line_segment(
                [
                    egui::pos2(rect.min.x + dx, rect.min.y + 3.0),
                    egui::pos2(rect.min.x + dx, rect.max.y - 3.0),
                ],
                Stroke::new(1.0_f32, col),
            );
        }
    }
}

/// 画一层菜单的所有行并处理命中。度量全按原版：命中区 `(x + 3, 行顶)`、宽 `wid - 5`、
/// 高 22；正文左边 `+36`（`check()` 的勾在 `+2` 的图标槽里），快捷字右对齐在
/// `wid - 20`，子菜单箭头在 `wid - 24`。
fn midi_menu_rows(
    ui: &Ui,
    th: &Theme,
    bank: Option<&crate::icons::Bank>,
    id: &str,
    org: Pos2,
    wid: f32,
    items: &[MidiMenuItem],
    open: &mut Option<usize>,
) -> Option<i16> {
    let p = ui.painter().clone();
    let fluent = th.kind == ThemeKind::Fluent;
    // 原版选中底色：Fluent 用 `accent[5]`，其余是 `16684072` = rgb(40,148,254)。
    let sel_bg = if fluent {
        th.accent(5)
    } else {
        Color32::from_rgb(40, 148, 254)
    };
    let sel_text = if fluent {
        menu_sel_text(th, sel_bg)
    } else {
        Color32::WHITE
    };
    let mut picked = None;
    for (i, it) in items.iter().enumerate() {
        let top = org.y + 3.0 + i as f32 * 22.0;
        let hit = Rect::from_min_size(egui::pos2(org.x + 3.0, top), Vec2::new(wid - 5.0, 22.0));
        let resp = ui.interact(hit, Id::new((id, i)), Sense::click());
        if resp.hovered() {
            // 原版：悬停到哪一行就先把更深的菜单收掉，那一行是标题再把它展开。
            *open = it.is_head().then_some(i);
        }
        if resp.clicked() && !it.is_head() {
            picked = Some(it.value);
        }
        let sel = resp.hovered();
        if sel {
            p.rect_filled(hit, if fluent { 8.0 } else { 0.0 }, sel_bg);
        }
        let ink = if sel { sel_text } else { th.text };
        // 原版 `menu_draw` 的图标层：勾在非 Fluent 完全不染色（精灵自带黑白，Dark 主题另取
        // 白字那帧），Fluent 是 `draw_sprite_ext(..., 选中 ? 选中文字色 : c_white)` 再叠一层
        // `spr_icons_col` 的 `accent[6 - 2 * !fdark]`（选中时这层也换成选中文字色）。
        let check_tint = if fluent && sel {
            sel_text
        } else {
            Color32::WHITE
        };
        let accent = fluent.then(|| if sel { sel_text } else { th.accent_icon() });
        if it.checked {
            if let Some(bank) = bank {
                let frame = if th.kind == ThemeKind::Dark {
                    crate::icons::frame::CHECK_DARK
                } else {
                    crate::icons::frame::CHECK
                };
                bank.menu_icon(
                    &p,
                    egui::pos2(org.x + 2.0, top - 1.0),
                    frame,
                    check_tint,
                    accent,
                    th,
                );
            } else {
                // 精灵资源缺失：位置和形状照旧，只是不拿贴图冒充。
                let b = egui::pos2(org.x + 5.0, top + 3.0);
                let s = Stroke::new(1.5_f32, ink);
                p.line_segment([b, b + Vec2::new(2.5, 3.0)], s);
                p.line_segment([b + Vec2::new(2.5, 3.0), b + Vec2::new(7.0, -2.0)], s);
            }
        }
        let g = p.layout_no_wrap(it.label.clone(), FontId::proportional(PREFS_FONT), ink);
        p.galley(egui::pos2(org.x + 36.0, top + 5.0), g, Color32::TRANSPARENT);
        if !it.shortcut.is_empty() {
            // 原版 `x$`：右对齐在 `wid - 20`，颜色和正文一样。
            let s = p.layout_no_wrap(
                it.shortcut.to_string(),
                FontId::proportional(PREFS_FONT),
                ink,
            );
            p.galley(
                egui::pos2(org.x + wid - 20.0 - s.size().x, top + 5.0),
                s,
                Color32::TRANSPARENT,
            );
        }
        if it.is_head() {
            // 原版 `icons.SUB_MENU` 画在 `(wid - 24, 行顶 - 1)`：非 Fluent 的箭头是白字蒙版，
            // 颜色 `theme = 2 ? (选中 ? 黑 : 白) : (选中 ? 白 : 黑)`；Fluent 未选中用
            // `c_gray` / `c_ltgray` 打底，再叠强调色。
            let at = egui::pos2(org.x + wid - 24.0, top - 1.0);
            let col = if fluent {
                if sel {
                    sel_text
                } else if th.fdark {
                    Color32::from_gray(211)
                } else {
                    Color32::from_gray(128)
                }
            } else if th.kind == ThemeKind::Dark {
                if sel {
                    Color32::BLACK
                } else {
                    Color32::WHITE
                }
            } else if sel {
                Color32::WHITE
            } else {
                Color32::BLACK
            };
            match bank {
                Some(bank) => {
                    bank.menu_icon(&p, at, crate::icons::frame::SUB_MENU, col, accent, th)
                }
                None => {
                    let c = at + Vec2::splat(12.5);
                    p.add(egui::Shape::convex_polygon(
                        vec![
                            c + Vec2::new(-2.5, -4.0),
                            c + Vec2::new(-2.5, 4.0),
                            c + Vec2::new(2.5, 0.0),
                        ],
                        col,
                        Stroke::NONE,
                    ));
                }
            }
        }
    }
    picked
}

/// 原版 `show_menu_ext(name, x, y, str)`：先铺一层吞掉全屏输入的遮罩（点哪儿都只用来
/// 收起菜单），再从锚点展开一列条目；板子高 = `Σ行高 + 6`（上下各留 3 像素）。
/// 位置也按原版：横向放不下就往锚点左边开，再按「父 + 子」的总跨度往回挪，纵向超出屏幕
/// 就整块上移。原版没有内滚动——菜单比屏幕还高时上半截直接画到屏幕外，这里只多做一个
/// 「贴住屏幕顶」的兜底。`open` 记着展开的是第几行标题，对应原版 `menu_show[]`：鼠标停在
/// 子菜单上时父行不算悬停，所以子菜单不会一闪一闪。
/// 返回（选中项的值，是否被点掉）。
fn midi_popup(
    ui: &Ui,
    id: &str,
    at: Pos2,
    items: &[MidiMenuItem],
    open: &mut Option<usize>,
    th: &Theme,
    bank: Option<&crate::icons::Bank>,
) -> (Option<i16>, bool) {
    let ctx = ui.ctx();
    let screen = ctx.screen_rect();
    let dismissed = egui::Area::new(Id::new((id, "blocker")))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::ZERO)
        .show(ctx, |ui| ui.allocate_rect(screen, Sense::click()).clicked())
        .inner;

    let wid = midi_menu_wid(ui, th, items);
    let hei = items.len() as f32 * 22.0 + 6.0;
    let span = items
        .iter()
        .filter(|it| it.is_head())
        .map(|it| wid + midi_menu_wid(ui, th, &it.children) - 3.0)
        .fold(wid, f32::max);
    // 原版 `rootx = clamp(sx - (sx + menu_wid[0] > rw) * menu_wid[0], 0, rw - menu_wid[0])`。
    let flip = if at.x + wid > screen.max.x {
        at.x - wid
    } else {
        at.x
    };
    let mut x = flip.clamp(0.0, (screen.max.x - wid).max(0.0));
    if span <= screen.width() {
        let right = x.min(screen.max.x - span);
        let left = x.max(span - wid);
        if (right - x).abs() <= (left - x).abs() {
            x = right
        } else {
            x = left
        }
    }
    let mut y = at.y;
    if y + hei > screen.max.y {
        y = screen.max.y - hei;
    }
    let pos = Pos2::new(x, y.max(0.0));

    let mut picked = None;
    egui::Area::new(Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(wid, hei), Sense::hover());
            midi_menu_board(ui.painter(), th, rect);
            picked = midi_menu_rows(ui, th, bank, id, rect.min, wid, items, open);
        });
    // 子菜单：原版 `menux = dx + menu_wid - 3`、`menuy = dy + 22 × 行号`，也就是子菜单第一行
    // 正好压在被悬停的那一行上。深层只按 `dx = clamp(dx, 0, rw - wid)` 收进屏幕。
    let head = open.filter(|i| items.get(*i).is_some_and(|it| it.is_head()));
    if let Some(i) = head {
        let kids = &items[i].children;
        let kw = midi_menu_wid(ui, th, kids);
        let kh = kids.len() as f32 * 22.0 + 6.0;
        let mut k = Pos2::new(pos.x + wid - 3.0, pos.y + i as f32 * 22.0);
        k.x = k.x.clamp(0.0, (screen.max.x - kw).max(0.0));
        if k.y + kh > screen.max.y {
            k.y = (screen.max.y - kh).max(0.0);
        }
        egui::Area::new(Id::new((id, "sub")))
            .order(egui::Order::Foreground)
            .fixed_pos(k)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(kw, kh), Sense::hover());
                midi_menu_board(ui.painter(), th, rect);
                // 子菜单下面没有第三层，命中状态用本地的，别把父层的 `open` 冲掉。
                let mut deeper = None;
                // 行号要和父菜单错开，否则同一帧里同一个 widget 出现在两个图层，egui 会直接 panic。
                picked = midi_menu_rows(
                    ui,
                    th,
                    bank,
                    &format!("{id}/sub"),
                    rect.min,
                    kw,
                    kids,
                    &mut deeper,
                );
            });
    }
    (picked, dismissed)
}

/// 原版 `draw_window_schematic_export`：设计 / 方块两页 + 记住我的更改 + 三个底部按钮。
fn schematic_export_window(state: &mut EditorState, ui: &mut Ui) {
    let mut tab = state.schematic_tab;
    ui.horizontal(|ui| {
        for (i, name) in [tr("Design", "设计"), tr("Blocks", "方块")]
            .iter()
            .enumerate()
        {
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
        ui.checkbox(
            &mut state.schematic_remember,
            tr("Remember changes", "记住我的更改"),
        )
        .on_hover_text(tr(
            "Whether to use these settings the\nnext time you export a Schematic.",
            "下次导出 Schematic 时是否使用同样的设定。",
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("Use default", "使用默认值")).clicked() {
                state.modals = ModalQueue::question(
                    tr("Confirm", "确定"),
                    tr("Are you sure?", "你确定吗？"),
                    Command::ResetSchematicOptions,
                );
            }
            if ui.button(tr("Cancel", "取消")).clicked() {
                state.emit(Command::CloseWindow);
            }
            if ui.button(tr("Export", "导出")).clicked() {
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
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    for sec in sections {
                        if let Some(t) = sec.title {
                            ui.label(RichText::new(t.to_string()).small().strong());
                        }
                        for m in &sec.items {
                            let text = format!(
                                "{}, {}  {}",
                                m.id,
                                m.data,
                                nbs_io::schematic::block_display_name(m.id, m.data)
                            );
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

fn design_tab(
    o: &mut nbs_io::schematic::SchematicOptions,
    st: &nbs_io::schematic::SchematicStats,
    ui: &mut Ui,
) {
    use nbs_io::schematic::{calc_size, export_guard};
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label(tr("Layout:", "分布:"));
            if ui.radio(o.layout == 1, tr("Simple walkway", "普通过道")).on_hover_text(tr("Generate a simple walkway that stretches\nas far as the length of the song.", "生成过道延伸到歌曲末尾。")).clicked() {
                o.layout = 1;
            }
            if ui.radio(o.layout == 0, tr("Circular walkway", "环形过道")).on_hover_text(tr("Generate a walkway where the\nplayer travels back and forth.", "生成让玩家来回走的过道。")).clicked() {
                o.layout = 0;
            }
            ui.label(tr("Export as Minecraft version:", "导出为 Minecraft 版本:"));
            if ui.radio(o.structure, "1.13+").on_hover_text(tr("Create a Structure Block file compatible with 1.13+.", "创建一个兼容 1.13+ 的结构方块文件。")).clicked() {
                o.structure = true;
            }
            if ui
                .radio(!o.structure && !o.minecraft_old, "1.11-1.12")
                .on_hover_text(tr("Create a Schematic file compatible with 1.11 and 1.12.", "创建一个兼容 1.11 和 1.12 的 Schematic 文件。"))
                .clicked()
            {
                o.minecraft_old = false;
                o.structure = false;
            }
            if ui
                .radio(!o.structure && o.minecraft_old, "pre 1.11")
                .on_hover_text(tr("Create a Schematic file only compatible with versions before 1.11.", "创建一个只兼容 1.11 之前旧版本的 Schematic 文件。"))
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
                ui.label(tr("Repeaters per row:", "每行中继器个数:"));
                if ui.add(egui::DragValue::new(&mut n).speed(1.0).range(5.0..=100.0)).changed() {
                    o.set_notesperrow(n as usize);
                }
            });
            ui.checkbox(&mut o.include_locked, tr("Include locked layers", "包括已静音的层"))
                .on_hover_text(tr("Whether to include locked layers in the Schematic.", "是否在 Schematic 内包括已静音的层。"));
            ui.checkbox(&mut o.compress, tr("Compress layers", "压缩每层")).on_hover_text(tr("Compress layers to save vertical space.", "压缩每层以节省竖向空间。"));
            if o.layout < 2 {
                ui.checkbox(&mut o.minecart, tr("Include minecart track", "包括矿车轨道"))
                    .on_hover_text(tr("Include a minecart track that\nautomatically goes along with the song.", "包括一个跟随歌曲进度的矿车轨道。"));
                ui.indent("sch_chest", |ui| {
                    ui.add_enabled_ui(o.minecart, |ui| {
                        ui.checkbox(&mut o.chest, tr("Add chest with minecarts", "添加矿车箱"))
                            .on_hover_text(tr("Whether to add a chest full of minecarts\nat the beginning of the song.", "是否在歌曲开始处添加一个装满矿车的箱子。"));
                    });
                });
                if o.layout == 0 {
                    ui.checkbox(&mut o.loop_option, tr("Include looping option", "包括循环选项"))
                        .on_hover_text(tr("Whether to add a lever that toggles looping.", "是否添加开关循环功能的拉杆。"));
                }
                if o.structure {
                    ui.checkbox(&mut o.command_block, tr("Use command blocks", "使用命令方块"))
                        .on_hover_text(tr("Whether to use command blocks instead of note blocks for a wider octave range.\n(Extra notes pack required)", "是否使用命令方块代替音符盒以获得更广音域。\n（需要更多音符资源包）"));
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
                ui.label(tr("Note blocks:", "音符盒:"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(st.totalblocks[incl].to_string());
                });
                ui.end_row();
                ui.label(tr("Repeaters:", "中继器:"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}x{}", st.repeaters[incl][o.tempo_mode as usize], size.layers));
                });
                ui.end_row();
                ui.label(tr("Size:", "大小:"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}x{}x{}", size.len, size.wid, size.hei));
                });
                ui.end_row();
            });
            ui.label(tr("Tempo:", "速度:"));
            let tempos = [
                (tr("10 ticks / second", "10 红石刻 / 秒"), tr("Generate song at 10 ticks / second", "生成一个 10 红石刻 / 秒 的歌曲")),
                (tr("5 ticks / second", "5 红石刻 / 秒"), tr("Generate song at 5 ticks / second", "生成一个 5 红石刻 / 秒 的歌曲")),
                (tr("2.5 ticks / second", "2.5 红石刻 / 秒"), tr("Generate song at 2.5 ticks / second", "生成一个 2.5 红石刻 / 秒 的歌曲")),
            ];
            for (i, (name, tip)) in tempos.iter().enumerate() {
                if ui.radio(o.tempo_mode == i as u8, *name).on_hover_text(*tip).clicked() {
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

fn blocks_tab(o: &mut nbs_io::schematic::SchematicOptions, names: &[(u8, String)], ui: &mut Ui) {
    use nbs_io::schematic::{
        block_display_name, instrument_block_menu, misc_block_menu, McBlock, INS_COUNT,
    };
    let rows = names.len().min(INS_COUNT);
    let mut pick_ins: Option<(usize, McBlock)> = None;
    egui::Grid::new("sch_ins_table")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong(tr("Instrument", "音色"));
            ui.strong(tr("Block", "方块"));
            ui.strong(tr("Block name", "方块名称"));
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
        (tr("Block for walkway:", "过道所用方块:"), &mut o.walkway, 0u8, tr("The block that should be used for the walkway, \nand everything else that isn't circuitry or ground.", "用于过道和除电路和地面的方块。")),
        (tr("Block for circuitry:", "电路所用方块:"), &mut o.circuit, 1, tr("The block that should be used for the circuitry.", "为电路用的方块。")),
        (tr("Block for ground:", "地面所用方块:"), &mut o.ground, 2, tr("The block that should be used for the ground.\nChoosing grass will result with a bunch of animals spawning.", "为地面用的方块。选草方块会刷新很多生物。")),
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
            ui.label(
                RichText::new(format!("{}, {}", cur.id, cur.data))
                    .small()
                    .weak(),
            );
        });
        ui.label(RichText::new(tip).small().weak());
    }
}

/// 原版 `minecraft_export_source_label` + `draw_window_datapack_export` 声源区的双语。
/// EN 按原版直接显示命令内部值；CN 用 Minecraft 官方简中分类名。
/// 元组第三项写进命令的必须是英文内部值，界面标签只作显示。
fn sound_sources() -> [(&'static str, &'static str, &'static str); 10] {
    [
        (
            "ambient",
            tr("ambient", "环境"),
            tr("Controlled by Ambient/Environment slider", "由环境音量控制"),
        ),
        (
            "block",
            tr("block", "方块"),
            tr("Controlled by Blocks slider", "由方块音量控制"),
        ),
        (
            "hostile",
            tr("hostile", "敌对生物"),
            tr(
                "Controlled by Hostile Creatures slider",
                "由敌对生物音量控制",
            ),
        ),
        (
            "master",
            tr("master", "主音量"),
            tr("Controlled by Master Volume slider", "由主音量控制"),
        ),
        (
            "music",
            tr("music", "音乐"),
            tr("Controlled by Music slider", "由音乐音量控制"),
        ),
        (
            "neutral",
            tr("neutral", "友好生物"),
            tr(
                "Controlled by Friendly Creatures slider",
                "由友好生物音量控制",
            ),
        ),
        (
            "player",
            tr("player", "玩家"),
            tr("Controlled by Players slider", "由玩家音量控制"),
        ),
        (
            "record",
            tr("record", "唱片机/音符盒"),
            tr(
                "Controlled by Jukebox/Note Blocks slider",
                "由唱片机/音符盒音量控制",
            ),
        ),
        (
            "voice",
            tr("voice", "复述功能/语音"),
            tr(
                "Controlled by Voice/Speech slider",
                "由复述功能/语音音量控制",
            ),
        ),
        (
            "weather",
            tr("weather", "天气"),
            tr("Controlled by Weather slider", "由天气音量控制"),
        ),
    ]
}

/// 与 `VisType::ALL` 同序的双语标签。
fn vis_types() -> [(&'static str, &'static str); 7] {
    [
        (
            tr("Arc", "弧形"),
            tr("Use Arc visualizer.", "使用弧形效果器。"),
        ),
        (
            tr("Fall", "下落"),
            tr("Use Fall visualizer.", "使用下落效果器。"),
        ),
        (
            tr("Rise", "上升"),
            tr("Use Rise visualizer.", "使用上升效果器。"),
        ),
        (
            tr("Bounce", "弹跳"),
            tr("Use Bounce visualizer.", "使用弹跳效果器。"),
        ),
        (
            tr("Piano Roll", "钢琴"),
            tr("Use Piano Roll visualizer.", "使用钢琴式效果器。"),
        ),
        (
            tr("Fountain", "喷泉"),
            tr("Use Fountain visualizer.", "使用喷泉效果器。"),
        ),
        (
            tr("Rittai Onkyou", "立体"),
            tr("Use Rittai Onkyou visualizer.", "使用立体音效效果器。"),
        ),
    ]
}

fn datapack_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (i, tab) in [tr("Settings", "设置"), tr("Visualizer", "效果")]
            .iter()
            .enumerate()
        {
            if ui
                .selectable_label(state.datapack_tab == i as u8, *tab)
                .clicked()
            {
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
    let mut save_pack = false;
    if tab == 0 {
        egui::Grid::new("datapack_grid").num_columns(2).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(tr("Unique name:", "唯一名称:"));
                ui.add(
                    egui::TextEdit::singleline(&mut o.name)
                        .desired_width(145.0)
                        .hint_text("<enter name>"),
                )
                .on_hover_text(tr("This name will be used in the command\nfor playing the song inside the game.", "该名称会作为在游戏里播放使用的命令名称。"));
                ui.label(tr("Namespace:", "命名空间:"));
                ui.add(
                    egui::TextEdit::singleline(&mut o.namespace)
                        .desired_width(145.0)
                        .hint_text(tr("(optional)", "(可选)")),
                )
                .on_hover_text(tr("(optional) Use this to place the functions under a custom namespace.\nIf empty, namespace will be the name of the song.", "（可选）将命令名称更改到自定义命名空间下。如为空，命名空间会设为歌曲名称。"));
                ui.label(tr("Path:", "路径:"));
                ui.add(
                    egui::TextEdit::singleline(&mut o.path)
                        .desired_width(145.0)
                        .hint_text(tr("(optional)", "(可选)")),
                )
                .on_hover_text(tr("(optional) Path to the song from the main function\nfolder. You can use '/' to add subfolders.", "（可选）从“functions”文件夹到歌曲的路径。用“/”来套文件夹。"));
                ui.label(tr("Command preview:", "命令预览:"));
                let preview = nbs_io::datapack::command_preview(&o.name, &o.namespace, &o.path);
                ui.add(
                    egui::Label::new(RichText::new(preview).monospace().strong())
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Truncate),
                );
                ui.label(tr("Sound source", "声音类型"));
                let sources = sound_sources();
                for (id, zh, tip) in &sources[..5] {
                    if ui.radio(o.source == *id, *zh).on_hover_text(*tip).clicked() {
                        o.source = (*id).to_string();
                    }
                }
                ui.label(tr("Minecraft version", "Minecraft 版本"));
                if ui
                    .radio(o.mc_version == 0, "1.13-1.20")
                    .on_hover_text(tr("Export to Minecraft: Java Edition\nversions 1.13 to 1.20.6.", "导出至 Minecraft: Java 版\n版本 1.13 至 1.20.6。"))
                    .clicked()
                {
                    o.mc_version = 0;
                }
                if ui
                    .radio(o.mc_version == 1, "1.21+")
                    .on_hover_text(tr("Export to Minecraft: Java Edition\nversions 1.21 and above.", "导出至 Minecraft: Java 版\n版本 1.21 及以上。"))
                    .clicked()
                {
                    o.mc_version = 1;
                }
                ui.checkbox(&mut o.use_zip, tr("Export as ZIP", "导出为 ZIP"))
                    .on_hover_text(tr("Whether to export the data pack as a ZIP file. If unchecked, it will be saved as a folder instead.", "是否导出为 ZIP 格式的文件。如关闭，将导出为文件夹。"));
            });
            ui.vertical(|ui| {
                ui.label(" ");
                ui.label(" ");
                ui.label(" ");
                ui.checkbox(&mut o.include_locked, tr("Include locked layers", "包括已静音的层"))
                    .on_hover_text(tr("Whether to include locked layers in the data pack.", "是否在数据包内包括已静音的层。"));
                ui.checkbox(&mut o.include_out_of_range, tr("Include out-of-range notes", "包括超范围音符"))
                    .on_hover_text(tr("Whether to include notes that don't fall into the 2 octave range supported by Minecraft. This will require an additional resource pack you can get below.", "是否包括超过 Minecraft 的 2 八度限制的音符。此选项需要安装额外的数据包。"));
                ui.checkbox(&mut o.enable_radius, tr("Nearby listening", "共享音乐"))
                    .on_hover_text(tr("Whether to let all players in a given radius hear the music as well.", "是否共享音乐给一定范围内的玩家。"));
                ui.horizontal(|ui| {
                    ui.label(tr("Radius (in blocks):", "范围（方块）:"));
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
                let radius_line = if crate::lang() == 1 {
                    format!("实际音量半径 = {:.2}", o.radius_value())
                } else {
                    format!("Effective volume radius = {:.2}", o.radius_value())
                };
                ui.label(RichText::new(radius_line).small().weak());
                ui.checkbox(&mut o.enable_looping, tr("Enable looping", "启用循环"))
                    .on_hover_text(tr("If enabled, the song will loop at the end of playback instead of stopping.", "如果开启，歌曲播放到最后将会循环。"));
                ui.horizontal(|ui| {
                    ui.label(tr("Loop start:", "循环开始:"));
                    ui.add_enabled_ui(o.enable_looping, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut loop_start)
                                .range(0.0..=enda)
                                .speed(0.5),
                        )
                        .on_hover_text(tr("Tick the song will jump to at the end of playback", "播放完循环时回到的时刻。"));
                    });
                });
                // 原版 `draw_button2(x1 + 380, y1 + 362, 152, "Get extra notes pack", !dat_includeoutofrange, 1)`：
                // 只有勾了「包括超范围音符」才解锁，点了就是 `datapack_getextranotes()`。
                if ui
                    .add_enabled(o.include_out_of_range, egui::Button::new(tr("Get extra notes pack", "保存更多音符资源包")))
                    .on_hover_text(tr("Save the extra notes resource pack for the data pack (pitches beyond the two octaves).", "保存数据包用的额外音符资源包（超出两个八度的那些音高）。"))
                    .clicked()
                {
                    save_pack = true;
                }
            });
            ui.end_row();
        });
    } else {
        ui.checkbox(&mut o.visualizer, tr("Enable visualizer", "启用效果器"))
            .on_hover_text(tr(
                "NOTE: Please use a VOID world as falling blocks will pile up!",
                "注意：请使用一个虚空地图不然方块就会堆起来！",
            ));
        ui.label(tr("Visualizer type", "效果类型"));
        for (vis, (zh, tip)) in nbs_io::datapack::VisType::ALL.iter().zip(vis_types()) {
            if ui
                .radio(o.vis_type == *vis, zh)
                .on_hover_text(tip)
                .clicked()
            {
                o.vis_type = *vis;
            }
        }
        ui.checkbox(&mut o.glow, tr("Add glow", "发光效果"))
            .on_hover_text(tr(
                "Whether to add a glowing effect to each note block.",
                "是否为每个音符盒添加发光效果。",
            ));
        ui.label(tr("Spawn height:", "生成高度:"));
        ui.add(
            egui::DragValue::new(&mut o.y_val)
                .range(0.0..=255.0)
                .speed(0.5),
        );
        ui.label(tr("Position in map:", "地图中位置:"));
        ui.horizontal(|ui| {
            ui.label(tr("X value:", "X值:"));
            ui.add(egui::DragValue::new(&mut o.x_val).speed(0.5));
        });
        ui.horizontal(|ui| {
            ui.label(tr("Z value:", "Z值:"));
            ui.add(egui::DragValue::new(&mut o.z_val).speed(0.5));
        });
        if o.visualizer {
            let line = if crate::lang() == 1 {
                format!(
                    "效果器类型：{}，音符盒生成在 y = {}",
                    o.vis_type.gml_name(),
                    o.y_val
                )
            } else {
                format!(
                    "Visualizer type: {}, note blocks spawn at y = {}",
                    o.vis_type.gml_name(),
                    o.y_val
                )
            };
            ui.label(RichText::new(line).small().weak());
        }
    }

    if loop_start != state.doc.song.meta.loop_start as f32 {
        state
            .doc
            .edit_meta(tr("Set loop start", "设置循环起点"), |m| {
                m.loop_start = loop_start as u16
            });
        state.sync_loop_params();
    }
    if save_pack {
        state.emit(Command::SaveExtraNotesPack);
    }

    if let Some(s) = &state.export_status {
        ui.separator();
        ui.label(RichText::new(s).small());
    }
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(tr("Export", "导出")).clicked() {
            state.emit(Command::OpenDialog(
                crate::state::DialogAction::ExportDatapack,
            ));
        }
        if ui.button(tr("Use default", "使用默认值")).clicked() {
            // 原版是 dat_reset(1) + 一个确认框；这里直接重置并给出提示。
            state.datapack_options = nbs_io::datapack::DatapackOptions::default();
            state.export_status = None;
            state.set_message(tr(
                "Data pack export defaults restored",
                "已恢复数据包导出默认设置",
            ));
        }
        if ui.button(tr("Close", "关闭")).clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn png_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(tr(
        "Renders the whole song grid as a PNG image; colors map to instruments, brightness to velocity.",
        "把整首歌的网格画成 PNG 图片，颜色对应乐器，亮度对应力度。",
    ));
    ui.label(RichText::new(tr(
        "Cells are scaled down to fit the maximum pixel size; not limited by the screen viewport.",
        "自动缩小格子以适配最大像素尺寸；不受屏幕视口限制。",
    )).small().weak());
    ui.separator();
    if let Some(s) = &state.export_status {
        ui.label(RichText::new(s).small().weak());
    }
    if ui.button(tr("Export…", "选择输出路径并导出…")).clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ExportPng));
    }
    footer(state, ui);
}

fn autosaves_window(state: &mut EditorState, ui: &mut Ui) {
    let files = nbs_io::prefs::list_autosaves();
    ui.label(
        RichText::new(tr(
            &format!(
                "Backup folder: {}",
                nbs_io::prefs::Preferences::backup_dir().display()
            ),
            &format!(
                "备份目录：{}",
                nbs_io::prefs::Preferences::backup_dir().display()
            ),
        ))
        .small()
        .weak(),
    );
    ui.separator();
    if files.is_empty() {
        ui.label(tr("No autosaves yet", "暂无自动备份"));
    }
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .show(ui, |ui| {
            for p in files {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if ui.button(&name).clicked() {
                    state.emit(Command::RestoreAutosave(p.clone()));
                }
            }
        });
    ui.separator();
    ui.add(
        egui::Slider::new(&mut state.prefs.auto_save_minutes, 0..=60).text(tr(
            "Autosave interval (minutes, 0 = off)",
            "自动保存间隔 (分钟，0=关闭)",
        )),
    );
    footer(state, ui);
}

/// 原版每行开头的 `spr_yesno`：画在 `(x1 + 25, yy + 8)`，精灵原点在中心。
/// 0 = 红叉，1 = 绿勾，2 = 橙勾。精灵资源缺失时退化成同一位置的 ✓ / ✗ / !，
/// 位置和判据都不变，只是不拿贴图冒充。
fn mc_status_icon(pg: &Page, bank: Option<&crate::icons::Bank>, n: f32, state: usize) {
    let center = pg.row(25.0, n + 8.0);
    let p = pg.ui.painter();
    let Some(bank) = bank else {
        let (ch, color) = match state {
            0 => ("✗", Color32::from_rgb(255, 0, 0)),
            1 => ("✓", Color32::from_rgb(0, 176, 0)),
            _ => ("!", Color32::from_rgb(255, 140, 0)),
        };
        pg.text(center - Vec2::splat(5.0), ch, PREFS_FONT_MED, color);
        return;
    };
    bank.yesno(&p, center, state, pg.th);
}

/// 原版 `draw_window_minecraft`：右上角兼容性徽章点开的检查窗口，490×450，两页
/// （Schematic / 数据包）。坐标全部照抄原版的 `x1 + …` / `yy + …`：页签在 `y1 + 78`，
/// 三行检查分别在 `y1 + 115 / 205 / 295`，确定在 `(x1 + 240 - 36, y1 + 413)`。
fn minecraft_compat_window(state: &mut EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let fluent = th.kind == ThemeKind::Fluent;
    let margin = ui.spacing().window_margin;
    let (ml, mr) = (margin.left as f32, margin.right as f32);
    // 和首选项窗口同一套换算：原版 `draw_window` 的 (x1, y1) 对应这里的行坐标 0。
    let body = ui.allocate_exact_size(egui::vec2(490.0 - ml - mr, 450.0 - 38.0), Sense::hover());
    let o = body.0.min - egui::vec2(ml, 0.0);
    let ry = if fluent { -8.0 } else { -26.0 };

    let tempo = state.doc.song.meta.tempo;
    let (outside, custom) = state.mc_stats();
    let bank = state.icons.bank();

    // 原版 `selected_tab_mc += keyboard_check_pressed(vk_right) - (vk_left)`，出界绕另一头。
    let (left, right) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),
        )
    });
    if left || right {
        let step = right as i32 - left as i32;
        state.mc_tab = (state.mc_tab as i32 + step).rem_euclid(2) as u8;
    }
    let tab = state.mc_tab.min(1);
    let font = if fluent { PREFS_FONT_MED } else { PREFS_FONT };
    // 原版三档文字颜色：通过 = c_green（主题 2 和 Fluent 深色换成 c_lime）、
    // 不通过 = c_red、能用但不理想 = c_orange。
    let ink = th.text;
    let green = if th.kind == ThemeKind::Dark || (fluent && th.fdark) {
        Color32::from_rgb(0, 255, 0)
    } else {
        Color32::from_rgb(0, 176, 0)
    };
    let red = Color32::from_rgb(255, 0, 0);
    let orange = Color32::from_rgb(255, 140, 0);

    let mut pg = Page {
        ui: &mut *ui,
        o,
        ry,
        th: &th,
        fluent,
    };

    // 原版 Fluent 只有一块盖住标题区的灰卡（`draw_roundrect(x1+1, y1+1, x1+488, y1+98, 0)`），
    // 内容区不再画面板框；其余主题反过来：灰卡不画，`yy + 46 .. yy + 350` 那一块画白底 +
    // (137,140,149) 描边（主题 2 铺 c_dark）。这块必须在文字之前画——原版就是先画卡再画标题。
    if fluent {
        let card = Rect::from_min_max(pg.at(1.0, 0.0), pg.row(488.0, 98.0));
        let fill = if th.fdark { ggray(51) } else { ggray(204) };
        pg.ui.painter().rect_filled(card, 0.0, fill);
    } else {
        let panel = Rect::from_min_max(pg.row(6.0, 96.0), pg.row(484.0, 400.0));
        let fill = if th.kind == ThemeKind::Dark {
            th.window_bg
        } else {
            Color32::WHITE
        };
        let p = pg.ui.painter();
        p.rect_filled(panel, 0.0, fill);
        p.rect_stroke(
            panel,
            0.0,
            Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
            StrokeKind::Inside,
        );
    }

    // 原版在窗口标题下面还有一行说明（`draw_text_dynamic(x1 + 16, y1 + 32, …)`）。
    pg.text(
        pg.row(16.0, 32.0),
        tr(
            "Due to the limitations of note blocks, the song must meet certain criteria in order\nto be properly imported into Minecraft.",
            "由于音符盒的种种限制，歌曲需要满足特定条件才能导入进 Minecraft。",
        ),
        font,
        ink,
    );

    // —— 页签：原版 `str[0] = "Schematic"` / `str[1] = "Data Pack"`（中文页签同名 "Schematic"），
    //    从 x1 + 8 起排。——
    let labels = [tr("Schematic", "Schematic"), tr("Data Pack", "数据包")];
    let mut b = 8.0_f32;
    let mut nsel = tab;
    let mut stab = (0.0_f32, 0.0_f32);
    for (i, name) in labels.iter().enumerate() {
        let tw = pg.measure(name, font);
        let sel = i as u8 == tab;
        if sel {
            stab = (b - 2.0, tw + 15.0);
        } else {
            pg.text(pg.row(b + 6.0, 80.0), name, font, ink);
        }
        let rect = Rect::from_min_size(pg.row(b, 78.0), Vec2::new(tw + 12.0, 18.0));
        if pg
            .ui
            .interact(rect, Id::new(("mc-tab", i)), Sense::click())
            .clicked()
        {
            nsel = i as u8;
        }
        b += tw + 12.0;
    }
    state.mc_tab = nsel;
    // 选中页签压在面板线上（原版先填一块和面板同色的框，再擦掉那 1 像素的描边）。
    let (stabx, stabw) = stab;
    let sel_rect = if fluent {
        Rect::from_min_max(pg.row(stabx, 76.0), pg.row(stabx + stabw, 101.0))
    } else {
        Rect::from_min_size(pg.row(stabx, 76.0), Vec2::new(stabw, 20.0))
    };
    let sel_fill = if fluent {
        if th.fdark {
            ggray(32)
        } else {
            ggray(243)
        }
    } else if th.kind == ThemeKind::Dark {
        th.window_bg
    } else {
        Color32::WHITE
    };
    let p = pg.ui.painter();
    p.rect_filled(sel_rect, 0.0, sel_fill);
    if !fluent {
        p.rect_stroke(
            sel_rect,
            0.0,
            Stroke::new(1.0_f32, Color32::from_rgb(137, 140, 149)),
            StrokeKind::Inside,
        );
        p.rect_filled(
            Rect::from_min_max(pg.row(stabx + 1.0, 96.0), pg.row(stabx + stabw - 1.0, 97.0)),
            0.0,
            sel_fill,
        );
    }
    pg.text(pg.row(stabx + 8.0, 78.0), labels[tab as usize], font, ink);

    if nsel == 0 {
        // —— Schematic 页 ——
        let tempo_ok = crate::state::tempo_schematic_ok(tempo);
        mc_status_icon(&pg, bank.as_deref(), 115.0, usize::from(tempo_ok));
        pg.text(
            pg.row(45.0, 115.0),
            tr(
                "The tempo must be either 2.5, 5 or 10 ticks per second.",
                "速度必须为每秒 2.5、5 或 10 红石刻。",
            ),
            font,
            ink,
        );
        let cur = if crate::lang() == 1 {
            format!("目前速度为每秒 {tempo} 红石刻。")
        } else {
            format!("The tempo is {tempo} ticks per second.")
        };
        pg.text(
            pg.row(45.0, 131.0),
            &cur,
            font,
            if tempo_ok { green } else { red },
        );
        if !tempo_ok
            && pg.button(
                45.0,
                149.0,
                140.0,
                tr("Fix tempo for schematic", "针对 Schematic 调整速度"),
                false,
            )
        {
            state.emit(Command::FixTempoSchematic);
        }

        mc_status_icon(&pg, bank.as_deref(), 205.0, usize::from(outside == 0));
        pg.text(
            pg.row(45.0, 205.0),
            tr(
                "All blocks must be within Minecraft's 2 octave range.",
                "所有方块必须在 Minecraft 的 2 八度范围内。",
            ),
            font,
            ink,
        );
        if outside > 0 {
            let n = if crate::lang() == 1 {
                format!("有 {outside} 个方块在 2 八度范围外。")
            } else if outside == 1 {
                "There is 1 block outside the 2 octave range.".to_string()
            } else {
                format!("There are {outside} blocks outside the 2 octave range.")
            };
            pg.text(pg.row(45.0, 221.0), &n, font, red);
            if pg.button(
                45.0,
                239.0,
                120.0,
                tr("Select lower blocks", "选择过低的音符"),
                false,
            ) {
                state.emit(Command::SelectOutside {
                    lower: true,
                    higher: false,
                });
            }
            if pg.button(
                175.0,
                239.0,
                120.0,
                tr("Select higher blocks", "选择过高的音符"),
                false,
            ) {
                state.emit(Command::SelectOutside {
                    lower: false,
                    higher: true,
                });
            }
            if pg.button(
                305.0,
                239.0,
                100.0,
                tr("Transpose notes", "转换音符"),
                false,
            ) {
                state.emit(Command::TransposeToMinecraft);
            }
        } else {
            pg.text(
                pg.row(45.0, 221.0),
                tr(
                    "There are no blocks outside the 2 octave range.",
                    "没有方块超出2八度限制。",
                ),
                font,
                green,
            );
        }

        mc_status_icon(&pg, bank.as_deref(), 295.0, usize::from(custom == 0));
        pg.text(
            pg.row(45.0, 295.0),
            tr(
                "No custom instruments may be used.",
                "不可使用任何自定义音色。",
            ),
            font,
            ink,
        );
        if custom > 0 {
            let n = if crate::lang() == 1 {
                format!("有 {custom} 个方块使用自定义音色。")
            } else if custom == 1 {
                "There is 1 block with custom instruments.".to_string()
            } else {
                format!("There are {custom} blocks with custom instruments.")
            };
            pg.text(pg.row(45.0, 311.0), &n, font, red);
            if pg.button(
                45.0,
                329.0,
                160.0,
                tr("Select affected blocks", "选择所指音符"),
                false,
            ) {
                state.emit(Command::SelectCustom);
            }
        } else {
            pg.text(
                pg.row(45.0, 311.0),
                tr(
                    "There are no blocks with custom instruments.",
                    "没有方块使用自定义音色。",
                ),
                font,
                green,
            );
        }
    } else {
        // —— 数据包页 ——
        let good = crate::state::tempo_datapack_good(tempo);
        mc_status_icon(&pg, bank.as_deref(), 115.0, if good { 1 } else { 2 });
        pg.text(
            pg.row(45.0, 115.0),
            tr(
                "Any tempo works when exporting as a data pack.",
                "导出数据包时可使用任意速度。",
            ),
            font,
            ink,
        );
        pg.text(
            pg.row(45.0, 131.0),
            tr(
                "However, the tempos 0.25, 0.5, 1, 1.25, 2, 2.5, 4, 5, 10 and 20 t/s work better.",
                "但是，使用每秒 0.25、0.5、1、1.25、2、2.5、4、5、10 或 20 红石刻的速度更佳。",
            ),
            font,
            ink,
        );
        let cur = if crate::lang() == 1 {
            format!("目前速度为每秒 {tempo} 红石刻。")
        } else {
            format!("The tempo is {tempo} ticks per second.")
        };
        pg.text(
            pg.row(45.0, 147.0),
            &cur,
            font,
            if good { green } else { orange },
        );
        if !good
            && pg.button(
                45.0,
                165.0,
                180.0,
                tr("Optimize tempo for data pack", "针对数据包调整速度"),
                false,
            )
        {
            state.emit(Command::FixTempoDatapack);
        }

        pg.text(
            pg.row(45.0, 205.0),
            tr(
                "Using a resource pack, you may extend the supported range to 6 octaves.",
                "使用数据包，即可将支持范围扩展到 6 个八度。",
            ),
            font,
            ink,
        );
        pg.text(
            pg.row(45.0, 221.0),
            tr(
                "To play without one, all blocks must be within Minecraft's 2 octave range.",
                "如果不想用，所有方块必须在 Minecraft 的 2 八度限制内。",
            ),
            font,
            ink,
        );
        if outside > 0 {
            mc_status_icon(&pg, bank.as_deref(), 205.0, 2);
            let n = if crate::lang() == 1 {
                format!("有 {outside} 个方块在 2 八度范围外。")
            } else if outside == 1 {
                "There is 1 block outside the 2 octave range.".to_string()
            } else {
                format!("There are {outside} blocks outside the 2 octave range.")
            };
            pg.text(pg.row(45.0, 237.0), &n, font, orange);
            if pg.button(45.0, 255.0, 80.0, tr("Select lower", "选择过低音符"), false) {
                state.emit(Command::SelectOutside {
                    lower: true,
                    higher: false,
                });
            }
            if pg.button(
                135.0,
                255.0,
                80.0,
                tr("Select higher", "选择过高音符"),
                false,
            ) {
                state.emit(Command::SelectOutside {
                    lower: false,
                    higher: true,
                });
            }
            if pg.button(
                225.0,
                255.0,
                100.0,
                tr("Transpose notes", "转换音符"),
                false,
            ) {
                state.emit(Command::TransposeToMinecraft);
            }
            if pg.button(
                335.0,
                255.0,
                120.0,
                tr("Save resource pack", "保存资源包"),
                false,
            ) {
                state.emit(Command::SaveExtraNotesPack);
            }
        } else {
            mc_status_icon(&pg, bank.as_deref(), 205.0, 1);
            pg.text(
                pg.row(45.0, 237.0),
                tr(
                    "There are no blocks outside the 2 octave range.",
                    "没有方块超出 2 八度限制。",
                ),
                font,
                green,
            );
        }

        pg.text(
            pg.row(45.0, 295.0),
            tr(
                "Using a resource pack, you may play custom instruments in Minecraft.",
                "使用数据包，即可在 Minecraft 中使用自定义音色。",
            ),
            font,
            ink,
        );
        pg.text(
            pg.row(45.0, 311.0),
            tr(
                "To play without one, no custom instruments must be used.",
                "如果不想用，就不能用任何自定义音色。",
            ),
            font,
            ink,
        );
        if custom > 0 {
            mc_status_icon(&pg, bank.as_deref(), 295.0, 2);
            let n = if crate::lang() == 1 {
                format!("有 {custom} 个方块使用自定义音色。")
            } else if custom == 1 {
                "There is 1 block with custom instruments.".to_string()
            } else {
                format!("There are {custom} blocks with custom instruments.")
            };
            pg.text(pg.row(45.0, 327.0), &n, font, orange);
            if pg.button(
                45.0,
                345.0,
                160.0,
                tr("Select affected blocks", "选择所指音符"),
                false,
            ) {
                state.emit(Command::SelectCustom);
            }
        } else {
            mc_status_icon(&pg, bank.as_deref(), 295.0, 1);
            pg.text(
                pg.row(45.0, 327.0),
                tr(
                    "There are no blocks with custom instruments.",
                    "没有方块使用自定义音色。",
                ),
                font,
                green,
            );
        }
    }

    // 原版 `draw_button2(x1 + 240 - 36, y1 + 413, 72, "确定")`。
    if pg.button(204.0, 413.0, 72.0, tr("OK", "确定"), false) {
        state.emit(Command::CloseWindow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首选项正文要和菜单栏同一档：菜单栏用的是 egui 的 `TextStyle::Button`，
    /// 谁改了那边就得同步改这里，否则窗口里会同时出现两种字号。
    #[test]
    fn prefs_font_matches_the_menu_bar() {
        let style = egui::Style::default();
        assert_eq!(
            PREFS_FONT,
            style.text_styles[&egui::TextStyle::Button].size,
            "菜单栏字号变了"
        );
    }

    #[test]
    fn hz_label_inserts_the_original_commas() {
        assert_eq!(hz_label(44100), "44,100 Hz");
        assert_eq!(hz_label(8000), "8,000 Hz");
        assert_eq!(hz_label(384000), "384,000 Hz");
        assert_eq!(hz_label(192000), "192,000 Hz");
    }

    /// 原版 seconds_to_str 两语言各自的 7 档阈值，边界值各测一次；
    /// 英文复数按整除后的值判（`1 second ago` / `2 seconds ago`）。
    #[test]
    fn seconds_to_str_matches_the_original_thresholds() {
        // 直接读全局 lang()，要和跑帧的测试互斥，防止别处一帧把语言翻走。
        let _lock = crate::frame_lock();
        let _cn = crate::LangGuard::set(1);
        assert_eq!(seconds_to_str(0), "0 秒前");
        assert_eq!(seconds_to_str(30), "30 秒前");
        assert_eq!(seconds_to_str(59), "59 秒前");
        assert_eq!(seconds_to_str(60), "1 分钟前");
        assert_eq!(seconds_to_str(3599), "59 分钟前");
        assert_eq!(seconds_to_str(3600), "1 小时前");
        assert_eq!(seconds_to_str(86399), "23 小时前");
        assert_eq!(seconds_to_str(86400), "1 天前");
        assert_eq!(seconds_to_str(604799), "6 天前");
        assert_eq!(seconds_to_str(604800), "1 星期前");
        assert_eq!(seconds_to_str(2678399), "4 星期前");
        assert_eq!(seconds_to_str(2678400), "1 月前");
        assert_eq!(seconds_to_str(32140799), "11 月前");
        assert_eq!(seconds_to_str(32140800), "1 年前");
        let _en = crate::LangGuard::set(0);
        assert_eq!(seconds_to_str(0), "0 seconds ago");
        assert_eq!(seconds_to_str(1), "1 second ago");
        assert_eq!(seconds_to_str(2), "2 seconds ago");
        assert_eq!(seconds_to_str(60), "1 minute ago");
        assert_eq!(seconds_to_str(120), "2 minutes ago");
        assert_eq!(seconds_to_str(3600), "1 hour ago");
        assert_eq!(seconds_to_str(86400), "1 day ago");
        assert_eq!(seconds_to_str(604800), "1 week ago");
        assert_eq!(seconds_to_str(2678400), "1 month ago");
        assert_eq!(seconds_to_str(32140800), "1 year ago");
    }
}
