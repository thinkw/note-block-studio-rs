//! 连接看板：谁连着这台编辑器、什么时候最后一次使唤它。
//!
//! 服务端每条**握过手**的连接在这里占一行，界面按帧读这份快照回答「现在谁在改我的工程」。
//! 三条规矩钉死：
//! 1. token 或协议版本没过的连接不登记——否则任何扫端口的进程都会显示成「有 AI 在连」。
//! 2. 服务停掉当场清空，并认「任期」：上一趟服务晚到的收尾不能抹掉这一趟刚登记的连接。
//! 3. 这里只放给人看的小字符串，不放请求体；连接线程只写自己的那行，主线程只读快照。

use std::sync::{Arc, Mutex};

use crate::discovery::now_unix;

/// 一行 = 一条活着的连接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientSession {
    /// 本看板内的编号，只用来销账。
    pub id: u64,
    /// 客户端握手时自报的名字（桥接默认 `nbs-mcp`，可用 `--client-name` 改）。
    pub client: String,
    /// 对端地址，固定是 `127.0.0.1:<临时端口>`；写出来是为了分清同一台机器上的两个 IDE。
    pub peer: String,
    pub connected_unix: i64,
    /// `None` = 连上了还没发过请求。
    pub last_request_unix: Option<i64>,
    /// 这条连接累计发过多少条请求。
    pub requests: u64,
}

#[derive(Debug, Default)]
struct Board {
    /// 当前服务趟次；`None` = 没在服务。
    epoch: Option<u64>,
    next_epoch: u64,
    next_id: u64,
    sessions: Vec<ClientSession>,
}

/// 克隆出来就能跨线程用：服务端持有它登记连接，编辑器持有它显示。
#[derive(Clone, Debug, Default)]
pub struct ClientBoard {
    inner: Arc<Mutex<Board>>,
}

/// 一条登记的把手。丢掉它（连接收尾）就从看板上销账。
#[derive(Debug)]
pub struct Session {
    board: ClientBoard,
    epoch: u64,
    id: u64,
}

impl ClientBoard {
    /// 一趟服务开始对外：清空上一趟的残留，领一个新任期。
    pub fn open_epoch(&self) -> u64 {
        let mut b = self.inner.lock().unwrap();
        b.next_epoch += 1;
        b.epoch = Some(b.next_epoch);
        b.sessions.clear();
        b.next_epoch
    }

    /// 一趟服务结束。任期不是自己那个就说明看板已经交给下一趟了，别去动。
    pub fn close_epoch(&self, epoch: u64) {
        let mut b = self.inner.lock().unwrap();
        if b.epoch == Some(epoch) {
            b.epoch = None;
            b.sessions.clear();
        }
    }

    /// 握手成功后登记。任期过期（服务已停或已换趟）返回 `None`，调用方不必特殊处理。
    pub fn attach(&self, epoch: u64, client: &str, peer: &str) -> Option<Session> {
        let mut b = self.inner.lock().unwrap();
        if b.epoch != Some(epoch) {
            return None;
        }
        b.next_id += 1;
        let id = b.next_id;
        b.sessions.push(ClientSession {
            id,
            client: client.to_string(),
            peer: peer.to_string(),
            connected_unix: now_unix(),
            last_request_unix: None,
            requests: 0,
        });
        Some(Session {
            board: self.clone(),
            epoch,
            id,
        })
    }

    /// 给人看的那份快照。没在服务时是空的，界面不用自己再判一次开关。
    pub fn snapshot(&self) -> Vec<ClientSession> {
        let b = self.inner.lock().unwrap();
        if b.epoch.is_none() {
            return Vec::new();
        }
        b.sessions.clone()
    }

    pub fn count(&self) -> usize {
        self.snapshot().len()
    }

    fn requested(&self, epoch: u64, id: u64) {
        let mut b = self.inner.lock().unwrap();
        if b.epoch != Some(epoch) {
            return;
        }
        if let Some(s) = b.sessions.iter_mut().find(|s| s.id == id) {
            s.requests += 1;
            s.last_request_unix = Some(now_unix());
        }
    }

    fn detach(&self, id: u64) {
        let mut b = self.inner.lock().unwrap();
        b.sessions.retain(|s| s.id != id);
    }
}

impl Session {
    /// 收到这条连接的一条请求。
    pub fn requested(&self) {
        self.board.requested(self.epoch, self.id);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.board.detach(self.id);
    }
}
