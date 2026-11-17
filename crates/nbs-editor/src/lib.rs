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
/// 原版 `link_changelog`（`macros.gml:14`）：更新历史窗「查看更旧版本……」打开的页。
pub const LINK_CHANGELOG: &str = "https://opennbs.org/changelog";
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

/// 跑帧的测试互相串行。语言已经改成按线程钉（见 `nbs_core::lang`），这把锁不再防串台，
/// 只起「一次只跑一个 GUI 用例」的作用：GUI 用例要走完整帧循环，并行时 egui 的
/// Context 全局状态会让红了的用例没法复现。
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

/// 测试用的配置目录是全进程共用的一份：`NBS_RUST_CONFIG_DIR` 是环境变量，
/// 谁后设谁生效，`prefs.json` 也只有一份。不串行时，一个用例的「落盘 → 回读 → 断言」
/// 会被另一个用例的删除或落盘插进来，红不红全看调度顺序。
#[cfg(test)]
static PREFS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `isolate_prefs()` 交出来的串行凭据，持到用例结束。
#[cfg(test)]
#[must_use = "要持到用例结束：中途放掉，并行用例会把 prefs.json 删掉或改走"]
pub(crate) struct PrefsLock(
    #[allow(dead_code)] // 只为持到 drop 而存在，用例不读它。
    std::sync::MutexGuard<'static, ()>,
);

/// 只把配置目录指到临时目录，不动里面的文件。给不校验首选项取值、
/// 只是不想读写用户真配置的用例用。
#[cfg(test)]
pub(crate) fn scratch_prefs_dir() {
    let dir = std::env::temp_dir().join("nbs-rust-test-prefs");
    let _ = std::fs::create_dir_all(&dir);
    std::env::set_var("NBS_RUST_CONFIG_DIR", &dir);
}

/// 出厂值断言 / 不该写用户配置的测试都用它：指向一个空的配置目录，
/// 调试界实时改过缩放 / 音量、或测试期间真的落盘过偏好，单测都会莫名其妙变红。
/// 目录里旧一轮跑剩的 prefs.json 也要清掉，否则上次测试写下的开关值
/// 会污染这一轮的「出厂值」断言。
#[cfg(test)]
pub(crate) fn isolate_prefs() -> PrefsLock {
    let lock = PREFS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    scratch_prefs_dir();
    let _ = std::fs::remove_file(
        std::env::temp_dir()
            .join("nbs-rust-test-prefs")
            .join("prefs.json"),
    );
    PrefsLock(lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_defaults() {
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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

    /// :747-753 的版本变化分支：亮「更新成功」+ 强制横幅 + 切 Fluent 主题 + 开 w_update。
    #[test]
    fn enter_startup_flags_update_success_on_version_change() {
        let _prefs = isolate_prefs();
        let mut s = EditorState::new();
        s.prefs.last_version = Some("0.0.1".into());
        s.prefs.show_welcome = false;
        // 模拟全新一次开机：原版 create 时 window 还没有任何值。
        s.window = None;
        s.theme = Theme::new(ThemeKind::Dark);
        let saved_theme = s.prefs.theme;
        s.enter_startup();
        assert!(s.update_success, "版本变了要亮「更新成功！」");
        assert!(s.donate_banner, "版本变化强制亮横幅");
        assert_eq!(
            s.window,
            Some(WindowKind::Update),
            ":750 的 window = w_update 排在 :707 之后，欢迎窗会被顶掉"
        );
        assert_eq!(
            s.theme.kind,
            ThemeKind::Fluent,
            ":749 把运行时主题切成 Fluent"
        );
        assert!(
            s.theme.fdark,
            ":748 `if (theme = 2) fdark = 1` → 深色 Fluent"
        );
        assert_eq!(
            s.prefs.theme, saved_theme,
            "prefs.theme 要等「确认」里的 save_settings 才写回，这里不该动"
        );
        // 同版本再启动（退出时 save_prefs 已把 last_version 写成本版）→ 不算更新成功
        s.prefs.last_version = Some(crate::VERSION.to_string());
        s.update_success = false;
        s.window = None;
        s.enter_startup();
        assert!(!s.update_success);
        assert_eq!(s.window, None, "show_welcome 关着就不开欢迎窗");
    }

    /// 原版 :69-75 的分行规则：只按换行切，行首字母是 `C` 才走粗体，
    /// 末尾不带换行符的那段按 `string_count("\n")` 计数，压根进不了表。
    #[test]
    fn changelog_lines_follow_the_original_split() {
        let lines =
            crate::windows::changelog_lines("Changes in v1:\n\n* alpha\nbeta\n未结尾的一段");
        assert_eq!(
            lines,
            vec![
                ("Changes in v1:".to_string(), true),
                (String::new(), false),
                ("* alpha".to_string(), false),
                ("beta".to_string(), false),
            ]
        );
    }

    /// 随程序那份 `Data/changelog.txt` 必须真的读得到，否则更新历史窗只会走
    /// 原版 :7-16 那句「找不到更新历史！」。
    #[test]
    fn bundled_changelog_is_readable() {
        let _prefs = isolate_prefs();
        let src = EditorState::new()
            .changelog
            .expect("assets/changelog.txt 应当能在资源目录里找到");
        assert!(!src.contains('\r'), "载入时要把 CRLF 归一成 LF");
        let lines = crate::windows::changelog_lines(&src);
        assert!(lines.len() > 100, "装机版那份是 {} 行", lines.len());
        assert!(
            lines[0].0.starts_with("Changes in v3.12.0-rc.1"),
            "首行：{}",
            lines[0].0
        );
        assert!(lines[0].1, "首行以 C 开头 → 原版画粗体");
    }

    /// check_updates.gml 回调的 UI 侧：后台写 1 → 弹一次问句；不重复弹；其他状态不弹。
    #[test]
    fn poll_update_check_asks_once_per_new_version() {
        let _prefs = isolate_prefs();
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

    /// 帮助菜单倒数第二条（原版 `control_draw.gml:1783/1878` 的 `Changelist...` /
    /// 「更新历史......」）＝ `menu_click.gml:224/229` 的 `window = w_changelist`。
    #[test]
    fn help_menu_opens_changelist() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        click_text(&mut state, &ctx, "帮助");
        click_text(&mut state, &ctx, "更新历史…");
        assert_eq!(
            state.window,
            Some(WindowKind::Changelist),
            "帮助 → 更新历史… 要开 w_changelist"
        );
    }

    /// 正文画的是随程序那份 changelog 的头 18 行；谢词只有 `w_update` 有（:52-59）。
    #[test]
    fn changelog_window_draws_the_bundled_lines() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Changelist);
        let shown = drawn_texts(&mut state, &ctx);
        assert!(
            shown
                .iter()
                .any(|t| t.starts_with("Changes in v3.12.0-rc.1")),
            "正文应当是 assets/changelog.txt 的行：{shown:?}"
        );
        assert!(
            !shown.iter().any(|t| t.contains("感谢你更新到")),
            "帮助菜单进来的那版不该有谢词"
        );
        state.window = Some(WindowKind::Update);
        let shown = drawn_texts(&mut state, &ctx);
        assert!(
            shown
                .iter()
                .any(|t| t == &format!("感谢你更新到 {}版本！", crate::VERSION)),
            "原版 :57 的中文谢词：{shown:?}"
        );
    }

    /// 「确认」的去处按窗口种类分岔（:88-95）：帮助菜单进来的关窗，
    /// `w_update` 进来的回欢迎窗并把新版本号写回设置。
    #[test]
    fn changelog_ok_button_targets_differ_by_kind() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.prefs.last_version = Some("0.0.1".into());
        state.window = Some(WindowKind::Changelist);
        click_text(&mut state, &ctx, "确认");
        assert_eq!(state.window, None, "w_changelist 的确认只是关窗（:94）");
        state.window = Some(WindowKind::Update);
        click_text(&mut state, &ctx, "确认");
        assert_eq!(state.window, Some(WindowKind::Greeting), ":90 回欢迎窗");
        assert_eq!(
            state.prefs.last_version.as_deref(),
            Some(crate::VERSION),
            ":91 那句 save_settings 要先把版本号记进 prefs"
        );
        assert_eq!(
            nbs_io::prefs::Preferences::load().last_version.as_deref(),
            Some(crate::VERSION),
            "而且真的落盘"
        );
    }

    /// 读不到更新历史时原版 :7-16 弹一句 message 就收窗：`w_update` 退回欢迎窗。
    #[test]
    fn missing_changelog_pops_message_and_closes() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.changelog = None;
        state.window = Some(WindowKind::Changelist);
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "w_changelist 直接关");
        state.window = Some(WindowKind::Update);
        frame(&mut state, &ctx);
        assert_eq!(
            state.window,
            Some(WindowKind::Greeting),
            "w_update 退回欢迎窗"
        );
        assert_eq!(
            state.modals.items.len(),
            1,
            "两次各弹一句「找不到更新历史！」"
        );
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        let _prefs = isolate_prefs();
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
        // 出厂值不在这里验：临时配置目录可能被并行用例写过，出厂值由
        // nbs-io/prefs.rs 的 `remove_effect` / `show_notechart` 用例钉。
        // 本用例只管界面点击和落盘。
        state.flags.remove_effect = true;
        state.flags.show_notechart = false;

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
    /// 三个档位 + 「滚动速度」拖拽行（`y1 + 150`，出厂 1）+ 「悬停层预览」
    /// （`y1 + 198`），和 `draw_areaheader(22, 240, 456, 105, "琴键与层")` 组名本身。
    /// 组名此前被写成「琴键」、滚动速度与悬停层预览两行缺失，这条用例把它们钉回原版。
    #[test]
    fn preferences_usability_page_shows_wheel_and_layer_groups() {
        let _prefs = isolate_prefs();
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
            "将悬停层的音量和声道调整应用于音符预览声音",
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
        assert!(!state.prefs.layerhov_vppreview, "悬停层预览出厂关闭");
        click_text(
            &mut state,
            &ctx,
            "将悬停层的音量和声道调整应用于音符预览声音",
        );
        assert!(
            state.prefs.layerhov_vppreview,
            "点标签要真的打开悬停层预览"
        );
        state.save_prefs();
        let back = nbs_io::prefs::Preferences::load();
        assert!(back.layerhov_vppreview, "悬停层预览开关要落盘");
    }

    /// 标尺速度格的原版就地输入框（`w_settempo`）：开框时按
    /// `string_format(real_tempo * 倍数, 0, 2)` 播种，Esc 只关窗不提交，
    /// 离开输入框才提交。这里走的是精灵图缺位时图层栏的退路版那颗「速度…」按钮，
    /// 两条路径共用同一套草稿字段和同一个提交函数。
    #[test]
    fn inline_tempo_box_opens_in_place_and_commits_when_left() {
        // 定点按下 + 抬手，各占一帧（`click_at` 那套是从文字反查坐标，这里要裸坐标）。
        fn tap_at(state: &mut EditorState, ctx: &egui::Context, p: egui::Pos2) {
            for pressed in [true, false] {
                let mut raw = base_input();
                raw.events.push(egui::Event::PointerMoved(p));
                raw.events.push(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
                frame_out(state, ctx, raw);
            }
        }

        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.prefs.use_bpm = false;
        state.window = None;
        state.doc.replace_song(nbs_core::model::Song::new());
        state.doc.song.meta.tempo = 10.0;

        // 原版点的是标尺上 (108, 57, 64, 22) 那一格；数值文字居中在 136，所以它的
        // 中心必落在那条判定框里。框外取右边 90px 的「小节·拍·十六分」，
        // 那一格只有悬停说明，抬手落在上面不会改动谱面。
        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        let on_box = text_center_if(&out.shapes, |t| t == "10.00 t / s");
        let off_box = on_box + egui::vec2(90.0, 0.0);

        tap_at(&mut state, &ctx, on_box);
        assert_eq!(
            state.tempo_input.as_deref(),
            Some("10.00"),
            "点速度格要就地弹出输入框，初值是 `string_format(速度 * 倍数, 0, 2)`"
        );
        assert!(state.window.is_none(), "原版这一格不是带标题的窗口");
        let shown = drawn_texts(&mut state, &ctx);
        assert!(
            !shown.iter().any(|t| t == "10.00 t / s"),
            "原版要 `window != w_settempo` 才画数值，开框后界面上不该再有它：{shown:?}"
        );

        // 框里改成别的值再按 Esc：`draw_window_set_tempo.gml:47-51` 只关窗、不改速度。
        state.tempo_input = Some("1200".into());
        let mut raw = base_input();
        raw.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        });
        frame_out(&mut state, &ctx, raw);
        assert_eq!(state.tempo_input, None, "Esc 要把输入框收掉");
        assert_eq!(state.doc.song.meta.tempo, 10.0, "Esc 是丢弃，速度不该动");

        // 提交只在「这次按下不在框里、这次抬手也在框外」时发生（`:26-46`）。
        tap_at(&mut state, &ctx, on_box);
        state.tempo_input = Some("1200".into());
        tap_at(&mut state, &ctx, off_box);
        assert_eq!(state.tempo_input, None, "抬手在框外要把输入框收掉");
        assert_eq!(
            state.doc.song.meta.tempo, 12.0,
            "1200 要先除以 100 再夹进 median(0.25, 值, 180)"
        );
    }

    /// 原版 `w_tempotapper`（速度测量器）：按 T 打点 → 界面按速度单位显示测到的值，
    /// 「双倍速度」可勾，确认把 `ctempo * 2 / 15` 写进歌曲速度并关窗。
    #[test]
    fn tempo_tapper_measures_taps_and_applies_the_tempo() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::TempoTapper);
        // 每 500 ms 一下 = 120 BPM；速度单位出厂 t/s，界面画 120 / 15 = 8。
        state.tapper.register_tap(0);
        state.tapper.register_tap(500);
        state.tapper.register_tap(1000);
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
        for t in ["t/s (按 T 打速度)", "双倍速度", "确认", "取消", "8"] {
            assert!(
                texts.iter().any(|s| s == t),
                "速度测量器缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
        click_text(&mut state, &ctx, "双倍速度");
        frame(&mut state, &ctx);
        assert!(state.prefs.tapdouble, "点标签要真的勾上双倍速度");
        click_text(&mut state, &ctx, "确认");
        frame(&mut state, &ctx);
        assert_eq!(
            state.doc.song.meta.tempo,
            16.0,
            "120 BPM 双倍 = 240 / 15 = 16 t/s"
        );
        assert_eq!(state.window, None, "确认即关窗（原版 windowclose = 1）");
        // 原版 OK 里清的那三条（draw_window_tempo_tapper.gml:56-58）；
        // 但双倍速度是 settings.ini 里的偏好（save_settings.gml:71），不跟着清。
        assert_eq!(state.tapper.bpm, 0.0, "确认后打点测量值清零");
        assert!(!state.tapper.tapping, "确认后 tapping 清零");
        assert_eq!(state.tapper.last_ms, 0, "确认后 ltime 清零");
        state.save_prefs();
        let back = nbs_io::prefs::Preferences::load();
        assert!(back.tapdouble, "双倍速度勾选要落盘（save_settings.gml:71）");
    }

    /// 原版 `w_saveoptions`（文件菜单「保存选项......」，`draw_window_save_options.gml`）：
    /// 140×162，七行版本单选 + 一颗确定。行按原版坐标画，锁行条件是自定义乐器数
    /// （`min_version` 与 v6 的 `nbs_custom_instrument_limit(6)`），点确定走
    /// `ApplySaveOptions`，非当前版本先问一句。
    #[test]
    fn save_options_window_locks_rows_by_instrument_count() {
        use commands::Command;
        let _prefs = isolate_prefs();
        let _cn = crate::LangGuard::set(1);
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        dispatch(&mut state, Command::OpenSaveOptions, &ctx);
        assert_eq!(state.window, Some(WindowKind::SaveOptions));
        assert_eq!(state.save_options_version, 6, "新歌的保存版本播种给单选");
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
        for t in ["v6", "v5", "v4", "v3", "v2", "v1", "经典", "确定"] {
            assert!(
                texts.iter().any(|s| s == t),
                "保存选项窗缺少「{t}」，当前文本：{}",
                texts.join(" | ")
            );
        }
        // 自定义音色 0 个：`min_version = 0`，最旧那行也能点。
        click_text(&mut state, &ctx, "经典");
        frame(&mut state, &ctx);
        assert_eq!(state.save_options_version, 0, "点「经典」要选中 v0");
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(state.modals.items.len(), 1, "退到旧版本要先问一句");
        assert_eq!(state.doc.song.meta.save_version, 6, "没答应不改歌曲");
        assert_eq!(
            state.save_options_version, 6,
            "原版否分支把单选退回歌曲版本，提前做掉"
        );
        dispatch(&mut state, Command::AnswerModal(true), &ctx);
        assert_eq!(state.doc.song.meta.save_version, 0, "答是才写回");
        assert_eq!(state.window, None, "写完关窗");

        // 19 个自定义音色：原版 `min_version = 5`，0..4 五行锁住，单选低于 5 的要抬上来。
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        state.doc.song.meta.save_version = 4;
        let custom = |i: usize| nbs_core::model::CustomInstrument {
            name: i.to_string(),
            filename: String::new(),
            key: 45,
            press: true,
        };
        state.doc.song.custom_instruments = (0..19).map(custom).collect();
        dispatch(&mut state, Command::OpenSaveOptions, &ctx);
        frame(&mut state, &ctx);
        frame_out(&mut state, &ctx, base_input());
        assert_eq!(state.save_options_version, 5, "低于 min_version 的单选要抬上来");
        click_text(&mut state, &ctx, "经典");
        frame(&mut state, &ctx);
        assert_eq!(state.save_options_version, 5, "v0 那行是锁的，点不动");
        // v6 上限那条（乐器数 > 236 时把 6 改回 5）由 nbs-core 的
        // `save_version_options_follows_the_instrument_count` 钉；这里不画：
        // 237 条自定义音色已经把 `instrument_list()` 的 u8 索引撑爆（见报告）。
    }

    /// 一帧画到屏幕上的全部文字（菜单/子菜单内容只能这样查，控件本身没有 API），
    /// 连起点一起返回：既看有没有画出来，也看画在哪儿。画两帧，弹层才定下尺寸。
    fn drawn_at(state: &mut EditorState, ctx: &egui::Context) -> Vec<(String, egui::Pos2)> {
        frame_out(state, ctx, base_input());
        let out = frame_out(state, ctx, base_input());
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos)),
                _ => None,
            })
            .collect()
    }

    fn drawn_texts(state: &mut EditorState, ctx: &egui::Context) -> Vec<String> {
        drawn_at(state, ctx).into_iter().map(|(t, _)| t).collect()
    }

    /// 三个宏窗口的靶子谱：三列 × 两层，全选后抬出一个 6 音符的浮动选区。
    /// `scratch_prefs_dir` 放在 `new()` 之前，免得读写到用户真机的配置；这里不删
    /// prefs.json，也不校验首选项取值，没必要占出厂值用例那把串行锁。
    fn state_with_selection() -> EditorState {
        scratch_prefs_dir();
        let mut state = EditorState::new();
        state.prefs.language = 1;
        state.window = None;
        // 空谱起步：`EditorState::new()` 自带演示曲，全选会连它一起抬进选区。
        state.doc.replace_song(nbs_core::model::Song::new());
        for (col, key) in [(0u32, 50u8), (1, 52), (2, 55)] {
            state
                .doc
                .place_note_raw(col, 0, nbs_core::model::Note::new(0, key));
            state
                .doc
                .place_note_raw(col, 1, nbs_core::model::Note::new(0, key + 12));
        }
        state.doc.select_all();
        assert_eq!(state.doc.selection.len(), 6, "测试前提：全选抬出 6 个音符");
        state
    }

    /// 原版三个宏窗口开头都是 `if (songs[song].selected = 0) { window = 0 }`：
    /// 没有选区，窗口画一帧就自己关掉，不会退化成「对整首歌生效」。
    #[test]
    fn macro_windows_self_close_without_a_selection() {
        let ctx = egui::Context::default();
        for kind in [
            WindowKind::Arpeggio,
            WindowKind::Portamento,
            WindowKind::Stagger,
        ] {
            let mut state = state_with_selection();
            state.theme.apply(&ctx);
            state.doc.place_selection();
            assert!(state.doc.selection.is_empty(), "测试前提：先把选区放回歌上");
            state.window = Some(kind.clone());
            // emit 的关窗命令要到下一帧开头才落地。
            frame(&mut state, &ctx);
            frame(&mut state, &ctx);
            assert_eq!(state.window, None, "{kind:?} 没有选区应当自己关掉");
        }
    }

    /// 琶音窗口：规律里没有 `|` 只弹错误、窗口留着；补上规律后按列循环取值
    /// （原版 `for (i = 0; i < arplen; i++)` 每一轮吃掉一整列），确定则落回歌曲并关窗。
    #[test]
    fn arpeggio_window_validates_then_applies_the_pattern_per_column() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        // 原版 `string_digits_symbol` 会把空格吃掉，所以「0 12」等于没有分隔符。
        state.arpeggio_pattern = "0 12".into();
        state.window = Some(WindowKind::Arpeggio);
        click_text(&mut state, &ctx, "确定");
        assert_eq!(state.modals.items.len(), 1, "缺分隔符要弹一次错误");
        assert_eq!(state.modals.items[0].title, "错误");
        assert_eq!(state.modals.items[0].text, "请使用“|”分隔值！");
        assert_eq!(
            state.window,
            Some(WindowKind::Arpeggio),
            "原版报错只 `return 1`，窗口留着"
        );
        assert_eq!(
            state.doc.selection.at(1, 0).map(|n| n.key),
            Some(52),
            "被拒的规律不该已经改到音高"
        );

        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        state.arpeggio_pattern = "0|12".into();
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "确定后关窗");
        let key = |st: &EditorState, col: u32, row: u32| st.doc.song.get(col, row).map(|n| n.key);
        assert_eq!(key(&state, 0, 0), Some(50), "第 1 列吃规律第 1 个值 +0");
        assert_eq!(key(&state, 1, 0), Some(64), "第 2 列吃 +12");
        assert_eq!(key(&state, 2, 0), Some(55), "第 3 列回到 +0：规律按列走");
        assert_eq!(key(&state, 1, 1), Some(76), "同一列的两个音符吃同一个值");
        assert!(state.doc.selection.is_empty(), "落回后不残留选区");
    }

    /// 错开窗口比琶音多一条原版校验：规律里出现 `-` 直接拒
    /// （`string_count("-", pattern) != 0`，因为 `string_digits_symbol` 会把负号吃掉）。
    #[test]
    fn stagger_window_rejects_a_negative_pattern() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.stagger_pattern = "0|-3".into();
        state.window = Some(WindowKind::Stagger);
        click_text(&mut state, &ctx, "确定");
        assert_eq!(state.modals.items.len(), 1, "负号要弹一次错误");
        assert_eq!(state.modals.items[0].title, "错误");
        assert_eq!(state.modals.items[0].text, "Stagger 只能向下延伸！");
        assert_eq!(state.window, Some(WindowKind::Stagger), "报错后窗口留着");
        assert_eq!(
            state.doc.selection.at(0, 1).map(|n| n.key),
            Some(62),
            "被拒的规律不该已经改到音高"
        );
    }

    /// 滑音窗口的复选框和数值就是原版那两个会话全局（`porta_reverse` / `port_cent`）：
    /// 勾上「反转」要改到状态，确定的偏移按原版 `decr = port_cent / 列数` 逐列累加。
    #[test]
    fn portamento_window_checkbox_and_cent_shift_reach_the_song() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.port_cent = 600;
        state.window = Some(WindowKind::Portamento);
        click_text(&mut state, &ctx, "反转");
        assert!(state.porta_reverse, "复选框没接上原版 porta_reverse");
        state.porta_reverse = false;
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "确定后关窗");
        let pitch = |st: &EditorState, col: u32| st.doc.song.get(col, 0).map(|n| n.pitch);
        assert_eq!(pitch(&state, 0), Some(200), "3 列 → 一个步长");
        assert_eq!(pitch(&state, 1), Some(400));
        assert_eq!(pitch(&state, 2), Some(600), "末列正好是要求的 cent");
    }

    /// 颤音窗口的五枚控件全部接到原版那五个会话全局：单选点了要改 `tremolotype` /
    /// `fade_auto`，确定走 `macro_tremolo_fadein`（淡入那一支）。
    /// 靶子谱三列列距只有 1，间隔 2 会把前两列整列吃掉，所以这里另铺一谱相隔 4 拍的。
    /// 结果里最后一列落在第 3 拍而不是原来的第 4 拍，就是原版「末列段头写 1」那条。
    #[test]
    fn tremolo_window_radios_and_ok_reach_the_original_session_globals() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.doc.replace_song(nbs_core::model::Song::new());
        state
            .doc
            .place_note_raw(0, 0, nbs_core::model::Note::new(0, 50));
        state
            .doc
            .place_note_raw(4, 0, nbs_core::model::Note::new(0, 52));
        state.doc.select_all();
        state.trem_spacing = 2;
        state.leg_dec = 25;
        state.leg_sus = 50;
        state.fade_auto = false;
        state.window = Some(WindowKind::Tremolo);
        let texts = drawn_texts(&mut state, &ctx);
        for want in [
            "Tremolo",
            "无音量变化",
            "淡入",
            "淡出",
            "自动音量",
            "手动音量",
            "触发/延长:",
            "延长时间:",
            "音符间隔:",
        ] {
            assert!(texts.iter().any(|t| t == want), "颤音窗口少画了「{want}」");
        }
        click_text(&mut state, &ctx, "淡入");
        assert_eq!(state.tremolotype, 1, "单选没接上原版 tremolotype");
        click_text(&mut state, &ctx, "自动音量");
        assert!(state.fade_auto, "单选没接上原版 fade_auto");
        state.fade_auto = false;
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "确定后关窗");
        let cell = |st: &EditorState, col: u32| {
            st.doc
                .song
                .get(col, 0)
                .map(|n| (n.key, n.velocity))
                .unwrap_or((0, 0))
        };
        assert_eq!(cell(&state, 0), (50, 25), "淡入第 1 列 = 触发 25");
        assert_eq!(cell(&state, 2), (50, 50), "第 2 列到 50 就撞上延长值");
        assert_eq!(cell(&state, 3), (52, 100), "末列只出一次、不上斜坡");
        assert!(
            state.doc.song.get(1, 0).is_none(),
            "间隔 2 中间那一拍是空的"
        );
    }

    /// 三扇批量属性窗口的草稿都是原版会话全局（`setvel` / `setpan` / `setpit` 加两个
    /// 开关），点了确定才落到选区。声道窗口那颗确定按钮原版没走 `condstr`，
    /// 中文界面也画 "OK"——这条也一并钉住，防止后面「顺手翻译」。
    #[test]
    fn batch_attr_windows_write_their_session_drafts() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.set_vel = 40;
        state.window = Some(WindowKind::SetVelocity);
        click_text(&mut state, &ctx, "百分比");
        assert!(state.vel_percent, "复选框没接上原版 percentvel");
        state.vel_percent = false;
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "确定后关窗");
        assert_eq!(
            state.doc.song.get(0, 0).map(|n| n.velocity),
            Some(40),
            "设定音量没按草稿写进选区"
        );

        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.set_pan = -30;
        state.window = Some(WindowKind::SetPanning);
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "OK"),
            "声道窗口的确定按钮原版写死 \"OK\""
        );
        click_text(&mut state, &ctx, "OK");
        frame(&mut state, &ctx);
        assert_eq!(
            state.doc.song.get(0, 0).map(|n| n.panning),
            Some(70),
            "原版存的是 100 + 值"
        );

        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.set_pit = 300;
        state.window = Some(WindowKind::SetPitch);
        click_text(&mut state, &ctx, "加法模式");
        assert!(state.pitch_additive, "复选框没接上原版 addpitch");
        state.pitch_additive = false;
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(
            state.doc.song.get(0, 0).map(|n| n.pitch),
            Some(300),
            "设定微调没按草稿写进选区"
        );
    }

    /// 立体声窗口：「反转」接 `stereo_reverse`，确定按列交替写声道。
    /// 原版不反转时第一列写的是 `100 + 立体程度`（源码那句注释反了，代码为准）。
    #[test]
    fn stereo_window_alternates_the_pan_by_column() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.stereo_width = 20;
        state.window = Some(WindowKind::Stereo);
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "立体程度"),
            "立体声窗口少画了「立体程度」"
        );
        click_text(&mut state, &ctx, "反转");
        assert!(state.stereo_reverse, "复选框没接上原版 stereo_reverse");
        state.stereo_reverse = false;
        click_text(&mut state, &ctx, "确定");
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "确定后关窗");
        let pan = |st: &EditorState, col: u32| st.doc.song.get(col, 0).map(|n| n.panning);
        assert_eq!(pan(&state, 0), Some(120), "第一列偏右");
        assert_eq!(pan(&state, 1), Some(80), "第二列偏左");
        assert_eq!(pan(&state, 2), Some(120), "第三列回到偏右：按列交替");
    }

    /// 原版错开那句警示带真换行（`draw_text_dynamic` 的 `\n`），而 `Page::text`
    /// 走的是不分行的排版，所以端口按 `\n` 拆开逐行画。这里验两行都画到了屏幕上。
    #[test]
    fn stagger_warning_splits_at_the_original_newline() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.prefs.language = 0;
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::Stagger);
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "This may replace "),
            "警示第一行没画出来：{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "lower note blocks!"),
            "警示第二行没画出来：{texts:?}"
        );
    }

    /// 原版 Edit 菜单的 `Macros...`（中文界面父项原文「快捷键......」）是一条 15 项
    /// 子菜单，顺序、文案、尾部的 `get_hotkey` 显示串逐条照抄。这里真点两下鼠标
    /// 把子菜单打开，再看屏幕上画出来的到底是哪 15 条、按什么顺序排。
    ///
    /// 叶子条目本身不点：egui 0.31 的弹层在进程内第一次打开时，前几帧还带着上一帧
    /// 的宽度（差 60px），照着文本中心按下会点空——这是离屏测试的时序问题，不是
    /// 界面接不上。条目 → 命令的接线由 `toolbar::macros_menu_items` 的用例钉住。
    #[test]
    fn edit_menu_macros_submenu_lists_the_original_fifteen_items() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        click_text(&mut state, &ctx, "编辑");
        click_text(&mut state, &ctx, "快捷键......");
        let rows = drawn_at(&mut state, &ctx);
        let mut prev_y = f32::MIN;
        for label in [
            "Tremolo...  Ctrl+Shift+A",
            "Stereo...  Ctrl+Shift+S",
            "Arpeggio...  Ctrl+Shift+D",
            "Portamento...  Ctrl+Shift+F",
            "Vibrato  Ctrl+Shift+G",
            "Stagger...  Ctrl+Shift+H",
            "Chorus  Ctrl+Shift+J",
            "Volume LFO  Ctrl+Shift+K",
            "淡入  Ctrl+Shift+Q",
            "淡出  Ctrl+Shift+W",
            "替换音  Ctrl+Shift+E",
            "设定音量......  Ctrl+Shift+R",
            "设定声道......  Ctrl+Shift+T",
            "设定音高......  Ctrl+Shift+Y",
            "重置所有属性  Ctrl+Shift+U",
        ] {
            let y = rows
                .iter()
                .find(|(t, _)| t == label)
                .map(|(_, pos)| pos.y)
                .unwrap_or_else(|| panic!("子菜单里没画出「{label}」，实得：{rows:?}"));
            assert!(y > prev_y, "「{label}」画在第 {y} 行，顺序不对");
            prev_y = y;
        }
    }

    /// 原版子菜单父项带 `inactive(selected = 0 || selection_l = 0)`：没有选区整条置灰，
    /// 点不开子菜单，也就开不了宏窗口。
    #[test]
    fn macros_submenu_is_locked_without_a_selection() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        state.doc.place_selection();
        assert!(state.doc.selection.is_empty(), "测试前提：选区已落回歌曲");
        click_text(&mut state, &ctx, "编辑");
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "快捷键......"),
            "父项要照常画出来（置灰），实得：{texts:?}"
        );
        click_text(&mut state, &ctx, "快捷键......");
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            !texts.iter().any(|t| t.starts_with("Arpeggio")),
            "锁住的父项不该展开子菜单：{texts:?}"
        );
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "无选区时开不了宏窗口");
    }

    // ---- 主题色取色器 `w_setaccent`（draw_window_set_accent.gml）----

    /// 屏幕上某个尺寸的实心矩形，按绘制顺序返回。
    fn filled_rects(out: &egui::FullOutput, w: f32, h: f32) -> Vec<egui::epaint::RectShape> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r)
                    if (r.rect.width() - w).abs() < 0.5 && (r.rect.height() - h).abs() < 0.5 =>
                {
                    Some(r.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// 让开窗的淡入跑完再取形状：`Window::open` 默认带 `fade_out`，动画期间整窗
    /// 形状的颜色都被透明度乘过一遍（实测两帧只剩 0.51、三帧 0.99），
    /// 直接比对原色会假失败。
    fn settle(state: &mut EditorState, ctx: &egui::Context) -> egui::FullOutput {
        let mut out = frame_out(state, ctx, base_input());
        for _ in 0..15 {
            out = frame_out(state, ctx, base_input());
        }
        out
    }

    /// 取色器右侧那上下两片色块（`:35-40`：`y1 + 42..154` 与 `y1 + 155..267`，
    /// 中间留着原版那 1 像素缝）。窗口后画、形状排在整帧末尾，所以从末尾取，
    /// 并要求两片同列且正好相接，免得拿错别处的同尺寸矩形。
    fn accent_chips(
        state: &mut EditorState,
        ctx: &egui::Context,
    ) -> (egui::epaint::RectShape, egui::epaint::RectShape) {
        let out = settle(state, ctx);
        let chips = filled_rects(&out, 43.0, 112.0);
        assert!(chips.len() >= 2, "取色器右侧应有上下两片色块");
        let top = chips[chips.len() - 2].clone();
        let bottom = chips[chips.len() - 1].clone();
        assert_eq!(
            top.rect.min.x, bottom.rect.min.x,
            "两片色块同在 x1 + 217 那一列"
        );
        assert!(
            (bottom.rect.min.y - top.rect.max.y - 1.0).abs() < 0.01,
            "原版两片之间留 1 像素"
        );
        (top, bottom)
    }

    /// 取色器两块拖拽判定区在屏幕上的实际位置：方块 187x225（`:46` 的
    /// `mouse_rectangle(x1 + 20, y1 + 43, 187, 225)`）、明度条 240x8（`:56`）。
    /// 上面那片色块的左上角是源码的 `pg.row(217, 42)`，Fluent 下 `row(x, n)`
    /// 就是 `o + (x, n - 8)`，减回去即页面原点。
    fn accent_zones(state: &mut EditorState, ctx: &egui::Context) -> (egui::Rect, egui::Rect) {
        let (top, _) = accent_chips(state, ctx);
        let o = top.rect.min - egui::vec2(217.0, 34.0);
        (
            egui::Rect::from_min_size(o + egui::vec2(20.0, 35.0), egui::vec2(187.0, 225.0)),
            egui::Rect::from_min_size(o + egui::vec2(20.0, 270.0), egui::vec2(240.0, 8.0)),
        )
    }

    /// 裸坐标点一下：按下、抬起各占一帧。
    fn click_pos_at(state: &mut EditorState, ctx: &egui::Context, p: egui::Pos2) {
        for pressed in [true, false] {
            let mut raw = base_input();
            raw.events.push(egui::Event::PointerMoved(p));
            raw.events.push(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            frame_out(state, ctx, raw);
        }
    }

    /// 按住 → 移到终点 → 松手，三段各占一帧。原版的 `hsdrag` / `vdrag` 正是这个
    /// 语义：按下落在哪块就认死那块，滑出判定区也照那块算，松手才清零。
    fn drag_between(
        state: &mut EditorState,
        ctx: &egui::Context,
        from: egui::Pos2,
        to: egui::Pos2,
    ) {
        let btn = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut press = base_input();
        press.events.push(egui::Event::PointerMoved(from));
        press.events.push(btn(from, true));
        frame_out(state, ctx, press);
        let mut slide = base_input();
        slide.events.push(egui::Event::PointerMoved(to));
        frame_out(state, ctx, slide);
        let mut lift = base_input();
        lift.events.push(egui::Event::PointerMoved(to));
        lift.events.push(btn(to, false));
        frame_out(state, ctx, lift);
    }

    /// 按一次键：按下 + 抬起挤在同一帧（取色器认的是原版的
    /// `keyboard_check_released(vk_escape)`）。
    fn press_key(state: &mut EditorState, ctx: &egui::Context, key: egui::Key) {
        let mk = |pressed: bool| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        let mut raw = base_input();
        raw.events.push(mk(true));
        raw.events.push(mk(false));
        frame_out(state, ctx, raw);
    }

    /// 直接开扇取色器，并把编辑态按当前基色复位一遍——原版靠 `resetcolor`
    /// 在开窗第一帧做同一件事（`draw_window_preferences.gml:320`）。
    fn open_accent_picker(state: &mut EditorState, ctx: &egui::Context) {
        state.prefs.language = 1;
        state.window = Some(WindowKind::SetAccent);
        let base = state.theme.accent_base;
        state.accent_pick = Some(crate::state::AccentPick::from_rgb(base));
        settle(state, ctx);
    }

    fn picked(state: &EditorState) -> crate::state::AccentPick {
        state.accent_pick.clone().expect("取色器应当带着编辑态")
    }

    /// 原版 `draw_window_preferences.gml:299-323`：Fluent 界面页「主题」那一组里，
    /// 下拉右边压一块 17x17，内层 13x13 才是色，常态档位 `accent[3]`。
    /// 点它开 `w_setaccent`，并按当前 accent1/2/3 复位。
    #[test]
    fn fluent_preferences_swatch_opens_the_accent_picker() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.theme.accent_base = [200, 20, 60];
        state.theme.accent_init();
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 1;
        frame(&mut state, &ctx);
        let out = settle(&mut state, &ctx);
        let all13 = filled_rects(&out, 13.0, 13.0);
        let swatch: Vec<_> = all13
            .iter()
            .filter(|r| r.fill == state.theme.accent(3))
            .collect();
        assert_eq!(
            swatch.len(),
            1,
            "界面页应当只有一块 13x13、填 accent[3] 的主题色，实有 {:?}，accent[3] = {:?}",
            all13.iter().map(|r| (r.rect, r.fill)).collect::<Vec<_>>(),
            state.theme.accent(3)
        );
        click_pos_at(&mut state, &ctx, swatch[0].rect.center());
        frame(&mut state, &ctx);
        assert_eq!(
            state.window,
            Some(WindowKind::SetAccent),
            "点主题色块要开取色器"
        );
        let p = picked(&state);
        assert_eq!(p.rgb, [200, 20, 60], "开窗先按 accent1/2/3 复位");
        assert_eq!(
            p.text,
            ["200".to_string(), "20".to_string(), "60".to_string()],
            "三个文本框跟着灌上十进制数"
        );
        assert_eq!(p.hsv, Theme::rgb_to_hsb(200, 20, 60));
    }

    /// 这扇窗画出来的东西：右侧两片色块（正在挑的 / 生效中的）、下面四格预览、
    /// 「RGB 调色」分组、三个数字框和三个按钮，逐条对 `:35-40`、`:74-98`、
    /// `:99-110`、`:131-153`。
    #[test]
    fn accent_picker_draws_the_original_pieces() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        open_accent_picker(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());

        let (top, bottom) = accent_chips(&mut state, &ctx);
        assert_eq!(
            top.fill,
            egui::Color32::from_rgb(0, 120, 212),
            "上面那片是正在挑的 rr/gg/bb"
        );
        assert_eq!(
            bottom.fill,
            state.theme.accent(3),
            "下面那片是生效中的 accent[3]"
        );

        let left = filled_rects(&out, 121.0, 20.0);
        let right = filled_rects(&out, 118.0, 20.0);
        assert_eq!(
            (left.len(), right.len()),
            (2, 2),
            "四格预览：左列两格、右列两格"
        );
        assert_eq!(
            left[0].fill,
            egui::Color32::from_rgb(0, 120, 212),
            "左上铺 rr/gg/bb"
        );
        assert_eq!(
            left[1].fill,
            egui::Color32::from_rgb(9, 9, 9),
            "左下铺近黑底"
        );
        assert_eq!(
            right[1].fill,
            egui::Color32::from_rgb(221, 221, 221),
            "右下铺近白底"
        );

        let mut texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        texts.sort_unstable();
        for want in [
            "设置主题颜色",
            "RGB 调色",
            "0",
            "120",
            "212",
            "使用默认值",
            "取消",
            "确定",
        ] {
            assert!(
                texts.iter().any(|t| t == want),
                "界面上没有「{want}」：{texts:?}"
            );
        }
        assert_eq!(
            texts.iter().filter(|t| t.as_str() == "预览").count(),
            4,
            "原版四格各写一次 Preview"
        );
    }

    /// 原版 `:46-54`：色相 = 方块内横向 / 187 * 360，饱和 = (1 - 纵向 / 225) * 100
    /// （越靠下越灰），每拖一步反算 rr/gg/bb 并把三个文本框刷掉。
    /// 滑出判定区仍认死这一块，但横纵都先夹进 0..187 / 0..225。
    #[test]
    fn accent_picker_square_drag_sets_hue_and_saturation() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        open_accent_picker(&mut state, &ctx);
        let (sq, _bar) = accent_zones(&mut state, &ctx);

        drag_between(
            &mut state,
            &ctx,
            sq.center(),
            sq.min + egui::vec2(46.75, 112.5),
        );
        let p = picked(&state);
        assert!(
            (p.hsv[0] - 90.0).abs() < 0.01,
            "横向 46.75px = 187 的四分之一，色相应当是 90°，实得 {}",
            p.hsv[0]
        );
        assert!(
            (p.hsv[1] - 50.0).abs() < 0.01,
            "纵向 112.5px = 225 的正中，饱和应当是 50%，实得 {}",
            p.hsv[1]
        );
        assert_eq!(
            p.rgb,
            Theme::hsb_to_rgb(p.hsv[0], p.hsv[1], p.hsv[2]),
            "rr/gg/bb 由这一步的 hsv 反算"
        );
        assert_eq!(
            p.text,
            std::array::from_fn(|i| p.rgb[i].to_string()),
            "拖一步就把三个文本框刷成新数字"
        );

        // 判定区外的点仍按这一块算，只是先夹住（原版那串括号展开就是这个）。
        drag_between(
            &mut state,
            &ctx,
            sq.center(),
            sq.min + egui::vec2(400.0, -30.0),
        );
        let p = picked(&state);
        assert!((p.hsv[0] - 360.0).abs() < 0.01, "横向夹进 0..187");
        assert!(
            (p.hsv[1] - 100.0).abs() < 0.01,
            "纵向夹进 0..225，最上缘是饱和 100"
        );
    }

    /// 原版 `:56-64`：明度条只认横向，判定区宽 240，左端 0、右端 100，出了区也夹住。
    /// 这一条不动色相和饱和。
    #[test]
    fn accent_picker_value_bar_drag_uses_the_240_travel() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        open_accent_picker(&mut state, &ctx);
        let (_sq, bar) = accent_zones(&mut state, &ctx);
        let before = picked(&state);

        drag_between(&mut state, &ctx, bar.center(), bar.min);
        let p = picked(&state);
        assert!(p.hsv[2] < 0.01, "拖到判定区左端是明度 0，实得 {}", p.hsv[2]);

        drag_between(
            &mut state,
            &ctx,
            bar.center(),
            bar.max + egui::vec2(60.0, 0.0),
        );
        let p = picked(&state);
        assert!(
            (p.hsv[2] - 100.0).abs() < 0.01,
            "判定区右端以外仍夹在 240，实得 {}",
            p.hsv[2]
        );
        assert_eq!(
            [p.hsv[0], p.hsv[1]],
            [before.hsv[0], before.hsv[1]],
            "明度条不该动色相和饱和"
        );
    }

    /// 原版 `:111-127`：三框都有内容才整批转数字，任何一框转不了就整批作废
    /// （`err = 1`，这一层不弹提示）。端口把 rr/gg/bb 存成 0..=255 整数，
    /// 所以超范围夹住、带小数截断；hsv 不在这里反推，否则每帧从文本往返一次
    /// 会把数字整数化、游标跳一格（对应原版的 `text_focus`）。
    #[test]
    fn accent_picker_rgb_boxes_commit_as_a_batch() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        open_accent_picker(&mut state, &ctx);
        let before = picked(&state).hsv;

        let mut p = state.accent_pick.take().expect("取色器已开窗");
        p.text = ["300", "12.7", "255"].map(String::from);
        state.accent_pick = Some(p);
        frame(&mut state, &ctx);
        let p = picked(&state);
        assert_eq!(p.rgb, [255, 12, 255], "300 夹成 255、12.7 截成 12");
        assert_eq!(p.hsv, before, "没在框里打字就不反推 hsv");

        let mut p = state.accent_pick.take().expect("取色器已开窗");
        p.text = ["1", "", "x"].map(String::from);
        state.accent_pick = Some(p);
        frame(&mut state, &ctx);
        assert_eq!(
            picked(&state).rgb,
            [255, 12, 255],
            "空框和「x」都不合格，整批不生效"
        );
    }

    /// 真的在绿框里敲一个字：TextEdit 拿到焦点后 `changed()` 为真，也就是原版的
    /// `text_focus`，这时才按新的 rr/gg/bb 反推 hsv、游标跟着挪。
    #[test]
    fn accent_picker_typed_box_rederives_the_hsv() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        open_accent_picker(&mut state, &ctx);
        let out = frame_out(&mut state, &ctx, base_input());
        let green = text_center_if(&out.shapes, |t| t == "120");

        click_pos_at(&mut state, &ctx, green);
        let mut raw = base_input();
        raw.events.push(egui::Event::Text("7".into()));
        frame_out(&mut state, &ctx, raw);

        let p = picked(&state);
        assert_ne!(p.text[1], "120", "敲进去的字要真的落在框里：{:?}", p.text);
        let typed: f32 = p.text[1].parse().expect("框里应当还是数字");
        assert_eq!(
            p.rgb[1],
            typed.clamp(0.0, 255.0).floor() as u8,
            "rgb 跟着文本框走"
        );
        assert_eq!(
            p.hsv,
            Theme::rgb_to_hsb(p.rgb[0], p.rgb[1], p.rgb[2]),
            "改过字才反推 hsv"
        );
    }

    /// 三个出口（`:140-153` 和 `draw_windows.gml:100-101`）：确定把挑的色写成
    /// accent1/2/3 并重算 9 档表后回首选项；取消和 Escape 都只回首选项、不碰基色。
    /// 原版这一层不落盘，写盘要等首选项自己按「确定」。
    #[test]
    fn accent_picker_close_paths() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        open_accent_picker(&mut state, &ctx);
        state.accent_pick = Some(crate::state::AccentPick::from_rgb([7, 8, 9]));

        click_text(&mut state, &ctx, "确定");
        assert_eq!(state.theme.accent_base, [7, 8, 9], "确定才写 accent1/2/3");
        assert_eq!(
            state.theme.accent(3),
            egui::Color32::from_rgb(7, 8, 9),
            "9 档表跟着重算（draw_accent_init）"
        );
        assert_eq!(state.window, Some(WindowKind::Preferences));
        assert_eq!(
            nbs_io::prefs::Preferences::load().accent,
            [0, 120, 212],
            "取色器自己不该写盘"
        );

        // 取消：基色保持刚确定的这支，只是退回首选项。
        state.accent_pick = Some(crate::state::AccentPick::from_rgb([1, 2, 3]));
        state.window = Some(WindowKind::SetAccent);
        click_text(&mut state, &ctx, "取消");
        assert_eq!(state.theme.accent_base, [7, 8, 9], "取消不改基色");
        assert_eq!(state.window, Some(WindowKind::Preferences));

        // Escape：同「取消」，是退回首选项而不是关窗。
        state.accent_pick = Some(crate::state::AccentPick::from_rgb([4, 5, 6]));
        state.window = Some(WindowKind::SetAccent);
        press_key(&mut state, &ctx, egui::Key::Escape);
        assert_eq!(state.theme.accent_base, [7, 8, 9], "Escape 不改基色");
        assert_eq!(
            state.window,
            Some(WindowKind::Preferences),
            "原版这扇的 Escape 是退回首选项"
        );
    }

    /// 原版 `:131-139`「使用默认值」：先问一句「你确定吗？」，答「是」才把基色写成
    /// (0,120,215)。注意这跟 `control_create.gml:202-204` 的出厂值 (0,120,212) 差 1，
    /// 原版两处本来就不同，端口照抄；答「否」整条不改，取色器也不关。
    #[test]
    fn accent_picker_use_default_asks_before_writing() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.theme.accent_base = [200, 20, 60];
        state.theme.accent_init();
        open_accent_picker(&mut state, &ctx);

        click_text(&mut state, &ctx, "使用默认值");
        assert_eq!(state.modals.items.len(), 1, "原版这里先弹一句确认");
        assert!(state.modals.items[0].yes_no);
        assert_eq!(state.theme.accent_base, [200, 20, 60], "答之前不改");
        click_text(&mut state, &ctx, "否");
        frame(&mut state, &ctx);
        assert_eq!(state.theme.accent_base, [200, 20, 60], "答「否」不还原");

        click_text(&mut state, &ctx, "使用默认值");
        click_text(&mut state, &ctx, "是");
        frame(&mut state, &ctx);
        assert_eq!(
            state.theme.accent_base,
            [0, 120, 215],
            "答「是」才写原版那组默认值"
        );
        let p = picked(&state);
        assert_eq!(p.rgb, [0, 120, 215], "取色器按新基色复位（resetcolor）");
        assert_eq!(
            p.text,
            ["0".to_string(), "120".to_string(), "215".to_string()]
        );
        assert_eq!(
            state.window,
            Some(WindowKind::SetAccent),
            "原版用完默认值仍停在这扇窗"
        );
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
