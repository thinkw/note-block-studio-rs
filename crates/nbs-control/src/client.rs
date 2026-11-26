//! 桥接进程用的客户端：`Hello` 握手 + 一问一答。
//!
//! 单连接内串行发请求，`id` 自增并跟服务端回的对账；并发请用多个连接
//! （每个连接一条线程，服务端本来就按连接串行处理，别在一条连接上抢答）。

use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use crate::bus::{Capabilities, Hello, HelloAck, Request, Response};
use crate::discovery::Discovery;
use crate::frame::{read_json_line, write_json_line};
use crate::op::Op;
use crate::outcome::{ErrorCode, Outcome};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// 连不上：编辑器没开、没起服务，或者端口写错了。
    Unreachable(String),
    /// token / 协议版本被服务端拒绝。
    Refused(String),
    /// 对端在回话前就断了。**编辑器把空闲 60s 的连接关掉，桥接看见的就是这一条。**
    Closed,
    /// 等主线程回执超时。
    TimedOut,
    /// 回的东西不是本端认识的形状。只有「帧真的坏了」才算这条——
    /// 连接断了、等超时了都不是「格式不对」，把它们报成 BadReply，
    /// 调用方就会去改参数而不是重连。
    BadReply(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(s) => write!(f, "连不上控制服务: {s}"),
            Self::Refused(s) => write!(f, "控制服务拒绝连接: {s}"),
            Self::Closed => write!(f, "控制服务断开了连接"),
            Self::TimedOut => write!(f, "控制服务没有按时回答"),
            Self::BadReply(s) => write!(f, "控制服务回了不对的东西: {s}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// 等一条回执的上限。必须**大于**服务端默认的 `request_timeout`（30s）：
/// 否则服务端还在等主线程，客户端先把这条连接读超时了，请求白排一次。
const REPLY_WAIT: Duration = Duration::from_secs(45);

/// `set_read_timeout` 到期算哪一种 `ErrorKind`，std 在不同版本/平台上给过
/// `WouldBlock`（文档现在这条）也给过 `TimedOut`，两种都认。
/// 判定只用 kind，不拿 Windows 的 10060 原始码——实测这两个 kind 就够了。
fn is_read_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

pub struct ControlClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    server_app: String,
    capabilities: Capabilities,
}

impl ControlClient {
    pub fn connect(
        port: u16,
        token: &str,
        client: &str,
        connect_timeout: Duration,
    ) -> Result<Self, ClientError> {
        Self::connect_with_reply_wait(port, token, client, connect_timeout, REPLY_WAIT)
    }

    /// 把「等一条回执等多久」交出来，只为在单测里把这条路径跑到秒级；
    /// 生产用的上限仍是上面的 `REPLY_WAIT`，别照测试里的短值改。
    pub fn connect_with_reply_wait(
        port: u16,
        token: &str,
        client: &str,
        connect_timeout: Duration,
        reply_wait: Duration,
    ) -> Result<Self, ClientError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&addr, connect_timeout)
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        stream
            .set_read_timeout(Some(reply_wait))
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        let _ = stream.set_nodelay(true);
        let mut writer = stream
            .try_clone()
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        let mut reader = BufReader::new(stream);

        let hello = Hello {
            token: token.to_string(),
            protocol: crate::PROTOCOL_VERSION,
            client: client.to_string(),
        };
        write_json_line(&mut writer, &hello).map_err(|_| ClientError::Closed)?;
        let ack: HelloAck = match read_json_line(&mut reader) {
            Ok(Some(ack)) => ack,
            Ok(None) => return Err(ClientError::Closed),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // 服务在 token/版本不匹配时回的是 Response，不是 HelloAck。
                return Err(ClientError::Refused(e.to_string()));
            }
            Err(_) => return Err(ClientError::Closed),
        };
        if !ack.hello {
            return Err(ClientError::Refused(ack.app));
        }
        Ok(Self {
            reader,
            writer,
            next_id: 1,
            server_app: ack.app,
            capabilities: ack.capabilities,
        })
    }

    /// 从配置目录里的发现文件连上「正在跑的那个编辑器」。
    pub fn from_discovery_dir(dir: &Path, client: &str) -> Result<Self, ClientError> {
        let found =
            Discovery::read_from(dir).map_err(|e| ClientError::Unreachable(e.to_string()))?;
        let mut me = Self::connect(found.port, &found.token, client, Duration::from_secs(2))?;
        me.set_server_app(found.app);
        Ok(me)
    }

    fn set_server_app(&mut self, app: String) {
        if self.server_app.is_empty() {
            self.server_app = app;
        }
    }

    /// 服务端自报的应用名，用来给工具回执加一句「这条通道连的是谁」。
    pub fn server_app(&self) -> &str {
        &self.server_app
    }

    /// 握手时编辑器公布的能力；后台写开关以这一次握手为准。
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// 发一条请求并等回执。失败既可能是传输层（[`ClientError`]），也可能是
    /// 语义层（[`Outcome::ok`] == false）——两个都要看，别只判 `is_ok()`。
    ///
    /// 传输层的失败**一律作废这条连接**（每一条都先走到 [`abandon`](Self::abandon)）：
    /// 一问一答的通道上没有「放弃这一答、接着用」这种安全选项——
    /// 我们不等了，可服务端还在往这条连接上写那一答。留着它，下一问先读到的是
    /// 上一问的回执（旧 `rev`、旧计数），看着像答对了其实答的是过去的事；
    /// 更坏的一版是它压根没来，于是每问都慢到等满。
    /// 重连之后这条判断也不用做：新连接上不存在上一问的回执。
    pub fn request(&mut self, op: Op) -> Result<Outcome, ClientError> {
        self.request_tab(op, None)
    }

    pub fn request_tab(
        &mut self,
        op: Op,
        tab: Option<crate::TabRef>,
    ) -> Result<Outcome, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request { id, tab, op };
        if write_json_line(&mut self.writer, &req).is_err() {
            self.abandon();
            // 写不进去 = 对端早就不认这条连接了（空闲被编辑器关掉就是这个样子）。
            // 别报成「回了不对的东西」：那会让调用方去改参数而不是重连。
            return Err(ClientError::Closed);
        }
        let resp: Response = match read_json_line(&mut self.reader) {
            Ok(Some(resp)) => resp,
            Ok(None) => {
                self.abandon();
                return Err(ClientError::Closed);
            }
            Err(e) if is_read_timeout(&e) => {
                self.abandon();
                return Err(ClientError::TimedOut);
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // 帧真的坏了（超长 / 不是 JSON）。连接本身还活着，但这一问已经没有回执，
                // id 也已经用掉，下一问不会读到它。
                return Err(ClientError::BadReply(e.to_string()));
            }
            Err(_) => {
                // 连接断了 / 被中止（Windows 的 10053、10054 都到这一支）：
                // 上一问已经不会有回执，这条也不再复用。
                self.abandon();
                return Err(ClientError::Closed);
            }
        };
        if resp.id != id {
            self.abandon();
            return Err(ClientError::BadReply(format!(
                "请求 id {id}，回来的是 {}（这条连接已经作废：上一问的回执晚到了）",
                resp.id
            )));
        }
        Ok(resp.outcome)
    }

    /// 作废这条连接：先 flush 再断，免得半写的字节被对端当成坏帧。
    /// 断完之后即使那一答晚到，也只会落在一个没人读的 socket 上。
    fn abandon(&mut self) {
        let _ = self.writer.flush();
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }

    /// 只探活：连不上、被拒、语义失败都归一成 `false`，给「要不要起桥接」用。
    pub fn alive(&mut self) -> bool {
        match self.request(Op::Ping) {
            Ok(outcome) => outcome.ok,
            Err(_) => false,
        }
    }

    /// 优雅收尾：shutdown 写一半的字节会被对端当坏帧，所以先 flush 再关。
    pub fn close(mut self) {
        let _ = self.writer.flush();
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }
}

/// 语义错误码里最该被调用方区分出来的几个，转成 `ClientError` 之外的第二层判断。
pub fn is_transient(outcome: &Outcome) -> bool {
    matches!(
        outcome.error_code(),
        Some(ErrorCode::ModalOpen)
            | Some(ErrorCode::WindowOpen)
            | Some(ErrorCode::Busy)
            | Some(ErrorCode::Timeout)
            | Some(ErrorCode::RevConflict)
    )
}
