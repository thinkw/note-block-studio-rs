//! 特效：声道方块（按声像分槽，摆的是**这一帧在响哪些乐器**）。
//!
//! 和方块排的区别只在「维度」：方块排一格 = 一枚音符，压在它按的那枚键上；这里
//! 一格 = **一个（声道槽 × 一件乐器）**——同一个槽里同一件乐器不管来了几个音、
//! 来自几层，都只占一格。所以密集和弦不会把画面糊成一堵墙，一眼能数出
//! 「现在响的是哪几件、偏左还是偏右」。
//!
//! 槽用的是**真正听到的**声像（[`super::timeline::FxNote::audible_pan`]，轨的声道会把
//! 音符自己的 pan 往中间拉一半），不是 `note.panning` 的裸值；而且只做分箱，
//! 不按 0..=200 精确摆位 —— 耳朵分不出 137 和 142，画面要的是「偏右那档」。
//!
//! 中间那一槽用金字塔形状（3 = 2+1、6 = 3+2+1、20 = 6+5+4+3+2），因为大部分声音
//! 都落在中间，值得给它一套能塞下 20 件乐器还不显乱排法；两侧每排只放一枚，
//! 竖着往上长 —— 偏音的乐器本来就少，排成一列更像「哪边有声音」。
//!
//! 摞法是「叠」不是「串」：排距 [`ROW_STEP`] 和列距 [`COL_STEP`] 都小于一个边长，上面那枚
//! 的底尖咬进下面那枚的顶面（0.9 这个数是量用户发的参考图得到的）。底排最左那几格可以固定
//! 留给 bass drum / snare drum / click（[`PINNED_PERCUSSION`] + `pin_percussion` 那条勾）；
//! 其余乐器的先后由 [`StackOrder`] 决定 —— 出厂按乐器号升序（塔形和播放先后无关，但一枚号更
//! 小的乐器进场会把已有的顶起来），切到「按进栈先后」则谁先响谁占底、后进的只往塔顶摞。
//! 「先响」算的是曲子里这一段连续发声的第一枚（[`Runs`]），不是屏幕上这个窗口里最老的那枚 ——
//! 后者的锚点每帧被左缘推着往前挪，密集连发的那枚会因此一点点爬过旁边只响了一次的乐器。
//!
//! 摆放是纯函数（[`center_shape`] / [`slot_cells`] / [`slot_peaks`] / [`arrange`]），
//! 观感调起来才能单测钉住。每槽占多宽**不逐帧现算**：开机/换曲/改参数时把整曲扫一遍
//! （[`slot_peaks`] 给峰值、[`run_starts`] 给段起点），区域和锚点都按整曲预留，
//! 所以侧槽的横位置、每格在塔里的先后，都在整首曲子里钉死。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Ui, Vec2};

use super::blockrow::{BlockRow, MAX_BLOCKS};
use super::stage::Frame;
use super::tiles::{tile_size, TileLook};
use super::timeline::FxNote;
use super::{fade, note_block, EffectInfo, FxContext, PlaybackEffect, Surface};
use crate::tr;

/// 声像取值范围（0 = 最左，100 = 正中，200 = 最右）。
pub const PAN_MAX: u32 = 200;

/// 列与列之间的横向步距（乘边长）。
///
/// 方块贴图的内容宽只有 268/300 ≈ 0.893（左右各留一点透明边），所以旧的 1.05 实际是
/// 「每格之间空 16%」，金字塔的斜边看着散。参考图（用户发的截图）里塔基那两枚的间距
/// 量出来 ≈ 1.02 个内容宽 ⇒ 换成边长就是 0.91，取 0.9：相邻两枚的竖边刚好贴上
/// （用户：三角叠的靠得比较近），再小就要互相切进身体了。
pub const COL_STEP: f32 = 0.9;

/// 排与排之间的竖向步距（乘边长）。
///
/// 0.9 是从参考图里量的：左塔那串方块，金色那颗顶顶点在 y=20、底顶点在 y=93（内容高
/// 73 px），第五颗的顶顶点在 y≈284 ⇒ 排距 66 px = **0.904 个方块高**，也就是上面那枚的
/// 底尖咬进下面那枚的顶面一点（用户：上方会叠住下方的一部分），同时每枚的顶面还看得见、
/// 数得出排数。
///
/// 还能更紧到 0.55 —— 那是贴图自身的竖向重复距离（内容高 296 里竖直棱占 163，实测自
/// `assets/BlocksIso` 那 20 张，bbox 全是 `(16,2)-(284,298)`）：排到这个数柱子就摞成一根
/// 连续的整体，下面那枚的顶面被完全盖住，代价是塔身糊成一坨、排数数不清。
/// 旧的 1.05（横竖共用一个数）则是每排之间空出半格，塔看着是一串悬空的方块。
pub const ROW_STEP: f32 = 0.9;

/// 自动缩小的**底线**（像素，未乘倍率）：宽度/高度把边长夹到这里就停，再挤不缩了，
/// 改成从顶上往下裁整排。用户自己用「方块大小」调得更小时不受这条限制。
///
/// 从 18 抬到 26：侧槽贴过来之后横向余量变大，缩小的来源基本只剩画面高度；18 px 时
/// 矮窗口里的塔糊成一列马赛克，26 在 2× 缩放下仍认得出方块。代价是矮窗口更早开始裁顶排
/// （`note()` 那句「画面高度不够」会更常见）。
pub const MIN_TILE: f32 = 26.0;

/// 相邻两槽之间**至少**留的空隙（像素，未乘倍率）：再挤也不许两坨贴脸上。
const SLOT_GAP: f32 = 12.0;

/// 相邻两槽之间**最多**留的空隙（像素，未乘倍率）。
///
/// 侧槽每排只有一枚，本来就不需要一格等宽的键道：窗口再宽，超出这个缝的余量只用来把
/// 整组居中留白，不再把侧槽往两边推（用户：其他槽不用隔这么远，因为他们只会纵向放方块）。
/// 48 = 一个方块那么宽，比原来的等分槽距（1600 宽 5 槽 ≈ 313）近了一个量级。
const SPREAD_MAX: f32 = 48.0;

/// 「缩进起始」的范围：1 = 一上来就走金字塔，20 = 一直摞「每排两枚」的细柱。
pub const INDENT_MIN: u32 = 1;
pub const INDENT_MAX: u32 = INSTRUMENT_CAP;

/// 「缩进起始」的出厂值 —— 就是形状表原来硬编码的那个 5，摆出来的塔和改动前逐项一样。
pub const INDENT_DEFAULT: u32 = 5;

/// 画面左右各留的边（像素，未乘倍率）：槽要铺满整个窗口的宽度才对得上「左右」。
const SIDE_MARGIN: f32 = 16.0;

/// 顶部留白（像素，未乘倍率）：窗口最上头是菜单和工具栏，塔不能怼上去。
const TOP_MARGIN: f32 = 24.0;

/// 底边离键盘上缘的缝 —— 和方块排用的是同一个数，两条特效默认才会叠在一起。
const KEY_GAP: f32 = 4.0;

/// 内置 + 铜块一族一共 20 件，所以「每槽乐器上限」拉到 20 就是不限。
pub const INSTRUMENT_CAP: u32 = 20;

/// 摞塔的排序方式（设置面板那条「摞塔排序」）。
///
/// 两种都不会改变**谁**进塔（上限挤掉的口径不变），只改变同一格里乐器的先后，也就是
/// 塔的形状怎么随时间变。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackOrder {
    /// 按乐器号升序（出厂）：塔形只跟「这一帧在场的是哪几件」有关，和播放先后无关。
    /// 代价是一枚号更小的乐器进场会把已在场的那几颗往上顶，它退场又掉下来。
    #[default]
    ByInstrument,
    /// 按进栈先后：本轮**第一次**发声早的排前面（底排最左），后进的只往塔顶摞，
    /// 不把已有的顶起来。一直发音的那枚因此钉在同一格 —— 前提是「本轮第一次」按曲子算，
    /// 不是按屏幕上这个窗口算，见 [`Runs`]。
    /// 治不了的是退场：下面那枚到寿命退出时，上面那几颗照样往下掉一格。
    ByArrival,
}

/// 设置面板里那条下拉的显示文字（两个选项各一句）。
fn order_label(order: StackOrder) -> &'static str {
    match order {
        StackOrder::ByInstrument => tr("By instrument id", "按乐器号"),
        StackOrder::ByArrival => tr("By arrival (stack)", "按进栈先后"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// 一件乐器最后一个音过了之后，格子还留几秒（按播放头算）。
    pub life_seconds: f32,
    /// 横向分几槽。只能是奇数（保证正中那一槽用金字塔形状），滑杆按 2 步进。
    pub slots: u32,
    /// 一槽最多摆几件乐器，超出的按「只留最近响过的」挤掉。
    pub cap: u32,
    /// 中间槽叠到第几件才开始缩进（每排两枚的细柱 → 金字塔的分界，见 [`center_shape`]）。
    pub indent_from: u32,
    /// 方块边长相对 [`tile_size`] 的倍率（还会被槽宽和画面高度夹一次）。
    pub tile_scale: f32,
    /// 整条塔往上抬多少像素，0 = 和方块排同一底线。
    pub lift_px: f32,
    /// 用方块贴图；关掉或素材没扒到时用稳定色块。
    pub use_tiles: bool,
    /// 塔里乐器的先后，见 [`StackOrder`]。
    pub order: StackOrder,
    /// 三颗打击乐（[`PINNED_PERCUSSION`]）是否钉在底排最左。
    ///
    /// 单独一条勾、不塞进 [`StackOrder`]：那是用户点名的口径，不该被一次换排序档悄悄改掉。
    pub pin_percussion: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            life_seconds: 1.6,
            slots: 5,
            cap: 12,
            indent_from: INDENT_DEFAULT,
            tile_scale: 1.0,
            lift_px: 0.0,
            use_tiles: true,
            order: StackOrder::ByInstrument,
            pin_percussion: true,
        }
    }
}

/// 上一轮算好的「整曲依据」：键没变就直接复用，不逐帧重扫全曲。
#[derive(Clone, Debug)]
struct Region {
    /// 算的时候的依据：(`FxTimeline::signature`, 槽数, 每槽上限, 停留秒数 × 1000)。
    /// 两张表都按秒算，所以速度必须在键里 —— 它藏在 `signature` 喂的那几笔 tempo/变速点里。
    key: (u64, usize, u32, u32),
    peaks: Vec<usize>,
    /// 连续发声段的起点表，见 [`Runs`]：`ByArrival` 的锚点从这儿查，不是从屏幕窗口查。
    runs: Runs,
}

/// 声道方块。
#[derive(Default)]
pub struct PanStack {
    params: Params,
    /// 上一帧有没有因为缺素材退成色块。
    tiles_missing: bool,
    /// 上一帧有没有乐器在导出表里还是默认石头。
    unconfigured: bool,
    /// 上一帧被「每槽上限」挤掉的乐器数。
    over_cap: usize,
    /// 上一帧因为画面不够高被裁掉的顶排格数。
    trimmed: usize,
    /// 整曲区域缓存（见 [`Region`]）：换曲子、改槽数/上限/停留秒数才重算一次。
    region: Option<Region>,
}

/// 槽数取成合法值：3..=9 里的奇数（用户能滑出来的都合法，这步是给首选项里
/// 存了个偶数或越界值兜底的）。
pub fn slots_used(raw: u32) -> usize {
    (raw.clamp(3, 9) | 1) as usize
}

/// 声像 0..=200 → 槽号 0..`n`-1（0 = 最左）。等宽分箱：`n` = 5 时正中那槽盖住 80..=119，
/// 也就是「正中间 ±20 都算中央」，耳朵分不出那 20 档的差别。
pub fn slot_of(pan: u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    ((pan.min(PAN_MAX as u8) as usize * n) / PAN_MAX as usize).min(n - 1)
}

/// 这一槽是不是正中那槽（只有奇数槽数才有正中间，[`slots_used`] 保证这点）。
pub fn is_center(slot: usize, n: usize) -> bool {
    n % 2 == 1 && slot == n / 2
}

/// 中间槽的形状：`count` 件乐器 → 每排几枚（自底向上）。
///
/// `indent`（设置里的「缩进起始」）之前一层两个，落单的摞顶上（默认 5 ⇒ 3 = 2+1、
/// 4 = 2+2、5 = 2+2+1，就是用户一口一口对出来的那几张）。
/// `count > indent` 起换金字塔：先找最小的 `b` 让 `b` 阶满金字塔 `T(b) = b(b+1)/2 ≥ count`；
/// 正好装满就用满金字塔（6 = 3+2+1、10 = 4+3+2+1、15 = 5+4+3+2+1），
/// 装不满就退回 `b-1` 阶那座，从最底下往上给每排补一枚（7 = 4+2+1、20 = 6+5+4+3+2）。
/// 这样每往上一排就少一两枚，塔尖始终比塔基窄，看着才是「叠」而不是「一堵墙」。
///
/// `indent` 调大 = 前段一直用「每排两枚的细柱」，塔往上长得更多才开始横向铺开；
/// 调小（1）= 基本就是金字塔。
pub fn center_shape(count: usize, indent: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    let indent = indent.max(1);
    if count <= indent {
        // 底排先装满 2 枚，剩下的往上摞，最后一排可能只剩 1 枚。
        return (0..count.div_ceil(2)).map(|r| (count - r * 2).min(2)).collect();
    }
    let mut b = 1usize;
    while b * (b + 1) / 2 < count {
        b += 1;
    }
    if b * (b + 1) / 2 == count {
        return (0..b).map(|r| b - r).collect();
    }
    let mut rows: Vec<usize> = (0..b - 1).map(|r| (b - 1) - r).collect();
    let extra = count - (b - 1) * b / 2;
    for r in rows.iter_mut().take(extra) {
        *r += 1;
    }
    rows
}

/// 某一槽的形状：正中那槽用 [`center_shape`]，两侧每排一枚（竖着一列）。
pub fn shape_of(slot: usize, n_slots: usize, count: usize, indent: usize) -> Vec<usize> {
    match count {
        0 => Vec::new(),
        n if is_center(slot, n_slots) => center_shape(n, indent),
        n => vec![1; n],
    }
}

/// 这一槽**最多**占几列宽（摆过的形状里最宽那排）。没格子的槽按 1 列算，
/// 位置还是给它留着，否则槽序会和声道对不上。
pub fn cols_of(shape: &[usize]) -> usize {
    shape.iter().copied().max().unwrap_or(1).max(1)
}

/// 这一槽最多占几排高（同上，空格按 1 排）。
pub fn rows_of(shape: &[usize]) -> usize {
    shape.len().max(1)
}

/// 一格里摆什么：乐器号 + 该乐器在这一槽里**最后一次**发声是那枚音符（下标）。
/// 下标留给调用方查时刻，淡出按最后一次发声算。
pub type Cell = (u8, usize);

/// 钉在最前的三件打击乐：`nbs_core::instruments::INSTRUMENTS` 里的 Bass Drum(2)、
/// Snare Drum(3)、Click(4)。名单顺序就是用户点名的那一条，底排从最左往右依次摆。
pub const PINNED_PERCUSSION: [u8; 3] = [2, 3, 4];

/// 摆位排序键：[`PINNED_PERCUSSION`] 里的三颗排到最前面（彼此按名单顺序），其余乐器
/// 仍按乐器号升序跟在后面。只影响**摆的位置**，不影响谁被上限挤掉。
pub fn cell_order(instrument: u8) -> (u8, u8) {
    match PINNED_PERCUSSION.iter().position(|id| *id == instrument) {
        Some(rank) => (0, rank as u8),
        None => (1, instrument),
    }
}

/// 一格的摆位键（元组小的排前面 = 更靠底排最左）。
///
/// 头一档是「打击乐钉最前」那条勾：勾上时 [`PINNED_PERCUSSION`] 三颗拿 0、其余 1，
/// 不勾大家一律 1，三颗就回去跟着排序档排。后两档由 [`StackOrder`] 给：按乐器号看号，
/// 按进栈先后看 `anchor`。钉住的那三颗在两种档下都按名单顺序，因为用户点名的就是那个先后。
/// 最后一档是乐器号，给「同一列开跑的两段」一个固定先后（否则这两颗谁前谁后取决于排序算法）。
pub fn cell_key(
    order: StackOrder,
    pin: bool,
    instrument: u8,
    anchor: usize,
) -> (u8, usize, usize, usize) {
    let (head, rank) = cell_order(instrument);
    let head = if pin { head } else { 1 };
    match order {
        StackOrder::ByInstrument => (
            head,
            if head == 0 {
                rank as usize
            } else {
                instrument as usize
            },
            0,
            instrument as usize,
        ),
        StackOrder::ByArrival => (
            head,
            if head == 0 { rank as usize } else { 0 },
            anchor,
            instrument as usize,
        ),
    }
}

/// 「连续发声段」的起点表：`[槽][乐器] -> 升序的起点列号`。由 [`run_starts`] 烘出来，
/// 只给 [`StackOrder::ByArrival`] 用（按乐器号那档不查它）。
///
/// 空表（`Runs::default()`）= 没烘过：锚点退回音符自己那一列，等价于「窗口里最老的那枚」。
#[derive(Clone, Debug, Default)]
pub struct Runs(Vec<HashMap<u8, Vec<u32>>>);

/// 烘一张起点表：`[槽][乐器] -> 升序起点列号`。见 [`Runs`]。
pub fn run_starts(
    notes: &[FxNote],
    n_slots: usize,
    life: f64,
    seconds_at: impl Fn(u32) -> f64,
) -> Runs {
    if n_slots == 0 {
        return Runs::default();
    }
    let mut per_slot: Vec<HashMap<u8, (u32, Vec<u32>)>> = vec![HashMap::new(); n_slots];
    for note in notes {
        let slot = slot_of(note.audible_pan(), n_slots);
        let secs = seconds_at(note.tick);
        let entry = per_slot[slot]
            .entry(note.instrument)
            .or_insert((note.tick, Vec::new()));
        // 第一枚开一段；之后只要和上一枚的**时间**间隔超过停留，就是断了重开一段。
        if entry.1.is_empty() || secs - seconds_at(entry.0) > life {
            entry.1.push(note.tick);
        }
        entry.0 = note.tick;
    }
    Runs(
        per_slot
            .into_iter()
            .map(|map| map.into_iter().map(|(i, (_, starts))| (i, starts)).collect())
            .collect(),
    )
}

impl Runs {
    /// 这一枚所属那一段的起点列号。表里没有这件乐器（槽数变了、没烘过）就退回它自己那一列。
    fn start_of(&self, slot: usize, instrument: u8, tick: u32) -> u32 {
        let Some(starts) = self.0.get(slot).and_then(|map| map.get(&instrument)) else {
            return tick;
        };
        match starts.binary_search(&tick) {
            Ok(i) => starts[i],
            // 比表里第一个起点还早 = 这张表对不上（换曲没重烘），宁可退回本枚。
            Err(0) => tick,
            Err(i) => starts[i - 1],
        }
    }
}

/// 把还在响的音符归拢成「每槽哪几件乐器」。
///
/// 一件乐器在一槽里只留一格（这是和方块排最根本的差别），但**三个数都要记**：
/// 最后一次发声给淡出和上限用，本轮第一次发声和它所属**连续段的起点**给排序用。
/// 起点必须查 [`Runs`]，不能拿「窗口里最老的那枚」当锚点 —— 窗口左缘每帧都在往前滑，
/// 密集连发时那个锚点会一直往后挪，于是这枚会越过旁边只响过一次、还没出窗的乐器，
/// 自己爬上去（`a_live_run_does_not_slide_when_the_window_moves` 量的就是这件事）。
/// 查了起点表之后，只要间隔不超过停留，锚点在整段里钉死。
///
/// 谁被上限挤掉与排序档无关（都是「最久没响的那几件」出局）。
pub fn slot_cells(
    notes: &[&FxNote],
    n_slots: usize,
    cap: usize,
    order: StackOrder,
    pin: bool,
    runs: &Runs,
) -> (Vec<Vec<Cell>>, usize) {
    let mut per_slot: Vec<Vec<(usize, u8)>> = vec![Vec::new(); n_slots];
    for (seq, note) in notes.iter().enumerate() {
        per_slot[slot_of(note.audible_pan(), n_slots)].push((seq, note.instrument));
    }
    let cap = cap.max(1);
    let mut dropped = 0usize;
    let out: Vec<Vec<Cell>> = per_slot
        .into_iter()
        .enumerate()
        .map(|(slot, here)| {
            // (本轮第一次发声, 最后一次发声, 乐器)：已在场就只更新后者。
            let mut spans: Vec<(usize, usize, u8)> = Vec::new();
            for (seq, instrument) in here {
                match spans.iter_mut().find(|(_, _, i)| *i == instrument) {
                    Some(old) => old.1 = seq,
                    None => spans.push((seq, seq, instrument)),
                }
            }
            // 上限挤掉的是「最久没响的那几件」，所以先按最后一次发声从新到旧截断。
            spans.sort_by_key(|(_, last, _)| std::cmp::Reverse(*last));
            if spans.len() > cap {
                dropped += spans.len() - cap;
                spans.truncate(cap);
            }
            spans.sort_by_key(|(first, _, instrument)| {
                // 拿这一格里最老那枚去查起点就够了：断一段要的间隔比停留还宽，而窗口就是
                // 一个停留那么长，两段同框只剩 `tick_at_seconds` 落到整列多出来的那一列——
                // 真压上了，锚到前一段的起点也是个确定的数，不会每帧翻。
                let anchor = runs.start_of(slot, *instrument, notes[*first].tick);
                cell_key(order, pin, *instrument, anchor as usize)
            });
            spans.into_iter().map(|(_, last, i)| (i, last)).collect()
        })
        .collect();
    (out, dropped)
}

/// 整曲里每一槽**同时**最多共存几件乐器：摆位用的区域按这个数预留，不再逐帧浮动。
///
/// 数的是「格子」不是音符 —— 一件乐器一槽里只占一格，所以 `(T - life, T]` 里同一件
/// 乐器来多少个音都只算一件（和 [`slot_cells`] 同一个口径）。直接数音符的话，一首
/// 3 万音的曲子会给中央槽留出几万列。
///
/// 峰值一定出现在某枚音符刚响完的那一刻（那一刻之后窗口里少一枚、不会多），所以按
/// 时刻从左到右滑一次窗就够，不用逐列试。`seconds_at` 由调用方给（变速点前后每 tick
/// 的秒数不一样）。结果按 `cap` 夹住：超上限的那几件本来就不画，不该占地方。
pub fn slot_peaks(
    notes: &[FxNote],
    n_slots: usize,
    cap: usize,
    life: f64,
    seconds_at: impl Fn(u32) -> f64,
) -> Vec<usize> {
    if n_slots == 0 {
        return Vec::new();
    }
    // 每槽一条按秒排好的 (时刻, 乐器) 序列。
    let mut per_slot: Vec<Vec<(f64, u8)>> = vec![Vec::new(); n_slots];
    for note in notes {
        per_slot[slot_of(note.audible_pan(), n_slots)].push((seconds_at(note.tick), note.instrument));
    }
    let cap = cap.max(1);
    per_slot
        .into_iter()
        .map(|mut list| {
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut seen = vec![0u32; u8::MAX as usize + 1];
            let mut distinct = 0usize;
            let mut left = 0usize;
            let mut peak = 0usize;
            for (right, &(t, instrument)) in list.iter().enumerate() {
                // 出窗：比 `t - life` 更早的（含正好压线的）从队头退掉。
                while left < right && t - list[left].0 >= life {
                    let k = list[left].1 as usize;
                    seen[k] -= 1;
                    if seen[k] == 0 {
                        distinct -= 1;
                    }
                    left += 1;
                }
                let k = instrument as usize;
                if seen[k] == 0 {
                    distinct += 1;
                }
                seen[k] += 1;
                peak = peak.max(distinct);
            }
            peak.min(cap)
        })
        .collect()
}

/// 一帧摆出来的格子。
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub instrument: u8,
    /// 进 [`slot_cells`] 时的那枚音符下标（查时刻用）。
    pub seq: usize,
    pub rect: Rect,
}

/// 一次摆放的结果。
pub struct Plan {
    /// 全局边长：整条塔只用一个尺寸，不然一排里两块大两块小更难看。
    pub tile: f32,
    pub cells: Vec<Placed>,
    /// 画面高度不够，从顶上裁掉的格数。
    pub trimmed: usize,
}

/// `height` 里放得下几排：第 `r` 排的顶边在 `bottom - tile * (1.0 + r * ROW_STEP)`。
pub fn rows_that_fit(height: f32, tile: f32) -> usize {
    if tile <= f32::EPSILON || height < tile {
        0
    } else {
        let fits = ((height / tile) - 1.0) / ROW_STEP;
        (fits.floor() as usize + 1).max(1)
    }
}

/// 把每槽的形状摆成画面坐标。`indent` = 中间槽的「缩进起始」（见 [`center_shape`]）。
///
/// **横向按区域分配**：一槽占的宽度是它最宽那排需要的宽度，槽与槽之间只留
/// [`SLOT_GAP`]..=[`SPREAD_MAX`] 的缝，整组再以窗口中线为中心摆 —— 侧槽每排一枚，
/// 所以它们会贴到中央塔旁边，不再各占一条等宽键道。
///
/// `envelope` = 每槽的区域依据（整曲峰值件数，见 [`slot_peaks`]）：**位置按它算，
/// 于是侧槽的横坐标在整首曲子里钉死**，不会跟着塔一帧宽一帧窄左右晃。传空表就是
/// 「按当前内容」＝逐帧浮动（单测拿它对照两种摆法）。某一帧真比峰值还密时取
/// `max(现场, 区域)`：宁可当场把区域撑开一点，也不许两坨方块摞到一起。
///
/// 边长取「用户给的基准 / 整组内容宽算出来的 / 可用高度算出来的」里最小的，但不低于
/// [`MIN_TILE`]；夹到下限还是放不下，就从顶上往下裁整排（[`Plan::trimmed`] 记格数），
/// 宁可少摆几件也不让塔长出画面。
pub fn arrange(
    cells: &[Vec<Cell>],
    envelope: &[usize],
    area: Rect,
    bottom: f32,
    scale: f32,
    base_tile: f32,
    indent: usize,
) -> Plan {
    let n_slots = cells.len().max(1);
    let shapes: Vec<Vec<usize>> = cells
        .iter()
        .enumerate()
        .map(|(slot, group)| shape_of(slot, n_slots, group.len(), indent))
        .collect();
    // 区域形状：整曲峰值算出来的那张表，逐槽和现场形状取大（列数、排数各取大）。
    let regions: Vec<Vec<usize>> = (0..n_slots)
        .map(|slot| {
            let count = envelope.get(slot).copied().unwrap_or_else(|| cells[slot].len());
            shape_of(slot, n_slots, count, indent)
        })
        .collect();
    let cols: Vec<usize> = (0..n_slots)
        .map(|slot| cols_of(&shapes[slot]).max(cols_of(&regions[slot])))
        .collect();
    let max_rows = (0..n_slots)
        .map(|slot| rows_of(&shapes[slot]).max(rows_of(&regions[slot])))
        .max()
        .unwrap_or(1)
        .max(1);
    // `col_span(c)` 排宽：c 枚方块按 [`COL_STEP`] 的步距摆，第一枚另算一个边长。整组的内容宽
    // 就是每槽最宽那排的排宽之和（槽与槽的缝另外算，见下面的 `gap`）。
    let col_span = |c: usize| COL_STEP * c as f32 - (COL_STEP - 1.0);
    let content: f32 = cols
        .iter()
        .map(|c| col_span(*c))
        .sum::<f32>()
        .max(col_span(1));
    let inner = (area.width() - 2.0 * SIDE_MARGIN * scale).max(MIN_TILE * scale);
    let seams = n_slots as f32 - 1.0;

    // 先按基准边长看余量够不够分缝：够就放宽到 SPREAD_MAX 为止，不够就退到 SLOT_GAP，
    // 再由 by_width 把边长夹小（侧槽贴过来省下的宽度优先留给塔，不是留给空白）。
    let gap = if seams > 0.0 {
        ((inner - base_tile * content) / seams).clamp(SLOT_GAP * scale, SPREAD_MAX * scale)
    } else {
        0.0
    };
    let by_width = (inner - gap * seams).max(MIN_TILE * scale) / content;
    // 竖向同理：`row_span(r)` 是 r 排摞起来的总高（乘边长），排距小 ⇒ 同样高的画面
    // 能容下更多排，也允许更大的边长。
    let row_span = |r: usize| ROW_STEP * r as f32 - (ROW_STEP - 1.0);
    let rows_span = row_span(max_rows);
    let top = area.min.y + TOP_MARGIN * scale;
    // 底线只管「自动收缩」那一层：用户自己把「方块大小」调到比底线还小时听他的
    // （否则滑杆下半段成了死的），那种情况下画面放不下同样靠裁顶排解决。
    let floor = (MIN_TILE * scale).min(base_tile);
    let height = (bottom - top).max(floor);
    let tile = base_tile
        .min(by_width)
        .min(height / rows_span)
        .max(floor);

    // 每槽的中心：从整组左缘开始按各自列数累加（中央塔那槽列数最多，正好压在中线上）。
    let group_w = tile * content + gap * seams;
    let mut cursor = area.center().x - group_w * 0.5;
    let mut centers: Vec<f32> = Vec::with_capacity(n_slots);
    for c in &cols {
        let band = tile * col_span(*c);
        centers.push(cursor + band * 0.5);
        cursor += band + gap;
    }

    let allowed = rows_that_fit(height, tile);
    let mut cells_out: Vec<Placed> = Vec::new();
    let mut trimmed = 0usize;
    for (slot, group) in cells.iter().enumerate() {
        let center_x = centers[slot];
        let mut taken = 0usize;
        for (row, cols) in shapes[slot].iter().enumerate() {
            if row >= allowed {
                trimmed += group.len() - taken;
                break;
            }
            // 底排先吃满，再往上摞：打击乐三件套在底排最左，其余按乐器号升序跟在后面。
            // 排是从下往上推的，所以后面（上面）那排会盖住下面那排的顶面 —— 等距图
            // 摞柱子要的就是这个层次。
            let y = bottom - tile * (0.5 + row as f32 * ROW_STEP);
            for col in 0..*cols {
                let Some(&(instrument, seq)) = group.get(taken + col) else {
                    break;
                };
                let x = center_x + (col as f32 - (*cols as f32 - 1.0) / 2.0) * COL_STEP * tile;
                cells_out.push(Placed {
                    instrument,
                    seq,
                    rect: Rect::from_center_size(Pos2::new(x, y), Vec2::new(tile, tile)),
                });
            }
            taken += cols;
        }
    }
    Plan {
        tile,
        cells: cells_out,
        trimmed,
    }
}

impl PanStack {
    pub const ID: &'static str = "pan_slots";

    /// 摞塔用的是 Wiki 那批等距立方体：只有它有「上面盖住下面一部分」的层次
    /// （排距 [`ROW_STEP`] 就是照它量的）。方块排/瀑布反过来用方块面，见 [`BlockRow::TILE_LOOK`]。
    pub const TILE_LOOK: TileLook = TileLook::Iso;

    /// 塔的底线：贴 [`BlockRow`] 那条基准线（键盘上缘往上一点），`lift_px` 再往上抬。
    /// 默认偏移 0 ⇒ 和方块排的方块重合，两条一起开也只看得到一排。
    pub fn bottom_line(cx: &FxContext, lift_px: f32) -> f32 {
        let scale = cx.scale.max(f32::EPSILON);
        BlockRow::layout(cx).origin.y - KEY_GAP * scale - lift_px * scale
    }
}

impl PlaybackEffect for PanStack {
    fn info(&self) -> EffectInfo {
        EffectInfo {
            id: Self::ID,
            name: tr("Pan instrument blocks", "声道方块"),
            surface: Surface::Overlay,
            // 比方块排晚一帧画：两条都开时（默认位置重合）声道方块压在上面，
            // 它才是「看现在响哪几件乐器」的那一条。
            z: 20,
            // 自己按键盘那条线摆，但不逼着钢琴栏显示：关掉键盘也照画。
            wants_piano: false,
        }
    }

    fn params(&self) -> serde_json::Value {
        serde_json::to_value(self.params).unwrap_or_default()
    }

    fn set_params(&mut self, value: &serde_json::Value) {
        if let Ok(parsed) = serde_json::from_value::<Params>(value.clone()) {
            self.params = parsed;
        }
    }

    fn settings_ui(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let p = &mut self.params;
        changed |= ui
            .add(
                egui::Slider::new(&mut p.life_seconds, 0.3..=6.0)
                    .suffix(" s")
                    .text(tr("Block lifetime", "方块停留")),
            )
            .changed();
        let mut slots = i64::from(p.slots);
        changed |= ui
            .add(
                egui::Slider::new(&mut slots, 3..=9)
                    .step_by(2.0)
                    .text(tr("Pan slots", "声道槽数")),
            )
            .changed();
        p.slots = slots.clamp(3, 9) as u32;
        let mut cap = i64::from(p.cap);
        changed |= ui
            .add(
                egui::Slider::new(&mut cap, 1..=INSTRUMENT_CAP as i64)
                    .text(tr("Instruments per slot", "每槽乐器上限")),
            )
            .changed();
        p.cap = cap.clamp(1, INSTRUMENT_CAP as i64) as u32;
        changed |= {
            let before = p.order;
            egui::ComboBox::from_id_salt("pan_stack_order")
                .selected_text(order_label(p.order))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut p.order,
                        StackOrder::ByInstrument,
                        order_label(StackOrder::ByInstrument),
                    );
                    ui.selectable_value(
                        &mut p.order,
                        StackOrder::ByArrival,
                        order_label(StackOrder::ByArrival),
                    );
                });
            p.order != before
        };
        changed |= ui
            .checkbox(
                &mut p.pin_percussion,
                tr("Pin the drum trio to the front", "打击乐钉最前"),
            )
            .changed();
        let mut indent = i64::from(p.indent_from);
        changed |= ui
            .add(
                egui::Slider::new(&mut indent, INDENT_MIN as i64..=INDENT_MAX as i64)
                    .text(tr("Indent the center tower from", "缩进起始")),
            )
            .changed();
        p.indent_from = indent.clamp(INDENT_MIN as i64, INDENT_MAX as i64) as u32;
        changed |= ui
            .add(egui::Slider::new(&mut p.tile_scale, 0.5..=2.5).text(tr("Block size", "方块大小")))
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut p.lift_px, -100.0..=600.0)
                    .suffix(" px")
                    .text(tr("Lift", "抬高")),
            )
            .changed();
        changed |= ui
            .checkbox(&mut p.use_tiles, tr("Block textures", "方块贴图"))
            .changed();
        changed
    }

    fn note(&self) -> Option<String> {
        if self.trimmed > 0 {
            return Some(
                tr(
                    "Not tall enough, top rows of instrument blocks were cut.",
                    "画面高度不够，顶上几排声道方块没摆出来。",
                )
                .to_string(),
            );
        }
        if self.over_cap > 0 {
            return Some(
                tr(
                    "Too many instruments in one slot, only the newest are drawn.",
                    "同一个声道槽里的乐器超过上限，只画了最近响过的几件。",
                )
                .to_string(),
            );
        }
        if self.tiles_missing {
            return Some(
                tr(
                    "Block textures not found, using flat colors (run tools/fetch_block_textures.py).",
                    "没找到方块贴图，改用色块代替（跑一次 tools/fetch_block_textures.py 再重启）。",
                )
                .to_string(),
            );
        }
        if self.unconfigured {
            return Some(
                tr(
                    "Some instruments have no block assigned in the schematic export, so they are shown in their own color.",
                    "有些乐器在导出表里还是默认的石头，这一帧改用各自的乐器色（导出设置 → 乐器方块 可以指定）。",
                )
                .to_string(),
            );
        }
        None
    }

    fn draw(&mut self, cx: &FxContext, out: &mut Frame) {
        let scale = cx.scale.max(f32::EPSILON);
        let life = self.params.life_seconds.max(0.1);
        let n_slots = slots_used(self.params.slots);
        let now_seconds = cx.timeline.seconds_at(cx.position) as f64;
        let from = cx
            .timeline
            .tick_at_seconds(now_seconds - life as f64);
        let mut live = cx.timeline.notes_in(from, cx.position + 0.001);
        if live.len() > MAX_BLOCKS {
            // 和方块排同一个截法：只留最新的一批，密集段落不该把画面拖垮。
            live = &live[live.len() - MAX_BLOCKS..];
        }
        let refs: Vec<&FxNote> = live.iter().collect();
        // 区域和「连续发声段起点」都按整曲算一次（键没变就复用），于是侧槽的横位置、
        // 以及每格在塔里的先后，都在整首曲子里钉死，不跟着播放头的窗口滑。
        // 扫的是全表：3 万音的曲子也就几毫秒，但只在换曲/改参数那一帧发生。
        let key = (
            cx.timeline.signature(),
            n_slots,
            self.params.cap,
            (f64::from(life) * 1000.0) as u32,
        );
        if self.region.as_ref().map(|region| region.key) != Some(key) {
            let timeline = cx.timeline;
            let seconds_at = |tick: u32| f64::from(timeline.seconds_at(f64::from(tick)));
            let peaks = slot_peaks(
                timeline.notes(),
                n_slots,
                self.params.cap as usize,
                life as f64,
                seconds_at,
            );
            let runs = run_starts(timeline.notes(), n_slots, life as f64, seconds_at);
            self.region = Some(Region { key, peaks, runs });
        }
        let no_runs = Runs::default();
        let (envelope, runs) = match &self.region {
            Some(region) => (&region.peaks[..], &region.runs),
            None => (&[][..], &no_runs),
        };
        let (groups, over_cap) = slot_cells(
            &refs,
            n_slots,
            self.params.cap as usize,
            self.params.order,
            self.params.pin_percussion,
            runs,
        );
        let bottom = Self::bottom_line(cx, self.params.lift_px);
        let base_tile = tile_size(scale) * self.params.tile_scale.max(0.2);
        let plan = arrange(
            &groups,
            envelope,
            cx.area,
            bottom,
            scale,
            base_tile,
            self.params.indent_from.max(INDENT_MIN) as usize,
        );

        let mut missing = false;
        let mut unconfigured = false;
        for placed in &plan.cells {
            let Some(note) = refs.get(placed.seq) else {
                continue;
            };
            let age = (now_seconds - cx.timeline.seconds_at(f64::from(note.tick)) as f64) as f32;
            let alpha = BlockRow::envelope(age, life).1;
            // 和方块排、瀑布的「乐器方块」样式共用同一条查表，同一件乐器三处同图同色。
            let blk = note_block(
                cx,
                placed.instrument,
                self.params.use_tiles,
                Self::TILE_LOOK,
            );
            unconfigured |= blk.unconfigured;
            match &blk.texture {
                Some(tex) => out.image(
                    placed.rect,
                    Arc::clone(tex),
                    Color32::from_white_alpha((255.0 * alpha) as u8),
                ),
                None => {
                    missing |= blk.missing;
                    out.rect(placed.rect, fade(blk.color, alpha), 2.0 * scale);
                    out.outlined(
                        placed.rect,
                        Color32::TRANSPARENT,
                        (1.0 * scale, fade(Color32::from_gray(20), alpha)),
                        2.0 * scale,
                    );
                }
            }
        }
        self.tiles_missing = missing;
        self.unconfigured = unconfigured;
        self.over_cap = over_cap;
        self.trimmed = plan.trimmed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::keyboard::{KeyboardLayout, WHITE_HIT_H, KEY_W};
    use crate::fx::stage::Primitive;
    use crate::fx::tiles::{BlockTiles, InstrumentBlocks, NotePalette};
    use crate::fx::timeline::FxTimeline;
    use egui::Context;
    use nbs_core::model::{Layer, Note, Song};

    /// `(tick, layer, instrument, key, note_panning, layer_stereo)`。
    fn song(notes: &[(u32, u32, u8, u8, u8, u8)]) -> Song {
        let mut s = Song::new();
        s.meta.tempo = 10.0;
        for (tick, layer, instrument, key, panning, stereo) in notes {
            while s.layers.len() <= *layer as usize {
                s.layers.push(Layer::default());
            }
            // 声道挂在轨上，所以同一层的多枚音符得写同一个值。
            s.layers[*layer as usize].stereo = *stereo;
            let mut n = Note::new(*instrument, *key);
            n.panning = *panning;
            s.set(*tick, *layer, n);
        }
        s.recalculate();
        s
    }

    struct Rig {
        ctx: Context,
        timeline: FxTimeline,
        blocks: InstrumentBlocks,
        tiles: BlockTiles,
        palette: NotePalette,
        layout: Option<KeyboardLayout>,
    }

    impl Rig {
        fn new(notes: &[(u32, u32, u8, u8, u8, u8)]) -> Self {
            Self {
                ctx: Context::default(),
                timeline: FxTimeline::bake(&song(notes), false),
                blocks: InstrumentBlocks::default(),
                tiles: BlockTiles::open_in(None),
                palette: NotePalette::default(),
                layout: None,
            }
        }

        fn with_keyboard(mut self) -> Self {
            self.layout = Some(KeyboardLayout::fit(
                Rect::from_min_size(Pos2::new(20.0, 600.0), Vec2::new(52.0 * KEY_W, WHITE_HIT_H)),
                1.0,
                0,
            ));
            self
        }

        fn cx(&self, position: f64, now: f64) -> FxContext<'_> {
            FxContext {
                tex: &self.ctx,
                now,
                dt: 0.05,
                position,
                playing: true,
                speed: 1.0,
                scale: 1.0,
                accent: Color32::from_rgb(80, 160, 255),
                backdrop: crate::fx::STAGE_BACKDROP,
                transparent: false,
                area: Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0)),
                keyboard: self.layout,
                timeline: &self.timeline,
                blocks: &self.blocks,
                tiles: Some(&self.tiles),
                palette: &self.palette,
            }
        }
    }

    fn rects(frame: &Frame) -> Vec<Rect> {
        frame
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect {
                    rect,
                    stroke: None,
                    ..
                } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// 中间槽那张表逐个钉住：这是用户一口一口对出来的形状，改规则必须显式改测试。
    #[test]
    fn the_center_shape_table_is_pinned() {
        let want: &[&[usize]] = &[
            &[1],
            &[2],
            &[2, 1],
            &[2, 2],
            &[2, 2, 1],
            &[3, 2, 1],
            &[4, 2, 1],
            &[4, 3, 1],
            &[4, 3, 2],
            &[4, 3, 2, 1],
            &[5, 3, 2, 1],
            &[5, 4, 2, 1],
            &[5, 4, 3, 1],
            &[5, 4, 3, 2],
            &[5, 4, 3, 2, 1],
            &[6, 4, 3, 2, 1],
            &[6, 5, 3, 2, 1],
            &[6, 5, 4, 2, 1],
            &[6, 5, 4, 3, 1],
            &[6, 5, 4, 3, 2],
        ];
        for (i, row) in want.iter().enumerate() {
            let n = i + 1;
            assert_eq!(center_shape(n, INDENT_DEFAULT as usize), *row, "{n} 件乐器的形状");
            assert_eq!(row.iter().sum::<usize>(), n, "{n} 件得全摆出来");
            // 塔基最宽、往上只减不增，否则看着不像叠。
            assert!(
                row.windows(2).all(|w| w[0] >= w[1]),
                "{n} 件的形状往上变宽了：{row:?}"
            );
        }
        assert_eq!(center_shape(0, INDENT_DEFAULT as usize), Vec::<usize>::new());
    }

    /// 「缩进起始」这条滑杆只挪动「每排两枚的细柱 → 金字塔」的分界，别的规则不动。
    #[test]
    fn the_indent_setting_moves_only_the_pyramid_boundary() {
        assert_eq!(INDENT_DEFAULT, 5, "出厂值必须还是原来那个 5（默认值是现在的值）");
        assert_eq!(Params::default().indent_from, INDENT_DEFAULT);
        // 分界以下一直是每排两枚：indent=9 时 6..=9 件还是细柱，10 件起才换金字塔。
        assert_eq!(center_shape(6, 9), vec![2, 2, 2]);
        assert_eq!(center_shape(9, 9), vec![2, 2, 2, 2, 1]);
        assert_eq!(center_shape(10, 9), vec![4, 3, 2, 1], "T(4) = 10 正好装满");
        // indent=1：细柱那一段直接没了，5 件就是 3+2 而不是 2+2+1。
        assert_eq!(center_shape(3, 1), vec![2, 1], "T(2) = 3 正好装满");
        assert_eq!(center_shape(5, 1), vec![3, 2], "装不满就退回低一阶补一枚");
        // 和默认值比：同一件数，indent 越大塔越细越高、且始终不缩进。
        for n in 1..=INDENT_MAX as usize {
            let tight = center_shape(n, INDENT_MIN as usize);
            let loose = center_shape(n, INDENT_MAX as usize);
            assert!(loose.len() >= tight.len(), "{n} 件：细柱该比金字塔高");
            assert!(
                loose.iter().copied().max().unwrap_or(1) <= 2,
                "{n} 件在缩进起始 = {INDENT_MAX} 下不该缩进：{loose:?}"
            );
            assert_eq!(loose.iter().sum::<usize>(), n, "{n} 件得全摆出来（细柱）");
            assert_eq!(tight.iter().sum::<usize>(), n, "{n} 件得全摆出来（金字塔）");
        }
        assert_eq!(center_shape(4, 0), center_shape(4, 1), "0 当 1 用，不许除出空形状");
        assert_eq!(center_shape(0, 5), Vec::<usize>::new(), "0 件就是没形状");
    }

    #[test]
    fn side_slots_stack_one_per_row() {
        for n in 1..=20 {
            assert_eq!(shape_of(0, 5, n, 5), vec![1; n], "左侧槽 {n} 件该排成一列");
            assert_eq!(shape_of(4, 5, n, 5), vec![1; n], "右侧槽也一样");
        }
        // 只有正中那槽用金字塔表（3 槽时中央是第 1 槽，不是第 2）。
        assert_eq!(shape_of(2, 5, 6, 5), vec![3, 2, 1]);
        assert_eq!(shape_of(1, 3, 6, 5), vec![3, 2, 1]);
        assert_eq!(shape_of(2, 3, 6, 5), vec![1, 1, 1, 1, 1, 1]);
        assert_eq!(shape_of(0, 5, 0, 5), Vec::<usize>::new());
        // 侧槽归侧槽：缩进起始调多大都不影响它们（每排一枚是硬规定）。
        assert_eq!(shape_of(0, 5, 8, 20), vec![1; 8]);
    }

    /// 每槽占多宽、多高，由形状说了算（摆的时候按这个分配横向宽度，不再等分）。
    #[test]
    fn column_and_row_bounds_count_empty_slots_as_one() {
        assert_eq!(cols_of(&[]), 1);
        assert_eq!(rows_of(&[]), 1);
        assert_eq!(cols_of(&[1]), 1);
        assert_eq!(cols_of(&[3, 2, 1]), 3);
        assert_eq!(rows_of(&[3, 2, 1]), 3);
        assert_eq!(cols_of(&center_shape(20, 5)), 6, "20 件那塔底排 6 枚");
    }

    /// 分箱要按「听到哪只耳朵」，而不是裸的 `note.panning`：轨的声道会把音符往中间拉一半。
    #[test]
    fn bins_use_the_audible_pan() {
        // 5 槽：正中那槽盖住 80..=119（±20）。
        assert_eq!(slot_of(0, 5), 0);
        assert_eq!(slot_of(39, 5), 0);
        assert_eq!(slot_of(40, 5), 1);
        assert_eq!(slot_of(79, 5), 1);
        assert_eq!(slot_of(80, 5), 2);
        assert_eq!(slot_of(100, 5), 2);
        assert_eq!(slot_of(119, 5), 2);
        assert_eq!(slot_of(120, 5), 3);
        assert_eq!(slot_of(160, 5), 4);
        assert_eq!(slot_of(200, 5), 4, "最右那一档不能溢出到第 5 槽");
        // 奇数槽都有正中一槽：槽越多数目越窄，5 槽盖住 80..=119，3 槽盖住 67..=133。
        for n in [3u32, 5, 7, 9] {
            let n = n as usize;
            assert!(is_center(n / 2, n));
            let half = PAN_MAX as usize / n / 2;
            assert_eq!(
                slot_of((100 - half as u32) as u8, n),
                n / 2,
                "{n} 槽偏左 ±{half} 以内该算中央"
            );
            assert_eq!(
                slot_of((100 + half as u32 - 1) as u8, n),
                n / 2,
                "{n} 槽偏右那一档边界在下个槽，这里取闭着的那一侧"
            );
            assert_eq!(slot_of(0, n), 0);
            assert_eq!(slot_of(200, n), n - 1);
        }
        // 只有奇数槽数才有正中一槽；偶数（只可能从旧首选项里来）一律 false，
        // 免得两边都自称中央、摆出两座金字塔。
        assert!(!is_center(1, 2));
        assert!(!is_center(2, 4));
    }

    /// 轨声道不在正中时，音符的 pan 会被往中间拉一半：`(60 + 140) / 2 = 100` ⇒ 落中央槽。
    #[test]
    fn a_track_wide_of_center_pulls_the_note_in() {
        let rig = Rig::new(&[(0, 0, 0, 60, 140, 60), (0, 1, 7, 62, 140, 100)]);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        assert_eq!(live[0].audible_pan(), 100);
        assert_eq!(live[1].audible_pan(), 140);
        let (groups, dropped) =
            slot_cells(&live, 5, 12, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(dropped, 0);
        // 同样 pan 140 的两件乐器，轨的声道不同就分道扬镳。
        assert_eq!(groups[2].iter().map(|c| c.0).collect::<Vec<_>>(), vec![0]);
        assert_eq!(groups[3].iter().map(|c| c.0).collect::<Vec<_>>(), vec![7]);
    }

    /// 一个槽里同一件乐器来多少个音都只占一格 —— 这就是和方块排分开的那条。
    #[test]
    fn one_instrument_never_takes_two_cells_in_a_slot() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = (0..9u32)
            .map(|t| (t, 0, (t % 3) as u8, 50 + t as u8, 100, 100))
            .collect();
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, dropped) =
            slot_cells(&live, 5, 12, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(dropped, 0);
        assert_eq!(groups[2].len(), 3, "3 件乐器 9 个音 ⇒ 3 格");
        assert_eq!(
            groups[2].iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![2, 0, 1],
            "摆位不随发声先后抖：钉最前的打击乐在左，其余按乐器号升序"
        );
        // 淡出按最后一次发声算。
        assert_eq!(groups[2].iter().map(|c| c.1).collect::<Vec<_>>(), vec![8, 6, 7]);
    }

    /// 上限挤掉的是「最久没响的」，剩下的仍按 [`cell_key`]（出厂那档）摆。
    #[test]
    fn the_cap_drops_the_oldest_not_the_highest_id() {
        // 乐器 0/1/2/3 依次在第 0/1/2/3 列响。
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> =
            (0..4u32).map(|t| (t, 0, t as u8, 60, 100, 100)).collect();
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, dropped) =
            slot_cells(&live, 5, 2, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(dropped, 2);
        assert_eq!(
            groups[2].iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![2, 3],
            "留最近响过的两件（bass drum、snare drum），摆的时候按名单顺序 bass 在左"
        );
    }

    /// 打击乐三件套一出现就钉最前：底排从最左起 bass drum → snare drum → click，
    /// 其余乐器仍按乐器号升序跟在后面（用户点名的顺序，也不管谁先响）。
    #[test]
    fn the_drum_trio_is_pinned_to_the_front() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 9, 60, 100, 100), // 先响的两件反而排到后面去
            (1, 1, 3, 60, 100, 100), // snare drum
            (2, 2, 0, 60, 100, 100), // harp
            (3, 3, 4, 60, 100, 100), // click
            (4, 4, 2, 60, 100, 100), // bass drum
        ];
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, dropped) =
            slot_cells(&live, 5, 12, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(dropped, 0);
        assert_eq!(
            groups[2].iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![2, 3, 4, 0, 9],
            "2/3/4 钉最前，其余按乐器号"
        );
        // 5 件在默认「缩进起始 5」下是 [2,2,1]：底排两枚正好是名单的前两颗。
        let plan = arrange(
            &groups,
            &[],
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0)),
            700.0,
            1.0,
            48.0,
            INDENT_DEFAULT as usize,
        );
        let bottom = plan
            .cells
            .iter()
            .filter(|c| (c.rect.center().y - (700.0 - plan.tile * 0.5)).abs() < 0.01)
            .map(|c| c.instrument)
            .collect::<Vec<_>>();
        assert_eq!(bottom, vec![2, 3], "底排最左两枚 = bass drum、snare drum");
    }

    /// 「按进栈先后」那一档要治的就是这个：后进的只往塔顶摞，不把已经在场的那颗顶起来。
    /// 排序看的是本轮**第一次**发声 —— 一直发音的那枚重触发十次也还在同一格。
    #[test]
    fn the_stack_option_puts_newcomers_on_top_and_leaves_the_rest_alone() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 8, 60, 100, 100), // 一直在响的那件：0 列和 2 列各响一次
            (1, 1, 0, 61, 100, 100), // 后进的，乐器号比它小
            (2, 2, 8, 62, 100, 100),
        ];
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let ids = |groups: &[Vec<Cell>]| groups[2].iter().map(|c| c.0).collect::<Vec<_>>();
        let (by_id, _) = slot_cells(&live, 5, 12, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(
            ids(&by_id),
            vec![0, 8],
            "出厂那档按乐器号：后进的 0 号插到 8 前面，把它顶上去"
        );
        let (stacked, _) = slot_cells(&live, 5, 12, StackOrder::ByArrival, true, &Runs::default());
        assert_eq!(
            ids(&stacked),
            vec![8, 0],
            "栈档按先后：8 还占底排最左，0 摞在它后面（按最后一次发声排就会翻过来）"
        );
        // 淡出和上限仍然只看最后一次发声，两档一样。
        assert_eq!(
            stacked[2].iter().map(|c| c.1).collect::<Vec<_>>(),
            vec![2, 1],
            "格子记的还是最后一次发声的下标"
        );
    }

    /// 「打击乐钉最前」是单独一条勾，不跟着排序档走：勾上时三颗永远在底排最左（彼此按
    /// 用户点名的顺序），不勾才回去按选定的排序方式排。
    #[test]
    fn the_pin_switch_and_the_order_switch_are_orthogonal() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 2, 60, 100, 100), // bass drum 先响
            (1, 1, 0, 61, 100, 100), // harp
            (2, 2, 3, 62, 100, 100), // snare drum 最后响
        ];
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let ids = |groups: &[Vec<Cell>]| groups[2].iter().map(|c| c.0).collect::<Vec<_>>();
        let cases: [((StackOrder, bool), Vec<u8>); 4] = [
            ((StackOrder::ByInstrument, true), vec![2, 3, 0]),
            ((StackOrder::ByArrival, true), vec![2, 3, 0]),
            ((StackOrder::ByInstrument, false), vec![0, 2, 3]),
            ((StackOrder::ByArrival, false), vec![2, 0, 3]),
        ];
        for ((order, pin), want) in cases {
            let (groups, dropped) = slot_cells(&live, 5, 12, order, pin, &Runs::default());
            assert_eq!(dropped, 0, "上限没动过：{order:?} pin={pin}");
            assert_eq!(
                ids(&groups),
                want,
                "{order:?} pin={pin} 该摆成 {want:?}，实际 {:?}（谁被挤掉与档位无关）",
                ids(&groups)
            );
        }
    }

    /// 这一条才是用户点的那个问题：一直发音的那枚会不会被后进的「一会托举上去、一会又
    /// 掉下来」。同一首曲子取两帧 —— 后进的那枚还没来时大家都只有一格，它进来之后按乐器号
    /// 那档会把它插到前面（原来那枚被顶上去），按进栈先后那档它只能摞到后面。
    #[test]
    fn the_stack_option_keeps_a_live_block_in_its_cell() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 8, 60, 100, 100), // 8 号从第 0 列一直响到第 4 列
            (2, 0, 8, 61, 100, 100),
            (4, 0, 8, 62, 100, 100),
            (3, 1, 0, 63, 100, 100), // 0 号第 3 列才进来，乐器号比它小
            (5, 1, 0, 63, 100, 100),
        ];
        let rig = Rig::new(&notes);
        let front = |pos: f64, order: StackOrder| -> Vec<u8> {
            // 窗口从 0 列起（停留秒数盖住整首），只挪播放头。
            let refs: Vec<&FxNote> = rig.timeline.notes_in(0.0, pos + 0.001).iter().collect();
            let (groups, _) = slot_cells(&refs, 5, 12, order, true, &Runs::default());
            groups[2].iter().map(|c| c.0).collect()
        };
        assert_eq!(front(2.5, StackOrder::ByInstrument), vec![8]);
        assert_eq!(front(2.5, StackOrder::ByArrival), vec![8]);
        assert_eq!(
            front(4.5, StackOrder::ByInstrument),
            vec![0, 8],
            "出厂那档：0 号一进来就插到 8 前面，把它顶上去（它退场时再掉回来）"
        );
        assert_eq!(
            front(4.5, StackOrder::ByArrival),
            vec![8, 0],
            "栈档：8 号还占它原来那一格，新来的摞在后面"
        );
    }

    /// 段的断点用**时间**算，不看列数：10 t/s 的曲子里隔 10 列是 1 秒，1 t/s 的曲子里
    /// 隔 10 列是 10 秒 —— 后者早断开了。
    #[test]
    fn a_run_breaks_when_the_gap_exceeds_the_dwell() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 8, 60, 100, 100),
            (10, 0, 8, 60, 100, 100), // 隔 1 秒，还在同一段
            (40, 0, 8, 60, 100, 100), // 隔 3 秒 > 停留 1.6 秒 ⇒ 另起一段
            (12, 1, 7, 60, 0, 0),     // pan 0 ⇒ 最左那槽，和上面两件不同槽
        ];
        let rig = Rig::new(&notes);
        let runs = run_starts(rig.timeline.notes(), 5, 1.6, |tick| {
            f64::from(rig.timeline.seconds_at(f64::from(tick)))
        });
        assert_eq!(runs.start_of(2, 8, 0), 0);
        assert_eq!(runs.start_of(2, 8, 10), 0, "第二段还没开，仍属第 0 列那一段");
        assert_eq!(runs.start_of(2, 8, 40), 40, "断开之后重新起段");
        assert_eq!(runs.start_of(0, 7, 12), 12);
        // 查不到（乐器不在这槽、或整张表没烘过）就退回本枚那一列，也就是旧行为。
        assert_eq!(runs.start_of(0, 8, 40), 40);
        assert_eq!(Runs::default().start_of(2, 8, 40), 40);
    }

    /// 这一条就是上面那句「同一枚两次发音不超过停留时间，它在栈里的相对位置不该变」的
    /// 回归测试。锚点取「窗口里最老的那枚」时它守不住：左缘每帧往前滑，滑过 8 号第 0 列
    /// 那枚之后，窗口里最老的变成第 20 列，比旁边只响过一次（第 15 列）的 0 号还晚，
    /// 于是 8 号从底排爬到第二格 —— 曲子没变，只是播放头动了。
    /// 锚点换成 [`Runs`]（这段连续发声第一次发声的那一列，曲子的属性）之后就不动了。
    #[test]
    fn a_live_run_does_not_slide_when_the_window_moves() {
        // 出厂 tempo 10 ⇒ 一列 0.1 秒，停留 1.6 秒 = 16 列。
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 8, 60, 100, 100), // 8 号 0/10/20/30 列各一次，间隔 1 秒 < 停留 ⇒ 一整段
            (10, 0, 8, 60, 100, 100),
            (20, 0, 8, 60, 100, 100),
            (30, 0, 8, 60, 100, 100),
            (15, 1, 0, 63, 100, 100), // 0 号只响这一次
        ];
        let rig = Rig::new(&notes);
        let life = 1.6f64;
        let runs = run_starts(rig.timeline.notes(), 5, life, |tick| {
            f64::from(rig.timeline.seconds_at(f64::from(tick)))
        });
        assert_eq!(runs.start_of(2, 8, 20), 0, "第 20 列那枚仍属第 0 列起的这一段");
        let front = |pos: f64, table: &Runs| -> Vec<u8> {
            // 和 draw 一模一样的窗口：从「播放头往前推一个停留」那列起。
            let from = rig
                .timeline
                .tick_at_seconds(f64::from(rig.timeline.seconds_at(pos)) - life);
            let refs: Vec<&FxNote> = rig.timeline.notes_in(from, pos + 0.001).iter().collect();
            let (groups, _) = slot_cells(&refs, 5, 12, StackOrder::ByArrival, true, table);
            groups[2].iter().map(|c| c.0).collect()
        };
        assert_eq!(front(20.5, &Runs::default()), vec![8, 0]);
        assert_eq!(front(20.5, &runs), vec![8, 0]);
        assert_eq!(
            front(30.5, &Runs::default()),
            vec![0, 8],
            "窗口相对锚点被左缘推着走，翻面了 —— 这就是量到的那个抖动"
        );
        assert_eq!(
            front(30.5, &runs),
            vec![8, 0],
            "曲子相对锚点：8 号整段钉在同一格，直到它自己出窗"
        );
    }

    /// 侧槽不再各占一条等宽键道：它们贴着中央塔摆，整组以窗口中线为中心。
    #[test]
    fn side_slots_hug_the_center_tower_and_stay_symmetric() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 0, 60, 10, 100),
            (0, 1, 1, 60, 190, 100),
            (0, 2, 2, 60, 100, 100),
        ];
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, _) =
            slot_cells(&live, 5, 12, StackOrder::ByInstrument, true, &Runs::default());
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0));
        let plan = arrange(&groups, &[], area, 700.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        assert_eq!(plan.cells.len(), 3);
        let x_of = |id: u8| {
            plan.cells
                .iter()
                .find(|c| c.instrument == id)
                .map(|c| c.rect.center().x)
                .unwrap_or(f32::NAN)
        };
        assert!(x_of(0) < x_of(2) && x_of(2) < x_of(1), "左/中/右该按槽序铺开");
        // 中央槽永远压在窗口中线上：pan 100 = 正中，这条一漂「左右」就废了。
        assert!(
            (x_of(2) - area.center().x).abs() < 0.01,
            "中央塔偏了：{}",
            x_of(2)
        );
        // 每槽占「自己的内容宽 + 一条缝」，缝在这里顶到上限（1600 宽的余量多）。
        let stride = plan.tile + SPREAD_MAX;
        for (id, slot) in [(0u8, 0usize), (2, 2), (1, 4)] {
            let want = area.center().x + (slot as f32 - 2.0) * stride;
            assert!(
                (x_of(id) - want).abs() < 0.5,
                "乐器 {id} 摆偏了：{} vs 槽 {slot} 中心 {want}",
                x_of(id)
            );
        }
        // 最外两槽之间是 4 条相邻槽距，拿它和方块排式的 5 等分槽距比（原来 ≈ 313，现在 96）。
        let pitch = (area.width() - 2.0 * SIDE_MARGIN) / 5.0;
        let stride = (x_of(1) - x_of(0)) / 4.0;
        assert!(
            stride < pitch * 0.4,
            "相邻槽还是隔太远：{stride} vs 等分槽距 {pitch}"
        );
    }

    /// 底排压线，而且压的就是方块排那条线：默认偏移 0 时两条特效一起开，方块的
    /// **底边**重合（用户要的「默认位置和方块排重叠」就是这个，边长不必相同）。
    #[test]
    fn the_bottom_row_sits_on_the_block_row_baseline() {
        let rig = Rig::new(&[(0, 0, 0, 60, 100, 100)]).with_keyboard();
        let mut fx = PanStack::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.5, 0.0), &mut frame);
        let got = rects(&frame);
        assert_eq!(got.len(), 1);

        let mut row = BlockRow::default();
        // 方块排平时会晃（位移幅度是个设置），比底线时先把摆动关掉，不然差的就是那点摆幅。
        row.set_params(&serde_json::json!({ "nudge_px": 0.0 }));
        let mut row_frame = Frame::default();
        row.draw(&rig.cx(0.5, 0.0), &mut row_frame);
        let theirs = rects(&row_frame);
        assert_eq!(theirs.len(), 1);
        assert!(
            (got[0].bottom() - theirs[0].bottom()).abs() < 0.01,
            "底边要和方块排齐：{:?} vs {:?}",
            got[0],
            theirs[0]
        );
        assert!(got[0].center().y < rig.layout.unwrap().origin.y, "塔压在键盘上缘之上");

        // 「抬高」把整条塔顶上去。
        fx.set_params(&serde_json::json!({ "lift_px": 100.0 }));
        let mut up = Frame::default();
        fx.draw(&rig.cx(0.5, 0.0), &mut up);
        let want = PanStack::bottom_line(&rig.cx(0.0, 0.0), 0.0) - 100.0;
        assert!(
            (rects(&up)[0].bottom() - want).abs() < 0.01,
            "{:?} 底边该抬到 {want}",
            rects(&up)[0]
        );
    }

    /// 上下摞**故意**是重叠的（上面那枚盖住下面那枚的顶面），所以这里查的是「一格一个位置」：
    /// 任何两格不许落在同一个点上，排宽要和形状表逐项一致。
    #[test]
    fn cells_never_land_on_the_same_spot() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = (0..20u32)
            .map(|t| (t / 4, t % 4, t as u8, 55, 100, 100))
            .collect();
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, _) =
            slot_cells(&live, 5, 20, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(groups[2].len(), 20, "20 件乐器全在中央槽");
        let plan = arrange(&groups, &[], Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0)), 700.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        assert_eq!(plan.cells.len(), 20);
        for (i, a) in plan.cells.iter().enumerate() {
            for b in &plan.cells[i + 1..] {
                assert!(
                    (a.rect.center() - b.rect.center()).length() > plan.tile * 0.4,
                    "{a:?} 和 {b:?} 摞在同一个位置"
                );
            }
        }
        // 塔基那排最宽，形状和 [`center_shape(20, 5)`] 对得上。
        let rows: Vec<usize> = (0..center_shape(20, 5).len())
            .map(|r| {
                plan.cells
                    .iter()
                    .filter(|c| (c.rect.bottom() - (700.0 - r as f32 * ROW_STEP * plan.tile)).abs() < 0.5)
                    .count()
            })
            .collect();
        assert_eq!(rows, center_shape(20, 5), "摆出来的排宽要和形状表一致");
    }

    /// 排距、列距都要比一个边长小（紧凑摞），而且格子是自底向上推的 —— 后画的在上面，
    /// 底尖咬住的正是下面那枚的顶面，和参考图里那串柱子同一个层次。
    #[test]
    fn the_tower_stacks_overlapping_and_bottom_first() {
        const {
            assert!(ROW_STEP < 1.0, "排距不小于边长才谈得上「上方叠住下方」");
            assert!(
                COL_STEP < 1.0 && COL_STEP > 268.0 / 300.0,
                "列距要贴着摆（不再留 15% 空缝），又不许切进相邻那枚的身体"
            );
        }
        // 直接摆 6 件在中央槽：center_shape(6, 5) = [3, 2, 1]。
        let groups: Vec<Vec<Cell>> = vec![
            Vec::new(),
            Vec::new(),
            (0..6u8).map(|i| (i, i as usize)).collect(),
            Vec::new(),
            Vec::new(),
        ];
        let plan = arrange(
            &groups,
            &[],
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0)),
            700.0,
            1.0,
            48.0,
            5,
        );
        assert_eq!(plan.cells.len(), 6);
        let tile = plan.tile;
        // 推入顺序 = 画序：前三格是底排，最后一格是塔尖。
        let row_of = |c: &Placed| ((700.0 - c.rect.bottom()) / (ROW_STEP * tile)).round() as usize;
        assert_eq!(
            plan.cells.iter().take(3).map(row_of).collect::<Vec<_>>(),
            vec![0, 0, 0],
            "底排先画"
        );
        assert_eq!(row_of(plan.cells.last().unwrap()), 2, "塔尖最后画，才会盖住下面");
        // 上下两排必然重叠：上排的底边落到下排的顶边之下。
        let lower = plan.cells[0];
        let above = plan.cells.iter().find(|c| row_of(c) == 1).unwrap();
        assert!(
            above.rect.center().y + tile * 0.5 > lower.rect.center().y - tile * 0.5,
            "上排的底边该压进下排的顶面里"
        );
        assert!(
            (lower.rect.center().y - above.rect.center().y - ROW_STEP * tile).abs() < 0.01,
            "排距不对：{}",
            lower.rect.center().y - above.rect.center().y
        );
        // 同排相邻两列的间距 = COL_STEP * tile。
        let right = plan.cells.iter().filter(|c| row_of(c) == 0).nth(1).unwrap();
        assert!(
            (right.rect.center().x - lower.rect.center().x - COL_STEP * tile).abs() < 0.01,
            "列距不对：{}",
            right.rect.center().x - lower.rect.center().x
        );
    }

    /// 画面矮到放不下时从顶上裁整排，并留一句说明。
    #[test]
    fn a_short_viewport_trims_whole_rows_from_the_top() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> =
            (0..15u32).map(|t| (t, 0, t as u8, 60, 100, 100)).collect();
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, _) =
            slot_cells(&live, 5, 20, StackOrder::ByInstrument, true, &Runs::default());
        assert_eq!(center_shape(15, 5).len(), 5, "15 件 = 5+4+3+2+1");
        // 只给 56px 高：边长夹到底线 26，够两排（5+4），顶上 6 件裁掉。
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 90.0));
        let plan = arrange(&groups, &[], area, 80.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        assert!(
            (plan.tile - MIN_TILE).abs() < 0.01,
            "边长该被夹到下限而不是压成 0：{}",
            plan.tile
        );
        assert_eq!(plan.trimmed, 6, "顶排的 6 件没地方摆：{}", plan.cells.len());
        assert_eq!(plan.cells.len(), 9);
        assert!(
            plan.cells
                .iter()
                .all(|c| c.rect.top() >= area.min.y + TOP_MARGIN - 0.01),
            "裁完不许怼出画面顶部"
        );
        assert_eq!(rows_that_fit(56.0, MIN_TILE), 2);
        assert_eq!(rows_that_fit(MIN_TILE - 1.0, MIN_TILE), 0, "连一排都放不下就是 0");
        assert_eq!(rows_that_fit(MIN_TILE, MIN_TILE), 1);
        assert_eq!(rows_that_fit(1.0, 0.0), 0, "边长为 0 不许除零");
        // 底线只夹「自动收缩」：用户把「方块大小」调到 20，就按 20 摆，不许被底线顶回去。
        // 20px 那档画面高度够三排（5+4+3），所以裁掉的只剩顶两排。
        let explicit = arrange(&groups, &[], area, 80.0, 1.0, 20.0, INDENT_DEFAULT as usize);
        assert!(
            (explicit.tile - 20.0).abs() < 0.01,
            "用户的边长被底线顶了：{}",
            explicit.tile
        );
        assert_eq!(explicit.trimmed, 3, "20px 装得下 5+4+3");
    }

    /// 窗口变窄时先收紧槽缝，再按整组内容宽缩边长；缩到底线就停，宁可让整组略微出界
    /// 也不把方块压成看不清的碎块。
    #[test]
    fn a_narrow_window_shrinks_the_tile_instead_of_spilling() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> =
            (0..20u32).map(|t| (t, 0, t as u8, 60, 100, 100)).collect();
        let rig = Rig::new(&notes);
        let live: Vec<&FxNote> = rig.timeline.notes().iter().collect();
        let (groups, _) =
            slot_cells(&live, 5, 20, StackOrder::ByInstrument, true, &Runs::default());
        let drawn_inside = |plan: &Plan, width: f32| {
            plan.cells
                .iter()
                .all(|c| c.rect.left() >= 0.0 - 0.01 && c.rect.right() <= width + 0.01)
        };
        // 600 宽：缝落在 [SLOT_GAP, SPREAD_MAX] 里，边长还守得住基准 48（侧槽贴过来后
        // 省下的宽度优先给塔，不再靠缩方块换空间）。
        let plan = arrange(&groups, &[], Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 900.0)), 800.0, 1.0, 48.0, 5);
        assert!((plan.tile - 48.0).abs() < 0.01, "有余量时不该白缩：{}", plan.tile);
        assert!(drawn_inside(&plan, 600.0), "整组要待在画面里");
        // 420 宽：缝先顶到下限，剩下的只能缩边长。
        let tight = arrange(&groups, &[], Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 900.0)), 800.0, 1.0, 48.0, 5);
        assert!(tight.tile < 48.0 - 0.01, "窄窗口该开始缩边长：{}", tight.tile);
        assert!(tight.tile > MIN_TILE + 0.01, "还轮不到底线：{}", tight.tile);
        assert!(tight.cells.iter().all(|c| c.rect.width() <= tight.tile + 0.01));
        assert!(drawn_inside(&tight, 420.0), "缩完还得待在画面里");
        // 300 宽：夹到底线就停，摆出来的塔仍然在画面内（塔自己只有 6 列宽）。
        let tiny = arrange(&groups, &[], Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 900.0)), 800.0, 1.0, 48.0, 5);
        assert!((tiny.tile - MIN_TILE).abs() < 0.01, "边长该夹在底线：{}", tiny.tile);
        assert!(drawn_inside(&tiny, 300.0));
    }

    /// 一帧没开（音符早过寿命）时不该留东西，也不该被上一条特效的历史影响。
    #[test]
    fn nothing_survives_its_lifetime() {
        let rig = Rig::new(&[(2, 0, 0, 60, 100, 100), (60, 0, 7, 62, 100, 100)]);
        let mut fx = PanStack::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.0, 0.0), &mut frame);
        assert!(rects(&frame).is_empty(), "还没响的音符不该有方块");
        frame.clear();
        fx.draw(&rig.cx(2.5, 0.0), &mut frame);
        assert_eq!(rects(&frame).len(), 1);
        frame.clear();
        fx.draw(&rig.cx(30.0, 0.0), &mut frame);
        assert!(rects(&frame).is_empty(), "过了寿命就该消失");
    }

    /// 缺素材时退成色块，并提一句去扒贴图。
    #[test]
    fn colors_and_notes_degrade_the_same_way() {
        let rig = Rig::new(&[(0, 0, 0, 60, 0, 100), (0, 1, 7, 62, 200, 100)]);
        let mut fx = PanStack::default();
        let mut frame = Frame::default();
        fx.draw(&rig.cx(0.5, 0.0), &mut frame);
        let fills: Vec<Color32> = frame
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect {
                    fill,
                    stroke: None,
                    ..
                } => Some(*fill),
                _ => None,
            })
            .collect();
        assert_eq!(fills.len(), 2, "左右两槽各一块");
        assert_ne!(fills[0], fills[1]);
        let msg = fx.note().unwrap();
        assert!(msg.contains("贴图") || msg.contains("textures"), "{msg}");
        let got = rects(&frame);
        assert!(got[0].center().x < got[1].center().x, "pan 0 的要在 pan 200 的左边");
    }

    #[test]
    fn params_round_trip() {
        let fx = PanStack::default();
        let value = fx.params();
        // `f32` 存不下 1.6，序列化出来是 1.600000023841858 —— 按近似比。
        assert!((value["life_seconds"].as_f64().unwrap() - 1.6).abs() < 1e-4);
        assert_eq!(value["slots"].as_u64(), Some(5));
        assert_eq!(value["cap"].as_u64(), Some(12));
        assert_eq!(
            value["indent_from"].as_u64(),
            Some(INDENT_DEFAULT as u64),
            "缩进起始的默认值必须还是形状表原来那个 5"
        );
        assert_eq!(value["lift_px"].as_f64(), Some(0.0), "默认和方块排重合");
        assert_eq!(value["use_tiles"].as_bool(), Some(true));
        assert_eq!(
            value["order"].as_str(),
            Some("by_instrument"),
            "出厂得是按乐器号，刚验收过的塔形不能因为这条链变"
        );
        assert_eq!(
            value["pin_percussion"].as_bool(),
            Some(true),
            "钉最前是已拍过的口径，默认得开着"
        );
        let mut back = PanStack::default();
        back.set_params(&serde_json::json!({ "slots": 9, "cap": 3, "indent_from": 12 }));
        assert_eq!(back.params.slots, 9);
        assert_eq!(back.params.cap, 3);
        assert_eq!(back.params.indent_from, 12);
        assert_eq!(
            back.params.order,
            StackOrder::ByInstrument,
            "旧首选项里没这两个键，得走默认而不是掉进另一档"
        );
        assert!(back.params.pin_percussion);
        // `set_params` 是整个 Params 换掉（缺的字段走默认），所以试另一组键得另开一个。
        let mut switched = PanStack::default();
        switched.set_params(&serde_json::json!({ "order": "by_arrival", "pin_percussion": false }));
        assert_eq!(switched.params.order, StackOrder::ByArrival);
        assert!(!switched.params.pin_percussion);
        assert!((back.params.life_seconds - 1.6).abs() < 1e-4, "缺的字段走默认");
        assert_eq!(slots_used(back.params.slots), 9);
        // 旧首选项里存了偶数/越界值：取最近能用的奇数，别画出没有中央槽的画面。
        assert_eq!(slots_used(4), 5);
        assert_eq!(slots_used(0), 3);
        assert_eq!(slots_used(99), 9);
        let before = back.params();
        back.set_params(&serde_json::json!("nope"));
        assert_eq!(back.params(), before, "读不出来就不能把参数改掉");
    }

    #[test]
    fn it_declares_itself_as_an_overlay_above_the_block_row() {
        let info = PanStack::default().info();
        assert_eq!(info.id, PanStack::ID);
        assert_eq!(info.surface, Surface::Overlay);
        assert!(
            info.z > BlockRow::default().info().z,
            "两条默认重合，声道方块要压在方块排上面"
        );
        assert!(!info.wants_piano, "关掉钢琴栏也照样画");
    }

    /// 预留宽度数的是**格子**（同槽同乐器只算一件），不是音符，还要按停留秒数开窗。
    #[test]
    fn the_envelope_peaks_on_cells_not_notes() {
        // 一件乐器连响 41 个音：音符一堆，中央槽该预留的还是一列。
        let repeats: Vec<(u32, u32, u8, u8, u8, u8)> =
            (0..=40u32).map(|t| (t, 0, 3, 100, 100, 100)).collect();
        let rig = Rig::new(&repeats);
        let at = |tick: u32| f64::from(rig.timeline.seconds_at(f64::from(tick)));
        let peaks = slot_peaks(rig.timeline.notes(), 5, 20, at(40) * 2.0, at);
        assert_eq!(peaks.len(), 5);
        assert_eq!(peaks[2], 1, "41 个音同一件乐器 ⇒ 中央槽 1 列，不是 41 列");
        assert_eq!(peaks[0], 0, "没音符的槽不留宽度");

        // 四件乐器依次响：窗盖得住就是 4，只容得下相邻两枚就是 2。
        let four: Vec<(u32, u32, u8, u8, u8, u8)> =
            (0..4u32).map(|i| (i * 10, 0, i as u8, 100, 100, 100)).collect();
        let rig = Rig::new(&four);
        let at = |tick: u32| f64::from(rig.timeline.seconds_at(f64::from(tick)));
        let span = at(30);
        assert_eq!(slot_peaks(rig.timeline.notes(), 5, 20, span * 2.0, at)[2], 4);
        assert_eq!(
            slot_peaks(rig.timeline.notes(), 5, 20, span / 2.0, at)[2],
            2,
            "窗只盖住相邻两枚，峰值就该是 2"
        );
        assert_eq!(
            slot_peaks(rig.timeline.notes(), 5, 3, span * 2.0, at)[2],
            3,
            "超上限的乐器本来就不画，不该占地方"
        );
    }

    /// 每槽各算各的：左边三件不会算进中央那一栏。
    #[test]
    fn each_slot_is_counted_on_its_own_side() {
        let notes: Vec<(u32, u32, u8, u8, u8, u8)> = vec![
            (0, 0, 0, 60, 20, 100),
            (0, 1, 1, 62, 20, 100),
            (0, 2, 2, 64, 20, 100),
            (0, 3, 3, 66, 100, 100),
            (0, 4, 4, 68, 100, 100),
        ];
        let rig = Rig::new(&notes);
        let at = |tick: u32| f64::from(rig.timeline.seconds_at(f64::from(tick)));
        assert_eq!(
            slot_peaks(rig.timeline.notes(), 5, 20, 6.0, at),
            vec![3, 0, 2, 0, 0]
        );
    }

    /// 区域按整曲峰值预留之后，中央槽空不空都不再挪侧槽 —— 这条就是「恒定」本身。
    #[test]
    fn a_precomputed_region_freezes_the_layout_while_the_song_thins_out() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0));
        let side = |instrument: u8| -> Vec<Cell> { vec![(instrument, 0)] };
        let center = |n: u8| -> Vec<Cell> { (0..n).map(|i| (i, 0)).collect() };
        let wide = vec![side(100), side(101), center(12), side(103), side(104)];
        let thin = vec![side(100), side(101), center(3), side(103), side(104)];
        let envelope = [1usize, 1, 12, 1, 1];
        let full = arrange(&wide, &envelope, area, 800.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        let spare = arrange(&thin, &envelope, area, 800.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        let live = arrange(&thin, &[], area, 800.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        let x_of_sides = |plan: &Plan| -> Vec<f32> {
            plan.cells
                .iter()
                .filter(|p| p.instrument >= 100)
                .map(|p| p.rect.center().x)
                .collect()
        };
        assert_eq!(full.tile, spare.tile, "边长不该随这一帧有几件乐器跳");
        assert_eq!(
            x_of_sides(&full),
            x_of_sides(&spare),
            "同一份区域 ⇒ 侧槽的横位置在满帧和空帧里一模一样"
        );
        assert!(
            x_of_sides(&spare)[0] < x_of_sides(&live)[0],
            "预留了区域，中央一空起来侧槽就不会往里缩：{:?} vs {:?}",
            x_of_sides(&spare),
            x_of_sides(&live)
        );
        // 整组仍然以窗口中线为中心。
        let xs = x_of_sides(&spare);
        let mid = area.center().x;
        assert!(
            ((mid - xs[0]).abs() - (xs[3] - mid).abs()).abs() < 0.01,
            "左右要对称：{xs:?}"
        );
    }

    /// 区域是**下限**不是天花板：某一帧真比峰值还密时按现场算，不许把方块挤到一起。
    #[test]
    fn a_frame_denser_than_the_region_spreads_to_the_frame() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0));
        let center: Vec<Cell> = (0..12u8).map(|i| (i, 0)).collect();
        let groups = vec![vec![], vec![], center, vec![], vec![]];
        let small = arrange(&groups, &[1, 1, 2, 1, 1], area, 800.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        let big = arrange(&groups, &[1, 1, 12, 1, 1], area, 800.0, 1.0, 48.0, INDENT_DEFAULT as usize);
        let shape = |plan: &Plan| -> Vec<(u8, f32, f32)> {
            plan.cells
                .iter()
                .map(|p| (p.instrument, p.rect.left(), p.rect.top()))
                .collect()
        };
        assert_eq!(small.cells.len(), 12);
        assert_eq!(shape(&small), shape(&big), "现场比区域密就该按现场摆");
    }

    /// 端到端：密集那一帧和中央空掉的那一帧，最右槽的方块必须落在同一个 x 上。
    #[test]
    fn a_side_block_stays_put_from_the_dense_frame_to_the_empty_one() {
        // 中央 12 件挤在开头 1.1 秒里（峰值 = 12），右槽那一件在头尾各响一次。
        let mut notes: Vec<(u32, u32, u8, u8, u8, u8)> = (0..12u32)
            .map(|t| (t, 0, t as u8, 60 + t as u8, 100, 100))
            .collect();
        notes.push((4, 1, 15, 70, 180, 100));
        notes.push((40, 2, 15, 70, 180, 100));
        let rig = Rig::new(&notes);
        let mut fx = PanStack::default();
        let mut dense = Frame::default();
        // 播到 tick 11.5（1.15 秒）：12 件中央 + 右槽那件都在屏上。
        fx.draw(&rig.cx(11.5, 0.0), &mut dense);
        assert_eq!(rects(&dense).len(), 13, "12 件中央 + 1 件右槽");
        let mut sparse = Frame::default();
        // 播到 tick 40.5（4.05 秒）：中央早淡完了，只剩右槽。
        fx.draw(&rig.cx(40.5, 0.0), &mut sparse);
        let only = rects(&sparse);
        assert_eq!(only.len(), 1);
        let mut all = rects(&dense);
        let right_in_dense = all.pop().unwrap();
        assert!(
            (right_in_dense.center().x - only[0].center().x).abs() < 0.01,
            "中央空了右槽就该不动：{right_in_dense:?} vs {:?}",
            only[0]
        );
        assert_eq!(right_in_dense.width(), only[0].width(), "边长也不该跳");
    }
}
