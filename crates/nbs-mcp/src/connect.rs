//! 桥接怎么找到「正在跑的那个编辑器」：端口和 token 都在配置目录的发现文件里。
//!
//! 命令行解析放这里而不是 `main.rs`，是为了让「哪种写法连哪个端口」这件事能被单测钉住
//! ——`nbs-mcp` 一启动就把 stdio 交给 MCP 客户端，届时再发现参数读错了已经来不及说话。

use std::path::PathBuf;
use std::time::Duration;

use crate::ToolList;
use nbs_control::ControlClient;

/// 连哪儿、怎么连。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// 发现文件所在目录；给了 `--port` 时可以不查。
    pub config_dir: PathBuf,
    /// 显式端口 + token，跳过发现文件（远程调试、或发现文件被删了的时候用）。
    pub direct: Option<(u16, String)>,
    /// 握手时自报的客户端名，会进编辑器的连接日志。
    pub client_name: String,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            // 和编辑器共用 `nbs_core::paths` 那一条规则：「打开窗口就能接上」这件事
            // 全靠两边算出同一个目录。桥接原先自己抄了一份，结果一致但不同源——
            // 规则改了这边不跟着动，症状是「IDE 配置明明填对了却连不上」。
            // 目录不对时的补救口是 `--config-dir`，连不上时报错会说出自己翻过哪个目录。
            config_dir: nbs_core::paths::config_dir(),
            direct: None,
            client_name: "nbs-mcp".to_string(),
        }
    }
}

pub const USAGE: &str = "\
nbs-mcp —— 把正在运行的 nbs-rs 编辑器接成 MCP 服务（stdio），或者一次性只调一条工具

用法: nbs-mcp [选项]
  --config-dir <目录>   发现文件所在目录，默认按 NBS_RUST_CONFIG_DIR / APPDATA 等顺序找
  --port <端口>         跳过发现文件直连这个端口，必须和 --token 一起给
  --token <令牌>        直连用的令牌
  --client-name <名字>  握手时自报的客户端名，默认 nbs-mcp
  --dump-tools [工具]   把工具面（名字 + 说明 + 入参 schema）打到 stderr 后退出；
                        带上工具名只打那一条，省得为了一个参数读完整张表
  --names               只把工具名打到 stdout（= --dump-tools 的名字版）
  --json                机器可读输出：配 --dump-tools 给全表 JSON，配 --call 给整份回执
  --call <工具> ['<json参数>']  连上编辑器、只调这一条、结果打到 stdout 后退出
  --help                打这段说明

一次性调用（不用开 IDE，也不用自己写 stdio 握手脚本）:
  nbs-mcp --call get_snapshot
  nbs-mcp --call clear_song '{\"discard_changes\":true}'
  nbs-mcp --call get_notes '{\"tick_from\":0,\"tick_to\":31}' --json
默认输出是一行状态：`工具 ok rev= cells_changed= layers_changed= meta_changed= dirty= data= \"message\"`；
要看数据体就加 --json。退出码 0 = 编辑器答成，1 = 编辑器答不（闸、参数、当前状态），
2 = 没连上或命令本身不对——1 和 2 分开是因为该改的东西不一样。

编辑器那一侧要先起着：nbs-rust.exe <歌.nbs> --control [--control-dir=<目录>] [--control-danger]
嫌旗标麻烦就在编辑器「设置 - 外部控制（MCP）」里勾启用：开窗口即监听，桥接这边参数照旧不用改。
";

/// 一次性调用的那条工具和它的参数原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRequest {
    pub tool: String,
    /// 参数原文，形状对不对要到 [`crate::call::op_for`] 才知道，那里报得比这里准。
    pub args: String,
    /// true = 打整份回执 JSON，而不是那一行状态。
    pub json: bool,
}

/// 解析出来的意图。`--dump-tools` / `--names` 不连编辑器：它们是给人核对
/// 「AI 会看到什么口径」的，编辑器没起来时同样要能跑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve(Endpoint),
    /// 输出形状 + 可选的「只打这一条工具」。带名字时那张表短到能一次读完，
    /// 报错文案里那句「形状看 `--dump-tools set_notes`」就是指向这个写法。
    DumpTools(ToolList, Option<String>),
    /// 一次性调用也带着 `Endpoint`：`--call` 同样要决定连哪个发现文件 / 哪个端口。
    Call(Endpoint, CallRequest),
    Help,
}

/// 取一个选项的值：`--opt=值` 已经拆好了，`--opt 值` 再往前吃一个参数。
fn take_value(
    args: &[String],
    i: &mut usize,
    inline: Option<String>,
    key: &str,
) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value);
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{key} 后面缺参数值"))
}

/// 解析命令行。
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut ep = Endpoint::default();
    let mut port: Option<u16> = None;
    let mut token: Option<String> = None;
    let mut dump = false;
    let mut only: Option<String> = None;
    let mut names = false;
    let mut json = false;
    let mut call: Option<CallRequest> = None;
    let mut help = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        // `--opt=值` 和 `--opt 值` 两种写法都收：MCP 客户端的配置文件里两种都常见。
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        match key {
            "--help" | "-h" => help = true,
            "--dump-tools" => {
                dump = true;
                // 后面紧跟的不是旗标就当工具名：报错文案里那句「形状看
                // `--dump-tools set_notes`」要真能用，否则人只会多打一份全表。
                if inline.is_some() || matches!(args.get(i + 1), Some(next) if !next.starts_with('-'))
                {
                    only = Some(take_value(args, &mut i, inline, key)?);
                }
            }
            "--names" => names = true,
            "--json" => json = true,
            "--config-dir" => ep.config_dir = PathBuf::from(take_value(args, &mut i, inline, key)?),
            "--port" => {
                let text = take_value(args, &mut i, inline, key)?;
                port = Some(
                    text.parse()
                        .map_err(|_| format!("--port 的参数 {text:?} 不是端口号"))?,
                );
            }
            "--token" => token = Some(take_value(args, &mut i, inline, key)?),
            "--client-name" => ep.client_name = take_value(args, &mut i, inline, key)?,
            "--call" => {
                let tool = take_value(args, &mut i, inline, key)?;
                // 参数 JSON 只在「下一个参数不是旗标」时才吃掉：
                // `--call get_notes --config-dir X` 里那个旗标不是参数。
                // 认「不是旗标」而不是认「看着像 JSON」，坏 JSON 才能走到报 JSON 错的那里，
                // 而不是先变成一句没头没尾的「未知参数 not-json」。
                let payload = args
                    .get(i + 1)
                    .filter(|v| !v.starts_with('-'))
                    .cloned();
                if payload.is_some() {
                    i += 1;
                }
                call = Some(CallRequest {
                    tool,
                    args: payload.unwrap_or_else(|| "{}".to_string()),
                    json: false,
                });
            }
            other => return Err(format!("未知参数 {other}\n\n{USAGE}")),
        }
        i += 1;
    }
    if help {
        return Ok(Command::Help);
    }
    // 直连的判定放在分岔之前：`--call` / `--dump-tools` 也是「连哪儿」的消费者，
    // 只在 Serve 里赋值会让 `--call ping --port 5 --token abc` 悄悄退回发现文件。
    match (port, token) {
        (Some(p), Some(t)) => ep.direct = Some((p, t)),
        (Some(p), None) => return Err(format!("给了 --port {p} 却没给 --token")),
        (None, Some(_)) => return Err("给了 --token 却没给 --port".to_string()),
        (None, None) => {}
    }
    if let Some(mut call) = call {
        if dump || names {
            return Err("--call 和 --dump-tools / --names 不能一起用：一个调编辑器，一个只列表面".to_string());
        }
        call.json = json;
        return Ok(Command::Call(ep, call));
    }
    if dump || names || json {
        // `--json` 单给也算列工具面：它是「输出形状」，不是「做什么」。
        let mode = if names {
            ToolList::Names
        } else if json {
            ToolList::Json
        } else {
            ToolList::Text
        };
        return Ok(Command::DumpTools(mode, only));
    }
    Ok(Command::Serve(ep))
}

/// 连上去。失败原因说清「翻过哪个目录 / 试过哪个端口」，因为这决定了下一步是启动编辑器还是改参数。
pub fn connect(ep: &Endpoint) -> Result<ControlClient, String> {
    if let Some((port, ref token)) = ep.direct {
        return ControlClient::connect(port, token, &ep.client_name, Duration::from_secs(5))
            .map_err(|e| format!("{e}（直连 127.0.0.1:{port}）"));
    }
    ControlClient::from_discovery_dir(&ep.config_dir, &ep.client_name).map_err(|e| {
        format!(
            "{e}\n在 {} 里找发现文件 {file}；编辑器要先用 --control 起：\
             nbs-rust.exe <歌.nbs> --control。目录不对就加 --config-dir，\
             或直接给 --port 和 --token。",
            ep.config_dir.display(),
            file = nbs_control::DISCOVERY_FILE,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn space_and_equals_forms_agree() {
        let a = parse_args(&argv(&[
            "--config-dir",
            "/tmp/x",
            "--client-name",
            "claude",
        ]))
        .unwrap();
        let b = parse_args(&argv(&["--config-dir=/tmp/x", "--client-name=claude"])).unwrap();
        assert_eq!(a, b);
        let Command::Serve(ep) = a else {
            panic!("只给了目录和名字，应该是 Serve")
        };
        assert_eq!(ep.config_dir, PathBuf::from("/tmp/x"));
        assert_eq!(ep.client_name, "claude");
        assert_eq!(ep.direct, None);
    }

    #[test]
    fn direct_needs_both_port_and_token() {
        assert!(parse_args(&argv(&["--port", "5"])).is_err());
        assert!(parse_args(&argv(&["--token", "abc"])).is_err());
        let Command::Serve(ep) = parse_args(&argv(&["--port", "5", "--token", "abc"])).unwrap()
        else {
            panic!("应该是 Serve")
        };
        assert_eq!(ep.direct, Some((5, "abc".to_string())));
        // 端口不是数字要报错，不能悄悄退回去查发现文件。
        assert!(parse_args(&argv(&["--port", "http", "--token", "abc"])).is_err());
    }

    #[test]
    fn help_and_unknown_args_are_explicit() {
        assert_eq!(parse_args(&argv(&["--help"])).unwrap(), Command::Help);
        // MCP 客户端的配置里常见「一个参数都不给」，那必须是照常起服务，不是打用法退出。
        assert!(matches!(parse_args(&argv(&[])).unwrap(), Command::Serve(_)));
        let err = parse_args(&argv(&["--confg-dir", "x"])).unwrap_err();
        assert!(err.contains("未知参数"), "{err}");
        assert!(err.contains("--config-dir"), "报错里要带上正确用法：{err}");
        assert!(parse_args(&argv(&["--port"])).is_err(), "缺值也要报错");
    }

    #[test]
    fn dump_tools_does_not_need_a_running_editor() {
        // 核对口径是离线动作：连着 --port 也只打表，不去敲任何 socket。
        assert_eq!(
            parse_args(&argv(&["--dump-tools"])).unwrap(),
            Command::DumpTools(ToolList::Text, None)
        );
        assert_eq!(
            parse_args(&argv(&["--dump-tools", "--port", "5", "--token", "abc"])).unwrap(),
            Command::DumpTools(ToolList::Text, None)
        );
    }

    /// `--dump-tools <工具>` 是失败回执里指路的那条命令，所以两种写法都要认，
    /// 而且紧跟的旗标不能被当成工具名吃掉（`--dump-tools --json` 是「全表 JSON」）。
    #[test]
    fn one_tool_can_be_dumped_without_dumping_the_whole_face() {
        for form in [
            &["--dump-tools", "set_notes"][..],
            &["--dump-tools=set_notes"][..],
        ] {
            assert_eq!(
                parse_args(&argv(form)).unwrap(),
                Command::DumpTools(ToolList::Text, Some("set_notes".to_string())),
                "{form:?}"
            );
        }
        assert_eq!(
            parse_args(&argv(&["--dump-tools", "set_notes", "--json"])).unwrap(),
            Command::DumpTools(ToolList::Json, Some("set_notes".to_string()))
        );
        // 名字不能和一次性调用混着给，否则「打表」这件事会顺带敲编辑器。
        assert!(parse_args(&argv(&["--dump-tools", "set_notes", "--call", "ping"])).is_err());
    }

    /// 机器可读的两条走 stdout，所以不能和文字版同一个形状；`--json` / `--names`
    /// 单给也算列工具面，因为除了列表面它们没有别的事可做。
    #[test]
    fn machine_readable_tool_lists_are_distinct_modes() {
        assert_eq!(
            parse_args(&argv(&["--dump-tools", "--json"])).unwrap(),
            Command::DumpTools(ToolList::Json, None)
        );
        assert_eq!(
            parse_args(&argv(&["--names"])).unwrap(),
            Command::DumpTools(ToolList::Names, None)
        );
        assert_eq!(
            parse_args(&argv(&["--json"])).unwrap(),
            Command::DumpTools(ToolList::Json, None)
        );
        // 名字版优先：既给 --names 又给 --json 时，人想要的是一列名字。
        assert_eq!(
            parse_args(&argv(&["--dump-tools", "--json", "--names"])).unwrap(),
            Command::DumpTools(ToolList::Names, None)
        );
    }

    #[test]
    fn call_takes_a_name_and_an_optional_json_payload() {
        let parsed = parse_args(&argv(&["--call", "clear_song", r#"{"discard_changes":true}"#]))
            .unwrap();
        let Command::Call(ep, req) = parsed else {
            panic!("应该是 Call，实际 {parsed:?}")
        };
        assert_eq!(req.tool, "clear_song");
        assert_eq!(req.args, r#"{"discard_changes":true}"#);
        assert!(!req.json);
        assert_eq!(ep.direct, None, "没给端口就该走发现文件");

        // 两种写法等价，且不给参数就是空对象（那些工具的参数全可省略）。
        assert_eq!(
            parse_args(&argv(&["--call=get_notes"])).unwrap(),
            parse_args(&argv(&["--call", "get_notes"])).unwrap()
        );
        let Command::Call(_, req) = parse_args(&argv(&["--call", "get_notes"])).unwrap() else {
            panic!()
        };
        assert_eq!(req.args, "{}");

        // JSON 吃紧跟的那一个；下一个看着像旗标就必须留着当旗标。
        let Command::Call(ep, req) =
            parse_args(&argv(&["--call", "get_notes", "--config-dir", "/tmp/x"])).unwrap()
        else {
            panic!()
        };
        assert_eq!(req.args, "{}", "把 --config-dir 当参数吃掉就没法连了");
        assert_eq!(ep.config_dir, PathBuf::from("/tmp/x"));

        // --json 位置随意；它只改输出形状。
        for order in [["--json", "--call", "get_notes"], ["--call", "get_notes", "--json"]] {
            let Command::Call(_, req) = parse_args(&argv(&order)).unwrap() else {
                panic!()
            };
            assert!(req.json, "{order:?}");
        }

        // 直连的两个旗标照样带得进一次性调用。
        let Command::Call(ep, _) =
            parse_args(&argv(&["--call", "ping", "--port", "5", "--token", "abc"])).unwrap()
        else {
            panic!()
        };
        assert_eq!(ep.direct, Some((5, "abc".to_string())));

        assert!(parse_args(&argv(&["--call"])).is_err(), "缺工具名要报错");
        let err = parse_args(&argv(&["--call", "ping", "--names"])).unwrap_err();
        assert!(err.contains("--names"), "混搭要说清为什么不行：{err}");
    }

    /// 参数原文这里不解析：形状对不对要按那条工具的结构体判，那是 `op_for` 的活。
    /// 这里报错只能报「不像 JSON」，还会把 `--call export '{...}'` 的路径引号一起误伤。
    #[test]
    fn call_keeps_the_payload_as_text() {
        for payload in ["{not json", "not-json"] {
            let Command::Call(_, req) =
                parse_args(&argv(&["--call", "get_notes", payload])).unwrap()
            else {
                panic!()
            };
            assert_eq!(req.args, payload, "坏 JSON 也要原样带到能说出为什么坏的地方");
        }
    }

    #[test]
    fn connect_failure_names_the_directory_it_read() {
        let dir = std::env::temp_dir().join(format!("nbs-mcp-no-such-dir-{}", std::process::id()));
        let ep = Endpoint {
            config_dir: dir.clone(),
            ..Default::default()
        };
        // 这一步不建目录：`connect` 只该读发现文件，读不到就必须把翻过的路径说出来。
        let err = match connect(&ep) {
            Ok(_) => panic!("临时目录里没有发现文件，不该连上"),
            Err(err) => err,
        };
        assert!(
            err.contains(&dir.to_string_lossy().to_string()),
            "报错要说出翻过哪个目录：{err}"
        );
        assert!(err.contains("--control"), "要给出下一步：{err}");
    }
}
