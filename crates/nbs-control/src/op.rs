//! 能力清单（`Op`）。这里的单位**不是按钮**，是「一次意图完整的操作」：
//! 所有会影响结果的上下文都必须写成参数，因为外部调用方没有鼠标、没有悬停格、
//! 也没有窗口草稿态。凡是人点一下才成立的流程，一律在这里改成一次提交。
//!
//! 命名口径：`tick` / `layer` 从 0 起算，与 `.nbs` 文件一致；
//! `key` 0..=87，`velocity` 0..=100，`panning` 0..=200（100 = 中央），
//! `pitch` ±1200 音分。这些边界由 `nbs-core` 钉死，本文件只负责把它们说清楚。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一个格子上的音符。`velocity` / `panning` / `pitch` 省略时取新建音符的默认值
/// （100 / 100 / 0），与界面画笔默认一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CellNote {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "tick 列号，从 0 起算，和 .nbs 文件一致")
    )]
    pub tick: u32,
    #[cfg_attr(feature = "schema", schemars(description = "层号，从 0 起算"))]
    pub layer: u32,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "音色号：内置 0..=19，自定义从 20 起（先看 get_instruments）")
    )]
    pub instrument: u8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "音高键 0..=87；Minecraft 只能播 33..=57")
    )]
    pub key: u8,
    #[cfg_attr(feature = "schema", schemars(description = "音量 0..=100，省略取 100"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "左右声道 0..=200，100 = 中央，省略取 100")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panning: Option<u8>,
    #[cfg_attr(feature = "schema", schemars(description = "音分偏移 ±1200，省略取 0"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<i16>,
}

/// 批量操作的作用范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Scope {
    /// 整首歌（含锁定层由服务端按原版语义决定，不静默改锁定层）。
    All,
    /// 当前浮动选区。
    Selection,
    /// 矩形范围；服务端只动其中**已有音符**的格子。
    Rect {
        tick_from: u32,
        layer_from: u32,
        tick_to: u32,
        layer_to: u32,
    },
}

/// 从一堆格子里挑出「就是这批」的条件。每一格省略 = 这一条不设限，全部成立才算命中（与，不是或）。
///
/// 它只回答「这一格是不是你要摘的那批」——不改任何值、不做取模。命中集合是从范围内
/// **已有音符**的格子里挑出来的，所以空格子永远不算命中，`matched=0` 说的就是「这一段没有你要的东西」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NoteFilter {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "只算这个音色的格子；本工程没这个音色会直接拒，不会当成「没命中」")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "键号下界（含），省略 = 不设限")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_from: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "键号上界（含），省略 = 不设限")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_to: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "力度下界（含），省略 = 不设限")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity_min: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "力度上界（含），省略 = 不设限。摘掉太轻的音符就填这一格")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity_max: Option<u8>,
    /// 这一条 `key_from` / `key_to` 表达不了：那是「低于下界**或**高于上界」，两个方向。
    #[cfg_attr(
        feature = "schema",
        schemars(description = "true = 只算 MC 两个八度（33..=57）之外的音符")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outside_mc_range: Option<bool>,
}

impl NoteFilter {
    /// 一个条件都没设 = 范围内全要。
    pub fn is_unrestricted(&self) -> bool {
        self.instrument.is_none()
            && self.key_from.is_none()
            && self.key_to.is_none()
            && self.velocity_min.is_none()
            && self.velocity_max.is_none()
            && self.outside_mc_range.is_none()
    }

    /// 这一格算不算命中。MC 那两个八度的边界由调用方给：那是导出侧的口径，
    /// 协议里不该写死一个数，否则哪天改了键域这边就悄悄错了。
    pub fn matches(&self, instrument: u8, key: u8, velocity: u8, mc_key: (u8, u8)) -> bool {
        if self.instrument.is_some_and(|want| want != instrument) {
            return false;
        }
        if self.key_from.is_some_and(|low| key < low) || self.key_to.is_some_and(|high| key > high) {
            return false;
        }
        if self.velocity_min.is_some_and(|low| velocity < low)
            || self.velocity_max.is_some_and(|high| velocity > high)
        {
            return false;
        }
        // 「超出 MC 音域」是低于下界**或**高于上界，两个方向，单段区间那种写法表达不了。
        if self.outside_mc_range == Some(true) && key >= mc_key.0 && key <= mc_key.1 {
            return false;
        }
        true
    }
}

/// 选区操作的目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SelectTarget {
    /// 矩形框选：默认摘成浮动选区，`copy=true` 只复制。
    Rect {
        tick_from: u32,
        layer_from: u32,
        tick_to: u32,
        layer_to: u32,
        /// true = 只复制不摘出（对应 Alt 框选）；false = 摘成浮动选区。
        copy: bool,
        /// true = 并入现有选区（对应 Ctrl 加选）。
        additive: bool,
    },
    /// 全选整首歌。
    All,
    /// 取消选择：浮动选区落回歌曲。
    None,
    /// 反选：现有选区之外、且有音符的格子全部进选区。
    Invert,
    /// 按音色选：`invert=true` 选的是「不是这个音色」的音符。
    ByInstrument { instrument: u8, invert: bool },
    /// Minecraft 两个八度（33..=57）之外的音符。
    Outside { lower: bool, higher: bool },
    /// 用了自定义音色的音符。
    Custom,
}

/// 批量改某个字段的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum BatchField {
    /// 换音色（内置 0..=19，自定义从 20 起）。
    Instrument,
    /// 移音高键 0..=87。
    Key,
    /// 改音量 0..=100。
    Velocity,
    /// 改声道 0..=200，100 = 中央。
    Panning,
    /// 改音分 ±1200。
    Pitch,
}

/// 元数据一次提交。省略的字段不动。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SetMeta {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "工程名（界面 Song name，会显示在标题栏）")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[cfg_attr(feature = "schema", schemars(description = "当前作者"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "原作者：.nbs 里独立一栏，改作者不会带动它")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_author: Option<String>,
    #[cfg_attr(feature = "schema", schemars(description = "工程说明，可多行"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// ticks/秒，服务端夹在 0.25..=180（与界面同一条闸）。
    #[cfg_attr(
        feature = "schema",
        schemars(description = "ticks/秒，服务端夹在 0.25..=180（和界面同一条闸）")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f32>,
    /// 2..=8，每小节 = time_signature × 4 ticks。
    #[cfg_attr(
        feature = "schema",
        schemars(description = "拍号 2..=8；每小节 = time_signature × 4 ticks")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "循环开关；关掉后播放到头即停")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_enabled: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "循环起点 tick，0 起算"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_start: Option<u16>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "循环额外重复次数（.nbs 的 loop count）；0 = 不重复")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_max: Option<u8>,
    /// 循环终点：`.nbs` 没有这个字段，靠结尾静音音符表达，服务端只延长不缩短。
    #[cfg_attr(
        feature = "schema",
        schemars(description = "循环终点：只能延长；缩短要自己删末尾音符，服务端会拒")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end: Option<u32>,
}

/// 图层属性的部分更新。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LayerSet {
    #[cfg_attr(feature = "schema", schemars(description = "层名"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[cfg_attr(feature = "schema", schemars(description = "层音量 0..=100"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "层声像 0..=200，100 = 中央")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stereo: Option<u8>,
    /// "normal" | "locked" | "solo"
    #[cfg_attr(
        feature = "schema",
        schemars(description = "锁定状态：normal | locked | solo")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
}

/// 播放控制。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Playback {
    /// 从当前落点起播（与界面播放按钮同一语义：停在歌尾先回 0）。
    Play,
    Pause,
    Stop,
    Seek {
        tick: f64,
    },
    /// 只改循环开关，不动其他元数据。
    SetLoop {
        enabled: bool,
    },
}

/// 导出格式。阶段一先接 wav / midi / png；schematic 与 datapack 依赖
/// 窗口草稿态与声源分配方案，作为后续能力单独加，不在这里给半成品开关。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ExportFormat {
    /// 用当前音色库离线混音成波形音频。
    Wav,
    /// 转出 .mid：只有音符和控制变化，不带采样音色。
    Midi,
    /// 钢琴卷帘截图。
    Png,
}

/// `GetNotes` 返回什么形状。三档互斥，所以是一个枚举而不是两个 bool——
/// 「列式的段摘要」这种没人能解释的组合不该被允许写出来。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum NoteView {
    /// 逐格：每格一个对象，字段名重复八遍。默认。
    #[default]
    Cells,
    /// 列式：八条等长数组，同一段数据比 `cells` 省一大半字节。
    Columns,
    /// 延音段摘要：同层、同音色、同键、tick 连续的一串格子合成一段。
    Runs,
}

/// 一次外部调用。所有变体都带显式参数，没有「按当前状态猜」的入口。
///
/// 线格式定成 `{"op":{"kind":"set_notes", ...}}`：`kind` 当 tag、外面再套一层 `op`，
/// 是因为 serde 的 `flatten` 配内部 tag 在反序列化上有坑，套两层最稳。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// 探活 + 版本核对；不碰任何状态。
    Ping,
    /// 列逃生舱（`RunAction`）可用的动作与它们的参数说明。
    ListActions,
    /// 逃生舱：按名字调用一条界面动作，参数走 `args`。
    /// 只接受 `ListActions` 里出现的动作名，未知名字一律拒绝。
    RunAction {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Value>,
    },

    // -------------------------------------------------------------- 读
    /// 工程概览：元数据、图层数、音符数、长度、播放位置、脏标记、兼容性诊断。
    GetSnapshot,
    GetNotes {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tick_from: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tick_to: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer_from: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer_to: Option<u32>,
        /// 小节宽度（格数）。省略时用工程的 `time_signature * 4`，也就是界面画竖线的那对数。
        /// 只给了它没给 `bars` 时，区间照原样读，文字视图按这个宽度切。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ticks_per_bar: Option<u32>,
        /// `[起始小节, 结束小节]`，两头都含。给了就换成按小节算的 tick 区间，
        /// 并且回执里带一份小节文字视图。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bars: Option<[u32; 2]>,
        /// 一次最多回多少条（`cells` / `columns` 按格算，`runs` 按段算）。
        /// 省略 = 1000 条；超过 20000 按 20000 走，回执会说。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        /// 字节预算：按每格（每段）的估算字节折算成条数，和 `limit` 谁小听谁的。
        /// 省略 = 128 KiB；超过 1 MiB 按 1 MiB 走，回执会说。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
        /// 返回形状：`cells`（默认）/ `columns`（列式，省字节）/ `runs`（延音段摘要）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<NoteView>,
    },
    /// 图层清单。`layer_from` / `layer_to` 是**层号闭区间**，省略 = 全部层。
    /// 层多的时候给个窗口：整张表要按层数算字节，86 层就是十来 KB。
    GetLayers {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer_from: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer_to: Option<u32>,
    },
    GetSelection,
    GetInstruments,
    GetDiagnostics,
    GetHistory,
    /// 标签清单；读取不切标签。
    ListTabs,
    /// 切到指定标签，播放与交互按原版 set_song 归零。
    SwitchTab {
        tab: crate::bus::TabRef,
    },
    /// 关闭指定标签；脏标签必须显式 discard=true。
    CloseTab {
        tab: crate::bus::TabRef,
        #[serde(default)]
        discard: Option<bool>,
    },

    // -------------------------------------------------------------- 写
    /// 批量写格子：**一次调用 = 一次撤销**。整批要么全收要么全拒（先校验后落笔）。
    SetNotes {
        cells: Vec<CellNote>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// 乐观并发令牌，取上一次回执里的 `rev`；省略 = 不校验。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    RemoveCells {
        /// 逐格摘除：这一支的行为和老版本一致（空格子不算改动、锁定层挡下）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cells: Vec<(u32, u32)>,
        /// 按范围摘除。和 `cells` 二选一：两份说法同时给等于没说清到底删哪批。
        /// 这里只认 `all` / `rect`——浮动选区里的音符已经离开网格了，摘它不是删格子。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<Scope>,
        /// 只在给了 `scope` 时有意义：从范围内已有音符里再挑一刀。省略 = 范围内全删。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<NoteFilter>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    BatchField {
        field: BatchField,
        /// "set" = 直接赋值，"add" = 在现值上加（越界由服务端夹取并回报）。
        mode: String,
        value: i32,
        scope: Scope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    Transpose {
        scope: Scope,
        semitones: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    Select {
        target: SelectTarget,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    /// 把浮动选区落回歌曲（人用鼠标拖放，这里就是「就地落下」）。
    PlaceSelection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    LayerAdd {
        /// 省略 = 追加到末尾。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    LayerRemove {
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    LayerUpdate {
        index: u32,
        set: LayerSet,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    LayerMove {
        from: u32,
        to: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    SetMeta {
        meta: SetMeta,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    Redo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
    },
    ClearSong {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_rev: Option<u64>,
        /// 歌曲有未保存改动时必须显式认错：清空是不可撤销地丢掉这些改动。
        #[serde(default)]
        discard_changes: bool,
    },

    // -------------------------------------------------------------- 传输 / 文件
    Playback(Playback),
    /// 直接开档，不弹选框。
    OpenPath {
        path: String,
        /// 当前工程有未保存改动时的开关；不开就报 `not_allowed`，
        /// 因为界面那条路径（`open_nbs_at`）会**静默**丢掉改动与全部历史。
        #[serde(default)]
        discard_changes: bool,
    },
    /// 存到指定路径（路径必须落在服务端允许的目录里）。
    SavePath {
        path: String,
        /// 省略 = 用当前歌曲的保存版本；越界由服务端拒绝而不是偷偷降版本。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u8>,
        /// 目标已存在且不是这个工程自己的文件时必须为 true。
        /// 存回当前文件名不需要它——那是「保存」的正常语义。
        #[serde(default)]
        overwrite: bool,
    },
    /// 存回当前文件名；没有文件名时报错，绝不弹「另存为」。
    Save,
    /// 导入 MIDI 并**替换**当前工程（历史清空，与界面导入一致）。
    /// 选项全部可省略，省略 = 复刻人打开导入窗时那份基线，见 [`crate::midi::ImportMidiOptions`]。
    ImportMidi {
        path: String,
        /// 同 `OpenPath`：替换会丢掉当前工程的改动与历史。
        #[serde(default)]
        discard_changes: bool,
        /// 传统导入的 14 个标量 + 通道/鼓映射 + 尾音声部；省略 = 基线。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<crate::midi::ImportMidiOptions>,
        /// 增强导入那一套；省略 = 传统路径。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enhanced: Option<crate::midi::ImportEnhancedOptions>,
        /// true = 把这次用到的通道/鼓映射写回 GM 表，并把标量存进偏好——
        /// 那是**改用户的长期设置**，所以省略算 false，且增强路径不写回（和界面一致）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remember: Option<bool>,
    },
    /// 只算不写：把某个 MIDI 文件按给定选项走一遍导入计算，回报会得到什么。
    /// 当前工程一点不动，所以它是 `import_midi` 之前的那一步核对。
    InspectMidi {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<crate::midi::ImportMidiOptions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enhanced: Option<crate::midi::ImportEnhancedOptions>,
    },

    // -------------------------------------------------------------- 重活 / 任务
    /// 起后台任务：返回句柄，之后用 `JobStatus` 轮询。
    Export {
        path: String,
        format: ExportFormat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_rate: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channels: Option<u16>,
        /// 省略 = 按原版默认排除锁定层。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_locked: Option<bool>,
        /// true = 覆盖已存在的目标文件；false 时目标存在直接报错。
        #[serde(default)]
        overwrite: bool,
    },
    JobStatus {
        id: u64,
    },
    JobList,
    JobCancel {
        id: u64,
    },

    // -------------------------------------------------------------- 音频转 MIDI
    // 这四条全部对应转录窗上那四个按钮：都起一个本机子进程，所以一律回任务句柄。
    // 它们既不改工程也不动撤销历史，唯一「写」的磁盘是 `TranscribeAudio` 里
    // 调用方自己点名的 `save_midi`——那条走和 `export` 完全相同的路径闸与覆盖闸。
    /// 问本机转录环境在不在（Python / torch / MuScriptor / GPU）。只读子进程的输出，
    /// 不下载任何东西，所以这一条**不需要**许可勾。
    TranscribeProbe,
    /// 装环境：uv 建 venv 并装 MuScriptor + torch，会下几百 MB。
    /// `cuda` = 装 cu128 构建还是 cpu 构建；省略 = 用转录窗上当前那个勾（这里不替人改它）。
    TranscribePrepareEnv {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cuda: Option<bool>,
    },
    /// 只把权重取来并加载一遍（预检模型），不识别音频。
    TranscribePrepareModel {
        #[serde(default)]
        options: crate::transcribe::TranscribeOptions,
    },
    /// 音频 → MIDI。产物先落在编辑器自己的 `.work/transcription/jobs/…` 里，
    /// 那个目录**不在**路径白名单内，所以要交给 `import_midi` 就得同时给 `save_midi`
    /// ——由调用方点名一份落在允许目录里的文件，覆盖照样要 `overwrite`。
    TranscribeAudio {
        /// 要识别的音频，必须落在允许的目录里且真的存在。
        audio: String,
        #[serde(default)]
        options: crate::transcribe::TranscribeOptions,
        /// 把识别出的 MIDI 另存到这个路径；省略 = 只留在任务产物里。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save_midi: Option<String>,
        /// true = 允许覆盖已存在的 `save_midi` 目标。
        #[serde(default)]
        overwrite: bool,
    },
}

impl Op {
    /// 只读操作：不改歌曲、不改播放、不碰磁盘，可以在任何状态下放行。
    /// 服务端用它决定「弹窗挡路时是否照样回答」。
    ///
    /// 断线之后要不要替调用方重发一次问的是另一件事，见 [`Self::is_resend_safe`]——
    /// 那一条比这条宽（`seek` / `stop` 幂等但不算只读），别把两者合成一个。
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Op::Ping
                | Op::ListActions
                | Op::GetSnapshot
                | Op::GetNotes { .. }
                | Op::GetLayers { .. }
                | Op::GetSelection
                | Op::GetInstruments
                | Op::GetDiagnostics
                | Op::GetHistory
                | Op::ListTabs
                | Op::JobStatus { .. }
                | Op::JobList
                // 读文件、算导入，但既不改工程也不落盘，所以弹窗挡在中间时照样该回答。
                | Op::InspectMidi { .. }
        )
    }

    /// 传输层断掉之后，桥接**能不能自己重发一次**这一问。
    ///
    /// 判据只有一个：第一次可能已经落进主线程收件箱并生效了，再来一次会不会多出后果。
    /// 只读的那些当然不会；此外两条幂等的走带动作也放进来，因为「放着不动一段时间之后
    /// 连 seek 都要人手工补一次」正是这轮要修的毛病，而它们改不出任何看不见的事：
    ///
    /// - `seek`：落的是绝对位置，重发就是再定位到同一个 tick；
    /// - `stop`：已经停着再停一次什么都不动。
    ///
    /// `play` 不在名单里，虽然它也「只是改走带」：重发会把人正听着的播放从头冲一遍，
    /// 或让本该停住的这一段又响起来——**响过几次这件事回执里看不见，只有耳朵知道**。
    /// `pause` / `set_loop` 同理留在名单外：桥接这一侧不知道服务端此刻的走带状态，
    /// 要不要重来一次该由调用方判断。
    pub fn is_resend_safe(&self) -> bool {
        self.is_read_only()
            || matches!(self, Op::Playback(Playback::Seek { .. } | Playback::Stop))
    }

    /// 可在后台标签上执行的文档级操作；历史与撤销仍走 Document 自己的事务。
    pub fn is_document_write(&self) -> bool {
        matches!(
            self,
            Op::SetNotes { .. }
                | Op::RemoveCells { .. }
                | Op::BatchField { .. }
                | Op::Transpose { .. }
                | Op::Select { .. }
                | Op::PlaceSelection { .. }
                | Op::LayerAdd { .. }
                | Op::LayerRemove { .. }
                | Op::LayerUpdate { .. }
                | Op::LayerMove { .. }
                | Op::SetMeta { .. }
                | Op::Undo { .. }
                | Op::Redo { .. }
        )
    }

    pub fn expect_rev(&self) -> Option<u64> {
        match self {
            Op::SetNotes { expect_rev, .. }
            | Op::RemoveCells { expect_rev, .. }
            | Op::BatchField { expect_rev, .. }
            | Op::Transpose { expect_rev, .. }
            | Op::Select { expect_rev, .. }
            | Op::PlaceSelection { expect_rev, .. }
            | Op::LayerAdd { expect_rev, .. }
            | Op::LayerRemove { expect_rev, .. }
            | Op::LayerUpdate { expect_rev, .. }
            | Op::LayerMove { expect_rev, .. }
            | Op::SetMeta { expect_rev, .. }
            | Op::Undo { expect_rev, .. }
            | Op::Redo { expect_rev, .. } => *expect_rev,
            _ => None,
        }
    }

    /// 只是「起一个后台任务」的操作：不改工程、不动撤销历史，所以弹窗挡在中间时
    /// 也该照样回答——转录窗本身开着的时候，人正是想让它去准备环境。
    /// （`TranscribeAudio` 的 `save_midi` 落盘是另一码事：那条过的是 `export` 同一套
    /// 路径闸与覆盖闸，工程状态一点不动。）
    pub fn only_spawns_a_job(&self) -> bool {
        matches!(
            self,
            Op::Export { .. }
                | Op::TranscribeProbe
                | Op::TranscribePrepareEnv { .. }
                | Op::TranscribePrepareModel { .. }
                | Op::TranscribeAudio { .. }
        )
    }
}
