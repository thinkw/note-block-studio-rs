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
    BatchFieldArgs, CellRef, ClearSong, Export, GetLayers, GetNotes, ImportMidi, InspectMidi,
    JobId, LayerAdd, LayerIndex, LayerMove, LayerUpdate, OpenPath, RemoveCells, RevOnly, RunAction,
    SavePath, Seek, Select, SetLoop, SetMetaArgs, SetNotes, Steps, TranscribeAudio,
    TranscribePrepareEnv, TranscribePrepareModel, Transpose,
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

/// 手里那条连接的可信状态。锁只护这几格，网络读全在锁外做。
#[derive(Default)]
struct Link {
    /// `None` = 现在没有可信连接：还没连上，或者上一条刚刚在传输层失败。
    client: Option<ControlClient>,
    /// 最近一次握手时编辑器自报的应用名，给每条回执定范围。
    app: String,
    /// 最近一次握手公布的标签能力；`server_info` 让外部调用方开局就能看到。
    parked_write: Option<bool>,
}

/// 一条到编辑器的连接，外加「断了就重新连上」。一次只能问一句，所以用 `Mutex` 串行化；
/// 要并行就再起一个桥接进程，别在一条通道上抢答。
///
/// **为什么必须重连**（2026-11-23）：编辑器那条服务端的连接线程把读超时设成
/// `request_timeout × 2`（默认 60s），空闲满 60s 就退出。IDE 里的 MCP 会话正是长时间空闲的那个，
/// 症状是「开局几条全成，放着不动一会儿之后每条都失败，且失败得越来越离谱」——
/// 因为旧桥接把断掉的连接一直攥在手里。重连是唯一的止血办法。
pub struct Bridge {
    endpoint: Endpoint,
    link: Arc<Mutex<Link>>,
}

/// 锁中毒只说明上一次持有它时 panic 过，连接本身的状态由对端说了算，
/// 拿回守卫继续用比把整个工具面卡死好。
fn link_guard(link: &Arc<Mutex<Link>>) -> std::sync::MutexGuard<'_, Link> {
    link.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 传输层彻底失败之后怎么说、怎么收尾：把连接重建一次（但不重发），
/// 让下一问站在一条新连接上，同时把「现在到底连不连得上」如实报出来。
///
/// `resend` 是这一问**本来允不允许重发**（不是「重发过没有」）：允许的那一类走到这里
/// 说明已经试过两次都不通，不允许的那一类要说清没替人重放、以及怎么自己确认落没落。
fn lost_link(endpoint: &Endpoint, link: &Arc<Mutex<Link>>, reason: &str, resend: bool) -> String {
    let now = match connect(endpoint) {
        Ok(fresh) => {
            let mut guard = link_guard(link);
            guard.app = fresh.server_app().to_string();
            guard.client = Some(fresh);
            "编辑器现在连得上，下一问走的是重建好的新连接".to_string()
        }
        Err(e) => format!("现在重新连也连不上：{e}"),
    };
    if resend {
        format!("{reason}；这一问属于「重发不会多出副作用」那一类，已经自动重发过一次，还是不通。{now}")
    } else {
        format!(
            "{reason}；这条连接已丢掉并重新连上，但**没有**替你重发这一问\
             （改数据、起播放、落盘的操作不自动重发：第一次可能已经落进主线程收件箱，\
             重发就是做两遍）。落没落别看猜——先 `get_history` 读 `undo_label`：\
             这一笔真生效的话它就是撤销列表第一条的 label（`set_notes` 那批给的 label，\
             没给则是默认那句），紧接着的 `rev` 也该比上一次回执大；\
             落盘的那一类直接看产物文件在不在。两个都对不上才是「没落下来」，那时重放才安全。\
             {now}"
        )
    }
}

/// `server_info` 的「这到底是哪个编辑器」那一格。
///
/// 两个来源各说一半：发现文件说**桥接以为**编辑器是哪条端口、哪个进程，`ping` 回包说
/// **谁真的答了**这一问。同机开好几个窗口、或发现文件是上一个实例留下的时候，
/// 只有把两个并排放才看得出连错了人——之前有人拿 `song_session` 当身份，
/// 而另存成别的文件之后它不变，于是把一次成功的调用读成「连错窗口了」。
///
/// 令牌（token）不在这一格里：它是本地访问控制的全部依据，不该出现在任何回执里。
fn instance_view(ep: &Endpoint, editor_pid: Option<u32>) -> serde_json::Value {
    // 直连是「我知道端口和令牌」那一档，没有发现文件可翻，不能说成「文件里没写」。
    let found = if ep.direct.is_some() {
        None
    } else {
        nbs_control::Discovery::read_from(&ep.config_dir).ok()
    };
    let direct = ep.direct.as_ref().map(|(port, _)| *port);
    let (source, port, discovery_pid, started_unix, app) = match (&found, direct) {
        (Some(d), _) => (
            "discovery",
            Some(u32::from(d.port)),
            Some(d.pid),
            Some(d.started_unix),
            Some(d.app.clone()),
        ),
        (None, Some(port)) => ("direct", Some(u32::from(port)), None, None, None),
        (None, None) => ("none", None, None, None, None),
    };
    // 两个都有值才谈得上对不对得上；缺一个回 null，那是「没法判」，不是「对不上」。
    let same = match (editor_pid, discovery_pid) {
        (Some(asked), Some(file)) => Some(asked == file),
        _ => None,
    };
    serde_json::json!({
        "source": source,
        "port": port,
        "editor_pid": editor_pid,
        "discovery_pid": discovery_pid,
        "same": same,
        "started_unix": started_unix,
        "app": app,
    })
}

/// 投递一次请求。
///
/// 传输层失败一律把这条连接丢掉（`nbs-control` 那边已经 `shutdown` 了 socket，
/// 这里丢的是句柄本身）：晚到的回执照对不到「还能继续用」——
/// 下一问若在这条连接上读，先读到的是上一问那张过期回执，看着像答对了其实答的是旧事。
///
/// `resend` 决定要不要在重连之后**自动重发一次**，调用方按 `Op::is_resend_safe()` 给：
/// 只读外加幂等的走带动作（`seek` / `stop`）重发没有副作用；写操作和起播绝不重发。
///
/// 返回的第二个值是「这一答是重发之后拿到的」，用来在回执里补一句说明。
fn dispatch_link(
    endpoint: &Endpoint,
    link: &Arc<Mutex<Link>>,
    op: Op,
    tab: Option<nbs_control::TabRef>,
    resend: bool,
) -> Result<(Outcome, bool), String> {
    let mut resent = false;
    loop {
        let mut client = {
            let mut guard = link_guard(link);
            match guard.client.take() {
                Some(client) => client,
                None => {
                    let fresh = connect(endpoint)?;
                    guard.app = fresh.server_app().to_string();
                    guard.parked_write = Some(fresh.capabilities().parked_write);
                    fresh
                }
            }
        };
        match client.request_tab(op.clone(), tab) {
            Ok(outcome) => {
                link_guard(link).client = Some(client);
                return Ok((outcome, resent));
            }
            Err(err) if resent || !resend => {
                return Err(lost_link(endpoint, link, &err.to_string(), resend))
            }
            Err(_) => resent = true,
        }
    }
}

impl Bridge {
    /// 已经连着一条时用这个；`endpoint` 留着给断了之后重连。
    pub fn new(endpoint: Endpoint, client: ControlClient) -> Self {
        let app = client.server_app().to_string();
        let parked_write = client.capabilities().parked_write;
        Self {
            endpoint,
            link: Arc::new(Mutex::new(Link {
                client: Some(client),
                app,
                parked_write: Some(parked_write),
            })),
        }
    }

    /// 还没连上也要能把 MCP 服务开起来：工具面照样列得出，第一问再去连，
    /// 连不上就每条回一句 `transport` 说清「翻过哪个目录、怎么起编辑器」。
    /// 旧做法是启动时连不上就 `exit(2)`，客户端只看见「MCP 服务起不来」，
    /// 编辑器随后开起来也没用——得重启 IDE 才行。
    pub fn detached(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            link: Arc::new(Mutex::new(Link::default())),
        }
    }

    /// 这条通道连的是哪个应用；没连上过时给个占位，别把空串当结论。
    pub fn channel(&self) -> String {
        let app = link_guard(&self.link).app.clone();
        if app.is_empty() {
            "nbs-rs".to_string()
        } else {
            app
        }
    }
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
/// `only` 是给 `--dump-tools <工具>` 用的：全表 46 条、每条带一份 schema，
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
    /// 发一条 `Op` 并等回执，然后按 [`to_result`] 的口径交给 MCP。
    /// 阻塞的网络读放在 `exec` 的 `spawn_blocking` 里，免得占住异步线程。
    async fn call(&self, op: Op) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(op, None).await
    }

    async fn call_tab(
        &self,
        op: Op,
        tab: Option<nbs_control::TabRef>,
    ) -> Result<Json<Reply>, CallToolResult> {
        to_result(self.exec_tab(op, tab).await)
    }

    /// [`call`](Self::call) 的无包装版本，给 `--call` 用。
    pub async fn exec(&self, op: Op) -> Reply {
        self.exec_tab(op, None).await
    }

    pub async fn exec_tab(&self, op: Op, tab: Option<nbs_control::TabRef>) -> Reply {
        let endpoint = self.endpoint.clone();
        let link = Arc::clone(&self.link);
        let channel = self.channel();
        // 断线重发名单：只读 + 幂等的走带动作（seek / stop），见 `Op::is_resend_safe`。
        let resend = op.is_resend_safe();
        let joined = tokio::task::spawn_blocking(move || {
            dispatch_link(&endpoint, &link, op, tab, resend)
        })
        .await;
        match joined {
            // 重发之后拿到的那一答是新连接上现算的，`rev` 也是那时的，正文不用改；
            // 但「中间断过一次」这件事要能被人从日志里翻出来，走 stderr（stdout 是协议流）。
            Ok(Ok((outcome, true))) => {
                eprintln!("nbs-mcp: 连接断过一次，这一答是重发之后从编辑器现问的");
                Reply::from_outcome(outcome, &channel)
            }
            Ok(Ok((outcome, false))) => Reply::from_outcome(outcome, &channel),
            Ok(Err(reason)) => Reply::transport(reason),
            Err(_) => Reply::transport("桥接内部：等待编辑器回执的任务被取消了"),
        }
    }

    /// 按计划发一条。校验由 [`call::plan`] 在连端口之前做完，所以这里只管发。
    pub async fn run(&self, plan: call::Plan) -> Reply {
        match plan {
            call::Plan::BridgeInfo => self.bridge_info().await,
            call::Plan::Editor { op, tab } => self.exec_tab(op, tab).await,
        }
    }

    /// 编辑器现在接不接得住：当场敲一次 `ping`。顺带把这条连接走通一遍——
    /// 空闲被编辑器关掉的话，这一次就会重连上，问出来的答案也是重连之后的。
    /// `Err` 里是「为什么接不住」，那句话直接给调用方看。
    async fn server_probe(&self) -> Result<(String, Option<u32>), (String, Option<u32>)> {
        let endpoint = self.endpoint.clone();
        let link = Arc::clone(&self.link);
        let channel = self.channel();
        match tokio::task::spawn_blocking(move || {
            dispatch_link(&endpoint, &link, Op::Ping, None, true)
        })
        .await
        {
            Ok(Ok((outcome, _))) if outcome.ok => {
                let pid = match &outcome.data {
                    Some(nbs_control::Data::Pong { pid, .. }) => (*pid != 0).then_some(*pid),
                    _ => None,
                };
                Ok((
                    format!(
                        "通：{channel}，协议 v{} 对得上",
                        nbs_control::PROTOCOL_VERSION
                    ),
                    pid,
                ))
            }
            Ok(Ok((outcome, _))) => Err((
                format!(
                    "{channel} 连上了，但把探活这一问拒了：{}",
                    outcome
                        .error
                        .map(|e| e.message)
                        .unwrap_or_else(|| "没有说明原因".to_string())
                ),
                None,
            )),
            Ok(Err(reason)) => Err((format!("编辑器没连上：{reason}"), None)),
            Err(_) => Err(("桥接内部：探活的任务被取消了".to_string(), None)),
        }
    }

    /// 这条通道连的是什么：`bridge` 那格是桥接自己（不用编辑器在场也答得出），
    /// `editor` 那格是当场问出来的。
    ///
    /// 两格分开答，别混成一格：「桥接活着」和「编辑器接得住」在空闲断线那件事上是
    /// 两个状态，只报前者就会出现「面板 ready、一调就错」。
    /// 接不住时整条走 `transport`：`isError` 要能停住调用方，`--call` 的退出码
    /// 也该是 2（没连上）而不是 0（成功）。
    pub async fn bridge_info(&self) -> Reply {
        let channel = self.channel();
        let face = |ok: bool, editor: &str, parked_write: Option<bool>, instance| {
            serde_json::json!({
                "ok": ok,
                "channel": channel,
                "bridge": env!("CARGO_PKG_VERSION"),
                "protocol": nbs_control::PROTOCOL_VERSION,
                "parked_write": parked_write,
                "editor": editor,
                "instance": instance,
            })
        };
        let probe = self.server_probe().await;
        let parked_write = link_guard(&self.link).parked_write;
        match probe {
            Ok((status, editor_pid)) => Reply {
                outcome: Some(face(
                    true,
                    &status,
                    parked_write,
                    instance_view(&self.endpoint, editor_pid),
                )),
                transport: None,
                channel: Some(channel.clone()),
            },
            Err((reason, editor_pid)) => Reply {
                outcome: Some(face(
                    false,
                    &reason,
                    parked_write,
                    instance_view(&self.endpoint, editor_pid),
                )),
                transport: Some(reason),
                channel: Some(channel),
            },
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
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    // -------------------------------------------------------------- 读

    /// 工程概览：文件路径、脏标记、元数据、图层数、音符数、长度、播放头、
    /// `rev` / `song_session`，外加兼容性诊断。想先看清局面就调这个。
    #[tool]
    async fn get_snapshot(
        &self,
        Parameters(a): Parameters<crate::params::TabOnly>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(Op::GetSnapshot, a.tab).await
    }

    /// 读一段范围内的音符格子。一次回多少由 `limit`（默认 1000）和 `max_bytes`（默认 128 KiB）
    /// 里更小的那个定，`truncated=true` 说明只回了一部分，`total` 是范围里的真实条数——
    /// 想少读点就缩小 `tick_from..=tick_to`，别把部分结果当整首。
    /// `view` 换形状：`cells`（默认，逐格）/ `columns`（八条等长数组，同样数据省三成字节，
    /// 整段读谱用它）/ `runs`（同层同音色同键的连续格子合成一段，读延音省地方，
    /// 代价是段内中间的力度分档看不见）。
    /// 想按小节读 melody：给 `bars:[起,止]`（两头都含，一次最多 256 节），tick 区间由小节号算，
    /// 回执的 `data.bar_overview` 每节一行 `tick:音名(音色)`——键号要心算才会唱，而错的那半音
    /// 只有耳朵能发现，所以这份文字视图由状态栏同一张表生成（只长在 `view=cells` 上）。
    #[tool]
    async fn get_notes(&self, Parameters(a): Parameters<GetNotes>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 图层清单：层号、名字、音量、左右声道、锁定 / solo 状态。
    /// 层多时给 `layer_from / layer_to`（闭区间）：整张表按层数算字节，八十来层就是十来 KB。
    /// 回的是哪一段会写在 message 里，省略两头才是全表。
    #[tool]
    async fn get_layers(
        &self,
        Parameters(a): Parameters<crate::params::GetLayers>,
    ) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 当前选区：是否浮动选区、包围盒、音符数。写之前想知道「selection 是哪儿」就看它。
    #[tool]
    async fn get_selection(
        &self,
        Parameters(a): Parameters<crate::params::TabOnly>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(Op::GetSelection, a.tab).await
    }

    /// 音色表：内置 0..=19 加自定义槽位，含每个槽位当前挂的音色名，以及
    /// Minecraft 原生可播的键号闭区间 `mc_key_range`。
    /// 写音符前用它确认 `instrument` 号真的存在、音高落在能响的那两个八度里。
    #[tool]
    async fn get_instruments(
        &self,
        Parameters(a): Parameters<crate::params::TabOnly>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(Op::GetInstruments, a.tab).await
    }

    /// 兼容性诊断：超出 Minecraft 两个八度的音符、原版没有的音色、锁层引用等。
    /// 只读，不改状态。
    #[tool]
    async fn get_diagnostics(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::GetDiagnostics).await
    }

    /// 撤销栈：当前游标、长度，以及每一步的说明文字。
    #[tool]
    async fn get_history(
        &self,
        Parameters(a): Parameters<crate::params::TabOnly>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(Op::GetHistory, a.tab).await
    }

    /// 标签清单：稳定 id、当前位置、标题、脏标记与内容版本；不切标签。
    #[tool]
    async fn list_tabs(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::ListTabs).await
    }

    /// 切到指定标签；稳定 id 推荐长期持有，index 会在重排后漂。
    #[tool]
    async fn switch_tab(
        &self,
        Parameters(a): Parameters<crate::params::SwitchTab>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 关闭指定标签；脏标签必须明确 discard=true，否则返回 needs_confirm。
    #[tool]
    async fn close_tab(
        &self,
        Parameters(a): Parameters<crate::params::CloseTab>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 写

    /// 批量写格子：一次调用 = 一次撤销，整批先校验后落笔，要么全成要么全不动。
    /// `key` 既收键号也收音名（`45` 与 `"F#4"` 等价），拼错的音名会被拒而不是挪到隔壁键。
    /// 回执的 `report.rev` 请留给下一次写操作的 `expect_rev`。
    #[tool]
    async fn set_notes(&self, Parameters(a): Parameters<SetNotes>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 摘音符，两种说法二选一：逐格点名 `cells`，或给 `scope`（`all` / `rect`）再配一条可选的
    /// `filter`（`instrument` / `key_from` / `key_to` / `velocity_min` / `velocity_max` /
    /// `outside_mc_range`，条件之间是「与」）按范围删。
    /// 空格子不算改动；锁定层上的音符是**跳过**不是删掉，回执把命中 / 摘掉 / 跳过三个数分开报。
    /// 一格都没命中时不动 `rev`、不进撤销表。`scope=selection` 不认：浮动选区里的音符已经离开网格了，
    /// 要放回去用 `place_selection`。
    #[tool]
    async fn remove_cells(&self, Parameters(a): Parameters<RemoveCells>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 批量改某个字段（音色 / 音高 / 音量 / 声道 / 音分）。
    /// `mode="add"` 在现值上加，越界由服务端夹取并在回执里说明夹了多少，不会静默丢弃。
    #[tool]
    async fn batch_field(&self, Parameters(a): Parameters<BatchFieldArgs>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 整体或局部移调。`scope` 决定动谁：整首歌、当前浮动选区，还是给定矩形内已有音符的格子。
    /// 越界夹到 0..=87 并在回执里说明。
    #[tool]
    async fn transpose(&self, Parameters(a): Parameters<Transpose>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 选区：框选 / 全选 / 反选 / 按音色 / 超出 MC 音域 / 自定义音色。
    /// 摘出的浮动选区会挡住写操作，写完记得 `place_selection` 或用 `select` 取消。
    #[tool]
    async fn select(&self, Parameters(a): Parameters<Select>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 把浮动选区就地落回歌曲，等价于「取消选择但不丢改动」。
    /// 有浮动选区时 `set_notes` 这类写操作会被状态闸挡回来，所以这一步不能省。
    #[tool]
    async fn place_selection(&self, Parameters(a): Parameters<RevOnly>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 加图层：`at` 省略 = 追加到末尾。
    #[tool]
    async fn layer_add(&self, Parameters(a): Parameters<LayerAdd>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 删图层。锁定层的音符删之前先 `get_snapshot` 看一眼，回执会说明丢了几格。
    #[tool]
    async fn layer_remove(&self, Parameters(a): Parameters<LayerIndex>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 改图层属性：名字、音量、声道、锁定状态（normal / locked / solo）。省略的字段不动。
    /// 回执的 `data.layer` 只有被改的这一层（`kind:"layer"`），要整张表再叫 `get_layers`。
    #[tool]
    async fn layer_update(&self, Parameters(a): Parameters<LayerUpdate>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 移动图层顺序，音符跟着层一起走。
    #[tool]
    async fn layer_move(&self, Parameters(a): Parameters<LayerMove>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 改元数据：名字、作者、说明、速度、拍号、循环。省略的字段不动，
    /// 一个都没给会被拒（避免「空提交也报成功」的假象）。
    #[tool]
    async fn set_meta(&self, Parameters(a): Parameters<SetMetaArgs>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
    }

    /// 撤销。`steps` 省略 = 1 步。回执的 `report` 会说清实际退了几步。
    #[tool]
    async fn undo(&self, Parameters(a): Parameters<Steps>) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(
            Op::Undo {
                steps: a.steps,
                expect_rev: a.expect_rev,
            },
            a.tab,
        )
        .await
    }

    /// 重做。`steps` 省略 = 1 步。
    #[tool]
    async fn redo(&self, Parameters(a): Parameters<Steps>) -> Result<Json<Reply>, CallToolResult> {
        self.call_tab(
            Op::Redo {
                steps: a.steps,
                expect_rev: a.expect_rev,
            },
            a.tab,
        )
        .await
    }

    /// 清空成一首空歌。工程有未保存改动时必须带 `discard_changes=true`：
    /// 这一步会连改动和全部撤销历史一起丢掉，不带就等于没认错。
    #[tool]
    async fn clear_song(&self, Parameters(a): Parameters<ClearSong>) -> Result<Json<Reply>, CallToolResult> {
        let tab = a.tab;
        self.call_tab(a.into(), tab).await
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

    // -------------------------------------------------------------- 音频转 MIDI

    /// 问一次本机转录环境：Python / torch / MuScriptor 版本和 GPU 情况。
    /// 它只起一个子进程读现状，**不下载任何东西**，所以是这四条里唯一不需要许可的。
    /// 重活不在这次调用里做完：回执是任务句柄，之后 `job_status` 轮询，`job_cancel` 能停。
    #[tool]
    async fn transcribe_probe(&self) -> Result<Json<Reply>, CallToolResult> {
        self.call(Op::TranscribeProbe).await
    }

    /// 装本机转录环境（uv 建 venv、装 MuScriptor 与 torch，几百 MB）。
    /// 这一下**要人先在编辑器「音频转 MIDI」窗口里勾过本地下载与模型使用许可**：
    /// AI 不能代勾，也没有绕过它的参数——没勾会得到 `not_allowed`，那句话里写了该谁去勾。
    /// `cuda` 省略 = 用窗上当前那个勾，不替人改它。回任务句柄，完活带环境与 GPU 情况。
    #[tool]
    async fn transcribe_prepare_env(
        &self,
        Parameters(a): Parameters<TranscribePrepareEnv>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 只把权重取来并加载一遍（不识别音频）：几十分钟的活之前先验一次显存够不够、
    /// dtype 选得对不对。要许可（权重是 CC BY-NC 4.0，下载由人勾过的那一下放行），
    /// 也要环境已装好。`options` 省略哪个字段就用窗上当前那一份。
    #[tool]
    async fn transcribe_prepare_model(
        &self,
        Parameters(a): Parameters<TranscribePrepareModel>,
    ) -> Result<Json<Reply>, CallToolResult> {
        self.call(a.into()).await
    }

    /// 音频 → MIDI。要许可、要装好的环境，音频路径受允许的目录约束；回任务句柄，
    /// 用 `job_status` 轮询（模型第一次跑还会先下权重，进度和消息都在句柄里）。
    /// **想接着导入就必须给 `save_midi`**：产物本来落在编辑器自己的
    /// `.work/transcription/jobs/…`，那个目录不在路径白名单里，`import_midi` 读不到它。
    /// 给一个落在放行目录里的 `.mid` 路径（目标已存在要带 `overwrite=true`），
    /// 之后就能走 `inspect_midi` 预演 → `import_midi` 落歌那条现成的链。
    /// 完活的 `result` 除了音符数，还带回转录窗那两条核对：`preview_notes`、
    /// `importer_events`、`note_count_mismatch`——对不上时窗上会直接停止导入，
    /// 这里照实报、不替人决定，下一步该看 `inspect_midi` 给的真正落歌数。
    /// 窗上的 SoundFont A/B 试听没接进来：要听效果请用 `export`（wav 能离线渲染出来）。
    #[tool]
    async fn transcribe_audio(
        &self,
        Parameters(a): Parameters<TranscribeAudio>,
    ) -> Result<Json<Reply>, CallToolResult> {
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

    /// 这条 MCP 通道连的是哪个编辑器、用什么参数连上的，以及**现在**编辑器接不接得住。
    /// 会当场敲一次 `ping`：「桥接活着」和「编辑器接得住」是两个状态，只报前者会出现
    /// 面板 ready、一调就错。接不住时这条的 `transport` 有值。
    ///
    /// `instance` 那格回答「刚才答话的是哪个进程」：`editor_pid` 是编辑器自报的，
    /// `discovery_pid` / `port` / `started_unix` 是发现文件里的，`same` 是两者对不对得上
    /// （直连或旧编辑器没回 pid 时是 null = 没法判）。**认文件用 `get_snapshot` 的 `path`，
    /// 认进程用这里的 pid**；`song_session` 只在整首替换时推进，另存不改它，别拿来当身份。
    #[tool]
    async fn server_info(&self) -> Result<Json<Reply>, CallToolResult> {
        to_result(self.bridge_info().await)
    }
}

#[tool_handler(
    name = "nbs-rs",
    instructions = "本服务驱动一个**正在运行的 nbs-rs（Note Block Studio 移植）编辑器窗口**，\
所有改动都落在那台机器的这个 GUI 进程里，不是内存里的模拟工程。\n\
开局先 list_tabs 看清标签，再用 get_snapshot 看清目标标签的 rev 和脏标记。\
读工具和文档级写工具可用 tab={\"id\":...} 或 tab={\"index\":...} 寻址；省略 tab 保持旧行为=当前标签。\
id 稳定，index 会随重排漂；关闭后的 id 返回 tab_gone。\n\
写操作带上一次回执里的 expect_rev，冲突会报 rev_conflict。后台标签默认只读，\
设置里开启后台写后还必须带 expect_rev；播放、文件、任务和交互动作只能打当前标签。\n\
弹窗和浮动选区会挡写操作：get_snapshot 会说明谁在挡路，用 run_action 的 answer_modal（answer=yes/no/cancel）/ \
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
    use nbs_control::{ExportFormat, NoteFilter, Scope};

    #[test]
    fn tool_names_are_unique_and_every_tool_says_something() {
        let tools = Bridge::tool_router().list_all();
        assert_eq!(tools.len(), 46, "工具面新增三条标签工具后应恰好 46 条");
        let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        names.sort();
        let dupes: Vec<String> = names
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0].clone())
            .collect();
        assert!(dupes.is_empty(), "重名工具: {dupes:?}");
        for name in [
            "get_snapshot",
            "set_notes",
            "run_action",
            "export",
            "list_tabs",
            "switch_tab",
            "close_tab",
        ] {
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
        let diagnostics = serde_json::to_value(&find("get_diagnostics").input_schema).unwrap();
        assert_eq!(diagnostics["type"], "object", "{diagnostics}");
        assert!(
            diagnostics["properties"]
                .as_object()
                .is_some_and(|p| p.is_empty()),
            "{diagnostics}"
        );
        // 读工具现在有可选 tab；id/index 二选一的形状必须出现在客户端 schema 里。
        let layers = text("get_layers");
        assert!(layers.contains("TabRef") && layers.contains("index"), "{layers}");
    }

    /// 报错文案里那句「形状看 `--dump-tools set_notes`」必须是真能跑的写法：
    /// 全表 46 条带 schema，为了看一个参数读完整张表，等于没给答案。
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
            tab: None,
            op: op.clone(),
        })
        .unwrap();
        let back: nbs_control::bus::Request =
            serde_json::from_str(&frame).unwrap_or_else(|e| panic!("{frame} 读不回来: {e}"));
        assert_eq!(back.op, op, "线上形状和发出去的不是同一条请求");
        frame
    }

    #[test]
    fn request_tab_envelope_round_trips_without_changing_the_op() {
        let tab = nbs_control::TabRef {
            id: Some(88),
            index: None,
        };
        let request = nbs_control::bus::Request {
            id: 5,
            tab: Some(tab),
            op: Op::GetLayers {
                layer_from: None,
                layer_to: None,
            },
        };
        let frame = serde_json::to_string(&request).unwrap();
        assert!(frame.contains(r#""tab":{"id":88}"#), "{frame}");
        let back: nbs_control::bus::Request = serde_json::from_str(&frame).unwrap();
        assert_eq!(back, request);
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
                tab: None,
                action: "redo".to_string(),
                args: Some(serde_json::json!({"steps": 2})),
            }),
            r#""kind":"run_action""#,
        );
        check(
            Op::from(Transpose {
                tab: None,
                scope: Scope::All,
                semitones: -2,
                expect_rev: Some(9),
            }),
            r#""scope":{"scope":"all"}"#,
        );
        let removed = roundtrip(Op::from(RemoveCells {
            tab: None,
            cells: vec![CellRef { tick: 4, layer: 1 }],
            scope: None,
            filter: None,
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
        assert!(
            !removed.contains("scope") && !removed.contains("filter"),
            "逐格那一支不该带上新字段的空壳：{removed}"
        );
        // 范围那一支：scope 是内部打标签的枚举，形状必须和 batch_field / transpose 用的一致，
        // filter 是嵌套对象——两条都要在线上看得见，否则服务端只会收到「cells 为空」。
        let by_range = roundtrip(Op::from(RemoveCells {
            tab: None,
            cells: vec![],
            scope: Some(Scope::Rect {
                tick_from: 0,
                layer_from: 0,
                tick_to: 31,
                layer_to: 3,
            }),
            filter: Some(NoteFilter {
                velocity_max: Some(20),
                ..Default::default()
            }),
            label: None,
            expect_rev: None,
        }));
        assert!(
            by_range.contains(r#""scope":{"scope":"rect""#) && by_range.contains(r#""tick_to":31"#),
            "范围要按 rect 上线：{by_range}"
        );
        assert!(
            by_range.contains(r#""filter":{"velocity_max":20}"#),
            "条件要带字段名上线，光一个数说不清是哪一条：{by_range}"
        );
        assert!(
            !by_range.contains(r#""cells""#),
            "空 cells 不该占字节，否则老服务端会以为要点名格子：{by_range}"
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
                tab: None,
                expect_rev: None,
                discard_changes: false,
            }),
            r#""discard_changes":false"#,
        );
        // 工具名和 `Op` 变体不同名时要格外盯住：save_as→SavePath、remove_layer→LayerRemove。
        check(
            Op::from(LayerIndex {
                tab: None,
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

    // -------------------------------------------------- 空闲断线之后怎么办
    //
    // 编辑器那条服务端的连接线程读超时 = `request_timeout × 2`（默认 60s），空闲满就退出。
    // 下面的假编辑器把「答一句就把这条连接丢掉」当默认行为，观察的是连接数和请求数：
    // 前者证明真重连了，后者证明写操作没被重发第二遍。

    use std::io::BufReader;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// 假编辑器自报的进程号：`server_info` 的 `instance.editor_pid` 要能指着它对账。
    const FAKE_EDITOR_PID: u32 = 4242;

    /// `AnswerOnce`：答完第一问就把连接关掉（空闲被服务端关掉之后的样子）。
    /// `QuitAfterHello`：握手成功但一句不答就关（下一问写出去再没人接）。
    #[derive(Clone, Copy)]
    enum Fate {
        AnswerOnce,
        QuitAfterHello,
    }

    struct FakeEditor {
        endpoint: Endpoint,
        connections: Arc<AtomicUsize>,
        requests: Arc<AtomicUsize>,
    }

    impl FakeEditor {
        /// 每条连接按 `fates` 的顺序办事，用完之后一直沿用最后一条。
        /// 顺序是这条夹具的关键：写操作那一问要「第一条连接死掉、第二条活着」，
        /// 否则重发与否在两边都看不见，测试咬不动（第一版就是这样漏的）。
        fn start(fates: &[Fate]) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let connections = Arc::new(AtomicUsize::new(0));
            let requests = Arc::new(AtomicUsize::new(0));
            let fates = Arc::new(fates.to_vec());
            let (cn, rq, fates) = (Arc::clone(&connections), Arc::clone(&requests), fates);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { return };
                    let index = cn.fetch_add(1, Ordering::SeqCst);
                    let fate = fates[index.min(fates.len() - 1)];
                    let rq = Arc::clone(&rq);
                    std::thread::spawn(move || {
                        let Ok(clone) = stream.try_clone() else { return };
                        let mut reader = BufReader::new(clone);
                        let mut writer = stream;
                        let hello: Option<nbs_control::bus::Hello> =
                            match nbs_control::read_json_line(&mut reader) {
                                Ok(hello) => hello,
                                Err(_) => return,
                            };
                        if hello.is_none() {
                            return;
                        }
                        let ack = nbs_control::bus::HelloAck {
                            hello: true,
                            protocol: nbs_control::PROTOCOL_VERSION,
                            app: "nbs-fake 0.0.0-test".to_string(),
                            capabilities: Default::default(),
                            snapshot: None,
                        };
                        if nbs_control::write_json_line(&mut writer, &ack).is_err() {
                            return;
                        }
                        if let Fate::QuitAfterHello = fate {
                            // 一句不答就走：下一问写出去就再没人接。
                            return;
                        }
                        let asked: Option<nbs_control::bus::Request> =
                            match nbs_control::read_json_line(&mut reader) {
                                Ok(asked) => asked,
                                Err(_) => return,
                            };
                        let Some(req) = asked else { return };
                        rq.fetch_add(1, Ordering::SeqCst);
                        // 只有探活那一问回 Pong：`server_info` 的 `instance.editor_pid` 就来自这里，
                        // 别的工具不需要形状，回 Empty 让它们专心验传输。
                        let data = match req.op {
                            nbs_control::Op::Ping => nbs_control::Data::Pong {
                                app_version: "0.0.0-test".to_string(),
                                protocol: nbs_control::PROTOCOL_VERSION,
                                rev: 7,
                                song_session: 2,
                                pid: FAKE_EDITOR_PID,
                            },
                            _ => nbs_control::Data::Empty,
                        };
                        let outcome = nbs_control::Outcome::ok(
                            data,
                            nbs_control::ChangeReport {
                                rev: 7,
                                ..Default::default()
                            },
                        );
                        let resp = nbs_control::bus::Response {
                            id: req.id,
                            outcome,
                        };
                        let _ = nbs_control::write_json_line(&mut writer, &resp);
                        // 答完就走：这条已经被服务端丢掉，下一问要撞上的就是这件事。
                    });
                }
            });
            Self {
                endpoint: Endpoint {
                    config_dir: std::env::temp_dir(),
                    direct: Some((port, "tok".to_string())),
                    client_name: "test".to_string(),
                },
                connections,
                requests,
            }
        }

        fn live(&self) -> Bridge {
            let client = connect(&self.endpoint).expect("假编辑器连不上");
            Bridge::new(self.endpoint.clone(), client)
        }
    }

    /// 重连之后的新连接是异步登记的，断言之前等一下（正常路径一次就过）。
    fn settled<F: FnMut() -> bool>(what: &str, mut f: F) {
        for _ in 0..500 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("等不到{what}");
    }

    #[tokio::test]
    async fn a_dead_link_does_not_stick_the_bridge_forever() {
        // 旧行为：连接被服务端关掉之后桥接一直攥着它，此后每条调用都失败，
        // 只能重启 IDE 或重新加载 MCP 服务。
        let fake = FakeEditor::start(&[Fate::AnswerOnce]);
        let bridge = fake.live();
        let first = bridge.exec(Op::GetSnapshot).await;
        assert!(first.succeeded(), "第一问该成：{:?}", first.transport);

        let second = bridge.exec(Op::GetSnapshot).await;
        assert!(
            second.succeeded(),
            "空闲断线之后读操作该自动重连重发，实际 transport = {:?}",
            second.transport
        );
        assert_eq!(
            fake.requests.load(Ordering::SeqCst),
            2,
            "读操作重发一次，服务端该收到两问"
        );
        settled("第二条连接在册", || {
            fake.connections.load(Ordering::SeqCst) >= 2
        });
    }

    #[tokio::test]
    async fn a_dead_link_never_resends_a_write() {
        // 写操作不重发：请求可能已经落进主线程收件箱，重发就是改两遍。
        // 但它照样要重连一条备着，失败文案也得指路「先看清这一笔落没落」——
        // 「先对一眼 rev」那句太空：rev 变大可能是别人改的，撤销列表第一条才是这一笔的凭据。
        let fake = FakeEditor::start(&[Fate::QuitAfterHello, Fate::AnswerOnce]);
        let bridge = fake.live();
        let reply = bridge
            .exec(Op::ClearSong {
                expect_rev: None,
                discard_changes: true,
            })
            .await;
        let text = reply.transport.expect("撞上断线的写操作必须报 transport");
        assert!(text.contains("没有"), "文案要说清没重发：{text}");
        assert!(
            text.contains("get_history") && text.contains("undo_label"),
            "文案要指名拿撤销列表第一条当凭据，而不是让人猜：{text}"
        );
        assert_eq!(reply.outcome, None, "没送到就不该编造一份编辑器回执");
        assert_eq!(
            fake.requests.load(Ordering::SeqCst),
            0,
            "第二条连接是会答话的：这一句变成 1，就说明写操作被重发了"
        );
        settled("重连好的第二条连接在册", || {
            fake.connections.load(Ordering::SeqCst) >= 2
        });
    }

    #[tokio::test]
    async fn seek_and_stop_are_replayed_but_play_is_not() {
        // 空闲断线之后「定位一下」「停下来」不该要人手工补一次——这两条幂等，重发不出副作用。
        // 但 play 不在名单里：重发会把人正听着的播放从头冲一遍，响过几次回执里看不见。
        let fake = FakeEditor::start(&[Fate::AnswerOnce]);
        let bridge = fake.live();
        assert!(
            bridge.exec(Op::GetSnapshot).await.succeeded(),
            "先把连接用到「已经答过一问、下一问会撞上死连接」这一步"
        );

        for (op, what) in [
            (Op::Playback(Playback::Seek { tick: 8.0 }), "seek"),
            (Op::Playback(Playback::Stop), "stop"),
        ] {
            let out = bridge.exec(op).await;
            assert!(
                out.succeeded(),
                "{what} 该在断线后自动重发一次，实际 transport = {:?}",
                out.transport
            );
        }
        assert_eq!(
            fake.requests.load(Ordering::SeqCst),
            3,
            "第一问 + seek 重发 + stop 重发：每条连接只答一问，所以每次都各问了一次"
        );

        // 同一套连接状态换成 play：写出去没人接，桥接不许替人重放，只能报 transport。
        let play = bridge.exec(Op::Playback(Playback::Play)).await;
        let text = play
            .transport
            .clone()
            .expect("play 撞上死连接必须报没送到");
        assert!(
            !play.succeeded() && text.contains("没有"),
            "play 不能进重发名单：{text}"
        );
        assert_eq!(
            fake.requests.load(Ordering::SeqCst),
            3,
            "play 那一问被重发了出去：请求数该停在 3"
        );
    }

    #[tokio::test]
    async fn a_detached_bridge_answers_the_first_question() {
        // 编辑器还没开窗就要能把 MCP 服务开起来（IDE 常常比窗口先起）。
        // 旧做法开局连不上直接 exit(2)，窗口后来开了也没用，得重启 IDE。
        let fake = FakeEditor::start(&[Fate::AnswerOnce]);
        let bridge = Bridge::detached(fake.endpoint.clone());
        let reply = bridge.exec(Op::GetSnapshot).await;
        assert!(
            reply.succeeded(),
            "detached 的第一问该自己连上：{:?}",
            reply.transport
        );
        assert_eq!(fake.connections.load(Ordering::SeqCst), 1, "只该连一条");
    }

    #[tokio::test]
    async fn server_info_answers_whether_the_editor_is_reachable() {
        // 「桥接活着」不等于「编辑器接得住」：这条工具现在要如实答出后者。
        let fake = FakeEditor::start(&[Fate::AnswerOnce]);
        let live = Bridge::detached(fake.endpoint.clone());
        let info = live.bridge_info().await;
        assert!(
            info.succeeded(),
            "编辑器在的时候该答成：{:?}",
            info.transport
        );
        let editor = info.outcome.as_ref().expect("有正文")["editor"]
            .as_str()
            .expect("editor 是字符串")
            .to_string();
        assert!(editor.contains("通"), "editor 那一格要说通：{editor}");
        assert_eq!(
            info.outcome.as_ref().expect("有正文")["parked_write"],
            false,
            "握手能力要暴露给 MCP 调用方"
        );

        // 端口上没人 listening：连接被拒，得说清是「没连上」而不是「编辑器答不」。
        let dead_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let dead_port = dead_listener.local_addr().unwrap().port();
        drop(dead_listener);
        let dead = Bridge::detached(Endpoint {
            config_dir: std::env::temp_dir(),
            direct: Some((dead_port, "tok".to_string())),
            client_name: "test".to_string(),
        });
        let info = dead.bridge_info().await;
        assert!(
            !info.succeeded(),
            "没人 listening 却说成功：{:?}",
            info.outcome
        );
        let reason = info.transport.clone().expect("该有 transport");
        assert!(reason.contains("编辑器没连上"), "要说清是没连上：{reason}");
        assert_eq!(
            crate::call::exit_code(&info),
            2,
            "探活失败的退出码要和「没连上」同一档，实际正文：{:?}",
            info.outcome
        );
    }

    /// 「连错窗口」这件事只有把两个来源并排放才看得见：发现文件说桥接以为是谁，
    /// `ping` 回包说谁真的答了。`song_session` 顶不上这个位置——另存成别的文件它也不动。
    #[tokio::test]
    async fn server_info_names_the_process_that_answered() {
        let fake = FakeEditor::start(&[Fate::AnswerOnce]);
        let live = Bridge::detached(fake.endpoint.clone());
        let info = live.bridge_info().await;
        let face = info.outcome.as_ref().expect("有正文").clone();
        let instance = &face["instance"];
        assert_eq!(
            instance["editor_pid"], FAKE_EDITOR_PID,
            "编辑器自报的 pid 要原样进回执，实际：{instance}"
        );
        assert_eq!(
            instance["source"], "direct",
            "这份夹具走的是 --port/--token，没发现文件可翻：{instance}"
        );
        assert_eq!(
            instance["same"], serde_json::Value::Null,
            "只有一个来源时判不了对不对得上，得回 null 而不是 false：{instance}"
        );
        assert_eq!(
            instance["port"],
            u32::from(fake.endpoint.direct.expect("夹具是直连").0),
            "直连也要把实际敲的端口报出来：{instance}"
        );
        assert!(
            serde_json::to_string(&instance)
                .unwrap()
                .find("token")
                .is_none(),
            "这一格里不能出现令牌"
        );
    }

    /// 发现文件是上一个实例留下的、或同机开了好几个编辑器：这一格要把「对不上」摆出来，
    /// 否则调用方只能拿 `rev` 归零、`dirty` 为假这类正常现象去猜自己连错了人。
    #[tokio::test]
    async fn server_info_spots_a_stale_discovery_file() {
        let fake = FakeEditor::start(&[Fate::AnswerOnce]);
        let (port, _) = fake.endpoint.direct.expect("夹具给的是端口和令牌");
        let dir = std::env::temp_dir().join(format!("nbs-mcp-instance-{port}"));
        let write = |pid: u32| {
            nbs_control::Discovery {
                protocol: nbs_control::PROTOCOL_VERSION,
                app: "nbs-fake 0.0.0-test".to_string(),
                pid,
                port,
                token: "tok".to_string(),
                started_unix: 1_700_000_000,
            }
            .write_to(&dir)
            .expect("发现文件写不下去");
        };

        // 文件里写的就是答题那一个：same = true。
        write(FAKE_EDITOR_PID);
        let ep = Endpoint {
            config_dir: dir.clone(),
            direct: None,
            client_name: "test".to_string(),
        };
        let info = Bridge::detached(ep.clone()).bridge_info().await;
        let instance = info.outcome.as_ref().expect("有正文")["instance"].clone();
        assert_eq!(instance["source"], "discovery", "实际：{instance}");
        assert_eq!(instance["discovery_pid"], FAKE_EDITOR_PID, "实际：{instance}");
        assert_eq!(instance["same"], true, "两个来源该说同一件事：{instance}");

        // 文件被另一个实例改写过（或者端口被别的过程占了）：same = false，且两边的数都摆出来。
        write(FAKE_EDITOR_PID + 1);
        let info = Bridge::detached(ep).bridge_info().await;
        let instance = info.outcome.as_ref().expect("有正文")["instance"].clone();
        assert_eq!(
            instance["same"], false,
            "自报 {} 对不上文件 {}：{instance}",
            instance["editor_pid"], instance["discovery_pid"]
        );
        assert_eq!(instance["editor_pid"], FAKE_EDITOR_PID, "实际：{instance}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
