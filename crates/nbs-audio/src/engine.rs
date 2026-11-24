//! 音频引擎：cpal 实时输出 + 离线导出。
//!
//! 实时路径：`cpal` 回调 → `SharedState::render_interleaved` → 同一套 Mixer。
//! 离线路径：同一套 `Voice`/`playback_ratio` 公式，按音符分片多线程混音；
//! 时间轴/限幅/声像按打包版 nbswave 0.4.0 的语义来（`tempo_segments` 定位、
//! 仅峰值超 1 时整轨归一），因此导出的 WAV 与原版「导出音频」一致。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nbs_core::model::{Note, Song, PAN_CENTER};

use crate::mixer::Voice;
use crate::reference::ReferenceTrack;
use crate::samples::{SoundBank, BUILTIN_FILES, SAMPLE_RATE};
use crate::scheduler::{
    build_event_table, make_shared, EventKind, NoteEvent, Shared, TransportClock, VisualEvent,
};

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
    /// 原版 `get_seconds_from_tick(marker_pos)`：参考音频按歌曲时间定位。
    fn position_seconds(&self) -> f64 {
        self.position_ticks() / self.clock().tempo().max(0.01) as f64
    }
    fn advance(&mut self, dt: f64);
    fn active_voices(&self) -> usize;
    fn clock(&self) -> &TransportClock;
    fn clock_mut(&mut self) -> &mut TransportClock;
    fn device_info(&self) -> String;
    /// 系统里可选的输出设备。原版把声音整个交给 GameMaker，只会走系统默认设备，
    /// 没有选择项；这一组方法是端口自加（用户要求 #94）。没有可枚举后端的实现返回空表。
    fn output_devices(&self) -> Vec<OutputDevice> {
        Vec::new()
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
    /// 换输出设备，`None` = 跟随系统默认。当场拆旧流、当场建新流：本机实测连拆带建约
    /// 0.1 秒，且只在用户点那一下付，所以没有「正在连接」这个中间态。
    /// `Ok` 之后接上了哪台问 [`Self::output_name`]；`Err` 里是给状态栏看的原因。
    fn set_output(&mut self, _wanted: Option<String>) -> Result<(), String> {
        Err(nbs_core::tr(
            "this audio engine has no switchable output device",
            "当前音频引擎没有可切换的输出设备",
        )
        .to_string())
    }
    /// 系统默认输出变了（插耳机、切蓝牙）就跟随过去；之前没建成流、现在系统又有可用
    /// 设备了就补一次。内部限流，每帧调用即可。返回换到的新设备名，没变化返回 `None`。
    fn poll_output(&mut self) -> Option<String> {
        None
    }
    /// 取走一句「自动跟随 / 补建那几趟没成」的原因（同一趟只报一次）。用户主动点的
    /// 那次切换不当场返 `Err`，走 [`Self::set_output`]。
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
    /// 原版 `load_reference_audio`：把当前标签的参考音频装进输出流。
    fn load_reference_audio(&mut self, _path: &Path) -> Result<(), String> {
        Err(nbs_core::tr(
            "this audio engine cannot load reference audio",
            "当前音频引擎不能载入参考音频",
        )
        .to_string())
    }
    fn clear_reference_audio(&mut self) {}
    /// 原版 `reference_audio_sync`：按歌曲播放头、偏移和静音/独奏状态同步参考轨。
    fn sync_reference_audio(
        &mut self,
        _song_playing: bool,
        _position_seconds: f64,
        _offset_ms: i64,
        _volume: u8,
        _master: f32,
        _mode: u8,
        _force: bool,
    ) {
    }
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
    /// 还没尝试建流。
    Uninitialised,
}
/// 一台可选的输出设备。`id` 是系统给的稳定标识（Windows 上是端点 ID，形如
/// `wasapi:{0.0.0.00000000}.{GUID}`），首选项存它 —— 改名、重插、换驱动都还认得同一台；
/// `name` 由系统给、只给人看。个别后端拿不到 ID 时 `id` 是空串，那就只能按名字认。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// cpal 0.18 把 `Device::name()` 收进了 `description()`，取名字这一步统一走这里。
fn device_name(device: &cpal::Device) -> Result<String, cpal::Error> {
    device.description().map(|d| d.name().to_owned())
}

/// 系统给一台设备的稳定标识（cpal 的 `DeviceId` 串，`host:后端 ID`）。
fn device_id(device: &cpal::Device) -> Option<String> {
    device.id().ok().map(|id| id.to_string())
}

/// 枚举系统里的输出设备。拿不到后端时返回空表（不报错：没设备只是没得选）。
pub fn list_output_devices() -> Vec<OutputDevice> {
    let host = cpal::default_host();
    let default = host.default_output_device();
    let default_id = default.as_ref().and_then(device_id);
    let default_name = default.as_ref().and_then(|d| device_name(d).ok());
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut out: Vec<OutputDevice> = Vec::new();
    for device in devices {
        let Ok(name) = device_name(&device) else {
            continue;
        };
        if out.iter().any(|o| o.name == name) {
            continue;
        }
        let id = device_id(&device).unwrap_or_default();
        // 认「哪台是系统默认」优先比 ID：`default_output_device()` 给的是「跟随默认」
        // 那个虚拟句柄，名字撞得上不代表是同一台。
        let is_default = match (id.is_empty(), default_id.as_ref()) {
            (false, Some(default_id)) => &id == default_id,
            _ => Some(&name) == default_name.as_ref(),
        };
        out.push(OutputDevice {
            id,
            name,
            is_default,
        });
    }
    out
}

/// 这台设备是不是 `wanted` 指的那一台：先认稳定 ID，再认名字（老偏好存的是名字）。
fn is_the_wanted(device: &cpal::Device, wanted: &str) -> bool {
    device_id(device).as_deref() == Some(wanted)
        || device_name(device).ok().as_deref() == Some(wanted)
}

/// 把一个标识（稳定 ID，或老偏好里的设备名）翻回给人看的那句设备名，认不出原样给回。
/// 只在报错那几趟调用（要枚举一次，实测 2.6~4.0 毫秒），别放进每帧的路径。
pub fn device_label(id_or_name: &str) -> String {
    list_output_devices()
        .into_iter()
        .find(|d| d.id == id_or_name || d.name == id_or_name)
        .map(|d| d.name)
        .unwrap_or_else(|| id_or_name.to_string())
}

/// 系统当前的默认输出设备名（一台设备都没有时 `None`）。
fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| device_name(&d).ok())
}

/// 取一台输出设备：`wanted` 为空就拿系统默认；指定时先比对默认（省一次枚举），再按
/// cpal 的稳定 ID 反查，最后按名字找（老偏好存的是名字）。找不到才报错，
/// 由调用方决定回落策略。
fn open_output_device(wanted: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let Some(wanted) = wanted.filter(|w| !w.is_empty()) else {
        return host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string());
    };
    let found = host
        .default_output_device()
        .filter(|d| is_the_wanted(d, wanted))
        .or_else(|| {
            wanted
                .parse::<cpal::DeviceId>()
                .ok()
                .and_then(|id| host.device_by_id(&id))
        })
        .or_else(|| {
            host.output_devices()
                .ok()?
                .find(|d| is_the_wanted(d, wanted))
        });
    match found {
        Some(device) => Ok(device),
        None => Err(format!(
            "output device \"{}\" is not available",
            device_label(wanted)
        )),
    }
}

/// 一趟「取设备 + 建流」做完后留下的结论。
pub struct StreamOutcome {
    /// 这一趟回应的是哪个请求（`None` = 跟随系统默认），回落那句提示要用它拼。
    pub for_wanted: Option<String>,
    /// 真正接上的设备（名 / 采样率 / 声道数）；`None` = 这趟没出声。
    pub built: Option<(String, u32, usize)>,
    /// 建流失败的原因（cpal 报错原文，按技术原文显示）。
    pub error: Option<String>,
    /// 点名的设备没拿到，已经回落成跟随系统默认：界面要跟着把偏好改回去，
    /// 别存一个并没有生效的点名下标。
    pub fell_back: bool,
}

/// 建流那一步的注入点：收到「想要的设备标识（`None` = 系统默认）」，返回这一趟的结论
/// 加上建好的流。用例往里塞假替身，就不必真去碰音频硬件。
type Connect =
    dyn Fn(&Shared, Option<String>) -> (StreamOutcome, Option<cpal::Stream>) + Send + 'static;

/// 一趟建流：按 `wanted` 找设备、开流。点名的设备拿不到就回落到系统默认——宁可换设备
/// 出声，也不要因为一次选择把声音整个丢掉；连默认都没有才降级成静音时钟。
/// 真建出来的流交给调用方（引擎的 `stream` 字段）持有。
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

/// 后端投进错误回调的那件事，是不是「这条流到此为止了」。
///
/// cpal 0.18.2 的 WASAPI 只会投「到此为止」那几种：默认设备换了投 `StreamInvalidated`
/// （`host/wasapi/stream.rs:730`：它从不重绑 IAudioClient，所以实话实说这条流已经废了）、
/// 点名的那台掉了投 `DeviceNotAvailable`（`host/wasapi/mod.rs:63` 把
/// `AUDCLNT_E_DEVICE_INVALIDATED` 映射成它）。
/// 剩下这三种是别的后端（ALSA / CoreAudio / ASIO）的「抖一下，我自己在恢复」：
/// `Xrun` 之后 ALSA 自己 prepare() 回来、`DeviceChanged` 文档明说流还活着不用重建、
/// `RealtimeDenied` 只说优先级没提上去。跟着它们停表拆流，等于我们把还在出声的播放掐掉。
fn stream_is_dead(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

/// 音频线程投过来的错误：只记账，不在回调里动流。
fn on_stream_error(shared: &Shared, e: cpal::Error) {
    let kind = e.kind();
    if !stream_is_dead(kind) {
        eprintln!("[nbs-audio] output stream hiccup ({kind:?}): {e}");
        return;
    }
    eprintln!("[nbs-audio] output stream ended ({kind:?}): {e}");
    let mut st = shared.lock();
    // 撤掉「有音频回调在驱动播放头」这个前提：不撤的话流一死，播放头就冻在那儿不动了。
    st.audio_active = false;
    st.stream_needs_rebuild = true;
}

/// 在一台具体设备上建输出流，建好的流连同结论一起交回。
fn open_stream_on(
    device: &cpal::Device,
    shared: &Shared,
    for_wanted: Option<String>,
) -> (StreamOutcome, Option<cpal::Stream>) {
    let name = device_name(device).unwrap_or_else(|_| "unknown device".to_string());
    let supported = match device.default_output_config() {
        Ok(supported) => supported,
        Err(e) => return stream_failed(for_wanted, format!("failed to read output config: {e}")),
    };
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let rate = config.sample_rate;
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
                config,
                move |data, _| {
                    if let Some(mut st) = cb_shared.try_lock() {
                        st.render_interleaved(data, channels);
                    } else {
                        data.fill(0.0);
                    }
                },
                move |e| on_stream_error(&err_shared, e),
                None,
            )
            .map_err(|e| format!("failed to build f32 output stream: {e}")),
        cpal::SampleFormat::I16 => {
            let mut scratch: Vec<f32> = Vec::new();
            let render_shared = shared.clone();
            device
                .build_output_stream::<i16, _, _>(
                    config,
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
                    move |e| on_stream_error(&err_shared, e),
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

/// 两次自动重试之间的最小间隔。设备出事不靠我们猜（cpal 的错误回调会投过来），这一档
/// 只管两件事：一条流都没有的时候隔一会儿回头看一眼系统默认出现了没；以及挡住
/// 「建起来就死」的坏设备把界面线程每帧都拖去重建一遍（建一条要 0.1 秒）。
const OUTPUT_RETRY: Duration = Duration::from_millis(1000);
pub struct CpalEngine {
    shared: Shared,
    clock: TransportClock,
    bank: Arc<SoundBank>,
    /// 唯一那条出声的流。cpal 0.17 起 WASAPI 的 `Stream` 是 `Send + Sync`，引擎拿得住它；
    /// 一 drop 就停声，所以换设备时先把它放掉再建新的一条。
    stream: Option<cpal::Stream>,
    /// 建流那一步（真实现是 [`connect_output`]，用例换成假替身）。
    connect: Box<Connect>,
    device: DeviceReport,
    /// 自动跟随 / 补建那几趟没成时留下的原因，等着界面取走（同一趟只报一次）。
    stream_error: Option<String>,
    /// 用户点名的输出设备标识（[`OutputDevice::id`]，老偏好里可能是设备名）；
    /// `None` = 跟随系统默认。原版没有这个概念（声音整个交给 GameMaker，
    /// 只会落在系统默认设备上），端口自加 #94。
    wanted: Option<String>,
    /// 上一次自动重试的时戳（见 [`OUTPUT_RETRY`]），和上一次建流时系统默认是哪台
    /// —— 同一台默认撞过一次就不必每秒再撞一次。
    probe_at: Instant,
    tried_default: Option<String>,
    last_song: Option<Song>,
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
    /// 于是不必真碰声卡也能验「点名拿不到要回落」「建流失败不许装作还在出声」这几条。
    fn start(bank: SoundBank, wanted: Option<String>, connect: Box<Connect>) -> Self {
        let bank = Arc::new(bank);
        let shared = make_shared((*bank).clone(), SAMPLE_RATE);
        let clock = TransportClock::with_shared(shared.clone());
        let mut engine = Self {
            shared,
            clock,
            bank,
            stream: None,
            connect,
            device: DeviceReport::Uninitialised,
            stream_error: None,
            wanted,
            probe_at: Instant::now(),
            tried_default: None,
            last_song: None,
        };
        let _ = engine.reconnect();
        // 启动这一趟的成败不进状态栏：那会儿窗口还没画起来，没人看得见。设备那一格
        // 会如实写接上了什么，点名那台拿不到也一样（`wanted` 已跟着回落清空）。
        engine.stream_error = None;
        engine
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
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

    pub fn load_reference_audio(&mut self, path: &Path) -> Result<(), String> {
        self.shared.lock().reference.clear();
        let decoded = ReferenceTrack::decode(path)?;
        self.shared.lock().reference = decoded;
        Ok(())
    }

    pub fn clear_reference_audio(&mut self) {
        self.shared.lock().reference.clear();
    }

    pub fn sync_reference_audio(
        &mut self,
        song_playing: bool,
        position_seconds: f64,
        offset_ms: i64,
        volume: u8,
        master: f32,
        mode: u8,
        force: bool,
    ) {
        self.shared.lock().reference.sync(
            song_playing,
            position_seconds,
            offset_ms,
            volume,
            master,
            mode,
            force,
        );
    }

    /// 换输出设备：`None` = 跟随系统默认。当场拆旧流、当场建新流，返回时结果已经落地
    /// （本机实测连拆带建约 0.1 秒，只在用户点那一下付）。
    pub fn set_output(&mut self, wanted: Option<String>) -> Result<(), String> {
        self.wanted = wanted.filter(|w| !w.is_empty());
        self.reconnect().map(|_| ())
    }

    /// 取走一句「自动跟随 / 补建那几趟没成」的原因（同一趟只报一次）。
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

    /// 已知的输出设备。cpal 0.18 在本机实测枚举一趟 2.6~4.0 毫秒（0.15 要 1.3 秒），
    /// 直接在调用方拿就行，不必挪到后台。
    pub fn output_devices(&self) -> Vec<OutputDevice> {
        list_output_devices()
    }

    /// 拆掉旧流、按 `wanted` 当场建新的一条。点名的设备拿不到就回落到系统默认，
    /// 这套策略在 [`connect_output`] 里。返回换到的新设备名（该报给界面那句）；
    /// 回落、以及名字本来就在报的不返。失败返 `Err`，原因给状态栏。
    fn reconnect(&mut self) -> Result<Option<String>, String> {
        // 先把旧流放掉再建：两条流同时从一份调度器里拉数据，播放时钟会被推快一倍。
        // cpal 的 `drop` 会等音频线程收工，所以这一步之后旧流再不会往信号位上记东西；
        // 记在那儿的事件这一趟就当着已处理掉。
        self.stream = None;
        self.shared.lock().stream_needs_rebuild = false;
        self.tried_default = default_output_name();
        let (outcome, stream) = (self.connect)(&self.shared, self.wanted.clone());
        let announced = match &outcome.built {
            Some((name, _, _)) if !outcome.fell_back => Some(name.clone()),
            _ => None,
        };
        self.stream = stream;
        self.apply_stream(&outcome);
        match &self.device {
            DeviceReport::Streamed { .. } => Ok(announced),
            DeviceReport::Fallback(reason) => Err(reason.clone()),
            DeviceReport::Uninitialised => Err("audio output is not initialised".to_string()),
        }
    }

    /// 到底有没有在出声？只认最近一趟落地的结论（用例里的假替身不真建流）。
    fn stream_live(&self) -> bool {
        matches!(self.device, DeviceReport::Streamed { .. })
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
                // 旧流在这一步之前已经被放掉（drop 即停声），这里得把「有音频回调在驱动
                // 播放头」这个前提一起撤掉，否则播放头冻着不动却装作还在出声。
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
                "output device \"{}\" is not available, now on {name}",
                device_label(&requested)
            ));
        } else if let Some(e) = &outcome.error {
            self.stream_error = Some(e.clone());
        }
    }

    /// 设备出事了没有？出事与否由 cpal 的错误回调在音频线程上记一笔（[`on_stream_error`]），
    /// 这里在界面线程取走、当场重建一条 —— 拆流建流这种事不能放在回调里做，那是它自己
    /// 那条流。返回换到的新设备名（该报给界面那句）。
    ///
    /// 还留着一条时间闸的理由：一条流都没有的时候（开机就没设备、系统默认被全禁用），
    /// 没有任何东西会来通知我们，只能隔一会儿回头看一眼系统默认出现了没。这条闸同时
    /// 挡住「建起来就死」的坏设备把每帧都变成一趟注定失败的重建。
    fn poll_output_change(&mut self) -> Option<String> {
        let died = self.shared.lock().stream_needs_rebuild;
        // 有流、又没出事：一个字节都不探测。以前这一档每秒要读一次系统默认设备名，
        // 点名的那台还要每秒全量枚举一次——cpal 0.18 之后这些都不要了，事件会告诉我们。
        if !died && self.stream_live() {
            return None;
        }
        if self.probe_at.elapsed() < OUTPUT_RETRY {
            return None;
        }
        self.probe_at = Instant::now();
        if died {
            return self.reconnect().ok().flatten();
        }
        // 走到这里是一条流都没有：系统有了可用的默认、而且这台还没撞过，才补建一次。
        let seen = default_output_name();
        if seen.is_some() && seen != self.tried_default {
            self.reconnect().ok().flatten()
        } else {
            None
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
        self.shared.lock().reference.stop();
    }
    fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }
    fn position_ticks(&self) -> f64 {
        self.clock.position()
    }
    fn position_seconds(&self) -> f64 {
        self.shared.lock().seconds_at(self.clock.position())
    }
    fn advance(&mut self, _dt: f64) {
        self.clock.update();
    }
    fn active_voices(&self) -> usize {
        self.shared.lock().mixer.active()
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
    fn output_name(&self) -> Option<String> {
        CpalEngine::output_name(self).map(str::to_string)
    }
    fn wanted_output(&self) -> Option<&str> {
        CpalEngine::wanted_output(self)
    }
    fn set_output(&mut self, wanted: Option<String>) -> Result<(), String> {
        CpalEngine::set_output(self, wanted)
    }
    fn poll_output(&mut self) -> Option<String> {
        self.poll_output_change()
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
    fn load_reference_audio(&mut self, path: &Path) -> Result<(), String> {
        CpalEngine::load_reference_audio(self, path)
    }
    fn clear_reference_audio(&mut self) {
        CpalEngine::clear_reference_audio(self);
    }
    fn sync_reference_audio(
        &mut self,
        song_playing: bool,
        position_seconds: f64,
        offset_ms: i64,
        volume: u8,
        master: f32,
        mode: u8,
        force: bool,
    ) {
        CpalEngine::sync_reference_audio(
            self,
            song_playing,
            position_seconds,
            offset_ms,
            volume,
            master,
            mode,
            force,
        );
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
    /// 参考轨仍真实解码，便于无设备环境验证加载/偏移/静音状态。
    reference: ReferenceTrack,
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
        self.reference.stop();
    }
    fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }
    fn position_ticks(&self) -> f64 {
        self.clock.position()
    }
    fn position_seconds(&self) -> f64 {
        let mut segment_start = 0.0;
        let mut tempo = self.clock().tempo().max(0.25) as f64;
        let tick = self.clock.position().max(0.0);
        let mut seconds = 0.0;
        for event in &self.events {
            let at = event.tick as f64;
            if at > tick {
                break;
            }
            if let EventKind::Tempo(pitch) = event.kind {
                seconds += (at - segment_start) / tempo;
                segment_start = at;
                tempo = (pitch.unsigned_abs() as f64 / 15.0).max(0.25);
            }
        }
        seconds + (tick - segment_start) / tempo
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
    fn load_reference_audio(&mut self, path: &Path) -> Result<(), String> {
        self.reference.load(path)
    }
    fn clear_reference_audio(&mut self) {
        self.reference.clear();
    }
    fn sync_reference_audio(
        &mut self,
        song_playing: bool,
        position_seconds: f64,
        offset_ms: i64,
        volume: u8,
        master: f32,
        mode: u8,
        force: bool,
    ) {
        self.reference.sync(
            song_playing,
            position_seconds,
            offset_ms,
            volume,
            master,
            mode,
            force,
        );
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
    /// 否则每次启动都会去点一台已经不存在的设备。
    #[test]
    fn dead_output_choice_does_not_stick() {
        let mut engine = CpalEngine::new();
        let missing = "nbs-rust 这台机器上不存在的输出设备".to_string();
        // 这台机器一台输出设备都没有时这趟会直接失败，那不是本用例要钉的东西。
        let _ = engine.set_output(Some(missing.clone()));
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

    /// 假建流：只回结论，绝不碰真实声卡（真接上的是哪台是环境实情，钉不住）。
    /// 点下去 `set_output` 返回时结果就该落地 —— 拆旧流 + 建新流实测约 0.1 秒，
    /// 只在用户点那一下付，所以产品里没有「正在连接」这一档。
    #[test]
    fn a_switch_lands_on_the_spot() {
        let connect = Box::new(|_shared: &Shared, wanted: Option<String>| match wanted {
            None => built_outcome(None, "系统默认"),
            Some(name) => built_outcome(Some(name.clone()), &name),
        });
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        assert_eq!(engine.output_name().as_deref(), Some("系统默认"));
        assert_eq!(engine.poll_output(), None, "没换过设备不许凭空报一个新名字");

        engine.set_output(Some("假设备".to_string())).unwrap();
        assert_eq!(
            engine.output_name().as_deref(),
            Some("假设备"),
            "set_output 返回时就得已经接上"
        );
        assert_eq!(engine.wanted_output(), Some("假设备"));
        assert_eq!(engine.poll_output(), None, "报过的那一趟不许再报一遍");
    }

    /// 点名的设备拿不到：当场回落系统默认，`wanted` 跟着清空并留下一句原因，
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
        assert_eq!(engine.output_name(), None);
        assert!(
            !engine.shared().lock().audio_active,
            "没建成流就不许还挂在「有音频回调驱动播放头」上"
        );
    }

    /// 数得出建了几趟流的假替身：接上的永远是同一台「假设备」，只用来数次数。
    fn counted_connect(tally: Arc<std::sync::atomic::AtomicUsize>) -> Box<Connect> {
        Box::new(move |_shared: &Shared, wanted: Option<String>| {
            tally.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            built_outcome(wanted, "假设备")
        })
    }

    /// 阶段 2：流废了这件事由 cpal 在音频线程上记一笔，界面线程下一帧取走就重建，
    /// 不再靠每秒去猜。取走之后信号位要跟着清掉，否则每帧重建一遍。
    #[test]
    fn a_dying_stream_is_replaced_on_the_next_poll() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine =
            CpalEngine::start(SoundBank::empty(), None, counted_connect(calls.clone()));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "启动建一趟");
        // 本机 WASAPI 在「系统默认换了」那一趟投的就是这个。
        on_stream_error(
            engine.shared(),
            cpal::Error::new(cpal::ErrorKind::StreamInvalidated),
        );
        assert!(
            !engine.shared().lock().audio_active,
            "流废了就不许还挂在「有音频回调驱动播放头」上"
        );
        engine.probe_at = Instant::now() - OUTPUT_RETRY * 2;
        assert_eq!(
            engine.poll_output().as_deref(),
            Some("假设备"),
            "取走事件当场重建，并把接上的那台报出去"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2, "一趟事件只重建一趟");
        assert!(
            !engine.shared().lock().stream_needs_rebuild,
            "取走了就该清掉，别留到下一帧再建一遍"
        );
        assert_eq!(engine.poll_output(), None, "报过的那一趟不许再报一遍");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// 阶段 2：后端投「抖一下，我自己在恢复」那几类事件时，既不许停表也不许拆流。
    /// WASAPI 不投这三种，投的是 ALSA / CoreAudio / ASIO —— 跟着它们停播放，等于
    /// 我们亲手把一条还在出声的流掐死。
    #[test]
    fn a_transient_event_neither_stops_the_clock_nor_rebuilds() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut engine =
                CpalEngine::start(SoundBank::empty(), None, counted_connect(calls.clone()));
            engine.shared().lock().audio_active = true;
            on_stream_error(engine.shared(), cpal::Error::new(kind));
            assert!(
                engine.shared().lock().audio_active,
                "{kind:?} 之后播放头还该由音频回调驱动"
            );
            assert!(
                !engine.shared().lock().stream_needs_rebuild,
                "{kind:?} 不该要求重建"
            );
            engine.probe_at = Instant::now() - OUTPUT_RETRY * 2;
            assert_eq!(engine.poll_output(), None, "{kind:?} 不该触发重建");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "{kind:?} 之后一条流都不该动"
            );
        }
    }

    /// 阶段 2 要省掉的那笔：有流又没出事，就绝不因为「过了一秒」再去读默认设备名、
    /// 再去全量枚举、再凭猜重建。改造前点名一台（用例里这台假设备真表里没有）的设备，
    /// 每秒都会被判成「掉线了」而重建一趟。
    #[test]
    fn a_live_stream_is_never_rebuilt_by_the_clock() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = CpalEngine::start(
            SoundBank::empty(),
            Some("假设备".to_string()),
            counted_connect(calls.clone()),
        );
        assert_eq!(engine.wanted_output(), Some("假设备"));
        for _ in 0..8 {
            engine.probe_at = Instant::now() - OUTPUT_RETRY * 2;
            assert_eq!(engine.poll_output(), None);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "点名的那台还在出声，一次都不该重建"
        );
    }

    /// 一条流都没有，且撞的还是同一条默认设备：不每秒傻撞（撞一次就往状态栏塞一句重复
    /// 的失败原因）。要等默认换个名儿——插了台新设备成了默认——才再试。
    #[test]
    fn a_bare_default_is_not_retried_every_second() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let tally = calls.clone();
        let connect = Box::new(move |_shared: &Shared, wanted: Option<String>| {
            tally.fetch_add(1, Ordering::SeqCst);
            stream_failed(wanted, "no audio device at all".to_string())
        });
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        assert_eq!(engine.output_name(), None);
        for _ in 0..3 {
            engine.probe_at = Instant::now() - OUTPUT_RETRY * 2;
            assert_eq!(engine.poll_output(), None);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "同一台默认不该反复撞");
    }

    /// 开机就没设备、后来插上来了：没有任何东西会来通知一个根本没建的流，只能隔一会儿
    /// 回头看一眼——但只在「系统默认换了 / 出现了」那一刻回头。
    #[test]
    fn a_stream_is_made_as_soon_as_a_default_shows_up() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        let gate = ready.clone();
        let tally = calls.clone();
        let connect = Box::new(move |_shared: &Shared, wanted: Option<String>| {
            tally.fetch_add(1, Ordering::SeqCst);
            if gate.load(Ordering::SeqCst) {
                built_outcome(wanted, "后来插上那台")
            } else {
                stream_failed(wanted, "no audio device at all".to_string())
            }
        });
        let mut engine = CpalEngine::start(SoundBank::empty(), None, connect);
        ready.store(true, Ordering::SeqCst);
        // 当成「开机时系统压根没有默认设备」：现在系统里那台就是新出现的。
        engine.tried_default = None;
        engine.probe_at = Instant::now() - OUTPUT_RETRY * 2;
        let before = calls.load(Ordering::SeqCst);
        let reported = engine.poll_output();
        let has_default = default_output_name().is_some();
        assert_eq!(
            calls.load(Ordering::SeqCst) > before,
            has_default,
            "系统有可用默认才补建，没默认补了也是白补"
        );
        if has_default {
            assert_eq!(
                reported.as_deref(),
                Some("后来插上那台"),
                "补建成功要报出新设备"
            );
        } else {
            assert_eq!(reported, None);
        }
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
