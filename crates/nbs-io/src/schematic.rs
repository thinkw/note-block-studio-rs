//! Minecraft `.schematic`（MCEdit 经典 NBT）与 1.13+ 结构方块 `.nbt` 导出。
//!
//! 逐条对齐原版脚本：`schematic_export.gml`（三种布局 + 两种格式）、
//! `calculate_size.gml`（音符盒/中继器/高度统计与 tempo 档位）、
//! `schematic_length/width/height.gml`（尺寸）、`block_repeater.gml`、
//! `block_walkway/block_circuit/block_other.gml`、`sch_block_write.gml`（网格寻址）、
//! `reset_schematic_export.gml`（默认值）、`draw_window_schematic_export.gml`（选项面）、
//! `block_get_namespaced_id.gml` 与 `block_get_name.gml`（方块名表）。
//!
//! 原版自身的怪癖按原样保留、不做"修正"，每处都有注释说明：
//! - structure 的 `size[0]` 恒写成 32（与真实 len 无关）。
//! - id 68（地板上的按钮）在 palette 里没有对应项，因此按 else 分支落成 air。
//! - `75, 4` 被原版注释成 "north lit walltorch"，但 75 其实是不亮的火把。
//! - `!compress` 时非音符层会占一个空位（原版用 `nblockins = -1` 表达），
//!   这样音符在行内的排布与原版一致。
//! - 网格写入越界时直接丢弃：原版越界单元也占位，但序列化只读 `0..len/wid/hei`
//!   范围内的格子，所以丢弃与原版逐字节等价。

use std::io::Write;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use nbs_core::model::Song;

use crate::datapack::{dat_pitch, instrument_sound};

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

/// 原版 `sch_exp_ins_block` 数组长度：内置 16 个 + 自定义上限 240 个。
pub const INS_COUNT: usize = 16 + 240;

// ------------------------------------------------------------ 原版方块数值 id
const AIR: u8 = 0;
const STONE: u8 = 1;
const GLASS: u8 = 20;
const NOTE_BLOCK: u8 = 25;
const POWEREDRAIL: u8 = 27;
const WOOL: u8 = 35;
const REDSTONE_WIRE: u8 = 55;
const CHEST: u8 = 54;
const BUTTON_FLOOR: u8 = 68;
const LADDER: u8 = 65;
const RAIL: u8 = 66;
const LEVER: u8 = 69;
const TORCHOFF: u8 = 75;
const TORCHON: u8 = 76;
const BUTTON_WALL: u8 = 77;
const REPEATER: u8 = 93;
/// `block_repeater(..., powered = 1)` 写出的 `93 + 1`。
const REPEATER_POWERED: u8 = 94;

/// 原版硬编码的三组 id（`schematic_export.gml` 开头）。
const INS_NAMES: [&str; 16] = [
    "harp",
    "bass",
    "basedrum",
    "snare",
    "hat",
    "guitar",
    "flute",
    "bell",
    "chime",
    "xylophone",
    "iron_xylophone",
    "cow_bell",
    "didgeridoo",
    "bit",
    "banjo",
    "pling",
];

/// 原版导出按钮的大小上限：`len >= 2000 || wid >= 2000 || hei >= 256` 拒绝。
pub const MAX_LEN: usize = 2000;
pub const MAX_WID: usize = 2000;
pub const MAX_HEI: usize = 256;

// ---------------------------------------------------------------- NBT 写入

/// 大端 NBT 流式写入器，语义与原版 `TAG_*.gml` 一一对应。
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
    /// `TAG_Compound(name)`
    pub fn compound(&mut self, name: &str) {
        self.header(10, name);
    }
    /// `TAG_End()`
    pub fn end(&mut self) {
        self.buf.push(0);
    }
    /// `TAG_Byte(name, value)`
    pub fn byte(&mut self, name: &str, v: u8) {
        self.header(1, name);
        self.buf.push(v);
    }
    /// `TAG_Short(name, value)`
    pub fn short(&mut self, name: &str, v: i16) {
        self.header(2, name);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    /// `TAG_Int(name, value)`
    pub fn int(&mut self, name: &str, v: i32) {
        self.header(3, name);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    /// `TAG_Float(name, value)`
    pub fn float(&mut self, name: &str, v: f32) {
        self.header(5, name);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    /// `TAG_String(name, value)`
    pub fn string(&mut self, name: &str, v: &str) {
        self.header(8, name);
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v.as_bytes());
    }
    /// `TAG_Byte_Array(name, length)`：只写头，数据由调用方续写。
    pub fn byte_array(&mut self, name: &str, v: &[u8]) {
        self.header(7, name);
        self.buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
        self.buf.extend_from_slice(v);
    }
    /// `TAG_List(name, length, type)` —— 参数顺序照原版：先长度后类型。
    pub fn list(&mut self, name: &str, count: i32, element_type: u8) {
        self.header(9, name);
        self.buf.push(element_type);
        self.buf.extend_from_slice(&count.to_be_bytes());
    }
    /// list 里的匿名 int 元素。
    pub fn item_int(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    /// list 里的匿名 double 元素。
    pub fn item_double(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    /// list 里的匿名 float 元素。
    pub fn item_float(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
}

// -------------------------------------------------------------- 方块名表（原版）

/// `block_get_namespaced_id` 的基础名（不带颜色前缀）；表里没有的 id 返回空串。
fn block_base_name(val: u8) -> &'static str {
    match val {
        0 => "air",
        1 => "stone",
        2 => "grass_block",
        3 => "dirt",
        4 => "cobblestone",
        5 => "oak_planks",
        7 => "bedrock",
        12 => "sand",
        13 => "gravel",
        14 => "gold_ore",
        15 => "iron_ore",
        16 => "coal_ore",
        17 => "oak_log",
        18 => "oak_leaves",
        19 => "sponge",
        20 => "glass",
        21 => "lapis_ore",
        22 => "lapis_block",
        23 => "dispenser",
        24 => "sandstone",
        25 => "note_block",
        26 => "bed",
        27 => "golden_rail",
        28 => "detector_rail",
        29 => "sticky_piston",
        30 => "cobweb",
        33 => "piston",
        35 => "wool",
        41 => "gold_block",
        42 => "iron_block",
        45 => "brick_block",
        46 => "tnt",
        47 => "bookshelf",
        48 => "mossy_cobblestone",
        49 => "obsidian",
        52 => "mob_spawner",
        54 => "chest",
        56 => "diamond_ore",
        57 => "diamond_block",
        58 => "crafting_table",
        60 => "farmland",
        61 => "furnace",
        62 => "lit_furnace",
        73 => "redstone_ore",
        74 => "lit_redstone_ore",
        78 => "snow_layer",
        79 => "ice",
        80 => "snow",
        82 => "clay",
        86 => "pumpkin",
        87 => "netherrack",
        88 => "soul_sand",
        89 => "glowstone",
        91 => "jack_o_lantern",
        92 => "cake",
        95 => "stained_glass",
        97 => "infested_stone",
        98 => "stone_bricks",
        99 => "brown_mushroom_block",
        100 => "red_mushroom_block",
        103 => "melon",
        110 => "mycelium",
        112 => "nether_brick",
        121 => "end_stone",
        123 => "redstone_lamp",
        124 => "lit_redstone_lamp",
        126 => "oak_slab",
        129 => "emerald_ore",
        133 => "emerald_block",
        137 => "command_block",
        138 => "beacon",
        151 => "daylight_detector",
        152 => "redstone_block",
        153 => "quartz_ore",
        155 => "quartz_block",
        159 => "terracotta",
        160 => "stained_glass_pane",
        161 => "dark_oak_leaves",
        162 => "dark_oak_log",
        165 => "slime_block",
        168 => "prismarine",
        169 => "sea_lantern",
        170 => "hay_block",
        171 => "carpet",
        172 => "terracotta",
        173 => "coal_block",
        174 => "packed_ice",
        179 => "red_sandstone",
        201 => "purpur_block",
        202 => "purpur_pillar",
        206 => "end_bricks",
        208 => "grass_path",
        210 => "repeating_command_block",
        211 => "chain_command_block",
        212 => "frosted_ice",
        213 => "magma",
        214 => "nether_wart_block",
        215 => "red_nether_brick",
        216 => "bone_block",
        251 => "concrete",
        252 => "concrete_powder",
        _ => "",
    }
}

/// 原版只有这 8 个 id 会取颜色前缀。
fn is_color_block(val: u8) -> bool {
    matches!(val, 35 | 95 | 159 | 160 | 171 | 172 | 251 | 252)
}

fn color_prefix(data: u8) -> &'static str {
    match data {
        1 => "orange_",
        2 => "magenta_",
        3 => "light_blue_",
        4 => "yellow_",
        5 => "lime_",
        6 => "pink_",
        7 => "gray_",
        8 => "light_gray_",
        9 => "cyan_",
        10 => "purple_",
        11 => "blue_",
        12 => "brown_",
        13 => "green_",
        14 => "red_",
        15 => "black_",
        _ => "white_",
    }
}

/// 原版 `block_get_namespaced_id(val, data)`，不含 `minecraft:` 前缀。
/// 注意 172 虽然在颜色判定里，但 `case 172` 直接返回 "terracotta"，前缀算了不用。
pub fn namespaced_id(val: u8, data: u8) -> String {
    let base = block_base_name(val);
    if base.is_empty() {
        return String::new();
    }
    if is_color_block(val) && val != 172 {
        format!("{}{}", color_prefix(data), base)
    } else {
        base.to_string()
    }
}

/// 原版 `block_get_name(val, data)`，界面方块名。
pub fn block_display_name(val: u8, data: u8) -> String {
    let color = match data {
        1 => "Orange",
        2 => "Magenta",
        3 => "Light Blue",
        4 => "Yellow",
        5 => "Lime",
        6 => "Pink",
        7 => "Gray",
        8 => "Light Gray",
        9 => "Cyan",
        10 => "Purple",
        11 => "Blue",
        12 => "Brown",
        13 => "Green",
        14 => "Red",
        15 => "Black",
        _ => "White",
    };
    let colored = matches!(val, 35 | 95 | 159 | 160 | 171 | 172);
    let named: &str = match val {
        0 => "Air",
        1 => "Stone",
        2 => "Grass Block",
        3 => "Dirt",
        4 => "Cobblestone",
        5 => "Wood Planks",
        7 => "Bedrock",
        12 => "Sand",
        13 => "Gravel",
        14 => "Gold Ore",
        15 => "Iron Ore",
        16 => "Coal Ore",
        17 => "Wood",
        18 => "Leaves",
        19 => "Sponge",
        20 => "Glass",
        21 => "Lapis Lazuli Ore",
        22 => "Lapis Lazuli Block",
        23 => "Dispenser",
        24 => "Sandstone",
        25 => "Note Block",
        26 => "Bed",
        27 => "Powered Rail",
        28 => "Detector Rail",
        29 => "Sticky Piston",
        30 => "Cobweb",
        33 => "Piston",
        41 => "Block of Gold",
        42 => "Block of Iron",
        45 => "Bricks",
        46 => "TNT",
        47 => "Bookshelf",
        48 => "Moss stone",
        49 => "Obsidian",
        52 => "Monster Spawner",
        54 => "Chest",
        56 => "Diamond Ore",
        57 => "Block of Diamond",
        58 => "Crafting Table",
        60 => "Farmland",
        61 => "Furnace",
        62 => "Burning Furnace",
        73 => "Redstone Ore",
        74 => "Glowing Redstone Ore",
        78 | 80 => "Snow",
        79 => "Ice",
        82 => "Clay",
        86 => "Pumpkin",
        87 => "Netherrack",
        88 => "Soul Sand",
        89 => "Glowstone",
        91 => "Jack - o'Lantern",
        92 => "Cake",
        97 => "Monster Egg",
        98 => "Stone Bricks",
        99 => "Brown Mushroom",
        100 => "Red Mushroom",
        103 => "Melon",
        110 => "Mycelium",
        112 => "Nether Brick",
        121 => "End Stone",
        123 => "Redstone Lamp (inactive)",
        124 => "Redstone Lamp (active)",
        126 => "Wooden Slab",
        129 => "Emerald Ore",
        133 => "Block of Emerald",
        137 => "Command Block",
        138 => "Beacon",
        151 => "Daylight Sensor",
        152 => "Block of Redstone",
        153 => "Nether Quartz Ore",
        155 => "Block of Quartz",
        161 => "Leaves (Acacia / Dark Oak)",
        162 => "Wood (Acacia / Dark Oak)",
        165 => "Slime Block",
        168 => "Prismarine",
        169 => "Sea Lantern",
        170 => "Hay Bale",
        172 => "Terracotta",
        173 => "Block of Coal",
        174 => "Packed Ice",
        179 => "Red Sandstone",
        201 => "Purpur Block",
        202 => "Purpur Pillar",
        206 => "End Bricks",
        208 => "Grass Path",
        210 => "Repeating Command Block",
        211 => "Chain Command Block",
        212 => "Frosted Ice",
        213 => "Magma Block",
        214 => "Nether Wart Block",
        215 => "Red Nether Brick",
        216 => "Bone Block",
        251 => "Concrete",
        252 => "Concrete Powder",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    // 只有带颜色且原表里确实存在的那几个 id 才走颜色分支（160/171 在原版里没有 case）。
    match val {
        35 if colored => format!("{color} Wool"),
        95 if colored => format!("{color} Stained Glass"),
        159 if colored => format!("{color} Terracotta"),
        _ => String::new(),
    }
}

/// 原版 `legacy_block_name`：`INSTRUMENT_BLOCKS` 里的 id 对应的 namespaced 名。
/// 未知 id 回落到 `stone`，与原版自定义乐器的默认方块（id 1）一致。
pub(crate) fn legacy_block_name(id: u8) -> &'static str {
    if id == WOOL {
        return "white_wool";
    }
    let n = block_base_name(id);
    if n.is_empty() {
        "stone"
    } else {
        n
    }
}

// ---------------------------------------------------------------- 方块选择菜单

/// 原版 `schexport_insblock` / `schexport_block` 菜单里的一段：35/95/159 三种带数据
/// 的方块收进一个子菜单（"Wool" 等）下，其余方块各自一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMenuSection {
    /// 子菜单标题，普通方块为 `None`。
    pub title: Option<&'static str>,
    pub items: Vec<McBlock>,
}

fn plain_blocks(ids: &[u8]) -> Vec<BlockMenuSection> {
    ids.iter()
        .map(|&id| BlockMenuSection { title: None, items: vec![McBlock::new(id, 0)] })
        .collect()
}

fn color_section(title: &'static str, id: u8) -> BlockMenuSection {
    BlockMenuSection {
        title: Some(title),
        items: (0..16u8).map(|data| McBlock::new(id, data)).collect(),
    }
}

/// 原版的「全部方块」候选表：只收 `block_get_name(a, 0) != ""` 的 id，
/// 35/95/159 展开成 16 色子菜单。电路菜单不给玻璃（20）和音符盒（25）。
fn all_blocks(titles: [&'static str; 3], circuit: bool) -> Vec<BlockMenuSection> {
    let mut out = Vec::new();
    for id in 0..=255u8 {
        if circuit && matches!(id, GLASS | NOTE_BLOCK) {
            continue;
        }
        match id {
            35 => out.push(color_section(titles[0], id)),
            95 => out.push(color_section(titles[1], id)),
            159 => out.push(color_section(titles[2], id)),
            _ => {
                if !block_display_name(id, 0).is_empty() {
                    out.push(BlockMenuSection { title: None, items: vec![McBlock::new(id, 0)] });
                }
            }
        }
    }
    out
}

/// 某个乐器的方块候选表。原版内置 0..=9 各有专用短表，10 及以上走全部方块。
pub fn instrument_block_menu(ins: usize) -> Vec<BlockMenuSection> {
    match ins {
        0 => plain_blocks(&[0, 2, 3, 18, 19]),
        1 => plain_blocks(&[5, 17, 25, 47]),
        2 => plain_blocks(&[1, 4, 7, 14, 15, 16, 21, 22, 24, 48, 49, 73, 87, 89]),
        3 => plain_blocks(&[12, 13, 88]),
        4 => plain_blocks(&[GLASS]),
        5 => vec![color_section("Wool", 35)],
        6 => plain_blocks(&[82]),
        7 => plain_blocks(&[41]),
        8 => plain_blocks(&[174]),
        9 => plain_blocks(&[216]),
        _ => all_blocks(["Wool", "Stained glass", "Colored Terracotta"], false),
    }
}

/// 过道 / 电路 / 地面所用方块的候选表（原版 `menua` 0/1/2）。
/// 两处 `Stained glass` 的大小写差异照抄原版（方块页小写 g，杂项页大写 G）。
pub fn misc_block_menu(which: u8) -> Vec<BlockMenuSection> {
    all_blocks(["Wool", "Stained Glass", "Colored Terracotta"], which == 1)
}

// -------------------------------------------------------------------- 导出选项

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McBlock {
    pub id: u8,
    pub data: u8,
}

impl McBlock {
    pub const fn new(id: u8, data: u8) -> Self {
        Self { id, data }
    }
    /// 原界面里显示的形式：`"35, 11"` 加方块名。
    pub fn label(&self) -> String {
        format!("{}, {} — {}", self.id, self.data, block_display_name(self.id, self.data))
    }
}

/// 原版 `sch_exp_*` 全套选项（`reset_schematic_export(0)` 的默认值）。
/// 整份随 `settings.ini` 的 `[schematic_export]` 段持久化，不受「记住我的更改」影响。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SchematicOptions {
    /// `sch_exp_layout`：0 = 环形过道，1 = 普通过道，2 = Classic。
    pub layout: u8,
    /// `sch_exp_notesperrow`：5..=100。
    pub notesperrow: usize,
    /// `sch_exp_includelocked`。
    pub include_locked: bool,
    /// `sch_exp_compress`。
    pub compress: bool,
    /// `sch_exp_minecart`（仅 layout < 2 生效）。
    pub minecart: bool,
    /// `sch_exp_chest`（仅 minecart 生效）。
    pub chest: bool,
    /// `sch_exp_loop`（仅 layout == 0 生效）。
    pub loop_option: bool,
    /// `sch_exp_glass`（仅 Classic 生效）。
    pub glass: bool,
    /// `structure`：true 走 1.13+ 结构方块，false 走经典 schematic。
    pub structure: bool,
    /// `sch_exp_minecraft_old`：pre-1.11 的 id 命名与 Version 段。
    pub minecraft_old: bool,
    /// `command_block`：仅 structure 生效，用命令方块换取更广音域。
    pub command_block: bool,
    /// `sch_exp_tempo`：0 = 10 t/s，1 = 5 t/s，2 = 2.5 t/s。
    /// 原版在打开窗口时由 `calculate_size()` 按歌曲 tempo 覆写一次，之后可手动改。
    pub tempo_mode: u8,
    pub walkway: McBlock,
    pub circuit: McBlock,
    pub ground: McBlock,
    /// `sch_exp_ins_block` / `sch_exp_ins_data`，按乐器索引（0..INS_COUNT）。
    pub ins_block: Vec<u8>,
    pub ins_data: Vec<u8>,
}

impl Default for SchematicOptions {
    fn default() -> Self {
        Self {
            layout: 1,
            notesperrow: 20,
            include_locked: false,
            compress: true,
            minecart: false,
            chest: true,
            loop_option: true,
            glass: true,
            structure: false,
            minecraft_old: false,
            command_block: false,
            tempo_mode: 0,
            walkway: McBlock::new(STONE, 0),
            circuit: McBlock::new(STONE, 0),
            ground: McBlock::new(STONE, 0),
            ins_block: default_ins_block(),
            ins_data: vec![0; INS_COUNT],
        }
    }
}

/// 原版 `reset_schematic_export` 的乐器方块默认表。
pub fn default_ins_block() -> Vec<u8> {
    let mut v = vec![STONE; INS_COUNT];
    v[..16].copy_from_slice(&INSTRUMENT_BLOCKS);
    v
}

impl SchematicOptions {
    /// 原版「使用默认值」按钮：`reset_schematic_export(1)`。
    /// `sch_exp_tempo`、`structure`、`command_block` 都不在这张默认值表里，
    /// 原版只把前两者写在 `[schematic_export]` / `[preferences]` 中持久化，重置时原样保留。
    pub fn reset(&mut self) {
        *self = Self {
            tempo_mode: self.tempo_mode,
            structure: self.structure,
            command_block: self.command_block,
            ..Default::default()
        };
    }

    /// 原版的 `median(5, drag, 100)` 夹取。
    pub fn set_notesperrow(&mut self, v: usize) {
        self.notesperrow = v.clamp(5, 100);
    }

    fn ins(&self, index: usize) -> McBlock {
        McBlock {
            id: self.ins_block.get(index).copied().unwrap_or(STONE),
            data: self.ins_data.get(index).copied().unwrap_or(0),
        }
    }

    /// 原界面里 16 个内置乐器 + 自定义乐器的方块设置行数。
    pub fn instrument_rows(&self, custom: usize) -> usize {
        (16 + custom).min(INS_COUNT)
    }
}

// ------------------------------------------------------------------ 歌曲统计

/// 原版 `calculate_size()` 计算出的、只与歌曲有关的统计量。
#[derive(Clone, Debug, Default)]
pub struct SchematicStats {
    /// `sch_exp_totalblocks[0]` = 不含静音层，`[1]` = 含静音层。
    pub totalblocks: [usize; 2],
    /// `sch_exp_maxheight[0]` = 不压缩（最大层号），`[1]` = 压缩（单 tick 最多音符数）。
    pub maxheight: [usize; 2],
    /// `sch_exp_repeaters[include_locked][tempo]`。
    pub repeaters: [[usize; 3]; 2],
    /// `scp_exp_enda[include_locked]`：最后一个还有可用音符的 tick。
    pub scp_enda: [u32; 2],
    /// `sch_colamount[tempo][tick]` 的 [不静音, 含静音] 两份。
    pub col_amount: Vec<[usize; 2]>,
    /// `sch_exp_tempo` 的自动值。
    pub tempo_mode: u8,
    /// 歌曲 tempo，用于「与 Minecraft 不兼容的速度」提示。
    pub tempo: f32,
    /// 原版随编辑维护的三个兼容性计数（这里从歌曲即时派生，条件相同）。
    pub block_outside: usize,
    pub block_custom: usize,
    pub block_pitched: usize,
    /// 歌曲音符总数（原版 `totalblocks`）。
    pub song_blocks: usize,
}

impl SchematicStats {
    /// 原版 `calculate_size()` 打开窗口前的三段 y/n 提问，顺序与判定条件一致；
    /// 任一段选「否」都不打开导出窗口。
    pub fn compat_questions(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.block_outside > 0 {
            v.push("一些方块在 Minecraft 的2八度范围外。你想无视这些方块导出 Schematic 吗？".into());
        }
        if self.block_custom > 0 {
            v.push("一些方块带有自定义音色。继续导出吗？".into());
        }
        if self.block_pitched > 0 {
            v.push("一些方块带有音符盒不支持的微分音。继续导出吗？".into());
        }
        v
    }

    /// 原版速度不兼容时的单次提示（`message`，只确定，不中断导出）。
    pub fn tempo_notice(&self) -> Option<&'static str> {
        let t = self.tempo;
        if !(t == 10.0 || t == 5.0 || t == 2.5) {
            Some("歌曲的速度与 Minecraft 不兼容。所以，在游戏内的播放会和软件内的有差别。")
        } else {
            None
        }
    }
}

/// tempo（tps）→ 原版 `sch_exp_tempo` 档位。
pub fn tempo_to_mode(tempo: f32) -> u8 {
    if tempo < 3.75 {
        2
    } else if tempo < 7.5 {
        1
    } else {
        0
    }
}

/// 每档 tempo 下，一个 tick 相当于多少个「中继器周期」：0 → +1，1 → +2，2 → +4。
fn rep_step(tempo_mode: u8) -> u32 {
    match tempo_mode {
        1 => 2,
        2 => 4,
        _ => 1,
    }
}

pub fn calculate_stats(song: &Song) -> SchematicStats {
    let locked = song.locked_layers();
    let enda = song.enda;
    let mut s = SchematicStats::default();
    s.col_amount = vec![[0usize; 2]; enda as usize + 1];
    s.scp_enda = [0; 2];
    s.tempo = song.meta.tempo;
    s.tempo_mode = tempo_to_mode(song.meta.tempo);

    for a in 0..=enda {
        let Some(col) = song.columns.get(&a) else { continue };
        if col.amount() == 0 {
            continue;
        }
        let last = col.last().unwrap_or(0) as usize;
        let mut c = 0usize; // 含静音层
        let mut d = 0usize; // 不含静音层
        for b in 0..=last {
            let Some(note) = col.notes.get(&(b as u32)) else { continue };
            if note.key > 32 && note.key < 58 {
                c += 1;
                s.totalblocks[1] += 1;
                if !locked.get(b).copied().unwrap_or(false) {
                    d += 1;
                    s.totalblocks[0] += 1;
                }
            }
        }
        s.maxheight[0] = s.maxheight[0].max(last);
        s.maxheight[1] = s.maxheight[1].max(c);
        s.col_amount[a as usize] = [d, c];
        if c > 0 {
            s.scp_enda[1] = a;
        }
        if d > 0 {
            s.scp_enda[0] = a;
        }
    }

    for (_, _, note) in song.iter_notes() {
        s.song_blocks += 1;
        if note.key < 33 || note.key > 57 {
            s.block_outside += 1;
        }
        if note.instrument >= nbs_core::model::FIRST_CUSTOM_INDEX {
            s.block_custom += 1;
        }
        if note.pitch != 0 {
            s.block_pitched += 1;
        }
    }

    // 原版：三个 tempo 档一次算完，cnt 记录「距离上一个中继器过了几个 tick」。
    let mut cnt = [[0usize; 3]; 2];
    for a in 0..=enda {
        for b in 0..2 {
            if a > s.scp_enda[b] {
                continue;
            }
            s.repeaters[b][2] += 1;
            if s.col_amount[a as usize][b] > 0 {
                s.repeaters[b][0] += 1;
                cnt[b][0] = 0;
                s.repeaters[b][1] += 1;
                cnt[b][1] = 0;
            } else {
                cnt[b][0] += 1;
                cnt[b][1] += 1;
            }
            if cnt[b][0] == 4 {
                s.repeaters[b][0] += 1;
                cnt[b][0] = 0;
            }
            if cnt[b][1] == 2 {
                s.repeaters[b][1] += 1;
                cnt[b][1] = 0;
            }
        }
    }
    s
}

// -------------------------------------------------------------------- 尺寸计算

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchematicSize {
    pub len: usize,
    pub wid: usize,
    pub hei: usize,
    pub layers: usize,
}

/// 原版 `schematic_width()`。
pub fn schematic_width(opts: &SchematicOptions) -> usize {
    if opts.layout < 2 {
        opts.notesperrow * 2 + 3 + if opts.layout == 0 { 4 } else { 0 }
    } else {
        opts.notesperrow * 2 + 3
    }
}

/// 原版 `schematic_height()`。
pub fn schematic_height(stats: &SchematicStats, opts: &SchematicOptions) -> usize {
    4 + 3 * ceil_div(stats.maxheight[opts.compress as usize], 4)
}

/// 原版 `schematic_length()`。
pub fn schematic_length(stats: &SchematicStats, opts: &SchematicOptions) -> usize {
    let rows = opts.notesperrow;
    let repeaters = stats.repeaters[opts.include_locked as usize][opts.tempo_mode as usize];
    if opts.layout < 2 {
        let mut len = 2 + ceil_div(repeaters, rows) * 3;
        if opts.layout == 0 && ceil_div(repeaters, rows) % 2 == 1 {
            len += 3;
        }
        len
    } else {
        (ceil_div(repeaters + 2, rows) * 3 + 1).max(6)
    }
}

pub fn calc_size(stats: &SchematicStats, opts: &SchematicOptions) -> SchematicSize {
    SchematicSize {
        len: schematic_length(stats, opts),
        wid: schematic_width(opts),
        hei: schematic_height(stats, opts),
        layers: ceil_div(stats.maxheight[opts.compress as usize], 4),
    }
}

fn ceil_div(a: usize, b: usize) -> usize {
    if b == 0 {
        0
    } else {
        a.div_ceil(b)
    }
}

// ---------------------------------------------------------------------- 网格

/// 原版 `sch_block_write(x, y, z, val)` 的三维网格；轴依次为
/// `a ∈ [0, len)`、`b ∈ [0, wid)`、`c ∈ [0, hei)`。
struct Grid {
    len: usize,
    wid: usize,
    hei: usize,
    blocks: Vec<u8>,
    data: Vec<u8>,
}

impl Grid {
    fn new(len: usize, wid: usize, hei: usize) -> Self {
        let n = len * wid * hei;
        Self { len, wid, hei, blocks: vec![AIR; n], data: vec![0; n] }
    }
    #[inline]
    fn idx(&self, a: usize, b: usize, c: usize) -> usize {
        (a * self.hei + c) * self.wid + b
    }
    /// 越界写入直接丢弃（原版会写进那块 2000×2000×256 的大表，但序列化读不到）。
    fn set(&mut self, a: i32, b: i32, c: i32, id: u8, data: u8) {
        if a < 0 || b < 0 || c < 0 {
            return;
        }
        let (a, b, c) = (a as usize, b as usize, c as usize);
        if a >= self.len || b >= self.wid || c >= self.hei {
            return;
        }
        let i = self.idx(a, b, c);
        self.blocks[i] = id;
        self.data[i] = data;
    }
    #[inline]
    fn at(&self, a: usize, b: usize, c: usize) -> (u8, u8) {
        let i = self.idx(a, b, c);
        (self.blocks[i], self.data[i])
    }
    /// 原版序列化顺序：`for c { for a { for b = wid-1 .. 0 } }`。
    fn serialize(&self) -> (Vec<u8>, Vec<u8>) {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        let mut data = Vec::with_capacity(self.blocks.len());
        for c in 0..self.hei {
            for a in 0..self.len {
                for b in (0..self.wid).rev() {
                    let i = self.idx(a, b, c);
                    blocks.push(self.blocks[i]);
                    data.push(self.data[i]);
                }
            }
        }
        (blocks, data)
    }
}

/// 一个 tick 里按原版排布顺序占的一格：真实音符，或 `!compress` 时的空位。
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// 乐器索引；-1 表示空位（原版 `nblockins = -1`）。
    ins: i32,
    key: u8,
    pit: i16,
}

/// 原版 `noteblockx/y/z/note/ins/pit` 六张平行表。
#[derive(Clone, Copy, Debug)]
struct NoteEntry {
    x: i32,
    y: i32,
    z: i32,
    note: i32,
    ins: usize,
    pit: i16,
}

/// 一次导出的完整中间结果：网格 + 音符盒表 + 原版在导出时算出的所有开关。
pub struct Built {
    grid: Grid,
    notes: Vec<NoteEntry>,
    size: SchematicSize,
    opts: SchematicOptions,
    /// 原版界面里「中继器」那一行显示的 `sch_exp_repeaters[includelocked][tempo]`。
    repeaters: usize,
    insnum: usize,
    /// 导出时真正生效的开关（原版做了与 layout 的与运算）。
    sch_loop: bool,
    minecart: bool,
    chest_on: bool,
    cyy: i32,
    signx: i32,
    signy: i32,
    signz: i32,
    chestx: i32,
    chesty: i32,
    chestz: i32,
}

fn gm_round(v: f64) -> i32 {
    // GameMaker 的 round() 是「.5 远离零」，与 Rust 的 f64::round 一致。
    v.round() as i32
}

/// 逐音符盒地放置，与原版 `schematic_export.gml` 的四个 note block 分支一致。
struct Builder<'a> {
    song: &'a Song,
    o: &'a SchematicOptions,
    locked: Vec<bool>,
    size: SchematicSize,
    g: Grid,
    notes: Vec<NoteEntry>,
    tempo_mode: u8,
    compress: bool,
    include_locked: bool,
    cyy: i32,
    sign: (i32, i32, i32),
    chest: (i32, i32, i32),
    sch_loop: bool,
}

impl<'a> Builder<'a> {
    fn block_walkway(&mut self, a: i32, b: i32, c: i32) {
        let w = self.o.walkway;
        self.g.set(a, b, c, w.id, w.data);
    }
    fn block_circuit(&mut self, a: i32, b: i32, c: i32) {
        let w = self.o.circuit;
        self.g.set(a, b, c, w.id, w.data);
    }
    fn block_other(&mut self, a: i32, b: i32, c: i32, id: u8, data: u8) {
        self.g.set(a, b, c, id, data);
    }
    fn block_repeater(&mut self, a: i32, b: i32, c: i32, delay: u8, direction: u8, powered: bool) {
        let id = if powered { REPEATER_POWERED } else { REPEATER };
        self.g.set(a, b, c, id, delay * 4 + direction);
    }

    /// 原版 `nblockins/nblockkey/nblockpit` 的构造过程。
    fn tick_slots(&self, tick: u32) -> Vec<Slot> {
        let mut out = Vec::new();
        let Some(col) = self.song.columns.get(&tick) else { return out };
        if col.amount() == 0 {
            return out;
        }
        let last = col.last().unwrap_or(0);
        for b in 0..=last {
            let note = col.notes.get(&b);
            let unlocked = !self.locked.get(b as usize).copied().unwrap_or(false);
            let mc_range = note.map(|n| n.key > 32 && n.key < 58).unwrap_or(false);
            let cb_range = note
                .map(|n| self.o.structure && self.o.command_block && n.key >= 9 && n.key <= 81)
                .unwrap_or(false);
            match note {
                Some(n) if unlocked || self.include_locked => {
                    if mc_range || cb_range {
                        out.push(Slot { ins: n.instrument as i32, key: n.key, pit: n.pitch });
                    } else if !self.compress {
                        out.push(Slot { ins: -1, key: 0, pit: 0 });
                    }
                }
                _ => {
                    if !self.compress {
                        out.push(Slot { ins: -1, key: 0, pit: 0 });
                    }
                }
            }
        }
        out
    }

    /// 原版一次成行时放的 4 个音符盒位（每层 b 一组，最多消耗 4 个 slot）。
    fn place_row(&mut self, xx: i32, yy: i32, dir: i32, rep: u32, slots: &[Slot], n: &mut i32) {
        let direction = if dir == 1 { 3 } else { 1 };
        for b in 0..self.size.layers {
            let y1 = yy + dir;
            self.block_repeater(xx, y1, (b * 3 + 2) as i32, (rep - 1) as u8, direction, false);
            self.block_circuit(xx, y1, (b * 3 + 1) as i32);
            self.block_circuit(xx, y1 + dir, (b * 3 + 2) as i32);
            if b > 0 {
                self.block_circuit(xx, y1, (b * 3) as i32);
                self.block_circuit(xx, y1 + dir, (b * 3) as i32);
            }
            self.block_circuit(xx, y1 + dir * 2, (b * 3 + 1) as i32);
            // Note block #1：这一格还会额外铺一段 wire。
            if *n > -1 {
                self.block_other(xx, y1 + dir, (b * 3 + 1) as i32, REDSTONE_WIRE, 0);
                self.put_note(xx - 1, y1 + dir * 2, (b * 3 + 1) as i32, (b * 3) as i32, slots, *n);
                *n -= 1;
            }
            if *n > -1 {
                self.put_note(xx + 1, y1 + dir * 2, (b * 3 + 1) as i32, (b * 3) as i32, slots, *n);
                *n -= 1;
            }
            if *n > -1 {
                self.put_note(xx - 1, y1 + dir, (b * 3 + 2) as i32, (b * 3 + 1) as i32, slots, *n);
                *n -= 1;
            }
            if *n > -1 {
                self.put_note(xx + 1, y1 + dir, (b * 3 + 2) as i32, (b * 3 + 1) as i32, slots, *n);
                *n -= 1;
            }
        }
    }

    fn put_note(&mut self, x: i32, y: i32, z: i32, z_below: i32, slots: &[Slot], n: i32) {
        let s = slots[n as usize];
        if s.ins < 0 {
            return;
        }
        let block = self.o.ins(s.ins as usize);
        self.block_other(x, y, z, NOTE_BLOCK, 0);
        self.block_other(x, y, z_below, block.id, block.data);
        self.notes.push(NoteEntry {
            x,
            y,
            z,
            note: s.key as i32 - 33,
            ins: s.ins as usize,
            pit: s.pit,
        });
    }

    fn build_walkway(&mut self) {
        let len = self.size.len as i32;
        let wid = self.size.wid as i32;
        let hei = self.size.hei as i32;
        let layers = self.size.layers;
        let layout = self.o.layout;

        // 地板
        for a in 0..len {
            for b in 0..wid {
                self.block_other(a, b, 0, self.o.ground.id, self.o.ground.data);
            }
        }

        // 立柱 + 梯子
        let yy0 = gm_round(wid as f64 / 2.0);
        self.cyy = yy0;
        for b in 1..hei - 2 {
            for a in 0..3 {
                self.block_walkway(1, yy0 + a, b);
            }
            self.block_other(0, yy0 + 2, b, LADDER, 2);
        }

        // 过道 + 矿车轨道
        let minecart_on = self.o.minecart;
        let start_a = if minecart_on { -1 } else { 0 };
        let end_a = 3 + if minecart_on && layout == 0 { 1 } else { 0 };
        let mut a = start_a;
        while a < end_a {
            for b in 2..len {
                self.block_walkway(b, yy0 + a, hei - 3);
                if a == -1 {
                    if b == len - 1 && layout == 1 {
                        self.block_walkway(b, yy0 + a, hei - 2);
                    } else if b.rem_euclid(12) == 2 {
                        self.block_walkway(b, yy0 + a, hei - 2);
                        if b > 3 {
                            self.block_other(b, yy0 + a, hei - 1, RAIL, 0);
                        }
                    } else if b.rem_euclid(12) == 3 {
                        self.block_other(b, yy0 - 1, hei - 2, POWEREDRAIL, 4);
                        self.block_other(b, yy0 - 2, hei - 2, TORCHOFF, 5);
                        self.block_walkway(b, yy0 - 2, hei - 3);
                        self.block_other(b, yy0 - 2, hei - 4, TORCHON, 5);
                    } else if b.rem_euclid(12) == 1 {
                        self.block_other(b, yy0 + a, hei - 2, RAIL, 5);
                    } else {
                        self.block_other(b, yy0 + a, hei - 2, RAIL, 0);
                    }
                }
            }
            a += 1;
        }

        if minecart_on && self.o.chest {
            self.chest = (2, yy0 - 1, hei - 1);
            self.block_other(2, yy0 - 1, hei - 1, CHEST, 4);
        }

        // 控制块
        let yy = yy0;
        self.block_circuit(1, yy, hei - 2);
        self.block_circuit(1, yy + 1, hei - 2);
        self.block_other(1, yy, hei - 1, WOOL, 11);
        self.block_other(1, yy + 1, hei - 1, WOOL, 11);
        self.block_other(2, yy, hei - 1, BUTTON_FLOOR, 3);
        self.block_other(2, yy + 1, hei - 1, BUTTON_WALL, 3);
        if self.sch_loop {
            self.block_other(2, yy, hei - 2, BUTTON_FLOOR, 3);
            self.block_other(2, yy + 1, hei - 2, LEVER, 3 + 8);
            self.block_other(1, yy + 1, hei - 3, REDSTONE_WIRE, 15);
            self.block_circuit(0, yy + 1, hei - 3);
            self.block_other(0, yy, hei - 3, TORCHOFF, 1);
        }
        self.sign = (2, yy, hei - 1);

        // 背面
        self.block_circuit(0, yy + 1, hei - 2);
        self.block_other(0, yy + 1, hei - 1, REDSTONE_WIRE, 0);
        self.block_circuit(0, yy, hei - 2);
        self.block_other(0, yy, hei - 1, REDSTONE_WIRE, 0);
        self.block_circuit(0, yy - 1, hei - 2);
        self.block_other(0, yy - 1, hei - 1, REDSTONE_WIRE, 0);
        self.block_other(0, yy - 2, hei - 2, TORCHON, 1);
        self.block_circuit(0, yy - 2, hei - 4);
        self.block_other(0, yy - 2, hei - 3, REDSTONE_WIRE, 15);
        self.block_other(0, yy - 3, hei - 4, TORCHOFF, 1);

        // 竖向传输
        let mut d: i32 = 0;
        for a in (0..layers).rev() {
            let a = a as i32;
            if a > 0 {
                self.block_other(0, yy - 1 - d, a * 3 + 1, TORCHON, 2);
                self.block_circuit(0, yy - 1, a * 3 - 1 + d);
                self.block_other(0, yy - 1, a * 3 + d, REDSTONE_WIRE, 15);
                self.block_other(0, yy - 2, a * 3 - 1 + d, TORCHOFF, 1);
            }
            let mut p = 0i32;
            let mut c = a * 2;
            let mut b = yy - 2 - d;
            while b >= 0 {
                self.block_circuit(0, b, a * 3 + 1);
                self.block_other(0, b, a * 3 + 2, REDSTONE_WIRE, 0);
                p += 1;
                if c > 0 && b < yy - 3 - d {
                    self.block_repeater(0, b, a * 3 + 2, c.min(4) as u8 - 1, 1, false);
                    c -= 4;
                    p = 0;
                }
                if p == 16 || b == 1 {
                    self.block_repeater(0, b, a * 3 + 2, 0, 1, false);
                    p = 0;
                }
                b -= 1;
            }
            for b in 0..4 {
                self.block_other(b, 0, a * 3 + 2, REDSTONE_WIRE, 0);
                self.block_circuit(b, 0, a * 3 + 1);
            }
            d = 1 - d;
        }

        // 音符盒排布
        let mut xx = 3i32;
        let mut yy = 0i32;
        let mut rep = 0u32;
        let mut dir = 1i32;
        let mut circ = 1i32;
        let mut lturnx = -1i32;
        let mut lturny = -1i32;
        let mut lturndel = 0i32;
        for a in 0..=self.song.enda {
            let mut nblocks = 0i32;
            rep += rep_step(self.tempo_mode);
            let slots = self.tick_slots(a);
            if !slots.is_empty() {
                nblocks = slots.len() as i32;
            }
            if nblocks > 0 || rep == 4 {
                let mut n = nblocks - 1;
                self.place_row(xx, yy, dir, rep, &slots, &mut n);
                rep = 0;
                yy += 2 * dir;
                let mut turn = false;
                if layout == 0 {
                    if circ == 1 {
                        if dir == 1 {
                            if xx == len - 2 {
                                if yy >= self.cyy {
                                    circ = -1;
                                    turn = false;
                                    yy += 1;
                                    for b in 0..layers {
                                        for cc in 0..2 {
                                            self.block_circuit(xx, yy + cc, (b * 3 + 1) as i32);
                                            self.block_other(
                                                xx,
                                                yy + cc,
                                                (b * 3 + 2) as i32,
                                                REDSTONE_WIRE,
                                                0,
                                            );
                                        }
                                    }
                                    yy += 1;
                                }
                            } else {
                                turn = yy >= self.cyy - 2;
                            }
                        }
                        if dir == -1 {
                            turn = yy <= 2;
                        }
                    } else {
                        if dir == 1 {
                            turn = yy >= wid - 3;
                        }
                        if dir == -1 {
                            turn = yy <= self.cyy + 5;
                        }
                    }
                } else {
                    if dir == 1 {
                        turn = yy >= wid - 3;
                    }
                    if dir == -1 {
                        turn = yy <= 2;
                    }
                }
                if a == self.song.enda {
                    turn = false;
                }
                if turn {
                    for b in 0..layers {
                        self.block_other(xx, yy, (b * 3 + 1) as i32, REDSTONE_WIRE, 0);
                        if b > 0 && b < layers - 1 {
                            self.block_circuit(xx, yy, (b * 4) as i32);
                        }
                        self.block_other(xx, yy + dir, (b * 3 + 2) as i32, REDSTONE_WIRE, 0);
                        self.block_circuit(xx, yy + dir, (b * 3 + 1) as i32);
                        for cc in 0..4 {
                            self.block_other(
                                xx + cc * circ,
                                yy + dir * 2,
                                (b * 3 + 2) as i32,
                                REDSTONE_WIRE,
                                0,
                            );
                            self.block_circuit(
                                xx + cc * circ,
                                yy + dir * 2,
                                (b * 3 + 1) as i32,
                            );
                        }
                    }
                    yy += dir * 2;
                    dir = -dir;
                    if circ == -1 && dir == 1 {
                        lturnx = xx;
                        lturny = yy - 1;
                        lturndel = self.song.enda as i32 - a as i32 - 8;
                    }
                    xx += 3 * circ;
                    if xx <= 0 {
                        break;
                    }
                }
            }
        }

        // 环形布局左侧矿车轨道 + 掉头
        if layout == 0 && minecart_on {
            let cyy = self.cyy;
            for b in 2..len {
                if b == 2 {
                    self.block_walkway(b, cyy + 3, hei - 2);
                } else if b < lturnx {
                    if b.rem_euclid(12) == 6 {
                        self.block_other(b, cyy + 3, hei - 2, POWEREDRAIL, 8);
                        self.block_other(b, cyy + 2, hei - 2, TORCHON, 5);
                    } else {
                        self.block_other(b, cyy + 3, hei - 2, RAIL, 0);
                    }
                } else if b.rem_euclid(12) == 6 {
                    self.block_other(b, cyy + 3, hei - 2, POWEREDRAIL, 5);
                    self.block_other(b, cyy + 4, hei - 2, TORCHOFF, 5);
                    self.block_walkway(b, cyy + 4, hei - 3);
                    self.block_other(b, cyy + 4, hei - 4, TORCHON, 5);
                } else if b.rem_euclid(12) == 7 {
                    self.block_other(b, cyy + 3, hei - 1, RAIL, 0);
                    self.block_walkway(b, cyy + 3, hei - 2);
                } else if b.rem_euclid(12) == 8 {
                    self.block_other(b, cyy + 3, hei - 2, RAIL, 4);
                } else {
                    self.block_other(b, cyy + 3, hei - 2, RAIL, 0);
                }
            }
            self.block_other(len - 1, cyy - 1, hei - 2, RAIL, 8);
            self.block_other(len - 1, cyy, hei - 2, RAIL, 1);
            self.block_other(len - 1, cyy + 1, hei - 2, RAIL, 1);
            self.block_other(len - 1, cyy + 2, hei - 2, RAIL, 1);
            self.block_other(len - 1, cyy + 3, hei - 2, RAIL, 9);
        }

        if self.sch_loop && lturnx > -1 {
            let cyy = self.cyy;
            let xx = lturnx;
            let yy = lturny;
            let zz = (layers as i32 - 1) * 3;
            self.block_other(xx, yy, zz + 1, TORCHON, 1);
            let mut a = 15i32;
            let mut b = xx - 1;
            while b > 3 {
                if zz > 0 {
                    self.block_circuit(b, yy, zz);
                }
                if lturndel > 0 || a == 0 {
                    let delay = lturndel.clamp(1, 4) as u8 - 1;
                    self.block_repeater(b, yy, zz + 1, delay, 0, true);
                    lturndel -= 4;
                    a = 15;
                } else {
                    self.block_other(b, yy, zz + 1, REDSTONE_WIRE, a as u8);
                    self.block_circuit(b, yy, zz + 2);
                    a -= 1;
                }
                b -= 1;
            }
            self.block_other(2, cyy + 1, hei - 4, TORCHON, 5);
            self.block_circuit(2, cyy + 1, hei - 5);
            self.block_other(2, cyy + 1, hei - 6, TORCHOFF, 4);
            self.block_circuit(3, cyy + 1, hei - 6);
        }
    }

    fn build_classic(&mut self) {
        let len = self.size.len as i32;
        let wid = self.size.wid as i32;
        let hei = self.size.hei as i32;
        let layers = self.size.layers;

        // 地板（玻璃区被原版重复写了两次，等价于一次）
        for a in 0..len {
            for b in 0..wid - 1 {
                if a > 0 && b > 0 && a < len - 1 && b < wid - 2 && self.o.glass {
                    self.block_other(a, b, hei - 3, GLASS, 0);
                } else {
                    self.block_walkway(a, b, hei - 3);
                }
            }
        }
        // 控制块
        self.block_walkway(2, wid - 2, hei - 2);
        self.block_walkway(3, wid - 2, hei - 2);
        self.block_other(2, wid - 2, hei - 1, WOOL, 11);
        self.block_other(3, wid - 2, hei - 1, WOOL, 11);
        self.block_other(2, wid - 3, hei - 1, BUTTON_FLOOR, 5);
        self.block_other(3, wid - 3, hei - 1, BUTTON_WALL, 1);
        // 背面
        self.block_other(3, wid - 1, hei - 1, REDSTONE_WIRE, 0);
        self.block_circuit(3, wid - 1, hei - 2);
        self.block_other(2, wid - 1, hei - 2, REDSTONE_WIRE, 0);
        self.block_circuit(2, wid - 1, hei - 3);
        self.block_other(1, wid - 1, hei - 3, TORCHON, 4);

        // 各层进线
        let mut c = 1i32;
        for a in (0..layers).rev() {
            let a = a as i32;
            let mut d = a * 2;
            let mut b = wid - 2 - c;
            while b > wid - 8 {
                self.block_circuit(1, b, a * 3 + 1);
                if d > 0 && b < wid - 2 - c {
                    self.block_repeater(1, b, a * 3 + 2, d.min(4) as u8 - 1, 1, false);
                    d -= 4;
                } else {
                    self.block_other(1, b, a * 3 + 2, REDSTONE_WIRE, 0);
                }
                b -= 1;
            }
            if a > 0 {
                self.block_other(1, wid - 1 - c, a * 3 + 1, TORCHON, 2);
            }
            self.block_circuit(1, wid - 1, a * 3 + 3 - c);
            self.block_other(1, wid - 1, a * 3 + 4 - c, REDSTONE_WIRE, 15);
            self.block_other(1, wid - 2, a * 3 + 3 - c, TORCHOFF, 1);
            c = 1 - c;
        }

        // 梯子
        for z in 1..hei - 2 {
            self.block_walkway(0, wid - 2, z);
            self.block_other(0, wid - 1, z, LADDER, 4);
        }
        self.sign = (2, wid - 3, hei - 1);

        let mut xx = 1i32;
        let mut yy = wid - 7;
        let mut rep = 0u32;
        let mut dir = -1i32;
        for a in 0..=self.song.enda {
            let mut nblocks = 0i32;
            rep += rep_step(self.tempo_mode);
            let slots = self.tick_slots(a);
            if !slots.is_empty() {
                nblocks = slots.len() as i32;
            }
            if nblocks > 0 || rep == 4 {
                let mut n = nblocks - 1;
                self.place_row(xx, yy, dir, rep, &slots, &mut n);
                rep = 0;
                yy += 2 * dir;
                let mut turn = false;
                if dir == 1 {
                    turn = yy >= wid - 4;
                }
                if dir == -1 {
                    turn = yy <= 2;
                }
                if a == self.song.enda {
                    turn = false;
                }
                if turn {
                    for b in 0..layers {
                        self.block_other(xx, yy, (b * 3 + 1) as i32, REDSTONE_WIRE, 0);
                        if b > 0 {
                            self.block_circuit(xx, yy, (b * 4) as i32);
                        }
                        self.block_other(xx, yy + dir, (b * 3 + 2) as i32, REDSTONE_WIRE, 0);
                        self.block_circuit(xx, yy + dir, (b * 3 + 1) as i32);
                        for cc in 0..4 {
                            self.block_other(
                                xx + cc,
                                yy + dir * 2,
                                (b * 3 + 2) as i32,
                                REDSTONE_WIRE,
                                0,
                            );
                            self.block_circuit(xx + cc, yy + dir * 2, (b * 3 + 1) as i32);
                        }
                    }
                    yy += dir * 2;
                    dir = -dir;
                    xx += 3;
                }
            }
        }
    }
}

/// 把歌曲 + 选项展开成网格与音符盒表。
pub fn build(song: &Song, opts: &SchematicOptions) -> Built {
    let stats = calculate_stats(song);
    build_with_stats(song, opts, &stats)
}

pub fn build_with_stats(song: &Song, opts: &SchematicOptions, stats: &SchematicStats) -> Built {
    let size = calc_size(stats, opts);
    let sch_loop = opts.loop_option && opts.layout == 0;
    let minecart = opts.minecart && opts.layout < 2;
    let want_chest = opts.chest && minecart;
    let mut b = Builder {
        song,
        o: opts,
        locked: song.locked_layers(),
        size,
        g: Grid::new(size.len, size.wid, size.hei),
        notes: Vec::new(),
        tempo_mode: opts.tempo_mode,
        compress: opts.compress,
        include_locked: opts.include_locked,
        cyy: 0,
        sign: (-1, -1, -1),
        chest: (-1, -1, -1),
        sch_loop,
    };
    if opts.layout < 2 {
        b.build_walkway();
    } else {
        b.build_classic();
    }
    let chest_on = want_chest && b.chest.0 >= 0;
    let Builder {
        g,
        notes,
        cyy,
        sign,
        chest,
        ..
    } = b;
    Built {
        grid: g,
        notes,
        size,
        opts: opts.clone(),
        repeaters: stats.repeaters[opts.include_locked as usize][opts.tempo_mode as usize],
        insnum: 16 + song.custom_instruments.len(),
        sch_loop,
        minecart,
        chest_on,
        cyy,
        signx: sign.0,
        signy: sign.1,
        signz: sign.2,
        chestx: chest.0,
        chesty: chest.1,
        chestz: chest.2,
    }
}

impl Built {
    pub fn info(&self) -> SchematicInfo {
        SchematicInfo {
            width: self.size.wid,
            height: self.size.hei,
            length: self.size.len,
            note_blocks: self.notes.len(),
            repeaters: self.repeaters,
        }
    }

    /// 原版未开启 `structure` 时写的经典 `.schematic` NBT。
    pub fn classic_nbt(&self) -> Vec<u8> {
        let mut n = Nbt::default();
        let wid = self.size.wid as i32;
        let old = self.opts.minecraft_old;
        n.compound("Schematic");
        n.short("Height", self.size.hei as i16);
        n.short("Length", self.size.len as i16);
        n.short("Width", self.size.wid as i16);
        n.list("Entities", self.minecart as i32, 10);
        if self.minecart {
            n.string("id", if old { "Minecart" } else { "minecraft:minecart" });
            n.byte("OnGround", 0);
            n.short("Air", 300);
            n.short("Fire", -1);
            n.int("Type", 0);
            n.float("FallDistance", 0.0);
            n.list("Motion", 3, 6);
            n.item_double(0.0);
            n.item_double(0.0);
            n.item_double(0.0);
            n.list("Pos", 3, 6);
            n.item_double(self.entity_pos_x());
            n.item_double((self.size.hei - 1) as f64);
            n.item_double(3.5);
            n.list("Rotation", 2, 5);
            n.item_float(0.0);
            n.item_float(0.0);
            n.end();
        }
        let tiles = 1 + self.notes.len() + self.sch_loop as usize + self.chest_on as usize;
        n.list("TileEntities", tiles as i32, 10);
        n.string("id", if old { "Sign" } else { "minecraft:sign" });
        n.int("x", wid - 1 - self.signy);
        n.int("y", self.signz);
        n.int("z", self.signx);
        self.write_sign_text(&mut n, old, false);
        n.end();
        if self.sch_loop {
            n.string("id", if old { "Sign" } else { "minecraft:sign" });
            n.int("x", wid - 1 - self.signy);
            n.int("y", self.signz - 1);
            n.int("z", self.signx);
            self.write_sign_text(&mut n, old, true);
            n.end();
        }
        for t in &self.notes {
            n.string("id", if old { "Music" } else { "minecraft:noteblock" });
            n.int("x", wid - 1 - t.y);
            n.int("y", t.z);
            n.int("z", t.x);
            n.byte("note", t.note as u8);
            n.end();
        }
        if self.chest_on {
            n.string("id", if old { "Chest" } else { "minecraft:chest" });
            n.int("x", wid - 1 - self.chesty);
            n.int("y", self.chestz);
            n.int("z", self.chestx);
            n.list("Items", 27, 10);
            for a in 0..27u8 {
                n.string("id", if old { "328" } else { "minecraft:minecart" });
                n.short("Damage", 0);
                n.byte("Count", 1);
                n.byte("Slot", a);
                n.end();
            }
            n.end();
        }
        n.string("Materials", "Alpha");
        let (blocks, data) = self.grid.serialize();
        n.byte_array("Blocks", &blocks);
        n.byte_array("Data", &data);
        n.compound("Version");
        if old {
            n.string("Name", "1.8");
            n.byte("Snapshot", 0);
        } else {
            n.int("Id", 922);
            n.string("Name", "1.11.2");
            n.byte("Snapshot", 0);
        }
        n.end();
        n.end();
        n.buf
    }

    fn entity_pos_x(&self) -> f64 {
        // 原版：wid - 1 - (cyy - 1 - 0.5)
        self.size.wid as f64 - 1.0 - (self.cyy as f64 - 1.0 - 0.5)
    }

    fn write_sign_text(&self, n: &mut Nbt, old: bool, loop_sign: bool) {
        let texts: [&str; 4] = if loop_sign {
            if old {
                ["Looping ON", "", "", "Looping OFF"]
            } else {
                [
                    "{\"text\": \"Looping ON\"}",
                    "{\"text\": \"\"}",
                    "{\"text\": \"\"}",
                    "{\"text\": \"Looping OFF\"}",
                ]
            }
        } else if old {
            ["Song generated", "by the", "Minecraft Note", "Block Studio"]
        } else {
            [
                "{\"text\": \"Song generated\"}",
                "{\"text\": \"by the\"}",
                "{\"text\": \"Minecraft Note\"}",
                "{\"text\": \"Block Studio\"}",
            ]
        };
        for (i, t) in texts.iter().enumerate() {
            n.string(&format!("Text{}", i + 1), t);
        }
    }

    /// 原版开启 `structure` 时写的 1.13+ 结构方块 NBT。
    pub fn structure_nbt(&self, song: &Song) -> Vec<u8> {
        let mut n = Nbt::default();
        let insnum = self.insnum;
        let wid = self.size.wid as i32;
        n.compound("");
        n.int("DataVersion", 1519);
        // 原版把 size[0] 硬编码成 32，与真实 len 无关（保持原样）。
        n.list("size", 3, 3);
        n.item_int(32);
        n.item_int(self.size.hei as i32);
        n.item_int(self.size.len.min(32) as i32);

        n.list("palette", 29 + insnum as i32 * 26, 10);
        for blk in [self.opts.walkway, self.opts.circuit, self.opts.ground] {
            n.string("Name", &format!("minecraft:{}", namespaced_id(blk.id, blk.data)));
            n.end();
        }
        for a in 0..insnum {
            let blk = self.opts.ins(a);
            n.string("Name", &format!("minecraft:{}", namespaced_id(blk.id, blk.data)));
            // 原版只给索引 9（木琴）加了 axis 属性，因为默认方块是竖放的骨块。
            if a == 9 {
                n.compound("Properties");
                n.string("axis", "y");
                n.end();
            }
            n.end();
        }
        for a in 0..insnum {
            for b in 0..25 {
                if self.opts.command_block {
                    n.string("Name", "minecraft:command_block");
                    n.compound("Properties");
                    n.string("facing", "up");
                    n.end();
                } else {
                    n.string("Name", "minecraft:note_block");
                    n.compound("Properties");
                    n.string("instrument", if a < 16 { INS_NAMES[a] } else { "harp" });
                    n.string("note", &b.to_string());
                    n.string("powered", "false");
                    n.end();
                }
                n.end();
            }
        }
        // 尾部 26 项，顺序即 state 偏移，必须逐字对应原版。
        prop(&mut n, "minecraft:blue_wool", &[]);
        prop(&mut n, "minecraft:air", &[]);
        prop(&mut n, "minecraft:ladder", &[("facing", "north"), ("waterlogged", "false")]);
        prop(
            &mut n,
            "minecraft:redstone_wire",
            &[
                ("east", "side"),
                ("north", "side"),
                ("power", "0"),
                ("south", "side"),
                ("west", "side"),
            ],
        );
        for a in 1..5 {
            prop(
                &mut n,
                "minecraft:repeater",
                &[
                    ("delay", &a.to_string()),
                    ("facing", "east"),
                    ("locked", "false"),
                    ("powered", "false"),
                ],
            );
        }
        for a in 1..5 {
            prop(
                &mut n,
                "minecraft:repeater",
                &[
                    ("delay", &a.to_string()),
                    ("facing", "west"),
                    ("locked", "false"),
                    ("powered", "false"),
                ],
            );
        }
        prop(
            &mut n,
            "minecraft:repeater",
            &[
                ("delay", "4"),
                ("facing", "south"),
                ("locked", "false"),
                ("powered", "true"),
            ],
        );
        prop(&mut n, "minecraft:redstone_torch", &[("lit", "false")]);
        prop(&mut n, "minecraft:redstone_torch", &[("lit", "true")]);
        prop(
            &mut n,
            "minecraft:stone_button",
            &[("face", "wall"), ("facing", "south"), ("powered", "false")],
        );
        prop(
            &mut n,
            "minecraft:oak_wall_sign",
            &[("facing", "south"), ("waterlogged", "false")],
        );
        prop(
            &mut n,
            "minecraft:powered_rail",
            &[("powered", "false"), ("shape", "north_south")],
        );
        prop(
            &mut n,
            "minecraft:chest",
            &[("facing", "west"), ("type", "single"), ("waterlogged", "false")],
        );
        prop(&mut n, "minecraft:rail", &[("shape", "north_south"), ("waterlogged", "false")]);
        prop(
            &mut n,
            "minecraft:redstone_wall_torch",
            &[("lit", "false"), ("facing", "east")],
        );
        prop(
            &mut n,
            "minecraft:redstone_wall_torch",
            &[("lit", "true"), ("facing", "east")],
        );
        prop(
            &mut n,
            "minecraft:redstone_wall_torch",
            &[("lit", "false"), ("facing", "west")],
        );
        prop(
            &mut n,
            "minecraft:redstone_wall_torch",
            &[("lit", "true"), ("facing", "west")],
        );
        prop(
            &mut n,
            "minecraft:lever",
            &[("powered", "true"), ("facing", "south"), ("face", "wall")],
        );
        prop(
            &mut n,
            "minecraft:redstone_wall_torch",
            &[("lit", "true"), ("facing", "north")],
        );

        n.list("entities", self.minecart as i32, 10);
        if self.minecart {
            n.list("pos", 3, 6);
            n.item_double(self.entity_pos_x());
            n.item_double((self.size.hei - 1) as f64);
            n.item_double(3.5);
            n.list("blockPos", 3, 3);
            // 原版对 wid-1-(cyy-1-0.5)-0.5 截断取整，这里两项都是整数。
            n.item_int(wid - self.cyy - 1);
            n.item_int(self.size.hei as i32 - 1);
            n.item_int(3);
            n.compound("");
            n.byte("OnGround", 0);
            n.short("Air", 300);
            n.short("Fire", -1);
            n.float("FallDistance", 0.0);
            n.list("Motion", 3, 6);
            n.item_double(0.0);
            n.item_double(0.0);
            n.item_double(0.0);
            n.list("Rotation", 2, 5);
            n.item_float(0.0);
            n.item_float(0.0);
            n.list("Pos", 3, 6);
            n.item_double(self.entity_pos_x());
            n.item_double((self.size.hei - 1) as f64);
            n.item_double(3.5);
            n.end();
            n.end();
        }

        n.list("blocks", (self.size.hei * self.size.len * self.size.wid + 1 + self.sch_loop as usize) as i32, 10);
        n.compound("nbt");
        n.string("Color", "black");
        n.string("id", "minecraft:sign");
        self.write_sign_text(&mut n, false, false);
        n.end();
        n.list("pos", 3, 3);
        n.item_int(wid - 1 - self.signy);
        n.item_int(self.signz);
        n.item_int(self.signx);
        n.int("state", (insnum * 26 + 19) as i32);
        n.end();
        if self.sch_loop {
            n.compound("nbt");
            n.string("Color", "black");
            n.string("id", "minecraft:sign");
            self.write_sign_text(&mut n, false, true);
            n.end();
            n.list("pos", 3, 3);
            n.item_int(wid - 1 - self.signy);
            n.item_int(self.signz - 1);
            n.item_int(self.signx);
            n.int("state", (insnum * 26 + 19) as i32);
            n.end();
        }
        for t in &self.notes {
            if self.opts.command_block {
                // 原版的音源名/音高拼接：pitch 用 note+33+pit/100，但加后缀的判断
                // 却用 note+pit/100（不含 +33），照抄。
                let mut soundname = instrument_sound(song, t.ins);
                let soundpitch = dat_pitch(t.note as f64 + 33.0 + t.pit as f64 / 100.0);
                let offset = t.note as f64 + t.pit as f64 / 100.0;
                if offset < 0.0 {
                    soundname += "_-1";
                } else if offset > 24.0 {
                    soundname += "_1";
                }
                n.compound("nbt");
                n.string(
                    "Command",
                    &format!("playsound {soundname} block @a ~ ~ ~ 3 {soundpitch}"),
                );
                n.byte("TrackOutput", 0);
                n.byte("powered", 0);
                n.byte("auto", 0);
                n.byte("conditionMet", 1);
                n.byte("UpdateLastExecution", 1);
                n.end();
            }
            n.list("pos", 3, 3);
            n.item_int(wid - 1 - t.y);
            n.item_int(t.z);
            n.item_int(t.x);
            let state = if self.opts.command_block {
                3 + insnum as i32
            } else {
                3 + insnum as i32 + (t.ins as i32 * 25 + t.note)
            };
            n.int("state", state);
            n.end();
        }
        if self.chest_on {
            n.compound("nbt");
            n.list("Items", 27, 10);
            for a in 0..27u8 {
                n.byte("Count", 1);
                n.string("id", "minecraft:minecart");
                n.byte("Slot", a);
                n.end();
            }
            n.string("id", "minecraft:chest");
            n.end();
            n.list("pos", 3, 3);
            n.item_int(wid - 1 - self.chesty);
            n.item_int(self.chestz);
            n.item_int(self.chestx);
            n.int("state", (insnum * 26 + 21) as i32);
            n.end();
        }
        for c in 0..self.size.hei {
            for a in 0..self.size.len {
                for bb in (0..self.size.wid).rev() {
                    let (id, data) = self.cell(a, bb, c);
                    if id == NOTE_BLOCK || id == CHEST {
                        continue;
                    }
                    n.list("pos", 3, 3);
                    n.item_int(wid - bb as i32 - 1);
                    n.item_int(c as i32);
                    n.item_int(a as i32);
                    n.int("state", self.state_of(id, data));
                    n.end();
                }
            }
        }
        n.end();
        n.buf
    }

    fn cell(&self, a: usize, b: usize, c: usize) -> (u8, u8) {
        self.grid.at(a, b, c)
    }

    /// 原版结构方块分支里那一长串 else-if 状态映射。
    fn state_of(&self, id: u8, data: u8) -> i32 {
        let base = self.insnum * 26;
        if id == AIR {
            return (base + 4) as i32;
        }
        if id == self.opts.walkway.id && data == self.opts.walkway.data {
            return 0;
        }
        if id == self.opts.circuit.id && data == self.opts.circuit.data {
            return 1;
        }
        if id == self.opts.ground.id && data == self.opts.ground.data {
            return 2;
        }
        let off = |k: usize| (base + k) as i32;
        if id == WOOL && data == 11 {
            return off(3);
        }
        if id == LADDER {
            return off(5);
        }
        if id == REDSTONE_WIRE {
            return off(6);
        }
        if id == REPEATER {
            match data {
                3 => return off(7),
                7 => return off(8),
                11 => return off(9),
                15 => return off(10),
                1 => return off(11),
                5 => return off(12),
                9 => return off(13),
                13 => return off(14),
                _ => {}
            }
        }
        if id == REPEATER_POWERED {
            return off(15);
        }
        if id == TORCHOFF && data == 5 {
            return off(16);
        }
        if id == TORCHON && data == 5 {
            return off(17);
        }
        if id == BUTTON_WALL {
            return off(18);
        }
        if id == POWEREDRAIL {
            return off(20);
        }
        if id == RAIL {
            return off(22);
        }
        if id == TORCHOFF && data == 1 {
            return off(23);
        }
        if id == TORCHON && data == 1 {
            return off(24);
        }
        if id == TORCHOFF && data == 2 {
            return off(25);
        }
        if id == TORCHON && data == 2 {
            return off(26);
        }
        if id == LEVER {
            return off(27);
        }
        if id == TORCHOFF && data == 4 {
            // 原版注释写作 "north lit walltorch"，但 75 是不亮的火把。
            return off(28);
        }
        for i in 0..self.insnum {
            let blk = self.opts.ins(i);
            if id == blk.id && data == blk.data {
                return (3 + i) as i32;
            }
        }
        (base + 4) as i32
    }
}

/// 写一个带 Properties 的 palette 项（原版的 `TAG_String("Name", ..)` + 可选 Properties + `TAG_End()`）。
fn prop(n: &mut Nbt, name: &str, props: &[(&str, &str)]) {
    n.string("Name", name);
    if !props.is_empty() {
        n.compound("Properties");
        for (k, v) in props {
            n.string(k, v);
        }
        n.end();
    }
    n.end();
}

// ---------------------------------------------------------------------- 导出

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchematicInfo {
    pub width: usize,
    pub height: usize,
    pub length: usize,
    pub note_blocks: usize,
    pub repeaters: usize,
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).map_err(|e| format!("gzip 压缩失败: {e}"))?;
    enc.finish().map_err(|e| format!("gzip 结束失败: {e}"))
}

/// 原版菜单打开窗口前的闸门（`calculate_size()` 的返回值）：
/// 可导出的音符数 = 音符总数 − 2 八度范围外的数量，为 0 时不弹窗口。
pub fn open_guard(stats: &SchematicStats) -> Result<(), String> {
    if stats.song_blocks.saturating_sub(stats.block_outside) == 0 {
        return Err("没有方块可以导出！".into());
    }
    Ok(())
}

/// 原版「导出」按钮的两道闸门，各自对应一个不同标题的 `message()`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportCheck {
    Ok,
    /// `sch_exp_totalblocks[include_locked] <= 0`。
    NoBlocks,
    /// 超出 2000x2000x256。
    TooBig,
}

impl ExportCheck {
    /// 原版 `message(正文, 标题)`，闸门通过时没有对话框。
    pub fn dialog(self) -> Option<(&'static str, &'static str)> {
        match self {
            ExportCheck::Ok => None,
            ExportCheck::NoBlocks => Some(("导出 Schematic", "没有方块可以导出！")),
            ExportCheck::TooBig => Some((
                "错误",
                "这个 Schematic 太大了。大小限制为 2000x2000x256。\n可以更改“每行中继器个数”来减小大小。",
            )),
        }
    }
}

/// 尺寸与方块数检查；`stats` 由 `calculate_stats()` 得到，尺寸随选项实时算。
pub fn export_check(stats: &SchematicStats, opts: &SchematicOptions) -> ExportCheck {
    if stats.totalblocks[opts.include_locked as usize] == 0 {
        return ExportCheck::NoBlocks;
    }
    let size = calc_size(stats, opts);
    if size.len >= MAX_LEN || size.wid >= MAX_WID || size.hei >= MAX_HEI {
        return ExportCheck::TooBig;
    }
    ExportCheck::Ok
}

pub fn export_guard(stats: &SchematicStats, opts: &SchematicOptions) -> Result<(), String> {
    match export_check(stats, opts).dialog() {
        Some((_, msg)) => Err(msg.into()),
        None => Ok(()),
    }
}

pub fn export_schematic(
    song: &Song,
    path: &Path,
    opts: &SchematicOptions,
) -> Result<SchematicInfo, String> {
    let stats = calculate_stats(song);
    export_guard(&stats, opts)?;
    let built = build_with_stats(song, opts, &stats);
    let nbt = if opts.structure {
        built.structure_nbt(song)
    } else {
        built.classic_nbt()
    };
    let packed = gzip(&nbt)?;
    std::fs::write(path, packed).map_err(|e| format!("写入 schematic 失败: {e}"))?;
    Ok(built.info())
}

/// 生成未压缩的 NBT 字节，供测试与检查用。
pub fn build_nbt(song: &Song, opts: &SchematicOptions) -> Result<Vec<u8>, String> {
    let stats = calculate_stats(song);
    export_guard(&stats, opts)?;
    let built = build_with_stats(song, opts, &stats);
    Ok(if opts.structure { built.structure_nbt(song) } else { built.classic_nbt() })
}

/// 解压并读出文件里真实存在的尺寸/音符统计，供界面与测试核对。
///
/// 两种格式都读：经典 `.schematic` 用 Width/Height/Length + TileEntities，
/// 结构方块文件用 size + palette/blocks 联表。
/// 结构文件里 `size[0]` 是原版硬编码的 32（不是真实长度），这里照文件读。
pub fn inspect_schematic(path: &Path) -> Result<SchematicInfo, String> {
    let packed = std::fs::read(path).map_err(|e| format!("打开文件失败: {e}"))?;
    Ok(inspect_bytes(&packed)?)
}

/// 输入是 gzip 后的文件字节。
pub fn inspect_bytes(d: &[u8]) -> Result<SchematicInfo, String> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut raw = Vec::new();
    GzDecoder::new(d)
        .read_to_end(&mut raw)
        .map_err(|e| format!("gzip 解压失败: {e}"))?;
    let (_, root) = crate::nbt::parse_named(&raw)?;
    read_info(&root)
}

fn read_info(root: &crate::nbt::Value) -> Result<SchematicInfo, String> {
    use crate::nbt::Value;
    let mut info = SchematicInfo::default();
    if let Some(v) = root.get("Width").and_then(Value::as_i16) {
        info.width = v.max(0) as usize;
        info.height = root.get("Height").and_then(Value::as_i16).unwrap_or(0).max(0) as usize;
        info.length = root.get("Length").and_then(Value::as_i16).unwrap_or(0).max(0) as usize;
        // 经典格式：note block 是 TileEntities 里 id 为 Music/minecraft:noteblock 的条目。
        for te in root.get("TileEntities").and_then(Value::as_list).unwrap_or((0, &[])).1 {
            match te.get("id").and_then(Value::as_str) {
                Some("Music") | Some("minecraft:noteblock") => info.note_blocks += 1,
                _ => {}
            }
        }
        // 中继器只能从 Blocks 字节数组里数（id 93/94）。
        if let Some(blocks) = root.get("Blocks").and_then(Value::as_bytes) {
            info.repeaters = blocks.iter().filter(|&&b| b == REPEATER || b == REPEATER_POWERED).count();
        }
        return Ok(info);
    }
    if let Some((_, size)) = root.get("size").and_then(Value::as_list) {
        let get = |i: usize| size.get(i).and_then(Value::as_i32).unwrap_or(0).max(0) as usize;
        info.width = get(0);
        info.height = get(1);
        info.length = get(2);
        let palette = structure_palette(root);
        for b in root.get("blocks").and_then(Value::as_list).unwrap_or((0, &[])).1 {
            let Some(state) = b.get("state").and_then(Value::as_i32) else { continue };
            let Some(entry) = palette.get(state.max(0) as usize).copied() else { continue };
            let name = entry.get("Name").and_then(Value::as_str).unwrap_or("");
            if name.ends_with("note_block") || name.ends_with("command_block") {
                info.note_blocks += 1;
            } else if name.ends_with("repeater") {
                info.repeaters += 1;
            }
        }
        return Ok(info);
    }
    Err("文件里既没有 Width/Height/Length，也没有 size，不是 Schematic 或结构文件".into())
}

/// 结构文件的 palette：原版写的是 compound 列表（`TAG_List("palette", n, 10)`）。
fn structure_palette(root: &crate::nbt::Value) -> Vec<&crate::nbt::Value> {
    use crate::nbt::Value;
    match root.get("palette") {
        Some(pal) => match pal.as_list() {
            Some((_, items)) => items.iter().collect(),
            None => match pal.get("default").and_then(Value::as_list) {
                Some((_, items)) => items.iter().collect(),
                None => Vec::new(),
            },
        },
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::Value;
    use nbs_core::model::{CustomInstrument, Layer, LockMode, Note, Song};

    /// 每个音符的默认力度/声像/微调与原版一致（100/100/0），所以测试只需要 tick/层/音色/音高。
    fn song_with(notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::new();
        let max_layer = notes.iter().map(|n| n.1).max().unwrap_or(0) as usize;
        s.layers = vec![Layer::default(); max_layer + 1];
        for (t, l, ins, key) in notes {
            s.set(*t, *l, Note::new(*ins, *key));
        }
        s.recalculate();
        s
    }

    fn keys_of(v: &Value) -> Vec<&str> {
        v.fields()
            .unwrap_or_default()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect()
    }

    fn items(v: &Value) -> &[Value] {
        v.as_list().map(|(_, i)| i).unwrap_or(&[])
    }

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
    fn default_options_match_reset_schematic_export() {
        let o = SchematicOptions::default();
        assert_eq!(o.layout, 1);
        assert_eq!(o.notesperrow, 20);
        assert_eq!(o.compress, true);
        assert_eq!(o.include_locked, false);
        assert_eq!(o.minecart, false);
        assert_eq!(o.chest, true);
        assert_eq!(o.loop_option, true);
        assert_eq!(o.glass, true);
        assert_eq!(o.structure, false);
        assert_eq!(o.minecraft_old, false);
        assert_eq!(o.command_block, false);
        assert_eq!(o.tempo_mode, 0);
        assert_eq!(o.walkway, McBlock::new(1, 0));
        assert_eq!(o.circuit, McBlock::new(1, 0));
        assert_eq!(o.ground, McBlock::new(1, 0));
        assert_eq!(&o.ins_block[..16], &INSTRUMENT_BLOCKS);
        assert!(o.ins_block[16..].iter().all(|&b| b == STONE));
        assert!(o.ins_data.iter().all(|&d| d == 0));
        assert_eq!(o.ins_block.len(), INS_COUNT);
    }

    #[test]
    fn reset_keeps_the_three_options_absent_from_the_original_table() {
        // `reset_schematic_export` 里没有 sch_exp_tempo / structure / command_block，
        // 「使用默认值」不能动这三项。
        let mut o = SchematicOptions {
            layout: 0,
            notesperrow: 100,
            compress: false,
            include_locked: true,
            minecart: true,
            chest: false,
            loop_option: false,
            glass: false,
            minecraft_old: true,
            structure: true,
            command_block: true,
            tempo_mode: 2,
            walkway: McBlock::new(2, 3),
            circuit: McBlock::new(4, 5),
            ground: McBlock::new(6, 7),
            ..Default::default()
        };
        o.ins_block[0] = 99;
        o.ins_data[3] = 12;
        o.reset();
        let d = SchematicOptions::default();
        assert_eq!(o.layout, d.layout);
        assert_eq!(o.notesperrow, d.notesperrow);
        assert_eq!(o.minecraft_old, false);
        assert_eq!(o.walkway, d.walkway);
        assert_eq!(o.circuit, d.circuit);
        assert_eq!(o.ground, d.ground);
        assert_eq!(&o.ins_block[..16], &INSTRUMENT_BLOCKS);
        assert!(o.ins_data.iter().all(|&v| v == 0));
        assert_eq!(o.tempo_mode, 2, "sch_exp_tempo 不在默认值表里");
        assert_eq!(o.structure, true, "structure 是全局变量，不被重置");
        assert_eq!(o.command_block, true, "command_block 同样不在表里");
    }

    #[test]
    fn notesperrow_is_clamped_to_the_original_range() {
        let mut o = SchematicOptions::default();
        o.set_notesperrow(1);
        assert_eq!(o.notesperrow, 5);
        o.set_notesperrow(500);
        assert_eq!(o.notesperrow, 100);
        o.set_notesperrow(37);
        assert_eq!(o.notesperrow, 37);
    }

    #[test]
    fn stats_count_blocks_and_repeaters_like_calculate_size() {
        // 4 个连续 tick、每 tick 一个音：maxheight[压缩]=1、maxheight[不压缩]=0（collast 恒为 0）。
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 0, 45), (2, 0, 0, 45), (3, 0, 0, 45)]);
        let st = calculate_stats(&s);
        assert_eq!(st.totalblocks, [4, 4]);
        assert_eq!(st.maxheight, [0, 1]);
        assert_eq!(st.repeaters[0][0], 4);
        assert_eq!(st.repeaters[0][1], 4);
        assert_eq!(st.repeaters[0][2], 4);
        assert_eq!(st.scp_enda, [3, 3]);
        assert_eq!(st.tempo_mode, 0);
    }

    #[test]
    fn empty_ticks_still_need_repeater_padding() {
        // tick0 有音、tick1..3 空、tick4 有音。
        // tempo 档 2（2.5 t/s）：每个 tick 都算一格 → 5 格。
        // 档 1（5 t/s）：2 个有音 + 空档每 2 格补 1 → 2 + 2 = 4。
        // 档 0（10 t/s）：2 个有音 + 空档每 4 格补 1 → 2 + 1 = 3。
        let s = song_with(&[(0, 0, 0, 45), (4, 0, 0, 45)]);
        let st = calculate_stats(&s);
        assert_eq!(st.scp_enda, [4, 4]);
        assert_eq!(st.repeaters[0][2], 5);
        assert_eq!(st.repeaters[0][1], 3);
        assert_eq!(st.repeaters[0][0], 2);
    }

    #[test]
    fn locked_layers_only_change_totalblocks_zero() {
        let mut s = song_with(&[(0, 0, 0, 45), (0, 1, 0, 45)]);
        s.layers[1].lock = LockMode::Locked;
        let st = calculate_stats(&s);
        assert_eq!(st.totalblocks, [1, 2]);
        // include_locked 时中继器按 [1] 计，两档都仍是 1 个 tick。
        assert_eq!(st.repeaters[0][0], 1);
        assert_eq!(st.repeaters[1][0], 1);
    }

    #[test]
    fn sizes_follow_the_three_formulas() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 0, 45), (2, 0, 0, 45)]);
        let st = calculate_stats(&s);
        let mut o = SchematicOptions::default();
        o.notesperrow = 5;
        // 普通过道（layout 1）
        assert_eq!(schematic_width(&o), 13);
        assert_eq!(schematic_length(&st, &o), 2 + 1 * 3);
        assert_eq!(schematic_height(&st, &o), 4 + 3 * 1);
        // 环形过道（layout 0）：宽 +4，行数为奇数时 len 再 +3
        o.layout = 0;
        assert_eq!(schematic_width(&o), 17);
        assert_eq!(schematic_length(&st, &o), 2 + 3 + 3);
        // Classic（layout 2）
        o.layout = 2;
        assert_eq!(schematic_width(&o), 13);
        // ceil((3 + 2) / 5) * 3 + 1 = 4，但 classic 有下限 max(6, …)。
        assert_eq!(schematic_length(&st, &o), 6);
        // 高度跟着 compress 走：关掉后用的是最大层号。
        // 这首歌只有第 0 层有音，collast 恒为 0 -> layers=0、hei=4（原版行为，
        // 此时 place_row 一层都不放，音符会整体丢失）。
        o.compress = false;
        assert_eq!(schematic_height(&st, &o), 4);
    }

    #[test]
    fn tempo_mode_maps_the_original_thresholds() {
        assert_eq!(tempo_to_mode(10.0), 0);
        assert_eq!(tempo_to_mode(7.5), 0);
        assert_eq!(tempo_to_mode(7.49), 1);
        assert_eq!(tempo_to_mode(3.75), 1);
        assert_eq!(tempo_to_mode(3.74), 2);
    }

    #[test]
    fn compat_questions_use_the_original_texts() {
        let mut s = song_with(&[(0, 0, 0, 45)]);
        s.meta.tempo = 7.0;
        s.set(1, 0, Note::new(20, 45)); // 自定义音色
        s.set(2, 0, {
            let mut n = Note::new(0, 20); // 2 八度范围外
            n.pitch = 50; // 微分音
            n
        });
        s.recalculate();
        let st = calculate_stats(&s);
        assert_eq!(
            st.compat_questions(),
            vec![
                "一些方块在 Minecraft 的2八度范围外。你想无视这些方块导出 Schematic 吗？",
                "一些方块带有自定义音色。继续导出吗？",
                "一些方块带有音符盒不支持的微分音。继续导出吗？",
            ]
        );
        assert_eq!(
            st.tempo_notice(),
            Some("歌曲的速度与 Minecraft 不兼容。所以，在游戏内的播放会和软件内的有差别。")
        );
        assert_eq!(st.block_outside, 1);
        assert_eq!(st.block_custom, 1);
        assert_eq!(st.block_pitched, 1);
        assert_eq!(st.song_blocks, 3);
        // 10/5/2.5 之外的速度才提示。
        for t in [10.0f32, 5.0, 2.5] {
            let mut s2 = song_with(&[(0, 0, 0, 45)]);
            s2.meta.tempo = t;
            assert_eq!(calculate_stats(&s2).tempo_notice(), None, "{t}");
        }
    }

    #[test]
    fn instrument_menus_follow_the_original_shortlists() {
        let ids = |m: Vec<BlockMenuSection>| -> String {
            m.iter()
                .map(|s| match s.title {
                    Some(t) => format!("{t}:{}", s.items.len()),
                    None => format!("{},{}", s.items[0].id, s.items[0].data),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(ids(instrument_block_menu(0)), "0,0 2,0 3,0 18,0 19,0");
        assert_eq!(ids(instrument_block_menu(1)), "5,0 17,0 25,0 47,0");
        assert_eq!(
            ids(instrument_block_menu(2)),
            "1,0 4,0 7,0 14,0 15,0 16,0 21,0 22,0 24,0 48,0 49,0 73,0 87,0 89,0"
        );
        assert_eq!(ids(instrument_block_menu(3)), "12,0 13,0 88,0");
        assert_eq!(ids(instrument_block_menu(4)), "20,0");
        assert_eq!(ids(instrument_block_menu(5)), "Wool:16");
        assert_eq!(ids(instrument_block_menu(8)), "174,0");
        assert_eq!(ids(instrument_block_menu(9)), "216,0");
        // 10 号起是全部方块：三种带色方块收成子菜单，其余每项都有名字。
        let full = instrument_block_menu(10);
        assert_eq!(ids(full.clone()).split(' ').filter(|s| s.contains(':')).count(), 3);
        for s in &full {
            if s.title.is_none() {
                assert!(!block_display_name(s.items[0].id, 0).is_empty());
            }
        }
        let titles: Vec<_> = full.iter().filter_map(|s| s.title).collect();
        assert_eq!(titles, vec!["Wool", "Stained glass", "Colored Terracotta"]);
    }

    #[test]
    fn misc_menus_only_drop_glass_and_note_block_for_circuitry() {
        let flat = |m: u8| -> Vec<u8> {
            misc_block_menu(m)
                .iter()
                .filter(|s| s.title.is_none())
                .map(|s| s.items[0].id)
                .collect()
        };
        let walkway = flat(0);
        let circuit = flat(1);
        assert!(walkway.contains(&20) && walkway.contains(&25));
        assert!(!circuit.contains(&20) && !circuit.contains(&25));
        assert_eq!(walkway.len(), circuit.len() + 2);
        assert_eq!(flat(2), walkway);
        // 杂项页的拼写是大写 G。
        let glass = misc_block_menu(0)
            .into_iter()
            .find(|s| s.items[0].id == 95)
            .unwrap();
        assert_eq!(glass.title, Some("Stained Glass"));
        assert_eq!(glass.items.len(), 16);
    }

    #[test]
    fn classic_root_tags_follow_the_original_order() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50)]);
        let nbt = build_nbt(&s, &SchematicOptions::default()).unwrap();
        let (name, root) = crate::nbt::parse_named(&nbt).unwrap();
        assert_eq!(name, "Schematic");
        assert_eq!(
            keys_of(&root),
            [
                "Height",
                "Length",
                "Width",
                "Entities",
                "TileEntities",
                "Materials",
                "Blocks",
                "Data",
                "Version"
            ]
        );
        assert_eq!(root.get("Height").and_then(Value::as_i16), Some(7));
        assert_eq!(root.get("Length").and_then(Value::as_i16), Some(5));
        assert_eq!(root.get("Width").and_then(Value::as_i16), Some(43));
        assert_eq!(root.get("Materials").and_then(Value::as_str), Some("Alpha"));
        // 没开矿车时 Entities 是空表，但元素类型仍写 10。
        let (ty, ents) = root.get("Entities").unwrap().as_list().unwrap();
        assert_eq!((ty, ents.len()), (10, 0));
        // Blocks/Data 都是 len*wid*hei 个字节。
        let blocks = root.get("Blocks").and_then(Value::as_bytes).unwrap();
        assert_eq!(blocks.len(), 7 * 5 * 43);
        assert_eq!(root.get("Data").and_then(Value::as_bytes).unwrap().len(), blocks.len());
        let version = root.get("Version").unwrap();
        assert_eq!(keys_of(version), ["Id", "Name", "Snapshot"]);
        assert_eq!(version.get("Id").and_then(Value::as_i32), Some(922));
        assert_eq!(version.get("Name").and_then(Value::as_str), Some("1.11.2"));
    }

    #[test]
    fn classic_tile_entities_carry_sign_then_notes() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50)]);
        let nbt = build_nbt(&s, &SchematicOptions::default()).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let (ty, tes) = root.get("TileEntities").unwrap().as_list().unwrap();
        assert_eq!(ty, 10);
        assert_eq!(tes.len(), 3); // 1 个签名 + 2 个音符盒
        assert_eq!(keys_of(&tes[0]), ["id", "x", "y", "z", "Text1", "Text2", "Text3", "Text4"]);
        assert_eq!(tes[0].get("id").and_then(Value::as_str), Some("minecraft:sign"));
        assert_eq!(
            tes[0].get("Text4").and_then(Value::as_str),
            Some("{\"text\": \"Block Studio\"}")
        );
        assert_eq!(keys_of(&tes[1]), ["id", "x", "y", "z", "note"]);
        assert_eq!(tes[1].get("id").and_then(Value::as_str), Some("minecraft:noteblock"));
        assert_eq!(tes[1].get("note").and_then(Value::as_i8), Some(45 - 33));
        assert_eq!(tes[2].get("note").and_then(Value::as_i8), Some(50 - 33));
        // 关掉循环、非环形过道 -> 只有一个签名；没开矿车 -> 没有箱子。
    }

    #[test]
    fn classic_minecraft_old_uses_legacy_ids_and_plain_text() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.minecraft_old = true;
        let nbt = build_nbt(&s, &o).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let tes = items(root.get("TileEntities").unwrap());
        assert_eq!(tes[0].get("id").and_then(Value::as_str), Some("Sign"));
        assert_eq!(tes[0].get("Text1").and_then(Value::as_str), Some("Song generated"));
        assert_eq!(tes[1].get("id").and_then(Value::as_str), Some("Music"));
        let version = root.get("Version").unwrap();
        assert_eq!(keys_of(version), ["Name", "Snapshot"]);
        assert_eq!(version.get("Name").and_then(Value::as_str), Some("1.8"));
    }

    #[test]
    fn classic_minecart_entity_has_the_original_fields() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.minecart = true;
        let nbt = build_nbt(&s, &o).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let ents = items(root.get("Entities").unwrap());
        assert_eq!(ents.len(), 1);
        assert_eq!(
            keys_of(&ents[0]),
            [
                "id",
                "OnGround",
                "Air",
                "Fire",
                "Type",
                "FallDistance",
                "Motion",
                "Pos",
                "Rotation"
            ]
        );
        assert_eq!(ents[0].get("id").and_then(Value::as_str), Some("minecraft:minecart"));
        assert_eq!(ents[0].get("Air").and_then(Value::as_i16), Some(300));
        assert_eq!(ents[0].get("Fire").and_then(Value::as_i16), Some(-1));
        assert_eq!(items(ents[0].get("Motion").unwrap()).len(), 3);
        // 矿车打开后箱子才会出现（chest 依赖 minecart），TileEntities 多一项。
        let tes = items(root.get("TileEntities").unwrap());
        assert_eq!(tes.last().unwrap().get("id").and_then(Value::as_str), Some("minecraft:chest"));
        assert_eq!(items(tes.last().unwrap().get("Items").unwrap()).len(), 27);
    }

    #[test]
    fn structure_root_follows_the_original_order_and_palette_size() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        let nbt = build_nbt(&s, &o).unwrap();
        let (name, root) = crate::nbt::parse_named(&nbt).unwrap();
        assert_eq!(name, "");
        assert_eq!(keys_of(&root), ["DataVersion", "size", "palette", "entities", "blocks"]);
        assert_eq!(root.get("DataVersion").and_then(Value::as_i32), Some(1519));
        let size = items(root.get("size").unwrap());
        assert_eq!(size.len(), 3);
        // 原版把 size[0] 硬编码成 32，第三维超过 32 也被截成 32。
        assert_eq!(size[0].as_i32(), Some(32));
        assert_eq!(size[2].as_i32(), Some(5));
        let (ty, pal) = root.get("palette").unwrap().as_list().unwrap();
        assert_eq!(ty, 10);
        assert_eq!(pal.len(), 29 + 16 * 26);
        assert_eq!(
            keys_of(&pal[0]),
            ["Name"]
        );
        assert_eq!(pal[0].get("Name").and_then(Value::as_str), Some("minecraft:stone"));
        assert_eq!(pal[3].get("Name").and_then(Value::as_str), Some("minecraft:dirt"));
        // 第 9 个音色方块（索引 3+9）带 axis 属性，其余不带。
        let axis = pal[3 + 9].get("Properties").unwrap();
        assert_eq!(axis.get("axis").and_then(Value::as_str), Some("y"));
        assert!(pal[3 + 8].get("Properties").is_none());
        // 音符盒调色板从 3+insnum 开始，第一项是 harp/note 0。
        let nb = &pal[3 + 16];
        assert_eq!(nb.get("Name").and_then(Value::as_str), Some("minecraft:note_block"));
        let props = nb.get("Properties").unwrap();
        assert_eq!(
            keys_of(props),
            ["instrument", "note", "powered"]
        );
        assert_eq!(props.get("instrument").and_then(Value::as_str), Some("harp"));
        assert_eq!(props.get("note").and_then(Value::as_str), Some("0"));
        // 尾部 26 项的第一项是蓝色羊毛，最后一项是朝北亮着的红石墙火把。
        let tail = &pal[pal.len() - 26];
        assert_eq!(tail.get("Name").and_then(Value::as_str), Some("minecraft:blue_wool"));
        let last = pal.last().unwrap();
        assert_eq!(last.get("Name").and_then(Value::as_str), Some("minecraft:redstone_wall_torch"));
        assert_eq!(last.get("Properties").unwrap().get("facing").and_then(Value::as_str), Some("north"));
    }

    #[test]
    fn structure_block_entries_skip_note_blocks_and_chests() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        o.minecart = true;
        let built = build(&s, &o);
        let nbt = built.structure_nbt(&s);
        let root = crate::nbt::parse(&nbt).unwrap();
        let blocks = items(root.get("blocks").unwrap());
        // 声明数量 = hei*len*wid + 1 + sch_loop，实际写入应与之相等。
        let size = items(root.get("size").unwrap());
        let s2 = size.len();
        assert_eq!(s2, 3);
        assert_eq!(blocks.len(), built.size.hei * built.size.len * built.size.wid + 1 + built.sch_loop as usize);
        // 第一个条目是签名方块：nbt -> pos -> state。
        assert_eq!(keys_of(&blocks[0]), ["nbt", "pos", "state"]);
        assert_eq!(
            blocks[0].get("nbt").unwrap().get("id").and_then(Value::as_str),
            Some("minecraft:sign")
        );
        // 音符盒条目不带 nbt，state = 3 + insnum + ins*25 + note。
        let note = blocks
            .iter()
            .find(|b| b.get("nbt").is_none() && keys_of(b) == ["pos", "state"])
            .unwrap();
        let _ = note;
        let states: Vec<i32> = blocks
            .iter()
            .filter_map(|b| b.get("state").and_then(Value::as_i32))
            .collect();
        assert!(states.contains(&(3 + 16 + (0 * 25 + (45 - 33)))), "{states:?}");
        // 矿车实体：pos -> blockPos -> 无名 compound。
        let ents = items(root.get("entities").unwrap());
        assert_eq!(ents.len(), 1);
        assert_eq!(keys_of(&ents[0]), ["pos", "blockPos", ""]);
    }

    #[test]
    fn structure_command_block_replaces_note_blocks_with_commands() {
        let mut s = song_with(&[(0, 0, 0, 45)]);
        s.custom_instruments.push(CustomInstrument {
            name: "piano".into(),
            filename: "piano.ogg".into(),
            key: 45,
            press: false,
        });
        let mut o = SchematicOptions::default();
        o.structure = true;
        o.command_block = true;
        let built = build(&s, &o);
        // 一个自定义音色让 insnum 变成 17，palette 也随之增长。
        assert_eq!(built.insnum, 17);
        let nbt = built.structure_nbt(&s);
        let root = crate::nbt::parse(&nbt).unwrap();
        let pal = items(root.get("palette").unwrap());
        assert_eq!(pal.len(), 29 + 17 * 26);
        // 命令方块调色板区（3+17 起）全叫 command_block，朝向向上。
        let cb = &pal[3 + 17];
        assert_eq!(cb.get("Name").and_then(Value::as_str), Some("minecraft:command_block"));
        assert_eq!(cb.get("Properties").unwrap().get("facing").and_then(Value::as_str), Some("up"));
        let blocks = items(root.get("blocks").unwrap());
        let cmd = blocks
            .iter()
            .find(|b| b.get("nbt").is_some() && b.get("nbt").unwrap().get("Command").is_some())
            .unwrap();
        let command = cmd.get("nbt").unwrap().get("Command").and_then(Value::as_str).unwrap();
        assert_eq!(
            command,
            "playsound minecraft:block.note_block.harp block @a ~ ~ ~ 3 1.000000"
        );
        assert_eq!(keys_of(cmd.get("nbt").unwrap()), ["Command", "TrackOutput", "powered", "auto", "conditionMet", "UpdateLastExecution"]);
        assert_eq!(cmd.get("state").and_then(Value::as_i32), Some(3 + 17));
    }

    #[test]
    fn classic_export_roundtrips_sizes_and_notes() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50), (2, 0, 2, 40)]);
        let path = scratch("roundtrip.schematic");
        let info = export_schematic(&s, &path, &SchematicOptions::default()).unwrap();
        assert_eq!((info.width, info.height, info.length), (43, 7, 5));
        assert_eq!(info.note_blocks, 3);
        assert_eq!(info.repeaters, 3);
        let back = inspect_schematic(&path).unwrap();
        assert_eq!(back.width, info.width);
        assert_eq!(back.height, info.height);
        assert_eq!(back.length, info.length);
        assert_eq!(back.note_blocks, info.note_blocks);
        // 文件里数出来的中继器是实际摆放的方块数，和窗口显示的解析值不是同一个量。
        assert!(back.repeaters > 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn structure_export_roundtrips_through_the_palette_join() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        let path = scratch("roundtrip.nbt");
        let info = export_schematic(&s, &path, &o).unwrap();
        let back = inspect_schematic(&path).unwrap();
        assert_eq!(back.note_blocks, info.note_blocks);
        assert_eq!(back.height, info.height);
        // size[0] 是原版硬编码的 32，所以结构文件的宽度读回来必然是 32。
        assert_eq!(back.width, 32);
        assert!(back.repeaters > 0, "中继器应能从 palette 数出来");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn layout_zero_adds_the_loop_sign_and_closes_the_walkway() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.layout = 0;
        o.loop_option = true;
        let built = build(&s, &o);
        assert!(built.sch_loop);
        let nbt = built.classic_nbt();
        let root = crate::nbt::parse(&nbt).unwrap();
        let tes = items(root.get("TileEntities").unwrap());
        assert_eq!(tes[1].get("Text1").and_then(Value::as_str), Some("{\"text\": \"Looping ON\"}"));
        assert_eq!(tes[1].get("Text4").and_then(Value::as_str), Some("{\"text\": \"Looping OFF\"}"));
        // 关掉循环选项后签名只剩一个。
        o.loop_option = false;
        let built = build(&s, &o);
        assert!(!built.sch_loop);
    }

    #[test]
    fn minecart_and_chest_and_loop_need_layout_below_classic() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.minecart = true;
        o.chest = true;
        o.loop_option = true;
        o.layout = 2;
        let built = build(&s, &o);
        assert!(!built.minecart);
        assert!(!built.chest_on);
        assert!(!built.sch_loop);
        o.layout = 0;
        let built = build(&s, &o);
        assert!(built.minecart);
        assert!(built.chest_on);
        assert!(built.sch_loop);
        // 箱子依赖矿车：关掉矿车后箱子也不会出现。
        o.minecart = false;
        let built = build(&s, &o);
        assert!(!built.chest_on);
    }

    #[test]
    fn compress_off_keeps_layer_placeholders() {
        // 3 层里只有第 1 层有音：不压缩时 maxheight[0]=collast=1，
        // layers=ceil(1/4)=1、hei=7，槽位数组里第 0 层是占位（不放方块）。
        let s = song_with(&[(0, 1, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.compress = false;
        let st = calculate_stats(&s);
        assert_eq!(st.maxheight, [1, 1]);
        let built = build_with_stats(&s, &o, &st);
        assert_eq!(built.size.layers, 1);
        assert_eq!(built.size.hei, 7);
        assert_eq!(built.notes.len(), 1);
        // 压缩模式下 layers 用单 tick 最多音符数，这里同样是 1。
        o.compress = true;
        let built = build(&s, &o);
        assert_eq!(built.size.layers, 1);
        assert_eq!(built.notes.len(), 1);
    }

    #[test]
    fn open_guard_uses_totalblocks_minus_block_outside() {
        let empty = Song::new();
        let st = calculate_stats(&empty);
        assert_eq!(open_guard(&st).unwrap_err(), "没有方块可以导出！");
        // 全在 2 八度范围外：原版同样拒绝打开导出窗口。
        let s = song_with(&[(0, 0, 0, 20), (1, 0, 0, 70)]);
        let st = calculate_stats(&s);
        assert_eq!(st.song_blocks, 2);
        assert_eq!(st.block_outside, 2);
        assert!(open_guard(&st).is_err());
        // 有一个可用音符就放行，哪怕它音高越界。
        let s = song_with(&[(0, 0, 0, 20), (1, 0, 0, 45)]);
        assert!(open_guard(&calculate_stats(&s)).is_ok());
    }

    #[test]
    fn too_big_is_refused_with_the_original_message() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.layout = 0;
        o.minecart = true;
        o.notesperrow = 100;
        let mut big = calculate_stats(&s);
        big.repeaters[0][0] = MAX_LEN * 100;
        let e = export_guard(&big, &o).unwrap_err();
        assert!(e.contains("太大"), "{e}");
        assert!(e.contains("2000x2000x256"), "{e}");
    }

    #[test]
    fn export_guard_follows_include_locked() {
        let mut s = song_with(&[(0, 0, 0, 45)]);
        s.layers[0].lock = LockMode::Locked;
        let st = calculate_stats(&s);
        let mut o = SchematicOptions::default();
        // 只有静音层有音：不勾选「包括已静音的层」时计数为 0。
        assert_eq!(st.totalblocks, [0, 1]);
        assert_eq!(export_guard(&st, &o).unwrap_err(), "没有方块可以导出！");
        o.include_locked = true;
        assert!(export_guard(&st, &o).is_ok());
    }

    #[test]
    fn out_of_range_notes_are_skipped_not_moved() {
        let s = song_with(&[(0, 0, 0, 20), (1, 0, 0, 70)]);
        let st = calculate_stats(&s);
        assert_eq!(st.totalblocks, [0, 0]);
        assert_eq!(st.block_outside, 2);
        assert!(export_guard(&st, &SchematicOptions::default()).is_err());
    }

    #[test]
    fn command_block_structure_accepts_the_wider_key_range() {
        let s = song_with(&[(0, 0, 0, 45), (0, 1, 0, 20)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        assert_eq!(build(&s, &o).notes.len(), 1, "普通音符盒模式只认 33..57");
        o.command_block = true;
        assert_eq!(build(&s, &o).notes.len(), 2, "命令方块模式认 9..81");
        // 但 layers 仍按 33..57 计数：整首都用范围外音高时一行都不放。
        assert_eq!(build(&song_with(&[(0, 0, 0, 20)]), &o).notes.len(), 0);
        // 连 9..81 之外（key 8）也不算。
        let s = song_with(&[(0, 0, 0, 45), (0, 1, 0, 8)]);
        assert_eq!(build(&s, &o).notes.len(), 1);
    }

    #[test]
    fn namespaced_and_display_names_match_the_tables() {
        assert_eq!(namespaced_id(35, 11), "blue_wool");
        assert_eq!(namespaced_id(35, 0), "white_wool");
        assert_eq!(namespaced_id(172, 3), "terracotta");
        assert_eq!(namespaced_id(251, 1), "orange_concrete");
        assert_eq!(namespaced_id(95, 1), "orange_stained_glass");
        assert_eq!(namespaced_id(93, 0), "");
        assert_eq!(namespaced_id(1, 0), "stone");
        assert_eq!(block_display_name(35, 11), "Blue Wool");
        assert_eq!(block_display_name(160, 1), "");
        assert_eq!(block_display_name(251, 1), "Concrete");
        assert_eq!(block_display_name(172, 0), "Terracotta");
        assert_eq!(block_display_name(1, 0), "Stone");
        assert_eq!(legacy_block_name(5), "oak_planks");
        assert_eq!(legacy_block_name(35), "white_wool");
    }

    #[test]
    fn state_table_matches_the_original_else_if_chain() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        let built = build(&s, &o);
        let base = built.insnum * 26;
        assert_eq!(built.state_of(0, 0), (base + 4) as i32); // air
        assert_eq!(built.state_of(1, 0), 0); // walkway
        assert_eq!(built.state_of(35, 11), (base + 3) as i32); // blue wool
        assert_eq!(built.state_of(65, 0), (base + 5) as i32); // ladder
        assert_eq!(built.state_of(55, 0), (base + 6) as i32); // wire
        assert_eq!(built.state_of(93, 3), (base + 7) as i32); // east repeater 1
        assert_eq!(built.state_of(93, 15), (base + 10) as i32); // east repeater 4
        assert_eq!(built.state_of(93, 1), (base + 11) as i32); // west repeater 1
        assert_eq!(built.state_of(93, 13), (base + 14) as i32); // west repeater 4
        assert_eq!(built.state_of(94, 0), (base + 15) as i32); // south repeater 4
        assert_eq!(built.state_of(75, 5), (base + 16) as i32); // unlit torch
        assert_eq!(built.state_of(76, 5), (base + 17) as i32); // lit torch
        assert_eq!(built.state_of(77, 0), (base + 18) as i32); // stone button
        assert_eq!(built.state_of(27, 0), (base + 20) as i32); // powered rail
        // 原版的 else-if 链里没有 54 分支（外层已经把箱子格跳过），落到 air。
        assert_eq!(built.state_of(54, 0), (base + 4) as i32);
        assert_eq!(built.state_of(66, 0), (base + 22) as i32); // rail
        assert_eq!(built.state_of(75, 1), (base + 23) as i32); // east unlit wall torch
        assert_eq!(built.state_of(76, 2), (base + 26) as i32); // west lit wall torch
        assert_eq!(built.state_of(69, 0), (base + 27) as i32); // lever
        // 原版这条写的是 75（不亮的火把），但调色板第 28 项叫 "lit=true facing=north"。
        assert_eq!(built.state_of(75, 4), (base + 28) as i32);
        assert_eq!(built.state_of(76, 4), (base + 4) as i32);
        // 音色可视化方块映射到调色板第 3+i 项；不在表里的落到 air。
        assert_eq!(built.state_of(20, 0), 3 + 4); // 第 5 个音色用玻璃
        assert_eq!(built.state_of(159, 5), (base + 4) as i32); // 未登记的组合
    }

    #[test]
    fn demo_song_exports_all_three_layouts_and_both_formats() {
        let s = Song::demo();
        for layout in [0u8, 1, 2] {
            for structure in [false, true] {
                let mut o = SchematicOptions::default();
                o.layout = layout;
                o.structure = structure;
                let path = scratch(&format!("demo-{layout}-{structure}.out"));
                let info = export_schematic(&s, &path, &o).unwrap();
                assert!(info.note_blocks > 0, "layout {layout} structure {structure}");
                assert!(info.length > 0 && info.width > 0 && info.height > 0);
                let raw = gzip_decode(&path);
                let (_, root) = crate::nbt::parse_named(&raw).unwrap();
                if structure {
                    assert_eq!(keys_of(&root), ["DataVersion", "size", "palette", "entities", "blocks"]);
                    // 声明的 blocks 数量必须和真正写出来的一致。
                    let blocks = items(root.get("blocks").unwrap());
                    // 声明数量 = hei*len*wid + 1 个签名格 + 循环签名格。
                    assert_eq!(
                        blocks.len(),
                        info.height * info.length * info.width + 1 + (layout == 0) as usize
                    );
                } else {
                    assert_eq!(keys_of(&root)[0], "Height");
                    let blocks = root.get("Blocks").and_then(Value::as_bytes).unwrap();
                    assert_eq!(blocks.len(), info.height * info.length * info.width);
                }
                std::fs::remove_file(&path).ok();
            }
        }
    }

    fn gzip_decode(path: &std::path::Path) -> Vec<u8> {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let f = std::fs::File::open(path).unwrap();
        let mut out = Vec::new();
        GzDecoder::new(f).read_to_end(&mut out).unwrap();
        out
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nbs_sch_{}", name))
    }
}
