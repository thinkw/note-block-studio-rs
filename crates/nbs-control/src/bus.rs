//! UI 线程收件箱。别的线程**不碰** `EditorState`（它不是 `Send`，也不允许被抢），
//! 只把请求排进这条队列；主线程每帧取走、执行、把回执送回各自的 channel。
//!
//! 请求/回执行格式（`Request` / `Response`）也放在这里，服务端与客户端共用一份定义。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::op::Op;
use crate::outcome::{Data, Outcome, Snapshot};

/// 一条待执行的请求。`id` 由客户端自己编号，服务端原样回填，方便断线重排时对账。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: Op,
}

/// 一条回执。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub outcome: Outcome,
}

/// 握手第一行：客户端声明 token、协议版本和自报身份。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub token: String,
    pub protocol: u32,
    pub client: String,
}

/// 握手指回应答。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelloAck {
    pub hello: bool,
    pub protocol: u32,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Box<Snapshot>>,
}

#[derive(Debug)]
pub struct PendingRequest {
    pub id: u64,
    pub client: String,
    pub op: Op,
    reply: Mutex<Option<Sender<Outcome>>>,
}

impl PendingRequest {
    /// 送回结果。`false` = 客户端已经不等了（超时或断线），调用方不必特殊处理。
    pub fn reply(&self, outcome: Outcome) -> bool {
        // 取出来再发：同一请求被误回两次时，第二次安静失败而不是 panic。
        let sender = self.reply.lock().unwrap().take();
        match sender {
            Some(sender) => sender.send(outcome).is_ok(),
            None => false,
        }
    }
}

type Wake = dyn Fn() + Send + Sync;

/// 克隆出来就能跨线程用：服务端持有它投递，编辑器主线程持有它取件。
#[derive(Clone)]
pub struct Inbox {
    queue: Arc<Mutex<VecDeque<PendingRequest>>>,
    depth: Arc<AtomicUsize>,
    wake: Arc<Wake>,
}

impl Default for Inbox {
    fn default() -> Self {
        Self::idle()
    }
}

/// `wake` 是个 `dyn Fn`，派生不出来，只把手里排着的条数报出来。
impl std::fmt::Debug for Inbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox")
            .field("queued", &self.depth.load(Ordering::Relaxed))
            .finish()
    }
}

impl Inbox {
    /// 没人唤醒的版本，给测试和「主线程自己轮询」的用法。
    pub fn idle() -> Self {
        Self::with_waker(|| ())
    }

    pub fn with_waker<F>(wake: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            depth: Arc::new(AtomicUsize::new(0)),
            wake: Arc::new(wake),
        }
    }

    /// 投递一条请求，拿到等回执的 `Receiver`。投完立刻催一次重绘，
    /// 否则窗口静止时 egui 不出新帧，请求会一直躺在队列里。
    pub fn submit(&self, id: u64, client: impl Into<String>, op: Op) -> Receiver<Outcome> {
        let (tx, rx) = mpsc::channel();
        let req = PendingRequest {
            id,
            client: client.into(),
            op,
            reply: Mutex::new(Some(tx)),
        };
        self.queue.lock().unwrap().push_back(req);
        self.depth.fetch_add(1, Ordering::SeqCst);
        (self.wake)();
        rx
    }

    /// 主线程取走全部待件（取完深度归零，用于「空闲时不催帧」的判断）。
    pub fn drain(&self) -> Vec<PendingRequest> {
        let mut queue = self.queue.lock().unwrap();
        if queue.is_empty() {
            self.depth.store(0, Ordering::SeqCst);
            return Vec::new();
        }
        let out = std::mem::take(&mut *queue);
        self.depth.store(0, Ordering::SeqCst);
        out.into_iter().collect()
    }

    pub fn pending(&self) -> usize {
        self.depth.load(Ordering::SeqCst)
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }
}

/// 给「服务还没接上主线程」的兜底回执：让调用方知道链路通了但没人干活。
pub fn unhandled(id: u64) -> Response {
    use crate::outcome::{ChangeReport, ErrorCode, ErrorInfo};
    Response {
        id,
        outcome: Outcome {
            ok: false,
            error: Some(ErrorInfo::new(
                ErrorCode::Closed,
                "编辑器主线程没有取件（服务已停止或窗口正在关闭）",
            )),
            data: Some(Data::Empty),
            report: ChangeReport::default(),
        },
    }
}
