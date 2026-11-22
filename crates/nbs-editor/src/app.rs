//! 一帧的整体编排：输入 → 命令 → 状态 → 面板绘制 → 弹窗。
//! 与 eframe 解耦，入口 crate 只负责创建窗口和调用 `show()`。

use egui::{Context, Frame, Rect, Sense};

use crate::fx::{self, FxInput};
use crate::grid::show_grid;
use crate::layers_panel::{panel_width, show_layers_panel};
use crate::piano::{paint_notechart, show_piano, ChartRequest, PANEL_HEIGHT};
use crate::shortcuts::collect_shortcuts;
use crate::state::EditorState;
use crate::statusbar::show_status_bar;
use crate::toolbar::{show_menu_bar, show_toolbar};
use crate::viewport::{MENU_HEIGHT, STATUS_HEIGHT, TOOLBAR_HEIGHT};
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

/// 一帧特效的输入。倍率取 `piano_scale` 而不是 `window_scale`：琴键几何那套常量
/// （39px 白键、128px 命中区）本来就是屏幕像素，#80 之后也不随界面缩放，
/// 特效和琴键面板用同一个倍率才对得上道。
/// 键盘摆放只在琴键面板这一帧真画过时给出，否则 `None`，特效自己 `fit`。
fn fx_input(state: &EditorState, ctx: &Context, area: Rect) -> FxInput {
    let scale = state.prefs.piano_scale.max(0.1);
    let clock = state.audio.clock();
    let keyboard = if state.piano_keys_shown == 0 {
        None
    } else {
        fx::panel_keyboard(
            state.piano_rect,
            scale,
            state.piano_start as usize,
            state.piano_keys_shown as usize,
        )
    };
    FxInput {
        now: ctx.input(|i| i.time),
        dt: ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) as f32,
        position: clock.position(),
        playing: clock.is_playing(),
        speed: clock.speed(),
        scale,
        accent: state.theme.accent,
        backdrop: state.theme.grid_bg,
        area,
        keyboard,
    }
}

/// 特效有没有把底部琴键栏钉死：开着舞台特效，或开着按琴键栏对齐的叠加特效。
/// 这时面板画不画由特效说了算，`flags.show_piano` 那个开关点不动任何东西，
/// 所以状态栏右下角那条「琴键」按钮要跟着收起（用户要求：直到关闭特效为止）。
pub(crate) fn piano_forced_by_fx(state: &EditorState) -> bool {
    !state.player && (state.fx.stage_active() || state.fx.wants_piano())
}

/// 图层栏的可见性。播放器没有它（原版 `control_draw.gml:308` 那个巨门），
/// 舞台特效开着时也没有：那一栏把编辑区往右推，瀑布的轨道就对不上下面的琴键。
fn layers_panel_shown(state: &EditorState, stage: bool) -> bool {
    state.flags.show_layers && !state.player && !stage
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
    // 外部控制（MCP 桥、脚本）投来的请求在这一帧就地执行。放在快捷键之后：
    // 同一帧里人的输入先落地，AI 拿到的 rev 才包含这一帧的人为改动。
    // `state.control` 是 None 时这里等于一次空判断，不开链路就没有任何开销。
    crate::control::drain(state, ctx);

    // 特效用的那份时间轴在绘制之前烘好：命令、快捷键、外部控制都已经落完，
    // `song.rev` 这一帧不会再变。
    state.refresh_fx();
    // 播放器档位是原版那套只读界面，特效一律不上：那里的设置菜单没有
    // 「播放特效…」，一旦接了舞台就没有关掉的入口。
    let fx_on = !state.player;
    let stage = fx_on && state.fx.stage_active();

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

    // 悬停琴键的五线谱：琴键面板里只登记请求，落笔延到中央面板之后（#112）。
    let mut chart: Option<ChartRequest> = None;
    // 原版 `control_draw.gml:2549`：钢琴调用条件是 `show_piano && !isplayer`。
    // 端口自加：特效的轨道都按这条琴键栏摆放（瀑布直接把胶囊落在真键面上，方块排压在
    // 键上方），所以用户关了面板，只要有任何特效要求就把它顶回来 —— 画面里始终只有一条键盘。
    let piano = fx_on && (state.flags.show_piano || state.fx.wants_piano());
    if piano {
        // 端口自加（用户要求）：琴键面板独立缩放，高度跟 `piano_scale`，
        // 不随 `window_scale`（与 #80「钢琴栏不随界面缩放」一致）。
        // #109：高度含黑键高出的 7px（见 `piano::PANEL_HEIGHT`），并且关掉 egui 面板
        // 默认自带的可拖边界和那条分隔线——原版没有，分隔线还会横穿黑键顶部。
        chart = egui::TopBottomPanel::bottom("nbs_piano")
            .frame(tight())
            .resizable(false)
            .show_separator_line(false)
            .exact_height(PANEL_HEIGHT * state.prefs.piano_scale)
            .show(ctx, |ui| show_piano(state, ui))
            .inner;
    } else {
        // 悬停路由用的矩形是每帧由面板写入的；面板关掉后必须清掉，
        // 否则悬停在原琴键位置时缩放键还会走琴键档。
        state.piano_rect = None;
    }

    // 原版图层行绘制在 `control_draw.gml:308` 巨门内，播放器不画；但播放器中央
    // 信息区包在 `!fullscreen && show_layers` 门里（:2341），所以 show_layers
    // 开关本身仍要参与播放器分支的判定。
    if layers_panel_shown(state, stage) {
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
        if stage {
            // 舞台独占这块：网格这一帧不画，就地编辑和右键菜单跟着停。
            // 空间还是要照样认领，否则 egui 会把这块地方让给别的面板。
            ui.allocate_rect(area, Sense::hover());
            // 画笔自己凑，不用 `ui.painter()`：那条只裁到中央面板下缘，而瀑布的
            // 胶囊和键位高亮要一路落到下面那条真琴键栏上。底色仍只铺 `area`
            // （在 `paint_stage` 里按 `input.area` 刷），琴键栏不会被染黑。
            let clip = match (piano, state.piano_rect) {
                (true, Some(panel)) => area.union(panel),
                _ => area,
            };
            let p = egui::Painter::new(ctx.clone(), egui::LayerId::background(), clip);
            let input = fx_input(state, ctx, area);
            state.fx.paint_stage(ctx, &input, &p);
        } else {
            show_grid(state, ui, area);
        }
    });

    // #112：五线谱画在网格之后才能盖住编辑区（同层按出队顺序绘制，用户报告「音符被
    // 主界面挡住出不来」）。弹窗是 Area，在更高的 `Order::Medium`，仍然压在五线谱上。
    if let Some(req) = chart {
        paint_notechart(ctx, state, req);
    }

    // 叠加类特效同理：压住面板、仍在弹窗之下（它自成一个 background 层 id，
    // 排在五线谱那条之后）。这里按表面判断而不是按「有没有开特效」：开着瀑布时
    // 这条路径整段跳过，`fx_input` 那次读偏好、算琴键栏矩形也一并省掉 ——
    // 两条表面互斥，同一帧只有一边在算图元。
    if fx_on && state.fx.overlay_active() {
        let overlay = fx_input(state, ctx, ctx.screen_rect());
        state.fx.paint_overlay(ctx, &overlay);
    }

    // 网格交互 / 面板按钮产生的命令在这里落地
    flush_all(state, ctx);

    show_windows(state, ctx);
    flush_all(state, ctx);
    // 对应原版每步结尾的 `prevwindow = window`：窗口自己的 Ctrl+P 关窗要等下一帧才生效。
    state.window_prev = state.window.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Vec2};

    /// `fx_input` 的两条口径：倍率取 `piano_scale`（琴键常量是屏幕像素，#80 起不随
    /// 界面缩放），键盘摆放只在琴键面板真画过那一帧才给出 —— 特效拿不到过期的矩形。
    #[test]
    fn fx_input_uses_the_piano_scale_and_only_reports_a_drawn_keyboard() {
        let _prefs = crate::isolate_prefs();
        let ctx = Context::default();
        let mut state = EditorState::for_test();
        state.prefs.piano_scale = 1.5;
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));

        state.piano_keys_shown = 0;
        state.piano_rect = Some(area);
        let got = fx_input(&state, &ctx, area);
        assert_eq!(got.scale, 1.5);
        assert_eq!(got.area, area);
        assert_eq!(got.keyboard, None, "面板没画过时不给摆放");

        state.piano_keys_shown = 12;
        state.piano_start = 24;
        let kb = fx_input(&state, &ctx, area)
            .keyboard
            .expect("面板画过那一帧就该有摆放");
        assert_eq!((kb.scale, kb.first, kb.slots), (1.5, 24, 12));
        assert_eq!(
            kb.origin.y,
            area.min.y + fx::keyboard::BLACK_LIFT * 1.5,
            "原点顶到黑键上沿，和琴键面板同一套像素"
        );
    }

    /// 瀑布的「地面」和琴键面板必须同源：`hit_y` 取的就是这里给的 `origin.y`，
    /// 也即面板顶边往下 `BLACK_LIFT × piano_scale`（白键面沿）。这条一断，
    /// 音符就落在键上方或扎进键里，横坐标同理 —— 槽范围直接抄面板上报的
    /// `piano_start / piano_keys_shown`，栏外的键没有轨道。
    #[test]
    fn the_waterfall_lands_on_the_keys_the_panel_draws() {
        let _prefs = crate::isolate_prefs();
        let ctx = Context::default();
        let mut state = EditorState::for_test();
        state.prefs.piano_scale = 1.0;
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 900.0));
        state.piano_rect = Some(Rect::from_min_size(
            Pos2::new(0.0, area.max.y),
            Vec2::new(1920.0, PANEL_HEIGHT),
        ));
        state.piano_keys_shown = 20;
        state.piano_start = 20;
        let layout = fx_input(&state, &ctx, area)
            .keyboard
            .expect("面板画过那一帧就该有摆放");

        assert_eq!(layout.origin.y, area.max.y + fx::keyboard::BLACK_LIFT);
        assert_eq!(
            layout.origin.x,
            ((1920.0 - 20.0 * fx::keyboard::KEY_W) / 2.0).floor()
        );
        assert!(layout.rect_of_key(60).is_some(), "栏内的键该有下落道");
        assert!(layout.rect_of_key(20).is_none(), "琴键栏没显示到的键不该有道");
        // 舞台画笔的裁剪框要一路盖到面板，否则越过区域下缘的那截被剪掉，
        // 键位高亮根本看不见。
        let clip = area.union(state.piano_rect.unwrap());
        assert!(
            clip.max.y >= layout.origin.y + fx::keyboard::WHITE_HIT_H,
            "裁剪框没盖到白键面底：{} < {}",
            clip.max.y,
            layout.origin.y + fx::keyboard::WHITE_HIT_H
        );
    }

    /// 两种特效都把底部琴键栏钉死：舞台瀑布的轨道按它放，方块排压在它上方。
    /// 这时 `flags.show_piano` 那个开关点了没反应，状态栏右下角的「琴键」按钮
    /// 要收起，特效关掉才放回来（用户要求）。
    #[test]
    fn an_effect_that_needs_the_keyboard_hides_the_keys_toggle() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        assert!(!piano_forced_by_fx(&state), "一条特效都没开时开关照旧");

        state.fx.stack.set_active(fx::synthesia::Synthesia::ID, true);
        assert!(piano_forced_by_fx(&state), "舞台瀑布要用琴键面板");
        state.fx.stack.set_active(fx::synthesia::Synthesia::ID, false);

        state.fx.stack.set_active(fx::blockrow::BlockRow::ID, true);
        assert!(piano_forced_by_fx(&state), "方块排按琴键面板对齐");
        state.fx.stack.set_active(fx::blockrow::BlockRow::ID, false);
        assert!(!piano_forced_by_fx(&state), "关掉特效按钮就回来");

        state.player = true;
        state.fx.stack.set_active(fx::synthesia::Synthesia::ID, true);
        assert!(!piano_forced_by_fx(&state), "播放器那一套界面不接特效");
    }

    /// 舞台开着期间图层栏收起，关掉立刻回来。
    #[test]
    fn the_stage_swallows_the_layer_column() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        assert!(layers_panel_shown(&state, false));
        assert!(!layers_panel_shown(&state, true), "舞台开着时左侧那栏让路");
        state.flags.show_layers = false;
        assert!(!layers_panel_shown(&state, false), "用户自己关的照关");
        state.flags.show_layers = true;
        state.player = true;
        assert!(!layers_panel_shown(&state, false), "播放器没有图层栏");
    }
}
