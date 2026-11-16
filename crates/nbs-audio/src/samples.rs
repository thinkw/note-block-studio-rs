//! 采样库：加载 OpenNBS 自带的 20 个 OGG Vorbis 内置采样。
//!
//! 关键坑（来自原版 `instrument_load.gml`）：`assets/Sounds` 里 6 个采样是
//! 44100Hz、14 个是 48000Hz（含 4 个铜块一族），而 GML 侧硬编码 44100。
//! 必须先按文件头实际采样率解码，再线性重采样到统一 44100，
//! 否则会整体降调约 11.9%。

use std::io::Cursor;
use std::path::{Path, PathBuf};

use nbs_core::model::Song;

/// 原版硬编码的输出采样率。
pub const SAMPLE_RATE: u32 = 44100;
/// 内置采样基准音高（`new_instrument` 默认 45，对应 pitch=1.0）。
pub const BASE_KEY: u8 = 45;

/// 内置乐器 → OGG 文件名（原版 `control_create.gml` 的 `instrument_list`；
/// 16..19 是装机版新增的铜块一族，采样来自 `Data/Sounds/trumpet*.ogg`）。
pub const BUILTIN_FILES: [&str; 20] = [
    "harp.ogg",
    "dbass.ogg",
    "bdrum.ogg",
    "sdrum.ogg",
    "click.ogg",
    "guitar.ogg",
    "flute.ogg",
    "bell.ogg",
    "icechime.ogg",
    "xylobone.ogg",
    "iron_xylophone.ogg",
    "cow_bell.ogg",
    "didgeridoo.ogg",
    "bit.ogg",
    "banjo.ogg",
    "pling.ogg",
    "trumpet.ogg",
    "trumpet_exposed.ogg",
    "trumpet_weathered.ogg",
    "trumpet_oxidized.ogg",
];

#[derive(Clone, Debug)]
pub struct Sample {
    /// 单声道 PCM，统一为 `SAMPLE_RATE`，范围 -1.0..=1.0。
    pub pcm: Vec<f32>,
    /// 该采样的基准 key（内置固定 45，自定义由 NBS 文件给出）。
    pub base_key: u8,
    pub name: String,
}

impl Sample {
    pub fn len(&self) -> usize {
        self.pcm.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pcm.is_empty()
    }
    /// 采样时长（秒）。
    pub fn duration(&self) -> f64 {
        self.pcm.len() as f64 / SAMPLE_RATE as f64
    }
}

#[derive(Clone, Debug, Default)]
pub struct SoundBank {
    /// 索引 0..19 内置，>= 20 自定义乐器。
    pub samples: Vec<Sample>,
    pub missing: Vec<String>,
    pub source: Option<PathBuf>,
    /// 节拍器采样（可选）。
    pub metronome: Option<Sample>,
}

impl SoundBank {
    pub fn empty() -> Self {
        Self::default()
    }

    /// 自动搜索采样目录并加载。
    pub fn discover() -> Self {
        Self::load_auto(None)
    }

    pub fn load_auto(extra: Option<&Path>) -> Self {
        let dir = find_sounds_dir(extra);
        match dir {
            Some(d) => Self::load_from(&d),
            None => {
                let mut b = Self::empty();
                b.missing = BUILTIN_FILES.iter().map(|s| s.to_string()).collect();
                b
            }
        }
    }

    pub fn load_from(dir: &Path) -> Self {
        let mut bank = Self {
            source: Some(dir.to_path_buf()),
            ..Default::default()
        };
        for (i, file) in BUILTIN_FILES.iter().enumerate() {
            let path = dir.join(file);
            match decode_ogg(&path) {
                Ok(pcm) => bank.samples.push(Sample {
                    pcm,
                    base_key: BASE_KEY,
                    name: nbs_core::instruments::instrument_name(i as u8),
                }),
                Err(e) => {
                    bank.samples.push(Sample {
                        pcm: vec![0.0; 128],
                        base_key: BASE_KEY,
                        name: format!("缺失 {}", file),
                    });
                    bank.missing.push(format!("{file}: {e}"));
                }
            }
        }
        if let Ok(pcm) = decode_ogg_any(&[
            dir.join("metronome.ogg"),
            dir.join("UI/metronome.ogg"),
            dir.join("click.ogg"),
        ]) {
            bank.metronome = Some(Sample { pcm, base_key: BASE_KEY, name: "节拍器".into() });
        }
        bank
    }

    /// 为歌曲引用的自定义乐器追加采样（NBS 文件第 5 段）。
    /// 采样基准 key 来自 `CustomInstrument.key`，音色对齐原版。
    pub fn extend_for_song(&mut self, song: &Song) {
        self.samples.truncate(BUILTIN_FILES.len());
        let dir = self.source.clone();
        for ins in &song.custom_instruments {
            let path = dir
                .as_ref()
                .map(|d| d.join(ins.filename.replace('\\', "/")))
                .unwrap_or_else(|| PathBuf::from(&ins.filename));
            let pcm = decode_ogg(&path).unwrap_or_default();
            self.samples.push(Sample {
                pcm,
                base_key: ins.key,
                name: ins.name.clone(),
            });
        }
    }

    pub fn get(&self, index: usize) -> Option<&Sample> {
        self.samples.get(index)
    }

    pub fn loaded_count(&self) -> usize {
        self.samples.iter().filter(|s| !s.is_empty()).count()
    }

    pub fn is_ready(&self) -> bool {
        self.missing.len() < BUILTIN_FILES.len()
    }
}

/// 搜索采样目录。优先级：显式配置 > 环境变量 > 程序自带 assets > 原版开发目录。
pub fn find_sounds_dir(extra: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = extra {
        candidates.push(p.to_path_buf());
    }
    if let Ok(p) = std::env::var("NBS_SOUNDS_DIR") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets/Sounds"));
            candidates.push(dir.join("Sounds"));
            // 开发期：target/debug/nbs-rust.exe → 项目根
            candidates.push(dir.join("../../assets/Sounds"));
            candidates.push(dir.join("../../../assets/Sounds"));
            candidates.push(dir.join("Data/Sounds"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("assets/Sounds"));
        candidates.push(cwd.join("Data/Sounds"));
        candidates.push(cwd.join("../assets/Sounds"));
        // 同一工作区里的原版项目，方便开发时零配置
        candidates.push(cwd.join("../NoteBlockStudio/datafiles/Data/Sounds"));
        candidates.push(cwd.join("../../NoteBlockStudio/datafiles/Data/Sounds"));
    }
    for c in candidates {
        if c.join("harp.ogg").is_file() {
            return Some(c.canonicalize().unwrap_or(c));
        }
    }
    None
}

/// 解码一个 OGG Vorbis 文件，输出统一 44100Hz 单声道 f32。
pub fn decode_ogg(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    decode_ogg_bytes(&bytes)
}

fn decode_ogg_any(paths: &[PathBuf]) -> Result<Vec<f32>, String> {
    let mut last = String::from("无候选文件");
    for p in paths {
        match decode_ogg(p) {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub fn decode_ogg_bytes(bytes: &[u8]) -> Result<Vec<f32>, String> {
    use lewton::inside_ogg::OggStreamReader;
    let cursor = Cursor::new(bytes.to_vec());
    let mut reader = OggStreamReader::new(cursor).map_err(|e| format!("Vorbis 头无效: {e}"))?;
    let rate = reader.ident_hdr.audio_sample_rate;
    let channels = reader.ident_hdr.audio_channels.max(1) as usize;
    if rate == 0 {
        return Err("采样率为 0".into());
    }
    let mut mono_i16: Vec<i16> = Vec::new();
    while let Some(packet) = reader
        .read_dec_packet_itl()
        .map_err(|e| format!("Vorbis 解码失败: {e}"))?
    {
        if channels == 1 {
            mono_i16.extend_from_slice(&packet);
        } else {
            // 交错多声道 → 取平均
            for frame in packet.chunks_exact(channels) {
                let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                mono_i16.push((sum / channels as i32) as i16);
            }
        }
    }
    let mono_f32: Vec<f32> = mono_i16.iter().map(|&s| s as f32 / 32768.0).collect();
    if rate == SAMPLE_RATE {
        Ok(mono_f32)
    } else {
        Ok(resample_linear(&mono_f32, rate, SAMPLE_RATE))
    }
}

/// 线性重采样（与原版 `audio_emitter_pitch` 的简单重采样语义一致）。
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let idx = src as usize;
        let frac = (src - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(0.0);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sounds_are_loadable() {
        let bank = SoundBank::discover();
        assert!(bank.is_ready(), "应能找到内置采样目录: {:?}", bank.source);
        assert_eq!(bank.samples.len(), BUILTIN_FILES.len());
        // 原版混合采样率，全部必须被统一到 44100
        assert!(bank.samples[0].len() > 1000, "harp 采样不能为空");
        assert_eq!(bank.missing.len(), 0, "缺失: {:?}", bank.missing);
    }

    #[test]
    fn all_samples_have_reasonable_length() {
        let bank = SoundBank::discover();
        for (i, s) in bank.samples.iter().enumerate() {
            assert!(s.duration() > 0.01 && s.duration() < 10.0, "乐器 {i} 时长异常: {}", s.duration());
            assert_eq!(s.base_key, BASE_KEY);
        }
    }

    #[test]
    fn resample_48000_to_44100_shortens() {
        let src: Vec<f32> = (0..48000).map(|i| (i as f32 * 0.01).sin()).collect();
        let dst = resample_linear(&src, 48000, 44100);
        assert_eq!(dst.len(), 44100);
    }
}
