//! 唱片工坊那一档：把曲子编码成「箱子 + 潜影盒 + 唱片」的数据箱投影（`.litematic`）。
//!
//! 与 [`crate::fractal`] 互补：那条出的是自带红石的整机，这一条只出**数据箱**——
//! 25 行编码箱格里各装哪几只唱片、哪一格是空拍，全在这份产物里；发声要靠机体本身已建好。
//! 所以它和 [`crate::maestro`] 同属「贴到已有机器上」那一族，只是编码方式完全不同：
//! MAESTRO 用错误码箱，这一档把音符压进唱片物品的 4 bit 组合里。
//!
//! 口径来自 liuyunmc 的 Encoded-Redstone-Music-Converter（`src/config.js`、`src/encoder.js`、
//! `src/litematic.js`，也就是 `nbs-converter/constants.py` 那套编码表的 JS 重写）。
//! **那个仓库没有 LICENSE**，所以这里只按行为重写、不 vendor 它任何一行代码；每张表的
//! 数值出处都标在它的定义上。对照用 `.work/ermc-run/` 里那份 npm 产物当 oracle。
//!
//! 与上游有意不同的几处，都在报告里说清：
//! - 超音域：上游有 `error` / `drop` / `transpose` 三档策略，我们固定 ±12 拉回 33..=57
//!   并计数（与 MAESTRO、Fractal 同一条口径），不给「直接丢弃」留口子。
//! - 未映射乐器：上游遇到没配轨道的乐器（现在只有自定义乐器会落到这里）当场报错，
//!   我们丢弃并逐条列进变更清单；一颗不剩时才拦下导出。
//! - 静音：上游跳过力度 0 与音量 0 图层上的音符，我们不跳——机体没有力度这一维，
//!   照编只会让它在机器上响一次，与 fractal / maestro 同一条口径。
//! - 时长：上游 `stats.seconds` 拿未取整的曲长 ÷ 网格速度，同一份报告里的 `ticks` 却已经
//!   向上取整到整周期，两个数对不上（一颗音符的曲子会同时报 0.05 秒和 8 个刻）。这里按
//!   取整后的时间轴报秒数，机器确实会空转到整周期走完。
//! - 选项面：上游把 `target`（26.1 / 1.20.4）和 `noteInterval` 分开配，这里由机体档一次
//!   决定两者；轨道数固定取上游的 20 乐器预设，不供 16 乐器那一档。
//!
//! 编码本身（[`CODE`]）不是我们拍的：一行箱格代表同一对半音的两个相位，具体哪几颗键
//! 在这格发声由唱片号那 4 个 bit 决定，所以一格只放一只唱片就能编下两个键的和。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use nbs_core::model::{Song, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN, PAN_CENTER};
use nbs_core::tr;

use crate::litematic::{Generation, Item, Litematic, Region};
use crate::maestro::{msg, trim_num};

// ==================================================== 编码表（数值出处都标在上游仓库）

/// 15 只唱片，索引 = 4 bit 编码值 - 1（也就是 `code - 1`）。
/// 出处：上游 `config.js` 的 `DISCS`，顺序照 vanilla 唱片列表。
pub const DISCS: [&str; 15] = [
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
    "minecraft:music_disc_pigstep",
    "minecraft:music_disc_otherside",
    "minecraft:music_disc_5",
];

/// 16 种可堆叠填充地毯。出处：上游 `config.js` 的 `FILLERS`。留白压成一条物品，
/// 两次留白之间换一种颜色，人才看得出分段。
pub const FILLERS: [&str; 16] = [
    "minecraft:white_carpet",
    "minecraft:light_gray_carpet",
    "minecraft:gray_carpet",
    "minecraft:black_carpet",
    "minecraft:brown_carpet",
    "minecraft:red_carpet",
    "minecraft:orange_carpet",
    "minecraft:yellow_carpet",
    "minecraft:lime_carpet",
    "minecraft:green_carpet",
    "minecraft:cyan_carpet",
    "minecraft:light_blue_carpet",
    "minecraft:blue_carpet",
    "minecraft:purple_carpet",
    "minecraft:magenta_carpet",
    "minecraft:pink_carpet",
];

/// 不可堆叠档用的空拍物品。出处：上游 `config.js` 的 `restItem`。
pub const REST_ITEM: &str = "minecraft:shears";

/// 每个半音的 4 bit 编码值，按 `key - 33` 索引，共 25 项。
/// 出处：上游 `config.js` 的 `CODE`（注释写明与 `nbs-converter/constants.py` 一致）。
/// 最高那个半音（第 24 项）独占一行、四个相位都能压发，所以是 `[8,4,2,1]`；
/// 其余按 `i % 4` 分成 `[4,1]` 与 `[8,2]` 两组，上游只用前两项（取 `phase % 2`），
/// 这里把后两格重复填成同样的值，只为了让类型统一，不改变任何编码语义。
pub const CODE: [[u8; 4]; 25] = build_code();

const fn build_code() -> [[u8; 4]; 25] {
    let mut table = [[0u8; 4]; 25];
    let mut i = 0;
    while i < 25 {
        table[i] = if i == 24 {
            [8, 4, 2, 1]
        } else if i % 4 == 0 || i % 4 == 3 {
            [4, 1, 4, 1]
        } else {
            [8, 2, 8, 2]
        };
        i += 1;
    }
    table
}

/// 25 行箱格：24 行是「两个半音一对」，最后一行由最高半音独占。
pub const ROWS: usize = 25;
/// 每条模块臂的格数 = 25 行 × 每行 2 格箱子。出处：上游 `moduleBounds` 取 `depth 0..49`，
/// `compileSong` 里每行占 `row*2`（右箱）与 `row*2+1`（左箱）。
pub const ARM_DEPTH: i32 = 50;
/// 一只潜影盒 27 格。出处：上游 `packStream` 的 `contents.length === 27`。
pub const SLOTS_PER_BOX: usize = 27;
/// 一只双箱最多挂 54 只盒子。出处：上游 `packStream` 的 `boxes.length === 54`。
pub const BOXES_PER_CHEST: usize = 54;
/// 一只盒子满 64 格的堆叠上限，留白按它压段。出处：上游 `Math.min(rests, 64)`。
pub const MAX_STACK: i64 = 64;
/// 尾箱至少三件物品。出处：上游 `packStream` 末尾的 `itemCount < 3`。
pub const MIN_TAIL_ITEMS: usize = 3;
/// 上游对时间轴设的硬上限。出处：`normalizeSong` 里的 `tick > 2000000`。
pub const MAX_TICK: u64 = 2_000_000;

/// 机器侧四方位，顺序照上游 `DIRECTIONS = ['west','north','east','south']`——
/// 子区域名的两位序号与区域排序都依赖它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    West,
    North,
    East,
    South,
}

const DIRECTIONS: [Direction; 4] = [Direction::West, Direction::North, Direction::East, Direction::South];

impl Direction {
    fn index(self) -> usize {
        DIRECTIONS.iter().position(|d| *d == self).unwrap_or(0)
    }

    /// 这一排箱子朝哪边开。出处：上游 `CHEST_FACING`。
    fn chest_facing(self) -> &'static str {
        match self {
            Self::East => "west",
            Self::South => "north",
            Self::West => "east",
            Self::North => "south",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::East => tr("East", "东"),
            Self::South => tr("South", "南"),
            Self::West => tr("West", "西"),
            Self::North => tr("North", "北"),
        }
    }

    /// 沿臂走 `depth` 格相对模块原点的位移。出处：上游 `ARM_OFFSET`（`lane` 那一项
    /// 上游恒传 0——两条模块臂靠各自的朝向与原点分开）。
    fn arm_offset(self, depth: i32) -> (i32, i32) {
        match self {
            Self::East => (0, depth),
            Self::South => (-depth, 0),
            Self::West => (0, -depth),
            Self::North => (depth, 0),
        }
    }

    /// 模块原点（相对投影中心），给的是 `[x, z]`。出处：上游 `PROJECTION_ORIGIN`。
    fn origin(self) -> (i32, i32) {
        match self {
            Self::East => (0, 1),
            Self::South => (-1, 0),
            Self::West => (0, -1),
            Self::North => (1, 0),
        }
    }
}

/// 轨道名。出处：上游 `config.js` 的 `INSTRUMENTS` 与 `i18n.js` 的 `englishInstruments`；
/// 下标即 NBS 内置乐器号，16..19 是 26.1 才有的铜块小号一族。
fn instrument_label(index: usize) -> &'static str {
    const ZH: [&str; 20] = [
        "钢琴", "贝斯", "底鼓", "军鼓", "踩镲", "吉他", "长笛", "铃铛", "风铃", "木琴", "铁木琴",
        "牛铃", "迪吉里杜管", "芯片", "班卓琴", "电子钢琴", "小号", "斑驳小号", "锈蚀小号", "氧化小号",
    ];
    const EN: [&str; 20] = [
        "Harp", "Bass", "Bass Drum", "Snare", "Hi-hat", "Guitar", "Flute", "Bell", "Chime",
        "Xylophone", "Iron Xylophone", "Cow Bell", "Didgeridoo", "Bit", "Banjo", "Pling", "Trumpet",
        "Exposed Trumpet", "Weathered Trumpet", "Oxidized Trumpet",
    ];
    match (ZH.get(index), EN.get(index)) {
        (Some(zh), Some(en)) => tr(en, zh),
        _ => tr("Custom", "自定义"),
    }
}

// ==================================================== 选项

/// 机体档：一次决定每格几个 game tick、几条轨道，以及箱内物品用哪一代 NBT 写法。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Machine {
    /// 20 条轨道、每 game tick 一音（上游 `noteInterval: 1`）：每件乐器拆偶/奇两条反相
    /// 模块臂，所以 20 轨是 40 个子区域。四颗小号要 26.1 的机体才有位置。
    #[default]
    TwentyHz,
    /// 20 条轨道、每 2 个 game tick 一音（上游 `noteInterval: 2`）：经典 10 音/秒那一台，
    /// 继电器最短延迟正好 2 刻，不需要反相。物品走 1.20.4 的旧写法。
    TenHz,
}

impl Machine {
    pub const ALL: [Self; 2] = [Self::TwentyHz, Self::TenHz];

    pub fn label(self) -> &'static str {
        match self {
            Self::TwentyHz => tr("20 t/s machine (20 tracks, even/odd arms)", "20 t/s 机体（20 轨，偶/奇双臂）"),
            Self::TenHz => tr("10 t/s machine (20 tracks, one arm each)", "10 t/s 机体（20 轨，每件乐器一条臂）"),
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::TwentyHz => tr(
                "One note per game tick: each instrument gets two antiphase module arms, so 20 tracks become 40 subregions. Tracks 17-20 (the copper trumpet family) sit on levels 9-10 and need a 26.1 machine.",
                "每 game tick 一音：每件乐器占两条反相模块臂，20 轨就是 40 个子区域。第 17-20 轨（铜块小号一族）落第 9-10 层，机体得在 26.1。",
            ),
            Self::TenHz => tr(
                "One note per two game ticks, the repeater's minimum delay: one module arm per instrument, 20 tracks over five levels. The copper trumpet family sits on level 5, but this preset writes 1.20.4 chests, where those note block sounds do not exist yet.",
                "每 2 个 game tick 一音，正好是继电器最短延迟：每件乐器一条模块臂，20 轨分五层。四颗小号在第 5 层，但这一档按 1.20.4 写箱子，那里的音符盒还没有铜块小号音效。",
            ),
        }
    }

    /// 每格几个 game tick，即上游的 `noteInterval`。
    pub fn note_interval(self) -> u32 {
        match self {
            Self::TwentyHz => 1,
            Self::TenHz => 2,
        }
    }

    /// 机器网格速度（tick/秒）= 20 ÷ 每格 game tick 数，也就是上游的 `20 / noteInterval`。
    pub fn grid_rate(self) -> f32 {
        20.0 / self.note_interval() as f32
    }

    /// 轨道数：两台都取上游的 20 乐器预设（上游另有 16 乐器预设，把小号一族整族省掉，
    /// 出处 `README.md` 的「投影布局」表）。我们不给这一档留口子，免得音符无声消失。
    pub fn tracks(self) -> usize {
        20
    }

    /// 同一件乐器拆几条模块臂（上游 `laneCount`）。
    pub fn lanes(self) -> usize {
        if self.note_interval() == 1 {
            2
        } else {
            1
        }
    }

    /// 一个周期占几个机器刻（上游 `cycleTicks = laneCount * 4`）。
    pub fn cycle_ticks(self) -> u32 {
        (self.lanes() * 4) as u32
    }

    pub fn generation(self) -> Generation {
        match self {
            Self::TwentyHz => Generation::Modern,
            Self::TenHz => Generation::Legacy,
        }
    }
}

/// 时间轴处理，对应上游 `config.js` 的 `timing: 'resample' | 'ticks'`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    /// 把 NBS 刻折算到机器网格（`round(tick * 网格速度 / 曲子速度)`），真实时长不变，
    /// 代价是相邻两刻可能被并到同一格。
    #[default]
    Resample,
    /// 原样保留 NBS 刻号，曲子按机器速度播，所以播放时长会变。
    KeepTicks,
}

impl Timing {
    pub const ALL: [Self; 2] = [Self::Resample, Self::KeepTicks];

    pub fn label(self) -> &'static str {
        match self {
            Self::Resample => tr("Rescale onto the machine grid", "折算到机器网格"),
            Self::KeepTicks => tr("Keep the NBS ticks", "保留 NBS 刻号"),
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Resample => tr(
                "The machine runs at its own grid rate, so ticks get rescaled and the song keeps its real length. Two neighbouring ticks can land on one cell and then sound together.",
                "机器按自己的网格速度播，所以刻号要折算，乐曲真实时长保持不变。相邻两刻可能并到同一格，于是同时响。",
            ),
            Self::KeepTicks => tr(
                "Tick numbers stay as written, so the song plays at the machine's rate and the length in seconds changes.",
                "刻号原样不动，曲子按机器速度播，所以秒数会变。",
            ),
        }
    }
}

/// 导出选项，随 `settings.ini` 的 `[disc_chest]` 段持久化。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DiscChestOptions {
    pub machine: Machine,
    pub timing: Timing,
    /// 层高（格）。出处：上游 `layerHeight`，出厂 8。
    pub layer_height: u32,
    /// 空拍用可堆叠地毯（true）还是不可堆叠剪刀（false）。上游 `useStackable`。
    pub stackable_rests: bool,
    /// 一整行都没有音符时连箱子也不放。上游 `removeEmptyChests`。
    pub remove_empty_chests: bool,
}

impl Default for DiscChestOptions {
    fn default() -> Self {
        Self {
            machine: Machine::TwentyHz,
            timing: Timing::Resample,
            layer_height: 8,
            stackable_rests: true,
            remove_empty_chests: true,
        }
    }
}

impl DiscChestOptions {
    /// 校验：层高越界当场拒。出处：上游 `validateConfig` 的 `1..1024`。
    fn validate(&self) -> Result<(), String> {
        if !(1..=1024).contains(&self.layer_height) {
            return Err(msg(
                "Layer height must be a whole number of blocks between 1 and 1024.".to_string(),
                "层高必须是 1-1024 之间的整数格。".to_string(),
            ));
        }
        Ok(())
    }
}

// ==================================================== 报告

/// 编码结果与变更清单。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiscChestReport {
    pub notes_in: usize,
    pub notes_kept: usize,
    /// 超出 33..=57、按 ±12 拉回的颗数。
    pub transposed: usize,
    /// 落在自定义乐器上（轨道表只到 20 个内置乐器）而被丢掉的颗数。
    pub dropped_unmapped: usize,
    /// 被丢掉的乐器号，供变更清单列出来。
    pub dropped_instruments: Vec<u8>,
    /// 同轨同刻同音高，合并成一次触发的颗数。
    pub merged: usize,
    /// 音分不是整百、已量化到最近半音的颗数。
    pub rounded_pitch: usize,
    /// 机器编码不带力度/声像，受影响的颗数。
    pub fx_ignored: usize,
    pub regions: usize,
    pub chests: usize,
    pub boxes: usize,
    /// 15 只唱片各用了多少次。
    pub discs: [usize; 15],
    /// 编码后的总机器刻数（向上取整到整周期）。
    pub ticks: u32,
    /// 折算前的曲子速度（tick/秒）。
    pub tempo_from: f32,
    /// 这台机器的网格速度（tick/秒）。
    pub grid_rate: f32,
    /// 播放时长（秒）。
    pub seconds: f32,
    pub note_interval: u32,
    pub lanes: usize,
    pub timing: Timing,
    pub size: [i32; 3],
}

impl DiscChestReport {
    pub fn grid_label(&self) -> String {
        trim_num(self.grid_rate as f64)
    }

    /// 一格一个音，所以发声频率就是网格速度。
    pub fn notes_per_sec(&self) -> f32 {
        self.grid_rate
    }

    /// 用掉的唱片总数。
    pub fn disc_total(&self) -> usize {
        self.discs.iter().sum()
    }

    /// 变更清单，只列真发生过的改写。
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        let interval = self.note_interval;
        let grid = self.grid_label();
        v.push(msg(
            format!(
                "One cell every {interval} game tick(s), so this machine plays at {grid} notes/s: {} subregion(s), {} disc chest(s), {} shulker box(es), {} disc(s).",
                self.regions, self.chests, self.boxes, self.disc_total()
            ),
            format!(
                "每 {interval} 个 game tick 一格，也就是 {grid} 音/秒：{} 个子区域、{} 只箱子、{} 个潜影盒、{} 张唱片。",
                self.regions, self.chests, self.boxes, self.disc_total()
            ),
        ));
        if self.lanes == 2 {
            v.push(msg(
                "20 notes/s comes from two antiphase module arms per instrument, not from a shorter delay: the repeater minimum is still 2 game ticks.",
                "20 音/秒靠每件乐器两条反相模块臂，不是把延迟压到 1 刻：继电器的最短延迟还是 2 个 game tick。",
            ));
        }
        v.push(msg(
            format!("The timeline is {} machine tick(s), about {} second(s) at this rate.", self.ticks, trim_num(self.seconds as f64)),
            format!("时间轴 {} 个机器刻，按这个网格速度约 {} 秒。", self.ticks, trim_num(self.seconds as f64)),
        ));
        match self.timing {
            Timing::Resample => {
                if (self.tempo_from - self.grid_rate).abs() > 0.001 {
                    v.push(msg(
                        format!("Rescaled from {} tick/s onto this machine's {} tick/s grid, so the song keeps its length.", trim_num(self.tempo_from as f64), grid),
                        format!("速度 {} t/s 折算到这台机器的 {} t/s 网格，乐曲真实时长保持不变。", trim_num(self.tempo_from as f64), grid),
                    ));
                }
            }
            Timing::KeepTicks => {
                if (self.tempo_from - self.grid_rate).abs() > 0.001 {
                    v.push(msg(
                        format!("Tick numbers are kept, so at {} tick/s the song's length in seconds changes.", grid),
                        format!("刻号原样保留，按 {} t/s 播的话秒数会变。", grid),
                    ));
                }
            }
        }
        if self.transposed > 0 {
            v.push(msg(
                format!("{} notes outside F#3-F#5 were moved by octaves.", self.transposed),
                format!("{} 个音符超出 F♯3–F♯5，已按八度移调。", self.transposed),
            ));
        }
        if self.rounded_pitch > 0 {
            v.push(msg(
                format!("{} notes had pitch offsets that are not whole semitones; they were quantised.", self.rounded_pitch),
                format!("{} 个音符的音分不是整百，已量化到最近半音。", self.rounded_pitch),
            ));
        }
        if self.dropped_unmapped > 0 {
            let ids: Vec<String> = self.dropped_instruments.iter().map(|i| i.to_string()).collect();
            v.push(msg(
                format!(
                    "{} note(s) use custom instrument(s) {} ; the disc-chest track table only covers the 20 built-in instruments, so they were dropped. Reassign those notes to a built-in instrument first.",
                    self.dropped_unmapped, ids.join(", ")
                ),
                format!(
                    "{} 个音符用的是自定义乐器（{}）；唱片工坊的轨道表只到 20 个内置乐器，已丢弃。要保留就得先把这些音符换成内置乐器。",
                    self.dropped_unmapped, ids.join("、")
                ),
            ));
        }
        if self.merged > 0 {
            v.push(msg(
                format!("{} note(s) share track, tick and pitch with another; they fire as one.", self.merged),
                format!("{} 个音符与别的音符同轨同刻同音高，合并成一次触发。", self.merged),
            ));
        }
        if self.fx_ignored > 0 {
            v.push(msg(
                format!("The disc encoding carries no velocity or panning; {} note(s) are affected.", self.fx_ignored),
                format!("唱片编码不承载力度与声像，{} 个音符的这些信息不会生效。", self.fx_ignored),
            ));
        }
        if self.notes_kept > 0 {
            let (en, zh) = match self.machine().generation() {
                Generation::Modern => (
                    "Tracks 17-20 (the copper trumpet family) sit on levels 9-10; they need a 26.1 machine, since older note blocks lack those sounds.",
                    "第 17-20 轨（铜块小号一族）占第 9-10 层，机体得在 26.1；老版本的音符盒没有这几种音效。",
                ),
                Generation::Legacy => (
                    "Tracks 17-20 (the copper trumpet family) sit on level 5, but this preset writes 1.20.4 chests, where those note block sounds do not exist yet.",
                    "第 17-20 轨（铜块小号一族）占第 5 层，但这一档按 1.20.4 写箱子，那里的音符盒还没有这几种音效。",
                ),
            };
            v.push(msg(en.to_string(), zh.to_string()));
        }
        v.push(msg(
            format!(
                "Bounding box {}x{}x{} blocks; this projection is only the disc chests - it holds no redstone and needs a built machine to sound.",
                self.size[0], self.size[1], self.size[2]
            ),
            format!(
                "包围盒 {}×{}×{} 格；这份只是数据箱，不含红石，要贴在已经建好的机体上才会响。",
                self.size[0], self.size[1], self.size[2]
            ),
        ));
        v
    }

    fn machine(&self) -> Machine {
        if self.lanes == 2 {
            Machine::TwentyHz
        } else {
            Machine::TenHz
        }
    }
}

// ==================================================== 归一化

/// 一个进了编码的音符：机器刻号、轨道号、半音序号（`key - 33`，0..24）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    tick: u32,
    track: usize,
    key: usize,
}

/// 把曲子搬进机器网格：静音、乐器映射、音域、音分、时间折算，全部记账。
fn normalize(song: &Song, opts: &DiscChestOptions, rep: &mut DiscChestReport) -> Result<Vec<Cell>, String> {
    if song.meta.tempo <= 0.0 {
        return Err(msg(
            "The song tempo must be above 0.".to_string(),
            "曲子速度必须大于 0。".to_string(),
        ));
    }
    let rate = opts.machine.grid_rate();
    let track_count = opts.machine.tracks();
    let mut cells: Vec<Cell> = Vec::with_capacity(song.total_blocks);
    let mut dropped: BTreeSet<u8> = BTreeSet::new();
    for (&tick, col) in &song.columns {
        for note in col.notes.values() {
            rep.notes_in += 1;
            // 力度 0 与音量 0 图层都不跳：上游 `normalizeSong` 会跳，但机体没有力度这一维，
            // 照编只会让它在机器上响一次；fractal / maestro 也都没跳，这里跟自家口径一致。
            if note.instrument >= FIRST_CUSTOM_INDEX || note.instrument as usize >= track_count {
                rep.dropped_unmapped += 1;
                dropped.insert(note.instrument);
                continue;
            }
            if note.pitch % 100 != 0 {
                rep.rounded_pitch += 1;
            }
            if note.velocity != 100 || note.panning != PAN_CENTER || note.pitch != 0 {
                rep.fx_ignored += 1;
            }
            // 音分折进半音，再 ±12 拉回 33..=57（与 MAESTRO、Fractal 同一条口径）。
            let raw = note.key as i32 + (note.pitch as f32 / 100.0).round() as i32;
            let mut key = raw;
            while key < MC_KEY_MIN as i32 {
                key += 12;
            }
            while key > MC_KEY_MAX as i32 {
                key -= 12;
            }
            if key != raw {
                rep.transposed += 1;
            }
            let at = match opts.timing {
                Timing::Resample => (tick as f32 * rate / song.meta.tempo).round() as u64,
                Timing::KeepTicks => tick as u64,
            };
            if at > MAX_TICK {
                return Err(msg(
                    "The timeline after rescaling goes past 2 000 000 ticks; shorten the song or change the machine.".to_string(),
                    "折算后的时间轴超过 200 万刻，请缩短乐曲或换一台机体。".to_string(),
                ));
            }
            cells.push(Cell { tick: at as u32, track: note.instrument as usize, key: (key - MC_KEY_MIN as i32) as usize });
            rep.notes_kept += 1;
        }
    }
    if cells.is_empty() {
        return Err(msg(
            "Nothing left to encode: every note uses a custom instrument, and the disc-chest track table only covers the 20 built-in ones.".to_string(),
            "没有可编码的音符：这些音符全用了自定义乐器，而唱片工坊的轨道表只到 20 个内置乐器。".to_string(),
        ));
    }
    rep.dropped_instruments = dropped.into_iter().collect();
    Ok(cells)
}

// ==================================================== 装箱

/// 某一编码通道的装箱过程：满 27 格封一只潜影盒，超过 54 只就是双箱溢出。
struct Packer<'a> {
    opts: &'a DiscChestOptions,
    discs: &'a mut [usize; 15],
    boxes: Vec<Vec<Item>>,
    contents: Vec<Item>,
    /// 与 `contents` 一一对应的唱片编码值，0 表示这一件不是唱片。
    codes: Vec<u8>,
    items: usize,
    filler: usize,
}

impl<'a> Packer<'a> {
    fn new(opts: &'a DiscChestOptions, discs: &'a mut [usize; 15]) -> Self {
        Self {
            opts,
            discs,
            boxes: Vec::new(),
            contents: Vec::new(),
            codes: Vec::new(),
            items: 0,
            filler: 0,
        }
    }

    fn push(&mut self, id: &str, count: i32, code: u8) -> Result<(), String> {
        self.contents.push(Item::new(id, count));
        self.codes.push(code);
        self.items += count as usize;
        if self.contents.len() == SLOTS_PER_BOX {
            self.finish()?;
        }
        Ok(())
    }

    /// 留白：可堆叠档一次压满 64 格并轮换地毯颜色，剪刀档一格一件。
    fn rests(&mut self, mut rests: i64) -> Result<(), String> {
        while rests > 0 {
            if self.opts.stackable_rests {
                let count = rests.min(MAX_STACK);
                self.push(FILLERS[self.filler % FILLERS.len()], count as i32, 0)?;
                rests -= count;
            } else {
                self.push(REST_ITEM, 1, 0)?;
                rests -= 1;
            }
        }
        Ok(())
    }

    /// 两次留白之间换一种地毯颜色，但只在盒还没封上时推进——上游同一条。
    fn advance_filler(&mut self, gap: i64) {
        if gap > 1 && !self.contents.is_empty() {
            self.filler += 1;
        }
    }

    fn tail_pad(&mut self) -> Result<(), String> {
        if self.contents.is_empty() || self.items >= MIN_TAIL_ITEMS {
            return Ok(());
        }
        let pad = (MIN_TAIL_ITEMS - self.items) as i32;
        if self.opts.stackable_rests {
            self.push(FILLERS[self.filler % FILLERS.len()], pad, 0)?;
        } else {
            for _ in 0..pad {
                self.push(REST_ITEM, 1, 0)?;
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        if self.contents.is_empty() {
            return Ok(());
        }
        if self.boxes.len() == BOXES_PER_CHEST {
            return Err(msg(
                "The double chest is full: one encoded lane needs more than 54 shulker boxes. Split the song.".to_string(),
                "双箱容量溢出：某一编码通道超过 54 个潜影盒，请拆分乐曲。".to_string(),
            ));
        }
        for code in self.codes.iter().filter(|c| **c > 0) {
            self.discs[(code - 1) as usize] += 1;
        }
        self.boxes.push(std::mem::take(&mut self.contents));
        self.codes.clear();
        self.items = 0;
        self.filler = 0;
        Ok(())
    }

    /// `cycle -> 编码值`，按刻号升序装箱（`BTreeMap` 本身就是升序）。
    fn run(&mut self, stream: &BTreeMap<u32, u8>) -> Result<Vec<Vec<Item>>, String> {
        let mut previous: i64 = -1;
        for (&cycle, &code) in stream {
            let cycle = cycle as i64;
            self.rests(cycle - previous - 1)?;
            self.advance_filler(cycle - previous);
            self.push(DISCS[(code - 1) as usize], 1, code)?;
            previous = cycle;
        }
        self.tail_pad()?;
        self.finish()?;
        Ok(std::mem::take(&mut self.boxes))
    }
}

// ==================================================== 几何

/// 某件乐器某条模块臂的原点、高度与朝向。出处：上游 `presetModules`。
fn module_of(track_index: usize, lane: usize, note_interval: u32, layer_height: u32) -> Module {
    let (side, level) = if note_interval == 2 {
        // 原版 10 t/s 那一台：每侧 4 件乐器，序号大的在底下。
        if track_index < 16 {
            (track_index / 4, 3 - track_index % 4)
        } else {
            ((track_index - 16) % 4, 4 + (track_index - 16) / 4)
        }
    } else if track_index < 16 {
        // 20 t/s：从上往下排，同乐器的两条臂相邻（W/N 或 E/S）。
        const STACKS: [[usize; 8]; 2] = [[4, 5, 6, 7, 0, 1, 2, 3], [12, 13, 14, 15, 8, 9, 10, 11]];
        let group = STACKS.iter().position(|s| s.contains(&track_index)).unwrap_or(0);
        let pos = STACKS[group].iter().position(|v| *v == track_index).unwrap_or(0);
        (group * 2, 7 - pos)
    } else {
        // 小号一族四轨：两侧往下叠。
        (((track_index - 16) % 2) * 2, 8 + (track_index - 16) / 2)
    };
    Module::new(DIRECTIONS[(side + lane) % 4], (level * layer_height as usize) as i32)
}

/// 一条模块臂：原点 + 高度 + 朝向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Module {
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
}

impl Module {
    fn new(direction: Direction, y: i32) -> Self {
        let (x, z) = direction.origin();
        Self { x, y, z, direction }
    }

    /// 沿臂走 `depth` 格的世界坐标。出处：上游 `modulePosition`。
    fn position(&self, depth: i32) -> [i32; 3] {
        let (dx, dz) = self.direction.arm_offset(depth);
        [self.x + dx, self.y, self.z + dz]
    }

    /// 包围盒（depth 0 与 49 两点定出来）。出处：上游 `moduleBounds`。
    fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        let a = self.position(0);
        let b = self.position(ARM_DEPTH - 1);
        let min = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
        let max = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
        (min, max)
    }

    fn size(&self) -> [i32; 3] {
        let (min, max) = self.bounds();
        [max[0] - min[0] + 1, max[1] - min[1] + 1, max[2] - min[2] + 1]
    }

    /// 第 `row` 行第 `half` 只箱子的**局部**坐标（读取方要的是相对 min 的那一套）。
    fn local(&self, depth: i32) -> [i32; 3] {
        let (min, _) = self.bounds();
        let at = self.position(depth);
        [at[0] - min[0], at[1] - min[1], at[2] - min[2]]
    }
}

// ==================================================== 核心

/// 归一化 + 编码 + 装箱 + 摆区域。
fn build(song: &Song, opts: &DiscChestOptions) -> Result<(Litematic, DiscChestReport), String> {
    opts.validate()?;
    let mut rep = DiscChestReport {
        tempo_from: song.meta.tempo,
        grid_rate: opts.machine.grid_rate(),
        note_interval: opts.machine.note_interval(),
        lanes: opts.machine.lanes(),
        timing: opts.timing,
        ..Default::default()
    };
    let cells = normalize(song, opts, &mut rep)?;

    let lanes = opts.machine.lanes();
    let cycle_ticks = opts.machine.cycle_ticks();
    // 每轨每道每行一条编码通道：cycle -> 4 bit 编码值的或。
    let mut streams: Vec<Vec<[BTreeMap<u32, u8>; ROWS]>> = Vec::new();
    for _ in 0..opts.machine.tracks() {
        let mut one: Vec<[BTreeMap<u32, u8>; ROWS]> = Vec::with_capacity(lanes);
        for _ in 0..lanes {
            one.push(std::array::from_fn(|_| BTreeMap::new()));
        }
        streams.push(one);
    }

    let mut seen: HashSet<(usize, u32, usize)> = HashSet::new();
    let mut max_tick = 0u32;
    for cell in &cells {
        debug_assert!(cell.key < ROWS, "音高拉回之后半音序号必定在 0..25 之内");
        let lane = (cell.tick % lanes as u32) as usize;
        let phase = (cell.tick / lanes as u32) % 4;
        let row = if cell.key == 24 { 24 } else { (cell.key / 2) * 2 + (phase / 2) as usize };
        let code = CODE[cell.key][if cell.key == 24 { phase as usize } else { (phase % 2) as usize }];
        match seen.insert((cell.track, cell.tick, cell.key)) {
            true => {}
            false => rep.merged += 1,
        }
        let slot = &mut streams[cell.track][lane][row];
        slot.entry(cell.tick / cycle_ticks).and_modify(|v| *v |= code).or_insert(code);
        max_tick = max_tick.max(cell.tick);
    }

    let mut doc = Litematic {
        gen: opts.machine.generation(),
        name: song_name(song),
        author: song.meta.author.clone(),
        description: String::new(),
        regions: Vec::new(),
        now_ms: now_ms(),
    };
    // 整条臂先占住：某条臂的空档若不被自己占住，读取方贴上去会拿空气盖掉邻区。
    let mut occupied: Vec<([i32; 3], usize)> = Vec::new();
    let mut placed: Vec<(usize, i32, usize, usize, Region)> = Vec::new();

    for (track_index, track_streams) in streams.iter().enumerate() {
        for (lane, rows) in track_streams.iter().enumerate() {
            let module = module_of(track_index, lane, opts.machine.note_interval(), opts.layer_height);
            for depth in 0..ARM_DEPTH {
                let at = module.position(depth);
                if let Some((_, other)) = occupied.iter().find(|(pos, _)| *pos == at) {
                    return Err(msg(
                        format!(
                            "Module arms overlap: track {track_index} collides with track {other} at ({}, {}, {}).",
                            at[0], at[1], at[2]
                        ),
                        format!(
                            "模块区域重叠：第 {} 轨与第 {} 轨撞在 ({}, {}, {})。",
                            track_index, other, at[0], at[1], at[2]
                        ),
                    ));
                }
                occupied.push((at, track_index));
            }
            let (min, _) = module.bounds();
            let mut region = Region::new(
                region_label(track_index, lane, lanes, module.direction),
                min,
                module.size(),
            );
            let facing = module.direction.chest_facing();
            for (row, stream) in rows.iter().enumerate() {
                let mut packer = Packer::new(opts, &mut rep.discs);
                let boxes = packer.run(stream)?;
                if boxes.is_empty() && opts.remove_empty_chests {
                    continue;
                }
                rep.boxes += boxes.len();
                // 右箱装前 27 只盒子，左装修剩下那批（最多也是 27 只）。
                for (offset, half) in [(0usize, "right"), (SLOTS_PER_BOX, "left")] {
                    let slice: Vec<Vec<Item>> = boxes.iter().skip(offset).take(SLOTS_PER_BOX).cloned().collect();
                    let at = module.local(row as i32 * 2 + if half == "right" { 0 } else { 1 });
                    region.chest(at[0], at[1], at[2], facing, half, slice);
                    rep.chests += 1;
                }
            }
            placed.push((module.direction.index(), min[1], track_index, lane, region));
        }
    }

    // 排序照上游：机器侧序号 -> 高度 -> 轨道 -> 模块号，好让界面里列出来的次序稳定。
    placed.sort_by_key(|(dir, y, track, lane, _)| (*dir, *y, *track, *lane));
    doc.regions = placed.into_iter().map(|(_, _, _, _, region)| region).collect();
    rep.regions = doc.regions.len();

    // 时长：先按处理方式折算整条曲轴，再保证最后一个音符所在刻也数进去，
    // 最后向上取整到整周期（每条臂的循环都得走完）。
    let grid_rate = opts.machine.grid_rate();
    let last_tick = max_tick + 1;
    let length = match opts.timing {
        Timing::Resample => (song.enda as f32 * grid_rate / song.meta.tempo).ceil() as u32,
        Timing::KeepTicks => song.enda + 1,
    }
    .max(last_tick);
    rep.ticks = length.div_ceil(cycle_ticks) * cycle_ticks;
    // 秒数按取整后的时间轴算：机器真会空转到整周期结束，报未取整的长度会短一截。
    rep.seconds = rep.ticks as f32 / grid_rate;
    let (min, max) = doc.enclosing();
    rep.size = [max[0] - min[0] + 1, max[1] - min[1] + 1, max[2] - min[2] + 1];
    doc.description = describe(&doc, opts);
    Ok((doc, rep))
}

/// 子区域名：`序号_方位_轨道_乐器名[_模块N]`。出处：上游 `compileSong` 里的 `label`。
/// 上游的模块后缀恒为中文「模块N」，这里在非中文界面写成 `ModuleN`——只差名字，不影响内容。
fn region_label(track_index: usize, lane: usize, lanes: usize, direction: Direction) -> String {
    let side = format!("{:02}", direction.index() + 1);
    let source = format!("{track_index:02}");
    let name = instrument_label(track_index);
    let dir = direction.label();
    if lanes == 2 {
        format!("{side}_{dir}_{source}_{name}_{}{}", tr("Module", "模块"), lane + 1)
    } else {
        format!("{side}_{dir}_{source}_{name}")
    }
}

fn song_name(song: &Song) -> String {
    if !song.meta.name.is_empty() {
        return song.meta.name.clone();
    }
    song.filename
        .as_ref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("nbs-rs disc chest")
        .to_owned()
}

/// `Metadata.Description`：逐条写清哪个子区域是哪件乐器的哪条臂。
fn describe(doc: &Litematic, opts: &DiscChestOptions) -> String {
    let list: Vec<String> = doc
        .regions
        .iter()
        .map(|r| r.name.clone())
        .collect();
    msg(
        format!(
            "Disc-chest projection; one fixed-size subregion per module arm, {} game tick(s) per cell. {}",
            opts.machine.note_interval(),
            list.join("; ")
        ),
        format!(
            "唱片数据箱投影；每条模块臂一个固定尺寸子区域，每 {} 个 game tick 一格。{}",
            opts.machine.note_interval(),
            list.join("；")
        ),
    )
}

/// 投影的创建/修改时间戳（毫秒）。上游取 `Date.now()`，对照脚本不读这两个键，
/// 所以真机导出用本机当前时间就好。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// 产出 `(litematic 字节, 报告)`，不落盘。
pub fn build_bytes(song: &Song, opts: &DiscChestOptions) -> Result<(Vec<u8>, DiscChestReport), String> {
    let (doc, report) = build(song, opts)?;
    let bytes = crate::litematic::write(&doc)?;
    Ok((bytes, report))
}

/// 只算报告不写文件，给导出窗口的预览用。
pub fn preview(song: &Song, opts: &DiscChestOptions) -> Result<DiscChestReport, String> {
    build(song, opts).map(|(_, report)| report)
}

/// 导出到 `path`，返回报告。
pub fn export(song: &Song, path: &Path, opts: &DiscChestOptions) -> Result<DiscChestReport, String> {
    let (bytes, report) = build_bytes(song, opts)?;
    std::fs::write(path, bytes).map_err(|e| {
        msg(
            format!("Failed to write the file: {e}"),
            format!("写入文件失败：{e}"),
        )
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::{Layer, Note};

    /// `(刻号, 乐器, 音高)` 造曲子；两颗同刻的音符放不同图层。
    fn song(tempo: f32, notes: &[(u32, u8, u8)]) -> Song {
        let mut s = Song::default();
        s.meta.tempo = tempo;
        s.layers = vec![Layer::default(); 3];
        for (i, &(tick, instrument, key)) in notes.iter().enumerate() {
            s.set(tick, (i % 3) as u32, Note::new(instrument, key));
        }
        s
    }

    fn opts(machine: Machine) -> DiscChestOptions {
        DiscChestOptions {
            machine,
            ..Default::default()
        }
    }

    /// 按「机器侧序号 + 轨道 + 模块号」找回一条臂。子区域名里的方位与乐器名会跟着界面
    /// 语言变，所以这里只认那三段不变的编号——否则并行跑的用例会互相把语言改掉。
    fn arm(doc: &Litematic, side: usize, track: usize, lane: usize) -> &Region {
        doc.regions
            .iter()
            .find(|r| {
                r.name.starts_with(&format!("{side:02}_"))
                    && r.name.contains(&format!("_{track:02}_"))
                    && lane_of(&r.name) == lane
            })
            .unwrap_or_else(|| panic!("没有第 {side} 侧第 {track} 轨第 {} 道的子区域：{:?}", lane + 1, doc.regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()))
    }

    /// 名称尾巴上的模块号（1 起）；10 t/s 那台不带后缀，恒为道 0。
    fn lane_of(name: &str) -> usize {
        let tail = name.as_bytes().last().filter(|b| b.is_ascii_digit()).map(|b| (b - b'0') as usize);
        match tail {
            Some(n) if name.contains("模块") || name.contains("Module") => n - 1,
            _ => 0,
        }
    }

    fn items(box_items: &[Item]) -> Vec<(&str, i32)> {
        box_items.iter().map(|i| (i.id.as_str(), i.count)).collect()
    }

    fn inflate(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut d = flate2::read::GzDecoder::new(bytes);
        let mut out = Vec::new();
        d.read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn the_code_table_keeps_the_two_documented_families() {
        // 数值出处：上游 config.js 的 CODE —— 第 0、3 组半音 [4,1]，第 1、2 组 [8,2]，
        // 最高那个半音独占一行，四个相位分别是 8/4/2/1。
        assert_eq!(CODE[0], [4, 1, 4, 1]);
        assert_eq!(CODE[3], [4, 1, 4, 1]);
        assert_eq!(CODE[1], [8, 2, 8, 2]);
        assert_eq!(CODE[2], [8, 2, 8, 2]);
        assert_eq!(CODE[24], [8, 4, 2, 1]);
        // 单个编码值必须是一颗 bit，否则 DISCS[code-1] 会指到 15 只唱片之外。
        for row in CODE.iter() {
            for code in row {
                assert!(matches!(code, 1 | 2 | 4 | 8), "编码值 {code} 不是单个 bit");
            }
        }
    }

    #[test]
    fn twenty_hertz_needs_two_arms_and_forty_subregions() {
        assert_eq!(Machine::TwentyHz.note_interval(), 1);
        assert_eq!(Machine::TwentyHz.grid_rate(), 20.0);
        assert_eq!(Machine::TwentyHz.lanes(), 2);
        assert_eq!(Machine::TwentyHz.cycle_ticks(), 8);
        assert_eq!(Machine::TenHz.grid_rate(), 10.0);
        assert_eq!(Machine::TenHz.lanes(), 1);
        assert_eq!(Machine::TenHz.cycle_ticks(), 4);

        let s = song(20.0, &[(0, 0, 45)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        // 20 轨 × 2 道 = 40 条臂。只有一颗音符也照样开满 40 个子区域：读取方按固定尺寸贴，
        // 少一条臂就等于把邻区的位置让出去。
        assert_eq!(rep.regions, 40);
        assert_eq!(doc.regions.len(), 40);
        // 同一件乐器的两条臂相邻：轨道 0 的第 1 道在西侧、第 2 道在北侧。
        assert_eq!(arm(&doc, 1, 0, 0).size, [1, 1, 50]);
        // 北侧那条臂沿 +x 走，所以尺寸是 50×1×1。
        assert_eq!(arm(&doc, 2, 0, 1).size, [50, 1, 1]);
        assert_eq!(rep.notes_in, 1);
        assert_eq!(rep.notes_kept, 1);
        assert_eq!((rep.transposed, rep.dropped_unmapped, rep.merged), (0, 0, 0));
    }

    #[test]
    fn consecutive_ticks_alternate_between_the_two_arms() {
        // 每 game tick 一音靠两条反相臂，不是把继电器压到 1 刻：刻 0 落第 1 道、刻 1 落第 2 道。
        let s = song(20.0, &[(0, 0, 45), (1, 0, 45)]);
        let (doc, _) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        let right = arm(&doc, 1, 0, 0);
        let left = arm(&doc, 2, 0, 1);
        // 有内容的那一行放左右两只箱子，左箱在盒子不满 27 只时是空的（上游同款）。
        assert_eq!(right.chests().len(), 2);
        assert_eq!(left.chests().len(), 2);
        // 半音 12 → 第 12 行 → 臂上第 24 格。西侧那条臂沿 -z 走，所以局部坐标是 49-24；
        // 北侧沿 +x 走，局部坐标就是 24。
        assert_eq!(right.chests()[0].at, [0, 0, 25]);
        assert_eq!(right.chests()[0].facing, "east");
        assert_eq!(left.chests()[0].at, [24, 0, 0]);
        assert_eq!(left.chests()[0].facing, "south");
        // 两条臂的 cycle 都是 0：相位差完全来自道号。
        assert_eq!(items(&right.chests()[0].boxes[0])[0].0, "minecraft:music_disc_chirp");
    }

    #[test]
    fn two_notes_sharing_a_cell_are_encoded_into_one_disc() {
        // 半音 12（[4,1]）与半音 13（[8,2]）在相位 0 各出 4 和 8，或起来是 12，
        // 于是这一格放第 12 号唱片——一格一件就编下了两颗键的和。
        let s = song(20.0, &[(0, 0, 45), (0, 0, 46)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        assert_eq!(rep.notes_kept, 2);
        let chest = &arm(&doc, 1, 0, 0).chests()[0];
        assert_eq!(chest.boxes.len(), 1);
        assert_eq!(items(&chest.boxes[0])[0], ("minecraft:music_disc_wait", 1));
        assert_eq!(rep.discs[11], 1, "用了 12 号唱片");
        assert_eq!(rep.disc_total(), 1);
    }

    #[test]
    fn a_gap_becomes_stackable_carpet_and_the_tail_is_padded_to_three() {
        // 刻 0 与刻 40 同属道 0、相位 0、第 12 行，cycle 从 0 跳到 5，中间留白 4 格。
        let s = song(20.0, &[(0, 0, 45), (40, 0, 45)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        let chest = &arm(&doc, 1, 0, 0).chests()[0];
        assert_eq!(
            items(&chest.boxes[0]),
            vec![
                ("minecraft:music_disc_chirp", 1),
                ("minecraft:white_carpet", 4),
                ("minecraft:music_disc_chirp", 1),
            ],
            "留白压成一条可堆叠地毯；尾巴够三件就不补"
        );
        assert_eq!(rep.boxes, 1);
        assert_eq!(rep.discs[3], 2, "编码值 4 → 索引 3");
        assert_eq!(rep.disc_total(), 2);

        // 只有一颗音符时补到三件（上游同款）。
        let one = song(20.0, &[(0, 0, 45)]);
        let (doc, _) = build(&one, &opts(Machine::TwentyHz)).unwrap();
        assert_eq!(
            items(&arm(&doc, 1, 0, 0).chests()[0].boxes[0]),
            vec![("minecraft:music_disc_chirp", 1), ("minecraft:white_carpet", 2)]
        );
    }

    #[test]
    fn non_stackable_rests_use_one_shear_per_cell() {
        let s = song(20.0, &[(0, 0, 45), (40, 0, 45)]);
        let options = DiscChestOptions {
            stackable_rests: false,
            ..opts(Machine::TwentyHz)
        };
        let (doc, _) = build(&s, &options).unwrap();
        assert_eq!(
            items(&arm(&doc, 1, 0, 0).chests()[0].boxes[0]),
            vec![
                ("minecraft:music_disc_chirp", 1),
                ("minecraft:shears", 1),
                ("minecraft:shears", 1),
                ("minecraft:shears", 1),
                ("minecraft:shears", 1),
                ("minecraft:music_disc_chirp", 1),
            ]
        );
    }

    #[test]
    fn an_empty_row_keeps_its_chests_only_when_asked() {
        let s = song(20.0, &[(0, 0, 45)]);
        let keep = DiscChestOptions {
            remove_empty_chests: false,
            ..opts(Machine::TwentyHz)
        };
        let (doc, rep) = build(&s, &keep).unwrap();
        // 25 行 × 左右两只 = 每条臂 50 只，40 条臂全放。
        assert_eq!(arm(&doc, 1, 0, 0).chests().len(), 50);
        assert_eq!(rep.chests, 50 * 40);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        assert_eq!(arm(&doc, 1, 0, 0).chests().len(), 2);
        assert_eq!(rep.chests, 2, "只有有内容那一行的左右两只");
    }

    #[test]
    fn the_ten_hertz_machine_puts_the_trumpet_family_on_level_five() {
        let s = song(10.0, &[(0, 0, 45), (1, 17, 45), (2, 20, 45)]);
        let (doc, rep) = build(&s, &opts(Machine::TenHz)).unwrap();
        assert_eq!(rep.regions, 20, "10 t/s 那台每件乐器一条臂");
        assert_eq!(doc.regions[0].size, [1, 1, 50]);
        assert_eq!(rep.note_interval, 2);
        assert_eq!(rep.lanes, 1);
        // 轨道 17（斑驳小号）在上游的 10 t/s 排法里落北侧、第 5 层（y = 4 × 层高）。
        assert_eq!(arm(&doc, 2, 17, 0).min[1], 4 * 8);
        assert_eq!(rep.dropped_unmapped, 1);
        assert_eq!(rep.dropped_instruments, vec![20]);
        assert_eq!(rep.notes_kept, 2);
        assert!(rep.lines().iter().any(|l| l.contains("20") && (l.contains("custom") || l.contains("自定义"))));
        // 10 t/s 的目标版本是 1.20.4，箱内物品走旧写法。
        assert_eq!(doc.gen, Generation::Legacy);
    }

    #[test]
    fn the_twenty_hertz_machine_keeps_the_trumpet_family() {
        let s = song(20.0, &[(0, 17, 45)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        assert_eq!(rep.dropped_unmapped, 0);
        assert_eq!(rep.notes_kept, 1);
        assert_eq!(rep.machine(), Machine::TwentyHz);
        // 小号一族四轨：轨道 17 落在西侧与北侧的第 9 层往上。
        // 轨道 17（斑驳小号）在上游的排法里落东侧、第 9 层。
        assert_eq!(arm(&doc, 3, 17, 0).min[1], 8 * 8);
        assert!(rep
            .lines()
            .iter()
            .any(|l| l.contains("26.1") || l.contains("trumpet")));
    }

    #[test]
    fn rescaling_onto_the_grid_can_merge_two_ticks_into_one_cell() {
        // 60 t/s 的曲子往 20 t/s 网格折算：刻 0、1 → 0、0（并格），刻 2、3 → 1、1（并格）。
        let s = song(60.0, &[(0, 0, 45), (1, 0, 45), (2, 0, 45), (3, 0, 45)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        assert!((rep.tempo_from - 60.0).abs() < f32::EPSILON);
        assert_eq!(rep.grid_rate, 20.0);
        assert_eq!((rep.notes_in, rep.notes_kept), (4, 4));
        assert_eq!(rep.merged, 2);
        // 折算后的刻 0 与刻 1 分属两条臂，所以两侧都只有一行有箱子。
        assert_eq!(arm(&doc, 1, 0, 0).chests().len(), 2);
        assert_eq!(arm(&doc, 2, 0, 1).chests().len(), 2);
        assert_eq!(rep.ticks, 8, "向上取整到整周期");
        assert!((rep.seconds - 0.4).abs() < 1e-6, "{}", rep.seconds);

        // 保留刻号：时间轴不动，秒数按网格速度重算，四颗各占一格。
        let keep = DiscChestOptions {
            timing: Timing::KeepTicks,
            ..opts(Machine::TwentyHz)
        };
        let (_, rep) = build(&s, &keep).unwrap();
        assert_eq!(rep.merged, 0);
        assert_eq!(rep.ticks, 8);
    }

    #[test]
    fn keeping_the_ticks_changes_the_playing_speed_instead_of_the_axis() {
        // 12 t/s 的曲子保留刻号、按 20 t/s 播，所以整条轴缩短、时长跟着变。
        let s = song(12.0, &[(0, 0, 45), (12, 0, 45)]);
        let resample = opts(Machine::TwentyHz);
        let keep = DiscChestOptions {
            timing: Timing::KeepTicks,
            ..resample
        };
        let (_, a) = build(&s, &resample).unwrap();
        let (_, b) = build(&s, &keep).unwrap();
        // 折算档：曲长 13 刻 → ceil(13 × 20 / 12) = 22 → 取整周期 24。
        assert_eq!(a.ticks, 24);
        assert!((a.seconds - 1.2).abs() < 1e-6, "{}", a.seconds);
        // 保留档：曲长仍按 13 个机器刻算，但每刻只有 1/20 秒。
        assert_eq!(b.ticks, 16);
        assert!((b.seconds - 0.8).abs() < 1e-6, "{}", b.seconds);
    }

    #[test]
    fn pitch_is_quantised_before_the_range_check() {
        // F♯5（57）升 50 音分：先量化成 +1 半音 = 58，再 ±12 拉回 46。
        // 少了这一步，57 会直接留在窗底，行号与唱片都不同。
        let mut s = song(20.0, &[(0, 0, 57)]);
        s.set(0, 0, Note { pitch: 50, ..Note::new(0, 57) });
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        assert_eq!((rep.rounded_pitch, rep.transposed), (1, 1));
        assert_eq!(rep.fx_ignored, 1, "机器编码不承载音分");
        // 拉回后 key 46 = 半音 13 → 第 12 行 → 臂上第 24 格，编码值 8 → 8 号唱片。
        let chest = &arm(&doc, 1, 0, 0).chests()[0];
        assert_eq!(chest.at, [0, 0, 25]);
        assert_eq!(chest.half, "right");
        assert_eq!(items(&chest.boxes[0])[0], ("minecraft:music_disc_stal", 1));
    }

    #[test]
    fn octaves_fold_into_the_same_row() {
        // 半音序号按「距窗底几个半音」算，所以 F♯1（21）拉回 33 之后与 33 同格：
        // 是八度折叠，不是按窗口边界裁掉。
        let s = song(20.0, &[(0, 0, 21), (8, 0, 33)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        assert_eq!(rep.transposed, 1);
        let right = arm(&doc, 1, 0, 0);
        assert_eq!(right.chests().len(), 2, "两颗都在第 0 行");
        // 第 0 行 → 臂上第 0 格；西侧沿 -z，局部坐标是 49。
        assert_eq!(right.chests()[0].at, [0, 0, 49]);
        // 刻 0 与刻 8 都是道 0、相位 0，只差 cycle，所以两只盒子还没满 27 格 → 同一只箱子里一只盒子。
        assert_eq!(items(&right.chests()[0].boxes[0]), vec![
            ("minecraft:music_disc_chirp", 1),
            ("minecraft:music_disc_chirp", 1),
            ("minecraft:white_carpet", 1),
        ]);
        assert!(arm(&doc, 2, 0, 1).chests().is_empty(), "刻 8 仍是道 0");
    }

    #[test]
    fn the_highest_note_takes_a_row_of_its_own() {
        // 半音 24（B5）独占第 24 行，四个相位各压发一次，编码值走 8/4/2/1。
        let s = song(20.0, &[(0, 0, 57), (1, 0, 57), (2, 0, 57), (3, 0, 57)]);
        let (doc, rep) = build(&s, &opts(Machine::TwentyHz)).unwrap();
        // 同一条臂里相位 0 与相位 1 都落在 cycle 0，编码值或成 8|4 = 12 → 一格一只唱片。
        assert_eq!(rep.disc_total(), 2);
        // 行 24 → 右箱臂上第 48 格、左箱第 49 格；臂只有 50 格，刚好放下。
        assert_eq!(arm(&doc, 1, 0, 0).chests()[0].at, [0, 0, 1]);
        assert_eq!(arm(&doc, 2, 0, 1).chests()[0].at, [48, 0, 0]);
    }

    #[test]
    fn layer_height_moves_every_arm_and_zero_is_refused() {
        let s = song(20.0, &[(0, 0, 45)]);
        let low = DiscChestOptions {
            layer_height: 4,
            ..opts(Machine::TwentyHz)
        };
        let (doc, _) = build(&s, &low).unwrap();
        assert_eq!(arm(&doc, 1, 0, 0).min[1] % 4, 0);
        let bad = DiscChestOptions {
            layer_height: 0,
            ..opts(Machine::TwentyHz)
        };
        assert!(build(&s, &bad).is_err());
    }

    #[test]
    fn the_output_is_a_gzip_litematic_that_our_own_reader_parses() {
        let s = song(20.0, &[(0, 0, 45), (1, 1, 47), (2, 2, 48)]);
        let options = opts(Machine::TwentyHz);
        let (bytes, rep) = build_bytes(&s, &options).unwrap();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "产物该是 gzip");
        assert_eq!(preview(&s, &options).unwrap(), rep);
        // 用我们自己的 NBT 解析器读一遍（消费方口径）。
        let root = crate::nbt::parse(&inflate(&bytes)).unwrap();
        assert_eq!(root.get("Version").and_then(|v| v.as_i32()), Some(7));
        assert_eq!(root.get("SubVersion").and_then(|v| v.as_i32()), Some(1));
        // Regions 在文件里是按子区域名索引的 compound（不是列表），所以排序只能靠写入次序保住。
        let fields = root.get("Regions").unwrap().fields().unwrap();
        assert_eq!(fields.len(), 40);
        assert!(
            fields.iter().take(10).all(|(name, _)| name.starts_with("01_")),
            "东侧之前的 01 侧应当连着排在文件开头：{:?}",
            fields.iter().map(|(n, _)| n.clone()).take(12).collect::<Vec<_>>()
        );
        let meta = root.get("Metadata").unwrap();
        // TotalBlocks 这里等于箱子数（一颗箱子就是一个被占用的格）。
        assert_eq!(
            meta.get("TotalBlocks").and_then(|v| v.as_i32()),
            Some(rep.chests as i32)
        );
        assert_eq!(
            meta.get("EnclosingSize")
                .and_then(|v| v.fields())
                .map(|f| f.len()),
            Some(3),
            "Litematica 的 Metadata 用 EnclosingSize 这个 x/y/z compound，不是 Min/Max"
        );
    }

    #[test]
    fn a_song_with_nothing_encodable_is_refused() {
        let s = song(20.0, &[(0, 20, 45)]);
        let err = build(&s, &opts(Machine::TwentyHz)).unwrap_err();
        assert!(err.contains("track") || err.contains("轨道"), "{err}");
    }

    #[test]
    fn names_follow_the_upstream_label_shape() {
        let _guard = crate::TEST_LANG_LOCK.lock().unwrap();
        nbs_core::set_language(1);
        let s = song(10.0, &[(0, 0, 45)]);
        let (doc, _) = build(&s, &opts(Machine::TenHz)).unwrap();
        // 上游：序号_方位_轨道_乐器名；10 t/s 那台一条臂一件乐器，不带模块后缀。
        // 上游：序号_方位_轨道_乐器名；10 t/s 那台一条臂一件乐器，不带模块后缀。
        // 取第 0 轨而不是 regions[0]：排序按 (侧, 高度, 轨道)，最低的格子是第 3 轨。
        assert_eq!(arm(&doc, 1, 0, 0).name, "01_西_00_钢琴");
        nbs_core::set_language(0);
        let (doc, _) = build(&s, &opts(Machine::TenHz)).unwrap();
        assert_eq!(arm(&doc, 1, 0, 0).name, "01_West_00_Harp");
    }
}
