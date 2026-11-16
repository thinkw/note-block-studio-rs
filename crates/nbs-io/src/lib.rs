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
pub mod midi_gm;
pub mod nbt;
pub mod pattern;
pub mod prefs;
pub mod schematic;

// ---- 多语言。原版 GML 的 `language` 是全局变量，深层脚本（导入导出、错误
// 消息）照样读它；这里给 IO 层一份同款原子量，由 nbs-editor 的
// `set_language()` 每帧同步下来，IO 侧文案统一写 tr(英文, 中文)。

use std::sync::atomic::{AtomicU8, Ordering};

static LANGUAGE: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> u8 {
    LANGUAGE.load(Ordering::Relaxed)
}

pub fn set_language(l: u8) {
    LANGUAGE.store(l, Ordering::Relaxed);
}

pub fn tr<'a>(en: &'a str, zh: &'a str) -> &'a str {
    if lang() == 1 {
        zh
    } else {
        en
    }
}

#[cfg(test)]
pub(crate) static TEST_LANG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use image_export::{export_png, render_image, ImageOptions};
pub use midi::{
    export_midi, export_midi_bytes, import_midi_bytes, import_midi_data, import_midi_path,
    parse_midi, MidiData, MidiEvent, MidiImportOptions, MidiImportPrefs, MidiPart, MidiTrack,
};
pub use midi_gm::{
    MidiDrum, MidiGmPersist, MidiGmTable, MidiPatch, MIDI_DRUMS, MIDI_PATCHES,
};
pub use pattern::{
    compress_code, decompress_code, export_pattern, export_pattern_bytes, import_pattern,
    parse_pattern, Pattern, PAT_VERSION,
};
pub use prefs::{
    autosave, list_autosaves, save_song_atomic, Preferences,
};
pub use schematic::{
    block_display_name, block_get_ins, build_nbt, calc_size, calculate_stats, default_ins_block,
    export_check, export_guard, export_schematic, id_isblock, import_bytes, import_schematic,
    inspect_bytes, inspect_schematic, instrument_block_menu, misc_block_menu, namespaced_id,
    open_guard, schematic_height, schematic_length, schematic_width, tempo_to_mode,
    BlockMenuSection, Built, ExportCheck, ImportError, McBlock, SchematicImport, SchematicInfo,
    SchematicOptions, SchematicSize, SchematicStats, INSTRUMENT_BLOCKS, INS_COUNT, MAX_HEI,
    MAX_LEN, MAX_WID,
};
