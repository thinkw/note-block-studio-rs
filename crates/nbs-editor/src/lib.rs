//! `nbs-editor` — egui 实现的网格音符编辑器（分析里的 D 模块）。
//!
//! ```text
//! app          一帧编排：输入 → 命令 → 状态 → 面板 → 弹窗
//! grid         超大滚动网格画布、视口剔除、交互状态机、右键菜单
//! viewport     坐标换算 / 可见区间 / 缩放 / 播放跟随
//! interaction  DragMode 状态机 + 一帧输入快照
//! commands     统一命令总线与 dispatch
//! document(*)  所有编辑命令的 diff 事务（在 nbs-core）
//! layers_panel / toolbar / timeline(grid) / piano / statusbar / instruments_panel
//! windows      歌曲信息、属性、统计、首选项、宏、文件对话框、帮助
//! theme        4 套主题
//! shortcuts    全局快捷键
//! ```
//! 只依赖 `egui`（不依赖 eframe），渲染后端由入口 crate 决定。

pub mod app;
pub mod commands;
pub mod grid;
pub mod instruments_panel;
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

    #[test]
    fn editor_state_defaults() {
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
