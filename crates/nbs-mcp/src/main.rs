//! nbs-mcp 可执行：stdio 上讲 MCP，背后接着正在跑的那个编辑器窗口。
//!
//! stdout 的规矩：讲协议时它是协议字节，`--call` / `--names` / `--dump-tools --json` 时它是
//! 脚本要的产物，`--help` 时它是给人看的用法。除此之外人和日志一律 stderr ——
//! 编辑器没起来时也要靠那条通道说话，两种字节混在一个流里谁也读不成。

use rmcp::{transport::stdio, ServiceExt};

use nbs_mcp::{
    call, connect, dump_tools, parse_args, Bridge, CallRequest, Command, Endpoint, ToolList, USAGE,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = parse_args(&args).unwrap_or_else(|message| {
        eprintln!("nbs-mcp: {message}");
        std::process::exit(2);
    });
    match command {
        Command::Help => print!("{USAGE}"),
        Command::DumpTools(mode, only) => {
            // 文字版是「给人核对口径」，走 stderr；机器可读的那两种是产物，走 stdout。
            let out: Box<dyn std::io::Write> = match mode {
                ToolList::Text => Box::new(std::io::stderr()),
                ToolList::Json | ToolList::Names => Box::new(std::io::stdout()),
            };
            // 名字打错不能装作成功：只打一句「没这条」，让人去看全表。
            if !dump_tools(out, mode, only.as_deref())? {
                eprintln!(
                    "nbs-mcp: 工具面里没有 {}；`nbs-mcp --names` 看全表名字",
                    only.unwrap_or_default()
                );
                std::process::exit(2);
            }
        }
        Command::Serve(endpoint) => serve(endpoint)?,
        Command::Call(endpoint, request) => run_call(endpoint, request)?,
    }
    Ok(())
}

/// 常驻模式：交管道给 MCP 客户端，直到对方关掉 stdin。
fn serve(endpoint: Endpoint) -> Result<(), Box<dyn std::error::Error>> {
    let client = connection(&endpoint);
    eprintln!(
        "nbs-mcp: 已连上 {}（{}），stdio 交给 MCP 客户端",
        bridge_label(&client),
        endpoint.config_dir.display()
    );
    let bridge = Bridge::new(client);

    runtime()?.block_on(async move {
        let service = bridge
            .serve(stdio())
            .await
            .map_err(|e| format!("MCP 服务起不来: {e}"))?;
        // 客户端关掉 stdin 就收束；socket 跟着 Bridge 的 drop 一起关。
        let _ = service.waiting().await;
        Ok(())
    })
}

/// 一次一条：定计划、连上、调用、把结果打到 stdout、按回执决定退出码。
/// 这条路径不开 stdio 服务，所以脚本不用握手，人也能直接在终端里问一句。
fn run_call(endpoint: Endpoint, request: CallRequest) -> Result<(), Box<dyn std::error::Error>> {
    // 先取来源再解析：读盘失败和 JSON 坏掉是两种不同的下一步，混在一句里只会让人改错地方。
    let text = match request.payload() {
        Ok(text) => text,
        Err(e) => {
            eprintln!("nbs-mcp: {e}");
            std::process::exit(2)
        }
    };
    let args = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(args) => args,
        Err(e) => {
            eprintln!(
                "nbs-mcp: {} 的参数不是合法 JSON（来自{}）: {e}",
                request.tool,
                request.origin()
            );
            std::process::exit(2)
        }
    };
    // 计划先于连接：名字和参数不对时，报错不该被「连不上」盖掉。
    let plan = match call::plan(&request.tool, &args) {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!("nbs-mcp: {}", err.message());
            std::process::exit(2)
        }
    };
    let client = connection(&endpoint);
    let bridge = Bridge::new(client);
    let reply = runtime()?.block_on(bridge.run(plan));
    // 结果打 stdout，成败用退出码说，脚本 `&&` / `||` 就能接。
    println!("{}", call::render(&request.tool, &reply, request.json));
    std::process::exit(call::exit_code(&reply))
}

/// 连不上就说话然后退出。退出码 2 和 `--call` 的「桥接没问到」同一个意思：
/// 该动的是启动方式（编辑器没起 / 没开外部控制 / 目录不对），不是调用参数。
fn connection(endpoint: &Endpoint) -> nbs_control::ControlClient {
    match connect(endpoint) {
        // 先把连接建起来再交管道：带着一条坏连接去讲 MCP，客户端只会看见「工具全在，一调就错」。
        Ok(client) => client,
        Err(message) => {
            eprintln!("nbs-mcp: {message}");
            std::process::exit(2);
        }
    }
}

/// `enable_all`：rmcp 的超时走 tokio 时间驱动，只起线程池会让它 panic 在「timer not running」。
fn runtime() -> Result<tokio::runtime::Runtime, Box<dyn std::error::Error>> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
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
