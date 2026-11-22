//! nbs-mcp — 把**正在运行的那个 nbs-rs 编辑器窗口**暴露成 MCP 工具面。
//!
//! 分工：编辑器主线程负责真正动手（`nbs-control` 的服务端 + UI 收件箱），本 crate 只做
//! stdio↔回环 TCP 的翻译，自己不持有任何工程状态。所以：
//!
//! - 编辑器没起来 → 工具照样列得出来，每条回执 `transport` 说清「连不上」，不假装成功；
//! - stdout 只走 MCP 协议，人和日志一律 stderr（`main.rs`）；
//! - 每个入参的口径写在 [`params`]，每条工具的一句话说明写在本文件的方法文档注释上。
//!
//! 一次调用的形状固定是 [`Reply`]：`outcome` 是编辑器给的结构化回执（含 `report` 里的
//! `rev` / `song_session` / `dirty`），`transport` 只在桥接这一层出问题时才有。
//! **两层都要看**，别只判 `ok`——「桥接发出去了」和「编辑器改成了」是两件事。
//! 两层里任何一层答「不」，MCP 那侧的 `isError` 都是 true（见 [`to_result`]），
//! 但整份 [`Reply`] 仍然留在 `structuredContent` 里，错误码照读。

mod connect;
mod params;

/// 一次性命令行调用（`--call`）的名字表和输出。公开成模块，因为 `--call` 的
/// 退出码和渲染口径是给外部脚本读的契约。
pub mod call;

use std::sync::{Arc, Mutex};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use schemars::JsonSchema;
use serde::Serialize;

pub use connect::{connect, parse_args, ArgsSource, CallRequest, Command, Endpoint, USAGE};
use nbs_control::{ControlClient, Op, Outcome, Playback};
pub use params::{
    BatchFieldArgs, CellRef, ClearSong, Export, GetNotes, ImportMidi, InspectMidi, JobId, LayerAdd,
    LayerIndex, LayerMove, LayerUpdate, OpenPath, RemoveCells, RevOnly, RunAction, SavePath, Seek,
    Select, SetLoop, SetMetaArgs, SetNotes, Steps, Transpose,
};

/// 一条工具回执。两个失败层各自点名自己的范围。
///
/// `JsonSchema` 不是给入参用的：`Json<Reply>` 会让宏把它当**输出** schema 挂到工具上，
/// 客户端据此知道会拿到什么形状，所以这里的字段说明和入参一样是口径。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Reply {
    /// 编辑器那一侧的结构化回执，原样转自 `nbs-control` 的 `Outcome`：
    /// `ok` / `error.code` / `data` / `report{rev, song_session, dirty, cells_changed, ...}`。
    /// 传输层失败时没有这一项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "编辑器给的结构化回执：ok / error.code / data / report（rev、song_session、dirty、cells_changed）。传输层出问题时没有这一项"
    )]
    pub outcome: Option<serde_json::Value>,
    /// 只有桥接这一层出问题才有它（连不上、令牌不对、对端断线、回执超时）。
    /// 有这一项就说明请求**没有**改变编辑器状态，工具会自己重连下一次调用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "只有桥接这一层失败才有：连不上、令牌不对、对端断线或等回执超时。有它就说明请求没碰到编辑器"
    )]
    pub transport: Option<String>,
    /// 这条通道连的是哪个应用，给结论定范围：同一台机器上可能开着好几个编辑器。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "这条通道连的是哪个应用（同一台机器可能开着多个编辑器），结论只对它成立"
    )]
    pub channel: Option<String>,
}

impl Reply {
    fn from_outcome(outcome: Outcome, channel: &str) -> Self {
        Self {
            outcome: serde_json::to_value(&outcome).ok(),
            transport: None,
            channel: Some(channel.to_string()),
        }
    }

    fn transport(text: impl Into<String>) -> Self {
        Self {
            outcome: None,
            transport: Some(text.into()),
            channel: None,
        }
    }

    /// 只有「编辑器答了且答成」才算成功；传输层失败同样算失败，但原因不同。
    pub fn succeeded(&self) -> bool {
        self.transport.is_none()
            && self
                .outcome
                .as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool())
                == Some(true)
    }
}

/// 一条到编辑器的连接。一条连接一次只能问一句，所以用 `Mutex` 串行化；
/// 要并行就再起一个桥接进程，别在一条通道上抢答。
pub struct Bridge {
    client: Arc<Mutex<ControlClient>>,
    channel: String,
}

/// 一条 `Reply` 怎么落成 MCP 的结果。
///
/// **口径（2026-11-21 改）**：编辑器答「不」、桥接没送到，都让 `isError=true`，
/// 同时把整份回执原样留在 `structuredContent` 里。之前无论成败都答成功、只把 `ok:false`
/// 写进 JSON，现场症状是 AI 客户端把那一行当正常返回，看完接着往下写——
/// 「调用失败」这个事实必须走客户端一定会看的那一位，而不是只待在载荷里。
///
/// 两层的区分没有丢：`transport` 有值 = 请求没碰到编辑器，`outcome.ok=false` = 编辑器答了不，
/// 读的还是同一份 JSON，错误码在 `outcome.error.code`。
fn to_result(reply: Reply) -> Result<Json<Reply>, CallToolResult> {
    if reply.succeeded() {
        return Ok(Json(reply));
    }
    let value = serde_json::to_value(&reply).unwrap_or_else(|e| {
        // 序列化失败是桥接自己的 bug，也要当成一条能读到的失败回执发出去。
        serde_json::json!({ "transport": format!("桥接回执序列化失败: {e}") })
    });
    Err(CallToolResult::structured_error(value))
}

/// 工具面的输出形状。人和机器分两条道：`Text` 走 stderr（那里只有给人读的东西），
/// `Json` / `Names` 走 stdout（那是脚本要的产物，混一行说明就 parse 不了）。
/// 原先 `--dump-tools` 只有文字版，想机器读就得把 stderr 整个吞进来再找左括号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolList {
    Text,
    Json,
    Names,
}

/// 把工具面打出来：给人核对「AI 会看到什么口径」，不需要编辑器在场，也不碰 stdio 协议。
///
/// `only` 是给 `--dump-tools <工具>` 用的：全表 39 条、每条带一份 schema，
/// 只想看一条时那输出比正文长十倍。名字不在表里返回 `Ok(false)`，让调用方说清
/// 「这条没有」而不是打一份空表装作成功。
pub fn dump_tools(
    mut out: impl std::io::Write,
    mode: ToolList,
    only: Option<&str>,
) -> std::io::Result<bool> {
    let mut tools = Bridge::tool_router().list_all();
    if let Some(name) = only {
        if !tools.iter().any(|tool| tool.name == name) {
            return Ok(false);
        }
        tools.retain(|tool| tool.name == name);
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    match mode {
        ToolList::Names => {
            for tool in tools {
                writeln!(out, "{}", tool.name)?;
            }
            Ok(true)
        }
        ToolList::Json => {
            let face: Vec<_> = tools
                .into_iter()
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description.unwrap_or_default(),
                        "args": tool.input_schema,
                    })
                })
                .collect();
            let text = serde_json::to_string_pretty(&face)
                .unwrap_or_else(|_| "[]".to_string())
                // 缩进两个空格，和文字版对得上，肉眼扫也能扫。
                .replace("    ", "  ");
            writeln!(out, "{text}")?;
            Ok(true)
        }
        ToolList::Text => {
            writeln!(out, "nbs-mcp 工具面：{} 条", tools.len())?;
            for tool in tools {
                let description = tool.description.clone().unwrap_or_default();
                let schema = serde_json::to_string(&tool.input_schema).unwrap_or_default();
                writeln!(out, "  {}", tool.name)?;
                for line in description.lines() {
                    writeln!(out, "      {line}")?;
                }
                writeln!(out, "      args: {schema}")?;
            }
            Ok(true)
        }
    }
}

#[tool_router]
impl Bridge {
    pub fn new(client: ControlClient) -> Self {
        let app = client.server_app().to_string();
        Self {
            client: Arc::new(Mutex::new(client)),
            channel: if app.is_empty() {
                "nbs-rs".to_string()
            } else {
                app
            },
        }
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// 发一条 `Op` 并等回执，然后按 [`to_result`] 的口径交给 MCP。
    /// 阻塞的网络读放在 `exec` 的 `spawn_blocking` 里，免得占住异步线程。
    async fn call(&self, op: Op) -> Result<Json<Reply>, CallToolResult> {
        to_result(self.exec(op).await)
    }

    /// [`call`](Self::call) 的无包装版本，给 `--call` 用。
    pub async fn exec(&self, op: Op) -> Reply {
        let client = Arc::clone(&self.client);
        let joined = tokio::task::spawn_blocking(move || {
            // 锁中毒只说明上一次持有它时 panic 过，连接本身的状态由对端说了算，
            // 拿回守卫继续用比把整个工具面卡死好。
            let mut guard = client
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.request(op)
        })
        .await;
        match joined {
            Ok(Ok(outcome)) => Reply::from_outcome(outcome, &self.channel),
            Ok(Err(err)) => Reply::transport(err.to_string()),
            Err(_) => Reply::transport("桥接内部：等待编辑器回执的任务被取消了"),
        }
    }

    /// 按计划发一条。校验由 [`call::plan`] 在连端口之前做完，所以这里只管发。
    pub async fn run(&self, plan: call::Plan) -> Reply {
        match plan {
            call::Plan::BridgeInfo => self.bridge_info(),
            call::Plan::Editor(op) => self.exec(op).await,
        }
    }

    /// 这条通道连的是什么（桥接自己答，不敲编辑器）。`server_info` 工具的同一条内容。
    pub fn bridge_info(&self) -> Reply {
        Reply {
            outcome: Some(serde_json::json!({
                "ok": true,
                "channel": self.channel(),
                "bridge": env!("CARGO_PKG_VERSION"),
                "protocol": nbs_control::PROTOCOL_VERSION,
            })),
            transport: None,
            channel: Some(self.channel().to_string()),
        }
    }

    // -------------------------------------------------------------- 通道 / 逃生舱

    /// 探活：确认这条 MCP 通道真的连着某个编辑器窗口，并核对协议版本。
    /// 不带状态改动。别的工具报 `transport` 时先用它判「是编辑器没起，还是参数不对」。
    #[tool]
    async fn ping(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Ping).await
    }

    /// 列出逃生舱 `run_action` 能调的全部界面动作：名字、一句话说明、参数形状，
    /// 以及它要不要 danger / discard_changes 这类显式认错。
    /// 结构化工具没覆盖的按钮都在这里；先调它，别猜动作名。
    #[tool]
    async fn list_actions(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::ListActions).await
    }

    /// 逃生舱：按名字调用一条界面动作，参数原样放进 `args`。
    /// 只接受 `list_actions` 里出现过的名字，未知名字一律拒绝而不是忽略；
    /// 回执里会带回动作自己的提示语和变更量。
    #[tool]
    async fn run_action(&self, Parameters(a): Parameters<RunAction>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 读

    /// 工程概览：文件路径、脏标记、元数据、图层数、音符数、长度、播放头、
    /// `rev` / `song_session`，外加兼容性诊断。想先看清局面就调这个。
    #[tool]
    async fn get_snapshot(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetSnapshot).await
    }

    /// 读一段范围内的音符格子。返回上限由服务端定，`truncated=true` 说明只给了一部分，
    /// 需要缩小 `tick_from..=tick_to` 再读。
    /// 想按小节读 melody：给 `bars:[起,止]`（两头都含，一次最多 256 节），tick 区间由小节号算，
    /// 回执的 `data.bar_overview` 每节一行 `tick:音名(音色)`——键号要心算才会唱，而错的那半音
    /// 只有耳朵能发现，所以这份文字视图由状态栏同一张表生成。
    #[tool]
    async fn get_notes(&self, Parameters(a): Parameters<GetNotes>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 图层清单：层号、名字、音量、左右声道、锁定 / solo 状态。
    #[tool]
    async fn get_layers(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetLayers).await
    }

    /// 当前选区：是否浮动选区、包围盒、音符数。写之前想知道「selection 是哪儿」就看它。
    #[tool]
    async fn get_selection(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetSelection).await
    }

    /// 音色表：内置 0..=19 加自定义槽位，含每个槽位当前挂的音色名，以及
    /// Minecraft 原生可播的键号闭区间 `mc_key_range`。
    /// 写音符前用它确认 `instrument` 号真的存在、音高落在能响的那两个八度里。
    #[tool]
    async fn get_instruments(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetInstruments).await
    }

    /// 兼容性诊断：超出 Minecraft 两个八度的音符、原版没有的音色、锁层引用等。
    /// 只读，不改状态。
    #[tool]
    async fn get_diagnostics(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetDiagnostics).await
    }

    /// 撤销栈：当前游标、长度，以及每一步的说明文字。
    #[tool]
    async fn get_history(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetHistory).await
    }

    // -------------------------------------------------------------- 写

    /// 批量写格子：一次调用 = 一次撤销，整批先校验后落笔，要么全成要么全不动。
    /// `key` 既收键号也收音名（`45` 与 `"F#4"` 等价），拼错的音名会被拒而不是挪到隔壁键。
    /// 回执的 `report.rev` 请留给下一次写操作的 `expect_rev`。
    #[tool]
    async fn set_notes(&self, Parameters(a): Parameters<SetNotes>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 按坐标摘掉若干格子。空格子不算改动，所以 `cells_changed` 可能小于你给的条数。
    #[tool]
    async fn remove_cells(&self, Parameters(a): Parameters<RemoveCells>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 批量改某个字段（音色 / 音高 / 音量 / 声道 / 音分）。
    /// `mode="add"` 在现值上加，越界由服务端夹取并在回执里说明夹了多少，不会静默丢弃。
    #[tool]
    async fn batch_field(&self, Parameters(a): Parameters<BatchFieldArgs>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 整体或局部移调。`scope` 决定动谁：整首歌、当前浮动选区，还是给定矩形内已有音符的格子。
    /// 越界夹到 0..=87 并在回执里说明。
    #[tool]
    async fn transpose(&self, Parameters(a): Parameters<Transpose>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 选区：框选 / 全选 / 反选 / 按音色 / 超出 MC 音域 / 自定义音色。
    /// 摘出的浮动选区会挡住写操作，写完记得 `place_selection` 或用 `select` 取消。
    #[tool]
    async fn select(&self, Parameters(a): Parameters<Select>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 把浮动选区就地落回歌曲，等价于「取消选择但不丢改动」。
    /// 有浮动选区时 `set_notes` 这类写操作会被状态闸挡回来，所以这一步不能省。
    #[tool]
    async fn place_selection(&self, Parameters(a): Parameters<RevOnly>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 加图层：`at` 省略 = 追加到末尾。
    #[tool]
    async fn layer_add(&self, Parameters(a): Parameters<LayerAdd>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 删图层。锁定层的音符删之前先 `get_snapshot` 看一眼，回执会说明丢了几格。
    #[tool]
    async fn layer_remove(&self, Parameters(a): Parameters<LayerIndex>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 改图层属性：名字、音量、声道、锁定状态（normal / locked / solo）。省略的字段不动。
    #[tool]
    async fn layer_update(&self, Parameters(a): Parameters<LayerUpdate>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 移动图层顺序，音符跟着层一起走。
    #[tool]
    async fn layer_move(&self, Parameters(a): Parameters<LayerMove>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 改元数据：名字、作者、说明、速度、拍号、循环。省略的字段不动，
    /// 一个都没给会被拒（避免「空提交也报成功」的假象）。
    #[tool]
    async fn set_meta(&self, Parameters(a): Parameters<SetMetaArgs>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 撤销。`steps` 省略 = 1 步。回执的 `report` 会说清实际退了几步。
    #[tool]
    async fn undo(&self, Parameters(a): Parameters<Steps>) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Undo { steps: a.steps }).await
    }

    /// 重做。`steps` 省略 = 1 步。
    #[tool]
    async fn redo(&self, Parameters(a): Parameters<Steps>) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Redo { steps: a.steps }).await
    }

    /// 清空成一首空歌。工程有未保存改动时必须带 `discard_changes=true`：
    /// 这一步会连改动和全部撤销历史一起丢掉，不带就等于没认错。
    #[tool]
    async fn clear_song(&self, Parameters(a): Parameters<ClearSong>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 播放

    /// 从当前落点起播。落点已经到或越过曲尾时**先归零再播**，回执会说一句归过零：
    /// 刚 `clear_song` / `open_file` 换了一首更短的歌时，不归零就是「按下去立刻停」。
    #[tool]
    async fn play(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Playback(Playback::Play)).await
    }

    /// 暂停，保留播放头位置。
    #[tool]
    async fn pause(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Playback(Playback::Pause)).await
    }

    /// 停止并把播放头送回起点。
    #[tool]
    async fn stop(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Playback(Playback::Stop)).await
    }

    /// 移动播放头。tick 可以是小数（落在两格之间），负数被拒。
    #[tool]
    async fn seek(&self, Parameters(a): Parameters<Seek>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 只改循环开关，不动其他元数据。
    #[tool]
    async fn set_loop(&self, Parameters(a): Parameters<SetLoop>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 文件

    /// 打开一个 .nbs / .schematic 文件替换当前工程，不弹文件框。
    /// 路径必须落在编辑器启动时允许的目录里；有未保存改动时要带 `discard_changes=true`。
    #[tool]
    async fn open_file(&self, Parameters(a): Parameters<OpenPath>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 存回当前文件名。没有文件名时报错，绝不弹「另存为」。
    #[tool]
    async fn save(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::Save).await
    }

    /// 存到指定路径。目标已存在且不是本工程自己的文件时要带 `overwrite=true`；
    /// 存回当前文件名不需要它。
    #[tool]
    async fn save_as(&self, Parameters(a): Parameters<SavePath>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 导入 MIDI 并**替换**当前工程：撤销历史清空，有未保存改动时要带 `discard_changes=true`。
    /// `options` / `enhanced` 省略 = 复刻人打开 MIDI 导入窗时的那份基线（标量取自偏好，
    /// 通道与鼓映射按这个文件和当前 GM 表现算）；要改哪一项就只写哪一项。
    /// `remember=true` 会写回用户的 GM 表和偏好，所以默认不动它，且只有传统路径会写回。
    /// 回执 `data` 是完整的导入核对单（`MidiPlan`），`applied` / `applied_enhanced`
    /// 回显这次**真正生效**的每一个字段——先 `inspect_midi` 预演、再拿同一份选项导，
    /// 两边逐字段比得动。
    #[tool]
    async fn import_midi(&self, Parameters(a): Parameters<ImportMidi>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 只读预演 MIDI 导入：读文件、按给定选项算出一整首歌，然后**把结果丢掉**，
    /// 工程一点不动（`rev` 也不变）。回执和 `import_midi` 同一个形状，用来先看清
    /// 「通道 3 是不是被我标成静音了」「层数会不会超」再动手。
    /// 这一步在主线程算，大文件会给几十毫秒到几百毫秒的等待。
    #[tool]
    async fn inspect_midi(&self, Parameters(a): Parameters<InspectMidi>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 导出 wav / midi / png。重活**不**在这一次调用里做完：回执 `data.id` 是任务句柄，
    /// 之后用 `job_status` 轮询到 done/failed。目标文件已存在时要带 `overwrite=true`；
    /// 路径同样受启动时允许的目录约束。
    #[tool]
    async fn export(&self, Parameters(a): Parameters<Export>) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 任务

    /// 查一个后台任务的状态：回执 `data` 里给 `status`（queued/running/done/failed/cancelled）、
    /// `progress`、`message`，成功完活时 `result` 带产物路径与统计。
    #[tool]
    async fn job_status(&self, Parameters(a): Parameters<JobId>) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::JobStatus { id: a.id }).await
    }

    /// 列出当前全部任务，用来找回句柄（比如桥接重启过）；条目在回执 `data.jobs` 里。
    #[tool]
    async fn job_list(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::JobList).await
    }

    /// 取消任务。已经在写的文件不保证回滚，回执会说实话。
    #[tool]
    async fn job_cancel(&self, Parameters(a): Parameters<JobId>) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::JobCancel { id: a.id }).await
    }

    // -------------------------------------------------------------- 桥接自己

    /// 这条 MCP 通道连的是哪个编辑器、用什么参数连上的。
    /// 不带网络请求——回执只描述桥接这一侧，别把它当成编辑器的状态。
    #[tool]
    async fn server_info(&self) -> Result<Json<Reply>, CallToolResult> {
        to_result(self.bridge_info())
    }
}

#[tool_handler(
    name = "nbs-rs",
    instructions = "本服务驱动一个**正在运行的 nbs-rs（Note Block Studio 移植）编辑器窗口**，\
所有改动都落在那台机器的这个 GUI 进程里，不是内存里的模拟工程。\n\
开局先 get_snapshot 看清工程、rev 和脏标记；写操作带上一次回执里的 expect_rev，\
冲突会报 rev_conflict 而不是覆盖别人的改动。\n\
弹窗和浮动选区会挡写操作：get_snapshot 会说明谁在挡路，用 run_action 的 answer_modal / \
close_window / place_selection 把路清开。\n\
每个结论都要分清两层：transport = 桥接没送到，outcome.ok = 编辑器答了什么。\n\
任何一层答「不」这条调用的 isError 都是 true；失败回执也带完整 JSON，读完 error.code 再决定下一步，\
不要接着往下写。\n\
没被结构化工具覆盖的按钮都在 list_actions 里，按名字走 run_action，未知名字会被拒。"
)]
impl ServerHandler for Bridge {}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_control::{ExportFormat, Scope};

    #[test]
    fn tool_names_are_unique_and_every_tool_says_something() {
        let tools = Bridge::tool_router().list_all();
        assert!(
            tools.len() >= 37,
            "工具面塌了，只剩 {} 条：结构化 Op 有 37 条，一条都不能少",
            tools.len()
        );
        let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        names.sort();
        let dupes: Vec<String> = names
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0].clone())
            .collect();
        assert!(dupes.is_empty(), "重名工具: {dupes:?}");
        for name in ["get_snapshot", "set_notes", "run_action", "export"] {
            assert!(names.iter().any(|n| n == name), "少了 {name}");
        }
        for tool in &tools {
            let description = tool.description.clone().unwrap_or_default();
            assert!(
                !description.trim().is_empty(),
                "{} 没写说明，AI 只能猜",
                tool.name
            );
            // 改成 `Result<Json<Reply>, CallToolResult>` 是为了 `isError`，
            // 但输出 schema 是 `Json<T>` 那一份带来的，掉它就是白丢客户端的校验依据。
            assert!(
                tool.output_schema.is_some(),
                "{} 掉了输出 schema：返回类型必须保住 Json<Reply>",
                tool.name
            );
            let schema = serde_json::to_value(&tool.input_schema).unwrap();
            assert!(
                schema.is_object(),
                "{} 的入参 schema 不是对象，MCP 客户端会拒",
                tool.name
            );
        }
    }

    #[test]
    fn parameter_docs_reach_the_schema() {
        let tools = Bridge::tool_router().list_all();
        let find = |name: &str| {
            tools
                .iter()
                .find(|t| &*t.name == name)
                .cloned()
                .unwrap_or_else(|| panic!("没有工具 {name}"))
        };
        let text = |name: &str| {
            serde_json::to_string(&serde_json::to_value(&find(name).input_schema).unwrap()).unwrap()
        };
        // schemars 不读文档注释，这些句子只能来自 #[schemars(description = ...)]。
        let set_notes = text("set_notes");
        assert!(set_notes.contains("一次调用 = 一次撤销"), "{set_notes}");
        assert!(set_notes.contains("tick 列号"), "格子坐标要带口径");
        // 音名这条路要在 schema 里露头，否则客户端校验先把字符串拒了，AI 只看见「类型不对」。
        assert!(set_notes.contains("anyOf"), "音高两种写法要都声明：{set_notes}");
        let export = text("export");
        assert!(export.contains("wav"), "格式枚举要说清取值");
        assert!(export.contains("overwrite"), "覆盖开关必须出现在 schema 里");
        // 无参工具：schema 是「空对象」而不是 null 或缺 type，客户端才敢发 `{}`。
        let layers = serde_json::to_value(&find("get_layers").input_schema).unwrap();
        assert_eq!(layers["type"], "object", "{layers}");
        assert!(
            layers["properties"]
                .as_object()
                .is_some_and(|p| p.is_empty()),
            "{layers}"
        );
    }

    /// 报错文案里那句「形状看 `--dump-tools set_notes`」必须是真能跑的写法：
    /// 全表 39 条带 schema，为了看一个参数读完整张表，等于没给答案。
    #[test]
    fn tool_face_dumps_whole_or_one_by_one() {
        let mut all = Vec::new();
        assert!(dump_tools(&mut all, ToolList::Json, None).unwrap());
        let all: serde_json::Value = serde_json::from_slice(&all).unwrap();
        assert!(all.as_array().is_some_and(|a| a.len() > 30), "全表该有几十条");

        let mut one = Vec::new();
        assert!(dump_tools(&mut one, ToolList::Json, Some("set_notes")).unwrap());
        let one: serde_json::Value = serde_json::from_slice(&one).unwrap();
        assert_eq!(one.as_array().unwrap().len(), 1, "带名字就只打那一条：{one}");
        assert_eq!(one[0]["name"], "set_notes");
        assert!(one[0]["description"].as_str().is_some_and(|d| !d.is_empty()));
        assert!(one[0]["args"].is_object(), "args 要是 schema 对象：{one}");

        // 名字打错要说「没这条」，不能打一份空表装作成功。
        let mut miss = Vec::new();
        assert!(!dump_tools(&mut miss, ToolList::Json, Some("set_note")).unwrap());
        assert!(miss.is_empty(), "不认的名字不该先写出去：{miss:?}");
    }

    /// 把 `#[tool]` 方法的返回值走一遍 rmcp 真正会做的转换，看线上那两位（`isError` /
    /// `structuredContent`）各是什么。只测 `Reply` 自己的字段测不到这件事。
    fn wire(result: Result<Json<Reply>, CallToolResult>) -> CallToolResult {
        use rmcp::handler::server::tool::IntoCallToolResult;
        match result.into_call_tool_result().expect("路由要收得下这个返回类型") {
            rmcp::model::CallToolResponse::Complete(result) => result,
            other => panic!("这些工具都是一次性答完，不该出 task / input 分支：{other:?}"),
        }
    }

    /// 编辑器答「不」要让客户端一定看得见：只把 `ok:false` 写进 JSON 时，
    /// AI 侧把这一行当正常返回，读完接着往下写（2026-11-21 那条现场记录）。
    #[test]
    fn an_editor_refusal_raises_is_error_and_keeps_the_payload() {
        let refused = Reply::from_outcome(
            Outcome::err(
                nbs_control::ErrorInfo::new(
                    nbs_control::ErrorCode::RevConflict,
                    "有人在你的调用之间改了工程",
                ),
                nbs_control::ChangeReport {
                    rev: 9,
                    ..Default::default()
                },
            ),
            "nbs-rust",
        );
        let result = wire(to_result(refused));
        assert_eq!(result.is_error, Some(true), "答不就要 isError=true");
        let content = result
            .structured_content
            .expect("失败也要带整份回执，错误码不能只活在文字里");
        assert_eq!(content["outcome"]["error"]["code"], "rev_conflict");
        assert_eq!(content["outcome"]["report"]["rev"], 9);
        // 只读 content 文字的客户端同样能看到错误码。
        let text = result.content[0]
            .as_text()
            .unwrap_or_else(|| panic!("第一条内容块该是文字：{:?}", result.content[0]))
            .text
            .clone();
        assert!(text.contains("rev_conflict"), "{text}");
    }

    /// 「桥接没送到」同样要停住调用方，但它点名的原因是 transport，不是编辑器的决定。
    #[test]
    fn a_transport_miss_raises_is_error_without_inventing_an_outcome() {
        let result = wire(to_result(Reply::transport("连不上控制服务")));
        assert_eq!(result.is_error, Some(true));
        let content = result.structured_content.expect("断线也要说清是断线");
        assert_eq!(content["transport"], "连不上控制服务");
        assert!(content.get("outcome").is_none(), "没送到就不该编造回执");
    }

    /// 成功那一路的载荷形状不许顺手变：整份 `Reply` 还在 `structuredContent` 里。
    #[test]
    fn a_success_keeps_is_error_false_and_the_same_payload() {
        let ok = Reply::from_outcome(
            Outcome::done(nbs_control::ChangeReport {
                rev: 7,
                ..Default::default()
            }),
            "nbs-rust",
        );
        let result = wire(to_result(ok));
        assert_eq!(result.is_error, Some(false));
        let content = result.structured_content.expect("成功要给结构化载荷");
        assert_eq!(content["outcome"]["ok"], true);
        assert_eq!(content["outcome"]["report"]["rev"], 7);
        assert_eq!(content["channel"], "nbs-rust");
    }

    #[test]
    fn transport_and_outcome_failures_are_different_layers() {
        let broken = Reply::transport("连不上控制服务");
        assert!(!broken.succeeded());
        let json = serde_json::to_value(&broken).unwrap();
        assert_eq!(json["transport"], "连不上控制服务");
        assert!(json.get("outcome").is_none(), "没送到就不该编造回执");

        let refused = Reply::from_outcome(
            Outcome::ok(
                nbs_control::Data::Empty,
                nbs_control::ChangeReport {
                    rev: 7,
                    history_len: 3,
                    ..Default::default()
                },
            ),
            "nbs-rust",
        );
        let json = serde_json::to_value(&refused).unwrap();
        assert!(refused.succeeded());
        assert_eq!(json["channel"], "nbs-rust", "结论要点名连的是谁");
        assert_eq!(json["outcome"]["report"]["rev"], 7);
    }

    /// 走一遍真实帧：`Request` 序列化后必须能原样读回同一条 `Op`。
    /// 键序由 serde 的内部 tag 决定（`kind` 落在最后一个键），这里不锁键序，只锁「接对变体」。
    fn roundtrip(op: Op) -> String {
        let frame = serde_json::to_string(&nbs_control::bus::Request {
            id: 1,
            op: op.clone(),
        })
        .unwrap();
        let back: nbs_control::bus::Request =
            serde_json::from_str(&frame).unwrap_or_else(|e| panic!("{frame} 读不回来: {e}"));
        assert_eq!(back.op, op, "线上形状和发出去的不是同一条请求");
        frame
    }

    #[test]
    fn playback_tools_map_to_the_wire_op() {
        // 五条播放工具共用 Op::Playback，只有 `action` 不同；逐条钉住，免得复制粘贴接错。
        let check = |action: Playback, needle: &str| {
            let frame = roundtrip(Op::Playback(action));
            assert!(frame.contains(r#""kind":"playback""#), "{frame}");
            assert!(frame.contains(needle), "{frame} 里少了 {needle:?}");
        };
        check(Playback::Play, r#""action":"play""#);
        check(Playback::Pause, r#""action":"pause""#);
        check(Playback::Stop, r#""action":"stop""#);
        check(Playback::Seek { tick: 3.5 }, r#""tick":3.5"#);
        check(Playback::SetLoop { enabled: true }, r#""enabled":true"#);
    }

    #[test]
    fn params_convert_to_the_expected_op() {
        // 每个 params 结构都得落到它名义上那条 `Op`，尤其是改名过的（save_as→SavePath）。
        let check = |op: Op, needle: &str| {
            let text = roundtrip(op);
            assert!(text.contains(needle), "线上形状少了 {needle:?}：{text}");
        };
        check(
            Op::from(RunAction {
                action: "redo".to_string(),
                args: Some(serde_json::json!({"steps": 2})),
            }),
            r#""kind":"run_action""#,
        );
        check(
            Op::from(Transpose {
                scope: Scope::All,
                semitones: -2,
                expect_rev: Some(9),
            }),
            r#""scope":{"scope":"all"}"#,
        );
        let removed = roundtrip(Op::from(RemoveCells {
            cells: vec![CellRef { tick: 4, layer: 1 }],
            label: None,
            expect_rev: None,
        }));
        assert!(
            removed.contains(r#""cells":[[4,1]]"#),
            "坐标要按 (tick, layer) 元组上线：{removed}"
        );
        assert!(
            !removed.contains("label"),
            "省略的字段不该占字节：{removed}"
        );
        check(
            Op::from(SavePath {
                path: "C:/x/y.nbs".to_string(),
                version: None,
                overwrite: true,
            }),
            r#""kind":"save_path""#,
        );
        check(
            Op::from(ClearSong {
                expect_rev: None,
                discard_changes: false,
            }),
            r#""discard_changes":false"#,
        );
        // 工具名和 `Op` 变体不同名时要格外盯住：save_as→SavePath、remove_layer→LayerRemove。
        check(
            Op::from(LayerIndex {
                index: 2,
                expect_rev: None,
            }),
            r#""kind":"layer_remove""#,
        );
        check(
            Op::from(OpenPath {
                path: "C:/nbs/s.nbs".to_string(),
                discard_changes: false,
            }),
            r#""kind":"open_path""#,
        );
        check(
            Op::from(Export {
                path: "C:/nbs/o.wav".to_string(),
                format: ExportFormat::Wav,
                sample_rate: Some(44100),
                channels: None,
                include_locked: None,
                overwrite: false,
            }),
            r#""format":"wav""#,
        );
        check(
            Op::PlaceSelection {
                expect_rev: Some(3),
            },
            r#""kind":"place_selection""#,
        );
    }
}
