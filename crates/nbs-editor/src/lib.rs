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
pub use state::{EditorState, EditMode, Flag, UiFlags, WindowKind};
pub use theme::{Theme, ThemeKind};
pub use viewport::Viewport;

#[cfg(test)]
mod tests {
    use super::*;

    /// 这条断言检查的是出厂值，不能读到人手上那份 `prefs.json`：
    /// 调试界实时改过缩放 / 音量，单测就会莫名其妙变红。指向一个空的配置目录。
    fn isolate_prefs() {
        let dir = std::env::temp_dir().join("nbs-rust-test-prefs");
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("NBS_RUST_CONFIG_DIR", &dir);
    }

    #[test]
    fn editor_state_defaults() {
        isolate_prefs();
        let s = EditorState::new();
        assert!(s.doc.song.total_blocks > 0);
        assert_eq!(s.viewport.zoom, 1.0);
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
            crate::show(ctx, &mut state);
        });
        assert!(!out.shapes.is_empty(), "一帧应当产生绘制形状");
        assert!(state.interaction.visible_notes > 0, "演示曲应有可见音符");
    }

    /// 用原版自带的大曲子验证「一帧只绘制屏幕内音符」这一硬性要求。
    #[test]
    fn real_song_frame_is_culled() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../NoteBlockStudio/datafiles/Songs/Megalovania - Super Smash Bros. Ultimate.nbs");
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
        let _ = ctx.run(input, |ctx| crate::show(ctx, &mut state));

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
            s.doc.place_note(i * 50, 0, nbs_core::model::Note::new(0, 45));
        }
        s.viewport.size = egui::vec2(1200.0, 600.0);
        let (t0, t1, _, _) = s.viewport.visible_range();
        assert!(t1 - t0 < 100, "可见区间不得随歌曲长度增长");
        // 只查询可见区，不能把 2000 个音符全捞出来
        assert!(s.doc.song.notes_in_view(t0, t1, 0, 20).len() < 100);
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
        let out = ctx.run(input, |ctx| crate::show(ctx, state));
        assert!(!out.shapes.is_empty(), "窗口应当真的画出内容");
    }

    /// 走一遍 GUI 的真实调用链：音频导出窗口 → 文件对话框 → dispatch 导出。
    /// 这里断言的是产物字节，不是「按钮没 panic」。
    #[test]
    fn audio_export_window_dialog_and_wav_output_are_real() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        state.window = Some(WindowKind::AudioExport);
        frame(&mut state, &ctx);

        state.prefs.export_format = "WAV".into();
        state.prefs.export_sample_rate = 22050;
        state.prefs.export_channels = 1;
        dispatch(&mut state, crate::commands::Command::OpenDialog(state::DialogAction::ExportAudio), &ctx);
        let dialog = state.file_dialog.as_ref().expect("对话框已打开");
        assert_eq!(dialog.ext, "wav", "扩展名跟着格式走");
        assert_eq!(state.window, Some(WindowKind::FileSave));

        let out = std::env::temp_dir().join("nbs_gui_export_mono.wav");
        let _ = std::fs::remove_file(&out);
        dispatch(&mut state, crate::commands::Command::DialogConfirm(out.clone()), &ctx);

        let bytes = std::fs::read(&out).expect("GUI 导出必须写出真实文件");
        assert!(bytes.len() > 1000, "产物只有 {} 字节", bytes.len());
        assert_eq!(&bytes[..4], b"RIFF");
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let ch = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
        assert_eq!((rate, ch), (22050, 1), "采样率/声道必须按窗口里的选项写进头部");
        assert!(state.export_status.as_deref().unwrap_or_default().contains("WAV 已导出"));
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
        dispatch(&mut state, crate::commands::Command::DialogConfirm(out.clone()), &ctx);
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
        ctx.run(input, |ctx| crate::show(ctx, state))
    }

    /// Ctrl+P 开窗：原版 596 行用 `prevwindow == window` 挡住同一帧的「开 → 关」，
    /// 这里同样要求窗口先活过一帧才接受 Ctrl+P 关窗，否则窗口一闪即消失。
    #[test]
    fn ctrl_p_opens_preferences_and_stays_open() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
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
        let d = std::env::temp_dir().join(format!(
            "nbs-gui-{tag}-{}",
            std::process::id()
        ));
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
        dispatch(&mut state, commands::Command::DialogConfirm(pack.clone()), &ctx);
        assert_eq!(state.window, Some(WindowKind::DatapackExport), "原版导出后回到本窗口");

        let mut files = Vec::new();
        collect(&pack, &pack, &mut files);
        let plan = nbs_io::datapack::generate(
            &state.doc.song,
            &state.datapack_options,
            "pack_folder",
        );
        assert_eq!(files.len(), plan.files.len(), "落盘文件数与计划不一致");
        let got: std::collections::BTreeMap<&str, String> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.clone()))
            .collect();
        for f in &plan.files {
            assert_eq!(
                got.get(f.path.as_str()).map(|s| s.as_str()),
                Some(f.content.as_str()),
                "{} 内容与生成计划不一致",
                f.path
            );
        }
        // 窗口里填的三项要真的决定包内结构（消毒后：命名空间 my_pack，路径 songs/jazz）。
        assert!(got.contains_key("data/my_pack/function/songs/jazz/rust_rewrite_demo/play.mcfunction"));
        assert!(got.contains_key("data/minecraft/tags/function/load.json"));
        // 目录分支的卸载命令不带 `file/` 前缀（原版按 dat_usezip 分两支），
        // 包名取对话框里选中的文件名。
        assert!(got["data/my_pack/function/songs/jazz/rust_rewrite_demo/uninstall.mcfunction"]
            .contains("datapack disable \"pack_folder\""));
        let status = state.export_status.clone().unwrap_or_default();
        assert!(status.contains("/function my_pack:songs/jazz/rust_rewrite_demo/play"), "{status}");

        // ZIP 分支：文件名没写 .zip 时原版会补上，产物必须是真 ZIP。
        let zip_in = dir.join("pack_zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        state.datapack_options.use_zip = true;
        dispatch(&mut state, commands::Command::DialogConfirm(zip_in.clone()), &ctx);
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
        state.datapack_options.name = "  !! ".into();
        let dir = scratch("bad-name");
        let out = dir.join("nope.zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        dispatch(&mut state, commands::Command::DialogConfirm(out.clone()), &ctx);
        assert!(state.export_status.is_none(), "失败却写了成功状态");
        assert!(!out.exists(), "报错时不应留下产物");
        assert!(
            msg(&state).contains("数据包需要一个有效的唯一名称"),
            "{:?}",
            msg(&state)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 路径套太多层要按原版规则拒绝（`string_count("/", dat_getpath(dat_path)) >= 5`）。
    #[test]
    fn datapack_export_refuses_too_many_subfolders() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
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
        dispatch(&mut state, commands::Command::DialogConfirm(ok.clone()), &ctx);
        assert!(ok.is_file(), "五个子文件夹应通过原版的闸门");

        state.datapack_options.path = "a/b/c/d/e/f".into();
        let out = dir.join("deep.zip");
        dispatch(
            &mut state,
            commands::Command::OpenDialog(state::DialogAction::ExportDatapack),
            &ctx,
        );
        dispatch(&mut state, commands::Command::DialogConfirm(out.clone()), &ctx);
        assert!(!out.exists());
        assert!(msg(&state).contains("路径最多只能套 5 个子文件夹"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真点一次鼠标：切页签、关掉「导出为 ZIP」、点「导出」。
    /// 证明界面上的控件写的就是导出器读的那份状态，而不只是「画出了几个控件」。
    #[test]
    fn clicking_window_controls_drives_the_export() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
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
        assert_ne!(state.datapack_options.glow, glow_before, "效果器页的复选框没接上");

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
        dispatch(&mut state, commands::Command::DialogConfirm(pack.clone()), &ctx);
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
        let mut state = EditorState::new();
        state.theme.apply(&ctx);
        // 原版第一次会写 settings.ini 记住「警告已看过」，测试预置成已看过，不碰用户配置。
        state.prefs.warning_schematic = true;

        click_text(&mut state, &ctx, "文件");
        click_text(&mut state, &ctx, "导出");
        click_text(&mut state, &ctx, "Minecraft 原理图…");
        answer_all_modals(&mut state, &ctx);
        assert_eq!(state.window, Some(WindowKind::SchematicExport), "提问答完才开窗");
        assert_eq!(state.schematic_tab, 0, "开窗固定在「设计」页");
        assert!(state.schematic_stats.totalblocks[0] > 0, "演示曲应有可导出的音符");

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
        assert!(state.schematic_options.command_block, "command_block 不在原版默认值表里");
        assert!(state.schematic_options.structure, "structure 同样不在表里");

        // 方块页：第一个乐器默认是 3, 0（泥土），改成 2, 0（草方块）。
        click_text(&mut state, &ctx, "方块");
        assert_eq!(state.schematic_tab, 1);
        click_at(&mut state, &ctx, |t| t == "3, 0");
        click_at(&mut state, &ctx, |t| t.starts_with("2, 0  "));
        assert_eq!(state.schematic_options.ins_block[0], 2, "下拉选择没写回选项");
        assert_eq!(state.schematic_options.ins_data[0], 0);

        click_text(&mut state, &ctx, "导出");
        assert_eq!(state.window, Some(WindowKind::FileSave), "点「导出」应打开保存对话框");
        let dir = scratch("schematic");
        let out = dir.join("by_click.nbt");
        dispatch(&mut state, commands::Command::DialogConfirm(out.clone()), &ctx);
        assert!(out.is_file(), "结构方块分支应写出 {}", out.display());
        let bytes = std::fs::read(&out).expect("导出必须写出真实文件");
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "前两端应是 gzip 魔数");

        // 原版保存成功后 `message("Schematic 已保存！")`，答完把导出窗口重新打开。
        assert_eq!(state.modals.items.len(), 1);
        assert_eq!(state.modals.items[0].title, "导出 Schematic");
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
        let recents = state.recent_files.len();
        // 真点菜单：文件 → 导入 → 从 Schematic 导入…
        click_text(&mut state, &ctx, "文件");
        click_text(&mut state, &ctx, "导入");
        click_text(&mut state, &ctx, "从 Schematic 导入…");
        let d = state.file_dialog.as_ref().expect("导入要弹文件对话框");
        assert!(!d.save, "导入用的是打开式对话框");
        assert_eq!(d.ext, "schematic", "原版过滤器是 Minecraft Schematics (*.schematic)");
        assert_eq!(state.window, Some(WindowKind::FileOpen));

        state.doc.replace_song(nbs_core::Song::new());
        assert_eq!(state.doc.song.total_blocks, 0);
        dispatch(&mut state, commands::Command::DialogConfirm(file.clone()), &ctx);
        assert!(state.file_dialog.is_none());
        assert_eq!(state.window, None);
        assert!(state.doc.song.total_blocks > 0, "读回来应该是有音符的谱面");
        assert!(msg(&state).starts_with("Schematic 导入完成"), "{}", msg(&state));
        assert_eq!(state.doc.song.filename, None, "不学原版把 filename 指向 .schematic");
        assert_eq!(state.recent_files.len(), recents, "原版 open_schematic() 不写最近列表");

        // 打开分流：.schematic 走导入，不会被当成 NBS 解析。
        let mut s2 = EditorState::new();
        s2.theme.apply(&ctx);
        let recents2 = s2.recent_files.len();
        s2.doc.replace_song(nbs_core::Song::new());
        dispatch(&mut s2, commands::Command::OpenPath(file), &ctx);
        assert!(s2.doc.song.total_blocks > 0);
        assert!(msg(&s2).starts_with("Schematic 导入完成"), "{}", msg(&s2));
        assert_eq!(s2.recent_files.len(), recents2);

        // 坏文件：原版的 message()，标题不经 condstr，保持英文。
        let bad = dir.join("bad.schematic");
        std::fs::write(&bad, b"not a schematic at all").unwrap();
        click_text(&mut s2, &ctx, "文件");
        click_text(&mut s2, &ctx, "导入");
        click_text(&mut s2, &ctx, "从 Schematic 导入…");
        dispatch(&mut s2, commands::Command::DialogConfirm(bad), &ctx);
        assert_eq!(s2.modals.items.len(), 1, "失败要弹模态，不只是状态栏一行字");
        assert_eq!(s2.modals.items[0].title, "Error");
        assert_eq!(s2.modals.items[0].yes_no, false, "原版这里是 message()，只有确定键");
        assert!(
            s2.modals.items[0].text.contains("ERROR: Not a Schematic file."),
            "{}",
            s2.modals.items[0].text
        );
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
        dispatch(&mut state, commands::Command::ShowWindow(WindowKind::Instruments), &ctx);
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
            texts.iter().filter(|t| t.starts_with("Ctrl+Shift+")).count(),
            7,
            "Ctrl+Shift+数字应当只有 7 个"
        );
        assert_eq!(
            texts.iter().filter(|t| t.starts_with("Ctrl+") && !t.contains("Shift")).count(),
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
        dispatch(&mut state, crate::commands::Command::DialogConfirm(out.clone()), &ctx);

        let bytes = std::fs::read(&out).unwrap_or_else(|e| panic!("{ffmpeg:?} 转码失败: {e}"));
        assert_eq!(&bytes[..4], b"fLaC", "产物不是 FLAC");
        assert!(!tmp_wav.exists(), "中间 WAV 必须清掉");
        assert!(state.export_status.as_deref().unwrap_or_default().contains("FLAC 已导出"));
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
        ctx.run(inp, |ctx| crate::show(ctx, state))
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

    /// 原版：白键步进 39、顶边在 `rh - 154`，整排居中；黑键画在左边 19px、上边 7px 处。
    /// 1440 宽时 `keysshow = min(30, floor((1440-32)/39)) = 30`，左边留 (1440-1170)/2 = 135。
    #[test]
    fn piano_uses_the_original_fixed_key_geometry() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        run(&mut state, &ctx, input());
        let out = run(&mut state, &ctx, input());
        let quads = images(&out);

        let mut whites: Vec<f32> = quads
            .iter()
            .filter(|r| (r.width() - 40.0).abs() < 0.6 && (r.height() - 121.0).abs() < 0.6 && (r.min.y - 746.0).abs() < 0.6)
            .map(|r| r.min.x)
            .collect();
        whites.sort_by(|a, b| a.total_cmp(b));
        whites.dedup();
        assert_eq!(whites.len(), 30, "白键应当是 30 根，实际 {}", whites.len());
        assert_eq!(whites.first().copied(), Some(135.0));
        assert_eq!(whites.last().copied(), Some(135.0 + 29.0 * 39.0));

        let mut blacks: Vec<f32> = quads
            .iter()
            .filter(|r| (r.width() - 40.0).abs() < 0.6 && (r.height() - 121.0).abs() < 0.6 && (r.min.y - 739.0).abs() < 0.6)
            .map(|r| r.min.x)
            .collect();
        blacks.sort_by(|a, b| a.total_cmp(b));
        blacks.dedup();
        // 30 根白键里有 8 根的左边没有黑键（C、F 以及最左那根），所以是 21 根黑键。
        assert_eq!(blacks.len(), 21, "黑键应当是 21 根，实际 {}", blacks.len());
        // 黑键画在白键左缘再往左 19px 处。
        assert_eq!(blacks.first().copied(), Some(135.0 + 39.0 - 19.0));

        let t = texts(&out);
        assert!(t.iter().any(|s| s == "F2"), "最左白键是 20 号键 = F2，实际文本 {t:?}");
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
        let mut state = EditorState::new();
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
        let mut state = EditorState::new();
        run(&mut state, &ctx, input());
        let start = state.piano_start;
        assert_eq!(start, 12, "默认 startkey = 27 - keysshow/2");

        let mut hover = input();
        hover.events.push(egui::Event::PointerMoved(egui::pos2(150.0, 800.0)));
        for _ in 0..60 {
            run(&mut state, &ctx, hover.clone());
        }
        assert!(
            state.piano_start < start,
            "停在最左白键上应当往低音平移，实际 startkey = {}",
            state.piano_start
        );

        let mut state = EditorState::new();
        run(&mut state, &ctx, input());
        let start = state.piano_start;
        let mut hover = input();
        // 最右一根白键：x = 135 + 29*39 = 1266，宽 39。
        hover.events.push(egui::Event::PointerMoved(egui::pos2(1280.0, 800.0)));
        for _ in 0..60 {
            run(&mut state, &ctx, hover.clone());
        }
        assert!(
            state.piano_start > start,
            "停在最右白键上应当往高音平移，实际 startkey = {}",
            state.piano_start
        );
    }
}
