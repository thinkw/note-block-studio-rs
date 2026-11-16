//! 音频引擎：cpal 实时输出 + 离线导出。
//!
//! 实时路径：`cpal` 回调 → `SharedState::render_interleaved` → 同一套 Mixer。
//! 离线路径：同一套 `Voice`/`playback_ratio` 公式，按音符分片多线程混音；
//! 时间轴/限幅/声像按打包版 nbswave 0.4.0 的语义来（`tempo_segments` 定位、
//! 仅峰值超 1 时整轨归一），因此导出的 WAV 与原版「导出音频」一致。

use std::path::Path;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nbs_core::model::{Note, PAN_CENTER, Song};

use crate::mixer::Voice;
use crate::samples::{SoundBank, BUILTIN_FILES, SAMPLE_RATE};
use crate::scheduler::{
    build_event_table, make_shared, EventKind, NoteEvent, Shared, TransportClock,
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
        let Some(&pos) = segs.get(ev.tick as usize) else { continue };
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
        let Some(sample) = bank.get(ev.note.instrument as usize) else { continue };
        if sample.is_empty() {
            continue;
        }
        let Some(&pos) = segs.get(ev.tick as usize) else { continue };
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
        assert!((stats.peak - 0.9).abs() < 1e-5, "未削顶不应缩放，实测 {}", stats.peak);
        // nbswave 时间轴：tick 0 落在 1/tempo = 0.1s（交错立体声 ×2）
        let start = (0.1 * 44100.0) as usize * 2;
        assert!((pcm[start] - 0.9).abs() < 1e-4, "首样本应原样 0.9，实测 {}", pcm[start]);
    }

    /// nbswave tempo_segments 的移植对照：10 t/s，tick 4 一枚变速器
    /// （pitch=600 → 40 t/s）。变速列自身的步长用新速度；第 0 列在 1/tempo。
    #[test]
    fn tempo_segments_match_nbswave() {
        let mut song = Song::new();
        song.meta.tempo = 10.0;
        song.custom_instruments.push(nbs_core::model::CustomInstrument {
            name: "Tempo Changer".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        song.set(0, 0, Note::new(0, 45));
        song.set(4, 0, Note { instrument: 20, key: 45, velocity: 100, panning: 100, pitch: 600 });
        song.set(8, 0, Note::new(0, 45));
        song.recalculate();
        let events = build_event_table(&song, false);
        let segs = tempo_segments(&song, &events);
        assert_eq!(segs.len(), 10, "歌长 9 → 覆盖 tick 0..=9");
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(near(segs[0], 0.1), "segments[0] = 1/tempo，实测 {}", segs[0]);
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
        song.custom_instruments.push(nbs_core::model::CustomInstrument {
            name: "Tempo Changer".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        song.set(8, 0, Note::new(0, 45));
        song.set(4, 1, Note { instrument: 20, key: 45, velocity: 100, panning: 100, pitch: 600 });
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
        assert!(near(l, 1.0) && near(r, 1.0), "居中必须不动，实测 ({l}, {r})");
        let (l, r) = pan_factors_pydub(-1.0);
        assert!(near(l, 2f32.sqrt()) && near(r, 0.0), "全左 = (√2, 0)，实测 ({l}, {r})");
        let (l, r) = pan_factors_pydub(1.0);
        assert!(near(l, 0.0) && near(r, 2f32.sqrt()), "全右 = (0, √2)，实测 ({l}, {r})");
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
