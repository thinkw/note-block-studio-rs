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
pub mod background;
pub mod commands;
pub mod control;
pub mod fonts;
/// 播放特效底座与内置特效（`Surface::Overlay` 叠加 / `Surface::Stage` 独占中央面板）。
pub mod fx;
pub mod grid;
pub mod icons;
pub mod interaction;
pub mod layers_panel;
pub mod live_midi;
mod midi_enhanced;
pub mod piano;
pub mod shortcuts;
pub mod sound_import;
pub mod state;
pub mod statusbar;
/// 原版歌曲标签条（draw_song_tabs）。
pub mod tabbar;
/// 歌曲标签槽位表（原版 `global.songs[]`），见模块注释。
pub mod tabs;
pub mod theme;
pub mod toolbar;
pub mod transcription;
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
impl EditorState {
    /// 用例用的状态：出厂的界面档位。`EditorState::new()` 会把本机 prefs.json 里
    /// 的主题、删除效果、琴键标注、界面缩放拷进 `flags` / `theme`，用户在首选项里
    /// 改过任何一项，「原版默认界面」的断言就只在这台机器上成立。这里按出厂值钉回去
    /// （出厂档位见 `control_create.gml:225,246-248,314-318`）。
    /// 语言不在这里钉：用例断言的是哪一种语言的文案，自己设 `prefs.language`
    /// 并配 `LangGuard`，出厂值取的是系统区域。
    pub(crate) fn for_test() -> Self {
        let mut s = Self::new();
        s.prefs.theme = 3;
        s.prefs.remove_effect = true;
        // 原版绝对像素的断言（键宽 40、步进 39、y = 746）只有在 1:1 界面上成立；
        // 用户存过 150% 的话整帧等比放大，一根琴键都筛不出来。无头用例里 egui 拿不到
        // 系统倍率（`native_pixels_per_point` 是每帧从平台层喂进来的，测试没喂 = 1.0），
        // 所以缩放这一档只剩 prefs.json 里存的这个值要钉。
        s.prefs.window_scale = Some(1.0);
        // 琴键缩放同理钉 1.0：键宽 39、面板顶 746 这类绝对像素断言只在 1:1 琴键上
        // 成立；本机 prefs.json 里实测琴键缩放留下的档位会漏进来（startkey 撞到
        // 52 键上限的 1 而不是 9）。
        s.prefs.piano_scale = 1.0;
        // 强调色也按出厂钉死：本机改过主题色的话，accent_base 相关断言会拿到
        // 用户的值（和 #67 同款的本机 prefs 泄漏）。
        s.prefs.accent = [0, 120, 212];
        s.theme = Theme::new(ThemeKind::Fluent);
        s.theme.fdark = false;
        s.theme.set_accent_base(s.prefs.accent);
        s.flags = UiFlags::default();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_defaults() {
        let _prefs = isolate_prefs();
        let s = EditorState::for_test();
        assert!(s.doc.song.total_blocks > 0);
        assert!(s.doc.history.can_undo() == false);
    }

    /// 离屏跑一帧完整 UI，确保布局 / 绘制 / 输入状态机不会 panic。
    #[test]
    fn ui_renders_one_frame_offscreen() {
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        crate::fonts::warm_up(&ctx);
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
        let mut state = EditorState::for_test();
        state.doc.replace_song(song);
        state.theme.apply(&ctx);
        crate::fonts::warm_up(&ctx);
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
        let mut s = EditorState::for_test();
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
        let s = EditorState::for_test();
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
        let mut s = EditorState::for_test();
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
        let src = EditorState::for_test()
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

    /// 原版 `draw_window_about.gml:44-58` 的分行：行首 `*` 是标题（`*` 本身吃掉），
    /// 含 `|` 的那行在竖线处切成「文字 + 链接」，竖线后的整段是 url。
    /// 末尾不带换行符的那段跟 `changelog_lines` 一样进不了表。
    #[test]
    fn credits_lines_follow_the_original_split() {
        let src = "*Lead developer\nBentroen|https://example.test/\n\n未结尾的一段";
        let lines = crate::windows::credits_lines(src);
        assert_eq!(
            lines,
            vec![
                ("Lead developer".to_string(), true, None),
                (
                    "Bentroen".to_string(),
                    false,
                    Some("https://example.test/".to_string())
                ),
                (String::new(), false, None),
            ]
        );
    }

    /// `assets/credits.txt` 是原版 `datafiles/Data/credits.txt` 的逐字节副本，
    /// 关于窗右栏直接吃它；载入时同样只归一 CRLF。
    #[test]
    fn bundled_credits_is_readable() {
        let _prefs = isolate_prefs();
        let src = EditorState::for_test()
            .credits
            .expect("assets/credits.txt 应当能在资源目录里找到");
        assert!(!src.contains('\r'), "载入时要把 CRLF 归一成 LF");
        let lines = crate::windows::credits_lines(&src);
        assert_eq!(
            lines[0],
            ("CREDITS".to_string(), true, None),
            "首行是 `*CREDITS` → 标题行"
        );
        assert!(lines.len() > 100, "装机版那份是 {} 行", lines.len());
        assert!(
            lines.iter().any(|(t, _, u)| t == "Boost.Filesystem"
                && u.as_deref() == Some("http://www.boost.org/libs/filesystem")),
            "栏里应当有 `名字|网址` 这种链接行"
        );
    }

    /// check_updates.gml 回调的 UI 侧：后台写 1 → 弹一次问句；不重复弹；其他状态不弹。
    #[test]
    fn poll_update_check_asks_once_per_new_version() {
        let _prefs = isolate_prefs();
        let mut s = EditorState::for_test();
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
        crate::fonts::warm_up(ctx);
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        crate::fonts::warm_up(ctx);
        let _lock = frame_lock();
        ctx.run(input, |ctx| crate::show(ctx, state))
    }

    /// 原版图层名是常驻 `draw_text_edit`（内容宽 72）：超框字符断行后被框高裁掉，
    /// 视觉上只显示放得下的前缀。非编辑态裸画文本没有框，长名会溢出压到音量/声像
    /// 图标（验收 #82）；截断实现见 layers_panel 的 `clip_label`。
    #[test]
    fn long_layer_names_clip_before_the_volume_glyph() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let mut song = nbs_core::model::Song::new();
        song.layers[0].name = "Distortion Guitar - 高声部1".into();
        song.layers.push(nbs_core::model::Layer {
            name: "String Ensemble 2 - 低声部1".into(),
            ..Default::default()
        });
        state.doc.replace_song(song);
        let out = frame_out(&mut state, &ctx, base_input());
        let row_texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => {
                    (t.galley.rect.left() < 100.0).then(|| t.galley.text().to_string())
                }
                _ => None,
            })
            .collect();
        assert!(
            row_texts.iter().any(|t| t.starts_with("Distortion G")),
            "图层行名称应当已绘制（截断前缀）: {row_texts:?}"
        );
        assert!(
            row_texts.iter().all(|t| !t.contains("高声部1")),
            "长名要按原版 draw_text_edit 的 72px 截断，不许画全名压到音量/声像图标: {row_texts:?}"
        );
    }

    /// 验收 #97：音量条右侧的「输出」格子必须真的画出来、并且真的点得开设备表。
    /// 之前布局把它挤成 0 宽（旧早退条件 `rect.max.x > area.max.x - 210` 恒成立），
    /// 代码在、界面上永远看不见——正是「代码里有 ≠ 上屏」的那类缺口。
    #[test]
    fn the_output_box_is_painted_and_opens_the_device_list() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None;
        state.output_menu = false;
        // 用宽屏档位：格子能拿到 150 宽，里面写的就是当前出声的设备名。
        let mut wide = base_input();
        wide.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1810.0, 980.0),
        ));
        let find = |out: &egui::FullOutput| -> Option<egui::Rect> {
            out.shapes.iter().find_map(|cs| match &cs.shape {
                egui::Shape::Rect(r)
                    if r.stroke.color == egui::Color32::from_gray(128)
                        && r.rect.height() == 20.0
                        && r.rect.top() < 60.0 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
        };
        // 文字图元的 `galley.rect` 不带屏幕位置，落笔点看 `pos`。
        let texts_at = |out: &egui::FullOutput, box_rect: egui::Rect| -> Vec<String> {
            out.shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t)
                        if t.pos.x >= box_rect.min.x
                            && t.pos.x <= box_rect.max.x
                            && t.pos.y >= box_rect.min.y
                            && t.pos.y <= box_rect.max.y =>
                    {
                        Some(t.galley.text().to_string())
                    }
                    _ => None,
                })
                .collect()
        };
        let click = |state: &mut EditorState, p: egui::Pos2| {
            for pressed in [true, false] {
                let mut raw = wide.clone();
                raw.events.push(egui::Event::PointerMoved(p));
                raw.events.push(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
                frame_out(state, &ctx, raw);
            }
        };
        let out = frame_out(&mut state, &ctx, wide.clone());
        let rect = find(&out).expect("音量条右侧的「输出」格子");
        // 格子里写的必须是当前真正出声的设备名；放不下时 `clip_width` 截成前缀 + 省略号。
        let label = state.output_label();
        let inside = texts_at(&out, rect);
        assert!(
            inside
                .iter()
                .any(|t| !t.is_empty() && label.starts_with(t.trim_end_matches('.'))),
            "格子里应写当前出声的设备名 {label:?}，实际画了 {inside:?}（格子 {rect:?}）"
        );
        click(&mut state, rect.center());
        assert!(state.output_menu, "点格子应当展开设备列表");
        // 弹层 Area 首帧只做 sizing，命中框要到下一帧才登记（`click_at` 同款前置）。
        frame_out(&mut state, &ctx, wide.clone());
        let menu = frame_out(&mut state, &ctx, wide.clone());
        let after = texts_at(&menu, egui::Rect::EVERYTHING);
        assert!(
            after
                .iter()
                .any(|t| t.contains("Follow system default") || t.contains("跟随系统默认")),
            "展开的列表里应有「跟随系统默认」这一项"
        );
        // 列表不能只是看得见：点第 0 项要真的派发切输出设备的命令。
        // 行文字落笔在 `(org.x + 36, 行顶 + 5)`，命中框是 `(org.x + 3, 行顶, wid - 5, 22)`，
        // 所以从文字左上角往右下走 (5, 8) 一定落在同一行里。
        let row = menu
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => {
                    let s = t.galley.text();
                    (s.contains("Follow system default") || s.contains("跟随系统默认"))
                        .then(|| t.pos + egui::vec2(5.0, 8.0))
                }
                _ => None,
            })
            .expect("设备列表第 0 项");
        click(&mut state, row);
        // 工具栏产生的命令在同一帧的 `flush_all` 里就落地了：`apply_output_choice` 当场
        // 切完并报名字。这台机器能不能真建成那条流是环境实情，接上和失败两种结果
        // 都说明点击已经打到命令总线。
        let m = msg(&state);
        assert!(
            [
                "Output device: ",
                "输出设备：",
                "Failed to switch output device: ",
                "切换输出设备失败："
            ]
            .iter()
            .any(|p| m.starts_with(p)),
            "点「跟随系统默认」应打到 apply_output_choice，实际消息 {m:?}"
        );
        assert!(!state.output_menu, "选完应当收起列表");
    }

    /// 原版框选拖拽中（`control_draw.gml:323-341`）没有矩形虚线框：框内音符是
    /// 「c 提高不透明度 + spr_block_sel 叠层」，松手后在原位画 c=1 + sel 叠层
    /// （`selection_draw.gml:79-104`）。验收 #83 反馈「没有框 / 外观不对」：
    /// 整帧驱动一次框选，断言框内每个音符格恰好多一张精灵（Mesh quad）叠层。
    #[test]
    fn box_select_drag_adds_one_sel_sprite_per_note_in_the_rect() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None;
        let mut song = nbs_core::model::Song::new();
        song.set(0, 1, nbs_core::model::Note::new(0, 45));
        song.set(1, 1, nbs_core::model::Note::new(0, 47));
        song.set(2, 1, nbs_core::model::Note::new(0, 48));
        state.doc.replace_song(song);

        // egui 0.31 的精灵是 4 顶点 Mesh quad：按顶点包围盒中心对上格子。
        fn mesh_bboxes(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| mesh_bboxes(s, out)),
                egui::Shape::Mesh(m) => {
                    let (mut mn, mut mx) = (
                        egui::pos2(f32::MAX, f32::MAX),
                        egui::pos2(f32::MIN, f32::MIN),
                    );
                    for v in &m.vertices {
                        mn.x = mn.x.min(v.pos.x);
                        mn.y = mn.y.min(v.pos.y);
                        mx.x = mx.x.max(v.pos.x);
                        mx.y = mx.y.max(v.pos.y);
                    }
                    out.push(egui::Rect::from_min_max(mn, mx));
                }
                _ => {}
            }
        }
        fn sprite_count(out: &egui::FullOutput, cell: egui::Rect) -> usize {
            let mut boxes = Vec::new();
            for cs in &out.shapes {
                mesh_bboxes(&cs.shape, &mut boxes);
            }
            boxes
                .iter()
                .filter(|r| r.center().distance(cell.center()) < 2.0)
                .count()
        }
        fn moved(p: egui::Pos2) -> egui::Event {
            egui::Event::PointerMoved(p)
        }
        fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }
        }

        // 第一帧让布局落定，格子矩形从 viewport 现读。
        let out = frame_out(&mut state, &ctx, base_input());
        assert!(!out.shapes.is_empty());
        let start = state.viewport.cell_rect(0, 3).center(); // 空白格：框选锚点
        let end = state.viewport.cell_rect(5, 1).center(); // 空白格：拖到这
        let cells: Vec<egui::Rect> = [(0i64, 1i64), (1, 1), (2, 1)]
            .iter()
            .map(|&(t, l)| state.viewport.cell_rect(t, l))
            .collect();

        // 基线：三个音符格各画了 N 张精灵（本体方块）。
        let mut input = base_input();
        input.events.push(moved(start));
        let base = frame_out(&mut state, &ctx, input);
        let baseline: Vec<usize> = cells.iter().map(|&c| sprite_count(&base, c)).collect();
        assert!(
            baseline.iter().all(|&n| n > 0),
            "音符本体精灵应当先画出来: {baseline:?}"
        );

        // 按下（还在锚点上，未拖开）：不该有叠层。
        let mut input = base_input();
        input.events.push(moved(start));
        input.events.push(button(start, true));
        let press = frame_out(&mut state, &ctx, input);
        let press_counts: Vec<usize> = cells.iter().map(|&c| sprite_count(&press, c)).collect();
        assert_eq!(press_counts, baseline, "还没拖开不该有选区叠层");

        // 拖开：进入框选，框内每个音符格恰好多一张 spr_block_sel。
        let mut input = base_input();
        input.events.push(moved(end));
        let drag = frame_out(&mut state, &ctx, input);
        let drag_counts: Vec<usize> = cells.iter().map(|&c| sprite_count(&drag, c)).collect();
        assert!(
            drag_counts.len() == baseline.len()
                && drag_counts.iter().zip(&baseline).all(|(d, b)| *d == b + 1),
            "框内每个音符格应恰好多一张 sel 叠层: drag {drag_counts:?} vs base {baseline:?}"
        );

        // 松手：音符抬进浮动选区，下一帧在原位仍各多一张叠层（原版 c=1 + salpha）。
        let mut input = base_input();
        input.events.push(button(end, false));
        let _ = frame_out(&mut state, &ctx, input);
        assert_eq!(state.doc.selection.len(), 3, "松手应抬出整框音符");
        assert!(
            !state.doc.song.has(0, 1) && !state.doc.song.has(1, 1) && !state.doc.song.has(2, 1),
            "音符应从谱面抬出"
        );
        let mut input = base_input();
        input.events.push(moved(end));
        let float = frame_out(&mut state, &ctx, input);
        let float_counts: Vec<usize> = cells.iter().map(|&c| sprite_count(&float, c)).collect();
        assert!(
            float_counts.len() == baseline.len()
                && float_counts.iter().zip(&baseline).all(|(f, b)| *f == b + 1),
            "浮选区原位仍应各多一张叠层: float {float_counts:?} vs base {baseline:?}"
        );
    }

    /// 原版 `control_draw.gml:903-922`：拖框时画半透明矩形——加选蓝（Fluent 主题
    /// 是亮蓝 14120960=RGB(0,127,215)）、减选红，填充 α0.25 + 1px 描边；
    /// 矩形从按下像素拉到当前鼠标，松手 / 未拖开不画。验收 #87 反馈「原版有选择框」。
    #[test]
    fn box_select_drag_draws_the_transparent_selection_rectangle() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None;
        let _ = frame_out(&mut state, &ctx, base_input());

        let start = state.viewport.cell_rect(2, 4).center();
        let end = state.viewport.cell_rect(9, 2).center();

        fn moved(p: egui::Pos2) -> egui::Event {
            egui::Event::PointerMoved(p)
        }
        fn press(p: egui::Pos2, secondary: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos: p,
                button: if secondary {
                    egui::PointerButton::Secondary
                } else {
                    egui::PointerButton::Primary
                },
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }
        }
        fn rect_fills(out: &egui::FullOutput) -> Vec<(egui::Rect, egui::Color32)> {
            fn walk(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
                match shape {
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                    _ => {}
                }
            }
            let mut v = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut v);
            }
            v
        }
        let blue = egui::Color32::from_rgba_unmultiplied(0, 127, 215, 64);
        let red = egui::Color32::from_rgba_unmultiplied(255, 0, 0, 64);

        // 按下未拖开：无矩形。
        let mut input = base_input();
        input.events.push(moved(start));
        input.events.push(press(start, false));
        let press_out = frame_out(&mut state, &ctx, input);
        assert!(
            !rect_fills(&press_out).iter().any(|&(_, c)| c == blue),
            "按下未拖开不该有选择框"
        );

        // 左键拖开：蓝色半透明矩形，从按下像素拉到当前鼠标。
        let mut input = base_input();
        input.events.push(moved(end));
        let drag = frame_out(&mut state, &ctx, input);
        let fills = rect_fills(&drag);
        let rect = fills
            .iter()
            .find(|&&(_, c)| c == blue)
            .map(|&(r, _)| r)
            .unwrap_or_else(|| panic!("拖动中应有蓝色选择框: {fills:?}"));
        assert!(
            rect.width() > 32.0 && rect.height() > 32.0,
            "矩形应跨多格: {rect:?}"
        );
        assert!(
            (rect.min.x - start.x.min(end.x)).abs() < 1.0
                && (rect.max.x - start.x.max(end.x)).abs() < 1.0
                && (rect.min.y - start.y.min(end.y)).abs() < 1.0
                && (rect.max.y - start.y.max(end.y)).abs() < 1.0,
            "矩形从按下像素拉到当前鼠标: {rect:?} vs {start:?}..{end:?}"
        );

        // 松开左键收掉加选框（egui 按键状态跨帧保留，不松手 mode 会一直占着
        // BoxSelect，右键进不了框选——真机操作也是拖完必松手）。
        let mut input = base_input();
        input.events.push(egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        });
        let _ = frame_out(&mut state, &ctx, input);

        // 右键拖开：红色减选框。
        let mut input = base_input();
        input.events.push(moved(start));
        input.events.push(press(start, true));
        let _ = frame_out(&mut state, &ctx, input);
        let mut input = base_input();
        input.events.push(moved(end));
        let sub = frame_out(&mut state, &ctx, input);
        assert!(
            rect_fills(&sub).iter().any(|&(_, c)| c == red),
            "右键拖动应有红色减选框: {:?}",
            rect_fills(&sub)
        );

        // 松开右键收掉减选框。
        let mut input = base_input();
        input.events.push(egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        });
        let _ = frame_out(&mut state, &ctx, input);

        // 两键都已松手：矩形消失。
        let mut input = base_input();
        input.events.push(moved(end));
        let after = frame_out(&mut state, &ctx, input);
        assert!(
            !rect_fills(&after)
                .iter()
                .any(|&(_, c)| c == blue || c == red),
            "松手后不再有选择框"
        );
    }

    /// 端口自加（用户要求）：状态栏最右侧的「琴键」开关切换 `flags.show_piano`。
    /// 顺带钉两条配套行为：琴键开着时每帧上报 `piano_rect`（快捷键悬停路由用），
    /// 关掉后下一帧矩形清空（悬停原琴键位置不再走琴键缩放档）。
    #[test]
    fn status_bar_keys_toggle_flips_show_piano_and_reports_the_piano_rect() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None;
        assert!(state.flags.show_piano, "出厂应显示琴键");

        // 第一帧布局落定：琴键面板上报矩形，且应紧贴最底那条状态栏的上方。
        let _ = frame_out(&mut state, &ctx, base_input());
        let piano = state.piano_rect.expect("琴键开着时每帧要有 piano_rect");
        assert!(
            piano.max.y < 900.0,
            "琴键面板应在状态栏（最底条）上方，不该贴到窗口底: {piano:?}"
        );

        // 开关在状态栏最右：距右缘 4、宽 42，中点 x = 1440-46+21；y 取状态栏中点。
        let at = egui::pos2(1440.0 - 42.0 - 4.0 + 21.0, piano.max.y + 12.0);
        let press = |state: &mut EditorState| {
            let mut input = base_input();
            input.events.push(egui::Event::PointerMoved(at));
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
            let _ = frame_out(state, &ctx, input);
        };
        let release = |state: &mut EditorState| {
            let mut input = base_input();
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            });
            let _ = frame_out(state, &ctx, input);
        };

        press(&mut state);
        assert!(state.flags.show_piano, "只按下不该翻转");
        release(&mut state);
        assert!(!state.flags.show_piano, "松手应关掉琴键");
        let _ = frame_out(&mut state, &ctx, base_input());
        assert!(
            state.piano_rect.is_none(),
            "琴键关掉后 piano_rect 要清空，悬停原位置不能再走琴键缩放档"
        );

        press(&mut state);
        release(&mut state);
        assert!(state.flags.show_piano, "再点一次要开回来");
        let _ = frame_out(&mut state, &ctx, base_input());
        assert!(state.piano_rect.is_some(), "重新打开后矩形要恢复上报");
    }

    /// 用真鼠标点开顶栏一个标题，并确认这一帧闸门已经抬起（`menubar_open` 由
    /// `toolbar::show_menu_bar` 每帧从 egui 的 bar 状态读出来）。
    fn open_menubar(state: &mut EditorState, ctx: &egui::Context, title: &str) {
        click_text(state, ctx, title);
        assert!(state.menubar_open, "{title} 的下拉应当是开着的");
    }

    /// 按下 → 继续按住 → 抬起，三点同一处。挡板要吃掉的是「按下」那一下：
    /// egui 的 `clicked` 要按下与抬起落在同一个控件，抬指单独落下不算。
    fn click_point(state: &mut EditorState, ctx: &egui::Context, at: egui::Pos2) {
        drag_between(state, ctx, at, at);
    }

    fn secondary_click_point(state: &mut EditorState, ctx: &egui::Context, at: egui::Pos2) {
        frame_out(state, ctx, base_input());
        frame_out(state, ctx, base_input());
        for pressed in [true, false] {
            let mut raw = base_input();
            raw.events.push(egui::Event::PointerMoved(at));
            raw.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Default::default(),
            });
            frame_out(state, ctx, raw);
        }
    }

    fn has_note(state: &EditorState, tick: u32, layer: u32) -> bool {
        state
            .doc
            .song
            .columns
            .get(&tick)
            .is_some_and(|c| c.notes.contains_key(&layer))
    }

    /// 用户要求 #129：菜单改回「鼠标移开不关」，但关掉它的那一次点击不许穿透到别处。
    /// 原版只有一道门：`show_menu_ext.gml:21` 的 `window += w_menu`——菜单在屏期间
    /// 全屏的输入轮询整帧停摆，`menu_draw.gml:202` 松开手才把它自毁。
    /// 端口这边读输入的三路各有各的算法（画布走 `canvas_blocked`、egui 控件走
    /// `app::click_shield` 那层挡板、琴键读裸指针），所以三路各钉一条用例，
    /// 每条都带一次「菜单没开时同一个点真的会起作用」的对照。
    #[test]
    fn a_dismiss_click_on_the_canvas_only_closes_the_menu() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let _ = frame_out(&mut state, &ctx, base_input());
        // 取靠右下的一格：既要离「文件」那条下拉远些（落在弹层上就不算穿透测试），
        // 也要避开标尺手柄和两侧的滚动条带。
        let (tick, layer) = (24i64, 6i64);
        let at = state.viewport.cell_rect(tick, layer).center();
        let (tick, layer) = (tick as u32, layer as u32);
        assert!(!has_note(&state, tick, layer), "这一格得是空的才好对照");

        open_menubar(&mut state, &ctx, "文件");
        click_point(&mut state, &ctx, at);
        assert!(!state.menubar_open, "菜单外的一次点击应已收掉菜单");
        assert!(
            !has_note(&state, tick, layer),
            "关菜单那次点击不许穿透到画布落下音符"
        );

        click_point(&mut state, &ctx, at);
        assert!(
            has_note(&state, tick, layer),
            "对照：没有菜单时同一个点会落一个音符"
        );
    }

    /// 挡板那一路：状态栏最右的「琴键」开关是 egui 控件（`Sense::click` 的矩形），
    /// 画布的 `canvas_blocked` 管不到它，靠 `Order::Middle` 那层透明挡板盖住。
    #[test]
    fn a_dismiss_click_on_a_status_bar_button_only_closes_the_menu() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let _ = frame_out(&mut state, &ctx, base_input());
        let piano = state.piano_rect.expect("琴键开着时每帧要有 piano_rect");
        // 与上面那条开关用例同一套坐标：距右缘 4、宽 42，y 取状态栏中线。
        let at = egui::pos2(1440.0 - 42.0 - 4.0 + 21.0, piano.max.y + 12.0);
        let before = state.flags.show_piano;

        open_menubar(&mut state, &ctx, "文件");
        click_point(&mut state, &ctx, at);
        assert!(!state.menubar_open, "菜单外的一次点击应已收掉菜单");
        assert_eq!(
            state.flags.show_piano, before,
            "关菜单那次点击不该按到状态栏的「琴键」开关"
        );

        click_point(&mut state, &ctx, at);
        assert_ne!(
            state.flags.show_piano, before,
            "对照：没有菜单时同一个点就是「琴键」开关"
        );
    }

    /// 琴键那一路：面板读的是裸指针位置（原版 `draw_piano.gml:25/44/88` 三段各带
    /// `window = 0`），挡板与画布门都够不着，只能自己让位。
    #[test]
    fn a_dismiss_click_on_the_keyboard_only_closes_the_menu() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let _ = frame_out(&mut state, &ctx, base_input());
        let panel = state.piano_rect.expect("琴键开着时每帧要有 piano_rect");
        let at = panel.center();
        let before = state.current.key;

        open_menubar(&mut state, &ctx, "文件");
        click_point(&mut state, &ctx, at);
        assert!(!state.menubar_open, "菜单外的一次点击应已收掉菜单");
        assert_eq!(state.current.key, before, "关菜单那次点击不该选中琴键");

        click_point(&mut state, &ctx, at);
        assert_ne!(
            state.current.key, before,
            "对照：没有菜单时同一个点会选中一个键"
        );
    }

    /// #124 的回退面（用户要求 #129「菜单移开即收改回原来的」）：鼠标从标题移开、
    /// 在画布上晃几帧，下拉必须还开着——原版只有抬手和 ESC 才收
    /// （`menu_draw.gml:202` 的自毁条件是松开左键）。
    #[test]
    fn moving_the_pointer_away_keeps_the_dropdown_open() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let _ = frame_out(&mut state, &ctx, base_input());
        open_menubar(&mut state, &ctx, "文件");
        for _ in 0..4 {
            let far = egui::pos2(700.0, 620.0);
            let mut raw = base_input();
            raw.events.push(egui::Event::PointerMoved(far));
            frame_out(&mut state, &ctx, raw);
        }
        assert!(
            state.menubar_open,
            "鼠标移开不该收起下拉（那是 #124，已回退）"
        );

        press_key(&mut state, &ctx, egui::Key::Escape);
        // 闸门口径是「本帧开过」，收菜单那一帧仍为真，收干净要再看一帧。
        frame(&mut state, &ctx);
        assert!(!state.menubar_open, "ESC 应当收起下拉");
    }

    /// 帮助菜单倒数第二条（原版 `control_draw.gml:1783/1878` 的 `Changelist...` /
    /// 「更新历史......」）＝ `menu_click.gml:224/229` 的 `window = w_changelist`。
    #[test]
    fn help_menu_opens_changelist() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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

    /// MAESTRO 那条导出（端口自加）走完整链路：菜单入口 → 一次性提示 → 窗口 →
    /// 保存框扩展名 → 真落盘。断言的是磁盘上的字节头与报告，不是内部函数返回值。
    #[test]
    fn maestro_menu_entry_notice_dialog_and_file_are_real() {
        let _prefs = crate::isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None; // for_test() 带着开机欢迎窗，先关掉再走入口
        state.prefs.warning_maestro = false;

        dispatch(&mut state, commands::Command::OpenMaestroExport, &ctx);
        assert_eq!(state.modals.items.len(), 1, "第一次用要先提示机器依赖");
        assert_eq!(state.window, None, "提示没答完之前不开窗");
        assert!(state.maestro_report.is_some(), "开窗前先算好改写清单");

        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        assert_eq!(state.window, Some(WindowKind::MaestroExport));
        assert!(
            state.prefs.warning_maestro,
            "提示看过就落盘，同 warning_schematic 的规矩"
        );

        // 上屏检查（「代码里有 ≠ 看得见」）：四档模式、清单标题、按钮都要真画出来。
        let _zh = crate::LangGuard::set(1);
        state.prefs.language = 1;
        let painted: Vec<String> = settle(&mut state, &ctx)
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for want in [
            "MAESTRO 机体节目单",
            "时间轴模式",
            "对歌曲的改写",
            "round",
            "approximate",
            "flexible",
            "none",
            "导出",
            "关闭",
        ] {
            assert!(
                painted.iter().any(|t| t == want),
                "窗口里应画得出「{want}」: {painted:?}"
            );
        }

        dispatch(&mut state, commands::Command::CloseWindow, &ctx);
        dispatch(&mut state, commands::Command::OpenMaestroExport, &ctx);
        assert!(state.modals.is_empty(), "第二次不再提示");
        assert_eq!(state.window, Some(WindowKind::MaestroExport));

        // 换取整模式：清单跟着重算，出厂档是 round。
        assert_eq!(state.maestro_options.timing, nbs_io::TimingMode::Round);
        state.maestro_options.timing = nbs_io::TimingMode::None;
        state.refresh_maestro_preview();
        frame(&mut state, &ctx);
        state.maestro_options.timing = nbs_io::TimingMode::Round;
        state.refresh_maestro_preview();

        // 空歌要像 Schematic 窗口那样被闸门拦下：只报错，不弹保存框。
        let demo = state.doc.song.clone();
        state.doc.replace_song(nbs_core::model::Song::new());
        state.refresh_maestro_preview();
        dispatch(&mut state, commands::Command::ExportMaestroChecked, &ctx);
        assert_eq!(state.modals.items.len(), 1, "空歌该被闸门拦下");
        assert!(state.file_dialog.is_none(), "拦下之后不该有保存框");
        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        state.doc.replace_song(demo);
        state.refresh_maestro_preview();

        // 保存框：扩展名与默认名都要是 .schem（默认名取当前文件名，未存过的歌是 untitled）。
        state.doc.song.filename = Some(PathBuf::from("Rust Rewrite Demo.nbs"));
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportMaestro),
            &ctx,
        );
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "schem", "机体吃的是 WorldEdit 的 .schem");
        assert!(dialog.save);
        assert_eq!(dialog.name, "Rust Rewrite Demo.schem");

        let dir = scratch("maestro");
        let out = dir.join("sheet.schem");
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        let bytes = std::fs::read(&out).expect("节目单已落盘");
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzip 魔数");
        assert!(
            state.export_status.is_some(),
            "窗口里要能看到这次改写了什么"
        );
        assert_eq!(
            state.window,
            Some(WindowKind::MaestroExport),
            "导出完窗口留着"
        );
        assert!(
            state.maestro_report.as_ref().unwrap().streams > 0,
            "示例歌该有声音流"
        );
        let plain = state.export_status.clone().unwrap_or_default();
        assert!(
            !plain.contains("/schem list"),
            "英文名不该带改名提示：{plain}"
        );

        // 创世神（WorldEdit）的目录监视器只收 ASCII 文件名：中文名不进 `/schem list`，
        // 按名加载也判非法，所以状态区要在导出后点一句（投影模组不受影响）。
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportMaestro),
            &ctx,
        );
        let cjk = dir.join("无间的钟.schem");
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(cjk.clone()),
            &ctx,
        );
        let hinted = state.export_status.clone().unwrap_or_default();
        assert!(
            hinted.contains("/schem list"),
            "中文名该在状态区提示改名：{hinted}"
        );
        let _ = std::fs::remove_file(&cjk);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fractal 那条导出（端口自加）走完整链路：菜单入口 → 一次性提示 → 窗口上屏 →
    /// 换布局重算 → 空歌闸门 → 保存框扩展名 → 真落盘。断言的是磁盘字节与报告，
    /// 不是内部函数返回值。
    #[test]
    fn fractal_menu_entry_notice_dialog_and_file_are_real() {
        let _prefs = crate::isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None; // for_test() 带着开机欢迎窗，先关掉再走入口
        state.prefs.warning_fractal = false;

        dispatch(&mut state, commands::Command::OpenFractalExport, &ctx);
        assert_eq!(
            state.modals.items.len(),
            1,
            "第一次用要先说明「这是自带红石的整机」"
        );
        assert_eq!(state.window, None, "提示没答完之前不开窗");
        assert!(state.fractal_report.is_some(), "开窗前先算好机体与清单");

        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        assert_eq!(state.window, Some(WindowKind::FractalExport));
        assert!(
            state.prefs.warning_fractal,
            "提示看过就落盘，同 warning_maestro 的规矩"
        );

        // 演示曲 tempo 10、音符全在偶数 tick ⇒ 折算到 game tick 后每个时刻都能被 4 整除，
        // 上游 Linear 取「能整除全部时刻的最粗档」= Scale4：每 4 gt 一格、5 音/秒。
        // 这一档是这首歌的性质，不是我们选的，所以断言钉在这里防它悄悄变。
        let lin = state.fractal_report.clone().unwrap();
        assert_eq!(
            (lin.layout, lin.cell_gt, lin.notes_per_sec()),
            (nbs_io::FractalLayout::Linear, 4, 5.0)
        );

        // 上屏检查（「代码里有 ≠ 看得见」）：两档布局、20 t/s 开关、按钮都要真画出来。
        let _zh = crate::LangGuard::set(1);
        state.prefs.language = 1;
        let painted: Vec<String> = settle(&mut state, &ctx)
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for want in [
            "机体布局",
            "Linear（主干＋支道）",
            "Compact（变长紧凑）",
            "每 game tick 一音（20 t/s）",
            "机体与改写",
            "导出",
            "关闭",
        ] {
            assert!(
                painted.iter().any(|t| t == want),
                "窗口里应画得出「{want}」: {painted:?}"
            );
        }

        dispatch(&mut state, commands::Command::CloseWindow, &ctx);
        dispatch(&mut state, commands::Command::OpenFractalExport, &ctx);
        assert!(state.modals.is_empty(), "第二次不再提示");
        assert_eq!(state.window, Some(WindowKind::FractalExport));

        // 换 Compact：每格间隔与道数都要跟着重算。
        state.fractal_options.layout = nbs_io::FractalLayout::Compact;
        state.refresh_fractal_preview();
        let com = state.fractal_report.clone().unwrap();
        // 演示曲没有奇数 tick 的音符 ⇒ 奇道整条是空的 ⇒ 只剩一条 2 gt 一格的道。
        // 20 t/s 开关亮着也不许把这台机器报成每刻一音（对照：两条反相道都用上才是 1 gt）。
        assert_eq!(
            (com.layout, com.tracks, com.cell_gt, com.notes_per_sec()),
            (nbs_io::FractalLayout::Compact, 1, 2, 10.0),
            "只用上一条反相道时报错：{com:?}"
        );
        state.fractal_options.layout = nbs_io::FractalLayout::Linear;
        state.refresh_fractal_preview();

        // 空歌要像节目单窗口那样被闸门拦下：只报错，不弹保存框。
        let demo = state.doc.song.clone();
        state.doc.replace_song(nbs_core::model::Song::new());
        state.refresh_fractal_preview();
        dispatch(&mut state, commands::Command::ExportFractalChecked, &ctx);
        assert_eq!(state.modals.items.len(), 1, "空歌该被闸门拦下");
        assert!(state.file_dialog.is_none(), "拦下之后不该有保存框");
        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        state.doc.replace_song(demo);
        state.refresh_fractal_preview();

        // 保存框：扩展名与默认名都要是 .litematic（默认名取当前文件名）。
        state.doc.song.filename = Some(PathBuf::from("Rust Rewrite Demo.nbs"));
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportFractal),
            &ctx,
        );
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "litematic", "Litematica 吃的是 .litematic");
        assert!(dialog.save);
        assert_eq!(dialog.name, "Rust Rewrite Demo.litematic");

        let dir = scratch("fractal");
        let out = dir.join("machine.litematic");
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        let bytes = std::fs::read(&out).expect("机体投影已落盘");
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzip 魔数");
        assert!(
            state.export_status.is_some(),
            "窗口里要能看到这台机器的口径"
        );
        assert_eq!(
            state.window,
            Some(WindowKind::FractalExport),
            "导出完窗口留着"
        );
        let rep = state.fractal_report.clone().unwrap();
        assert!(rep.notes_kept > 0, "示例歌该有音符进机体");
        let status = state.export_status.clone().unwrap_or_default();
        assert!(
            status.contains("game tick"),
            "状态区要写出实际时间基：{status}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 编码唱片数据箱那条导出（端口自加）走完整链路：菜单入口 → 一次性提示 → 窗口上屏 →
    /// 换机体重算 → 空歌闸门 → 保存框扩展名 → 真落盘。断言的是磁盘字节与报告。
    #[test]
    fn disc_chest_menu_entry_notice_dialog_and_file_are_real() {
        let _prefs = crate::isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.window = None; // for_test() 带着开机欢迎窗，先关掉再走入口
        state.prefs.warning_disc_chest = false;

        dispatch(&mut state, commands::Command::OpenDiscChestExport, &ctx);
        assert_eq!(
            state.modals.items.len(),
            1,
            "第一次用要先说明「这只是数据箱，得贴在已建好的机体里」"
        );
        assert_eq!(state.window, None, "提示没答完之前不开窗");
        assert!(state.disc_chest_report.is_some(), "开窗前先算好箱子与清单");

        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        assert_eq!(state.window, Some(WindowKind::DiscChestExport));
        assert!(
            state.prefs.warning_disc_chest,
            "提示看过就落盘，同 warning_maestro / warning_fractal 的规矩"
        );

        // 出厂档 = 上游 defaultConfig 的 20 音/秒：每件乐器偶/奇两条臂 ⇒ 20 轨 40 个子区域。
        let twenty = state.disc_chest_report.clone().unwrap();
        assert_eq!(
            (
                twenty.machine(),
                twenty.lanes,
                twenty.regions,
                twenty.grid_rate
            ),
            (nbs_io::DiscChestMachine::TwentyHz, 2, 40, 20.0)
        );

        // 上屏检查（「代码里有 ≠ 看得见」）：两档机体、两种刻号处理、层高与三个按钮都要真画出来。
        let _zh = crate::LangGuard::set(1);
        state.prefs.language = 1;
        let painted: Vec<String> = settle(&mut state, &ctx)
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for want in [
            "机体",
            "20 t/s 机体（20 轨，偶/奇双臂）",
            "10 t/s 机体（20 轨，每件乐器一条臂）",
            "折算到机器网格",
            "保留 NBS 刻号",
            "层高",
            "数据箱与改写",
            "导出",
            "关闭",
        ] {
            assert!(
                painted.iter().any(|t| t == want),
                "窗口里应画得出「{want}」: {painted:?}"
            );
        }

        dispatch(&mut state, commands::Command::CloseWindow, &ctx);
        dispatch(&mut state, commands::Command::OpenDiscChestExport, &ctx);
        assert!(state.modals.is_empty(), "第二次不再提示");
        assert_eq!(state.window, Some(WindowKind::DiscChestExport));

        // 换 10 音/秒：一条臂一件乐器，子区域数减半，网格速度也跟着减半。
        state.disc_chest_options.machine = nbs_io::DiscChestMachine::TenHz;
        state.refresh_disc_chest_preview();
        let ten = state.disc_chest_report.clone().unwrap();
        assert_eq!(
            (ten.machine(), ten.lanes, ten.regions, ten.grid_rate),
            (nbs_io::DiscChestMachine::TenHz, 1, 20, 10.0),
            "换机体后清单要跟着重算：{ten:?}"
        );
        state.disc_chest_options.machine = nbs_io::DiscChestMachine::TwentyHz;
        state.refresh_disc_chest_preview();

        // 空歌：这条路的编码端会直接拒绝（没有可编码的音符），闸门照样拦下且不弹保存框。
        let demo = state.doc.song.clone();
        state.doc.replace_song(nbs_core::model::Song::new());
        state.refresh_disc_chest_preview();
        assert!(state.disc_chest_error.is_some(), "空歌预览就该报错");
        dispatch(&mut state, commands::Command::ExportDiscChestChecked, &ctx);
        assert_eq!(state.modals.items.len(), 1, "空歌该被闸门拦下");
        assert!(state.file_dialog.is_none(), "拦下之后不该有保存框");
        dispatch(&mut state, commands::Command::AnswerModal(true), &ctx);
        state.doc.replace_song(demo);
        state.refresh_disc_chest_preview();

        // 保存框：扩展名与默认名都要是 .litematic（同 Fractal 那条，容器就是 Litematica）。
        state.doc.song.filename = Some(PathBuf::from("Rust Rewrite Demo.nbs"));
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDiscChest),
            &ctx,
        );
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "litematic", "Litematica 吃的是 .litematic");
        assert!(dialog.save);
        assert_eq!(dialog.name, "Rust Rewrite Demo.litematic");

        let dir = scratch("disc_chest");
        let out = dir.join("disc-chests.litematic");
        dispatch(
            &mut state,
            commands::Command::DialogConfirm(out.clone()),
            &ctx,
        );
        let bytes = std::fs::read(&out).expect("数据箱已落盘");
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzip 魔数");
        assert!(
            state.export_status.is_some(),
            "窗口里要能看到这批箱子的口径"
        );
        assert_eq!(
            state.window,
            Some(WindowKind::DiscChestExport),
            "导出完窗口留着"
        );
        let rep = state.disc_chest_report.clone().unwrap();
        assert!(rep.notes_kept > 0, "示例歌该有音符进数据箱");
        assert!(rep.chests > 0 && rep.boxes > 0, "箱子与潜影盒要真放进去");
        let status = state.export_status.clone().unwrap_or_default();
        assert!(
            status.contains("game tick") && status.contains("已经建好的机体"),
            "状态区要写出时间基和「要先有机体」：{status}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空名称必须像原版一样报错，并且不留半成品。
    #[test]
    fn datapack_export_refuses_an_empty_name() {
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
            click_text(
                state,
                ctx,
                match front.buttons {
                    state::ModalButtons::Ok => "确定",
                    state::ModalButtons::YesNo | state::ModalButtons::YesNoCancel => "是",
                },
            );
        }
        panic!("模态框一直没答完，流程可能卡住了");
    }

    /// 三键确认框必须真的画出「是 / 否 / 取消」；点取消后这一首还在，并回到原来那首。
    #[test]
    fn close_tab_cancel_button_is_drawn_and_works() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let home = state.active_tab().expect("开机至少有一首");
        let mut other = nbs_core::model::Song::new();
        other.filename = Some(PathBuf::from("gui-close.nbs"));
        other.set(4, 0, nbs_core::model::Note::new(0, 48));
        other.recalculate();
        let second = state.push_tab(other);

        dispatch(&mut state, commands::Command::CloseTab(second), &ctx);
        assert_eq!(state.modals.items.len(), 1, "脏标签应先弹确认");
        assert_eq!(state.pending_close.map(|p| p.id), Some(second));
        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        assert_eq!(state.modals.items.len(), 1, "模态框画完也应还在");
        let x = |label: &str| text_center_if(&out.shapes, |t| t == label).x;
        assert!(x("是") < x("否"), "按钮顺序应是 是 / 否 / 取消");
        assert!(x("否") < x("取消"), "取消键必须真的画在第三位");

        click_text(&mut state, &ctx, "取消");
        assert!(state.modals.is_empty());
        assert_eq!(state.store.len(), 2, "取消不能把标签关掉");
        assert_eq!(state.active_tab(), Some(home));
        assert!(state.pending_close.is_none());
        assert!(state.parked_doc(second).unwrap().song.dirty);
    }

    /// 退出确认也必须走真实命令闸门：点第三键后回到原标签，窗口保持打开。
    #[test]
    fn quit_cancel_button_is_drawn_and_keeps_the_editor_open() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let home = state.active_tab().expect("开机至少有一首");
        let mut other = nbs_core::model::Song::new();
        other.filename = Some(PathBuf::from("quit-gui.nbs"));
        other.set(4, 0, nbs_core::model::Note::new(0, 48));
        other.recalculate();
        state.push_tab(other);

        dispatch(&mut state, commands::Command::Exit, &ctx);
        frame(&mut state, &ctx);
        click_text(&mut state, &ctx, "取消");
        assert!(state.modals.is_empty());
        assert!(state.quit.is_none());
        assert!(!state.quit_confirmed);
        assert_eq!(state.store.len(), 2);
        assert_eq!(state.active_tab(), Some(home));
    }

    /// 真点鼠标走完原版 Schematic 导出流程：菜单 → `calculate_size()` 的提问 →
    /// 设计页三个单选 → 「使用默认值」的确认 → 方块页下拉 → 导出 → 磁盘上的 gzip 字节。
    #[test]
    fn clicking_through_the_schematic_export_window() {
        let ctx = egui::Context::default();
        let _prefs = isolate_prefs();
        let mut state = EditorState::for_test();
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
        // 格式下拉：选 1.11-1.12 再切回结构方块，structure/minecraft_old 跟着 set_format 派生。
        click_text(&mut state, &ctx, ".nbt (结构方块)");
        click_text(&mut state, &ctx, ".schematic (1.11-1.12)");
        assert!(!state.schematic_options.structure && state.schematic_options.format == 2);
        assert!(!state.schematic_options.minecraft_old);
        click_text(&mut state, &ctx, ".schematic (1.11-1.12)");
        click_text(&mut state, &ctx, ".nbt (结构方块)");
        assert!(state.schematic_options.structure && state.schematic_options.format == 0);
        click_text(&mut state, &ctx, "使用命令方块");
        assert!(state.schematic_options.command_block);
        // 计数器：structure+command_block 时改走「导出时计算」分支（原版 gml:137-145）。
        assert!(
            drawn_texts(&mut state, &ctx)
                .iter()
                .any(|t| t == "导出时计算"),
            "命令方块开启后三个计数器都应显示「导出时计算」"
        );

        // 「使用默认值」先问「你确定吗？」，答否保持原样。
        click_text(&mut state, &ctx, "使用默认值");
        assert_eq!(state.modals.items.len(), 1, "原版这里先弹确认");
        assert_eq!(state.modals.items[0].buttons, state::ModalButtons::YesNo);
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

        // 声音页：structure+command_block 开着，应显示首选声源而不是闸门提示。
        click_text(&mut state, &ctx, "声音");
        assert_eq!(state.schematic_tab, 2, "第三个页签是声音");
        assert!(
            drawn_texts(&mut state, &ctx)
                .iter()
                .any(|t| t == "首选声源"),
            "声音页应有首选声源单选"
        );

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
        // 经典图样要选 1.11-1.12 格式造：原版出厂 format=0（结构方块），那不是 .schematic。
        let mut classic = nbs_io::schematic::SchematicOptions::default();
        classic.set_format(2);
        nbs_io::schematic::export_schematic(&nbs_core::Song::demo(), &file, &classic, None)
            .expect("导出经典图样");

        let mut state = EditorState::for_test();
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
        let mut s2 = EditorState::for_test();
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
            s2.modals.items[0].buttons,
            state::ModalButtons::Ok,
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

    /// 文件 → 导入 → 背景图片……：真造一张 PNG，选完当场烘焙成贴图并真的画出来；
    /// 坏文件按原版语义清掉背景，只留状态栏一行说明。
    #[test]
    fn gui_imports_a_background_image_and_paints_it() {
        let ctx = egui::Context::default();
        let _prefs = isolate_prefs();
        let dir = scratch("background-import");
        let file = dir.join("wall.png");
        image::RgbaImage::from_fn(640, 480, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 199) as u8, 90, 255])
        })
        .save(&file)
        .unwrap();

        // 整幅宽的贴图网格只可能是背景图：精灵图都是几十像素的方块。
        // 同时带回它在整帧里的序号 —— 背景图必须排在最前，各面板才盖得住它。
        let full_width_meshes =
            |state: &mut EditorState, ctx: &egui::Context| -> Vec<(usize, egui::TextureId, f32)> {
                frame_out(state, ctx, base_input())
                    .shapes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, cs)| match &cs.shape {
                        egui::Shape::Mesh(m) => {
                            let b = m.calc_bounds();
                            (b.width() >= 1439.5).then(|| (i, m.texture_id, b.height()))
                        }
                        _ => None,
                    })
                    .collect()
            };

        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        assert!(
            full_width_meshes(&mut state, &ctx).is_empty(),
            "没导入时不该有整窗贴图"
        );

        click_text(&mut state, &ctx, "文件");
        click_text(&mut state, &ctx, "导入");
        click_text(&mut state, &ctx, "背景图片……");
        let d = state
            .file_dialog
            .as_ref()
            .expect("导入背景图片要弹文件对话框");
        assert!(!d.save, "导入用的是打开式对话框");
        let spec = d.picker_spec();
        assert_eq!(spec.title, "打开背景图片", "原版 condstr 的中文标题");
        assert_eq!(spec.filters[0].name, "Image Files (*.png, *.jpg, *.jpeg)");
        assert_eq!(spec.filters[0].patterns, "*.png;*.jpg;*.jpeg");
        assert_eq!(state.window, Some(WindowKind::FileOpen));

        dispatch(&mut state, commands::Command::DialogConfirm(file), &ctx);
        assert_eq!(state.window, None, "原版选完直接生效，不开任何窗口");
        let wp = state.wallpaper.as_ref().expect("背景图已烘焙");
        // 640×480 预缩放 1.5 ⇒ 960×720，再按 2% 降采样 ⇒ 20×15。
        assert_eq!(wp.size, egui::vec2(20.0, 15.0));
        let tex = wp.texture.id();
        let h = full_width_meshes(&mut state, &ctx);
        assert_eq!(h.len(), 1, "整窗贴图只有一张");
        assert_eq!(h[0].1, tex, "画出来的就是刚导入的那张贴图");
        assert!(h[0].2 > 900.0, "cover 要溢出窗口高度，实测 {h:?}");
        // 层序：原版把壁纸画在所有面板之前（control_draw.gml:48-67），端口必须压在
        // 整帧最底下——画在面板之后会把琴键顶部那几像素盖成图片色（实测踩过）。
        assert_eq!(h[0].0, 0, "背景图是整帧第一个图元，各面板都在它之后盖上来");

        // 坏文件：原版先 `sprite_delete` 再 `sprite_add`，读失败就没有背景了，
        // 端口同样整体覆盖；原版这里是静默的，端口在状态栏给一句。
        let bad = dir.join("bad.png");
        std::fs::write(&bad, b"not an image").unwrap();
        click_text(&mut state, &ctx, "文件");
        click_text(&mut state, &ctx, "导入");
        click_text(&mut state, &ctx, "背景图片……");
        let _lock = crate::frame_lock();
        dispatch(&mut state, commands::Command::DialogConfirm(bad), &ctx);
        assert!(
            state.wallpaper.is_none(),
            "读失败不留旧背景（原版 sprite_delete 在前）"
        );
        assert!(msg(&state).contains("背景图片"), "{}", msg(&state));
        drop(_lock);
        assert!(full_width_meshes(&mut state, &ctx).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 音色窗口：v6 起列出 20 种内置乐器，但快捷键只印原版真绑了的那 18 个
    /// （`control_draw.gml:735-757`：Ctrl+1..9 → 0..8，Ctrl+Shift+1..9 → 9..17）。
    /// 铜块一族的后两个按了没反应，就不许在界面上给它们编一个组合键。
    #[test]
    fn instruments_window_lists_twenty_and_advertises_only_real_bindings() {
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
            9,
            "Ctrl+Shift+数字应当只有 9 个（#122：原版到索引 17）"
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
        let mut state = EditorState::for_test();
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

    /// 欢迎窗「打开歌曲」必须走 replace=1 的打开框；取消后欢迎窗收起但原标签不动。
    #[test]
    fn greeting_load_button_opens_the_replace_dialog() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Greeting);
        let first = state.active_tab().unwrap();

        click_text(&mut state, &ctx, "打开歌曲");
        assert_eq!(state.window, Some(WindowKind::FileOpen));
        assert_eq!(
            state.pending_replace.as_ref().map(|p| p.target),
            Some(first)
        );
        click_text(&mut state, &ctx, "取消");
        assert_eq!(state.window, None);
        assert!(state.pending_replace.is_none());
        assert_eq!(state.active_tab(), Some(first));
        assert_eq!(state.store.len(), 1);
    }

    /// 标签条真的画出来、点标题能切歌、点加号能新建、点关闭叉能关掉指定标签。
    #[test]
    fn song_tabs_render_switch_add_and_close() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let mut first = nbs_core::model::Song::new();
        first.filename = Some(PathBuf::from("first.nbs"));
        state.doc.replace_song(first);
        let first_id = state.active_tab().unwrap();
        let mut second = nbs_core::model::Song::new();
        second.filename = Some(PathBuf::from("second.nbs"));
        let second_id = state.push_tab(second);

        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "first.nbs"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "second.nbs"), "{texts:?}");
        click_text(&mut state, &ctx, "second.nbs");
        assert_eq!(state.active_tab(), Some(second_id));

        // 1440 宽、2 标签时 span=179，加号命中中心 = 7 + 2*179 + 14。
        click_point(&mut state, &ctx, egui::pos2(379.0, 40.0));
        assert_eq!(state.store.len(), 3, "点加号要真的新建标签");
        // Fluent 第一枚关闭叉：x=7+179-35，宽30；y=29，高22。
        click_point(&mut state, &ctx, egui::pos2(166.0, 40.0));
        assert_eq!(state.store.len(), 2);
        assert!(!state.store.contains(first_id), "叉号关的是第一枚标签");
    }

    /// 参考音频、画笔和乐器面板滚动位置都跟着标签走，关标签后落在下一首的那份上。
    #[test]
    fn reference_config_and_instrument_scroll_follow_their_tab() {
        let _prefs = isolate_prefs();
        let mut state = EditorState::for_test();
        state.audio = Box::new(nbs_audio::SilentEngine::new());
        let first = state.active_tab().unwrap();
        let harp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/Sounds/harp.ogg");
        let bell = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/Sounds/bell.ogg");
        state.reference.path = Some(harp.clone());
        state.reference.option = 1;
        state.reference.offset_ms = -125;
        state.reference.volume = 40;
        state.current = nbs_core::model::Note::new(7, 60);
        state.insbox_start = 21;

        let second = state.open_tab(nbs_core::Document::new());
        assert_eq!(state.reference.path, None);
        assert_eq!((state.reference.option, state.reference.volume), (2, 100));
        assert_eq!(state.current, nbs_core::model::Note::default());
        assert_eq!(state.insbox_start, 0);
        state.reference.path = Some(bell.clone());
        state.reference.option = 0;
        state.reference.offset_ms = 250;
        state.reference.volume = 70;

        assert!(state.switch_tab(first));
        assert_eq!(state.reference.path, Some(harp));
        assert_eq!(state.reference.option, 1);
        assert_eq!(state.reference.offset_ms, -125);
        assert_eq!(state.reference.volume, 40);
        assert_eq!(state.current, nbs_core::model::Note::new(7, 60));
        assert_eq!(state.insbox_start, 21);

        state.close_tab(first);
        assert_eq!(state.active_tab(), Some(second));
        assert_eq!(state.reference.path, Some(bell));
        assert_eq!(state.reference.option, 0);
        assert_eq!(state.reference.offset_ms, 250);
        assert_eq!(state.reference.volume, 70);
    }

    /// 参考音频行只在**已加载**之后才占一行（原版 `control_draw.gml:1456` 的
    /// `reference_audio >= 0` 门），删掉之后整行一并消失；加载入口是
    /// 文件→导入→参考音频……（`:1706/1801`），行内名称格只做「更换」。
    #[test]
    fn reference_row_loads_and_its_controls_change_the_real_track() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.audio = Box::new(nbs_audio::SilentEngine::new());
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;

        let has_row = |out: &egui::FullOutput| {
            out.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text)
                    if text.galley.text() == "参考音频")
            })
        };
        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        assert!(!has_row(&out), "没加载参考音频时不该画出参考行");

        dispatch(
            &mut state,
            commands::Command::OpenDialog(crate::state::DialogAction::LoadReference),
            &ctx,
        );
        assert_eq!(
            state.file_dialog.as_ref().map(|d| d.action),
            Some(crate::state::DialogAction::LoadReference),
            "文件→导入→参考音频…… 必须打开参考音频文件框"
        );
        state.file_dialog = None;
        state.window = None;
        let harp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/Sounds/harp.ogg");
        dispatch(
            &mut state,
            commands::Command::LoadReferenceAudio(harp),
            &ctx,
        );
        state.reference.volume = 40;
        state.reference.offset_ms = 100;
        let out = frame_out(&mut state, &ctx, base_input());
        assert!(has_row(&out), "加载后参考行必须出现");
        let name = text_center_if(&out.shapes, |t| t == "参考音频");
        let row_y = name.y + 2.0;

        click_point(&mut state, &ctx, name);
        assert_eq!(
            state.file_dialog.as_ref().map(|d| d.action),
            Some(crate::state::DialogAction::LoadReference),
            "名称格必须真的打开参考音频文件框"
        );
        state.file_dialog = None;
        state.window = None;

        click_point(&mut state, &ctx, egui::pos2(138.0, row_y));
        assert_eq!(state.reference.option, 0, "静音按钮要生效");
        click_point(&mut state, &ctx, egui::pos2(156.0, row_y));
        assert_eq!(state.reference.option, 1, "独奏按钮要生效");
        drag_between(
            &mut state,
            &ctx,
            egui::pos2(100.0, row_y),
            egui::pos2(100.0, row_y - 15.0),
        );
        assert_eq!(state.reference.volume, 50, "向上拖 15 应量化到 50%");
        drag_between(
            &mut state,
            &ctx,
            egui::pos2(120.0, row_y),
            egui::pos2(120.0, row_y + 12.0),
        );
        assert_eq!(state.reference.offset_ms, 88, "向下拖 12 应按原版减少 12ms");
        click_point(&mut state, &ctx, egui::pos2(174.0, row_y));
        assert_eq!(state.reference.path, None, "删除按钮要清掉当前标签的参考轨");
        assert_eq!((state.reference.option, state.reference.volume), (2, 100));
        assert_eq!(state.reference.offset_ms, 0);
        let out = frame_out(&mut state, &ctx, base_input());
        assert!(!has_row(&out), "删除后参考行要一并消失");
        state.reference.path = Some(PathBuf::from("ref.ogg"));
        state.fullscreen = true;
        let out = frame_out(&mut state, &ctx, base_input());
        assert!(!has_row(&out), "全屏时不画参考行");
        state.fullscreen = false;
        state.reference.path = Some(PathBuf::from("old.ogg"));
        dispatch(
            &mut state,
            commands::Command::LoadReferenceAudio(PathBuf::from("bad.mp3")),
            &ctx,
        );
        assert_eq!(state.reference.path, None, "加载失败不能假装旧文件仍在轨上");
    }

    #[test]
    fn copy_and_cut_use_the_global_clipboard_across_tabs() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.audio = Box::new(nbs_audio::SilentEngine::new());
        let mut song = nbs_core::model::Song::new();
        song.set(3, 0, nbs_core::model::Note::new(0, 45));
        song.recalculate();
        state.doc.replace_song(song);
        let first = state.active_tab().unwrap();
        state.doc.select_all();
        dispatch(&mut state, commands::Command::Copy, &ctx);
        assert_eq!(state.clipboard.len(), 1);
        let copied_text = state.clipboard_text.clone();

        let second = state.open_tab(nbs_core::Document::new());
        assert_eq!(state.clipboard_text, copied_text, "切标签不清全局剪贴板");
        state.hover_cell = Some((20, 1));
        dispatch(&mut state, commands::Command::Paste, &ctx);
        assert_eq!(state.doc.selection.notes.len(), 1);
        state.doc.place_selection();
        assert!(has_note(&state, 20, 1));

        assert!(state.switch_tab(first));
        state.doc.select_all();
        dispatch(&mut state, commands::Command::Cut, &ctx);
        assert!(state.doc.song.get(3, 0).is_none(), "剪切要摘走源音符");
        assert!(state.switch_tab(second));
        state.hover_cell = Some((30, 2));
        dispatch(&mut state, commands::Command::Paste, &ctx);
        state.doc.place_selection();
        assert!(has_note(&state, 30, 2), "另一首也能粘到剪切内容");
        assert_eq!(state.clipboard.len(), 1);
    }

    /// 原版 4px 起拖阈值与松手重排：拖到哪一首，台前选中项仍按 id 保持。
    #[test]
    fn song_tab_drag_threshold_reorders_and_keeps_the_active_id() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let mut first = nbs_core::model::Song::new();
        first.filename = Some(PathBuf::from("drag-first.nbs"));
        state.doc.replace_song(first);
        let first_id = state.active_tab().unwrap();
        let mut second = nbs_core::model::Song::new();
        second.filename = Some(PathBuf::from("drag-second.nbs"));
        let second_id = state.push_tab(second);
        let mut third = nbs_core::model::Song::new();
        third.filename = Some(PathBuf::from("drag-third.nbs"));
        let third_id = state.push_tab(third);
        frame_out(&mut state, &ctx, base_input());
        frame_out(&mut state, &ctx, base_input());

        let mut press = base_input();
        press
            .events
            .push(egui::Event::PointerMoved(egui::pos2(50.0, 40.0)));
        press.events.push(egui::Event::PointerButton {
            pos: egui::pos2(50.0, 40.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        frame_out(&mut state, &ctx, press);
        assert!(state.tab_drag.is_some());
        assert!(!state.tab_drag.unwrap().dragging, "刚按下还没过 4px");
        let mut slide = base_input();
        slide
            .events
            .push(egui::Event::PointerMoved(egui::pos2(53.0, 40.0)));
        frame_out(&mut state, &ctx, slide);
        assert!(!state.tab_drag.unwrap().dragging, "3px 不算起拖");
        let mut slide = base_input();
        slide
            .events
            .push(egui::Event::PointerMoved(egui::pos2(70.0, 40.0)));
        frame_out(&mut state, &ctx, slide);
        assert!(state.tab_drag.unwrap().dragging, "超过 4px 才起拖");
        let mut slide = base_input();
        slide
            .events
            .push(egui::Event::PointerMoved(egui::pos2(400.0, 40.0)));
        frame_out(&mut state, &ctx, slide);
        assert_eq!(state.tab_drag.unwrap().target, 2);
        let mut release = base_input();
        release
            .events
            .push(egui::Event::PointerMoved(egui::pos2(400.0, 40.0)));
        release.events.push(egui::Event::PointerButton {
            pos: egui::pos2(400.0, 40.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        frame_out(&mut state, &ctx, release);
        assert_eq!(state.store.order(), &[second_id, third_id, first_id]);
        assert_eq!(state.active_tab(), Some(first_id));
        assert!(state.tab_drag.is_none());
    }

    /// 原版拖拽中按 Escape 或窗口失焦：取消重排，标签顺序与台前选中项都不动。
    #[test]
    fn song_tab_drag_escape_and_focus_loss_cancel() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let mut first = nbs_core::model::Song::new();
        first.filename = Some(PathBuf::from("cancel-first.nbs"));
        state.doc.replace_song(first);
        let first_id = state.active_tab().unwrap();
        let mut second = nbs_core::model::Song::new();
        second.filename = Some(PathBuf::from("cancel-second.nbs"));
        let second_id = state.push_tab(second);
        frame_out(&mut state, &ctx, base_input());
        frame_out(&mut state, &ctx, base_input());
        let start = |state: &mut EditorState, ctx: &egui::Context| {
            let mut raw = base_input();
            raw.events
                .push(egui::Event::PointerMoved(egui::pos2(50.0, 40.0)));
            raw.events.push(egui::Event::PointerButton {
                pos: egui::pos2(50.0, 40.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            frame_out(state, ctx, raw);
            let mut raw = base_input();
            raw.events
                .push(egui::Event::PointerMoved(egui::pos2(80.0, 40.0)));
            frame_out(state, ctx, raw);
            assert!(state.tab_drag.unwrap().dragging);
        };
        start(&mut state, &ctx);
        press_key(&mut state, &ctx, egui::Key::Escape);
        assert!(state.tab_drag.is_none());
        assert_eq!(state.store.order(), &[first_id, second_id]);

        let mut release = base_input();
        release
            .events
            .push(egui::Event::PointerMoved(egui::pos2(80.0, 40.0)));
        release.events.push(egui::Event::PointerButton {
            pos: egui::pos2(80.0, 40.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        frame_out(&mut state, &ctx, release);
        start(&mut state, &ctx);
        let mut lost = base_input();
        for viewport in lost.viewports.values_mut() {
            viewport.focused = Some(false);
        }
        lost.events
            .push(egui::Event::PointerMoved(egui::pos2(120.0, 40.0)));
        frame_out(&mut state, &ctx, lost);
        assert!(state.tab_drag.is_none());
        assert_eq!(state.store.order(), &[first_id, second_id]);
        assert_eq!(state.active_tab(), Some(first_id));
    }

    /// 标签条右键菜单四条动作真实可点；这里验收「关闭右侧歌曲」。
    #[test]
    fn song_tab_context_menu_closes_the_right_side() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let mut first = nbs_core::model::Song::new();
        first.filename = Some(PathBuf::from("menu-first.nbs"));
        state.doc.replace_song(first);
        let first_id = state.active_tab().unwrap();
        let mut second = nbs_core::model::Song::new();
        second.filename = Some(PathBuf::from("menu-second.nbs"));
        let second_id = state.push_tab(second);
        let mut third = nbs_core::model::Song::new();
        third.filename = Some(PathBuf::from("menu-third.nbs"));
        let third_id = state.push_tab(third);

        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        let at = text_center_if(&out.shapes, |t| t == "menu-second.nbs");
        secondary_click_point(&mut state, &ctx, at);
        frame(&mut state, &ctx);
        click_text(&mut state, &ctx, "关闭右侧歌曲");
        assert_eq!(state.store.order(), &[first_id, second_id]);
        assert!(!state.store.contains(third_id));
        assert_eq!(state.active_tab(), Some(second_id));
        assert!(state.tab_menu.is_none());
    }

    /// logo 是欢迎窗／关于窗里唯一一块 165×165 的贴图（`spr_logo` 半尺寸），帧号直接
    /// 写在网格 uv 的左边界上：3 帧横排 → 0 / ⅓ / ⅔。
    fn logo_u0(state: &mut EditorState, ctx: &egui::Context) -> f32 {
        let out = frame_out(state, ctx, base_input());
        let hits: Vec<f32> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Mesh(m) => Some((
                    m.calc_bounds(),
                    m.vertices.iter().map(|v| v.uv.x).fold(f32::MAX, f32::min),
                )),
                _ => None,
            })
            .filter(|(r, _)| (r.width() - 165.0).abs() < 0.6 && (r.height() - 165.0).abs() < 0.6)
            .map(|(_, u)| u)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "这一屏里应当只有一张 logo，实际 {} 张",
            hits.len()
        );
        hits[0]
    }

    /// 原版 `draw_window_greeting.gml:39-41`：logo 帧 = `window_icon + (window_icon && fdark)`，
    /// 首选项那个「旧图标」开关会连带换掉欢迎窗左上角那张图；`window_icon` 关着时
    /// `fdark` 不参与（原版括号里的 `&&`）。
    #[test]
    fn greeting_logo_frame_follows_window_icon_and_fdark() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.window = Some(WindowKind::Greeting);
        state.theme.apply(&ctx);
        frame(&mut state, &ctx);
        assert!(!state.prefs.window_icon, "出厂是不带旧图标的原版 logo");
        assert_eq!(logo_u0(&mut state, &ctx), 0.0);
        state.prefs.window_icon = true;
        assert!(
            (logo_u0(&mut state, &ctx) - 1.0 / 3.0).abs() < 1e-4,
            "开「旧图标」→ 第 1 帧"
        );
        state.theme = Theme::new(ThemeKind::Fluent);
        state.theme.fdark = true;
        state.theme.apply(&ctx);
        assert!(
            (logo_u0(&mut state, &ctx) - 2.0 / 3.0).abs() < 1e-4,
            "「旧图标」+ 暗色 Fluent → 第 2 帧"
        );
        state.prefs.window_icon = false;
        assert_eq!(
            logo_u0(&mut state, &ctx),
            0.0,
            "「旧图标」关着时 fdark 不算"
        );
    }

    /// 关于窗谢词栏那一列，返回 `（行顶 y，文本，是否粗体）` 再加程序名那行的行顶。
    /// 栏心是原版 `xx + w/2` = `x1 + 425`，而程序名在 `x1 + 150`，所以拿程序名的中心
    /// 往右 275 定基准，测试就不依赖窗口落在屏幕哪儿。粗体在端口是同一行叠两层
    /// （`greet_text_center` 的 0.5px 近似），同 y 出现两层即原版那句 `font_main_bold`。
    fn credit_column(
        state: &mut EditorState,
        ctx: &egui::Context,
    ) -> (Vec<(f32, String, bool)>, f32) {
        frame_out(state, ctx, base_input());
        let out = frame_out(state, ctx, base_input());
        // `Shape::Text` 的绝对位置在 `t.pos`，`galley.rect` 只是相对自身的大小。
        let text_rect =
            |t: &egui::epaint::TextShape| egui::Rect::from_min_size(t.pos, t.galley.rect.size());
        let title = out
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if t.galley.text() == "Note Block Studio" => {
                    Some(text_rect(t))
                }
                _ => None,
            })
            .expect("关于窗应当画出程序名");
        let cx = title.center().x + 275.0;
        let mut all: Vec<(f32, String)> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => {
                    let r = text_rect(t);
                    ((r.center().x - cx).abs() < 0.6)
                        .then(|| (r.min.y, t.galley.text().to_string()))
                }
                _ => None,
            })
            .collect();
        all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut rows: Vec<(f32, String, bool)> = Vec::new();
        for (y, text) in all {
            match rows.last_mut() {
                Some(last) if (last.0 - y).abs() < 0.6 => last.2 = true,
                _ => rows.push((y, text, false)),
            }
        }
        (rows, title.min.y)
    }

    /// 原版 `draw_window_about.gml:19-28` 的正文：程序名 + 版本 + 延续声明。
    /// 端口原先那份自加的功能清单（「A 数据层 / egui 0.31 …」）不属于原版。
    #[test]
    fn about_window_shows_the_original_text() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::About);
        let texts = drawn_texts(&mut state, &ctx);
        let head = format!(
            "Version {} - Released {}",
            crate::VERSION,
            crate::VERSION_DATE
        );
        for t in [
            "Note Block Studio",
            head.as_str(),
            "This program is a continuation of",
            // 源文这两行行首、行尾都带空格，逐字符照抄。
            " Minecraft Note Block Studio, ",
            "created by David Andrei. ",
            "or the game Minecraft.",
            "Website",
            "GitHub",
            "Donate",
            "OK",
        ] {
            assert!(texts.iter().any(|s| s == t), "关于窗缺「{t}」：{texts:?}");
        }
        for t in [
            "A Data layer",
            "egui 0.31 · glow (switchable to wgpu) · cpal · Rust 2021",
            "A 数据层",
        ] {
            assert!(!texts.iter().any(|s| s == t), "原版关于窗没有「{t}」");
        }
    }

    /// 谢词栏的网格：一屏 19 行（:60 `a < sb_val + 19`）、行距 16（:63）、
    /// 第一行行顶 `yy + 9` = `y1 + 44`、`*` 行走粗体（:64-67）。
    /// 滚一格窗口整体下移一行。
    #[test]
    fn about_credit_rows_follow_the_original_grid() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::About);
        let want = crate::windows::credits_lines(state.credits.as_ref().unwrap());
        let (rows, title_y) = credit_column(&mut state, &ctx);
        assert_eq!(rows.len(), 19, "原版一屏 19 行，实际 {} 行", rows.len());
        assert!(
            (rows[0].0 - title_y + 156.0).abs() < 0.6,
            "首行行顶在 y1+44，程序名在 y1+200（差 -156），实际 {:+.1}",
            rows[0].0 - title_y
        );
        for i in 1..rows.len() {
            assert!(
                (rows[i].0 - rows[i - 1].0 - 16.0).abs() < 0.6,
                "第 {i} 行行距是 {}",
                rows[i].0 - rows[i - 1].0
            );
        }
        for (i, (_, text, bold)) in rows.iter().enumerate() {
            assert_eq!(text, &want[i].0, "第 {i} 行不是 credits.txt 的第 {i} 行");
            assert_eq!(*bold, want[i].1, "第 {i} 行「{text}」的粗体档位");
        }
        state.credits_scroll = 1.0;
        let (rows, _) = credit_column(&mut state, &ctx);
        assert_eq!(rows[0].1, want[1].0, "滚一格，首行是第二行");
        assert_eq!(rows[18].1, want[19].0, "最后一行也跟着下移");
    }

    /// :44-58 的 `|` 行交给 `draw_text_url`（:70），:82-107 六个站点按钮加「确定」。
    #[test]
    fn about_credits_links_and_site_buttons_open_the_right_urls() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        let _cn = crate::LangGuard::set(1);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::About);
        state.credits = Some("*Section\nLink text|https://example.test/a\nPlain\n".to_string());
        click_text(&mut state, &ctx, "Link text");
        assert_eq!(
            commands::take_open_url().as_deref(),
            Some("https://example.test/a"),
            "竖线后面那半截才是 url，文字本身不带竖线"
        );
        for (label, url) in [
            ("官方网站", crate::LINK_WEBSITE),
            ("GitHub", crate::LINK_GITHUB),
            ("Discord", crate::LINK_DISCORD),
            ("Twitter", crate::LINK_TWITTER),
            ("YouTube", crate::LINK_YOUTUBE),
            ("捐赠", crate::LINK_DONATE),
        ] {
            click_text(&mut state, &ctx, label);
            assert_eq!(
                commands::take_open_url().as_deref(),
                Some(url),
                "「{label}」要开 {url}"
            );
        }
        click_text(&mut state, &ctx, "确定");
        assert_eq!(state.window, None, ":106-107 的「确定」是 windowclose = 1");
    }

    /// 读不到 `credits.txt` 时原版 `string_count` 当场报错，端口按「一栏都不画」处理，
    /// 窗口其余部分照常。
    #[test]
    fn about_without_credits_file_draws_an_empty_column() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::About);
        state.credits = None;
        let (rows, _) = credit_column(&mut state, &ctx);
        assert!(rows.is_empty(), "没有谢词就一栏都不画：{rows:?}");
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "Note Block Studio"),
            "左半和按钮照旧：{texts:?}"
        );
    }

    /// :10-11 的标题就是一个「About」/「关于」；:15-17 的 logo 帧号和欢迎窗共用
    /// 同一条 `window_icon + (window_icon && fdark)` 算式；中文那两行「版本」
    /// 「发布于」后面不带空格（英文的空格来自语言表）。
    #[test]
    fn about_title_and_logo_follow_the_original() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        let _cn = crate::LangGuard::set(1);
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::About);
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "关于"),
            "中文标题是「关于」：{texts:?}"
        );
        let head = format!("版本{} - 发布于{}", crate::VERSION, crate::VERSION_DATE);
        assert!(texts.iter().any(|t| t == &head), "中文版本行：{texts:?}");
        assert_eq!(logo_u0(&mut state, &ctx), 0.0, "不带旧图标是第 0 帧");
        state.prefs.window_icon = true;
        assert!(
            (logo_u0(&mut state, &ctx) - 1.0 / 3.0).abs() < 1e-4,
            "开「旧图标」→ 第 1 帧"
        );
    }

    /// 语言切到英文（原版 language=0 分支）后欢迎窗整套文案与行位切换：
    /// 第二行社区链接只剩 GitHub / Donate（中文还有 QQ 群 / 捐赠），各字符串
    /// 全部照 draw_window_greeting.gml 的 `if (language != 1)` 分支。
    #[test]
    fn greeting_renders_english_when_language_zero() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        assert_eq!(state.modals.items[0].buttons, state::ModalButtons::YesNo);
        assert!(!state.prefs.check_prerelease, "答之前不能先改值");
        click_text(&mut state, &ctx, "是");
        assert!(state.prefs.check_prerelease, "答「是」要真的打开");
    }

    /// #122：原版 `draw_windows.gml:31-38` —— Escape 先收展开的下拉，同一帧不再
    /// 关窗。端口原先是「下拉开合不管，Escape 一律关窗」。
    #[test]
    fn escape_in_preferences_closes_the_dropdown_before_the_window() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 1;
        frame(&mut state, &ctx);
        state.prefs_language_menu = true;
        frame(&mut state, &ctx);
        press_key(&mut state, &ctx, egui::Key::Escape);
        assert_eq!(
            state.window,
            Some(WindowKind::Preferences),
            "下拉开着时 Escape 不该把窗口一起关掉"
        );
        assert!(!state.prefs_language_menu, "Escape 要把下拉收掉");
        press_key(&mut state, &ctx, egui::Key::Escape);
        assert_eq!(state.window, None, "没有下拉时 Escape 照常关窗");
    }

    /// #122：数据打包窗的 ← / → 翻页签（`draw_window_datapack_export.gml:89-91`），
    /// 两页之间绕圈。原版另有五扇窗同款，端口早已实现，这里补上漏掉的这一扇。
    #[test]
    fn datapack_window_pages_tabs_with_arrow_keys() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::DatapackExport);
        frame(&mut state, &ctx);
        assert_eq!(state.datapack_tab, 0, "开窗固定在设置页");
        press_key(&mut state, &ctx, egui::Key::ArrowRight);
        assert_eq!(state.datapack_tab, 1, "→ 翻到效果页");
        press_key(&mut state, &ctx, egui::Key::ArrowRight);
        assert_eq!(state.datapack_tab, 0, "两页之间绕圈");
        press_key(&mut state, &ctx, egui::Key::ArrowLeft);
        assert_eq!(state.datapack_tab, 1, "← 反向绕圈");
    }

    /// #122：F1「快捷键一览」这张表此前没有任何用例碰过，先证明它真画得出来。
    /// 表格比窗口高，可见的只有开头几行；新补的 PageUp / F7 两行落在滚动区下面，
    /// 由 `shortcuts::tests::help_table_lists_the_newly_ported_keys` 在数据面钉住。
    #[test]
    fn shortcuts_window_renders_the_help_table() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        dispatch(
            &mut state,
            commands::Command::ShowWindow(WindowKind::Shortcuts),
            &ctx,
        );
        let t = drawn_texts(&mut state, &ctx);
        for want in [
            "左键单击",
            "放置 / 覆盖音符",
            "整窗口缩放（原版档位 0.5..4）",
        ] {
            assert!(
                t.iter().any(|x| x == want),
                "F1 窗没把表格画出来，缺「{want}」"
            );
        }
    }

    /// 原版 draw_window_preferences.gml:412-421：界面页的语言下拉。
    /// 标题跟着语言切换，框里的值始终是两种语言的专名；菜单里选中文要写回
    /// prefs.language 并收起菜单，下一帧 app::show 把全局开关同步成中文。
    #[test]
    fn preferences_language_row_switches_interface_language() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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

    /// 标尺右键菜单只有原版那六条（`control_draw.gml:156-168` + `menu_click.gml:235-251`）：
    /// 没有区间时前四条整条置灰（`inactive(...)` 前缀），后两条是 `check(marker_start)` /
    /// `check(marker_end)` 的前缀勾，和首选项「播放」页共用同一对全局量。
    #[test]
    fn timeline_context_menu_follows_the_section_and_the_two_checks() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        // 两行勾的判据是这两个开关，出厂是「开 / 关」；用户在首选项里翻过就得跟着翻，
        // 断言前先钉回出厂值。
        state.prefs.marker_start = true;
        state.prefs.marker_end = false;
        // 「跳到区间开始/结束」的置灰判据是 `section_start > enda`，空谱 enda = 0
        // 会把它们永久钉灰，所以靶子谱要有第 12 列；选区那条要数得出音符，故 3..8 内也放两颗。
        let mut song = nbs_core::model::Song::new();
        song.set(4, 0, nbs_core::model::Note::new(0, 48));
        song.set(7, 0, nbs_core::model::Note::new(0, 50));
        song.set(12, 0, nbs_core::model::Note::new(0, 52));
        state.doc.replace_song(song);
        state.section_exists = true;
        state.section_start = 3;
        state.section_end = 8;
        let open_menu = |state: &mut EditorState| {
            state.context_menu = Some(crate::state::ContextMenu {
                pos: egui::pos2(300.0, 300.0),
                cell: (4, 0),
                kind: crate::state::ContextKind::Timeline,
            });
        };

        open_menu(&mut state);
        let texts = drawn_texts(&mut state, &ctx);
        for t in ["移除区间", "跳到区间开始", "跳到区间结束", "选中区间内方块"]
        {
            assert!(
                texts.iter().any(|s| s == t),
                "标尺菜单缺「{t}」：{}",
                texts.join(" | ")
            );
        }
        // 后两条带 `check()` 前缀，勾在不在取决于 prefs，这里只认标签本体。
        for t in ["从区间开始播放", "区间后结束播放"] {
            assert!(
                texts.iter().any(|s| s.ends_with(t)),
                "标尺菜单缺「{t}」：{}",
                texts.join(" | ")
            );
        }
        assert!(
            !texts.iter().any(|s| s.starts_with("全选")),
            "标尺菜单不该串进网格那套：{}",
            texts.join(" | ")
        );
        let marked: Vec<String> = texts
            .iter()
            .filter(|s| s.starts_with('✓'))
            .cloned()
            .collect();
        assert_eq!(
            marked,
            vec!["✓ 从区间开始播放".to_string()],
            "出厂 marker_start 开、marker_end 关，勾只能落在第一条上"
        );

        // 点带勾的那条：翻 `marker_start`、勾下一帧消失，并且落盘（原版两条都写 settings.ini）。
        click_text(&mut state, &ctx, "✓ 从区间开始播放");
        assert!(!state.prefs.marker_start);
        frame_out(&mut state, &ctx, base_input());
        assert!(
            !nbs_io::prefs::Preferences::load().marker_start,
            "菜单这条和首选项是同一个全局量，要一起落盘"
        );

        // 没有区间时前四条整条点不动：点了既不收菜单也不改状态。
        state.section_exists = false;
        open_menu(&mut state);
        click_text(&mut state, &ctx, "移除区间");
        assert!(state.context_menu.is_some(), "置灰条目点不动，菜单不该收");
        assert_eq!(state.section_start, 3, "点不动就不该有任何副作用");

        // 解禁后四条各有去处（`menu_click.gml:235-244`）。
        state.section_exists = true;
        open_menu(&mut state);
        click_text(&mut state, &ctx, "跳到区间开始");
        assert_eq!(state.viewport.start_tick, 3, "starta = section_start");
        assert!(state.context_menu.is_none(), "点中要收菜单");

        open_menu(&mut state);
        click_text(&mut state, &ctx, "跳到区间结束");
        assert_eq!(state.viewport.start_tick, 7, "starta = section_end - 1");

        open_menu(&mut state);
        click_text(&mut state, &ctx, "选中区间内方块");
        assert!(
            state.doc.selection.contains(4, 0) && state.doc.selection.contains(7, 0),
            "区间两端内的方块都要进选区"
        );
        assert!(
            !state.doc.selection.contains(12, 0),
            "原版取的是 section_end - 1，右端那一列之后的方块不该进选区"
        );

        open_menu(&mut state);
        click_text(&mut state, &ctx, "区间后结束播放");
        assert!(state.prefs.marker_end, "点未勾的那条要打上勾");
        open_menu(&mut state);
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|s| s == "✓ 区间后结束播放"),
            "勾要跟着 prefs 画出来：{}",
            texts.join(" | ")
        );

        open_menu(&mut state);
        click_text(&mut state, &ctx, "移除区间");
        assert!(!state.section_exists, "有区间时「移除区间」要真的删掉");
    }

    /// 首选项「播放」页的两格（原版 `draw_window_preferences.gml:600-601`，
    /// 中文 `:652-653`，都在 `y1 + 170 / 190` 的「进度条」框里）：标签用的是
    /// 「在区间时开始播放 / 在区间后停止播放」，和标尺菜单那两条措辞不同但同值。
    #[test]
    fn preferences_playback_rows_toggle_the_section_options_and_persist() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 4;
        // 两条断言都是「点一下翻到出厂值的另一面」，先把出厂值钉住，
        // 否则用户存过 开/关 就只剩一次点击的机会。
        state.prefs.marker_start = true;
        state.prefs.marker_end = false;
        frame(&mut state, &ctx);
        let texts = drawn_texts(&mut state, &ctx);
        for t in ["在区间时开始播放", "在区间后停止播放"] {
            assert!(
                texts.iter().any(|s| s == t),
                "播放页缺少「{t}」：{}",
                texts.join(" | ")
            );
        }
        click_text(&mut state, &ctx, "在区间时开始播放");
        assert!(!state.prefs.marker_start, "出厂开着，点标签要关掉");
        click_text(&mut state, &ctx, "在区间后停止播放");
        assert!(state.prefs.marker_end, "出厂关着，点标签要打开");
        frame_out(&mut state, &ctx, base_input());
        let back = nbs_io::prefs::Preferences::load();
        assert!(!back.marker_start, "两格都要落盘（原版写 settings.ini）");
        assert!(back.marker_end);
    }

    /// 跟随播放头这一组：复选框 + 三档单选（原版 `draw_window_preferences.gml:596-599`，
    /// 中文 `:648-651`）。出厂 `marker_follow = 1` / `marker_pagebypage = 1`
    /// （`control_create.gml:379-380`）即「开着、按页」；关掉跟随后三格整体锁定，点不动。
    /// 档位和开关都得真的落盘，否则重启回到按页等于白设。
    #[test]
    fn preferences_follow_rows_pick_the_mode_and_lock_when_off() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.marker_follow = true;
        state.follow_mode = 1;
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 4;
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "Follow marker when playing",
            "Page by page",
            "Tick by tick",
            "Frame by frame",
        ] {
            assert!(
                texts.iter().any(|s| s == t),
                "播放页缺少「{t}」：{}",
                texts.join(" | ")
            );
        }
        click_text(&mut state, &ctx, "Tick by tick");
        assert_eq!(state.follow_mode, 0, "「按刻」没接上 marker_pagebypage = 0");
        click_text(&mut state, &ctx, "Frame by frame");
        assert_eq!(state.follow_mode, 2, "「按帧」没接上 marker_pagebypage = 2");
        click_text(&mut state, &ctx, "Follow marker when playing");
        assert!(!state.marker_follow, "复选框没接 ToggleFollow");
        click_text(&mut state, &ctx, "Page by page");
        assert_eq!(state.follow_mode, 2, "跟随关掉时三格是锁定态，不该被点动");
        let back = nbs_io::prefs::Preferences::load();
        assert_eq!(back.follow_mode, 2, "跟随档位要落盘");
        assert!(!back.follow_marker, "跟随开关要落盘");
    }

    /// 原版页 3「访问」的两组：`draw_areaheader(22, 74, 456, 140, "鼠标滚轮")` 里的
    /// 三个档位 + 「滚动速度」拖拽行（`y1 + 150`，出厂 1）+ 「悬停层预览」
    /// （`y1 + 198`），和 `draw_areaheader(22, 240, 456, 105, "琴键与层")` 组名本身。
    /// 组名此前被写成「琴键」、滚动速度与悬停层预览两行缺失，这条用例把它们钉回原版。
    #[test]
    fn preferences_usability_page_shows_wheel_and_layer_groups() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
        assert!(state.prefs.layerhov_vppreview, "点标签要真的打开悬停层预览");
        state.save_prefs();
        let back = nbs_io::prefs::Preferences::load();
        assert!(back.layerhov_vppreview, "悬停层预览开关要落盘");
    }

    /// 用户要求 #119：首选项「帧率限制」那一组里要有「垂直同步」这颗勾，
    /// 并且点下去真的生效（改设置 + 落盘 + 把切换请求交给宿主），不是画个样子。
    /// 用户要求 #120a：顶部「编辑」菜单里那六条和右键菜单是同一份标签表
    /// （原版 `control_draw.gml:1741-1746`），也要跟着编辑模式变、真的改选区。
    #[test]
    fn the_edit_menu_mode_actions_follow_the_edit_mode() {
        let ctx = egui::Context::default();
        let mut state = state_with_selection();
        state.theme.apply(&ctx);
        // 音高模式：四条移调，没有 5、6 号。
        state.tool = crate::state::EditMode::Key;
        click_text(&mut state, &ctx, "编辑");
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "升八度  Ctrl+E",
            "降八度  Ctrl+D",
            "升半音  Ctrl+R",
            "降半音  Ctrl+F",
        ] {
            assert!(texts.iter().any(|s| s == t), "编辑菜单缺「{t}」");
        }
        assert!(
            !texts
                .iter()
                .any(|s| s.contains("设定音量") || s.contains("重置音量")),
            "音高模式不该有第 5、6 条"
        );
        click_text(&mut state, &ctx, "升八度  Ctrl+E");
        for dx in 0..3u32 {
            assert_eq!(
                state.doc.selection.at(dx, 0).unwrap().key,
                [62u8, 64, 67][dx as usize],
                "选区里每条都要升一个八度（不是只动画笔）"
            );
        }
        // 声道模式：六条换成声道的叫法，点第 1 条真的把选区声像 +10。
        state.doc.place_selection();
        state.doc.select_all();
        state.tool = crate::state::EditMode::Panning;
        click_text(&mut state, &ctx, "编辑");
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "向右声道移动 10  Ctrl+E",
            "设定音符声道……  Ctrl+T",
            "重置立体声  Ctrl+G",
        ] {
            assert!(texts.iter().any(|s| s == t), "声道模式的编辑菜单缺「{t}」");
        }
        assert!(
            !texts.iter().any(|s| s.contains("升八度")),
            "声道模式不该还挂着升八度"
        );
        click_text(&mut state, &ctx, "向右声道移动 10  Ctrl+E");
        assert_eq!(
            state.doc.selection.at(0, 0).unwrap().panning,
            110,
            "第 1 条 = 选区声像 +10"
        );
    }

    /// 用户要求 #120a：音符右键菜单里那几条要跟着编辑模式变（原版
    /// `get_mode_actions.gml` 出标签，`control_draw.gml:550-555` 在音高模式下整条
    /// 不画第 5、6 项），音量模式下不该还是「升八度 / 降八度」。
    #[test]
    fn the_grid_context_menu_items_follow_the_edit_mode() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = None;
        let mut song = nbs_core::model::Song::new();
        let mut n = nbs_core::model::Note::new(0, 48);
        n.velocity = 50;
        song.set(4, 0, n);
        state.doc.replace_song(song);
        state.doc.select_rect(4, 0, 4, 0, false, false, false);
        let open_menu = |state: &mut EditorState| {
            state.context_menu = Some(crate::state::ContextMenu {
                pos: egui::pos2(300.0, 300.0),
                cell: (4, 0),
                kind: crate::state::ContextKind::Grid,
            });
        };
        let has = |texts: &[String], t: &str| texts.iter().any(|s| s == t);

        // 音高模式：四条移调，没有第 5、6 条。
        state.tool = crate::state::EditMode::Key;
        open_menu(&mut state);
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "升八度  Ctrl+E",
            "降八度  Ctrl+D",
            "升半音  Ctrl+R",
            "降半音  Ctrl+F",
        ] {
            assert!(
                has(&texts, t),
                "音高模式菜单缺「{t}」：{}",
                texts.join(" | ")
            );
        }
        assert!(
            !texts
                .iter()
                .any(|s| s.contains("设定音量") || s.contains("重置音量")),
            "音高模式不该有第 5、6 条：{}",
            texts.join(" | ")
        );

        // 力度模式：原版那六条，且不该再挂移调。
        state.tool = crate::state::EditMode::Velocity;
        open_menu(&mut state);
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "增加 10 音量  Ctrl+E",
            "减小 10 音量  Ctrl+D",
            "增加音量  Ctrl+R",
            "减小音量  Ctrl+F",
            "设定音量……  Ctrl+T",
            "重置音量  Ctrl+G",
        ] {
            assert!(
                has(&texts, t),
                "音量模式菜单缺「{t}」：{}",
                texts.join(" | ")
            );
        }
        assert!(
            !texts.iter().any(|s| s.contains("升八度")),
            "音量模式不该还是升八度：{}",
            texts.join(" | ")
        );

        // 点「增加 10 音量」：动的是选区里那条音符的力度，画笔值不碰，菜单收起。
        let brush = state.current.velocity;
        click_text(&mut state, &ctx, "增加 10 音量  Ctrl+E");
        assert_eq!(
            state.doc.selection.at(0, 0).unwrap().velocity,
            60,
            "音量模式第 1 条要把选区音量 +10"
        );
        assert_eq!(state.current.velocity, brush, "原版 mode_action 不动画笔值");
        assert!(state.context_menu.is_none(), "点中条目要收菜单");

        // 声道 / 微调模式各用各的标签。
        state.tool = crate::state::EditMode::Panning;
        open_menu(&mut state);
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "向右声道移动 10  Ctrl+E",
            "向左声道移动 10  Ctrl+D",
            "向右声道移动  Ctrl+R",
            "向左声道移动  Ctrl+F",
            "设定音符声道……  Ctrl+T",
            "重置立体声  Ctrl+G",
        ] {
            assert!(
                has(&texts, t),
                "声道模式菜单缺「{t}」：{}",
                texts.join(" | ")
            );
        }
        state.tool = crate::state::EditMode::Pitch;
        open_menu(&mut state);
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "上调 10 微分  Ctrl+E",
            "下调 10 微分  Ctrl+D",
            "上调 1 微分  Ctrl+R",
            "下调 1 微分  Ctrl+F",
            "设定音高……  Ctrl+T",
            "重置音高  Ctrl+G",
        ] {
            assert!(
                has(&texts, t),
                "微调模式菜单缺「{t}」：{}",
                texts.join(" | ")
            );
        }

        // 没有选区时六条整条点不动（原版 `inactive(current_song.selected = 0)`）。
        state.tool = crate::state::EditMode::Velocity;
        state.doc.deselect();
        open_menu(&mut state);
        click_text(&mut state, &ctx, "增加 10 音量  Ctrl+E");
        assert!(state.context_menu.is_some(), "置灰条目点不动，菜单不该收");
    }

    #[test]
    fn the_preferences_page_has_a_working_vsync_toggle() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.window = Some(WindowKind::Preferences);
        state.prefs_tab = 1; // 「界面」页才有帧率限制那一组
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
        assert!(
            texts.iter().any(|t| t == "垂直同步"),
            "帧率限制那一组里应有「垂直同步」，当前文本：{}",
            texts.join(" | ")
        );
        assert!(state.prefs.vsync, "出厂垂直同步开着");
        click_text(&mut state, &ctx, "垂直同步");
        assert!(!state.prefs.vsync, "点标签要真的关掉垂直同步");
        assert_eq!(
            state.vsync_request,
            Some(false),
            "切换要求要交给宿主（nbs-app 才碰得到呈现层）"
        );
        assert_eq!(
            nbs_io::prefs::Preferences::load().vsync,
            false,
            "关掉要落盘"
        );
        click_text(&mut state, &ctx, "垂直同步");
        assert!(state.prefs.vsync, "再点一次要开回来");
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
        let mut state = EditorState::for_test();
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

    /// 用户要求 #118：就地速度框填好数字后按回车当场生效，不必再点别处。
    /// 原版 Enter 是留给文本框自己的（`draw_window_set_tempo.gml:29` 那行注释
    /// "Enter stays with the text field"），端口在这里刻意偏离。
    #[test]
    fn the_inline_tempo_box_commits_on_enter() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.use_bpm = false;
        state.window = None;
        state.doc.replace_song(nbs_core::model::Song::new());
        state.doc.song.meta.tempo = 10.0;
        let tap = |state: &mut EditorState, p: egui::Pos2| {
            for pressed in [true, false] {
                let mut raw = base_input();
                raw.events.push(egui::Event::PointerMoved(p));
                raw.events.push(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
                frame_out(state, &ctx, raw);
            }
        };
        let key = |state: &mut EditorState, key: egui::Key| {
            let mut raw = base_input();
            raw.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            frame_out(state, &ctx, raw);
        };

        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        let on_box = text_center_if(&out.shapes, |t| t == "10.00 t / s");
        tap(&mut state, on_box);
        assert_eq!(state.tempo_input.as_deref(), Some("10.00"), "框要开着");
        // 输入框在开框后的下一帧才抢到焦点，先让它拿到。
        frame_out(&mut state, &ctx, base_input());
        state.tempo_input = Some("12.5".into());
        frame_out(&mut state, &ctx, base_input());
        key(&mut state, egui::Key::Enter);
        assert_eq!(
            state.doc.song.meta.tempo, 12.5,
            "回车要当场把速度设进去（用户要求 #118）"
        );
        assert_eq!(state.tempo_input, None, "回车后输入框要关掉");
    }

    /// 原版 `w_tempotapper`（速度测量器）：按 T 打点 → 界面按速度单位显示测到的值，
    /// 「双倍速度」可勾，确认把 `ctempo * 2 / 15` 写进歌曲速度并关窗。
    #[test]
    fn tempo_tapper_measures_taps_and_applies_the_tempo() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
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
            state.doc.song.meta.tempo, 16.0,
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        assert_eq!(
            state.save_options_version, 5,
            "低于 min_version 的单选要抬上来"
        );
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
        let mut state = EditorState::for_test();
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

    /// #125：「仅限资源包」那扇窗的「确定」原先压在窗底外面。原版窗口是
    /// `draw_window(x1, y1, x1 + 490, y1 + 450)`（`draw_window_minecraft.gml:10`），按钮
    /// 在 `(x1 + 240 - 36, y1 + 413)` 高 21（`:495`）——按钮下沿离窗底 16 像素。端口先前把
    /// 内容高度写成 `450 - 38`（比 450 - 30/26 少 8/12 像素），又没按首选项那套把 Fluent
    /// 的 22 减回去：Fluent（出厂主题）下按钮整颗往下挪 26 像素，下沿超出窗底 7 像素被裁掉。
    /// 这里四套主题各量一次：按钮下沿离窗底都是 23 = 原版那 16 + egui 自己的 7 像素边框余量。
    #[test]
    fn compat_window_ok_button_stays_inside_the_frame() {
        for kind in [
            crate::theme::ThemeKind::Aqua,
            crate::theme::ThemeKind::Retro,
            crate::theme::ThemeKind::Dark,
            crate::theme::ThemeKind::Fluent,
        ] {
            let ctx = egui::Context::default();
            let mut state = state_with_selection();
            state.theme = crate::theme::Theme::new(kind);
            state.theme.apply(&ctx);
            state.window = Some(WindowKind::MinecraftCompat);
            frame_out(&mut state, &ctx, base_input());
            let out = frame_out(&mut state, &ctx, base_input());
            let win = ctx
                .memory(|m| m.area_rect(egui::Id::new("nbs_win_MinecraftCompat")))
                .expect("窗口矩形已记账");
            // 「确定」这颗窗里只有一颗：先按文字定位，再取套住它的那颗按钮矩形。
            let label = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) if t.galley.text() == "确定" => Some(t.pos),
                    _ => None,
                })
                .next()
                .expect("窗口里画了「确定」");
            let ok = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Rect(r)
                        if (r.rect.width() - 71.0).abs() < 4.0
                            && r.rect.height() < 30.0
                            && r.rect.contains(label) =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .next()
                .unwrap_or_else(|| panic!("{kind:?}：没找到套住「确定」的按钮矩形"));
            assert!(
                (win.max.y - ok.max.y - 23.0).abs() < 0.6,
                    "{kind:?}：按钮下沿离窗底应是 23（原版 16 + 边框 7），实得 {:.1}，按钮 {ok:?}、窗 {win:?}",
                    win.max.y - ok.max.y
            );
        }
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
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
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.theme.accent_base = [200, 20, 60];
        state.theme.accent_init();
        open_accent_picker(&mut state, &ctx);

        click_text(&mut state, &ctx, "使用默认值");
        assert_eq!(state.modals.items.len(), 1, "原版这里先弹一句确认");
        assert_eq!(state.modals.items[0].buttons, state::ModalButtons::YesNo);
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

    /// 属性窗的 Information 区（原版 `draw_window_properties.gml:29-66`）：歌曲名 / 作者 /
    /// 原作者 / 简介四个框。原版是直接写 `cursong.*` 再标一句 `changed = 1`（:42/49/56），
    /// 不进撤销栈，所以端口也只标脏、不入历史——一次敲字压一条历史反而离原版更远。
    #[test]
    fn properties_window_edits_the_song_meta() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.doc.song.meta.name.clear();
        state.doc.song.meta.author.clear();
        state.doc.song.meta.original_author.clear();
        state.doc.song.meta.description.clear();
        state.doc.song.dirty = false;
        state.window = Some(WindowKind::Properties);
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "Song name",
            "Song author",
            "Original song author",
            "Description",
        ] {
            assert!(texts.iter().any(|s| s == t), "属性窗缺「{t}」：{texts:?}");
        }
        // 空框画的是占位文案，它的中心就落在输入框里，点一下即聚焦。
        frame_out(&mut state, &ctx, base_input());
        let out = frame_out(&mut state, &ctx, base_input());
        let at = text_center_if(&out.shapes, |t| t == "Untitled song");
        click_pos_at(&mut state, &ctx, at);
        for chunk in ["O", "de"] {
            let mut raw = base_input();
            raw.events.push(egui::Event::Text(chunk.into()));
            frame_out(&mut state, &ctx, raw);
        }
        assert_eq!(state.doc.song.meta.name, "Ode", "敲进去的字没落到歌曲名上");
        assert!(state.doc.song.dirty, "改元数据要标脏");
        assert!(
            !state.doc.history.can_undo(),
            "原版敲字不进撤销栈（逐字符压历史会把 Ctrl+Z 变成删字）"
        );
    }

    /// 歌曲信息窗在原版是**只读**的（`draw_window_songinfo.gml:62-136`）：字段为空整行不画，
    /// 底部只有「修改」跳属性窗、「确定」关窗两个按钮。端口原来在这儿放了四个输入框，
    /// 和原版正好反了。
    #[test]
    fn song_info_window_is_a_readonly_card() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);
        state.doc.song.meta.name = "Ring".into();
        state.doc.song.meta.author = "Notch".into();
        state.doc.song.meta.original_author.clear();
        state.doc.song.meta.description.clear();
        state.doc.song.meta.midi_file = "ring.mid".into();
        state.window = Some(WindowKind::SongInfoEdit);
        let texts = drawn_texts(&mut state, &ctx);
        for t in [
            "Ring",
            "Created by Notch",
            "Imported from ring.mid",
            "This message is shown when the file is opened.",
            "Edit",
            "OK",
        ] {
            assert!(texts.iter().any(|s| s == t), "信息窗缺「{t}」：{texts:?}");
        }
        for t in ["Originally created by ", "Song name", "Untitled song"] {
            assert!(
                !texts.iter().any(|s| s.starts_with(t)),
                "空字段不该画出来，编辑态标签也不该出现在只读窗里：{t}"
            );
        }
        let mut raw = base_input();
        raw.events.push(egui::Event::Text("Z".into()));
        frame_out(&mut state, &ctx, raw);
        assert_eq!(state.doc.song.meta.name, "Ring", "只读窗不该吃键盘输入");
        click_text(&mut state, &ctx, "OK");
        frame(&mut state, &ctx);
        assert_eq!(state.window, None, "「确定」关窗");

        state.window = Some(WindowKind::SongInfoEdit);
        click_text(&mut state, &ctx, "Edit");
        frame(&mut state, &ctx);
        assert_eq!(
            state.window,
            Some(WindowKind::Properties),
            "「修改」应换到属性窗（原版 :130-133 就是直接切窗）"
        );
    }

    /// 用户要求 #130：打开歌曲后弹的那张信息卡要能用 ESC 关掉。原版没有这条
    /// （`draw_window_songinfo.gml:139` 只读版认全屏左键、:133/136 带按钮版认「确定」），
    /// 两版同一扇窗、同一个绘制函数，所以各钉一次。
    #[test]
    fn esc_closes_both_song_info_variants() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        for kind in [WindowKind::SongInfo, WindowKind::SongInfoEdit] {
            let mut state = EditorState::for_test();
            state.prefs.language = 1;
            state.theme.apply(&ctx);
            state.doc.song.meta.name = "Ring".into();
            state.window = Some(kind.clone());
            frame(&mut state, &ctx);
            assert_eq!(state.window, Some(kind.clone()), "前提：{kind:?} 开着");
            press_key(&mut state, &ctx, egui::Key::Escape);
            assert_eq!(state.window, None, "ESC 该关掉 {kind:?}");
        }
    }

    /// 同一次 ESC 只收下拉、不关窗（原版 `draw_windows.gml:31-38` 的两步规则）。
    /// 顶栏那一路必须靠 #129 的 `menubar_open`：egui 在 `menu.rs:366` 就把这次 Escape
    /// 用掉了，等画到信息窗时 `any_popup_open()` 已经是假，只看它会让一按既收下拉又关窗。
    /// 用带按钮那一版：只读版「点任意处即关」（:139），点开下拉的那一下本身就会把它带走，
    /// 两扇在真实操作里不可能同时挂着（下面 first_click_dismisses 那条就是这个）。
    #[test]
    fn esc_under_an_open_dropdown_only_closes_the_dropdown() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        state.theme.apply(&ctx);
        state.doc.song.meta.name = "Ring".into();
        state.window = Some(WindowKind::SongInfoEdit);
        frame(&mut state, &ctx);
        open_menubar(&mut state, &ctx, "文件");
        assert_eq!(
            state.window,
            Some(WindowKind::SongInfoEdit),
            "前提：带按钮那一版不会被这一下点没"
        );

        press_key(&mut state, &ctx, egui::Key::Escape);
        assert_eq!(
            state.window,
            Some(WindowKind::SongInfoEdit),
            "收下拉这一下不该顺手关窗口"
        );
        // 闸门口径是「本帧开过」（#129），收下拉那一帧仍为真，收干净要再看一帧。
        frame(&mut state, &ctx);
        assert!(!state.menubar_open, "ESC 该把顶栏下拉收掉");

        press_key(&mut state, &ctx, egui::Key::Escape);
        assert_eq!(state.window, None, "第二次 ESC 才是关窗");
    }

    /// 真实键盘的「按下」和「抬起」多半落在不同帧（eframe 每个原始事件催一帧），
    /// 所以这条把两个事件拆到两帧喂。闸门若认「抬起」：抬起那帧下拉已经收干净、
    /// `menubar_open` 也翻回假，同一次 ESC 就会既收菜单又关窗，违反原版
    /// `draw_windows.gml:31-38` 的两步规则。原版窗口级 ESC 本身就取按下
    /// （`draw_windows.gml:28` 的 `keyboard_check_pressed(vk_escape)`）。
    #[test]
    fn esc_split_across_frames_still_only_closes_the_dropdown() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        state.theme.apply(&ctx);
        state.doc.song.meta.name = "Ring".into();
        state.window = Some(WindowKind::SongInfoEdit);
        frame(&mut state, &ctx);
        open_menubar(&mut state, &ctx, "文件");
        assert_eq!(
            state.window,
            Some(WindowKind::SongInfoEdit),
            "前提：窗还开着"
        );

        let mk = |pressed: bool| egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        let mut press = base_input();
        press.events.push(mk(true));
        frame_out(&mut state, &ctx, press);
        assert_eq!(
            state.window,
            Some(WindowKind::SongInfoEdit),
            "按下这一下只该收下拉"
        );

        let mut lift = base_input();
        lift.events.push(mk(false));
        frame_out(&mut state, &ctx, lift);
        assert_eq!(
            state.window,
            Some(WindowKind::SongInfoEdit),
            "抬起那帧不该补一刀关窗"
        );
        frame(&mut state, &ctx);
        assert!(!state.menubar_open, "下拉该已收掉");

        let mut esc = base_input();
        esc.events.push(mk(true));
        esc.events.push(mk(false));
        frame_out(&mut state, &ctx, esc);
        assert_eq!(state.window, None, "第二次 ESC 才是关窗");
    }

    /// 原版 `load_song.gml:64`：`if (window != w_instruments && newsong.song_name != "") window = w_songinfo`。
    /// 拆成三条闸门——歌名非空才弹、音色窗开着不抢、整段又在 `if (!backup)` 里；
    /// 弹的是没按钮、按左键即关的那一版（`:110/138-143`），不是工具栏开的带按钮版。
    #[test]
    fn opening_a_named_song_pops_the_plain_info_card() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let dir = scratch("auto-pop");
        let named = dir.join("ring.nbs");
        let blank = dir.join("blank.nbs");
        let mut song = nbs_core::Song::new();
        song.set(0, 0, nbs_core::Note::new(0, 40));
        song.meta.name = "Ring".into();
        nbs_core::nbs::save(&named, &song, nbs_core::nbs::NBS_VERSION).unwrap();
        song.meta.name.clear();
        nbs_core::nbs::save(&blank, &song, nbs_core::nbs::NBS_VERSION).unwrap();

        let mut state = EditorState::for_test();
        state.prefs.language = 0;
        let _en = crate::LangGuard::set(0);
        state.theme.apply(&ctx);

        dispatch(&mut state, commands::Command::OpenPath(named.clone()), &ctx);
        assert_eq!(
            state.window,
            Some(WindowKind::SongInfo),
            "原版开一首有名字的歌就弹这一版"
        );
        let texts = drawn_texts(&mut state, &ctx);
        assert!(texts.iter().any(|t| t == "Ring"), "卡片要念得出曲名");
        assert!(
            texts.iter().any(|t| t == "Click anywhere to dismiss"),
            "原版这一版的脚注：{texts:?}"
        );
        // 「Edit」不算：菜单栏本来就有一格叫 Edit，负向断言只能挑窗口独有的文案。
        for t in ["OK", "This message is shown when the file is opened."] {
            assert!(
                !texts.iter().any(|s| s == t),
                "自动弹的这一版没有按钮、也不是另一句脚注：{t}"
            );
        }

        // 原版 :139 的左键判定是全屏的，落在卡片外照样关；同一帧网格仍被窗口挡着。
        let notes = state.doc.song.total_blocks;
        click_pos_at(&mut state, &ctx, egui::pos2(400.0, 400.0));
        assert_eq!(state.window, None, "按左键即关");
        assert_eq!(
            state.doc.song.total_blocks, notes,
            "关窗那一下不该顺手落音符"
        );

        dispatch(&mut state, commands::Command::OpenPath(blank.clone()), &ctx);
        assert_eq!(state.window, None, "原版 song_name 为空时不弹");

        dispatch(
            &mut state,
            commands::Command::ShowWindow(WindowKind::Instruments),
            &ctx,
        );
        dispatch(&mut state, commands::Command::OpenPath(named.clone()), &ctx);
        assert_ne!(
            state.window,
            Some(WindowKind::SongInfo),
            "原版 `window != w_instruments` 是弹出的前提"
        );

        let before = state.recent_files.len();
        dispatch(&mut state, commands::Command::RestoreAutosave(named), &ctx);
        assert_eq!(state.window, None, "备份档在原版 `if (!backup)` 外，不弹");
        assert_eq!(state.recent_files.len(), before, "原版备份档不写最近列表");
        assert!(state.doc.song.dirty, "原版 :69 备份档标 changed = 1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「谁连着」的两个常驻出口都从真绘制结果里读：菜单那一格（面板关着也看得见）
    /// 和面板里的那几行。断言的是屏幕上画出来的字，不是拼装函数的返回值。
    #[test]
    fn menu_and_panel_both_show_who_is_connected() {
        let _cn = crate::LangGuard::set(1);
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        state.theme.apply(&ctx);
        // 链路挂上但服务不起：这一栏的账本由用例自己记，免得测试去抢真端口。
        let board = crate::control::attach(&mut state, &ctx).clients.clone();
        state.window = None;

        // 没在监听：菜单那一格不带尾巴（这时报端口会骗人）。
        click_text(&mut state, &ctx, "设置");
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "外部控制（MCP）…"),
            "设置菜单里没找到外部控制那一格：{texts:?}"
        );

        state.control_info = Some(crate::control::ControlInfo {
            port: 13537,
            discovery_path: Some("C:/cfg/control.json".to_string()),
            source: "设置页这一栏".to_string(),
            allow_danger: false,
            cli_dirs: Vec::new(),
        });
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "外部控制（MCP）… · 监听 :13537"),
            "监听中却没报出端口：{texts:?}"
        );

        let epoch = board.open_epoch();
        let busy = board
            .attach(epoch, "probe-ide", "127.0.0.1:40001")
            .expect("刚领的任期");
        busy.requested();
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts
                .iter()
                .any(|t| t == "外部控制（MCP）… · 监听 :13537 · 1 个客户端在连"),
            "有人在连着，菜单那一格必须说出来：{texts:?}"
        );

        // 面板：握手名、对端、调用次数都画出来，一条连接一行。
        state.window = Some(WindowKind::ExternalControl);
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts.iter().any(|t| t == "当前 1 个 AI 客户端连着："),
            "面板没报连接数：{texts:?}"
        );
        let row = texts
            .iter()
            .find(|t| t.contains("probe-ide") && t.contains("127.0.0.1:40001"))
            .unwrap_or_else(|| panic!("面板没画出这条连接：{texts:?}"));
        assert!(row.contains("1 次调用"), "调用次数没进那一行：{row}");

        // 只连上没调用的（IDE 把桥接起了放着）也要露面，不能显示成好像没人来。
        let idle = board
            .attach(epoch, "idle-ide", "127.0.0.1:40002")
            .expect("同一任期还能登记第二条");
        let texts = drawn_texts(&mut state, &ctx);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("idle-ide") && t.contains("还没调用")),
            "连上但没调用的客户端也要露面：{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t == "外部控制（MCP）… · 监听 :13537 · 2 个客户端在连"),
            "两条连接该报两条：{texts:?}"
        );

        drop(idle);
        drop(busy);
        assert_eq!(board.count(), 0, "用例自己收账，别把连接留给别的用例");
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
        let mut state = EditorState::for_test();
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
        crate::fonts::warm_up(ctx);
        let _lock = frame_lock();
        ctx.run(inp, |ctx| crate::show(ctx, state))
    }

    /// 这几条用例断言的是原版默认界面，用 `for_test()` 把界面档位钉回出厂值
    /// （钢琴、琴键标注、1:1 缩放），用户盘里存过什么都不影响。
    fn piano_state() -> EditorState {
        let mut state = EditorState::for_test();
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
    /// 端口偏离（用户要求）：琴键横向占满——keysshow = min(52, floor(1440/39)) = 36，
    /// startkey = 27 - 18 = 9，左端 x0 = floor((1440-1404)/2) = 18。
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
        assert_eq!(whites.len(), 36, "白键应当是 36 根，实际 {}", whites.len());
        assert_eq!(whites.first().copied(), Some(18.0));
        assert_eq!(whites.last().copied(), Some(18.0 + 35.0 * 39.0));

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
        // 36 根白键里有 11 根的左边没有黑键（C、F 以及最左那根），所以是 25 根黑键。
        assert_eq!(blacks.len(), 25, "黑键应当是 25 根，实际 {}", blacks.len());
        // 黑键画在白键左缘再往左 19px 处。
        assert_eq!(blacks.first().copied(), Some(18.0 + 39.0 - 19.0));

        let t = texts(&out);
        assert!(
            t.iter().any(|s| s == "C2"),
            "最左白键是 16 号键 = C2，实际文本 {t:?}"
        );
        assert!(t.iter().any(|s| s == "C7"), "最右白键是 76 号键 = C7");
        assert!(
            !t.iter().any(|s| s == "<" || s == ">"),
            "原版没有翻页按钮，界面上不该出现 < 或 >"
        );
    }

    /// 偏离原版（用户要求）：琴键横向占满——显示到放不下为止，不再有
    /// 「同屏显示的琴键数」（原版 keysmax）上限。1000 宽：keysshow =
    /// min(52, floor(1000/39)) = 25，startkey = 27 - 12 = 15，x0 = floor((1000-975)/2) = 12。
    #[test]
    fn the_piano_fills_the_panel_width_without_a_keys_to_show_cap() {
        let ctx = egui::Context::default();
        let mut state = piano_state();
        run(&mut state, &ctx, input());
        let mut input = input();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1000.0, 900.0),
        ));
        run(&mut state, &ctx, input.clone());
        let out = run(&mut state, &ctx, input);
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
        assert_eq!(
            whites.len(),
            25,
            "1000 宽应显示 25 根白键，实际 {}",
            whites.len()
        );
        assert_eq!(whites.first().copied(), Some(12.0));
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
        let p = egui::pos2(30.0, 800.0);
        press.events.push(egui::Event::PointerMoved(p));
        press.events.push(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        run(&mut state, &ctx, press);
        assert_eq!(state.current.key, 15, "最左白键是 15 号键（C2）");
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
        assert_eq!(start, 9, "默认 startkey = 27 - keysshow/2（keysshow=36）");

        let mut hover = input();
        hover
            .events
            .push(egui::Event::PointerMoved(egui::pos2(30.0, 800.0)));
        for _ in 0..60 {
            run(&mut state, &ctx, hover.clone());
        }
        assert!(
            state.piano_start < start,
            "停在最左白键上应当往低音平移，实际 startkey = {}",
            state.piano_start
        );

        let mut state = piano_state();
        // 同一个 Context 复用：上一阶段指针停在 (30,800)（最左白键），warm 帧
        // 会接着左移一格，先把指针挪出琴键区再取基准值。
        let mut warm = input();
        warm.events
            .push(egui::Event::PointerMoved(egui::pos2(640.0, 200.0)));
        run(&mut state, &ctx, warm);
        let start = state.piano_start;
        let mut hover = input();
        // 最右一根白键：x = 18 + 35×39 = 1383，宽 39（占满后的 36 键布局）。
        hover
            .events
            .push(egui::Event::PointerMoved(egui::pos2(1400.0, 800.0)));
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

/// 播放特效的接线：`app::show` 里那三扇「谁占中央面板 / 琴键面板开不开 / 全都不开」
/// 的门，以及启用状态与调参的落盘回读。要跑整帧，所以放在这里而不是 `fx/`。
#[cfg(test)]
mod fx_wiring_tests {
    use super::*;

    /// 跑一帧并把画到的文字全捞出来：菜单项、特效清单这些只能这样查。
    fn frame(state: &mut EditorState, ctx: &egui::Context) -> Vec<String> {
        crate::fonts::warm_up(ctx);
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
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// 分离浮窗与主窗同时存活，但浮窗接管绘制时主窗中央区停画网格，只显示提示；
    /// 关闭分离后舞台特效和琴键回主窗。
    #[test]
    fn detached_effects_pause_main_grid_and_show_notice() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        state.fx.lock().stack.set_active("synthesia", true);

        frame(&mut state, &ctx);
        assert_eq!(state.interaction.visible_notes, 0, "未分离时舞台接管网格");
        assert!(state.piano_rect.is_some(), "未分离时琴键在主窗");

        state.prefs.fx_window = true;
        state.interaction.visible_notes = 23;
        let texts = frame(&mut state, &ctx);
        assert_eq!(state.interaction.visible_notes, 0, "分离后主窗不画编辑网格");
        assert!(state.piano_rect.is_none(), "分离后主窗不画琴键");
        assert!(
            texts.iter().any(|s| s.contains("当前正在特效窗口渲染")),
            "主窗中央区要显示浮窗渲染提示"
        );

        state.prefs.fx_window = false;
        assert!(!state.prefs.fx_window);
        assert!(state.fx.lock().stage_active(), "特效应仍启用");
        state.interaction.visible_notes = 0;
        frame(&mut state, &ctx);
        assert!(!state.prefs.fx_window, "跑一帧后分离开关仍关闭");
        assert!(!state.player, "测试状态不是播放器档");
        assert!(state.fx.lock().stage_active(), "跑一帧后特效仍启用");
        assert_eq!(state.interaction.visible_notes, 0, "关闭分离后舞台回主窗");
        assert!(state.piano_rect.is_some(), "关闭分离后琴键回主窗");
    }

    /// `visible_notes` 只有 `draw_grid` 会写，正好当「这一帧网格跑没跑」的探针。
    #[test]
    fn a_stage_effect_takes_the_center_panel_and_keeps_the_piano_panel() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);

        frame(&mut state, &ctx);
        assert!(
            state.interaction.visible_notes > 0,
            "一条特效都没开时网格照跑"
        );
        assert!(
            state.piano_rect.is_some(),
            "show_piano 出厂为真，琴键面板要上报矩形"
        );

        state.interaction.visible_notes = 0;
        state.fx.lock().stack.set_active("synthesia", true);
        frame(&mut state, &ctx);
        assert_eq!(
            state.interaction.visible_notes, 0,
            "舞台接管中央面板后 draw_grid 不该再跑"
        );
        let panel = state
            .piano_rect
            .expect("瀑布的键盘就是底部那条面板，舞台期间它照样要画");
        assert!(
            !state.fx.lock().stack.frame().is_empty(),
            "舞台那一帧要真的排出图元"
        );
        // 「音符刚好落到琴键上」的口径：击中线压在面板上沿（黑键顶出来的那条剪影线），
        // 不横穿黑键；胶囊落点仍是白键面沿 panel.min.y + BLACK_LIFT × 缩放。
        let hit = panel.min.y;
        assert!(
            state
                .fx
                .lock()
                .stack
                .frame()
                .prims
                .iter()
                .any(|prim| matches!(prim,
                    fx::stage::Primitive::Line { from, to, .. }
                        if (from.y - hit).abs() < 0.5 && (to.y - hit).abs() < 0.5)),
            "瀑布的击中线没压在键盘剪影的顶 {hit} 上"
        );
    }

    /// 叠加类特效不动网格，但 `wants_piano` 会把用户关掉的琴键面板顶回来。
    #[test]
    fn an_overlay_effect_keeps_the_grid_and_demands_the_piano_panel() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.flags.show_piano = false;

        frame(&mut state, &ctx);
        assert!(state.piano_rect.is_none(), "琴键面板关掉后矩形要清空");
        assert!(state.interaction.visible_notes > 0, "网格照跑");

        state.fx.lock().stack.set_active("block_row", true);
        frame(&mut state, &ctx);
        assert!(state.fx.lock().wants_piano(), "乐器方块排要求底部有琴键");
        assert!(
            state.piano_rect.is_some(),
            "开着 wants_piano 的特效时琴键面板要被顶回来"
        );
        assert!(
            state.interaction.visible_notes > 0,
            "叠加类特效不接管中央面板"
        );
    }

    #[test]
    fn fx_prefs_survive_a_restart() {
        let _prefs = isolate_prefs();
        let mut state = EditorState::for_test();
        state.fx.lock().stack.set_active("synthesia", true);
        state.fx.lock().stack.set_opacity(0.4);
        state.sync_prefs();
        assert_eq!(state.prefs.fx_active, vec!["synthesia".to_owned()]);
        assert!((state.prefs.fx_opacity - 0.4).abs() < 1e-6);
        assert!(
            state.prefs.fx.contains_key("synthesia"),
            "参数快照要和启用状态一起存"
        );
        state.save_prefs();

        let back = EditorState::new();
        assert!(back.fx.lock().stack.is_active("synthesia"), "重启后勾选状态要回来");
        assert!(
            (back.fx.lock().stack.opacity() - 0.4).abs() < 1e-3,
            "透明度落在 prefs.fx_opacity 那一档"
        );
    }

    /// 「设置 → 播放特效…」那扇窗：清单是全量枚举注册表来的，每条内置特效各带
    /// 自己的身份（舞台 / 叠加）。底座不该认识任何一条具体特效。
    #[test]
    fn the_effects_window_lists_every_registered_effect() {
        let _prefs = isolate_prefs();
        let ctx = egui::Context::default();
        let mut state = EditorState::for_test();
        state.theme.apply(&ctx);
        state.prefs.language = 1;
        assert_eq!(state.fx.lock().stack.ids().len(), 3, "内置三条特效");

        state.window = Some(WindowKind::PlaybackEffects);
        // 两帧：egui 的 Window 第一帧只定下尺寸，文字在第二帧的 shapes 里才捞得到。
        frame(&mut state, &ctx);
        let texts = frame(&mut state, &ctx);
        for (name, kind) in [("钢琴瀑布", "舞台"), ("乐器方块排", "叠加"), ("声道方块", "叠加")] {
            let row = texts
                .iter()
                .find(|t| t.contains(name) && t.contains(kind))
                .unwrap_or_else(|| panic!("清单里少了「{name}（{kind}）」这一行：{texts:?}"));
            assert!(row.starts_with(name), "行首是特效名，勾在它前面：{row}");
        }
    }
}
