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

mod connect;
mod params;

use std::sync::{Arc, Mutex};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use schemars::JsonSchema;
use serde::Serialize;

pub use connect::{connect, parse_args, Command, Endpoint, USAGE};
use nbs_control::{ControlClient, Op, Outcome, Playback};
pub use params::{
    BatchFieldArgs, CellRef, ClearSong, Export, GetNotes, ImportMidi, JobId, LayerAdd, LayerIndex,
    LayerMove, LayerUpdate, OpenPath, RemoveCells, RevOnly, RunAction, SavePath, Seek, Select,
    SetLoop, SetMetaArgs, SetNotes, Steps, Transpose,
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

/// 把工具面打到 `out`：给人核对「AI 会看到什么口径」，不需要编辑器在场，也不碰 stdout。
pub fn dump_tools(mut out: impl std::io::Write) -> std::io::Result<()> {
    let mut tools = Bridge::tool_router().list_all();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
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
    Ok(())
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

    /// 发一条 `Op` 并等回执。阻塞的网络读放在 `spawn_blocking` 里，
    /// 免得占住异步线程；失败不进 `Result`，而是进 [`Reply::transport`]——
    /// 让 AI 看见「没送到」比看见一个协议级错误更有用。
    async fn call(&self, op: Op) -> Json<Reply> {
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
        Json(match joined {
            Ok(Ok(outcome)) => Reply::from_outcome(outcome, &self.channel),
            Ok(Err(err)) => Reply::transport(err.to_string()),
            Err(_) => Reply::transport("桥接内部：等待编辑器回执的任务被取消了"),
        })
    }

    // -------------------------------------------------------------- 通道 / 逃生舱

    /// 探活：确认这条 MCP 通道真的连着某个编辑器窗口，并核对协议版本。
    /// 不带状态改动。别的工具报 `transport` 时先用它判「是编辑器没起，还是参数不对」。
    #[tool]
    async fn ping(&self) -> Json<Reply> {
        self.call(Op::Ping).await
    }

    /// 列出逃生舱 `run_action` 能调的全部界面动作：名字、一句话说明、参数形状，
    /// 以及它要不要 danger / discard_changes 这类显式认错。
    /// 结构化工具没覆盖的按钮都在这里；先调它，别猜动作名。
    #[tool]
    async fn list_actions(&self) -> Json<Reply> {
        self.call(Op::ListActions).await
    }

    /// 逃生舱：按名字调用一条界面动作，参数原样放进 `args`。
    /// 只接受 `list_actions` 里出现过的名字，未知名字一律拒绝而不是忽略；
    /// 回执里会带回动作自己的提示语和变更量。
    #[tool]
    async fn run_action(&self, Parameters(a): Parameters<RunAction>) -> Json<Reply> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 读

    /// 工程概览：文件路径、脏标记、元数据、图层数、音符数、长度、播放头、
    /// `rev` / `song_session`，外加兼容性诊断。想先看清局面就调这个。
    #[tool]
    async fn get_snapshot(&self) -> Json<Reply> {
        self.call(Op::GetSnapshot).await
    }

    /// 读一段范围内的音符格子。返回上限由服务端定，`truncated=true` 说明只给了一部分，
    /// 需要缩小 `tick_from..=tick_to` 再读。
    #[tool]
    async fn get_notes(&self, Parameters(a): Parameters<GetNotes>) -> Json<Reply> {
        self.call(Op::GetNotes {
            tick_from: a.tick_from,
            tick_to: a.tick_to,
            layer_from: a.layer_from,
            layer_to: a.layer_to,
        })
        .await
    }

    /// 图层清单：层号、名字、音量、左右声道、锁定 / solo 状态。
    #[tool]
    async fn get_layers(&self) -> Json<Reply> {
        self.call(Op::GetLayers).await
    }

    /// 当前选区：是否浮动选区、包围盒、音符数。写之前想知道「selection 是哪儿」就看它。
    #[tool]
    async fn get_selection(&self) -> Json<Reply> {
        self.call(Op::GetSelection).await
    }

    /// 音色表：内置 0..=14 加自定义槽位，含每个槽位当前挂的音色名。
    /// 写音符前用它确认 `instrument` 号真的存在。
    #[tool]
    async fn get_instruments(&self) -> Json<Reply> {
        self.call(Op::GetInstruments).await
    }

    /// 兼容性诊断：超出 Minecraft 两个八度的音符、原版没有的音色、锁层引用等。
    /// 只读，不改状态。
    #[tool]
    async fn get_diagnostics(&self) -> Json<Reply> {
        self.call(Op::GetDiagnostics).await
    }

    /// 撤销栈：当前游标、长度，以及每一步的说明文字。
    #[tool]
    async fn get_history(&self) -> Json<Reply> {
        self.call(Op::GetHistory).await
    }

    // -------------------------------------------------------------- 写

    /// 批量写格子：一次调用 = 一次撤销，整批先校验后落笔，要么全成要么全不动。
    /// 回执的 `report.rev` 请留给下一次写操作的 `expect_rev`。
    #[tool]
    async fn set_notes(&self, Parameters(a): Parameters<SetNotes>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 按坐标摘掉若干格子。空格子不算改动，所以 `cells_changed` 可能小于你给的条数。
    #[tool]
    async fn remove_cells(&self, Parameters(a): Parameters<RemoveCells>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 批量改某个字段（音色 / 音高 / 音量 / 声道 / 音分）。
    /// `mode="add"` 在现值上加，越界由服务端夹取并在回执里说明夹了多少，不会静默丢弃。
    #[tool]
    async fn batch_field(&self, Parameters(a): Parameters<BatchFieldArgs>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 整体或局部移调。`scope` 决定动谁：整首歌、当前浮动选区，还是给定矩形内已有音符的格子。
    /// 越界夹到 0..=87 并在回执里说明。
    #[tool]
    async fn transpose(&self, Parameters(a): Parameters<Transpose>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 选区：框选 / 全选 / 反选 / 按音色 / 超出 MC 音域 / 自定义音色。
    /// 摘出的浮动选区会挡住写操作，写完记得 `place_selection` 或用 `select` 取消。
    #[tool]
    async fn select(&self, Parameters(a): Parameters<Select>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 把浮动选区就地落回歌曲，等价于「取消选择但不丢改动」。
    /// 有浮动选区时 `set_notes` 这类写操作会被状态闸挡回来，所以这一步不能省。
    #[tool]
    async fn place_selection(&self, Parameters(a): Parameters<RevOnly>) -> Json<Reply> {
        self.call(Op::PlaceSelection {
            expect_rev: a.expect_rev,
        })
        .await
    }

    /// 加图层：`at` 省略 = 追加到末尾。
    #[tool]
    async fn layer_add(&self, Parameters(a): Parameters<LayerAdd>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 删图层。锁定层的音符删之前先 `get_snapshot` 看一眼，回执会说明丢了几格。
    #[tool]
    async fn layer_remove(&self, Parameters(a): Parameters<LayerIndex>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 改图层属性：名字、音量、声道、锁定状态（normal / locked / solo）。省略的字段不动。
    #[tool]
    async fn layer_update(&self, Parameters(a): Parameters<LayerUpdate>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 移动图层顺序，音符跟着层一起走。
    #[tool]
    async fn layer_move(&self, Parameters(a): Parameters<LayerMove>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 改元数据：名字、作者、说明、速度、拍号、循环。省略的字段不动，
    /// 一个都没给会被拒（避免「空提交也报成功」的假象）。
    #[tool]
    async fn set_meta(&self, Parameters(a): Parameters<SetMetaArgs>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 撤销。`steps` 省略 = 1 步。回执的 `report` 会说清实际退了几步。
    #[tool]
    async fn undo(&self, Parameters(a): Parameters<Steps>) -> Json<Reply> {
        self.call(Op::Undo { steps: a.steps }).await
    }

    /// 重做。`steps` 省略 = 1 步。
    #[tool]
    async fn redo(&self, Parameters(a): Parameters<Steps>) -> Json<Reply> {
        self.call(Op::Redo { steps: a.steps }).await
    }

    /// 清空成一首空歌。工程有未保存改动时必须带 `discard_changes=true`：
    /// 这一步会连改动和全部撤销历史一起丢掉，不带就等于没认错。
    #[tool]
    async fn clear_song(&self, Parameters(a): Parameters<ClearSong>) -> Json<Reply> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 播放

    /// 从当前落点起播（和界面播放按钮同一语义：停在歌尾会先回 0）。
    #[tool]
    async fn play(&self) -> Json<Reply> {
        self.call(Op::Playback(Playback::Play)).await
    }

    /// 暂停，保留播放头位置。
    #[tool]
    async fn pause(&self) -> Json<Reply> {
        self.call(Op::Playback(Playback::Pause)).await
    }

    /// 停止并把播放头送回起点。
    #[tool]
    async fn stop(&self) -> Json<Reply> {
        self.call(Op::Playback(Playback::Stop)).await
    }

    /// 移动播放头。tick 可以是小数（落在两格之间），负数被拒。
    #[tool]
    async fn seek(&self, Parameters(a): Parameters<Seek>) -> Json<Reply> {
        self.call(Op::Playback(Playback::Seek { tick: a.tick }))
            .await
    }

    /// 只改循环开关，不动其他元数据。
    #[tool]
    async fn set_loop(&self, Parameters(a): Parameters<SetLoop>) -> Json<Reply> {
        self.call(Op::Playback(Playback::SetLoop { enabled: a.enabled }))
            .await
    }

    // -------------------------------------------------------------- 文件

    /// 打开一个 .nbs / .schematic 文件替换当前工程，不弹文件框。
    /// 路径必须落在编辑器启动时允许的目录里；有未保存改动时要带 `discard_changes=true`。
    #[tool]
    async fn open_file(&self, Parameters(a): Parameters<OpenPath>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 存回当前文件名。没有文件名时报错，绝不弹「另存为」。
    #[tool]
    async fn save(&self) -> Json<Reply> {
        self.call(Op::Save).await
    }

    /// 存到指定路径。目标已存在且不是本工程自己的文件时要带 `overwrite=true`；
    /// 存回当前文件名不需要它。
    #[tool]
    async fn save_as(&self, Parameters(a): Parameters<SavePath>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 导入 MIDI 并**替换**当前工程：撤销历史清空，有未保存改动时要带 `discard_changes=true`。
    #[tool]
    async fn import_midi(&self, Parameters(a): Parameters<ImportMidi>) -> Json<Reply> {
        self.call(a.into()).await
    }

    /// 导出 wav / midi / png。重活**不**在这一次调用里做完：回执 `data.id` 是任务句柄，
    /// 之后用 `job_status` 轮询到 done/failed。目标文件已存在时要带 `overwrite=true`；
    /// 路径同样受启动时允许的目录约束。
    #[tool]
    async fn export(&self, Parameters(a): Parameters<Export>) -> Json<Reply> {
        self.call(a.into()).await
    }

    // -------------------------------------------------------------- 任务

    /// 查一个后台任务的状态：回执 `data` 里给 `status`（queued/running/done/failed/cancelled）、
    /// `progress`、`message`，成功完活时 `result` 带产物路径与统计。
    #[tool]
    async fn job_status(&self, Parameters(a): Parameters<JobId>) -> Json<Reply> {
        self.call(Op::JobStatus { id: a.id }).await
    }

    /// 列出当前全部任务，用来找回句柄（比如桥接重启过）；条目在回执 `data.jobs` 里。
    #[tool]
    async fn job_list(&self) -> Json<Reply> {
        self.call(Op::JobList).await
    }

    /// 取消任务。已经在写的文件不保证回滚，回执会说实话。
    #[tool]
    async fn job_cancel(&self, Parameters(a): Parameters<JobId>) -> Json<Reply> {
        self.call(Op::JobCancel { id: a.id }).await
    }

    // -------------------------------------------------------------- 桥接自己

    /// 这条 MCP 通道连的是哪个编辑器、用什么参数连上的。
    /// 不带网络请求——回执只描述桥接这一侧，别把它当成编辑器的状态。
    #[tool]
    async fn server_info(&self) -> Json<Reply> {
        Json(Reply {
            outcome: Some(serde_json::json!({
                "ok": true,
                "channel": self.channel(),
                "bridge": env!("CARGO_PKG_VERSION"),
                "protocol": nbs_control::PROTOCOL_VERSION,
            })),
            transport: None,
            channel: Some(self.channel().to_string()),
        })
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
