//! Voice 池与混音器。实时播放与离线导出共用同一套 voice 取值 / 增益 / 音高代码；
//! 总线过载处理不共用——导出照 nbswave 归一化，实时走电平下探（见 `Mixer::duck_frame`）。

use std::collections::VecDeque;

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

    /// 真正听到的声像 0..=200（原版 `play_sound.gml:24`）：轨的声道不在正中时，
    /// 音符自己的 pan 会被往中间拉一半。
    ///
    /// 单独抽出来是因为播放特效（声道方块）要按同一个数摆位置：画面里方块落在哪一槽，
    /// 必须和耳朵听到的左右一致，而不是照 `note.panning` 的裸值。
    pub fn effective_pan(note_panning: u8, layer_stereo: u8) -> u8 {
        if layer_stereo == PAN_CENTER {
            note_panning
        } else {
            ((layer_stereo as u16 + note_panning as u16) / 2) as u8
        }
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
        let pan_eff = Self::effective_pan(note.panning, layer_stereo);
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

/// 总线「电平下探」限幅的三个参数。
///
/// 原版这条链是闭源的（GM 后端 = modified OpenAL，上游 issue #452 只说了
/// 「响的部分整体压低」），任何数值都无法从 GML 推出，因此这三个数是**有意偏离**：
/// 只按包络整体压低电平、不逐点削波形，为的是消掉谐波失真（听感上的「炸」）。
/// 前视 3 ms + 下探瞬时、释放 60 ms；等价性与「不削顶」由
/// `duck_window_matches_brute_force_lookahead` / `duck_dips_the_level_instead_of_shaving_waveforms`
/// 两个用例守住。
const DUCK_LOOKAHEAD_MS: f32 = 3.0;
const DUCK_RELEASE_MS: f32 = 60.0;
/// 下探的目标电平：留 0.001 的余量，保证输出永远够不到满量程（后端 i16 转换不再截顶）。
const DUCK_CEILING: f32 = 0.999;

/// 定长窗口内的峰值最大值（单调队列，摊还 O(1)）。
/// 队列里只保留「还可能成为窗口最大值」的帧，前端即当前窗口最大值。
#[derive(Clone, Debug, Default)]
struct PeakWindow {
    len: usize,
    /// (帧号, 该帧峰值)，按帧号递增、峰值递减排列。
    items: VecDeque<(u64, f32)>,
}

impl PeakWindow {
    fn new(len: usize) -> Self {
        Self {
            len: len.max(1),
            items: VecDeque::with_capacity(len.max(1)),
        }
    }

    fn push(&mut self, at: u64, peak: f32) {
        while self.items.back().is_some_and(|&(_, p)| p <= peak) {
            self.items.pop_back();
        }
        self.items.push_back((at, peak));
        let oldest = at.saturating_sub(self.len as u64 - 1);
        while self.items.front().is_some_and(|&(i, _)| i < oldest) {
            self.items.pop_front();
        }
    }

    fn max(&self) -> f32 {
        self.items.front().map_or(0.0, |&(_, p)| p)
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
    /// 前视窗口（即将输出那一帧的未来 DUCK_LOOKAHEAD_MS 帧）。
    duck_win: PeakWindow,
    /// 延迟线：为了「看到未来」，每一帧要在这里等 DUCK_LOOKAHEAD_MS 才输出。
    duck_delay: VecDeque<(f32, f32)>,
    /// 当前总线增益（下探后的电平）。
    duck_gain: f32,
    /// 每帧向目标增益回升的比例（释放时间常数换算而来）。
    duck_release: f32,
    /// 已进入总线的帧数，用于窗口的淘汰判断。
    duck_step: u64,
}

impl Mixer {
    pub fn new(output_rate: u32) -> Self {
        let lookahead = Self::duck_frames(output_rate);
        let duck_release =
            1.0 - (-1.0 / (output_rate.max(1) as f64 * DUCK_RELEASE_MS as f64 / 1000.0)).exp();
        Self {
            voices: Vec::with_capacity(64),
            max_voices: DEFAULT_MAX_VOICES,
            output_rate,
            mono: false,
            master: 1.0,
            serial: 0,
            peak: 0.0,
            jitter_ms: 0.0,
            duck_win: PeakWindow::new(lookahead),
            duck_delay: VecDeque::with_capacity(lookahead),
            duck_gain: 1.0,
            duck_release: duck_release as f32,
            duck_step: 0,
        }
    }

    /// 前视窗口长度（帧）：输出比调度晚这么多，见 [`DUCK_LOOKAHEAD_MS`]。
    fn duck_frames(rate: u32) -> usize {
        ((rate.max(1) as f64 * DUCK_LOOKAHEAD_MS as f64 / 1000.0).ceil() as usize).max(1)
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
        // 总线状态一起归零：否则暂停/seek 后延迟线里最多残留 DUCK_LOOKAHEAD_MS
        // 的旧音频，会在新的落点后面冒出来。
        self.duck_win = PeakWindow::new(Self::duck_frames(self.output_rate));
        self.duck_delay.clear();
        self.duck_gain = 1.0;
        self.duck_step = 0;
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
        // 密集列的求和远超满量程。原版靠引擎后端把响的部分整体压低（GM 2022.0.3 LTS
        // 用的是 modified OpenAL，见上游 issue #452，曲线闭源）。这里用「电平下探」
        // 复刻这个整体压低，而不是逐点削波形：逐点曲线（先前是 SoLoud CLIP_ROUNDOFF）
        // 本身就是谐波失真的来源，实测同曲尾部把它听成「炸」。
        self.duck_frame(l, r)
    }

    /// 一帧进总线：先记进前视窗口，攒满窗口后按包络增益输出最早那一帧。
    /// 增益只随包络变化，波形逐点原样通过，因此不会产生谐波失真。
    fn duck_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        let at = self.duck_step;
        self.duck_step += 1;
        self.duck_win.push(at, l.abs().max(r.abs()));
        // 窗口 = 最近 duck_frames() 帧，正好是即将输出那一帧的前视区间。
        let top = self.duck_win.max();
        let want = if top > DUCK_CEILING {
            DUCK_CEILING / top
        } else {
            1.0
        };
        // 下探瞬时（前视已保证不越界，不需要慢 attack），回升按释放时间常数。
        self.duck_gain = if want < self.duck_gain {
            want
        } else {
            self.duck_gain + (want - self.duck_gain) * self.duck_release
        };
        self.duck_delay.push_back((l, r));
        if self.duck_delay.len() < self.duck_win.len {
            return (0.0, 0.0);
        }
        let Some((dl, dr)) = self.duck_delay.pop_front() else {
            return (0.0, 0.0);
        };
        (dl * self.duck_gain, dr * self.duck_gain)
    }
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

    /// 前视下探的等价性：单调队列的窗口最大值必须与「逐帧重扫未来 L 帧」
    /// 的暴力算法得到同一串输出（实时侧只是整体晚 L-1 帧）。
    #[test]
    fn duck_window_matches_brute_force_lookahead() {
        let rate = 44100u32;
        let win = Mixer::duck_frames(rate);
        // 确定性伪随机：有安静段、有 4 倍满量程的尖峰。
        let mut seq: Vec<f32> = Vec::new();
        let mut x = 1234.5f32;
        for i in 0..4000usize {
            x = (x * 0.999 + (i % 37) as f32) % 7919.0;
            let spike = if i % 511 == 0 { 4.0 } else { 1.0 };
            seq.push((x / 7919.0 * 2.4 - 1.2) * spike);
        }
        // 暴力参考：want[i] = min(1, 天花板/|x[i]|)，peek[i] = want[i..i+win] 的最小值。
        let want: Vec<f32> = seq
            .iter()
            .map(|s| {
                let m = s.abs();
                if m > DUCK_CEILING {
                    DUCK_CEILING / m
                } else {
                    1.0
                }
            })
            .collect();
        let n = seq.len();
        let k = (1.0 - (-1.0 / (rate as f64 * DUCK_RELEASE_MS as f64 / 1000.0)).exp()) as f32;
        let mut expect: Vec<f32> = Vec::with_capacity(n);
        let mut g = 1.0f32;
        for i in 0..n {
            let peek = want[i..(i + win).min(n)]
                .iter()
                .copied()
                .fold(1.0f32, f32::min);
            g = if peek < g { peek } else { g + (peek - g) * k };
            expect.push(seq[i] * g);
        }
        let mut mixer = Mixer::new(rate);
        for i in 0..n {
            let got = mixer.duck_frame(seq[i], 0.0).0;
            if i + 1 < win {
                assert_eq!(got, 0.0, "前视填充期（帧 {i}）应静音");
                continue;
            }
            let out_i = i + 1 - win;
            if out_i + win > n {
                break; // 参考串末尾的前视窗口被截短，实时侧这时还看不到未来
            }
            assert_eq!(got, expect[out_i], "帧 {out_i} 输出不符");
        }
    }

    /// 下探不会把任何采样推到满量程，也不会削平波形：尖峰整段被压低，
    /// 但采样之间的差别原样保留。
    #[test]
    fn duck_dips_the_level_instead_of_shaving_waveforms() {
        let rate = 44100u32;
        let win = Mixer::duck_frames(rate);
        let mut mixer = Mixer::new(rate);
        let mut out: Vec<f32> = Vec::new();
        // 3 倍满量程的锯齿：逐点限幅会把它削成一堆相同的最大值。
        for i in 0..3000usize {
            let s = 3.0 * ((i % 97) as f32 / 96.0 * 2.0 - 1.0);
            out.push(mixer.duck_frame(s, s).0);
        }
        let over = out.iter().filter(|s| s.abs() > DUCK_CEILING + 1e-6).count();
        assert_eq!(over, 0, "前视下探不该有任何采样越过天花板");
        // 锯齿每 97 帧才回到同一个值，相邻输出不该相等；被逐点削顶则会成片同值。
        // 跳过前视填充期（那段本来就是静音，会被当成连续同值）。
        let steady = &out[win..];
        let mut run = 0usize;
        let mut longest = 0usize;
        for w in steady.windows(2) {
            if w[0].to_bits() == w[1].to_bits() {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        assert!(
            longest < 8,
            "出现连续 {longest} 个相同采样＝波形被削平了，应只按电平整体压低"
        );
    }

    /// 响的一段整体压低、安静段要按释放时间常数回到原电平，不能一路压着不放。
    #[test]
    fn duck_releases_back_to_unity() {
        let rate = 44100u32;
        let win = Mixer::duck_frames(rate);
        let mut mixer = Mixer::new(rate);
        // 4 倍满量程的段落喂满窗口和延迟线后，出来的仍是那一段，被压到天花板。
        for _ in 0..win * 2 {
            mixer.duck_frame(4.0, 4.0);
        }
        let dipped = mixer.duck_frame(4.0, 4.0).0;
        assert!(
            (dipped - DUCK_CEILING).abs() < 1e-4,
            "4 倍满量程应被整体压到 {DUCK_CEILING}，实测 {dipped}"
        );
        // 换成 0.2 的小电平，等延迟线里的 4.0 全部出来、包络再走 8 个释放周期。
        let settle = win + (rate as usize * DUCK_RELEASE_MS as usize / 1000) * 8;
        for _ in 0..settle {
            mixer.duck_frame(0.2, 0.2);
        }
        let back = mixer.duck_frame(0.2, 0.2).0;
        assert!(
            (back - 0.2).abs() < 1e-3,
            "安静 8 个释放周期后应基本回到原电平，实测 {back}"
        );
    }

    /// 暂停 / seek 调 `clear()`：延迟线里剩下的旧音频不许冒到新落点后面。
    #[test]
    fn duck_clear_flushes_the_delay_line() {
        let rate = 44100u32;
        let win = Mixer::duck_frames(rate);
        let mut mixer = Mixer::new(rate);
        for _ in 0..win * 2 {
            mixer.duck_frame(0.3, 0.3);
        }
        mixer.clear();
        for i in 0..win {
            assert_eq!(
                mixer.duck_frame(0.0, 0.0).0,
                0.0,
                "clear 后第 {i} 帧还在前视填充期，应为静音"
            );
        }
        assert_eq!(
            mixer.duck_step, win as u64,
            "clear 要把帧计数归零，否则窗口淘汰会算错"
        );
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
        // 总线上还套了一层前视下探：输出比调度晚 duck_frames()-1 帧。
        let lookahead = Mixer::duck_frames(rate) - 1;
        for _ in 0..pre + lookahead {
            assert_eq!(mixer.render_sample(&bank).0, 0.0, "延迟期内应静音");
        }
        assert!(
            mixer.render_sample(&bank).0 > 0.0,
            "延迟（列抖动 + 前视）结束后应开始发声"
        );
    }
}
