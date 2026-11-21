//! 一次性命令行调用（`--call`）：工具名 → `Op` 的表，加上给脚本看的那一行输出。
//!
//! 为什么不复用 rmcp 的工具路由：路由的 `call` 要一个 `RequestContext`，它里面的 `Peer`
//! 只有服务端传输层造得出来（`Peer::new` 是 rmcp 私有的），命令行这一侧拿不到。
//! 于是「表和工具面口径一致」这件事改由两样东西撑住：
//!
//! 1. `Params → Op` 只有 `params` 里那一份 `From` 实现，工具方法体和这张表都走它；
//! 2. 下面 `table_covers_every_tool` 那条测试拿路由上真实注册的名字逐条核对，漏一条就失败。
//!
//! 参数校验的口径因此和 MCP 完全相同（同一套结构体、同一套 serde 规则），差别只在「谁触发」：
//! 一条 shell 就能问编辑器一句，不用开 IDE。

use serde::de::DeserializeOwned;
use serde_json::Value;

use nbs_control::{Data, Op, Outcome, Playback};

use crate::params::{
    BatchFieldArgs, ClearSong, Export, GetNotes, ImportMidi, JobId, LayerAdd, LayerIndex,
    LayerMove, LayerUpdate, OpenPath, RemoveCells, RevOnly, RunAction, SavePath, Seek, Select,
    SetLoop, SetMetaArgs, SetNotes, Steps, Transpose,
};
use crate::Reply;

/// 名字不在表上、参数读不进那个工具的结构体，或这条根本不发往编辑器。
/// 三种都没敲到编辑器，所以退出码要和「编辑器答了个不」分开：
/// 前者改命令，后者改参数或改工程状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    Unknown(String),
    BadArgs { tool: String, message: String },
    BridgeLocal(String),
}

impl DispatchError {
    /// 打给用户看的一行，带上下一步该敲什么。
    pub fn message(&self) -> String {
        match self {
            Self::Unknown(name) => format!(
                "没有这个工具 {name:?}；`nbs-mcp --names` 看全表"
            ),
            Self::BadArgs { tool, message } => {
                format!("{tool} 的参数不对：{message}；形状看 `--dump-tools {tool}`")
            }
            Self::BridgeLocal(name) => format!(
                "{name} 由桥接自己回答，不带工程状态；要整份 JSON 加 --json"
            ),
        }
    }
}

/// 桥接自己回答、不经过编辑器的工具。列在这里而不是塞进 [`op_for`]，是因为它们压根没有 `Op`。
pub const BRIDGE_LOCAL: &[&str] = &["server_info"];

pub fn is_bridge_local(name: &str) -> bool {
    BRIDGE_LOCAL.contains(&name)
}

/// 一条 `--call` 要做的事。分两支是因为有一条压根不发往编辑器。
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Editor(Op),
    BridgeInfo,
}

/// 先定计划再连端口：名字拼错、必填缺了，都不该为了报个错去开一条 socket。
/// 顺序反了的话，编辑器没起来时你看到的永远是「连不上」，而不是「你工具名打错了」。
pub fn plan(name: &str, args: &Value) -> Result<Plan, DispatchError> {
    if is_bridge_local(name) {
        return Ok(Plan::BridgeInfo);
    }
    Ok(Plan::Editor(op_for(name, args)?))
}

/// `--call` 给的工具名 + 参数 → 一条 `Op`。
pub fn op_for(name: &str, args: &Value) -> Result<Op, DispatchError> {
    if is_bridge_local(name) {
        return Err(DispatchError::BridgeLocal(name.to_string()));
    }
    match name {
        // 无参工具：参数给了也不认，免得把「拼错的名字」读成「省略」。
        "ping" => Ok(Op::Ping),
        "list_actions" => Ok(Op::ListActions),
        "get_snapshot" => Ok(Op::GetSnapshot),
        "get_layers" => Ok(Op::GetLayers),
        "get_selection" => Ok(Op::GetSelection),
        "get_instruments" => Ok(Op::GetInstruments),
        "get_diagnostics" => Ok(Op::GetDiagnostics),
        "get_history" => Ok(Op::GetHistory),
        "job_list" => Ok(Op::JobList),
        "save" => Ok(Op::Save),
        "play" => Ok(Op::Playback(Playback::Play)),
        "pause" => Ok(Op::Playback(Playback::Pause)),
        "stop" => Ok(Op::Playback(Playback::Stop)),
        // 一个结构体对应两个动作的，方向在这张表里定（参数校验仍走同一个结构体）。
        "undo" => Ok(Op::Undo {
            steps: typed::<Steps>(name, args)?.steps,
        }),
        "redo" => Ok(Op::Redo {
            steps: typed::<Steps>(name, args)?.steps,
        }),
        "job_status" => Ok(Op::JobStatus {
            id: typed::<JobId>(name, args)?.id,
        }),
        "job_cancel" => Ok(Op::JobCancel {
            id: typed::<JobId>(name, args)?.id,
        }),
        // 其余每条的 `Params → Op` 只有一份实现，工具面走的也是那一条。
        "run_action" => Ok(typed::<RunAction>(name, args)?.into()),
        "get_notes" => Ok(typed::<GetNotes>(name, args)?.into()),
        "set_notes" => Ok(typed::<SetNotes>(name, args)?.into()),
        "remove_cells" => Ok(typed::<RemoveCells>(name, args)?.into()),
        "batch_field" => Ok(typed::<BatchFieldArgs>(name, args)?.into()),
        "transpose" => Ok(typed::<Transpose>(name, args)?.into()),
        "select" => Ok(typed::<Select>(name, args)?.into()),
        "place_selection" => Ok(typed::<RevOnly>(name, args)?.into()),
        "layer_add" => Ok(typed::<LayerAdd>(name, args)?.into()),
        "layer_remove" => Ok(typed::<LayerIndex>(name, args)?.into()),
        "layer_update" => Ok(typed::<LayerUpdate>(name, args)?.into()),
        "layer_move" => Ok(typed::<LayerMove>(name, args)?.into()),
        "set_meta" => Ok(typed::<SetMetaArgs>(name, args)?.into()),
        "clear_song" => Ok(typed::<ClearSong>(name, args)?.into()),
        "seek" => Ok(typed::<Seek>(name, args)?.into()),
        "set_loop" => Ok(typed::<SetLoop>(name, args)?.into()),
        "open_file" => Ok(typed::<OpenPath>(name, args)?.into()),
        "save_as" => Ok(typed::<SavePath>(name, args)?.into()),
        "import_midi" => Ok(typed::<ImportMidi>(name, args)?.into()),
        "export" => Ok(typed::<Export>(name, args)?.into()),
        other => Err(DispatchError::Unknown(other.to_string())),
    }
}

fn typed<T: DeserializeOwned>(tool: &str, args: &Value) -> Result<T, DispatchError> {
    serde_json::from_value(args.clone()).map_err(|e| DispatchError::BadArgs {
        tool: tool.to_string(),
        message: e.to_string(),
    })
}

/// 退出码：0 = 编辑器答「成」；1 = 编辑器答「不」（工程状态没按你要的改）；
/// 2 = 桥接没问到，或命令本身不对。脚本靠这一个数分「重试」还是「改参数」。
pub fn exit_code(reply: &Reply) -> i32 {
    if reply.transport.is_some() {
        2
    } else if reply.succeeded() {
        0
    } else {
        1
    }
}

/// `--call` 打到 stdout 的内容。默认一行摘要，`full`（即 `--json`）时是整份回执 JSON（含 `data`）。
/// 摘要刻意不带 `data`：`get_notes` 全曲几万像素塞进一行没法看，要就读 `--json` 那份。
pub fn render(name: &str, reply: &Reply, full: bool) -> String {
    if full {
        return serde_json::to_string(reply).unwrap_or_else(|_| "{}".to_string());
    }
    let Some(outcome) = reply.outcome.as_ref() else {
        let reason = reply
            .transport
            .as_deref()
            .unwrap_or("桥接没有拿到任何回执");
        return format!("{name} FAIL transport {}", quoted(reason));
    };
    let Ok(outcome) = serde_json::from_value::<Outcome>(outcome.clone()) else {
        // 桥接自己答的那条（`server_info`）没有 `report` 那一段，套不进 `Outcome`。
        // 这里不能顺势报 FAIL：退出码是按 `ok` 算的，一行摘要说失败而退出码说成功，
        // 脚本 `&&` 接下去就看见两份真相。认不出形状就原样把那一行摊开。
        let ok = outcome.get("ok").and_then(Value::as_bool) == Some(true);
        let state = serde_json::to_string(outcome).unwrap_or_else(|_| "{}".to_string());
        return format!("{name} {} {state}", if ok { "ok" } else { "FAILED" });
    };
    let report = &outcome.report;
    let state = format!(
        "rev={} cells_changed={} layers_changed={} meta_changed={} dirty={}",
        report.rev,
        report.cells_changed,
        report.layers_changed,
        report.meta_changed,
        report.dirty
    );
    if !outcome.ok {
        // 不 expect：回执是外部字节，读不懂时也要把话说完再退出。
        let (code, reason) = match outcome.error.as_ref() {
            Some(e) => (e.code.as_str(), e.message.clone()),
            None => ("internal", "回执说失败，但没写原因".to_string()),
        };
        return format!("{name} FAILED {code} {state} {}", quoted(&reason));
    }
    let data = match data_tag(outcome.data.as_ref()) {
        // `Empty` 只是「这个动作没有返回值」，别让它看起来像「数据是空的」。
        None => String::new(),
        Some(tag) => format!(" data={tag}"),
    };
    format!("{name} ok {state}{data} {}", quoted(&report.message))
}

/// 数据体的 tag 名（`layers` / `snapshot` / `notes` / …）：脚本据此知道该不该再读 `--json` 那份。
/// 认 tag 而不是逐个枚举变体，是为了加新 `Data` 时不用回来改这里。
fn data_tag(data: Option<&Data>) -> Option<String> {
    let data = data?;
    if matches!(data, Data::Empty) {
        return None;
    }
    serde_json::to_value(data)
        .ok()?
        .get("kind")?
        .as_str()
        .map(str::to_string)
}

/// 一行里出现的自由文本都要过这个：JSON 转义管得住引号和换行，中文原样留着。
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| r#""""#.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bridge;
    use serde_json::json;

    fn payload() -> Value {
        Value::Object(Default::default())
    }

    /// 表漏一条，症状是「IDE 里看得见的工具，命令行说没有」——最容易在加完工具之后发生。
    #[test]
    fn table_covers_every_tool() {
        let names: Vec<String> = Bridge::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(!names.is_empty());
        for name in &names {
            let outcome = op_for(name, &payload());
            if is_bridge_local(name) {
                assert!(
                    matches!(outcome, Err(DispatchError::BridgeLocal(_))),
                    "{name} 在 BRIDGE_LOCAL 里，就该由桥接自己答"
                );
                continue;
            }
            if let Err(DispatchError::Unknown(missing)) = outcome {
                panic!("工具面有 {missing:?} 但 --call 的表没有；加一条 arm");
            }
        }
        // 反过来也咬得住：表里的名字必须真在路由上（拼死的工具不能留在 CLI 里）。
        let table = [
            "ping",
            "list_actions",
            "get_snapshot",
            "get_layers",
            "get_selection",
            "get_instruments",
            "get_diagnostics",
            "get_history",
            "job_list",
            "save",
            "play",
            "pause",
            "stop",
            "undo",
            "redo",
            "job_status",
            "job_cancel",
            "run_action",
            "get_notes",
            "set_notes",
            "remove_cells",
            "batch_field",
            "transpose",
            "select",
            "place_selection",
            "layer_add",
            "layer_remove",
            "layer_update",
            "layer_move",
            "set_meta",
            "clear_song",
            "seek",
            "set_loop",
            "open_file",
            "save_as",
            "import_midi",
            "export",
        ];
        for name in table {
            assert!(names.iter().any(|n| n == name), "表里的 {name} 不在工具面上");
        }
        assert_eq!(names.len(), table.len() + BRIDGE_LOCAL.len());
    }

    /// 报错顺序也是口径：工具名拼错要在「连不上」之前说出来，否则编辑器没起时
    /// 你永远看不到真正的原因。
    #[test]
    fn plan_decides_before_any_socket_is_opened() {
        assert_eq!(plan("ping", &payload()).unwrap(), Plan::Editor(Op::Ping));
        assert_eq!(plan("server_info", &payload()).unwrap(), Plan::BridgeInfo);
        assert_eq!(
            plan("get_note", &payload()).unwrap_err(),
            DispatchError::Unknown("get_note".to_string())
        );
    }

    #[test]
    fn an_unknown_name_is_not_a_silent_empty_call() {
        let err = op_for("get_note", &payload()).unwrap_err();
        assert!(matches!(err, DispatchError::Unknown(_)), "{err:?}");
        assert!(err.message().contains("--names"), "{}", err.message());
    }

    /// 缺必填参数要报错，不能退成「当没给处理」——那会把一次写变成另一次写。
    #[test]
    fn missing_required_args_are_rejected() {
        let err = op_for("export", &payload()).unwrap_err();
        assert!(matches!(err, DispatchError::BadArgs { .. }), "{err:?}");
        assert!(op_for("export", &json!({"path":"a.wav","format":"wav"})).is_ok());
        // 全默认参数的工具，空对象就是合法调用。
        assert!(op_for("get_notes", &payload()).is_ok());
    }

    /// 转换必须和工具面给的 `Op` 一字不差，否则命令行和 MCP 会走出两条路。
    #[test]
    fn params_land_on_the_same_op_as_the_tool_face() {
        assert_eq!(
            op_for("get_notes", &json!({"tick_from":0,"tick_to":31})).unwrap(),
            Op::GetNotes {
                tick_from: Some(0),
                tick_to: Some(31),
                layer_from: None,
                layer_to: None
            }
        );
        assert_eq!(
            op_for("seek", &json!({"tick":4.5})).unwrap(),
            Op::Playback(Playback::Seek { tick: 4.5 })
        );
        assert_eq!(
            op_for("undo", &json!({"steps":3})).unwrap(),
            Op::Undo { steps: Some(3) }
        );
        assert_eq!(
            op_for("layer_remove", &json!({"index":3})).unwrap(),
            Op::LayerRemove {
                index: 3,
                expect_rev: None
            }
        );
        assert_eq!(op_for("stop", &payload()).unwrap(), Op::Playback(Playback::Stop));
    }

    fn reply(outcome: Value) -> Reply {
        Reply {
            outcome: Some(outcome),
            transport: None,
            channel: Some("nbs-rs".to_string()),
        }
    }

    #[test]
    fn summary_line_names_status_rev_and_changes() {
        let ok = reply(json!(
            {"ok":true,"report":{"cells_changed":66,"layers_changed":0,"meta_changed":false,
             "rev":8,"song_session":1,"dirty":true,"history_len":1,"history_cursor":1,
             "message":"音符已全部清空"},"data":{"kind":"layers","layers":[]}}
        ));
        let line = render("clear_song", &ok, false);
        assert!(line.starts_with("clear_song ok rev=8 cells_changed=66"), "{line}");
        assert!(line.contains("data=layers"), "{line}");
        assert!(line.contains("\"音符已全部清空\""), "{line}");
        assert_eq!(exit_code(&ok), 0);

        let bad = reply(json!(
            {"ok":false,"error":{"code":"bad_argument","message":"tick_to 不能小于 tick_from"},
             "report":{"cells_changed":0,"layers_changed":0,"meta_changed":false,"rev":6,
             "song_session":1,"dirty":false,"history_len":0,"history_cursor":0,"message":""}}
        ));
        let line = render("get_notes", &bad, false);
        assert!(line.contains("FAILED bad_argument"), "{line}");
        assert_eq!(exit_code(&bad), 1);
    }

    /// 桥接自己答的那条工具（`server_info`）不带 `report`，套不进 `Outcome` 的形状。
    /// 摘要不能因此报 FAIL：退出码按 `ok` 算给 0，两边各说一套时脚本只会跟着退出码走。
    #[test]
    fn a_bridge_local_answer_is_summarised_not_called_a_shape_error() {
        let ok = reply(json!({"ok":true,"channel":"nbs-rs 0.1.0","bridge":"0.1.0","protocol":1}));
        let line = render("server_info", &ok, false);
        assert!(line.starts_with("server_info ok"), "摘要要说成功：{line}");
        assert!(line.contains("protocol"), "桥接答的内容也要看得见：{line}");
        assert_eq!(exit_code(&ok), 0, "退出码要和那一行说同一件事");

        let bad = reply(json!({"ok":false,"why":"桥接没配到端口"}));
        let line = render("server_info", &bad, false);
        assert!(line.starts_with("server_info FAILED"), "{line}");
        assert_eq!(exit_code(&bad), 1);
    }

    /// 桥接层失败既不是 0 也不是 1：命令没碰到编辑器，改的是启动方式而不是参数。
    #[test]
    fn transport_failure_is_its_own_exit_code() {
        let reply = Reply {
            outcome: None,
            transport: Some("控制服务断开了连接".to_string()),
            channel: None,
        };
        let line = render("get_snapshot", &reply, false);
        assert!(line.contains("FAIL transport"), "{line}");
        assert!(line.contains("控制服务断开了连接"), "{line}");
        // 2 而不是 1：编辑器一格都没被碰到，该改的是启动方式。
        assert_eq!(exit_code(&reply), 2);
    }

    #[test]
    fn full_output_is_the_whole_reply_as_one_json_object() {
        let ok = reply(json!({"ok":true,"report":{"rev":6,"message":"x"},"data":{"kind":"pong","app_version":"0.1.0","protocol":1,"rev":6,"song_session":1}}));
        let line = render("ping", &ok, true);
        let parsed: Value = serde_json::from_str(&line).expect("--json 那行必须是能直接 parse 的 JSON");
        assert_eq!(parsed["outcome"]["data"]["kind"], "pong");
        assert_eq!(parsed["channel"], "nbs-rs");
    }
}
