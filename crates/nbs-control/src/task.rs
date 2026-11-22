//! 重活任务表。导出 wav / 数据包 / 声源分配方案这类活动动辄几秒到几十秒，
//! 让主线程当场做完就等于冻住窗口，所以口径是：**立即回句柄，之后轮询**。
//!
//! 任务体跑在自己的线程里，只能通过 [`JobHandle`] 回写进度，不许碰 `EditorState`。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::outcome::{JobInfo, JobStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnReject {
    /// 并发任务已满。
    Busy,
    /// 线程起不来（系统资源）。
    Thread(String),
}

/// 任务表里 `kind` 的那几个写法。收成类型是因为闸要按它数「同一类活是不是已经在跑」——
/// 拼错一个字母的字符串会让那道闸形同虚设，而且没人看得见。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Export,
    TranscribeProbe,
    TranscribePrepareEnv,
    TranscribePrepareModel,
    TranscribeAudio,
}

impl JobKind {
    /// 闸要遍历的就是这一张表：新加一类重活得同时加进来，否则「一次只跑一个」看不见它。
    pub const ALL: &'static [Self] = &[
        Self::Export,
        Self::TranscribeProbe,
        Self::TranscribePrepareEnv,
        Self::TranscribePrepareModel,
        Self::TranscribeAudio,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::TranscribeProbe => "transcribe_probe",
            Self::TranscribePrepareEnv => "transcribe_prepare_env",
            Self::TranscribePrepareModel => "transcribe_prepare_model",
            Self::TranscribeAudio => "transcribe_audio",
        }
    }

    /// 转录那四条共用「一次只许跑一个」：它们都起本机 python 子进程，并发跑等于
    /// 抢同一份 venv、同一份权重缓存，出错的形状还很难看。
    pub fn is_transcription(&self) -> bool {
        !matches!(self, Self::Export)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    NotFound,
    /// 已经结束，取消无意义。
    Finished,
    /// 置位成功；还在跑的任务体要自己检查取消位才能真的停下来。
    Signalled,
}

struct Entry {
    info: JobInfo,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct JobRegistry {
    inner: Arc<Mutex<VecDeque<Entry>>>,
    next: Arc<AtomicU64>,
    limits: Arc<Limits>,
}

struct Limits {
    max_running: usize,
    keep_finished: usize,
}

impl JobRegistry {
    /// `max_running` 同时跑几个；`keep_finished` 完成后还留几条历史供轮询。
    pub fn new(max_running: usize, keep_finished: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
            next: Arc::new(AtomicU64::new(1)),
            limits: Arc::new(Limits {
                max_running: max_running.max(1),
                keep_finished: keep_finished.max(1),
            }),
        }
    }

    /// 还在跑（含刚登记还没起线程）的任务数。
    pub fn running(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e.info.status, JobStatus::Running | JobStatus::Queued))
            .count()
    }

    /// 某一类活现在有几个在册（刚登记还没起线程也算）。「一次只许跑一个」那道闸读这条。
    pub fn running_of(&self, kind: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter(|e| {
                e.info.kind == kind
                    && matches!(e.info.status, JobStatus::Running | JobStatus::Queued)
            })
            .count()
    }

    /// 起一个任务。闭包拿到 [`JobHandle`]，`Ok(Value)` 进 `result`，
    /// `Err(String)` 进 `message` 并标失败；被取消过则状态一律记 `cancelled`。
    pub fn spawn<F>(&self, kind: &str, work: F) -> Result<u64, SpawnReject>
    where
        F: FnOnce(&JobHandle) -> Result<Value, String> + Send + 'static,
    {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut jobs = self.inner.lock().unwrap();
            let running = jobs
                .iter()
                .filter(|e| matches!(e.info.status, JobStatus::Running | JobStatus::Queued))
                .count();
            if running >= self.limits.max_running {
                return Err(SpawnReject::Busy);
            }
            jobs.push_back(Entry {
                info: JobInfo {
                    id,
                    kind: kind.to_string(),
                    progress: 0.0,
                    status: JobStatus::Queued,
                    message: String::new(),
                    started_unix: now_unix(),
                    finished_unix: None,
                    result: None,
                },
                cancel: Arc::clone(&cancel),
            });
            self.prune_locked(&mut jobs);
        }

        let registry = self.clone();
        let thread_cancel = Arc::clone(&cancel);
        let thread_kind = kind.to_string();
        std::thread::Builder::new()
            .name(format!("nbs-job-{thread_kind}"))
            .spawn(move || {
                // 兜底：任务体 panic 时不能把条目永远留在 Running。
                let mut guard = PanicGuard {
                    registry: registry.clone(),
                    id,
                    settled: false,
                };
                registry.set(id, |info| info.status = JobStatus::Running);
                let handle = JobHandle {
                    registry: registry.clone(),
                    id,
                    cancel: thread_cancel,
                };
                match work(&handle) {
                    Ok(value) => {
                        let cancelled = handle.cancelled();
                        registry.set(id, |info| {
                            info.progress = 1.0;
                            info.result = Some(value);
                            info.status = if cancelled {
                                JobStatus::Cancelled
                            } else {
                                JobStatus::Done
                            };
                        });
                    }
                    Err(message) => {
                        let cancelled = handle.cancelled();
                        registry.set(id, |info| {
                            info.status = if cancelled {
                                JobStatus::Cancelled
                            } else {
                                JobStatus::Failed
                            };
                            info.message = message;
                        });
                    }
                }
                guard.settled = true;
            })
            .map_err(|e| {
                let text = e.to_string();
                self.set(id, |info| {
                    info.status = JobStatus::Failed;
                    info.message = format!("任务线程起不来: {text}");
                });
                SpawnReject::Thread(e.to_string())
            })?;
        Ok(id)
    }

    pub fn status(&self, id: u64) -> Option<JobInfo> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.info.id == id)
            .map(|e| e.info.clone())
    }

    /// 新条目在前，方便轮询方一眼看到最新的。
    pub fn list(&self) -> Vec<JobInfo> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .rev()
            .map(|e| e.info.clone())
            .collect()
    }

    pub fn cancel(&self, id: u64) -> CancelOutcome {
        let mut jobs = self.inner.lock().unwrap();
        let Some(entry) = jobs.iter_mut().find(|e| e.info.id == id) else {
            return CancelOutcome::NotFound;
        };
        if matches!(
            entry.info.status,
            JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
        ) {
            return CancelOutcome::Finished;
        }
        entry.cancel.store(true, Ordering::SeqCst);
        // 还没起线程的任务当场定性；已经在跑的由任务体在检查点自行收尾。
        if entry.info.status == JobStatus::Queued {
            entry.info.status = JobStatus::Cancelled;
            entry.info.message = "起线程前已取消".to_string();
            entry.info.finished_unix = Some(now_unix());
        }
        CancelOutcome::Signalled
    }

    fn set<F>(&self, id: u64, f: F)
    where
        F: FnOnce(&mut JobInfo),
    {
        let mut jobs = self.inner.lock().unwrap();
        let Some(entry) = jobs.iter_mut().find(|e| e.info.id == id) else {
            return;
        };
        f(&mut entry.info);
        let finished = is_finished(entry.info.status);
        if finished {
            entry.info.finished_unix = Some(now_unix());
        }
        // 收工即剪枝：历史条数在任何时刻都不超过 keep_finished。
        if finished {
            self.prune_locked(&mut jobs);
        }
    }

    /// 完成的条目只留最近 `keep_finished` 条，跑着的一律保留。
    fn prune_locked(&self, jobs: &mut VecDeque<Entry>) {
        let mut finished = jobs.iter().filter(|e| is_finished(e.info.status)).count();
        while finished > self.limits.keep_finished {
            match jobs.front() {
                Some(front) if is_finished(front.info.status) => {
                    jobs.pop_front();
                    finished -= 1;
                }
                _ => break,
            }
        }
    }
}

fn is_finished(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
    )
}

/// 交给任务体的把手：报进度、看有没有被取消。
#[derive(Clone)]
pub struct JobHandle {
    registry: JobRegistry,
    id: u64,
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 协作式取消：任务体要在自己的检查点调用它，没人调用就照样跑完。
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// `progress` 夹在 0..=1；估不出来的活就别调。
    pub fn progress(&self, progress: f32, message: impl Into<String>) {
        let message = message.into();
        self.registry.set(self.id, |info| {
            info.progress = progress.clamp(0.0, 1.0);
            info.message = message;
        });
    }

    pub fn note(&self, message: impl Into<String>) {
        let message = message.into();
        self.registry.set(self.id, |info| {
            info.message = message;
        });
    }
}

struct PanicGuard {
    registry: JobRegistry,
    id: u64,
    settled: bool,
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.registry.set(self.id, |info| {
                if matches!(info.status, JobStatus::Running | JobStatus::Queued) {
                    info.status = JobStatus::Failed;
                    info.message = "任务线程异常退出".to_string();
                }
            });
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
