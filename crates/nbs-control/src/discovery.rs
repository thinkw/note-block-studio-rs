//! 发现文件：桥接进程要连上「本机正在跑的那个编辑器」，只靠约定端口不够稳
//! （多实例、端口被占、用户同时开两个曲子的窗口）。所以服务端起来时把
//! 端口 + token + pid 落在配置目录里，客户端读它。
//!
//! token 是本地访问控制的全部依据（回环端口挡不住同机其他进程）。它落在用户自己的
//! 配置目录里，权限跟随该目录；不想开这条通路就别开服务开关。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 发现文件名，放在 `Preferences::config_dir()` 下。
pub const DISCOVERY_FILE: &str = "control.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// 线协议版本，不匹配就别连，防止拿旧字段猜新服务。
    pub protocol: u32,
    pub app: String,
    pub pid: u32,
    pub port: u16,
    /// 访问令牌，仅本机。
    pub token: String,
    pub started_unix: i64,
}

impl Discovery {
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(DISCOVERY_FILE)
    }

    /// 原子落盘：先写 `.tmp` 再 rename，避免客户端读到半截 JSON。
    pub fn write_to(&self, dir: &Path) -> IoResult<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path_in(dir);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        // Windows 上 rename 不能覆盖已存在目标，先删旧的（删失败无所谓）。
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    pub fn read_from(dir: &Path) -> Result<Self, DiscoveryError> {
        let path = Self::path_in(dir);
        let text = std::fs::read_to_string(&path).map_err(|e| DiscoveryError::Io(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| DiscoveryError::BadFormat(e.to_string()))
    }

    /// 停服时清掉，别让客户端连到已经不存在的端口上再报一个看不懂的错。
    /// 文件不是自己写的（另一个实例还活着）时不能乱删，所以带 token 比对。
    pub fn remove_matching(dir: &Path, token: &str) {
        if let Ok(found) = Self::read_from(dir) {
            if found.token == token {
                let _ = std::fs::remove_file(Self::path_in(dir));
            }
        }
    }

    /// 只在本机回环里用，不需要密码学强度；够随机、够短就行。
    pub fn random_token() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let counter = {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        let mut state = nanos ^ (counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ (pid() as u64);
        let mut out = String::with_capacity(32);
        for _ in 0..4 {
            // splitmix64：两步混合，输出打散成 4 段 8 hex。
            state = state
                .wrapping_add(0x9E37_79B9_7F4A_7C15)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            out.push_str(&format!("{z:016x}"));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    Io(String),
    BadFormat(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "读不到发现文件: {s}"),
            Self::BadFormat(s) => write!(f, "发现文件格式不对: {s}"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

type IoResult<T> = std::io::Result<T>;

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn pid() -> u32 {
    std::process::id()
}
