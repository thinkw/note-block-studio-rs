//! 参考音频轨（原版 `load_reference_audio` / `reference_audio_sync`）。
//!
//! 原版把参考音频作为独立声音挂在歌曲实例上；端口每个标签保存配置，切标签时把
//! 对应轨道装回共享输出流。格式比原版宽（原版只有 Ogg Vorbis）：`.ogg` 沿用乐器
//! 采样那套 lewton 解码，其余走 symphonia，统一落到 44.1 kHz 单声道 f32。

use std::path::{Path, PathBuf};

use crate::samples::{decode_ogg, resample_linear, SAMPLE_RATE};

/// 参考音频认的扩展名。`decode` 的分派表、文件对话框的过滤器和浏览器里放行的那批
/// 后缀都读这一份，避免「选项列了却解不动」。
pub const REFERENCE_EXTENSIONS: &[&str] = &["ogg", "wav", "mp3", "flac", "aif", "aiff"];

#[derive(Default)]
pub struct ReferenceTrack {
    samples: Vec<f32>,
    path: Option<PathBuf>,
    position: f64,
    last_target: Option<f64>,
    playing: bool,
    mode: u8,
    gain: f32,
}

impl ReferenceTrack {
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        self.clear();
        *self = Self::decode(path)?;
        Ok(())
    }

    pub fn decode(path: &Path) -> Result<Self, String> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let samples = match ext.as_str() {
            "ogg" => decode_ogg(path)?,
            "wav" | "mp3" | "flac" | "aif" | "aiff" => decode_symphonia(path)?,
            _ => {
                return Err(format!(
                    "{}{}",
                    nbs_core::tr("Supported formats: ", "支持的格式："),
                    REFERENCE_EXTENSIONS.join(" / ")
                ))
            }
        };
        if samples.is_empty() {
            return Err(nbs_core::tr(
                "The audio file has no playable samples",
                "音频文件没有可播放的采样",
            )
            .into());
        }
        Ok(Self {
            samples,
            path: Some(path.to_path_buf()),
            ..Self::default()
        })
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.path = None;
        self.position = 0.0;
        self.last_target = None;
        self.playing = false;
        self.mode = 2;
        self.gain = 0.0;
    }

    pub fn is_loaded(&self) -> bool {
        !self.samples.is_empty()
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn is_solo(&self) -> bool {
        self.is_loaded() && self.mode == 1
    }

    pub fn is_muted(&self) -> bool {
        self.mode == 0
    }

    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / SAMPLE_RATE as f64
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.last_target = None;
    }

    /// 原版 `reference_audio_sync`：歌曲停、静音、负偏移或越过尾端都不出声；
    /// 回退超过 50ms、强制同步或偏差超过 100ms 时重新定位。
    pub fn sync(
        &mut self,
        song_playing: bool,
        position_seconds: f64,
        offset_ms: i64,
        volume: u8,
        master: f32,
        mode: u8,
        force: bool,
    ) {
        self.mode = mode;
        self.gain = volume as f32 / 100.0 * master;
        if !self.is_loaded() || !song_playing || mode == 0 {
            self.stop();
            return;
        }
        let target = position_seconds + offset_ms as f64 / 1000.0;
        let moved_back = self.last_target.is_some_and(|last| target < last - 0.05);
        self.last_target = Some(target);
        if target < 0.0 || target >= self.duration() {
            self.playing = false;
            return;
        }
        if force || moved_back || (self.position - target).abs() > 0.1 {
            self.position = target;
        }
        self.playing = true;
    }

    pub fn render_sample(&mut self) -> f32 {
        if !self.playing || self.is_muted() {
            return 0.0;
        }
        let at = self.position * SAMPLE_RATE as f64;
        let index = at.floor() as usize;
        let frac = (at - index as f64) as f32;
        let a = self.samples.get(index).copied().unwrap_or(0.0);
        let b = self.samples.get(index + 1).copied().unwrap_or(0.0);
        self.position += 1.0 / SAMPLE_RATE as f64;
        if self.position >= self.duration() {
            self.playing = false;
        }
        (a + (b - a) * frac) * self.gain
    }
}

/// symphonia 解码：探测容器 → 取默认音频轨 → 逐包解成 f32 → 多声道取平均 →
/// 线性重采样到 [`SAMPLE_RATE`]，交出的形状和 [`decode_ogg`] 一致。
///
/// 走内容探测而不是按扩展名挑解码器，所以 `.wav` 里装的是 IEEE float、24-bit，
/// 或者整个是 DAW 导出的 WAVE_FORMAT_EXTENSIBLE，都照样解得开。
fn decode_symphonia(path: &Path) -> Result<Vec<f32>, String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::{probe::Hint, FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).map_err(|e| format!("读取失败: {e}"))?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("无法识别的音频容器: {e}"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "文件里没有音频轨".to_string())?;
    let track_id = track.id;
    // 克隆出来：下面的 next_packet() 要可变借用 format，不能把借用链留在这儿。
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .cloned()
        .ok_or_else(|| "音频轨缺少编码参数".to_string())?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("没有可用的解码器: {e}"))?;
    let mut rate = params.sample_rate.unwrap_or(SAMPLE_RATE);

    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // symphonia 的 FLAC 解析器收尾时会拿 STREAMINFO 声明的总帧数和实际读到的比，
            // 声明值偏大（剪辑过的曲子常见）就抛 UnexpectedEof 而不是正常结束。此时音频几乎
            // 全在手，判成整轨打不开等于把能用的东西扔掉，所以有样本就收尾、颗粒无收才算失败。
            Err(_) if !mono.is_empty() => break,
            Err(e) => return Err(format!("解码中断: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        // 单包坏掉跳过：整首里一两帧的代价远小于整轨打不开。
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        let mut planes: Vec<Vec<f32>> = Vec::new();
        decoded.copy_to_vecs_planar::<f32>(&mut planes);
        let Some(first) = planes.first() else {
            continue;
        };
        rate = decoded.spec().rate();
        if planes.len() == 1 {
            mono.extend_from_slice(first);
        } else {
            let n = planes.len() as f32;
            for i in 0..first.len() {
                let sum: f32 = planes.iter().map(|p| p[i]).sum();
                mono.push(sum / n);
            }
        }
    }
    Ok(resample_linear(&mono, rate, SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn riff(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = kind.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    /// 手搓 RIFF/WAVE：`format` + `bits` + `extra` 拼 `fmt `，`data` 原样落块。
    fn wav(format: u16, bits: u16, rate: u32, extra: &[u8], data: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&format.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * bits as u32 / 8).to_le_bytes());
        fmt.extend_from_slice(&(bits / 8).to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(extra);
        let mut body = riff(b"fmt ", &fmt);
        body.extend_from_slice(&riff(b"data", data));
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }

    /// 1 秒正弦按位深/格式编码成单声道 data 块。
    fn tone(rate: u32, bits: u16, float: bool) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..rate {
            let value = (i as f32 * 0.01).sin() * 0.9;
            if float {
                data.extend_from_slice(&value.to_le_bytes());
            } else if bits == 16 {
                data.extend_from_slice(&((value * 32767.0) as i16).to_le_bytes());
            } else if bits == 8 {
                data.push(((value + 1.0) * 127.5) as u8);
            } else {
                let scaled = (value * 8_388_607.0) as i32;
                data.extend_from_slice(&scaled.to_le_bytes()[..3]);
            }
        }
        data
    }

    fn temp_path(name: &str, ext: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nbs_ref_{name}_{}.{ext}", std::process::id()))
    }

    fn write_temp(name: &str, ext: &str, bytes: &[u8]) -> PathBuf {
        let path = temp_path(name, ext);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn decode_temp(name: &str, bytes: &[u8]) -> Vec<f32> {
        let path = write_temp(name, "wav", bytes);
        let out = decode_symphonia(&path);
        let _ = std::fs::remove_file(&path);
        out.unwrap()
    }

    /// AIFF：和 WAV 同族但全 big-endian，采样率还是 80-bit IEEE extended。
    /// 手搓一份，免得往仓库里塞几十 KB 无压缩音频。
    fn aiff(rate: u32) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..rate {
            let value = (i as f32 * 0.01).sin() * 0.9;
            data.extend_from_slice(&((value * 32767.0) as i16).to_be_bytes());
        }
        let width = 32 - rate.leading_zeros();
        // 80-bit IEEE extended：2 字节指数 + 8 字节显式整数位尾数，一共 10 字节。
        let mut extended = [0u8; 10];
        extended[..2].copy_from_slice(&((16383 + width - 1) as u16).to_be_bytes());
        extended[2..].copy_from_slice(&((rate as u64) << (64 - width)).to_be_bytes());
        // COMM 的字段顺序是声道数、总帧数、位深，最后才是 80-bit 采样率。
        let comm = [
            &1u16.to_be_bytes()[..],
            &rate.to_be_bytes()[..],
            &16u16.to_be_bytes()[..],
            &extended,
        ]
        .concat();
        let ssnd = [&0u32.to_be_bytes()[..], &0u32.to_be_bytes()[..], &data[..]].concat();
        let chunk = |kind: &[u8; 4], body: &[u8]| {
            let mut out = kind.to_vec();
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(body);
            out
        };
        let mut body = b"AIFF".to_vec();
        body.extend_from_slice(&chunk(b"COMM", &comm));
        body.extend_from_slice(&chunk(b"SSND", &ssnd));
        let mut out = b"FORM".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn fixture(ext: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        if ext == "ogg" {
            root.join("../../assets/Sounds/harp.ogg")
        } else {
            root.join(format!("tests/data/reftone.{ext}"))
        }
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// 原来手搓的解析器只认 tag 1 + 8/16 bit，DAW 导出的 float/extensible 一律报「只支持
    /// PCM WAV」。换 symphonia 后这几种都要真解开，且幅值不能标错。
    #[test]
    fn wav_bit_depths_and_extensible_all_decode() {
        for (name, bytes) in [
            ("pcm16", wav(1, 16, 48000, &[], &tone(48000, 16, false))),
            ("pcm24", wav(1, 24, 48000, &[], &tone(48000, 24, false))),
            ("f32", wav(3, 32, 48000, &[], &tone(48000, 32, true))),
            (
                "ext16",
                wav(
                    0xFFFE,
                    16,
                    48000,
                    &[
                        22, 0, // cbSize
                        16, 0, // wValidBitsPerSample
                        1, 0, 0, 0, // dwChannelMask = FRONT_CENTER
                        1, 0, 0, 0, 0, 0, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B,
                        0x71, // KSDATAFORMAT_SUBTYPE_PCM
                    ],
                    &tone(48000, 16, false),
                ),
            ),
        ] {
            let decoded = decode_temp(name, &bytes);
            assert_eq!(decoded.len(), SAMPLE_RATE as usize, "{name} 时长/重采样");
            let peak = peak(&decoded);
            assert!(
                (0.85..=0.95).contains(&peak),
                "{name} 峰值应为 ~0.9，实际 {peak}"
            );
        }
    }

    /// 清单（对话框过滤器、浏览器放行、`decode` 分派）和真能解开的东西必须一致：
    /// 每一项都走 `ReferenceTrack::decode` 这条真实入口，扩展名不对就先报错。
    #[test]
    fn every_advertised_extension_really_decodes() {
        let aiff_bytes = aiff(22050);
        let synthesized = [
            ("aif", aiff_bytes.clone()),
            ("aiff", aiff_bytes),
            ("wav", wav(1, 16, 44100, &[], &tone(44100, 16, false))),
        ];
        let generated: Vec<(&str, PathBuf)> = synthesized
            .iter()
            .map(|(ext, bytes)| (*ext, write_temp("list", ext, bytes)))
            .collect();
        let stored = [
            ("ogg", fixture("ogg")),
            ("mp3", fixture("mp3")),
            ("flac", fixture("flac")),
        ];
        for ext in REFERENCE_EXTENSIONS {
            let path = generated
                .iter()
                .chain(stored.iter())
                .find(|(e, _)| e == ext)
                .map(|(_, path)| path.clone())
                .unwrap_or_else(|| panic!("{ext} 在清单里却没有用例"));
            let track = ReferenceTrack::decode(&path)
                .unwrap_or_else(|e| panic!("{ext} 列进了清单却解不开（{}）：{e}", path.display()));
            assert!(
                track.duration() > 0.1,
                "{ext} 解出来只有 {}s",
                track.duration()
            );
        }
        for (_, path) in generated {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 剪辑过的 FLAC 常见 STREAMINFO 谎报总帧数，symphonia 会在读完的那一刻判成硬错误，
    /// 整轨作废。把用例的声明值抬高复现，验证已解出来的样本能留下。
    #[test]
    fn flac_with_a_lying_frame_count_keeps_the_audio() {
        let mut liar = std::fs::read(fixture("flac")).unwrap();
        // STREAMINFO 紧跟 fLaC + 4 字节块头，这 8 字节的低 36 位是总帧数。
        let mut field = u64::from_be_bytes(liar[18..26].try_into().unwrap());
        field = (field & !((1u64 << 36) - 1)) | (1u64 << 32);
        liar[18..26].copy_from_slice(&field.to_be_bytes());
        let honest = decode_symphonia(&fixture("flac")).unwrap();
        let path = write_temp("liar", "flac", &liar);
        let fixed = decode_symphonia(&path);
        let _ = std::fs::remove_file(path);
        let fixed = fixed.expect("STREAMINFO 谎报总长不能判成整轨解不开");
        assert!(!honest.is_empty(), "用例本身得先有声音");
        assert_eq!(
            fixed.len(),
            honest.len(),
            "谎报总长的代价只能是报错那一段，已解出来的样本不许丢"
        );
    }

    #[test]
    fn ogg_loads_and_a_failed_replacement_clears_the_old_track() {
        let path = fixture("ogg");
        let mut track = ReferenceTrack::default();
        track.load(&path).unwrap();
        assert!(track.is_loaded());
        assert!(track.duration() > 0.01);
        let bad = write_temp("bad", "mp3", b"not audio");
        assert!(track.load(&bad).is_err());
        assert!(!track.is_loaded(), "换轨失败不能继续放上一首");
        let _ = std::fs::remove_file(bad);
    }

    #[test]
    fn normal_mute_and_solo_mix_the_reference_track_independently() {
        use crate::samples::{Sample, SampleSource, SoundBank};
        use crate::scheduler::SharedState;
        use nbs_core::model::Note;
        use std::sync::Arc;

        let mut bank = SoundBank::empty();
        bank.samples.push(Sample {
            pcm: vec![0.4; 44100],
            base_key: 45,
            name: "test".into(),
        });
        let bank = Arc::new(bank);
        let mut state = SharedState::new(bank.clone(), SAMPLE_RATE);
        state.reference.samples = vec![0.2; 44100];
        let mut note = Note::new(0, 45);
        note.panning = 0;
        state
            .mixer
            .spawn_source(SampleSource::Instrument(0), &note, 100, 100, &bank, 0, -1);
        let mut warmup = [0.0; 512];
        state.render_interleaved(&mut warmup, 2);
        let sample = |state: &mut SharedState, mode: u8| {
            state.reference.sync(true, 0.0, 0, 100, 1.0, mode, true);
            let mut out = [0.0; 2];
            state.render_interleaved(&mut out, 2);
            out[0]
        };
        let normal = sample(&mut state, 2);
        let muted = sample(&mut state, 0);
        let solo = sample(&mut state, 1);
        assert!(
            normal > muted && muted > solo && solo > 0.0,
            "normal={normal} muted={muted} solo={solo}"
        );
        assert!((normal - muted - solo).abs() < 1e-6);
    }

    #[test]
    fn sync_handles_negative_offset_end_and_resync() {
        let mut track = ReferenceTrack {
            samples: vec![0.25; SAMPLE_RATE as usize],
            ..Default::default()
        };
        track.sync(true, 0.0, -100, 100, 1.0, 2, false);
        assert!(!track.is_playing(), "负偏移是起始前的静音");
        track.sync(true, 2.0, 0, 100, 1.0, 2, false);
        assert!(!track.is_playing(), "越过尾端停止");
        track.sync(true, 0.5, 0, 50, 2.0, 2, false);
        assert!(track.is_playing());
        assert_eq!(track.render_sample(), 0.25);
        track.sync(true, 0.1, 0, 50, 2.0, 2, false);
        assert!((track.position - 0.1).abs() < 1e-9, "回退要重定位");
        track.sync(true, 0.1, 0, 100, 1.0, 0, false);
        assert!(!track.is_playing());
        assert!(track.is_muted());
    }
}
