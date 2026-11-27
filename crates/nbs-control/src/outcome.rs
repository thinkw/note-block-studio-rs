//! 结构化回执。界面里「结果」是一行状态栏文字加一个可选弹窗，外部调用方两者都看不见，
//! 所以每个调用必须自己把结论说清楚：改了几格、新的 `rev`、失败原因用哪个错误码。
//!
//! 错误码是对外契约，字符串一旦定下不要随便换；文案（`message`）可以随时改。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 稳定错误码。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 参数不合法（越界、缺字段、格式不认识）。
    BadArgument,
    /// 目标不存在（格子、图层、任务 id）。
    NotFound,
    /// 该层被锁定（含「有别的层独奏所以这层实际静音」）。
    LockedLayer,
    /// `expect_rev` 与当前 `rev` 不符：有人在你的调用之间改了工程。
    RevConflict,
    /// 客户端持有的标签会话号已经失效（标签被关闭）。
    TabGone,
    /// 后台标签默认只读；设置里开启后台写后才接受文档级操作。
    ParkedTabReadonly,
    /// 播放、文件、任务等全局交互只能打在当前标签上。
    RequiresActiveTab,
    /// 后台写必须带 expect_rev，禁止无锁覆盖。
    ExpectRequired,
    /// 关闭脏标签必须显式确认丢弃，MCP 没有默认应答方。
    NeedsConfirm,
    /// 有模态框挂着，写操作被挡（只读操作不受影响）。
    ModalOpen,
    /// 有窗口开着且它握着这次操作的草稿态，外部提交会互相打架。
    WindowOpen,
    /// 同类任务还在跑，或者并发任务已满。
    Busy,
    /// 这条路在外部控制里没有等价语义（例如必须靠鼠标拖动的流程）。
    Unsupported,
    /// 被服务端策略拒绝（路径白名单、危险动作未开）。
    NotAllowed,
    /// 磁盘 / 网络失败。
    Io,
    /// 数值超出模型允许范围，且调用方没要求夹取。
    OutOfRange,
    /// 等待主线程应答超时。
    Timeout,
    /// 服务已关闭 / 连接断。
    Closed,
    /// 兜底：内部异常，细节在 message。
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadArgument => "bad_argument",
            Self::NotFound => "not_found",
            Self::LockedLayer => "locked_layer",
            Self::RevConflict => "rev_conflict",
            Self::TabGone => "tab_gone",
            Self::ParkedTabReadonly => "parked_tab_readonly",
            Self::RequiresActiveTab => "requires_active_tab",
            Self::ExpectRequired => "expect_required",
            Self::NeedsConfirm => "needs_confirm",
            Self::ModalOpen => "modal_open",
            Self::WindowOpen => "window_open",
            Self::Busy => "busy",
            Self::Unsupported => "unsupported",
            Self::NotAllowed => "not_allowed",
            Self::Io => "io",
            Self::OutOfRange => "out_of_range",
            Self::Timeout => "timeout",
            Self::Closed => "closed",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl ErrorInfo {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// 每次写操作之后的状态快照，供调用方判断「真的落到我这个版本上了吗」。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeReport {
    /// 本次实际变化的格子数（diff 里非 noop 的条目）。
    pub cells_changed: usize,
    /// 本次变化的图层数（图层增删改名等整表变化按 1 计）。
    pub layers_changed: usize,
    pub meta_changed: bool,
    /// 变更后的内容版本号；与 `nbs-core` 的 `song.rev` 同源。
    pub rev: u64,
    /// 开档 / 新建才变的会话号；用它识别「工程被换掉了」。
    pub song_session: u64,
    pub dirty: bool,
    pub history_len: usize,
    pub history_cursor: usize,
    /// 给人看的一行说明，等价状态栏那条文字；不要拿它做判断。
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: u64,
    /// 线上叫 `job_kind`：`Data` 是内部 tag 的枚举、tag 名就是 `kind`，
    /// 这个字段若同名摊平出去会写成两个 `kind`，客户端读回来直接报
    /// `duplicate field` —— 序列化端不报错，坏在对面，所以必须改名。
    #[serde(rename = "job_kind")]
    pub kind: String,
    pub status: JobStatus,
    /// 0.0..=1.0；无法估的活固定 0.0。
    pub progress: f32,
    pub message: String,
    pub started_unix: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_unix: Option<i64>,
    /// 完成后的产物说明（路径、时长、峰值等），失败时留在 `message`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteCell {
    pub tick: u32,
    pub layer: u32,
    pub instrument: u8,
    pub key: u8,
    pub velocity: u8,
    pub panning: u8,
    pub pitch: i16,
    /// true = 超出 Minecraft 原生可播放范围（key 33..=57 或用了自定义音色）。
    pub out_of_range: bool,
}

/// 一段连续发声：同层、同音色、同键、tick 一格接一格。`get_notes` 的 `runs` 视图用它，
/// 读整段延音时比逐格省一大半字节——代价是段内每一格只剩起止两个力度值。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteRun {
    pub layer: u32,
    pub instrument: u8,
    pub key: u8,
    /// 闭区间：段首格。
    pub tick_from: u32,
    /// 闭区间：段尾格。连续段里 `tick_to - tick_from + 1 == cells`。
    pub tick_to: u32,
    pub cells: usize,
    /// 段首与段尾的力度。中间怎么分档这段看不出来，要逐格得用 `cells` 视图。
    pub velocity_from: u8,
    pub velocity_to: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarOverview {
    /// 小节号，从 0 起算。
    pub bar: u32,
    /// 这一小节的 tick 闭区间，和界面竖线对齐。
    pub tick_from: u32,
    pub tick_to: u32,
    /// 这一小节里的真实格数（`text` 可能因为打印上限而少列几格）。
    pub cells: usize,
    /// 形如 `0:C4(2) 6:G4(2)`：tick 绝对坐标、音名、音色号。
    /// 超出打印上限时以 `…` 收尾，`cells` 仍是真实条数。
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerInfo {
    pub index: u32,
    pub name: String,
    pub volume: u8,
    pub stereo: u8,
    /// "normal" | "locked" | "solo"
    pub lock: String,
    /// 综合 solo 之后的**生效**锁定态：有别的层独奏时，普通层也算锁定。
    pub effectively_locked: bool,
    pub notes: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentInfo {
    pub index: u8,
    pub name: String,
    pub custom: bool,
    /// 自定义音色的采样文件；内置音色为 null。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// 采样是否真的加载到了；缺采样的音符不会发声，外部调用方必须看得见这件事。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_ready: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionInfo {
    /// 浮动选区是否为空。
    pub empty: bool,
    pub notes: usize,
    /// 选区左上角在歌曲里的绝对坐标（空选区为 null）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryInfo {
    pub can_undo: bool,
    pub can_redo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redo_label: Option<String>,
    pub cursor: usize,
    pub len: usize,
    pub capacity: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    /// 1 = 可直接进 schematic；2 = 速度需要修；0 = 有超窗或自定义音色音符。
    pub compat: u8,
    pub notes_out_of_mc_range: usize,
    pub notes_custom_instrument: usize,
    pub pitched_notes: usize,
    pub tempo: f32,
    pub bpm: f32,
    /// 用了但采样缺失的音色索引：这些音符导出后是静音的。
    pub missing_samples: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    /// 稳定会话号；标签关闭后失效，不会因重排改变。
    pub id: u64,
    /// 当前标签条位置，从 0 起算。
    pub index: u32,
    pub title: String,
    pub filename: Option<String>,
    pub dirty: bool,
    pub rev: u64,
    pub total_blocks: usize,
    pub song_length: u32,
    pub layer_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub app_version: String,
    pub protocol: u32,
    pub path: Option<String>,
    pub dirty: bool,
    pub rev: u64,
    pub song_session: u64,
    pub name: String,
    pub author: String,
    pub description: String,
    pub tempo: f32,
    pub bpm: f32,
    pub time_signature: u8,
    pub loop_enabled: bool,
    pub loop_start: u16,
    pub loop_max: u8,
    pub layers: usize,
    pub notes: usize,
    pub length_ticks: u32,
    pub save_version: u8,
    pub first_custom_index: u8,
    #[serde(default)]
    pub active_tab: Option<u64>,
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    /// 播放位置（tick，浮点）与是否在播；收起标签没有交互态，返回 null。
    pub playhead: Option<f64>,
    pub playing: Option<bool>,
    pub selection: Box<SelectionInfo>,
    pub diagnostics: Box<Diagnostics>,
    /// 未答完的模态框数量；收起标签返回 null。非 0 时写操作会被 `modal_open` 挡下。
    pub modal_count: Option<usize>,
    /// 界面**当前开着的那一扇面板**的名字（`WindowKind` 的枚举名，如 `SongInfo`），没开就 null。
    ///
    /// 它不是「哪个窗口」的身份，别拿它认工程：那是编辑器里唯一的一扇程序窗口，
    /// 这里报的是程序窗**里面**摊开的面板/模态窗。认身份看 `path` + `pid`。
    pub open_window: Option<String>,
    /// 答话这一方的进程号：`path` 认文件，这一格认进程。旧版编辑器不回，默认 0 = 没说。
    #[serde(default)]
    pub pid: u32,
}

/// 成功回执的数据体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Data {
    Empty,
    Pong {
        app_version: String,
        protocol: u32,
        rev: u64,
        song_session: u64,
        /// 答话这一方的进程号。同一台机器开着好几个编辑器时，这是唯一不会说谎的身份——
        /// `song_session` 只说「这份工程被换过几次」，`path` 另存就会变，两个都认不出「是哪个进程」。
        /// 旧版编辑器不回这一格，所以默认 0 = 对面没说。
        #[serde(default)]
        pid: u32,
    },
    Snapshot(Box<Snapshot>),
    Tabs {
        tabs: Vec<TabInfo>,
        active_tab: Option<u64>,
    },
    Notes {
        cells: Vec<NoteCell>,
        /// 因为超过上限而被截断时为 true，调用方得知道自己在看部分结果。
        truncated: bool,
        limit: usize,
        /// 范围里的真实格数（`cells` 可能被上限截短）。有它才谈得上分页：
        /// 不用再从文字里数「我是不是少读了」。
        total: usize,
        /// 小节文字视图：请求里给了 `bars` 或 `ticks_per_bar` 才非空。
        /// 名字和入参的 `bars`（一对小节号）刻意错开——同一个词指两样形状会读错。
        bar_overview: Vec<BarOverview>,
    },
    /// 同样一段格子的列式写法：字段名每列只出现一次，整段读下来比 `notes` 省一大半字节。
    /// 八条数组等长，第 `i` 项合起来就是第 `i` 格；顺序和 `notes` 一致（tick 升序、同 tick 按层）。
    NoteColumns {
        tick: Vec<u32>,
        layer: Vec<u32>,
        instrument: Vec<u8>,
        key: Vec<u8>,
        velocity: Vec<u8>,
        panning: Vec<u8>,
        pitch: Vec<i16>,
        out_of_range: Vec<bool>,
        truncated: bool,
        limit: usize,
        total: usize,
    },
    /// 延音段摘要：见 [`NoteRun`]。段数与格数各自都有真实值，段数少不等于格子少。
    NoteRuns {
        runs: Vec<NoteRun>,
        truncated: bool,
        limit: usize,
        total_runs: usize,
        total_cells: usize,
    },
    Layers {
        layers: Vec<LayerInfo>,
    },
    /// 单层回声：改完了只报被改的那一层，别每次都把整张层表推回去。
    /// `get_layers` 才给 [`Data::Layers`] 的清单。
    Layer {
        layer: LayerInfo,
    },
    Selection(Box<SelectionInfo>),
    Instruments {
        instruments: Vec<InstrumentInfo>,
        /// Minecraft 原生可播的键号闭区间 `[min, max]`。放在这而不是让调用方去查文档：
        /// 「能不能在 MC 里响」的决定权和音色表同时在手里，才不会有人拿 33..=57 之外的
        /// 键号导出一堆静音音符盒。这里给的是编辑器自己判 `out_of_range` 用的同一对数。
        mc_key_range: [u8; 2],
    },
    Diagnostics(Box<Diagnostics>),
    History(Box<HistoryInfo>),
    Job(Box<JobInfo>),
    Jobs {
        jobs: Vec<JobInfo>,
    },
    Actions {
        actions: Vec<ActionInfo>,
    },
    /// 逃生舱动作的回执：动作名 + 它自己给出的说明。
    Action {
        action: String,
        note: String,
    },
    /// 文件类操作（打开 / 保存 / 导入）的结果说明。
    File {
        path: String,
        file_kind: String,
        notes: usize,
    },
    /// MIDI 导入的核对单：`inspect_midi` 给的是「会导成什么」，`import_midi` 给的是
    /// 「导成了什么」，同一个形状才比得动。
    MidiPlan(Box<crate::midi::MidiPlan>),
}

/// 逃生舱里一条动作的自述。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionInfo {
    pub name: String,
    pub description: String,
    /// 期望的 `args` 形状，例：`{"tick":"u32","layer":"u32"}`；无参动作给 null。
    pub args: Option<Value>,
    /// true = 会改工程（撤销/写权限检查用）。
    pub mutating: bool,
}

/// 统一回执。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Data>,
    /// 写操作必带；只读操作给一份当前状态，省一次 `GetSnapshot`。
    pub report: ChangeReport,
}

impl Outcome {
    pub fn ok(data: Data, report: ChangeReport) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
            report,
        }
    }

    /// 成功但无数据体。
    pub fn done(report: ChangeReport) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(Data::Empty),
            report,
        }
    }

    pub fn err(error: ErrorInfo, report: ChangeReport) -> Self {
        Self {
            ok: false,
            error: Some(error),
            data: None,
            report,
        }
    }

    pub fn error_code(&self) -> Option<ErrorCode> {
        self.error.as_ref().map(|e| e.code)
    }
}
