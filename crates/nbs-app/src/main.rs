//! Open Note Block Studio — Rust 重写版桌面入口 + 无界面批处理 CLI。
//!
//! ```text
//! cargo run -p nbs-app                                 # 打开演示曲（GUI）
//! cargo run -p nbs-app -- path/to/song.nbs            # 直接打开 .nbs
//! cargo run -p nbs-app -- --info song.nbs             # 打印歌曲信息
//! cargo run -p nbs-app -- --export-wav song.nbs out.wav
//! cargo run -p nbs-app -- --export-midi song.nbs out.mid
//! cargo run -p nbs-app -- --export-png song.nbs out.png
//! cargo run -p nbs-app -- --export-schematic song.nbs out.schematic
//! ```
//! 默认使用 glow (OpenGL) 后端；在根 Cargo.toml 的
//! `workspace.dependencies.eframe.features` 改成 `wgpu` 即可切换，
//! 网格画布只依赖 `egui::Painter`，无需改动任何编辑器代码。

use std::path::{Path, PathBuf};

use eframe::egui;
use nbs_editor::EditorState;

struct NbsApp {
    state: EditorState,
    last_title: String,
}

impl NbsApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut state = EditorState::new();
        state.theme.apply(&cc.egui_ctx);
        // egui 自带字体没有中文字形，不装系统字体整个界面会是一排空框。
        if nbs_editor::fonts::install(&cc.egui_ctx).is_none() {
            eprintln!("警告: 未找到可用的系统中文字体，界面中文会显示成空框");
        }

        if let Some(arg) = std::env::args().nth(1) {
            let path = PathBuf::from(arg);
            if path.extension().and_then(|s| s.to_str()) == Some("nbs") {
                match nbs_core::nbs::load(&path) {
                    Ok(song) => {
                        state.doc.replace_song(song);
                        state.prepare_audio();
                        state.add_recent(path);
                        state.set_message("已从命令行载入歌曲");
                    }
                    Err(e) => state.set_message(format!("载入失败: {e}")),
                }
            }
        }
        Self { state, last_title: String::new() }
    }
}

impl Drop for NbsApp {
    fn drop(&mut self) {
        // 退出时持久化主题、音频参数、最近文件等偏好
        self.state.save_prefs();
    }
}

impl eframe::App for NbsApp {
    /// 原版 `control_draw.gml:153` 的 `draw_clear(window_background)`。
    /// eframe 默认清成透明，glow 后端下就是一整片黑，凡是自己没重绘的面板都露黑底。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.state.theme.window_bg;
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let title = self.state.song_title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        nbs_editor::show(ctx, &mut self.state);
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a.starts_with("--")) {
        if let Err(e) = run_cli(&args) {
            eprintln!("错误: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 620.0])
            .with_title("nbs-rust"),
        ..Default::default()
    };
    eframe::run_native(
        "nbs-rust",
        options,
        Box::new(|cc| Ok(Box::new(NbsApp::new(cc)))),
    )
}

/// 无界面批处理：便于 CI / 脚本批量转换，不需要音频设备或显示器。
fn run_cli(args: &[String]) -> Result<(), String> {
    let cmd = args[0].as_str();
    if cmd == "--help" || cmd == "-h" {
        println!(
            "nbs-rust CLI\n\
             --info <song.nbs>                          打印歌曲信息\n\
             --export-wav <song.nbs> <out.wav>         离线导出 PCM WAV\n\
             --export-audio <song.nbs> <out.mp3|aiff|flac> [--rate N] [--channels N]  经 ffmpeg 转码导出\n\
             --export-midi <song.nbs> <out.mid>        导出 SMF MIDI\n\
             --export-png <song.nbs> <out.png>         导出 PNG 谱面\n\
             --export-schematic <song.nbs> <out.schematic>  导出原理图\n\
             --export-datapack <song.nbs> <out.zip|目录名>  导出 Minecraft 数据包\n\
             --export-nbp <song.nbs> <out.nbp>         导出选区片段\n\
             --import-midi <in.mid> <out.nbs>          导入 MIDI 为 NBS\n\
             --import-schematic <in.schematic> <out.nbs>  导入 Minecraft 原理图为 NBS\n\
             --audio-info                             显示音频设备与采样加载状态"
        );
        return Ok(());
    }
    if cmd == "--audio-info" {
        use nbs_audio::AudioEngine as _;
        let engine = nbs_audio::CpalEngine::new();
        println!("引擎: {}", engine.name());
        println!("设备: {}", engine.device_info());
        println!("采样: {}", engine.sample_status());
        println!("采样目录: {:?}", nbs_audio::find_sounds_dir(None));
        return Ok(());
    }
    if cmd == "--info" {
        if args.len() < 2 {
            return Err("用法: --info <song.nbs>".into());
        }
        let song = nbs_core::nbs::load(Path::new(&args[1])).map_err(|e| e.to_string())?;
        println!(
            "名称: {}\n作者: {}\n版本: NBS v{}\n音符: {}\n图层: {}\n长度: {} ticks\n速度: {:.2} t/s ({:.0} BPM)",
            song.meta.name, song.meta.author, song.meta.save_version,
            song.total_blocks, song.layers.len(), song.song_length(),
            song.meta.tempo, song.meta.bpm()
        );
        return Ok(());
    }
    if args.len() < 3 {
        return Err(format!("用法: {cmd} <输入> <输出>（--help 查看全部）"));
    }
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);

    if cmd == "--import-midi" {
        // precision=0 → deltapertick = PPQ/4 = 120，与本程序导出的 MIDI 1:1 对应
        let opts = nbs_io::midi::MidiImportOptions { precision: 0, ..Default::default() };
        let song = nbs_io::midi::import_midi_path(input, &opts)?;
        nbs_core::nbs::save(output, &song, nbs_core::nbs::NBS_VERSION).map_err(|e| e.to_string())?;
        println!("已导入 {} 个音符 → {}", song.total_blocks, output.display());
        return Ok(());
    }

    if cmd == "--import-schematic" {
        let info = nbs_io::schematic::import_schematic(input).map_err(|e| e.to_string())?;
        nbs_core::nbs::save(output, &info.song, nbs_core::nbs::NBS_VERSION).map_err(|e| e.to_string())?;
        println!(
            "图样 {}×{}×{}：{} 个输入 / {} 个音符盒 / 落子 {} → {}",
            info.width, info.length, info.height, info.inputs, info.note_blocks,
            info.added_notes, output.display()
        );
        return Ok(());
    }

    let song = nbs_core::nbs::load(input).map_err(|e| e.to_string())?;
    match cmd {
        "--export-wav" => {
            let bank = nbs_audio::SoundBank::discover();
            let st = nbs_audio::export_wav(
                &song,
                &bank,
                output,
                &nbs_audio::ExportOptions::default(),
            )?;
            println!(
                "已导出 {} 音符 / {:.1}s / {} 线程 / 峰值 {:.2} → {}",
                st.notes, st.duration_seconds, st.threads, st.peak, output.display()
            );
        }
        // 与原版一样：先渲染 WAV，再交给 ffmpeg 转目标格式。格式由输出扩展名决定。
        "--export-audio" => {
            let format = nbs_io::audio_export::AudioFormat::from_path(output)
                .ok_or_else(|| format!("无法从 {} 判断音频格式", output.display()))?;
            let mut opts = nbs_audio::ExportOptions::default();
            for (i, a) in args.iter().enumerate() {
                match a.as_str() {
                    "--rate" => {
                        opts.sample_rate = args[i + 1].parse().map_err(|_| "采样率不是数字")?
                    }
                    "--channels" => {
                        opts.channels = args[i + 1].parse().map_err(|_| "声道数不是数字")?
                    }
                    _ => {}
                }
            }
            if !nbs_io::audio_export::SAMPLE_RATES.contains(&opts.sample_rate) {
                return Err(format!("采样率 {} 不在原版列表中", opts.sample_rate));
            }
            let tmp = output.with_extension("nbs-tmp.wav");
            let st = nbs_audio::export_wav(&song, &nbs_audio::SoundBank::discover(), &tmp, &opts)?;
            let enc = nbs_io::audio_export::EncodeOptions {
                format,
                sample_rate: opts.sample_rate,
                channels: opts.channels,
            };
            let res = nbs_io::audio_export::encode_from_wav(&tmp, output, &enc);
            let _ = std::fs::remove_file(&tmp);
            res.map_err(|e| e.to_string())?;
            println!(
                "已导出 {} 音符 / {:.1}s → {}（{}，{} Hz，{} 声道）",
                st.notes,
                st.duration_seconds,
                output.display(),
                format.name(),
                opts.sample_rate,
                opts.channels
            );
        }
        "--export-midi" => {
            nbs_io::midi::export_midi(&song, output)?;
            println!("已导出 MIDI → {}", output.display());
        }
        "--export-png" => {
            let (w, h) = nbs_io::image_export::export_png(
                &song,
                output,
                &nbs_io::image_export::ImageOptions::default(),
            )?;
            println!("已导出 PNG 谱面 {w}×{h} → {}", output.display());
        }
        "--export-schematic" => {
            let info = nbs_io::schematic::export_schematic(
                &song,
                output,
                &nbs_io::schematic::SchematicOptions::default(),
            )?;
            println!(
                "已导出原理图 {}×{}×{} / {} 音符盒 → {}",
                info.length, info.width, info.height, info.note_blocks, output.display()
            );
        }
        "--export-datapack" => {
            let mut opts = nbs_io::datapack::DatapackOptions::default();
            // GUI 打开窗口时把 dat_name 预填成歌曲名，CLI 保持一致；没有名字就退回输出文件名。
            let stem = output
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            opts.name = song.meta.name.trim().to_string();
            if opts.name.is_empty() {
                opts.name = stem;
            }
            // 原版用 dat_usezip 同时决定容器和 uninstall.mcfunction 里的引用形式；
            // CLI 没有这个开关，就以输出名 itself 为准，两者不许各说各话。
            opts.use_zip = output.extension().and_then(|e| e.to_str()) == Some("zip");
            let info = nbs_io::datapack::export(&song, &opts, output)?;
            println!(
                "已导出数据包 {} 个文件（{}）→ {}\n播放命令：/function {}play",
                info.files,
                if info.zip { "ZIP" } else { "目录" },
                output.display(),
                info.functionpath
            );
        }
        "--export-nbp" => {
            let items: Vec<_> = song.iter_notes().collect();
            let sel = nbs_core::Selection::from_absolute(0, 0, &items);
            nbs_io::pattern::export_pattern(&sel, &song, output)?;
            println!("已导出片段 {} 音符 → {}", sel.len(), output.display());
        }
        other => return Err(format!("未知命令: {other}，用 --help 查看")),
    }
    Ok(())
}
