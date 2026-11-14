//! 音频导出的容器/编码层。
//!
//! 原版把这一步交给内嵌 CPython 的 `nbswave 0.4.0`，后者再用随程序分发的
//! `datafiles/ffmpeg.exe` 做转码（`audio_export.py` 只负责 patch 掉控制台窗口）。
//! 这里保留同样的分工：先离线渲染成 WAV，再调用 ffmpeg 转成目标格式，
//! 因此支持的格式集合与原版一致 —— MP3 / WAV / AIFF / FLAC（OGG 在原版 UI 里是注释掉的）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 原版 `draw_window_audio_export` 提供的采样率列表，顺序与菜单一致。
pub const SAMPLE_RATES: [u32; 12] = [
    8000, 11025, 16000, 22050, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000,
];

/// 原版 `audio_exp_target_bitrate` 固定 320，且从不被 UI 修改。
pub const MP3_BITRATE_KBPS: u32 = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Aiff,
    Flac,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 4] = [AudioFormat::Mp3, AudioFormat::Wav, AudioFormat::Aiff, AudioFormat::Flac];

    pub fn name(&self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Mp3 => "MP3",
            Self::Aiff => "AIFF",
            Self::Flac => "FLAC",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Aiff => "aiff",
            Self::Flac => "flac",
        }
    }

    /// 文件名或格式菜单文本 → 格式；无法识别时返回 None（调用方必须报错，不能默认成 WAV）。
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_ascii_lowercase();
        match t.as_str() {
            "wav" | "wave" | "aiff" | "aif" | "flac" | "mp3" => {}
            _ => return None,
        }
        Some(match t.as_str() {
            "mp3" => Self::Mp3,
            "aiff" | "aif" => Self::Aiff,
            "flac" => Self::Flac,
            _ => Self::Wav,
        })
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        Self::parse(path.extension()?.to_str()?)
    }

    /// 是否需要外部编码器；WAV 由 `nbs-audio` 自己写。
    pub fn needs_encoder(&self) -> bool {
        !matches!(self, Self::Wav)
    }
}

/// 依次尝试：`NBS_FFMPEG` 环境变量、可执行文件同级与 `datafiles\`、当前目录、PATH。
/// 与原版一致（原版靠 curdir 命中 `datafiles/ffmpeg.exe`，否则退回 PATH）。
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NBS_FFMPEG") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(exe_name));
            candidates.push(dir.join("datafiles").join(exe_name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(exe_name));
        candidates.push(cwd.join("datafiles").join(exe_name));
    }
    candidates.into_iter().find(|p| p.is_file()).or_else(|| {
        // PATH 查找：Windows 用 where，类 Unix 用 which。
        let probe = if cfg!(windows) { "where" } else { "which" };
        let out = Command::new(probe).arg(exe_name).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(PathBuf::from)
    })
}

#[derive(Debug)]
pub enum EncodeError {
    /// 没找到可用的 ffmpeg：这是真实失败，绝不静默改写成 WAV。
    EncoderNotFound,
    /// 采样率不在原版列表里。
    UnsupportedRate(u32),
    /// ffmpeg 退出码非 0，或产出空文件。
    Failed(String),
    Io(std::io::Error),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EncoderNotFound => write!(f, "未找到 ffmpeg，无法导出该格式（原版随程序附带 ffmpeg.exe）"),
            Self::UnsupportedRate(r) => write!(f, "不支持的采样率 {r}"),
            Self::Failed(m) => write!(f, "ffmpeg 转码失败: {m}"),
            Self::Io(e) => write!(f, "IO 错误: {e}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// 转码参数，与原版 kwargs 一一对应（`sample_rate` / `channels` / `format`）。
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    pub format: AudioFormat,
    pub sample_rate: u32,
    pub channels: u16,
}

/// 把一个已渲染好的 WAV 转成目标格式。`out` 的扩展名由调用方决定。
pub fn encode_from_wav(wav: &Path, out: &Path, opts: &EncodeOptions) -> Result<(), EncodeError> {
    if !SAMPLE_RATES.contains(&opts.sample_rate) {
        return Err(EncodeError::UnsupportedRate(opts.sample_rate));
    }
    if opts.channels != 1 && opts.channels != 2 {
        return Err(EncodeError::Failed(format!("声道数只支持 1 或 2，收到 {}", opts.channels)));
    }
    if !matches!(opts.format, AudioFormat::Mp3 | AudioFormat::Aiff | AudioFormat::Flac) {
        // WAV 不需要转码；调用方不该走到这里。
        return Ok(());
    }
    let ffmpeg = find_ffmpeg().ok_or(EncodeError::EncoderNotFound)?;
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]);
    cmd.arg(wav);
    match opts.format {
        AudioFormat::Mp3 => {
            cmd.args(["-c:a", "libmp3lame"]);
            cmd.arg("-b:a").arg(format!("{}k", MP3_BITRATE_KBPS));
        }
        AudioFormat::Flac => {
            cmd.args(["-c:a", "flac"]);
        }
        AudioFormat::Aiff => {
            // 原版 nbswave 走 pydub，AIFF 即 PCM s16be。
            cmd.args(["-c:a", "pcm_s16be"]);
        }
        AudioFormat::Wav => {}
    }
    cmd.arg("-ar").arg(opts.sample_rate.to_string());
    cmd.arg("-ac").arg(opts.channels.to_string());
    cmd.arg(out);
    let status = cmd.status().map_err(EncodeError::Io)?;
    if !status.success() {
        return Err(EncodeError::Failed(format!(
            "{} 退出码 {:?}",
            ffmpeg.display(),
            status.code()
        )));
    }
    // ffmpeg 4.4 里 vorbis 这类实验编码器会「返回 0 但写出空文件」，
    // 所以必须真的检查产物大小，而不是只看退出码。
    let size = std::fs::metadata(out).map_err(EncodeError::Io)?.len();
    if size == 0 {
        let _ = std::fs::remove_file(out);
        return Err(EncodeError::Failed("产物为空".into()));
    }
    Ok(())
}

/// `ffmpeg -version` 的第一行，用于在设置窗口里显示真实可用的编码器。
pub fn ffmpeg_version() -> Option<String> {
    let ffmpeg = find_ffmpeg()?;
    let out = Command::new(&ffmpeg).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(format!("{} — {}", ffmpeg.display(), text.lines().next()?.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_parse_rejects_unknown_names() {
        assert_eq!(AudioFormat::parse("MP3"), Some(AudioFormat::Mp3));
        assert_eq!(AudioFormat::parse("  flac "), Some(AudioFormat::Flac));
        assert_eq!(AudioFormat::parse("aif"), Some(AudioFormat::Aiff));
        assert_eq!(AudioFormat::parse("ogg"), None, "原版没有 OGG");
        assert_eq!(AudioFormat::parse(""), None);
    }

    #[test]
    fn sample_rates_match_original_menu() {
        assert_eq!(
            SAMPLE_RATES,
            [8000, 11025, 16000, 22050, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000]
        );
    }

    #[test]
    fn encode_rejects_unsupported_rate_before_spawning() {
        let dir = std::env::temp_dir();
        let opts = EncodeOptions {
            format: AudioFormat::Flac,
            sample_rate: 44101,
            channels: 2,
        };
        let err = encode_from_wav(&dir, &dir.join("nope.flac"), &opts).unwrap_err();
        assert!(matches!(err, EncodeError::UnsupportedRate(44101)), "{err}");
    }

    /// 真跑一次 ffmpeg：需要环境里有可用编码器（原版 datafiles/ffmpeg.exe 或 PATH）。
    /// 标成 ignore 而不是静默 return —— 「跳过」必须是看得见的，不能伪装成通过。
    /// 跑法：`NBS_FFMPEG=<path> cargo test -p nbs-io -- --ignored`
    #[test]
    #[ignore = "需要 ffmpeg：设置 NBS_FFMPEG 或放进 PATH"]
    fn encodes_a_real_wav_when_ffmpeg_available() {
        let Some(ffmpeg) = find_ffmpeg() else {
            panic!("未找到 ffmpeg：设置 NBS_FFMPEG 或把它放进 PATH 后再跑本用例");
        };
        let dir = std::env::temp_dir();
        let wav = dir.join("nbs_encode_test.wav");
        // 16-bit / 44100 / 立体声，0.1s 的 440Hz 正弦，够让所有容器产出非零字节。
        write_test_wav(&wav);
        for (format, magic) in [
            (AudioFormat::Mp3, &b"ID3"[..]),
            (AudioFormat::Flac, &b"fLaC"[..]),
            (AudioFormat::Aiff, &b"FORM"[..]),
        ] {
            let out = dir.join(format!("nbs_encode_test.{}", format.extension()));
            let opts = EncodeOptions { format, sample_rate: 44100, channels: 2 };
            encode_from_wav(&wav, &out, &opts)
                .unwrap_or_else(|e| panic!("{:?} 转码失败（{}）: {e}", format, ffmpeg.display()));
            let bytes = std::fs::read(&out).unwrap();
            assert!(bytes.len() > 1024, "{format:?} 产物过小：{} 字节", bytes.len());
            if format == AudioFormat::Mp3 {
                // ffmpeg 的 mp3 muxer 实测会先写 ID3v2；没有标签时则以帧同步字开头。
                let ok = bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0);
                assert!(ok, "MP3 头不符：{:02X?}", &bytes[..4.min(bytes.len())]);
            } else {
                assert_eq!(&bytes[..magic.len()], magic, "{format:?} 魔数不符");
            }
            let _ = std::fs::remove_file(out);
        }
        let _ = std::fs::remove_file(wav);
    }

    fn write_test_wav(path: &Path) {
        let rate = 44100u32;
        let frames = 4410usize;
        let mut pcm: Vec<i16> = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = ((i as f32 / 4410.0) * std::f32::consts::TAU * 440.0).sin();
            let v = (s * 12000.0) as i16;
            pcm.extend_from_slice(&[v, v]);
        }
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut w: Vec<u8> = Vec::new();
        let u16b = |v: u16| v.to_le_bytes().to_vec();
        let u32b = |v: u32| v.to_le_bytes().to_vec();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&u32b(36 + bytes.len() as u32));
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&u32b(16));
        w.extend_from_slice(&u16b(1));
        w.extend_from_slice(&u16b(2));
        w.extend_from_slice(&u32b(rate));
        w.extend_from_slice(&u32b(rate * 2 * 2));
        w.extend_from_slice(&u16b(4));
        w.extend_from_slice(&u16b(16));
        w.extend_from_slice(b"data");
        w.extend_from_slice(&u32b(bytes.len() as u32));
        w.extend_from_slice(&bytes);
        std::fs::write(path, w).unwrap();
    }
}
