//! 桥接进程用的客户端：`Hello` 握手 + 一问一答。
//!
//! 单连接内串行发请求，`id` 自增并跟服务端回的对账；并发请用多个连接
//! （每个连接一条线程，服务端本来就按连接串行处理，别在一条连接上抢答）。

use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use crate::bus::{Hello, HelloAck, Request, Response};
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
    /// 对端在回话前就断了。
    Closed,
    /// 回的东西不是本端认识的形状。
    BadReply(String),
    /// 等主线程回执超时（这条连接可以接着用，id 已作废）。
    TimedOut,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(s) => write!(f, "连不上控制服务: {s}"),
            Self::Refused(s) => write!(f, "控制服务拒绝连接: {s}"),
            Self::Closed => write!(f, "控制服务断开了连接"),
            Self::BadReply(s) => write!(f, "控制服务回了不对的东西: {s}"),
            Self::TimedOut => write!(f, "控制服务没有按时回答"),
        }
    }
}

impl std::error::Error for ClientError {}

/// 等一条回执的上限。必须**大于**服务端默认的 `request_timeout`（30s）：
/// 否则服务端还在等主线程，客户端先把这条连接读超时了，请求白排一次。
const REPLY_WAIT: Duration = Duration::from_secs(45);

pub struct ControlClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    server_app: String,
}

impl ControlClient {
    pub fn connect(
        port: u16,
        token: &str,
        client: &str,
        connect_timeout: Duration,
    ) -> Result<Self, ClientError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&addr, connect_timeout)
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        stream
            .set_read_timeout(Some(REPLY_WAIT))
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

    /// 发一条请求并等回执。失败既可能是传输层（[`ClientError`]），也可能是
    /// 语义层（[`Outcome::ok`] == false）——两个都要看，别只判 `is_ok()`。
    pub fn request(&mut self, op: Op) -> Result<Outcome, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request { id, op };
        write_json_line(&mut self.writer, &req).map_err(|_| ClientError::Closed)?;
        let resp: Response = match read_json_line(&mut self.reader) {
            Ok(Some(resp)) => resp,
            Ok(None) => return Err(ClientError::Closed),
            Err(e) => return Err(ClientError::BadReply(e.to_string())),
        };
        if resp.id != id {
            return Err(ClientError::BadReply(format!(
                "请求 id {id}，回来的是 {}",
                resp.id
            )));
        }
        Ok(resp.outcome)
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
