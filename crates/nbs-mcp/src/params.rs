//! MCP 那一侧的入参结构：字段和 [`nbs_control::Op`] 的变体逐一对齐，转换是纯搬运。
//!
//! 为什么要多这一层：工具面要的是「每个参数都带说明的 JSON Schema」，而线协议要的是
//! 紧凑稳定的字节。schemars 不读 Rust 文档注释，所以给人看的说明写在这里的
//! `#[schemars(description = ...)]` 里 —— 这些句子是 AI 唯一能读到的口径，
//! 改了 `Op` 的语义就必须同时改这里。

use schemars::JsonSchema;
use serde::Deserialize;

use nbs_control::{
    BatchField, CellNote, ExportFormat, ImportEnhancedOptions, ImportMidiOptions, LayerSet, Op,
    Playback, Scope, SelectTarget, SetMeta, TabRef, TranscribeOptions,
};

/// 只读工具的可选标签寻址；省略 = 当前标签。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TabOnly {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SwitchTab {
    #[schemars(description = "稳定 id 或当前位置 index，二选一")]
    pub tab: TabRef,
}

impl From<SwitchTab> for Op {
    fn from(a: SwitchTab) -> Self {
        Op::SwitchTab { tab: a.tab }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CloseTab {
    #[schemars(description = "稳定 id 或当前位置 index，二选一")]
    pub tab: TabRef,
    #[schemars(description = "true = 明确丢弃未保存改动并关闭；脏标签省略会返回 needs_confirm")]
    #[serde(default)]
    pub discard: Option<bool>,
}

impl From<CloseTab> for Op {
    fn from(a: CloseTab) -> Self {
        Op::CloseTab {
            tab: a.tab,
            discard: a.discard,
        }
    }
}

/// `run_action` 逃生舱的一条调用：动作名 + 它的参数原样。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RunAction {
    #[schemars(description = "稳定 id 或当前位置 index；交互动作只允许当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(
        description = "动作名，必须是 list_actions 里出现过的一个；未知名字会被拒而不是被忽略"
    )]
    pub action: String,
    #[schemars(
        description = "该动作的参数，形状看 list_actions 里同名的 args 字段；没有参数就省略"
    )]
    #[serde(default)]
    pub args: Option<serde_json::Value>,
}

impl From<RunAction> for Op {
    fn from(a: RunAction) -> Self {
        Op::RunAction {
            action: a.action,
            args: a.args,
        }
    }
}

/// `set_notes` / `remove_cells` 里的一格坐标。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CellRef {
    #[schemars(description = "tick 列号，从 0 起算，和 .nbs 文件一致")]
    pub tick: u32,
    #[schemars(description = "层号，从 0 起算")]
    pub layer: u32,
}

/// 音高入参：键号 `45` 和音名 `"F#4"` 都收，解析时就折成键号，后面全是搬运。
///
/// 收音名是为了让「把这段移到高八度」这种话不必先心算 12 进制偏移，但**服务端那套
/// `0..=87` 校验仍是唯一裁判**：这里只负责把写法认出来，不负责判它合不合法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pitch(pub u8);

impl<'de> serde::Deserialize<'de> for Pitch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        // MCP 的入参本来就是 JSON，所以中间借一次 `Value`；换来的是「说得出为什么不行」，
        // 而 `#[serde(untagged)]` 只会吐一句「哪个变体都不匹配」。
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Number(n) => match n.as_u64() {
                Some(n) if n <= u64::from(nbs_core::KEY_MAX) => Ok(Self(n as u8)),
                Some(n) => Err(D::Error::custom(format!(
                    "键号 {n} 超出 0..={}",
                    nbs_core::KEY_MAX
                ))),
                None => Err(D::Error::custom(format!("键号要是整数，实际是 {n}"))),
            },
            serde_json::Value::String(text) => nbs_core::key_from_name(&text)
                .map(Self)
                .ok_or_else(|| {
                    D::Error::custom(format!(
                        "音名 {text:?} 认不出来：要字母 A–G、可选升降号（# ♯ b ♭）、再跟八度数字，\
                         例如 \"C4\"、\"F#3\"、\"Bb4\"；等音异名（\"Cb4\"）和不写八度（\"C\"）一律拒收，\
                         因为它们在键盘上没有唯一位置"
                    ))
                }),
            other => Err(D::Error::custom(format!(
                "音高要么给 0..={} 的整数，要么给音名字符串，实际是 {other}",
                nbs_core::KEY_MAX
            ))),
        }
    }
}

/// 手写而不是 derive：`anyOf` 的两种写法各自要带一句说明，derive 给不出这个形状。
impl JsonSchema for Pitch {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Pitch".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "音高：键号数字或音名字符串，两种写法等价",
            "anyOf": [
                {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": nbs_core::KEY_MAX,
                    "description": "键号 0..=87；Minecraft 原生音符盒只播 33..=57，超出的会标 out_of_range"
                },
                {
                    "type": "string",
                    "description": "音名：字母 A–G + 可选升降号（# ♯ b ♭）+ 八度数字，如 \"C4\"、\"F#3\"、\"Bb4\"。等音异名与省略八度会被拒而不是被猜"
                }
            ]
        })
    }
}

/// `set_notes` 的一格。字段和 [`CellNote`] 一一对应，只差音高允许用音名写。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CellInput {
    #[schemars(description = "tick 列号，从 0 起算，和 .nbs 文件一致")]
    pub tick: u32,
    #[schemars(description = "层号，从 0 起算")]
    pub layer: u32,
    #[schemars(description = "音色号：内置 0..=19，自定义从 20 起（先看 get_instruments）")]
    pub instrument: u8,
    #[schemars(description = "音高：键号数字或音名，例如 45 与 \"F#4\" 等价")]
    pub key: Pitch,
    #[schemars(description = "音量 0..=100，省略取 100")]
    #[serde(default)]
    pub velocity: Option<u8>,
    #[schemars(description = "左右声道 0..=200，100 = 中央，省略取 100")]
    #[serde(default)]
    pub panning: Option<u8>,
    #[schemars(description = "音分偏移 ±1200，省略取 0")]
    #[serde(default)]
    pub pitch: Option<i16>,
}

impl From<CellInput> for CellNote {
    fn from(c: CellInput) -> Self {
        Self {
            tick: c.tick,
            layer: c.layer,
            instrument: c.instrument,
            key: c.key.0,
            velocity: c.velocity,
            panning: c.panning,
            pitch: c.pitch,
        }
    }
}

/// 只带 `expect_rev` 的写操作（把浮动选区落回歌曲）。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RevOnly {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<RevOnly> for Op {
    fn from(a: RevOnly) -> Self {
        Op::PlaceSelection {
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetNotes {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "起始 tick，含；省略 = 0")]
    #[serde(default)]
    pub tick_from: Option<u32>,
    #[schemars(description = "结束 tick，含；省略 = 歌尾。整首读回是允许的，超上限会显式标 truncated")]
    #[serde(default)]
    pub tick_to: Option<u32>,
    #[schemars(description = "起始层号，含；省略 = 0")]
    #[serde(default)]
    pub layer_from: Option<u32>,
    #[schemars(description = "结束层号，含；省略 = 最后一层")]
    #[serde(default)]
    pub layer_to: Option<u32>,
    #[schemars(
        description = "小节宽度（格数），给 0 按 1 算；省略 = 工程的「拍号 × 4」，也就是界面画竖线的间距。给了它就额外回一份小节文字视图"
    )]
    #[serde(default)]
    pub ticks_per_bar: Option<u32>,
    #[schemars(
        description = "[起始小节, 结束小节]，两头都含，一次最多 256 小节。给了就按小节算 tick 区间并回小节文字视图；和 tick_from / tick_to 互斥"
    )]
    #[serde(default)]
    pub bars: Option<[u32; 2]>,
}

impl From<GetNotes> for Op {
    fn from(a: GetNotes) -> Self {
        Op::GetNotes {
            tick_from: a.tick_from,
            tick_to: a.tick_to,
            layer_from: a.layer_from,
            layer_to: a.layer_to,
            ticks_per_bar: a.ticks_per_bar,
            bars: a.bars,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetLayers {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "起始层号，含；省略 = 0")]
    #[serde(default)]
    pub layer_from: Option<u32>,
    #[schemars(description = "结束层号，含；省略 = 最后一层。层多时给个窗口，整张表按层数算字节")]
    #[serde(default)]
    pub layer_to: Option<u32>,
}

impl From<GetLayers> for Op {
    fn from(a: GetLayers) -> Self {
        Op::GetLayers {
            layer_from: a.layer_from,
            layer_to: a.layer_to,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetNotes {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(
        description = "要写的格子。一次调用 = 一次撤销；整批要么全收要么全拒（先校验后落笔）"
    )]
    pub cells: Vec<CellInput>,
    #[schemars(description = "写进撤销历史的那句话，省略则用默认描述")]
    #[serde(default)]
    pub label: Option<String>,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<SetNotes> for Op {
    fn from(a: SetNotes) -> Self {
        Op::SetNotes {
            cells: a.cells.into_iter().map(CellNote::from).collect(),
            label: a.label,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RemoveCells {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "要摘掉的格子坐标；空格子不算改动")]
    pub cells: Vec<CellRef>,
    #[schemars(description = "写进撤销历史的那句话，省略则用默认描述")]
    #[serde(default)]
    pub label: Option<String>,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<RemoveCells> for Op {
    fn from(a: RemoveCells) -> Self {
        Op::RemoveCells {
            cells: a.cells.into_iter().map(|c| (c.tick, c.layer)).collect(),
            label: a.label,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BatchFieldArgs {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "要批量改哪个字段")]
    pub field: BatchField,
    #[schemars(description = "\"set\" = 直接赋值；\"add\" = 在现值上加，越界由服务端夹取并回报")]
    pub mode: String,
    #[schemars(description = "写入或叠加的数，边界由 field 决定")]
    pub value: i32,
    #[schemars(description = "作用范围：all / selection / rect。rect 只动范围内已有音符的格子")]
    pub scope: Scope,
    #[schemars(description = "写进撤销历史的那句话，省略则用默认描述")]
    #[serde(default)]
    pub label: Option<String>,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<BatchFieldArgs> for Op {
    fn from(a: BatchFieldArgs) -> Self {
        Op::BatchField {
            field: a.field,
            mode: a.mode,
            value: a.value,
            scope: a.scope,
            label: a.label,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Transpose {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "作用范围：all / selection / rect。rect 只动范围内已有音符的格子")]
    pub scope: Scope,
    #[schemars(description = "半音数，可为负；越界夹到 0..=87 并在回执里说明")]
    pub semitones: i32,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<Transpose> for Op {
    fn from(a: Transpose) -> Self {
        Op::Transpose {
            scope: a.scope,
            semitones: a.semitones,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Select {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(
        description = "选什么。rect 里 copy=true 只复制不摘出（对应 Alt 框选），additive=true 并入现有选区（对应 Ctrl 加选）"
    )]
    pub target: SelectTarget,
    #[schemars(description = "乐观并发令牌；后台写必填")]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<Select> for Op {
    fn from(a: Select) -> Self {
        Op::Select {
            target: a.target,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LayerAdd {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "插入位置（0 起算）；省略 = 追加到末尾")]
    #[serde(default)]
    pub at: Option<u32>,
    #[schemars(description = "层名；省略则用默认名")]
    #[serde(default)]
    pub name: Option<String>,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<LayerAdd> for Op {
    fn from(a: LayerAdd) -> Self {
        Op::LayerAdd {
            at: a.at,
            name: a.name,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LayerIndex {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "层号，0 起算")]
    pub index: u32,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<LayerIndex> for Op {
    fn from(a: LayerIndex) -> Self {
        Op::LayerRemove {
            index: a.index,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LayerUpdate {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "层号，0 起算")]
    pub index: u32,
    #[schemars(description = "要改的属性，省略的字段不动")]
    pub set: LayerSet,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<LayerUpdate> for Op {
    fn from(a: LayerUpdate) -> Self {
        Op::LayerUpdate {
            index: a.index,
            set: a.set,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LayerMove {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "源层号，0 起算")]
    pub from: u32,
    #[schemars(description = "目标层号，0 起算；音符跟着层一起走")]
    pub to: u32,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<LayerMove> for Op {
    fn from(a: LayerMove) -> Self {
        Op::LayerMove {
            from: a.from,
            to: a.to,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetMetaArgs {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "要改的元数据，省略的字段不动；一个都没给会被拒而不是空成功")]
    pub meta: SetMeta,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

impl From<SetMetaArgs> for Op {
    fn from(a: SetMetaArgs) -> Self {
        Op::SetMeta {
            meta: a.meta,
            expect_rev: a.expect_rev,
        }
    }
}

/// `undo` / `redo` 共用：一个形状对应两个方向，所以转成哪个 `Op` 由调用处定。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct Steps {
    #[schemars(description = "稳定 id 或当前位置 index，二选一；省略 = 当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(description = "步数；省略 = 1 步")]
    #[serde(default)]
    pub steps: Option<u32>,
    #[schemars(description = "乐观并发令牌；后台写必填")]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ClearSong {
    #[schemars(description = "稳定 id 或当前位置 index；清空只允许当前标签")]
    #[serde(default)]
    pub tab: Option<TabRef>,
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
    #[schemars(description = "歌曲有未保存改动时必须为 true：清空会连改动和全部撤销历史一起丢掉")]
    #[serde(default)]
    pub discard_changes: bool,
}

impl From<ClearSong> for Op {
    fn from(a: ClearSong) -> Self {
        Op::ClearSong {
            expect_rev: a.expect_rev,
            discard_changes: a.discard_changes,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Seek {
    #[schemars(description = "落点 tick，可以是小数（落在两格之间）；负数被拒")]
    pub tick: f64,
}

impl From<Seek> for Op {
    fn from(a: Seek) -> Self {
        Op::Playback(Playback::Seek { tick: a.tick })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetLoop {
    #[schemars(description = "循环开关；只改这一项，不动其他元数据")]
    pub enabled: bool,
}

impl From<SetLoop> for Op {
    fn from(a: SetLoop) -> Self {
        Op::Playback(Playback::SetLoop { enabled: a.enabled })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OpenPath {
    #[schemars(
        description = "要开的 .nbs / .schematic 绝对路径，必须落在编辑器允许的目录里（启动时的 --control-dir）"
    )]
    pub path: String,
    #[schemars(description = "当前工程有未保存改动时必须为 true：开档会丢掉改动和全部撤销历史")]
    #[serde(default)]
    pub discard_changes: bool,
}

impl From<OpenPath> for Op {
    fn from(a: OpenPath) -> Self {
        Op::OpenPath {
            path: a.path,
            discard_changes: a.discard_changes,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SavePath {
    #[schemars(description = "目标 .nbs 路径，必须落在编辑器允许的目录里")]
    pub path: String,
    #[schemars(
        description = "存成的 .nbs 版本号；省略 = 当前歌曲的保存版本。越界会被拒而不是偷偷降版本"
    )]
    #[serde(default)]
    pub version: Option<u8>,
    #[schemars(
        description = "目标已存在且不是本工程自己的文件时必须为 true；存回当前文件名不需要它"
    )]
    #[serde(default)]
    pub overwrite: bool,
}

impl From<SavePath> for Op {
    fn from(a: SavePath) -> Self {
        Op::SavePath {
            path: a.path,
            version: a.version,
            overwrite: a.overwrite,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ImportMidi {
    #[schemars(description = "要导入的 .mid 路径，必须落在编辑器允许的目录里")]
    pub path: String,
    #[schemars(description = "当前工程有未保存改动时必须为 true：导入会替换整首歌并清空撤销历史")]
    #[serde(default)]
    pub discard_changes: bool,
    #[schemars(
        description = "传统导入的选项；整个字段省略 = 复刻人打开导入窗时的基线（标量取自偏好、通道/鼓映射按这个文件和当前 GM 表现算）。给了哪个字段就覆盖哪个，没给的仍走基线——注意出厂默认值不是基线，别拿它当'不设'"
    )]
    #[serde(default)]
    pub options: Option<ImportMidiOptions>,
    #[schemars(
        description = "增强导入那一套；省略 = 走传统路径。要启用必须显式写 enabled=true，否则只给子字段会被拒（因为那些字段根本不会被读到）"
    )]
    #[serde(default)]
    pub enhanced: Option<ImportEnhancedOptions>,
    #[schemars(
        description = "true = 把这次的通道/鼓映射写回 GM 表、标量存进偏好——这是改用户的长期设置，所以省略算 false。只有传统路径会写回，enhanced.enabled=true 时这个字段无效（和界面一致）"
    )]
    #[serde(default)]
    pub remember: Option<bool>,
}

impl From<ImportMidi> for Op {
    fn from(a: ImportMidi) -> Self {
        Op::ImportMidi {
            path: a.path,
            discard_changes: a.discard_changes,
            options: a.options,
            enhanced: a.enhanced,
            remember: a.remember,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InspectMidi {
    #[schemars(description = "要看的那个 .mid 路径，必须落在编辑器允许的目录里")]
    pub path: String,
    #[schemars(
        description = "按哪套选项预演；形状和 import_midi.options 完全一致，省略 = 编辑器基线。想验证'就这么导会得到什么'，把准备发给 import_midi 的 options 原样发过来"
    )]
    #[serde(default)]
    pub options: Option<ImportMidiOptions>,
    #[schemars(description = "增强导入的预演，形状和 import_midi.enhanced 一致；省略 = 预演传统路径")]
    #[serde(default)]
    pub enhanced: Option<ImportEnhancedOptions>,
}

impl From<InspectMidi> for Op {
    fn from(a: InspectMidi) -> Self {
        Op::InspectMidi {
            path: a.path,
            options: a.options,
            enhanced: a.enhanced,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Export {
    #[schemars(description = "输出路径，必须落在编辑器允许的目录里")]
    pub path: String,
    #[schemars(description = "格式：wav / midi / png")]
    pub format: ExportFormat,
    #[schemars(description = "采样率，仅 wav 有意义；省略 = 原版默认")]
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[schemars(description = "声道数，仅 wav 有意义；省略 = 原版默认")]
    #[serde(default)]
    pub channels: Option<u16>,
    #[schemars(description = "是否包含锁定层；省略 = 按原版默认排除锁定层")]
    #[serde(default)]
    pub include_locked: Option<bool>,
    #[schemars(description = "目标文件已存在时必须为 true，否则直接报错")]
    #[serde(default)]
    pub overwrite: bool,
}

impl From<Export> for Op {
    fn from(a: Export) -> Self {
        Op::Export {
            path: a.path,
            format: a.format,
            sample_rate: a.sample_rate,
            channels: a.channels,
            include_locked: a.include_locked,
            overwrite: a.overwrite,
        }
    }
}

/// `job_status` / `job_cancel` 共用：同一个 id 对应两个动作，转成哪个 `Op` 由调用处定。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct JobId {
    #[schemars(description = "export 回执里带回来的任务句柄")]
    pub id: u64,
}

/// `transcribe_probe` 没有参数：它只是问本机转录环境现在是什么样子。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TranscribePrepareEnv {
    #[schemars(
        description = "装 cu128（CUDA）构建还是 CPU 构建；省略 = 用编辑器「音频转 MIDI」窗口里当前那个勾。这一步要下几百 MB"
    )]
    #[serde(default)]
    pub cuda: Option<bool>,
}

/// `transcribe_prepare_model` 与 `transcribe_audio` 共用同一份 `options`。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TranscribePrepareModel {
    #[schemars(
        description = "要预检的模型与解码参数；省略哪个字段就用窗口里当前那一份。这一步会按名字下权重（CC BY-NC 4.0），也要人勾过许可"
    )]
    #[serde(default)]
    pub options: Option<TranscribeOptions>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TranscribeAudio {
    #[schemars(
        description = "要识别的音频路径，必须落在编辑器允许的目录里且真的存在（wav/flac/mp3/ogg/m4a/aac/aif/aiff/opus/webm）"
    )]
    pub audio: String,
    #[schemars(
        description = "模型与解码参数；省略哪个字段就用窗口里当前那一份。第一次跑这个模型会先下权重"
    )]
    #[serde(default)]
    pub options: Option<TranscribeOptions>,
    #[schemars(
        description = "把识别出的 MIDI 另存到这个路径（.mid/.midi，必须在允许的目录里）。不给也能拿到产物路径，但那个目录在编辑器自己的 .work/transcription 下，import_midi 读不到——要接着导入就得给它"
    )]
    #[serde(default)]
    pub save_midi: Option<String>,
    #[schemars(description = "save_midi 的目标文件已存在时必须为 true，否则直接报错")]
    #[serde(default)]
    pub overwrite: bool,
}

impl From<TranscribePrepareEnv> for Op {
    fn from(a: TranscribePrepareEnv) -> Self {
        Op::TranscribePrepareEnv { cuda: a.cuda }
    }
}

impl From<TranscribePrepareModel> for Op {
    fn from(a: TranscribePrepareModel) -> Self {
        Op::TranscribePrepareModel {
            options: a.options.unwrap_or_default(),
        }
    }
}

impl From<TranscribeAudio> for Op {
    fn from(a: TranscribeAudio) -> Self {
        Op::TranscribeAudio {
            audio: a.audio,
            options: a.options.unwrap_or_default(),
            save_midi: a.save_midi,
            overwrite: a.overwrite,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cell(key: serde_json::Value) -> Result<CellInput, serde_json::Error> {
        serde_json::from_value(json!({
            "tick": 12, "layer": 1, "instrument": 0, "key": key
        }))
    }

    /// 音名这条路的存在理由就是「不必先算键号」，所以两种写法必须落到同一个键上；
    /// 只测能过的写法等于没测——真正会出事的是那些看着像音名的拼法。
    #[test]
    fn note_names_and_numbers_land_on_the_same_key() {
        for (text, want) in [
            ("F#4", 45),
            ("C4", 39),
            ("Bb4", 49),
            ("F♯3", 33),
            ("B♭4", 49),
            ("c4", 39),
            (" C4 ", 39),
        ] {
            assert_eq!(cell(json!(text)).unwrap().key.0, want, "{text:?}");
        }
        assert_eq!(cell(json!(45)).unwrap().key.0, 45);
        // 边界：0 和 KEY_MAX 都收。
        assert_eq!(cell(json!(0)).unwrap().key.0, 0);
        assert_eq!(cell(json!(87)).unwrap().key.0, 87);

        // 折出来的键号和 `Op` 那侧一致，音名不会在搬运时掉回默认键。
        let Op::SetNotes { cells, .. } = Op::from(SetNotes {
            tab: None,
            cells: vec![cell(json!("F#4")).unwrap()],
            label: None,
            expect_rev: None,
        }) else {
            panic!()
        };
        assert_eq!(cells[0].key, 45);
    }

    /// 每一条都要能读出一句人话：这些字符串是 AI 唯一能拿到的纠错依据。
    #[test]
    fn unreadable_pitches_are_rejected_not_guessed() {
        for bad in ["Cb4", "B#3", "C", "H4", "C4x", "D#8", "A-1"] {
            let err = cell(json!(bad)).unwrap_err().to_string();
            assert!(err.contains("认不出来"), "{bad:?} → {err}");
            assert!(err.contains(bad), "报错要原样引用收到的写法：{err}");
        }
        // 数字超窗、小数、以及既不是数也不是串。
        let err = cell(json!(88)).unwrap_err().to_string();
        assert!(err.contains("超出 0..=87"), "{err}");
        assert!(cell(json!(3.5)).unwrap_err().to_string().contains("整数"));
        for bad in [json!(null), json!(true), json!([39])] {
            let err = cell(bad.clone()).unwrap_err().to_string();
            assert!(err.contains("要么给"), "{bad} → {err}");
        }
    }

    /// AI 是先读 schema 再拼参数的，所以 schema 必须同时把两种写法说出来；
    /// 只在描述里写「也收音名」而 `anyOf` 缺席，客户端的校验会先把字符串拒了。
    #[test]
    fn pitch_schema_offers_both_spellings() {
        let schema = serde_json::to_string(&schemars::schema_for!(SetNotes)).unwrap();
        assert!(schema.contains("\"anyOf\""), "{schema}");
        assert!(schema.contains("\"maximum\":87"), "{schema}");
        assert!(schema.contains("音名"), "{schema}");
        assert!(schema.contains("C4"), "schema 里要有能照抄的例子：{schema}");
    }
}
