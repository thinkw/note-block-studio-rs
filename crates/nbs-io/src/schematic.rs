//! Minecraft `.schematic`（MCEdit 经典 NBT）与 1.13+ 结构方块 `.nbt` 导出。
//!
//! 对齐原版 `schematic_export.gml` / `TAG_*.gml`：
//! - NBT 全部大端；字符串长度是 u16 BE；`Blocks`/`Data` 按 YZX 顺序，
//!   `index = (y*Length + z)*Width + x`。
//! - 音符盒 id=25，TileEntity `note = NBS key - 33`（仅导出 key 33..57）。
//! - 音符盒下方方块决定音色（16 个默认 id 见 `INSTRUMENT_BLOCKS`）。
//! - 红石中继器 data = delay*4 + direction；tempo 映射到 10/5/2.5 tps。
//! - 整个 NBT 用 gzip 压缩。

use std::io::Write;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use nbs_core::model::Song;

/// 16 种内置乐器对应的「音符盒下方方块」数值 id（原版 `reset_schematic_export.gml`）。
pub const INSTRUMENT_BLOCKS: [u8; 16] = [
    3,   // Harp            dirt
    5,   // Double Bass     wooden plank
    4,   // Bass Drum       cobblestone
    12,  // Snare Drum      sand
    20,  // Click           glass
    35,  // Guitar          wool
    82,  // Flute           clay
    41,  // Bell            gold block
    174, // Chime           packed ice
    216, // Xylophone       bone block
    42,  // Iron Xylophone  iron block
    88,  // Cow Bell        soul sand
    86,  // Didgeridoo      pumpkin
    133, // Bit             emerald block
    170, // Banjo           hay block
    89,  // Pling           glowstone
];

const NOTE_BLOCK: u8 = 25;
const REDSTONE_WIRE: u8 = 55;
const REPEATER: u8 = 93;
const STONE: u8 = 1;
const AIR: u8 = 0;

// ---------------------------------------------------------------- NBT 写入

#[derive(Default)]
pub struct Nbt {
    pub buf: Vec<u8>,
}

impl Nbt {
    fn header(&mut self, id: u8, name: &str) {
        self.buf.push(id);
        self.buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(name.as_bytes());
    }
    pub fn compound(&mut self, name: &str) {
        self.header(10, name);
    }
    pub fn end(&mut self) {
        self.buf.push(0);
    }
    pub fn byte(&mut self, name: &str, v: u8) {
        self.header(1, name);
        self.buf.push(v);
    }
    pub fn short(&mut self, name: &str, v: i16) {
        self.header(2, name);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn int(&mut self, name: &str, v: i32) {
        self.header(3, name);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn string(&mut self, name: &str, v: &str) {
        self.header(8, name);
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v.as_bytes());
    }
    pub fn byte_array(&mut self, name: &str, v: &[u8]) {
        self.header(7, name);
        self.buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
        self.buf.extend_from_slice(v);
    }
    pub fn list(&mut self, name: &str, element_type: u8, len: i32) {
        self.header(9, name);
        self.buf.push(element_type);
        self.buf.extend_from_slice(&len.to_be_bytes());
    }
    /// list 里的 int 数组元素（无名字）。
    pub fn list_int(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SchematicOptions {
    /// 每 tick 之间插入的中继器延迟档 0..3（None = 按 tempo 自动）。
    pub repeater_delay: Option<u8>,
    /// 是否把超出 MC 音域（33..57）的音符也放成音符盒（note 夹取）。
    pub include_out_of_range: bool,
    /// 1.13+ 结构方块格式。
    pub structure: bool,
}

impl Default for SchematicOptions {
    fn default() -> Self {
        Self { repeater_delay: None, include_out_of_range: false, structure: false }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SchematicInfo {
    pub width: i16,
    pub height: i16,
    pub length: i16,
    pub note_blocks: usize,
    pub blocks_bytes: usize,
}

/// tempo（tps）→ 中继器延迟档。原版只支持 10 / 5 / 2.5 tps。
pub fn tempo_to_delay(tempo: f32) -> u8 {
    if tempo < 3.75 {
        3
    } else if tempo < 7.5 {
        1
    } else {
        0
    }
}

#[derive(Default)]
struct Grid {
    wid: usize,
    hei: usize,
    len: usize,
    blocks: Vec<u8>,
    data: Vec<u8>,
    tiles: Vec<(i32, i32, i32, u8)>,
}

impl Grid {
    fn new(wid: usize, hei: usize, len: usize) -> Self {
        Self {
            wid,
            hei,
            len,
            blocks: vec![AIR; wid * hei * len],
            data: vec![0; wid * hei * len],
            tiles: Vec::new(),
        }
    }
    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (y * self.len + z) * self.wid + x
    }
    fn set(&mut self, x: usize, y: usize, z: usize, id: u8, data: u8) {
        if x >= self.wid || y >= self.hei || z >= self.len {
            return;
        }
        let i = self.idx(x, y, z);
        self.blocks[i] = id;
        self.data[i] = data;
    }
}

fn build_grid(song: &Song, opts: &SchematicOptions) -> Grid {
    let ticks = song.song_length().max(1) as usize + 2;
    let layers = song.layers.len().max(1);
    let wid = ticks + 2;
    let len = layers * 2 + 3;
    let hei = 5;
    let mut g = Grid::new(wid, hei, len);
    let delay = opts.repeater_delay.unwrap_or_else(|| tempo_to_delay(song.meta.tempo));

    // y=0 地基；y=3 沿 X 的红石线（每条 layer 一行）
    for z in 0..len {
        for x in 0..wid {
            g.set(x, 0, z, STONE, 0);
        }
    }
    for layer in 0..layers {
        let z = layer * 2 + 1;
        for x in 0..wid {
            let is_repeater = x > 0 && x % 4 == 0;
            if is_repeater {
                // data = delay*4 + direction(1 = 朝东)
                g.set(x, 3, z, REPEATER, delay * 4 + 1);
            } else {
                g.set(x, 3, z, REDSTONE_WIRE, 0);
            }
        }
    }

    for (tick, layer, note) in song.iter_notes() {
        let x = tick as usize + 1;
        let z = layer as usize * 2 + 1;
        if x >= wid || z >= len {
            continue;
        }
        if !opts.include_out_of_range && (note.key < 33 || note.key > 57) {
            continue;
        }
        let ins = INSTRUMENT_BLOCKS
            .get(note.instrument as usize)
            .copied()
            .unwrap_or(STONE);
        g.set(x, 1, z, ins, 0);
        g.set(x, 2, z, NOTE_BLOCK, 0);
        g.tiles.push((
            x as i32,
            2,
            z as i32,
            (note.key as i32 - 33).clamp(0, 24) as u8,
        ));
    }
    g
}

/// 生成未压缩的经典 `.schematic` NBT 字节。
pub fn build_schematic_nbt(song: &Song, opts: &SchematicOptions) -> Vec<u8> {
    let g = build_grid(song, opts);
    let mut n = Nbt::default();
    n.compound("Schematic");
    n.short("Width", g.wid as i16);
    n.short("Height", g.hei as i16);
    n.short("Length", g.len as i16);
    n.string("Materials", "Alpha");
    n.byte_array("Blocks", &g.blocks);
    n.byte_array("Data", &g.data);
    n.list("Entities", 10, 0);
    n.list("TileEntities", 10, g.tiles.len() as i32);
    for (x, y, z, note) in &g.tiles {
        n.compound("");
        n.string("id", "minecraft:noteblock");
        n.int("x", *x);
        n.int("y", *y);
        n.int("z", *z);
        n.byte("note", *note);
        n.end();
    }
    n.compound("Version");
    n.int("Id", 922);
    n.string("Name", "1.11.2");
    n.byte("Snapshot", 0);
    n.end();
    n.end();
    n.buf
}

/// 生成 1.13+ 结构方块 `.nbt`（palette + blocks，DataVersion 1519）。
pub fn build_structure_nbt(song: &Song, opts: &SchematicOptions) -> Vec<u8> {
    let g = build_grid(song, opts);
    let mut n = Nbt::default();
    n.compound("");
    n.int("DataVersion", 1519);
    n.list("size", 3, 3);
    n.list_int(g.wid as i32);
    n.list_int(g.hei as i32);
    n.list_int(g.len as i32);

    // palette：0=air 1=stone 2=redstone_wire 3=repeater 4..19=乐器方块 20=note_block
    let palette_names: Vec<String> = {
        let mut v = vec![
            "minecraft:air".to_string(),
            "minecraft:stone".to_string(),
            "minecraft:redstone_wire".to_string(),
            "minecraft:repeater".to_string(),
        ];
        for id in INSTRUMENT_BLOCKS {
            v.push(format!("minecraft:{}", legacy_block_name(id)));
        }
        v.push("minecraft:note_block".to_string());
        v
    };
    n.list("palette", 10, palette_names.len() as i32);
    for name in &palette_names {
        n.compound("");
        n.string("Name", name);
        n.end();
    }

    let mut block_entries: Vec<(usize, usize, usize, i32)> = Vec::new();
    for y in 0..g.hei {
        for z in 0..g.len {
            for x in 0..g.wid {
                let id = g.blocks[g.idx(x, y, z)];
                if id == AIR {
                    continue;
                }
                let state = if id == STONE {
                    1
                } else if id == REDSTONE_WIRE {
                    2
                } else if id == REPEATER {
                    3
                } else if id == NOTE_BLOCK {
                    20
                } else if let Some(p) = INSTRUMENT_BLOCKS.iter().position(|&b| b == id) {
                    4 + p as i32
                } else {
                    0
                };
                block_entries.push((x, y, z, state));
            }
        }
    }
    n.list("blocks", 10, block_entries.len() as i32);
    for (x, y, z, state) in &block_entries {
        n.compound("");
        n.list("pos", 3, 3);
        n.list_int(*x as i32);
        n.list_int(*y as i32);
        n.list_int(*z as i32);
        n.int("state", *state);
        n.end();
    }
    n.list("entities", 10, 0);
    n.end();
    n.buf
}

fn legacy_block_name(id: u8) -> &'static str {
    match id {
        3 => "dirt",
        5 => "oak_planks",
        4 => "cobblestone",
        12 => "sand",
        20 => "glass",
        35 => "white_wool",
        82 => "clay",
        41 => "gold_block",
        174 => "packed_ice",
        216 => "bone_block",
        42 => "iron_block",
        88 => "soul_sand",
        86 => "pumpkin",
        133 => "emerald_block",
        170 => "hay_block",
        89 => "glowstone",
        _ => "stone",
    }
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).map_err(|e| format!("gzip 压缩失败: {e}"))?;
    enc.finish().map_err(|e| format!("gzip 结束失败: {e}"))
}

pub fn export_schematic(song: &Song, path: &Path, opts: &SchematicOptions) -> Result<SchematicInfo, String> {
    let nbt = if opts.structure {
        build_structure_nbt(song, opts)
    } else {
        build_schematic_nbt(song, opts)
    };
    let packed = gzip(&nbt)?;
    std::fs::write(path, packed).map_err(|e| format!("写入 schematic 失败: {e}"))?;
    let g = build_grid(song, opts);
    Ok(SchematicInfo {
        width: g.wid as i16,
        height: g.hei as i16,
        length: g.len as i16,
        note_blocks: g.tiles.len(),
        blocks_bytes: g.blocks.len(),
    })
}

/// 简易 NBT 检查（gzip 解压后读根 compound 的顶层字段），供 UI 与测试用。
pub fn inspect_schematic(path: &Path) -> Result<SchematicInfo, String> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("打开文件失败: {e}"))?;
    let mut data = Vec::new();
    GzDecoder::new(file)
        .read_to_end(&mut data)
        .map_err(|e| format!("gzip 解压失败: {e}"))?;
    let mut info = SchematicInfo::default();
    let mut p = 0usize;
    if data.first() != Some(&10) {
        return Err("根标签不是 Compound".into());
    }
    p += 1;
    let name_len = u16::from_be_bytes([data.get(p).copied().unwrap_or(0), data.get(p + 1).copied().unwrap_or(0)]) as usize;
    p += 2 + name_len;
    while p + 1 < data.len() {
        let id = data[p];
        p += 1;
        if id == 0 {
            break;
        }
        let nl = u16::from_be_bytes([data.get(p).copied().unwrap_or(0), data.get(p + 1).copied().unwrap_or(0)]) as usize;
        p += 2;
        let name = String::from_utf8_lossy(&data[p.min(data.len())..(p + nl).min(data.len())]).into_owned();
        p += nl;
        let skip = |p: &mut usize, bytes: usize| *p += bytes;
        match id {
            1 => skip(&mut p, 1),
            2 => {
                let v = i16::from_be_bytes([data.get(p).copied().unwrap_or(0), data.get(p + 1).copied().unwrap_or(0)]);
                match name.as_str() {
                    "Width" => info.width = v,
                    "Height" => info.height = v,
                    "Length" => info.length = v,
                    _ => {}
                }
                skip(&mut p, 2);
            }
            3 => skip(&mut p, 4),
            7 => {
                let len = i32::from_be_bytes([
                    data.get(p).copied().unwrap_or(0), data.get(p + 1).copied().unwrap_or(0),
                    data.get(p + 2).copied().unwrap_or(0), data.get(p + 3).copied().unwrap_or(0),
                ]) as usize;
                skip(&mut p, 4);
                if name == "Blocks" {
                    info.blocks_bytes = len;
                }
                skip(&mut p, len);
            }
            8 => {
                let l = u16::from_be_bytes([data.get(p).copied().unwrap_or(0), data.get(p + 1).copied().unwrap_or(0)]) as usize;
                skip(&mut p, 2 + l);
            }
            9 => {
                let _ty = data.get(p).copied().unwrap_or(0);
                let count = i32::from_be_bytes([
                    data.get(p + 1).copied().unwrap_or(0), data.get(p + 2).copied().unwrap_or(0),
                    data.get(p + 3).copied().unwrap_or(0), data.get(p + 4).copied().unwrap_or(0),
                ]) as usize;
                skip(&mut p, 5);
                if name == "TileEntities" {
                    info.note_blocks = count;
                }
                // 复合 list 无法简单跳过，遇到就停止解析（尺寸/Blocks 已读到）
                break;
            }
            10 => break,
            _ => break,
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Song;

    #[test]
    fn nbt_header_is_big_endian() {
        let mut n = Nbt::default();
        n.compound("Schematic");
        n.short("Width", 12);
        n.end();
        assert_eq!(n.buf[0], 10);
        assert_eq!(&n.buf[1..3], &[0, 9]);
        assert_eq!(&n.buf[3..12], b"Schematic");
    }

    #[test]
    fn schematic_exports_readable_nbt() {
        let song = Song::demo();
        let dir = std::env::temp_dir().join("nbs_rust_test.schematic");
        let info = export_schematic(&song, &dir, &SchematicOptions::default()).unwrap();
        let back = inspect_schematic(&dir).unwrap();
        assert_eq!(back.width, info.width);
        assert_eq!(back.height, info.height);
        assert_eq!(back.length, info.length);
        assert_eq!(back.blocks_bytes, (info.width as usize) * (info.height as usize) * (info.length as usize));
        assert!(info.note_blocks > 0);
        let _ = std::fs::remove_file(dir);
    }

    #[test]
    fn tempo_maps_to_repeater_delay() {
        assert_eq!(tempo_to_delay(10.0), 0);
        assert_eq!(tempo_to_delay(5.0), 1);
        assert_eq!(tempo_to_delay(2.5), 3);
    }
}
