use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const WORKER: &str = include_str!("muscriptor_worker.py");
const MUSCRIPTOR_VERSION: &str = "0.4.0";
static JOB_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptionOptions {
    pub model: String,
    pub model_path: Option<PathBuf>,
    pub device: String,
    pub dtype: String,
    pub instruments: Vec<String>,
    pub use_sampling: bool,
    pub temperature: f64,
    pub beam_size: u32,
    pub cfg_coef: f64,
    pub strict_eos: bool,
}

impl Default for TranscriptionOptions {
    fn default() -> Self {
        Self {
            model: "medium".into(),
            model_path: None,
            device: "auto".into(),
            dtype: "auto".into(),
            instruments: Vec::new(),
            use_sampling: false,
            temperature: 1.0,
            beam_size: 1,
            cfg_coef: 1.0,
            strict_eos: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TranscriptionRuntime {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeInfo {
    pub python_version: String,
    pub torch_version: String,
    pub muscriptor_version: String,
    pub cuda_available: bool,
    pub gpu_name: Option<String>,
    pub gpu_memory_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TranscriptionOutput {
    pub midi_path: PathBuf,
    pub events_path: PathBuf,
    pub raw_note_count: usize,
    pub model: String,
    pub device: String,
    pub dtype: String,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum TranscriptionEvent {
    Message(String),
    Progress {
        stage: String,
        message: String,
        completed: Option<u64>,
        total: Option<u64>,
        note_count: Option<u64>,
    },
    Ready(RuntimeInfo),
    ModelReady {
        model: String,
        device: String,
        dtype: String,
    },
    Complete(TranscriptionOutput),
    Failed(String),
    Cancelled,
}

impl TranscriptionEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Ready(_)
                | Self::ModelReady { .. }
                | Self::Complete(_)
                | Self::Failed(_)
                | Self::Cancelled
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerMessage {
    Progress {
        stage: String,
        #[serde(default)]
        message: String,
        completed: Option<u64>,
        total: Option<u64>,
        note_count: Option<u64>,
    },
    Ready(RuntimeInfo),
    ModelReady {
        model: String,
        device: String,
        dtype: String,
    },
    Result(TranscriptionOutput),
    Error {
        message: String,
    },
}

pub struct TranscriptionTask {
    receiver: Option<mpsc::Receiver<TranscriptionEvent>>,
    cancelled: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TranscriptionTask {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn drain(&self) -> Vec<TranscriptionEvent> {
        self.receiver
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default()
    }
}

impl Drop for TranscriptionTask {
    fn drop(&mut self) {
        self.cancel();
        self.receiver.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn workspace_root_from(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    dir.ancestors()
        .find(|ancestor| ancestor.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

impl TranscriptionRuntime {
    /// Anchored to the executable's workspace, not the launch directory: a double-clicked
    /// exe would otherwise build a second runtime tree and never find the weights already
    /// downloaded. CWD is only the fallback for binaries outside any Cargo.toml tree.
    pub fn in_workspace_directory() -> Self {
        let root = std::env::current_exe()
            .ok()
            .as_deref()
            .and_then(workspace_root_from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        Self {
            root: root.join(".work/transcription"),
        }
    }

    pub fn python_path(&self) -> PathBuf {
        self.root.join(if cfg!(windows) {
            "venv/Scripts/python.exe"
        } else {
            "venv/bin/python"
        })
    }

    pub fn installed(&self) -> bool {
        self.python_path().is_file()
    }

    pub fn default_soundfont(&self) -> PathBuf {
        self.root.join("soundfonts/preview.sf2")
    }

    pub fn cached_model(&self, model: &str) -> Option<PathBuf> {
        if !matches!(model, "small" | "medium" | "large") {
            return None;
        }
        let path = self
            .root
            .join("models")
            .join(model)
            .join("model.safetensors");
        path.is_file().then_some(path)
    }

    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        let paths = [
            ("UV_CACHE_DIR", "cache/uv"),
            ("UV_PYTHON_INSTALL_DIR", "python"),
            ("UV_TOOL_DIR", "tools"),
            ("UV_TOOL_BIN_DIR", "bin"),
            ("PIP_CACHE_DIR", "cache/pip"),
            ("PIP_CONFIG_FILE", "pip.ini"),
            ("HF_HOME", "cache/huggingface"),
            ("HF_HUB_CACHE", "cache/huggingface/hub"),
            ("HUGGINGFACE_HUB_CACHE", "cache/huggingface/hub"),
            ("HF_ASSETS_CACHE", "cache/huggingface/assets"),
            ("TORCH_HOME", "cache/torch"),
            ("CUDA_CACHE_PATH", "cache/cuda"),
            ("CUPY_CACHE_DIR", "cache/cupy"),
            ("TORCH_EXTENSIONS_DIR", "cache/torch_extensions"),
            ("TORCHINDUCTOR_CACHE_DIR", "cache/torchinductor"),
            ("TRITON_CACHE_DIR", "cache/triton"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("NUMBA_CACHE_DIR", "cache/numba"),
            ("MPLCONFIGDIR", "cache/matplotlib"),
            ("PYTHONPYCACHEPREFIX", "cache/pycache"),
            ("PYTHONUSERBASE", "cache/python-user"),
            ("TMP", "tmp"),
            ("TEMP", "tmp"),
            ("TMPDIR", "tmp"),
            ("HOME", "home"),
            ("USERPROFILE", "home"),
            ("APPDATA", "cache/appdata"),
            ("LOCALAPPDATA", "cache/localappdata"),
        ];
        let mut values: Vec<_> = paths
            .iter()
            .map(|(key, path)| (OsString::from(*key), self.root.join(path).into_os_string()))
            .collect();
        for (key, value) in [
            ("UV_NO_CONFIG", "1"),
            ("UV_NO_PROGRESS", "1"),
            ("UV_PYTHON_PREFERENCE", "only-managed"),
            ("PYTHONNOUSERSITE", "1"),
            ("PYTHONUNBUFFERED", "1"),
            ("PYTHONUTF8", "1"),
            ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
            ("NO_COLOR", "1"),
        ] {
            values.push((key.into(), value.into()));
        }
        values
    }

    pub fn prepare(&self, cuda: bool) -> TranscriptionTask {
        self.spawn(move |runtime, sender, cancel| {
            runtime.prepare_layout()?;
            if !runtime.installed() {
                send_message(sender, "Preparing workspace-local Python 3.12 / 正在准备工作区内的 Python 3.12");
                let mut command = Command::new("uv");
                // uv rejects --managed-python together with UV_PYTHON_PREFERENCE, which
                // configure() already sets to only-managed for every child process.
                command.args(["venv", "--python", "3.12"])
                    .arg(runtime.root.join("venv"));
                runtime.configure(&mut command);
                run_command(command, false, sender, cancel)?;
            }
            send_message(sender, "Installing MuScriptor and PyTorch locally / 正在本工作区安装 MuScriptor 与 PyTorch");
            let mut command = Command::new("uv");
            command.args(["pip", "install", "--python"])
                .arg(runtime.python_path())
                .args(["--torch-backend", if cuda { "cu128" } else { "cpu" }])
                .arg(format!("muscriptor=={MUSCRIPTOR_VERSION}"))
                .arg("torch==2.10.0");
            runtime.configure(&mut command);
            run_command(command, false, sender, cancel)?;
            runtime.run_worker("probe", None, None, sender, cancel)
        })
    }

    pub fn probe(&self) -> TranscriptionTask {
        self.spawn(|runtime, sender, cancel| {
            runtime.run_worker("probe", None, None, sender, cancel)
        })
    }

    pub fn prepare_model(&self, options: TranscriptionOptions) -> TranscriptionTask {
        self.spawn(move |runtime, sender, cancel| {
            runtime.run_worker("prepare_model", Some(options), None, sender, cancel)
        })
    }

    pub fn transcribe(&self, audio: PathBuf, options: TranscriptionOptions) -> TranscriptionTask {
        self.spawn(move |runtime, sender, cancel| {
            runtime.run_worker("transcribe", Some(options), Some(audio), sender, cancel)
        })
    }

    fn spawn<F>(&self, action: F) -> TranscriptionTask
    where
        F: FnOnce(
                &Self,
                &mpsc::SyncSender<TranscriptionEvent>,
                &AtomicBool,
            ) -> Result<WorkerMessage, String>
            + Send
            + 'static,
    {
        let runtime = self.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let (sender, receiver) = mpsc::sync_channel(128);
        let thread = std::thread::spawn(move || {
            let result = action(&runtime, &sender, &cancel);
            let terminal = if cancel.load(Ordering::Acquire) {
                TranscriptionEvent::Cancelled
            } else {
                match result {
                    Ok(WorkerMessage::Ready(info)) => TranscriptionEvent::Ready(info),
                    Ok(WorkerMessage::ModelReady {
                        model,
                        device,
                        dtype,
                    }) => TranscriptionEvent::ModelReady {
                        model,
                        device,
                        dtype,
                    },
                    Ok(WorkerMessage::Result(result)) => TranscriptionEvent::Complete(result),
                    Ok(WorkerMessage::Error { message }) | Err(message) => {
                        TranscriptionEvent::Failed(message)
                    }
                    Ok(WorkerMessage::Progress { .. }) => TranscriptionEvent::Failed(
                        "Worker did not complete / 转录进程没有完成结果".into(),
                    ),
                }
            };
            let _ = sender.send(terminal);
        });
        TranscriptionTask {
            receiver: Some(receiver),
            cancelled,
            thread: Some(thread),
        }
    }

    fn prepare_layout(&self) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        for (key, path) in self.environment() {
            if key == "PIP_CONFIG_FILE" {
                let path = PathBuf::from(path);
                if !path.exists() {
                    fs::write(path, b"").map_err(|e| e.to_string())?;
                }
            } else if Path::new(&path).starts_with(&self.root) {
                fs::create_dir_all(path).map_err(|e| e.to_string())?;
            }
        }
        fs::create_dir_all(self.root.join("jobs")).map_err(|e| e.to_string())
    }

    fn configure(&self, command: &mut Command) {
        command
            .current_dir(&self.root)
            .envs(self.environment())
            .env_remove("PYTHONHOME")
            .env_remove("PYTHONPATH")
            .env_remove("VIRTUAL_ENV")
            .env_remove("CONDA_PREFIX")
            .env_remove("CONDA_DEFAULT_ENV")
            .env_remove("KMP_DUPLICATE_LIB_OK")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
    }

    fn run_worker(
        &self,
        action: &str,
        options: Option<TranscriptionOptions>,
        audio: Option<PathBuf>,
        sender: &mpsc::SyncSender<TranscriptionEvent>,
        cancel: &AtomicBool,
    ) -> Result<WorkerMessage, String> {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled / 已取消".into());
        }
        if !self.installed() {
            return Err("Prepare the workspace-local Python environment first / 请先准备工作区内的 Python 环境".into());
        }
        self.prepare_layout()?;
        let sequence = JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let job = self.root.join("jobs").join(format!("{stamp}-{sequence}"));
        fs::create_dir(&job).map_err(|e| e.to_string())?;
        let mut request = if let Some(mut options) = options {
            if options.model_path.is_none() {
                options.model_path = self.cached_model(&options.model);
            }
            serde_json::to_value(options).map_err(|e| e.to_string())?
        } else {
            serde_json::json!({})
        };
        request["action"] = action.into();
        request["output_dir"] = serde_json::to_value(&job).map_err(|e| e.to_string())?;
        if let Some(audio) = audio {
            request["audio_path"] =
                serde_json::to_value(fs::canonicalize(audio).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
        }
        let request_path = job.join("request.json");
        let worker_path = job.join("worker.py");
        fs::write(
            &request_path,
            serde_json::to_vec(&request).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        fs::write(&worker_path, WORKER).map_err(|e| e.to_string())?;
        let mut command = Command::new(self.python_path());
        command.arg(&worker_path).arg(&request_path);
        self.configure(&mut command);
        let message = run_command(command, true, sender, cancel)?;
        if let WorkerMessage::Result(ref result) = message {
            for (actual, expected) in [
                (&result.midi_path, job.join("raw.mid")),
                (&result.events_path, job.join("events.jsonl")),
            ] {
                let actual = fs::canonicalize(actual).map_err(|e| e.to_string())?;
                let expected = fs::canonicalize(expected).map_err(|e| e.to_string())?;
                if actual != expected {
                    return Err(
                        "Unexpected worker output path / 转录进程返回了意外的输出路径".into(),
                    );
                }
            }
            let bytes = fs::read(&result.midi_path).map_err(|e| e.to_string())?;
            crate::midi_preview::parse_midi_preview(&bytes)?;
        }
        Ok(message)
    }
}

fn send_message(sender: &mpsc::SyncSender<TranscriptionEvent>, text: &str) {
    let text: String = text.chars().take(1600).collect();
    let _ = sender.try_send(TranscriptionEvent::Message(text));
}

fn output_reader<R: Read + Send + 'static>(
    reader: R,
    stdout: bool,
    sender: mpsc::SyncSender<(bool, String)>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            let mut text = String::new();
            match (&mut reader).take(16384).read_line(&mut text) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if sender.send((stdout, text.trim_end().to_owned())).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn terminate(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_command(
    mut command: Command,
    protocol: bool,
    sender: &mpsc::SyncSender<TranscriptionEvent>,
    cancel: &AtomicBool,
) -> Result<WorkerMessage, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Cancelled / 已取消".into());
    }
    let mut child = command.spawn().map_err(|e| format!("Cannot start local runtime (uv/Python): {e} / 无法启动本地运行环境，请检查 uv 和 Python 路径"))?;
    let (lines, receiver) = mpsc::sync_channel(64);
    output_reader(
        child.stdout.take().ok_or("Missing stdout")?,
        true,
        lines.clone(),
    );
    output_reader(
        child.stderr.take().ok_or("Missing stderr")?,
        false,
        lines.clone(),
    );
    drop(lines);
    let mut terminal = None;
    let mut recent = VecDeque::new();
    let mut status: Option<ExitStatus> = None;
    loop {
        if cancel.load(Ordering::Acquire) {
            terminate(&mut child);
            return Err("Cancelled / 已取消".into());
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok((stdout, line)) => {
                if protocol && stdout {
                    if let Ok(message) = serde_json::from_str::<WorkerMessage>(&line) {
                        match message {
                            WorkerMessage::Progress {
                                stage,
                                message,
                                completed,
                                total,
                                note_count,
                            } => {
                                let _ = sender.try_send(TranscriptionEvent::Progress {
                                    stage,
                                    message,
                                    completed,
                                    total,
                                    note_count,
                                });
                            }
                            message => terminal = Some(message),
                        }
                        continue;
                    }
                }
                if !line.is_empty() {
                    let clipped: String = line.chars().take(1600).collect();
                    send_message(sender, &clipped);
                    recent.push_back(clipped);
                    if recent.len() > 10 {
                        recent.pop_front();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => match child.try_wait() {
                Ok(Some(code)) => {
                    status = Some(code);
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => {
                    terminate(&mut child);
                    return Err(error.to_string());
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(next) => status = next,
                Err(error) => {
                    terminate(&mut child);
                    return Err(error.to_string());
                }
            }
        }
    }
    let status = status
        .map(Ok)
        .unwrap_or_else(|| child.wait())
        .map_err(|e| e.to_string())?;
    if let Some(WorkerMessage::Error { message }) = terminal {
        return Err(message);
    }
    if !status.success() {
        return Err(format!(
            "Local process exited with {status} / 本地进程失败\n{}",
            recent.into_iter().collect::<Vec<_>>().join("\n")
        ));
    }
    if protocol {
        terminal.ok_or_else(|| {
            "Worker exited without a complete result / 转录进程退出，但没有完整结果".into()
        })
    } else {
        Ok(WorkerMessage::Progress {
            stage: "setup".into(),
            message: String::new(),
            completed: None,
            total: None,
            note_count: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_python_caches_and_build_directories_stay_in_runtime_root() {
        let root = std::env::temp_dir().join("nbs-transcription-env-contract");
        let runtime = TranscriptionRuntime { root: root.clone() };
        let env = runtime.environment();
        for (key, value) in &env {
            let key = key.to_string_lossy();
            if key.ends_with("_DIR")
                || key.ends_with("_HOME")
                || key.ends_with("_CACHE")
                || [
                    "TMP",
                    "TEMP",
                    "TMPDIR",
                    "HOME",
                    "USERPROFILE",
                    "APPDATA",
                    "LOCALAPPDATA",
                    "PYTHONPYCACHEPREFIX",
                    "PYTHONUSERBASE",
                    "PIP_CONFIG_FILE",
                ]
                .contains(&key.as_ref())
            {
                assert!(Path::new(value).starts_with(&root), "{key}: {value:?}");
            }
        }
        assert!(env.iter().any(|(key, _)| key == "UV_PYTHON_INSTALL_DIR"));
        assert!(env.iter().any(|(key, _)| key == "TORCH_EXTENSIONS_DIR"));
        assert!(env.iter().any(|(key, _)| key == "HF_HUB_CACHE"));
    }

    #[test]
    fn transcription_defaults_preserve_raw_timing_and_sequential_decoding() {
        let options = TranscriptionOptions::default();
        assert_eq!(options.model, "medium");
        assert!(!options.use_sampling);
        assert_eq!(options.beam_size, 1);
        assert_eq!(options.cfg_coef, 1.0);
        assert!(options.strict_eos);
        let value = serde_json::to_value(options).unwrap();
        assert!(value.get("quantize").is_none());
        assert!(value.get("detect_tempo").is_none());
        assert!(value.get("batch_size").is_none());
    }

    #[test]
    fn probe_and_completion_messages_are_distinct() {
        let ready = r#"{"type":"ready","python_version":"3.12","torch_version":"2.10","muscriptor_version":"0.4.0","cuda_available":false,"gpu_name":null,"gpu_memory_bytes":null}"#;
        assert!(matches!(
            serde_json::from_str::<WorkerMessage>(ready).unwrap(),
            WorkerMessage::Ready(_)
        ));
        assert!(serde_json::from_str::<WorkerMessage>(r#"{"type":"result"}"#).is_err());
        assert!(!TranscriptionEvent::Message("ready".into()).is_terminal());
        assert!(TranscriptionEvent::Cancelled.is_terminal());
    }

    fn shell_command(windows: &str, unix: &str) -> Command {
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            let mut command = Command::new("cmd.exe");
            command
                .args(["/D", "/C"])
                .raw_arg(windows)
                .creation_flags(0x0800_0000);
            let _ = unix;
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", unix]);
            let _ = windows;
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    #[test]
    fn successful_exit_without_a_result_cannot_be_imported() {
        let (tx, _rx) = mpsc::sync_channel(16);
        let result = run_command(
            shell_command("exit 0", "exit 0"),
            true,
            &tx,
            &AtomicBool::new(false),
        );
        assert!(result.unwrap_err().contains("without a complete result"));
    }

    #[test]
    fn successful_worker_ready_message_is_accepted() {
        let payload = r#"{"type":"ready","python_version":"3.12","torch_version":"2.10","muscriptor_version":"0.4.0","cuda_available":false,"gpu_name":null,"gpu_memory_bytes":null}"#;
        let (tx, _rx) = mpsc::sync_channel(16);
        let result = run_command(
            shell_command(
                &format!("echo {payload}"),
                &format!("printf '%s\\n' '{payload}'"),
            ),
            true,
            &tx,
            &AtomicBool::new(false),
        );
        assert!(matches!(result.unwrap(), WorkerMessage::Ready(_)));
    }

    #[test]
    fn protocol_error_preserves_the_actionable_message() {
        let (tx, _rx) = mpsc::sync_channel(16);
        let result = run_command(
            shell_command(
                "echo {\"type\":\"error\",\"message\":\"model unavailable\"} & exit /b 3",
                "printf '%s\\n' '{\"type\":\"error\",\"message\":\"model unavailable\"}'; exit 3",
            ),
            true,
            &tx,
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap_err(), "model unavailable");
    }

    #[test]
    fn late_process_failure_overrides_a_ready_message() {
        let payload = r#"{"type":"ready","python_version":"3.12","torch_version":"2.10","muscriptor_version":"0.4.0","cuda_available":false,"gpu_name":null,"gpu_memory_bytes":null}"#;
        let (tx, _rx) = mpsc::sync_channel(16);
        let result = run_command(
            shell_command(
                &format!("echo {payload} & exit /b 7"),
                &format!("printf '%s\\n' '{payload}'; exit 7"),
            ),
            true,
            &tx,
            &AtomicBool::new(false),
        );
        assert!(result.unwrap_err().contains("process exited"));
    }

    #[test]
    fn cancelled_process_is_stopped_without_a_result() {
        let (tx, _rx) = mpsc::sync_channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let result = run_command(
            shell_command("ping -n 20 127.0.0.1 >nul", "exec sleep 20"),
            true,
            &tx,
            &cancel,
        );
        setter.join().unwrap();
        assert!(result.unwrap_err().contains("Cancelled"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn model_cache_never_accepts_path_segments_as_model_names() {
        let runtime = TranscriptionRuntime {
            root: std::env::temp_dir(),
        };
        assert!(runtime.cached_model("../outside").is_none());
        assert!(runtime.cached_model("MEDIUM").is_none());
    }

    #[test]
    fn workspace_root_is_the_nearest_manifest_above_the_executable() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nbs-workspace-root-{stamp}"));
        let exe = dir.join("target").join("debug").join("nbs-rust.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(dir.join("target").join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            workspace_root_from(&exe).as_deref(),
            Some(dir.join("target").as_path())
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
