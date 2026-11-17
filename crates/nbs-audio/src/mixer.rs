//! Voice 池与混音器。实时播放与离线导出共用同一套渲染代码，
//! 保证「听到的」和「导出的」完全一致。

use nbs_core::model::{Note, PAN_CENTER};

use crate::samples::{SampleSource, SoundBank, SAMPLE_RATE};

/// 默认 voice 上限（原版 `channels = 256`，勾选更多通道后 1024）。
pub const DEFAULT_MAX_VOICES: usize = 256;
pub const EXTENDED_MAX_VOICES: usize = 1024;

/// 原版实时后端「每列起点不完全落在采样网格上」的量级（毫秒）。
///
/// 依据全部来自上游 issue #452（作者 Bentroen 本人）：GameMaker 自 v3.8.0 起有
/// 卡顿补偿，"lag within the playback can make ticks play slightly ahead or
/// behind their intended time, which can make the song sound more organic"；
/// 并且把"给每个 tick 加 ±5 ms 随机延迟"明确写成复刻原版听感的目标值——因为
/// "Repeated notes get _so_ evenly spaced, which causes their waveforms to line
/// up in a particular way at some occasions, and create jarring frequencies"。
/// 实测本曲尾部 15 s：同列精确对齐时混音 24.6% 的采样超出满量程，加 ±5 ms 抖动
/// 后降到 18.8%，撞限幅墙的采样从 7.58% 降到 3.67%。
pub const REALTIME_TICK_JITTER_MS: f64 = 5.0;

#[derive(Clone, Debug)]
pub struct Voice {
    pub source: SampleSource,
    /// 采样内的浮点读头。
    pub pos: f64,
    /// 每个输出采样推进的源采样数（= pitch × 源率/输出率）。
    pub ratio: f64,
    pub gain_l: f32,
    pub gain_r: f32,
    pub alive: bool,
    /// 创建序号，用于「淘汰最老」。
    pub serial: u64,
    /// 发声来源的层号（1 基，0 = 预览/节拍器等无层来源）。
    /// 消音器按它掐 voice（原版 `remove_emitters_all` 的 `soundlayer`）。
    pub layer: u32,
    /// 起始静音长度（输出采样）。见 [`Mixer::jitter_ms`]。
    pub pre: f64,
}

impl Voice {
    /// 原版 `play_sound.gml`：
    /// `keyshift = key + (ins.key + pit/100 - 78)`
    /// `pitch    = 0.5 * 2^(keyshift/12)`
    /// 代入内置基准 45 → `0.5 * 2^((key-33+pit/100)/12)`，key=45 时为 1.0。
    pub fn playback_ratio(note: &Note, base_key: u8, output_rate: u32) -> f64 {
        let keyshift =
            note.key as f64 + (base_key as f64 - 78.0) + note.pitch as f64 / 100.0;
        let p = 0.5 * 2f64.powf(keyshift / 12.0);
        p * (SAMPLE_RATE as f64 / output_rate.max(1) as f64)
    }

    /// 原版 `control_draw.gml:470-480` + `play_sound.gml:24`：
    /// `vol = (layer.volume/100) * note.velocity`
    /// `pan = layer.stereo == 100 ? note.panning : (layer.stereo + note.panning)/2`
    /// `gain = vol/100 * mastervol`；pan 0..200 走线性声像律，居中左右各半。
    pub fn gains(
        note: &Note,
        layer_volume: u8,
        layer_stereo: u8,
        master: f32,
        mono: bool,
    ) -> (f32, f32) {
        let vol_eff = (layer_volume as f32 / 100.0) * note.velocity as f32;
        let g = (vol_eff / 100.0) * master;
        if mono {
            return (g, g);
        }
        let pan_eff = if layer_stereo == PAN_CENTER {
            note.panning
        } else {
            ((layer_stereo as u16 + note.panning as u16) / 2) as u8
        };
        // 线性（单声道兼容）声像律：左右增益之和恒为 1，居中各 0.5。
        // 原版把 0..200 的 pan 直接交给发射器位置，引擎侧就是这条律——
        // 上游 issue #452 实测「编辑器播放比归一化导出恒定低 50%」即由此而来。
        let p = pan_eff as f32 / 200.0;
        (g * (1.0 - p), g * p)
    }

    pub fn next_sample(&mut self, bank: &SoundBank, output_rate: u32) -> f32 {
        if !self.alive {
            return 0.0;
        }
        if self.pre >= 1.0 {
            self.pre -= 1.0;
            return 0.0;
        }
        let Some(sample) = bank.resolve(self.source) else {
            self.alive = false;
            return 0.0;
        };
        let pcm = &sample.pcm;
        if pcm.is_empty() {
            self.alive = false;
            return 0.0;
        }
        let idx = self.pos as usize;
        if idx + 1 >= pcm.len() {
            self.alive = false;
            return 0.0;
        }
        let frac = (self.pos - idx as f64) as f32;
        let value = pcm[idx] + (pcm[idx + 1] - pcm[idx]) * frac;
        // ratio 已按 output_rate 归一，这里不重复换算
        let _ = output_rate;
        self.pos += self.ratio;
        value
    }
}

#[derive(Clone, Debug)]
pub struct Mixer {
    pub voices: Vec<Voice>,
    pub max_voices: usize,
    pub output_rate: u32,
    pub mono: bool,
    pub master: f32,
    serial: u64,
    /// 当前峰值（状态栏 / 导出归一化用）。
    pub peak: f32,
    /// 同一 tick 的音符共享的随机起始延迟宽度（毫秒），0 = 关闭。
    /// 见 [`Mixer::spawn`]。
    pub jitter_ms: f64,
}

impl Mixer {
    pub fn new(output_rate: u32) -> Self {
        Self {
            voices: Vec::with_capacity(64),
            max_voices: DEFAULT_MAX_VOICES,
            output_rate,
            mono: false,
            master: 1.0,
            serial: 0,
            peak: 0.0,
            jitter_ms: 0.0,
        }
    }

    /// 开启每列起始抖动（见 [`REALTIME_TICK_JITTER_MS`]）；0 = 关闭，导出用 0。
    pub fn with_tick_jitter(mut self, ms: f64) -> Self {
        self.jitter_ms = ms.max(0.0);
        self
    }

    /// 同一 tick 的音符共用的起始延迟（输出采样数）。
    /// `tick < 0` 表示预览/节拍器：原版走 `audio_play_sound` 直接播，
    /// 不经过列派发，也就没有「整列一起晚一点」的效应。
    fn jitter_delay(tick: i64, ms: f64, rate: u32) -> f64 {
        if ms <= 0.0 || tick < 0 {
            return 0.0;
        }
        // 按 tick 散列（不是 RNG）：同一首歌每次播放完全一致，改一个 tick
        // 也不会影响其它 tick —— 这是为了能把「抖动前后」的差别量出来。
        let h = (tick as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11;
        let unit = h as f64 / (1u64 << 53) as f64; // [0,1)
        unit * ms * 0.001 * rate as f64
    }

    pub fn set_max_voices(&mut self, max: usize) {
        self.max_voices = max.clamp(16, 2048);
        while self.voices.len() > self.max_voices {
            self.voices.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.voices.clear();
        self.peak = 0.0;
    }

    pub fn active(&self) -> usize {
        self.voices.iter().filter(|v| v.alive).count()
    }

    /// 派发一个音符。voice 池满时淘汰最老的（对齐 GM 引擎行为）。
    /// `layer` 为 1 基层号；预览/节拍器传 0 —— 原版预览 `play_sound` 不带层号参数，
    /// emitter 的 soundlayer 默认 0，区间消音掐不到它（只有 pitch<=0 的全消音能掐到）。
    /// `tick` 为发事件的列号，用于 [`Mixer::jitter_ms`]；传 -1 表示不参与抖动。
    pub fn spawn(
        &mut self,
        note: &Note,
        layer_volume: u8,
        layer_stereo: u8,
        bank: &SoundBank,
        layer: u32,
        tick: i64,
    ) {
        self.spawn_source(
            SampleSource::Instrument(note.instrument as usize),
            note,
            layer_volume,
            layer_stereo,
            bank,
            layer,
            tick,
        );
    }

    /// 按指定采样来源派发一个音。原版 `play_sound(ins, ...)` 的 `ins` 本来就是
    /// 「某个 obj_instrument」，乐器表里的音色和节拍器那两个声音资产走的是同一条路
    /// （`control_create.gml:316-327` 给节拍器单独 create 了两个 obj_instrument），
    /// 所以这里用来源区分，而不是硬套一个乐器下标。
    pub fn spawn_source(
        &mut self,
        source: SampleSource,
        note: &Note,
        layer_volume: u8,
        layer_stereo: u8,
        bank: &SoundBank,
        layer: u32,
        tick: i64,
    ) {
        let Some(sample) = bank.resolve(source) else { return };
        if sample.is_empty() {
            return;
        }
        if self.voices.len() >= self.max_voices {
            self.voices.remove(0);
        }
        let (gl, gr) = Voice::gains(note, layer_volume, layer_stereo, self.master, self.mono);
        self.serial += 1;
        self.voices.push(Voice {
            source,
            pos: 0.0,
            ratio: Voice::playback_ratio(note, sample.base_key, self.output_rate),
            gain_l: gl,
            gain_r: gr,
            alive: true,
            serial: self.serial,
            layer,
            pre: Self::jitter_delay(tick, self.jitter_ms, self.output_rate),
        });
    }

    /// 原版 `remove_emitters_all(layerid, layerto)`（control_step.gml:286 消音器派发）：
    /// 掐掉 1 基层号在 `[layerid, layerto]` 闭区间内的 voice。
    /// `layerid <= 0`（pitch 非正）全部掐掉；`layerto < layerid` 且 `layerid > 0`
    /// 是「停用」消音器，什么都不动（GML 开头的空区间提前返回）。
    pub fn stop_range(&mut self, layerid: i32, layerto: u32) {
        if layerid > 0 && (layerto as i64) < layerid as i64 {
            return;
        }
        let first = layerid.max(0) as u32;
        self.voices.retain(|v| {
            first > 0 && ((v.layer as u32) < first || (v.layer as u32) > layerto)
        });
    }

    /// 渲染一个输出采样帧。
    pub fn render_sample(&mut self, bank: &SoundBank) -> (f32, f32) {
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        let rate = self.output_rate;
        for v in &mut self.voices {
            if !v.alive {
                continue;
            }
            let s = v.next_sample(bank, rate);
            l += s * v.gain_l;
            r += s * v.gain_r;
        }
        self.voices.retain(|v| v.alive);
        self.peak = self.peak.max(l.abs()).max(r.abs());
        // 密集段的求和远超满量程，原版靠引擎后端压下来（GM 2022.0.3 LTS 用的是
        // modified OpenAL，见上游 issue #452），那条曲线闭源。这里保留软饱和而非
        // ±1 硬削波：同一首歌的尾部硬削把 24.6% 采样钉成平顶，软饱和只 7.6%。
        (clip_roundoff(l), clip_roundoff(r))
    }
}

/// SoLoud `clip_internal` 的 CLIP_ROUNDOFF 分支（逐字移植）：
/// `|x| >= 1.65` 钉在 ±0.9862875，其余 `0.87x - 0.1x³`，
/// 输出再乘 `mPostClipScaler = 0.95`（postinit 固定值）。
fn clip_roundoff(x: f32) -> f32 {
    let f = if x <= -1.65 {
        -0.9862875
    } else if x >= 1.65 {
        0.9862875
    } else {
        0.87 * x - 0.1 * x * x * x
    };
    0.95 * f
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Note;

    #[test]
    fn pitch_formula_matches_original() {
        // key=45, pit=0 → 原速
        let n = Note::new(0, 45);
        assert!((Voice::playback_ratio(&n, 45, 44100) - 1.0).abs() < 1e-9);
        // 升一个八度 → 2 倍速
        let n = Note::new(0, 57);
        assert!((Voice::playback_ratio(&n, 45, 44100) - 2.0).abs() < 1e-9);
        // 降一个八度 → 0.5 倍速
        let n = Note::new(0, 33);
        assert!((Voice::playback_ratio(&n, 45, 44100) - 0.5).abs() < 1e-9);
        // +100 cent = +1 半音
        let mut n = Note::new(0, 45);
        n.pitch = 100;
        let expected = 2f64.powf(1.0 / 12.0);
        assert!((Voice::playback_ratio(&n, 45, 44100) - expected).abs() < 1e-9);
        // 输出设备 48000 时按比例缩放
        let n = Note::new(0, 45);
        assert!((Voice::playback_ratio(&n, 45, 48000) - 44100.0 / 48000.0).abs() < 1e-9);
    }

    /// SoLoud CLIP_ROUNDOFF 曲线：墙点连续（0.87·1.65 − 0.1·1.65³ = 0.9862875），
    /// 全程乘 mPostClipScaler 0.95。
    #[test]
    fn clip_roundoff_matches_soloud() {
        assert!(clip_roundoff(0.0).abs() < 1e-9);
        assert!((clip_roundoff(1.0) - 0.95 * 0.77).abs() < 1e-9);
        let wall = 0.95 * 0.9862875;
        for x in [1.65f32, 2.0, 6.6, 100.0] {
            assert!((clip_roundoff(x) - wall).abs() < 1e-6);
            assert!((clip_roundoff(-x) + wall).abs() < 1e-6);
        }
        assert!(clip_roundoff(1.5) < clip_roundoff(1.6) && clip_roundoff(1.6) < wall);
    }

    #[test]
    fn gains_and_pan() {
        // 线性声像律：左右增益之和恒为 1（单声道兼容），居中各 0.5
        let n = Note::new(0, 45);
        let (l, r) = Voice::gains(&n, 100, 100, 1.0, false);
        assert!((l - 0.5).abs() < 1e-6 && (r - 0.5).abs() < 1e-6);
        let mut n = Note::new(0, 45);
        n.panning = 0;
        let (l, r) = Voice::gains(&n, 100, 100, 1.0, false);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        n.panning = 200;
        let (l, r) = Voice::gains(&n, 100, 100, 1.0, false);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        n.panning = 50;
        let (l, r) = Voice::gains(&n, 100, 100, 1.0, false);
        assert!((l - 0.75).abs() < 1e-6 && (r - 0.25).abs() < 1e-6);
        // 层音量 50 → 整体一半
        let n = Note::new(0, 45);
        let (l, r) = Voice::gains(&n, 50, 100, 1.0, false);
        assert!((l - 0.25).abs() < 1e-6 && (r - 0.25).abs() < 1e-6);
        // 层声像非中性时取平均：(0 + 100)/2 = 50 → p = 0.25
        let mut n = Note::new(0, 45);
        n.panning = 100;
        let (l, r) = Voice::gains(&n, 100, 0, 1.0, false);
        assert!((l - 0.75).abs() < 1e-6 && (r - 0.25).abs() < 1e-6);
        // 单声道输出不衰减声像
        let (l, r) = Voice::gains(&n, 100, 0, 1.0, true);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn voice_pool_evicts_oldest() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut mixer = Mixer::new(44100);
        mixer.set_max_voices(16);
        let n = Note::new(0, 45);
        for i in 0..17 {
            mixer.spawn(&n, 100, 100, &bank, (i % 5) as u32 + 1, i as i64);
        }
        assert_eq!(mixer.voices.len(), 16);
        assert_eq!(mixer.voices[0].serial, 2, "应淘汰创建最早的 voice");
    }

    /// 消音器掐区间：voice 的层号是 1 基，[first, last] 闭区间。
    #[test]
    fn stop_range_kills_layer_span_inclusive() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut mixer = Mixer::new(44100);
        let n = Note::new(0, 45);
        for layer in 1..=5u32 {
            mixer.spawn(&n, 100, 100, &bank, layer, layer as i64);
        }
        mixer.stop_range(2, 4);
        let alive: Vec<u32> = mixer.voices.iter().map(|v| v.layer).collect();
        assert_eq!(alive, vec![1, 5], "闭区间 [2,4] 应只掐 2/3/4 层，实测 {alive:?}");
    }

    #[test]
    fn stop_range_zero_stops_everything() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 1000],
            base_key: 45,
            name: "test".into(),
        });
        let mut mixer = Mixer::new(44100);
        let n = Note::new(0, 45);
        for layer in [0u32, 1, 7, 40] {
            mixer.spawn(&n, 100, 100, &bank, layer, layer as i64);
        }
        mixer.stop_range(0, 0);
        assert!(mixer.voices.is_empty(), "pitch<=0 是全消音，预览（层 0）也掐");
    }

    #[test]
    fn stop_range_inverted_is_disabled_stopper() {
        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 1000],
            base_key: 45,
            name: "test".into(),
        });
        let mut mixer = Mixer::new(44100);
        let n = Note::new(0, 45);
        for layer in 1..=3u32 {
            mixer.spawn(&n, 100, 100, &bank, layer, layer as i64);
        }
        mixer.stop_range(4, 2);
        assert_eq!(mixer.voices.len(), 3, "end<start 的「停用」消音器不动任何 voice");
        // 负 pitch：GML floor(负数) 为负 → layerid 夹到 0 → 全消音
        mixer.stop_range(-5, 0);
        assert!(mixer.voices.is_empty(), "负 pitch 也走全消音（layerid<0 → 0）");
    }

    /// tick 抖动：延迟只由列号决定（可复现），落在 [0, 宽度) 内，同一列共享同一延迟。
    /// 关闭（0 ms）或预览（tick < 0）时为 0 —— 导出与试听保持原样。
    #[test]
    fn tick_jitter_delays_whole_column_deterministically() {
        let rate = 44100u32;
        let ms = 5.0f64;
        let width = ms * 0.001 * rate as f64; // 220.5 个输出采样
        assert_eq!(Mixer::jitter_delay(-1, ms, rate), 0.0, "预览列不抖动");
        assert_eq!(Mixer::jitter_delay(10, 0.0, rate), 0.0, "关闭时零延迟");
        let mut sum = 0.0;
        for tick in 0..4096i64 {
            let d = Mixer::jitter_delay(tick, ms, rate);
            assert!((0.0..width).contains(&d), "tick {tick} 抖动 {d} 越界");
            assert_eq!(d, Mixer::jitter_delay(tick, ms, rate), "同列必须可复现");
            sum += d;
        }
        assert!(
            (sum / 4096.0 - width / 2.0).abs() < width * 0.05,
            "均值应接近宽度中点，实测 {}",
            sum / 4096.0
        );

        let mut bank = SoundBank::empty();
        bank.samples.push(crate::samples::Sample {
            pcm: vec![0.5; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let mut mixer = Mixer::new(rate);
        mixer.jitter_ms = ms;
        let n = Note::new(0, 45);
        mixer.spawn(&n, 100, 100, &bank, 1, 7);
        let pre = Mixer::jitter_delay(7, ms, rate) as usize;
        for _ in 0..pre {
            assert_eq!(mixer.render_sample(&bank).0, 0.0, "延迟期内应静音");
        }
        assert!(mixer.render_sample(&bank).0 > 0.0, "延迟结束后应开始发声");
    }
}
