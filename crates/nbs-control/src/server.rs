//! 本地回环控制服务：只绑 `127.0.0.1`，握手要 token，消息是行分隔 JSON。
//!
//! 三件事在这里定死：
//! 1. **绝不绑 0.0.0.0**：回环之外任何人都能改你的工程，这是事故不是功能。
//! 2. **绝不在连接线程里碰编辑器状态**：只投递进 [`crate::bus::Inbox`]，等主线程回答。
//! 3. **日志不进 stdout**：stdio MCP 把 stdout 当协议流，这里出错也只能写 stderr。

use std::io::BufReader;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::bus::{Capabilities, Hello, HelloAck, Inbox, Request, Response};
use crate::clients::ClientBoard;
use crate::discovery::{now_unix, pid, Discovery};
use crate::frame::{read_json_line, write_json_line};
use crate::outcome::{ChangeReport, Data, ErrorCode, ErrorInfo, Outcome};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// 自报名字，进握手应答和日志。
    pub app_name: String,
    pub app_version: String,
    pub protocol: u32,
    pub token: String,
    pub inbox: Inbox,
    /// 落发现文件的目录；`None` = 不落（测试或只走显式端口的用法）。
    pub discovery_dir: Option<PathBuf>,
    /// 谁连着就登记在这里，界面按帧读。编辑器持有的是**同一个看板的另一份克隆**，
    /// 所以传进来的必须已经挂在链路上，而不是在这里新造一个。
    pub clients: ClientBoard,
    /// 等主线程回执的上限。主线程正常一帧就答完，超了说明它被长任务或关闭流程占住。
    pub request_timeout: Duration,
    pub max_clients: usize,
    /// 后台标签写开关；设置页修改时由编辑器同步这个共享原子量。
    pub parked_write: Arc<AtomicBool>,
}

impl ServerConfig {
    pub fn new(token: String, inbox: Inbox) -> Self {
        Self {
            app_name: "nbs-rs".to_string(),
            // 版本号由宿主传进来（本 crate 的 CARGO_PKG_VERSION 不是编辑器版本）。
            app_version: String::new(),
            protocol: crate::PROTOCOL_VERSION,
            token,
            inbox,
            discovery_dir: None,
            clients: ClientBoard::default(),
            request_timeout: Duration::from_secs(30),
            max_clients: 8,
            parked_write: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub struct ControlServer {
    port: u16,
    token: String,
    discovery_dir: Option<PathBuf>,
    clients: ClientBoard,
    /// 本趟服务在看板上的任期，停服时只对账自己这一趟。
    epoch: u64,
    shutdown: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// 在 `127.0.0.1:port` 起服务；`port = 0` 让系统挑，实际端口从 [`Self::port`] 拿。
    pub fn start(port: u16, cfg: ServerConfig) -> std::io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
        let actual = listener.local_addr()?.port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let token = cfg.token.clone();
        let discovery_dir = cfg.discovery_dir.clone();

        if let Some(dir) = discovery_dir.clone() {
            let record = Discovery {
                protocol: cfg.protocol,
                app: cfg.app_name.clone(),
                pid: pid(),
                port: actual,
                token: token.clone(),
                started_unix: now_unix(),
            };
            record.write_to(&dir)?;
        }

        let live = Arc::new(AtomicUsize::new(0));
        let thread_shutdown = Arc::clone(&shutdown);
        // 看板先清账再领任期：上一趟服务晚到的收尾碰不到这一趟登记的连接。
        let epoch = cfg.clients.open_epoch();
        let clients = cfg.clients.clone();
        let state = Arc::new(HandlerState {
            cfg,
            live: Arc::clone(&live),
            epoch,
            shutdown: Arc::clone(&thread_shutdown),
        });
        listener.set_nonblocking(true)?;
        let accept = std::thread::Builder::new()
            .name("nbs-control-accept".to_string())
            .spawn(move || {
                // 非阻塞轮询而不是阻塞 accept：停服时不用伪造一条连接去踢醒它。
                while !thread_shutdown.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            // 监听套接字的非阻塞标志会跟着 accept 出来的连接一起下来，
                            // 不关回去一读就是 WouldBlock，连接会被当成对端断开。
                            let _ = stream.set_nonblocking(false);
                            let state = Arc::clone(&state);
                            if let Err(e) = std::thread::Builder::new()
                                .name("nbs-control-conn".to_string())
                                .spawn(move || handle(stream, peer, state))
                            {
                                eprintln!("nbs-control: 起连接线程失败: {e}");
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        Err(e) => {
                            eprintln!("nbs-control: accept 失败: {e}");
                            break;
                        }
                    }
                }
            })?;

        Ok(Self {
            port: actual,
            token,
            discovery_dir,
            clients,
            epoch,
            shutdown,
            accept: Some(accept),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// 停止：先把发现文件删掉（且只删自己写的那份），再收 accept 线程。
    ///
    /// 看板的账在收线程之前就得清：连接线程要等读超时才收尾，最长能拖一分多钟，
    /// 而用户在面板上取消勾选的那一下期望看到的就是「没人连着」。
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.clients.close_epoch(self.epoch);
        if let Some(dir) = self.discovery_dir.take() {
            Discovery::remove_matching(&dir, &self.token);
        }
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // 忘记显式 stop 时也要留下干净的现场：端口没了就别让文件骗人。
        self.shutdown.store(true, Ordering::Relaxed);
        self.clients.close_epoch(self.epoch);
        if let Some(dir) = self.discovery_dir.take() {
            Discovery::remove_matching(&dir, &self.token);
        }
    }
}

struct HandlerState {
    cfg: ServerConfig,
    /// 活着的连接数，用来挡住「客户端忘了关」导致的线程堆积。
    live: Arc<AtomicUsize>,
    /// 本趟服务的看板任期，握手时据此登记连接。
    epoch: u64,
    /// accept 线程用的那同一个闸：连接线程只读它。
    shutdown: Arc<AtomicBool>,
}

fn reject(stream: &mut TcpStream, id: u64, code: ErrorCode, message: &str) {
    let mut outcome = Outcome::err(ErrorInfo::new(code, message), ChangeReport::default());
    outcome.data = Some(Data::Empty);
    let _ = write_json_line(stream, &Response { id, outcome });
}

fn handle(mut stream: TcpStream, peer: SocketAddr, state: Arc<HandlerState>) {
    let mut guard = ConnectionGuard::new(Arc::clone(&state.live));
    if !guard.admitted(state.cfg.max_clients) {
        reject(
            &mut stream,
            0,
            ErrorCode::Busy,
            &format!("并发连接已达上限 {}", state.cfg.max_clients),
        );
        return;
    }
    if state.shutdown.load(Ordering::Relaxed) {
        // 停服那一下已经过了：这条连接是赶在关线程之前握进来的，不能再往里走。
        return;
    }
    let _ = stream.set_nodelay(true);
    // 读一定要超时，否则客户端不管控时这条线程永远停在 read 上；
    // 给到两个请求期限，慢客户端也不会被误杀。
    let _ = stream.set_read_timeout(Some(state.cfg.request_timeout * 2));
    // 读端要自己的那份句柄：BufReader 借走 stream 就没法再往里写。
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let peer = peer.to_string();

    // ---- 握手：token 不对直接关，不告诉对方差在哪一位。
    let hello: Hello = match read_json_line(&mut reader) {
        Ok(Some(h)) => h,
        Ok(None) => return,
        Err(e) => {
            reject(&mut stream, 0, ErrorCode::BadArgument, &e.to_string());
            return;
        }
    };
    if hello.token != state.cfg.token {
        reject(&mut stream, 0, ErrorCode::NotAllowed, "token 不对");
        return;
    }
    if hello.protocol != state.cfg.protocol {
        reject(
            &mut stream,
            0,
            ErrorCode::NotAllowed,
            &format!(
                "协议版本不匹配：服务 {} / 客户端 {}",
                state.cfg.protocol, hello.protocol
            ),
        );
        return;
    }
    let ack = HelloAck {
        hello: true,
        protocol: state.cfg.protocol,
        app: format!("{} {}", state.cfg.app_name, state.cfg.app_version)
            .trim()
            .to_string(),
        capabilities: Capabilities {
            parked_write: state.cfg.parked_write.load(Ordering::Relaxed),
        },
        snapshot: None,
    };
    if write_json_line(&mut stream, &ack).is_err() {
        return;
    }
    // 握手过了才登记：扫端口、token 不对的进程不该显示成「有 AI 连着」。
    // 这个把手活到本函数结尾，连接一断就自动销账。
    let session = state.cfg.clients.attach(state.epoch, &hello.client, &peer);

    // ---- 请求循环：一条一行，同一连接内串行，后一条一定看得到前一条的结果。
    loop {
        let req: Request = match read_json_line(&mut reader) {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                // 只有「帧本身不对」才留在同一连接上继续；读超时或断线一律收尾。
                if e.kind() == std::io::ErrorKind::InvalidData {
                    reject(&mut stream, 0, ErrorCode::BadArgument, &e.to_string());
                    continue;
                }
                break;
            }
        };
        if state.shutdown.load(Ordering::Relaxed) {
            // 闸在中间也要查：连接线程只能等读超时才收尾，最长拖一分多钟，
            // 而用户在面板上取消勾选的那一下，期望是「还在连的客户端从此使唤不动我」。
            break;
        }
        if let Some(session) = &session {
            session.requested();
        }
        let rx =
            state
                .cfg
                .inbox
                .submit_tab(req.id, format!("{peer} {}", hello.client), req.op, req.tab);
        let outcome = match rx.recv_timeout(state.cfg.request_timeout) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => Outcome::err(
                ErrorInfo::new(
                    ErrorCode::Timeout,
                    "主线程没有在期限内取件（可能被长任务或关闭流程占住）",
                ),
                ChangeReport::default(),
            ),
            Err(RecvTimeoutError::Disconnected) => Outcome::err(
                ErrorInfo::new(ErrorCode::Closed, "服务已停止"),
                ChangeReport::default(),
            ),
        };
        if write_json_line(
            &mut stream,
            &Response {
                id: req.id,
                outcome,
            },
        )
        .is_err()
        {
            break;
        }
    }
}

struct ConnectionGuard {
    live: Arc<AtomicUsize>,
    counted: bool,
}

impl ConnectionGuard {
    fn new(live: Arc<AtomicUsize>) -> Self {
        Self {
            live,
            counted: false,
        }
    }

    /// 先增再比上限：宁可短暂多一个连接，也不要「查数—再增」之间的竞态放行超量。
    fn admitted(&mut self, max: usize) -> bool {
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.counted = true;
        now <= max
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if self.counted {
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
