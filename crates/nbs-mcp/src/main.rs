//! nbs-mcp 可执行：stdio 上讲 MCP，背后接着正在跑的那个编辑器窗口。
//!
//! stdout 只留给协议字节，人和日志一律 stderr —— 编辑器没起来时也要靠这条通道说话。
//! 唯一的例外是 `--help`：它发生在协议开始之前，是给人看的用法，不是日志。

use rmcp::{transport::stdio, ServiceExt};

use nbs_mcp::{connect, dump_tools, parse_args, Bridge, Command, USAGE};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = parse_args(&args).unwrap_or_else(|message| {
        eprintln!("nbs-mcp: {message}");
        std::process::exit(2);
    });
    let endpoint = match command {
        // `--help` 是唯一往 stdout 写字节的分支：协议还没开始，且这是人在终端里问的。
        Command::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Command::DumpTools => {
            dump_tools(std::io::stderr())?;
            return Ok(());
        }
        Command::Serve(endpoint) => endpoint,
    };

    // 先把连接建起来再交管道：带着一条坏连接去讲 MCP，客户端只会看见「工具全在，一调就错」。
    let client = match connect(&endpoint) {
        Ok(client) => client,
        Err(message) => {
            eprintln!("nbs-mcp: {message}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "nbs-mcp: 已连上 {}（{}），stdio 交给 MCP 客户端",
        bridge_label(&client),
        endpoint.config_dir.display()
    );
    let bridge = Bridge::new(client);

    // `enable_all`：rmcp 的超时走 tokio 时间驱动，只起线程池会让它 panic 在「timer not running」。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let service = bridge
            .serve(stdio())
            .await
            .map_err(|e| format!("MCP 服务起不来: {e}"))?;
        // 客户端关掉 stdin 就收束；socket 跟着 Bridge 的 drop 一起关。
        let _ = service.waiting().await;
        Ok(())
    })
}

/// 握手时服务端自报的应用名；没报就用通用说法，别在日志里留个空字符串。
fn bridge_label(client: &nbs_control::ControlClient) -> String {
    let app = client.server_app();
    if app.is_empty() {
        "nbs-rs 编辑器".to_string()
    } else {
        format!("{app} 编辑器")
    }
}
