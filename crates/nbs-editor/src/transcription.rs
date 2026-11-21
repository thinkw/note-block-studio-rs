use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use egui::{Color32, Sense, Stroke, StrokeKind};
use nbs_audio::midi_preview::{MidiPlaybackState, MidiPreviewPlayer};
use nbs_io::midi_preview::{parse_midi_preview, MidiPreview};
use nbs_io::transcription::{
    RuntimeInfo, TranscriptionEvent, TranscriptionOptions, TranscriptionRuntime, TranscriptionTask,
};

use crate::commands::Command;
use crate::state::{DialogAction, EditorState, WindowKind};
use crate::tr;

#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptionAction {
    PrepareRuntime,
    Probe,
    PrepareModel,
    Run,
    Cancel,
    Play,
    /// Drag the preview scrub bar: restart the stream at this position.
    Seek(f64),
    PauseResume,
    Stop,
    Import,
    SaveMidiConfirmed(PathBuf),
}

pub struct MidiPreviewAsset {
    pub path: PathBuf,
    pub name: String,
    pub bytes: Vec<u8>,
    pub data: MidiPreview,
    pub warnings: Vec<String>,
}

pub struct TranscriptionState {
    pub runtime: TranscriptionRuntime,
    pub options: TranscriptionOptions,
    pub audio_path: String,
    pub model_path: String,
    pub soundfont_path: String,
    pub instruments: String,
    pub accepted_downloads: bool,
    pub install_cuda: bool,
    pub task: Option<TranscriptionTask>,
    pub info: Option<RuntimeInfo>,
    pub preview: Option<MidiPreviewAsset>,
    pub player: Option<MidiPreviewPlayer>,
    pub error: Option<String>,
    pub status: String,
    pub progress: Option<f32>,
    pub note_count: u64,
    pub volume: f32,
    pub import_from_preview: bool,
    messages: VecDeque<String>,
    preview_loading: Option<mpsc::Receiver<Result<MidiPreviewAsset, String>>>,
    scrub: Option<f64>,
    source_name: String,
    cancel_requested: bool,
}

impl Default for TranscriptionState {
    fn default() -> Self {
        let runtime = TranscriptionRuntime::in_workspace_directory();
        let soundfont_path = runtime.default_soundfont().to_string_lossy().into_owned();
        Self {
            runtime,
            options: TranscriptionOptions::default(),
            audio_path: String::new(),
            model_path: String::new(),
            soundfont_path,
            instruments: String::new(),
            accepted_downloads: false,
            install_cuda: true,
            task: None,
            info: None,
            preview: None,
            player: None,
            error: None,
            status: String::new(),
            progress: None,
            note_count: 0,
            volume: 0.6,
            import_from_preview: false,
            messages: VecDeque::new(),
            preview_loading: None,
            scrub: None,
            source_name: String::new(),
            cancel_requested: false,
        }
    }
}

impl TranscriptionState {
    pub fn busy(&self) -> bool {
        self.task.is_some() || self.preview_loading.is_some()
    }

    pub fn stop_preview(&self) {
        if let Some(player) = &self.player {
            player.stop();
        }
    }

    fn request_options(&self) -> TranscriptionOptions {
        let mut options = self.options.clone();
        options.model_path =
            (!self.model_path.trim().is_empty()).then(|| PathBuf::from(self.model_path.trim()));
        options.instruments = self
            .instruments
            .split([',', '，'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        options
    }

    fn begin(&mut self, task: TranscriptionTask) {
        self.task = Some(task);
        self.cancel_requested = false;
        self.error = None;
        self.status = tr("Starting local task…", "正在启动本机任务…").into();
        self.progress = None;
        self.note_count = 0;
        self.messages.clear();
    }

    fn open_preview(&mut self, path: PathBuf, name: String, warnings: Vec<String>) {
        self.stop_preview();
        self.preview = None;
        self.error = None;
        self.status = tr("Reading MIDI preview…", "正在读取 MIDI 预览…").into();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                file.take(64 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 64 * 1024 * 1024 {
                    return Err("MIDI exceeds 64 MiB / MIDI 超过 64 MiB".into());
                }
                let data = parse_midi_preview(&bytes)?;
                Ok(MidiPreviewAsset {
                    path,
                    name,
                    bytes,
                    data,
                    warnings,
                })
            })();
            let _ = tx.send(result);
        });
        self.preview_loading = Some(rx);
    }
}

pub fn poll(state: &mut EditorState, ctx: &egui::Context) {
    let t = &mut state.transcription;
    let events = t
        .task
        .as_ref()
        .map(TranscriptionTask::drain)
        .unwrap_or_default();
    for event in events {
        let terminal = event.is_terminal();
        let event = if t.cancel_requested && terminal {
            TranscriptionEvent::Cancelled
        } else {
            event
        };
        match event {
            TranscriptionEvent::Message(text) => {
                t.messages.push_back(text);
                if t.messages.len() > 40 {
                    t.messages.pop_front();
                }
            }
            TranscriptionEvent::Progress {
                stage,
                message,
                completed,
                total,
                note_count,
            } => {
                t.status = if message.is_empty() { stage } else { message };
                t.progress = completed
                    .zip(total)
                    .filter(|(_, total)| *total > 0)
                    .map(|(done, total)| (done as f32 / total as f32).clamp(0.0, 1.0));
                if let Some(count) = note_count {
                    t.note_count = count;
                }
            }
            TranscriptionEvent::Ready(info) => {
                t.status = tr(
                    "Local runtime check passed. This check did not transcribe audio.",
                    "本地环境检查通过，本次检查没有执行音频识别。",
                )
                .into();
                t.info = Some(info);
            }
            TranscriptionEvent::ModelReady {
                model,
                device,
                dtype,
            } => {
                t.status = format!(
                    "{model} · {device} · {dtype} · {}",
                    tr("model load passed", "模型加载通过")
                );
            }
            TranscriptionEvent::Complete(output) => {
                t.note_count = output.raw_note_count as u64;
                let mut warnings = output.warnings;
                warnings.push(format!(
                    "{}: {} · {} · {}",
                    tr("Recognition", "识别配置"),
                    output.model,
                    output.device,
                    output.dtype
                ));
                warnings.push(format!(
                    "{}: {}",
                    tr("Original event audit", "原始事件记录"),
                    output.events_path.display()
                ));
                let name = Path::new(&t.source_name)
                    .with_extension("mid")
                    .to_string_lossy()
                    .into_owned();
                t.open_preview(output.midi_path, name, warnings);
            }
            TranscriptionEvent::Failed(error) => {
                t.status = tr(
                    "Failed; no new MIDI result is available for import.",
                    "任务失败；没有可导入的新 MIDI 结果。",
                )
                .into();
                t.error = Some(error);
            }
            TranscriptionEvent::Cancelled => {
                t.status = tr(
                    "Cancelled. The current NBS song is unchanged.",
                    "已取消，当前 NBS 曲谱未改动。",
                )
                .into();
            }
        }
        if terminal {
            t.task = None;
            t.progress = None;
        }
    }
    if let Some(rx) = &t.preview_loading {
        match rx.try_recv() {
            Ok(Ok(asset)) => {
                t.status = if asset.data.notes.is_empty() {
                    tr(
                        "MIDI ready, but it holds no notes; not imported into NBS",
                        "MIDI 已就绪，但没有任何音符，尚未导入 NBS",
                    )
                    .into()
                } else {
                    format!(
                        "{} · {} {} · {:.2} s",
                        tr(
                            "MIDI ready; not imported into NBS",
                            "MIDI 已就绪，尚未导入 NBS"
                        ),
                        asset.data.notes.len(),
                        tr("notes", "个音符"),
                        asset.data.duration_seconds
                    )
                };
                t.preview = Some(asset);
                t.preview_loading = None;
            }
            Ok(Err(error)) => {
                t.error = Some(error);
                t.preview_loading = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                t.error = Some(tr("MIDI preview task stopped", "MIDI 预览任务异常结束").into());
                t.preview_loading = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    let playing = t
        .player
        .as_ref()
        .map(|p| {
            matches!(
                p.snapshot().state,
                MidiPlaybackState::Loading | MidiPlaybackState::Playing
            )
        })
        .unwrap_or(false);
    if t.busy() || playing {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

pub fn handle(state: &mut EditorState, action: TranscriptionAction) {
    let t = &mut state.transcription;
    if matches!(
        action,
        TranscriptionAction::PrepareRuntime
            | TranscriptionAction::Probe
            | TranscriptionAction::PrepareModel
            | TranscriptionAction::Run
    ) && t.busy()
    {
        return;
    }
    match action {
        TranscriptionAction::PrepareRuntime => {
            if !t.accepted_downloads {
                t.error = Some(
                    tr(
                        "Confirm the local downloads first.",
                        "请先确认本地下载和模型使用许可。",
                    )
                    .into(),
                );
                return;
            }
            t.begin(t.runtime.prepare(t.install_cuda));
        }
        TranscriptionAction::Probe => t.begin(t.runtime.probe()),
        TranscriptionAction::PrepareModel => {
            if !t.accepted_downloads {
                t.error =
                    Some(tr("Confirm the model license first.", "请先确认模型使用许可。").into());
                return;
            }
            t.begin(t.runtime.prepare_model(t.request_options()));
        }
        TranscriptionAction::Run => {
            if !t.accepted_downloads {
                t.error =
                    Some(tr("Confirm the model license first.", "请先确认模型使用许可。").into());
                return;
            }
            let audio = PathBuf::from(t.audio_path.trim());
            if !audio.is_file() {
                t.error =
                    Some(tr("Choose an existing audio file.", "请选择存在的音频文件。").into());
                return;
            }
            t.stop_preview();
            t.preview = None;
            t.source_name = audio
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            t.begin(t.runtime.transcribe(audio, t.request_options()));
        }
        TranscriptionAction::Cancel => {
            t.cancel_requested = true;
            if let Some(task) = &t.task {
                task.cancel();
                t.status = tr("Cancelling local task…", "正在取消本机任务…").into();
            }
            if t.preview_loading.take().is_some() {
                t.status = tr("Preview loading cancelled.", "已取消读取预览。").into();
            }
        }
        TranscriptionAction::Play => start_playback(state, 0.0, false),
        TranscriptionAction::Seek(at) => {
            let start_paused = t
                .player
                .as_ref()
                .is_some_and(|p| p.snapshot().state == MidiPlaybackState::Paused);
            start_playback(state, at, start_paused);
        }
        TranscriptionAction::PauseResume => {
            if let Some(player) = &t.player {
                if player.snapshot().state == MidiPlaybackState::Paused {
                    player.resume();
                } else {
                    player.pause();
                }
            }
        }
        TranscriptionAction::Stop => t.stop_preview(),
        TranscriptionAction::SaveMidiConfirmed(path) => save_preview(t, &path),
        TranscriptionAction::Import => {
            if t.busy() {
                return;
            }
            let Some(asset) = &t.preview else {
                return;
            };
            if asset.data.notes.is_empty() {
                t.error = Some(
                    tr(
                        "The MIDI contains no notes to import.",
                        "MIDI 中没有可导入的音符。",
                    )
                    .into(),
                );
                return;
            }
            if let Some(error) = &asset.data.import_blocked_reason {
                t.error = Some(error.clone());
                return;
            }
            match nbs_io::midi::parse_midi(&asset.bytes) {
                Ok(midi) => {
                    let imported_notes: usize =
                        midi.tracks.iter().map(|track| track.events.len()).sum();
                    if imported_notes != asset.data.notes.len() {
                        t.error = Some(tr("The NBS importer would change the source note count before mapping; import was stopped.", "NBS 导入器在映射前已改变源音符数量，已停止导入以避免静默丢音。").into());
                        return;
                    }
                    let name = asset.name.clone();
                    crate::commands::load_midi_for_import(state, midi, name);
                    let t = &mut state.transcription;
                    t.stop_preview();
                    t.import_from_preview = true;
                }
                Err(error) => t.error = Some(error),
            }
        }
    }
}

/// Starts the preview stream, optionally at a scrubbed position. The player keeps the
/// decoded SoundFont and MIDI between requests, so only the first start pays for the load.
fn start_playback(state: &mut EditorState, from_seconds: f64, start_paused: bool) {
    let t = &mut state.transcription;
    let Some(asset) = &t.preview else {
        return;
    };
    let font = PathBuf::from(t.soundfont_path.trim());
    if !font.is_file() {
        t.error = Some(
            tr(
                "Choose a GM SoundFont (.sf2) for MIDI playback. NBS samples are not used.",
                "请先选择用于 MIDI 试听的 GM SoundFont（.sf2），不会使用 NBS 采样代替。",
            )
            .into(),
        );
        return;
    }
    let from = from_seconds.clamp(0.0, asset.data.duration_seconds.max(0.0));
    let bytes = asset.bytes.clone();
    let device = state.audio.output_name();
    state.audio.clock_mut().pause();
    t.scrub = None;
    let player = t.player.get_or_insert_with(MidiPreviewPlayer::new);
    player.set_volume(t.volume);
    match player.play_at(bytes, font, device, from, start_paused) {
        Ok(()) => t.error = None,
        Err(error) => t.error = Some(error),
    }
}

pub fn selected_file(state: &mut EditorState, action: DialogAction, path: PathBuf) {
    let t = &mut state.transcription;
    match action {
        DialogAction::TranscriptionAudio => t.audio_path = path.to_string_lossy().into_owned(),
        DialogAction::TranscriptionModel => t.model_path = path.to_string_lossy().into_owned(),
        DialogAction::TranscriptionSoundFont => {
            t.stop_preview();
            t.soundfont_path = path.to_string_lossy().into_owned();
        }
        DialogAction::TranscriptionMidi => {
            if !t.busy() {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                t.open_preview(path, name, Vec::new());
            }
        }
        DialogAction::SaveTranscriptionMidi => {
            if !midi_extension(&path) {
                t.error = Some(
                    tr(
                        "Save MIDI with a .mid or .midi extension.",
                        "请使用 .mid 或 .midi 扩展名保存，不能覆盖源音频。",
                    )
                    .into(),
                );
            } else if t.preview.is_some() && path.exists() {
                state.modals = crate::state::ModalQueue::question(
                    tr("Replace MIDI file?", "覆盖 MIDI 文件？"),
                    &format!(
                        "{}\n{}",
                        path.display(),
                        tr(
                            "This file already exists. Replace it with the previewed MIDI?",
                            "文件已存在，是否用当前预览的 MIDI 覆盖？"
                        )
                    ),
                    Command::Transcription(TranscriptionAction::SaveMidiConfirmed(path)),
                );
            } else {
                save_preview(t, &path);
            }
        }
        _ => return,
    }
    state.file_dialog = None;
    state.window = Some(WindowKind::Transcription);
}

fn midi_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
}

fn save_preview(state: &mut TranscriptionState, path: &Path) {
    let Some(asset) = &state.preview else {
        return;
    };
    if !midi_extension(path) {
        state.error = Some(
            tr(
                "The output must be a MIDI file.",
                "输出文件必须使用 MIDI 扩展名。",
            )
            .into(),
        );
        return;
    }
    let result = (|| -> Result<(), String> {
        use std::io::Write;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let temporary = path.with_file_name(format!(".nbs-midi-{stamp}.tmp"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let written = file.write_all(&asset.bytes).and_then(|_| file.sync_all());
        drop(file);
        let result = written.and_then(|_| std::fs::rename(&temporary, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => {
            state.error = None;
            state.status = format!(
                "{} {}",
                tr("Saved original MIDI:", "已保存原始 MIDI："),
                path.display()
            );
        }
        Err(error) => state.error = Some(error),
    }
}

pub fn show(state: &mut EditorState, ui: &mut egui::Ui) {
    let mut actions = Vec::new();
    let player_mode = state.player;
    {
        let t = &mut state.transcription;
        let busy = t.busy();
        ui.label(tr(
            "Audio → MIDI file → independent preview → NBS",
            "音频 → MIDI 文件 → 独立预览 → NBS",
        ));
        ui.small(tr("Nothing is written to the current song until you confirm in the NBS import window. Closing this panel does not cancel a task.", "只有在 NBS 导入设置中确认后才会改动当前曲谱；关闭本面板不会取消任务。"));
        ui.separator();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Audio", "音频"));
                ui.add(egui::TextEdit::singleline(&mut t.audio_path).desired_width(420.0));
                if ui.button(tr("Browse…", "选择…")).clicked() { actions.push(Command::OpenDialog(DialogAction::TranscriptionAudio)); }
            });
            ui.horizontal(|ui| {
                ui.label(tr("Model", "模型"));
                egui::ComboBox::from_id_salt("transcription_model").selected_text(&t.options.model).show_ui(ui, |ui| {
                    for value in ["small", "medium", "large"] { ui.selectable_value(&mut t.options.model, value.into(), value); }
                });
                ui.label(tr("Device", "计算设备"));
                egui::ComboBox::from_id_salt("transcription_device").selected_text(&t.options.device).show_ui(ui, |ui| {
                    for value in ["auto", "cpu", "cuda"] { ui.selectable_value(&mut t.options.device, value.into(), value); }
                });
                ui.label(tr("Weights", "权重精度"));
                egui::ComboBox::from_id_salt("transcription_dtype").selected_text(&t.options.dtype).show_ui(ui, |ui| {
                    for value in ["auto", "float32", "float16"] { ui.selectable_value(&mut t.options.dtype, value.into(), value); }
                });
            });
            if t.options.model == "large" {
                ui.colored_label(Color32::from_rgb(180, 110, 20), tr("Large has about 5.47 GB of FP32 weights. Loading can need more memory; a 4 GiB GPU is insufficient even when requesting FP16 conversion.", "large 的 FP32 权重约 5.47 GB，加载峰值更高；4 GiB 显卡不足以直接加载，指定 FP16 也不会绕过加载峰值。"));
            }
            egui::CollapsingHeader::new(tr("Advanced recognition settings", "高级识别设置")).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("Local model (optional)", "本地权重（可选）"));
                    ui.add(egui::TextEdit::singleline(&mut t.model_path).desired_width(310.0));
                    if ui.button(tr("Choose weights…", "选择权重…")).clicked() { actions.push(Command::OpenDialog(DialogAction::TranscriptionModel)); }
                });
                ui.small(tr("A local model needs its matching config.json. Leave blank to use the workspace model cache or the official download.", "本地权重需配套 config.json；留空时使用工作区模型缓存或官方下载。"));
                ui.horizontal(|ui| {
                    ui.label(tr("Instrument constraint", "限定乐器"));
                    ui.add(egui::TextEdit::singleline(&mut t.instruments).desired_width(330.0).hint_text("acoustic_piano,voice"));
                });
                ui.small(tr("Comma-separated group names; blank = automatic. Examples: acoustic_piano, acoustic_guitar, violin, voice, drums. Wrong constraints suppress real instruments.", "组名以逗号分隔，留空=自动。例：acoustic_piano、acoustic_guitar、violin、voice、drums；选错会屏蔽真实乐器。"));
                ui.horizontal(|ui| {
                    ui.label(tr("Beam", "束宽"));
                    ui.add(egui::DragValue::new(&mut t.options.beam_size).range(1..=4));
                    ui.label("CFG");
                    ui.add(egui::DragValue::new(&mut t.options.cfg_coef).range(0.0..=3.0).speed(0.1));
                    ui.checkbox(&mut t.options.use_sampling, tr("Sampling", "随机采样"));
                    ui.add_enabled(t.options.use_sampling && t.options.beam_size == 1, egui::DragValue::new(&mut t.options.temperature).range(0.0..=2.0).speed(0.1).prefix("T "));
                });
                ui.small(tr("Greedy is the default. Beam/CFG can use more memory and are not guaranteed to improve accuracy. Cross-chunk ties stay enabled; incomplete decoding is rejected.", "默认贪心；束搜索/CFG 可能增加显存，不能保证更准。保留跨块延音衔接，拒绝未完整结束的识别。"));
            });
        });
        egui::CollapsingHeader::new(tr("Local runtime and model downloads", "本地环境与模型下载")).default_open(!t.runtime.installed() || !t.accepted_downloads).show(ui, |ui| {
            ui.small(format!("{} {}", tr("All Python, build files and caches:", "Python、构建文件与全部缓存："), t.runtime.root.display()));
            ui.add_enabled_ui(!busy, |ui| {
                ui.checkbox(&mut t.accepted_downloads, tr("Allow local downloads; I confirm non-commercial use under the model's CC BY-NC 4.0 license.", "允许本地下载，并确认按模型 CC BY-NC 4.0 许可用于非商业用途。"));
                ui.checkbox(&mut t.install_cuda, tr("Install NVIDIA CUDA runtime (larger download)", "安装 NVIDIA CUDA 运行时（下载量较大）"));
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(t.accepted_downloads, egui::Button::new(tr("Prepare environment", "准备环境"))).clicked() { actions.push(Command::Transcription(TranscriptionAction::PrepareRuntime)); }
                    if ui.add_enabled(t.runtime.installed(), egui::Button::new(tr("Check environment", "检测环境"))).clicked() { actions.push(Command::Transcription(TranscriptionAction::Probe)); }
                    if ui.add_enabled(t.accepted_downloads && t.runtime.installed(), egui::Button::new(tr("Download / check model", "下载／检查模型"))).clicked() { actions.push(Command::Transcription(TranscriptionAction::PrepareModel)); }
                    ui.hyperlink_to(tr("Model license", "模型许可"), "https://huggingface.co/MuScriptor");
                });
            });
            ui.small(tr("Requires uv on PATH. First setup downloads several GB. Official Hugging Face weights require accepting the model license and authentication; a local safetensors file may also be selected.", "需要 PATH 中可用的 uv；首次配置可能下载数 GB。Hugging Face 官方权重需要接受许可并认证，也可选择本地 safetensors 文件。"));
            ui.small(tr("Transcription calls the third-party MuScriptor package (MIT, Kyutai x Mirelo; license text: crates/nbs-io/MUSCRIPTOR-LICENSE). Its CC BY-NC 4.0 weights are downloaded onto this machine and are never redistributed with this app.", "识别功能调用第三方 MuScriptor 包（MIT，Kyutai x Mirelo；许可全文见 crates/nbs-io/MUSCRIPTOR-LICENSE）。其 CC BY-NC 4.0 权重只下载到本机，不随本应用分发。"));
            if let Some(info) = &t.info {
                ui.small(format!("Python {} · PyTorch {} · MuScriptor {} · {}", info.python_version, info.torch_version, info.muscriptor_version, info.gpu_name.as_deref().unwrap_or("CPU")));
                if let Some(bytes) = info.gpu_memory_bytes { ui.small(format!("{} {:.1} GiB", tr("GPU memory", "显存"), bytes as f64 / 1024f64.powi(3))); }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !busy
                        && t.runtime.installed()
                        && t.accepted_downloads
                        && !t.audio_path.trim().is_empty(),
                    egui::Button::new(tr("Transcribe to MIDI", "识别并生成 MIDI")),
                )
                .clicked()
            {
                actions.push(Command::Transcription(TranscriptionAction::Run));
            }
            if ui
                .add_enabled(busy, egui::Button::new(tr("Cancel task", "取消任务")))
                .clicked()
            {
                actions.push(Command::Transcription(TranscriptionAction::Cancel));
            }
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(tr("Open MIDI preview…", "打开 MIDI 预览…")),
                )
                .clicked()
            {
                actions.push(Command::OpenDialog(DialogAction::TranscriptionMidi));
            }
        });
        if busy {
            ui.spinner();
        }
        if !t.status.is_empty() {
            ui.label(&t.status);
        }
        if let Some(progress) = t.progress {
            ui.add(egui::ProgressBar::new(progress).show_percentage());
        }
        if busy && t.note_count > 0 {
            ui.small(format!(
                "{} {}",
                t.note_count,
                tr("raw note onsets", "个原始起音事件")
            ));
        }
        if let Some(error) = &t.error {
            ui.colored_label(Color32::from_rgb(195, 55, 45), error);
        }
        if !t.messages.is_empty() {
            egui::CollapsingHeader::new(tr("Task log", "任务日志")).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(100.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &t.messages {
                            ui.small(line);
                        }
                    });
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("GM SoundFont");
            if ui
                .add(egui::TextEdit::singleline(&mut t.soundfont_path).desired_width(365.0))
                .changed()
            {
                t.stop_preview();
            }
            if ui.button(tr("Choose SF2…", "选择 SF2…")).clicked() {
                actions.push(Command::OpenDialog(DialogAction::TranscriptionSoundFont));
            }
        });
        ui.small(tr("MIDI uses the selected GM sound bank, not Minecraft sounds. No beat correction, quantization or gap filling is applied to the transcription.", "MIDI 使用所选 GM 音色库，而非音符盒声音；转录不自动做节拍校正、网格量化或补延音。"));
        if let Some(asset) = &t.preview {
            ui.label(format!(
                "{} · {} {} · {} {} · {:.2} s",
                asset.name,
                asset.data.tracks.len(),
                tr("tracks", "轨"),
                asset.data.notes.len(),
                tr("notes", "音符"),
                asset.data.duration_seconds
            ));
            let playback = t.player.as_ref().map(MidiPreviewPlayer::snapshot);
            let duration = asset.data.duration_seconds.max(0.0);
            let live = playback
                .as_ref()
                .map(|p| p.position_seconds)
                .unwrap_or(0.0)
                .clamp(0.0, duration);
            let mut at = t.scrub.unwrap_or(live);
            piano_roll(ui, &asset.data, at);
            let scrub = ui.add_enabled(
                !busy && duration > 0.0,
                egui::Slider::new(&mut at, 0.0..=duration)
                    .text(tr("Position", "播放位置"))
                    .custom_formatter(|seconds, _| clock_position(seconds)),
            );
            if scrub.changed() {
                if scrub.dragged() {
                    t.scrub = Some(at);
                } else {
                    actions.push(Command::Transcription(TranscriptionAction::Seek(at)));
                }
            } else if scrub.drag_stopped() {
                if let Some(target) = t.scrub.take() {
                    actions.push(Command::Transcription(TranscriptionAction::Seek(target)));
                }
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("Play from start", "从头试听")))
                    .clicked()
                {
                    actions.push(Command::Transcription(TranscriptionAction::Play));
                }
                let active = playback
                    .as_ref()
                    .map(|p| {
                        matches!(
                            p.state,
                            MidiPlaybackState::Playing | MidiPlaybackState::Paused
                        )
                    })
                    .unwrap_or(false);
                let paused = playback
                    .as_ref()
                    .map(|p| p.state == MidiPlaybackState::Paused)
                    .unwrap_or(false);
                if ui
                    .add_enabled(
                        active,
                        egui::Button::new(if paused {
                            tr("Resume", "继续")
                        } else {
                            tr("Pause", "暂停")
                        }),
                    )
                    .clicked()
                {
                    actions.push(Command::Transcription(TranscriptionAction::PauseResume));
                }
                if ui.button(tr("Stop preview", "停止试听")).clicked() {
                    actions.push(Command::Transcription(TranscriptionAction::Stop));
                }
                if ui
                    .add(
                        egui::Slider::new(&mut t.volume, 0.0..=1.0)
                            .text(tr("Preview volume", "试听音量")),
                    )
                    .changed()
                {
                    if let Some(player) = &t.player {
                        player.set_volume(t.volume);
                    }
                }
            });
            if let Some(playback) = playback {
                ui.small(format!(
                    "{:?} · {} / {}",
                    playback.state,
                    clock_position(playback.position_seconds),
                    clock_position(playback.duration_seconds.max(duration))
                ));
                if let Some(error) = playback.error {
                    ui.colored_label(Color32::from_rgb(195, 55, 45), error);
                }
            }
            egui::CollapsingHeader::new(tr("Tracks and conversion warnings", "轨道与转换提示"))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(125.0)
                        .show(ui, |ui| {
                            for (i, track) in asset.data.tracks.iter().enumerate() {
                                ui.small(format!(
                                    "{} · {} · {} {} · Program {:?}",
                                    i + 1,
                                    track.name,
                                    track.note_count,
                                    tr("notes", "音符"),
                                    track.programs
                                ));
                            }
                            for warning in asset.data.warnings.iter().chain(&asset.warnings) {
                                ui.small(warning);
                            }
                        });
                });
            if let Some(reason) = &asset.data.import_blocked_reason {
                ui.colored_label(Color32::from_rgb(195, 55, 45), reason);
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("Save MIDI…", "保存 MIDI…")).clicked() {
                    actions.push(Command::OpenDialog(DialogAction::SaveTranscriptionMidi));
                }
                if ui
                    .add_enabled(
                        !busy
                            && !player_mode
                            && !asset.data.notes.is_empty()
                            && asset.data.import_blocked_reason.is_none(),
                        egui::Button::new(tr("Continue to NBS import…", "继续转换为 NBS…")),
                    )
                    .clicked()
                {
                    actions.push(Command::Transcription(TranscriptionAction::Import));
                }
            });
            ui.small(format!(
                "{} {}",
                tr("MIDI file:", "MIDI 文件："),
                asset.path.display()
            ));
        }
    }
    for action in actions {
        state.emit(action);
    }
}

fn clock_position(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor().max(0.0) as u32;
    let rest = (seconds - minutes as f64 * 60.0).clamp(0.0, 59.95);
    format!("{minutes}:{rest:04.1}")
}

fn piano_roll(ui: &mut egui::Ui, midi: &MidiPreview, position_seconds: f64) {
    let width = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 125.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let duration = midi.duration_seconds.max(0.01);
    let min_key = midi.notes.iter().map(|n| n.key).min().unwrap_or(48) as f32;
    let max_key = midi.notes.iter().map(|n| n.key).max().unwrap_or(72) as f32;
    let rows = (max_key - min_key + 1.0).max(12.0);
    for i in 0..=8 {
        let x = rect.left() + rect.width() * i as f32 / 8.0;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(0.5_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }
    for note in midi.notes.iter().take(10000) {
        let left = rect.left() + rect.width() * (note.start_seconds / duration) as f32;
        let right = rect.left() + rect.width() * (note.end_seconds / duration) as f32;
        let top = rect.bottom() - (note.key as f32 - min_key + 1.0) / rows * rect.height();
        let colors = [
            Color32::from_rgb(60, 150, 215),
            Color32::from_rgb(80, 180, 120),
            Color32::from_rgb(210, 130, 60),
            Color32::from_rgb(175, 100, 190),
        ];
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(left, top),
                egui::pos2(
                    right.max(left + 1.5),
                    top + (rect.height() / rows - 0.5).max(1.0),
                ),
            ),
            0.0,
            colors[note.track % colors.len()],
        );
    }
    let playhead = rect.left()
        + rect.width() * (position_seconds / duration).clamp(0.0, 1.0) as f32;
    painter.line_segment(
        [
            egui::pos2(playhead, rect.top()),
            egui::pos2(playhead, rect.bottom()),
        ],
        Stroke::new(1.5_f32, Color32::from_rgb(240, 205, 90)),
    );
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );
    if midi.notes.len() > 10000 {
        ui.small(tr(
            "Drawing shows the first 10,000 notes; playback and saving keep all notes.",
            "画面最多显示前 10,000 个音符，试听与保存保留全部音符。",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn midi_bytes() -> Vec<u8> {
        let track = [
            0, 0xff, 0x51, 3, 7, 0xa1, 0x20, 0, 0xc0, 0, 0, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0,
            0, 0xff, 0x2f, 0,
        ];
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&track);
        bytes
    }

    #[test]
    fn midi_preview_to_import_settings_does_not_replace_song() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        let before = state.doc.song.total_blocks;
        let bytes = midi_bytes();
        state.transcription.preview = Some(MidiPreviewAsset {
            path: "original.mid".into(),
            name: "original.mid".into(),
            data: parse_midi_preview(&bytes).unwrap(),
            bytes,
            warnings: Vec::new(),
        });
        handle(&mut state, TranscriptionAction::Import);
        assert_eq!(state.window, Some(WindowKind::MidiImport));
        assert_eq!(state.doc.song.total_blocks, before);
        assert!(state.transcription.preview.is_some());
        assert!(state.midi_data.is_some());
    }

    /// 增强配器表是从「这一份 MIDI 的通道 program」派生的数据，不是用户偏好：
    /// 转录导入必须整套重算，否则上一首曲子留下的表会被拿来映射当前这首。
    #[test]
    fn transcription_import_rederives_enhanced_tables_from_the_current_midi() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        let mut previous = nbs_io::midi::MidiData::default();
        previous.channel_patch[0] = 40;
        crate::commands::load_midi_for_import(&mut state, previous, "previous.mid".to_string());
        state.midi_enhanced.enabled = true;

        let bytes = midi_bytes();
        state.transcription.preview = Some(MidiPreviewAsset {
            path: "original.mid".into(),
            name: "original.mid".into(),
            data: parse_midi_preview(&bytes).unwrap(),
            bytes: bytes.clone(),
            warnings: Vec::new(),
        });
        handle(&mut state, TranscriptionAction::Import);

        let current = nbs_io::midi::parse_midi(&bytes).unwrap();
        assert_eq!(current.channel_patch[0], 0);
        assert_eq!(
            state.midi_enhanced,
            nbs_io::midi_enhanced::EnhancedMidiOptions::for_midi(&current)
        );
        assert_eq!(
            state.midi_enhanced_options.channel_ins,
            state.midi_options.channel_ins
        );
    }

    #[test]
    fn cancelling_nbs_import_returns_to_the_unchanged_midi_preview() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        let before = state.doc.song.total_blocks;
        let bytes = midi_bytes();
        state.transcription.preview = Some(MidiPreviewAsset {
            path: "original.mid".into(),
            name: "original.mid".into(),
            data: parse_midi_preview(&bytes).unwrap(),
            bytes,
            warnings: Vec::new(),
        });
        handle(&mut state, TranscriptionAction::Import);
        crate::commands::dispatch(
            &mut state,
            Command::MidiImportCancel,
            &egui::Context::default(),
        );
        assert_eq!(state.window, Some(WindowKind::Transcription));
        assert!(state.transcription.preview.is_some());
        assert!(state.midi_data.is_none());
        assert_eq!(state.doc.song.total_blocks, before);
    }

    #[test]
    fn saving_midi_requires_confirmation_before_replacing_a_file() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        let bytes = midi_bytes();
        state.transcription.preview = Some(MidiPreviewAsset {
            path: "original.mid".into(),
            name: "original.mid".into(),
            data: parse_midi_preview(&bytes).unwrap(),
            bytes: bytes.clone(),
            warnings: Vec::new(),
        });
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nbs-midi-save-{stamp}"));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("copy.mid");
        std::fs::write(&path, b"keep existing file").unwrap();
        selected_file(
            &mut state,
            DialogAction::SaveTranscriptionMidi,
            path.clone(),
        );
        assert!(!state.modals.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep existing file");
        handle(
            &mut state,
            TranscriptionAction::SaveMidiConfirmed(path.clone()),
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let source_audio = dir.join("source.wav");
        std::fs::write(&source_audio, b"original audio").unwrap();
        selected_file(
            &mut state,
            DialogAction::SaveTranscriptionMidi,
            source_audio.clone(),
        );
        assert_eq!(std::fs::read(source_audio).unwrap(), b"original audio");
        assert!(state.transcription.error.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_audio_or_unconfirmed_downloads_do_not_start_a_task() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        let before = state.doc.song.total_blocks;
        handle(&mut state, TranscriptionAction::Run);
        assert!(state.transcription.task.is_none());
        assert!(state.transcription.error.is_some());
        state.transcription.accepted_downloads = true;
        handle(&mut state, TranscriptionAction::Run);
        assert!(state.transcription.task.is_none());
        assert_eq!(state.doc.song.total_blocks, before);
    }

    #[test]
    fn preview_window_renders_without_model_or_audio_device() {
        let _prefs = crate::isolate_prefs();
        let _frame = crate::frame_lock();
        let mut state = EditorState::for_test();
        state.window = Some(WindowKind::Transcription);
        let ctx = egui::Context::default();
        state.theme.apply(&ctx);
        crate::fonts::warm_up(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            crate::show(ctx, &mut state);
        });
        assert!(state.transcription.task.is_none());
        assert!(state.transcription.player.is_none());
    }
}
