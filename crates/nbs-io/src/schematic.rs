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
//! - structure 的 `size` 头：Litematica 写精确边长，其余写 `min(边长, 32)`。
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

/// 16 种内置乐器对应的「音符盒下方方块」数值 id（原版 `reset_schematic_export.gml`）。
///
/// 铜块一族（16..19）没有这一代的数值 id——这套表是 1.12 之前的 `id + data`，
/// 铜方块只存在于 1.17 之后的命名 id 里。所以这 4 格沿用「取不到就退回石头」的
/// 既有分支（[`SchematicOptions::ins`]），不编造 id。
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

/// 原版 `sch_exp_ins_block` 数组长度：内置 20 个（v6 起）+ 自定义上限 236 个。
/// 总数仍是 256，因为乐器索引是 u8；v6 只是把自定义的名额让出 4 格给铜块一族。
pub const INS_COUNT: usize = 20 + 236;

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
const SLAB: u8 = 44;
const REPEATER: u8 = 93;
/// `block_repeater(..., powered = 1)` 写出的 `93 + 1`。
const REPEATER_POWERED: u8 = 94;

/// 原版硬编码的三组 id（`schematic_export.gml` 开头）。
///
/// 前 16 项逐字抄自原版 `ins[0..15]`；后 4 项是 v6 的铜块一族，取值顺序与装机版
/// 自己导出的 .nbt 参照文件里 `instrument=` 属性逐字一致
/// （`harp … pling, trumpet, trumpet_exposed, trumpet_weathered, trumpet_oxidized`）。
/// 自定义乐器在原版里统一写 `"harp"`（`schematic_export.gml:41-43`），
/// 所以取值越界时也退回 harp。
const INS_NAMES: [&str; 20] = [
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
    "trumpet",
    "trumpet_exposed",
    "trumpet_weathered",
    "trumpet_oxidized",
];

/// 装机版结构导出给 16..19 号铜块一族写的「乐器底下的方块」。
///
/// 这 4 个方块是 1.17+ 的，1.12 数字 id 表里没有，所以 `INSTRUMENT_BLOCKS`
/// 那套数字表（`.schematic` 路径用）填不进去；.nbt 路径直接按名字写。
/// 取值逐字来自装机版自己导出的参照文件 palette[19..23]。
const COPPER_INS_BLOCKS: [&str; 4] = [
    "waxed_copper_block",
    "waxed_exposed_copper",
    "waxed_weathered_copper",
    "waxed_oxidized_copper",
];

/// 牌面第 3、4 行的断句。仓库 GML 两处（.schematic 与 .nbt）都写
/// `Minecraft Note` / `Block Studio`；装机版只改了 .nbt 那一处，
/// .schematic 那处没有新版样本可查，仍按仓库 GML。
const SIGN_TAIL_GML: [&str; 2] = ["Minecraft Note", "Block Studio"];
const SIGN_TAIL_INSTALLED: [&str; 2] = ["Note Block", "Studio"];

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
        self.buf
            .extend_from_slice(&(name.len() as u16).to_be_bytes());
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
    /// `TAG_Int_Array(name, values)`——结构方块的 `size`/`Offset` 一类用得到。
    /// 注意这是数组标签（元素类型由标签本身决定），不是 [`Self::list`] 的列表。
    ///
    /// 编号按 NBT 规范：11 是 Int_Array，12 是 Long_Array（规范里没有 short 数组）。
    /// 这里曾写成 12，Java 侧（创世神内置的 linbus、投影模组）照规范当 3 个 long 读，
    /// 多吞 12 字节后整份文件错位，报 `Invalid NBT ID: 80`。
    pub fn int_array(&mut self, name: &str, v: &[i32]) {
        self.header(11, name);
        self.buf
            .extend_from_slice(&(v.len() as i32).to_be_bytes());
        for x in v {
            self.item_int(*x);
        }
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
        .map(|&id| BlockMenuSection {
            title: None,
            items: vec![McBlock::new(id, 0)],
        })
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
                    out.push(BlockMenuSection {
                        title: None,
                        items: vec![McBlock::new(id, 0)],
                    });
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
        format!(
            "{}, {} — {}",
            self.id,
            self.data,
            block_display_name(self.id, self.data)
        )
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
    /// 原版是全局变量，出厂 `control_create.gml:638` 的 `structure = 1`，
    /// 随后在 format 下拉（`menu_click.gml:254-256`）、加载设置
    /// （`load_settings.gml:155`）与导出入口（`schematic_export.gml:5`）由
    /// `sch_exp_format <= 1` 重算——写 format 时必须同步。
    pub structure: bool,
    /// `sch_exp_format`（reset_schematic_export.gml:42）：0 = 结构方块 NBT，
    /// 1 = Litematica NBT（精确尺寸头），2 = 1.11-1.12 Schematic，
    /// 3 = pre-1.11 Schematic。落盘键 `format`；`structure` / `minecraft_old`
    /// 都是它的派生（`load_settings.gml:154-156`），原版没有独立的
    /// litematica 开关。
    pub format: u8,
    /// `sch_exp_minecraft_old`：pre-1.11 的 id 命名与 Version 段。
    pub minecraft_old: bool,
    /// `command_block`：仅 structure 生效，用命令方块换取更广音域。
    pub command_block: bool,
    /// `sch_command_source`：首选声源（0..9 的 minecraft_export_sources 名）。
    pub command_source: String,
    /// `sch_command_allowed_sources`：允许分配器使用的声源（master 默认关）。
    pub command_allowed_sources: [bool; 10],
    /// `sch_command_tempo_grid`：音符吸附到所选红石网格。
    pub command_tempo_grid: bool,
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
            structure: true,
            format: 0,
            minecraft_old: false,
            command_block: false,
            command_source: crate::mc_export::SOURCES[1].to_string(),
            command_allowed_sources: crate::mc_export::default_allowed_sources(),
            command_tempo_grid: false,
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
    /// 命令声源三项则按 `reset_schematic_export.gml:66-68` 恢复出厂；
    /// `sch_exp_format` 在表里（gml:42）重置为 0；命令方案缓存（`sch_command_plan`）由调用方清空。
    pub fn reset(&mut self) {
        *self = Self {
            tempo_mode: self.tempo_mode,
            structure: self.structure,
            command_block: self.command_block,
            ..Default::default()
        };
    }

    /// 原版 format 菜单（`menu_click.gml:254-256`）：`sch_exp_format = sel;
    /// structure = (sel <= 1); sch_exp_minecraft_old = (sel = 3)`。
    pub fn set_format(&mut self, v: u8) {
        self.format = v.min(3);
        self.structure = self.format <= 1;
        self.minecraft_old = self.format == 3;
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
            v.push(crate::tr(
                "Some note blocks are outside of Minecraft's 2 octave limit. Do you want to export a Schematic anyway, ignoring the affected blocks?",
                "一些方块在 Minecraft 的2八度范围外。你想无视这些方块导出 Schematic 吗？",
            )
            .into());
        }
        if self.block_custom > 0 {
            v.push(
                crate::tr(
                    "Some note blocks have a custom instrument assigned to them. Export anyway?",
                    "一些方块带有自定义音色。继续导出吗？",
                )
                .into(),
            );
        }
        if self.block_pitched > 0 {
            v.push(crate::tr(
                "Some note blocks have fine pitch tuning, which isn't supported in note blocks. Export anyway?",
                "一些方块带有音符盒不支持的微分音。继续导出吗？",
            )
            .into());
        }
        v
    }

    /// 原版速度不兼容时的单次提示（`message`，只确定，不中断导出）。
    pub fn tempo_notice(&self) -> Option<&'static str> {
        let t = self.tempo;
        if !(t == 10.0 || t == 5.0 || t == 2.5) {
            Some(crate::tr(
                "The tempo of the song is not compatible with Minecraft. As a result, the playback speed of the song in-game will differ from the one in the program.",
                "歌曲的速度与 Minecraft 不兼容。所以，在游戏内的播放会和软件内的有差别。",
            ))
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
        let Some(col) = song.columns.get(&a) else {
            continue;
        };
        if col.amount() == 0 {
            continue;
        }
        let last = col.last().unwrap_or(0) as usize;
        let mut c = 0usize; // 含静音层
        let mut d = 0usize; // 不含静音层
        for b in 0..=last {
            let Some(note) = col.notes.get(&(b as u32)) else {
                continue;
            };
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

/// 原版 `schematic_height()`：命令方案存在时 `maximum` 用
/// `command_plan.maximum_slots`，且仅 `sch_exp_layout < 2`（schematic_height.gml:5-6）。
pub fn schematic_height_with(
    stats: &SchematicStats,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
) -> usize {
    let maximum = match plan {
        Some(p) if opts.layout < 2 => p.maximum_slots,
        _ => stats.maxheight[opts.compress as usize],
    };
    4 + 3 * ceil_div(maximum, 4)
}

/// 原版 `schematic_height()`。
pub fn schematic_height(stats: &SchematicStats, opts: &SchematicOptions) -> usize {
    schematic_height_with(stats, opts, None)
}

/// 原版 `schematic_length()`：命令方案存在时中继器数用
/// `plan_repeaters(plan)`。注意原版 length 没有 layout 闸——方案由调用方按
/// `structure && command_block && layout < 2` 传入（与 `schematic_export.gml:72` 一致）。
pub fn schematic_length_with(
    stats: &SchematicStats,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
) -> usize {
    let rows = opts.notesperrow;
    let repeaters = match plan {
        Some(p) => crate::mc_export::plan_repeaters(p, opts.tempo_mode),
        None => stats.repeaters[opts.include_locked as usize][opts.tempo_mode as usize],
    };
    if opts.layout < 2 {
        let mut len = 2 + ceil_div(repeaters, rows) * 3;
        // 原版 `len += 3*!(rows mod 2)`：GML 的 `!` 让偶数行才补这 3。
        if opts.layout == 0 && ceil_div(repeaters, rows) % 2 == 0 {
            len += 3;
        }
        len
    } else {
        (ceil_div(repeaters + 2, rows) * 3 + 1).max(6)
    }
}

/// 原版 `schematic_length()`。
pub fn schematic_length(stats: &SchematicStats, opts: &SchematicOptions) -> usize {
    schematic_length_with(stats, opts, None)
}

/// 尺寸计算；`plan` 是命令方案（由调用方按 `structure && command_block &&
/// layout < 2` 传入，与 `schematic_export.gml:72` 一致）。
pub fn calc_size_with(
    stats: &SchematicStats,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
) -> SchematicSize {
    SchematicSize {
        len: schematic_length_with(stats, opts, plan),
        wid: schematic_width(opts),
        hei: schematic_height_with(stats, opts, plan),
        // 原版 `layers = ceil((command_plan 存在 ? maximum_slots : maxheight) / 4)`
        // （schematic_export.gml:73，闸门在 command_plan 的赋值处）。
        layers: match plan {
            Some(p) => ceil_div(p.maximum_slots, 4),
            None => ceil_div(stats.maxheight[opts.compress as usize], 4),
        },
    }
}

pub fn calc_size(stats: &SchematicStats, opts: &SchematicOptions) -> SchematicSize {
    calc_size_with(stats, opts, None)
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
        Self {
            len,
            wid,
            hei,
            blocks: vec![AIR; n],
            data: vec![0; n],
        }
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
    /// 非命令模式：原曲音高；命令模式：方案行号（供 TileEntity 查 `rows[pit].command`）。
    pit: i32,
}

/// 原版 `noteblockx/y/z/note/ins` 平行表（`pit` 在放置时已被命令查询消费，
/// 不再随表携带）。
#[derive(Clone, Copy, Debug)]
struct NoteEntry {
    x: i32,
    y: i32,
    z: i32,
    note: i32,
    ins: usize,
}

/// 一次导出的完整中间结果：网格 + 音符盒表 + 原版在导出时算出的所有开关。
pub struct Built {
    grid: Grid,
    notes: Vec<NoteEntry>,
    /// 与 notes 平行：命令模式为方案行命令，其余 None。
    commands: Vec<Option<String>>,
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

/// 逐音符盒地放置，与原版 `schematic_export.gml` 的四个 note block 分支一致。
struct Builder<'a> {
    song: &'a Song,
    o: &'a SchematicOptions,
    /// 命令模式（structure + command_block + layout<2）的声源分配方案；其余为 None。
    plan: Option<&'a crate::mc_export::Plan>,
    locked: Vec<bool>,
    size: SchematicSize,
    g: Grid,
    notes: Vec<NoteEntry>,
    /// 与 notes 平行：命令模式为 `Some(rows[pit].command)`，其余 None。
    commands: Vec<Option<String>>,
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
        let Some(col) = self.song.columns.get(&tick) else {
            return out;
        };
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
                        out.push(Slot {
                            ins: n.instrument as i32,
                            key: n.key,
                            pit: n.pitch as i32,
                        });
                    } else if !self.compress {
                        out.push(Slot {
                            ins: -1,
                            key: 0,
                            pit: 0,
                        });
                    }
                }
                _ => {
                    if !self.compress {
                        out.push(Slot {
                            ins: -1,
                            key: 0,
                            pit: 0,
                        });
                    }
                }
            }
        }
        out
    }

    /// 命令模式下原版 `command_plan.slots_by_grid[a]` 的取值（schematic_export.gml:236-248）：
    /// 每项是方案行号，undefined（我们的 usize::MAX）→ `ins=-1, key=33, pit=-1`，
    /// 否则 `ins=instrument, key=33, pit=行号`。
    fn plan_slots(&self, grid: u32) -> Vec<Slot> {
        let p = match self.plan {
            Some(p) => p,
            None => return Vec::new(),
        };
        p.slots_by_grid
            .get(grid as usize)
            .map(|rows| {
                rows.iter()
                    .map(|&r| {
                        if r == usize::MAX {
                            Slot {
                                ins: -1,
                                key: 33,
                                pit: -1,
                            }
                        } else {
                            Slot {
                                ins: p.rows[r].instrument as i32,
                                key: 33,
                                pit: r as i32,
                            }
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 原版一次成行时放的 4 个音符盒位（每层 b 一组，最多消耗 4 个 slot）。
    fn place_row(&mut self, xx: i32, yy: i32, dir: i32, rep: u32, slots: &[Slot], n: &mut i32) {
        let direction = if dir == 1 { 3 } else { 1 };
        for b in 0..self.size.layers {
            let y1 = yy + dir;
            self.block_repeater(
                xx,
                y1,
                (b * 3 + 2) as i32,
                (rep - 1) as u8,
                direction,
                false,
            );
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
                self.put_note(
                    xx - 1,
                    y1 + dir * 2,
                    (b * 3 + 1) as i32,
                    (b * 3) as i32,
                    slots,
                    *n,
                );
                *n -= 1;
            }
            if *n > -1 {
                self.put_note(
                    xx + 1,
                    y1 + dir * 2,
                    (b * 3 + 1) as i32,
                    (b * 3) as i32,
                    slots,
                    *n,
                );
                *n -= 1;
            }
            if *n > -1 {
                self.put_note(
                    xx - 1,
                    y1 + dir,
                    (b * 3 + 2) as i32,
                    (b * 3 + 1) as i32,
                    slots,
                    *n,
                );
                *n -= 1;
            }
            if *n > -1 {
                self.put_note(
                    xx + 1,
                    y1 + dir,
                    (b * 3 + 2) as i32,
                    (b * 3 + 1) as i32,
                    slots,
                    *n,
                );
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
        // 原版 TileEntity 直接查 `command_plan.rows[noteblockpit[a]].command`
        // （schematic_export.gml:1014）；命令模式下 pit 就是方案行号。
        let command = self.plan.map(|p| p.rows[s.pit as usize].command.clone());
        self.commands.push(command);
        self.notes.push(NoteEntry {
            x,
            y,
            z,
            note: s.key as i32 - 33,
            ins: s.ins as usize,
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

        // 立柱 + 梯子。过道中线：仓库 GML 写 `round(wid / 2)`（wid 恒为奇数 → 上取整），
        // 但原版导出的 .nbt 样本里牌子、矿车与整片音符列都落在下取整那一列，按样本走。
        let yy0 = wid / 2;
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

        // 音符盒排布。原版 `export_end = plan ? plan.last_command_grid : enda`
        // （schematic_export.gml:229）。
        let export_end = self
            .plan
            .map(|p| p.last_command_grid)
            .unwrap_or(self.song.enda as i64);
        let mut xx = 3i32;
        let mut yy = 0i32;
        let mut rep = 0u32;
        let mut dir = 1i32;
        let mut circ = 1i32;
        let mut lturnx = -1i32;
        let mut lturny = -1i32;
        let mut lturndel = 0i32;
        for a in 0..=export_end {
            let a = a as u32;
            let mut nblocks = 0i32;
            rep += rep_step(self.tempo_mode);
            let slots = if self.plan.is_some() {
                self.plan_slots(a)
            } else {
                self.tick_slots(a)
            };
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
                if a as i64 == export_end {
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
                            self.block_circuit(xx + cc * circ, yy + dir * 2, (b * 3 + 1) as i32);
                        }
                    }
                    yy += dir * 2;
                    dir = -dir;
                    if circ == -1 && dir == 1 {
                        lturnx = xx;
                        lturny = yy - 1;
                        lturndel = export_end as i32 - a as i32 - 8;
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
///
/// `plan` 是命令模式的声源分配方案；调用方按原版闸门
/// `structure && command_block && layout < 2` 传入（schematic_export.gml:16/72），
/// 其余情况传 None——生成器在 layout>=2（经典排布）不用方案，与原版一致。
pub fn build(song: &Song, opts: &SchematicOptions, plan: Option<&crate::mc_export::Plan>) -> Built {
    let stats = calculate_stats(song);
    build_with_stats(song, opts, &stats, plan)
}

pub fn build_with_stats(
    song: &Song,
    opts: &SchematicOptions,
    stats: &SchematicStats,
    plan: Option<&crate::mc_export::Plan>,
) -> Built {
    let size = calc_size_with(stats, opts, plan);
    let sch_loop = opts.loop_option && opts.layout == 0;
    let minecart = opts.minecart && opts.layout < 2;
    let want_chest = opts.chest && minecart;
    let mut b = Builder {
        song,
        o: opts,
        plan,
        locked: song.locked_layers(),
        size,
        g: Grid::new(size.len, size.wid, size.hei),
        notes: Vec::new(),
        commands: Vec::new(),
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
        commands,
        cyy,
        sign,
        chest,
        ..
    } = b;
    Built {
        grid: g,
        notes,
        commands,
        size,
        opts: opts.clone(),
        repeaters: stats.repeaters[opts.include_locked as usize][opts.tempo_mode as usize],
        insnum: nbs_core::model::FIRST_CUSTOM_INDEX as usize + song.custom_instruments.len(),
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
        let old = self.opts.format == 3;
        n.compound("Schematic");
        n.short("Height", self.size.hei as i16);
        n.short("Length", self.size.len as i16);
        n.short("Width", self.size.wid as i16);
        n.list("Entities", self.minecart as i32, 10);
        if self.minecart {
            n.string(
                "id",
                if old {
                    "Minecart"
                } else {
                    "minecraft:minecart"
                },
            );
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
        self.write_sign_text(&mut n, old, false, SIGN_TAIL_GML);
        n.end();
        if self.sch_loop {
            n.string("id", if old { "Sign" } else { "minecraft:sign" });
            n.int("x", wid - 1 - self.signy);
            n.int("y", self.signz - 1);
            n.int("z", self.signx);
            self.write_sign_text(&mut n, old, true, SIGN_TAIL_GML);
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

    /// `schematic_export.gml` 里两处牌面文字（626-634 的 .schematic 与
    /// 921-924 的结构方块）。`json` 对应原版的 1.13+ 分支：每行包成
    /// `{"text": "…"}`；`tail` 是第 3、4 行的断句，见 `SIGN_TAIL_*`。
    fn write_sign_text(&self, n: &mut Nbt, old: bool, loop_sign: bool, tail: [&str; 2]) {
        let lines: [&str; 4] = if loop_sign {
            ["Looping ON", "", "", "Looping OFF"]
        } else {
            ["Song generated", "by the", tail[0], tail[1]]
        };
        for (i, t) in lines.iter().enumerate() {
            let text = if old {
                t.to_string()
            } else {
                format!("{{\"text\": \"{t}\"}}")
            };
            n.string(&format!("Text{}", i + 1), &text);
        }
    }

    /// 原版开启 `structure` 时写的 1.13+ 结构方块 NBT。
    pub fn structure_nbt(&self) -> Vec<u8> {
        let mut n = Nbt::default();
        let insnum = self.insnum;
        let wid = self.size.wid as i32;
        n.compound("");
        n.int("DataVersion", 1519);
        // 原版（schematic_export.gml:774-783）：`sch_exp_format = 1`（Litematica）
        // 写精确尺寸；其余 structure 格式写 `min(边长, 32)`（结构方块 UI 的 32 格预览上限）。
        n.list("size", 3, 3);
        if self.opts.format == 1 {
            n.item_int(wid);
            n.item_int(self.size.hei as i32);
            n.item_int(self.size.len as i32);
        } else {
            n.item_int(wid.min(32));
            n.item_int((self.size.hei as i32).min(32));
            n.item_int(self.size.len.min(32) as i32);
        }

        n.list("palette", 29 + insnum as i32 * 26, 10);
        for blk in [self.opts.walkway, self.opts.circuit, self.opts.ground] {
            n.string(
                "Name",
                &format!("minecraft:{}", namespaced_id(blk.id, blk.data)),
            );
            n.end();
        }
        for a in 0..insnum {
            // 16..19 的铜块一族没有 1.12 数字 id，装机版在 .nbt 里直接写方块名；
            // 其余行仍按可配置的 digital id 走（`.schematic` 那条路径不变）。
            let copper = a
                .checked_sub(INSTRUMENT_BLOCKS.len())
                .and_then(|k| COPPER_INS_BLOCKS.get(k));
            let name = match copper {
                Some(c) => format!("minecraft:{c}"),
                None => {
                    let blk = self.opts.ins(a);
                    format!("minecraft:{}", namespaced_id(blk.id, blk.data))
                }
            };
            n.string("Name", &name);
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
                    n.string("instrument", INS_NAMES.get(a).copied().unwrap_or("harp"));
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
        prop(
            &mut n,
            "minecraft:ladder",
            &[("facing", "north"), ("waterlogged", "false")],
        );
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
            &[
                ("facing", "west"),
                ("type", "single"),
                ("waterlogged", "false"),
            ],
        );
        prop(
            &mut n,
            "minecraft:rail",
            &[("shape", "north_south"), ("waterlogged", "false")],
        );
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
            // 原版：wid - 1 - (cyy - 1 - 0.5) - 0.5 = wid - cyy，正好是 pos 去掉 0.5 的整格。
            n.item_int(wid - self.cyy);
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

        // 原版按网格体积声明条数：牌子/音符/箱子这些"特殊"格子由前面的显式条目
        // 负责，网格扫描会跳过它们占的格子，所以总数正好是 hei*len*wid。
        let cells = self.size.hei * self.size.len * self.size.wid;
        n.list("blocks", cells as i32, 10);
        let mut written = 0usize;
        n.compound("nbt");
        n.string("Color", "black");
        n.string("id", "minecraft:sign");
        self.write_sign_text(&mut n, false, false, SIGN_TAIL_INSTALLED);
        n.end();
        n.list("pos", 3, 3);
        n.item_int(wid - 1 - self.signy);
        n.item_int(self.signz);
        n.item_int(self.signx);
        n.int("state", (insnum * 26 + 19) as i32);
        n.end();
        written += 1;
        if self.sch_loop {
            n.compound("nbt");
            n.string("Color", "black");
            n.string("id", "minecraft:sign");
            self.write_sign_text(&mut n, false, true, SIGN_TAIL_INSTALLED);
            n.end();
            n.list("pos", 3, 3);
            n.item_int(wid - 1 - self.signy);
            n.item_int(self.signz - 1);
            n.item_int(self.signx);
            n.int("state", (insnum * 26 + 19) as i32);
            n.end();
            written += 1;
        }
        for (t, cmd) in self.notes.iter().zip(self.commands.iter()) {
            if self.opts.command_block {
                // 原版 `TAG_String("Command", command_plan.rows[noteblockpit[a]].command)`
                // （schematic_export.gml:1014）：命令由声源分配方案给出，不在导出时合成。
                n.compound("nbt");
                n.string("id", "minecraft:command_block");
                n.string("Command", cmd.as_deref().unwrap_or_default());
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
            written += 1;
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
            written += 1;
        }
        for c in 0..self.size.hei {
            for a in 0..self.size.len {
                for bb in (0..self.size.wid).rev() {
                    let (id, data) = self.cell(a, bb, c);
                    if id == NOTE_BLOCK || id == CHEST {
                        continue;
                    }
                    // 牌子占的那格（循环牌再往前一格）上面已经写过显式条目，
                    // 扫描要跳过：装机版参照文件里没有任何重复格子，
                    // blocks 条数正好等于网格体积。
                    let same_sign_column = a as i32 == self.signx && bb as i32 == self.signy;
                    if same_sign_column
                        && (c as i32 == self.signz || (self.sch_loop && c as i32 == self.signz - 1))
                    {
                        continue;
                    }
                    n.list("pos", 3, 3);
                    n.item_int(wid - bb as i32 - 1);
                    n.item_int(c as i32);
                    n.item_int(a as i32);
                    n.int("state", self.state_of(id, data));
                    n.end();
                    written += 1;
                }
            }
        }
        debug_assert_eq!(
            written, cells,
            "blocks 声明 {cells} 条，实际写出 {written} 条"
        );
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

// ------------------------------------------------------------------ 直轨导出

/// 直轨一个车道占位：原版 `nblockins/key/pit` 平行数组的一项，从尾部消费。
/// `ins == -1` 表示这一格只消耗名额不放方块（原版 `idx * C - !C` 的产物）。
#[derive(Clone, Copy, Debug)]
struct TrackSlot {
    ins: i32,
    key: i32,
    pit: i32,
}

/// 已放置的音符盒记录（原版 `noteblockx/y/z/note/ins/pit` 平行表）。
#[derive(Clone, Copy, Debug)]
struct TrackNote {
    x: i32,
    y: i32,
    z: i32,
    /// 原版 `noteblocknote`：key - 33。
    note: i32,
    ins: i32,
    /// 命令模式下指向方案行号（`rows[pit].command`），classic 模式不用。
    pit: i32,
}

/// 原版 `track_export()` 的 dummy 对象：structure 模式直接累积 NBT 字节流，
/// classic 模式只有网格（布线写网格，最后组装 Schematic compound）。
struct TrackBuilder<'a> {
    song: &'a Song,
    opts: &'a SchematicOptions,
    plan: Option<&'a crate::mc_export::Plan>,
    insnum: usize,
    structure: bool,
    command_block: bool,
    old: bool,
    track_tick_delay: i32,
    len: usize,
    hei: usize,
    yy: i64,
    layers: usize,
    export_end: u32,
    grid: Option<Grid>,
    nbt: Nbt,
    /// structure 模式：`size` 三个 int 与 `blocks` 列表头的回填位置。
    sizepos: usize,
    blocktagpos: usize,
    totalblocksc: usize,
    /// 原版 `structure_max_x/y/z`。classic 模式停在 sign 初值（辅助函数不更新）。
    smax_x: i64,
    smax_y: i64,
    smax_z: i64,
    /// 原版 `xx`：每 tick 结束 +2*dir（dir 恒 1，直轨无转向）。
    xx: i64,
    notes: Vec<TrackNote>,
}

impl<'a> TrackBuilder<'a> {
    fn new(
        song: &'a Song,
        opts: &'a SchematicOptions,
        plan: Option<&'a crate::mc_export::Plan>,
    ) -> Self {
        let insnum = nbs_core::model::FIRST_CUSTOM_INDEX as usize + song.custom_instruments.len();
        let layers = match plan {
            Some(p) => 1.max((p.track_maximum_slots + 3) / 4),
            None => 4,
        };
        let export_end = match plan {
            Some(p) => p.last_command_grid.max(0) as u32,
            None => song.enda,
        };
        let len = 40 + export_end as usize * 2;
        let hei = 20usize.max(layers * 4 + 4);
        let yy = 49i64; // round(99/2) - 1
        let grid = if opts.structure {
            None
        } else {
            Some(Grid::new(len, 100, hei))
        };
        let (smax_x, smax_y, smax_z) = if opts.structure {
            (99 - yy, hei as i64 - 1, 2)
        } else {
            (2, yy, hei as i64 - 1)
        };
        let track_tick_delay = match opts.tempo_mode {
            0 => 0,
            1 => 1,
            _ => 3,
        };
        Self {
            song,
            opts,
            plan,
            insnum,
            structure: opts.structure,
            command_block: opts.command_block,
            old: opts.minecraft_old,
            track_tick_delay,
            len,
            hei,
            yy,
            layers,
            export_end,
            grid,
            nbt: Nbt::default(),
            sizepos: 0,
            blocktagpos: 0,
            totalblocksc: 0,
            smax_x,
            smax_y,
            smax_z,
            xx: 39,
            notes: Vec::new(),
        }
    }

    fn ins_name(&self, a: usize) -> &'static str {
        INS_NAMES.get(a).copied().unwrap_or("harp")
    }

    /// 原版 `sch_block_write` 的 classic 分支：越界丢弃。原版写的是那张
    /// 2000×2000×256 的大表，序列化只读 [0, len) 区间，写在其外的方块读不到。
    fn grid_set(&mut self, x: i64, y: i64, z: i64, id: u8, data: u8) {
        if let Some(g) = &mut self.grid {
            g.set(x as i32, y as i32, z as i32, id, data);
        }
    }

    /// structure 模式条目的公共段：structure_max 更新 + pos 列表。
    fn entry_pos(&mut self, x: i64, y: i64, z: i64) {
        let blockx = 99 - y;
        self.smax_x = self.smax_x.max(blockx);
        self.smax_y = self.smax_y.max(z);
        self.smax_z = self.smax_z.max(x);
        self.nbt.list("pos", 3, 3);
        self.nbt.item_int(blockx as i32);
        self.nbt.item_int(z as i32);
        self.nbt.item_int(x as i32);
    }

    fn ins_block_at(&self, i: i32) -> (u8, u8) {
        (
            self.opts
                .ins_block
                .get(i as usize)
                .copied()
                .unwrap_or(STONE),
            self.opts.ins_data.get(i as usize).copied().unwrap_or(0),
        )
    }

    /// 原版 `block_walkway_track`。
    fn walkway(&mut self, x: i64, y: i64, z: i64) {
        if !self.structure {
            let (id, data) = (self.opts.walkway.id, self.opts.walkway.data);
            self.grid_set(x, y, z, id, data);
            return;
        }
        self.entry_pos(x, y, z);
        self.nbt.int("state", 0);
        self.nbt.end();
        self.totalblocksc += 1;
    }

    /// 原版 `block_circuit_track`。
    fn circuit(&mut self, x: i64, y: i64, z: i64) {
        if !self.structure {
            let (id, data) = (self.opts.circuit.id, self.opts.circuit.data);
            self.grid_set(x, y, z, id, data);
            return;
        }
        self.entry_pos(x, y, z);
        self.nbt.int("state", 1);
        self.nbt.end();
        self.totalblocksc += 1;
    }

    /// 原版 `block_other_track` 的 id/data 判定链（顺序与原版逐字一致）。
    fn other_state(&self, id: u8, data: u8) -> i32 {
        let base = (self.insnum * 26) as i32;
        if id == AIR {
            base + 4
        } else if id == WOOL && data == 11 {
            base + 3
        } else if id == LADDER {
            base + 5
        } else if id == REDSTONE_WIRE {
            base + 6
        } else if id == TORCHOFF && data == 5 {
            base + 16
        } else if id == TORCHON && data == 5 {
            base + 17
        } else if id == BUTTON_WALL {
            base + 18
        } else if id == POWEREDRAIL {
            base + 20
        } else if id == RAIL {
            base + 22
        } else if id == TORCHOFF && data == 1 {
            base + 23
        } else if id == TORCHON && data == 1 {
            base + 24
        } else if id == TORCHOFF && data == 2 {
            base + 25
        } else if id == TORCHON && data == 2 {
            base + 26
        } else if id == LEVER {
            base + 27
        } else if id == TORCHOFF && data == 4 {
            // 原版注释写作 "north lit walltorch"，但 75 是不亮的火把。
            base + 28
        } else if id == SLAB {
            base + 29
        } else {
            for i in 0..self.insnum {
                let (bid, bdata) = self.ins_block_at(i as i32);
                if id == bid && data == bdata {
                    return (3 + i) as i32;
                }
            }
            base + 4
        }
    }

    /// 原版 `block_other_track`。id = 25 在 structure 模式提前返回（音符盒由
    /// 收尾循环统一写出）；classic 模式照常写网格。
    fn other(&mut self, x: i64, y: i64, z: i64, id: u8, data: u8) {
        if !self.structure {
            self.grid_set(x, y, z, id, data);
            return;
        }
        if id == NOTE_BLOCK {
            return;
        }
        let state = self.other_state(id, data);
        self.entry_pos(x, y, z);
        self.nbt.int("state", state);
        self.nbt.end();
        self.totalblocksc += 1;
    }

    /// 原版 `block_repeater_track`（powered 参数所有调用都传 0）。
    fn repeater(&mut self, x: i64, y: i64, z: i64, delay: i32, direction: i32, powered: u8) {
        if !self.structure {
            let data = (delay * 4 + direction) as u8;
            self.grid_set(x, y, z, REPEATER + powered, data);
            return;
        }
        let dir: i32 = match direction {
            1 => match delay {
                0 => 11,
                1 => 12,
                2 => 13,
                _ => 14,
            },
            2 => match delay {
                0 => 7,
                1 => 8,
                2 => 9,
                _ => 10,
            },
            3 => 15,
            _ => 0,
        };
        self.entry_pos(x, y, z);
        self.nbt.int("state", (self.insnum * 26) as i32 + dir);
        self.nbt.end();
        self.totalblocksc += 1;
    }

    fn run(&mut self) {
        if self.structure {
            self.begin_structure();
        } else {
            let (y, z) = (self.yy as i32, self.hei as i32 - 1);
            if let Some(g) = &mut self.grid {
                g.set(2, y, z, BUTTON_FLOOR, 3);
            }
        }
        self.build_wiring();
        self.build_notes();
        self.finish();
    }

    /// 原版 gml:124-321：根 compound、size/palette/entities/blocks 头与 sign 条目。
    fn begin_structure(&mut self) {
        self.nbt.compound("");
        self.nbt.int("DataVersion", 1519);
        self.nbt.list("size", 3, 3);
        self.sizepos = self.nbt.buf.len();
        self.nbt.item_int(99);
        self.nbt.item_int(self.hei as i32);
        self.nbt.item_int(self.len as i32);
        self.nbt.list("palette", (30 + self.insnum * 26) as i32, 10);
        let (w, wd) = (self.opts.walkway.id, self.opts.walkway.data);
        prop(
            &mut self.nbt,
            &format!("minecraft:{}", namespaced_id(w, wd)),
            &[],
        );
        let (c, cd) = (self.opts.circuit.id, self.opts.circuit.data);
        prop(
            &mut self.nbt,
            &format!("minecraft:{}", namespaced_id(c, cd)),
            &[],
        );
        let (g, gd) = (self.opts.ground.id, self.opts.ground.data);
        prop(
            &mut self.nbt,
            &format!("minecraft:{}", namespaced_id(g, gd)),
            &[],
        );
        for a in 0..self.insnum {
            let blk = self.opts.ins(a);
            let props: &[(&str, &str)] = if a == 9 { &[("axis", "y")] } else { &[] };
            prop(
                &mut self.nbt,
                &format!("minecraft:{}", namespaced_id(blk.id, blk.data)),
                props,
            );
        }
        for a in 0..self.insnum {
            for b in 0..25 {
                if !self.command_block {
                    let note_str = b.to_string();
                    let instrument = self.ins_name(a);
                    prop(
                        &mut self.nbt,
                        "minecraft:note_block",
                        &[
                            ("instrument", instrument),
                            ("note", note_str.as_str()),
                            ("powered", "false"),
                        ],
                    );
                } else {
                    prop(
                        &mut self.nbt,
                        "minecraft:command_block",
                        &[("facing", "up")],
                    );
                }
            }
        }
        // 27 条固定尾部（gml:168-298），属性顺序逐字照原版。
        prop(&mut self.nbt, "minecraft:blue_wool", &[]);
        prop(&mut self.nbt, "minecraft:air", &[]);
        prop(
            &mut self.nbt,
            "minecraft:ladder",
            &[("facing", "north"), ("waterlogged", "false")],
        );
        prop(
            &mut self.nbt,
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
            let d = a.to_string();
            prop(
                &mut self.nbt,
                "minecraft:repeater",
                &[
                    ("delay", d.as_str()),
                    ("facing", "north"),
                    ("locked", "false"),
                    ("powered", "false"),
                ],
            );
        }
        for a in 1..5 {
            let d = a.to_string();
            prop(
                &mut self.nbt,
                "minecraft:repeater",
                &[
                    ("delay", d.as_str()),
                    ("facing", "west"),
                    ("locked", "false"),
                    ("powered", "false"),
                ],
            );
        }
        prop(
            &mut self.nbt,
            "minecraft:repeater",
            &[
                ("delay", "0"),
                ("facing", "east"),
                ("locked", "false"),
                ("powered", "false"),
            ],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_torch",
            &[("lit", "false")],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_torch",
            &[("lit", "true")],
        );
        prop(
            &mut self.nbt,
            "minecraft:stone_button",
            &[("face", "wall"), ("facing", "south"), ("powered", "false")],
        );
        prop(
            &mut self.nbt,
            "minecraft:oak_wall_sign",
            &[("facing", "south"), ("waterlogged", "false")],
        );
        prop(
            &mut self.nbt,
            "minecraft:powered_rail",
            &[("powered", "false"), ("shape", "north_south")],
        );
        prop(
            &mut self.nbt,
            "minecraft:chest",
            &[
                ("facing", "west"),
                ("type", "single"),
                ("waterlogged", "false"),
            ],
        );
        prop(
            &mut self.nbt,
            "minecraft:rail",
            &[("shape", "north_south"), ("waterlogged", "false")],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_wall_torch",
            &[("lit", "false"), ("facing", "east")],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_wall_torch",
            &[("lit", "true"), ("facing", "east")],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_wall_torch",
            &[("lit", "false"), ("facing", "west")],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_wall_torch",
            &[("lit", "true"), ("facing", "west")],
        );
        prop(
            &mut self.nbt,
            "minecraft:lever",
            &[("powered", "true"), ("facing", "south"), ("face", "wall")],
        );
        prop(
            &mut self.nbt,
            "minecraft:redstone_wall_torch",
            &[("lit", "true"), ("facing", "north")],
        );
        prop(
            &mut self.nbt,
            "minecraft:smooth_stone_slab",
            &[("type", "top")],
        );
        self.nbt.list("entities", 0, 10);
        self.blocktagpos = self.nbt.buf.len();
        self.nbt.list("blocks", 114514, 10);
        // sign 条目（gml:303-317）：牌面文字与 schematic 版不同，写死四行。
        self.nbt.compound("nbt");
        self.nbt.string("Color", "black");
        self.nbt.string("id", "minecraft:sign");
        self.nbt.string("Text1", "{\"text\": \"Song generated\"}");
        self.nbt.string("Text2", "{\"text\": \"by the\"}");
        self.nbt.string("Text3", "{\"text\": \"Note Block\"}");
        self.nbt.string("Text4", "{\"text\": \"Studio\"}");
        self.nbt.end();
        self.nbt.list("pos", 3, 3);
        self.nbt.item_int((99 - self.yy) as i32);
        self.nbt.item_int(self.hei as i32 - 1);
        self.nbt.item_int(2);
        self.nbt.int("state", (self.insnum * 26 + 19) as i32);
        self.nbt.end();
        self.totalblocksc += 1;
    }

    /// 原版 gml:323-472 的固定布线：过道、控制、回程电路与节拍阶梯。
    fn build_wiring(&mut self) {
        let yy = self.yy;
        let hei = self.hei as i64;
        let len = self.len as i64;
        // Create walkway
        for a in 0..3 {
            for b in 2..len + 2 {
                self.walkway(b, yy + a, hei - 3);
            }
        }
        // Controls
        self.circuit(1, yy, hei - 2);
        self.circuit(1, yy + 1, hei - 2);
        self.other(1, yy, hei - 1, WOOL, 11);
        self.other(1, yy + 1, hei - 1, WOOL, 11);
        self.other(2, yy + 1, hei - 1, BUTTON_WALL, 3);
        // Back
        self.circuit(0, yy + 1, hei - 2);
        self.other(0, yy + 1, hei - 1, REDSTONE_WIRE, 0);
        self.other(0, yy, hei - 2, TORCHON, 1);
        self.other(0, yy, hei - 3, REDSTONE_WIRE, 0);
        self.circuit(0, yy, hei - 4);
        self.other(0, yy + 1, hei - 4, TORCHOFF, 2);
        self.other(0, yy + 1, hei - 5, REDSTONE_WIRE, 0);
        self.circuit(0, yy + 1, hei - 6);
        for a in 0..13i64 {
            self.repeater(1 + 2 * a, yy + 1, hei - 5 - a, 0, 2, 0);
            self.other(2 + 2 * a, yy + 1, hei - 5 - a, REDSTONE_WIRE, 0);
            self.circuit(1 + 2 * a, yy + 1, hei - 6 - a);
            self.circuit(2 + 2 * a, yy + 1, hei - 6 - a);
        }
        for a in 0..4i64 {
            self.repeater(27 + a * 2, yy + 1, hei - 18, 0, 2, 0);
            self.circuit(27 + a * 2, yy + 1, hei - 19);
            self.circuit(28 + a * 2, yy + 1, hei - 18);
        }
        for a in 0..11i64 {
            self.circuit(34, yy - a, hei - 19);
            self.other(34, yy - a, hei - 18, REDSTONE_WIRE, 0);
            self.circuit(34, yy + 2 + a, hei - 19);
            self.other(34, yy + 2 + a, hei - 18, REDSTONE_WIRE, 0);
        }
        for a in 0..12i64 {
            self.circuit(34, yy - 11 - a, hei - 18);
            if a != 4 {
                self.other(34, yy - 11 - a, hei - 17, REDSTONE_WIRE, 0);
            } else {
                self.repeater(34, yy - 11 - a, hei - 17, 0, 1, 0);
            }
            self.circuit(34, yy + 13 + a, hei - 18);
            if a != 4 {
                self.other(34, yy + 13 + a, hei - 17, REDSTONE_WIRE, 0);
            } else {
                self.repeater(34, yy + 13 + a, hei - 17, 0, 3, 0);
            }
        }
        for a in 0..12i64 {
            self.circuit(34, yy - 23 - a, hei - 17);
            if a != 8 {
                self.other(34, yy - 23 - a, hei - 16, REDSTONE_WIRE, 0);
            } else {
                self.repeater(34, yy - 23 - a, hei - 16, 0, 1, 0);
            }
            self.circuit(34, yy + 25 + a, hei - 17);
            if a != 8 {
                self.other(34, yy + 25 + a, hei - 16, REDSTONE_WIRE, 0);
            } else {
                self.repeater(34, yy + 25 + a, hei - 16, 0, 3, 0);
            }
        }
        for a in 0..12i64 {
            self.circuit(34, yy - 35 - a, hei - 16);
            self.other(34, yy - 35 - a, hei - 15, REDSTONE_WIRE, 0);
            self.circuit(34, yy + 37 + a, hei - 16);
            self.other(34, yy + 37 + a, hei - 15, REDSTONE_WIRE, 0);
        }
        // 角落电路（gml:392-420，原版逐字展开，左右两翼镜像）。
        self.circuit(35, yy - 11, hei - 18);
        self.circuit(36, yy - 11, hei - 18);
        self.circuit(37, yy - 11, hei - 18);
        self.circuit(35, yy - 11 - 12, hei - 17);
        self.circuit(36, yy - 11 - 12, hei - 17);
        self.circuit(37, yy - 11 - 12, hei - 17);
        self.circuit(35, yy - 11 - 12 - 12, hei - 16);
        self.circuit(36, yy - 11 - 12 - 12, hei - 16);
        self.circuit(37, yy - 11 - 12 - 12, hei - 16);
        self.circuit(35, yy - 11 - 12 - 12 - 12, hei - 16);
        self.circuit(36, yy - 11 - 12 - 12 - 12, hei - 15);
        self.circuit(37, yy - 11 - 12 - 12 - 12, hei - 15);
        self.circuit(34, yy - 11 - 12 - 12 - 12, hei - 15);
        self.circuit(35, yy + 11 + 2, hei - 18);
        self.circuit(36, yy + 11 + 2, hei - 18);
        self.circuit(37, yy + 11 + 2, hei - 18);
        self.circuit(35, yy + 11 + 12 + 2, hei - 17);
        self.circuit(36, yy + 11 + 12 + 2, hei - 17);
        self.circuit(37, yy + 11 + 12 + 2, hei - 17);
        self.circuit(35, yy + 11 + 12 + 12 + 2, hei - 16);
        self.circuit(36, yy + 11 + 12 + 12 + 2, hei - 16);
        self.circuit(37, yy + 11 + 12 + 12 + 2, hei - 16);
        self.circuit(35, yy + 11 + 12 + 12 + 12 + 2, hei - 16);
        self.circuit(36, yy + 11 + 12 + 12 + 12 + 2, hei - 15);
        self.circuit(37, yy + 11 + 12 + 12 + 12 + 2, hei - 15);
        self.circuit(34, yy + 11 + 12 + 12 + 12 + 2, hei - 15);
        self.circuit(35, yy + 1, hei - 19);
        self.circuit(36, yy + 1, hei - 18);
        self.circuit(37, yy + 1, hei - 19);
        // 竖向红石线列（gml:422-438）。
        self.other(36, yy - 11, hei - 17, REDSTONE_WIRE, 0);
        self.other(37, yy - 11, hei - 17, REDSTONE_WIRE, 0);
        self.other(36, yy - 11 - 12, hei - 16, REDSTONE_WIRE, 0);
        self.other(37, yy - 11 - 12, hei - 16, REDSTONE_WIRE, 0);
        self.other(36, yy - 11 - 12 - 12, hei - 15, REDSTONE_WIRE, 0);
        self.other(37, yy - 11 - 12 - 12, hei - 15, REDSTONE_WIRE, 0);
        self.other(36, yy - 11 - 12 - 12 - 12, hei - 14, REDSTONE_WIRE, 0);
        self.other(37, yy - 11 - 12 - 12 - 12, hei - 14, REDSTONE_WIRE, 0);
        self.other(36, yy + 11 + 2, hei - 17, REDSTONE_WIRE, 0);
        self.other(37, yy + 11 + 2, hei - 17, REDSTONE_WIRE, 0);
        self.other(36, yy + 11 + 12 + 2, hei - 16, REDSTONE_WIRE, 0);
        self.other(37, yy + 11 + 12 + 2, hei - 16, REDSTONE_WIRE, 0);
        self.other(36, yy + 11 + 12 + 12 + 2, hei - 15, REDSTONE_WIRE, 0);
        self.other(37, yy + 11 + 12 + 12 + 2, hei - 15, REDSTONE_WIRE, 0);
        self.other(36, yy + 11 + 12 + 12 + 12 + 2, hei - 14, REDSTONE_WIRE, 0);
        self.other(37, yy + 11 + 12 + 12 + 12 + 2, hei - 14, REDSTONE_WIRE, 0);
        self.other(37, yy + 1, hei - 18, REDSTONE_WIRE, 0);
        // 九组中继器（gml:440-448）。
        self.repeater(35, yy - 11, hei - 17, 2, 2, 0);
        self.repeater(35, yy - 11 - 12, hei - 16, 1, 2, 0);
        self.repeater(35, yy - 11 - 12 - 12, hei - 15, 0, 2, 0);
        self.repeater(35, yy - 11 - 12 - 12 - 12, hei - 15, 0, 2, 0);
        self.repeater(35, yy + 11 + 2, hei - 17, 2, 2, 0);
        self.repeater(35, yy + 11 + 12 + 2, hei - 16, 1, 2, 0);
        self.repeater(35, yy + 11 + 12 + 12 + 2, hei - 15, 0, 2, 0);
        self.repeater(35, yy + 11 + 12 + 12 + 12 + 2, hei - 15, 0, 2, 0);
        self.repeater(35, yy + 1, hei - 18, 2, 2, 0);
        // 37/38 交替的台阶 + 39 列中继器（gml:450-472）。
        for a in 0..4i64 {
            for b in 0..13i64 {
                let x = 37 + (b % 2 == 0) as i64;
                self.other(x, yy - 11 - 12 * a, hei - 17 + b + a, SLAB, 2);
                self.other(x, yy - 11 - 12 * a, hei - 16 + b + a, REDSTONE_WIRE, 2);
                self.other(x, yy + 11 + 12 * a + 2, hei - 17 + b + a, SLAB, 2);
                self.other(x, yy + 11 + 12 * a + 2, hei - 16 + b + a, REDSTONE_WIRE, 2);
            }
            for b in 0..4i64 {
                self.circuit(39, yy - 11 - 12 * a, hei - 17 + 4 * b + a);
                self.repeater(39, yy - 11 - 12 * a, hei - 16 + 4 * b + a, 0, 2, 0);
                self.circuit(39, yy + 11 + 12 * a + 2, hei - 17 + 4 * b + a);
                self.repeater(39, yy + 11 + 12 * a + 2, hei - 16 + 4 * b + a, 0, 2, 0);
            }
        }
        for b in 0..13i64 {
            let x = 37 + (b % 2 == 0) as i64;
            self.other(x, yy + 1, hei - 18 + b, SLAB, 2);
            self.other(x, yy + 1, hei - 17 + b, REDSTONE_WIRE, 2);
        }
        for b in 0..4i64 {
            self.circuit(39, yy + 1, hei - 18 + 4 * b);
            self.repeater(39, yy + 1, hei - 17 + 4 * b, 0, 2, 0);
        }
    }

    /// 原版 gml:474-1270：逐 tick 把 9 条车道的名额放进车道组（每组 4 个音符盒）。
    /// 命令模式读方案的 `track_rows_by_grid`；非命令模式对每个音符按 9 条判定式
    /// 独立入道——判定不互斥，同一音符可同时落两条车道（原版语义，勿合并）。
    fn build_notes(&mut self) {
        let yy = self.yy;
        let dir = 1i64;
        for a in 0..=self.export_end {
            let mut lanes: [Vec<TrackSlot>; 9] = Default::default();
            match self.plan {
                Some(plan) => {
                    let channels = &plan.track_rows_by_grid[a as usize];
                    for ch in 0..9 {
                        for &row_index in &channels[ch] {
                            let row = &plan.rows[row_index];
                            lanes[ch].push(TrackSlot {
                                ins: row.instrument as i32,
                                key: 33,
                                pit: row.index as i32,
                            });
                        }
                    }
                }
                None => {
                    if let Some(col) = self.song.columns.get(&a) {
                        if col.amount() > 0 {
                            let locked = self.song.locked_layers();
                            for b in 0..self.song.layers.len() {
                                let Some(note) = self.song.get(a, b as u32) else {
                                    continue;
                                };
                                let is_locked = locked.get(b).copied().unwrap_or(false);
                                if is_locked && !self.opts.include_locked {
                                    continue;
                                }
                                let key = note.key as i32;
                                if !((key > 32 && key < 58)
                                    || (self.structure
                                        && self.command_block
                                        && key >= 9
                                        && key <= 81))
                                {
                                    continue;
                                }
                                let layer = &self.song.layers[b];
                                let layerstereo = layer.stereo as f64;
                                let song_pan = note.panning as f64;
                                let song_vel = note.velocity as f64;
                                let cpan = if layerstereo == 100.0 {
                                    song_pan
                                } else {
                                    (layerstereo + song_pan) / 2.0
                                };
                                let cvol = (layer.volume as f64 / 100.0) * song_vel;
                                let cpanvol = (cpan - 100.0) * cvol + 100.0;
                                // 原版 `ds_list_find_index(instrument_list, song_ins)`：
                                // .nbs 加载的整数恒找不到（-1），编辑器/MIDI 放置的才
                                // 有索引。我们的模型统一存索引，按编辑器语义取值。
                                let idx = note.instrument as i32;
                                let pit = note.pitch as i32;
                                let push = |c: i32| TrackSlot {
                                    ins: idx * c - (1 - c),
                                    key: key * c,
                                    pit: pit * c,
                                };
                                let l =
                                    (cpan > 80.0 && cpan < 120.0) || (cpan > 0.0 && cpan <= 80.0);
                                let r = (cpan > 80.0 && cpan < 120.0)
                                    || (cpan > 120.0 && cpan <= 200.0);
                                // l4..r3 共用一套 `idx*C - !C`（gml:579-610，逐字）。
                                let c =
                                    ((l && cvol > 0.0 && cvol <= 0.2) || cpanvol >= 180.0) as i32;
                                lanes[0].push(push(c));
                                let c = ((l && cvol > 0.2 && cvol <= 0.4)
                                    || (cpanvol >= 160.0 && cpanvol < 180.0))
                                    as i32;
                                lanes[1].push(push(c));
                                let c = ((l && cvol > 0.4 && cvol <= 0.6)
                                    || (cpanvol >= 140.0 && cpanvol < 160.0))
                                    as i32;
                                lanes[2].push(push(c));
                                let c = ((((cpan > 80.0 && cpan < 120.0 && cvol <= 0.8)
                                    || (cpan > 0.0 && cpan <= 80.0))
                                    && cvol > 0.6)
                                    || (cpanvol >= 120.0 && cpanvol < 140.0))
                                    as i32;
                                lanes[3].push(push(c));
                                let c = (cpan > 80.0 && cpan < 120.0 && cvol > 0.8) as i32;
                                lanes[4].push(push(c));
                                let c = ((((cpan > 80.0 && cpan < 120.0 && cvol <= 0.8)
                                    || (cpan > 120.0 && cpan <= 200.0))
                                    && cvol > 0.6)
                                    || (cpanvol > 60.0 && cpanvol <= 80.0))
                                    as i32;
                                lanes[5].push(push(c));
                                let c = ((r && cvol > 0.4 && cvol <= 0.6)
                                    || (cpanvol > 40.0 && cpanvol <= 60.0))
                                    as i32;
                                lanes[6].push(push(c));
                                let c = ((r && cvol > 0.2 && cvol <= 0.4)
                                    || (cpanvol > 20.0 && cpanvol <= 40.0))
                                    as i32;
                                lanes[7].push(push(c));
                                // r4 的取反项与乘法项条件不同（gml:611 逐字）：
                                // 乘法项看 cpanvol <= 20，取反项看 cpan <= 20。
                                let cm =
                                    ((r && cvol > 0.0 && cvol <= 0.2) || cpanvol <= 20.0) as i32;
                                let cn = ((r && cvol > 0.0 && cvol <= 0.2) || cpan <= 20.0) as i32;
                                lanes[8].push(TrackSlot {
                                    ins: idx * cm - (1 - cn),
                                    key: key * cm,
                                    pit: pit * cm,
                                });
                            }
                        }
                    }
                }
            }
            // 消费：中央 + 左四道 + 右四道，从车道缓冲尾部 pop（原版计数器向
            // -1 递减等价）。y1 = yy + dir ± 12k，adds = 1..5。
            self.emit_lane(&mut lanes[4], yy + dir, 1);
            self.emit_lane(&mut lanes[3], yy + dir - 12, 2);
            self.emit_lane(&mut lanes[2], yy + dir - 24, 3);
            self.emit_lane(&mut lanes[1], yy + dir - 36, 4);
            self.emit_lane(&mut lanes[0], yy + dir - 48, 5);
            self.emit_lane(&mut lanes[5], yy + dir + 12, 2);
            self.emit_lane(&mut lanes[6], yy + dir + 24, 3);
            self.emit_lane(&mut lanes[7], yy + dir + 36, 4);
            self.emit_lane(&mut lanes[8], yy + dir + 48, 5);
            self.xx += 2 * dir;
        }
    }

    /// 原版每组车道（gml:629-1266 九段的公共形状）：固定布线 + 从车道尾部
    /// 逐个消费最多 4 个名额（#1/#2 在 x1+dir*2 的 y∓1，#3/#4 在 x1+dir 的 y∓1）。
    fn emit_lane(&mut self, lane: &mut Vec<TrackSlot>, y1: i64, adds: i64) {
        let dir = 1i64;
        let x1 = self.xx;
        for b in 0..self.layers {
            let bz = (b * 4) as i64;
            self.repeater(x1 + 2, y1, bz + 2 + adds, self.track_tick_delay, 2, 0);
            self.circuit(x1 + dir, y1, bz + 2 + adds);
            self.circuit(x1 + 2, y1, bz + adds);
            self.circuit(x1 + dir, y1, bz + adds);
            self.circuit(x1 + dir * 2, y1, bz + 1 + adds);
            self.other(x1 + dir, y1, bz + 1 + adds, REDSTONE_WIRE, 0);
            for part in 0..4 {
                let Some(s) = lane.pop() else { break };
                if s.ins < 0 {
                    continue;
                }
                let (nx, ny, nz) = match part {
                    0 => {
                        self.other(x1 + dir * 2, y1 - 1, bz + 1 + adds, NOTE_BLOCK, 0);
                        let (id, data) = self.ins_block_at(s.ins);
                        self.other(x1 + dir * 2, y1 - 1, bz + adds, id, data);
                        self.circuit(x1 + dir * 2, y1 - 1, bz - 1 + adds);
                        (x1 + dir * 2, y1 - 1, bz + 1 + adds)
                    }
                    1 => {
                        self.other(x1 + dir * 2, y1 + 1, bz + 1 + adds, NOTE_BLOCK, 0);
                        let (id, data) = self.ins_block_at(s.ins);
                        self.other(x1 + dir * 2, y1 + 1, bz + adds, id, data);
                        self.circuit(x1 + dir * 2, y1 + 1, bz - 1 + adds);
                        (x1 + dir * 2, y1 + 1, bz + 1 + adds)
                    }
                    2 => {
                        self.other(x1 + dir, y1 - 1, bz + 2 + adds, NOTE_BLOCK, 0);
                        let (id, data) = self.ins_block_at(s.ins);
                        self.other(x1 + dir, y1 - 1, bz + 1 + adds, id, data);
                        self.circuit(x1 + dir, y1 - 1, bz + adds);
                        (x1 + dir, y1 - 1, bz + 2 + adds)
                    }
                    _ => {
                        self.other(x1 + dir, y1 + 1, bz + 2 + adds, NOTE_BLOCK, 0);
                        let (id, data) = self.ins_block_at(s.ins);
                        self.other(x1 + dir, y1 + 1, bz + 1 + adds, id, data);
                        self.circuit(x1 + dir, y1 + 1, bz + adds);
                        (x1 + dir, y1 + 1, bz + 2 + adds)
                    }
                };
                self.notes.push(TrackNote {
                    x: nx as i32,
                    y: ny as i32,
                    z: nz as i32,
                    note: s.key - 33,
                    ins: s.ins,
                    pit: s.pit,
                });
            }
        }
    }

    /// 原版 gml:1272-1373：structure 补音符盒条目并回填 size/blocks 头；
    /// classic 组装 Schematic compound（TileEntities + Blocks/Data + Version）。
    fn finish(&mut self) {
        if self.structure {
            for i in 0..self.notes.len() {
                let note = self.notes[i];
                self.nbt.compound("nbt");
                if self.command_block {
                    self.nbt.string("id", "minecraft:command_block");
                }
                let command = if self.command_block {
                    self.plan
                        .and_then(|p| p.rows.get(note.pit as usize))
                        .map(|r| r.command.clone())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                self.nbt.string("Command", &command);
                self.nbt.byte("TrackOutput", 0);
                self.nbt.byte("powered", 0);
                self.nbt.byte("auto", 0);
                self.nbt.byte("conditionMet", 1);
                self.nbt.byte("UpdateLastExecution", 1);
                self.nbt.end();
                self.entry_pos(note.x as i64, note.y as i64, note.z as i64);
                let state = 3
                    + self.insnum as i32
                    + if self.command_block {
                        0
                    } else {
                        note.ins * 25 + note.note
                    };
                self.nbt.int("state", state);
                self.nbt.end();
                self.totalblocksc += 1;
            }
            // TAG_End 闭合 blocks 列表。根 compound 没有闭合字节——与
            // schematic_export 同款（装机版如此，structure_nbt 已实测可加载）。
            self.nbt.end();
            let blockamount = self.totalblocksc as i32;
            let (sx, sy, sz) = if self.opts.format == 1 {
                (
                    (self.smax_x + 1) as i32,
                    (self.smax_y + 1) as i32,
                    (self.smax_z + 1) as i32,
                )
            } else {
                (
                    (self.smax_x + 1).min(32) as i32,
                    (self.smax_y + 1).min(32) as i32,
                    (self.smax_z + 1).min(32) as i32,
                )
            };
            let at = self.sizepos;
            self.nbt.buf[at..at + 4].copy_from_slice(&sx.to_be_bytes());
            self.nbt.buf[at + 4..at + 8].copy_from_slice(&sy.to_be_bytes());
            self.nbt.buf[at + 8..at + 12].copy_from_slice(&sz.to_be_bytes());
            let mut hdr = Vec::with_capacity(14);
            hdr.push(9);
            hdr.extend_from_slice(&6u16.to_be_bytes());
            hdr.extend_from_slice(b"blocks");
            hdr.push(10);
            hdr.extend_from_slice(&blockamount.to_be_bytes());
            let at = self.blocktagpos;
            self.nbt.buf[at..at + hdr.len()].copy_from_slice(&hdr);
        } else {
            let signx = 2i64;
            let signy = self.yy;
            let signz = self.hei as i64 - 1;
            // classic 的 structure_max 不被辅助函数更新，三向实际不变：
            // len 不扩、wid = max(100, 50) = 100、hei 不扩。
            let len = (self.len as i64).max(self.smax_x + 1) as usize;
            let wid = 100usize.max(self.smax_y as usize + 1);
            let hei = (self.hei as i64).max(self.smax_z + 1) as usize;
            self.nbt.compound("Schematic");
            self.nbt.short("Height", hei as i16);
            self.nbt.short("Length", len as i16);
            self.nbt.short("Width", wid as i16);
            self.nbt.list("Entities", 0, 10);
            self.nbt
                .list("TileEntities", (1 + self.notes.len()) as i32, 10);
            if self.old {
                self.nbt.string("id", "Sign");
            } else {
                self.nbt.string("id", "minecraft:sign");
            }
            self.nbt.int("x", (wid as i64 - 1 - signy) as i32);
            self.nbt.int("y", signz as i32);
            self.nbt.int("z", signx as i32);
            if self.old {
                self.nbt.string("Text1", "Song generated");
                self.nbt.string("Text2", "by the");
                self.nbt.string("Text3", "Note Block");
                self.nbt.string("Text4", "Studio");
            } else {
                self.nbt.string("Text1", "{\"text\": \"Song generated\"}");
                self.nbt.string("Text2", "{\"text\": \"by the\"}");
                self.nbt.string("Text3", "{\"text\": \"Note Block\"}");
                self.nbt.string("Text4", "{\"text\": \"Studio\"}");
            }
            self.nbt.end();
            for note in &self.notes {
                if self.old {
                    self.nbt.string("id", "Music");
                } else {
                    self.nbt.string("id", "minecraft:noteblock");
                }
                self.nbt.int("x", (wid as i64 - 1 - note.y as i64) as i32);
                self.nbt.int("y", note.z);
                self.nbt.int("z", note.x);
                self.nbt.byte("note", note.note as i8 as u8);
                self.nbt.end();
            }
            self.nbt.string("Materials", "Alpha");
            let (blocks, data) = self
                .grid
                .as_ref()
                .map(|g| g.serialize())
                .unwrap_or_default();
            self.nbt.byte_array("Blocks", &blocks);
            self.nbt.byte_array("Data", &data);
            self.nbt.compound("Version");
            if self.old {
                self.nbt.string("Name", "1.8");
                self.nbt.byte("Snapshot", 0);
            } else {
                self.nbt.int("Id", 922);
                self.nbt.string("Name", "1.11.2");
                self.nbt.byte("Snapshot", 0);
            }
            self.nbt.end();
            self.nbt.end();
        }
    }
}

/// 原版 `track_export()` 的生成与落盘（保存对话框与消息弹窗由编辑器层负责）。
/// `plan` 只在 structure && command_block 下生效（与原版入口闸门一致），
/// 由 `mc_export::prepare_track_plan` 填充后传入。
pub fn export_track(
    song: &Song,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
    path: &std::path::Path,
) -> Result<(), String> {
    let plan = if opts.structure && opts.command_block {
        plan
    } else {
        None
    };
    let mut b = TrackBuilder::new(song, opts, plan);
    b.run();
    let gz = gzip(&b.nbt.buf)?;
    std::fs::write(path, gz).map_err(|e| {
        crate::tr(
            &format!("failed to write track file: {e}"),
            &format!("写入直轨文件失败: {e}"),
        )
        .to_string()
    })?;
    Ok(())
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

pub(crate) fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).map_err(|e| {
        crate::tr(
            &format!("gzip compression failed: {e}"),
            &format!("gzip 压缩失败: {e}"),
        )
        .to_string()
    })?;
    enc.finish().map_err(|e| {
        crate::tr(
            &format!("gzip finish failed: {e}"),
            &format!("gzip 结束失败: {e}"),
        )
        .to_string()
    })
}

/// 原版菜单打开窗口前的闸门（`calculate_size()` 的返回值）：
/// 可导出的音符数 = 音符总数 − 2 八度范围外的数量，为 0 时不弹窗口。
pub fn open_guard(stats: &SchematicStats) -> Result<(), String> {
    if stats.song_blocks.saturating_sub(stats.block_outside) == 0 {
        return Err(crate::tr("There are no blocks to export!", "没有方块可以导出！").into());
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
    /// 原版 CN 分支（`draw_window_schematic_export.gml:346-351`）的
    /// 无方块标题是「导出结构」，不是「导出 Schematic」。
    pub fn dialog(self) -> Option<(&'static str, &'static str)> {
        match self {
            ExportCheck::Ok => None,
            ExportCheck::NoBlocks => Some((
                crate::tr("Schematic export", "导出结构"),
                crate::tr("There are no blocks to export!", "没有方块可以导出！"),
            )),
            ExportCheck::TooBig => Some((
                crate::tr("Error", "错误"),
                crate::tr(
                    "The Schematic is too big. The maximum size is 2000x2000x256.\nTry changing the \"Repeaters per row\" value to decrease the size.",
                    "这个结构太大了。大小限制为 2000x2000x256。\n可以更改“每行中继器个数”来减小大小。",
                ),
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

/// 原版 `schematic_export.gml:16-26` 命令模式的两道闸门（方案行数为空 / 方案尺寸
/// 超限），返回 `(标题, 正文)`；通过时 `None`。与按钮闸门不同：尺寸按方案算，
/// 且不做「没有方块」检查（那是按钮用歌曲计数器做的）。
pub fn command_export_check(
    plan: &crate::mc_export::Plan,
    stats: &SchematicStats,
    opts: &SchematicOptions,
) -> Option<(&'static str, &'static str)> {
    if plan.rows.is_empty() {
        return Some((
            crate::tr("Schematic export", "导出结构"),
            crate::tr(
                "There are no sound commands to export!",
                "没有声音命令可以导出！",
            ),
        ));
    }
    if schematic_length_with(stats, opts, Some(plan)) >= MAX_LEN
        || schematic_width(opts) >= MAX_WID
        || schematic_height_with(stats, opts, Some(plan)) >= MAX_HEI
    {
        return Some((
            crate::tr("Error", "错误"),
            crate::tr(
                "The Schematic is too big. The maximum size is 2000x2000x256.\nTry changing the \"Repeaters per row\" value to decrease the size.",
                "这个结构太大了。大小限制为 2000x2000x256。\n可以更改“每行中继器个数”来减小大小。",
            ),
        ));
    }
    None
}

pub fn export_guard(stats: &SchematicStats, opts: &SchematicOptions) -> Result<(), String> {
    match export_check(stats, opts).dialog() {
        Some((_, msg)) => Err(msg.into()),
        None => Ok(()),
    }
}

/// 导出结构到 `path`。
///
/// `plan` 是命令模式的声源分配方案：原版在 `schematic_export()` 里以
/// `get_schematic_plan(true)` 现算（全预算）再校验（schematic_export.gml:16-26），
/// 这里由调用方传入。命令方块 + 经典排布（layout>=2）在原版里不可达且必崩
/// （`command_plan` 未定义就被读 `rows`），这里提前报错而不是写出空命令。
pub fn export_schematic(
    song: &Song,
    path: &Path,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
) -> Result<SchematicInfo, String> {
    let stats = calculate_stats(song);
    check_export_gates(&stats, opts, plan)?;
    let built = build_with_stats(song, opts, &stats, plan);
    // 原版导出入口重算 `structure = (sch_exp_format <= 1)`（schematic_export.gml:5），
    // 所以 NBT 形态以 format 为准，不读可能滞后的全局字段。
    let nbt = if opts.format <= 1 {
        built.structure_nbt()
    } else {
        built.classic_nbt()
    };
    let packed = gzip(&nbt)?;
    std::fs::write(path, packed).map_err(|e| {
        crate::tr(
            &format!("Failed to write schematic: {e}"),
            &format!("写入 schematic 失败: {e}"),
        )
        .to_string()
    })?;
    Ok(built.info())
}

/// 创世神（WorldEdit 7.4.5）认不认这个文件名。
///
/// 判据照它自己的白名单正则 `WorldEdit.SAFE_FILENAME_REGEX`：
/// `^[A-Za-z0-9_\- \./\\'$@~!%^*()\[\]\+{},\?]+\.[A-Za-z0-9]+$`
/// 字符类里一个非 ASCII 字符都没有，所以中文歌名命不中：目录监视器
/// `RecursiveDirectoryWatcher` 会跳过这个文件并写一行 `Invalid file name detected`
/// （于是 `/schem list` 里看不见），按名 `/schem load` 走 `getSafeOpenFile` 同样判非法。
/// 投影模组（Litematica）没这条限制，中文名照读。
pub fn worldedit_rejects_filename(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return true;
    };
    // 正则要求 `名字.扩展名`，扩展名只能是 ASCII 字母数字。
    let Some(dot) = name.rfind('.') else {
        return true;
    };
    let (stem, ext) = (&name[..dot], &name[dot + 1..]);
    if stem.is_empty() || ext.is_empty() {
        return true;
    }
    !stem.chars().all(worldedit_name_char) || !ext.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 那条字符类里除字母数字之外的成员。
const WORLDEDIT_NAME_PUNCT: &str = "_- ./\\'$@~!%^*()[]+{},?";

fn worldedit_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || WORLDEDIT_NAME_PUNCT.contains(c)
}

/// 生成未压缩的 NBT 字节，供测试与检查用。
pub fn build_nbt(
    song: &Song,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
) -> Result<Vec<u8>, String> {
    let stats = calculate_stats(song);
    check_export_gates(&stats, opts, plan)?;
    let built = build_with_stats(song, opts, &stats, plan);
    Ok(if opts.format <= 1 {
        built.structure_nbt()
    } else {
        built.classic_nbt()
    })
}

/// 原版按钮闸门 + `schematic_export()` 内部闸门的合并：命令模式查方案
/// （schematic_export.gml:16-26），其余走「没有方块/尺寸超限」。
fn check_export_gates(
    stats: &SchematicStats,
    opts: &SchematicOptions,
    plan: Option<&crate::mc_export::Plan>,
) -> Result<(), String> {
    if opts.format <= 1 && opts.command_block {
        if opts.layout >= 2 {
            return Err(crate::tr(
                "Internal error: command-block export needs a walkway layout.",
                "内部错误：命令方块导出需要过道布局。",
            )
            .to_string());
        }
        let Some(plan) = plan else {
            return Err(crate::tr(
                "Internal error: command-block export is missing the sound-source plan.",
                "内部错误：命令方块导出缺少声源分配方案。",
            )
            .to_string());
        };
        if let Some((_, msg)) = command_export_check(plan, stats, opts) {
            return Err(msg.into());
        }
        Ok(())
    } else {
        export_guard(stats, opts)
    }
}

/// 解压并读出文件里真实存在的尺寸/音符统计，供界面与测试核对。
///
/// 两种格式都读：经典 `.schematic` 用 Width/Height/Length + TileEntities，
/// 结构方块文件用 size + palette/blocks 联表。
/// 结构文件里 `size[0]` 是原版硬编码的 32（不是真实长度），这里照文件读。
pub fn inspect_schematic(path: &Path) -> Result<SchematicInfo, String> {
    let packed = std::fs::read(path).map_err(|e| {
        crate::tr(
            &format!("Failed to open file: {e}"),
            &format!("打开文件失败: {e}"),
        )
        .to_string()
    })?;
    Ok(inspect_bytes(&packed)?)
}

/// 输入是 gzip 后的文件字节。
pub fn inspect_bytes(d: &[u8]) -> Result<SchematicInfo, String> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut raw = Vec::new();
    GzDecoder::new(d).read_to_end(&mut raw).map_err(|e| {
        crate::tr(
            &format!("gzip decompression failed: {e}"),
            &format!("gzip 解压失败: {e}"),
        )
        .to_string()
    })?;
    let (_, root) = crate::nbt::parse_named(&raw)?;
    read_info(&root)
}

fn read_info(root: &crate::nbt::Value) -> Result<SchematicInfo, String> {
    use crate::nbt::Value;
    let mut info = SchematicInfo::default();
    if let Some(v) = root.get("Width").and_then(Value::as_i16) {
        info.width = v.max(0) as usize;
        info.height = root
            .get("Height")
            .and_then(Value::as_i16)
            .unwrap_or(0)
            .max(0) as usize;
        info.length = root
            .get("Length")
            .and_then(Value::as_i16)
            .unwrap_or(0)
            .max(0) as usize;
        // 经典格式：note block 是 TileEntities 里 id 为 Music/minecraft:noteblock 的条目。
        for te in root
            .get("TileEntities")
            .and_then(Value::as_list)
            .unwrap_or((0, &[]))
            .1
        {
            match te.get("id").and_then(Value::as_str) {
                Some("Music") | Some("minecraft:noteblock") => info.note_blocks += 1,
                _ => {}
            }
        }
        // 中继器只能从 Blocks 字节数组里数（id 93/94）。
        if let Some(blocks) = root.get("Blocks").and_then(Value::as_bytes) {
            info.repeaters = blocks
                .iter()
                .filter(|&&b| b == REPEATER || b == REPEATER_POWERED)
                .count();
        }
        return Ok(info);
    }
    if let Some((_, size)) = root.get("size").and_then(Value::as_list) {
        let get = |i: usize| size.get(i).and_then(Value::as_i32).unwrap_or(0).max(0) as usize;
        info.width = get(0);
        info.height = get(1);
        info.length = get(2);
        let palette = structure_palette(root);
        for b in root
            .get("blocks")
            .and_then(Value::as_list)
            .unwrap_or((0, &[]))
            .1
        {
            let Some(state) = b.get("state").and_then(Value::as_i32) else {
                continue;
            };
            let Some(entry) = palette.get(state.max(0) as usize).copied() else {
                continue;
            };
            let name = entry.get("Name").and_then(Value::as_str).unwrap_or("");
            if name.ends_with("note_block") || name.ends_with("command_block") {
                info.note_blocks += 1;
            } else if name.ends_with("repeater") {
                info.repeaters += 1;
            }
        }
        return Ok(info);
    }
    Err(crate::tr(
        "Neither Width/Height/Length nor size found; not a Schematic or structure file",
        "文件里既没有 Width/Height/Length，也没有 size，不是 Schematic 或结构文件",
    )
    .into())
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

// ---------------------------------------------------------------------- 导入
//
// 逐条对齐 `open_schematic.gml` + `read_tags.gml`。
//
// 三处刻意偏离原版，写在明面上免得被当成漏实现：
// - 原版在解析文件之前就先 `reset()`，坏文件会把用户当前的歌清空；这里只在解析
//   成功后才替换歌曲（与本仓库 MIDI 导入的既有做法一致）。
// - 原版的 `filename = fn` 会把工程文件名指向 `.schematic`，此时按 Ctrl+S 会用
//   NBS 覆盖原理图；这里同样不设 `filename`，交给「另存为」。
// - 原版末尾把 `debugstr` 那份传播过程 `clipboard_set_text()` 覆盖到系统剪贴板
//   （调试残留）。同一份文本放在 `SchematicImport::trace` 里，由测试断言。

use nbs_core::model::{Note, KEY_MAX};
use std::collections::HashMap;

/// 原版 `obj_dummy` 里的 `t_AIR..t_INPUT`。
const T_AIR: u8 = 0;
const T_WIRE: u8 = 1;
const T_NOTE: u8 = 2;
const T_REPEATER: u8 = 3;
const T_TORCH: u8 = 4;
const T_BLOCK: u8 = 5;
const T_INPUT: u8 = 6;

/// 原版 `typestr[]`，只出现在调试文本里。
fn typestr(kind: u8) -> &'static str {
    match kind {
        T_WIRE => "wire",
        T_NOTE => "note block",
        T_REPEATER => "repeater",
        T_TORCH => "torch",
        T_BLOCK => "block",
        T_INPUT => "input",
        _ => "nothing",
    }
}

/// 原版 `id_isblock()`：能挡线、也能被信号点亮的实体方块。
/// 20（玻璃）、23、25（音符盒自己）、55、69、75、76、93、94 都不在这张表里。
pub fn id_isblock(id: u8) -> bool {
    matches!(
        id,
        1..=5 | 7 | 12..=17 | 19 | 21 | 22 | 24 | 35 | 41..=45 | 47..=49 | 56..=58
            | 73 | 74 | 82 | 86..=89 | 97..=100 | 103
    )
}

/// 原版 `block_get_ins()`：音符盒下面的方块决定乐器，表里没列出来的一律是 0 号竖琴。
/// 这张表只覆盖 1.8 以前的 10 种乐器，所以导出用的 10~15 号（铁片琴、牛铃……）
/// 回读会落成竖琴 —— 原版如此。
pub fn block_get_ins(id: u8) -> u8 {
    match id {
        5 | 17 | 25 | 47 | 53 | 54 | 56 | 84 | 85 | 107 => 1,
        1 | 4 | 7 | 14 | 15 | 16 | 21 | 22 | 23 | 24 | 43 | 44 | 45 | 48 | 49 | 61 | 62 | 67
        | 73 | 74 | 87 | 89 | 97 | 98 | 108 | 109 => 2,
        12 | 13 | 88 => 3,
        20 => 4,
        35 => 5,
        82 => 6,
        41 => 7,
        174 => 8,
        216 => 9,
        _ => 0,
    }
}

/// 原版 `get_repeater_delay(data) = data div 4`。
fn repeater_delay(data: u8) -> i64 {
    (data / 4) as i64
}

/// 原版 `get_repeater_direction(data) = data mod 4`。
fn repeater_direction(data: u8) -> usize {
    (data % 4) as usize
}

/// 原版把三维网格压成一个整数：`t = xx*2000*256 + zz*2000 + yy`，再用
/// `t div 32000` / `t mod 32000` 当稀疏二维数组的两个下标。这里直接拿 `t` 作键，
/// 越界坐标互相覆盖的行为与原版一致（原版也是靠这个整数复用格子）。
fn cell_t(xx: i64, yy: i64, zz: i64) -> i64 {
    xx * 512000 + zz * 2000 + yy
}

#[derive(Clone, Copy, Default)]
struct Cell {
    block: u8,
    data: u8,
    kind: u8,
    added: bool,
}

/// 原版 `sch_block` / `sch_data` / `sch_type` / `sch_added` 四张稀疏表。
/// 没写过的格子读出来是 0，与 GameMaker 数组的默认值一致。
#[derive(Default)]
struct Sch {
    cells: HashMap<i64, Cell>,
    length: i64,
    width: i64,
    height: i64,
}

impl Sch {
    fn at(&self, t: i64) -> Cell {
        self.cells.get(&t).copied().unwrap_or_default()
    }
    /// 原版那句边界检查：`tx < 0 || ty < 0 || tz < 0 || tx >= sch_length ||
    /// ty >= sch_width || tz >= sch_height`。
    fn inside(&self, xx: i64, yy: i64, zz: i64) -> bool {
        !(xx < 0 || yy < 0 || zz < 0 || xx >= self.length || yy >= self.width || zz >= self.height)
    }
    fn insert(&mut self, t: i64, cell: Cell) {
        self.cells.insert(t, cell);
    }
    fn set_data(&mut self, t: i64, data: u8) {
        self.cells.entry(t).or_default().data = data;
    }
    /// TileEntities 覆盖：只改方块/数据/类型，`sch_added` 保持原值。
    fn overlay_note(&mut self, t: i64, note: i64) {
        let c = self.cells.entry(t).or_default();
        c.block = NOTE_BLOCK;
        c.data = note as u8;
        c.kind = T_NOTE;
    }
    fn mark(&mut self, t: i64) {
        self.cells.entry(t).or_default().added = true;
    }
}

/// 原版 `read_tags.gml` 的状态机。
///
/// 原版的取值硬绑在标签类型上（Width 只认 TAG_Short、x/y/z/WEOffset 只认 TAG_Int、
/// note 只认 TAG_Byte），写成别的类型就被当普通数据跳过去，这里照抄。
#[derive(Default)]
struct Tags {
    width: i64,
    length: i64,
    height: i64,
    blocks: Option<Vec<u8>>,
    data: Option<Vec<u8>>,
    /// 原版 `WEOffsetX/Y/Z`：WorldEdit 复制时的原点。
    offsetx: i64,
    offsety: i64,
    offsetz: i64,
    /// 原版 `nb_n`：条目之间不清零，只有被收走时才置回 -1。于是「没有 note 标签的
    /// 方块实体」（例如命令方块）会沿用上一个音符盒的音高，「没给坐标的条目」会沿用
    /// 上一组的坐标 —— 原版的既有行为。
    nb_n: i64,
    nb_x: i64,
    nb_y: i64,
    nb_z: i64,
    /// 原版 `noteblock_x/y/z/n`。
    notes: Vec<(i64, i64, i64, i64)>,
}

impl Tags {
    fn new() -> Self {
        Self {
            nb_n: -1,
            ..Default::default()
        }
    }

    /// 按文件顺序走一遍标签树；`name` 是当前标签的名字（列表元素没有名字，传空串）。
    fn walk(&mut self, name: &str, v: &crate::nbt::Value) {
        use crate::nbt::Value;
        match v {
            Value::Byte(b) => {
                // 原版是 `buffer_read_byte()`，无符号 0..255。
                if name == "note" {
                    self.nb_n = (*b as u8) as i64;
                }
            }
            Value::Short(s) => match name {
                "Width" => self.width = *s as i64,
                "Height" => self.height = *s as i64,
                "Length" => self.length = *s as i64,
                _ => {}
            },
            Value::Int(i) => match name {
                // 原版的轴映射：NBT x → 本网格的 yy（反向），y → zz（高度），z → xx。
                "x" => self.nb_y = self.width - 1 - *i as i64,
                "y" => self.nb_z = *i as i64,
                "z" => self.nb_x = *i as i64,
                "WEOffsetX" => self.offsetx = *i as i64,
                "WEOffsetY" => self.offsety = *i as i64,
                "WEOffsetZ" => self.offsetz = *i as i64,
                _ => {}
            },
            Value::ByteArray(b) => match name {
                // 原版每次命中都覆盖记录的位置，所以同名数组取文件里最后一个。
                "Blocks" => self.blocks = Some(b.clone()),
                "Data" => self.data = Some(b.clone()),
                _ => {}
            },
            Value::List(ty, items) => {
                // 原版只对元素类型 10（compound）递归，其余按字节跳掉。
                if *ty != 10 {
                    return;
                }
                for item in items {
                    self.walk("", item);
                    if name == "TileEntities" && self.nb_n > -1 {
                        self.notes
                            .push((self.nb_x, self.nb_y, self.nb_z, self.nb_n));
                        self.nb_n = -1;
                    }
                }
            }
            Value::Compound(fields) => {
                for (k, val) in fields {
                    self.walk(k, val);
                }
            }
            _ => {}
        }
    }
}

/// 原版 `open_schematic()` 的失败分支。这些对话框的标题在原版里就没翻译，照抄英文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// 读不到文件：原版在 `file_exists_lib()` 那步就静默 return，没有对话框。
    Io(String),
    /// gzip 解不开、NBT 结构坏了，或者根标签不叫 `Schematic`。
    NotASchematic,
    InvalidSize,
    NoBlocksArray,
    NoDataArray,
    NoNoteBlocks,
    NoInputs,
    NoNotesAdded,
}

impl ImportError {
    /// 原版 `message(正文, 标题)`；`Io` 在原版没有对话框。
    pub fn dialog(&self) -> Option<(&'static str, &'static str)> {
        match self {
            ImportError::Io(_) => None,
            ImportError::NotASchematic => Some((
                "Error",
                "Failed to load Schematic.\n\nERROR: Not a Schematic file.",
            )),
            ImportError::InvalidSize => {
                Some(("Error", "Failed to load Schematic.\n\nERROR: Invalid size."))
            }
            ImportError::NoBlocksArray => Some((
                "Error",
                "Failed to load Schematic.\n\nERROR: No Blocks array found.",
            )),
            ImportError::NoDataArray => Some((
                "Error",
                "Failed to load Schematic.\n\nERROR: No Data array found.",
            )),
            ImportError::NoNoteBlocks => {
                Some(("Error", "Couldn't find any note blocks in the Schematic!"))
            }
            ImportError::NoInputs => Some((
                "Error",
                "The Schematic did not include any supported inputs.\nSupported inputs are:\n\n* Buttons\n* Levers",
            )),
            ImportError::NoNotesAdded => Some((
                "Error",
                "None of the note blocks in the Schematic could be added. This could be to none of \
                 them being connected to any inputs (buttons or levers).",
            )),
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io(e) => write!(f, "{e}"),
            other => write!(f, "{}", other.dialog().unwrap_or(("", "")).1),
        }
    }
}

impl std::error::Error for ImportError {}

/// 一次成功的导入。
#[derive(Clone, Debug)]
pub struct SchematicImport {
    pub song: Song,
    /// 原版 `sch_width`（NBT 的 x 轴，本网格里的 yy）。
    pub width: i64,
    /// 原版 `sch_length`（NBT 的 z 轴，本网格里的 xx）。
    pub length: i64,
    /// 原版 `sch_height`（NBT 的 y 轴，本网格里的 zz）。
    pub height: i64,
    /// 原版 `inputam`：拉杆（69）+ 墙上按钮（77）的个数。大于 1 时原版会多提示一句。
    pub inputs: usize,
    /// 原版 `noteblocks`：文件里数到的音符盒。
    pub note_blocks: usize,
    /// 原版 `queuenoteblocks`：真正放上网格的音符盒。
    pub added_notes: usize,
    /// 原版 `debugstr` 那份传播过程，换行同样是 `\r\n`。
    pub trace: String,
}

pub fn import_schematic(path: &Path) -> Result<SchematicImport, ImportError> {
    let packed =
        std::fs::read(path).map_err(|e| ImportError::Io(format!("{}: {e}", path.display())))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    import_bytes(&packed, &stem)
}

/// `packed` 是磁盘上的原始字节（gzip 后的 `.schematic`），`stem` 只用于调试文本里
/// 那句原版开头打印的文件名（`filename_name(fn)`）。
pub fn import_bytes(packed: &[u8], stem: &str) -> Result<SchematicImport, ImportError> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut raw = Vec::new();
    // 原版先 `gzunzip()` 到临时文件再读；不是 gzip 就得到空文件或上一次的残留，
    // 最终都停在下面那句 "Schematic" 名字检查上，所以解压失败并到同一个错误里。
    if GzDecoder::new(packed).read_to_end(&mut raw).is_err() {
        return Err(ImportError::NotASchematic);
    }
    let (root_name, root) =
        crate::nbt::parse_named(&raw).map_err(|_| ImportError::NotASchematic)?;
    if root_name != "Schematic" {
        return Err(ImportError::NotASchematic);
    }
    let mut tags = Tags::new();
    tags.walk("", &root);

    if tags.width <= 0 || tags.length <= 0 || tags.height <= 0 {
        return Err(ImportError::InvalidSize);
    }
    let blocks = tags.blocks.take().ok_or(ImportError::NoBlocksArray)?;
    let data = tags.data.take().ok_or(ImportError::NoDataArray)?;
    if tags.notes.is_empty() {
        return Err(ImportError::NoNoteBlocks);
    }

    // 扫描顺序是原版的 `for zz { for xx { for yy = width-1 .. 0 } }`。
    // 原版不管数组多长都连读 length*width*height 个字节（读空了就啃后面的标签字节），
    // 这里读到数组末尾为止 —— 只有坏文件才有差别。
    let mut sch = Sch {
        cells: HashMap::new(),
        length: tags.length,
        width: tags.width,
        height: tags.height,
    };
    let plane = tags.length as usize * tags.width as usize;
    let wid = tags.width as usize;
    let total = plane.saturating_mul(tags.height as usize);
    let mut inputs: Vec<(i64, i64, i64)> = Vec::new();
    for (i, a) in blocks.into_iter().take(total).enumerate() {
        let zz = (i / plane) as i64;
        let xx = ((i % plane) / wid) as i64;
        let yy = wid as i64 - 1 - (i % wid) as i64;
        // 原版在 69/77 分支里只登记输入，没有把 type 设成 t_INPUT，格子类型仍是 AIR；
        // 但 `sch_block` 照样写成了 69/77，「拉杆压着方块」那条判断靠的就是这个 id。
        // 这决定了「红石线不会把按钮/拉杆当邻居传过去」，照抄。
        let mut kind = T_AIR;
        if a == LEVER || a == BUTTON_WALL {
            inputs.push((xx, yy, zz));
        } else {
            kind = if a == REDSTONE_WIRE {
                T_WIRE
            } else if a == NOTE_BLOCK {
                T_NOTE
            } else if a == REPEATER || a == REPEATER_POWERED {
                T_REPEATER
            } else if a == TORCHOFF || a == TORCHON {
                T_TORCH
            } else if id_isblock(a) {
                T_BLOCK
            } else {
                T_AIR
            };
        }
        sch.insert(
            cell_t(xx, yy, zz),
            Cell {
                block: a,
                data: 0,
                kind,
                added: false,
            },
        );
    }
    for (i, d) in data.into_iter().take(total).enumerate() {
        let zz = (i / plane) as i64;
        let xx = ((i % plane) / wid) as i64;
        let yy = wid as i64 - 1 - (i % wid) as i64;
        sch.set_data(cell_t(xx, yy, zz), d);
    }

    if inputs.is_empty() {
        return Err(ImportError::NoInputs);
    }

    // TileEntities 覆盖成音符盒（原版在 inputam 检查之后、入队之前做这一步）。
    for (nx, ny, nz, n) in &tags.notes {
        let t = cell_t(nx - tags.offsetx, ny - tags.offsetz, nz - tags.offsety);
        sch.overlay_note(t, *n);
    }

    let mut engine = Propagator::new(sch, stem, inputs.len());
    engine.run(&inputs);
    let Propagator {
        mut song,
        added,
        trace,
        ..
    } = engine;
    if added == 0 {
        return Err(ImportError::NoNotesAdded);
    }
    while song.layers.len() <= song.endb as usize {
        song.layers.push(Default::default());
    }
    // 原版 `reset()` 之后 `changed = 0`，导入的曲子不算改过。
    song.dirty = false;
    Ok(SchematicImport {
        song,
        width: tags.width,
        length: tags.length,
        height: tags.height,
        inputs: inputs.len(),
        note_blocks: tags.notes.len(),
        added_notes: added,
        trace,
    })
}

#[derive(Clone, Copy)]
struct QueueItem {
    /// 原版 `queuet`：这一格是什么。
    kind: u8,
    x: i64,
    y: i64,
    z: i64,
    /// 原版 `queuedel`：相对输入的延迟（tick）。
    delay: i64,
    /// 原版 `queueinput`：是谁把这一格排进队列的。
    from: u8,
}

/// 原版 `open_schematic` 的传播循环：一个 LIFO 队列（`queuelength` 就是栈顶指针），
/// 从每个输入出发把红石信号铺满整张电路。
struct Propagator {
    sch: Sch,
    song: Song,
    queue: Vec<QueueItem>,
    /// 原版 `queuenoteblocks`。
    added: usize,
    trace: String,
}

impl Propagator {
    fn new(sch: Sch, stem: &str, inputs: usize) -> Self {
        let trace = format!(
            "{stem}, width: {}, length: {}, height: {}, inputs: {inputs}",
            sch.width, sch.length, sch.height
        );
        Self {
            sch,
            song: Song::new(),
            queue: Vec::new(),
            added: 0,
            trace,
        }
    }

    /// 原版 `str += chr(13) + chr(10) + ...`。
    fn log(&mut self, s: &str) {
        self.trace.push_str("\r\n");
        self.trace.push_str(s);
    }

    /// 原版入队三件套：写队尾、`queuelength += 1`、把 `sch_added` 置 1，
    /// 外加一行 "Found ..." 调试文本。
    fn enqueue(&mut self, phrase: &str, item: QueueItem) {
        self.log(&format!("Found {} {phrase}", typestr(item.kind)));
        self.queue.push(item);
        self.sch.mark(cell_t(item.x, item.y, item.z));
    }

    /// 原版 `add_block(x, y, ins, key, vel, pan, pit)`：这一格已经有音符就返回 0，
    /// 于是 `while (!add_block(...)) b += 1` 变成「往上找第一个空格」。
    /// 音高原版过 `median(0, key, 87)`；力度/声像/微调固定 100/100/0，正是 `Note::new`。
    fn add_block(&mut self, tick: i64, layer: u32, instrument: u8, key: i64) -> bool {
        if self.song.has(tick as u32, layer) {
            return false;
        }
        // 原版的 `instrument_list[| a]`：本仓库的内置乐器顺序与那 10 项一致，不用换表。
        let key = key.clamp(0, KEY_MAX as i64) as u8;
        self.song
            .set(tick as u32, layer, Note::new(instrument, key));
        true
    }

    fn run(&mut self, inputs: &[(i64, i64, i64)]) {
        for (i, (ix, iy, iz)) in inputs.iter().enumerate() {
            // 原版 `start = obj_controller.enda + (16 * (i > 0))`：enda 跟着前一个
            // 输入的落子增长，所以多输入是被空 16 tick 首尾接起来的。
            let start = self.song.enda + if i > 0 { 16 } else { 0 };
            self.log(&format!("_____INPUT {}_____", i + 1));
            let t = cell_t(*ix, *iy, *iz);
            self.sch.mark(t);
            self.queue.clear();
            self.queue.push(QueueItem {
                kind: T_INPUT,
                x: *ix,
                y: *iy,
                z: *iz,
                delay: 0,
                from: T_AIR,
            });
            while let Some(item) = self.queue.pop() {
                self.step(start, item);
            }
        }
    }

    fn step(&mut self, start: u32, it: QueueItem) {
        let ct = it.kind;
        let (cx, cy, cz) = (it.x, it.y, it.z);
        let (cd, ci) = (it.delay, it.from);
        let t = cell_t(cx, cy, cz);
        match ct {
            T_WIRE => {
                self.log("__Wire__");
                let mut dir = [false; 4];
                for dz in -1..=1i64 {
                    // 第一遍：只认线、中继器、火把、输入。
                    for b in 0..4usize {
                        let tx = cx + (b == 1) as i64 - (b == 3) as i64;
                        let ty = cy + (b == 0) as i64 - (b == 2) as i64;
                        let tz = cz + dz;
                        if !self.sch.inside(tx, ty, tz) {
                            continue;
                        }
                        let nt = cell_t(tx, ty, tz);
                        let cell = self.sch.at(nt);
                        if cell.added {
                            continue;
                        }
                        match cell.kind {
                            T_WIRE => {
                                if dz == -1 {
                                    // 下面那根线：本层同一列被方块压住就上不来了。
                                    if self.sch.at(cell_t(tx, ty, cz)).kind == T_BLOCK {
                                        continue;
                                    }
                                } else if dz == 1 && cz < self.sch.height - 1 {
                                    // 上面那根线：本格头顶有方块就下不去。
                                    if self.sch.at(cell_t(cx, cy, cz + 1)).kind == T_BLOCK {
                                        continue;
                                    }
                                }
                                dir[b] = true;
                            }
                            T_REPEATER => {
                                // 只收「朝自己吹」的中继器，且不能是头顶那格。
                                if 3 - repeater_direction(cell.data) != b {
                                    continue;
                                }
                                if dz == 1 {
                                    continue;
                                }
                                if dz == 0 {
                                    dir[b] = true;
                                }
                            }
                            // 网格里的输入格子类型是 AIR（见扫描那段的注释），这支走不到。
                            T_TORCH | T_INPUT => dir[b] = true,
                            _ => continue,
                        }
                        self.enqueue(
                            "next to wire",
                            QueueItem {
                                kind: cell.kind,
                                x: tx,
                                y: ty,
                                z: tz,
                                delay: cd,
                                from: ct,
                            },
                        );
                    }
                }
                // 第二遍：音符盒和方块，但左右两邻向已经被线占用就不算这条边的延长线。
                for b in 0..4usize {
                    let tx = cx + (b == 1) as i64 - (b == 3) as i64;
                    let ty = cy + (b == 0) as i64 - (b == 2) as i64;
                    if !self.sch.inside(tx, ty, cz) {
                        continue;
                    }
                    let nt = cell_t(tx, ty, cz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind != T_NOTE && cell.kind != T_BLOCK {
                        continue;
                    }
                    if dir[(b + 1) % 4] || dir[(b + 3) % 4] {
                        continue;
                    }
                    self.enqueue(
                        "next to wire",
                        QueueItem {
                            kind: cell.kind,
                            x: tx,
                            y: ty,
                            z: cz,
                            delay: cd,
                            from: ct,
                        },
                    );
                }
                // 正下方那一格。
                loop {
                    let tz = cz - 1;
                    if tz < 0 {
                        break;
                    }
                    let nt = cell_t(cx, cy, tz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind != T_BLOCK {
                        break;
                    }
                    self.enqueue(
                        "below wire",
                        QueueItem {
                            kind: cell.kind,
                            x: cx,
                            y: cy,
                            z: tz,
                            delay: cd,
                            from: ct,
                        },
                    );
                    break;
                }
            }
            T_REPEATER => {
                self.log("__Repeater__");
                loop {
                    let b = (repeater_direction(self.sch.at(t).data) + 2) % 4;
                    let tx = cx + (b == 0) as i64 - (b == 2) as i64;
                    let ty = cy + (b == 1) as i64 - (b == 3) as i64;
                    if !self.sch.inside(tx, ty, cz) {
                        break;
                    }
                    let nt = cell_t(tx, ty, cz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind == T_AIR || cell.kind == T_TORCH {
                        break;
                    }
                    if cell.kind == T_REPEATER && (repeater_direction(cell.data) + 2) % 4 != b {
                        break;
                    }
                    // 延迟用的是自己这一格的 data（原版在算之前把 t 复位回本格）。
                    let delay = cd + repeater_delay(self.sch.at(t).data) + 1;
                    self.enqueue(
                        "next to repeater",
                        QueueItem {
                            kind: cell.kind,
                            x: tx,
                            y: ty,
                            z: cz,
                            delay,
                            from: ct,
                        },
                    );
                    break;
                }
            }
            T_BLOCK => {
                self.log("__Block__");
                loop {
                    let tz = cz - 1;
                    if tz < 0 {
                        break;
                    }
                    let nt = cell_t(cx, cy, tz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind != T_WIRE {
                        break;
                    }
                    if ci == T_WIRE {
                        break;
                    }
                    self.enqueue(
                        "below block",
                        QueueItem {
                            kind: cell.kind,
                            x: cx,
                            y: cy,
                            z: tz,
                            delay: cd,
                            from: ct,
                        },
                    );
                    break;
                }
                loop {
                    let tz = cz + 1;
                    if tz >= self.sch.height {
                        break;
                    }
                    let nt = cell_t(cx, cy, tz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind != T_TORCH && cell.kind != T_WIRE {
                        break;
                    }
                    if ci == T_WIRE && cell.kind == T_WIRE {
                        break;
                    }
                    // 只有朝上的火把（data 5）才算被方块点亮。
                    if cell.kind == T_TORCH && cell.data != 5 {
                        break;
                    }
                    self.enqueue(
                        "above block",
                        QueueItem {
                            kind: cell.kind,
                            x: cx,
                            y: cy,
                            z: tz,
                            delay: cd,
                            from: ct,
                        },
                    );
                    break;
                }
                for b in 0..4usize {
                    let tx = cx + (b == 1) as i64 - (b == 3) as i64;
                    let ty = cy + (b == 0) as i64 - (b == 2) as i64;
                    if !self.sch.inside(tx, ty, cz) {
                        continue;
                    }
                    let nt = cell_t(tx, ty, cz);
                    let cell = self.sch.at(nt);
                    if cell.added {
                        continue;
                    }
                    match cell.kind {
                        T_TORCH => {
                            // 火把朝向：1 朝 +yy、2 朝 +xx、3 朝 -yy、4 朝 -xx，其余不算。
                            let want = match cell.data {
                                1 => 2,
                                2 => 0,
                                3 => 1,
                                4 => 3,
                                _ => continue,
                            };
                            if b != want {
                                continue;
                            }
                        }
                        T_REPEATER => {
                            if repeater_direction(cell.data) != 3 - b {
                                continue;
                            }
                        }
                        T_WIRE => {
                            if ci == T_WIRE {
                                continue;
                            }
                        }
                        _ => {
                            if cell.kind != T_NOTE {
                                continue;
                            }
                        }
                    }
                    self.enqueue(
                        "next to block",
                        QueueItem {
                            kind: cell.kind,
                            x: tx,
                            y: ty,
                            z: cz,
                            delay: cd,
                            from: ct,
                        },
                    );
                }
            }
            T_NOTE => {
                self.log("__Note block__");
                // 音符盒下面的方块决定乐器；原版这里还有一段「旁边有带电就静音」的
                // 检查，但整段被注释掉了，所以不实现。
                let below = cell_t(cx, cy, cz - 1);
                let ins = if cz > -1 {
                    block_get_ins(self.sch.at(below).block)
                } else {
                    0
                };
                let blocked =
                    cz + 1 < self.sch.height && self.sch.at(cell_t(cx, cy, cz + 1)).kind == T_BLOCK;
                if blocked {
                    self.log("Didn't add note block.");
                } else {
                    let key = self.sch.at(t).data as i64 + 33;
                    self.added += 1;
                    self.log(&format!("Added note block no. {}!", self.added));
                    let mut layer = 0u32;
                    while !self.add_block(start as i64 + cd, layer, ins, key) {
                        layer += 1;
                    }
                }
            }
            T_TORCH => {
                self.log("__Torch__");
                loop {
                    let tz = cz - 1;
                    if tz < 0 {
                        break;
                    }
                    let nt = cell_t(cx, cy, tz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind != T_WIRE {
                        break;
                    }
                    self.enqueue(
                        "below torch",
                        QueueItem {
                            kind: cell.kind,
                            x: cx,
                            y: cy,
                            z: tz,
                            delay: cd + 1,
                            from: ct,
                        },
                    );
                    break;
                }
                loop {
                    let tz = cz + 1;
                    if tz >= self.sch.height {
                        break;
                    }
                    let nt = cell_t(cx, cy, tz);
                    let cell = self.sch.at(nt);
                    if cell.added || cell.kind != T_BLOCK && cell.kind != T_NOTE {
                        break;
                    }
                    self.enqueue(
                        "above torch",
                        QueueItem {
                            kind: cell.kind,
                            x: cx,
                            y: cy,
                            z: tz,
                            delay: cd + 1,
                            from: ct,
                        },
                    );
                    break;
                }
                for b in 0..4usize {
                    let tx = cx + (b == 1) as i64 - (b == 3) as i64;
                    let ty = cy + (b == 0) as i64 - (b == 2) as i64;
                    if !self.sch.inside(tx, ty, cz) {
                        continue;
                    }
                    let nt = cell_t(tx, ty, cz);
                    let cell = self.sch.at(nt);
                    if cell.added {
                        continue;
                    }
                    if cell.kind == T_REPEATER {
                        if repeater_direction(cell.data) != 3 - b {
                            continue;
                        }
                    } else if cell.kind != T_WIRE {
                        continue;
                    }
                    self.enqueue(
                        "next to torch",
                        QueueItem {
                            kind: cell.kind,
                            x: tx,
                            y: ty,
                            z: cz,
                            delay: cd + 1,
                            from: ct,
                        },
                    );
                }
            }
            T_INPUT => {
                self.log("__Input__");
                loop {
                    let tz = cz - 1;
                    if tz < 0 {
                        break;
                    }
                    let nt = cell_t(cx, cy, tz);
                    let cell = self.sch.at(nt);
                    if cell.added {
                        break;
                    }
                    if cell.kind != T_BLOCK && cell.kind != T_WIRE {
                        break;
                    }
                    // 拉杆要压着方块才算数（原版只认 id 69，按钮 77 不走这条）。
                    if cell.kind == T_BLOCK && self.sch.at(t).block != LEVER {
                        break;
                    }
                    self.enqueue(
                        "below input",
                        QueueItem {
                            kind: cell.kind,
                            x: cx,
                            y: cy,
                            z: tz,
                            delay: cd,
                            from: ct,
                        },
                    );
                    break;
                }
                for b in 0..4usize {
                    let tx = cx + (b == 1) as i64 - (b == 3) as i64;
                    let ty = cy + (b == 0) as i64 - (b == 2) as i64;
                    if !self.sch.inside(tx, ty, cz) {
                        continue;
                    }
                    let nt = cell_t(tx, ty, cz);
                    let cell = self.sch.at(nt);
                    // 原版先看 type = 0（输入格子自己也是 0），再看 added。
                    if cell.kind == T_AIR || cell.added {
                        continue;
                    }
                    if cell.kind == T_NOTE || cell.kind == T_TORCH {
                        continue;
                    }
                    self.enqueue(
                        "next to input",
                        QueueItem {
                            kind: cell.kind,
                            x: tx,
                            y: ty,
                            z: cz,
                            delay: cd,
                            from: ct,
                        },
                    );
                }
            }
            // 原版的 if 链没有 else：弹到 AIR/未知类型什么都不做。
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::Value;
    use nbs_core::model::{CustomInstrument, Layer, LockMode, Note, Song};

    /// 每条都对着创世神 jar 里的 `SAFE_FILENAME_REGEX` 实跑过一遍（2026-11-21）：
    /// 中文名被 `RecursiveDirectoryWatcher` 记成 `Invalid file name detected`，
    /// 所以下面这批判定不是猜的，是照那台模组自己的正则复现。
    #[test]
    fn worldedit_filename_whitelist_follows_the_mod() {
        let accepted = [
            "untitled.schem",
            "wujiande_zhong_v3.schem",
            "song (1).schem",
            "a~b!c%s.schem",
            "a-b_c.d.schem",
            "n0.5.schem",
            "x[s].schem",
            "a+.schem",
            "a{}.schem",
            "a,?.schem",
            "a'.schem",
            "a@.schem",
        ];
        let rejected = [
            "无间的钟_v3.schem",
            "MAESTRO 节目单.schem",
            "no_ext",
            ".schem",
            "a#.schem",
            "a&b.schem",
            "a;b.schem",
            "a=b.schem",
            "a<b.schem",
            // 正则里 `:` 同样不在字符类，但 Windows 文件名根本容不下它，就不拿来当样本了。
            "a\u{e9}.schem",
            "a.schem~",
            "a|.schem",
            "a\tb.schem",
        ];
        for name in accepted {
            // 挂在目录下取 file_name，判的是文件名而不是整条路径（同模组的用法）。
            let path = Path::new("schematics").join(name);
            assert!(!worldedit_rejects_filename(&path), "{name} 该被创世神接受");
        }
        for name in rejected {
            let path = Path::new("schematics").join(name);
            assert!(worldedit_rejects_filename(&path), "{name} 该被创世神拒绝");
        }
    }

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

    /// 经典 1.11-1.12 格式选项（出厂 format=0 是结构方块，classic 测试要显式切过去）。
    fn classic_options() -> SchematicOptions {
        let mut o = SchematicOptions::default();
        o.set_format(2);
        o
    }

    /// 钉住中文文案：串行化语言敏感断言，退出时恢复原语言。
    struct CnGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>, u8);
    impl Drop for CnGuard {
        fn drop(&mut self) {
            crate::set_language(self.1);
        }
    }

    fn pin_cn() -> CnGuard {
        let lock = crate::TEST_LANG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = crate::lang();
        crate::set_language(1);
        CnGuard(lock, prev)
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
        // control_create.gml:638 出厂 `structure = 1`，format 默认 0（reset 表 42 行）。
        assert_eq!(o.structure, true);
        assert_eq!(o.format, 0);
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
        o.command_source = "music".into();
        o.command_allowed_sources = [false; 10];
        o.command_tempo_grid = true;
        o.format = 1;
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
        // gml:66-68：声源三项在默认值表里；gml:42：format 重置为 0。
        assert_eq!(o.command_source, "block");
        assert_eq!(
            o.command_allowed_sources,
            crate::mc_export::default_allowed_sources()
        );
        assert_eq!(o.command_tempo_grid, false);
        assert_eq!(o.format, 0);
    }

    #[test]
    fn set_format_derives_structure_and_minecraft_old() {
        // menu_click.gml:254-256：structure = (sel <= 1)，old = (sel = 3)。
        let mut o = SchematicOptions::default();
        o.set_format(1);
        assert!(o.structure && o.format == 1 && !o.minecraft_old);
        o.set_format(3);
        assert!(!o.structure && o.minecraft_old && o.format == 3);
        o.set_format(2);
        assert!(!o.structure && !o.minecraft_old && o.format == 2);
        o.set_format(7);
        assert!(
            !o.structure && o.minecraft_old && o.format == 3,
            "越界下拉值夹到 3"
        );
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
        // 环形过道（layout 0）：宽 +4
        o.layout = 0;
        assert_eq!(schematic_width(&o), 17);
        assert_eq!(schematic_length(&st, &o), 2 + 3);
        // 行数为偶数才补 3（原版 `3*!(rows mod 2)`）：3 个中继器 / 每行 2 → 2 行。
        o.notesperrow = 2;
        assert_eq!(schematic_length(&st, &o), 2 + 2 * 3 + 3);
        o.notesperrow = 1;
        assert_eq!(schematic_length(&st, &o), 2 + 3 * 3);
        o.notesperrow = 5;
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
        let _cn = pin_cn();
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
        assert_eq!(
            ids(full.clone())
                .split(' ')
                .filter(|s| s.contains(':'))
                .count(),
            3
        );
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
        let nbt = build_nbt(&s, &classic_options(), None).unwrap();
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
        assert_eq!(
            root.get("Data").and_then(Value::as_bytes).unwrap().len(),
            blocks.len()
        );
        let version = root.get("Version").unwrap();
        assert_eq!(keys_of(version), ["Id", "Name", "Snapshot"]);
        assert_eq!(version.get("Id").and_then(Value::as_i32), Some(922));
        assert_eq!(version.get("Name").and_then(Value::as_str), Some("1.11.2"));
    }

    #[test]
    fn classic_tile_entities_carry_sign_then_notes() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50)]);
        let nbt = build_nbt(&s, &classic_options(), None).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let (ty, tes) = root.get("TileEntities").unwrap().as_list().unwrap();
        assert_eq!(ty, 10);
        assert_eq!(tes.len(), 3); // 1 个签名 + 2 个音符盒
        assert_eq!(
            keys_of(&tes[0]),
            ["id", "x", "y", "z", "Text1", "Text2", "Text3", "Text4"]
        );
        assert_eq!(
            tes[0].get("id").and_then(Value::as_str),
            Some("minecraft:sign")
        );
        assert_eq!(
            tes[0].get("Text4").and_then(Value::as_str),
            Some("{\"text\": \"Block Studio\"}")
        );
        assert_eq!(keys_of(&tes[1]), ["id", "x", "y", "z", "note"]);
        assert_eq!(
            tes[1].get("id").and_then(Value::as_str),
            Some("minecraft:noteblock")
        );
        assert_eq!(tes[1].get("note").and_then(Value::as_i8), Some(45 - 33));
        assert_eq!(tes[2].get("note").and_then(Value::as_i8), Some(50 - 33));
        // 关掉循环、非环形过道 -> 只有一个签名；没开矿车 -> 没有箱子。
    }

    #[test]
    fn classic_minecraft_old_uses_legacy_ids_and_plain_text() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = classic_options();
        o.set_format(3);
        let nbt = build_nbt(&s, &o, None).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let tes = items(root.get("TileEntities").unwrap());
        assert_eq!(tes[0].get("id").and_then(Value::as_str), Some("Sign"));
        assert_eq!(
            tes[0].get("Text1").and_then(Value::as_str),
            Some("Song generated")
        );
        assert_eq!(tes[1].get("id").and_then(Value::as_str), Some("Music"));
        let version = root.get("Version").unwrap();
        assert_eq!(keys_of(version), ["Name", "Snapshot"]);
        assert_eq!(version.get("Name").and_then(Value::as_str), Some("1.8"));
    }

    #[test]
    fn classic_minecart_entity_has_the_original_fields() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = classic_options();
        o.minecart = true;
        let nbt = build_nbt(&s, &o, None).unwrap();
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
        assert_eq!(
            ents[0].get("id").and_then(Value::as_str),
            Some("minecraft:minecart")
        );
        assert_eq!(ents[0].get("Air").and_then(Value::as_i16), Some(300));
        assert_eq!(ents[0].get("Fire").and_then(Value::as_i16), Some(-1));
        assert_eq!(items(ents[0].get("Motion").unwrap()).len(), 3);
        // 矿车打开后箱子才会出现（chest 依赖 minecart），TileEntities 多一项。
        let tes = items(root.get("TileEntities").unwrap());
        assert_eq!(
            tes.last().unwrap().get("id").and_then(Value::as_str),
            Some("minecraft:chest")
        );
        assert_eq!(items(tes.last().unwrap().get("Items").unwrap()).len(), 27);
    }

    #[test]
    fn structure_root_follows_the_original_order_and_palette_size() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        let nbt = build_nbt(&s, &o, None).unwrap();
        let (name, root) = crate::nbt::parse_named(&nbt).unwrap();
        assert_eq!(name, "");
        assert_eq!(
            keys_of(&root),
            ["DataVersion", "size", "palette", "entities", "blocks"]
        );
        assert_eq!(root.get("DataVersion").and_then(Value::as_i32), Some(1519));
        let size = items(root.get("size").unwrap());
        assert_eq!(size.len(), 3);
        // 原版把 size[0] 硬编码成 32，第三维超过 32 也被截成 32。
        assert_eq!(size[0].as_i32(), Some(32));
        assert_eq!(size[2].as_i32(), Some(5));
        let (ty, pal) = root.get("palette").unwrap().as_list().unwrap();
        assert_eq!(ty, 10);
        // 20 个内置乐器（v6 铜块一族）：palette 条数 = 29 + insnum * 26，
        // 与装机版自己导出的参照文件一致。
        assert_eq!(pal.len(), 29 + 20 * 26);
        assert_eq!(keys_of(&pal[0]), ["Name"]);
        assert_eq!(
            pal[0].get("Name").and_then(Value::as_str),
            Some("minecraft:stone")
        );
        assert_eq!(
            pal[3].get("Name").and_then(Value::as_str),
            Some("minecraft:dirt")
        );
        // 第 9 个音色方块（索引 3+9）带 axis 属性，其余不带。
        let axis = pal[3 + 9].get("Properties").unwrap();
        assert_eq!(axis.get("axis").and_then(Value::as_str), Some("y"));
        assert!(pal[3 + 8].get("Properties").is_none());
        // 铜块一族的 4 行方块名逐字取自参照文件 palette[19..23]。
        assert_eq!(
            pal[3 + 16..3 + 20]
                .iter()
                .map(|p| p.get("Name").and_then(Value::as_str).unwrap())
                .collect::<Vec<_>>(),
            [
                "minecraft:waxed_copper_block",
                "minecraft:waxed_exposed_copper",
                "minecraft:waxed_weathered_copper",
                "minecraft:waxed_oxidized_copper"
            ]
        );
        // 音符盒调色板从 3+insnum 开始，第一项是 harp/note 0。
        let nb = &pal[3 + 20];
        assert_eq!(
            nb.get("Name").and_then(Value::as_str),
            Some("minecraft:note_block")
        );
        let props = nb.get("Properties").unwrap();
        assert_eq!(keys_of(props), ["instrument", "note", "powered"]);
        assert_eq!(
            props.get("instrument").and_then(Value::as_str),
            Some("harp")
        );
        assert_eq!(props.get("note").and_then(Value::as_str), Some("0"));
        // 尾部 26 项的第一项是蓝色羊毛，最后一项是朝北亮着的红石墙火把。
        let tail = &pal[pal.len() - 26];
        assert_eq!(
            tail.get("Name").and_then(Value::as_str),
            Some("minecraft:blue_wool")
        );
        let last = pal.last().unwrap();
        assert_eq!(
            last.get("Name").and_then(Value::as_str),
            Some("minecraft:redstone_wall_torch")
        );
        assert_eq!(
            last.get("Properties")
                .unwrap()
                .get("facing")
                .and_then(Value::as_str),
            Some("north")
        );
    }

    #[test]
    fn size_header_follows_notesperrow_and_litematica() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        // notesperrow=5 时宽 5*2+3=13 < 32：size[0] 就是真实宽度，不是硬编码 32。
        o.notesperrow = 5;
        let nbt = build_nbt(&s, &o, None).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let size = items(root.get("size").unwrap());
        assert_eq!(size[0].as_i32(), Some(13));
        assert_eq!(size[0].as_i32(), Some(built2_size(&s, &o).0));
        // format=1（Litematica）写精确三围；同歌默认格式宽 43 会被截成 32。
        o.notesperrow = 20;
        o.format = 1;
        let nbt = build_nbt(&s, &o, None).unwrap();
        let root = crate::nbt::parse(&nbt).unwrap();
        let size = items(root.get("size").unwrap());
        let (w, h, l) = built2_size(&s, &o);
        assert_eq!(
            size.iter().map(|v| v.as_i32()).collect::<Vec<_>>(),
            vec![Some(w), Some(h), Some(l)]
        );
        assert_eq!(size[0].as_i32(), Some(43), "Litematica 不截 32");
    }

    /// 换个写法拿尺寸，避免依赖 Builder 的私有构造。
    fn built2_size(s: &Song, o: &SchematicOptions) -> (i32, i32, i32) {
        let size = calc_size_with(&calculate_stats(s), o, None);
        (size.wid as i32, size.hei as i32, size.len as i32)
    }

    #[test]
    fn plan_aware_size_functions_use_the_command_plan() {
        let s = song_with(&[(0, 0, 0, 45), (2, 0, 0, 45), (4, 1, 0, 45)]);
        let st = calculate_stats(&s);
        let mut o = SchematicOptions::default();
        o.structure = true;
        o.command_block = true;
        o.layout = 0;
        let plan = crate::mc_export::build_plan(
            &s,
            "command",
            "block",
            &crate::mc_export::default_allowed_sources(),
            true,
            true,
            true,
            false,
            10.0,
            false,
            false,
            true,
            1.0,
            &|_| None,
            &|_| None,
        );
        assert!(!plan.rows.is_empty(), "真实方案才测得准");
        // 原版 schematic_height.gml：4 + 3*ceil(maximum/4)，maximum 取方案的 maximum_slots。
        let hei = schematic_height_with(&st, &o, Some(&plan));
        assert_eq!(hei, 4 + 3 * ceil_div(plan.maximum_slots, 4));
        // 证明函数真消费方案：抬最大槽数，高度跟着抬一层（3 格）。
        let mut bigger = plan.clone();
        bigger.maximum_slots += 4;
        assert_eq!(schematic_height_with(&st, &o, Some(&bigger)), hei + 3);
        // 原版 schematic_length.gml：2 + ceil(中继器/每行数)*3，偶数圈再补 3。
        let rep = crate::mc_export::plan_repeaters(&plan, o.tempo_mode);
        let rows = o.notesperrow;
        let expect = 2 + ceil_div(rep, rows) * 3 + if ceil_div(rep, rows) % 2 == 0 { 3 } else { 0 };
        assert_eq!(schematic_length_with(&st, &o, Some(&plan)), expect);
        // 原版 schematic_export.gml:73：层数 = ceil(maximum_slots / 4)。
        let size = calc_size_with(&st, &o, Some(&plan));
        assert_eq!(size.layers, ceil_div(plan.maximum_slots, 4));
        assert_eq!(size.hei, hei);
        assert_eq!(size.len, schematic_length_with(&st, &o, Some(&plan)));
    }

    #[test]
    fn structure_block_entries_skip_note_blocks_and_chests() {
        let s = song_with(&[(0, 0, 0, 45)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        o.minecart = true;
        let built = build(&s, &o, None);
        let nbt = built.structure_nbt();
        let root = crate::nbt::parse(&nbt).unwrap();
        let blocks = items(root.get("blocks").unwrap());
        // 装机版：网格扫描跳过已写过牌子的格子，所以条数正好等于网格体积。
        let size = items(root.get("size").unwrap());
        let s2 = size.len();
        assert_eq!(s2, 3);
        assert_eq!(
            blocks.len(),
            built.size.hei * built.size.len * built.size.wid
        );
        // 第一个条目是签名方块：nbt -> pos -> state。
        assert_eq!(keys_of(&blocks[0]), ["nbt", "pos", "state"]);
        assert_eq!(
            blocks[0]
                .get("nbt")
                .unwrap()
                .get("id")
                .and_then(Value::as_str),
            Some("minecraft:sign")
        );
        // 音符盒条目不带 nbt，state = 3 + insnum + ins*25 + note。
        let states: Vec<i32> = blocks
            .iter()
            .filter_map(|b| b.get("state").and_then(Value::as_i32))
            .collect();
        assert!(
            states.contains(&(3 + 20 + (0 * 25 + (45 - 33)))),
            "{states:?}"
        );
        // 每个格子只出现一次：牌格不再被网格扫描重复写成 air。
        let cells: std::collections::HashSet<Vec<i32>> = blocks
            .iter()
            .map(|b| {
                items(b.get("pos").unwrap())
                    .iter()
                    .filter_map(Value::as_i32)
                    .collect()
            })
            .collect();
        assert_eq!(cells.len(), blocks.len(), "有格子被写了两次");
        // 矿车实体：pos -> blockPos -> 无名 compound。
        let ents = items(root.get("entities").unwrap());
        assert_eq!(ents.len(), 1);
        assert_eq!(keys_of(&ents[0]), ["pos", "blockPos", ""]);
    }

    /// 铜块一族（16..19）的音符必须落在自己那 25 行 trumpet 上；`insnum`
    /// 还写死 16 的时候，这两条会串到 palette 尾部的通用方块上去。
    /// 行名与参照文件 palette[23 + ins*25] 的 `instrument=` 逐字一致。
    #[test]
    fn copper_notes_land_on_their_own_trumpet_rows() {
        let s = song_with(&[(0, 0, 16, 45), (1, 0, 19, 57)]);
        let mut o = SchematicOptions::default();
        o.structure = true;
        let built = build(&s, &o, None);
        let root = crate::nbt::parse(&built.structure_nbt()).unwrap();
        let pal = items(root.get("palette").unwrap());
        let instrument_of = |state: usize| -> String {
            pal[state]
                .get("Properties")
                .and_then(|p| p.get("instrument"))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        };
        // note = key - 33：45 → 12，57 → 24。
        let want = [
            (3 + 20 + 16 * 25 + 12, "trumpet"),
            (3 + 20 + 19 * 25 + 24, "trumpet_oxidized"),
        ];
        let states: Vec<i32> = items(root.get("blocks").unwrap())
            .iter()
            .filter(|b| b.get("nbt").is_none())
            .filter_map(|b| b.get("state").and_then(Value::as_i32))
            .collect();
        for (state, name) in want {
            assert_eq!(instrument_of(state as usize), name, "palette 第 {state} 行");
            assert!(states.contains(&state), "音符没写到 {state}：{states:?}");
        }
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
        // 原版 TileEntity 的 Command 来自声源分配方案（schematic_export.gml:1014），
        // 不在导出时合成——用一份真实方案喂给生成器。
        let plan = crate::mc_export::build_plan(
            &s,
            "command",
            "block",
            &crate::mc_export::default_allowed_sources(),
            true,
            true,
            true,
            false,
            10.0,
            false,
            false,
            true,
            1.0,
            &|_| None,
            &|_| None,
        );
        assert!(!plan.rows.is_empty(), "命令方案要装下 9..81 里的音符");
        let built = build(&s, &o, Some(&plan));
        // 20 个内置 + 一个自定义音色让 insnum 变成 21，palette 也随之增长。
        assert_eq!(built.insnum, 21);
        let nbt = built.structure_nbt();
        let root = crate::nbt::parse(&nbt).unwrap();
        let pal = items(root.get("palette").unwrap());
        assert_eq!(pal.len(), 29 + 21 * 26);
        // 命令方块调色板区（3+21 起）全叫 command_block，朝向向上。
        let cb = &pal[3 + 21];
        assert_eq!(
            cb.get("Name").and_then(Value::as_str),
            Some("minecraft:command_block")
        );
        assert_eq!(
            cb.get("Properties")
                .unwrap()
                .get("facing")
                .and_then(Value::as_str),
            Some("up")
        );
        let blocks = items(root.get("blocks").unwrap());
        let cmd = blocks
            .iter()
            .find(|b| b.get("nbt").is_some() && b.get("nbt").unwrap().get("Command").is_some())
            .unwrap();
        let command = cmd
            .get("nbt")
            .unwrap()
            .get("Command")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(command, plan.rows[0].command, "命令直接来自方案行");
        assert!(command.starts_with("playsound "));
        assert_eq!(
            keys_of(cmd.get("nbt").unwrap()),
            [
                "id",
                "Command",
                "TrackOutput",
                "powered",
                "auto",
                "conditionMet",
                "UpdateLastExecution"
            ]
        );
        assert_eq!(cmd.get("state").and_then(Value::as_i32), Some(3 + 21));
    }

    #[test]
    fn classic_export_roundtrips_sizes_and_notes() {
        let s = song_with(&[(0, 0, 0, 45), (1, 0, 1, 50), (2, 0, 2, 40)]);
        let path = scratch("roundtrip.schematic");
        let info = export_schematic(&s, &path, &classic_options(), None).unwrap();
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
        let info = export_schematic(&s, &path, &o, None).unwrap();
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
        let built = build(&s, &o, None);
        assert!(built.sch_loop);
        let nbt = built.classic_nbt();
        let root = crate::nbt::parse(&nbt).unwrap();
        let tes = items(root.get("TileEntities").unwrap());
        assert_eq!(
            tes[1].get("Text1").and_then(Value::as_str),
            Some("{\"text\": \"Looping ON\"}")
        );
        assert_eq!(
            tes[1].get("Text4").and_then(Value::as_str),
            Some("{\"text\": \"Looping OFF\"}")
        );
        // 关掉循环选项后签名只剩一个。
        o.loop_option = false;
        let built = build(&s, &o, None);
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
        let built = build(&s, &o, None);
        assert!(!built.minecart);
        assert!(!built.chest_on);
        assert!(!built.sch_loop);
        o.layout = 0;
        let built = build(&s, &o, None);
        assert!(built.minecart);
        assert!(built.chest_on);
        assert!(built.sch_loop);
        // 箱子依赖矿车：关掉矿车后箱子也不会出现。
        o.minecart = false;
        let built = build(&s, &o, None);
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
        let built = build_with_stats(&s, &o, &st, None);
        assert_eq!(built.size.layers, 1);
        assert_eq!(built.size.hei, 7);
        assert_eq!(built.notes.len(), 1);
        // 压缩模式下 layers 用单 tick 最多音符数，这里同样是 1。
        o.compress = true;
        let built = build(&s, &o, None);
        assert_eq!(built.size.layers, 1);
        assert_eq!(built.notes.len(), 1);
    }

    #[test]
    fn open_guard_uses_totalblocks_minus_block_outside() {
        let _cn = pin_cn();
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
        let _cn = pin_cn();
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
        let _cn = pin_cn();
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
        assert_eq!(
            build(&s, &o, None).notes.len(),
            1,
            "普通音符盒模式只认 33..57"
        );
        o.command_block = true;
        assert_eq!(build(&s, &o, None).notes.len(), 2, "命令方块模式认 9..81");
        // 但 layers 仍按 33..57 计数：整首都用范围外音高时一行都不放。
        assert_eq!(build(&song_with(&[(0, 0, 0, 20)]), &o, None).notes.len(), 0);
        // 连 9..81 之外（key 8）也不算。
        let s = song_with(&[(0, 0, 0, 45), (0, 1, 0, 8)]);
        assert_eq!(build(&s, &o, None).notes.len(), 1);
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
        let built = build(&s, &o, None);
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
                o.set_format(if structure { 0 } else { 2 });
                let path = scratch(&format!("demo-{layout}-{structure}.out"));
                let info = export_schematic(&s, &path, &o, None).unwrap();
                assert!(
                    info.note_blocks > 0,
                    "layout {layout} structure {structure}"
                );
                assert!(info.length > 0 && info.width > 0 && info.height > 0);
                let raw = gzip_decode(&path);
                let (_, root) = crate::nbt::parse_named(&raw).unwrap();
                if structure {
                    assert_eq!(
                        keys_of(&root),
                        ["DataVersion", "size", "palette", "entities", "blocks"]
                    );
                    // 声明的 blocks 数量必须和真正写出来的一致。
                    let blocks = items(root.get("blocks").unwrap());
                    // 声明数量 = 网格体积：牌子/音符/箱子都由前面的显式条目写，
                    // 网格扫描把它们占的格子跳过。
                    assert_eq!(blocks.len(), info.height * info.length * info.width);
                } else {
                    assert_eq!(keys_of(&root)[0], "Height");
                    let blocks = root.get("Blocks").and_then(Value::as_bytes).unwrap();
                    assert_eq!(blocks.len(), info.height * info.length * info.width);
                }
                std::fs::remove_file(&path).ok();
            }
        }
    }

    // ---------------------------------------------------------------- 导入

    /// 手写一个经典 `.schematic`（gzip 后的字节），只用原版 `read_tags` 认得的标签类型。
    /// `blocks` / `data` 传 `None` 可以复现「少一个数组」的坏文件。
    /// 字节顺序是原版的 `for zz { for xx { for yy = width-1 .. 0 } }`。
    fn classic_file(
        width: i16,
        height: i16,
        length: i16,
        blocks: Option<&[u8]>,
        data: Option<&[u8]>,
        notes: &[(i32, i32, i32, u8)],
    ) -> Vec<u8> {
        let mut n = Nbt::default();
        n.compound("Schematic");
        n.short("Width", width);
        n.short("Height", height);
        n.short("Length", length);
        n.string("Materials", "Alpha");
        if let Some(b) = blocks {
            n.byte_array("Blocks", b);
        }
        if let Some(d) = data {
            n.byte_array("Data", d);
        }
        n.list("TileEntities", notes.len() as i32, 10);
        // list 里的 compound 元素没有标签头（原版 `schematic_save.gml:32` 直接写 id/x/y/z/note）。
        for (x, y, z, note) in notes {
            n.string("id", "Music");
            n.int("x", *x);
            n.int("y", *y);
            n.int("z", *z);
            n.byte("note", *note);
            n.end();
        }
        n.list("Entities", 0, 10);
        n.end();
        gzip(&n.buf).unwrap()
    }

    /// 格子坐标 → `Blocks` 数组下标，写测试时用它和 `classic_file` 对齐。
    fn flat(width: usize, length: usize, xx: usize, yy: usize, zz: usize) -> usize {
        zz * length * width + xx * width + (width - 1 - yy)
    }

    #[test]
    fn id_isblock_matches_the_original_chain() {
        let listed: Vec<u8> = vec![
            1, 2, 3, 4, 5, 7, 12, 13, 14, 15, 16, 17, 19, 21, 22, 24, 35, 41, 42, 43, 44, 45, 47,
            48, 49, 56, 57, 58, 73, 74, 82, 86, 87, 88, 89, 97, 98, 99, 100, 103,
        ];
        for id in 0..=255u8 {
            assert_eq!(id_isblock(id), listed.contains(&id), "id {id}");
        }
        // 玻璃、音符盒自己、线、火把、中继器都不算实体方块。
        assert!(!id_isblock(GLASS));
        assert!(!id_isblock(NOTE_BLOCK));
        assert!(!id_isblock(REDSTONE_WIRE));
        assert!(!id_isblock(TORCHON));
        assert!(!id_isblock(REPEATER));
    }

    #[test]
    fn block_get_ins_matches_the_original_table() {
        let table = [
            (1, 2),
            (4, 2),
            (5, 1),
            (7, 2),
            (12, 3),
            (13, 3),
            (14, 2),
            (15, 2),
            (16, 2),
            (17, 1),
            (20, 4),
            (21, 2),
            (22, 2),
            (23, 2),
            (24, 2),
            (25, 1),
            (35, 5),
            (41, 7),
            (43, 2),
            (44, 2),
            (45, 2),
            (47, 1),
            (48, 2),
            (49, 2),
            (53, 1),
            (54, 1),
            (56, 1),
            (61, 2),
            (62, 2),
            (67, 2),
            (73, 2),
            (74, 2),
            (82, 6),
            (84, 1),
            (85, 1),
            (87, 2),
            (88, 3),
            (89, 2),
            (97, 2),
            (98, 2),
            (107, 1),
            (108, 2),
            (109, 2),
            (174, 8),
            (216, 9),
        ];
        assert_eq!(table.len(), 45, "原版 block_get_ins 一共 45 条 if");
        for id in 0..=255u8 {
            let want = table
                .iter()
                .find(|(b, _)| *b == id)
                .map(|(_, i)| *i)
                .unwrap_or(0);
            assert_eq!(block_get_ins(id), want, "id {id}");
        }
        // 导出表的前 10 项要能被这张表原样读回乐器；10 号之后原版表里没有，落成竖琴。
        for (ins, &blk) in INSTRUMENT_BLOCKS[..10].iter().enumerate() {
            assert_eq!(block_get_ins(blk), ins as u8, "乐器 {ins} 的方块 {blk}");
        }
        assert_eq!(
            block_get_ins(INSTRUMENT_BLOCKS[10]),
            0,
            "铁片琴的方块回读是竖琴"
        );
    }

    #[test]
    fn import_follows_the_lever_block_note_path() {
        // 3x3x2：拉杆 (0,0,1) 挨着石头 (1,0,1)，石头旁边两个音符盒；
        // 音符盒下面的方块决定乐器，同一 tick 的第二个音符往上叠一层。
        let (wid, hei, len) = (3usize, 2usize, 3usize);
        let mut blocks = vec![0u8; wid * len * hei];
        blocks[flat(wid, len, 1, 1, 0)] = 41; // 音符盒 A 下面是金块 → 铃
        blocks[flat(wid, len, 2, 0, 0)] = STONE; // 音符盒 B 下面是石头 → 地鼓
        blocks[flat(wid, len, 0, 0, 1)] = LEVER;
        blocks[flat(wid, len, 1, 0, 1)] = STONE;
        let data = vec![0u8; wid * len * hei];
        // NBT 轴映射（原版 read_tags）：cell(xx,yy,zz) ← x = width-1-yy、y = zz、z = xx。
        let packed = classic_file(
            wid as i16,
            hei as i16,
            len as i16,
            Some(&blocks),
            Some(&data),
            &[(1, 1, 1, 7), (2, 1, 2, 12)],
        );
        let got = import_bytes(&packed, "song").unwrap();
        assert_eq!((got.width, got.length, got.height), (3, 3, 2));
        assert_eq!((got.inputs, got.note_blocks, got.added_notes), (1, 2, 2));
        let notes: Vec<_> = got.song.iter_notes().collect();
        // 后入队的 B 先弹出（原版的队列是 LIFO），所以 B 落在第 0 层。
        assert_eq!(
            notes,
            vec![(0, 0, Note::new(2, 45)), (0, 1, Note::new(7, 40))]
        );
        assert_eq!(got.song.layers.len(), 2);
        assert!(!got.song.dirty, "原版 reset() 之后 changed = 0");
        assert_eq!(
            got.trace,
            "song, width: 3, length: 3, height: 2, inputs: 1\r\n\
             _____INPUT 1_____\r\n__Input__\r\nFound block next to input\r\n\
             __Block__\r\nFound note block next to block\r\n\
             Found note block next to block\r\n\
             __Note block__\r\nAdded note block no. 1!\r\n\
             __Note block__\r\nAdded note block no. 2!"
        );
    }

    #[test]
    fn import_puts_a_second_input_sixteen_ticks_after_the_first() {
        // 两组「拉杆压石头、石头挨音符盒」，原版靠 `start = enda + 16*(i>0)` 接起来。
        let (wid, hei, len) = (2usize, 2usize, 2usize);
        let mut blocks = vec![0u8; wid * len * hei];
        blocks[flat(wid, len, 0, 0, 0)] = STONE;
        blocks[flat(wid, len, 1, 0, 0)] = STONE;
        blocks[flat(wid, len, 0, 0, 1)] = LEVER;
        blocks[flat(wid, len, 1, 0, 1)] = LEVER;
        let data = vec![0u8; wid * len * hei];
        // 音符盒 A 在 (0,1,0)、B 在 (1,1,0)。
        let packed = classic_file(
            wid as i16,
            hei as i16,
            len as i16,
            Some(&blocks),
            Some(&data),
            &[(0, 0, 0, 7), (0, 0, 1, 12)],
        );
        let got = import_bytes(&packed, "two").unwrap();
        assert_eq!(got.inputs, 2, "原版 inputam = 2 时还会多弹一句提示");
        assert_eq!(got.added_notes, 2);
        let notes: Vec<_> = got.song.iter_notes().collect();
        assert_eq!(
            notes,
            vec![(0, 0, Note::new(0, 40)), (16, 0, Note::new(0, 45))]
        );
        assert_eq!(got.song.song_length(), 17);
    }

    #[test]
    fn import_refuses_files_the_original_refuses() {
        let one = [0u8; 1];
        let note = [(0, 0, 0, 7u8)];
        // 不是 gzip。
        assert_eq!(
            import_bytes(b"plain text", "x").unwrap_err(),
            ImportError::NotASchematic
        );
        // 根标签不叫 Schematic（1.13 的 .sponge / 结构文件就是这样）。
        let mut n = Nbt::default();
        n.compound("Resources");
        n.end();
        assert_eq!(
            import_bytes(&gzip(&n.buf).unwrap(), "x").unwrap_err(),
            ImportError::NotASchematic
        );
        // 尺寸写 0。
        assert_eq!(
            import_bytes(&classic_file(0, 1, 1, Some(&one), Some(&one), &note), "x").unwrap_err(),
            ImportError::InvalidSize
        );
        // 少 Blocks / 少 Data。
        assert_eq!(
            import_bytes(&classic_file(1, 1, 1, None, Some(&one), &note), "x").unwrap_err(),
            ImportError::NoBlocksArray
        );
        assert_eq!(
            import_bytes(&classic_file(1, 1, 1, Some(&one), None, &note), "x").unwrap_err(),
            ImportError::NoDataArray
        );
        // 没有 TileEntities 音符盒 —— 原版只看 TileEntities，Blocks 数组里有 25 也不算。
        assert_eq!(
            import_bytes(&classic_file(1, 1, 1, Some(&one), Some(&one), &[]), "x").unwrap_err(),
            ImportError::NoNoteBlocks
        );
        // 没有拉杆/按钮。
        let mut blocks = vec![NOTE_BLOCK; 4];
        blocks[3] = STONE;
        assert_eq!(
            import_bytes(
                &classic_file(2, 1, 2, Some(&blocks), Some(&[0; 4]), &[(0, 0, 0, 7)]),
                "x"
            )
            .unwrap_err(),
            ImportError::NoInputs
        );
        // 音符盒离输入太远：原版会先清空歌曲再报这句。
        let (wid, hei, len) = (3usize, 2usize, 3usize);
        let mut blocks = vec![0u8; wid * len * hei];
        blocks[flat(wid, len, 0, 0, 1)] = LEVER;
        blocks[flat(wid, len, 0, 0, 0)] = STONE;
        let packed = classic_file(
            wid as i16,
            hei as i16,
            len as i16,
            Some(&blocks),
            Some(&[0u8; 18]),
            &[(0, 1, 2, 7)],
        );
        assert_eq!(
            import_bytes(&packed, "x").unwrap_err(),
            ImportError::NoNotesAdded
        );
        // 对话框文案与标题（原版没翻译，逐字照抄）。
        assert_eq!(
            ImportError::NoNotesAdded.dialog(),
            Some((
                "Error",
                "None of the note blocks in the Schematic could be added. This could be to none of \
                 them being connected to any inputs (buttons or levers)."
            ))
        );
        assert_eq!(
            ImportError::NoInputs.dialog(),
            Some((
                "Error",
                "The Schematic did not include any supported inputs.\nSupported inputs are:\n\n* Buttons\n* Levers"
            ))
        );
        // 读不到文件在原版没有对话框（`file_exists_lib()` 那步就 return 了）。
        assert_eq!(ImportError::Io("nope".into()).dialog(), None);
    }

    #[test]
    fn import_reads_only_the_tag_types_the_original_reads() {
        // Width/Height/Length 写成 TAG_Int、note 写成 TAG_Short，原版一律不认。
        let mut n = Nbt::default();
        n.compound("Schematic");
        n.int("Width", 1);
        n.int("Height", 1);
        n.int("Length", 1);
        n.byte_array("Blocks", &[LEVER]);
        n.byte_array("Data", &[0]);
        n.list("TileEntities", 1, 10);
        n.short("note", 7);
        n.int("x", 0);
        n.end();
        n.end();
        assert_eq!(
            import_bytes(&gzip(&n.buf).unwrap(), "x").unwrap_err(),
            ImportError::InvalidSize,
            "TAG_Int 的 Width 在原版里读不到，尺寸仍是 0"
        );
        // 同样的内容换个列表名（不叫 TileEntities）也不会被收音符盒。
        let mut n = Nbt::default();
        n.compound("Schematic");
        n.short("Width", 1);
        n.short("Height", 1);
        n.short("Length", 1);
        n.byte_array("Blocks", &[LEVER]);
        n.byte_array("Data", &[0]);
        n.list("Notes", 1, 10);
        n.byte("note", 7);
        n.int("x", 0);
        n.end();
        n.end();
        assert_eq!(
            import_bytes(&gzip(&n.buf).unwrap(), "x").unwrap_err(),
            ImportError::NoNoteBlocks
        );
    }

    #[test]
    fn import_applies_worldedit_offsets_and_the_note_inheritance() {
        // 原版对 TileEntities 的坐标一律减去 WEOffset*，且 `nb_n` 只在被收走时清零：
        // 没有 note 标签的条目不计数，但会把坐标留给下一条。
        let (wid, hei, len) = (3usize, 2usize, 3usize);
        let mut blocks = vec![0u8; wid * len * hei];
        blocks[flat(wid, len, 0, 0, 1)] = LEVER;
        blocks[flat(wid, len, 0, 0, 0)] = STONE;
        let mut n = Nbt::default();
        n.compound("Schematic");
        n.short("Width", wid as i16);
        n.short("Height", hei as i16);
        n.short("Length", len as i16);
        n.string("Materials", "Alpha");
        n.byte_array("Blocks", &blocks);
        n.byte_array("Data", &[0u8; 18]);
        n.int("WEOffsetX", 2);
        n.int("WEOffsetY", 5);
        n.int("WEOffsetZ", 1);
        n.list("TileEntities", 3, 10);
        // (x=1,y=5,z=3) - 偏移 → cell(1,0,0)，正好在石头旁边。
        n.string("id", "Music");
        n.int("x", 1);
        n.int("y", 5);
        n.int("z", 3);
        n.byte("note", 7);
        n.end();
        // 没有 note 的条目（比如命令方块）：只留坐标，自己不计数。
        n.string("id", "ControlBlock");
        n.int("x", 0);
        n.int("y", 5);
        n.int("z", 2);
        n.end();
        // 只给 note 的条目：沿用上一条的坐标 → cell(0,1,0)。
        n.byte("note", 9);
        n.end();
        n.end();
        n.end();
        let got = import_bytes(&gzip(&n.buf).unwrap(), "offset").unwrap();
        assert_eq!((got.note_blocks, got.added_notes), (2, 2));
        let notes: Vec<_> = got.song.iter_notes().collect();
        assert_eq!(
            notes,
            vec![(0, 0, Note::new(0, 40)), (0, 1, Note::new(0, 42))]
        );
    }

    #[test]
    fn import_reads_back_a_classic_schematic_the_exporter_wrote() {
        // 自己导出的经典图样要能自己读回来：这是原版「Schematic 导入」最常用的来源，
        // 也顺带验一遍电路两端（乐器 ↔ 方块 id、音高 ↔ note 数据）用的是同一套编码。
        // 注意 tick 对不上：读侧每根红石线补给中继器都要 `delay + 1`（原版
        // `open_schematic.gml`），而写侧每隔 16 格就插一根 delay=0 的补给中继器，
        // 所以一圈回来每个音符都被往后推了几 tick —— 原版自己导入自己的图样也是这个结果。
        let song = Song::demo();
        let opts = SchematicOptions::default();
        let stats = calculate_stats(&song);
        let packed = gzip(&build_with_stats(&song, &opts, &stats, None).classic_nbt()).unwrap();
        let got = import_bytes(&packed, "demo").unwrap();
        assert_eq!(got.inputs, 1, "经典图样只写一个拉杆");
        let mut want: Vec<_> = song
            .iter_notes()
            .map(|(_, _, n)| (n.instrument, n.key))
            .collect();
        let mut have: Vec<_> = got
            .song
            .iter_notes()
            .map(|(_, _, n)| (n.instrument, n.key))
            .collect();
        want.sort_unstable();
        have.sort_unstable();
        assert_eq!(have, want);
        assert_eq!(got.added_notes, want.len());
        assert!(got.trace.contains("_____INPUT 1_____"));
    }

    /// 用户给的一对参照物：装机版原版自己导出的 .nbs + 对应 .nbt。
    /// 按截图那套选项重导一遍，size / palette / 方块表 / 牌面文字逐条比对。
    #[test]
    fn structure_export_matches_the_original_reference_file() {
        // 参照曲是用户自己的音乐，不入库（assets/Songs 在 .gitignore 里）。
        let Some(dir) = reference_songs_dir() else {
            eprintln!("跳过：本机没有 assets/Songs（参照曲不入库）");
            return;
        };
        let stem = "祭司王约翰的黄金境1.9(导出准备";
        let nbs_path = dir.join(format!("{stem}.nbs"));
        let nbt_path = dir.join(format!("{stem}.nbt"));
        if !nbs_path.is_file() || !nbt_path.is_file() {
            eprintln!("跳过：本机没有参照曲 {stem}");
            return;
        }
        let song = nbs_core::nbs::load(&nbs_path).unwrap();
        let theirs = crate::nbt::parse(&gzip_decode(&nbt_path)).unwrap();

        // 截图：环形过道 / .nbt 结构方块 / 每行 15 / 不含静音层 / 压缩 / 矿车轨道 /
        // 矿车箱 / 循环选项 / 不用命令方块 / 10 红石刻每秒。
        let mut o = SchematicOptions::default();
        o.layout = 0;
        o.structure = true;
        o.notesperrow = 15;
        o.include_locked = false;
        o.compress = true;
        o.minecart = true;
        o.chest = true;
        o.loop_option = true;
        o.command_block = false;
        o.tempo_mode = 0;

        let ours = crate::nbt::parse(&build_nbt(&song, &o, None).unwrap()).unwrap();

        // 骨架与矿车实体必须逐字段相同（blockPos 曾因过道中线取整差 1）。
        assert_eq!(keys_of(&ours), keys_of(&theirs), "根节点的键序不同");
        assert_eq!(ours.get("DataVersion"), theirs.get("DataVersion"));
        assert_eq!(
            items(ours.get("size").unwrap()),
            items(theirs.get("size").unwrap()),
            "size 三元组不同"
        );
        assert_eq!(
            items(ours.get("entities").unwrap()),
            items(theirs.get("entities").unwrap()),
            "矿车实体不同"
        );

        // palette 整表相同：29 + 20 个乐器的方块行与音符行（含铜块一族的
        // waxed 铜块和 trumpet_* 属性名）+ 尾部 26 项，顺序也必须一致。
        assert_eq!(
            palette_names(&ours),
            palette_names(&theirs),
            "调色板与原版不同"
        );

        // 方块表：逐条同序同值。原版网格扫描会跳过已经写过牌子的格子，
        // 所以牌格里只有牌子那一条，任何格子都不该出现两次。
        let our_cells = cells_in_order(&ours);
        let their_cells = cells_in_order(&theirs);
        assert_eq!(our_cells.len(), their_cells.len(), "方块条数不同");
        let first_diff = our_cells
            .iter()
            .zip(their_cells.iter())
            .position(|(a, b)| a != b);
        match first_diff {
            None => {}
            Some(i) => panic!(
                "第 {} 格不同：我们 {:?} / 原版 {:?}",
                i, our_cells[i], their_cells[i]
            ),
        }
        let unique: std::collections::HashSet<((i32, i32, i32), String)> =
            our_cells.iter().cloned().collect();
        assert_eq!(unique.len(), our_cells.len(), "我们把同一个格子写了两次");

        // 牌面四行逐字相同：署名牌是装机版的新断句，循环牌两处一致。
        assert_eq!(sign_texts(&ours, 0), sign_texts(&theirs, 0), "署名牌不同");
        assert_eq!(sign_texts(&ours, 1), sign_texts(&theirs, 1), "循环牌不同");
    }

    /// palette 项的人读描述：`名字[属性按键排序]`。
    fn describe_palette_entry(entry: &Value) -> String {
        let mut s = entry
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        if let Some(p) = entry.get("Properties") {
            let mut kv: Vec<(&str, &str)> = p
                .fields()
                .unwrap_or_default()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or("?")))
                .collect();
            kv.sort_unstable();
            s.push('[');
            let parts: Vec<String> = kv.iter().map(|(k, v)| format!("{k}={v}")).collect();
            s.push_str(&parts.join(","));
            s.push(']');
        }
        s
    }

    fn palette_names(root: &Value) -> Vec<String> {
        items(root.get("palette").unwrap())
            .iter()
            .map(describe_palette_entry)
            .collect()
    }

    fn block_pos(entry: &Value) -> (i32, i32, i32) {
        let p = items(entry.get("pos").unwrap());
        (
            p[0].as_i32().unwrap(),
            p[1].as_i32().unwrap(),
            p[2].as_i32().unwrap(),
        )
    }

    /// 按写入顺序的「(格子, 方块描述)」；方块用 palette 文本描述，
    /// 这样不受 palette 下标重编号的影响，能直接看出差在哪一格。
    fn cells_in_order(root: &Value) -> Vec<((i32, i32, i32), String)> {
        let pal = items(root.get("palette").unwrap());
        items(root.get("blocks").unwrap())
            .iter()
            .map(|b| {
                let state = b.get("state").and_then(Value::as_i32).unwrap() as usize;
                (block_pos(b), describe_palette_entry(&pal[state]))
            })
            .collect()
    }

    /// 第 index 张告示牌的四行文字（按 nbt 里的 Text1..Text4）。
    fn sign_texts(root: &Value, index: usize) -> [String; 4] {
        let signs: Vec<&Value> = items(root.get("blocks").unwrap())
            .iter()
            .filter(|b| {
                b.get("nbt")
                    .and_then(|n| n.get("id"))
                    .and_then(Value::as_str)
                    == Some("minecraft:sign")
            })
            .collect();
        let nbt = signs[index].get("nbt").unwrap();
        [
            nbt.get("Text1")
                .and_then(Value::as_str)
                .unwrap()
                .to_string(),
            nbt.get("Text2")
                .and_then(Value::as_str)
                .unwrap()
                .to_string(),
            nbt.get("Text3")
                .and_then(Value::as_str)
                .unwrap()
                .to_string(),
            nbt.get("Text4")
                .and_then(Value::as_str)
                .unwrap()
                .to_string(),
        ]
    }

    fn reference_songs_dir() -> Option<std::path::PathBuf> {
        let m = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        ["../../assets/Songs", "../../../assets/Songs"]
            .iter()
            .map(|up| m.join(up))
            .find(|p| p.is_dir())
    }

    fn gzip_decode(path: &std::path::Path) -> Vec<u8> {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let f = std::fs::File::open(path).unwrap();
        let mut out = Vec::new();
        GzDecoder::new(f).read_to_end(&mut out).unwrap();
        out
    }

    // ------------------------------------------------------------- 直轨导出

    /// 直轨测试专用：能同时指定力度/声像（默认 100/100）。
    fn track_song(notes: &[(u32, u32, u8, u8, u8, u8)]) -> Song {
        let mut s = Song::new();
        let max_layer = notes.iter().map(|n| n.1).max().unwrap_or(0) as usize;
        s.layers = vec![Layer::default(); max_layer + 1];
        for (t, l, ins, key, pan, vel) in notes {
            let mut n = Note::new(*ins, *key);
            n.panning = *pan;
            n.velocity = *vel;
            s.set(*t, *l, n);
        }
        s.recalculate();
        s
    }

    fn fget<'v>(v: &'v Value, name: &str) -> &'v Value {
        v.fields()
            .unwrap_or_else(|| panic!("不是 compound: {v:?}"))
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("缺字段 {name}"))
    }

    fn palette(v: &Value) -> &[Value] {
        fget(v, "palette").as_list().unwrap().1
    }

    fn block_entries(v: &Value) -> &[Value] {
        fget(v, "blocks").as_list().unwrap().1
    }

    fn pos_of(entry: &Value) -> Vec<i32> {
        fget(entry, "pos")
            .as_list()
            .unwrap()
            .1
            .iter()
            .map(|p| p.as_i32().unwrap())
            .collect()
    }

    /// classic 网格序列化下标：外层 c→中层 a，内层 b 从宽-1 递减（gml:1348 逐字）。
    fn classic_idx(a: usize, b: usize, c: usize, len: usize, wid: usize) -> usize {
        (c * len + a) * wid + (wid - 1 - b)
    }

    fn no_track_mappings(_: usize) -> Option<String> {
        None
    }

    fn no_track_durations(_: usize) -> Option<f64> {
        None
    }

    fn track_plan(song: &Song) -> crate::mc_export::Plan {
        let mut plan = crate::mc_export::build_plan(
            song,
            "command",
            "block",
            &crate::mc_export::default_allowed_sources(),
            false,
            true,
            false,
            false,
            10.0,
            false,
            true,
            true,
            0.0,
            &no_track_mappings,
            &no_track_durations,
        );
        crate::mc_export::prepare_track_plan(&mut plan, song);
        plan
    }

    #[test]
    fn classic_track_roundtrip_matches_schematic_layout() {
        // 两个音符：tick0 在网格内，tick1（最后一个 tick）出界只剩 TileEntity。
        let song = song_with(&[(0, 0, 0, 45), (1, 0, 0, 57)]);
        let opts = classic_options(); // format=2 → classic .schematic
        let path = scratch("track_classic.schematic");
        export_track(&song, &opts, None, &path).unwrap();
        let v = crate::nbt::parse(&gzip_decode(&path)).unwrap();
        std::fs::remove_file(&path).ok();

        let names: Vec<&str> = v
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
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
        // len = 40 + enda*2 = 42，宽固定 100，高 = max(20, 层*4+4)。
        assert_eq!(fget(&v, "Height").as_i16(), Some(20));
        assert_eq!(fget(&v, "Length").as_i16(), Some(42));
        assert_eq!(fget(&v, "Width").as_i16(), Some(100));
        assert_eq!(fget(&v, "Materials").as_str(), Some("Alpha"));

        let (blocks, data) = match (fget(&v, "Blocks"), fget(&v, "Data")) {
            (Value::ByteArray(b), Value::ByteArray(d)) => (b, d),
            _ => panic!("Blocks/Data 不是字节数组"),
        };
        assert_eq!(blocks.len(), 42 * 100 * 20);
        assert_eq!(data.len(), blocks.len());

        // 网格里的牌子：(2, 49, hei-1) id 68 data 3。
        let i = classic_idx(2, 49, 19, 42, 100);
        assert_eq!(blocks[i], 68);
        assert_eq!(data[i], 3);
        // 过道（出厂石头）：(10, 49, hei-3)。
        let i = classic_idx(10, 49, 17, 42, 100);
        assert_eq!(blocks[i], 1);
        assert_eq!(data[i], 0);
        // 车道中继器：tempo_mode 0 → delay 0、direction 2 → data 2，id 93。
        let i = classic_idx(41, 50, 3, 42, 100);
        assert_eq!(blocks[i], 93);
        assert_eq!(data[i], 2);
        // tick0 音符盒在网格 (41, 49, 2)；tick1 的 x=43 出界 → 网格里只有 1 个音符盒。
        let i = classic_idx(41, 49, 2, 42, 100);
        assert_eq!(blocks[i], 25);
        assert_eq!(blocks.iter().filter(|&&b| b == 25).count(), 1);

        // TileEntities：牌子 + 两个音符盒（出界的那个也记录，指向网格外的空位）。
        let tes = fget(&v, "TileEntities").as_list().unwrap().1;
        assert_eq!(tes.len(), 3);
        assert_eq!(fget(&tes[0], "id").as_str(), Some("minecraft:sign"));
        assert_eq!(fget(&tes[0], "x").as_i32(), Some(50)); // wid-1-signy
        assert_eq!(fget(&tes[0], "y").as_i32(), Some(19));
        assert_eq!(fget(&tes[0], "z").as_i32(), Some(2));
        assert_eq!(
            fget(&tes[0], "Text1").as_str(),
            Some("{\"text\": \"Song generated\"}")
        );
        assert_eq!(fget(&tes[1], "id").as_str(), Some("minecraft:noteblock"));
        assert_eq!(fget(&tes[1], "x").as_i32(), Some(50));
        assert_eq!(fget(&tes[1], "y").as_i32(), Some(2));
        assert_eq!(fget(&tes[1], "z").as_i32(), Some(41));
        assert_eq!(fget(&tes[1], "note").as_i8(), Some(12)); // key-33
        assert_eq!(fget(&tes[2], "z").as_i32(), Some(43));
        assert_eq!(fget(&tes[2], "note").as_i8(), Some(24)); // 57-33

        let version = fget(&v, "Version");
        assert_eq!(fget(version, "Id").as_i32(), Some(922));
        assert_eq!(fget(version, "Name").as_str(), Some("1.11.2"));
    }

    #[test]
    fn classic_track_old_format_uses_plain_sign_and_1_8() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let mut opts = classic_options();
        opts.minecraft_old = true;
        let path = scratch("track_classic_old.schematic");
        export_track(&song, &opts, None, &path).unwrap();
        let v = crate::nbt::parse(&gzip_decode(&path)).unwrap();
        std::fs::remove_file(&path).ok();

        let tes = fget(&v, "TileEntities").as_list().unwrap().1;
        assert_eq!(tes.len(), 2);
        assert_eq!(fget(&tes[0], "id").as_str(), Some("Sign"));
        assert_eq!(fget(&tes[0], "Text1").as_str(), Some("Song generated"));
        assert_eq!(fget(&tes[1], "id").as_str(), Some("Music"));
        let version = fget(&v, "Version");
        let names: Vec<&str> = version
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(names, vec!["Name", "Snapshot"]);
        assert_eq!(fget(version, "Name").as_str(), Some("1.8"));
    }

    #[test]
    fn structure_track_palette_and_size_header() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let opts = SchematicOptions::default(); // structure=1、format=0
        let path = scratch("track_struct.nbt");
        export_track(&song, &opts, None, &path).unwrap();
        let v = crate::nbt::parse(&gzip_decode(&path)).unwrap();
        std::fs::remove_file(&path).ok();

        // 根 compound 没有 TAG_End（与 schematic_export 同款，装机版如此）；
        // 字段顺序与原版 gml:124-133 一致。
        let names: Vec<&str> = v
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["DataVersion", "size", "palette", "entities", "blocks"]
        );
        assert_eq!(fget(&v, "DataVersion").as_i32(), Some(1519));

        // size：smax=(50,19,41) → (min(51,32), 20, min(42,32))，format=0 每向封顶 32。
        let size: Vec<i32> = fget(&v, "size")
            .as_list()
            .unwrap()
            .1
            .iter()
            .map(|n| n.as_i32().unwrap())
            .collect();
        assert_eq!(size, vec![32, 20, 32]);

        // palette = 30 + 26*insnum，insnum = 20（无自定义乐器）。
        let pal = palette(&v);
        assert_eq!(pal.len(), 30 + 26 * 20);
        assert_eq!(fget(&pal[0], "Name").as_str(), Some("minecraft:stone"));
        assert_eq!(fget(&pal[1], "Name").as_str(), Some("minecraft:stone"));
        assert_eq!(fget(&pal[2], "Name").as_str(), Some("minecraft:stone"));
        // 乐器 9（钢琴 → 木头）是唯一带 axis:y 的乐器底块。
        let props = fget(&pal[12], "Properties");
        let pnames: Vec<&str> = props
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(pnames, vec!["axis"]);
        assert_eq!(fget(props, "axis").as_str(), Some("y"));
        // 音符条目：state = 3 + insnum + ins*25 + note；harp note 12 → 35。
        let nb = &pal[35];
        assert_eq!(fget(nb, "Name").as_str(), Some("minecraft:note_block"));
        let props = fget(nb, "Properties");
        let pnames: Vec<&str> = props
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(pnames, vec!["instrument", "note", "powered"]);
        assert_eq!(fget(props, "instrument").as_str(), Some("harp"));
        assert_eq!(fget(props, "note").as_str(), Some("12"));
        assert_eq!(fget(props, "powered").as_str(), Some("false"));
        // 27 条固定尾巴：首 blue_wool、air，尾 smooth_stone_slab type top。
        let tail = 3 + 20 + 20 * 25;
        assert_eq!(
            fget(&pal[tail], "Name").as_str(),
            Some("minecraft:blue_wool")
        );
        assert_eq!(fget(&pal[tail + 1], "Name").as_str(), Some("minecraft:air"));
        let slab = &pal[pal.len() - 1];
        assert_eq!(
            fget(slab, "Name").as_str(),
            Some("minecraft:smooth_stone_slab")
        );
        assert_eq!(fget(fget(slab, "Properties"), "type").as_str(), Some("top"));

        // sign 条目：pos [50,19,2]、state = insnum*26+19 = 539、四行牌面。
        let entries = block_entries(&v);
        let sign = entries
            .iter()
            .find(|e| pos_of(e) == vec![50, 19, 2])
            .expect("缺 sign 条目");
        assert_eq!(fget(sign, "state").as_i32(), Some(20 * 26 + 19));
        let nbt = fget(sign, "nbt");
        assert_eq!(fget(nbt, "Color").as_str(), Some("black"));
        assert_eq!(fget(nbt, "id").as_str(), Some("minecraft:sign"));
        assert_eq!(
            fget(nbt, "Text1").as_str(),
            Some("{\"text\": \"Song generated\"}")
        );
        assert_eq!(fget(nbt, "Text4").as_str(), Some("{\"text\": \"Studio\"}"));

        // 音符条目（中央车道 part#1）：pos [50,2,41]、state 35、
        // 非命令模式也带 nbt compound + 空 Command（原版 gml:1275 注释「magic」）。
        let note_entry = entries
            .iter()
            .find(|e| pos_of(e) == vec![50, 2, 41])
            .expect("缺音符条目");
        let efields: Vec<&str> = note_entry
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(efields, vec!["nbt", "pos", "state"]);
        assert_eq!(fget(note_entry, "state").as_i32(), Some(3 + 20 + 12));
        let nbt = fget(note_entry, "nbt");
        let nfields: Vec<&str> = nbt
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            nfields,
            vec![
                "Command",
                "TrackOutput",
                "powered",
                "auto",
                "conditionMet",
                "UpdateLastExecution"
            ]
        );
        assert_eq!(fget(nbt, "Command").as_str(), Some(""));
        assert_eq!(fget(nbt, "TrackOutput").as_i8(), Some(0));
        assert_eq!(fget(nbt, "conditionMet").as_i8(), Some(1));
    }

    #[test]
    fn structure_track_lanes_split_by_pan_and_volume() {
        // 四拍（cvol 是 0..100 量纲，原版那些 cvol<=0.2 分支几乎恒假）：
        // tick0 中央 pan100/vel100 → 车道 center；
        // tick1 极左 pan0/vel100 → 只有 r4 收下（cpanvol<=20 与 cpan<=20 都成立）；
        // tick2 极右 pan200/vel100 → cpanvol>=180 让左翼 l4 也收一份（判定不互斥，
        //   同一音符落两条车道：l4 + r1）；
        // tick3 pan10/vel0 → cm=0 而 cn=1 → key*0 → note=-33 的原版负 state bug。
        let song = track_song(&[
            (0, 0, 0, 45, 100, 100),
            (1, 0, 0, 45, 0, 100),
            (2, 0, 0, 45, 200, 100),
            (3, 0, 0, 45, 10, 0),
        ]);
        let opts = SchematicOptions::default();
        let path = scratch("track_lanes.nbt");
        export_track(&song, &opts, None, &path).unwrap();
        let v = crate::nbt::parse(&gzip_decode(&path)).unwrap();
        std::fs::remove_file(&path).ok();

        // 音符条目 = nbt 里有 Command 字段的条目（布线条目没有 nbt，sign 的 nbt 没有）。
        let entries = block_entries(&v);
        let notes: Vec<(Vec<i32>, i32)> = entries
            .iter()
            .filter(|e| {
                e.fields().unwrap().iter().any(|(k, _)| k == "nbt")
                    && fget(e, "nbt")
                        .fields()
                        .unwrap()
                        .iter()
                        .any(|(k, _)| k == "Command")
            })
            .map(|e| (pos_of(e), fget(e, "state").as_i32().unwrap()))
            .collect();
        // pos = [99-y, z, x]；y1 = yy+dir±12k，part#1 在 (x1+2, y1∓1, bz+1+adds)。
        let expected = vec![
            (vec![50, 2, 41], 35), // tick0 center: y1=50, adds=1
            (vec![2, 6, 43], 35),  // tick1 r4: y1=98, adds=5
            (vec![98, 6, 45], 35), // tick2 l4: y1=2
            (vec![38, 3, 45], 35), // tick2 r1: y1=62, adds=2
            (vec![2, 6, 47], -10), // tick3 r4: note = 0*key-33
        ];
        assert_eq!(notes.len(), expected.len(), "实际落点: {notes:?}");
        for (p, s) in &expected {
            assert!(
                notes.contains(&(p.clone(), *s)),
                "缺落点 pos={p:?} state={s}，实际: {notes:?}"
            );
        }
    }

    #[test]
    fn structure_track_command_mode_uses_plan_rows() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let plan = track_plan(&song);
        let mut opts = SchematicOptions::default();
        opts.command_block = true;
        let path = scratch("track_command.nbt");
        export_track(&song, &opts, Some(&plan), &path).unwrap();
        let v = crate::nbt::parse(&gzip_decode(&path)).unwrap();
        std::fs::remove_file(&path).ok();

        // 命令模式 palette 里 ins*25 全是 command_block，不再有 note_block。
        let pal = palette(&v);
        assert_eq!(
            fget(&pal[35], "Name").as_str(),
            Some("minecraft:command_block")
        );
        assert_eq!(
            fget(fget(&pal[35], "Properties"), "facing").as_str(),
            Some("up")
        );

        // 音符条目：state = 3+insnum（没有 ins*25+note 项），
        // Command 逐字取自方案行。
        let row_index = plan.track_rows_by_grid[0][4][0];
        let entries = block_entries(&v);
        let note_entry = entries
            .iter()
            .find(|e| pos_of(e) == vec![50, 2, 41])
            .expect("缺命令方块条目");
        assert_eq!(fget(note_entry, "state").as_i32(), Some(3 + 20));
        let nbt = fget(note_entry, "nbt");
        assert_eq!(fget(nbt, "id").as_str(), Some("minecraft:command_block"));
        let command = plan.rows[row_index].command.clone();
        assert!(!command.is_empty());
        assert_eq!(fget(nbt, "Command").as_str(), Some(command.as_str()));
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nbs_sch_{}", name))
    }
}
