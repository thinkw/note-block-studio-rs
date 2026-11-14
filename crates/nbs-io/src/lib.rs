//! `nbs-io` — E 模块：导入导出与持久化。
//!
//! - [`midi`]          SMF MIDI 导入/导出（量化公式对齐原版 `import_midi.gml`）
//! - [`pattern`]       `.nbp` 选区片段（含 zlib+base64 剪贴板兼容）
//! - [`schematic`]     `.schematic` / 结构方块 `.nbt`（大端 NBT + gzip）
//! - [`image_export`]  PNG 谱面导出
//! - [`prefs`]         JSON 偏好、最近文件、原子保存、自动备份

pub mod audio_export;
pub mod image_export;
pub mod midi;
pub mod pattern;
pub mod prefs;
pub mod schematic;

pub use image_export::{export_png, render_image, ImageOptions};
pub use midi::{
    export_midi, export_midi_bytes, import_midi_bytes, import_midi_data, import_midi_path,
    parse_midi, MidiData, MidiImportOptions, MidiNote,
};
pub use pattern::{
    compress_code, decompress_code, export_pattern, export_pattern_bytes, import_pattern,
    parse_pattern, Pattern, PAT_VERSION,
};
pub use prefs::{
    autosave, list_autosaves, save_song_atomic, Preferences,
};
pub use schematic::{
    build_schematic_nbt, build_structure_nbt, export_schematic, inspect_schematic,
    tempo_to_delay, SchematicInfo, SchematicOptions, INSTRUMENT_BLOCKS,
};
