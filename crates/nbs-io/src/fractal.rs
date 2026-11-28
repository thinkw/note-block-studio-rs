//! nbs-fractal 机体投影导出（`.litematic`）。
//!
//! 和 [`crate::maestro`] 那条互补：MAESTRO 只出错码箱，贴到已建好的机体上才响；这里产出机体
//! 自己的几何。**但别把它说成「贴下去就能响」**——2026-11-23 用消费方库实测两份 Compact 投影
//! 里既没有时钟也没有电源（palette 零火把/红石块/比较器/观察者，TileEntities 0），喂电要用户
//! 自己接；接上之后能走多远，由 [`LaneStats`] 那条红石线自检说。
//! 几何、延迟元件、方块选择全部来自上游
//! `nbs-fractal`（AGPL-3.0-or-later，本仓 AGPL-3.0-only 的接收方可取 3.0；它的
//! `rsnbs` / `litematica` 依赖是 MIT OR Apache-2.0），我们不重写它任何一格几何。
//!
//! 20 t/s（每 game tick 一音）走上游那两档：`linear` 的 Scale1（奇刻落支道、活塞压）
//! 与 `compact` 的 halves（偶奇各拆一条 10 Hz 道）。三家 20 Hz 生态的机制同形——都是
//! 「两条 10 Hz 单元反相 1 刻」，没有谁把单个延迟元件压到 1 刻，所以这里不需要自拍常数。
//!
//! 音符取舍沿用 MAESTRO 那条口径（不是上游的口径）：上游的 `playable()` 对超窗音高与
//! 自定义乐器是**静默丢弃**，且布局侧拿不到音符盒状态时回落到链条方块，产物里看不出少了
//! 音。这里改成 ±12 拉回 33..=57 + 自定义乐器按采样文件名映射怪物头 + 映射不到的丢弃，
//! 三者都计数并写进 [`FractalReport::lines`]。
//!
//! 时间轴换算刻意保持上游 `rsnbs` 的做法（见 [`tempo_step`]）：它把 `target / tempo`
//! 近似成 `{n, 1/n}` 再整除，只有速度是 20 的整数比时才精确；不近似到整的档位会当场
//! 报错而不是悄悄改变播放时长。

use std::io::Cursor;
use std::num::NonZero;
use std::path::Path;

use nbs_core::model::{Song, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN, PAN_CENTER};
use nbs_core::nbs::{save_bytes, MAX_NBS_VERSION};
use nbs_core::tr;
use nbs_fractal::layout::compact::MultiCompactLayout;
use nbs_fractal::layout::linear::{MultiLinearLayout, ScaleMode, StackedLinearLayout};
use nbs_fractal::layout::{Layout, LayoutExt, Metadata};
use nbs_fractal::{BlockPos, BlockState};
use rsnbs::note::{Instrument as UpInstrument, Note as UpNote, NoteStreamExt, Notes};
use rsnbs::song::Song as Upstream;
use rsnbs::types::TimeAnchor;

use crate::maestro::{msg, MOB_SOUNDS};

// ==================================================== 选项

/// 布局族。上游有 9 个布局模块，这里只接两族能兑现 20 t/s 的。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FractalLayout {
    /// 主干＋支道的时间格；档位由时间戳自动选，最细一格一个 game tick。
    #[default]
    Linear,
    /// 变长紧凑延迟线，每组音符一条道；20 t/s 需要奇偶分道。
    Compact,
}

impl FractalLayout {
    pub const ALL: [Self; 2] = [Self::Linear, Self::Compact];

    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => tr("Linear (backbone + branch)", "Linear（主干＋支道）"),
            Self::Compact => tr("Compact (variable-length packed)", "Compact（变长紧凑）"),
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Linear => tr(
                "Cells step by 1, 2, 3 or 4 game ticks, whichever is the coarsest that divides every note timestamp; odd game ticks land on the branch line, driven by a piston. The speed comes from the song, not from a switch.",
                "每格走 1/2/3/4 个 game tick，取能整除所有音符时刻的最粗档；奇数刻落在支道上，由活塞压发。速度是这首歌本身的性质，不由开关决定。",
            ),
            Self::Compact => tr(
                "Packed delay lines, one track per note group; 20 t/s needs the even/odd split.",
                "变长紧凑延迟线，每组音符一条道；20 t/s 要靠奇偶分道。",
            ),
        }
    }

    /// 这一族能不能被「每 game tick 一音」开关影响。
    pub fn honors_half_ticks(self) -> bool {
        matches!(self, Self::Compact)
    }
}

/// 导出选项。默认值对齐上游 CLI 的出厂档，随 `settings.ini` 的 `[fractal_export]` 段持久化。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FractalOptions {
    pub layout: FractalLayout,
    /// 每 game tick 一音（上游 `--halves`）。`Linear` 本来就按时间戳自动选档，此项只对
    /// `Compact` 生效。
    pub twenty_hz: bool,
    /// 每行最多几列后换行，0 = 不换行（上游 Compact 的出厂值是 16）。
    pub wrap: usize,
    /// 相邻道之间的方块间距，0 = 啮合。
    pub gap: u32,
    /// 底部铺整层地板。
    pub full_floor: bool,
    /// 继电器延迟粗度 0..=4，0 = 不限（仅 `Compact` 用得到）。
    pub coarse: u32,
    /// 按空层拆分声道。
    pub breaks: bool,
}

impl Default for FractalOptions {
    fn default() -> Self {
        Self {
            layout: FractalLayout::Linear,
            twenty_hz: true,
            wrap: 0,
            gap: 0,
            full_floor: false,
            coarse: 0,
            breaks: true,
        }
    }
}

impl FractalOptions {
    /// 校验：把上游 CLI 的取值范围搬过来，越界当场拒，而不是让它 `NonZero` 崩。
    fn validate(&self, tempo: f32) -> Result<(), String> {
        if self.coarse > 4 {
            return Err(msg(
                format!(
                    "Repeater coarseness must be 0-4 (0 = unlimited), got {}.",
                    self.coarse
                ),
                format!("继电器粗度只能是 0-4（0 = 不限），当前是 {}。", self.coarse),
            ));
        }
        let target = if self.twenty_hz { 20 } else { 10 };
        // 上游在 tempo 落在 (0, 30) 之外时把 NBS tick 当作 game tick，只按 target/20 折叠，
        // 于是 40 t/s 的歌会原样铺到 20 t/s 网格上、播成两倍长。这条不报错但时长变了。
        if !(0.0..30.0).contains(&tempo) {
            return Err(msg(
                format!("Tempo {tempo} t/s is at or above the {target} t/s grid's ceiling; a note block machine cannot play faster than one note per game tick. Export the classic structure instead."),
                format!("速度 {tempo} t/s 不低于 {target} t/s 网格的上限，音符盒机器最快只能每 game tick 一音，装不下。请改走经典过道导出。"),
            ));
        }
        let (num, den) = tempo_step(tempo, target);
        let ideal = ideal_step(tempo, target);
        let exact = (ideal - num as f32 / den as f32).abs() < 1e-4;
        if !exact {
            return Err(msg(
                format!(
                    "Tempo {tempo} t/s is not a whole-number ratio of the {target} t/s grid ({ideal:.3}); the upstream resampler would round it and change the song's duration. Pick 20/10 t/s-tempo songs, or export the classic structure instead."
                ),
                format!(
                    "速度 {tempo} t/s 不是 {target} t/s 网格的整数比（比值 {ideal:.3}）；上游换算会取整并改变乐曲时长。请改用 20 或 10 的整数比速度，或走经典过道导出。"
                ),
            ));
        }
        Ok(())
    }
}

// ==================================================== 报告

/// 导出过程中对歌曲做过的改写与产出统计。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FractalReport {
    pub layout: FractalLayout,
    /// 机体每格实际间隔几个 game tick——也就是这台机器最快多久发一声。
    /// `Linear` 由上游按音符时刻自动选档（1/2/3/4），`Compact` 跟 `twenty_hz` 一致。
    pub cell_gt: u32,
    /// 每个原 NBS tick 折算成的目标 tick 数（[`tempo_step`] 的 `num / den`）。
    pub step_num: u32,
    pub step_den: u32,
    /// 歌曲原速度（ticks/秒）。
    pub tempo_from: f32,
    /// 输入音符总数。
    pub notes_in: usize,
    /// 实际进机体的音符数。
    pub notes_kept: usize,
    /// 被 ±12 移调拉回 33..=57 的音符数。
    pub transposed: usize,
    /// 自定义乐器命中怪物头、成功改写的音符数。
    pub remapped: usize,
    /// 自定义乐器命中不了怪物头、被丢弃的音符数。
    pub dropped_custom: usize,
    /// 携带力度/声像/音分偏移、但这些信息在音符盒里无处安放的音符数。
    pub fx_ignored: usize,
    /// 拆出来的道数（`Compact` 开奇偶分道时会翻倍）。
    pub tracks: usize,
    /// 用了铜块小号一族（NBS 16..19）的音符数：音符盒的 `instrument=trumpet*` 属性要 26.1 才有。
    pub trumpets: usize,
    /// 机体包围盒（方块）。
    pub size: [i32; 3],
    /// 用到的方块状态种数（palette 长度），给「铺不铺得开」那类核对留个抓手。
    pub palette_len: usize,
    /// 红石线连通段普查：见 [`LaneStats`]。
    pub lanes: LaneStats,
}

impl FractalReport {
    /// 这台机器的发声频率（音/秒）= 20 game tick ÷ 每格间隔。
    pub fn notes_per_sec(&self) -> f32 {
        if self.cell_gt == 0 {
            0.0
        } else {
            20.0 / self.cell_gt as f32
        }
    }

    /// 发声频率的显示串：每 3 格一机的 6.666… 不留一长串小数。
    pub fn grid_label(&self) -> String {
        crate::maestro::trim_num(self.notes_per_sec() as f64)
    }

    /// 变化说明：只列真正发生过的改写，给导出对话框直接显示。
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        let grid = self.grid_label();
        let gt = self.cell_gt;
        if self.layout == FractalLayout::Linear {
            v.push(msg(
                format!("Linear puts one cell every {gt} game tick(s), so it plays at {grid} notes/s. The scale is picked from the note timestamps: {gt} divides every one of them."),
                format!("Linear 每 {gt} 个 game tick 一格，也就是 {grid} 音/秒。档位由音符时刻自动选：这些时刻都能被 {gt} 整除。"),
            ));
        } else {
            v.push(msg(
                format!(
                    "Compact uses {} track(s), one cell every {gt} game tick(s), {grid} notes/s.",
                    self.tracks
                ),
                format!(
                    "Compact 用 {} 条道，每 {gt} 个 game tick 一格，{grid} 音/秒。",
                    self.tracks
                ),
            ));
        }
        if self.step_num != self.step_den {
            let ratio = self.step_num as f64 / self.step_den as f64;
            let collapse = if self.step_num < self.step_den {
                msg(
                    " Notes from the merged ticks sound together at the same instant.",
                    " 被并到一起的相邻两刻，音符会在同一刻同时响。",
                )
            } else {
                String::new()
            };
            v.push(msg(
                format!("Tempo {} t/s is rescaled by {ratio} onto this machine's grid, so the song keeps its length.{collapse}", crate::maestro::trim_num(self.tempo_from as f64)),
                format!("速度 {} t/s 按 {ratio} 折算到这台机器的网格（每 {gt} game tick 一格），乐曲时长保持不变。{collapse}", crate::maestro::trim_num(self.tempo_from as f64)),
            ));
        }
        if self.transposed > 0 {
            v.push(msg(
                format!(
                    "{} notes outside F#3-F#5 were moved by octaves.",
                    self.transposed
                ),
                format!("{} 个音符超出 F♯3–F♯5，已按八度移调。", self.transposed),
            ));
        }
        if self.remapped > 0 {
            v.push(msg(
                format!(
                    "{} notes on custom instruments were rewritten onto mob heads.",
                    self.remapped
                ),
                format!(
                    "{} 个自定义乐器音符按采样文件名改写到怪物头。",
                    self.remapped
                ),
            ));
        }
        if self.dropped_custom > 0 {
            v.push(msg(
                format!(
                    "{} notes could not be rendered by a note block and were dropped.",
                    self.dropped_custom
                ),
                format!(
                    "{} 个音符音符盒发不出来（自定义乐器映射不到怪物头），已丢弃。",
                    self.dropped_custom
                ),
            ));
        }
        if self.trumpets > 0 {
            v.push(msg(
                format!("{} notes use the copper trumpet family (NBS indexes 16-19); that note-block instrument property only exists from 26.1 on, so an older machine will not sound them.", self.trumpets),
                format!("{} 个音符用了铜块小号一族（NBS 索引 16-19）；音符盒的 instrument=trumpet* 属性要到 26.1 才有，老机体贴上去这些不会响。", self.trumpets),
            ));
        }
        if self.fx_ignored > 0 {
            v.push(msg(
                format!("Velocity, panning and pitch offsets are not carried by note blocks; {} notes are affected.", self.fx_ignored),
                format!("音符盒不承载力度/声像/音分，{} 个音符的这些信息不会生效。", self.fx_ignored),
            ));
        }
        v.push(msg(
            format!(
                "Bounding box {}x{}x{} blocks, {} block states.",
                self.size[0], self.size[1], self.size[2], self.palette_len
            ),
            format!(
                "包围盒 {}×{}×{} 格，{} 种方块状态。",
                self.size[0], self.size[1], self.size[2], self.palette_len
            ),
        ));
        // 电源这句只报实测格数。旧文案「自带红石，贴下去就能响」是错的：Compact 两份投影
        // 实测零电源（无火把/红石块/拉杆），Linear 那份反而一贴就带电。别再替机型下结论。
        let l = &self.lanes;
        if l.sources == 0 {
            v.push(msg(
                "Nothing in here feeds redstone dust (no redstone torch, redstone block or lever), so the wiring needs a power feed of your own before anything moves.",
                "这里面没有任何能给红石线供电的方块（红石火把、红石块、拉杆都没有），要你自己接一路喂电，线才会开始走。",
            ));
        } else {
            v.push(msg(
                format!(
                    "{} block(s) here feed redstone dust, so the projection is live the moment it is pasted.",
                    l.sources
                ),
                format!(
                    "这里面有 {} 格方块能给红石线供电，贴下去当场就带电。",
                    l.sources
                ),
            ));
        }
        if l.wire_cells == 0 {
            v.push(msg(
                "This layout lays no redstone wire, so the 15-block dust check does not apply to it.",
                "这份布局不铺红石线，红石线 15 格传导预算这条自检对它不适用。",
            ));
        } else if l.over == 0 {
            v.push(msg(
                format!(
                    "Dust check: {} connected run(s), longest {} block(s) - all inside the {}-block range, so one feed reaches every note on them.",
                    l.lanes, l.longest, DUST_BUDGET
                ),
                format!(
                    "红石线自检：{} 段连通线，最长 {} 格，都在 {} 格传导预算之内，从一头喂电就够得到全部音符。",
                    l.lanes, l.longest, DUST_BUDGET
                ),
            ));
        } else {
            let mut tail = format!(
                "Dust check: {} connected run(s), longest {} block(s); {} of them pass the {}-block range ({} blocks in those runs). Fed from one end only, {} block(s) sit past the budget and {} note block(s) hang on those over-length runs - which of them fall silent depends on where you feed from.",
                l.lanes,
                l.longest,
                l.over,
                DUST_BUDGET,
                l.cells_over,
                l.tail_cells,
                l.notes_over
            );
            let mut zh = format!(
                "红石线自检：{} 段连通线，最长 {} 格；其中 {} 段超过 {} 格传导预算（这类段共 {} 格）。只从一头喂电时，{} 格落在预算外拿不到电，挂在这些超预算段旁边的音符盒有 {} 颗——哪几颗不响看你从哪一头喂。",
                l.lanes,
                l.longest,
                l.over,
                DUST_BUDGET,
                l.cells_over,
                l.tail_cells,
                l.notes_over
            );
            if l.dead_both > 0 {
                tail.push_str(&format!(
                    " {} block(s) are past the budget from both ends, so no feed direction saves them.",
                    l.dead_both
                ));
                zh.push_str(&format!(
                    "另有 {} 格从两头算都超出预算，换喂电方向救不回来。",
                    l.dead_both
                ));
            }
            for (len, min, max) in l.worst.iter() {
                tail.push_str(&format!(
                    " Worst run {} blocks: local x[{}..{}] y[{}..{}] z[{}..{}].",
                    len, min[0], max[0], min[1], max[1], min[2], max[2]
                ));
                zh.push_str(&format!(
                    " 最坏一段 {} 格，局部坐标 x[{}..{}] y[{}..{}] z[{}..{}]（粘贴后按粘贴原点平移）。",
                    len, min[0], max[0], min[1], max[1], min[2], max[2]
                ));
            }
            v.push(msg(tail, zh));
        }
        v
    }
}

// ==================================================== 红石线自检

/// 红石线的传导预算。minecraft.wiki「Redstone Dust」原文：红石线「can transmit a redstone
/// power signal up to 15 blocks in distance」，每走一格掉 1 点强度，到 0 就走不动了。
pub const DUST_BUDGET: usize = 15;

/// 一格在自检里只分四种身份。连接关系取自上游烘进方块状态里的那四个属性，
/// 所以普查回答的是「文件声称怎么连」，不是「粘贴后游戏会怎么重算」——后者没实测过。
#[derive(Clone, Copy, PartialEq)]
enum Cell {
    Other,
    Note,
    Source,
    /// 四个方向各占 2 bit（0 不连 / 1 平连 / 2 上阶），位序同 [`DIRS`]。
    Wire(u8),
}

/// 红石线的四个方向。`up` 连的是高一格的线，所以那条偏移还要再加一层 y。
const DIRS: [(&str, i32, i32); 4] = [
    ("north", 0, -1),
    ("south", 0, 1),
    ("west", -1, 0),
    ("east", 1, 0),
];

/// 红石线连通段的普查结果。道数、长度、超预算的段数都是量出来的，不是按曲子长度推的。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneStats {
    /// 连通段总数。继电器、音符盒、固体方块都会把线切断，所以一段就是一截中间没有刷新点的裸线。
    pub lanes: usize,
    /// 这些段占了几格红石线。0 = 这份布局不铺线，自检不适用。
    pub wire_cells: usize,
    /// 最长一段几格。
    pub longest: usize,
    /// 超过 [`DUST_BUDGET`] 的段数。
    pub over: usize,
    /// 超预算段里的格数合计。
    pub cells_over: usize,
    /// `Σ(段长 − 15)`：只从一头喂电时，落在预算外、拿不到电的格数。
    pub tail_cells: usize,
    /// `Σ max(0, 段长 − 30)`：不管从哪一头喂都拿不到电的格数。
    pub dead_both: usize,
    /// 挂在超预算段旁边的音符盒颗数。哪几颗真不响取决于从哪一头喂，这个数是上界。
    pub notes_over: usize,
    /// 能给红石线供电的方块格数（`redstone_torch` / `redstone_wall_torch` /
    /// `redstone_block` / `lever`）。0 = 这份投影里没有电源，得自己接喂电。
    pub sources: usize,
    /// 最长的几段（最多 3 条）：`(段长, 局部包围盒最小角, 最大角)`。粘贴后按粘贴原点平移就是能 tp 的坐标。
    pub worst: Vec<(usize, [i32; 3], [i32; 3])>,
}

/// 把上游烘在红石线状态里的四个连接方向折成一个字节。
fn wire_bits(st: &BlockState) -> u8 {
    let mut bits = 0u8;
    for (k, (key, _, _)) in DIRS.iter().enumerate() {
        let v = match st.properties.get(*key).map(|c| c.as_ref()) {
            Some("side") => 1u8,
            Some("up") => 2u8,
            _ => 0u8,
        };
        bits |= v << (2 * k);
    }
    bits
}

/// 逐格读布局，折成 [`Cell`] 表，再交给 [`lanes_of`]。
fn census<L: Layout<()>>(layout: &L, size: [i32; 3]) -> LaneStats {
    let (sx, sy, sz) = (size[0] as usize, size[1] as usize, size[2] as usize);
    let mut cells = vec![Cell::Other; sx * sy * sz];
    for y in 0..size[1] {
        for z in 0..size[2] {
            for x in 0..size[0] {
                let Some(st) = layout.get_block(BlockPos::new(x, y, z)) else {
                    continue;
                };
                let i = (y as usize * sz + z as usize) * sx + x as usize;
                cells[i] = match st.name.as_ref() {
                    "minecraft:note_block" => Cell::Note,
                    "minecraft:redstone_wire" => Cell::Wire(wire_bits(&st)),
                    "minecraft:redstone_torch"
                    | "minecraft:redstone_wall_torch"
                    | "minecraft:redstone_block"
                    | "minecraft:lever" => Cell::Source,
                    _ => Cell::Other,
                };
            }
        }
    }
    lanes_of(size, &cells)
}

/// 并查集走连通段：只有当这格自己说「往这方向连」、而且那边也是红石线时才并进去。
fn lanes_of(size: [i32; 3], cells: &[Cell]) -> LaneStats {
    let (sx, sy, sz) = (size[0] as usize, size[1] as usize, size[2] as usize);
    let at = |x: usize, y: usize, z: usize| (y * sz + z) * sx + x;
    let (wx, wy, wz) = (sx as i32, sy as i32, sz as i32);
    let mut out = LaneStats::default();
    let mut ids = vec![u32::MAX; cells.len()];
    let mut nth: Vec<usize> = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        match c {
            Cell::Source => out.sources += 1,
            Cell::Wire(_) => {
                ids[i] = nth.len() as u32;
                nth.push(i);
            }
            _ => {}
        }
    }
    out.wire_cells = nth.len();
    if out.wire_cells == 0 {
        return out;
    }
    let n = nth.len();
    let mut par: Vec<u32> = (0..n as u32).collect();
    for (i, c) in cells.iter().enumerate() {
        let Cell::Wire(bits) = *c else { continue };
        let x = (i % sx) as i32;
        let z = ((i / sx) % sz) as i32;
        let y = (i / (sx * sz)) as i32;
        for (k, (_, dx, dz)) in DIRS.iter().enumerate() {
            let v = (bits >> (2 * k)) & 3;
            if v == 0 {
                continue;
            }
            let (nx, ny, nz) = (x + dx, y + (v == 2) as i32, z + dz);
            if nx < 0 || ny < 0 || nz < 0 || nx >= wx || ny >= wy || nz >= wz {
                continue;
            }
            let j = at(nx as usize, ny as usize, nz as usize);
            let b = ids[j];
            if b == u32::MAX {
                continue;
            }
            let (a, b) = (ids[i] as usize, b as usize);
            let (ra, rb) = (root(&mut par, a), root(&mut par, b));
            if ra != rb {
                par[rb] = ra as u32;
            }
        }
    }
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); n];
    for k in 0..n {
        members[root(&mut par, k)].push(k as u32);
    }
    let mut marked = vec![false; cells.len()];
    let mut worst: Vec<(usize, [i32; 3], [i32; 3])> = Vec::new();
    for m in members.iter().filter(|m| !m.is_empty()) {
        let l = m.len();
        out.lanes += 1;
        out.longest = out.longest.max(l);
        if l <= DUST_BUDGET {
            continue;
        }
        out.over += 1;
        out.cells_over += l;
        out.tail_cells += l - DUST_BUDGET;
        out.dead_both += l.saturating_sub(2 * DUST_BUDGET);
        let mut box_min = [i32::MAX; 3];
        let mut box_max = [i32::MIN; 3];
        for &w in m {
            let i = nth[w as usize];
            let (x, z, y) = ((i % sx) as i32, ((i / sx) % sz) as i32, (i / (sx * sz)) as i32);
            box_min = [box_min[0].min(x), box_min[1].min(y), box_min[2].min(z)];
            box_max = [box_max[0].max(x), box_max[1].max(y), box_max[2].max(z)];
            for (dx, dy, dz) in NEIGHBOURS {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if nx < 0 || ny < 0 || nz < 0 || nx >= wx || ny >= wy || nz >= wz {
                    continue;
                }
                let j = at(nx as usize, ny as usize, nz as usize);
                if cells[j] == Cell::Note && !marked[j] {
                    marked[j] = true;
                    out.notes_over += 1;
                }
            }
        }
        worst.push((l, box_min, box_max));
    }
    worst.sort_by(|a, b| b.0.cmp(&a.0));
    worst.truncate(3);
    out.worst = worst;
    out
}

/// 并查集找根，顺手做路径折半。
fn root(par: &mut Vec<u32>, a: usize) -> usize {
    let mut x = a;
    while par[x] as usize != x {
        let p = par[x] as usize;
        par[x] = par[p];
        x = par[x] as usize;
    }
    x
}

/// 六邻，用来找贴着线的音符盒。
const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

// ==================================================== 时间换算（照抄上游 rsnbs 的口径）

/// 上游 `rescale_to_tick_rate` 里 `target / tempo` 的理想值。
fn ideal_step(tempo: f32, target: u32) -> f32 {
    if (0.0..30.0).contains(&tempo) {
        target as f32 / tempo
    } else {
        // 上游：速度落在 (0, 30) 之外时当作 NBS tick 已等于 game tick。
        target as f32 / 20.0
    }
}

/// 上游把那比值近似成 `{n, 1/n}` 以便整除；这里复刻同一近似，好在报告里如实说明。
pub(crate) fn tempo_step(tempo: f32, target: u32) -> (u32, u32) {
    let scale = ideal_step(tempo, target);
    if scale >= 1.0 {
        (scale.round() as u32, 1)
    } else {
        (1, (1.0 / scale).round() as u32)
    }
}

// ==================================================== 核心

/// 把音符表搬成上游认得的 `.nbs` 字节，顺手做掉音域与乐器改写并记账。
fn prepare(song: &Song) -> Result<(Vec<u8>, FractalReport), String> {
    let mut rep = FractalReport {
        notes_in: song.total_blocks,
        tempo_from: song.meta.tempo,
        ..Default::default()
    };
    let customs = song.custom_instruments.clone();
    // 官方 `mobInstruments` 查表：自定义乐器 -> 怪物头；查不到就丢。同 MAESTRO 一张表。
    let remap_target = |instrument: u8| -> Option<u8> {
        let custom = (instrument as usize)
            .checked_sub(FIRST_CUSTOM_INDEX as usize)
            .and_then(|i| customs.get(i))?;
        MOB_SOUNDS
            .iter()
            .find(|(file, _)| *file == custom.filename)
            .map(|(_, id)| *id)
    };

    let mut out = Song {
        meta: song.meta.clone(),
        layers: song.layers.clone(),
        custom_instruments: customs.clone(),
        ..Default::default()
    };

    for (&tick, col) in &song.columns {
        for (&layer, note) in &col.notes {
            let mut note = *note;
            if note.instrument >= FIRST_CUSTOM_INDEX {
                match remap_target(note.instrument) {
                    Some(id) => {
                        rep.remapped += 1;
                        note.instrument = id;
                    }
                    None => {
                        rep.dropped_custom += 1;
                        continue;
                    }
                }
            }
            // 音高拉回 33..=57：±12 反复，直到落进窗内。
            let mut key = note.key as i32;
            while key < MC_KEY_MIN as i32 {
                key += 12;
            }
            while key > MC_KEY_MAX as i32 {
                key -= 12;
            }
            if key != note.key as i32 {
                rep.transposed += 1;
                note.key = key as u8;
            }
            if note.velocity != 100 || note.panning != PAN_CENTER || note.pitch != 0 {
                rep.fx_ignored += 1;
            }
            rep.notes_kept += 1;
            out.set(tick, layer, note);
        }
    }
    if out.total_blocks == 0 {
        return Err(msg(
            "No note left can be rendered by a note block.",
            "没有任何音符是音符盒能发出来的。",
        ));
    }
    let bytes = save_bytes(&out, MAX_NBS_VERSION).map_err(|e| {
        msg(
            format!("Failed to write the intermediate .nbs: {e}"),
            format!("中间 .nbs 写入失败：{e}"),
        )
    })?;
    Ok((bytes, rep))
}

/// Compact 的一条道：格号（机器刻）+ 音符，外加继电器粗度。
type CompactedTrack = (Vec<(u32, UpNote)>, Option<NonZero<u32>>);

/// `(机体, 道数, 折算目标网格 t/s, 每格几个 game tick, 小号一族颗数)`。
type Assembled = (Box<dyn Layout<()>>, usize, u32, u32, usize);

/// 转成上游布局对象。返回机体、道数与实际时间基。
fn assemble(
    bytes: &[u8],
    opts: &FractalOptions,
) -> Result<Assembled, String> {
    let parsed = Upstream::parse(&mut Cursor::new(bytes)).map_err(|e| {
        msg(
            format!("The upstream NBS reader refused the song: {e}"),
            format!("上游 NBS 解析器拒绝这份曲子：{e}"),
        )
    })?;
    let tempo = parsed.header.tempo;
    let target = if opts.twenty_hz { 20 } else { 10 };
    // 铜块小号一族（NBS v6 的 16..19）是 26.1 才有的音符盒乐器属性，老机体的
    // note_block 状态里没有这一项；颗数如实报，不替用户的 MC 版本做决定。
    let trumpets = parsed
        .notes
        .iter()
        .filter(|(_, n)| {
            matches!(
                n.tone.instrument,
                UpInstrument::Trumpet
                    | UpInstrument::TrumpetExposed
                    | UpInstrument::TrumpetWeathered
                    | UpInstrument::TrumpetOxidized
            )
        })
        .count();

    let (layout, n_tracks, rate, cell_gt) = match opts.layout {
        FractalLayout::Linear => {
            // 上游 Linear 一律落 game tick 域，档位（Scale4/3/2/1）由时间戳自动选。
            let notes: Notes = parsed.notes.rescale_to_game_tick(tempo).collect();
            let song_length = notes.tick_count();
            let tracks: Vec<Notes> = if opts.breaks {
                notes.split_by_layer_gaps()
            } else {
                vec![notes]
            };
            let count = tracks.len();
            // 上游按「能整除所有音符时刻的最粗档」选 ScaleMode，所以档位是这首歌的性质，
            // 不是我们的设置；这里量出来如实报，别替他喊 20 t/s。
            let cell_gt = ScaleMode::new(
                tracks
                    .iter()
                    .flat_map(|t| t.keys().map(|pos| pos.into_tick())),
            )
            .scale();
            let layout: Box<dyn Layout<()>> = match NonZero::new(opts.wrap) {
                Some(wrap) => Box::new(StackedLinearLayout::new(
                    tracks,
                    Some(wrap),
                    opts.gap,
                    opts.full_floor,
                    song_length,
                )),
                None => Box::new(MultiLinearLayout::new(tracks, opts.gap, song_length)),
            };
            (layout, count, 20, cell_gt)
        }
        FractalLayout::Compact => {
            let coarse = NonZero::new(opts.coarse);
            let groups: Vec<Notes> = if opts.breaks {
                parsed.notes.split_by_layer_gaps()
            } else {
                vec![parsed.notes]
            };
            let mut tracks: Vec<CompactedTrack> = Vec::new();
            // 两条反相道是不是都真用上了：只用上一条时这台机器仍是 2 刻一格，
            // 不能因为开关亮着就喊 20 音/秒。
            let mut halves_used = [false; 2];
            for group in groups {
                let stream = group
                    .rescale_to_tick_rate(tempo, target)
                    .map(|(pos, note)| (pos.into_tick(), note));
                if opts.twenty_hz {
                    // 上游 split_halves：共用一个红石刻的两颗音符拆到两条道上。
                    let mut lanes: [Vec<(u32, UpNote)>; 2] = Default::default();
                    for (tick, note) in stream {
                        lanes[(tick % 2) as usize].push((tick / 2, note));
                    }
                    for (i, lane) in lanes.iter().enumerate() {
                        halves_used[i] |= !lane.is_empty();
                    }
                    tracks.extend(
                        lanes
                            .into_iter()
                            .filter(|l| !l.is_empty())
                            .map(|l| (l, coarse)),
                    );
                } else {
                    let line: Vec<(u32, UpNote)> = stream.collect();
                    if !line.is_empty() {
                        tracks.push((line, coarse));
                    }
                }
            }
            let count = tracks.len();
            let layout: Box<dyn Layout<()>> = Box::new(MultiCompactLayout::new(
                tracks,
                NonZero::new(opts.wrap),
                opts.gap,
                opts.full_floor,
            ));
            // Compact 的一条道恒是 2 game tick 一格；奇偶两条道都用上，
            // 错开的那 1 刻才真在机器里，相邻发声间隔才降到 1 刻。
            let both_halves = halves_used[0] && halves_used[1];
            (
                layout,
                count,
                target,
                if opts.twenty_hz && both_halves { 1 } else { 2 },
            )
        }
    };
    Ok((layout, n_tracks, rate, cell_gt, trumpets))
}

/// 产出 `(litematic 字节, 报告)`，不落盘。
pub fn build_bytes(song: &Song, opts: &FractalOptions) -> Result<(Vec<u8>, FractalReport), String> {
    opts.validate(song.meta.tempo)?;
    let (bytes, mut rep) = prepare(song)?;
    let (layout, tracks, rate, cell_gt, trumpets) = assemble(&bytes, opts)?;

    rep.layout = opts.layout;
    rep.cell_gt = cell_gt;
    let (num, den) = tempo_step(song.meta.tempo, rate);
    rep.step_num = num;
    rep.step_den = den;
    rep.tracks = tracks;
    rep.trumpets = trumpets;
    let size = layout.size();
    rep.size = [size.x, size.y, size.z];
    rep.palette_len = layout.block_palette().len();
    rep.lanes = census(&layout, rep.size);

    let name = if song.meta.name.is_empty() {
        song.filename
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("nbs-rs projection")
            .to_owned()
    } else {
        song.meta.name.clone()
    };
    let author = song.meta.author.clone();
    let description = msg(
        format!(
            "{} layout from nbs-rs, built with nbs-fractal.",
            opts.layout.label()
        ),
        format!(
            "由 nbs-rs 生成的 {} 布局，几何取自 nbs-fractal。",
            opts.layout.label()
        ),
    );
    let metadata = Metadata {
        name: &name,
        description: &description,
        author: &author,
    };
    let mut buf = Vec::new();
    layout.as_litematic(metadata, &mut buf).map_err(|e| {
        msg(
            format!("The litematic writer failed: {e}"),
            format!("投影写出失败：{e}"),
        )
    })?;
    Ok((buf, rep))
}

/// 只算报告不写文件，给导出窗口的预览用。
pub fn preview(song: &Song, opts: &FractalOptions) -> Result<FractalReport, String> {
    build_bytes(song, opts).map(|(_, report)| report)
}

/// 导出到 `path`，返回报告。
pub fn export(song: &Song, path: &Path, opts: &FractalOptions) -> Result<FractalReport, String> {
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
    use nbs_core::model::Note;

    fn song(tempo: f32, notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::default();
        s.meta.tempo = tempo;
        s.layers = vec![nbs_core::model::Layer::default(); 4];
        for &(tick, layer, instrument, key) in notes {
            s.set(tick, layer, Note::new(instrument, key));
        }
        s
    }

    #[test]
    fn linear_export_is_a_gzip_litematic_and_keeps_every_note() {
        let s = song(
            20.0,
            &[
                (0, 0, 0, 45),
                (1, 0, 1, 47),
                (2, 1, 2, 48),
                (3, 1, 3, 50),
                (4, 0, 0, 45),
            ],
        );
        let opts = FractalOptions::default();
        let (bytes, rep) = build_bytes(&s, &opts).unwrap();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "产物该是 gzip");
        assert_eq!(rep.notes_in, 5);
        assert_eq!(rep.notes_kept, 5);
        assert_eq!(
            (rep.transposed, rep.remapped, rep.dropped_custom),
            (0, 0, 0)
        );
        assert_eq!((rep.cell_gt, rep.notes_per_sec()), (1, 20.0));
        assert!(rep.size[0] > 0 && rep.size[1] > 0 && rep.size[2] > 0);
        assert!(rep.palette_len > 1);
    }

    #[test]
    fn the_upstream_parser_reads_what_our_writer_writes() {
        // 用第三方解析器验自家 .nbs 写出器：颗数与速度必须一字不差。
        let s = song(10.0, &[(0, 0, 0, 45), (2, 1, 5, 47), (7, 2, 9, 52)]);
        let (bytes, rep) = prepare(&s).unwrap();
        let up = Upstream::parse(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(up.notes.iter().count(), rep.notes_kept);
        assert!((up.header.tempo - 10.0).abs() < f32::EPSILON);
        // 音高要一字不差地回来：Key 存的是 MC note 0..=24，加回 33 才是 NBS 音高。
        let mut got: Vec<(u32, u8)> = up
            .notes
            .iter()
            .map(|(pos, note)| {
                (
                    pos.into_tick(),
                    note.tone.key.minecraft_note().unwrap() + 33,
                )
            })
            .collect();
        got.sort();
        assert_eq!(got, vec![(0, 45), (2, 47), (7, 52)]);
    }

    #[test]
    fn out_of_range_keys_move_by_octaves() {
        let s = song(20.0, &[(0, 0, 0, 21), (1, 0, 0, 69), (2, 0, 0, 45)]);
        let (_, rep) = build_bytes(&s, &FractalOptions::default()).unwrap();
        assert_eq!(rep.transposed, 2);
        assert_eq!(rep.notes_kept, 3);
    }

    #[test]
    fn custom_instrument_without_a_mob_head_sample_is_dropped() {
        let mut s = song(20.0, &[(0, 0, FIRST_CUSTOM_INDEX, 45), (1, 0, 0, 45)]);
        s.custom_instruments = vec![nbs_core::model::CustomInstrument {
            name: "Pad".into(),
            filename: "not_a_mob_head".into(),
            key: 45,
            press: true,
        }];
        let (_, rep) = build_bytes(&s, &FractalOptions::default()).unwrap();
        assert_eq!(rep.dropped_custom, 1);
        assert_eq!(rep.notes_kept, 1);
        assert!(rep
            .lines()
            .iter()
            .any(|l| l.contains("丢弃") || l.contains("dropped")));
    }

    #[test]
    fn compact_halves_doubles_the_track_count() {
        // 同一首曲子只差 `twenty_hz` 这一个因子：10 Hz 一条道，20 Hz 拆成奇偶两条。
        let s = song(
            20.0,
            &[(0, 0, 0, 45), (1, 0, 0, 46), (2, 0, 0, 47), (3, 0, 0, 48)],
        );
        let opts = FractalOptions {
            layout: FractalLayout::Compact,
            breaks: false,
            ..FractalOptions::default()
        };
        let (_, ten) = build_bytes(
            &s,
            &FractalOptions {
                twenty_hz: false,
                ..opts
            },
        )
        .unwrap();
        let (_, twenty) = build_bytes(&s, &opts).unwrap();
        assert_eq!((ten.tracks, ten.cell_gt), (1, 2));
        assert_eq!((twenty.tracks, twenty.cell_gt), (2, 1));
        assert_ne!(ten.size, twenty.size, "两条档位的机体形状本来就不同");
        assert!(
            twenty.size[1] > ten.size[1],
            "两条道竖直堆叠，高度该更大：{:?} vs {:?}",
            ten.size,
            twenty.size
        );
    }

    /// 20 t/s 开关亮着、但这首歌根本没有奇数刻的音符：反相那条道是空的，
    /// 机器只剩一条 2 刻一格的道。报告不许跟着开关喊 20 音/秒。
    #[test]
    fn compact_reports_the_grid_it_can_actually_play() {
        let even_only = song(
            20.0,
            &[(0, 0, 0, 45), (2, 0, 0, 47), (4, 0, 0, 49), (6, 0, 0, 51)],
        );
        let opts = FractalOptions {
            layout: FractalLayout::Compact,
            breaks: false,
            ..FractalOptions::default()
        };
        let (_, rep) = build_bytes(&even_only, &opts).unwrap();
        assert_eq!(
            (rep.tracks, rep.cell_gt, rep.notes_per_sec()),
            (1, 2, 10.0),
            "只用上一条道时报的是这台机器的真实间隔"
        );

        // 只差一个因子：把其中一颗挪到奇数刻，两条道都用上了，才是每 game tick 一格。
        let mixed = song(
            20.0,
            &[(0, 0, 0, 45), (1, 0, 0, 47), (4, 0, 0, 49), (6, 0, 0, 51)],
        );
        let (_, both) = build_bytes(&mixed, &opts).unwrap();
        assert_eq!(
            (both.tracks, both.cell_gt, both.notes_per_sec()),
            (2, 1, 20.0)
        );
    }

    #[test]
    fn a_tempo_that_is_not_a_whole_ratio_is_refused() {
        // 上游会把 20/15 取整成 1，等于把时长改掉 1.33 倍；我们当场拒绝。
        let s = song(15.0, &[(0, 0, 0, 45)]);
        let err = build_bytes(&s, &FractalOptions::default()).unwrap_err();
        assert!(err.contains("15"), "{err}");
    }

    #[test]
    fn tempo_step_matches_the_upstream_approximation() {
        assert_eq!(tempo_step(20.0, 20), (1, 1));
        assert_eq!(tempo_step(10.0, 20), (2, 1));
        assert_eq!(tempo_step(15.0, 20), (1, 1), "1.33 会被上游取整成 1");
        // 上游对 (0,30) 之外的速度按「NBS tick 已等于 game tick」处理：40 折到 20 是 1，
        // 折到 10 才是 1/2。这条分支等于把 40 t/s 当 20 t/s 用，时长会变，所以我们拒掉。
        assert_eq!(tempo_step(40.0, 20), (1, 1));
        assert_eq!(tempo_step(40.0, 10), (1, 2));
    }

    #[test]
    fn a_tempo_at_or_above_the_grid_is_refused() {
        // 每 game tick 一音已经是最快的一档，比它更快的源歌铺不进去。
        let s = song(40.0, &[(0, 0, 0, 45), (1, 0, 0, 46)]);
        let err = build_bytes(&s, &FractalOptions::default()).unwrap_err();
        assert!(err.contains("40"), "{err}");
    }

    #[test]
    fn linear_ignores_the_half_tick_switch() {
        let s = song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 46)]);
        let (_, a) = build_bytes(&s, &FractalOptions::default()).unwrap();
        let (_, b) = build_bytes(
            &s,
            &FractalOptions {
                twenty_hz: false,
                ..FractalOptions::default()
            },
        )
        .unwrap();
        assert_eq!((a.cell_gt, b.cell_gt), (1, 1));
        assert_eq!(a.size, b.size);
    }

    /// 逐格读布局，收集所有音符盒的坐标与状态——时间轴是哪根轴不靠猜。
    fn note_blocks(layout: Box<dyn Layout<()>>) -> Vec<(i32, i32, i32, String, String)> {
        use nbs_fractal::BlockPos;
        let size = layout.size();
        let mut out = Vec::new();
        for x in 0..size.x {
            for y in 0..size.y {
                for z in 0..size.z {
                    if let Some(b) = layout.get_block(BlockPos::new(x, y, z)) {
                        if &b.name[..] == "minecraft:note_block" {
                            out.push((
                                x,
                                y,
                                z,
                                b.properties
                                    .get("instrument")
                                    .map(|c| c.to_string())
                                    .unwrap_or_default(),
                                b.properties
                                    .get("note")
                                    .map(|c| c.to_string())
                                    .unwrap_or_default(),
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    fn compact_notes(ticks: &[(u32, u8)], hz: u32) -> Vec<(i32, i32, i32, String, String)> {
        let s = song(
            20.0,
            &ticks
                .iter()
                .map(|(t, k)| (*t, 0u32, 0u8, *k))
                .collect::<Vec<_>>(),
        );
        let opts = FractalOptions {
            layout: FractalLayout::Compact,
            twenty_hz: hz == 20,
            breaks: false,
            ..FractalOptions::default()
        };
        let bytes = prepare(&s).unwrap().0;
        let (layout, _, _, _, _) = assemble(&bytes, &opts).unwrap();
        note_blocks(layout)
    }

    #[test]
    fn linear_scale_is_a_property_of_the_timestamps() {
        // 上游 Linear 取「能整除所有音符时刻的最粗档」：全偶刻的歌只能出 2 gt 一格（10 音/秒），
        // 加一颗奇刻才降到 1 gt。所以 20 音/秒不是我们设出来的，是这首歌本来达不到的。
        let even = song(20.0, &[(0, 0, 0, 45), (2, 0, 0, 47), (4, 0, 0, 49)]);
        let (_, rep) = build_bytes(&even, &FractalOptions::default()).unwrap();
        assert_eq!((rep.cell_gt, rep.notes_per_sec()), (2, 10.0));
        let mixed = song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 47), (2, 0, 0, 49)]);
        let (_, rep) = build_bytes(&mixed, &FractalOptions::default()).unwrap();
        assert_eq!((rep.cell_gt, rep.notes_per_sec()), (1, 20.0));
    }

    #[test]
    fn compact_lanes_realize_the_game_tick_offset() {
        // 同一份四刻输入，只差 hz 这一个因子，逐格读音符盒落在哪儿。
        let notes = [(0u32, 45u8), (1, 47), (2, 49), (3, 51)];
        let twenty = compact_notes(&notes, 20);
        let ten = compact_notes(&notes, 10);
        assert_eq!((twenty.len(), ten.len()), (4, 4), "并刻不许丢音");

        // 20 Hz：奇偶两颗进两条道（y 不同），各自每 2 个 z 一格；那 1 刻的差在道的相位上。
        let lanes: Vec<i32> = {
            let mut v: Vec<i32> = twenty.iter().map(|b| b.1).collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(lanes.len(), 2, "奇偶分道后该有两条道：{twenty:?}");
        for y in lanes.clone() {
            let mut in_lane: Vec<i32> = twenty.iter().filter(|b| b.1 == y).map(|b| b.2).collect();
            in_lane.sort();
            assert_eq!(in_lane.len(), 2);
            assert_eq!(in_lane[1] - in_lane[0], 2, "一条道内相邻格隔 2 刻");
        }

        // 10 Hz：只剩一条道，被折到一起的两颗同处一格（同 y 同 z），于是一起响而不是排成两拍。
        assert_eq!(
            ten.iter()
                .map(|b| b.1)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );
        let mut cells: std::collections::BTreeMap<i32, Vec<String>> = Default::default();
        for b in &ten {
            cells.entry(b.2).or_default().push(b.4.clone());
        }
        let mut cells: Vec<(i32, Vec<String>)> = cells
            .into_iter()
            .map(|(z, mut ns)| {
                ns.sort();
                (z, ns)
            })
            .collect();
        cells.sort();
        assert_eq!(
            cells,
            vec![
                (0, vec!["12".into(), "14".into()]),
                (2, vec!["16".into(), "18".into()])
            ],
            "tick0+1 与 tick2+3 各自并到同一格"
        );
    }

    // -------------------------------------------------- 红石线自检

    /// 沿 z 铺 `n` 格裸线所需的方向位：首格只往南、末格只往北、中间两头都连。
    fn z_bits(i: usize, n: usize) -> u8 {
        (i > 0) as u8 | (((i + 1 < n) as u8) << 2)
    }

    struct Sheet {
        size: [i32; 3],
        cells: Vec<Cell>,
    }

    impl Sheet {
        fn new(x: i32, y: i32, z: i32) -> Self {
            Self {
                size: [x, y, z],
                cells: vec![Cell::Other; (x * y * z) as usize],
            }
        }
        fn put(&mut self, x: i32, y: i32, z: i32, c: Cell) {
            let i =
                ((y as usize) * self.size[2] as usize + z as usize) * self.size[0] as usize + x as usize;
            self.cells[i] = c;
        }
        /// 在 (x,y,z0) 起沿 z 铺一条 `n` 格的红石线。
        fn lane_z(&mut self, x: i32, y: i32, z0: i32, n: usize) {
            for i in 0..n {
                self.put(x, y, z0 + i as i32, Cell::Wire(z_bits(i, n)));
            }
        }
        fn stats(&self) -> LaneStats {
            lanes_of(self.size, &self.cells)
        }
    }

    #[test]
    fn a_lane_past_the_dust_budget_is_flagged_with_its_dead_tail() {
        let mut s = Sheet::new(1, 1, 20);
        s.lane_z(0, 0, 0, 20);
        let l = s.stats();
        assert_eq!((l.lanes, l.longest, l.over, l.cells_over), (1, 20, 1, 20));
        // 只从一头喂：第 16 格起拿不到电。
        assert_eq!(l.tail_cells, 5);
        // 换另一头喂就能救回来，所以「两头都够不着」是 0。
        assert_eq!(l.dead_both, 0);
        assert_eq!(l.worst, vec![(20, [0, 0, 0], [0, 0, 19])]);
    }

    #[test]
    fn a_lane_long_enough_to_lose_its_middle_cannot_be_saved_by_the_feed_side() {
        let mut s = Sheet::new(1, 1, 31);
        s.lane_z(0, 0, 0, 31);
        let l = s.stats();
        assert_eq!((l.longest, l.over, l.tail_cells), (31, 1, 16));
        assert_eq!(l.dead_both, 1);
    }

    #[test]
    fn anything_that_is_not_wire_cuts_a_lane() {
        // 中间换成一格非线方块（游戏里就是一颗继电器），两段各 10/9 格，谁都不超预算。
        let mut s = Sheet::new(1, 1, 20);
        s.lane_z(0, 0, 0, 20);
        s.put(0, 0, 10, Cell::Other);
        let l = s.stats();
        assert_eq!(
            (l.lanes, l.longest, l.over, l.wire_cells),
            (2, 10, 0, 19)
        );
    }

    #[test]
    fn an_up_connection_reaches_the_wire_one_level_higher() {
        // (0,0,1) 说自己北向是 up，连到高一格的 (0,1,0)；这两格必须算同一段。
        let mut s = Sheet::new(1, 2, 2);
        s.put(0, 0, 1, Cell::Wire(2));
        s.put(0, 1, 0, Cell::Wire(1 << 2));
        assert_eq!((s.stats().lanes, s.stats().longest), (1, 2));
    }

    #[test]
    fn a_note_touching_two_over_budget_lanes_is_counted_once() {
        let mut s = Sheet::new(3, 1, 16);
        s.lane_z(0, 0, 0, 16);
        s.lane_z(2, 0, 0, 16);
        s.put(1, 0, 1, Cell::Note);
        let l = s.stats();
        assert_eq!((l.lanes, l.over), (2, 2));
        assert_eq!(l.notes_over, 1);
    }

    #[test]
    fn power_blocks_are_counted_and_a_wireless_grid_reports_no_lanes() {
        let mut s = Sheet::new(1, 1, 4);
        s.put(0, 0, 0, Cell::Source);
        s.put(0, 0, 1, Cell::Source);
        let l = s.stats();
        assert_eq!((l.sources, l.lanes, l.wire_cells, l.longest), (2, 0, 0, 0));
    }

    #[test]
    fn the_report_names_the_over_budget_lanes_instead_of_promising_a_playing_machine() {
        let mut rep = FractalReport {
            lanes: LaneStats {
                lanes: 5776,
                wire_cells: 48091,
                longest: 27,
                over: 1383,
                cells_over: 27786,
                tail_cells: 7041,
                dead_both: 0,
                notes_over: 29149,
                sources: 0,
                worst: vec![(27, [767, 10, 0], [769, 11, 23])],
            },
            ..Default::default()
        };
        let lines = rep.lines();
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("贴下去就能响") || l.contains("it needs no prebuilt")),
            "{lines:?}"
        );
        assert!(lines
            .iter()
            .any(|l| l.contains("没有任何能给红石线供电") || l.contains("Nothing in here feeds")));
        let check = lines
            .iter()
            .find(|l| l.contains("红石线自检") || l.contains("Dust check"))
            .expect("超预算段要进报告");
        for n in ["5776", "27", "1383", "27786", "7041", "29149", "767"] {
            assert!(check.contains(n), "缺 {n}：{check}");
        }
        assert!(!check.contains("两头"), "dead_both 为 0 时不该提这句：{check}");

        rep.lanes.dead_both = 10381;
        let check = rep
            .lines()
            .into_iter()
            .find(|l| l.contains("红石线自检") || l.contains("Dust check"))
            .unwrap();
        assert!(check.contains("10381"), "{check}");
    }
}
