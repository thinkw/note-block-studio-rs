//! 参考音频轨（原版 `load_reference_audio` / `reference_audio_sync`）。
//!
//! 原版把参考音频作为独立声音挂在歌曲实例上；端口每个标签保存配置，切标签时把
//! 对应轨道装回共享输出流。只支持原版声明的 Ogg Vorbis 与 8/16-bit PCM WAV。

use std::path::{Path, PathBuf};

use crate::samples::{decode_ogg, resample_linear, SAMPLE_RATE};

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
            "wav" => decode_wav(path)?,
            _ => {
                return Err(nbs_core::tr(
                    "Only Ogg Vorbis or 8/16-bit PCM WAV is supported",
                    "只支持 Ogg Vorbis 或 8/16-bit PCM WAV",
                )
                .into())
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

fn decode_wav(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("WAV 头无效".into());
    }
    let mut format = None;
    let mut channels = 0usize;
    let mut rate = 0u32;
    let mut bits = 0u16;
    let mut data = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let kind = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if body + size > bytes.len() {
            return Err("WAV 数据块被截断".into());
        }
        match kind {
            b"fmt " if size >= 16 => {
                format = Some(u16::from_le_bytes(
                    bytes[body..body + 2].try_into().unwrap(),
                ));
                channels =
                    u16::from_le_bytes(bytes[body + 2..body + 4].try_into().unwrap()) as usize;
                rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
                bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap());
            }
            b"data" => data = Some(&bytes[body..body + size]),
            _ => {}
        }
        at = body + size + size % 2;
    }
    if format != Some(1) || !(1..=8).contains(&channels) || rate == 0 {
        return Err("只支持 PCM WAV".into());
    }
    let data = data.ok_or_else(|| "WAV 缺少数据块".to_string())?;
    let width = match bits {
        8 => 1,
        16 => 2,
        _ => return Err("只支持 8-bit 或 16-bit PCM WAV".into()),
    };
    let frame = width * channels;
    if frame == 0 || data.len() % frame != 0 {
        return Err("WAV 采样帧不完整".into());
    }
    let mut mono = Vec::with_capacity(data.len() / frame);
    for sample in data.chunks_exact(frame) {
        let mut sum = 0.0f32;
        for c in sample.chunks_exact(width) {
            let value = if width == 1 {
                (c[0] as f32 - 128.0) / 128.0
            } else {
                i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0
            };
            sum += value;
        }
        mono.push(sum / channels as f32);
    }
    Ok(resample_linear(&mono, rate, SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(bytes: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((bytes.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn pcm_wav_decodes_and_resamples() {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&48000u32.to_le_bytes());
        fmt.extend_from_slice(&96000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let mut chunks = b"fmt ".to_vec();
        chunks.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        chunks.extend_from_slice(&fmt);
        chunks.extend_from_slice(b"data");
        let samples: Vec<i16> = (0..48000)
            .map(|i| ((i as f32 * 0.01).sin() * 30000.0).round() as i16)
            .collect();
        let mut data = Vec::new();
        for sample in samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        chunks.extend_from_slice(&(data.len() as u32).to_le_bytes());
        chunks.extend_from_slice(&data);
        let path = std::env::temp_dir().join(format!("nbs_ref_{}.wav", std::process::id()));
        std::fs::write(&path, wav(&chunks)).unwrap();
        let decoded = decode_wav(&path).unwrap();
        assert_eq!(decoded.len(), SAMPLE_RATE as usize);
        assert!(decoded.iter().any(|s| s.abs() > 0.5));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ogg_loads_and_a_failed_replacement_clears_the_old_track() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/Sounds/harp.ogg");
        let mut track = ReferenceTrack::default();
        track.load(&path).unwrap();
        assert!(track.is_loaded());
        assert!(track.duration() > 0.01);
        let bad = std::env::temp_dir().join(format!("nbs_ref_bad_{}.mp3", std::process::id()));
        std::fs::write(&bad, b"not audio").unwrap();
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
