//! MCP 那一侧的入参结构：字段和 [`nbs_control::Op`] 的变体逐一对齐，转换是纯搬运。
//!
//! 为什么要多这一层：工具面要的是「每个参数都带说明的 JSON Schema」，而线协议要的是
//! 紧凑稳定的字节。schemars 不读 Rust 文档注释，所以给人看的说明写在这里的
//! `#[schemars(description = ...)]` 里 —— 这些句子是 AI 唯一能读到的口径，
//! 改了 `Op` 的语义就必须同时改这里。

use schemars::JsonSchema;
use serde::Deserialize;

use nbs_control::{BatchField, CellNote, ExportFormat, LayerSet, Op, Scope, SelectTarget, SetMeta};

/// `run_action` 逃生舱的一条调用：动作名 + 它的参数原样。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RunAction {
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

/// 只带 `expect_rev` 的写操作（把浮动选区落回歌曲）。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RevOnly {
    #[schemars(
        description = "乐观并发令牌：填上一次回执里的 rev；省略 = 不校验，直接覆盖当前状态"
    )]
    #[serde(default)]
    pub expect_rev: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetNotes {
    #[schemars(description = "起始 tick，含")]
    pub tick_from: u32,
    #[schemars(description = "结束 tick，含")]
    pub tick_to: u32,
    #[schemars(description = "起始层号，含；省略 = 0")]
    #[serde(default)]
    pub layer_from: Option<u32>,
    #[schemars(description = "结束层号，含；省略 = 最后一层")]
    #[serde(default)]
    pub layer_to: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetNotes {
    #[schemars(
        description = "要写的格子。一次调用 = 一次撤销；整批要么全收要么全拒（先校验后落笔）"
    )]
    pub cells: Vec<CellNote>,
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
            cells: a.cells,
            label: a.label,
            expect_rev: a.expect_rev,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RemoveCells {
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
    #[schemars(
        description = "选什么。rect 里 copy=true 只复制不摘出（对应 Alt 框选），additive=true 并入现有选区（对应 Ctrl 加选）"
    )]
    pub target: SelectTarget,
}

impl From<Select> for Op {
    fn from(a: Select) -> Self {
        Op::Select { target: a.target }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LayerAdd {
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

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct Steps {
    #[schemars(description = "步数；省略 = 1 步")]
    #[serde(default)]
    pub steps: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ClearSong {
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetLoop {
    #[schemars(description = "循环开关；只改这一项，不动其他元数据")]
    pub enabled: bool,
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
}

impl From<ImportMidi> for Op {
    fn from(a: ImportMidi) -> Self {
        Op::ImportMidi {
            path: a.path,
            discard_changes: a.discard_changes,
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct JobId {
    #[schemars(description = "export 回执里带回来的任务句柄")]
    pub id: u64,
}
