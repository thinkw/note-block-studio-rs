//! 音频引擎：cpal 实时输出 + 离线导出。
//!
//! 实时路径：`cpal` 回调 → `SharedState::render_interleaved` → 同一套 Mixer。
//! 离线路径：同一套 `Voice`/`playback_ratio`/`gains` 公式，按音符分片多线程混音。
//! 因此「听到的」与「导出的 WAV」音色完全一致。

use std::path::Path;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nbs_core::model::{Note, Song};

use crate::mixer::Voice;
use crate::samples::{SoundBank, BUILTIN_FILES, SAMPLE_RATE};
use crate::scheduler::{
    build_event_table, make_shared, NoteEvent, Shared, TransportClock,
};

// 注意：cpal 在 Windows/WASAPI 后端的 `Stream` 不是 Send（内部持有线程亲和的
// COM 对象）。引擎只在创建它的主线程使用，因此这里不要求 Send。
pub trait AudioEngine {
    fn name(&self) -> &str;
    /// 载入歌曲（构建事件表，必要时补载自定义采样）。
    fn prepare(&mut self, song: &Song);
    /// 原版 `play_sound()`：不动播放头，立刻用当前音色试听一个音。
    fn preview_note(&mut self, note: &Note);
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
    fn bank(&self) -> Arc<SoundBank>;
    fn sample_status(&self) -> String;
    /// 重新扫描采样目录（首选项改路径后调用）。
    fn reload_bank(&mut self, _dir: Option<&Path>) {}
}

pub struct CpalEngine {
    shared: Shared,
    clock: TransportClock,
    bank: Arc<SoundBank>,
    stream: Option<cpal::Stream>,
    device_info: String,
    last_song: Option<Song>,
    export_options: ExportOptions,
}

impl CpalEngine {
    /// 创建引擎。找不到音频设备时自动降级为静音时钟，绝不 panic。
    pub fn new() -> Self {
        Self::with_bank(SoundBank::discover())
    }

    pub fn with_bank(bank: SoundBank) -> Self {
        let bank = Arc::new(bank);
        let shared = make_shared((*bank).clone(), SAMPLE_RATE);
        let clock = TransportClock::with_shared(shared.clone());
        let mut engine = Self {
            shared,
            clock,
            bank,
            stream: None,
            device_info: "静音时钟（未初始化音频设备）".into(),
            last_song: None,
            export_options: ExportOptions::default(),
        };
        match engine.try_start_stream() {
            Ok(info) => engine.device_info = info,
            Err(e) => {
                engine.device_info = format!("静音时钟（{e}）");
                engine.shared.lock().audio_active = false;
            }
        }
        engine
    }

    fn try_start_stream(&mut self) -> Result<String, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "没有默认输出设备".to_string())?;
        let name = device.name().unwrap_or_else(|_| "未知设备".into());
        let supported = device
            .default_output_config()
            .map_err(|e| format!("读取输出配置失败: {e}"))?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let rate = config.sample_rate.0;
        let channels = config.channels as usize;

        {
            let mut st = self.shared.lock();
            st.sample_rate = rate;
            st.mixer.output_rate = rate;
        }

        let shared = self.shared.clone();
        let cb_shared = shared.clone();
        let err_shared = shared.clone();
        let stream = match sample_format {
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
                        eprintln!("[nbs-audio] 输出流错误: {e}");
                        err_shared.lock().audio_active = false;
                    },
                    None,
                )
                .map_err(|e| format!("创建 f32 输出流失败: {e}"))?,
            cpal::SampleFormat::I16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device
                    .build_output_stream::<i16, _, _>(
                        &config,
                        move |data, _| {
                            scratch.resize(data.len(), 0.0);
                            if let Some(mut st) = cb_shared.try_lock() {
                                st.render_interleaved(&mut scratch, channels);
                            } else {
                                scratch.fill(0.0);
                            }
                            for (d, &s) in data.iter_mut().zip(scratch.iter()) {
                                *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                            }
                        },
                        move |e| {
                            eprintln!("[nbs-audio] 输出流错误: {e}");
                            err_shared.lock().audio_active = false;
                        },
                        None,
                    )
                    .map_err(|e| format!("创建 i16 输出流失败: {e}"))?
            }
            other => return Err(format!("不支持的采样格式 {other:?}")),
        };
        stream.play().map_err(|e| format!("启动输出流失败: {e}"))?;
        // 必须把 Stream 保存到结构体：cpal 的 Stream 一旦 drop 就立刻停止输出。
        self.stream = Some(stream);
        self.shared.lock().audio_active = true;
        Ok(format!("{name} · {} Hz · {} 声道", rate, channels))
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    pub fn set_export_options(&mut self, opts: ExportOptions) {
        self.export_options = opts;
    }

    pub fn export_options(&self) -> &ExportOptions {
        &self.export_options
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
}

impl AudioEngine for CpalEngine {
    fn name(&self) -> &str {
        if self.stream.is_some() {
            "原版采样音频引擎"
        } else {
            "静音时钟（无音频设备）"
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
    fn preview_note(&mut self, note: &Note) {
        let mut st = self.shared.lock();
        let bank = st.bank.clone();
        // 试听固定按图层音量 100 / 平衡 100，跟原版 `play_sound(ins, key, 100, 100, 0)` 一致。
        // 层号 0：原版预览调用不带层号，emitter 的 soundlayer 默认 0，
        // 区间消音掐不到它（只有 pitch<=0 的全消音能掐到）。
        st.mixer.spawn(note, 100, 100, &bank, 0);
    }
    fn start(&mut self, from_tick: f64) {
        self.clock.seek(from_tick);
        self.clock.play();
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
        self.device_info.clone()
    }
    fn bank(&self) -> Arc<SoundBank> {
        self.bank.clone()
    }
    fn sample_status(&self) -> String {
        if self.bank.is_ready() {
            format!(
                "已加载 {}/{} 个内置采样{}",
                self.bank.loaded_count(),
                BUILTIN_FILES.len(),
                if self.bank.missing.is_empty() { "" } else { "（部分缺失）" }
            )
        } else {
            "未找到内置采样目录".into()
        }
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
    events: usize,
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
    pub fn event_count(&self) -> usize {
        self.events
    }
}

impl AudioEngine for SilentEngine {
    fn name(&self) -> &str {
        "静音时钟"
    }
    fn prepare(&mut self, song: &Song) {
        self.events = build_event_table(song, false).len();
        self.clock.set_tempo(song.meta.tempo);
        self.clock.set_loop(
            song.meta.loop_enabled,
            song.meta.loop_start as f64,
            song.song_length().max(1) as f64,
            song.meta.loop_max,
        );
    }
    fn preview_note(&mut self, _note: &Note) {
        // 没有输出设备，试听无处可去；时钟照常推进。
    }
    fn start(&mut self, from_tick: f64) {
        self.clock.seek(from_tick);
        self.clock.play();
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
        0
    }
    fn clock(&self) -> &TransportClock {
        &self.clock
    }
    fn clock_mut(&mut self) -> &mut TransportClock {
        &mut self.clock
    }
    fn device_info(&self) -> String {
        "无音频输出（纯时钟）".into()
    }
    fn bank(&self) -> Arc<SoundBank> {
        self.bank.clone()
    }
    fn sample_status(&self) -> String {
        "未加载采样".into()
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
    /// 末尾留白秒数，让长采样完整衰减。
    pub tail_seconds: f32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels: 2,
            include_locked: false,
            tail_seconds: 2.0,
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

/// 渲染成交错 f32 PCM（可复用于其它容器编码）。
pub fn render_offline(
    song: &Song,
    bank: &SoundBank,
    events: &[NoteEvent],
    opts: &ExportOptions,
) -> (Vec<f32>, ExportStats) {
    let rate = opts.sample_rate.max(8000);
    let channels = opts.channels.clamp(1, 2) as usize;
    let tempo = song.meta.tempo.max(0.01) as f64;
    let song_seconds = song.song_length() as f64 / tempo;
    let total_seconds = song_seconds + opts.tail_seconds as f64;
    let frames = (total_seconds * rate as f64).ceil() as usize;

    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
        .min(events.len().max(1));
    let chunk = events.len().div_ceil(thread_count).max(1);
    let mut partials: Vec<Vec<f32>> = Vec::with_capacity(thread_count);
    let bank_ref = &bank;
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(thread_count);
        for group in events.chunks(chunk) {
            let handle = scope.spawn(move || {
                let mut buf = vec![0.0f32; frames * channels];
                render_event_group(group, bank_ref, &mut buf, frames, channels, rate, tempo);
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
            duration_seconds: total_seconds,
            peak,
            threads: thread_count,
        },
    )
}

fn render_event_group(
    events: &[NoteEvent],
    bank: &SoundBank,
    buf: &mut [f32],
    frames: usize,
    channels: usize,
    rate: u32,
    tempo: f64,
) {
    for ev in events {
        let Some(sample) = bank.get(ev.note.instrument as usize) else { continue };
        if sample.is_empty() {
            continue;
        }
        let start = ((ev.tick as f64 / tempo) * rate as f64) as usize;
        if start >= frames {
            continue;
        }
        let ratio = Voice::playback_ratio(&ev.note, sample.base_key, rate);
        // 导出不用播放主音量：原版 nbswave 没有 master 概念，音符增益
        // 只由层音量 × 力度决定（mono 时 pydub 会把两声道取平均）。
        let (gl, gr) = Voice::gains(&ev.note, ev.layer_volume, ev.layer_stereo, 1.0, channels == 1);
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
    std::fs::write(path, out).map_err(|e| format!("写入 WAV 失败: {e}"))
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

    #[test]
    fn offline_render_produces_nonzero_audio() {
        let bank = SoundBank::discover();
        if !bank.is_ready() {
            return;
        }
        let song = Song::demo();
        let events = build_event_table(&song, false);
        let opts = ExportOptions { tail_seconds: 0.5, ..Default::default() };
        let (pcm, stats) = render_offline(&song, &bank, &events, &opts);
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
        assert!((stats.peak - 0.9).abs() < 1e-5, "未削顶不应缩放，实测 {}", stats.peak);
        assert!((pcm[0] - 0.9).abs() < 1e-4, "首样本应原样 0.9，实测 {}", pcm[0]);
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
