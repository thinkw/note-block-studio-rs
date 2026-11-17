//! `nbs-editor` — egui 实现的网格音符编辑器（分析里的 D 模块）。
//!
//! ```text
//! app          一帧编排：输入 → 命令 → 状态 → 面板 → 弹窗
//! grid         超大滚动网格画布、视口剔除、交互状态机、右键菜单
//! viewport     坐标换算 / 可见区间 / 缩放 / 播放跟随
//! interaction  DragMode 状态机 + 一帧输入快照
//! commands     统一命令总线与 dispatch
//! document(*)  所有编辑命令的 diff 事务（在 nbs-core）
//! layers_panel / toolbar / timeline(grid) / piano / statusbar
//! windows      歌曲信息、属性、统计、首选项、宏、文件对话框、帮助
//! theme        4 套主题
//! fonts        启动时装系统中文字体（egui 自带字体没有 CJK 字形）
//! shortcuts    全局快捷键
//! ```
//! 只依赖 `egui`（不依赖 eframe），渲染后端由入口 crate 决定。

pub mod app;
pub mod commands;
pub mod fonts;
pub mod grid;
pub mod icons;
pub mod interaction;
pub mod layers_panel;
pub mod piano;
pub mod shortcuts;
pub mod state;
pub mod statusbar;
pub mod theme;
pub mod toolbar;
pub mod viewport;
pub mod windows;

pub use app::show;
pub use state::{EditMode, EditorState, Flag, UiFlags, WindowKind};
pub use theme::{Theme, ThemeKind};
pub use viewport::Viewport;

/// 本构建的版本号（欢迎窗「版本 x.y.z - 发布于 …」行 + 更新检查的被比较方）。
/// 发布时随 Cargo.toml 一起改；上次运行版本存在 prefs.last_version 里，
/// 不同则开机走「更新成功」流程（原版 control_create.gml:747-753）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 发布日期，原版 `version_date`（macros.gml）。欢迎窗里以「版本 … - 发布于 …」显示。
pub const VERSION_DATE: &str = "2026-11-16";
/// 更新检查的对象是本仓库的 releases 页。原版查 OpenNBS 官方源
/// （link_releases / link_latest，macros.gml:11-12）；我们是独立重写版，
/// 不能把用户引去官方安装包——已记录的偏差。API 地址在 nbs-app 的 update 模块。
pub const RELEASES_PAGE: &str = "https://github.com/thinkw/note-block-studio-rs/releases";

/// 欢迎窗的链接组（macros.gml:9-20）。这些是 OpenNBS 社区的入口，重写版照原样
/// 指向官方页面；只有更新检查指向本仓库（见 RELEASES_PAGE 的偏差说明）。
pub const LINK_WEBSITE: &str = "https://opennbs.org/";
pub const LINK_DONATE: &str = "https://opencollective.com/OpenNBS/donate";
pub const LINK_DISCORD: &str = "https://discord.gg/sKPGjyVcyy";
pub const LINK_TWITTER: &str = "https://twitter.com/OpenNBS";
pub const LINK_YOUTUBE: &str = "https://youtube.com/@OpenNBS";
pub const LINK_GITHUB: &str = "https://github.com/OpenNBS/OpenNoteBlockStudio";
pub const LINK_QQ_GROUP: &str = "http://qm.qq.com/cgi-bin/qm/qr?_wv=1027&k=9iT2JIMOPWcrhzVpOROtlv7PJ1jSGEu0&authKey=VmmwRR7507A9fEVxraPmXn224vhbHXc4xs4QmwkveFZBriU%2Fq2NCj9fbYE5QCaLJ&noverify=0&group_code=218993157";

// ---- 多语言。原版只有 0 = English / 1 = 简体中文两种，全部文案都是
// `if (language != 1) { 英文 } else { 中文 }`（condstr 的封装）。
// 开关本体在 nbs-core（内核的历史标签、读档报错要用同一个），app::show 每帧
// 把 prefs.language 同步进去，文案侧统一写 tr(英文, 中文)。

pub use nbs_core::{lang, tr};

/// app::show 每帧从 prefs 同步；原版 language 只在设置菜单和读档时赋值，
/// 效果等价（开关在下一帧生效，帧内不跳变）。
pub fn set_language(l: u8) {
    nbs_core::set_language(l);
}

/// 跑帧的测试互相串行。语言已经改成按线程钉（见 `nbs_core::lang`），不再需要这把锁
/// 防串台；现在它只起「一次只跑一个 GUI 用例」的作用，红了好复现。
/// 等 #7 死代码清理时再决定要不要撤掉。
#[cfg(test)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct LangGuard(u8);
#[cfg(test)]
impl LangGuard {
    pub fn set(l: u8) -> LangGuard {
        let prev = lang();
        set_language(l);
        LangGuard(prev)
    }
}
#[cfg(test)]
impl Drop for LangGuard {
    fn drop(&mut self) {
        set_language(self.0);
    }
}

#[cfg(test)]
pub(crate) fn frame_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 出厂值断言 / 不该写用户配置的测试都用它：指向一个空的配置目录，
/// 调试界实时改过缩放 / 音量、或测试期间真的落盘过偏好，单测都会莫名其妙变红。
/// 目录里旧一轮跑剩的 prefs.json 也要清掉，否则上次测试写下的开关值
/// 会污染这一轮的「出厂值」断言。
#[cfg(test)]
pub(crate) fn isolate_prefs() {
    let dir = std::env::temp_dir().join("nbs-rust-test-prefs");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::remove_file(dir.join("prefs.json"));
    std::env::set_var("NBS_RUST_CONFIG_DIR", &dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_defaults() {
        isolate_prefs();
        let s = EditorState::new();
        assert!(s.doc.song.total_blocks > 0);
        assert!(s.doc.history.can_undo() == false);
    }

    /// 离屏跑一帧完整 UI，确保布局 / 绘制 / 输入状态机不会 panic。
    #[test]
    fn ui_renders_one_frame_offscreen() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            let _lock = frame_lock();
            crate::show(ctx, &mut state);
        });
        assert!(!out.shapes.is_empty(), "一帧应当产生绘制形状");
        assert!(state.interaction.visible_notes > 0, "演示曲应有可见音符");
    }

    /// 用原版自带的大曲子验证「一帧只绘制屏幕内音符」这一硬性要求。
    #[test]
    fn real_song_frame_is_culled() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../NoteBlockStudio/datafiles/Songs/Megalovania - Super Smash Bros. Ultimate.nbs",
        );
        if !path.exists() {
            return;
        }
        let song = nbs_core::nbs::load(&path).unwrap();
        let total = song.total_blocks;
        assert!(total > 1000, "测试需要一个足够大的样本曲");

        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.doc.replace_song(song);
        state.theme.apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            let _lock = frame_lock();
            crate::show(ctx, &mut state)
        });

        let drawn = state.interaction.visible_notes;
        println!("  大曲子共 {total} 音符，一帧只绘制 {drawn} 个");
        assert!(drawn > 0, "应当绘制到可见音符");
        assert!(
            drawn < total / 4,
            "一帧绘制了 {drawn} / {total} 个音符，视口剔除可能失效"
        );
    }

    #[test]
    fn culling_range_stays_small_for_huge_song() {
        let mut s = EditorState::new();
        for i in 0..2000u32 {
            s.doc
                .place_note(i * 50, 0, nbs_core::model::Note::new(0, 45));
        }
        s.viewport.size = egui::vec2(1200.0, 600.0);
        let (t0, t1, _, _) = s.viewport.visible_range();
        assert!(t1 - t0 < 100, "可见区间不得随歌曲长度增长");
        // 只查询可见区，不能把 2000 个音符全捞出来
        assert!(s.doc.song.notes_in_view(t0, t1, 0, 20).len() < 100);
    }

    /// 原版 control_create.gml 结尾的开机流程：出厂状态横幅亮 + 欢迎窗开。
    #[test]
    fn enter_startup_factory_defaults_show_greeting_and_banner() {
        isolate_prefs();
        let s = EditorState::new();
        // donate_banner_time 出厂 0（= 原版 −1），now > 0 → 横幅亮
        assert!(s.donate_banner, "出厂应亮捐赠横幅");
        assert_eq!(
            s.window,
            Some(WindowKind::Greeting),
            "show_welcome 出厂为真"
        );
        assert!(!s.update_success, "首次运行不算「更新成功」");
    }

    /// :747-753 的版本变化分支：上次版本与本版不同 → update_success + 强制横幅。
    #[test]
    fn enter_startup_flags_update_success_on_version_change() {
        isolate_prefs();
        let mut s = EditorState::new();
        s.prefs.last_version = Some("0.0.1".into());
        s.prefs.show_welcome = false;
        // 模拟全新一次开机：原版 create 时 window 还没有任何值。
        s.window = None;
        s.enter_startup();
        assert!(s.update_success, "版本变了要亮「更新成功！」");
        assert!(s.donate_banner, "版本变化强制亮横幅");
        assert_eq!(s.window, None, "show_welcome 关着就不开欢迎窗");
        // 同版本再启动（退出时 save_prefs 已把 last_version 写成本版）→ 不算更新成功
        s.prefs.last_version = Some(crate::VERSION.to_string());
        s.update_success = false;
        s.window = None;
        s.enter_startup();
        assert!(!s.update_success);
    }

    /// check_updates.gml 回调的 UI 侧：后台写 1 → 弹一次问句；不重复弹；其他状态不弹。
    #[test]
    fn poll_update_check_asks_once_per_new_version() {
        isolate_prefs();
        let mut s = EditorState::new();
        s.poll_update_check();
        assert!(s.modals.is_empty(), "检查中不应弹窗");
        s.update_check.lock().unwrap().status = 1;
        s.update_check.lock().unwrap().new_version = "9.9.9".into();
        s.poll_update_check();
        assert_eq!(s.modals.items.len(), 1, "有新版本要弹一次问句");
        assert!(s.modals.items.front().unwrap().text.contains("9.9.9"));
        assert!(s.update_question, "问过一次后不再重复弹");
        let n1 = s.modals.items.len();
        s.poll_update_check();
        assert_eq!(s.modals.items.len(), n1, "同一版本不重复弹");
        s.update_check.lock().unwrap().status = 2;
        s.poll_update_check();
        assert_eq!(s.modals.items.len(), n1, "已最新不弹窗");
    }
}

#[cfg(test)]
mod gui_export_tests {
    use commands::dispatch;
    use std::path::PathBuf;

    use super::*;

    fn frame(state: &mut EditorState, ctx: &egui::Context) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            let _lock = frame_lock();
            crate::show(ctx, state)
        });
        assert!(!out.shapes.is_empty(), "窗口应当真的画出内容");
    }

    /// 走一遍 GUI 的真实调用链：音频导出窗口 → 文件对话框 → dispatch 导出。
    /// 这里断言的是产物字节，不是「按钮没 panic」。
    #[test]
    fn audio_export_window_dialog_and_wav_output_are_real() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::AudioExport);
        frame(&mut state, &ctx);

        state.prefs.export_format = "WAV".into();
        state.prefs.export_sample_rate = 22050;
        state.prefs.export_channels = 1;
        dispatch(
            &mut state,
            crate::commands::Command::OpenDialog(state::DialogAction::ExportAudio),
            &ctx,
        );
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "wav", "扩展名跟着格式走");
        assert_eq!(state.window, Some(WindowKind::FileSave));

        let out = std::env::temp_dir().join("nbs_gui_export_mono.wav");
        let _ = std::fs::remove_file(&out);
        // dispatch 与随后的文案断言整段持帧锁，理由同 gui_imports。
        let _lock = crate::frame_lock();
        dispatch(
            &mut state,
            crate::commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );

        let bytes = std::fs::read(&out).expect("GUI 导出必须写出真实文件");
        assert!(bytes.len() > 1000, "产物只有 {} 字节", bytes.len());
        assert_eq!(&bytes[..4], b"RIFF");
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let ch = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
        assert_eq!(
            (rate, ch),
            (22050, 1),
            "采样率/声道必须按窗口里的选项写进头部"
        );
        assert!(
            state
                .export_status
                .as_deref()
                .unwrap_or_default()
                .contains("WAV 已导出"),
            "实际状态：{:?}（全局语言 {}）",
            state.export_status,
            crate::lang()
        );
        drop(_lock);
        let _ = std::fs::remove_file(&out);
    }

    /// 找不到 ffmpeg 时不能假成功：报错，且不留下目标文件。
    #[test]
    fn missing_encoder_reports_truthfully() {
        let saved = std::env::var("NBS_FFMPEG").ok();
        if nbs_io::audio_export::find_ffmpeg().is_some() {
            // 环境里有 ffmpeg，这条用例的前提不成立；让它可见地跳过而不是静默通过。
            eprintln!("跳过：本机找到 ffmpeg，无法验证「缺失编码器」分支");
            return;
        }
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.export_format = "MP3".into();
        let out: PathBuf = std::env::temp_dir().join("nbs_gui_export_should_not_exist.mp3");
        let _ = std::fs::remove_file(&out);
        dispatch(
            &mut state,
            crate::commands::Command::OpenDialog(state::DialogAction::ExportAudio),
            &ctx,
        );
        dispatch(
            &mut state,
            crate::commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        assert!(state.export_status.is_none(), "失败却写了成功状态");
        assert!(!out.exists(), "转码失败不应留下产物");
        if let Some(v) = saved {
            std::env::set_var("NBS_FFMPEG", v);
        }
    }
}

#[cfg(test)]
mod gui_window_tests {
    use commands::dispatch;
    use std::path::{Path, PathBuf};

    use super::*;

    fn frame(state: &mut EditorState, ctx: &egui::Context) {
        let out = frame_out(state, ctx, base_input());
        assert!(!out.shapes.is_empty(), "窗口应当真的画出内容");
    }

    fn base_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        }
    }

    fn frame_out(
        state: &mut EditorState,
        ctx: &egui::Context,
        input: egui::RawInput,
    ) -> egui::FullOutput {
        let _lock = frame_lock();
        ctx.run(input, |ctx| crate::show(ctx, state))
    }

    /// Ctrl+P 开窗：原版 596 行用 `prevwindow == window` 挡住同一帧的「开 → 关」，
    /// 这里同样要求窗口先活过一帧才接受 Ctrl+P 关窗，否则窗口一闪即消失。
    #[test]
    fn ctrl_p_opens_preferences_and_stays_open() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        // 开机流程会开欢迎窗（窗口开着快捷键全被门槛拦掉）；Ctrl+P 的靶子是裸编辑器。
        state.window = None;
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let tap = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1440.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        // 按下和抬起挤在同一帧：正是 SendInput / 人手快按送进来的样子。
        let key = |pressed: bool| egui::Event::Key {
            key: egui::Key::P,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: ctrl,
        };
        frame_out(&mut state, &ctx, tap(vec![key(true), key(false)]));
        assert_eq!(
            state.window,
            Some(WindowKind::Preferences),
            "Ctrl+P 开的窗口被同帧的关窗判断收掉了"
        );
        frame_out(&mut state, &ctx, tap(vec![]));
        frame_out(&mut state, &ctx, tap(vec![key(true), key(false)]));
        assert_eq!(state.window, None, "窗口已开，第二次 Ctrl+P 应当关窗");
    }

    /// 一帧里**最后**一个满足条件的文本中心位置。egui 的 Checkbox/Button 把标签一起
    /// 算进交互矩形，所以点文字就等于点控件。
    /// 取最后一个而不是第一个：绘制顺序就是叠放顺序，面板先画、窗口后画，
    /// 所以同名文字以窗口里的那份才真的收得到点击（例如菜单栏的「设置」页签和
    /// 数据包导出窗口里的「设置」页签重名）。
    fn text_center_if(
        shapes: &[egui::epaint::ClippedShape],
        want: impl Fn(&str) -> bool,
    ) -> egui::Pos2 {
        let mut found = None;
        for cs in shapes {
            if let egui::Shape::Text(t) = &cs.shape {
                if want(t.galley.text()) {
                    found = Some(egui::Rect::from_min_size(t.pos, t.galley.rect.size()).center());
                }
            }
        }
        if let Some(p) = found {
            return p;
        }
        panic!(
            "界面上找不到符合条件的文本，当前文本有：{}",
            shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }

    /// 真的用鼠标事件点一次控件：按下、抬起各占一帧。
    /// 注意 egui 0.31 要把窗口先画一帧，区域内容才会进 `FullOutput::shapes`，
    /// 所以定位文本前固定跑两帧。
    fn click_at(state: &mut EditorState, ctx: &egui::Context, want: impl Fn(&str) -> bool) {
        frame_out(state, ctx, base_input());
        let out = frame_out(state, ctx, base_input());
        let p = text_center_if(&out.shapes, want);
        let mut press = base_input();
        press.events.push(egui::Event::PointerMoved(p));
        press.events.push(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        frame_out(state, ctx, press);
        let mut release = base_input();
        release.events.push(egui::Event::PointerMoved(p));
        release.events.push(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        frame_out(state, ctx, release);
    }

    fn click_text(state: &mut EditorState, ctx: &egui::Context, label: &str) {
        click_at(state, ctx, |t| t == label);
    }

    /// 状态栏消息原始值（`active_message` 带 4 秒 TTL，测试里直接读字段）。
    fn msg(state: &EditorState) -> String {
        state
            .message
            .as_ref()
            .map(|(s, _)| s.clone())
            .unwrap_or_default()
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nbs-gui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("临时目录可建");
        d
    }

    fn collect(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).expect("目录可读") {
            let p = e.expect("目录项").path();
            if p.is_dir() {
                collect(&p, base, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .expect("相对路径")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&p).expect("文本文件")));
            }
        }
    }

    /// 打开窗口 → 填选项 → 文件对话框 → dispatch 导出，全程走真实命令总线。
    /// 断言的是磁盘上的字节：目录分支逐文件和 `generate()` 对比，ZIP 分支查结构。
    #[test]
    fn datapack_window_dialog_and_folder_output_are_real() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);

        // ShowWindow 会像原版首帧那样把 dat_name 预填成歌曲名。
        dispatch(
            &mut state,
            commands::Command::ShowWindow(WindowKind::DatapackExport),
            &ctx,
        );
        assert_eq!(state.window, Some(WindowKind::DatapackExport));
        assert_eq!(state.datapack_options.name, "Rust Rewrite Demo");
        // 两个页签都要能画出来。
        frame(&mut state, &ctx);
        state.datapack_tab = 1;
        frame(&mut state, &ctx);
        state.datapack_tab = 0;

        state.datapack_options.namespace = "My Pack".into();
        state.datapack_options.path = "songs//Jazz".into();
        state.datapack_options.source = "music".into();
        state.datapack_options.mc_version = 1;
        state.datapack_options.enable_radius = true;
        state.datapack_options.radius = 40;
        state.datapack_options.visualizer = true;
        state.datapack_options.vis_type = nbs_io::datapack::VisType::Fountain;
        state.datapack_options.glow = true;
        state.doc.song.meta.loop_start = 1;
        state.datapack_options.enable_looping = true;

        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "zip", "默认 ZIP 分支就是 *.zip");
        assert_eq!(dialog.name, "Rust Rewrite Demo.zip", "默认名跟 dat_name 走");
        assert_eq!(state.window, Some(WindowKind::FileSave));

        let dir = scratch("folder");
        // 关掉 ZIP：对话框换成无扩展名的目录目标，导出结果必须是目录。
        state.datapack_options.use_zip = false;
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "", "非 ZIP 分支不过滤扩展名");
        assert_eq!(dialog.name, "Rust Rewrite Demo");

        let pack = dir.join("pack_folder");
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(pack.clone()),
            &ctx,
        );
        assert_eq!(
            state.window,
            Some(WindowKind::DatapackExport),
            "原版导出后回到本窗口"
        );

        let mut files = Vec::new();
        collect(&pack, &pack, &mut files);
        let plan =
            nbs_io::datapack::generate(&state.doc.song, &state.datapack_options, "pack_folder");
        assert_eq!(files.len(), plan.files.len(), "落盘文件数与计划不一致");
        let got: std::collections::BTreeMap<&str, String> =
            files.iter().map(|(p, c)| (p.as_str(), c.clone())).collect();
        for f in &plan.files {
            assert_eq!(
                got.get(f.path.as_str()).map(|s| s.as_str()),
                Some(f.content.as_str()),
                "{} 内容与生成计划不一致",
                f.path
            );
        }
        // 窗口里填的三项要真的决定包内结构（消毒后：命名空间 my_pack，路径 songs/jazz）。
        assert!(
            got.contains_key("data/my_pack/function/songs/jazz/rust_rewrite_demo/play.mcfunction")
        );
        assert!(got.contains_key("data/minecraft/tags/function/load.json"));
        // 目录分支的卸载命令不带 `file/` 前缀（原版按 dat_usezip 分两支），
        // 包名取对话框里选中的文件名。
        assert!(
            got["data/my_pack/function/songs/jazz/rust_rewrite_demo/uninstall.mcfunction"]
                .contains("datapack disable \"pack_folder\"")
        );
        let status = state.export_status.clone().unwrap_or_default();
        assert!(
            status.contains("/function my_pack:songs/jazz/rust_rewrite_demo/play"),
            "{status}"
        );

        // ZIP 分支：文件名没写 .zip 时原版会补上，产物必须是真 ZIP。
        let zip_in = dir.join("pack_zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        state.datapack_options.use_zip = true;
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(zip_in.clone()),
            &ctx,
        );
        let bytes = std::fs::read(dir.join("pack_zip.zip")).expect("补扩展名后写出 ZIP");
        assert_eq!(&bytes[..4], b"PK\x03\x04", "本地文件头魔数");
        assert!(bytes
            .windows(4)
            .rev()
            .take(32.min(bytes.len()))
            .any(|w| w == b"PK\x05\x06"));
        let names: Vec<String> = plan
            .files
            .iter()
            .map(|f| f.path.clone())
            .filter(|p| p.ends_with("play.mcfunction") || p.ends_with("pack.mcmeta"))
            .collect();
        assert_eq!(names.len(), 2);
        for n in &names {
            assert!(
                bytes.windows(n.len()).any(|w| w == n.as_bytes()),
                "中央目录里应能看到 {n}"
            );
        }
        // 选中的名字已经是 .zip 时不能变成 .zip.zip。
        assert!(!dir.join("pack_zip.zip.zip").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空名称必须像原版一样报错，并且不留半成品。
    #[test]
    fn datapack_export_refuses_an_empty_name() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.datapack_options.name = "  !! ".into();
        let dir = scratch("bad-name");
        let out = dir.join("nope.zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        let _lock = crate::frame_lock();
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        assert!(state.export_status.is_none(), "失败却写了成功状态");
        assert!(!out.exists(), "报错时不应留下产物");
        assert!(
            msg(&state).contains("请输入一个有效的数据包名"),
            "{:?}",
            msg(&state)
        );
        drop(_lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 路径套太多层要按原版规则拒绝（`string_count("/", dat_getpath(dat_path)) >= 5`）。
    #[test]
    fn datapack_export_refuses_too_many_subfolders() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        dispatch(
            &mut state,
            commands::Command::ShowWindow(WindowKind::DatapackExport),
            &ctx,
        );
        state.datapack_options.namespace = "ns".into();
        // 4 个斜杠（= 5 个子文件夹）是原版允许的上限，5 个才开始报错。
        state.datapack_options.path = "a/b/c/d/e".into();
        let dir = scratch("deep-path");
        let ok = dir.join("ok.zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(ok.clone()),
            &ctx,
        );
        assert!(ok.is_file(), "五个子文件夹应通过原版的闸门");

        state.datapack_options.path = "a/b/c/d/e/f".into();
        let out = dir.join("deep.zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        let _lock = crate::frame_lock();
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        assert!(!out.exists());
        assert!(msg(&state).contains("路径最多只能套五个文件夹"));
        drop(_lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真点一次鼠标：切页签、关掉「导出为 ZIP」、点「导出」。
    /// 证明界面上的控件写的就是导出器读的那份状态，而不只是「画出了几个控件」。
    #[test]
    fn clicking_window_controls_drives_the_export() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        dispatch(
            &mut state,
            commands::Command::ShowWindow(WindowKind::DatapackExport),
            &ctx,
        );
        assert!(state.datapack_options.use_zip, "原版默认开 ZIP");

        click_text(&mut state, &ctx, "导出为 ZIP");
        assert!(!state.datapack_options.use_zip, "点复选框没有改掉选项");

        click_text(&mut state, &ctx, "效果");
        assert_eq!(state.datapack_tab, 1, "点页签没有切过去");
        let glow_before = state.datapack_options.glow;
        click_text(&mut state, &ctx, "发光效果");
        assert_ne!(
            state.datapack_options.glow, glow_before,
            "效果器页的复选框没接上"
        );

        click_text(&mut state, &ctx, "设置");
        assert_eq!(state.datapack_tab, 0);
        click_text(&mut state, &ctx, "导出");
        // emit 的命令在下一帧开头落地（app::show 先 flush 再画）。
        frame(&mut state, &ctx);
        let dialog = state.file_dialog.as_ref().expect("点导出应打开保存对话框");
        assert_eq!(dialog.ext, "", "关掉 ZIP 后不再要求 .zip");
        assert_eq!(state.window, Some(WindowKind::FileSave));

        let dir = scratch("clicked");
        let pack = dir.join("by_click");
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(pack.clone()),
            &ctx,
        );
        assert!(pack.is_dir(), "非 ZIP 分支要写出目录");
        assert_eq!(
            std::fs::read_to_string(pack.join("pack.mcmeta")).expect("pack.mcmeta"),
            "{\n\t\"pack\": {\n\t\t\"pack_format\": 48,\n\t\t\"description\": \"Rust Rewrite Demo\\nMade with Note Block Studio\"\n\t}\n}"
        );
        assert!(!dir.join("by_click.zip").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 把当前挂着的模态框逐个答完（提问答「是」，提示答「确定」）。
    fn answer_all_modals(state: &mut EditorState, ctx: &egui::Context) {
        for _ in 0..10 {
            let Some(front) = state.modals.items.front().cloned() else {
                return;
            };
            click_text(state, ctx, if front.yes_no { "是" } else { "确定" });
        }
        panic!("模态框一直没答完，流程可能卡住了");
    }

    /// 真点鼠标走完原版 Schematic 导出流程：菜单 → `calculate_size()` 的提问 →
    /// 设计页三个单选 → 「使用默认值」的确认 → 方块页下拉 → 导出 → 磁盘上的 gzip 字节。
    #[test]
    fn clicking_through_the_schematic_export_window() {
        let ctx = egui::Context::default();
        isolate_prefs();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        // 原版第一次会写 settings.ini 记住「警告已看过」，测试预置成已看过，不碰用户配置。
        state.prefs.warning_schematic = true;
        // 菜单文案跟着界面语言走，钉成中文，免得跟着本机 locale 变。
        state.prefs.language = 1;

        click_text(&mut state, &ctx, "文件");
        click_text(&mut state, &ctx, "导出");
        click_text(&mut state, &ctx, "Minecraft 原理图…");
        answer_all_modals(&mut state, &ctx);
        assert_eq!(
            state.window,
            Some(WindowKind::SchematicExport),
            "提问答完才开窗"
        );
        assert_eq!(state.schematic_tab, 0, "开窗固定在「设计」页");
        assert!(
            state.schematic_stats.totalblocks[0] > 0,
            "演示曲应有可导出的音符"
        );

        click_text(&mut state, &ctx, "环形过道");
        assert_eq!(state.schematic_options.layout, 0, "布局单选没接上选项");
        click_text(&mut state, &ctx, "1.13+");
        assert!(state.schematic_options.structure);
        assert!(!state.schematic_options.minecraft_old);
        click_text(&mut state, &ctx, "使用命令方块");
        assert!(state.schematic_options.command_block);

        // 「使用默认值」先问「你确定吗？」，答否保持原样。
        click_text(&mut state, &ctx, "使用默认值");
        assert_eq!(state.modals.items.len(), 1, "原版这里先弹确认");
        assert!(state.modals.items[0].yes_no);
        click_text(&mut state, &ctx, "否");
        assert_eq!(state.schematic_options.layout, 0, "答否不该还原");

        click_text(&mut state, &ctx, "使用默认值");
        click_text(&mut state, &ctx, "是");
        assert_eq!(state.schematic_options.layout, 1, "答是才还原成默认");
        assert!(!state.schematic_options.include_locked);
        assert!(
            state.schematic_options.command_block,
            "command_block 不在原版默认值表里"
        );
        assert!(state.schematic_options.structure, "structure 同样不在表里");

        // 方块页：第一个乐器默认是 3, 0（泥土），改成 2, 0（草方块）。
        click_text(&mut state, &ctx, "方块");
        assert_eq!(state.schematic_tab, 1);
        click_at(&mut state, &ctx, |t| t == "3, 0");
        click_at(&mut state, &ctx, |t| t.starts_with("2, 0  "));
        assert_eq!(
            state.schematic_options.ins_block[0], 2,
            "下拉选择没写回选项"
        );
        assert_eq!(state.schematic_options.ins_data[0], 0);

        click_text(&mut state, &ctx, "导出");
        assert_eq!(
            state.window,
            Some(WindowKind::FileSave),
            "点「导出」应打开保存对话框"
        );
        let dir = scratch("schematic");
        let out = dir.join("by_click.nbt");
        // dispatch 与随后的文案断言整段持帧锁，理由同 gui_imports。
        let _lock = crate::frame_lock();
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        assert!(out.is_file(), "结构方块分支应写出 {}", out.display());
        let bytes = std::fs::read(&out).expect("导出必须写出真实文件");
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "前两端应是 gzip 魔数");

        // 原版保存成功后 `message("Schematic 已保存！")`，答完把导出窗口重新打开。
        assert_eq!(state.modals.items.len(), 1);
        assert_eq!(
            state.modals.items[0].title,
            "导出 Schematic",
            "全局语言 {}，prefs 语言 {}",
            crate::lang(),
            state.prefs.language
        );
        drop(_lock);
        click_text(&mut state, &ctx, "确定");
        assert_eq!(state.window, Some(WindowKind::SchematicExport));
        assert_eq!(state.schematic_tab, 0, "重新开窗回到设计页");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件 → 导入 → 从 Schematic 导入：先在磁盘上真造一张经典图样，再走文件对话框读回来；
    /// 顺带验 `load_song.gml` 的扩展名分流和原版的失败模态框。
    #[test]
    fn gui_imports_a_schematic_through_the_dialog_and_the_open_route() {
        let ctx = egui::Context::default();
        isolate_prefs();
        let dir = scratch("schematic-import");
        let file = dir.join("demo.schematic");
        nbs_io::schematic::export_schematic(
            &nbs_core::Song::demo(),
            &file,
            &nbs_io::schematic::SchematicOptions::default(),
        )
        .expect("导出经典图样");

        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        // 菜单文案跟着界面语言走，钉成中文，免得跟着本机 locale 变。
        state.prefs.language = 1;
        let recents = state.recent_files.len();
        // 真点菜单：文件 → 导入 → 从 Schematic 导入…
        click_text(&mut state, &ctx, "文件");
        click_text(&mut state, &ctx, "导入");
        click_text(&mut state, &ctx, "从 Schematic 导入…");
        let d = state.file_dialog.as_ref().expect("导入要弹文件对话框");
        assert!(!d.save, "导入用的是打开式对话框");
        assert_eq!(
            d.ext, "schematic",
            "原版过滤器是 Minecraft Schematics (*.schematic)"
        );
        assert_eq!(state.window, Some(WindowKind::FileOpen));

        state.doc.replace_song(nbs_core::Song::new());
        assert_eq!(state.doc.song.total_blocks, 0);
        // dispatch 与随后的文案断言要整段持帧锁：字符串在 dispatch 线程上构建，
        // 并行用例的帧会把全局语言翻走（生产是单主线程，没有这个窗口）。
        let _lock = crate::frame_lock();
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(file.clone()),
            &ctx,
        );
        assert!(state.file_dialog.is_none());
        assert_eq!(state.window, None);
        assert!(state.doc.song.total_blocks > 0, "读回来应该是有音符的谱面");
        assert!(
            msg(&state).starts_with("Schematic 导入完成"),
            "{}",
            msg(&state)
        );
        assert_eq!(
            state.doc.song.filename, None,
            "不学原版把 filename 指向 .schematic"
        );
        assert_eq!(
            state.recent_files.len(),
            recents,
            "原版 open_schematic() 不写最近列表"
        );
        drop(_lock);

        // 打开分流：.schematic 走导入，不会被当成 NBS 解析。
        let mut s2 = EditorState::new();
        s2.theme.apply(&ctx);
        s2.prefs.language = 1;
        let recents2 = s2.recent_files.len();
        s2.doc.replace_song(nbs_core::Song::new());
        let _lock = crate::frame_lock();
        dispatch(&mut s2, commands::Command::OpenPath(file), &ctx);
        assert!(s2.doc.song.total_blocks > 0);
        assert!(msg(&s2).starts_with("Schematic 导入完成"), "{}", msg(&s2));
        assert_eq!(s2.recent_files.len(), recents2);
        drop(_lock);

        // 坏文件：原版的 message()，标题不经 condstr，保持英文。
        let bad = dir.join("bad.schematic");
        std::fs::write(&bad, b"not a schematic at all").unwrap();
        click_text(&mut s2, &ctx, "文件");
        click_text(&mut s2, &ctx, "导入");
        click_text(&mut s2, &ctx, "从 Schematic 导入…");
        let _lock = crate::frame_lock();
        dispatch(&mut s2, commands::Command::DialogConfirm(bad), &ctx);
        assert_eq!(s2.modals.items.len(), 1, "失败要弹模态，不只是状态栏一行字");
        assert_eq!(s2.modals.items[0].title, "Error");
        assert_eq!(
            s2.modals.items[0].yes_no, false,
            "原版这里是 message()，只有确定键"
        );
        assert!(
            s2.modals.items[0]
                .text
                .contains("ERROR: Not a Schematic file."),
            "{}",
            s2.modals.items[0].text
        );
        drop(_lock);
        // 模态框要能画出来、答完就清空。
        frame(&mut s2, &ctx);
        click_text(&mut s2, &ctx, "确定");
        assert!(s2.modals.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 音色窗口：v6 起列出 20 种内置乐器，但快捷键只印原版真绑了的那 16 个
    /// （`control_draw.gml:822-843`：Ctrl+1..9 → 0..8，Ctrl+Shift+1..7 → 9..15）。
    /// 铜块一族按了没反应，就不许在界面上给它们编一个组合键。
    #[test]
    fn instruments_window_lists_twenty_and_advertises_only_real_bindings() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.prefs.language = 1;
        dispatch(
            &mut state,
            commands::Command::ShowWindow(WindowKind::Instruments),
            &ctx,
        );
        assert_eq!(state.window, Some(WindowKind::Instruments));
        // 刚 dispatch 完的第一帧只登记窗口，正文要第二帧才画出来。
        frame(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for name in [
            "Trumpet",
            "Exposed Trumpet",
            "Weathered Trumpet",
            "Oxidized Trumpet",
        ] {
            assert!(
                texts.iter().any(|t| t == name),
                "窗口里缺少内置乐器 {name}，当前文本：{}",
                texts.join(" | ")
            );
        }
        assert_eq!(
            texts
                .iter()
                .filter(|t| t.starts_with("Ctrl+Shift+"))
                .count(),
            7,
            "Ctrl+Shift+数字应当只有 7 个"
        );
        assert_eq!(
            texts
                .iter()
                .filter(|t| t.starts_with("Ctrl+") && !t.contains("Shift"))
                .count(),
            9,
            "Ctrl+数字应当只有 9 个"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("20 种内置乐器") && t.contains("索引 >= 20")),
            "底部说明要跟着 v6 的边界走"
        );
    }

    /// 欢迎窗真的按原版 700×430 布局渲染：文案齐全，点「创建歌曲」真的关窗。
    /// 出厂 show_welcome=true 时欢迎窗就是开机默认窗，这里显式打开避免读到人手 prefs。
    #[test]
    fn greeting_window_renders_and_create_button_closes_it() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        // 工厂默认跟系统语言走，测试里显式钉中文（英文分支有单独用例）。
        state.prefs.language = 1;
        let _cn = crate::LangGuard::set(1);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::Greeting);
        frame(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<&str> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect();
        for t in [
            "Note Block Studio",
            "要做什么？",
            "创建歌曲",
            "打开歌曲",
            "从 MIDI 文件生成",
            "浏览社区制作的歌曲",
            "NEW!",
            "关注 OpenNBS：",
            "Discord",
            "Twitter",
            "YouTube",
            "GitHub",
            "QQ 群",
            "捐赠",
            "noteblock.studio",
            "一个开源的 Minecraft 音乐制作软件",
            "版本 0.1.0 - 发布于 2026-11-16",
        ] {
            assert!(
                texts.iter().any(|s| *s == t),
                "欢迎窗缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
        // 更新状态行：出厂 check_update=true、还没查完 → 正在检查更新……
        assert!(
            texts.iter().any(|s| *s == "正在检查更新……"),
            "更新状态行缺失"
        );
        // 真点「创建歌曲」→ 关窗（原版 windowclose=1）。
        click_text(&mut state, &ctx, "创建歌曲");
        assert_eq!(state.window, None, "创建歌曲应当关掉欢迎窗");
    }

    /// 语言切到英文（原版 language=0 分支）后欢迎窗整套文案与行位切换：
    /// 第二行社区链接只剩 GitHub / Donate（中文还有 QQ 群 / 捐赠），各字符串
    /// 全部照 draw_window_greeting.gml 的 `if (language != 1)` 分支。
    #[test]
    fn greeting_renders_english_when_language_zero() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::Greeting);
        frame(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<&str> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect();
        for t in [
            "What do you want to do?",
            "Create a new song",
            "Load a song",
            "Generate song out of MIDI file",
            "Browse songs made by the community",
            "Go to noteblock.world",
            "Follow OpenNBS:",
            "Donate",
            "An open-source Minecraft music maker",
            "Version 0.1.0 - Released 2026-11-16",
            "Checking for updates...",
        ] {
            assert!(
                texts.iter().any(|s| *s == t),
                "英文欢迎窗缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
        // 中文专属的行和文案在英文分支不存在。
        for t in [
            "要做什么？",
            "创建歌曲",
            "QQ 群",
            "捐赠",
            "版本 0.1.0 - 发布于 2026-11-16",
        ] {
            assert!(!texts.iter().any(|s| *s == t), "英文分支不该出现「{t}」");
        }
        // 检查失败一行（-1）：英文原文没有句号，别改成带句号的形式。
        state.update_check.lock().unwrap().status = -1;
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<&str> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|s| *s == "Could not check for updates"));
    }

    /// 欢迎窗最近歌曲点开一个不存在的文件：弹「找不到文件」并把同路径条目清掉
    /// （draw_window_greeting.gml:219-231）。
    #[test]
    fn greeting_missing_recent_reports_and_clears_entries() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.prefs.language = 1;
        let _cn = crate::LangGuard::set(1);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::Greeting);
        let missing = PathBuf::from("Z:/definitely-not-here/ghost song.nbs");
        state.add_recent(missing.clone());
        state.add_recent(missing.clone()); // 重复条目也要一起清掉
        frame(&mut state, &ctx);
        click_text(&mut state, &ctx, "ghost song.nbs");
        assert_eq!(state.modals.items.len(), 1, "要弹错误框");
        assert_eq!(state.modals.items[0].title, "错误");
        assert!(state.modals.items[0].text.contains("找不到文件"));
        assert!(
            state.recent_files.iter().all(|r| r.path != missing),
            "同路径条目（含重复）要整串移走"
        );
    }

    /// 首选项「启动」组三行真实渲染 + 开发版本警告问句。
    #[test]
    fn preferences_startup_group_renders_and_warns_on_prerelease() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Preferences);
        frame(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for t in ["启动", "显示欢迎界面", "检查更新", "检查开发版本"] {
            assert!(
                texts.iter().any(|s| s == t),
                "启动组缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
        // 出厂 check_update=true：勾「检查开发版本」先弹原版警告问句，答「是」才真正打开。
        assert!(!state.prefs.check_prerelease);
        click_text(&mut state, &ctx, "检查开发版本");
        assert_eq!(state.modals.items.len(), 1, "开启开发版检查要先弹警告");
        assert!(state.modals.items[0].yes_no);
        assert!(!state.prefs.check_prerelease, "答之前不能先改值");
        click_text(&mut state, &ctx, "是");
        assert!(state.prefs.check_prerelease, "答「是」要真的打开");
    }

    /// 原版 draw_window_preferences.gml:412-421：界面页的语言下拉。
    /// 标题跟着语言切换，框里的值始终是两种语言的专名；菜单里选中文要写回
    /// prefs.language 并收起菜单，下一帧 app::show 把全局开关同步成中文。
    #[test]
    fn preferences_language_row_switches_interface_language() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 1;
        state.prefs.language = 0;
        // 窗口要暖一帧才真正铺开（既有 GUI 用例同款前置）。
        frame(&mut state, &ctx);
        let texts = |out: &egui::FullOutput| -> Vec<String> {
            out.shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect()
        };
        let out = frame_out(&mut state, &ctx, base_input());
        let t = texts(&out);
        assert!(
            t.iter().any(|s| s == "Language"),
            "英文界面标题应是 Language: {}",
            t.join(" | ")
        );
        assert!(t.iter().any(|s| s == "English"), "当前值行印的是语言专名");

        // 箭头是图标按钮，主题 / 帧率下拉的既有路径同样直接置位开菜单。
        // egui 的 Area 首帧是 sizing pass（不落形状），下一帧才真正画出来。
        state.prefs_language_menu = true;
        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        let t = texts(&out);
        for s in ["English", "简体中文"] {
            assert!(
                t.iter().any(|x| x == s),
                "菜单缺少「{s}」: {}",
                t.join(" | ")
            );
        }
        click_text(&mut state, &ctx, "简体中文");
        assert_eq!(state.prefs.language, 1, "选中要写回偏好");
        assert!(!state.prefs_language_menu, "选完菜单要收起");
        let out = frame_out(&mut state, &ctx, base_input());
        let t = texts(&out);
        assert!(
            t.iter().any(|s| s == "语言"),
            "切到中文后标题应是「语言」: {}",
            t.join(" | ")
        );
        // 还原全局开关（测试并行跑，别把其它用例的语言污染成中文）。
        state.prefs.language = 0;
        frame_out(&mut state, &ctx, base_input());
    }

    /// 首选项「外观」页的删除效果与琴键五线谱两行（原版 `draw_window_preferences.gml:455/466`
    /// 的 `remove_effect`、`show_notechart`）：要按原版坐标真渲染、点标签真翻运行时开关、
    /// 并且写进 prefs（原版两条都在 settings.ini 里）。
    /// 效果本身的画法另有专门用例（`gui_piano_tests` 的五线谱、commands 里的淡出边框动画）。
    #[test]
    fn preferences_appearance_rows_render_toggle_and_persist() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 2;
        frame(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for t in ["显示删除效果", "在琴键上时显示五线谱对应音符"] {
            assert!(
                texts.iter().any(|s| s == t),
                "外观页缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
        // 出厂值：删除效果开（control_create.gml:225）、五线谱关（:461）。
        assert!(state.flags.remove_effect);
        assert!(!state.flags.show_notechart);

        click_text(&mut state, &ctx, "显示删除效果");
        assert!(!state.flags.remove_effect, "点标签要真的关掉删除效果");
        click_text(&mut state, &ctx, "在琴键上时显示五线谱对应音符");
        assert!(state.flags.show_notechart, "点标签要真的打开五线谱");

        state.save_prefs();
        let back = nbs_io::prefs::Preferences::load();
        assert!(!back.remove_effect, "删除效果开关要落盘");
        assert!(back.show_notechart, "五线谱开关要落盘");
    }

    /// 原版页 3「访问」的两组：`draw_areaheader(22, 74, 456, 140, "鼠标滚轮")` 里的
    /// 三个档位 + 「滚动速度」拖拽行（`y1 + 150`，出厂 1），和
    /// `draw_areaheader(22, 240, 456, 105, "琴键与层")` 组名本身。
    /// 组名此前被写成「琴键」、滚动速度整行缺失，这条用例把它们钉回原版。
    #[test]
    fn preferences_usability_page_shows_wheel_and_layer_groups() {
        isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 3;
        frame(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for t in [
            "鼠标滚轮",
            "滚动速度：",
            "当在音符上使用鼠标滚轮改变音符属性",
            "琴键与层",
            "选择按下的键",
            // 出厂步长 1 直接画在拖拽位上（原版 mousewheel = 0 时也是数字本体）。
            "1",
        ] {
            assert!(
                texts.iter().any(|s| s == t),
                "访问页缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
    }
}

#[cfg(test)]
mod gui_encoder_tests {
    use commands::dispatch;

    use super::*;

    /// GUI → ffmpeg 的真实转码链路。默认不跑（依赖本机编码器），需要显式选择：
    /// `NBS_FFMPEG=<path> cargo test -p nbs-editor -- --ignored`
    /// 找不到 ffmpeg 时直接 panic，不静默通过。
    #[test]
    #[ignore = "需要 ffmpeg：设置 NBS_FFMPEG 或把它放进 PATH"]
    fn gui_encodes_flac_and_removes_the_temp_wav() {
        let ffmpeg = nbs_io::audio_export::find_ffmpeg()
            .expect("未找到 ffmpeg：设置 NBS_FFMPEG 或放进 PATH 后再跑本用例");
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.export_format = "FLAC".into();
        state.prefs.export_sample_rate = 44100;
        state.prefs.export_channels = 2;
        dispatch(
            &mut state,
            crate::commands::Command::OpenDialog(state::DialogAction::ExportAudio),
            &ctx,
        );
        assert_eq!(state.file_dialog.as_ref().unwrap().ext, "flac");

        let out = std::env::temp_dir().join("nbs_gui_export.flac");
        let tmp_wav = out.with_extension("nbs-export-tmp.wav");
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&tmp_wav);
        dispatch(
            &mut state,
            crate::commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );

        let bytes = std::fs::read(&out).unwrap_or_else(|e| panic!("{ffmpeg:?} 转码失败: {e}"));
        assert_eq!(&bytes[..4], b"fLaC", "产物不是 FLAC");
        assert!(!tmp_wav.exists(), "中间 WAV 必须清掉");
        assert!(state
            .export_status
            .as_deref()
            .unwrap_or_default()
            .contains("FLAC 已导出"));
        let _ = std::fs::remove_file(&out);
    }
}

/// 底部钢琴：几何、点击、边缘平移，全部对着原版 `draw_piano` 的绝对像素。
#[cfg(test)]
mod gui_piano_tests {
    use super::*;

    fn input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        }
    }

    fn run(state: &mut EditorState, ctx: &egui::Context, inp: egui::RawInput) -> egui::FullOutput {
        let _lock = frame_lock();
        ctx.run(inp, |ctx| crate::show(ctx, state))
    }

    /// `EditorState::new()` 会把本机 prefs 拷进 flags，而这几条用例断言的是原版默认视图。
    /// 用户把钢琴键盘关掉时也必须照样跑，所以在这里把相关开关钉回出厂值。
    fn piano_state() -> EditorState {
        let mut state = EditorState::new();
        state.flags.show_piano = true;
        state.flags.show_keyboard = true;
        state.prefs.keys_to_show = 30;
        // 这三条断言量的是原版绝对像素（键宽 40、步进 39、y=746），用户盘里存着
        // 缩放档（比如 150%）的话整帧都被等比放大，一根键都筛不出来。测试环境里
        // `Context::default()` 的系统倍率是 1.0，钉成出厂值就恒等于 1.0。
        state.prefs.window_scale = None;
        // 开机流程会开欢迎窗；钢琴测试断言整帧的形状/贴图，欢迎窗和它最近的
        // 歌曲行（frame5 是 320×16 贴图）会混进来，靶子必须是裸编辑器。
        state.window = None;
        state
    }

    /// 琴键现在是 `spr_piano` 贴图，egui 0.31 把贴图统一走 `Shape::Mesh`，一帧 40×121、按键步进 39。
    fn images(out: &egui::FullOutput) -> Vec<egui::Rect> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Mesh(m) => Some(m.calc_bounds()),
                _ => None,
            })
            .collect()
    }

    fn texts(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn lines(out: &egui::FullOutput) -> Vec<([egui::Pos2; 2], egui::Stroke)> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::LineSegment { points, stroke } => Some((*points, *stroke)),
                _ => None,
            })
            .collect()
    }

    /// 原版：白键步进 39、顶边在 `rh - 154`，整排居中；黑键画在左边 19px、上边 7px 处。
    /// 1440 宽时 `keysshow = min(30, floor((1440-32)/39)) = 30`，左边留 (1440-1170)/2 = 135。
    #[test]
    fn piano_uses_the_original_fixed_key_geometry() {
        let ctx = egui::Context::default();
        let mut state = piano_state();
        run(&mut state, &ctx, input());
        let out = run(&mut state, &ctx, input());
        let quads = images(&out);

        let mut whites: Vec<f32> = quads
            .iter()
            .filter(|r| {
                (r.width() - 40.0).abs() < 0.6
                    && (r.height() - 121.0).abs() < 0.6
                    && (r.min.y - 746.0).abs() < 0.6
            })
            .map(|r| r.min.x)
            .collect();
        whites.sort_by(|a, b| a.total_cmp(b));
        whites.dedup();
        assert_eq!(whites.len(), 30, "白键应当是 30 根，实际 {}", whites.len());
        assert_eq!(whites.first().copied(), Some(135.0));
        assert_eq!(whites.last().copied(), Some(135.0 + 29.0 * 39.0));

        let mut blacks: Vec<f32> = quads
            .iter()
            .filter(|r| {
                (r.width() - 40.0).abs() < 0.6
                    && (r.height() - 121.0).abs() < 0.6
                    && (r.min.y - 739.0).abs() < 0.6
            })
            .map(|r| r.min.x)
            .collect();
        blacks.sort_by(|a, b| a.total_cmp(b));
        blacks.dedup();
        // 30 根白键里有 8 根的左边没有黑键（C、F 以及最左那根），所以是 21 根黑键。
        assert_eq!(blacks.len(), 21, "黑键应当是 21 根，实际 {}", blacks.len());
        // 黑键画在白键左缘再往左 19px 处。
        assert_eq!(blacks.first().copied(), Some(135.0 + 39.0 - 19.0));

        let t = texts(&out);
        assert!(
            t.iter().any(|s| s == "F2"),
            "最左白键是 20 号键 = F2，实际文本 {t:?}"
        );
        assert!(t.iter().any(|s| s == "G6"), "最右白键是 70 号键 = G6");
        assert!(
            !t.iter().any(|s| s == "<" || s == ">"),
            "原版没有翻页按钮，界面上不该出现 < 或 >"
        );
    }

    /// 原版按下琴键即选中并把力度 / 平衡 / 音高偏移复位成 100 / 100 / 0。
    #[test]
    fn pressing_a_key_selects_it_and_resets_the_note_properties() {
        let ctx = egui::Context::default();
        let mut state = piano_state();
        state.current.velocity = 42;
        state.current.panning = 7;
        state.current.pitch = 30;
        run(&mut state, &ctx, input());
        let mut press = input();
        let p = egui::pos2(150.0, 800.0);
        press.events.push(egui::Event::PointerMoved(p));
        press.events.push(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        run(&mut state, &ctx, press);
        assert_eq!(state.current.key, 20, "最左白键是 20 号键");
        assert_eq!(state.current.velocity, 100);
        assert_eq!(state.current.panning, 100);
        assert_eq!(state.current.pitch, 0);
    }

    /// 原版没有翻页按钮，靠鼠标停在最左 / 最右白键上自动平移音域。
    #[test]
    fn hovering_an_edge_key_shifts_the_keyboard() {
        let ctx = egui::Context::default();
        let mut state = piano_state();
        run(&mut state, &ctx, input());
        let start = state.piano_start;
        assert_eq!(start, 12, "默认 startkey = 27 - keysshow/2");

        let mut hover = input();
        hover
            .events
            .push(egui::Event::PointerMoved(egui::pos2(150.0, 800.0)));
        for _ in 0..60 {
            run(&mut state, &ctx, hover.clone());
        }
        assert!(
            state.piano_start < start,
            "停在最左白键上应当往低音平移，实际 startkey = {}",
            state.piano_start
        );

        let mut state = piano_state();
        run(&mut state, &ctx, input());
        let start = state.piano_start;
        let mut hover = input();
        // 最右一根白键：x = 135 + 29*39 = 1266，宽 39。
        hover
            .events
            .push(egui::Event::PointerMoved(egui::pos2(1280.0, 800.0)));
        for _ in 0..60 {
            run(&mut state, &ctx, hover.clone());
        }
        assert!(
            state.piano_start > start,
            "停在最右白键上应当往高音平移，实际 startkey = {}",
            state.piano_start
        );
    }

    /// 开「在琴键上时显示五线谱对应音符」并悬停最左白键（key 20，列号 startkey + a = 12）：
    /// 面板 / 指针 / 谱号 / 音符贴图和五线谱横线、括号、加线都按 draw_notechart.gml
    /// 的坐标出现在琴键上方（y0 = 746，yy = 714，x1 = 153，xx = median(0, 121, 1120) = 121）。
    #[test]
    fn notechart_draws_over_the_hovered_key() {
        let ctx = egui::Context::default();
        let mut state = piano_state();
        state.flags.show_notechart = true;
        run(&mut state, &ctx, input());
        let mut hover = input();
        hover
            .events
            .push(egui::Event::PointerMoved(egui::pos2(150.0, 800.0)));
        let out = run(&mut state, &ctx, hover);

        let quads = images(&out);
        let at = |x: f32, y: f32, w: f32, h: f32| {
            quads.iter().any(|r| {
                (r.min.x - x).abs() < 0.6
                    && (r.min.y - y).abs() < 0.6
                    && (r.width() - w).abs() < 0.6
                    && (r.height() - h).abs() < 0.6
            })
        };
        assert!(at(121.0, 395.0, 320.0, 320.0), "面板画在 (xx, yy-319)");
        assert!(at(153.0, 714.0, 20.0, 20.0), "指针画在 (x1, yy)");
        assert!(at(161.0, 462.0, 30.0, 82.0), "G 谱号（原点 18,50）");
        assert!(at(161.0, 563.0, 40.0, 44.0), "F 谱号（原点 8,13）");
        assert!(at(243.0, 558.0, 49.0, 150.0), "音符（原点 30,74）");

        let ls = lines(&out);
        // 10 条五线谱横线跳过第 5 条：y = yy-250+16a，x 从 xx+32 到 xx+288。
        for a in 0..11i32 {
            let y = 464.0 + 16.0 * a as f32;
            let hit = ls.iter().any(|(p, _)| {
                (p[0].y - y).abs() < 0.6
                    && (p[0].x - 153.0).abs() < 0.6
                    && (p[1].x - 409.0).abs() < 0.6
                    && (p[1].y - y).abs() < 0.6
            });
            assert_eq!(
                hit,
                a != 5,
                "五线谱横线 a={a} 应当{}",
                if a == 5 { "被跳过" } else { "存在" }
            );
        }
        // 左括号两条实线：y 464..528 与 560..624（中缝留给中加线）。
        for (y0, y1) in [(464.0, 528.0), (560.0, 624.0)] {
            assert!(
                ls.iter().any(|(p, _)| {
                    (p[0].x - 153.0).abs() < 0.6
                        && (p[0].y - y0).abs() < 0.6
                        && (p[1].x - 153.0).abs() < 0.6
                        && (p[1].y - y1).abs() < 0.6
                }),
                "左括号竖线 {y0}..{y1} 缺失"
            );
        }
        // key = 12：没有中加线 / 上加线，只有 1 条下加线（y = 640，x 243..301）。
        assert!(
            ls.iter().any(|(p, _)| {
                (p[0].y - 640.0).abs() < 0.6
                    && (p[0].x - 243.0).abs() < 0.6
                    && (p[1].x - 301.0).abs() < 0.6
            }),
            "下加线缺失"
        );
        assert!(
            !ls.iter().any(|(p, _)| (p[0].y - 544.0).abs() < 0.6),
            "第 5 条线必须空出来"
        );
    }

    /// 首选项开关关闭时，五线谱一个形状都不该有（原版 show_notechart 门）。
    #[test]
    fn notechart_is_gated_by_the_flag() {
        let ctx = egui::Context::default();
        let mut state = piano_state();
        assert!(!state.flags.show_notechart, "出厂默认应当是关");
        run(&mut state, &ctx, input());
        let mut hover = input();
        hover
            .events
            .push(egui::Event::PointerMoved(egui::pos2(150.0, 800.0)));
        let out = run(&mut state, &ctx, hover);
        assert!(
            !images(&out).iter().any(|r| (r.width() - 320.0).abs() < 0.6),
            "开关关闭时不应出现五线谱面板"
        );
    }
}
