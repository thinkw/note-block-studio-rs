//! MAESTRO 音乐机「节目单」导出（WorldEdit `.schem`）。
//!
//! 对齐上游转换器 vladdeSV/maestro（`source/parse-nbs.ts`、
//! `source/process-binary-stream.ts`、`source/schematic/*.ts`）：把整首歌编码成
//! 一组装着唱片与羊毛的箱子，贴到**已经建好的 MAESTRO 机体**上才会发声。
//! 它不是自包含可响的结构，和经典过道导出是两条并行路线，互不取代。
//!
//! 上游仓库没有 LICENSE，这里不搬它的代码，只按它公开在源码里的编码规范独立实现。
//! 三张表（读盘器面板、灰度码、方向×分区→乐器）由脚本从上游原文抽取并逐条校验，
//! 见 `tests::tables_match_the_upstream_invariants`——手抄 112 个格子迟早出错。
//!
//! 与原转换器的两处实现差异（行为一致）：
//! - 转换器用 `process.exit(1)` / `throw` 终止，这里返回 [`Err`]。
//! - 转换器靠 JS「整数键对象自动按升序遍历」保证写入顺序，这里用 `BTreeMap`。
//!
//! 刻意保留的上游行为：
//! - 锁定图层照样导出（转换器不看 `isLocked`），力度/声像/微调音分全部忽略。
//! - 超出 33..=57 的音高按 ±12 反复移调；自定义乐器按采样文件名映射到怪物头，
//!   映射不到的整颗音符丢弃（详见 [`MaestroReport::lines`]）。
//! - 速度不是 20 t/s 时按所选取整模式改写时间轴；>20 t/s 一律走「每 N tick 取 1」的压缩。

use std::collections::BTreeMap;
use std::path::Path;

use nbs_core::model::{FIRST_CUSTOM_INDEX, Song, TEMPO_CHANGER_NAME};

use crate::schematic::Nbt;

// ==================================================== 从上游抽取的常量表

/// MAESTRO 读盘器面板表，逐格来自官方转换器的 `discReaderLayout`。
/// 每格是读盘器编号（`A24`、`M`），或三个空格表示该处没有读盘器。
/// 四条不变量（A/B/C 各 25、D+E 合计 25、`M` 至多一格、总数 100+M）在文件末尾
/// 的 const 校验里逐条断言。
pub(crate) const DISC_LAYOUT: [[&str; 14]; 8] = [
    [
        "   ", "   ", "A24", "   ", "A21", "   ", "A18", "   ", "A22", "   ", "A23", "   ", "   ",
        "   ",
    ],
    [
        "E21", "M", "A15", "A20", "A11", "A16", "A06", "A13", "A12", "A17", "A14", "A19", "D20",
        "D24",
    ],
    [
        "E13", "E17", "A08", "A09", "A01", "A05", "A00", "A03", "A02", "A04", "A07", "A10", "D12",
        "D16",
    ],
    [
        "E05", "E09", "B19", "B23", "B05", "B12", "B01", "B09", "B06", "B13", "B20", "B24", "D04",
        "D08",
    ],
    [
        "E02", "E01", "B18", "B15", "B08", "B04", "B02", "B00", "B07", "B03", "B17", "B16", "D03",
        "D00",
    ],
    [
        "E10", "E06", "C07", "B22", "C02", "B11", "C00", "B10", "C01", "B14", "C08", "B21", "D11",
        "D07",
    ],
    [
        "E18", "E14", "C14", "C10", "C12", "C04", "C06", "C03", "C11", "C05", "C15", "C09", "D19",
        "D15",
    ],
    [
        "   ", "E22", "C23", "C19", "C22", "C17", "C18", "C13", "C21", "C16", "C24", "C20", "D23",
        "   ",
    ],
];

/// 官方 `xoliksCode`：4 tick 打包值 -> 红石信号强度（选哪张唱片）。
/// 编码用的是**反查**：以本数组的值查下标，下标才是信号强度。
pub(crate) const GREY_SIGNAL: [u8; 16] = [
    0, 4, 5, 7, 6, 1, 3, 2,
    10, 11, 9, 8, 12, 13, 15, 14,
];

pub(crate) const WALL_DISTANCE: i32 = 135;
pub(crate) const VERTICAL_SPACING: i32 = 8;
pub(crate) const GLOBAL_Y_OFFSET: i32 = -27;

/// 信号强度 1..15 对应的唱片物品名（官方 `signalStrengthToDiscName`，0 号占位不用）。
pub(crate) const DISC_BY_SIGNAL: [&str; 16] = [
    "",
    "minecraft:music_disc_13",
    "minecraft:music_disc_cat",
    "minecraft:music_disc_blocks",
    "minecraft:music_disc_chirp",
    "minecraft:music_disc_far",
    "minecraft:music_disc_mall",
    "minecraft:music_disc_mellohi",
    "minecraft:music_disc_stal",
    "minecraft:music_disc_strad",
    "minecraft:music_disc_ward",
    "minecraft:music_disc_11",
    "minecraft:music_disc_wait",
    "minecraft:music_disc_precipice",
    "minecraft:music_disc_otherside",
    "minecraft:music_disc_5",
];

/// 暂停计数的羊毛颜色表（官方 `woolBlockIds`，下标 = 第几次「暂停 -> 唱片」切换）。
pub(crate) const WOOL_BY_INCREMENT: [&str; 16] = [
    "minecraft:white_wool",
    "minecraft:orange_wool",
    "minecraft:magenta_wool",
    "minecraft:light_blue_wool",
    "minecraft:yellow_wool",
    "minecraft:lime_wool",
    "minecraft:pink_wool",
    "minecraft:gray_wool",
    "minecraft:light_gray_wool",
    "minecraft:cyan_wool",
    "minecraft:purple_wool",
    "minecraft:blue_wool",
    "minecraft:brown_wool",
    "minecraft:green_wool",
    "minecraft:red_wool",
    "minecraft:black_wool",
];

/// 乐器索引 0..19 的发声方块（官方 `instrumentBlockIds`，前 16 个即 NBS 内置乐器）。
pub(crate) const INSTRUMENT_BASE_BLOCKS: [&str; 20] = [
    "minecraft:dirt",
    "minecraft:oak_planks",
    "minecraft:stone",
    "minecraft:sand",
    "minecraft:glass",
    "minecraft:white_wool",
    "minecraft:clay",
    "minecraft:gold_block",
    "minecraft:packed_ice",
    "minecraft:bone_block",
    "minecraft:iron_block",
    "minecraft:soul_sand",
    "minecraft:pumpkin",
    "minecraft:emerald_block",
    "minecraft:hay_block",
    "minecraft:glowstone",
    "minecraft:dragon_head",
    "minecraft:creeper_head",
    "minecraft:skeleton_skull",
    "minecraft:zombie_head",
];

/// 官方 `directionSectionToInstrument`：`[方向][分区] -> 乐器索引`。
/// 方向顺序 north/south/east/west、分区顺序 top/middle/bottom/percussion-left/
/// percussion-right/mob-head，与转换器 `findInstrumentPositions` 的遍历顺序一致：
/// 同一乐器占多个位置时，取先匹配到的那个。
pub(crate) const DIR_SECTION_INSTRUMENT: [[u8; 6]; 4] = [
    [15, 7, 8, 2, 11, 17],    // north
    [6, 10, 9, 4, 3, 18],     // south
    [13, 12, 14, 11, 4, 19],  // east
    [5, 1, 0, 3, 2, 16],      // west
];

/// 官方 `mobSoundList` 展开后的「采样文件名 -> 怪物头乐器索引」。
/// 自定义乐器只有命中这张表才会被改写，否则整颗音符丢弃（内置 16..19 也走这条，
/// 于是铜块一族全被丢掉——和转换器一致）。
///
/// ⚠ 本模块里的乐器索引一律是**转换器/面板**那套（0..15 原版乐器、
/// 16..19 四颗怪物头，见 [`INSTRUMENT_BASE_BLOCKS`] 与 [`DIR_SECTION_INSTRUMENT`]），
/// 和 NBS 文件里的 16..19（MC 26.1 的铜块一族）不是同一批东西。
/// [`transcribe_events`] 就是这两套编号之间唯一的桥。
const MOB_SOUNDS: [(&str, u8); 11] = [
    ("minecraft/mob/enderdragon/growl1.ogg", 16),
    ("minecraft/mob/enderdragon/growl2.ogg", 16),
    ("minecraft/mob/enderdragon/growl3.ogg", 16),
    ("minecraft/mob/enderdragon/growl4.ogg", 16),
    ("minecraft/random/fuse.ogg", 17),
    ("minecraft/mob/skeleton/say1.ogg", 18),
    ("minecraft/mob/skeleton/say2.ogg", 18),
    ("minecraft/mob/skeleton/say3.ogg", 18),
    ("minecraft/mob/zombie/say1.ogg", 19),
    ("minecraft/mob/zombie/say2.ogg", 19),
    ("minecraft/mob/zombie/say3.ogg", 19),
];

/// 箱子 + 朝向 + 左右八态，表序即 palette 编号顺序（north/south/east/west × left/right）。
const CHEST_PALETTE: [&str; 8] = [
    "minecraft:chest[facing=north,type=left]",
    "minecraft:chest[facing=north,type=right]",
    "minecraft:chest[facing=south,type=left]",
    "minecraft:chest[facing=south,type=right]",
    "minecraft:chest[facing=east,type=left]",
    "minecraft:chest[facing=east,type=right]",
    "minecraft:chest[facing=west,type=left]",
    "minecraft:chest[facing=west,type=right]",
];
/// 面板牌匾用的墙上告示牌（节目单本身不放牌子，只在 palette 里备着）。
/// 插入顺序照上游：east/west/north/south。
const SIGN_PALETTE: [&str; 4] = [
    "minecraft:oak_wall_sign[facing=east]",
    "minecraft:oak_wall_sign[facing=west]",
    "minecraft:oak_wall_sign[facing=north]",
    "minecraft:oak_wall_sign[facing=south]",
];

/// palette 编号一律按表序连着排（0..条目数），这是端侧改上游的一处：上游
/// `customPaletteBlockIds` 把箱子/告示牌/空气/堆肥桶摆在 100..121，而投影模组
/// （Litematica）的 Sponge 读取器按 `id < Palette 条目数` 校验，稀疏编号会被判成
/// 「Invalid ID in the Sponge schematic palette」，整份 `.schem` 直接读不进来。
/// 编号只是文件内部的索引，机体读的是箱子里的物品，改掉不影响节目单语义。
const CHEST_PAL_BASE: i8 = INSTRUMENT_BASE_BLOCKS.len() as i8;
const SIGN_PAL_BASE: i8 = CHEST_PAL_BASE + CHEST_PALETTE.len() as i8;
/// 空气：区域里没铺到方块的格子。
const PAL_AIR: i8 = SIGN_PAL_BASE + SIGN_PALETTE.len() as i8;
/// 「这个音高没有音符」的占位方块（上游 `noteNotUsedBlockId`，堆肥桶）。
const PAL_COMPOSTER: i8 = PAL_AIR + 1;

const LAYOUT_ROWS: usize = 8;
const LAYOUT_COLS: usize = 14;

/// 机体区域尺寸：宽 = 深 = 2×135 + 1 + 2，高 = 8 行 × 8 格。
pub(crate) const REGION_W: i32 = WALL_DISTANCE * 2 + 1 + 2;
pub(crate) const REGION_D: i32 = REGION_W;
pub(crate) const REGION_H: i32 = LAYOUT_ROWS as i32 * VERTICAL_SPACING;

/// 单个「乐器 + 音高」最多能装的物品堆栈数：2 只箱子 × 27 格 × 27 只潜影盒。
const MAX_STACKS_PER_NOTE: usize = 27 * 27 * 2;

// ======================================================== 选项与导出报告

/// 时间轴取整模式，对应转换器 `timingSelect` 的四项。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TimingMode {
    /// 官方默认：把「每个原 tick 占几个机体 tick」取整。
    #[default]
    Round,
    /// 按 `20 / tempo` 精确摊开（格距可以是小数）。
    Approximate,
    /// 跟随曲内 Tempo Changer 逐段换算，撞格时叠到下一层。
    Flexible,
    /// 完全不动时间轴（tempo 不是 20 t/s 时播放速度会错）。
    None,
}

impl TimingMode {
    pub const ALL: [TimingMode; 4] = [
        TimingMode::Round,
        TimingMode::Approximate,
        TimingMode::Flexible,
        TimingMode::None,
    ];

    pub fn index(self) -> usize {
        match self {
            TimingMode::Round => 0,
            TimingMode::Approximate => 1,
            TimingMode::Flexible => 2,
            TimingMode::None => 3,
        }
    }

    pub fn from_index(v: usize) -> TimingMode {
        TimingMode::ALL.get(v).copied().unwrap_or_default()
    }

    /// 选择框文案。上游 UI 的下拉项就是这四个英文词，中文里也照抄，
    /// 免得和机体文档对不上。
    pub fn label(self) -> &'static str {
        match self {
            TimingMode::Round => "round",
            TimingMode::Approximate => "approximate",
            TimingMode::Flexible => "flexible",
            TimingMode::None => "none",
        }
    }

    /// 选项的一句话解释。
    pub fn hint(self) -> String {
        msg(
            match self {
                TimingMode::Round => "Round each note's slot to a whole number of machine ticks.",
                TimingMode::Approximate => {
                    "Spread notes exactly by 20 / tempo; timing stays true, spacing may be uneven."
                }
                TimingMode::Flexible => {
                    "Follow Tempo Changer notes; collisions stack onto extra layers."
                }
                TimingMode::None => {
                    "Keep the original ticks; the machine then plays at the wrong speed unless tempo is 20."
                }
            },
            match self {
                TimingMode::Round => "把每颗音符占用的机体 tick 数取成整数。",
                TimingMode::Approximate => "严格按 20 / tempo 摊开：速度准确，但格距可能不均。",
                TimingMode::Flexible => "跟随曲内 Tempo Changer 换算；撞格时叠到额外图层。",
                TimingMode::None => {
                    "原样保留 tick；除非速度本就是 20 t/s，否则机器会以错误速度播放。"
                }
            },
        )
    }
}

/// MAESTRO 导出选项，随 `settings.ini` 的 `[maestro_export]` 段持久化。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MaestroOptions {
    /// 时间轴取整模式，出厂 `round`（同官方转换器默认）。
    pub timing: TimingMode,
}

/// 导出过程中对歌曲做过的改写与产出统计。
#[derive(Clone, Debug, Default)]
pub struct MaestroReport {
    pub timing: TimingMode,
    /// 歌曲原速度（ticks/秒）。
    pub tempo_from: f32,
    /// 每个原 tick 实际占用的机体 tick 数（`1 + tempoDelta`；压缩模式无意义）。
    pub stretch: f64,
    /// >0 表示提速压缩：每 `compression` 个 tick 只取第 1 个。
    pub compression: usize,
    /// 被 ±12 移调拉回 33..=57 的音符数。
    pub transposed: usize,
    /// 自定义乐器采样命中怪物头、成功改写的音符数。
    pub remapped: usize,
    /// 自定义乐器命中不了怪物头、被丢弃的音符数。
    pub dropped_custom: usize,
    /// 提速压缩跳过的音符数。
    pub dropped_compressed: usize,
    /// 锁定图层上的音符数（转换器不看锁定，这里只报告）。
    pub locked_notes: usize,
    /// 有内容的「乐器 + 音高」流数量（每条占偶/奇两条流）。
    pub streams: usize,
    /// 实际装东西的箱子对数。
    pub chest_pairs: usize,
    /// 写出物品列表的箱子（方块实体）数。
    pub block_entities: usize,
}

impl MaestroReport {
    /// 变化说明：只列真正发生过的改写，给导出对话框直接显示。
    pub fn lines(&self) -> Vec<String> {
        let tempo = trim_num(self.tempo_from as f64);
        let mut v = Vec::new();
        if (self.tempo_from - 20.0).abs() > f32::EPSILON {
            match self.timing {
                TimingMode::None => v.push(msg(
                    format!("Tempo {tempo} t/s is kept as-is; the machine runs at 20 t/s, so playback speed will be wrong."),
                    format!("速度 {tempo} t/s 原样保留；机体按 20 t/s 运转，播放速度会不对。"),
                )),
                TimingMode::Flexible => v.push(msg(
                    "Timing follows the Tempo Changer notes in the song.",
                    "时间轴按曲内 Tempo Changer 换算。",
                )),
                _ if self.compression > 0 => v.push(msg(
                    format!("Tempo {tempo} t/s is faster than the machine's 20 t/s: keeping 1 of every {} ticks.", self.compression),
                    format!("速度 {tempo} t/s 高于机体的 20 t/s：每 {} 个 tick 只取 1 个。", self.compression),
                )),
                _ => v.push(msg(
                    format!("Tempo {tempo} t/s rewritten onto the machine's 20 t/s grid (1 tick = {} machine ticks).", trim_num(self.stretch)),
                    format!("速度 {tempo} t/s 已摊到机体的 20 t/s 网格：1 tick = {} 机体 tick。", trim_num(self.stretch)),
                )),
            }
        }
        if self.transposed > 0 {
            v.push(msg(
                format!("{} notes outside the machine's range were transposed by octaves.", self.transposed),
                format!("{} 个音符超出机体音域，已按八度移调。", self.transposed),
            ));
        }
        if self.remapped > 0 {
            v.push(msg(
                format!("{} notes were remapped to mob heads by their sample file.", self.remapped),
                format!("{} 个音符按采样文件名改写成怪物头。", self.remapped),
            ));
        }
        if self.dropped_custom > 0 {
            v.push(msg(
                format!("{} custom-instrument notes have no mob-head equivalent and were dropped.", self.dropped_custom),
                format!("{} 个自定义乐器音符没有对应的怪物头，已丢弃。", self.dropped_custom),
            ));
        }
        if self.dropped_compressed > 0 {
            v.push(msg(
                format!("{} notes were skipped by the tempo compression.", self.dropped_compressed),
                format!("{} 个音符被提速压缩跳过。", self.dropped_compressed),
            ));
        }
        if self.locked_notes > 0 {
            v.push(msg(
                format!("{} notes sit on locked layers; the MAESTRO sheet includes them anyway.", self.locked_notes),
                format!("{} 个音符在锁定图层上；节目单仍然会导出它们（与官方转换器一致）。", self.locked_notes),
            ));
        }
        v
    }
}

/// `1.75` 打印成 `1.75`，`2.0` 打印成 `2`。
fn trim_num(v: f64) -> String {
    if v.is_finite() && (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// 带插值的文案：`tr` 借的是入参的生命周期，所以 `format!` 出来的串要走这里落成 owned。
fn msg<T: Into<String>>(en: T, zh: T) -> String {
    if crate::lang() == 1 {
        zh.into()
    } else {
        en.into()
    }
}

// ============================================================== 坐标数学

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dir {
    North,
    South,
    East,
    West,
}

impl Dir {
    /// `rotateDirectionQuarterClockwise` 的表序（顺时针）：北 -> 东 -> 南 -> 西。
    fn cw_index(self) -> usize {
        match self {
            Dir::North => 0,
            Dir::East => 1,
            Dir::South => 2,
            Dir::West => 3,
        }
    }

    fn from_cw_index(i: usize) -> Dir {
        [Dir::North, Dir::East, Dir::South, Dir::West][i % 4]
    }

    fn rotate_cw(self, steps: i32) -> Dir {
        Dir::from_cw_index((self.cw_index() as i32 + steps).rem_euclid(4) as usize)
    }

    /// `DIR_SECTION_INSTRUMENT` 的行号（JS 字面量顺序，不是顺时针序）。
    fn table_row(self) -> usize {
        match self {
            Dir::North => 0,
            Dir::South => 1,
            Dir::East => 2,
            Dir::West => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Coord {
    x: i32,
    y: i32,
    z: i32,
}

/// `coordinateOffset90DegBasedOnDirection`：沿面朝方向的切线移动。
fn offset_90(c: Coord, offset: i32, facing: Dir) -> Coord {
    match facing {
        Dir::South => Coord { x: c.x - offset, ..c },
        Dir::North => Coord { x: c.x + offset, ..c },
        Dir::East => Coord { z: c.z + offset, ..c },
        Dir::West => Coord { z: c.z - offset, ..c },
    }
}

/// `coordinateOffset`：沿面朝方向移动。
fn offset_along(c: Coord, facing: Dir, offset: i32) -> Coord {
    match facing {
        Dir::South => Coord { z: c.z + offset, ..c },
        Dir::North => Coord { z: c.z - offset, ..c },
        Dir::East => Coord { x: c.x + offset, ..c },
        Dir::West => Coord { x: c.x - offset, ..c },
    }
}

/// `getInRegionCoordinates`：把「面朝这面墙时的面板格」换成机体区域坐标。
fn in_region_coords(facing: Dir, local_x: usize, local_y: usize) -> Coord {
    // 上游自称 "here be dragons"，那两个 -2 / -4 的凑位偏移逐条照抄。
    let adjusted_x = local_x as i32 * 4
        + (WALL_DISTANCE - 4 * (LAYOUT_COLS / 2) as i32)
        + 1
        + if facing == Dir::South || facing == Dir::West {
            -2
        } else {
            0
        }
        - 4;
    // 面板 y=0 在顶部，世界 y=0 在底部。
    let inverted_y = (LAYOUT_ROWS as i32 - 1 - local_y as i32) * VERTICAL_SPACING;
    let full = WALL_DISTANCE * 2;
    match facing {
        Dir::South => Coord { x: full - adjusted_x, y: inverted_y, z: full + 1 },
        Dir::West => Coord { x: 1, y: inverted_y, z: full - adjusted_x },
        Dir::North => Coord { x: adjusted_x, y: inverted_y, z: 1 },
        Dir::East => Coord { x: full + 1, y: inverted_y, z: adjusted_x },
    }
}

/// `[top, middle, bottom, percussion-left, percussion-right, mob-head]` 对应的面板字母。
/// 注意 perc-left 是 E、perc-right 是 D，与上游一致（别按字母顺序想当然）。
const SECTION_LETTER: [char; 6] = ['A', 'B', 'C', 'E', 'D', 'M'];

/// `getLocalCoordinates`：某个「乐器 + 音高」在读盘器面板上的格子。
/// 同一乐器占多个位置时，取 `DIR_SECTION_INSTRUMENT` 遍历顺序里先出现的那个。
pub(crate) fn local_coords(instrument: u8, note: u8) -> Option<(usize, usize, Dir)> {
    if note > 24 {
        return None;
    }
    for dir in [Dir::North, Dir::South, Dir::East, Dir::West] {
        for (si, letter) in SECTION_LETTER.iter().enumerate() {
            if DIR_SECTION_INSTRUMENT[dir.table_row()][si] != instrument {
                continue;
            }
            let pattern = if *letter == 'M' {
                "M".to_string()
            } else {
                format!("{letter}{note:02}")
            };
            for (y, row) in DISC_LAYOUT.iter().enumerate() {
                for (x, cell) in row.iter().enumerate() {
                    if cell.trim() == pattern {
                        return Some((x, y, dir));
                    }
                }
            }
        }
    }
    None
}

/// `createAccessIndexFunction`：区域坐标 -> `Data` 数组下标；越界返回 None。
fn access_index(c: Coord) -> Option<usize> {
    let i = c.x as i64
        + REGION_W as i64 * c.z as i64
        + c.y as i64 * REGION_W as i64 * REGION_D as i64;
    let total = REGION_W as i64 * REGION_D as i64 * REGION_H as i64;
    if i < 0 || i >= total {
        return None;
    }
    Some(i as usize)
}

/// `getChestPaletteId`：MC 的 left/right 与坐标朝向相反，照抄上游的绕法。
fn chest_palette_id(left: bool, facing: Dir) -> i8 {
    // 表序是 north/south/east/west，而读盘器给的朝向正好错一对：
    // south 朝向的读盘器把箱子摆在南墙上，取的是 facing=north 的那一对编号。
    let pair = match facing {
        Dir::South => 0,
        Dir::North => 1,
        Dir::West => 2,
        Dir::East => 3,
    };
    CHEST_PAL_BASE + pair * 2 + if left { 0 } else { 1 }
}

// ============================================================ 物品编码

/// 一格物品：一张唱片（信号强度），或一串「暂停」（羊毛计数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Disc(u8),
    Pause(u32),
}

/// `streamToItemRepresentation`：灰度值流 -> 物品槽位。
/// 上游把「超过 64」的判断放在循环开头，于是一段 65 格的暂停会拆成 64 + 1 两组，
/// 这里保持同样的切分次序。
fn item_slots(stream: &[u8]) -> Result<Vec<Slot>, String> {
    const MIN_ITEMS: usize = 3;
    let mut slots: Vec<Slot> = Vec::new();
    let mut pause: u32 = 0;
    for &v in stream {
        if pause > 64 {
            slots.push(Slot::Pause(64));
            pause -= 64;
        }
        if v == 0 {
            pause += 1;
            continue;
        }
        if !(1..=15).contains(&v) {
            return Err(msg(
                format!("Internal error: grey value {v} cannot be turned into a disc."),
                format!("内部错误：灰度值 {v} 无法换算成唱片。"),
            ));
        }
        if pause > 0 {
            slots.push(Slot::Pause(pause));
            pause = 0;
        }
        slots.push(Slot::Disc(v));
    }
    // 最后一只潜影盒至少要有 3 件物品，否则机体的红石不认。
    let in_last = slots.len() % 27;
    if in_last < MIN_ITEMS {
        // 上游 `slice(-0)` 拿到的是整个数组，这里照做。
        let tail: &[Slot] = if in_last == 0 {
            &slots[..]
        } else {
            &slots[slots.len() - in_last..]
        };
        let count: usize = tail
            .iter()
            .map(|s| match s {
                Slot::Pause(n) => *n as usize,
                Slot::Disc(_) => 1,
            })
            .sum();
        if count < MIN_ITEMS {
            slots.push(Slot::Pause((MIN_ITEMS - count) as u32));
        }
    }
    Ok(slots)
}

/// 改写后的一颗粒：`(机体 tick, 乐器 0..19, 音高 0..24)`。
type Event = (u32, u8, u8);
/// 全部有内容的流：`(乐器, 音高) -> [偶数 tick 流, 奇数 tick 流]`，流里是灰度值 0..15。
type Streams = BTreeMap<(u8, u8), [Vec<u8>; 2]>;
/// 一只潜影盒里的一个槽位：`(物品名, 数量, 盒内槽位)`。
type Item = (&'static str, i32, i32);
/// 一只潜影盒（最多 27 个槽位）。
type Shulker = Vec<Item>;
/// 一只箱子：27 格里各自装着哪只潜影盒。
type Chest = Vec<Shulker>;

/// `createShulkerBoxContainerListFromItemRepresentations`：
/// 每经历一次「暂停 -> 唱片」的切换，下一段暂停就换一种羊毛颜色。
fn shulker_container(items: &[Slot]) -> Result<Shulker, String> {
    if items.len() > 27 {
        return Err(msg(
            "Internal error: more than 27 stacks in one shulker box.",
            "内部错误：单只潜影盒超过 27 组物品。",
        ));
    }
    let mut out: Shulker = Vec::new();
    let mut increment: usize = 0;
    let mut currently_pause = false;
    for (index, item) in items.iter().enumerate() {
        match item {
            Slot::Pause(count) => {
                currently_pause = true;
                let id = WOOL_BY_INCREMENT.get(increment).ok_or_else(|| {
                    msg(
                        "Internal error: more pause-to-disc switches than wool colours.",
                        "内部错误：暂停到唱片的切换次数超过羊毛颜色表。",
                    )
                })?;
                out.push((id, *count as i32, index as i32));
            }
            Slot::Disc(signal) => {
                if currently_pause {
                    increment += 1;
                    currently_pause = false;
                }
                out.push((DISC_BY_SIGNAL[*signal as usize], 1, index as i32));
            }
        }
    }
    Ok(out)
}

/// `streamToDoubleChestContents`：一条流最多撑满 2 只箱子（各 27 格、每格一只潜影盒）。
fn double_chest_contents(stream: &[u8]) -> Result<Vec<Chest>, String> {
    let items = item_slots(stream)?;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    if items.len() > MAX_STACKS_PER_NOTE {
        return Err(msg(
            "Too many notes: one instrument + pitch cannot hold more than 27 × 27 × 2 item stacks. Shorten the song or split it.",
            "音符过多：单个「乐器 + 音高」装不下超过 27 × 27 × 2 组物品，请缩短曲子或拆分。",
        ));
    }
    let mut chests: Vec<Chest> = Vec::new();
    for per_chest in items.chunks(27 * 27) {
        let mut shulkers: Chest = Vec::new();
        for group in per_chest.chunks(27) {
            shulkers.push(shulker_container(group)?);
        }
        chests.push(shulkers);
    }
    Ok(chests)
}

// ============================================== 阶段二：布尔流 -> 灰度流

/// 4 tick 打包值 -> 信号强度（`getReverseGrayCode`，灰度表是双射所以必有解）。
fn signal_of(packed: u8) -> u8 {
    (0..16u8)
        .find(|k| GREY_SIGNAL[*k as usize] == packed)
        .unwrap_or(0)
}

/// `separateEveryOther` + `removeTrailingFalse` + `padToMultipleOf4` + `encodeStream`。
/// 偶数 tick 与奇数 tick 各走一条独立的流（机体两边同时读盘）。
pub(crate) fn encode_stream(bits: &[bool]) -> [Vec<u8>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    for (half, sink) in out.iter_mut().enumerate() {
        let picked: Vec<bool> = bits
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 2 == half)
            .map(|(_, b)| *b)
            .collect();
        let trimmed_len = match picked.iter().rposition(|b| *b) {
            Some(last) => last + 1,
            None => 0,
        };
        let mut padded = picked[..trimmed_len].to_vec();
        let pad = (4 - padded.len() % 4) % 4;
        padded.resize(padded.len() + pad, false);
        for group in padded.chunks(4) {
            let packed = group
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, b)| acc | ((*b as u8) << (3 - i)));
            sink.push(signal_of(packed));
        }
    }
    out
}

// ==================================================== 阶段一：音符改写

/// `calculateTempoDelta`：每个原 tick 额外占用多少机体 tick。
pub(crate) fn tempo_delta(tempo: f64, timing: TimingMode) -> f64 {
    if timing == TimingMode::None {
        return 0.0;
    }
    let cutoff = 0.75;
    if tempo <= 20.0 {
        if timing == TimingMode::Round {
            return (20.0 / tempo + cutoff).floor() - 1.0;
        }
        return 20.0 / tempo - 1.0;
    }
    // 20 t/s 以上一律走压缩，四个模式在这一支没有区别。
    -(2f64.powf((tempo / (20.0 * (1.0 + cutoff))).log2().floor() + 1.0)) + 1.0
}

/// 曲子里所有 Tempo Changer 的乐器索引（官方按乐器名判断，不看采样文件）。
fn tempo_changer_ids(song: &Song) -> Vec<u8> {
    song.custom_instruments
        .iter()
        .enumerate()
        .filter(|(_, ci)| ci.name == TEMPO_CHANGER_NAME)
        .filter_map(|(i, _)| u8::try_from(FIRST_CUSTOM_INDEX as usize + i).ok())
        .map(|id| id - 1)
        .collect()
}

/// `getTempoSegments`：tick -> 该 tick 起生效的速度（同一 tick 取更靠上的图层）。
fn tempo_segments(song: &Song, changers: &[u8]) -> BTreeMap<u32, f64> {
    let mut seg: BTreeMap<u32, f64> = BTreeMap::new();
    if !changers.is_empty() {
        for layer in (0..song.layers.len()).rev() {
            for (&tick, col) in &song.columns {
                let Some(note) = col.notes.get(&(layer as u32)) else {
                    continue;
                };
                if !changers.contains(&note.instrument) || seg.contains_key(&tick) {
                    continue;
                }
                // 音分字段在这里被当作 BPM 用：pitch = BPM = (t/s) × 15。
                seg.insert(tick, (note.pitch.unsigned_abs() as f64) / 15.0);
            }
        }
    }
    seg.entry(0).or_insert(song.meta.tempo as f64);
    seg
}

/// `getTickMap`：原 tick -> 换算后的浮点 tick（0 号固定为 0）。
fn tick_map(song: &Song, seg: &BTreeMap<u32, f64>) -> Vec<f64> {
    let mut current = song.meta.tempo as f64;
    let mut map = vec![0.0f64];
    for tick in 1..=song.song_length() {
        if let Some(&t) = seg.get(&tick) {
            current = t;
        }
        let step = if current == 0.0 {
            f64::INFINITY
        } else {
            20.0 / current
        };
        map.push(map[map.len() - 1] + step);
    }
    map
}

/// 阶段一的计数：哪些音符被改写、被丢弃。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AdjustCounts {
    pub transposed: usize,
    pub remapped: usize,
    pub dropped_custom: usize,
    pub dropped_compressed: usize,
    pub locked_notes: usize,
}

/// `validateAndAdjustSong` + `createAdjustedSong` + `convertNBStoStreams` 的音符部分：
/// 产出 `(机体 tick, 乐器 0..19, 音高 0..24)` 事件表。
pub(crate) fn transcribe_events(
    song: &Song,
    timing: TimingMode,
) -> Result<(Vec<Event>, AdjustCounts), String> {
    let mut counts = AdjustCounts::default();
    let delta = tempo_delta(song.meta.tempo as f64, timing);
    let compression = if delta < 0.0 {
        (-delta + 1.0) as usize
    } else {
        0
    };
    let changers = tempo_changer_ids(song);
    let seg = (timing == TimingMode::Flexible).then(|| tempo_segments(song, &changers));
    let map = seg
        .as_ref()
        .map(|s| tick_map(song, s))
        .unwrap_or_default();
    let max_stacked = if let Some(seg) = seg.as_ref() {
        let max_tempo = seg.values().cloned().fold(f64::NEG_INFINITY, f64::max);
        ((max_tempo / 20.0).ceil() as usize).max(1)
    } else {
        1
    };
    let locked = song.locked_layers();
    // 官方 `mobInstruments` 查表：自定义乐器 -> 怪物头；查不到就丢。
    let remap_target = |instrument: u8| -> Option<u8> {
        let custom = (instrument as usize)
            .checked_sub(FIRST_CUSTOM_INDEX as usize)
            .and_then(|i| song.custom_instruments.get(i))?;
        MOB_SOUNDS
            .iter()
            .find(|(file, _)| *file == custom.filename)
            .map(|(_, id)| *id)
    };

    let mut events: Vec<(u32, u8, u8)> = Vec::new();
    // 官方把图层倒过来（自上而下）处理，只影响撞格叠层的先后；
    // 保持同序，方便和上游逐字节对照。
    for layer in (0..song.layers.len()).rev() {
        let layer_locked = locked.get(layer).copied().unwrap_or(false);
        let mut current_stacked: i32 = -1;
        let mut last_populated: i64 = 0;
        for (&tick, col) in &song.columns {
            let Some(note) = col.notes.get(&(layer as u32)) else {
                continue;
            };
            if layer_locked {
                counts.locked_notes += 1;
            }
            // 提速压缩：只留每 compression 个 tick 的第一颗。
            if timing != TimingMode::Flexible
                && compression > 0
                && tick % compression as u32 != 0
            {
                counts.dropped_compressed += 1;
                continue;
            }
            let instrument = if note.instrument > 15 {
                match remap_target(note.instrument) {
                    Some(id) => {
                        counts.remapped += 1;
                        id
                    }
                    None => {
                        counts.dropped_custom += 1;
                        continue;
                    }
                }
            } else {
                note.instrument
            };
            // 音高拉回 33..=57：±12 反复，直到落进窗内。
            let mut key = note.key as i32;
            while key < 33 {
                key += 12;
            }
            while key > 57 {
                key -= 12;
            }
            if key != note.key as i32 {
                counts.transposed += 1;
            }
            let note_id = (key - 33) as u8;

            let adjusted: i64 = if timing != TimingMode::Flexible {
                if compression > 0 {
                    (tick / compression as u32) as i64
                } else {
                    // 上游写的是 `tick + tick * delta`，浮点末位按同序算。
                    ((tick as f64) + (tick as f64) * delta).floor() as i64
                }
            } else {
                let raw = map.get(tick as usize).copied().unwrap_or(0.0);
                if !raw.is_finite() || raw < 0.0 {
                    return Err(msg(
                        "Internal error: a tempo change produced an invalid tick.",
                        "内部错误：速度变更换算出了非法 tick。",
                    ));
                }
                let rounded = raw.round() as i64;
                if rounded <= last_populated {
                    current_stacked += 1;
                } else {
                    current_stacked = 0;
                }
                if current_stacked < 0 || current_stacked as usize >= max_stacked {
                    return Err(msg(
                        format!(
                            "Tempo changes collide on more than {max_stacked} notes at the same machine tick. Use round or approximate instead of flexible."
                        ),
                        format!(
                            "同一机体 tick 上叠了超过 {max_stacked} 颗音符：flexible 模式撑不住，请改用 round 或 approximate。"
                        ),
                    ));
                }
                rounded
            };
            last_populated = adjusted;
            events.push((adjusted as u32, instrument, note_id));
        }
    }
    Ok((events, counts))
}

/// 事件表 -> 布尔流表（只保留有音符的「乐器 + 音高」，等价上游删空流）。
pub(crate) fn bool_streams(events: &[Event]) -> BTreeMap<(u8, u8), Vec<bool>> {
    let song_tick_length = events.iter().map(|(t, _, _)| *t).max().unwrap_or(0) + 1;
    let mut streams: BTreeMap<(u8, u8), Vec<bool>> = BTreeMap::new();
    for (tick, ins, note) in events {
        streams
            .entry((*ins, *note))
            .or_insert_with(|| vec![false; song_tick_length as usize])[*tick as usize] = true;
    }
    streams
}

/// 阶段一 + 二：歌曲 -> `BTreeMap<(乐器, 音高), [灰度流; 2]>`。
pub(crate) fn encode_song(
    song: &Song,
    opts: &MaestroOptions,
) -> Result<(Streams, AdjustCounts), String> {
    let (events, counts) = transcribe_events(song, opts.timing)?;
    let mut encoded: Streams = BTreeMap::new();
    for (key, bits) in bool_streams(&events) {
        let gray = encode_stream(&bits);
        // 上游：整条全 0 的流直接不要。
        if gray[0].iter().any(|v| *v != 0) || gray[1].iter().any(|v| *v != 0) {
            encoded.insert(key, gray);
        }
    }
    Ok((encoded, counts))
}

// ==================================================== 阶段三：区域与 NBT

/// 一只箱子的落位与内容。
#[derive(Clone, Debug)]
pub(crate) struct ChestAt {
    coord: Coord,
    facing: Dir,
    left: bool,
    items: Chest,
}

/// 上游内容阶段的坐标推导：两条流各占一对相邻箱子（沿面板墙的切线错开 2 格）。
pub(crate) fn plan_chests(
    contents: &Streams,
) -> Result<(Vec<ChestAt>, usize), String> {
    let mut chests: Vec<ChestAt> = Vec::new();
    let mut pairs = 0usize;
    for ((ins, note), streams) in contents {
        let (lx, ly, facing) = local_coords(*ins, *note).ok_or_else(|| {
            msg(
                format!("Internal error: instrument {ins} pitch {note} is not on the reader panel."),
                format!("内部错误：乐器 {ins} 音高 {note} 不在读盘器面板上。"),
            )
        })?;
        let base = in_region_coords(facing, lx, ly);
        for (si, stream) in streams.iter().enumerate() {
            if stream.iter().all(|v| *v == 0) {
                continue;
            }
            pairs += 1;
            let origin = offset_90(base, if si == 0 { 0 } else { 2 }, facing);
            // 箱子摆在读盘器上方一格（VERTICAL_SPACING - 1）。
            let first = Coord { y: origin.y + (VERTICAL_SPACING - 1), ..origin };
            let second = offset_90(first, 1, facing);
            let all = double_chest_contents(stream)?;
            // MC 的左右与坐标朝向相反：第 1 只是 right 型，装前一半内容。
            for (coord, left) in [(first, false), (second, true)] {
                let items = all.get(if left { 1 } else { 0 }).cloned().unwrap_or_default();
                chests.push(ChestAt { coord, facing, left, items });
            }
        }
    }
    Ok((chests, pairs))
}

/// 方块网格 + 方块实体 -> 未压缩的 WorldEdit v3 NBT 字节。
pub(crate) fn schematic_nbt(chests: &[ChestAt]) -> Result<Vec<u8>, String> {
    let cells = (REGION_W * REGION_D * REGION_H) as usize;
    let mut ids = vec![PAL_AIR; cells];
    let mut out_of_bounds: Option<Coord> = None;
    {
        let mut place = |c: Coord, id: i8| match access_index(c) {
            Some(i) => ids[i] = id,
            None => out_of_bounds = Some(c),
        };
        // 1) 预置：每个面板格子先摆上一对空输出箱，再沿法线铺 4 格堆肥桶占位。
        //    上游把 20 × 25 全组合走一遍，怪物头的 25 个音高会重复落到 M 那一格。
        for ins in 0..INSTRUMENT_BASE_BLOCKS.len() as u8 {
            for note in 0..25u8 {
                let Some((lx, ly, facing)) = local_coords(ins, note) else {
                    continue;
                };
                let reader = in_region_coords(facing, lx, ly);
                for off in [0, 2] {
                    let base = offset_90(reader, off, facing);
                    // 输出箱朝向机体中心，即面朝方向再逆时针 90°。
                    let looking = facing.rotate_cw(3);
                    let second = offset_90(base, 1, looking);
                    place(base, chest_palette_id(false, looking));
                    place(second, chest_palette_id(true, looking));
                }
                let normal = facing.rotate_cw(1);
                for i in 0..4 {
                    let c = offset_along(reader, normal, i);
                    place(Coord { y: c.y + (VERTICAL_SPACING - 1), ..c }, PAL_COMPOSTER);
                }
            }
        }
        // 2) 有内容的箱子覆盖占位（没有内容的那一侧留堆肥桶）。
        for chest in chests {
            place(chest.coord, chest_palette_id(chest.left, chest.facing));
        }
    }
    if let Some(c) = out_of_bounds {
        return Err(msg(
            format!("Internal error: coordinate ({}, {}, {}) falls outside the {REGION_W} × {REGION_H} × {REGION_D} region.", c.x, c.y, c.z),
            format!("内部错误：({}, {}, {}) 落在 {REGION_W} × {REGION_H} × {REGION_D} 区域之外。", c.x, c.y, c.z),
        ));
    }

    let mut n = Nbt::default();
    // 上游用 nbtify 写出的是「匿名根标签里再套一个 Schematic 复合标签」，
    // 它自己的 verify.ts 也按 `data.Schematic` 读取，这里逐字对齐。
    n.compound("");
    n.compound("Schematic");
    n.int("Version", 3);
    n.int("DataVersion", 4189); // Java 1.21.4
    n.short("Width", REGION_W as i16);
    n.short("Height", REGION_H as i16);
    n.short("Length", REGION_D as i16);
    n.int_array("Offset", &[-(REGION_W / 2), GLOBAL_Y_OFFSET, -(REGION_D / 2)]);
    n.compound("Blocks");
    n.compound("Palette");
    for (i, block) in INSTRUMENT_BASE_BLOCKS.iter().enumerate() {
        n.int(block, i as i32);
    }
    for (i, name) in CHEST_PALETTE.iter().enumerate() {
        n.int(name, (CHEST_PAL_BASE + i as i8) as i32);
    }
    for (i, name) in SIGN_PALETTE.iter().enumerate() {
        n.int(name, (SIGN_PAL_BASE + i as i8) as i32);
    }
    n.int("minecraft:air", PAL_AIR as i32);
    n.int("minecraft:composter", PAL_COMPOSTER as i32);
    n.end(); // Palette
    n.byte_array("Data", &ids.iter().map(|v| *v as u8).collect::<Vec<u8>>());
    // 方块实体：只给真装了物品的箱子写 Items。
    let filled: Vec<&ChestAt> = chests.iter().filter(|c| !c.items.is_empty()).collect();
    n.list("BlockEntities", filled.len() as i32, 10);
    for chest in &filled {
        n.string("Id", "minecraft:chest");
        n.int_array("Pos", &[chest.coord.x, chest.coord.y, chest.coord.z]);
        n.compound("Data");
        n.string("id", "minecraft:chest");
        // 每格一只潜影盒（count 恒为 1），盒内的 container 才是唱片与羊毛。
        n.list("Items", chest.items.len() as i32, 10);
        for (slot, shulker) in chest.items.iter().enumerate() {
            n.int("count", 1);
            n.byte("Slot", slot as u8);
            n.compound("components");
            n.list("minecraft:container", shulker.len() as i32, 10);
            for (id, count, index) in shulker.iter() {
                n.compound("item");
                n.string("id", id);
                n.int("count", *count);
                n.end(); // item
                n.int("slot", *index);
                n.end(); // container 元素
            }
            n.end(); // components
            n.string("id", "minecraft:shulker_box");
            n.end(); // Items 元素
        }
        n.end(); // Data
        n.end(); // BlockEntities 元素
    }
    n.end(); // Blocks
    n.end(); // Schematic
    n.end(); // 根
    Ok(n.buf)
}

// ========================================================== 对外接口

/// 只算不改盘：给导出对话框列出会发生哪些改写。
pub fn preview(song: &Song, opts: &MaestroOptions) -> Result<MaestroReport, String> {
    let (contents, counts) = encode_song(song, opts)?;
    let (chests, pairs) = plan_chests(&contents)?;
    Ok(report(song, opts, &counts, contents.len(), pairs, &chests))
}

/// 生成未压缩的 `.schem` NBT 字节与报告（测试与对照用）。
pub fn build_bytes(song: &Song, opts: &MaestroOptions) -> Result<(Vec<u8>, MaestroReport), String> {
    let (contents, counts) = encode_song(song, opts)?;
    let (chests, pairs) = plan_chests(&contents)?;
    let bytes = schematic_nbt(&chests)?;
    Ok((bytes, report(song, opts, &counts, contents.len(), pairs, &chests)))
}

/// 导出 MAESTRO 节目单到 `path`（gzip 后的 WorldEdit `.schem`）。
pub fn export(song: &Song, path: &Path, opts: &MaestroOptions) -> Result<MaestroReport, String> {
    let (bytes, report) = build_bytes(song, opts)?;
    let packed = crate::schematic::gzip(&bytes)?;
    std::fs::write(path, packed)
        .map_err(|e| msg(format!("Failed to write schematic: {e}"), format!("写入 schematic 失败: {e}")))?;
    Ok(report)
}

fn report(
    song: &Song,
    opts: &MaestroOptions,
    counts: &AdjustCounts,
    streams: usize,
    chest_pairs: usize,
    chests: &[ChestAt],
) -> MaestroReport {
    let delta = tempo_delta(song.meta.tempo as f64, opts.timing);
    MaestroReport {
        timing: opts.timing,
        tempo_from: song.meta.tempo,
        stretch: 1.0 + delta,
        compression: if delta < 0.0 {
            (-delta + 1.0) as usize
        } else {
            0
        },
        transposed: counts.transposed,
        remapped: counts.remapped,
        dropped_custom: counts.dropped_custom,
        dropped_compressed: counts.dropped_compressed,
        locked_notes: counts.locked_notes,
        streams,
        chest_pairs,
        block_entities: chests.iter().filter(|c| !c.items.is_empty()).count(),
    }
}

// ====================================================== 表格自校验（编译期）

const _: () = {
    /// 逐格统计各分区的音高覆盖，顺带记有没有 M 格。
    const fn collect() -> [[bool; 25]; 6] {
        let mut seen = [[false; 25]; 6];
        let mut y = 0;
        while y < LAYOUT_ROWS {
            let mut x = 0;
            while x < LAYOUT_COLS {
                let cell = DISC_LAYOUT[y][x].as_bytes();
                if cell.len() == 1 && cell[0] == b'M' {
                    seen[5][0] = true;
                } else if !(cell.len() == 3 && cell[0] == b' ' && cell[1] == b' ' && cell[2] == b' ')
                {
                    let section = match cell[0] {
                        b'A' => 0,
                        b'B' => 1,
                        b'C' => 2,
                        b'D' => 3,
                        b'E' => 4,
                        _ => panic!("layout cell has an unknown section"),
                    };
                    if cell.len() != 3 || cell[1] < b'0' || cell[1] > b'9' || cell[2] < b'0'
                        || cell[2] > b'9'
                    {
                        panic!("layout cell must be 3 characters");
                    }
                    let n = (cell[1] - b'0') * 10 + (cell[2] - b'0');
                    if n > 24 {
                        panic!("layout cell note out of range");
                    }
                    if seen[section][n as usize] {
                        panic!("duplicate layout cell");
                    }
                    seen[section][n as usize] = true;
                }
                x += 1;
            }
            y += 1;
        }
        seen
    }

    let seen = collect();
    let mut section = 0;
    while section < 3 {
        let mut n = 0;
        while n < 25 {
            assert!(seen[section][n], "A/B/C must cover notes 00-24");
            n += 1;
        }
        section += 1;
    }
    let mut percussion = 0;
    let mut n = 0;
    while n < 25 {
        // D、E 两区合起来必须铺满 00..24：打击乐器的搜索会先在 E 区找，
        // 找不到再落到 D 区，缺一个号就等于把那个音从面板上抹掉。
        assert!(seen[3][n] || seen[4][n], "D + E must cover every note 00-24");
        if seen[3][n] {
            percussion += 1;
        }
        if seen[4][n] {
            percussion += 1;
        }
        n += 1;
    }
    assert!(percussion == 25, "D + E must hold exactly 25 notes");
    let total = 75 + percussion + if seen[5][0] { 1 } else { 0 };
    assert!(total == 101, "reader count must be 100 + the mob-head cell");

    // 灰度表必须是 0..15 的双射，反查才不会有空洞。
    let mut hit = [false; 16];
    let mut i = 0;
    while i < 16 {
        let v = GREY_SIGNAL[i];
        assert!(v < 16, "grey code out of range");
        assert!(!hit[v as usize], "grey code table is not a bijection");
        hit[v as usize] = true;
        i += 1;
    }

    // 乐器表 20 项、唱片 15 项、羊毛 16 项，与上游一致。
    assert!(INSTRUMENT_BASE_BLOCKS.len() == 20, "20 instrument blocks");
    assert!(WOOL_BY_INCREMENT.len() == 16, "16 wool colours");
    assert!(DISC_BY_SIGNAL.len() == 16, "15 discs plus a gap");
    assert!(CHEST_PALETTE.len() == 8, "8 chest states");
    assert!(SIGN_PALETTE.len() == 4, "4 wall sign states");
    // palette 编号由表序推导，密集性不会走偏；剩下的红线只有一条：
    // 最大编号必须 < 128，`Data` 是 TAG_Byte_Array，每格一个字节才读得成单字节 varint。
    assert!((PAL_COMPOSTER as i32) < 128, "palette ids must stay single-byte varints");
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::Value;
    use nbs_core::model::{CustomInstrument, Layer, LockMode, Note};

    fn song(tempo: f32) -> Song {
        let mut s = Song::new();
        s.meta.tempo = tempo;
        s
    }

    #[test]
    fn layout_cells_resolve_in_the_upstream_traversal_order() {
        // glowstone = 15 号乐器，只在 north/top 出现 -> A 区。
        let (x, y, dir) = local_coords(15, 0).unwrap();
        assert_eq!(dir, Dir::North);
        assert_eq!(DISC_LAYOUT[y][x], "A00");
        // hay_block = 14 只在 east/bottom -> C 区。
        let (x, y, dir) = local_coords(14, 5).unwrap();
        assert_eq!(dir, Dir::East);
        assert_eq!(DISC_LAYOUT[y][x], "C05");
        // stone = 2 同时是 north/percussion-left（E 区）与 west/percussion-right（D 区）。
        // 方向先北后西，所以 E 区有这格就用 E 区；E07 在面板上根本不存在，
        // 音高 7 只能顺延到西墙的 D07。换墙不是 bug，是遍历顺序的必然结果。
        let (x, y, dir) = local_coords(2, 1).unwrap();
        assert_eq!(dir, Dir::North);
        assert_eq!(DISC_LAYOUT[y][x], "E01");
        let (x, y, dir) = local_coords(2, 7).unwrap();
        assert_eq!(dir, Dir::West);
        assert_eq!(DISC_LAYOUT[y][x], "D07");
        // soul_sand = 11：north/percussion-right（D 区）先命中，东墙的 E 区轮不到。
        let (x, y, dir) = local_coords(11, 7).unwrap();
        assert_eq!(dir, Dir::North);
        assert_eq!(DISC_LAYOUT[y][x], "D07");
        // 怪物头不看音高：25 个音高全落在同一个 M 格。
        let mob = local_coords(17, 0).unwrap();
        assert_eq!(DISC_LAYOUT[mob.1][mob.0], "M");
        for n in 1..25u8 {
            assert_eq!(local_coords(17, n), Some(mob), "音高 {n} 不该换格");
        }
        // 20 × 25 全组合都能落格，且格子里的编号与该音高对得上。
        for ins in 0..20u8 {
            for n in 0..25u8 {
                let (x, y, _) = local_coords(ins, n).unwrap_or_else(|| panic!("{ins}/{n} 不在面板上"));
                let cell = DISC_LAYOUT[y][x];
                if cell != "M" {
                    assert_eq!(&cell[1..], &format!("{n:02}"), "乐器 {ins} 音高 {n}");
                }
            }
        }
    }

    #[test]
    fn grey_lookup_is_a_bijection() {
        let mut seen = [false; 16];
        for packed in 0..16u8 {
            let s = signal_of(packed);
            assert_eq!(GREY_SIGNAL[s as usize], packed, "反查必须回到原打包值");
            seen[s as usize] = true;
        }
        assert!(seen.iter().all(|b| *b), "16 个打包值要映射到 16 个信号强度");
    }

    #[test]
    fn four_tick_packing_and_even_odd_split() {
        // 前 4 tick 全响：偶/奇各取到 2 个 true，补满 4 位 -> 0b1100 = 12 -> 信号 12。
        assert_eq!(encode_stream(&[true; 4]), [vec![12u8], vec![12u8]]);
        // 只有 tick 0 响：偶数流补成 0b1000 = 8 -> 信号 11，奇数流整条为空。
        let mut bits = vec![false; 6];
        bits[0] = true;
        assert_eq!(encode_stream(&bits), [vec![11u8], vec![]]);
        // 只有 tick 5 响：奇数流 0b0010 = 2 -> 信号 7。
        let mut bits = vec![false; 6];
        bits[5] = true;
        assert_eq!(encode_stream(&bits), [vec![], vec![7u8]]);
        // 空曲：两条流都没有尾巴。
        assert_eq!(encode_stream(&[]), [Vec::<u8>::new(), Vec::<u8>::new()]);
    }

    #[test]
    fn long_pause_splits_at_64_because_the_check_leads_the_loop() {
        let mut stream = vec![0u8; 65];
        stream.push(1);
        assert_eq!(
            item_slots(&stream).unwrap(),
            vec![Slot::Pause(64), Slot::Pause(1), Slot::Disc(1)]
        );
    }

    #[test]
    fn last_shulker_is_padded_to_three_items() {
        assert_eq!(item_slots(&[1]).unwrap(), vec![Slot::Disc(1), Slot::Pause(2)]);
        assert_eq!(
            item_slots(&[1, 1]).unwrap(),
            vec![Slot::Disc(1), Slot::Disc(1), Slot::Pause(1)]
        );
        // 已经凑够 3 件就不补。
        assert_eq!(item_slots(&[1, 1, 1]).unwrap().len(), 3);
    }

    #[test]
    fn wool_colour_cycles_on_each_pause_then_disc() {
        let slots = vec![Slot::Pause(1), Slot::Disc(1), Slot::Pause(1), Slot::Disc(2)];
        assert_eq!(
            shulker_container(&slots).unwrap(),
            vec![
                ("minecraft:white_wool", 1, 0),
                ("minecraft:music_disc_13", 1, 1),
                ("minecraft:orange_wool", 1, 2),
                ("minecraft:music_disc_cat", 1, 3),
            ]
        );
    }

    #[test]
    fn one_note_holds_at_most_two_full_chests() {
        let full = |n: usize| vec![1u8; n];
        assert_eq!(double_chest_contents(&full(300)).unwrap().len(), 1);
        assert_eq!(double_chest_contents(&full(800)).unwrap().len(), 2);
        assert!(double_chest_contents(&full(MAX_STACKS_PER_NOTE + 1)).is_err());
    }

    #[test]
    fn no_two_chests_share_a_cell() {
        let mut s = song(20.0);
        for _ in 1..20 {
            s.layers.push(Layer::default());
        }
        for ins in 0..20u8 {
            // 一个乐器一层，25 个音高铺满 0..24 tick（16..19 会被丢掉，见下）。
            for (i, key) in (33..=57u8).enumerate() {
                s.set(i as u32, ins as u32, Note::new(ins, key));
            }
        }
        s.recalculate();
        let (contents, _) = encode_song(&s, &MaestroOptions::default()).unwrap();
        let (chests, pairs) = plan_chests(&contents).unwrap();
        // 内置只有 0..15 能直接发声，16..19 是铜块一族，转换器没有对应的怪物头。
        assert_eq!(pairs, 16 * 25, "这条测试曲该铺开 16 个乐器 × 25 个音高");
        assert_eq!(chests.len(), pairs * 2, "每对流两只箱子");
        let mut seen: Vec<Coord> = chests.iter().map(|c| c.coord).collect();
        let before = seen.len();
        seen.sort_by_key(|c| (c.x, c.y, c.z));
        seen.dedup_by(|a, b| a.x == b.x && a.y == b.y && a.z == b.z);
        assert_eq!(seen.len(), before, "两只箱子占了同一个格子");
        assert!(chests.iter().all(|c| access_index(c.coord).is_some()));
    }

    #[test]
    fn tempo_delta_matches_the_cutoff_formula() {
        assert_eq!(tempo_delta(20.0, TimingMode::Round), 0.0);
        assert_eq!(tempo_delta(15.0, TimingMode::Round), 1.0);
        assert_eq!(tempo_delta(10.0, TimingMode::Round), 1.0);
        assert_eq!(tempo_delta(5.0, TimingMode::Round), 3.0);
        assert!((tempo_delta(15.0, TimingMode::Approximate) - (20.0 / 15.0 - 1.0)).abs() < 1e-12);
        assert_eq!(tempo_delta(15.0, TimingMode::None), 0.0);
        // 20 t/s 以上强制走压缩，四个模式在这一支没有区别。
        assert_eq!(tempo_delta(40.0, TimingMode::Approximate), -1.0);
        assert_eq!(tempo_delta(80.0, TimingMode::Round), -3.0);
    }

    #[test]
    fn fast_tempo_compresses_unless_the_mode_is_flexible() {
        let mut s = song(80.0);
        s.layers.push(Layer::default());
        s.set(0, 0, Note::new(0, 45));
        s.set(1, 1, Note::new(0, 45));
        s.recalculate();
        // round：压缩系数 4，落在奇数 tick 的那颗被跳过。
        let (events, counts) = transcribe_events(&s, TimingMode::Round).unwrap();
        assert_eq!(events, vec![(0, 0, 12)]);
        assert_eq!(counts.dropped_compressed, 1);
        // flexible：两颗都留，靠叠层错开到同一 tick 的第二层。
        let (events, counts) = transcribe_events(&s, TimingMode::Flexible).unwrap();
        assert_eq!(events, vec![(0, 0, 12), (0, 0, 12)]);
        assert_eq!(counts.dropped_compressed, 0);
    }

    #[test]
    fn out_of_range_keys_move_by_octaves() {
        let mut s = song(20.0);
        s.set(0, 0, Note::new(0, 30));
        s.set(1, 0, Note::new(0, 70));
        s.recalculate();
        let (events, counts) = transcribe_events(&s, TimingMode::Round).unwrap();
        assert_eq!(counts.transposed, 2);
        assert_eq!(events, vec![(0, 0, 9), (1, 0, 13)], "30->42->9，70->46->13");
    }

    #[test]
    fn custom_instruments_map_by_sample_file() {
        let mut s = song(20.0);
        s.custom_instruments.push(CustomInstrument {
            name: "Bone".into(),
            filename: "minecraft/mob/skeleton/say1.ogg".into(),
            key: 45,
            press: false,
        });
        s.custom_instruments.push(CustomInstrument {
            name: "Weird".into(),
            filename: "samples/weird.ogg".into(),
            key: 45,
            press: false,
        });
        s.set(0, 0, Note::new(FIRST_CUSTOM_INDEX, 45));
        s.set(1, 0, Note::new(FIRST_CUSTOM_INDEX + 1, 45));
        s.recalculate();
        let (events, counts) = transcribe_events(&s, TimingMode::Round).unwrap();
        assert_eq!(counts.remapped, 1);
        assert_eq!(counts.dropped_custom, 1);
        assert_eq!(events, vec![(0, 18, 12)], "say1 的采样改到骷髅头（18）");
    }

    #[test]
    fn locked_layers_still_get_exported() {
        let mut s = song(20.0);
        s.layers[0].lock = LockMode::Locked;
        s.set(0, 0, Note::new(0, 45));
        s.recalculate();
        let (events, counts) = transcribe_events(&s, TimingMode::Round).unwrap();
        assert_eq!(events.len(), 1, "转换器不看 isLocked，这里也照做");
        assert_eq!(counts.locked_notes, 1);
    }

    #[test]
    fn schematic_bytes_carry_the_expected_contents() {
        let mut s = song(20.0);
        // dirt = 0 号乐器（west/top -> A 区），key 45 -> 音高 12。
        s.set(0, 0, Note::new(0, 45));
        s.recalculate();
        let (bytes, rep) = build_bytes(&s, &MaestroOptions::default()).unwrap();
        assert_eq!(rep.chest_pairs, 1);
        assert_eq!(rep.block_entities, 1);

        let root = crate::nbt::parse(&bytes).unwrap();
        let sch = root.get("Schematic").expect("根复合标签里该有 Schematic");
        assert_eq!(sch.get("Version").and_then(Value::as_i32), Some(3));
        assert_eq!(sch.get("DataVersion").and_then(Value::as_i32), Some(4189));
        assert_eq!(sch.get("Width").and_then(Value::as_i16), Some(273));
        assert_eq!(sch.get("Height").and_then(Value::as_i16), Some(64));
        assert_eq!(sch.get("Length").and_then(Value::as_i16), Some(273));
        let (ty, off) = sch.get("Offset").unwrap().as_list().unwrap();
        assert_eq!(ty, 3, "Offset 是 TAG_Int_Array");
        assert_eq!(
            off.iter().map(|v| v.as_i32()).collect::<Vec<_>>(),
            vec![Some(-136), Some(-27), Some(-136)]
        );

        let blocks = sch.get("Blocks").unwrap();
        let pal = blocks.get("Palette").unwrap();
        assert_eq!(pal.get("minecraft:dirt").and_then(Value::as_i32), Some(0));
        assert_eq!(pal.get("minecraft:composter").and_then(Value::as_i32), Some(33));
        assert_eq!(pal.get("minecraft:air").and_then(Value::as_i32), Some(32));
        let data = blocks.get("Data").unwrap().as_bytes().unwrap();
        assert_eq!(data.len(), (REGION_W * REGION_D * REGION_H) as usize);
        assert!(
            data.iter().all(|b| (*b as i8) < PAL_COMPOSTER + 1),
            "palette 编号得留在 signed byte 窗口里，否则 Data 数组会读成负数"
        );

        // 单颗音符：1 tick -> 偶数流 1 个灰度值 -> 唱片 + 补足到 3 件的羊毛。
        let be = blocks.get("BlockEntities").unwrap().as_list().unwrap();
        assert_eq!(be.1.len(), 1);
        let chest = &be.1[0];
        assert_eq!(chest.get("Id").and_then(Value::as_str), Some("minecraft:chest"));
        let items = chest.get("Data").unwrap().get("Items").unwrap().as_list().unwrap();
        assert_eq!(items.0, 10);
        assert_eq!(items.1.len(), 1, "一只潜影盒");
        let box_item = &items.1[0];
        assert_eq!(box_item.get("count").and_then(Value::as_i32), Some(1));
        assert_eq!(box_item.get("Slot").and_then(Value::as_i8), Some(0));
        assert_eq!(box_item.get("id").and_then(Value::as_str), Some("minecraft:shulker_box"));
        let container = box_item
            .get("components")
            .unwrap()
            .get("minecraft:container")
            .unwrap()
            .as_list()
            .unwrap();
        let mut names = Vec::new();
        for entry in container.1 {
            let item = entry.get("item").unwrap();
            names.push((
                item.get("id").and_then(Value::as_str).unwrap().to_string(),
                item.get("count").and_then(Value::as_i32).unwrap(),
                entry.get("slot").and_then(Value::as_i32).unwrap(),
            ));
        }
        assert_eq!(
            names,
            vec![
                ("minecraft:music_disc_11".to_string(), 1, 0),
                ("minecraft:white_wool".to_string(), 2, 1),
            ]
        );
    }

    /// 按投影模组（Litematica）的 Sponge 导入校验走一遍导出的字节：
    /// `LitematicaSchematic.readFromSpongeSchematic()`（分支 pre-rewrite/fabric/1.21.1-masa）
    /// 依次做 v3 判定 → 尺寸 → Blocks 三件套 → varint 解 Data → palette 编号必须密集。
    /// 上游 100..121 的稀疏编号就是在最后这一步被判死的。
    #[test]
    fn exported_bytes_pass_the_litematica_sponge_v3_checks() {
        let (bytes, _rep) = build_bytes(&song(20.0), &MaestroOptions::default()).unwrap();
        let root = crate::nbt::parse(&bytes).unwrap();
        let sch = root.get("Schematic").expect("根复合标签里该有 Schematic");
        // isValidSpongeSchematicv3()：Version >= 3，且 Blocks、DataVersion 都在。
        assert!(sch.get("Version").and_then(Value::as_i32).unwrap_or_default() >= 3);
        assert!(sch.get("DataVersion").is_some());
        // readSizeFromTagSponge()：三个尺寸都得是正数。
        let size: Vec<i32> = ["Width", "Height", "Length"]
            .iter()
            .map(|k| sch.get(k).and_then(Value::as_i32).unwrap_or_default())
            .collect();
        assert!(size.iter().all(|v| *v > 0), "{size:?}");
        let volume = size[0] as i64 * size[1] as i64 * size[2] as i64;

        let blocks = sch.get("Blocks").expect("readSpongeBlocksFromTag() 要 Blocks 复合标签");
        // 三件套少任一个就直接 return false。
        let pal = blocks.get("Palette").and_then(Value::fields).expect("Palette 得是复合标签");
        let data = blocks.get("Data").and_then(Value::as_bytes).expect("Data 得是 TAG_Byte_Array");
        let (be_ty, bes) = blocks.get("BlockEntities").and_then(Value::as_list).expect("BlockEntities 得是列表");
        assert_eq!(be_ty, 10, "BlockEntities 的元素得是复合标签");
        assert_eq!(
            pal.len(),
            INSTRUMENT_BASE_BLOCKS.len() + CHEST_PALETTE.len() + SIGN_PALETTE.len() + 2,
            "palette 条目数"
        );

        // convertVarIntByteArrayToPackedLongArray()：每格读一个 varint，
        // 编号 <128 才是单字节 varint，长度才会刚好等于 volume。
        assert_eq!(data.len() as i64, volume, "varint 流得刚好解出 {volume} 格");
        assert!(data.iter().all(|b| *b < 0x80), "编号超过 127 会读成多字节 varint");

        // readSpongePaletteFromTag()：0 <= id < 条目数，越界即整份文件拒读。
        for (name, value) in pal {
            let id = value.as_i32().unwrap_or(-1);
            assert!(
                id >= 0 && (id as usize) < pal.len(),
                "{name} 的 palette 编号 {id} 不在 0..{} 里",
                pal.len()
            );
        }

        // readBlockPosFromArrayTag() / readVec3iFromIntArray() 都只认长度 3 的 TAG_Int_Array。
        for be in bes {
            let (ty, pos) = be.get("Pos").and_then(Value::as_list).expect("每只箱子都要有 Pos");
            assert_eq!(ty, 3, "Pos 得是 TAG_Int_Array");
            assert_eq!(pos.len(), 3, "Pos 得是三个整数");
            assert!(be.get("Data").is_some(), "v3 的方块实体内容装在 Data 里");
        }
        let (ty, off) = sch.get("Offset").and_then(Value::as_list).expect("Offset");
        assert_eq!(ty, 3, "Offset 得是 TAG_Int_Array");
        assert_eq!(off.len(), 3);
    }

    /// 按 NBT 规范严格走一遍导出的字节流，等价于创世神 7.4.5 内置 linbus
    /// (`LinNbtReader`) 和投影模组读侧的走法：每个标签编号都得落在 0..=12，
    /// 数组宽度照规范（11 = 4 字节 int、12 = 8 字节 long），走完必须刚好消耗干净。
    ///
    /// 需要这道闸门是因为 `crate::nbt::parse` 宽容：它按自己那套编号解释，
    /// 写侧一旦把 Int_Array 标成 12，两边一起错就谁都看不出错位，而 Java 侧会在
    /// 多吞 12 字节后把 "Palette" 的名字首字母 'P'（0x50 = 80）当标签编号读，
    /// 报 `Invalid NBT ID: 80` 并整份拒读。
    fn assert_nbt_stream_is_spec_exact(bytes: &[u8]) {
        struct R<'a> {
            d: &'a [u8],
            p: usize,
        }
        impl<'a> R<'a> {
            fn take(&mut self, n: usize) -> &'a [u8] {
                assert!(self.p + n <= self.d.len(), "越界：@{} 还要 {n} 字节", self.p);
                let s = &self.d[self.p..self.p + n];
                self.p += n;
                s
            }
            fn u8(&mut self) -> u8 {
                self.take(1)[0]
            }
            fn i16(&mut self) -> i16 {
                let b = self.take(2);
                i16::from_be_bytes([b[0], b[1]])
            }
            fn i32(&mut self) -> i32 {
                let b = self.take(4);
                i32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }
            fn name(&mut self) -> String {
                let n = self.i16().max(0) as usize;
                String::from_utf8_lossy(self.take(n)).into_owned()
            }
            fn payload(&mut self, id: u8, depth: u32) {
                assert!(depth <= 64, "嵌套过深");
                match id {
                    1 => self.take(1),
                    2 => self.take(2),
                    3 => self.take(4),
                    4 => self.take(8),
                    5 => self.take(4),
                    6 => self.take(8),
                    7 => {
                        let n = self.i32().max(0) as usize;
                        self.take(n)
                    }
                    8 => {
                        self.name();
                        &[]
                    }
                    9 => {
                        let elem = self.u8();
                        assert!(elem <= 12, "Invalid NBT ID: {elem}（list 元素类型）");
                        let n = self.i32().max(0) as usize;
                        assert!(n <= self.d.len() - self.p, "list 声明 {n} 个元素，剩不下");
                        for _ in 0..n {
                            self.payload(elem, depth + 1);
                        }
                        &[]
                    }
                    10 => {
                        loop {
                            let next = self.u8();
                            assert!(next <= 12, "Invalid NBT ID: {next}");
                            if next == 0 {
                                break;
                            }
                            self.name();
                            self.payload(next, depth + 1);
                        }
                        &[]
                    }
                    11 => {
                        let n = self.i32().max(0) as usize;
                        self.take(n.checked_mul(4).expect("int 数组长度溢出"))
                    }
                    12 => {
                        let n = self.i32().max(0) as usize;
                        self.take(n.checked_mul(8).expect("long 数组长度溢出"))
                    }
                    other => panic!("Invalid NBT ID: {other}"),
                };
            }
        }

        let mut r = R { d: bytes, p: 0 };
        let root_id = r.u8();
        assert_eq!(root_id, 10, "根标签得是 compound");
        r.name();
        r.payload(10, 0);
        assert_eq!(r.p, bytes.len(), "字节流没走完，说明中间某处宽度算错");
    }

    /// 数组标签的编号必须能在原始字节里查到（读侧解析器一旦跟着写侧一起错就查不出来）。
    fn assert_array_tag_ids_are_int_arrays(bytes: &[u8], key: &str) {
        let mut name = vec![(key.len() as u16 >> 8) as u8, key.len() as u8];
        name.extend_from_slice(key.as_bytes());
        let mut found = 0usize;
        for w in bytes.windows(name.len() + 1) {
            if w[1..] != name[..] {
                continue;
            }
            found += 1;
            assert_eq!(
                w[0], 11,
                "{key} 的标签编号是 {}，规范里 11 才是 TAG_Int_Array（12 = TAG_Long_Array）",
                w[0]
            );
        }
        assert!(found > 0, "原始字节里找不到 {key} 标签");
    }

    #[test]
    fn exported_bytes_are_a_spec_exact_nbt_stream() {
        // 有箱子（带 Pos）与空曲（只有 Offset）都得过同一条闸门。
        let mut s = song(20.0);
        s.set(0, 0, Note::new(0, 45));
        s.set(3, 2, Note::new(1, 50));
        s.recalculate();
        let (bytes, _rep) = build_bytes(&s, &MaestroOptions::default()).unwrap();
        assert_nbt_stream_is_spec_exact(&bytes);
        assert_array_tag_ids_are_int_arrays(&bytes, "Offset");
        assert_array_tag_ids_are_int_arrays(&bytes, "Pos");

        let empty = build_bytes(&song(20.0), &MaestroOptions::default())
            .unwrap()
            .0;
        assert_nbt_stream_is_spec_exact(&empty);
    }

    #[test]
    fn preview_reports_every_rewrite_it_made() {
        let _lock = crate::TEST_LANG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        struct ResetLang(u8);
        impl Drop for ResetLang {
            fn drop(&mut self) {
                crate::set_language(self.0);
            }
        }
        let _lang = ResetLang(crate::lang());
        crate::set_language(0);

        let mut s = song(10.0);
        s.set(0, 0, Note::new(0, 20));
        let rep = preview(&s, &MaestroOptions::default()).unwrap();
        assert_eq!(rep.stretch, 2.0);
        assert_eq!(rep.compression, 0);
        let lines = rep.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("1 tick = 2 machine ticks"), "{}", lines[0]);
        assert!(lines[1].contains("transposed"), "{}", lines[1]);

        // 速度正好 20 t/s、音域内、纯内置乐器：不该有任何说明。
        let mut clean = song(20.0);
        clean.set(0, 0, Note::new(0, 45));
        clean.recalculate();
        assert!(preview(&clean, &MaestroOptions::default()).unwrap().lines().is_empty());
    }
}
