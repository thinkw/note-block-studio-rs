//! 音频引擎：cpal 实时输出 + 离线导出。
//!
//! 实时路径：`cpal` 回调 → `SharedState::render_interleaved` → 同一套 Mixer。
//! 离线路径：同一套 `Voice`/`playback_ratio` 公式，按音符分片多线程混音；
//! 时间轴/限幅/声像按打包版 nbswave 0.4.0 的语义来（`tempo_segments` 定位、
//! 仅峰值超 1 时整轨归一），因此导出的 WAV 与原版「导出音频」一致。

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nbs_core::model::{Note, Song, PAN_CENTER};

use crate::mixer::Voice;
use crate::samples::{SoundBank, BUILTIN_FILES, SAMPLE_RATE};
use crate::scheduler::{
    build_event_table, make_shared, EventKind, NoteEvent, Shared, TransportClock, VisualEvent,
};

// 注意：cpal 在 Windows/WASAPI 后端的 `Stream` 不是 Send（内部持有线程亲和的
// COM 对象）。引擎只在创建它的主线程使用，因此这里不要求 Send。
pub trait AudioEngine {
    fn name(&self) -> &str;
    /// 载入歌曲（构建事件表，必要时补载自定义采样）。
    fn prepare(&mut self, song: &Song);
    /// 原版 `play_sound()`：不动播放头，立刻用当前音色试听一个音。
    /// 除了 `layerhov_vppreview` 打开时的滚轮预览，原版所有试听调用传的都是 100/100。
    fn preview_note(&mut self, note: &Note) {
        self.preview_note_vp(note, 100, 100);
    }
    /// 原版 `play_sound(ins, key, vol, pan, pit)` 的 vol/pan 两个入参：
    /// 按 `control_draw.gml:256-263`，它们是「图层音量 / 图层声道」，
    /// 与音符自身的力度、声像一起进 [`crate::mixer::Voice::gains`]。
    fn preview_note_vp(&mut self, note: &Note, layer_volume: u8, layer_stereo: u8);
    /// 原版 `play_sound(soundmetronome, 45, 100, 100, 0)`：速度测量器打点那一声。
    /// 它不走乐器表，用的是独立采样（`crate::samples::SoundBank::metronome`）。
    fn preview_metronome(&mut self, key: u8);
    fn start(&mut self, from_tick: f64);
    fn stop(&mut self);
    fn is_playing(&self) -> bool;
    fn position_ticks(&self) -> f64;
    fn advance(&mut self, dt: f64);
    fn active_voices(&self) -> usize;
    fn export_wav(&mut self, path: &Path) -> bool {
        let _ = path;
        false
    }
    fn clock(&self) -> &TransportClock;
    fn clock_mut(&mut self) -> &mut TransportClock;
    fn device_info(&self) -> String;
    /// 系统里可选的输出设备。原版把声音整个交给 GameMaker，只会走系统默认设备，
    /// 没有选择项；这一组方法是端口自加（用户要求 #94）。没有可枚举后端的实现返回空表。
    /// 实现方**不许在这里等系统枚举**：界面每开一次下拉就调一次，慢的话整个界面跟着卡。
    fn output_devices(&self) -> Vec<OutputDevice> {
        Vec::new()
    }
    /// 设备表还在路上（还没扫完过，或正有一趟扫描在跑）：下拉要显示「正在扫描…」，
    /// 别把「还不知道」画成「这台机器没有输出设备」。
    fn output_scan_pending(&self) -> bool {
        false
    }
    /// 当前真正在出声的设备名；降级成静音时钟时 `None`。
    fn output_name(&self) -> Option<String> {
        None
    }
    /// 用户点名的设备（`None` = 跟随系统默认）。切换失败被回落后会跟着变回 `None`，
    /// 界面用它把偏好写成和引擎一致，免得下次启动又去点那台已经不存在的设备。
    fn wanted_output(&self) -> Option<&str> {
        None
    }
    /// 换输出设备，`None` = 跟随系统默认。请求派给后台线程后立即返回，
    /// 真正接上了哪台由 [`Self::poll_output`] 带回来（拿指定设备的句柄要过一遍系统
    /// 枚举，实测 0.5~1.3 秒，界面一秒都不能等）。`Err` 只表示这个引擎没有可切换的设备。
    fn set_output(&mut self, _wanted: Option<String>) -> Result<(), String> {
        Err(nbs_core::tr(
            "this audio engine has no switchable output device",
            "当前音频引擎没有可切换的输出设备",
        )
        .to_string())
    }
    /// 有一趟切换还在后台跑（点了到结果落地之间）。界面用它显示「正在连接」，
    /// 别拿上一台的旧名字装作已经切好了。
    fn output_connecting(&self) -> bool {
        false
    }
    /// 系统默认输出变了（插耳机、切蓝牙）就跟随过去。内部限流，每帧调用即可；
    /// 也在这里取走后台建流的结果。返回换到的新设备名，没变化返回 `None`。
    fn poll_output(&mut self) -> Option<String> {
        None
    }
    /// 取走一次「后台建流失败」的告示（同一趟只报一次）。切换不再当场有结果，
    /// 失败原因只能这样延后交给状态栏。
    fn take_output_error(&mut self) -> Option<String> {
        None
    }
    fn bank(&self) -> Arc<SoundBank>;
    fn sample_status(&self) -> String;
    /// 取走改界面的事件乐器副作用（彩虹 / 改色 / 存档弹窗 / 背景强调）。
    /// 没有调度器的实现永远为空。
    fn take_visual_events(&mut self) -> Vec<VisualEvent> {
        Vec::new()
    }
    /// 重新扫描采样目录（首选项改路径后调用）。
    fn reload_bank(&mut self, _dir: Option<&Path>) {}
}

/// 输出设备快照。设备名是系统给的（Windows 会按系统语言给「耳机 (Realtek(R) Audio)」
/// 这类名字），不能由我们翻译；句式文案留到显示时按当前语言拼。
enum DeviceReport {
    Streamed {
        name: String,
        rate: u32,
        channels: usize,
    },
    /// 降级为静音时钟的原因（cpal 的报错串，按技术原文显示）。
    Fallback(String),
    /// 有一趟切换在后台跑。这时候上一台的流已经被放掉、新的还没建好，
    /// 不能拿旧名字装作已经切过去了。
    Connecting,
    /// 还没尝试建流。
    Uninitialised,
}
/// 一台可选的输出设备。cpal 0.15 不对外暴露稳定设备 ID，只能用设备名当标识
/// （Windows 给的名字自带设备描述，如「耳机 (Realtek(R) Audio)」，实际重名概率极低；
/// 真撞上同名就选枚举到的第一台）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    pub name: String,
    pub is_default: bool,
}

/// 枚举系统里的输出设备。拿不到后端时返回空表（不报错：没设备只是没得选）。
pub fn list_output_devices() -> Vec<OutputDevice> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut out: Vec<OutputDevice> = Vec::new();
    for device in devices {
        let Ok(name) = device.name() else { continue };
        if out.iter().any(|o| o.name == name) {
            continue;
        }
        out.push(OutputDevice {
            is_default: Some(&name) == default_name.as_ref(),
            name,
        });
    }
    out
}

/// 系统当前的默认输出设备名（一台设备都没有时 `None`）。
fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
}

/// 取一台输出设备：`wanted` 为空就拿系统默认；指定名字时先比对默认（省一次枚举），
/// 再全表按名字找。找不到报错，由调用方决定回落策略。
fn open_output_device(wanted: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    if let Some(device) = host.default_output_device() {
        if wanted.is_none() || device.name().ok().as_deref() == wanted {
            return Ok(device);
        }
    } else if wanted.is_none() {
        return Err("no default output device".to_string());
    }
    let wanted = wanted.unwrap_or_default();
    let devices = host
        .output_devices()
        .map_err(|e| format!("failed to enumerate output devices: {e}"))?;
    for device in devices {
        if device.name().ok().as_deref() == Some(wanted) {
            return Ok(device);
        }
    }
    Err(format!("output device \"{wanted}\" is not available"))
}

/// 后台建流线程一趟做完后留下的结论。`cpal::Stream` 不在里面——它 !Send，
/// 只能留在干活的那条线程上（见 [`StreamWorker::spawn`]）。
pub struct StreamOutcome {
    /// 这一趟回答的是哪个请求。界面拿它和当前 `wanted` 比对，过期的结果直接丢掉。
    pub for_wanted: Option<String>,
    /// 真正接上的设备（名 / 采样率 / 声道数）；`None` = 这趟没出声。
    pub built: Option<(String, u32, usize)>,
    /// 建流失败的原因（cpal 报错原文，按技术原文显示）。
    pub error: Option<String>,
    /// 点名的设备没拿到，后台已经回落成跟随系统默认：界面要跟着把偏好改回去，
    /// 别存一个并没有生效的点名下标。
    pub fell_back: bool,
}

/// 启动那趟建流最多等多久。超时就先按「还没初始化」把窗口开出来，晚一点由
/// [`CpalEngine::poll_output`] 补上——宁可晚出声，不要打不开窗口。
const STARTUP_STREAM_WAIT: Duration = Duration::from_secs(15);

/// 建流干活的那条线程。为什么要搬到后台：实测在 Windows 上打开一台**非默认**输出设备
/// 要先过一遍全量枚举再建流，约 0.64 秒（默认设备只要几毫秒）。这一步以前在界面线程上
/// 同步跑完，点一下设备菜单整个窗口就僵在那里。
struct StreamWorker {
    request: mpsc::Sender<Option<String>>,
    /// 结果队列。`Receiver` 是 Send 不是 Sync，包进 Mutex 才放得进结构体。
    results: Mutex<mpsc::Receiver<StreamOutcome>>,
}

/// 建流那一步的注入点：收到「想要的设备名（`None` = 系统默认）」，返回这一趟的结论
/// 加上要由后台线程持有的流。用例往里塞假替身，就不必真去碰音频硬件。
type Connect =
    dyn Fn(&Shared, Option<String>) -> (StreamOutcome, Option<cpal::Stream>) + Send + 'static;

impl StreamWorker {
    fn spawn(shared: Shared, connect: Box<Connect>) -> Self {
        let (request, inbox) = mpsc::channel::<Option<String>>();
        let (reporter, outcomes) = mpsc::channel::<StreamOutcome>();
        std::thread::spawn(move || {
            // Stream 只能活在这条线程的局部变量里。每趟先把旧流放掉（cpal 的 Stream
            // 一 drop 就停声），否则重复建同一台设备时旧流还占着端点。
            let mut current: Option<cpal::Stream> = None;
            for wanted in inbox.iter() {
                drop(current.take());
                let (outcome, stream) = connect(&shared, wanted);
                current = stream;
                if reporter.send(outcome).is_err() {
                    break; // 引擎已经不在了，没必要继续占着这台设备
                }
            }
        });
        Self {
            request,
            results: Mutex::new(outcomes),
        }
    }

    /// 派一趟请求，立即返回。
    fn dispatch(&self, wanted: Option<String>) -> Result<(), String> {
        self.request
            .send(wanted)
            .map_err(|_| "audio stream worker stopped".to_string())
    }

    /// 取走已经回来的结果，只认最后一趟：请求按顺序处理，最后一条才是当前想要的状态。
    fn harvest(&self) -> Option<StreamOutcome> {
        let rx = slot_guard(&self.results);
        let mut latest = None;
        while let Ok(outcome) = rx.try_recv() {
            latest = Some(outcome);
        }
        latest
    }

    /// 等第一趟结果。只在启动时用——那时等和以前在界面线程上同步建流没两样，窗口还没出来。
    fn wait_first(&self) -> Option<StreamOutcome> {
        slot_guard(&self.results)
            .recv_timeout(STARTUP_STREAM_WAIT)
            .ok()
    }
}

/// 一趟建流：按 `wanted` 找设备、开流。点名的设备拿不到就回落到系统默认——宁可换设备
/// 出声，也不要因为一次选择把声音整个丢掉；连默认都没有才降级成静音时钟。
/// 真建出来的流不在这里保存，交给调用方（后台线程）持有。
fn connect_output(
    shared: &Shared,
    wanted: Option<String>,
) -> (StreamOutcome, Option<cpal::Stream>) {
    let (device, fell_back) = match pick_output_device(wanted.as_deref()) {
        Ok(picked) => picked,
        Err(reason) => return stream_failed(wanted, reason),
    };
    let (mut outcome, stream) = open_stream_on(&device, shared, wanted);
    outcome.fell_back = fell_back;
    (outcome, stream)
}

/// 这趟要用哪台设备，顺带说明是不是从用户点名的那台回落下来的。
fn pick_output_device(wanted: Option<&str>) -> Result<(cpal::Device, bool), String> {
    match open_output_device(wanted) {
        Ok(device) => Ok((device, false)),
        Err(e) => {
            if wanted.is_none() {
                return Err(e);
            }
            match open_output_device(None) {
                Ok(device) => Ok((device, true)),
                Err(e2) => Err(if e == e2 { e } else { format!("{e}; {e2}") }),
            }
        }
    }
}

fn stream_failed(
    for_wanted: Option<String>,
    error: String,
) -> (StreamOutcome, Option<cpal::Stream>) {
    (
        StreamOutcome {
            for_wanted,
            built: None,
            error: Some(error),
            fell_back: false,
        },
        None,
    )
}

/// 在一台具体设备上建输出流。**这里不留流**：`cpal::Stream` 不是 Send，只能由后台线程
/// 自己拿着，这里只把它连同结论一起交出去（原来它存在引擎字段里，于是每次换设备都得
/// 在界面线程上等这一趟跑完）。
fn open_stream_on(
    device: &cpal::Device,
    shared: &Shared,
    for_wanted: Option<String>,
) -> (StreamOutcome, Option<cpal::Stream>) {
    let name = device
        .name()
        .unwrap_or_else(|_| "unknown device".to_string());
    let supported = match device.default_output_config() {
        Ok(supported) => supported,
        Err(e) => return stream_failed(for_wanted, format!("failed to read output config: {e}")),
    };
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let rate = config.sample_rate.0;
    let channels = config.channels as usize;

    {
        let mut st = shared.lock();
        st.sample_rate = rate;
        st.mixer.output_rate = rate;
    }

    let cb_shared = shared.clone();
    let err_shared = shared.clone();
    let built = match sample_format {
        cpal::SampleFormat::F32 => device
            .build_output_stream::<f32, _, _>(
                &config,
                move |data, _| {
                    if let Some(mut st) = cb_shared.try_lock() {
                        st.render_interleaved(data, channels);
                    } else {
                        data.fill(0.0);
                    }
                },
                move |e| {
                    eprintln!("[nbs-audio] output stream error: {e}");
                    err_shared.lock().audio_active = false;
                },
                None,
            )
            .map_err(|e| format!("failed to build f32 output stream: {e}")),
        cpal::SampleFormat::I16 => {
            let mut scratch: Vec<f32> = Vec::new();
            let render_shared = shared.clone();
            device
                .build_output_stream::<i16, _, _>(
                    &config,
                    move |data, _| {
                        scratch.resize(data.len(), 0.0);
                        if let Some(mut st) = render_shared.try_lock() {
                            st.render_interleaved(&mut scratch, channels);
                        } else {
                            scratch.fill(0.0);
                        }
                        for (d, &s) in data.iter_mut().zip(scratch.iter()) {
                            *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                    },
                    move |e| {
                        eprintln!("[nbs-audio] output stream error: {e}");
                        err_shared.lock().audio_active = false;
                    },
                    None,
                )
                .map_err(|e| format!("failed to build i16 output stream: {e}"))
        }
        other => Err(format!("unsupported sample format {other:?}")),
    };
    let stream = match built {
        Ok(stream) => stream,
        Err(e) => return stream_failed(for_wanted, e),
    };
    if let Err(e) = stream.play() {
        return stream_failed(for_wanted, format!("failed to start output stream: {e}"));
    }
    shared.lock().audio_active = true;
    (
        StreamOutcome {
            for_wanted,
            built: Some((name, rate, channels)),
            error: None,
            fell_back: false,
        },
        Some(stream),
    )
}

/// 默认设备探测间隔：插耳机、切蓝牙到跟随切换的最大延迟。这一档只做便宜的事
/// （读系统默认设备名，本机实测 0.4~1.5 毫秒），不碰全量枚举。
const OUTPUT_PROBE: Duration = Duration::from_millis(1000);
/// 点名了某台设备时，保持设备表新鲜的下限间隔：把那台设备拔掉并不会改系统默认，
/// 便宜的那个探测看不出来，只能靠后台按这个间隔补扫（本机实测一趟 1.3 秒，在后台线程上）。
const OUTPUT_RESCAN: Duration = Duration::from_secs(20);

/// 后台扫描线程和界面线程交接的那份设备表。
#[derive(Default)]
struct ScanSlot {
    devices: Vec<OutputDevice>,
    /// 最近一趟扫描完成的时刻；`None` = 还没扫完过 —— 这时候的空表是「还不知道」，
    /// 不是「真的一台都没有」，不许拿它下结论。
    finished: Option<Instant>,
    /// 有一趟扫描正在路上：同一时间的重复请求并成一次。
    scanning: bool,
}

/// 输出设备扫描线程。cpal 列设备要过一遍系统的 `EnumAudioEndpoints`，本机（3 台端点）
/// 实测 1.26~1.52 秒，Realtek 那两台各要 0.5~0.7 秒，虚拟声卡多的机器只会更慢。
/// 这笔开销绝不能落在界面线程上：#94 头一版每秒枚举一次，实测每次 1.17~1.32 秒，
/// 整个界面每隔一秒僵死一次 —— 就是用户反馈的「处处都卡」（2026-11-19）。
/// 跨这条线的只有设备名（`String`）：cpal 的句柄内部是线程亲和的 COM 指针，
/// 拿句柄、建流都只能在真正要用它的那条线程上做（见 [`StreamWorker`]）。
struct DeviceScanner {
    request: mpsc::Sender<()>,
    slot: Arc<Mutex<ScanSlot>>,
}

/// std 的锁在别的线程 panic 时会「毒化」，取不出值。交接的那两份东西读不到的最坏
/// 结果只是这一帧没有设备表 / 没有建流结果，不该把界面线程一起拖崩，所以一律取
/// `into_inner()` 接着用（与音色导入那条下载链 `sound_import::lock` 同一个做法）。
fn slot_guard<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl DeviceScanner {
    /// 拉起扫描线程。`scan` 就是那笔昂贵的系统调用，用例里换成假替身
    /// （真实耗时取决于本机声卡，断言它的快慢就是环境相关的用例）。
    fn spawn(scan: impl Fn() -> Vec<OutputDevice> + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(ScanSlot::default()));
        let (request, inbox) = mpsc::channel();
        let shared = slot.clone();
        // 引擎 drop 掉 `request` 后 `recv()` 返回 Err，线程自己收尾，不需要 join。
        std::thread::spawn(move || {
            while inbox.recv().is_ok() {
                let devices = scan();
                let mut s = slot_guard(&shared);
                s.devices = devices;
                s.finished = Some(Instant::now());
                s.scanning = false;
            }
        });
        Self { request, slot }
    }

    /// 请后台重列一次设备；已经在路上就不再排第二趟（点一下不该排几个 1.3 秒）。
    fn request_scan(&self) {
        {
            let mut s = slot_guard(&self.slot);
            if s.scanning {
                return;
            }
            s.scanning = true;
        }
        let _ = self.request.send(());
    }

    /// 最近一趟扫完的设备表；还没扫完过时 `None`。
    fn scanned(&self) -> Option<Vec<OutputDevice>> {
        let s = slot_guard(&self.slot);
        s.finished.map(|_| s.devices.clone())
    }

    /// 最近一趟扫描是什么时候完成的（还没扫过 = `None`）。
    fn finished_at(&self) -> Option<Instant> {
        slot_guard(&self.slot).finished
    }

    /// 设备表是否还在路上：正有一趟在扫，或者从来没扫完过。
    fn pending(&self) -> bool {
        let s = slot_guard(&self.slot);
        s.scanning || s.finished.is_none()
    }
}

pub struct CpalEngine {
    shared: Shared,
    clock: TransportClock,
    bank: Arc<SoundBank>,
    /// 建流、以及那条约出声的流，都归这条后台线程管（见 [`StreamWorker`]）。
    /// 界面这边只留最近一趟的结论，切设备时不等它。
    streamer: StreamWorker,
    device: DeviceReport,
    /// 有一趟建流请求在路上、结果还没回来：这段时间 `device` 是上一台的旧值，
    /// 界面该显示「正在连接」，探测也不许再排第二趟。
    connecting: bool,
    /// 最近一趟建流失败的原因，等着界面取走（切换不再当场有结果，只能延后报）。
    stream_error: Option<String>,
    /// 用户点名的输出设备；`None` = 跟随系统默认。原版没有这个概念
    /// （声音整个交给 GameMaker，只会落在系统默认设备上），端口自加 #94。
    wanted: Option<String>,
    /// 默认设备探测的限流时戳，和最近看到 / 最近尝试建流时系统默认设备的名字。
    probe_at: Instant,
    seen_default: Option<String>,
    tried_default: Option<String>,
    /// 设备表由后台线程扫（见 [`DeviceScanner`]），第一次真要用的时候才把线程拉起来。
    scanner: OnceLock<DeviceScanner>,
    last_song: Option<Song>,
    export_options: ExportOptions,
}

impl CpalEngine {
    /// 创建引擎。找不到音频设备时自动降级为静音时钟，绝不 panic。
    pub fn new() -> Self {
        Self::with_bank(SoundBank::discover())
    }

    pub fn with_bank(bank: SoundBank) -> Self {
        Self::start(bank, None, Box::new(connect_output))
    }

    /// 同上，但一开始就点名要哪台输出设备（首选项里存的上一台）。
    /// 这一趟仍然同步等回来：那时窗口还没画起来，等和不等用户都看不见。
    pub fn with_bank_output(bank: SoundBank, wanted: Option<String>) -> Self {
        let wanted = wanted.filter(|w| !w.is_empty());
        Self::start(bank, wanted, Box::new(connect_output))
    }

    /// `connect` 是建流那一步的注入点：真实引擎用 [`connect_output`]，用例换成假替身，
    /// 于是不必真碰声卡也能验「派发不许等结果」「过期结果要丢掉」这几条。
    fn start(bank: SoundBank, wanted: Option<String>, connect: Box<Connect>) -> Self {
        let bank = Arc::new(bank);
        let shared = make_shared((*bank).clone(), SAMPLE_RATE);
        let clock = TransportClock::with_shared(shared.clone());
        let streamer = StreamWorker::spawn(shared.clone(), connect);
        let seen_default = default_output_name();
        let mut engine = Self {
            shared,
            clock,
            bank,
            streamer,
            device: DeviceReport::Uninitialised,
            connecting: true,
            stream_error: None,
            wanted,
            probe_at: Instant::now(),
            seen_default,
            tried_default: None,
            scanner: OnceLock::new(),
            last_song: None,
            export_options: ExportOptions::default(),
        };
        // 只有启动这一趟是同步等的，为的是把「接上了哪台」当场写进设备那一格。
        let _ = engine.streamer.dispatch(engine.wanted.clone());
        engine.tried_default = engine.seen_default.clone();
        match engine.streamer.wait_first() {
            Some(outcome) => engine.apply_stream(&outcome),
            None => engine.device = DeviceReport::Fallback("audio stream worker stopped".into()),
        }
        engine.connecting = false;
        // 启动这一趟的成败不进状态栏：那会儿没人看得见，偏好里点名的设备拿不到也一样，
        // 设备那一格会如实写，稍后探测自己补。
        engine.stream_error = None;
        engine
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    pub fn export_with_options(&self, song: &Song, path: &Path) -> Result<ExportStats, String> {
        export_wav(song, &self.bank, path, &self.export_options)
    }

    /// 重新扫描采样目录（首选项里改路径后调用）。
    pub fn reload_bank(&mut self, dir: Option<&Path>) {
        let mut bank = SoundBank::load_auto(dir);
        if let Some(song) = &self.last_song {
            bank.extend_for_song(song);
        }
        self.bank = Arc::new(bank);
        let mut st = self.shared.lock();
        st.bank = self.bank.clone();
    }

    /// 换输出设备：`None` = 跟随系统默认。请求派给后台线程后就返回，**不等结果**
    /// （在非默认设备上建流实测 0.6 秒，以前在这儿把界面僵住）。接上了哪台由
    /// [`Self::harvest_stream`] 带回来。
    pub fn set_output(&mut self, wanted: Option<String>) -> Result<(), String> {
        self.wanted = wanted.filter(|w| !w.is_empty());
        self.reconnect()
    }

    /// 有一趟建流还在路上（点了设备到结果回来之间那一段）。界面用它显示「正在连接」，
    /// 用例用它判断什么时候落定。
    pub fn output_connecting(&self) -> bool {
        self.connecting
    }

    /// 取走一句「这趟没成」的原因（同一趟只报一次）。
    pub fn take_output_error(&mut self) -> Option<String> {
        self.stream_error.take()
    }

    /// 用户点名的设备（`None` = 跟随系统默认）。
    pub fn wanted_output(&self) -> Option<&str> {
        self.wanted.as_deref()
    }

    /// 当前正在出声的设备名。
    pub fn output_name(&self) -> Option<&str> {
        match &self.device {
            DeviceReport::Streamed { name, .. } => Some(name),
            _ => None,
        }
    }

    /// 后台扫描线程（第一次调用时拉起）。
    fn scanner(&self) -> &DeviceScanner {
        self.scanner
            .get_or_init(|| DeviceScanner::spawn(list_output_devices))
    }

    /// 已知的输出设备：先请后台重列一次，然后立刻返回手上这一份缓存。
    /// **绝不等枚举结果** —— 那一趟要 1.3 秒（见 [`DeviceScanner`]）。
    /// 缓存还没回来时返回空表，界面用 [`Self::output_scan_pending`] 区分
    /// 「还不知道」和「真的一台都没有」。
    pub fn output_devices(&self) -> Vec<OutputDevice> {
        self.scanner().request_scan();
        self.scanner().scanned().unwrap_or_default()
    }

    /// 设备表还在路上吗？线程还没拉起来时算「不在路上」（没人在等它）。
    pub fn output_scan_pending(&self) -> bool {
        self.scanner.get().is_some_and(DeviceScanner::pending)
    }

    /// 派一趟建流请求给后台线程，立即返回。真正接上了哪台由 [`Self::harvest_stream`]
    /// 取回；指定的设备拿不到就回落到系统默认，这套策略在 [`connect_output`] 里。
    fn reconnect(&mut self) -> Result<(), String> {
        let sent = self.streamer.dispatch(self.wanted.clone());
        if sent.is_ok() {
            self.connecting = true;
            self.device = DeviceReport::Connecting;
            // 派发就算「这台默认试过了」：否则探测那一档看到下一帧还没结果，会每秒再排
            // 一趟 0.6 秒的活，把后台淹掉。
            self.tried_default = self.seen_default.clone();
        }
        sent
    }

    /// 到底有没有在出声？只认最近一趟落地的结论——`cpal::Stream` 不在这边，问不到它。
    fn stream_live(&self) -> bool {
        matches!(self.device, DeviceReport::Streamed { .. })
    }

    /// 取回已经落地的建流结果，返回要报给界面的新设备名。回落和失败不在这儿报，
    /// 走 [`Self::take_output_error`]，一句话只报一次。
    fn harvest_stream(&mut self) -> Option<String> {
        let outcome = self.streamer.harvest()?;
        if outcome.for_wanted != self.wanted {
            // 过期结果：这趟跑完之前用户又点了一次，以最后一次为准，这一条直接丢掉。
            return None;
        }
        self.connecting = false;
        let announced = match &outcome.built {
            Some((name, _, _)) if !outcome.fell_back => Some(name.clone()),
            _ => None,
        };
        self.apply_stream(&outcome);
        announced
    }

    /// 把一趟结论并进界面看得到那份设备快照，顺带处理「点名却拿不到」的回落。
    fn apply_stream(&mut self, outcome: &StreamOutcome) {
        match &outcome.built {
            Some((name, rate, channels)) => {
                self.device = DeviceReport::Streamed {
                    name: name.clone(),
                    rate: *rate,
                    channels: *channels,
                }
            }
            None => {
                self.device = DeviceReport::Fallback(
                    outcome
                        .error
                        .clone()
                        .unwrap_or_else(|| "no audio output".to_string()),
                );
                // 旧流已经被后台放掉（drop 即停声），这里得把「有音频回调在驱动播放头」
                // 这个前提一起撤掉，否则播放头冻着不动却装作还在出声。
                self.shared.lock().audio_active = false;
            }
        }
        if outcome.fell_back {
            let requested = outcome.for_wanted.clone().unwrap_or_default();
            let name = match &self.device {
                DeviceReport::Streamed { name, .. } => name.clone(),
                _ => String::new(),
            };
            // 点名的那台没拿到：`wanted` 跟着清空，界面据此把偏好改回「跟随系统默认」，
            // 免得下次启动又去点一台已经不在的设备。
            self.wanted = None;
            self.stream_error = Some(format!(
                "output device \"{requested}\" is not available, now on {name}"
            ));
        } else if let Some(e) = &outcome.error {
            self.stream_error = Some(e.clone());
        }
    }

    /// 系统默认输出换了（插耳机、切蓝牙）就跟随过去；用户点名的设备掉线也回落默认；
    /// 之前没建成流、现在系统又有可用设备了就补一次。约每秒探测一次，
    /// 而且只做便宜的那半：读默认设备名（实测 0.4~1.5 毫秒），全量枚举交给后台线程，
    /// 这里只读它扫回来的缓存（#94 治本 —— 以前在这儿每秒枚举一次，界面被拖死）。
    /// 建流同样是派给后台，不在这里等。
    fn poll_output_change(&mut self) {
        if self.probe_at.elapsed() < OUTPUT_PROBE {
            return;
        }
        self.probe_at = Instant::now();
        let seen = default_output_name();
        let default_changed = seen != self.seen_default;
        self.seen_default = seen.clone();
        let wanted_gone;
        {
            let scanner = self.scanner();
            if default_changed {
                // 插拔设备一般也改了设备表本身，让后台补扫一次，下拉里才不会
                // 列出一台已经不存在的设备。
                scanner.request_scan();
            }
            // 点名的那台还在不在，只看后台扫回来的表；表还没回来就不下结论。
            wanted_gone = match (self.wanted.as_ref(), scanner.scanned()) {
                // 点名了设备却一次都没扫过（比如这份偏好是上次启动存的）：
                // 不去扫就永远不知道它还在不在，声音可能一直出在一台已经不存在的设备上。
                (Some(_), None) => {
                    scanner.request_scan();
                    false
                }
                (Some(wanted), Some(list)) => {
                    if list.iter().any(|d| &d.name == wanted) {
                        // 没人开下拉时也得让表保持新鲜，否则那台被拔掉就永远看不出来。
                        if scanner
                            .finished_at()
                            .is_some_and(|at| at.elapsed() >= OUTPUT_RESCAN)
                        {
                            scanner.request_scan();
                        }
                        false
                    } else {
                        true
                    }
                }
                _ => false,
            };
        }
        // 建流失败过（没流）+ 系统有可用默认 + 这台默认还没试过：再试一次。
        // 已经有一趟在路上的就不许再排，否则每秒排一趟 0.6 秒的活。
        let dead_but_available =
            !self.stream_live() && !self.connecting && seen.is_some() && seen != self.tried_default;
        let follow_default = self.wanted.is_none() && default_changed;
        if follow_default || wanted_gone || dead_but_available {
            let _ = self.reconnect();
        }
    }
}

impl AudioEngine for CpalEngine {
    fn name(&self) -> &str {
        if self.stream_live() {
            nbs_core::tr("Sample audio engine", "原版采样音频引擎")
        } else {
            nbs_core::tr("Silent clock (no audio device)", "静音时钟（无音频设备）")
        }
    }
    fn prepare(&mut self, song: &Song) {
        let mut bank = (*self.bank).clone();
        bank.extend_for_song(song);
        self.bank = Arc::new(bank);
        let mut st = self.shared.lock();
        st.prepare(song, self.bank.clone());
        drop(st);
        self.clock.set_tempo(song.meta.tempo);
        self.last_song = Some(song.clone());
    }
    fn preview_note_vp(&mut self, note: &Note, layer_volume: u8, layer_stereo: u8) {
        let mut st = self.shared.lock();
        let bank = st.bank.clone();
        // 层音量/声道由调用方给：原版 `play_sound(ins, key, vol, pan, 0)` 的 vol/pan
        // 就是这两个位置，绝大多数调用传 100/100，只有 `layerhov_vppreview` 打开时的
        // 滚轮预览传悬停层的值（`control_draw.gml:256-263`）。
        // 层号 0：原版预览调用不带层号，emitter 的 soundlayer 默认 0，
        // 区间消音掐不到它（只有 pitch<=0 的全消音能掐到）。tick -1：预览不抖动。
        st.mixer
            .spawn(note, layer_volume, layer_stereo, &bank, 0, -1);
    }
    fn preview_metronome(&mut self, key: u8) {
        let mut st = self.shared.lock();
        let bank = st.bank.clone();
        // 原版这一声用的是声音资产本身，乐器下标只是 Note 结构里的占位。
        let click = nbs_core::model::Note::new(0, key);
        st.mixer.spawn_source(
            crate::samples::SampleSource::Metronome,
            &click,
            100,
            100,
            &bank,
            0,
            -1,
        );
    }
    fn start(&mut self, from_tick: f64) {
        self.clock.seek(from_tick);
        self.clock.play();
        // 原版 toggle_playing：按播放键先取落点列速度（get_tempo_from_tick），
        // 再把该列整列派发。只 seek 不派发会让落点列的变速器/消音器
        // 拖到下一列跨过时才生效。
        self.shared.lock().dispatch_landing_column(from_tick);
    }
    fn stop(&mut self) {
        self.clock.pause();
    }
    fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }
    fn position_ticks(&self) -> f64 {
        self.clock.position()
    }
    fn advance(&mut self, _dt: f64) {
        self.clock.update();
    }
    fn active_voices(&self) -> usize {
        self.shared.lock().mixer.active()
    }
    fn export_wav(&mut self, path: &Path) -> bool {
        if let Some(song) = self.last_song.clone() {
            self.export_with_options(&song, path).is_ok()
        } else {
            false
        }
    }
    fn clock(&self) -> &TransportClock {
        &self.clock
    }
    fn clock_mut(&mut self) -> &mut TransportClock {
        &mut self.clock
    }
    fn device_info(&self) -> String {
        match &self.device {
            DeviceReport::Streamed {
                name,
                rate,
                channels,
            } => {
                let ch = if nbs_core::lang() == 1 {
                    format!("{channels} 声道")
                } else {
                    format!("{channels} channels")
                };
                format!("{name} · {rate} Hz · {ch}")
            }
            DeviceReport::Fallback(reason) => {
                if nbs_core::lang() == 1 {
                    format!("静音时钟（{reason}）")
                } else {
                    format!("Silent clock ({reason})")
                }
            }
            DeviceReport::Connecting => nbs_core::tr(
                "Silent clock (connecting output device)",
                "静音时钟（正在连接输出设备）",
            )
            .to_string(),
            DeviceReport::Uninitialised => nbs_core::tr(
                "Silent clock (audio device not initialised)",
                "静音时钟（未初始化音频设备）",
            )
            .to_string(),
        }
    }
    fn bank(&self) -> Arc<SoundBank> {
        self.bank.clone()
    }
    fn output_devices(&self) -> Vec<OutputDevice> {
        CpalEngine::output_devices(self)
    }
    fn output_scan_pending(&self) -> bool {
        CpalEngine::output_scan_pending(self)
    }
    fn output_name(&self) -> Option<String> {
        CpalEngine::output_name(self).map(str::to_string)
    }
    fn wanted_output(&self) -> Option<&str> {
        CpalEngine::wanted_output(self)
    }
    fn set_output(&mut self, wanted: Option<String>) -> Result<(), String> {
        CpalEngine::set_output(self, wanted)
    }
    fn output_connecting(&self) -> bool {
        CpalEngine::output_connecting(self)
    }
    fn poll_output(&mut self) -> Option<String> {
        let announced = self.harvest_stream();
        self.poll_output_change();
        announced
    }
    fn take_output_error(&mut self) -> Option<String> {
        CpalEngine::take_output_error(self)
    }
    fn sample_status(&self) -> String {
        if self.bank.is_ready() {
            let (count, total) = (self.bank.loaded_count(), BUILTIN_FILES.len());
            if nbs_core::lang() == 1 {
                let miss = if self.bank.missing.is_empty() {
                    ""
                } else {
                    "（部分缺失）"
                };
                format!("已加载 {count}/{total} 个内置采样{miss}")
            } else {
                let miss = if self.bank.missing.is_empty() {
                    ""
                } else {
                    " (some missing)"
                };
                format!("Loaded {count}/{total} built-in samples{miss}")
            }
        } else {
            nbs_core::tr("Built-in samples folder not found", "未找到内置采样目录").into()
        }
    }
    fn take_visual_events(&mut self) -> Vec<VisualEvent> {
        self.shared.lock().take_visual_events()
    }
    fn reload_bank(&mut self, dir: Option<&Path>) {
        CpalEngine::reload_bank(self, dir);
    }
}

/// 纯时钟实现（无共享状态、无音频设备），用于测试与显式静音。
#[derive(Default)]
pub struct SilentEngine {
    clock: TransportClock,
    bank: Arc<SoundBank>,
    events: Vec<NoteEvent>,
    /// 无混音器也要发「改界面」的事件乐器副作用（原版 else-if 链与设备无关）。
    visual: Vec<VisualEvent>,
}

impl SilentEngine {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clock(&self) -> &TransportClock {
        &self.clock
    }
    pub fn clock_mut(&mut self) -> &mut TransportClock {
        &mut self.clock
    }
    /// [from, to] 列内改界面的事件乐器入队；时钟速度同步取 ≤to 的最后一个
    /// 变速点（原版 get_tempo_from_tick 语义）。发声/消音没有混音器可去，跳过，
    /// 但琴键时间戳不依赖混音器（原版 key_played 与 play_sound 是两条独立闸门）。
    fn dispatch_visual_columns(&mut self, from: i64, to: i64) {
        if to < from {
            return;
        }
        let mut tempo = self.clock.tempo().max(0.25);
        for ev in &self.events {
            if ev.tick as i64 > to {
                break;
            }
            if let EventKind::Tempo(pitch) = ev.kind {
                tempo = (pitch.unsigned_abs() as f32 / 15.0).max(0.25);
            }
        }
        for ev in &self.events {
            let t = ev.tick as i64;
            if t > to {
                break;
            }
            if t < from {
                continue;
            }
            match ev.kind {
                EventKind::Rainbow => self.visual.push(VisualEvent::RainbowToggle),
                EventKind::SavePopup => self.visual.push(VisualEvent::SavePopup),
                EventKind::AccentColor(rgb) => self.visual.push(VisualEvent::AccentColor(rgb)),
                EventKind::BackgroundAccent => {
                    self.visual.push(VisualEvent::BackgroundAccentToggle)
                }
                EventKind::Note if ev.press => {
                    self.visual.push(VisualEvent::KeyPlayed(ev.note.key))
                }
                // Tempo / Stopper：一个已在上面的循环里同步过时钟，一个要靠混音器。
                _ => {}
            }
        }
        self.clock.set_tempo(tempo);
    }
}

impl AudioEngine for SilentEngine {
    fn name(&self) -> &str {
        nbs_core::tr("Silent clock", "静音时钟")
    }
    fn prepare(&mut self, song: &Song) {
        self.events = build_event_table(song, false);
        self.clock.set_tempo(song.meta.tempo);
        self.clock.set_loop(
            song.meta.loop_enabled,
            song.meta.loop_start as f64,
            song.song_length().max(1) as f64,
            song.meta.loop_max,
        );
    }
    fn preview_note_vp(&mut self, _note: &Note, _layer_volume: u8, _layer_stereo: u8) {
        // 没有输出设备，试听无处可去；时钟照常推进。
    }
    fn preview_metronome(&mut self, _key: u8) {}
    fn start(&mut self, from_tick: f64) {
        self.clock.seek(from_tick);
        self.clock.play();
        let column = from_tick.floor() as i64;
        if column >= 0 {
            self.dispatch_visual_columns(column, column);
        }
    }
    fn stop(&mut self) {
        self.clock.pause();
    }
    fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }
    fn position_ticks(&self) -> f64 {
        self.clock.position()
    }
    fn advance(&mut self, _dt: f64) {
        let prev = self.clock.position().floor() as i64;
        self.clock.update();
        let now = self.clock.position().floor() as i64;
        if now > prev {
            self.dispatch_visual_columns(prev + 1, now);
        }
    }
    fn active_voices(&self) -> usize {
        0
    }
    fn clock(&self) -> &TransportClock {
        &self.clock
    }
    fn clock_mut(&mut self) -> &mut TransportClock {
        &mut self.clock
    }
    fn device_info(&self) -> String {
        nbs_core::tr("No audio output (clock only)", "无音频输出（纯时钟）").into()
    }
    fn bank(&self) -> Arc<SoundBank> {
        self.bank.clone()
    }
    fn sample_status(&self) -> String {
        nbs_core::tr("No samples loaded", "未加载采样").into()
    }
    fn take_visual_events(&mut self) -> Vec<VisualEvent> {
        std::mem::take(&mut self.visual)
    }
}

// ---------------------------------------------------------------- 离线导出

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub sample_rate: u32,
    /// 1 = 单声道，2 = 立体声（原版默认 2）。
    pub channels: u16,
    /// 是否包含锁定层（原版默认排除）。
    pub include_locked: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels: 2,
            include_locked: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExportStats {
    pub frames: usize,
    pub notes: usize,
    pub duration_seconds: f64,
    pub peak: f32,
    pub threads: usize,
}

/// 离线导出 WAV：按音符分片多线程混音，最后相加写出 s16 PCM。
pub fn export_wav(
    song: &Song,
    bank: &SoundBank,
    path: &Path,
    opts: &ExportOptions,
) -> Result<ExportStats, String> {
    let events = build_event_table(song, opts.include_locked);
    let (pcm, stats) = render_offline(song, bank, &events, opts);
    write_wav_i16(path, &pcm, opts.sample_rate, opts.channels)?;
    Ok(stats)
}

/// nbswave `tempo_segments`：第 t 项 = tick t 发声的时间点（秒）。
/// 从 0 起每跨一列累加 1/当时速度；自定义乐器名恰为 "Tempo Changer" 的音符
/// 把后续步长换成 pitch/15（同列多枚时最后一枚生效，nbswave 相同）。
/// 注意 `segments[0] = 1/tempo`：导出的第 0 列比实时播放晚一整步——这是
/// nbswave 的既有行为，照搬（实时调度仍走 GML 语义，不受影响）。
fn tempo_segments(song: &Song, events: &[NoteEvent]) -> Vec<f64> {
    let last_tick = song.song_length() as usize;
    let mut seg = Vec::with_capacity(last_tick + 1);
    let mut t_cur = 0usize;
    let mut tempo = (song.meta.tempo as f64).max(0.01);
    let mut secs = 0.0f64;
    let changers = events.iter().filter_map(|ev| match ev.kind {
        EventKind::Tempo(pitch) => Some((ev.tick as usize, pitch)),
        _ => None,
    });
    for (tick, pitch) in changers {
        while t_cur < tick.min(last_tick + 1) {
            secs += 1.0 / tempo;
            seg.push(secs);
            t_cur += 1;
        }
        if tick <= last_tick {
            // pitch<=0 的病态值按实时播放的下限 0.25 t/s 兜底（nbswave 对
            // 0 直接除零崩溃；正常文件由编辑窗校验 >0）。
            tempo = if pitch > 0 { pitch as f64 / 15.0 } else { 0.25 };
        }
        if t_cur < tick {
            t_cur = tick;
        }
    }
    while t_cur <= last_tick {
        secs += 1.0 / tempo;
        seg.push(secs);
        t_cur += 1;
    }
    seg
}

/// 渲染成交错 f32 PCM（可复用于其它容器编码）。
pub fn render_offline(
    song: &Song,
    bank: &SoundBank,
    events: &[NoteEvent],
    opts: &ExportOptions,
) -> (Vec<f32>, ExportStats) {
    let rate = opts.sample_rate.max(8000);
    let channels = opts.channels.clamp(1, 2) as usize;
    let segs = tempo_segments(song, events);

    // 原版 get_length：长度 = 最后一个停止发声的音符（缺采样/事件乐器只
    // 贡献自己的起点位置），没有外加尾巴；锁定层的音符**参与定长**——
    // nbswave 的 get_length 永远遍历 weighted_notes（全部音符），排除锁定层
    // 只影响混音本身。整歌没有音符时兜底为整首歌长（原版此处直接崩溃）。
    let all_notes = build_event_table(song, true);
    let mut frames = if all_notes.is_empty() {
        (segs[segs.len() - 1] * rate as f64) as usize
    } else {
        0
    };
    for ev in &all_notes {
        let Some(&pos) = segs.get(ev.tick as usize) else {
            continue;
        };
        let start = pos * rate as f64;
        let end = match bank.get(ev.note.instrument as usize) {
            Some(sample) if !sample.is_empty() => {
                let ratio = Voice::playback_ratio(&ev.note, sample.base_key, rate);
                start + sample.pcm.len() as f64 / ratio
            }
            _ => start,
        };
        frames = frames.max(end as usize);
    }

    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
        .min(events.len().max(1));
    let chunk = events.len().div_ceil(thread_count).max(1);
    let mut partials: Vec<Vec<f32>> = Vec::with_capacity(thread_count);
    let bank_ref = &bank;
    let segs_ref = &segs;
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(thread_count);
        for group in events.chunks(chunk) {
            let handle = scope.spawn(move || {
                let mut buf = vec![0.0f32; frames * channels];
                render_event_group(group, bank_ref, segs_ref, &mut buf, frames, channels, rate);
                buf
            });
            handles.push(handle);
        }
        for h in handles {
            partials.push(h.join().unwrap_or_default());
        }
    });

    let mut out = vec![0.0f32; frames * channels];
    for p in &partials {
        for (d, &s) in out.iter_mut().zip(p.iter()) {
            *d += s;
        }
    }

    let mut peak = 0.0f32;
    for &s in &out {
        peak = peak.max(s.abs());
    }
    // 原版 nbswave（打包的 v0.4.0）没有限幅器，也没有 headroom——混音在 int32
    // 里自由累加，只在收尾时看峰值：超过 32767 就整轨除以峰值归到 0dBFS，
    // 否则原样写出。它的 `headroom` 参数从头到尾没被用过；这里照搬同一语义。
    if peak > 1.0 {
        let g = 1.0 / peak;
        for s in &mut out {
            *s *= g;
        }
        peak = 1.0;
    }

    (
        out,
        ExportStats {
            frames,
            notes: events.len(),
            duration_seconds: frames as f64 / rate as f64,
            peak,
            threads: thread_count,
        },
    )
}

/// pydub 0.25.1 `AudioSegment.pan`（effects.py:284）——原版导出的声像曲线。
/// pan ∈ -1..1（0 = 中）：响侧提升 |pan|·6.0206 dB 的一半（最多 +3.01 dB），
/// 另一侧按 `2 − 10^(|pan|·6.0206/20)` 线性扣减（pydub 的怪癖：在比值域直接
/// 相减），极偏时归零；居中不动。实时播放走 GameMaker 发射器的衰减式声像，
/// 导出走 pydub——原版两边本就不是同一条曲线。
fn pan_factors_pydub(pan: f32) -> (f32, f32) {
    if pan == 0.0 {
        return (1.0, 1.0);
    }
    let max_boost_db = 20.0 * 2.0f32.log10();
    let boost_db = pan.abs() * max_boost_db;
    let boost_factor = 10f32.powf(boost_db / 20.0);
    let reduce_factor = 2.0 - boost_factor;
    let boost_half = 10f32.powf(boost_db / 2.0 / 20.0);
    if pan < 0.0 {
        (boost_half, reduce_factor)
    } else {
        (reduce_factor, boost_half)
    }
}

fn render_event_group(
    events: &[NoteEvent],
    bank: &SoundBank,
    segs: &[f64],
    buf: &mut [f32],
    frames: usize,
    channels: usize,
    rate: u32,
) {
    for ev in events {
        let Some(sample) = bank.get(ev.note.instrument as usize) else {
            continue;
        };
        if sample.is_empty() {
            continue;
        }
        let Some(&pos) = segs.get(ev.tick as usize) else {
            continue;
        };
        let start = (pos * rate as f64) as usize;
        if start >= frames {
            continue;
        }
        let ratio = Voice::playback_ratio(&ev.note, sample.base_key, rate);
        // 导出不用播放主音量：原版 nbswave 没有 master 概念，音符增益只由
        // 层音量 × 力度决定；声像走 pydub pan 曲线（mono 时 pydub 把已声像
        // 的两声道取平均，下面的混合循环正是 (gl+gr)/2）。
        let vol_eff = (ev.layer_volume as f32 / 100.0) * ev.note.velocity as f32;
        let g = vol_eff / 100.0;
        let pan_eff = if ev.layer_stereo == PAN_CENTER {
            ev.note.panning as f32 - 100.0
        } else {
            (ev.layer_stereo as f32 + ev.note.panning as f32) / 2.0 - 100.0
        };
        let (pl, pr) = pan_factors_pydub(pan_eff / 100.0);
        let (gl, gr) = (g * pl, g * pr);
        let pcm = &sample.pcm;
        let mut pos = 0.0f64;
        let mut f = start;
        while f < frames {
            let idx = pos as usize;
            if idx + 1 >= pcm.len() {
                break;
            }
            let frac = (pos - idx as f64) as f32;
            let value = pcm[idx] + (pcm[idx + 1] - pcm[idx]) * frac;
            let base = f * channels;
            if channels == 1 {
                buf[base] += value * (gl + gr) * 0.5;
            } else {
                buf[base] += value * gl;
                buf[base + 1] += value * gr;
            }
            pos += ratio;
            f += 1;
        }
    }
}

/// 手写 RIFF/WAVE（PCM s16），零额外依赖。
pub fn write_wav_i16(
    path: &Path,
    interleaved: &[f32],
    rate: u32,
    channels: u16,
) -> Result<(), String> {
    let channels = channels.clamp(1, 2);
    let data_len = interleaved.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in interleaved {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| {
        if nbs_core::lang() == 1 {
            format!("写入 WAV 失败: {e}")
        } else {
            format!("failed to write WAV: {e}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Song;

    #[test]
    fn wav_header_is_valid() {
        let dir = std::env::temp_dir().join("nbs_rust_wav_test.wav");
        write_wav_i16(&dir, &[0.0, 0.5, -0.5, 1.0], 44100, 2).unwrap();
        let b = std::fs::read(&dir).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"WAVE");
        assert_eq!(&b[36..40], b"data");
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 8);
        let _ = std::fs::remove_file(dir);
    }

    /// #94：点名一台不存在的设备必须报错。「回落成跟随默认」是 `reconnect` 里显式
    /// 做的动作，不能混在取设备的步骤里悄悄发生。
    #[test]
    fn unknown_output_device_is_rejected() {
        let missing = "nbs-rust 这台机器上不存在的输出设备";
        assert!(open_output_device(Some(missing)).is_err());
        assert!(list_output_devices().iter().all(|d| d.name != missing));
    }

    /// #94：切换没成功时，引擎里不能留下这个没生效的点名下标——
    /// 否则每次启动都会去点一台已经不存在的设备。结果在后台线程上落，所以要轮询到落定。
    #[test]
    fn dead_output_choice_does_not_stick() {
        let mut engine = CpalEngine::new();
        let missing = "nbs-rust 这台机器上不存在的输出设备".to_string();
        engine.set_output(Some(missing.clone())).unwrap();
        for _ in 0..400 {
            let _ = engine.poll_output();
            if !engine.output_connecting() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_ne!(engine.wanted_output(), Some(missing.as_str()));
    }

    fn built_outcome(
        for_wanted: Option<String>,
        name: &str,
    ) -> (StreamOutcome, Option<cpal::Stream>) {
        (
            StreamOutcome {
                for_wanted,
                built: Some((name.to_string(), 44_100, 2)),
                error: None,
                fell_back: false,
            },
            None,
        )
    }

    /// 假建流：只回结论，绝不碰真实声卡。跟随默认那趟（`wanted` 为空）当场答复，
    /// 点名的那台卡在信道上等放行——真在这一趟上等 0.6 秒就是用户反馈的「一点设备菜单
    /// 整个窗口僵住」，断言真实耗时是环境相关用例，所以拿信道当替身。
    #[test]
    fn a_stream_request_is_dispatched_but_never_waited_for() {
        let (gate, wait) = mpsc::channel::<()>();
        let connect = Box::new(
            move |_shared: &Shared, wanted: Option<String>| match wanted {
                None => built_outcome(None, "系统默认"),
                Some(name) => {
                    let _ = wait.recv();
                    built_outcome(Some(name.clone()), &name)
                }
            },
        );
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        assert!(
            !engine.output_connecting(),
            "启动那趟已经落地，不该还挂着「正在连接」"
        );

        let t = Instant::now();
        engine.set_output(Some("假设备".to_string())).unwrap();
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "切设备不许等结果，实际等了 {:?}",
            t.elapsed()
        );
        assert!(engine.output_connecting(), "结果还没回来就该报「正在连接」");
        assert_eq!(engine.poll_output(), None, "还没接上就不许报出新设备");

        gate.send(()).unwrap();
        for _ in 0..400 {
            if engine.poll_output().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(engine.output_name().as_deref(), Some("假设备"));
        assert!(!engine.output_connecting(), "落定后要报「已经接上」");
    }

    /// 连点两台：前一趟还在路上时用户又点了一次。旧结果必须丢掉，一切以最后一次点为准。
    #[test]
    fn a_superseded_stream_result_is_dropped() {
        let (gate, wait) = mpsc::channel::<()>();
        let connect =
            Box::new(
                move |_shared: &Shared, wanted: Option<String>| match wanted.as_deref() {
                    None => built_outcome(None, "系统默认"),
                    Some("慢的那台") => {
                        let _ = wait.recv();
                        built_outcome(wanted, "慢的那台")
                    }
                    other => built_outcome(other.map(str::to_string), "快的那台"),
                },
            );
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        engine.set_output(Some("慢的那台".to_string())).unwrap();
        engine.set_output(Some("快的那台".to_string())).unwrap();
        gate.send(()).unwrap();
        for _ in 0..400 {
            let _ = engine.poll_output();
            if !engine.output_connecting() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            engine.output_name().as_deref(),
            Some("快的那台"),
            "跑得更慢的那台不许翻盘"
        );
        assert_eq!(
            engine.output_name().as_deref(),
            Some("快的那台"),
            "旧结果不许迟一步再翻一次"
        );
    }

    /// 点名的设备拿不到：后台已经回落系统默认，这边要把 `wanted` 清空并留下一句原因，
    /// 界面才能把偏好改回「跟随默认」，别存一个没生效的点名下标。
    #[test]
    fn a_missing_device_falls_back_with_a_reason() {
        let connect = Box::new(|_shared: &Shared, wanted: Option<String>| {
            let fell_back = wanted.is_some();
            (
                StreamOutcome {
                    for_wanted: wanted,
                    built: Some(("新的系统默认".to_string(), 48_000, 2)),
                    error: None,
                    fell_back,
                },
                None,
            )
        });
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        engine.set_output(Some("拔掉的那台".to_string())).unwrap();
        for _ in 0..400 {
            let _ = engine.poll_output();
            if !engine.output_connecting() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(engine.wanted_output(), None, "没生效的点名下标不能留下");
        assert_eq!(engine.output_name().as_deref(), Some("新的系统默认"));
        let reason = engine.take_output_error().unwrap_or_default();
        assert!(
            reason.contains("拔掉的那台"),
            "原因里要点出是哪台没拿到：{reason}"
        );
        assert!(engine.take_output_error().is_none(), "同一趟只报一次");
        assert!(
            engine.poll_output().is_none(),
            "回落这一趟不许再报一遍新设备名"
        );
    }

    /// 启动那趟失败不往状态栏报（那会儿窗口还没画起来，没人看得见），只进设备那一格；
    /// 但「有没有在出声」必须当场说清楚，不许装作还在播放。
    #[test]
    fn the_startup_pass_says_nothing_in_the_status_bar() {
        let connect = Box::new(|_shared: &Shared, wanted: Option<String>| {
            stream_failed(wanted, "no audio device at all".to_string())
        });
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        assert_eq!(
            engine.take_output_error(),
            None,
            "启动那趟的失败不该报进状态栏"
        );
        assert!(!engine.output_connecting());
        assert_eq!(engine.output_name(), None);
        assert!(
            !engine.shared().lock().audio_active,
            "没建成流就不许还挂在「有音频回调驱动播放头」上"
        );
    }

    /// #94 治本：列设备要 1.3 秒（慢在系统的 `EnumAudioEndpoints`，本机三台端点实测
    /// 1.26~1.52 秒），界面线程一次都不能等它。这里用假扫描（卡在信道上等放行）
    /// 把「不许等」和「同时间的重复请求并成一趟」钉死 —— 断言真实耗时就成了环境相关用例。
    #[test]
    fn a_device_scan_is_requested_but_never_waited_for() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (gate, released) = mpsc::channel::<()>();
        let scans = Arc::new(AtomicUsize::new(0));
        let counted = scans.clone();
        let scanner = DeviceScanner::spawn(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = released.recv();
            vec![OutputDevice {
                name: "假设备".to_string(),
                is_default: true,
            }]
        });

        let t = Instant::now();
        scanner.request_scan();
        scanner.request_scan();
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "请求扫描不许等结果，实际等了 {:?}",
            t.elapsed()
        );
        assert!(scanner.pending(), "还没扫完就是「还不知道」");
        assert_eq!(
            scanner.scanned(),
            None,
            "没扫完不许交出一份看着像真的的空表"
        );

        gate.send(()).unwrap();
        for _ in 0..200 {
            if scanner.scanned().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            scans.load(Ordering::SeqCst),
            1,
            "两趟请求并成一趟，不许排第二份 1.3 秒"
        );
        assert_eq!(
            scanner.scanned().map(|l| l[0].name.clone()),
            Some("假设备".to_string())
        );
        assert!(!scanner.pending(), "扫完了就该报「知道了」");
    }

    #[test]
    fn offline_render_produces_nonzero_audio() {
        let bank = SoundBank::discover();
        if !bank.is_ready() {
            return;
        }
        let song = Song::demo();
        let events = build_event_table(&song, false);
        let (pcm, stats) = render_offline(&song, &bank, &events, &Default::default());
        assert!(stats.frames > 44100);
        assert_eq!(stats.notes, song.total_blocks);
        assert!(pcm.iter().any(|&s| s.abs() > 0.001), "应有实际声音");
        assert!(pcm.iter().all(|&s| s.abs() <= 1.0 + 1e-6), "必须限幅");
    }

    /// 原版 nbswave 没有限幅器：峰值超过 1.0 才整轨除以峰值（0dBFS），
    /// 未超时必须原样写出（既不归一也不留余量）。
    #[test]
    fn export_normalizes_only_when_clipping() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.9; 1000],
            base_key: 45,
            name: "test".into(),
        });
        // 同 tick 两枚满力度音符：峰值 1.8 → 整轨缩到 1.0
        let mut hot = Song::new();
        hot.set(0, 0, Note::new(0, 45));
        hot.set(0, 1, Note::new(0, 45));
        hot.recalculate();
        let events = build_event_table(&hot, false);
        let (pcm, stats) = render_offline(&hot, &bank, &events, &Default::default());
        assert!(
            (stats.peak - 1.0).abs() < 1e-5,
            "削顶必须整轨归一到 1.0，实测 {}",
            stats.peak
        );
        assert!(pcm.iter().all(|&s| s.abs() <= 1.0 + 1e-6));
        // 单枚音符：峰值 0.9 < 1，一个样本都不许动
        let mut quiet = Song::new();
        quiet.set(0, 0, Note::new(0, 45));
        quiet.recalculate();
        let events = build_event_table(&quiet, false);
        let (pcm, stats) = render_offline(&quiet, &bank, &events, &Default::default());
        assert!(
            (stats.peak - 0.9).abs() < 1e-5,
            "未削顶不应缩放，实测 {}",
            stats.peak
        );
        // nbswave 时间轴：tick 0 落在 1/tempo = 0.1s（交错立体声 ×2）
        let start = (0.1 * 44100.0) as usize * 2;
        assert!(
            (pcm[start] - 0.9).abs() < 1e-4,
            "首样本应原样 0.9，实测 {}",
            pcm[start]
        );
    }

    /// nbswave tempo_segments 的移植对照：10 t/s，tick 4 一枚变速器
    /// （pitch=600 → 40 t/s）。变速列自身的步长用新速度；第 0 列在 1/tempo。
    #[test]
    fn tempo_segments_match_nbswave() {
        let mut song = Song::new();
        song.meta.tempo = 10.0;
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Tempo Changer".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(0, 0, Note::new(0, 45));
        song.set(
            4,
            0,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 600,
            },
        );
        song.set(8, 0, Note::new(0, 45));
        song.recalculate();
        let events = build_event_table(&song, false);
        let segs = tempo_segments(&song, &events);
        assert_eq!(segs.len(), 10, "歌长 9 → 覆盖 tick 0..=9");
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            near(segs[0], 0.1),
            "segments[0] = 1/tempo，实测 {}",
            segs[0]
        );
        assert!(near(segs[3], 0.4), "{}", segs[3]);
        assert!(
            near(segs[4], 0.4 + 1.0 / 40.0),
            "进入变速列的那一步用新速度，实测 {}",
            segs[4]
        );
        assert!(near(segs[8], 0.4 + 5.0 / 40.0), "{}", segs[8]);
    }

    /// 导出落点按 tempo_segments：变速后的音符从新位置发声，长度 = 采样
    /// 放完为止（没有 2 秒尾巴）。
    #[test]
    fn offline_render_places_notes_on_tempo_segments() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![1.0; 1000],
            base_key: 45,
            name: "test".into(),
        });
        let mut song = Song::new();
        song.meta.tempo = 10.0;
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "Tempo Changer".into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        song.set(8, 0, Note::new(0, 45));
        song.set(
            4,
            1,
            Note {
                instrument: 20,
                key: 45,
                velocity: 100,
                panning: 100,
                pitch: 600,
            },
        );
        song.recalculate();
        let events = build_event_table(&song, false);
        let (pcm, stats) = render_offline(&song, &bank, &events, &Default::default());
        let expect = (0.4 + 5.0 / 40.0) * 44100.0;
        let first = pcm.iter().position(|&s| s != 0.0).expect("应有声音");
        let first_frame = (first / 2) as f64;
        assert!(
            (first_frame - expect).abs() < 2.0,
            "起点应在 {} 帧，实测 {}",
            expect,
            first_frame
        );
        // 长度 = 起点 + 采样本身（1000 帧，ratio=1），无尾巴
        let start = (expect) as usize;
        assert_eq!(stats.frames, start + 1000);
    }

    /// pydub pan 曲线：居中不动；极偏响侧 +3.01dB（√2）、另一侧归零；
    /// +50（pan 150/右半）左侧 2−10^(3.0103/20)、右侧 10^(1.50515/20)。
    #[test]
    fn export_pan_matches_pydub_curve() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let (l, r) = pan_factors_pydub(0.0);
        assert!(
            near(l, 1.0) && near(r, 1.0),
            "居中必须不动，实测 ({l}, {r})"
        );
        let (l, r) = pan_factors_pydub(-1.0);
        assert!(
            near(l, 2f32.sqrt()) && near(r, 0.0),
            "全左 = (√2, 0)，实测 ({l}, {r})"
        );
        let (l, r) = pan_factors_pydub(1.0);
        assert!(
            near(l, 0.0) && near(r, 2f32.sqrt()),
            "全右 = (0, √2)，实测 ({l}, {r})"
        );
        let (l, r) = pan_factors_pydub(0.5);
        // boost_factor 恰为 √2：reduce = 2−√2，boost/2 = 2^0.25
        assert!(
            near(l, 2.0 - 2f32.sqrt()) && near(r, 2f32.powf(0.25)),
            "半右，实测 ({l}, {r})"
        );
    }

    /// 20 种内置乐器都得真的出声：逐个渲染单音符歌曲，检查峰值不为零。
    /// 铜块一族（16..19）是 v6 新增的，光「采样文件在」不算实现，要能响。
    #[test]
    fn every_builtin_instrument_renders_audio() {
        let bank = SoundBank::discover();
        if !bank.is_ready() {
            return;
        }
        for ins in 0..BUILTIN_FILES.len() as u8 {
            let mut song = Song::new();
            song.set(0, 0, nbs_core::model::Note::new(ins, 45));
            song.recalculate();
            let events = build_event_table(&song, false);
            let (pcm, stats) = render_offline(&song, &bank, &events, &Default::default());
            assert_eq!(stats.notes, 1, "乐器 {ins} 没进事件表");
            assert!(
                pcm.iter().any(|&s| s.abs() > 0.01),
                "乐器 {ins}（{}）渲染出来是静音",
                BUILTIN_FILES[ins as usize]
            );
        }
    }

    #[test]
    fn engine_never_panics_without_device() {
        let mut e = CpalEngine::new();
        let song = Song::demo();
        e.prepare(&song);
        e.start(0.0);
        e.advance(0.01);
        e.stop();
        assert!(!e.name().is_empty());
    }
}
