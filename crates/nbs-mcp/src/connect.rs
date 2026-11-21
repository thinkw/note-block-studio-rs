//! 桥接怎么找到「正在跑的那个编辑器」：端口和 token 都在配置目录的发现文件里。
//!
//! 命令行解析放这里而不是 `main.rs`，是为了让「哪种写法连哪个端口」这件事能被单测钉住
//! ——`nbs-mcp` 一启动就把 stdio 交给 MCP 客户端，届时再发现参数读错了已经来不及说话。

use std::path::PathBuf;
use std::time::Duration;

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
nbs-mcp —— 把正在运行的 nbs-rs 编辑器接成 MCP 服务（stdio）

用法: nbs-mcp [选项]
  --config-dir <目录>   发现文件所在目录，默认按 NBS_RUST_CONFIG_DIR / APPDATA 等顺序找
  --port <端口>         跳过发现文件直连这个端口，必须和 --token 一起给
  --token <令牌>        直连用的令牌
  --client-name <名字>  握手时自报的客户端名，默认 nbs-mcp
  --dump-tools          把工具面（名字 + 说明 + 入参 schema）打到 stderr 后退出
  --help                打这段说明

编辑器那一侧要先起着：nbs-rust.exe <歌.nbs> --control [--control-dir=<目录>] [--control-danger]
嫌旗标麻烦就在编辑器「设置 - 外部控制（MCP）」里勾启用：开窗口即监听，桥接这边参数照旧不用改。
";

/// 解析出来的意图。`--dump-tools` 不连编辑器：它是给人核对「AI 会看到什么口径」的，
/// 编辑器没起来时同样要能跑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve(Endpoint),
    DumpTools,
    Help,
}

/// 解析命令行。
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut ep = Endpoint::default();
    let mut port: Option<u16> = None;
    let mut token: Option<String> = None;
    let mut dump = false;
    let mut help = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        // `--opt=值` 和 `--opt 值` 两种写法都收：MCP 客户端的配置文件里两种都常见。
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        // 每次循环新造一个闭包，match 只走一个分支，所以这里最多调用一次 —— 不需要 FnMut。
        let value = || -> Result<String, String> {
            if let Some(v) = inline {
                return Ok(v);
            }
            it.next()
                .cloned()
                .ok_or_else(|| format!("{key} 后面缺参数值"))
        };
        match key {
            "--help" | "-h" => help = true,
            "--dump-tools" => dump = true,
            "--config-dir" => ep.config_dir = PathBuf::from(value()?),
            "--port" => {
                let text = value()?;
                port = Some(
                    text.parse()
                        .map_err(|_| format!("--port 的参数 {text:?} 不是端口号"))?,
                );
            }
            "--token" => token = Some(value()?),
            "--client-name" => ep.client_name = value()?,
            other => return Err(format!("未知参数 {other}\n\n{USAGE}")),
        }
    }
    if help {
        return Ok(Command::Help);
    }
    if dump {
        return Ok(Command::DumpTools);
    }
    match (port, token) {
        (Some(p), Some(t)) => ep.direct = Some((p, t)),
        (Some(p), None) => return Err(format!("给了 --port {p} 却没给 --token")),
        (None, Some(_)) => return Err("给了 --token 却没给 --port".to_string()),
        (None, None) => {}
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
            Command::DumpTools
        );
        assert_eq!(
            parse_args(&argv(&["--dump-tools", "--port", "5", "--token", "abc"])).unwrap(),
            Command::DumpTools
        );
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
