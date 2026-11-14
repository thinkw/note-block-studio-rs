//! `nbs-io` — E 模块：导入导出与持久化。
//!
//! - [`midi`]          SMF MIDI 导入/导出（量化公式对齐原版 `import_midi.gml`）
//! - [`pattern`]       `.nbp` 选区片段（含 zlib+base64 剪贴板兼容）
//! - [`schematic`]     `.schematic` / 结构方块 `.nbt`（大端 NBT + gzip）
//! - [`datapack`]      Minecraft 数据包（mcfunction + 二分 tick 查找树）
//! - [`image_export`]  PNG 谱面导出
//! - [`prefs`]         JSON 偏好、最近文件、原子保存、自动备份

pub mod audio_export;
pub mod datapack;
pub mod image_export;
pub mod midi;
pub mod nbt;
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
    block_display_name, build_nbt, calc_size, calculate_stats, default_ins_block, export_check,
    export_guard, export_schematic, inspect_bytes, inspect_schematic, instrument_block_menu,
    misc_block_menu, namespaced_id, open_guard, schematic_height, schematic_length, schematic_width,
    tempo_to_mode, BlockMenuSection, Built, ExportCheck, McBlock, SchematicInfo, SchematicOptions,
    SchematicSize, SchematicStats, INSTRUMENT_BLOCKS, INS_COUNT, MAX_HEI, MAX_LEN, MAX_WID,
};
