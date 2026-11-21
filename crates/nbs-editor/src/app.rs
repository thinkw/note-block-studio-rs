//! 一帧的整体编排：输入 → 命令 → 状态 → 面板绘制 → 弹窗。
//! 与 eframe 解耦，入口 crate 只负责创建窗口和调用 `show()`。

use egui::{Context, Frame};

use crate::grid::show_grid;
use crate::layers_panel::{panel_width, show_layers_panel};
use crate::piano::show_piano;
use crate::shortcuts::collect_shortcuts;
use crate::state::EditorState;
use crate::statusbar::show_status_bar;
use crate::toolbar::{show_menu_bar, show_toolbar};
use crate::viewport::{MENU_HEIGHT, PIANO_HEIGHT, STATUS_HEIGHT, TOOLBAR_HEIGHT};
use crate::windows::show_windows;

/// 原版界面全是绝对像素坐标，面板默认的 8×4 内边距会把每个按钮都推歪，
/// 所以所有面板都用零边距的框。
fn tight() -> Frame {
    Frame::new().inner_margin(0.0)
}

fn flush_all(state: &mut EditorState, ctx: &Context) {
    for _ in 0..6 {
        if state.pending.is_empty() {
            break;
        }
        state.flush(ctx);
    }
}

pub fn show(ctx: &Context, state: &mut EditorState) {
    // 界面语言每帧跟 prefs 对齐；原版 language 只在设置菜单和读档时赋值，
    // 下拉切换后下一帧生效，这里的语义相同。
    crate::set_language(state.prefs.language);
    // 字体表与语言无关（#98 之后统一用默认字体 + 中文兜底），一个 Context 只装一次：
    // `set_fonts` 内部会把整张表的 TTF 字节比对一遍，每帧装代价太大。
    if !state.fonts_installed {
        crate::fonts::install(ctx);
        state.fonts_installed = true;
    }
    // egui 内置控件（菜单、下拉条目、tooltip）走 TextStyle，原版这些控件全用
    // `font_main` 12px / `font_small` 10.67px —— 把默认档钉到原版字号。
    ctx.style_mut(|style| {
        let ts = &mut style.text_styles;
        ts.insert(egui::TextStyle::Small, crate::fonts::small_font());
        ts.insert(egui::TextStyle::Body, crate::fonts::main_font());
        ts.insert(egui::TextStyle::Button, crate::fonts::main_font());
        ts.insert(egui::TextStyle::Heading, crate::fonts::main_font());
        ts.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(crate::fonts::MAIN_SIZE, egui::FontFamily::Monospace),
        );
    });
    // 原版是 GameMaker 的像素绘制：1px 的网格线就是结结实实一个像素。
    // egui 默认把每条边羽化成渐变，1px 竖线会变成三列淡色，深度只有原版的一半，所以关掉。
    ctx.tessellation_options_mut(|o| o.feathering = false);
    // 精灵图只需加载一次；缺失时面板自己退回文字按钮。
    state.icons.ensure(ctx);
    // 独立播放时钟（不绑 UI 帧率）+ 快捷键 + 偏好定时持久化
    state.tick_transport(ctx);
    state.maybe_save_prefs();
    // 原版 create 事件发起 HTTP 请求，Draw 事件里轮询结果更新欢迎窗的状态行。
    state.poll_update_check();
    crate::transcription::poll(state, ctx);
    for cmd in collect_shortcuts(state, ctx) {
        state.emit(cmd);
    }
    flush_all(state, ctx);

    // 原版把壁纸画在 `draw_clear` 之后、所有面板之前（control_draw.gml:48-67），
    // 端口对应的位置是背景层最前面：菜单条 / 工具栏 / 状态栏 / 琴键 / 图层栏随后
    // 各自盖上来，只有编辑区四周的空白会露出图片。
    crate::background::paint(state, ctx);

    egui::TopBottomPanel::top("nbs_menu")
        .frame(tight())
        .exact_height(MENU_HEIGHT)
        .show(ctx, |ui| show_menu_bar(state, ui));

    egui::TopBottomPanel::top("nbs_toolbar")
        .frame(tight())
        .exact_height(TOOLBAR_HEIGHT)
        .show(ctx, |ui| show_toolbar(state, ui));

    egui::TopBottomPanel::bottom("nbs_status")
        .frame(tight())
        .exact_height(STATUS_HEIGHT)
        .show(ctx, |ui| show_status_bar(state, ui));

    // 原版 `control_draw.gml:2549`：钢琴调用条件是 `show_piano && !isplayer`。
    if state.flags.show_piano && !state.player {
        // 端口自加（用户要求）：琴键面板独立缩放，高度跟 `piano_scale`，
        // 不随 `window_scale`（与 #80「钢琴栏不随界面缩放」一致）。
        egui::TopBottomPanel::bottom("nbs_piano")
            .frame(tight())
            .exact_height(PIANO_HEIGHT * state.prefs.piano_scale)
            .show(ctx, |ui| show_piano(state, ui));
    } else {
        // 悬停路由用的矩形是每帧由面板写入的；面板关掉后必须清掉，
        // 否则悬停在原琴键位置时缩放键还会走琴键档。
        state.piano_rect = None;
    }

    // 原版图层行绘制在 `control_draw.gml:308` 巨门内，播放器不画；但播放器中央
    // 信息区包在 `!fullscreen && show_layers` 门里（:2341），所以 show_layers
    // 开关本身仍要参与播放器分支的判定。
    if state.flags.show_layers && !state.player {
        // #108：侧栏横轴脱离界面缩放 —— 宽度取 `prefs.layer_panel_width`（屏幕像素，
        // 拖面板右边界改，双击回 264），只有栏内行高跟缩放；原版是整幅画面等比缩放。
        egui::SidePanel::left("nbs_layers")
            .frame(tight())
            .exact_width(panel_width(state))
            // egui 自己的那条边界分隔条默认就能拖（`SidePanel::resizable` 默认 true），
            // 且比我们的把手晚注册，会把这 5 px 的命中抢走。宽度既然由首选项说了算，
            // 就把它的拖拽关掉，只留我们这一条。
            .resizable(false)
            .show(ctx, |ui| show_layers_panel(state, ui));
    }

    egui::CentralPanel::default().frame(tight()).show(ctx, |ui| {
        let area = ui.available_rect_before_wrap();
        show_grid(state, ui, area);
    });

    // 网格交互 / 面板按钮产生的命令在这里落地
    flush_all(state, ctx);

    show_windows(state, ctx);
    flush_all(state, ctx);
    // 对应原版每步结尾的 `prevwindow = window`：窗口自己的 Ctrl+P 关窗要等下一帧才生效。
    state.window_prev = state.window.clone();
}
