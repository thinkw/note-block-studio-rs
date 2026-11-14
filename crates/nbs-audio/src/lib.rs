//! `nbs-audio` — B 音频引擎 + C 播放调度（已完整实现）。
//!
//! 与 OpenNBS 3.11.0 对齐的关键常量（见 `play_sound.gml` / `control_draw.gml`）：
//! - 采样基准 key = 45；`pitch = 0.5 * 2^((key + base_key + pitch/100 - 78)/12)`
//!   代入 45 即 `0.5 * 2^((key - 33 + pitch/100)/12)`，key=45 时原速。
//! - `vol = layer.volume/100 * note.velocity`；`pan = stereo==100 ? pan : (stereo+pan)/2`
//! - 16 个内置 OGG 有 6×44100 + 10×48000，统一重采样到 44100。
//! - voice 上限 256（可切 1024），满时淘汰最老。
//! - tempo 是 ticks/second；播放由音频采样计数推进，不绑 UI 帧率。
//!
//! 模块地图：
//! - [`samples`]   OGG 解码与采样库
//! - [`mixer`]     voice 池 / pitch shift / gain / pan / 限幅
//! - [`scheduler`] 事件表、tick 时钟、循环、UI↔音频线程共享状态
//! - [`engine`]    cpal 实时输出、离线多线程 WAV 导出
//!
//! 无音频设备时 `CpalEngine` 自动降级为纯时钟，编辑器仍可完整使用。

pub mod engine;
pub mod mixer;
pub mod samples;
pub mod scheduler;

pub use engine::{
    export_wav, render_offline, write_wav_i16, AudioEngine, CpalEngine, ExportOptions,
    ExportStats, SilentEngine,
};
pub use mixer::{Mixer, Voice, DEFAULT_MAX_VOICES, EXTENDED_MAX_VOICES};
pub use samples::{
    decode_ogg, find_sounds_dir, resample_linear, SoundBank, BASE_KEY, BUILTIN_FILES, SAMPLE_RATE,
};
pub use scheduler::{
    build_event_table, make_shared, NoteEvent, PlayParams, Shared, SharedState, TransportClock,
};
