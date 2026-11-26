//! 外部控制的执行层：把 `nbs-control` 的一条 `Op` 落到正在运行的编辑器上。
//!
//! 三条规矩，后面每个分支都按它们走：
//!
//! 1. **只在 UI 线程执行**。`EditorState` 不是 `Send`，所以别的线程只往 `Inbox` 投递；
//!    每帧由 [`drain`] 取走、就地执行、把回执送回去。
//! 2. **能借现成 `Command` 的一律借**。界面按钮和外部调用必须走同一条代码路径，
//!    不然「AI 改出来的」和「人点出来的」会出现第二种语义。只有「一次调用 = 一次撤销」
//!    的批量写才直接开事务——拆成一串命令会变成一串历史。
//! 3. **拒绝也要回报**。`rev`、脏标记、历史位置照旧给出去，调用方不用先猜工程现在的样子。
//!
//! 弹窗纪律：`EditorState::flush` 在模态队列没答完之前会**丢弃**除 `AnswerModal` 以外的
//! 所有命令。所以这里凡是要走命令队列的操作，先检查 `state.modals`，挡下来并说明怎么解，
//! 绝不返回一个「其实什么都没发生」的成功。

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::Context;
use serde_json::{json, Value};

use nbs_control::task::{CancelOutcome, JobKind, SpawnReject};
use nbs_control::{
    ActionInfo, BarOverview, BatchField, CellNote, ChangeReport, ClientBoard, ControlServer, Data,
    Diagnostics, Discovery, ErrorCode, ErrorInfo, HistoryInfo, ImportChannelMap, ImportDrumMap,
    ImportEnhancedChannel, ImportEnhancedOptions, ImportMidiOptions, ImportRangeMode, ImportSlot,
    ImportSubstitution, Inbox, InstrumentInfo, JobHandle, JobInfo, JobRegistry, JobStatus,
    LayerInfo, MidiChannelPlan, MidiDrumPlan, MidiEnhancedReport, MidiPartPlan, MidiPlan,
    MidiTrackInfo, NoteCell, Op, Outcome, Playback, Scope, SelectTarget, SelectionInfo,
    ServerConfig, SetMeta, Snapshot, TabInfo, TabRef, TranscribeOptions, PROTOCOL_VERSION,
};
use nbs_core::model::{
    Note, Song, FIRST_CUSTOM_INDEX, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN, PAN_MAX, PIT_MAX, VEL_MAX,
};

use crate::commands::Command;
use crate::state::{EditorState, Flag};

/// 一条请求最多写多少格。AI 一次生成上万音符是正常活，但让主线程一帧卡死不值得。
const MAX_CELLS_PER_CALL: usize = 50_000;
/// `GetNotes` 一次最多回多少格，截断会显式标出来。
const MAX_CELLS_RETURNED: usize = 20_000;
/// 小节文字视图一次最多切几节：再多就不是「读一段」，而是让主线程拼一份没人读的长文本。
const MAX_BARS_RETURNED: usize = 256;
/// 一小节的文字最多列几格，超了以 `…` 收尾（`cells` 仍是真实条数）。
const MAX_BAR_CELLS_PRINTED: usize = 64;
/// 一次 `Undo` / `Redo` 最多滚多远。
const MAX_HISTORY_STEPS: u32 = 64;

/// 界面语言：1 = 中文。执行层自己产的文字按它出词。
fn zh() -> bool {
    crate::lang() == 1
}

/// 双语一行：`say("写好的中文", "english")`。
fn say(cn: impl std::fmt::Display, en: impl std::fmt::Display) -> String {
    if zh() {
        cn.to_string()
    } else {
        en.to_string()
    }
}

/// 外部调用能碰到哪些文件、能不能碰危险动作。
///
/// 已知边界：路径只做词法归一（消掉 `.` 与 `..`），不解析符号链接，所以白名单目录里
/// 如果放了指向别处的链接，这条路是拦不住的。要真的放开就用 `allow_any_path`，
/// 别靠改目录列表假装安全。
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// 允许读写的目录（工程所在目录、最近文件目录、配置目录会预先种进来）。
    pub allowed_dirs: Vec<PathBuf>,
    /// true = 不查目录，任意路径都放行。
    pub allow_any_path: bool,
    /// 危险动作（退出、开外链、清空最近列表、回答人的确认框）是否可调。
    pub allow_danger: bool,
}

/// 一条控制链路的共享部分。克隆它只是为了在 `execute` 之前把它从 `state` 里拿出来，
/// 免得 `&mut EditorState` 和它自己的字段打架。
#[derive(Clone)]
pub struct ControlChannel {
    pub inbox: Inbox,
    pub jobs: JobRegistry,
    pub policy: Arc<Mutex<Policy>>,
    pub parked_write: Arc<std::sync::atomic::AtomicBool>,
    /// 谁连着：服务端往这里登记，面板按帧读。没起服务时它一直是空的。
    pub clients: ClientBoard,
    pub app_version: String,
    pub protocol: u32,
}

impl std::fmt::Debug for ControlChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlChannel")
            .field("queued", &self.inbox.pending())
            .field("running_jobs", &self.jobs.running())
            .field("clients", &self.clients.count())
            .field("protocol", &self.protocol)
            .finish()
    }
}

/// [`start`] 的成功产物：服务端本体交给宿主管（退出时要停），端口和 token 拿去写状态栏日志。
pub struct ControlStart {
    pub server: ControlServer,
    pub port: u16,
    pub token: String,
    /// 一行给人看的话（同时进状态栏和 `state.log_strs`）。
    pub log: String,
    /// 发现文件落在哪，`discovery = false` 时是 `None`。界面要原样显示这一行，
    /// 所以路径在这里给出去，而不是让调用方再拼一次文件名（那条规则一改，
    /// 拼出来的路径就成了「看着对但连不上」）。
    pub discovery_path: Option<String>,
}

/// 装好链路但不启服务：无 GUI 的测试、或者「先接上、等用户打开开关」的场合。
pub fn attach<'a>(state: &'a mut EditorState, ctx: &Context) -> &'a ControlChannel {
    let inbox = {
        let ctx = ctx.clone();
        Inbox::with_waker(move || ctx.request_repaint())
    };
    state.control = Some(ControlChannel {
        inbox: inbox.clone(),
        jobs: JobRegistry::new(2, 16),
        policy: Arc::new(Mutex::new(Policy {
            allowed_dirs: seed_dirs(state),
            allow_any_path: false,
            allow_danger: false,
        })),
        parked_write: Arc::new(std::sync::atomic::AtomicBool::new(
            state.prefs.control_allow_parked_write,
        )),
        clients: ClientBoard::default(),
        app_version: crate::VERSION.to_string(),
        protocol: PROTOCOL_VERSION,
    });
    state.control.as_ref().expect("刚赋的值不可能为空")
}

/// 起回环服务并把链路挂到 `state` 上。`port = 0` 让系统挑端口。
/// 服务端的 `ControlServer` 由**调用方**持有：它要在程序退出时 `stop()`，好把发现文件删掉。
pub fn start(
    state: &mut EditorState,
    ctx: &Context,
    port: u16,
    discovery: bool,
    allow_danger: bool,
) -> Result<ControlStart, String> {
    let parked_write = state.prefs.control_allow_parked_write;
    let channel = attach(state, ctx);
    if let Ok(mut policy) = channel.policy.lock() {
        policy.allow_danger = allow_danger;
    }
    let inbox = channel.inbox.clone();
    let token = Discovery::random_token();
    let discovery_dir = discovery.then(nbs_io::prefs::Preferences::config_dir);
    // 路径在这里就定下来：`cfg` 要整个交给服务端（它按值拿走目录），而界面还要读这一行。
    let discovery_path = discovery_dir
        .as_ref()
        .map(|dir| Discovery::path_in(dir).to_string_lossy().into_owned());
    let mut cfg = ServerConfig::new(token.clone(), inbox);
    cfg.app_version = crate::VERSION.to_string();
    cfg.discovery_dir = discovery_dir;
    channel
        .parked_write
        .store(parked_write, std::sync::atomic::Ordering::Relaxed);
    cfg.parked_write = channel.parked_write.clone();
    // 看板必须是链路上这一份：服务端往里记账，面板从 `state.control.clients` 读，
    // 新造一个的话面板永远是「没人连着」。
    cfg.clients = channel.clients.clone();
    let server = ControlServer::start(port, cfg).map_err(|e| format!("回环服务起不来: {e}"))?;
    let actual = server.port();
    let log = say(
        format!(
            "外部控制已监听 127.0.0.1:{actual}（发现文件：{}）",
            if discovery { "已写" } else { "未写" }
        ),
        format!(
            "Control listening on 127.0.0.1:{actual} (discovery file: {})",
            if discovery { "written" } else { "skipped" }
        ),
    );
    state.set_message(log.clone());
    Ok(ControlStart {
        server,
        port: actual,
        token,
        log,
        discovery_path,
    })
}

/// 外部控制这一页要显示、要改的几个东西，都放在这里，界面上不另存一份真相。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlInfo {
    /// 现在听着的端口（0 都不该出现：真起不来时整个 `Option` 是 `None`）。
    pub port: u16,
    /// 发现文件的绝对路径；`None` = 这一趟没写发现文件（`start` 的 discovery 传了 false），
    /// 那时桥接只能拿 `--port/--token` 直连。面板要说的是这个区别，不是空路径。
    pub discovery_path: Option<String>,
    /// 这一串话说明是谁把服务开起来的（命令行旗标 / 环境变量 / 设置页），
    /// 由宿主按当前语言定好型，界面原样显示。
    pub source: String,
    /// 危险动作现在放不放行。显示的是**生效值**（`--control-danger` 或设置页勾了），
    /// 不是 `prefs.control_danger` 单独那一条——两者不一致时后者会骗人。
    pub allow_danger: bool,
    /// 启动旗标 `--control-dir` 给的那些目录。这一趟一直在生效、面板删不掉，
    /// 所以要在面板上说出来，否则用户会以为「移除」清空了全部白名单。
    pub cli_dirs: Vec<String>,
}

/// 设置页这一帧对服务生命周期提的要求。宿主（`nbs-app`）在画完一帧后读走并置回 `None`：
/// 起停要动 socket 和发现文件，这些不归编辑器的画布管——和文件选择器同一个分工。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlChange {
    /// 按 prefs 现在的口径把服务起起来：端口交系统挑，发现文件照写。
    Enable,
    /// 停服务、删发现文件。链路（`state.control`）留着，再开不用重接。
    Disable,
    /// 服务照旧开着，只把目录白名单和 danger 按 prefs 重算一遍。
    Reapply,
}

/// 按「启动旗标 + 设置页」重算白名单和 danger，整表替换而不是只追加。
///
/// 用户在设置页删掉一条目录，期望是这一帧之后就写不进去，而不是等下次启动；
/// 只追加的那套做不到这件事。种子目录（配置目录 / 当前工程目录 / 最近文件目录）
/// 每次都重新算，所以中途换工程该放的照样放进来。
///
/// `danger` 传的是**已经判好的生效值**，不在这里做优先级：启动那趟是
/// 「旗标 或 设置页」，面板碰过之后一律以设置页为准（人亲手勾的那一下要能撤销
/// 命令行给的那一次同意，反过来不行）。链路没开（`state.control` 是 `None`）时是空操作。
pub fn sync_policy(state: &EditorState, cli_dirs: &[PathBuf], danger: bool) {
    let Some(channel) = state.control.as_ref() else {
        return;
    };
    let prefs_dirs: Vec<PathBuf> = state.prefs.control_dirs.iter().map(PathBuf::from).collect();
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in seed_dirs(state)
        .iter()
        .chain(cli_dirs)
        .chain(prefs_dirs.iter())
    {
        let dir = normalize(dir);
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    if let Ok(mut policy) = channel.policy.lock() {
        policy.allowed_dirs = out;
        policy.allow_danger = danger;
    }
    channel.parked_write.store(
        state.prefs.control_allow_parked_write,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// 白名单目录的种子：配置目录 + 当前工程目录 + 最近文件目录（最多 12 个）。
/// 之后再开别的工程，那条判定会现场把它的目录算进来，不需要往列表里堆。
fn seed_dirs(state: &EditorState) -> Vec<PathBuf> {
    let mut out = vec![nbs_io::prefs::Preferences::config_dir()];
    if let Some(path) = state.doc.song.filename.as_deref() {
        if let Some(dir) = path.parent() {
            out.push(dir.to_path_buf());
        }
    }
    for recent in state.recent_files.iter().take(12) {
        if let Some(dir) = recent.path.parent() {
            out.push(dir.to_path_buf());
        }
    }
    out
}

/// 每帧调用：把这一帧之前排到的请求全部处理掉。返回处理条数，方便宿主决定是否催帧。
pub fn drain(state: &mut EditorState, ctx: &Context) -> usize {
    let Some(channel) = state.control.clone() else {
        return 0;
    };
    let pending = channel.inbox.drain();
    if pending.is_empty() {
        return 0;
    }
    let n = pending.len();
    for req in pending {
        let outcome = execute(state, ctx, &channel, req.tab.as_ref(), &req.op);
        req.reply(outcome);
    }
    n
}

// ------------------------------------------------------------------ 分发

fn execute(
    state: &mut EditorState,
    ctx: &Context,
    ch: &ControlChannel,
    tab: Option<&TabRef>,
    op: &Op,
) -> Outcome {
    let target = match tab {
        Some(tab) => match resolve_tab(state, tab) {
            Ok(id) => Some(id),
            Err(error) => return fail(state, error),
        },
        None => state.store.active(),
    };
    if matches!(
        op,
        Op::ListTabs | Op::SwitchTab { .. } | Op::CloseTab { .. }
    ) {
        return execute_active(state, ctx, ch, op, true, state.store.active());
    }
    let Some(target) = target else {
        return execute_active(state, ctx, ch, op, true, state.store.active());
    };
    if state.store.active() == Some(target) {
        return execute_active(state, ctx, ch, op, true, state.store.active());
    }
    if is_tab_read(op) {
        let previous = state.store.active();
        return with_swapped_doc(state, target, |state| {
            execute_active(state, ctx, ch, op, false, previous)
        });
    }
    if op.is_document_write() {
        if !state.prefs.control_allow_parked_write {
            return refuse(
                state,
                ErrorCode::ParkedTabReadonly,
                say(
                    "后台标签默认只读；要在设置里开启后台写才能修改它",
                    "Parked tabs are read-only by default; enable parked writes in settings first",
                ),
            );
        }
        if op.expect_rev().is_none() {
            return refuse(
                state,
                ErrorCode::ExpectRequired,
                say(
                    "后台写必须带 expect_rev；先读该标签的 snapshot/notes 再重试",
                    "Parked writes require expect_rev; read that tab before retrying",
                ),
            );
        }
        return with_swapped_doc(state, target, |state| {
            mutate_inner(state, ctx, ch, op, false)
        });
    }
    refuse(
        state,
        ErrorCode::RequiresActiveTab,
        say(
            "这个操作涉及播放、文件或全局交互，只能打在当前标签上",
            "This operation touches playback, files, or global UI and requires the active tab",
        ),
    )
}

fn is_tab_read(op: &Op) -> bool {
    matches!(
        op,
        Op::GetSnapshot
            | Op::GetNotes { .. }
            | Op::GetLayers
            | Op::GetSelection
            | Op::GetInstruments
            | Op::GetHistory
    )
}

fn resolve_tab(state: &EditorState, tab: &TabRef) -> Result<crate::tabs::SongId, ErrorInfo> {
    let tab = tab
        .validate()
        .map_err(|message| ErrorInfo::new(ErrorCode::BadArgument, say(message, message)))?;
    let id = if let Some(session) = tab.id {
        state.store.id_for_session(session).ok_or_else(|| {
            ErrorInfo::new(
                ErrorCode::TabGone,
                say(
                    format!("标签 {session} 已经关闭，引用失效"),
                    format!("Tab {session} is gone"),
                ),
            )
            .with_detail(json!({ "tab_id": session }))
        })?
    } else {
        let index = tab.index.expect("validate 已保证 index 存在");
        state
            .store
            .order()
            .get(index as usize)
            .copied()
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorCode::NotFound,
                    say(
                        format!("标签位置 {index} 不存在"),
                        format!("Tab index {index} does not exist"),
                    ),
                )
            })?
    };
    Ok(id)
}

fn with_swapped_doc(
    state: &mut EditorState,
    id: crate::tabs::SongId,
    f: impl FnOnce(&mut EditorState) -> Outcome,
) -> Outcome {
    let previous = state.store.active().expect("有后台标签时台前一定存在");
    let message = state.message.take();
    if !state.store.swap_active(id, &mut state.doc) {
        state.message = message;
        return fail(
            state,
            ErrorInfo::new(
                ErrorCode::TabGone,
                say("目标标签已经关闭", "The target tab is gone"),
            ),
        );
    }
    let outcome = f(state);
    let restored = state.store.swap_active(previous, &mut state.doc);
    debug_assert!(restored, "后台操作后原标签一定还在");
    state.message = message;
    outcome
}

fn tab_infos(
    state: &EditorState,
    active_id: Option<crate::tabs::SongId>,
    swapped: bool,
) -> Vec<TabInfo> {
    state
        .store
        .order()
        .iter()
        .enumerate()
        .filter_map(|(index, id)| {
            let song = if swapped && active_id == Some(*id) {
                &state.store.parked_doc(*id)?.song
            } else if state.store.active() == Some(*id) {
                &state.doc.song
            } else {
                &state.store.parked_doc(*id)?.song
            };
            let filename = song.filename.as_ref().map(|p| p.display().to_string());
            let title = song
                .filename
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| say("新文件", "Unsaved song"));
            Some(TabInfo {
                id: state.store.session(*id)?,
                index: index as u32,
                title,
                filename,
                dirty: song.dirty,
                rev: song.rev,
                total_blocks: song.total_blocks,
                song_length: song.song_length(),
                layer_count: song.layers.len(),
            })
        })
        .collect()
}
fn execute_active(
    state: &mut EditorState,
    ctx: &Context,
    ch: &ControlChannel,
    op: &Op,
    active: bool,
    original_active: Option<crate::tabs::SongId>,
) -> Outcome {
    match op {
        Op::ListTabs => ok_data(
            state,
            Data::Tabs {
                tabs: tab_infos(state, state.store.active(), false),
                active_tab: state.store.active_session().into(),
            },
            String::new(),
        ),
        Op::SwitchTab { tab } => {
            let id = match resolve_tab(state, tab) {
                Ok(id) => id,
                Err(error) => return fail(state, error),
            };
            if state.store.active() != Some(id) {
                run_command(state, ctx, Command::SwitchTab(id));
            }
            ok_data(
                state,
                Data::Tabs {
                    tabs: tab_infos(state, state.store.active(), false),
                    active_tab: state.store.active_session().into(),
                },
                String::new(),
            )
        }
        Op::CloseTab { tab, discard } => {
            let id = match resolve_tab(state, tab) {
                Ok(id) => id,
                Err(error) => return fail(state, error),
            };
            let dirty = if state.store.active() == Some(id) {
                state.doc.song.dirty
            } else {
                state
                    .store
                    .parked_doc(id)
                    .map(|doc| doc.song.dirty)
                    .unwrap_or(false)
            };
            if dirty && !state.player && *discard != Some(true) {
                return refuse(
                    state,
                    ErrorCode::NeedsConfirm,
                    say(
                        "这个标签有未保存改动；明确丢弃请带 discard=true",
                        "This tab has unsaved changes; pass discard=true to discard them explicitly",
                    ),
                );
            }
            let command = if *discard == Some(true) {
                Command::CloseTabDiscard(id)
            } else {
                Command::CloseTab(id)
            };
            run_command(state, ctx, command);
            ok_data(
                state,
                Data::Tabs {
                    tabs: tab_infos(state, state.store.active(), false),
                    active_tab: state.store.active_session().into(),
                },
                String::new(),
            )
        }
        Op::Ping => Outcome::ok(
            Data::Pong {
                app_version: ch.app_version.clone(),
                protocol: ch.protocol,
                rev: state.doc.song.rev,
                song_session: state.store.active_session(),
            },
            current_report(state, String::new()),
        ),
        Op::ListActions => ok_data(
            state,
            Data::Actions {
                actions: list_actions(),
            },
            say("动作清单已返回", "Action list returned"),
        ),
        Op::GetSnapshot => ok_data(
            state,
            Data::Snapshot(Box::new(snapshot(state, ch, active, original_active))),
            String::new(),
        ),
        Op::GetNotes {
            tick_from,
            tick_to,
            layer_from,
            layer_to,
            ticks_per_bar,
            bars,
        } => match read_notes(
            state,
            *tick_from,
            *tick_to,
            *layer_from,
            *layer_to,
            *ticks_per_bar,
            *bars,
        ) {
            Ok((data, message)) => ok_data(state, data, message),
            Err(error) => Outcome::err(error, current_report(state, String::new())),
        },
        Op::GetLayers => ok_data(
            state,
            Data::Layers {
                layers: layer_infos(state),
            },
            String::new(),
        ),
        Op::GetSelection => ok_data(
            state,
            Data::Selection(Box::new(selection_info(&state.doc.selection))),
            String::new(),
        ),
        Op::GetInstruments => ok_data(
            state,
            Data::Instruments {
                instruments: instrument_infos(state, active),
                mc_key_range: [MC_KEY_MIN, MC_KEY_MAX],
            },
            String::new(),
        ),
        Op::GetDiagnostics => ok_data(
            state,
            Data::Diagnostics(Box::new(diagnostics(state))),
            String::new(),
        ),
        Op::GetHistory => ok_data(
            state,
            Data::History(Box::new(history_info(state))),
            String::new(),
        ),
        // 任务表只动自己的记录，不碰工程，弹窗挡着也照样能查、能取消。
        Op::JobStatus { id } => match ch.jobs.status(*id) {
            Some(info) => ok_data(state, Data::Job(Box::new(info)), String::new()),
            None => refuse(
                state,
                ErrorCode::NotFound,
                say(format!("没有编号 {id} 的任务"), format!("No job {id}")),
            ),
        },
        Op::JobList => ok_data(
            state,
            Data::Jobs {
                jobs: ch.jobs.list(),
            },
            String::new(),
        ),
        Op::JobCancel { id } => match ch.jobs.cancel(*id) {
            CancelOutcome::Signalled => ok_data(
                state,
                Data::Empty,
                say(
                    format!("已要求取消任务 {id}（它会在下一个检查点停下）"),
                    format!("Cancel requested for job {id} (stops at next checkpoint)"),
                ),
            ),
            CancelOutcome::Finished => refuse(
                state,
                ErrorCode::NotFound,
                say(
                    format!("任务 {id} 已经结束了，没有可取消的东西"),
                    format!("Job {id} already finished, nothing to cancel"),
                ),
            ),
            CancelOutcome::NotFound => refuse(
                state,
                ErrorCode::NotFound,
                say(format!("没有编号 {id} 的任务"), format!("No job {id}")),
            ),
        },
        // 预检会读盘并把整首歌算一遍，但工程一点不动；主线程这一下可能几百毫秒，
        // 换的是「写之前先看清会得到什么」，值。
        Op::InspectMidi {
            path,
            options,
            enhanced,
        } => match midi_inspect(state, ch, path, options.as_ref(), enhanced.as_ref()) {
            Ok((data, message)) => ok_data(state, data, message),
            Err(error) => Outcome::err(error, current_report(state, String::new())),
        },
        other => mutate(state, ctx, ch, other),
    }
}

// ------------------------------------------------------------------ 写路径

fn mutate(state: &mut EditorState, ctx: &Context, ch: &ControlChannel, op: &Op) -> Outcome {
    mutate_inner(state, ctx, ch, op, true)
}

fn mutate_inner(
    state: &mut EditorState,
    ctx: &Context,
    ch: &ControlChannel,
    op: &Op,
    active: bool,
) -> Outcome {
    // 起后台任务的那几条只是拍快照再交给别的线程，不改状态，所以不受弹窗与窗口草稿闸约束。
    if !op.only_spawns_a_job() {
        if let Err(e) = gate(state, op) {
            return fail(state, e);
        }
    }
    let stamp = Stamp::capture(state);
    match write(state, ctx, ch, op, active) {
        Ok((data, message)) => {
            if !message.is_empty() {
                state.set_message(message.clone());
            }
            let report = stamp.report(state, message);
            match data {
                Some(data) => Outcome::ok(data, report),
                None => Outcome::done(report),
            }
        }
        Err(e) => fail(state, e),
    }
}

/// 写操作前的两道闸：弹窗、窗口草稿。
fn gate(state: &EditorState, op: &Op) -> Result<(), ErrorInfo> {
    // answer_modal 是唯一一条「必须趁着弹窗开着才成立」的动作，两道闸都不能挡它——
    // 挡在这里就等于把解开弹窗的唯一入口锁死在弹窗后面。
    if matches!(op, Op::RunAction { action, .. } if action == "answer_modal") {
        return Ok(());
    }
    if !state.modals.is_empty() {
        let titles = state
            .modals
            .items
            .iter()
            .map(|m| m.title)
            .collect::<Vec<_>>()
            .join(" / ");
        return Err(ErrorInfo::new(
            ErrorCode::ModalOpen,
            say(
                format!(
                    "界面上还挂着没答完的确认框（{titles}）。外部写操作会被界面丢掉，先读 get_snapshot，再用 run_action 的 answer_modal 回答，或者让人处理。"
                ),
                format!(
                    "Unanswered dialog(s) on screen ({titles}); writes would be dropped by the UI. Read get_snapshot, then run_action answer_modal, or let a human handle it."
                ),
            ),
        ));
    }
    // 这些窗口攒着「还没提交」的草稿数据（待落盘的 MIDI、导出的目标路径与选项），
    // 外部调用在这中间改歌曲或换工程，人回到窗口里看到的就是对不上号的东西。
    let draft_window = match state.window.as_ref() {
        Some(kind) => {
            let name = format!("{kind:?}");
            matches!(
                kind,
                crate::state::WindowKind::MidiImport
                    | crate::state::WindowKind::FileSave
                    | crate::state::WindowKind::FileOpen
                    | crate::state::WindowKind::AudioExport
                    | crate::state::WindowKind::DatapackExport
                    | crate::state::WindowKind::SchematicExport
                    | crate::state::WindowKind::TrackExport
                    | crate::state::WindowKind::PngExport
                    | crate::state::WindowKind::SoundImport
            )
            .then_some(name)
        }
        None => None,
    };
    if let Some(name) = draft_window {
        return Err(ErrorInfo::new(
            ErrorCode::WindowOpen,
            say(
                format!("窗口 {name} 开着且它手里有未提交的东西。要外部操作先 RunAction close_window。"),
                format!("Window {name} is open holding uncommitted input. RunAction close_window first."),
            ),
        ));
    }
    Ok(())
}

fn policy_allow(ch: &ControlChannel, f: impl FnOnce(&Policy) -> bool) -> bool {
    ch.policy.lock().map(|p| f(&p)).unwrap_or(false)
}

/// 一次调用可能落多条历史（例如先改保存版本再落盘），所以变更量按**新增的那几条**汇总，
/// 而不是只看最后一条。
struct Stamp {
    history_len: usize,
    /// 撤销游标：undo/redo 用它算出这次滚过了哪几条。
    cursor: usize,
    /// 开档 / 导入 MIDI 这类整首替换会把撤销历史一起清空，历史那条路于是算不出任何变更量，
    /// 回执看着像「什么都没改」。会话号一变就是换了歌，变更量改按新工程的规模报。
    song_session: u64,
}

impl Stamp {
    fn capture(state: &EditorState) -> Self {
        Self {
            history_len: state.doc.history.len(),
            cursor: state.doc.history.cursor(),
            song_session: state.store.active_session(),
        }
    }

    fn report(self, state: &EditorState, message: String) -> ChangeReport {
        let after = state.doc.history.len();
        let added = after.saturating_sub(self.history_len);
        // undo/redo 不加条目，只移游标：那就看游标跨过的那几条。
        let (from, to) = if added > 0 {
            (after - added, after)
        } else if state.doc.history.cursor() < self.cursor {
            (state.doc.history.cursor(), self.cursor)
        } else {
            (self.cursor, state.doc.history.cursor())
        };
        let (cells, layers, meta) = if state.store.active_session() != self.song_session {
            (state.doc.song.total_blocks, state.doc.song.layers.len(), true)
        } else {
            edit_totals(state, from, to)
        };
        ChangeReport {
            cells_changed: cells,
            layers_changed: layers,
            meta_changed: meta,
            rev: state.doc.song.rev,
            song_session: state.store.active_session(),
            dirty: state.doc.song.dirty,
            history_len: after,
            history_cursor: state.doc.history.cursor(),
            message,
        }
    }
}

/// 历史条目 `[from, to)` 里的变更量合计。
fn edit_totals(state: &EditorState, from: usize, to: usize) -> (usize, usize, bool) {
    let mut cells = 0usize;
    let mut layers = 0usize;
    let mut meta = false;
    for (_, edit) in state
        .doc
        .history
        .entries()
        .skip(from)
        .take(to.saturating_sub(from))
    {
        cells += edit.cells.iter().filter(|d| !d.is_noop()).count();
        if let Some(l) = &edit.layers {
            layers += layer_diff(&l.before, &l.after);
        }
        if let Some(m) = &edit.meta {
            meta |= m.before != m.after;
        }
    }
    (cells, layers, meta)
}

fn layer_diff(before: &[nbs_core::model::Layer], after: &[nbs_core::model::Layer]) -> usize {
    if before.len() != after.len() {
        // 增删层：整表算一次变化，具体几层由调用方对比图层列表自己看。
        return 1;
    }
    before
        .iter()
        .zip(after)
        .filter(|(a, b)| {
            a.name != b.name || a.lock != b.lock || a.volume != b.volume || a.stereo != b.stereo
        })
        .count()
}

fn current_report(state: &EditorState, message: String) -> ChangeReport {
    ChangeReport {
        cells_changed: 0,
        layers_changed: 0,
        meta_changed: false,
        rev: state.doc.song.rev,
        song_session: state.store.active_session(),
        dirty: state.doc.song.dirty,
        history_len: state.doc.history.len(),
        history_cursor: state.doc.history.cursor(),
        message,
    }
}

fn ok_data(state: &EditorState, data: Data, message: String) -> Outcome {
    Outcome::ok(data, current_report(state, message))
}

fn refuse(state: &mut EditorState, code: ErrorCode, message: String) -> Outcome {
    state.set_message(message.clone());
    Outcome::err(
        ErrorInfo::new(code, message),
        current_report(state, String::new()),
    )
}

fn fail(state: &mut EditorState, error: ErrorInfo) -> Outcome {
    let message = error.message.clone();
    state.set_message(message);
    Outcome::err(error, current_report(state, String::new()))
}

/// `expect_rev` 是乐观并发令牌：它挡住的是「你读的是 rev=41，我在中间又动了三下」这种
/// 自以为是的覆盖，不是权限。
fn rev_check(expect: Option<u64>, rev: u64) -> Result<(), ErrorInfo> {
    match expect {
        Some(want) if want != rev => Err(ErrorInfo::new(
            ErrorCode::RevConflict,
            say(
                format!("工程已经变了：你给的 expect_rev={want}，现在 rev={rev}。重新 get_notes / get_snapshot 再决定。"),
                format!("Song changed under you: expect_rev={want} but rev={rev}. Re-read before retrying."),
            ),
        )
            .with_detail(json!({ "expect_rev": want, "rev": rev }))),
        _ => Ok(()),
    }
}

/// 走界面同一条命令队列：`emit` + `flush` 之后，音频事件表、脏标记、历史全都按按钮那条路走。
fn run_command(state: &mut EditorState, ctx: &Context, cmd: Command) {
    state.emit(cmd);
    state.flush(ctx);
}

/// 后台文档写不走 `flush`：否则帧末的音频事件表同步会把当前标签的缓存改成后台标签的 rev。
fn run_document_command(state: &mut EditorState, ctx: &Context, cmd: Command) {
    crate::commands::dispatch(state, cmd, ctx);
}

/// 界面状态栏那一行字。命令自己会写回执文字，外部调用直接借用，省得第二套说法。
fn status_text(state: &EditorState) -> String {
    state
        .message
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ 写操作

type Written = Result<(Option<Data>, String), ErrorInfo>;

#[allow(clippy::too_many_arguments)]
fn write(
    state: &mut EditorState,
    ctx: &Context,
    ch: &ControlChannel,
    op: &Op,
    active: bool,
) -> Written {
    match op {
        Op::SetNotes {
            cells,
            label,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let prepared = prepare_cells(state, cells)?;
            let count = prepared.len();
            let mut tx = state.doc.begin(
                label
                    .clone()
                    .unwrap_or_else(|| say("写入音符", "Set notes")),
            );
            for (tick, layer, _) in &prepared {
                tx.watch(*tick, *layer);
            }
            for (tick, layer, note) in prepared {
                tx.song().set(tick, layer, note);
            }
            tx.commit();
            Ok((
                None,
                say(
                    format!("已写入 {count} 个音符"),
                    format!("Wrote {count} notes"),
                ),
            ))
        }
        Op::RemoveCells {
            cells,
            label,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            if cells.is_empty() {
                return Err(bad("cells 为空", "cells is empty"));
            }
            if cells.len() > MAX_CELLS_PER_CALL {
                return Err(too_many(cells.len()));
            }
            for (tick, layer) in cells {
                check_cell_target(state, *tick, *layer)?;
            }
            let count = cells.len();
            let mut tx = state.doc.begin(
                label
                    .clone()
                    .unwrap_or_else(|| say("删除音符", "Remove notes")),
            );
            for (tick, layer) in cells {
                tx.watch(*tick, *layer);
            }
            for (tick, layer) in cells {
                tx.song().remove(*tick, *layer);
            }
            tx.commit();
            Ok((
                None,
                say(
                    format!("已请求删除 {count} 个格子（原本空着的不会计入变更量）"),
                    format!("Removed {count} cells (already-empty ones show as zero change)"),
                ),
            ))
        }
        Op::BatchField {
            field,
            mode,
            value,
            scope,
            label,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let add = match mode.as_str() {
                "set" => false,
                "add" => true,
                other => {
                    return Err(bad(
                        format!("mode 只认 set / add，收到 {other:?}"),
                        format!("mode must be set or add, got {other:?}"),
                    ))
                }
            };
            let label = label
                .clone()
                .unwrap_or_else(|| say("批量修改", "Batch change"));
            let limits = field_limits(state);
            let mut skipped = 0usize;
            let (touched, clamped) = match scope {
                Scope::Selection => {
                    if state.doc.selection.is_empty() {
                        return Err(ErrorInfo::new(
                            ErrorCode::NotFound,
                            say(
                                "浮动选区是空的：先用 Select 选一批音符，或用 scope=rect/all 直接指定范围。",
                                "Floating selection is empty: Select first, or use scope=rect/all.",
                            ),
                        ));
                    }
                    let mut touched = 0usize;
                    let mut clamped = 0usize;
                    let mut tx = state.doc.begin(label);
                    tx.track_selection();
                    tx.selection().change_all(|note| {
                        touched += 1;
                        if apply_field(note, *field, add, *value, limits) {
                            clamped += 1;
                        }
                    });
                    tx.commit();
                    (touched, clamped)
                }
                other => {
                    let (targets, locked) = notes_in_scope(state, other)?;
                    skipped = locked;
                    let mut touched = 0usize;
                    let mut clamped = 0usize;
                    let mut tx = state.doc.begin(label);
                    for (tick, layer, _) in &targets {
                        tx.watch(*tick, *layer);
                    }
                    for (tick, layer, mut note) in targets {
                        touched += 1;
                        if apply_field(&mut note, *field, add, *value, limits) {
                            clamped += 1;
                        }
                        tx.song().set(tick, layer, note);
                    }
                    tx.commit();
                    (touched, clamped)
                }
            };
            Ok((
                None,
                change_line(field_name(*field), touched, clamped, add, skipped),
            ))
        }
        Op::Transpose {
            scope,
            semitones,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            if *semitones == 0 {
                return Ok((
                    None,
                    say("移调 0 半音，没有改动", "Transpose by 0: nothing to do"),
                ));
            }
            let limits = field_limits(state);
            let mut touched = 0usize;
            let mut clamped = 0usize;
            let mut skipped = 0usize;
            let label = say("移调", "Transpose");
            match scope {
                Scope::Selection => {
                    if state.doc.selection.is_empty() {
                        return Err(ErrorInfo::new(
                            ErrorCode::NotFound,
                            say(
                                "浮动选区是空的：先用 Select 选一批音符，或用 scope=rect/all。",
                                "Floating selection is empty: Select first, or use scope=rect/all.",
                            ),
                        ));
                    }
                    let mut tx = state.doc.begin(label);
                    tx.track_selection();
                    tx.selection().change_all(|note| {
                        touched += 1;
                        if apply_field(note, BatchField::Key, true, *semitones, limits) {
                            clamped += 1;
                        }
                    });
                    tx.commit();
                }
                other => {
                    let (targets, locked) = notes_in_scope(state, other)?;
                    skipped = locked;
                    let mut tx = state.doc.begin(label);
                    for (tick, layer, _) in &targets {
                        tx.watch(*tick, *layer);
                    }
                    for (tick, layer, mut note) in targets {
                        touched += 1;
                        if apply_field(&mut note, BatchField::Key, true, *semitones, limits) {
                            clamped += 1;
                        }
                        tx.song().set(tick, layer, note);
                    }
                    tx.commit();
                }
            }
            let mut message = say(
                format!("移调 {semitones} 半音：{touched} 个音符"),
                format!("Transposed {semitones} semitones over {touched} notes"),
            );
            if clamped > 0 {
                message.push_str(&say(
                    format!("（{clamped} 个撞到音域边界）"),
                    format!(" ({clamped} hit the range edge)"),
                ));
            }
            if skipped > 0 {
                message.push_str(&locked_skip_line(skipped));
            }
            Ok((None, message))
        }
        Op::Select { target, expect_rev } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let cmd = match target {
                SelectTarget::Rect {
                    tick_from,
                    layer_from,
                    tick_to,
                    layer_to,
                    copy,
                    additive,
                } => Command::SelectRect {
                    x0: *tick_from,
                    y0: *layer_from,
                    x1: *tick_to,
                    y1: *layer_to,
                    copy: *copy,
                    additive: *additive,
                    subtract: false,
                },
                SelectTarget::All => Command::SelectAll,
                SelectTarget::None => Command::Deselect,
                SelectTarget::Invert => Command::InvertSelection,
                SelectTarget::ByInstrument { instrument, invert } => {
                    check_instrument(state, *instrument)?;
                    Command::SelectByInstrument {
                        instrument: *instrument,
                        invert: *invert,
                    }
                }
                SelectTarget::Outside { lower, higher } => Command::SelectOutside {
                    lower: *lower,
                    higher: *higher,
                },
                SelectTarget::Custom => Command::SelectCustom,
            };
            if active {
                run_command(state, ctx, cmd);
            } else {
                run_document_command(state, ctx, cmd);
            }
            let info = selection_info(&state.doc.selection);
            Ok((
                Some(Data::Selection(Box::new(info.clone()))),
                say(
                    format!("选中 {} 个音符", info.notes),
                    format!("{} notes selected", info.notes),
                ),
            ))
        }
        Op::PlaceSelection { expect_rev } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            if state.doc.selection.is_empty() {
                return Err(ErrorInfo::new(
                    ErrorCode::NotFound,
                    say(
                        "浮动选区是空的，没什么可落回歌曲",
                        "Floating selection is empty",
                    ),
                ));
            }
            if active {
                run_command(state, ctx, Command::Deselect);
            } else {
                run_document_command(state, ctx, Command::Deselect);
            }
            Ok((
                None,
                say("选区已落回歌曲", "Selection placed back into the song"),
            ))
        }
        Op::LayerAdd {
            at,
            name,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let len = state.doc.song.layers.len() as u32;
            let index = at.unwrap_or(len);
            if index > len {
                return Err(out_of_range(
                    format!("图层序号 {index} 超出可插入范围 0..={len}"),
                    format!("Layer index {index} is outside the insertable range 0..={len}"),
                ));
            }
            let mut cmds = vec![Command::InsertLayer(index)];
            if let Some(text) = name {
                cmds.push(Command::RenameLayer(index, text.clone()));
            }
            for cmd in cmds {
                if active {
                    run_command(state, ctx, cmd);
                } else {
                    run_document_command(state, ctx, cmd);
                }
            }
            Ok((
                Some(Data::Layers {
                    layers: layer_infos(state),
                }),
                say(
                    format!(
                        "已在序号 {index} 插入图层，现在共 {} 层",
                        state.doc.song.layers.len()
                    ),
                    format!(
                        "Inserted layer at {index}, now {} layers",
                        state.doc.song.layers.len()
                    ),
                ),
            ))
        }
        Op::LayerRemove { index, expect_rev } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let len = state.doc.song.layers.len() as u32;
            if len <= 1 {
                return Err(bad(
                    "最后一层不能删（和界面一样）",
                    "The last layer cannot be removed (same as the UI)",
                ));
            }
            if *index >= len {
                return Err(out_of_range(
                    format!("图层序号 {index} 不存在，当前 0..={}", len - 1),
                    format!(
                        "Layer index {index} does not exist; valid range is 0..{}",
                        len - 1
                    ),
                ));
            }
            if active {
                run_command(state, ctx, Command::RemoveLayer(*index));
            } else {
                run_document_command(state, ctx, Command::RemoveLayer(*index));
            }
            Ok((
                Some(Data::Layers {
                    layers: layer_infos(state),
                }),
                say(
                    format!("已删除图层 {index}"),
                    format!("Removed layer {index}"),
                ),
            ))
        }
        Op::LayerUpdate {
            index,
            set,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let len = state.doc.song.layers.len() as u32;
            if *index >= len {
                return Err(out_of_range(
                    format!("图层序号 {index} 不存在，当前 0..={}", len - 1),
                    format!(
                        "Layer index {index} does not exist; valid range is 0..{}",
                        len - 1
                    ),
                ));
            }
            if set.name.is_none()
                && set.volume.is_none()
                && set.stereo.is_none()
                && set.lock.is_none()
            {
                return Err(bad("set 里一个字段都没有", "set carries no field"));
            }
            let lock = match set.lock.as_deref() {
                None => None,
                Some("normal") => Some(nbs_core::model::LockMode::Normal),
                Some("locked") => Some(nbs_core::model::LockMode::Locked),
                Some("solo") => Some(nbs_core::model::LockMode::Solo),
                Some(other) => {
                    return Err(bad(
                        format!("lock 只认 normal / locked / solo，收到 {other:?}"),
                        format!("lock must be normal / locked / solo, got {other:?}"),
                    ))
                }
            };
            // 属性改动不涉及层号重映射，一条事务里全部写完：一次调用 = 一次撤销。
            let mut tx = state.doc.begin(say("修改图层", "Modify layer"));
            tx.track_layers();
            if let Some(layer) = tx.song().layers.get_mut(*index as usize) {
                if let Some(name) = &set.name {
                    layer.name = name.clone();
                }
                if let Some(volume) = set.volume {
                    layer.volume = volume.min(VEL_MAX);
                }
                if let Some(stereo) = set.stereo {
                    layer.stereo = stereo.min(PAN_MAX);
                }
                if let Some(mode) = lock {
                    layer.lock = mode;
                }
            }
            tx.commit();
            Ok((
                Some(Data::Layers {
                    layers: layer_infos(state),
                }),
                say(
                    format!("已更新图层 {index}"),
                    format!("Updated layer {index}"),
                ),
            ))
        }
        Op::LayerMove {
            from,
            to,
            expect_rev,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            let len = state.doc.song.layers.len() as u32;
            if *from >= len || *to >= len {
                return Err(out_of_range(
                    format!(
                        "图层序号 from={from} to={to} 超出 0..{}",
                        len.saturating_sub(1)
                    ),
                    format!(
                        "Layer indices from={from} to={to} are outside 0..{}",
                        len.saturating_sub(1)
                    ),
                ));
            }
            if *from != *to {
                state.doc.move_layer(*from, *to);
            }
            Ok((
                Some(Data::Layers {
                    layers: layer_infos(state),
                }),
                say(
                    format!("图层 {from} → {to}"),
                    format!("Layer {from} moved to {to}"),
                ),
            ))
        }
        Op::SetMeta { meta, expect_rev } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            if meta == &SetMeta::default() {
                return Err(bad("meta 里一个字段都没有", "meta carries no field"));
            }
            let mut changed: Vec<&str> = Vec::new();
            // 元数据不走 `Song::set/remove`，dirty 不会自己亮。界面那条路是显式标的
            // （属性窗 `info_dirty`，lib.rs 那条「改元数据要标脏」钉着它），外部调用改的是
            // 同一批字段，不标就会出现「AI 改了速度但标题栏不带 *、退出也不问」，
            // 而 keep_or_replace 那道 discard_changes 闸正是读 dirty 的——漏标等于让闸看漏。
            let meta_before = state.doc.song.meta.clone();
            // 循环终点在 .nbs 里就是「歌曲长度」，只能靠结尾那颗 0 音量音符表达；
            // 直接复用 Document 的那条写法，不在这里抄第二份。
            if let Some(end) = meta.loop_end {
                let len = state.doc.song.song_length();
                if end > len {
                    state.doc.extend_loop_end(end);
                    changed.push("loop_end");
                } else if end < len {
                    // 缩短意味着删掉末尾的音符，不能在"改元数据"里偷偷做。
                    return Err(bad(
                        format!(
                            "loop_end={end} 小于当前歌曲长度 {len}；要缩短请先删掉末尾的音符"
                        ),
                        format!("loop_end={end} is shorter than the song ({len}); remove the trailing notes first"),
                    ));
                }
            }
            {
                let mut tx = state.doc.begin(say("修改元数据", "Change metadata"));
                tx.track_meta();
                let m = &mut tx.song().meta;
                if let Some(text) = &meta.name {
                    m.name = text.clone();
                    changed.push("name");
                }
                if let Some(text) = &meta.author {
                    m.author = text.clone();
                    changed.push("author");
                }
                if let Some(text) = &meta.original_author {
                    m.original_author = text.clone();
                    changed.push("original_author");
                }
                if let Some(text) = &meta.description {
                    m.description = text.clone();
                    changed.push("description");
                }
                if let Some(tempo) = meta.tempo {
                    let clamped = tempo.clamp(0.25, 180.0);
                    // 界面那条闸是 median(0.25, 值, 180)，这里同样夹取，但要说出来。
                    changed.push(if (clamped - tempo).abs() > 1e-6 {
                        "tempo(clamped to 0.25..=180)"
                    } else {
                        "tempo"
                    });
                    m.tempo = clamped;
                }
                if let Some(ts) = meta.time_signature {
                    m.time_signature = ts.clamp(2, 8);
                    changed.push("time_signature");
                }
                if let Some(on) = meta.loop_enabled {
                    m.loop_enabled = on;
                    changed.push("loop_enabled");
                }
                if let Some(start) = meta.loop_start {
                    m.loop_start = start;
                    changed.push("loop_start");
                }
                if let Some(max) = meta.loop_max {
                    m.loop_max = max;
                    changed.push("loop_max");
                }
                tx.commit();
            }
            // 改完元数据必须显式推一次时钟，否则循环与速度要等下一次起播才生效。
            if active {
                state.sync_loop_params();
            }
            // `changed` 记的是「调用方给了哪个字段」，同值也会记，所以标脏要另比一次。
            if state.doc.song.meta != meta_before {
                state.doc.song.dirty = true;
            }
            if changed.is_empty() {
                return Ok((
                    None,
                    say(
                        "元数据没变化：给的每个值都已等于当前值",
                        "Metadata unchanged: every value already matched",
                    ),
                ));
            }
            Ok((
                None,
                say(
                    format!("已修改元数据：{}", changed.join(", ")),
                    format!("Updated metadata: {}", changed.join(", ")),
                ),
            ))
        }
        Op::Undo { steps, expect_rev } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            roll(state, ctx, *steps, true, active)
        }
        Op::Redo { steps, expect_rev } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            roll(state, ctx, *steps, false, active)
        }
        Op::ClearSong {
            expect_rev,
            discard_changes,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            keep_or_replace(state, *discard_changes, ("清空", "Clearing the song"))?;
            run_command(state, ctx, Command::ClearSong);
            // 清掉的只有音符，图层一格都没动。不说清这一点，调用方就会按「空工程 = 一层」
            // 去配器，多出来的层还得再发一次 layer_remove 收——现场真踩过。
            let layers = state.doc.song.layers.len();
            let mut names: Vec<String> = state
                .doc
                .song
                .layers
                .iter()
                .take(12)
                .map(|l| l.name.clone())
                .collect();
            if layers > names.len() {
                names.push(say("…", "…"));
            }
            let joined = names.join(" / ");
            Ok((
                Some(Data::Layers {
                    layers: layer_infos(state),
                }),
                say(
                    format!("音符已全部清空；图层没动，还是 {layers} 层：{joined}"),
                    format!("All notes cleared; the {layers} layers are untouched: {joined}"),
                ),
            ))
        }
        Op::Playback(action) => playback(state, ctx, action),
        Op::OpenPath {
            path,
            discard_changes,
        } => {
            let target = resolve_path(state, ch, path, &["nbs", "schematic", "schem"], true)?;
            keep_or_replace(state, *discard_changes, ("开档", "Opening another file"))?;
            run_command(state, ctx, Command::OpenPath(target.clone()));
            let kind = extension(&target);
            let mut message = status_text(state);
            if !state.modals.is_empty() {
                message.push_str(&say(
                    "（界面还挂着旧版文件的提示框，需要 answer_modal 或人来回答）",
                    " (an old-file warning dialog is now pending: answer_modal or a human)",
                ));
            }
            Ok((
                Some(Data::File {
                    path: target.display().to_string(),
                    file_kind: format!("open:{kind}"),
                    notes: state.doc.song.total_blocks,
                }),
                message,
            ))
        }
        Op::SavePath {
            path,
            version,
            overwrite,
        } => {
            let target = resolve_path(state, ch, path, &["nbs"], false)?;
            if let Some(v) = version {
                if *v == 0 || *v > nbs_core::nbs::NBS_VERSION {
                    return Err(out_of_range(
                        format!(
                            "保存版本 {v} 不支持，可用 1..{}",
                            nbs_core::nbs::NBS_VERSION
                        ),
                        format!(
                            "Save version {v} unsupported; use 1..{}",
                            nbs_core::nbs::NBS_VERSION
                        ),
                    ));
                }
                if *v != state.doc.song.meta.save_version {
                    // 和界面「保存选项」走同一条：版本是元数据，所以它进历史、能撤销。
                    state
                        .doc
                        .edit_meta(say("修改保存版本", "Change save version"), |m| {
                            m.save_version = *v
                        });
                }
            }
            let same_file = state
                .doc
                .song
                .filename
                .as_deref()
                .is_some_and(|cur| normalize(Path::new(cur)) == target);
            if target.exists() && !*overwrite && !same_file {
                return Err(ErrorInfo::new(
                    ErrorCode::NotAllowed,
                    say(
                        format!(
                            "{} 已经存在，且不是这个工程当前对应的文件。确认要覆盖就带 overwrite=true。",
                            target.display()
                        ),
                        format!(
                            "{} already exists and is not this song's current file. Pass overwrite=true to replace it.",
                            target.display()
                        ),
                    ),
                ));
            }
            run_command(state, ctx, Command::SavePath(target.clone()));
            saved_or_error(state, &target)
        }
        Op::Save => {
            let Some(path) = state.doc.song.filename.clone() else {
                return Err(bad(
                    "当前工程还没有文件名，保存需要人指一条路径（界面会弹另存为）",
                    "Song has no file name yet; saving needs a human to pick a path (the UI opens Save As)",
                ));
            };
            run_command(state, ctx, Command::Save);
            saved_or_error(state, &path)
        }
        Op::ImportMidi {
            path,
            discard_changes,
            options,
            enhanced,
            remember,
        } => {
            let target = resolve_path(state, ch, path, &["mid", "midi"], true)?;
            keep_or_replace(state, *discard_changes, ("导入 MIDI", "Importing MIDI"))?;
            let computed = midi_compute(
                state,
                midi_parse(&target)?,
                target.display().to_string(),
                options.as_ref(),
                enhanced.as_ref(),
                remember.unwrap_or(false),
            )?;
            let MidiComputed {
                plan,
                song,
                opts,
                enh,
                midi,
            } = computed;
            if remember.unwrap_or(false) && !enh.enabled {
                // 与界面 `MidiImportRun` 传统分支同一个收尾：映射写回 GM 表，再落盘偏好。
                // `remember` 没给真时连 `save_prefs` 都不跟——那次调用没改用户的任何东西，
                // 顺手写一次设置文件就是无中生有。增强路径界面也不写回，照旧。
                state.midi_options = opts;
                state.midi_options.finish_import(&midi, &mut state.midi_gm);
                state.save_prefs();
            }
            apply_replacement(state, song);
            let mut message = midi_plan_line(&plan);
            if remember.unwrap_or(false) && !enh.enabled {
                message.push_str(&say(
                    "；映射已写回 GM 表并存进偏好",
                    "; mappings written back to the GM table and saved to preferences",
                ));
            }
            Ok((
                Some(Data::MidiPlan(Box::new(plan))),
                say(
                    format!("MIDI 导入完成（工程已换成这份）：{message}"),
                    format!("MIDI imported (song replaced): {message}"),
                ),
            ))
        }
        Op::Export {
            path,
            format,
            sample_rate,
            channels,
            include_locked,
            overwrite,
        } => export_job(
            state,
            ch,
            path,
            format,
            *sample_rate,
            *channels,
            *include_locked,
            *overwrite,
        ),
        Op::RunAction { action, args } => run_action(state, ctx, ch, action, args.as_ref()),
        Op::TranscribeProbe => transcribe_probe(state, ch),
        Op::TranscribePrepareEnv { cuda } => transcribe_prepare_env(state, ch, *cuda),
        Op::TranscribePrepareModel { options } => transcribe_prepare_model(state, ch, options),
        Op::TranscribeAudio {
            audio,
            options,
            save_midi,
            overwrite,
        } => transcribe_audio(
            state,
            ch,
            audio,
            options,
            save_midi.as_deref(),
            *overwrite,
        ),
        other => Err(ErrorInfo::new(
            ErrorCode::Unsupported,
            say(
                format!("这条能力在执行层里还没有对应实现：{other:?}"),
                format!("No executor support for {other:?}"),
            ),
        )),
    }
}

// ------------------------------------------------------------------ 校验与错误

fn out_of_range(cn: impl std::fmt::Display, en: impl std::fmt::Display) -> ErrorInfo {
    ErrorInfo::new(ErrorCode::OutOfRange, say(cn, en))
}

fn bad(cn: impl std::fmt::Display, en: impl std::fmt::Display) -> ErrorInfo {
    ErrorInfo::new(ErrorCode::BadArgument, say(cn, en))
}

fn too_many(count: usize) -> ErrorInfo {
    bad(
        format!("一次调用最多 {MAX_CELLS_PER_CALL} 格，这里给了 {count} 格，请分批。"),
        format!("{count} cells exceeds the {MAX_CELLS_PER_CALL}-per-call limit; split it up."),
    )
}

/// 一格能不能写。锁定层单独报出来，因为原版画笔同样拒绝在锁定层落子
/// （`commands.rs` 的 `Command::PlaceNote` 那条 `Layer is locked`）。
fn check_cell_target(state: &EditorState, tick: u32, layer: u32) -> Result<(), ErrorInfo> {
    let _ = tick;
    let len = state.doc.song.layers.len() as u32;
    if layer >= len {
        return Err(out_of_range(
            format!("图层 {layer} 不存在，当前只有 0..{}", len.saturating_sub(1)),
            format!(
                "Layer {layer} does not exist; only 0..{}",
                len.saturating_sub(1)
            ),
        ));
    }
    if state.doc.song.is_locked(layer) {
        return Err(ErrorInfo::new(
            ErrorCode::LockedLayer,
            say(
                format!("图层 {layer} 被锁定（或有别的层在独奏），界面也画不进去。先用 LayerUpdate 解锁。"),
                format!("Layer {layer} is locked (or another layer solos); the UI refuses to paint here too. Unlock it with LayerUpdate first."),
            ),
        ));
    }
    Ok(())
}

/// 音色索引能不能对上号：这是 AI 最容易看不见的那张表——它不知道本工程有几个自定义音色，
/// 随手写个 25 就会导出静音音符。宁可拒了让它先 GetInstruments。
fn check_instrument(state: &EditorState, instrument: u8) -> Result<(), ErrorInfo> {
    let customs = state.doc.song.custom_instruments.len() as u8;
    if instrument < FIRST_CUSTOM_INDEX || instrument - FIRST_CUSTOM_INDEX < customs {
        return Ok(());
    }
    Err(out_of_range(
        format!(
            "音色 {instrument} 不存在：内置 0..{}，自定义从 {FIRST_CUSTOM_INDEX} 起，本工程只有 {customs} 个自定义音色。先 get_instruments。",
            FIRST_CUSTOM_INDEX - 1
        ),
        format!(
            "Instrument {instrument} does not exist: built-ins 0..{}, customs start at {FIRST_CUSTOM_INDEX} and this song has {customs}. Call get_instruments first.",
            FIRST_CUSTOM_INDEX - 1
        ),
    ))
}

/// 把一条 `CellNote` 变成可落笔的 `(tick, layer, Note)`，全部越界与重复在这里拒掉，
/// 事务里不再有半途失败的可能。
fn prepare_cells(
    state: &EditorState,
    cells: &[CellNote],
) -> Result<Vec<(u32, u32, Note)>, ErrorInfo> {
    if cells.is_empty() {
        return Err(bad("cells 为空", "cells is empty"));
    }
    if cells.len() > MAX_CELLS_PER_CALL {
        return Err(too_many(cells.len()));
    }
    let mut seen = std::collections::HashSet::with_capacity(cells.len());
    let mut out = Vec::with_capacity(cells.len());
    for cell in cells {
        if !seen.insert((cell.tick, cell.layer)) {
            return Err(bad(
                format!(
                    "同一格 ({}, {}) 在一次调用里出现两次，落哪份不清楚。",
                    cell.tick, cell.layer
                ),
                format!(
                    "Cell ({}, {}) appears twice in one call; which value wins is ambiguous.",
                    cell.tick, cell.layer
                ),
            ));
        }
        if cell.key > KEY_MAX {
            return Err(out_of_range(
                format!("key {} 超出 0..={KEY_MAX}", cell.key),
                format!("key {} is outside 0..={KEY_MAX}", cell.key),
            ));
        }
        if cell.velocity.is_some_and(|v| v > VEL_MAX) {
            return Err(out_of_range(
                format!("velocity {:?} 超出 0..={VEL_MAX}", cell.velocity),
                format!("velocity {:?} is outside 0..={VEL_MAX}", cell.velocity),
            ));
        }
        if cell.panning.is_some_and(|v| v > PAN_MAX) {
            return Err(out_of_range(
                format!("panning {:?} 超出 0..={PAN_MAX}", cell.panning),
                format!("panning {:?} is outside 0..={PAN_MAX}", cell.panning),
            ));
        }
        if cell.pitch.is_some_and(|v| v.abs() > PIT_MAX) {
            return Err(out_of_range(
                format!("pitch {:?} 超出 ±{PIT_MAX} 音分", cell.pitch),
                format!("pitch {:?} is outside ±{PIT_MAX} cents", cell.pitch),
            ));
        }
        check_instrument(state, cell.instrument)?;
        check_cell_target(state, cell.tick, cell.layer)?;
        // 省略的三个字段按界面画笔默认：100 / 100 / 0。
        let mut note = Note::new(cell.instrument, cell.key);
        if let Some(v) = cell.velocity {
            note.velocity = v;
        }
        if let Some(v) = cell.panning {
            note.panning = v;
        }
        if let Some(v) = cell.pitch {
            note.pitch = v;
        }
        out.push((cell.tick, cell.layer, note));
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct FieldLimits {
    instrument_max: i32,
}

fn field_limits(state: &EditorState) -> FieldLimits {
    FieldLimits {
        instrument_max: FIRST_CUSTOM_INDEX as i32 + state.doc.song.custom_instruments.len() as i32
            - 1,
    }
}

fn field_name(field: BatchField) -> &'static str {
    match field {
        BatchField::Instrument => "instrument",
        BatchField::Key => "key",
        BatchField::Velocity => "velocity",
        BatchField::Panning => "panning",
        BatchField::Pitch => "pitch",
    }
}

/// 改一个字段；返回是否被夹过（夹过就得告诉调用方，不能悄悄吞掉一半音符）。
fn apply_field(
    note: &mut Note,
    field: BatchField,
    add: bool,
    value: i32,
    limits: FieldLimits,
) -> bool {
    let (current, low, high) = match field {
        BatchField::Instrument => (note.instrument as i32, 0, limits.instrument_max),
        BatchField::Key => (note.key as i32, 0, KEY_MAX as i32),
        BatchField::Velocity => (note.velocity as i32, 0, VEL_MAX as i32),
        BatchField::Panning => (note.panning as i32, 0, PAN_MAX as i32),
        BatchField::Pitch => (note.pitch as i32, -PIT_MAX as i32, PIT_MAX as i32),
    };
    let target = if add { current + value } else { value };
    let clamped = target.clamp(low, high.max(low));
    match field {
        BatchField::Instrument => note.instrument = clamped as u8,
        BatchField::Key => note.key = clamped as u8,
        BatchField::Velocity => note.velocity = clamped as u8,
        BatchField::Panning => note.panning = clamped as u8,
        BatchField::Pitch => note.pitch = clamped as i16,
    }
    clamped != target
}

fn locked_skip_line(skipped: usize) -> String {
    say(
        format!("，跳过锁定层上的 {skipped} 个音符"),
        format!(", skipped {skipped} notes on locked layers"),
    )
}

// ------------------------------------------------------------------ 换工程与落盘

/// 开档 / 导入 / 清空都会连着丢掉未保存改动与全部历史。界面那条路是**静默**丢的
/// （`open_nbs_at` 直接 `replace_song`），外部调用不许有这种代价不明的默认值。
///
/// `verb` 带两种说法：这里拼的是整句话，只给中文的话在英文档位会拼出半中半英的回执。
fn keep_or_replace(
    state: &EditorState,
    discard_changes: bool,
    verb: (&str, &str),
) -> Result<(), ErrorInfo> {
    if !state.doc.song.dirty || discard_changes {
        return Ok(());
    }
    let len = state.doc.history.len();
    Err(ErrorInfo::new(
        ErrorCode::NotAllowed,
        say(
            format!(
                "{}会丢掉当前工程的未保存改动和全部撤销历史（当前 {len} 条历史）。先 Save，或确认要丢就带 discard_changes=true。",
                verb.0
            ),
            format!(
                "{} would drop unsaved edits and the whole undo stack ({len} {}). Save first, or pass discard_changes=true to confirm the loss.",
                verb.1,
                if len == 1 { "entry" } else { "entries" }
            ),
        ),
    ))
}

/// 换掉整首歌之后要做的一整套，抄 `open_nbs_at` / `MidiImportRun` 的收尾：
/// 会话号、音频事件表、视口、落点、区间标记缺一不可。
fn apply_replacement(state: &mut EditorState, song: Song) {
    state.doc.replace_song(song);
    state.song_backup_path = None;
    state.reissue_session();
    state.prepare_audio();
    state.viewport.start_tick = 0;
    state.viewport.start_layer = 0;
    state.set_marker(0.0);
    state.clear_section();
}

/// 存盘的成败只能从后续状态反推：`Command::SavePath` 自己只往状态栏写一句话。
/// 脏标记被清零才算成功，这样错误不会伪装成一次成功回执。
fn saved_or_error(state: &EditorState, path: &Path) -> Written {
    let text = status_text(state);
    if state.doc.song.dirty {
        return Err(ErrorInfo::new(
            ErrorCode::Io,
            say(
                format!("写入 {} 失败：{text}", path.display()),
                format!("Writing {} failed: {text}", path.display()),
            ),
        ));
    }
    Ok((
        Some(Data::File {
            path: path.display().to_string(),
            file_kind: "nbs".into(),
            notes: state.doc.song.total_blocks,
        }),
        text,
    ))
}

// --------------------------------------------------------------- MIDI 导入
//
// 界面导入（`Command::MidiImportRun`）、`inspect_midi`、`import_midi` 三条路必须算出
// 同一首歌，所以基线只有 [`crate::commands::midi_options_base`] 一处、覆盖只在这里合并、
// 落歌只走 `nbs-io` 的公开入口。以后要加第四路，先把这三条并起来。

/// 一次 MIDI 计算的全部产物。`midi` 留着是因为 `remember` 收尾写回 GM 表还要用它。
struct MidiComputed {
    plan: MidiPlan,
    song: Song,
    opts: nbs_io::midi::MidiImportOptions,
    enh: nbs_io::midi_enhanced::EnhancedMidiOptions,
    midi: nbs_io::midi::MidiData,
}

/// 把「哪一行出的错」补到文案前面，否则调用方只看见一句「音色 25 不存在」，
/// 不知道自己是在通道 3 还是鼓音 38 上写错的。
fn where_about(prefix_cn: String, prefix_en: String, error: ErrorInfo) -> ErrorInfo {
    ErrorInfo {
        code: error.code,
        message: say(
            format!("{prefix_cn}{}", error.message),
            format!("{prefix_en}{}", error.message),
        ),
        detail: error.detail,
    }
}

/// 覆盖项校验：越界数、不存在的鼓音号、重复通道都在这里拒掉，
/// 合并阶段因此不可能半途失败。
fn check_midi_overrides(
    state: &EditorState,
    midi: &nbs_io::midi::MidiData,
    options: Option<&ImportMidiOptions>,
    enhanced: Option<&ImportEnhancedOptions>,
) -> Result<(), ErrorInfo> {
    if let Some(o) = options {
        if let Some(p) = o.precision {
            if !matches!(p, 0 | 1 | 3 | 7) {
                return Err(bad(
                    format!("精准度只收 0 / 1 / 3 / 7（界面 1 / 2 / 4 / 8 倍），收到 {p}"),
                    format!("precision must be 0, 1, 3 or 7 (UI 1x/2x/4x/8x), got {p}"),
                ));
            }
        }
        if let Some(h) = o.max_height {
            if h == 0 || h > 20 {
                return Err(out_of_range(
                    format!("每通道层数上限只收 1..=20（20 = 不限），收到 {h}"),
                    format!("max_height must be 1..=20 (20 = unlimited), got {h}"),
                ));
            }
        }
        for (field, value) in [
            ("note_duration_fade_start", o.note_duration_fade_start),
            ("note_duration_fade_end", o.note_duration_fade_end),
        ] {
            if let Some(v) = value {
                if v > 100 {
                    return Err(out_of_range(
                        format!("{field} 是百分比，只收 0..=100，收到 {v}"),
                        format!("{field} is a percentage 0..=100, got {v}"),
                    ));
                }
            }
        }
        let mut seen_channel = [false; 16];
        for c in &o.channels {
            if usize::from(c.channel) >= 16 {
                return Err(out_of_range(
                    format!("通道号只收 0..=15，收到 {}", c.channel),
                    format!("channel must be 0..=15, got {}", c.channel),
                ));
            }
            let index = usize::from(c.channel);
            if seen_channel[index] {
                return Err(bad(
                    format!("通道 {} 在 channels 里出现两次，不知道以哪条为准", c.channel),
                    format!("channel {} appears twice in channels", c.channel),
                ));
            }
            seen_channel[index] = true;
            if c.channel == 9 {
                return Err(bad(
                    "通道 9 是打击乐，映射写在 drums，不在 channels",
                    "Channel 9 is percussion; map it in drums, not channels",
                ));
            }
            if !(-12..=12).contains(&c.octave) {
                return Err(out_of_range(
                    format!("通道 {} 的八度位移收 -12..=12，收到 {}", c.channel, c.octave),
                    format!("Channel {} octave shift must be -12..=12, got {}", c.channel, c.octave),
                ));
            }
            if c.instrument != -1 {
                let raw = c.instrument;
                let index = u8::try_from(raw).map_err(|_| {
                    out_of_range(
                        format!("通道 {index} 的音色只收 -1（忽略该通道）或音色号，收到 {raw}"),
                        format!("Channel {index} instrument must be -1 (ignore) or an index, got {raw}"),
                    )
                })?;
                check_instrument(state, index)
                    .map_err(|e| where_about(format!("通道 {index}："), format!("Channel {index}: "), e))?;
            }
        }
        let mut seen_drum: Vec<u8> = Vec::new();
        for d in &o.drums {
            if d.key < 0 || d.key > KEY_MAX as i8 {
                return Err(out_of_range(
                    format!(
                        "鼓音 {} 的键号只收 0..={KEY_MAX}（界面上那列已经是加过 33 的最终键号），收到 {}",
                        d.note, d.key
                    ),
                    format!(
                        "Drum note {}: key must be 0..={KEY_MAX} (the UI column is the final key, +33 already applied), got {}",
                        d.note, d.key
                    ),
                ));
            }
            if midi.perc_notes.iter().all(|&n| n != d.note) {
                return Err(bad(
                    format!(
                        "这个文件的鼓音里没有音号 {}；可用清单看 inspect_midi 回包的 drums",
                        d.note
                    ),
                    format!(
                        "Note {} is not a percussion note in this file; see drums from inspect_midi",
                        d.note
                    ),
                ));
            }
            if seen_drum.contains(&d.note) {
                return Err(bad(
                    format!("鼓音 {} 在 drums 里出现两次，不知道以哪条为准", d.note),
                    format!("drum note {} appears twice in drums", d.note),
                ));
            }
            seen_drum.push(d.note);
            if d.instrument != -1 {
                let raw = d.instrument;
                let index = u8::try_from(raw).map_err(|_| {
                    out_of_range(
                        format!("鼓音 {} 的音色只收 -1（静音）或音色号，收到 {raw}", d.note),
                        format!("Drum note {} instrument must be -1 (mute) or an index, got {raw}", d.note),
                    )
                })?;
                check_instrument(state, index).map_err(|e| {
                    where_about(format!("鼓音 {}：", d.note), format!("Drum note {}: ", d.note), e)
                })?;
            }
        }
        if let Some(parts) = &o.tail_parts {
            for &index in parts {
                if usize::try_from(index)
                    .map(|i| i >= midi.parts.len())
                    .unwrap_or(true)
                {
                    return Err(out_of_range(
                        format!(
                            "声部下标 {index} 不存在：这个文件只有 {} 个声部（0..{}）",
                            midi.parts.len(),
                            midi.parts.len().saturating_sub(1)
                        ),
                        format!(
                            "Part index {index} is out of range: this file has {} parts (0..{})",
                            midi.parts.len(),
                            midi.parts.len().saturating_sub(1)
                        ),
                    ));
                }
            }
        }
    }
    if let Some(e) = enhanced {
        let extras = e.range_mode.is_some()
            || e.smart_substitute.is_some()
            || e.force_fold.is_some()
            || e.sustain.is_some()
            || e.sustain_fade.is_some()
            || e.separate_voices.is_some()
            || e.voice_ignore_metadata.is_some()
            || e.dedupe_notes.is_some()
            || !e.channels.is_empty()
            || !e.substitutes.is_empty();
        if !e.enabled.unwrap_or(false) && extras {
            return Err(bad(
                "给了 enhanced 的其他字段却没带 enabled=true：这些字段会被整个跳过、走传统导入。要么带上 enabled，要么别给。",
                "enhanced fields given without enabled=true would be skipped and the traditional importer used. Pass enabled or drop the fields.",
            ));
        }
        let mut seen_channel = [false; 16];
        for c in &e.channels {
            if usize::from(c.channel) >= 16 {
                return Err(out_of_range(
                    format!("通道号只收 0..=15，收到 {}", c.channel),
                    format!("channel must be 0..=15, got {}", c.channel),
                ));
            }
            let index = usize::from(c.channel);
            if seen_channel[index] {
                return Err(bad(
                    format!("通道 {} 在 enhanced.channels 里出现两次", c.channel),
                    format!("channel {} appears twice in enhanced.channels", c.channel),
                ));
            }
            seen_channel[index] = true;
            if let Some(t) = c.transpose {
                if !(-72..=72).contains(&t) {
                    return Err(out_of_range(
                        format!("通道 {} 的移调收 -72..=72 半音，收到 {t}", c.channel),
                        format!("Channel {} transpose must be -72..=72 semitones, got {t}", c.channel),
                    ));
                }
            }
            if c.slots.len() > 3 {
                return Err(bad(
                    format!("通道 {} 给了 {} 个槽位，一个通道最多 3 个", c.channel, c.slots.len()),
                    format!("Channel {} has {} slots; at most 3 per channel", c.channel, c.slots.len()),
                ));
            }
            for (slot, value) in c.slots.iter().enumerate() {
                if let Some(instrument) = value.instrument {
                    if instrument >= FIRST_CUSTOM_INDEX {
                        return Err(out_of_range(
                            format!(
                                "通道 {} 的槽位 {} 只收内置音色 0..{}（增强导入不用自定义音色），收到 {instrument}",
                                c.channel,
                                slot + 1,
                                FIRST_CUSTOM_INDEX - 1
                            ),
                            format!(
                                "Channel {} slot {} must be a built-in 0..{} (enhanced import does not use custom instruments), got {instrument}",
                                c.channel,
                                slot + 1,
                                FIRST_CUSTOM_INDEX - 1
                            ),
                        ));
                    }
                }
                if let Some(volume) = value.volume {
                    if volume > 100 {
                        return Err(out_of_range(
                            format!("通道 {} 的槽位 {} 音量是百分比，只收 0..=100，收到 {volume}", c.channel, slot + 1),
                            format!("Channel {} slot {} volume is a percentage 0..=100, got {volume}", c.channel, slot + 1),
                        ));
                    }
                }
            }
        }
        let mut seen_substitute: Vec<u8> = Vec::new();
        for s in &e.substitutes {
            if s.instrument >= FIRST_CUSTOM_INDEX {
                return Err(out_of_range(
                    format!(
                        "替换表的音色只收内置 0..{}，收到 {}",
                        FIRST_CUSTOM_INDEX - 1,
                        s.instrument
                    ),
                    format!(
                        "Substitution rows cover built-ins 0..{} only, got {}",
                        FIRST_CUSTOM_INDEX - 1,
                        s.instrument
                    ),
                ));
            }
            if seen_substitute.contains(&s.instrument) {
                return Err(bad(
                    format!("音色 {} 在 substitutes 里出现两次", s.instrument),
                    format!("instrument {} appears twice in substitutes", s.instrument),
                ));
            }
            seen_substitute.push(s.instrument);
            for (side, target) in [("high", s.high), ("low", s.low)] {
                if let Some(instrument) = target {
                    if instrument >= FIRST_CUSTOM_INDEX {
                        return Err(out_of_range(
                            format!("音色 {} 的 {side} 替换只收内置 0..{}，收到 {instrument}", s.instrument, FIRST_CUSTOM_INDEX - 1),
                            format!("Instrument {} {side} replacement must be a built-in 0..{}, got {instrument}", s.instrument, FIRST_CUSTOM_INDEX - 1),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// 基线 ⊕ 覆盖 = 这次真正生效的两套选项。校验先过，所以这里每一步都直接落值。
fn midi_effective(
    state: &EditorState,
    midi: &nbs_io::midi::MidiData,
    options: Option<&ImportMidiOptions>,
    enhanced: Option<&ImportEnhancedOptions>,
    remember: bool,
) -> Result<(nbs_io::midi::MidiImportOptions, nbs_io::midi_enhanced::EnhancedMidiOptions), ErrorInfo> {
    check_midi_overrides(state, midi, options, enhanced)?;
    let mut opts = crate::commands::midi_options_base(state, midi);
    // 省略 `remember` 就是「别动我的长期设置」。界面上那个勾不参与这条判定——
    // 写回 GM 表动的是用户的偏好，外部调用方不能替用户点它。
    opts.remember = remember;
    if let Some(o) = options {
        opts.remove_silent = o.remove_silent.unwrap_or(opts.remove_silent);
        opts.name_layers = o.name_layers.unwrap_or(opts.name_layers);
        opts.name_after_patch = o.name_after_patch.unwrap_or(opts.name_after_patch);
        opts.tempo_from_file = o.tempo_from_file.unwrap_or(opts.tempo_from_file);
        opts.tempo_changer = o.tempo_changer.unwrap_or(opts.tempo_changer);
        opts.keep_octave_range = o.keep_octave_range.unwrap_or(opts.keep_octave_range);
        opts.use_velocity = o.use_velocity.unwrap_or(opts.use_velocity);
        opts.note_duration = o.note_duration.unwrap_or(opts.note_duration);
        opts.note_duration_fade = o.note_duration_fade.unwrap_or(opts.note_duration_fade);
        if let Some(value) = o.max_height {
            opts.max_height = value;
        }
        if let Some(value) = o.precision {
            opts.precision = value;
        }
        if let Some(value) = o.note_duration_fade_start {
            opts.note_duration_fade_start = value;
        }
        if let Some(value) = o.note_duration_fade_end {
            opts.note_duration_fade_end = value;
        }
        for c in &o.channels {
            let index = usize::from(c.channel);
            opts.channel_ins[index] = c.instrument;
            opts.channel_octave[index] = c.octave;
        }
        for d in &o.drums {
            // 校验阶段保证音号在这个文件里、下标两张表都取到到；取不到就当没这条。
            if let Some(index) = midi.perc_notes.iter().position(|&n| n == d.note) {
                if let Some(ins) = opts.perc_ins.get_mut(index) {
                    *ins = d.instrument;
                }
                if let Some(key) = opts.perc_pitch.get_mut(index) {
                    *key = d.key;
                }
            }
        }
        if let Some(parts) = &o.tail_parts {
            // 给了就以它为准：先全关，再点亮列出的那些；空数组 = 尾音全关。
            for flag in &mut opts.part_durations {
                *flag = false;
            }
            for &index in parts {
                if let Some(flag) = opts.part_durations.get_mut(usize::try_from(index).unwrap_or(usize::MAX)) {
                    *flag = true;
                }
            }
        }
    }
    // 增强档的基线是「这个文件的预设」，和 `load_midi_for_import` 里给界面的那份同源；
    // 预设本身就带 `enabled = false`，所以省略即传统路径。
    let mut enh = nbs_io::midi_enhanced::EnhancedMidiOptions::for_midi(midi);
    if let Some(e) = enhanced {
        enh.enabled = e.enabled.unwrap_or(false);
        if let Some(mode) = e.range_mode {
            enh.range_mode = match mode {
                ImportRangeMode::Keep => nbs_io::midi_enhanced::RangeMode::Keep,
                ImportRangeMode::Fold => nbs_io::midi_enhanced::RangeMode::Fold,
                ImportRangeMode::Octave => nbs_io::midi_enhanced::RangeMode::Octave,
                ImportRangeMode::Semitone => nbs_io::midi_enhanced::RangeMode::Semitone,
            };
        }
        enh.smart_substitute = e.smart_substitute.unwrap_or(enh.smart_substitute);
        enh.force_fold = e.force_fold.unwrap_or(enh.force_fold);
        // MCP 契约里 sustain 还是 bool（origin/main 已发布）：true = 所有轨道都重敲。
        // 三态的「只重敲选中的轨道」和白名单只在界面里给，MCP 读回统一报 true。
        if let Some(on) = e.sustain {
            enh.sustain = if on {
                nbs_io::midi_enhanced::SustainMode::All
            } else {
                nbs_io::midi_enhanced::SustainMode::Off
            };
        }
        enh.sustain_fade = e.sustain_fade.unwrap_or(enh.sustain_fade);
        enh.separate_voices = e.separate_voices.unwrap_or(enh.separate_voices);
        enh.voice_ignore_metadata = e
            .voice_ignore_metadata
            .unwrap_or(enh.voice_ignore_metadata);
        enh.dedupe_notes = e.dedupe_notes.unwrap_or(enh.dedupe_notes);
        let mut rerecommend = e.range_mode.is_some();
        for c in &e.channels {
            let combination = &mut enh.channels[usize::from(c.channel)];
            if !c.slots.is_empty() {
                let mut slots = [nbs_io::midi_enhanced::MidiSlot::default(); 3];
                for (index, slot) in c.slots.iter().enumerate() {
                    slots[index] = nbs_io::midi_enhanced::MidiSlot {
                        instrument: slot.instrument,
                        volume: slot.volume.unwrap_or(100),
                    };
                }
                combination.slots = slots;
                // 界面里改槽位乐器/音量也会重算推荐位移，这里跟着走。
                rerecommend = true;
            }
        }
        if rerecommend {
            enh.recommend_offsets(midi);
        }
        for c in &e.channels {
            if let Some(value) = c.transpose {
                // 手动值最后落地：`recommend_offsets` 会覆盖 transpose，界面上也是手改赢。
                enh.channels[usize::from(c.channel)].transpose = value;
            }
        }
        for s in &e.substitutes {
            // 给一行就整行替换：省略 high/low 就是那一侧不换。
            enh.substitutes[usize::from(s.instrument)] = nbs_io::midi_enhanced::InstrumentSubstitution {
                high: s.high,
                low: s.low,
            };
        }
    }
    Ok((opts, enh))
}

/// 读文件 + 解析。失败一律 `io`：这时候工程一点没动，剩下的只有磁盘和格式。
fn midi_parse(target: &Path) -> Result<nbs_io::midi::MidiData, ErrorInfo> {
    nbs_io::midi::parse_midi_path(target).map_err(|error| ErrorInfo::new(ErrorCode::Io, error))
}

/// 算歌 + 出核对单，但不碰工程。`inspect_midi` 用完就丢，`import_midi` 拿去落地。
///
/// 收的是已经解析好的 `MidiData` 而不是路径，因为预检、导入和测试都要走这同一条算法；
/// 谁也别在旁边再算一遍自己的版本。
fn midi_compute(
    state: &EditorState,
    midi: nbs_io::midi::MidiData,
    path_label: String,
    options: Option<&ImportMidiOptions>,
    enhanced: Option<&ImportEnhancedOptions>,
    remember: bool,
) -> Result<MidiComputed, ErrorInfo> {
    let (opts, enh) = midi_effective(state, &midi, options, enhanced, remember)?;
    let (song, report) = if enh.enabled {
        let (song, report) = nbs_io::midi_enhanced::import_midi_enhanced(&midi, &opts, &enh)
            // 增强导入只会因为配置本身不合法而失败（比如槽位写了自定义音色），
            // 文案由 `nbs-io` 按当前语言出，这里原样转给调用方。
            .map_err(|error| bad(error.clone(), error))?;
        (song, Some(report))
    } else {
        (nbs_io::midi::import_midi_data(&midi, &opts), None)
    };
    let mut per_channel = [0usize; 16];
    let mut per_drum = [0usize; 128];
    let mut per_part = vec![0usize; midi.parts.len()];
    let mut silenced_notes = 0usize;
    for track in &midi.tracks {
        for event in &track.events {
            per_channel[usize::from(event.channel.min(15))] += 1;
            if event.channel == 9 {
                per_drum[usize::from(event.note)] += 1;
            }
            if let Some(count) = per_part.get_mut(event.part) {
                *count += 1;
            }
            if nbs_io::midi::is_silenced(&midi, &opts, event) {
                silenced_notes += 1;
            }
        }
    }
    let out_of_range = song
        .iter_notes()
        .filter(|(_, _, note)| note.key < MC_KEY_MIN || note.key > MC_KEY_MAX)
        .count();
    let plan = MidiPlan {
        path: path_label,
        format: midi.format,
        division: midi.division,
        smpte_division: midi.division & 0x8000 != 0,
        channels_seen: midi.channels,
        tracks: midi
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| MidiTrackInfo {
                index: index as u32,
                name: track.name.clone(),
                events: track.events.len(),
            })
            .collect(),
        channels: (0..=midi.channels)
            .filter(|channel| *channel != 9)
            .map(|channel| {
                let index = usize::from(channel);
                let patch = midi.channel_patch[index];
                let instrument = opts.channel_ins[index];
                MidiChannelPlan {
                    channel,
                    patch,
                    patch_name: if patch < 128 {
                        nbs_io::MidiGmTable::patch_name(patch as u8).to_string()
                    } else {
                        String::new()
                    },
                    instrument,
                    octave: opts.channel_octave[index],
                    notes: per_channel[index],
                    silent: instrument == -1,
                }
            })
            .collect(),
        drums: midi
            .perc_notes
            .iter()
            .enumerate()
            .map(|(index, &note)| MidiDrumPlan {
                note,
                name: nbs_io::MidiGmTable::drum_name(note).to_string(),
                instrument: opts.perc_ins.get(index).copied().unwrap_or(-1),
                key: opts.perc_pitch.get(index).copied().unwrap_or(0),
                notes: per_drum[usize::from(note)],
            })
            .collect(),
        parts: midi
            .parts
            .iter()
            .enumerate()
            .map(|(index, part)| MidiPartPlan {
                index: index as u32,
                track: part.track as u32,
                channel: part.channel,
                patch: part.patch,
                tail: opts
                    .part_durations
                    .get(index)
                    .copied()
                    .unwrap_or(part.note_duration),
                notes: per_part[index],
            })
            .collect(),
        tempo_changes: midi.tempo_changers.len(),
        notes: song.total_blocks,
        layers: song.layers.len(),
        layer_names: song.layers.iter().map(|layer| layer.name.clone()).collect(),
        length_ticks: song.song_length(),
        tempo: song.meta.tempo,
        bpm: song.meta.bpm(),
        out_of_range,
        silenced_notes,
        enhanced: report.map(|report| MidiEnhancedReport {
            notes: report.notes,
            out_of_range: report.out_of_range,
            substituted: report.substituted,
            folded: report.folded,
            voiced_notes: report.voiced_notes,
            deduped: report.deduped,
        }),
        applied: midi_echo(&midi, &opts),
        applied_enhanced: midi_echo_enhanced(&enh),
    };
    Ok(MidiComputed {
        plan,
        song,
        opts,
        enh,
        midi,
    })
}

/// 生效选项回显成线上形状，每个字段都给满——回显的是「这次真正用了什么」，
/// 不是调用方当初发过来的那份省略稿。
fn midi_echo(midi: &nbs_io::midi::MidiData, opts: &nbs_io::midi::MidiImportOptions) -> ImportMidiOptions {
    ImportMidiOptions {
        remove_silent: Some(opts.remove_silent),
        name_layers: Some(opts.name_layers),
        name_after_patch: Some(opts.name_after_patch),
        tempo_from_file: Some(opts.tempo_from_file),
        tempo_changer: Some(opts.tempo_changer),
        max_height: Some(opts.max_height),
        keep_octave_range: Some(opts.keep_octave_range),
        use_velocity: Some(opts.use_velocity),
        precision: Some(opts.precision),
        note_duration: Some(opts.note_duration),
        note_duration_fade: Some(opts.note_duration_fade),
        note_duration_fade_start: Some(opts.note_duration_fade_start),
        note_duration_fade_end: Some(opts.note_duration_fade_end),
        channels: (0..=midi.channels)
            .filter(|channel| *channel != 9)
            .map(|channel| {
                let index = usize::from(channel);
                ImportChannelMap {
                    channel,
                    instrument: opts.channel_ins[index],
                    octave: opts.channel_octave[index],
                }
            })
            .collect(),
        drums: midi
            .perc_notes
            .iter()
            .enumerate()
            .map(|(index, &note)| ImportDrumMap {
                note,
                instrument: opts.perc_ins.get(index).copied().unwrap_or(-1),
                key: opts.perc_pitch.get(index).copied().unwrap_or(0),
            })
            .collect(),
        tail_parts: Some(
            opts.part_durations
                .iter()
                .enumerate()
                .filter(|(_, flag)| **flag)
                .map(|(index, _)| index as u32)
                .collect(),
        ),
    }
}

/// 走传统路径时这里只留一句 `enabled: false`——把没用上的 16 通道 × 3 槽和 20 行替换表
/// 塞进每一份回执，只会让调用方多读两百行与本次无关的 JSON。
fn midi_echo_enhanced(enh: &nbs_io::midi_enhanced::EnhancedMidiOptions) -> ImportEnhancedOptions {
    if !enh.enabled {
        return ImportEnhancedOptions {
            enabled: Some(false),
            ..ImportEnhancedOptions::default()
        };
    }
    ImportEnhancedOptions {
        enabled: Some(true),
        range_mode: Some(match enh.range_mode {
            nbs_io::midi_enhanced::RangeMode::Keep => ImportRangeMode::Keep,
            nbs_io::midi_enhanced::RangeMode::Fold => ImportRangeMode::Fold,
            nbs_io::midi_enhanced::RangeMode::Octave => ImportRangeMode::Octave,
            nbs_io::midi_enhanced::RangeMode::Semitone => ImportRangeMode::Semitone,
        }),
        smart_substitute: Some(enh.smart_substitute),
        force_fold: Some(enh.force_fold),
        sustain: Some(enh.sustain != nbs_io::midi_enhanced::SustainMode::Off),
        sustain_fade: Some(enh.sustain_fade),
        separate_voices: Some(enh.separate_voices),
        voice_ignore_metadata: Some(enh.voice_ignore_metadata),
        dedupe_notes: Some(enh.dedupe_notes),
        channels: (0..16u8)
            .map(|channel| {
                let combination = &enh.channels[usize::from(channel)];
                ImportEnhancedChannel {
                    channel,
                    transpose: Some(combination.transpose),
                    slots: combination
                        .slots
                        .iter()
                        .map(|slot| ImportSlot {
                            instrument: slot.instrument,
                            volume: Some(slot.volume),
                        })
                        .collect(),
                }
            })
            .collect(),
        substitutes: (0..FIRST_CUSTOM_INDEX)
            .map(|instrument| ImportSubstitution {
                instrument,
                high: enh.substitutes[usize::from(instrument)].high,
                low: enh.substitutes[usize::from(instrument)].low,
            })
            .collect(),
    }
}

/// 核对单里最该被看见的那几个数，一行说完；细节调用方自己从 `data` 里取。
fn midi_plan_line(plan: &MidiPlan) -> String {
    let mode = if plan.applied_enhanced.enabled == Some(true) {
        say("增强导入", "enhanced")
    } else {
        say("传统导入", "traditional")
    };
    let mut line = say(
        format!(
            "{mode}：{} 个音符、{} 层、{} tick（BPM {:.0}）；超 MC 音域 {}、按映射判静音 {}",
            plan.notes,
            plan.layers,
            plan.length_ticks,
            plan.bpm,
            plan.out_of_range,
            plan.silenced_notes
        ),
        format!(
            "{mode}: {} notes, {} layers, {} ticks (BPM {:.0}); {} outside native range (key {MC_KEY_MIN}..{MC_KEY_MAX}), {} silenced by the mappings",
            plan.notes,
            plan.layers,
            plan.length_ticks,
            plan.bpm,
            plan.out_of_range,
            plan.silenced_notes
        ),
    );
    if plan.tempo_changes > 0 {
        line.push_str(&say(
            format!("；文件有 {} 处速度变化，tempo_changer={}", plan.tempo_changes, plan.applied.tempo_changer.unwrap_or(false)),
            format!("; {} tempo change(s) in the file, tempo_changer={}", plan.tempo_changes, plan.applied.tempo_changer.unwrap_or(false)),
        ));
    }
    if let Some(report) = &plan.enhanced {
        line.push_str(&say(
            format!(
                "；替换 {}、折叠 {}、拆声部音符 {}、去重 {}",
                report.substituted, report.folded, report.voiced_notes, report.deduped
            ),
            format!(
                "; substituted {}, folded {}, voiced {}, deduped {}",
                report.substituted, report.folded, report.voiced_notes, report.deduped
            ),
        ));
    }
    line
}

/// 只读预检：算一遍、报一份核对单，工程一动不动。
fn midi_inspect(
    state: &EditorState,
    ch: &ControlChannel,
    raw: &str,
    options: Option<&ImportMidiOptions>,
    enhanced: Option<&ImportEnhancedOptions>,
) -> Result<(Data, String), ErrorInfo> {
    let target = resolve_path(state, ch, raw, &["mid", "midi"], true)?;
    let midi = midi_parse(&target)?;
    let computed = midi_compute(
        state,
        midi,
        target.display().to_string(),
        options,
        enhanced,
        false,
    )?;
    let line = midi_plan_line(&computed.plan);
    Ok((
        Data::MidiPlan(Box::new(computed.plan)),
        say(
            format!("MIDI 预检完成（工程未改动）：{line}"),
            format!("MIDI preview done (song untouched): {line}"),
        ),
    ))
}

// ------------------------------------------------------------------ 路径闸

/// 词法归一：补上当前目录、就地消掉 `.` 与 `..`。不用 `canonicalize`，它要求文件已存在，
/// 而「存到新路径」正是我们要支持的写法。
fn normalize(path: &Path) -> PathBuf {
    let mut out = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
            Component::RootDir | Component::Prefix(_) => out.push(component.as_os_str()),
        }
    }
    out
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// 把调用方给的路径过三道：非空、扩展名对、落在允许的目录里。
/// `must_exist` 用于「读」的一侧——开一个不存在的文件不该先烧掉一次白名单检查。
fn resolve_path(
    state: &EditorState,
    ch: &ControlChannel,
    raw: &str,
    exts: &[&str],
    must_exist: bool,
) -> Result<PathBuf, ErrorInfo> {
    if raw.trim().is_empty() {
        return Err(bad("path 为空", "empty path"));
    }
    let path = normalize(Path::new(raw));
    let ext = extension(&path);
    if !exts.iter().any(|want| *want == ext) {
        return Err(bad(
            format!("扩展名 {ext:?} 不认识，这里只接受 {}", exts.join(" / ")),
            format!(
                "Extension {ext:?} not accepted; only {} here",
                exts.join(" / ")
            ),
        ));
    }
    if must_exist && !path.is_file() {
        return Err(ErrorInfo::new(
            ErrorCode::NotFound,
            say(
                format!("文件不存在或不是普通文件：{}", path.display()),
                format!("Not an existing file: {}", path.display()),
            ),
        ));
    }
    let allowed = ch
        .policy
        .lock()
        .map(|policy| policy.allow_any_path || path_in_allowed_dirs(state, &policy, &path))
        .unwrap_or(false);
    if !allowed {
        let dirs = ch
            .policy
            .lock()
            .map(|policy| {
                let mut list = seed_dirs(state);
                list.extend(policy.allowed_dirs.iter().cloned());
                list
            })
            .unwrap_or_default();
        return Err(ErrorInfo::new(
            ErrorCode::NotAllowed,
            say(
                format!(
                    "{} 不在允许的目录里。当前允许：{}。要放开更多目录，启动时加 --control-dir=<目录>。",
                    path.display(),
                    dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("、")
                ),
                format!(
                    "{} is outside the allowed directories. Allowed: {}. Add more with --control-dir=<path> at startup.",
                    path.display(),
                    dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")
                ),
            ),
        ));
    }
    Ok(path)
}

fn path_in_allowed_dirs(state: &EditorState, policy: &Policy, path: &Path) -> bool {
    // 工程自己的目录随时算进来：它随开档变，往白名单里堆会变陈旧。
    let song_dir = state
        .doc
        .song
        .filename
        .as_deref()
        .and_then(|p| p.parent().map(PathBuf::from));
    policy
        .allowed_dirs
        .iter()
        .chain(song_dir.iter())
        .any(|dir| path.starts_with(dir))
}

// ------------------------------------------------------------------ 其余写操作

fn roll(
    state: &mut EditorState,
    ctx: &Context,
    steps: Option<u32>,
    undo: bool,
    active: bool,
) -> Written {
    let want = steps.unwrap_or(1).clamp(1, MAX_HISTORY_STEPS) as usize;
    let mut done = 0usize;
    while done < want {
        let more = if undo {
            state.doc.history.can_undo()
        } else {
            state.doc.history.can_redo()
        };
        if !more {
            break;
        }
        let command = if undo { Command::Undo } else { Command::Redo };
        if active {
            run_command(state, ctx, command);
        } else {
            run_document_command(state, ctx, command);
        }
        done += 1;
    }
    if done == 0 {
        return Err(ErrorInfo::new(
            ErrorCode::NotFound,
            say(
                if undo {
                    "没有可撤销的操作"
                } else {
                    "没有可重做的操作"
                },
                if undo {
                    "Nothing to undo"
                } else {
                    "Nothing to redo"
                },
            ),
        ));
    }
    let tail = if done < want {
        say("（已到栈边界）", "(reached the end of the stack)")
    } else {
        String::new()
    };
    Ok((
        Some(Data::History(Box::new(history_info(state)))),
        say(
            format!("已{} {done} 步{tail}", if undo { "撤销" } else { "重做" }),
            format!(
                "{} {done} step(s){tail}",
                if undo { "Undid" } else { "Redid" }
            ),
        ),
    ))
}

fn playback(state: &mut EditorState, ctx: &Context, action: &Playback) -> Written {
    match action {
        Playback::Play => {
            if state.audio.is_playing() {
                return Ok((None, say("已经在播放", "Already playing")));
            }
            // 播放头停在曲尾之外时，`begin_playback` 那道归零闸还带一个尾巴余量（尾音要放完），
            // 于是「落在 137、歌只有 121」这种情况会起播即停，回执再说一句「开始播放」，
            // 调用方只能靠自己先 seek。这里按真正的曲尾归零，并在回执里说清了落点动过。
            let length = state.doc.song.song_length() as f64;
            let parked = state.audio.position_ticks();
            let rewound = parked > 0.0 && parked >= length;
            if rewound {
                state.set_marker(0.0);
            }
            run_command(state, ctx, Command::PlayPause);
            let message = if rewound {
                say(
                    format!(
                        "开始播放（落点原本停在 {parked:.2} tick，已过曲尾 {length:.0} tick，已归零）",
                    ),
                    format!(
                        "Playback started (playhead was parked at {parked:.2} ticks, past the song's {length:.0}; reset to 0)",
                    ),
                )
            } else {
                say("开始播放", "Playback started")
            };
            Ok((None, message))
        }
        Playback::Pause => {
            if !state.audio.is_playing() {
                return Ok((
                    None,
                    say(
                        "本来就没在播（落点保持在当前播放位置）",
                        "Not playing (playhead unchanged)",
                    ),
                ));
            }
            run_command(state, ctx, Command::PlayPause);
            Ok((None, say("已暂停，落点保持", "Paused, playhead kept")))
        }
        Playback::Stop => {
            run_command(state, ctx, Command::Stop);
            Ok((
                None,
                say("已停止，落点回到 0", "Stopped, playhead back to 0"),
            ))
        }
        Playback::Seek { tick } => {
            if !tick.is_finite() {
                return Err(bad("tick 不是有限数字", "tick is not finite"));
            }
            if *tick < 0.0 {
                return Err(out_of_range("tick 不能为负", "tick must not be negative"));
            }
            state.set_marker(*tick);
            Ok((
                None,
                say(
                    format!("落点 → {tick:.2} tick"),
                    format!("Playhead → {tick:.2} ticks"),
                ),
            ))
        }
        Playback::SetLoop { enabled } => {
            if state.doc.song.meta.loop_enabled == *enabled {
                return Ok((
                    None,
                    say(
                        "循环开关已经是这个值",
                        "Loop flag already set to that value",
                    ),
                ));
            }
            run_command(state, ctx, Command::ToggleLoop);
            Ok((
                None,
                say(
                    format!("循环{}", if *enabled { "已开" } else { "已关" }),
                    format!("Loop {}", if *enabled { "on" } else { "off" }),
                ),
            ))
        }
    }
}

fn format_extensions(format: &nbs_control::ExportFormat) -> &'static [&'static str] {
    use nbs_control::ExportFormat;
    match format {
        ExportFormat::Wav => &["wav"],
        ExportFormat::Midi => &["mid", "midi"],
        ExportFormat::Png => &["png"],
    }
}

fn format_tag(format: &nbs_control::ExportFormat) -> &'static str {
    use nbs_control::ExportFormat;
    match format {
        ExportFormat::Wav => "wav",
        ExportFormat::Midi => "midi",
        ExportFormat::Png => "png",
    }
}

/// 读一侧的取格子：范围越界直接拒，命中太多则截断并明说。
/// 截断必须显式，否则调用方会把前 2 万格当成整首歌。
fn read_notes(
    state: &EditorState,
    tick_from: Option<u32>,
    tick_to: Option<u32>,
    layer_from: Option<u32>,
    layer_to: Option<u32>,
    ticks_per_bar: Option<u32>,
    bars: Option<[u32; 2]>,
) -> Result<(Data, String), ErrorInfo> {
    let song = &state.doc.song;
    let layers = song.layers.len() as u32;
    // 宽度默认取界面画竖线用的那对数（拍号 × 4），0 是病态值，别拿它除。
    let width = ticks_per_bar
        .unwrap_or_else(|| song.meta.ticks_per_bar())
        .max(1);
    let (tf, tt) = match bars {
        Some([b0, b1]) => {
            if tick_from.is_some() || tick_to.is_some() {
                return Err(bad(
                    "bars 已经定了区间，别再给 tick_from / tick_to",
                    "bars already sets the range; drop tick_from / tick_to",
                ));
            }
            if b1 < b0 {
                return Err(bad(
                    "bars 的终点不能小于起点",
                    "bars[1] must not be below bars[0]",
                ));
            }
            let count = b1 - b0 + 1;
            if count > MAX_BARS_RETURNED as u32 {
                return Err(bad(
                    format!("一次最多 {MAX_BARS_RETURNED} 小节，这里要 {count} 节"),
                    format!("{count} bars requested, the cap is {MAX_BARS_RETURNED}")),
                );
            }
            // 小节号乘宽度不能悄悄绕回一个小数字去读：溢出就是区间错，说清是哪一节。
            let start = b0.checked_mul(width);
            let end = b1.checked_mul(width).and_then(|t| t.checked_add(width - 1));
            match (start, end) {
                (Some(from), Some(to)) => (from, to),
                _ => {
                    return Err(bad(
                        format!("小节号 {b0}..={b1} 乘宽度 {width} 超出了 tick 范围"),
                        format!("bars {b0}..={b1} at {width} ticks each exceeds the tick range"),
                    ))
                }
            }
        }
        None => {
            let tf = tick_from.unwrap_or(0);
            let end = song.song_length().saturating_sub(1);
            // 省略 tick_to = 读到歌尾。起点已经在歌尾之外时读空，而不是报一个自相矛盾的区间错。
            let tt = tick_to.unwrap_or_else(|| tf.max(end));
            if tt < tf {
                return Err(bad(
                    "tick_to 不能小于 tick_from",
                    "tick_to must be >= tick_from",
                ));
            }
            // 只给宽度没给小节号时区间照原样读，但那份文本同样不能长成上万行。
            let count = (tt - tf) / width + 1;
            if ticks_per_bar.is_some() && count > MAX_BARS_RETURNED as u32 {
                return Err(bad(
                    format!("这段有 {count} 小节，一次最多 {MAX_BARS_RETURNED}；用 bars 分段读"),
                    format!("{count} bars in this range, the cap is {MAX_BARS_RETURNED}; chunk it with bars"),
                ));
            }
            (tf, tt)
        }
    };
    let lf = layer_from.unwrap_or(0);
    let lt = layer_to.unwrap_or(layers.saturating_sub(1));
    if lf > lt || lt >= layers {
        return Err(out_of_range(
            format!("图层范围 {lf}..={lt} 超出 0..{}", layers.saturating_sub(1)),
            format!(
                "Layer range {lf}..={lt} outside 0..{}",
                layers.saturating_sub(1)
            ),
        ));
    }
    let raw = song.notes_in_rect(tf, lf, tt, lt);
    let total = raw.len();
    let truncated = total > MAX_CELLS_RETURNED;
    let cells: Vec<NoteCell> = raw
        .iter()
        .take(MAX_CELLS_RETURNED)
        .map(|&(tick, layer, note)| note_cell(tick, layer, note))
        .collect();
    // 文字视图只在请求点名要时给：不要的人不该多一份没人读的文本。
    let bar_views = if bars.is_some() || ticks_per_bar.is_some() {
        bar_overviews(&raw, tf, tt, width, state.flags.flats)
    } else {
        Vec::new()
    };
    let mut message = String::new();
    // 省略的边界要说清算成了什么，否则调用方分不清「这段本来就空」和「我少读了」。
    if bars.is_none() && (tick_from.is_none() || tick_to.is_none()) {
        message.push_str(&say(
            format!("tick 范围按全曲算：{tf}..={tt}"),
            format!("tick range resolved from the song: {tf}..={tt}"),
        ));
    }
    if !bar_views.is_empty() {
        if !message.is_empty() {
            message.push_str(if zh() { "；" } else { "; " });
        }
        // 宽度是从拍号来的还是人给的要说出来：同一份谱按 16 和按 12 切出来的小节不是一回事。
        message.push_str(&say(
            format!(
                "{} 小节，每节 {width} 格（{}）",
                bar_views.len(),
                if ticks_per_bar.is_some() { "指定" } else { "取自拍号" }
            ),
            format!(
                "{} bars, {width} ticks each ({})",
                bar_views.len(),
                if ticks_per_bar.is_some() { "given" } else { "from time signature" }
            ),
        ));
    }
    if truncated {
        if !message.is_empty() {
            message.push_str(if zh() { "；" } else { "; " });
        }
        message.push_str(&say(
            format!("范围里共 {total} 格，只回了前 {MAX_CELLS_RETURNED} 格"),
            format!("Range holds {total} cells, returning the first {MAX_CELLS_RETURNED}"),
        ));
    }
    Ok((
        Data::Notes {
            cells,
            truncated,
            limit: MAX_CELLS_RETURNED,
            bar_overview: bar_views,
        },
        message,
    ))
}

/// 把矩形里的音符按小节切成一行行文字：`0:C4(2) 6:G4(2)`。
///
/// 音名在这里由 `nbs_core::model::key_name` 生成，和状态栏同一份实现：让调用方拿键号自己心算，
/// 错的是半音，而写进去、读回来用的是同一张表，只有耳朵能发现。
fn bar_overviews(
    raw: &[(u32, u32, Note)],
    tick_from: u32,
    tick_to: u32,
    width: u32,
    flats: bool,
) -> Vec<BarOverview> {
    let first = tick_from / width;
    let last = tick_to / width;
    let mut out: Vec<BarOverview> = (first..=last)
        .map(|bar| {
            // 两端落在小节中间时把边界收到实际读到的范围：条目里写的区间必须真含得住它列的格。
            BarOverview {
                bar,
                tick_from: (bar * width).max(tick_from),
                tick_to: (bar * width + width - 1).min(tick_to),
                cells: 0,
                text: String::new(),
            }
        })
        .collect();
    // `notes_in_rect` 是 tick 主序且已经按区间过滤过，所以一格一趟就够。
    for (tick, _layer, note) in raw {
        let bar = &mut out[(tick / width - first) as usize];
        bar.cells += 1;
        if bar.cells > MAX_BAR_CELLS_PRINTED {
            if bar.cells == MAX_BAR_CELLS_PRINTED + 1 {
                bar.text.push('…');
            }
            continue;
        }
        if !bar.text.is_empty() {
            bar.text.push(' ');
        }
        bar.text.push_str(&format!(
            "{tick}:{}({})",
            nbs_core::model::key_name(note.key, true, flats),
            note.instrument
        ));
    }
    out
}

fn note_cell(tick: u32, layer: u32, note: Note) -> NoteCell {
    NoteCell {
        tick,
        layer,
        instrument: note.instrument,
        key: note.key,
        velocity: note.velocity,
        panning: note.panning,
        pitch: note.pitch,
        out_of_range: note.is_out_of_range(),
    }
}

fn layer_infos(state: &EditorState) -> Vec<LayerInfo> {
    let song = &state.doc.song;
    let any_solo = song.any_solo();
    let locked = song.locked_layers();
    let mut counts = vec![0usize; song.layers.len()];
    for (_, layer, _) in song.iter_notes() {
        if let Some(slot) = counts.get_mut(layer as usize) {
            *slot += 1;
        }
    }
    song.layers
        .iter()
        .enumerate()
        .map(|(index, layer)| LayerInfo {
            index: index as u32,
            name: layer.name.clone(),
            volume: layer.volume,
            stereo: layer.stereo,
            lock: match layer.lock {
                nbs_core::model::LockMode::Normal => "normal",
                nbs_core::model::LockMode::Locked => "locked",
                nbs_core::model::LockMode::Solo => "solo",
            }
            .to_string(),
            // 「生效」锁定态和界面一致：有别的层独奏时，普通层其实发不出声。
            effectively_locked: layer.is_locked(any_solo)
                || locked.get(index).copied().unwrap_or(false),
            notes: counts.get(index).copied().unwrap_or(0),
        })
        .collect()
}

fn instrument_infos(state: &EditorState, active: bool) -> Vec<InstrumentInfo> {
    let song = &state.doc.song;
    let bank = state.audio.bank();
    let mut out = Vec::with_capacity(FIRST_CUSTOM_INDEX as usize + song.custom_instruments.len());
    for index in 0..FIRST_CUSTOM_INDEX {
        out.push(InstrumentInfo {
            index,
            name: song.instrument_name(index).to_string(),
            custom: false,
            filename: None,
            sample_ready: Some(sample_ready(&bank, index as usize)),
        });
    }
    for (offset, custom) in song.custom_instruments.iter().enumerate() {
        let index = FIRST_CUSTOM_INDEX + offset as u8;
        out.push(InstrumentInfo {
            index,
            name: custom.name.clone(),
            custom: true,
            filename: Some(custom.filename.clone()),
            sample_ready: active.then(|| sample_ready(&bank, index as usize)),
        });
    }
    out
}

/// 采样到底在不在内存里。自定义音色缺文件时 `extend_for_song` 塞进来的是空采样，
/// 所以「有槽位」不等于「能发声」——这一列就是为了让调用方看得见静音。
fn sample_ready(bank: &nbs_audio::SoundBank, index: usize) -> bool {
    bank.get(index).is_some_and(|sample| !sample.is_empty())
}

fn selection_info(selection: &nbs_core::selection::Selection) -> SelectionInfo {
    let empty = selection.is_empty();
    SelectionInfo {
        empty,
        notes: selection.len(),
        x: (!empty).then_some(selection.x),
        y: (!empty).then_some(selection.y),
        width: (!empty).then_some(selection.width),
        height: (!empty).then_some(selection.height),
    }
}

fn history_info(state: &EditorState) -> HistoryInfo {
    let history = &state.doc.history;
    HistoryInfo {
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label().map(str::to_string),
        redo_label: history.redo_label().map(str::to_string),
        cursor: history.cursor(),
        len: history.len(),
        capacity: history.capacity(),
    }
}

/// 自己走一遍音符，不复用 `refresh_compat` 的缓存：那条缓存有空选区时不刷新，
/// 而且这里还要按音色统计缺了哪些采样。
fn diagnostics(state: &EditorState) -> Diagnostics {
    use nbs_core::model::{MC_KEY_MAX, MC_KEY_MIN};
    let song = &state.doc.song;
    let bank = state.audio.bank();
    let (mut outside, mut custom, mut pitched) = (0usize, 0usize, 0usize);
    let mut used = vec![false; usize::from(u8::MAX) + 1];
    for (_, _, note) in song.iter_notes() {
        if note.key < MC_KEY_MIN || note.key > MC_KEY_MAX {
            outside += 1;
        }
        if note.instrument >= FIRST_CUSTOM_INDEX {
            custom += 1;
        }
        pitched += usize::from(note.pitch != 0);
        used[note.instrument as usize] = true;
    }
    let missing_samples: Vec<u8> = (0..used.len())
        .filter(|index| used[*index] && !sample_ready(&bank, *index))
        .map(|index| index as u8)
        .collect();
    let compat = if outside > 0 || custom > 0 {
        0
    } else if crate::state::tempo_schematic_ok(song.meta.tempo) && pitched == 0 {
        1
    } else {
        2
    };
    Diagnostics {
        compat,
        notes_out_of_mc_range: outside,
        notes_custom_instrument: custom,
        pitched_notes: pitched,
        tempo: song.meta.tempo,
        bpm: song.meta.bpm(),
        missing_samples,
    }
}

fn snapshot(
    state: &EditorState,
    ch: &ControlChannel,
    active: bool,
    original_active: Option<crate::tabs::SongId>,
) -> Snapshot {
    let song = &state.doc.song;
    Snapshot {
        app_version: ch.app_version.clone(),
        protocol: ch.protocol,
        path: song.filename.as_ref().map(|p| p.display().to_string()),
        dirty: song.dirty,
        rev: song.rev,
        song_session: state.store.active_session(),
        name: song.meta.name.clone(),
        author: song.meta.author.clone(),
        description: song.meta.description.clone(),
        tempo: song.meta.tempo,
        bpm: song.meta.bpm(),
        time_signature: song.meta.time_signature,
        loop_enabled: song.meta.loop_enabled,
        loop_start: song.meta.loop_start,
        loop_max: song.meta.loop_max,
        layers: song.layers.len(),
        notes: song.total_blocks,
        length_ticks: song.song_length(),
        save_version: song.meta.save_version,
        first_custom_index: FIRST_CUSTOM_INDEX,
        active_tab: original_active.map(|id| state.store.session(id)).flatten(),
        tabs: tab_infos(state, original_active, !active),
        playhead: active.then(|| state.audio.position_ticks()),
        playing: active.then(|| state.audio.is_playing()),
        selection: Box::new(selection_info(&state.doc.selection)),
        diagnostics: Box::new(diagnostics(state)),
        modal_count: active.then(|| state.modals.items.len()),
        open_window: state.window.as_ref().map(|kind| format!("{kind:?}")),
    }
}

// ------------------------------------------------------------------ 逃生舱

/// 一条动作的自述。`build` 返回 `None` 表示当前已经是这个值，什么都不用做。
struct ActionSpec {
    description: &'static str,
    /// `args` 期望的形状，直接写成一段 JSON 文本给调用方看例子。
    args: Option<&'static str>,
    mutating: bool,
    danger: bool,
    build: fn(&EditorState, Option<&Value>) -> Result<Option<Command>, ErrorInfo>,
}

const ACTIONS: &[(&str, ActionSpec)] = &[
    (
        "play",
        ActionSpec {
            description: "从当前落点起播（停在歌尾时先回 0，与界面播放按钮同一条路）",
            args: None,
            mutating: false,
            danger: false,
            build: |state, _| yes_if(!state.audio.is_playing(), Command::PlayPause),
        },
    ),
    (
        "pause",
        ActionSpec {
            description: "暂停，落点保持（对应播放中的 PlayPause）",
            args: None,
            mutating: false,
            danger: false,
            build: |state, _| yes_if(state.audio.is_playing(), Command::PlayPause),
        },
    ),
    (
        "stop",
        ActionSpec {
            description: "停止并把落点归零",
            args: None,
            mutating: false,
            danger: false,
            build: |_, _| Ok(Some(Command::Stop)),
        },
    ),
    (
        "seek_home",
        ActionSpec {
            description: "落点回到 0",
            args: None,
            mutating: false,
            danger: false,
            build: |_, _| Ok(Some(Command::SeekHome)),
        },
    ),
    (
        "seek_end",
        ActionSpec {
            description: "落点跳到歌曲末尾",
            args: None,
            mutating: false,
            danger: false,
            build: |_, _| Ok(Some(Command::SeekEnd)),
        },
    ),
    (
        "toggle_follow",
        ActionSpec {
            description: "切换「跟随播放头」",
            args: None,
            mutating: false,
            danger: false,
            build: |_, _| Ok(Some(Command::ToggleFollow)),
        },
    ),
    (
        "toggle_record",
        ActionSpec {
            description: "切换实时 MIDI 录音武装（需要界面已选中图层，且会开始往工程里落子）",
            args: None,
            mutating: true,
            danger: false,
            build: |_, _| Ok(Some(Command::ToggleRecord)),
        },
    ),
    (
        "window_scale_up",
        ActionSpec { description: "界面放大一档", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::WindowScaleUp)) },
    ),
    (
        "window_scale_down",
        ActionSpec { description: "界面缩小一档", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::WindowScaleDown)) },
    ),
    (
        "window_scale_reset",
        ActionSpec { description: "界面缩放回到 1", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::WindowScaleReset)) },
    ),
    (
        "piano_scale_up",
        ActionSpec { description: "钢琴栏放大一档", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::PianoScaleUp)) },
    ),
    (
        "piano_scale_down",
        ActionSpec { description: "钢琴栏缩小一档", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::PianoScaleDown)) },
    ),
    (
        "set_flag",
        ActionSpec {
            description: "设界面开关的显式值（显示图层、钢琴、网格、力度数字……），已经是这个值就不动",
            args: Some(r#"{"name":"show_layers","on":true}"#),
            mutating: false,
            danger: false,
            build: |state, args| {
                let flag = flag_by_name(arg_str(args, "name")?)?;
                let want = arg_bool(args, "on")?;
                yes_if(flag.get(&state.flags) != want, Command::ToggleFlag(flag))
            },
        },
    ),
    (
        "select_layer",
        ActionSpec {
            description: "按序号选图层（与点图层名同一条命令：已在选且只有它时取消）",
            args: Some(r#"{"index":0}"#),
            mutating: false,
            danger: false,
            build: |_state, args| Ok(Some(Command::SelectLayer(arg_u32(args, "index")?))),
        },
    ),
    (
        "set_instrument",
        ActionSpec {
            description: "设画笔音色（不影响已写的音符，除非再批量改）",
            args: Some(r#"{"index":5}"#),
            mutating: false,
            danger: false,
            build: |state, args| {
                let index = arg_u8(args, "index")?;
                check_instrument(state, index)?;
                yes_if(state.current.instrument != index, Command::SetInstrument(index))
            },
        },
    ),
    (
        "set_key",
        ActionSpec {
            description: "设画笔音高 0..=87",
            args: Some(r#"{"key":45}"#),
            mutating: false,
            danger: false,
            build: |state, args| {
                let key = arg_u8(args, "key")?;
                if key > KEY_MAX {
                    return Err(out_of_range(format!("key {key} 超出 0..={KEY_MAX}"), format!("key {key} outside 0..={KEY_MAX}")));
                }
                yes_if(state.current.key != key, Command::SetKey(key as i32))
            },
        },
    ),
    (
        "set_velocity",
        ActionSpec {
            description: "设画笔力度 0..=100",
            args: Some(r#"{"velocity":100}"#),
            mutating: false,
            danger: false,
            build: |state, args| {
                let v = arg_i32(args, "velocity")?;
                range_arg(v as i64, 0, VEL_MAX as i64, "velocity")?;
                yes_if(state.current.velocity as i32 != v, Command::SetVel(v))
            },
        },
    ),
    (
        "set_panning",
        ActionSpec {
            description: "设画笔声像 0..=200（100 = 中央）",
            args: Some(r#"{"panning":100}"#),
            mutating: false,
            danger: false,
            build: |state, args| {
                let v = arg_i32(args, "panning")?;
                range_arg(v as i64, 0, PAN_MAX as i64, "panning")?;
                yes_if(state.current.panning as i32 != v, Command::SetPan(v))
            },
        },
    ),
    (
        "set_pitch",
        ActionSpec {
            description: "设画笔微调 ±1200 音分",
            args: Some(r#"{"pitch":0}"#),
            mutating: false,
            danger: false,
            build: |state, args| {
                let v = arg_i32(args, "pitch")?;
                range_arg(v as i64, -PIT_MAX as i64, PIT_MAX as i64, "pitch")?;
                yes_if(state.current.pitch as i32 != v, Command::SetPit(v))
            },
        },
    ),
    (
        "set_edit_mode",
        ActionSpec {
            description: "切编辑模式：key / velocity / panning / pitch",
            args: Some(r#"{"mode":"velocity"}"#),
            mutating: false,
            danger: false,
            build: |_state, args| Ok(Some(Command::SetEditMode(edit_mode_by_name(arg_str(args, "mode")?)?))),
        },
    ),
    (
        "select_all",
        ActionSpec {
            description: "全选（原版语义：音符被摘进浮动选区，撤销能退回来）",
            args: None,
            mutating: true,
            danger: false,
            build: |_, _| Ok(Some(Command::SelectAll)),
        },
    ),
    (
        "copy_selection",
        ActionSpec {
            description: "复制浮动选区到剪贴板（同时写系统剪贴板的文本形式）",
            args: None,
            mutating: false,
            danger: false,
            build: |state, _| yes_if(!state.doc.selection.is_empty(), Command::Copy),
        },
    ),
    (
        "cut_selection",
        ActionSpec {
            description: "剪切浮动选区",
            args: None,
            mutating: true,
            danger: false,
            build: |state, _| yes_if(!state.doc.selection.is_empty(), Command::Cut),
        },
    ),
    (
        "delete_selection",
        ActionSpec {
            description: "删除浮动选区里的音符",
            args: None,
            mutating: true,
            danger: false,
            build: |state, _| yes_if(!state.doc.selection.is_empty(), Command::Delete),
        },
    ),
    (
        "transpose_selection",
        ActionSpec {
            description: "对浮动选区移调（半音，可为负）",
            args: Some(r#"{"semitones":12}"#),
            mutating: true,
            danger: false,
            build: |state, args| {
                let semi = arg_i32(args, "semitones")?;
                yes_if(!state.doc.selection.is_empty() && semi != 0, Command::Transpose(semi))
            },
        },
    ),
    (
        "transpose_to_minecraft",
        ActionSpec {
            description: "把整首歌挪进 Minecraft 能播的两个八度（原版「转到 MC 音域」的确定分支，不问直接转）；它会留一个「已编辑 N 个音符」的提示框，在答掉之前所有写操作都会被弹窗闸挡回来",
            args: None,
            mutating: true,
            danger: false,
            build: |_, _| Ok(Some(Command::TransposeToMinecraftApply)),
        },
    ),
    (
        "fix_tempo_schematic",
        ActionSpec {
            description: "把速度改成原理图能接受的那档",
            args: None,
            mutating: true,
            danger: false,
            build: |_, _| Ok(Some(Command::FixTempoSchematic)),
        },
    ),
    (
        "fix_tempo_datapack",
        ActionSpec {
            description: "把速度改成数据包能接受的那档",
            args: None,
            mutating: true,
            danger: false,
            build: |_, _| Ok(Some(Command::FixTempoDatapack)),
        },
    ),
    (
        "add_layer",
        ActionSpec {
            description: "在末尾追加图层",
            args: None,
            mutating: true,
            danger: false,
            build: |_, _| Ok(Some(Command::AddLayer)),
        },
    ),
    (
        "show_window",
        ActionSpec {
            description: "打开一扇界面窗口（只列不依赖鼠标上下文的那些）",
            args: Some(r#"{"kind":"instruments"}"#),
            mutating: false,
            danger: false,
            build: |_state, args| Ok(Some(Command::ShowWindow(window_by_name(arg_str(args, "kind")?)?))),
        },
    ),
    (
        "close_window",
        ActionSpec {
            description: "关掉当前窗口（也用来解开 window_open 那种挡路的草稿态）",
            args: None,
            mutating: false,
            danger: false,
            build: |state, _| yes_if(state.window.is_some(), Command::CloseWindow),
        },
    ),
    (
        "toggle_metronome",
        ActionSpec { description: "切换节拍器", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::ToggleMetronome)) },
    ),
    (
        "toggle_mono",
        ActionSpec { description: "切换单声道输出", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::ToggleMono)) },
    ),
    (
        "toggle_extended_voices",
        ActionSpec { description: "切换扩展同时发声数", args: None, mutating: false, danger: false, build: |_, _| Ok(Some(Command::ToggleExtendedVoices)) },
    ),
    (
        "set_master_volume",
        ActionSpec {
            description: "设总音量 0..=100",
            args: Some(r#"{"volume":80}"#),
            mutating: false,
            danger: false,
            build: |_state, args| {
                let v = arg_i32(args, "volume")?;
                range_arg(v as i64, 0, 100, "volume")?;
                Ok(Some(Command::SetMasterVolume(v as f32)))
            },
        },
    ),
    (
        "set_play_speed",
        ActionSpec {
            description: "设播放速度倍率（原版的 play_speed）",
            args: Some(r#"{"speed":1.0}"#),
            mutating: false,
            danger: false,
            build: |_state, args| Ok(Some(Command::SetPlaySpeed(arg_f32(args, "speed")?))),
        },
    ),
    (
        "set_output",
        ActionSpec {
            description: "切输出设备；device 传 null 或省略 = 跟随系统默认",
            args: Some(r#"{"device":"wasapi:{0.0.0.00000000}.{GUID}"}"#),
            mutating: false,
            danger: false,
            build: |_state, args| {
                Ok(Some(Command::SetOutput(match args {
                    Some(Value::Null) | None => None,
                    Some(value) => match value.get("device") {
                        Some(Value::String(name)) => Some(name.clone()),
                        Some(Value::Null) | None => None,
                        Some(_) => return Err(bad("device 要是字符串或 null", "device must be a string or null")),
                    },
                })))
            },
        },
    ),
    (
        "set_sounds_dir",
        ActionSpec {
            description: "设采样目录（只改内存里的首选项，落盘要再调 save_preferences）",
            args: Some(r#"{"path":"C:\\Users\\me\\.minecraft\\assets"}"#),
            mutating: false,
            danger: false,
            build: |_state, args| Ok(Some(Command::SetSoundsDir(arg_str(args, "path")?.to_string()))),
        },
    ),
    (
        "reload_samples",
        ActionSpec {
            description: "重新发现并加载采样目录",
            args: None,
            mutating: false,
            danger: false,
            build: |_, _| Ok(Some(Command::ReloadSamples)),
        },
    ),
    (
        "save_preferences",
        ActionSpec {
            description: "把首选项写到 settings.ini",
            args: None,
            mutating: false,
            danger: false,
            build: |_, _| Ok(Some(Command::SavePreferences)),
        },
    ),
    (
        "answer_modal",
        ActionSpec {
            description: "回答界面当前那个确认框：answer=yes/no/cancel；兼容旧的 yes 布尔。等于替人按按钮，默认关掉",
            args: Some(r#"{"answer":"yes"}"#),
            mutating: false,
            danger: true,
            build: |_state, args| {
                if let Some(answer) = args.as_ref().and_then(|a| a.get("answer")) {
                    return match answer.as_str() {
                        Some("yes") => Ok(Some(Command::AnswerModal(true))),
                        Some("no") => Ok(Some(Command::AnswerModal(false))),
                        Some("cancel") => Ok(Some(Command::CancelModal)),
                        _ => Err(bad(
                            "answer 只认 yes / no / cancel",
                            "answer must be yes, no, or cancel",
                        )),
                    };
                }
                if args
                    .as_ref()
                    .and_then(|a| a.get("cancel"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    return Ok(Some(Command::CancelModal));
                }
                Ok(Some(Command::AnswerModal(arg_bool(args, "yes")?)))
            },
        },
    ),
    (
        "open_url",
        ActionSpec {
            description: "用系统浏览器打开一个链接。等于让程序替人点外链，默认关掉",
            args: Some(r#"{"url":"https://example.com"}"#),
            mutating: false,
            danger: true,
            build: |_state, args| Ok(Some(Command::OpenUrl(arg_str(args, "url")?.to_string()))),
        },
    ),
    (
        "clear_recent",
        ActionSpec {
            description: "清空「最近打开」列表（会连着写首选项）。默认关掉",
            args: None,
            mutating: false,
            danger: true,
            build: |_, _| Ok(Some(Command::ClearRecent)),
        },
    ),
    (
        "exit",
        ActionSpec {
            description: "退出程序。界面退出会逐标签询问；MCP 当前工程有未保存改动时仍先拒，确认要丢就带 discard_changes=true 立即退出。默认关掉",
            args: Some(r#"{"discard_changes":true}"#),
            mutating: false,
            danger: true,
            build: |state, args| {
                let discard = arg_bool_or(args, "discard_changes")?;
                keep_or_replace(state, discard, ("退出", "Quitting"))?;
                Ok(Some(if discard {
                    Command::ExitDiscard
                } else {
                    Command::Exit
                }))
            },
        },
    ),
];

fn find_action(name: &str) -> Option<&'static ActionSpec> {
    ACTIONS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, spec)| spec)
}

fn list_actions() -> Vec<ActionInfo> {
    ACTIONS
        .iter()
        .map(|(name, spec)| ActionInfo {
            name: (*name).to_string(),
            description: spec.description.to_string(),
            args: spec.args.map(|raw| {
                serde_json::from_str::<Value>(raw)
                    .unwrap_or_else(|_| Value::String(raw.to_string()))
            }),
            mutating: spec.mutating,
        })
        .collect()
}

fn run_action(
    state: &mut EditorState,
    ctx: &Context,
    ch: &ControlChannel,
    name: &str,
    args: Option<&Value>,
) -> Written {
    let Some(spec) = find_action(name) else {
        return Err(bad(
            format!("未知动作 {name:?}。用 list_actions 看可用动作。"),
            format!("Unknown action {name:?}. See list_actions."),
        ));
    };
    if spec.danger && !policy_allow(ch, |policy| policy.allow_danger) {
        return Err(ErrorInfo::new(
            ErrorCode::NotAllowed,
            say(
                format!(
                    "动作 {name:?} 属于「等于替人按按钮」那一类。要么启动时加 --control-danger，\
                     要么在设置页「外部控制（MCP）」里单独勾上危险动作——环境变量给不了这条。"
                ),
                format!(
                    "Action {name:?} is in the 'pushes a human's button' class; allow it with \
                     --control-danger at launch or by ticking dangerous actions on its own in \
                     Settings - External control (MCP). An environment variable never grants this."
                ),
            ),
        ));
    }
    match (spec.build)(state, args) {
        Ok(None) => Ok((
            Some(Data::Action {
                action: name.to_string(),
                note: String::new(),
            }),
            say("无需改动：已经是这个值", "No change: already at that value"),
        )),
        Ok(Some(cmd)) => {
            // 状态栏那句话只有在这条动作真的换了文案时才算它的回执；
            // 否则会把上一次调用的留言当成这次的说明。
            let before = status_text(state);
            run_command(state, ctx, cmd);
            let note = status_text(state);
            Ok((
                Some(Data::Action {
                    action: name.to_string(),
                    note: if note == before { String::new() } else { note },
                }),
                say(format!("已执行动作 {name}"), format!("Action {name} run")),
            ))
        }
        Err(error) => Err(error),
    }
}

fn yes_if(cond: bool, cmd: Command) -> Result<Option<Command>, ErrorInfo> {
    Ok(cond.then_some(cmd))
}

fn flag_by_name(name: &str) -> Result<Flag, ErrorInfo> {
    let flag = match name {
        "show_layers" => Flag::ShowLayers,
        "show_piano" => Flag::ShowPiano,
        "use_colors" => Flag::UseColors,
        "use_shapes" => Flag::UseShapes,
        "use_icons" => Flag::UseIcons,
        "show_numbers" => Flag::ShowNumbers,
        "show_octaves" => Flag::ShowOctaves,
        "show_key_names" => Flag::ShowKeyNames,
        "show_key_numbers" => Flag::ShowKeyNumbers,
        "show_keyboard" => Flag::ShowKeyboard,
        "show_out_of_range" => Flag::ShowOutOfRange,
        "show_incompatible" => Flag::ShowIncompatible,
        "fade" => Flag::Fade,
        "show_grid_lines" => Flag::ShowGridLines,
        "flats" => Flag::Flats,
        "blackout" => Flag::Blackout,
        "remove_effect" => Flag::RemoveEffect,
        "show_notechart" => Flag::ShowNoteChart,
        other => {
            return Err(bad(
                format!(
                    "开关名 {other:?} 不认识。可用：{}。",
                    "show_layers, show_piano, use_colors, use_shapes, use_icons, show_numbers, show_octaves, show_key_names, show_key_numbers, show_keyboard, show_out_of_range, show_incompatible, fade, show_grid_lines, flats, blackout, remove_effect, show_notechart"
                ),
                format!("Unknown flag {other:?}."),
            ))
        }
    };
    Ok(flag)
}

fn edit_mode_by_name(name: &str) -> Result<crate::state::EditMode, ErrorInfo> {
    let mode = match name {
        "key" => crate::state::EditMode::Key,
        "velocity" => crate::state::EditMode::Velocity,
        "panning" => crate::state::EditMode::Panning,
        "pitch" => crate::state::EditMode::Pitch,
        other => {
            return Err(bad(
                format!("编辑模式 {other:?} 不认识，可用 key / velocity / panning / pitch"),
                format!("Unknown edit mode {other:?}; use key / velocity / panning / pitch"),
            ))
        }
    };
    Ok(mode)
}

fn window_by_name(name: &str) -> Result<crate::state::WindowKind, ErrorInfo> {
    use crate::state::WindowKind;
    let kind = match name {
        "song_info" => WindowKind::SongInfo,
        "song_info_edit" => WindowKind::SongInfoEdit,
        "properties" => WindowKind::Properties,
        "statistics" => WindowKind::Statistics,
        "preferences" => WindowKind::Preferences,
        // 让人看一眼「AI 现在被允许做什么」的那个面板，也开给调用方：
        // 它只能显示，改勾选仍然是人的手（面板上的开关要 UI 线程点，外部没有等价 Op）。
        "external_control" => WindowKind::ExternalControl,
        "instruments" => WindowKind::Instruments,
        "about" => WindowKind::About,
        "tempo_tapper" => WindowKind::TempoTapper,
        "save_options" => WindowKind::SaveOptions,
        "shortcuts" => WindowKind::Shortcuts,
        "clip_editor" => WindowKind::ClipEditor,
        "audio_settings" => WindowKind::AudioSettings,
        "midi_devices" => WindowKind::MidiDevices,
        "autosaves" => WindowKind::Autosaves,
        "minecraft_compat" => WindowKind::MinecraftCompat,
        "audio_export" => WindowKind::AudioExport,
        "datapack_export" => WindowKind::DatapackExport,
        "png_export" => WindowKind::PngExport,
        "schematic_export" => WindowKind::SchematicExport,
        other => {
            return Err(bad(
                format!("窗口名 {other:?} 不认识或它需要鼠标上下文。"),
                format!("Unknown window {other:?}, or it needs mouse context."),
            ))
        }
    };
    Ok(kind)
}

// ------------------------------------------------------------------ 参数取值

fn need<'a>(args: Option<&'a Value>, key: &str) -> Result<&'a Value, ErrorInfo> {
    args.and_then(|value| value.get(key)).ok_or_else(|| {
        bad(
            format!("动作缺少参数 {key:?}"),
            format!("Action is missing argument {key:?}"),
        )
    })
}

fn arg_str<'a>(args: Option<&'a Value>, key: &str) -> Result<&'a str, ErrorInfo> {
    need(args, key)?.as_str().ok_or_else(|| {
        bad(
            format!("参数 {key:?} 要是字符串"),
            format!("Argument {key:?} must be a string"),
        )
    })
}

fn arg_bool(args: Option<&Value>, key: &str) -> Result<bool, ErrorInfo> {
    need(args, key)?.as_bool().ok_or_else(|| {
        bad(
            format!("参数 {key:?} 要是 true / false"),
            format!("Argument {key:?} must be true / false"),
        )
    })
}

/// 省略即 false 的那一类开关，和 `Op` 里同名字段的 `#[serde(default)]` 保持一个口径：
/// `discard_changes` 在两条路上都必须是「不带就等于不认错」。给了值却不是布尔要报错——
/// 这是一道破坏性闸门，把 `"yes"` 当成 false 悄悄放行比拒一次更难查。
fn arg_bool_or(args: Option<&Value>, key: &str) -> Result<bool, ErrorInfo> {
    match args.and_then(|value| value.get(key)) {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| {
            bad(
                format!("参数 {key:?} 要是 true / false"),
                format!("Argument {key:?} must be true / false"),
            )
        }),
    }
}

fn arg_i64(args: Option<&Value>, key: &str) -> Result<i64, ErrorInfo> {
    need(args, key)?.as_i64().ok_or_else(|| {
        bad(
            format!("参数 {key:?} 要是整数"),
            format!("Argument {key:?} must be an integer"),
        )
    })
}

fn range_arg(value: i64, low: i64, high: i64, key: &str) -> Result<(), ErrorInfo> {
    if (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(out_of_range(
            format!("参数 {key:?} = {value} 超出 {low}..={high}"),
            format!("Argument {key:?} = {value} outside {low}..={high}"),
        ))
    }
}

fn arg_u8(args: Option<&Value>, key: &str) -> Result<u8, ErrorInfo> {
    let value = arg_i64(args, key)?;
    range_arg(value, 0, u8::MAX as i64, key)?;
    Ok(value as u8)
}

fn arg_u32(args: Option<&Value>, key: &str) -> Result<u32, ErrorInfo> {
    let value = arg_i64(args, key)?;
    range_arg(value, 0, u32::MAX as i64, key)?;
    Ok(value as u32)
}

fn arg_i32(args: Option<&Value>, key: &str) -> Result<i32, ErrorInfo> {
    let value = arg_i64(args, key)?;
    range_arg(value, i32::MIN as i64, i32::MAX as i64, key)?;
    Ok(value as i32)
}

fn arg_f32(args: Option<&Value>, key: &str) -> Result<f32, ErrorInfo> {
    let value = need(args, key)?.as_f64().ok_or_else(|| {
        bad(
            format!("参数 {key:?} 要是数字"),
            format!("Argument {key:?} must be a number"),
        )
    })?;
    if !value.is_finite() {
        return Err(bad(
            format!("参数 {key:?} 不是有限数字"),
            format!("Argument {key:?} is not finite"),
        ));
    }
    Ok(value as f32)
}

/// 任务表起不动的两种原因，说法分开：满不满是调用方能解决的（等、取消），线程起不来不是。
fn spawn_reject(reject: SpawnReject) -> ErrorInfo {
    match reject {
        SpawnReject::Busy => ErrorInfo::new(
            ErrorCode::Busy,
            say(
                "并发任务已满，先用 job_list 看看有什么在跑，或 job_cancel 掉一个。",
                "All job slots are busy; check job_list or cancel one first.",
            ),
        ),
        SpawnReject::Thread(message) => ErrorInfo::new(ErrorCode::Internal, message),
    }
}

/// 刚登记、线程还没排上时 `status()` 通常已经查得到；查不到就照实回一条 queued，
/// 不编一个进度出来。
fn job_snapshot(ch: &ControlChannel, id: u64, kind: &str) -> JobInfo {
    ch.jobs.status(id).unwrap_or_else(|| JobInfo {
        id,
        kind: kind.into(),
        status: JobStatus::Queued,
        progress: 0.0,
        message: String::new(),
        started_unix: 0,
        finished_unix: None,
        result: None,
    })
}

/// 「目标已存在」这道闸：`export` 和转录的 MIDI 交付共用同一条，所以字面实现只有一份。
/// 认错的方式就是显式带 `overwrite=true`——不带就当调用方没打算覆盖别人的文件。
fn overwrite_gate(target: &Path, overwrite: bool) -> Result<(), ErrorInfo> {
    if target.exists() && !overwrite {
        return Err(ErrorInfo::new(
            ErrorCode::NotAllowed,
            say(
                format!("目标已存在：{}。确认覆盖就带 overwrite=true。", target.display()),
                format!(
                    "Target exists: {}. Pass overwrite=true to replace it.",
                    target.display()
                ),
            ),
        ));
    }
    Ok(())
}

/// 导出走任务表：拍一份歌曲快照，剩下的全在别的线程做，回一个句柄。
/// 主线程既不等混音也不为它挂住一条界面请求。
fn export_job(
    state: &EditorState,
    ch: &ControlChannel,
    path: &str,
    format: &nbs_control::ExportFormat,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    include_locked: Option<bool>,
    overwrite: bool,
) -> Written {
    let target = resolve_path(state, ch, path, format_extensions(format), false)?;
    overwrite_gate(&target, overwrite)?;
    let sample_rate = sample_rate.unwrap_or(state.prefs.export_sample_rate);
    if !(8_000..=192_000).contains(&sample_rate) {
        return Err(out_of_range(
            format!("采样率 {sample_rate} 超出 8000..=192000"),
            format!("Sample rate {sample_rate} outside 8000..=192000"),
        ));
    }
    let channels = channels.unwrap_or(state.prefs.export_channels);
    if !(1..=2).contains(&channels) {
        return Err(out_of_range(
            format!("声道数 {channels} 只支持 1 或 2"),
            format!("Channels {channels}: only 1 or 2 supported"),
        ));
    }
    let include_locked = include_locked.unwrap_or(state.prefs.include_locked_in_export);
    let png_options = state.png_options.clone();
    let sounds_dir = state.prefs.sounds_dir.clone();
    let song = state.doc.song.clone();
    let notes = song.total_blocks;
    let out = target.clone();
    let fmt = *format;
    let id = ch
        .jobs
        .spawn("export", move |job: &JobHandle| {
            job.progress(0.05, "准备声源");
            // 界面那份 bank 只有当前工程用得到的自定义采样；离线路径按 CLI 的做法
            // 重新发现一遍目录，这样导出的音频和界面听到的不会差一批乐器。
            let mut bank = nbs_audio::SoundBank::load_auto(sounds_dir.as_deref().map(Path::new));
            bank.extend_for_song(&song);
            if job.cancelled() {
                return Err(say("已取消", "cancelled"));
            }
            match fmt {
                nbs_control::ExportFormat::Wav => {
                    job.progress(0.2, "离线混音");
                    let opts = nbs_audio::ExportOptions {
                        sample_rate,
                        channels,
                        include_locked,
                    };
                    let stats = nbs_audio::export_wav(&song, &bank, &out, &opts)?;
                    job.progress(1.0, "完成");
                    Ok(json!({
                        "path": out.display().to_string(),
                        "notes": stats.notes,
                        "seconds": stats.duration_seconds,
                        "peak": stats.peak,
                        "threads": stats.threads,
                        "bytes": std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
                    }))
                }
                nbs_control::ExportFormat::Midi => {
                    job.progress(0.4, "写 SMF");
                    nbs_io::midi::export_midi(&song, &out)?;
                    Ok(json!({
                        "path": out.display().to_string(),
                        "bytes": std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
                    }))
                }
                nbs_control::ExportFormat::Png => {
                    job.progress(0.4, "画谱面");
                    let (width, height) =
                        nbs_io::image_export::export_png(&song, &out, &png_options)?;
                    Ok(json!({
                        "path": out.display().to_string(),
                        "width": width,
                        "height": height,
                    }))
                }
            }
        })
        .map_err(spawn_reject)?;
    let info = job_snapshot(ch, id, JobKind::Export.as_str());
    Ok((
        Some(Data::Job(Box::new(info))),
        say(
            format!(
                "已起 {fmt} 导出任务（快照 {notes} 个音符）→ {}；用 job_status 轮询",
                target.display(),
                fmt = format_tag(format)
            ),
            format!(
                "Export job started ({notes} notes snapshotted) → {}; poll with job_status",
                target.display()
            ),
        ),
    ))
}

// ------------------------------------------------------------------ 音频转 MIDI
//
// 转录窗上那四个按钮各有一条对应 op。形状一律「立即回任务句柄，之后 job_status 轮询」：
// python 子进程动辄几十秒到几十分钟，界面线程既不该为它等，也不该有一条请求挂在那里
// 占住收件箱。事件、进度、取消都按窗上那份语义走，只是把落点从窗口换成任务表。
//
// 闸的顺序也照窗上，一条都不放松：许可（只有人自己在窗上勾得动——这里没有代填的入口，
// 也没有为 AI 新开的总开关）→ 环境装没装 → 一次只跑一个 → 路径与覆盖。
// `transcribe_probe` 是窗上唯一不需要许可的那条，这里也照样不要：它只是问一句现状。

/// worker 回给窗口的事件里「这轮活干完了」的三种形状。`Failed` / `Cancelled` 不进这里，
/// 它们直接把任务标失败、消息原样带出去。
#[derive(Debug)]
enum TranscribeTerminal {
    Ready(nbs_io::transcription::RuntimeInfo),
    ModelReady {
        model: String,
        device: String,
        dtype: String,
    },
    Complete(nbs_io::transcription::TranscriptionOutput),
}

/// 音频扩展名，和界面「选择音频」那格放的过滤器同一张表。
const AUDIO_EXTS: &[&str] = &[
    "wav", "flac", "mp3", "ogg", "m4a", "aac", "aif", "aiff", "opus", "webm",
];

/// 转录产物另存前要先读一遍（核对音符数），这是它的上限：和窗上预览那条一样，
/// 宁可报错也不吃进内存。
const MAX_MIDI_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

/// 取值域检查，错在起子进程**之前**。worker 那侧的 ValueError 到不了调用方手上，
/// 只会变成二十秒后一句「任务失败」。
fn one_of(field: &str, value: &str, allowed: &[&str]) -> Result<(), ErrorInfo> {
    if allowed.iter().any(|want| *want == value) {
        return Ok(());
    }
    Err(bad(
        format!("{field} {value:?} 不认识，这里只接受 {}", allowed.join(" / ")),
        format!(
            "{field} {value:?} is not one of {}",
            allowed.join(" / ")
        ),
    ))
}

fn number_in(field: &str, value: f64, lower: f64, upper: f64) -> Result<(), ErrorInfo> {
    if !value.is_finite() || !(lower..=upper).contains(&value) {
        return Err(out_of_range(
            format!("{field} 要是 {lower}..={upper} 之间的有限数字"),
            format!("{field} must be a finite number in {lower}..={upper}"),
        ));
    }
    Ok(())
}

/// 窗上当前那一份参数，加上调用方点名要改的字段。省略的字段**不动**窗上那份，
/// 所以「AI 只改 device」和「人只改 device」算出来的是同一件事。
///
/// `model_path` 给空串是显式清空（回到按模型名取缓存那份）；省略才是「用窗上那一份」。
fn transcribe_options(
    state: &EditorState,
    given: &TranscribeOptions,
) -> Result<nbs_io::transcription::TranscriptionOptions, ErrorInfo> {
    let mut out = state.transcription.request_options();
    if let Some(value) = given.model.as_deref() {
        one_of("model", value, &["small", "medium", "large"])?;
        out.model = value.to_string();
    }
    if let Some(value) = given.device.as_deref() {
        one_of("device", value, &["auto", "cpu", "cuda"])?;
        out.device = value.to_string();
    }
    if let Some(value) = given.dtype.as_deref() {
        one_of("dtype", value, &["auto", "float32", "float16"])?;
        out.dtype = value.to_string();
    }
    if let Some(value) = given.beam_size {
        if !(1..=4).contains(&value) {
            return Err(out_of_range(
                "beam_size 是 1..=4 的整数",
                "beam_size must be an integer in 1..=4",
            ));
        }
        out.beam_size = value;
    }
    if let Some(value) = given.temperature {
        number_in("temperature", value, 0.0, 2.0)?;
        out.temperature = value;
    }
    if let Some(value) = given.cfg_coef {
        number_in("cfg_coef", value, 0.0, 3.0)?;
        out.cfg_coef = value;
    }
    if let Some(value) = given.use_sampling {
        out.use_sampling = value;
    }
    if let Some(value) = given.strict_eos {
        out.strict_eos = value;
    }
    if let Some(raw) = given.model_path.as_deref() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            out.model_path = None;
        } else {
            let path = Path::new(trimmed);
            if extension(path) != "safetensors" {
                return Err(bad(
                    format!(
                        "model_path 要的是本地 .safetensors 权重文件，扩展名是 {:?}",
                        extension(path)
                    ),
                    format!(
                        "model_path must be a local .safetensors weight file, extension {:?}",
                        extension(path)
                    ),
                ));
            }
            if !path.exists() {
                return Err(ErrorInfo::new(
                    ErrorCode::NotFound,
                    say(
                        format!("权重文件不存在：{}", path.display()),
                        format!("Weights not found: {}", path.display()),
                    ),
                ));
            }
            out.model_path = Some(path.to_path_buf());
        }
    }
    if let Some(list) = given.instruments.as_ref() {
        out.instruments = list.clone();
    }
    Ok(out)
}

/// 三条共用的前置闸。`downloads` = 这一下会往本机下东西（环境或权重），
/// `needs_env` = 这一下得有装好的 venv 才跑得动。
fn transcribe_gate(
    state: &EditorState,
    ch: &ControlChannel,
    downloads: bool,
    needs_env: bool,
) -> Result<(), ErrorInfo> {
    if downloads && !state.transcription.accepted_downloads {
        return Err(ErrorInfo::new(
            ErrorCode::NotAllowed,
            say(
                "本机下载和模型使用许可还没确认。MuScriptor 的权重是 CC BY-NC 4.0，环境要现装几百 MB——那一下只能由人自己在「音频转 MIDI」窗口里勾，外部调用不许代勾，这里也没有为它新开的开关。",
                "Local downloads and the model license are not accepted yet. MuScriptor weights are CC BY-NC 4.0 and the environment is hundreds of MB: only the person at the transcription window can tick that. No external call may answer it, and there is no separate switch for it.",
            ),
        ));
    }
    if needs_env && !state.transcription.runtime.installed() {
        let python = state.transcription.runtime.python_path();
        return Err(ErrorInfo::new(
            ErrorCode::NotAllowed,
            say(
                format!(
                    "本机转录环境还没装：{} 不存在。先 transcribe_prepare_env（同样要人勾过许可），完活后再回来。",
                    python.display()
                ),
                format!(
                    "The local transcription environment is not installed: {} does not exist. Run transcribe_prepare_env first (same license tick), then come back.",
                    python.display()
                ),
            ),
        ));
    }
    if state.transcription.busy() {
        return Err(ErrorInfo::new(
            ErrorCode::Busy,
            say(
                "转录窗口里已经有一个本机任务在跑。先等它结束，或在那扇窗口里按取消。",
                "The transcription window already has a local task running; wait for it or cancel it there.",
            ),
        ));
    }
    let in_flight: usize = JobKind::ALL
        .iter()
        .filter(|kind| kind.is_transcription())
        .map(|kind| ch.jobs.running_of(kind.as_str()))
        .sum();
    if in_flight > 0 {
        return Err(ErrorInfo::new(
            ErrorCode::Busy,
            say(
                "已经有一个转录任务在册（转录的活共用同一份 venv 和权重缓存，所以一次只跑一个）。用 job_list 找回句柄、job_status 看进度、job_cancel 停它。",
                "A transcription job is already registered (they share one venv and one weight cache, so only one runs at a time). Use job_list for the handle, job_status to poll, job_cancel to stop it.",
            ),
        ));
    }
    Ok(())
}

/// 把 worker 的事件泵进任务表：`Message` / `Progress` 进句柄的 message 与 progress，
/// 终态交给 `done` 变成 result。取消是协作式的——`job_cancel` 置位后这里转成
/// `task.cancel()`（那边去 taskkill 子进程），再给 10 秒收尾；子进程不应答就按「已取消」
/// 结掉，任务位不能永远占着。
fn transcribe_job(
    ch: &ControlChannel,
    kind: JobKind,
    task: nbs_io::transcription::TranscriptionTask,
    opening: String,
    done: impl FnOnce(TranscribeTerminal) -> Result<Value, String> + Send + 'static,
) -> Written {
    let id = ch
        .jobs
        .spawn(kind.as_str(), move |job: &JobHandle| {
            use nbs_io::transcription::TranscriptionEvent as Event;
            let mut cancelling: Option<Instant> = None;
            loop {
                for event in task.drain() {
                    match event {
                        Event::Message(text) => job.note(text),
                        Event::Progress {
                            stage,
                            message,
                            completed,
                            total,
                            note_count,
                        } => {
                            let fraction = match (completed, total) {
                                (Some(step), Some(of)) if of > 0 => step as f32 / of as f32,
                                _ => 0.0,
                            };
                            let mut line = if message.is_empty() {
                                stage
                            } else if stage.is_empty() {
                                message
                            } else {
                                format!("{stage} · {message}")
                            };
                            if let Some(count) = note_count {
                                line.push_str(&say(
                                    format!(" · {count} 个音符"),
                                    format!(" · {count} notes"),
                                ));
                            }
                            if line.is_empty() {
                                line = say("处理中", "working");
                            }
                            job.progress(fraction, line);
                        }
                        Event::Ready(info) => return done(TranscribeTerminal::Ready(info)),
                        Event::ModelReady { model, device, dtype } => {
                            return done(TranscribeTerminal::ModelReady {
                                model,
                                device,
                                dtype,
                            })
                        }
                        Event::Complete(output) => return done(TranscribeTerminal::Complete(output)),
                        Event::Failed(error) => return Err(error),
                        Event::Cancelled => return Err(say("已取消", "cancelled")),
                    }
                }
                if job.cancelled() {
                    task.cancel();
                    let since = *cancelling.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(10) {
                        return Err(say(
                            "已取消（本机子进程 10 秒内没应答）",
                            "cancelled (the local worker did not answer within 10s)",
                        ));
                    }
                }
                std::thread::sleep(Duration::from_millis(80));
            }
        })
        .map_err(spawn_reject)?;
    Ok((
        Some(Data::Job(Box::new(job_snapshot(ch, id, kind.as_str())))),
        opening,
    ))
}

fn runtime_info_json(
    runtime: &nbs_io::transcription::TranscriptionRuntime,
    info: &nbs_io::transcription::RuntimeInfo,
) -> Value {
    json!({
        "runtime_root": runtime.root.display().to_string(),
        "environment_installed": runtime.installed(),
        "python": runtime.python_path().display().to_string(),
        "python_version": info.python_version,
        "torch_version": info.torch_version,
        "muscriptor_version": info.muscriptor_version,
        "cuda_available": info.cuda_available,
        "gpu_name": info.gpu_name,
        "gpu_memory_bytes": info.gpu_memory_bytes,
    })
}

/// 窗上的按钮串了才会走到这里：probe 只回 Ready，prepare_model 只回 ModelReady。
/// 形状不对就照实报，不猜成另一种成功。
fn wrong_terminal(kind: &str, got: &TranscribeTerminal) -> String {
    say(
        format!("{kind} 回来的是别种终态：{got:?}"),
        format!("{kind} returned an unexpected terminal state: {got:?}"),
    )
}

fn transcribe_probe(state: &EditorState, ch: &ControlChannel) -> Written {
    transcribe_gate(state, ch, false, false)?;
    let runtime = state.transcription.runtime.clone();
    let task = runtime.probe();
    transcribe_job(
        ch,
        JobKind::TranscribeProbe,
        task,
        say(
            "已起转录环境检查任务（不下载任何东西）；用 job_status 轮询",
            "Transcription environment probe started (it downloads nothing); poll with job_status",
        ),
        move |terminal| match terminal {
            TranscribeTerminal::Ready(info) => Ok(runtime_info_json(&runtime, &info)),
            other => Err(wrong_terminal("transcribe_probe", &other)),
        },
    )
}

fn transcribe_prepare_env(state: &EditorState, ch: &ControlChannel, cuda: Option<bool>) -> Written {
    transcribe_gate(state, ch, true, false)?;
    // 省略 = 窗上那个勾当前是什么就是是什么；这条链上「AI 没给」从来不该改人的设置。
    let cuda = cuda.unwrap_or(state.transcription.install_cuda);
    let runtime = state.transcription.runtime.clone();
    let task = runtime.prepare(cuda);
    transcribe_job(
        ch,
        JobKind::TranscribePrepareEnv,
        task,
        say(
            format!(
                "已起环境安装任务（{} 构建，几百 MB，装到 {}）；用 job_status 轮询",
                if cuda { "CUDA" } else { "CPU" },
                runtime.root.display()
            ),
            format!(
                "Environment install started ({} build, hundreds of MB, into {}); poll with job_status",
                if cuda { "CUDA" } else { "CPU" },
                runtime.root.display()
            ),
        ),
        move |terminal| match terminal {
            TranscribeTerminal::Ready(info) => {
                let mut out = runtime_info_json(&runtime, &info);
                out["cuda_build"] = json!(cuda);
                Ok(out)
            }
            other => Err(wrong_terminal("transcribe_prepare_env", &other)),
        },
    )
}

fn transcribe_prepare_model(
    state: &EditorState,
    ch: &ControlChannel,
    given: &TranscribeOptions,
) -> Written {
    // 参数先过，闸后过：`model` 写错和「没勾许可」是两件不同的事，前者改调用方的 JSON，
    // 后者要人去窗口，两条撞在一起只会让人去查错的那一头。
    let options = transcribe_options(state, given)?;
    transcribe_gate(state, ch, true, true)?;
    let runtime = state.transcription.runtime.clone();
    let asked = options.model.clone();
    let task = runtime.prepare_model(options);
    transcribe_job(
        ch,
        JobKind::TranscribePrepareModel,
        task,
        say(
            format!("已起模型预检任务（{asked}）；用 job_status 轮询"),
            format!("Model preflight started ({asked}); poll with job_status"),
        ),
        move |terminal| match terminal {
            TranscribeTerminal::ModelReady {
                model,
                device,
                dtype,
            } => Ok(json!({
                "model": model,
                "device": device,
                "dtype": dtype,
                "cached_model": runtime.cached_model(&asked).map(|p| p.display().to_string()),
            })),
            other => Err(wrong_terminal("transcribe_prepare_model", &other)),
        },
    )
}

fn transcribe_audio(
    state: &EditorState,
    ch: &ControlChannel,
    audio: &str,
    given: &TranscribeOptions,
    save_midi: Option<&str>,
    overwrite: bool,
) -> Written {
    let options = transcribe_options(state, given)?;
    transcribe_gate(state, ch, true, true)?;
    let source = resolve_path(state, ch, audio, AUDIO_EXTS, true)?;
    let target = match save_midi {
        Some(raw) => {
            let target = resolve_path(state, ch, raw, &["mid", "midi"], false)?;
            overwrite_gate(&target, overwrite)?;
            Some(target)
        }
        None => None,
    };
    let runtime = state.transcription.runtime.clone();
    let model = options.model.clone();
    let task = runtime.transcribe(source.clone(), options);
    let where_to = match &target {
        Some(path) => say(
            format!("，完活后把 MIDI 另存到 {}", path.display()),
            format!("; the MIDI is then delivered to {}", path.display()),
        ),
        None => say(
            "（产物只留在转录目录里）",
            " (the artifact stays in the transcription folder)",
        ),
    };
    let deliver = target;
    transcribe_job(
        ch,
        JobKind::TranscribeAudio,
        task,
        say(
            format!("已起转录任务（{model}，{}）{where_to}；用 job_status 轮询", source.display()),
            format!(
                "Transcription started ({model}, {}){where_to}; poll with job_status",
                source.display()
            ),
        ),
        move |terminal| match terminal {
            TranscribeTerminal::Complete(output) => {
                transcribed_midi(&source, deliver.as_deref(), output, &model)
            }
            other => Err(wrong_terminal("transcribe_audio", &other)),
        },
    )
}

/// 转录完的核对：先把 MIDI 落一份到调用方点名的路径（那条路径已经过完路径闸与覆盖闸），
/// 再把窗上那两条数一遍带回去。音符数对不上是窗上「停止导入」的那条判据，这里**照实报、
/// 不替人决定**——下一步 `inspect_midi` 会给真正落进歌里的数量。
fn transcribed_midi(
    source: &Path,
    target: Option<&Path>,
    output: nbs_io::transcription::TranscriptionOutput,
    model: &str,
) -> Result<Value, String> {
    let bytes = read_bounded(&output.midi_path)?;
    let preview = nbs_io::midi_preview::parse_midi_preview(&bytes)?;
    let importer_events = nbs_io::midi::parse_midi(&bytes)
        .map(|midi| midi.tracks.iter().map(|track| track.events.len()).sum::<usize>())
        .map_err(|e| format!("转录产物已写好，但 NBS 的 MIDI 读取器打不开它：{e}"))?;
    if let Some(path) = target {
        std::fs::write(path, &bytes).map_err(|e| {
            format!(
                "转录完成了，但把 MIDI 写到 {} 失败：{e}；原产物仍在 {}",
                path.display(),
                output.midi_path.display()
            )
        })?;
    }
    Ok(json!({
        "audio": source.display().to_string(),
        "model": model,
        "midi_path": output.midi_path.display().to_string(),
        "events_path": output.events_path.display().to_string(),
        "delivered_midi": target.map(|p| p.display().to_string()),
        "raw_note_count": output.raw_note_count,
        "preview_notes": preview.notes.len(),
        "preview_seconds": preview.duration_seconds,
        "importer_events": importer_events,
        "note_count_mismatch": importer_events != preview.notes.len(),
        "import_blocked_reason": preview.import_blocked_reason,
        "device": output.device,
        "dtype": output.dtype,
        "warnings": output.warnings,
    }))
}

/// 读一份产物，带上限。转录出来的 MIDI 正常也就几百 KB，撞到上限说明结果不对。
fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("读不到 {}：{e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_MIDI_PREVIEW_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
    if bytes.len() as u64 > MAX_MIDI_PREVIEW_BYTES {
        return Err(format!(
            "{} 超过 {MAX_MIDI_PREVIEW_BYTES} 字节上限，不像转录产物",
            path.display()
        ));
    }
    Ok(bytes)
}

fn change_line(field: &str, touched: usize, clamped: usize, add: bool, skipped: usize) -> String {
    let mut line = say(
        format!(
            "{} {field}：{touched} 个音符",
            if add { "累加" } else { "设置" },
        ),
        format!(
            "{} {field}: {touched} notes",
            if add { "Added to" } else { "Set" },
        ),
    );
    if clamped > 0 {
        line.push_str(&say(
            format!("（{clamped} 个被夹到边界）"),
            format!(" ({clamped} clamped to the edge)"),
        ));
    }
    if skipped > 0 {
        line.push_str(&locked_skip_line(skipped));
    }
    line
}

/// 范围内的既有音符，返回（音符，被跳过的锁定层音符数）。锁定层按原版语义**跳过**——
/// 界面的批量改走的是浮动选区，那里本来就进不来锁定层。跳过的数量必须回到回执文字里，
/// 不然「批量改了 3 个音符」会让人以为锁定层也改了。
fn notes_in_scope(
    state: &EditorState,
    scope: &Scope,
) -> Result<(Vec<(u32, u32, Note)>, usize), ErrorInfo> {
    let song = &state.doc.song;
    let locked = song.locked_layers();
    let raw: Vec<(u32, u32, Note)> = match scope {
        Scope::All => song.iter_notes().collect(),
        Scope::Selection => {
            return Err(ErrorInfo::new(
                ErrorCode::Internal,
                say(
                    "Scope::Selection 不该走到这条分支",
                    "Scope::Selection should not reach notes_in_scope",
                ),
            ))
        }
        Scope::Rect {
            tick_from,
            layer_from,
            tick_to,
            layer_to,
        } => {
            if tick_to < tick_from || layer_to < layer_from {
                return Err(bad(
                    "范围的 to 必须不小于 from",
                    "range bounds must have to >= from",
                ));
            }
            song.notes_in_rect(*tick_from, *layer_from, *tick_to, *layer_to)
        }
    };
    let mut skipped = 0usize;
    let out = raw
        .into_iter()
        .filter(|(_, layer, _)| {
            let is_locked = locked.get(*layer as usize).copied().unwrap_or(false);
            skipped += usize::from(is_locked);
            !is_locked
        })
        .collect();
    Ok((out, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Modal, ModalButtons, ModalQueue, WindowKind};
    use nbs_control::LayerSet;

    fn cell(tick: u32, layer: u32, instrument: u8, key: u8) -> CellNote {
        CellNote {
            tick,
            layer,
            instrument,
            key,
            velocity: None,
            panning: None,
            pitch: None,
        }
    }

    /// 链路挂好、服务不起：TCP 与收件箱由 `nbs-control` 自己测，这里只测执行层。
    struct Harness {
        state: EditorState,
        ctx: Context,
        channel: ControlChannel,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = Context::default();
            let mut state = EditorState::for_test();
            let channel = attach(&mut state, &ctx).clone();
            Self {
                state,
                ctx,
                channel,
            }
        }

        fn run(&mut self, op: &Op) -> Outcome {
            execute(&mut self.state, &self.ctx, &self.channel, None, op)
        }

        /// 演示曲换成空谱：断言只关心这次调用真正写进去的东西。
        fn empty_song(&mut self) {
            let out = self.run(&Op::ClearSong {
                expect_rev: None,
                discard_changes: true,
            });
            assert!(out.ok, "{:?}", out.error);
        }

        fn note(&self, tick: u32, layer: u32) -> Option<Note> {
            self.state.doc.song.get(tick, layer)
        }

        /// 补足到 `n` 层，用例不依赖演示曲自带几层。
        fn ensure_layers(&mut self, n: usize) {
            while self.state.doc.song.layers.len() < n {
                let out = self.run(&Op::LayerAdd {
                    at: None,
                    name: None,
                    expect_rev: None,
                });
                assert!(out.ok, "{:?}", out.error);
            }
        }
    }

    fn song_with_note(tick: u32, key: u8) -> Song {
        let mut song = Song::new();
        song.set(tick, 0, Note::new(0, key));
        song.recalculate();
        song.dirty = true;
        song
    }

    #[test]
    fn tab_reads_are_stable_across_reorder_and_snapshots_hide_interaction_state() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.state.doc.song.set(1, 0, Note::new(0, 45));
        h.state.doc.song.recalculate();
        let first = h.state.active_tab().unwrap();
        let second = h.state.push_tab(song_with_note(5, 50));
        let third = h.state.push_tab(song_with_note(9, 57));
        let second_session = h.state.store.session(second).unwrap();
        let third_session = h.state.store.session(third).unwrap();

        let out = h.run(&Op::ListTabs);
        assert!(out.ok, "{:?}", out.error);
        let Data::Tabs { tabs, active_tab } = out.data.unwrap() else {
            panic!("ListTabs 数据形状不对")
        };
        assert_eq!(tabs.len(), 3);
        assert_eq!(active_tab, Some(h.state.store.session(first).unwrap()));

        let by_id = TabRef {
            id: Some(second_session),
            index: None,
        };
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&by_id),
            &Op::GetNotes {
                tick_from: None,
                tick_to: None,
                layer_from: None,
                layer_to: None,
                ticks_per_bar: None,
                bars: None,
            },
        );
        assert!(out.ok, "{:?}", out.error);
        let Data::Notes { cells, .. } = out.data.unwrap() else {
            panic!("GetNotes 数据形状不对")
        };
        assert_eq!(cells.len(), 1);
        assert_eq!((cells[0].tick, cells[0].key), (5, 50));
        assert_eq!(h.state.active_tab(), Some(first), "读后台标签不能切歌");

        assert!(h.state.store.reorder(second, 0));
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&by_id),
            &Op::GetSnapshot,
        );
        assert!(out.ok, "{:?}", out.error);
        let Data::Snapshot(snapshot) = out.data.unwrap() else {
            panic!("GetSnapshot 数据形状不对")
        };
        assert_eq!(snapshot.song_session, second_session);
        assert_eq!(
            snapshot.active_tab,
            Some(h.state.store.session(first).unwrap())
        );
        assert_eq!(snapshot.tabs[0].id, second_session, "稳定 id 不随重排漂");
        assert_eq!(snapshot.playhead, None);
        assert_eq!(snapshot.playing, None);
        assert_eq!(snapshot.modal_count, None);
        assert_eq!(h.state.active_tab(), Some(first));

        let by_index = TabRef {
            id: None,
            index: Some(2),
        };
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&by_index),
            &Op::GetNotes {
                tick_from: None,
                tick_to: None,
                layer_from: None,
                layer_to: None,
                ticks_per_bar: None,
                bars: None,
            },
        );
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(
            out.report.song_session, third_session,
            "index=2 现在指向第三首"
        );
        let gone = TabRef {
            id: Some(third_session + 1000),
            index: None,
        };
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&gone),
            &Op::GetSnapshot,
        );
        assert_eq!(code(&out), ErrorCode::TabGone);

        let out = h.run(&Op::SwitchTab {
            tab: TabRef {
                id: Some(third_session),
                index: None,
            },
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(h.state.active_tab(), Some(third));
    }

    #[test]
    fn parked_writes_need_the_switch_and_a_rev_without_touching_active_caches() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let first = h.state.active_tab().unwrap();
        let second = h.state.push_tab(song_with_note(5, 50));
        let tab = TabRef {
            id: Some(h.state.store.session(second).unwrap()),
            index: None,
        };
        let rev = h.state.store.parked_doc(second).unwrap().song.rev;
        let active_rev = h.state.doc.song.rev;
        let audio_rev = h.state.audio_rev;
        let active_tempo = h.state.audio.clock().tempo();
        let write = Op::SetNotes {
            cells: vec![cell(6, 0, 0, 55)],
            label: None,
            expect_rev: Some(rev),
        };

        let out = execute(&mut h.state, &h.ctx, &h.channel, Some(&tab), &write);
        assert_eq!(code(&out), ErrorCode::ParkedTabReadonly);
        assert!(h
            .state
            .store
            .parked_doc(second)
            .unwrap()
            .song
            .get(6, 0)
            .is_none());

        h.state.prefs.control_allow_parked_write = true;
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&tab),
            &Op::SetNotes {
                cells: vec![cell(6, 0, 0, 55)],
                label: None,
                expect_rev: None,
            },
        );
        assert_eq!(code(&out), ErrorCode::ExpectRequired);

        let out = execute(&mut h.state, &h.ctx, &h.channel, Some(&tab), &write);
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(
            out.report.song_session,
            h.state.store.session(second).unwrap()
        );
        assert_eq!(
            h.state.store.parked_doc(second).unwrap().song.get(6, 0),
            Some(Note::new(0, 55))
        );
        assert_eq!(h.state.active_tab(), Some(first));
        assert_eq!(h.state.doc.song.rev, active_rev);
        assert_eq!(
            h.state.audio_rev, audio_rev,
            "后台写不能污染当前标签的事件表缓存"
        );

        let rev = out.report.rev;
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&tab),
            &Op::Undo {
                steps: Some(1),
                expect_rev: Some(rev),
            },
        );
        assert!(out.ok, "{:?}", out.error);
        assert!(h
            .state
            .store
            .parked_doc(second)
            .unwrap()
            .song
            .get(6, 0)
            .is_none());
        let rev = out.report.rev;
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&tab),
            &Op::Redo {
                steps: Some(1),
                expect_rev: Some(rev),
            },
        );
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(
            h.state.store.parked_doc(second).unwrap().song.get(6, 0),
            Some(Note::new(0, 55))
        );
        assert_eq!(h.state.active_tab(), Some(first));
        assert_eq!(h.state.audio_rev, audio_rev);

        let rev = out.report.rev;
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&tab),
            &Op::SetMeta {
                meta: SetMeta {
                    tempo: Some(12.0),
                    ..SetMeta::default()
                },
                expect_rev: Some(rev),
            },
        );
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(
            h.state.store.parked_doc(second).unwrap().song.meta.tempo,
            12.0
        );
        assert_eq!(
            h.state.audio.clock().tempo(),
            active_tempo,
            "后台改速度不能改当前播放时钟"
        );

        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&tab),
            &Op::Playback(Playback::Play),
        );
        assert_eq!(code(&out), ErrorCode::RequiresActiveTab);

        let out = h.run(&Op::CloseTab { tab, discard: None });
        assert_eq!(code(&out), ErrorCode::NeedsConfirm);
        assert!(h.state.store.contains(second));
        let out = h.run(&Op::CloseTab {
            tab,
            discard: Some(true),
        });
        assert!(out.ok, "{:?}", out.error);
        assert!(!h.state.store.contains(second));
        let out = execute(
            &mut h.state,
            &h.ctx,
            &h.channel,
            Some(&tab),
            &Op::GetSnapshot,
        );
        assert_eq!(code(&out), ErrorCode::TabGone);
    }

    #[test]
    fn answer_modal_accepts_no_and_cancel_without_running_the_yes_action() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.channel.policy.lock().unwrap().allow_danger = true;
        h.state.prefs.master_volume = 0.75;
        for (answer, expected) in [("no", 0.25), ("cancel", 0.75)] {
            h.state.prefs.master_volume = 0.75;
            h.state.modals = ModalQueue::yes_no_cancel(
                "Confirm",
                "test",
                Command::SetMasterVolume(0.0),
                Command::SetMasterVolume(0.25),
            );
            let out = h.run(&Op::RunAction {
                action: "answer_modal".into(),
                args: Some(json!({ "answer": answer })),
            });
            assert!(out.ok, "{:?}", out.error);
            assert!(h.state.modals.is_empty());
            assert_eq!(h.state.prefs.master_volume, expected, "{answer} 续作不正确");
        }
    }
    #[test]
    fn exit_action_keeps_the_explicit_discard_contract() {
        let _prefs = crate::isolate_prefs();
        let mut state = EditorState::for_test();
        state.doc.song.dirty = true;
        let spec = find_action("exit").unwrap();
        assert!(
            (spec.build)(&state, None).is_err(),
            "脏工程必须先显式确认丢弃"
        );
        let discard = json!({ "discard_changes": true });
        assert_eq!(
            (spec.build)(&state, Some(&discard)).unwrap(),
            Some(Command::ExitDiscard)
        );
        state.doc.song.dirty = false;
        assert_eq!((spec.build)(&state, None).unwrap(), Some(Command::Exit));
    }

    fn code(out: &Outcome) -> ErrorCode {
        out.error.as_ref().expect("失败回执必带错误码").code
    }

    /// 只报音色不报音域，调用方还是会往 33..=57 外面写，导出成一片静音音符盒。
    /// 这里钉的是「说出去的上下界」和「编辑器自己判超窗用的那一把尺」是同一对数。
    #[test]
    fn instruments_carry_the_playable_key_range() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        let out = h.run(&Op::GetInstruments);
        assert!(out.ok, "{:?}", out.error);
        let Some(Data::Instruments {
            instruments,
            mc_key_range,
        }) = out.data
        else {
            panic!("GetInstruments 该回 Instruments，实际 {:?}", out.data);
        };
        assert!(!instruments.is_empty(), "音色表空了就是 bank 没接上");
        let [low, high] = mc_key_range;
        for key in [low.saturating_sub(1), high.saturating_add(1)] {
            assert!(
                Note::new(0, key).is_out_of_range(),
                "{key} 在 {low}..={high} 之外，回执却把它算作可播"
            );
        }
        for key in [low, high] {
            assert!(
                !Note::new(0, key).is_out_of_range(),
                "{key} 是承诺的可播边界，编辑器却说它超窗"
            );
        }
    }

    #[test]
    fn set_notes_is_one_undo_step() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let rev = h.state.doc.song.rev;
        let out = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 1, 36), cell(1, 0, 1, 40)],
            label: None,
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.report.cells_changed, 2);
        // 一次调用只推进一个 rev，不管它写了多少格。
        assert_eq!(out.report.rev, rev + 1);
        assert_eq!(h.note(0, 0).map(|n| n.key), Some(36));
        assert_eq!(h.note(1, 0).map(|n| n.velocity), Some(VEL_MAX));

        let out = h.run(&Op::Undo {
            steps: None,
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert!(
            h.note(0, 0).is_none() && h.note(1, 0).is_none(),
            "两条音符要一起退回"
        );
    }

    /// 清空只动音符，层一格没动。回执要把留下的层报出来——现场真有人按
    /// 「空工程 = 一层」配器，写完才发现多出一层空轨，还得再发一次 layer_remove 收。
    #[test]
    fn clear_song_reports_the_layers_it_leaves_behind() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.ensure_layers(3);
        for index in 0..3u32 {
            let out = h.run(&Op::LayerUpdate {
                index,
                set: LayerSet {
                    name: Some(format!("谱{index}")),
                    ..Default::default()
                },
                expect_rev: None,
            });
            assert!(out.ok, "{:?}", out.error);
        }
        assert!(h
            .run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok);

        let out = h.run(&Op::ClearSong {
            expect_rev: None,
            discard_changes: true,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.report.cells_changed, 1, "这次只清掉一枚音符");
        let names: Vec<String> = match out
            .data
            .as_ref()
            .expect("清空要顺带把层报出来，省一次 get_layers")
        {
            Data::Layers { layers } => layers.iter().map(|l| l.name.clone()).collect(),
            other => panic!("形状不对：{other:?}"),
        };
        assert_eq!(names, vec!["谱0", "谱1", "谱2"], "层必须原样还在");
        for name in &names {
            assert!(
                out.report.message.contains(name.as_str()),
                "message 要点名留下的层：{}",
                out.report.message
            );
        }
        assert_eq!(h.state.doc.song.total_blocks, 0, "音符确实清干净了");
    }

    /// 省略的边界要按全曲算，并且要在回执里说清算成了什么——调用方分不清
    /// 「这段本来就空」和「我少读了」的时候，这一句就是它唯一的依据。
    #[test]
    fn get_notes_without_bounds_reads_the_whole_song() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        assert!(h
            .run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36), cell(40, 0, 1, 40), cell(80, 1, 2, 44)],
                label: None,
                expect_rev: None,
            })
            .ok);

        let out = h.run(&Op::GetNotes {
            tick_from: None,
            tick_to: None,
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: None,
        });
        assert!(out.ok, "{:?}", out.error);
        let cells = match out.data.as_ref().expect("读音符必带 data") {
            Data::Notes { cells, .. } => cells,
            other => panic!("形状不对：{other:?}"),
        };
        assert_eq!(cells.len(), 3, "一个参数都不给要读回整首，含最后一层");
        assert!(!out.report.message.is_empty(), "省略的边界要说明算成了什么");

        // 起点在歌尾之外：读空，而不是报一个自相矛盾的区间错。
        let out = h.run(&Op::GetNotes {
            tick_from: Some(9_000),
            tick_to: None,
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert!(
            matches!(out.data.as_ref(), Some(Data::Notes { cells, .. }) if cells.is_empty()),
            "歌尾之外读空"
        );

        // 两个都给且倒过来，仍然要拦住。
        let out = h.run(&Op::GetNotes {
            tick_from: Some(10),
            tick_to: Some(2),
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: None,
        });
        assert!(!out.ok, "显式给反的区间不能静默读空");
        assert_eq!(code(&out), ErrorCode::BadArgument);
    }

    /// 按小节读的要拿到音名而不是键号：39 心算成「A3 那一档」这种事，错了只有耳朵能发现，
    /// 而写进去和读回来用的是同一张表，逐行对比也对得上。空小节同样占一行——
    /// 「这段本来就空」和「我少读了」必须分得开。
    #[test]
    fn bars_view_names_the_notes_and_keeps_the_empty_bars() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        assert!(h
            .run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 39), cell(11, 0, 1, 43), cell(12, 0, 2, 45)],
                label: None,
                expect_rev: None,
            })
            .ok);

        let out = h.run(&Op::GetNotes {
            tick_from: None,
            tick_to: None,
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: Some([0, 1]),
        });
        assert!(out.ok, "{:?}", out.error);
        let view = match out.data.as_ref().expect("读音符必带 data") {
            Data::Notes {
                bar_overview,
                cells,
                ..
            } => {
                assert_eq!(cells.len(), 3, "bars 只切区间，不该少给格子");
                bar_overview
            }
            other => panic!("形状不对：{other:?}"),
        };
        // 默认拍号 4 → 每节 16 格，和第 0 节里的三格一起落在同一节；第 1 节是空的。
        assert_eq!(view.len(), 2);
        assert_eq!(
            (view[0].bar, view[0].tick_from, view[0].tick_to, view[0].cells),
            (0, 0, 15, 3)
        );
        assert_eq!(view[0].text, "0:C4(1) 11:E4(1) 12:F#4(2)");
        assert_eq!(
            (view[1].bar, view[1].tick_from, view[1].tick_to, view[1].cells),
            (1, 16, 31, 0)
        );
        assert!(view[1].text.is_empty());
        // 宽度是从拍号来的，回执要说清，否则调用方以为自己按 12 切了。
        assert!(
            out.report.message.contains(&say("每节 16 格", "16 ticks each")),
            "{}",
            out.report.message
        );

        // 没点名要小节视图时，那份文本一份都不给。
        let out = h.run(&Op::GetNotes {
            tick_from: Some(0),
            tick_to: Some(12),
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: None,
        });
        assert!(
            matches!(out.data.as_ref(), Some(Data::Notes { bar_overview, .. }) if bar_overview.is_empty())
        );
    }

    /// 只给宽度时区间不变，但落在小节中间的端点要把边界收到真读到的范围——
    /// 条目里写的区间必须含得住它自己列出的格。
    #[test]
    fn ticks_per_bar_alone_recuts_the_requested_range() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        assert!(h
            .run(&Op::SetNotes {
                cells: vec![cell(3, 0, 1, 39), cell(11, 0, 1, 43), cell(12, 0, 2, 45)],
                label: None,
                expect_rev: None,
            })
            .ok);

        let out = h.run(&Op::GetNotes {
            tick_from: Some(6),
            tick_to: Some(30),
            layer_from: None,
            layer_to: None,
            ticks_per_bar: Some(12),
            bars: None,
        });
        assert!(out.ok, "{:?}", out.error);
        let view = match out.data.as_ref().expect("读音符必带 data") {
            Data::Notes { bar_overview, .. } => bar_overview,
            other => panic!("形状不对：{other:?}"),
        };
        assert_eq!(view.len(), 3, "6..=30 按 12 切是第 0、1、2 节");
        assert_eq!((view[0].tick_from, view[0].tick_to), (6, 11));
        assert_eq!((view[0].cells, view[0].text.as_str()), (1, "11:E4(1)"));
        assert_eq!((view[1].tick_from, view[1].tick_to, view[1].cells), (12, 23, 1));
        // tick 3 在请求区间之外，这一节就该看不见它，而不是数出一个没列出来的格。
        assert_eq!(view[2].cells, 0);
    }

    /// 三种说不通的小节写法都要在动工程之前拒掉，并且报错要说该改哪一个参数。
    #[test]
    fn bar_requests_that_do_not_make_sense_are_refused() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let mut ask = |from: Option<u32>, to: Option<u32>, bars: Option<[u32; 2]>| {
            h.run(&Op::GetNotes {
                tick_from: from,
                tick_to: to,
                layer_from: None,
                layer_to: None,
                ticks_per_bar: None,
                bars,
            })
        };

        let out = ask(Some(4), None, Some([0, 1]));
        assert_eq!(code(&out), ErrorCode::BadArgument);
        let reason = out.error.as_ref().expect("被拒要有原因").message.clone();
        assert!(reason.contains("bars"), "互斥要说清是哪一条：{reason}");
        assert_eq!(
            code(&ask(None, None, Some([3, 1]))),
            ErrorCode::BadArgument,
            "终点小于起点"
        );
        // 256 节是上限：再多就不是「读一段」，而是让主线程拼一份没人读的长文本。
        assert!(!ask(None, None, Some([0, 256])).ok, "257 节要拒");
        assert!(ask(None, None, Some([0, 255])).ok, "256 节正好放行");
        // 小节号乘宽度溢出 tick：不能悄悄绕回一个小数字去读。
        assert_eq!(code(&ask(None, None, Some([4_000_000_000, 4_000_000_100]))), ErrorCode::BadArgument);
    }

    #[test]
    fn stale_expect_rev_refuses_without_writing() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let rev = h.state.doc.song.rev;
        assert!(
            h.run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok
        );
        let out = h.run(&Op::SetNotes {
            cells: vec![cell(2, 0, 1, 36)],
            label: None,
            expect_rev: Some(rev),
        });
        assert!(!out.ok);
        assert_eq!(code(&out), ErrorCode::RevConflict);
        assert!(h.note(2, 0).is_none(), "冲突的那次调用一格都没写");
        // 拒绝照样把当前 rev 报回去，调用方不用先 GetSnapshot 对账。
        assert_eq!(out.report.rev, rev + 1);
    }

    #[test]
    fn unknown_instrument_and_duplicate_cell_are_rejected() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        // 自定义音色从 20 起，而空谱一个自定义都没有：25 必然对不上号。
        let out = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 25, 36)],
            label: None,
            expect_rev: None,
        });
        assert_eq!(code(&out), ErrorCode::OutOfRange);
        let out = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 1, 36), cell(0, 0, 1, 40)],
            label: None,
            expect_rev: None,
        });
        assert_eq!(code(&out), ErrorCode::BadArgument);
        assert!(h.note(0, 0).is_none());
    }

    #[test]
    fn locked_layer_blocks_point_write_and_is_skipped_by_batch() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.ensure_layers(2);
        assert!(
            h.run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok
        );
        let out = h.run(&Op::LayerUpdate {
            index: 0,
            set: LayerSet {
                lock: Some("locked".into()),
                ..Default::default()
            },
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);

        // 点和面两套语义都和界面一致：画笔落在锁定层是拒绝，批量是跳过并计入变更量。
        let out = h.run(&Op::SetNotes {
            cells: vec![cell(4, 0, 1, 36)],
            label: None,
            expect_rev: None,
        });
        assert_eq!(code(&out), ErrorCode::LockedLayer);
        assert!(h.note(4, 0).is_none());

        let out = h.run(&Op::BatchField {
            field: BatchField::Velocity,
            mode: "set".into(),
            value: 50,
            scope: Scope::All,
            label: None,
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.report.cells_changed, 0, "锁定层上的音符不该被改");
        assert_eq!(h.note(0, 0).map(|n| n.velocity), Some(VEL_MAX));
    }

    #[test]
    fn modal_blocks_writes_but_answer_modal_gets_through() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.state.modals.items.push_back(Modal {
            title: "Confirm",
            text: String::new(),
            buttons: ModalButtons::YesNo,
        });

        let out = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 1, 36)],
            label: None,
            expect_rev: None,
        });
        assert_eq!(code(&out), ErrorCode::ModalOpen);
        assert!(h.note(0, 0).is_none());
        // 只读不受弹窗影响。
        assert!(h.run(&Op::GetSnapshot).ok);

        // 替人答确认框属于「按人的按钮」那一类：默认策略先拒一道，闸的豁免轮不到它。
        let out = h.run(&Op::RunAction {
            action: "answer_modal".into(),
            args: Some(json!({ "yes": true })),
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed);
        assert!(!h.state.modals.is_empty());

        h.channel.policy.lock().unwrap().allow_danger = true;
        let out = h.run(&Op::RunAction {
            action: "answer_modal".into(),
            args: Some(json!({ "yes": true })),
        });
        assert!(out.ok, "{:?}", out.error);
        assert!(h.state.modals.is_empty());
        assert!(
            h.run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok
        );
    }

    #[test]
    fn draft_window_blocks_writes() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.state.window = Some(WindowKind::AudioExport);
        let out = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 1, 36)],
            label: None,
            expect_rev: None,
        });
        assert_eq!(code(&out), ErrorCode::WindowOpen);
    }

    #[test]
    fn danger_action_needs_policy_and_ordinary_one_does_not() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        let out = h.run(&Op::RunAction {
            action: "open_url".into(),
            args: Some(json!({ "url": "https://example.com" })),
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed);

        // 不危险的动作默认就能调：缩放走的是界面同一条命令。
        let before = h.state.prefs.window_scale;
        let out = h.run(&Op::RunAction {
            action: "window_scale_up".into(),
            args: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_ne!(h.state.prefs.window_scale, before);
    }

    #[test]
    fn exit_refuses_while_dirty_and_a_bad_flag_is_not_a_pass() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.channel.policy.lock().unwrap().allow_danger = true;
        h.empty_song();
        assert!(
            h.run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok
        );
        assert!(h.state.doc.song.dirty, "用例前提：工程要有未保存改动");
        let history = h.state.doc.history.len();

        // 界面那颗「退出」是不问就关的，外部没有鼠标去点掉它，所以脏着就得拒。
        let out = h.run(&Op::RunAction {
            action: "exit".into(),
            args: None,
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed);
        assert_eq!(
            h.state.doc.history.len(),
            history,
            "被拒的退出不能顺手丢掉历史"
        );

        // 破坏性闸门的参数只认真伪：给个字符串不许当成「没给」而放行。
        let out = h.run(&Op::RunAction {
            action: "exit".into(),
            args: Some(json!({ "discard_changes": "yes" })),
        });
        assert_eq!(code(&out), ErrorCode::BadArgument);

        let out = h.run(&Op::RunAction {
            action: "exit".into(),
            args: Some(json!({ "discard_changes": true })),
        });
        assert!(out.ok, "{:?}", out.error);
    }

    #[test]
    fn loop_end_only_extends_the_song() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        // 空谱的 song_length 是 0，「缩短」就没有可减的量：先在 tick 10 落一枚音符。
        assert!(
            h.run(&Op::SetNotes {
                cells: vec![cell(10, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok
        );
        let len = h.state.doc.song.song_length();
        assert!(len > 1, "用例前提：歌曲要有长度可减");
        let out = h.run(&Op::SetMeta {
            meta: SetMeta {
                loop_end: Some(1),
                ..Default::default()
            },
            expect_rev: None,
        });
        assert_eq!(
            code(&out),
            ErrorCode::BadArgument,
            "缩短要删末尾音符，不能藏在元数据里"
        );

        let out = h.run(&Op::SetMeta {
            meta: SetMeta {
                loop_end: Some(len + 4),
                ..Default::default()
            },
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(h.state.doc.song.song_length(), len + 4);

        // 一个字段都没给的 meta 属于调用方写错了，不是「空操作成功」。
        let out = h.run(&Op::SetMeta {
            meta: SetMeta::default(),
            expect_rev: None,
        });
        assert_eq!(code(&out), ErrorCode::BadArgument);
    }

    /// 外部改元数据必须和人在属性窗里改留下同样的痕迹：标脏、被 `discard_changes` 闸看见。
    #[test]
    fn metadata_writes_mark_the_song_unsaved() {
        let mut h = Harness::new();
        assert!(!h.state.doc.song.dirty, "用例前提：新开的工程不该已经脏");

        // 只给了和当前值一样的字段：数据没动，不许冒充「有未保存改动」。
        let same = h.state.doc.song.meta.tempo;
        h.run(&Op::SetMeta {
            meta: SetMeta {
                tempo: Some(same),
                ..Default::default()
            },
            expect_rev: None,
        });
        assert!(!h.state.doc.song.dirty, "同值写入不算改动");

        h.run(&Op::SetMeta {
            meta: SetMeta {
                name: Some("外部改的名".into()),
                ..Default::default()
            },
            expect_rev: None,
        });
        assert!(h.state.doc.song.dirty, "改名要标脏，和属性窗同一条口径");

        let out = h.run(&Op::ClearSong {
            expect_rev: None,
            discard_changes: false,
        });
        assert_eq!(
            code(&out),
            ErrorCode::NotAllowed,
            "元数据改动也得被 discard_changes 闸看见：{:?}",
            out.error
        );
        let out = h.run(&Op::ClearSong {
            expect_rev: None,
            discard_changes: true,
        });
        assert!(out.ok, "认错后放行：{:?}", out.error);
    }

    #[test]
    fn layer_move_moves_notes_with_the_layer() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.ensure_layers(3);
        for index in 0..3u32 {
            let out = h.run(&Op::LayerUpdate {
                index,
                set: LayerSet {
                    name: Some(format!("L{index}")),
                    ..Default::default()
                },
                expect_rev: None,
            });
            assert!(out.ok, "{:?}", out.error);
        }
        assert!(
            h.run(&Op::SetNotes {
                cells: vec![cell(0, 0, 1, 36)],
                label: None,
                expect_rev: None,
            })
            .ok
        );

        let out = h.run(&Op::LayerMove {
            from: 0,
            to: 2,
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);
        match out.data.as_ref().expect("LayerMove 应回图层表") {
            Data::Layers { layers } => {
                let names: Vec<&str> = layers.iter().map(|l| l.name.as_str()).collect();
                assert_eq!(names, vec!["L1", "L2", "L0"]);
            }
            other => panic!("拿到的是 {other:?}"),
        }
        assert!(h.note(0, 0).is_none(), "音符跟着层走");
        assert_eq!(h.note(0, 2).map(|n| n.key), Some(36));

        let out = h.run(&Op::Undo {
            steps: None,
            expect_rev: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(h.state.doc.song.layers.len(), 3);
        assert_eq!(h.note(0, 0).map(|n| n.key), Some(36));
        assert!(h.note(0, 2).is_none());
    }

    #[test]
    fn save_path_needs_directory_permission() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        // 和隔离用的配置目录同级，绝不在白名单里。
        let dir =
            std::env::temp_dir().join(format!("nbs-control-path-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.nbs");
        let _ = std::fs::remove_file(&target);

        let out = h.run(&Op::SavePath {
            path: target.display().to_string(),
            version: None,
            overwrite: false,
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed);
        assert!(!target.exists());

        // 等价于启动时的 --control-dir：目录加进白名单后同一条路径就该通。
        sync_policy(&h.state, std::slice::from_ref(&dir), false);
        let out = h.run(&Op::SavePath {
            path: target.display().to_string(),
            version: None,
            overwrite: false,
        });
        assert!(out.ok, "{:?}", out.error);
        assert!(target.is_file());
        // 落盘之后工程不再脏。
        assert!(!out.report.dirty);
        let _ = std::fs::remove_file(&target);
    }

    /// 设置页那栏目录的加入和移除都要当场生效。白名单是策略层每次重算的：
    /// 只在启动时拼一次的话，删掉一条要等下次启动才收得走——那是最坏的一种「已经关掉了」。
    #[test]
    fn settings_dirs_are_granted_and_revoked_in_place() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let base = std::env::temp_dir().join(format!("nbs-control-dirs-{}", std::process::id()));
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // 读表要现读现放：写成闭包会把 `h.state` 借到过 `h.run` 之后，两者打架。
        fn listed(h: &Harness, dir: &Path) -> bool {
            let policy = h.state.control.as_ref().unwrap().policy.lock().unwrap();
            policy.allowed_dirs.iter().any(|d| d.as_path() == dir)
        }
        fn table(h: &Harness) -> String {
            let policy = h.state.control.as_ref().unwrap().policy.lock().unwrap();
            policy
                .allowed_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        }
        let save = |dir: &Path, name: &str| Op::SavePath {
            path: dir.join(name).display().to_string(),
            version: None,
            overwrite: false,
        };

        // 加一条：立刻就能往里写（行为，不是只看表）。
        h.state.prefs.control_dirs = vec![a.to_string_lossy().into_owned()];
        sync_policy(&h.state, &[], false);
        let out = h.run(&save(&a, "in-a.nbs"));
        assert!(out.ok, "设置页刚加的那条目录要立刻能写：{:?}", out.error);

        // 再加一条：表里跟着多一条。这一步先只看表——真写一次会把工程挪到那里，
        // 那条目录就同时成了种子目录，撤不撤都还放行，测不出「撤销」这件事。
        h.state.prefs.control_dirs.push(b.to_string_lossy().into_owned());
        sync_policy(&h.state, &[], false);
        assert!(listed(&h, &b), "加了 b 之后表里要有它：{}", table(&h));

        // 撤掉 b：表里没了，写也写不进去。
        h.state.prefs.control_dirs.retain(|d| d != &b.to_string_lossy().into_owned());
        sync_policy(&h.state, &[], false);
        assert!(!listed(&h, &b), "撤掉的 b 不该还留在表里：{}", table(&h));
        let out = h.run(&save(&b, "in-b.nbs"));
        assert_eq!(code(&out), ErrorCode::NotAllowed, "撤掉的那条要立刻写不进去");
        assert!(!b.join("in-b.nbs").exists());
        // 而 a 仍然放行——不是因为 prefs 还记着它，是因为工程此刻就住在那儿（种子目录）。
        // 这个区别要钉住：撤掉设置页那一栏不该把正在用的工程目录一起收走。
        assert!(listed(&h, &a), "工程所在目录该由种子保留：{}", table(&h));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 种子目录、启动旗标、设置页三处汇成同一张表，同一条只留一份。
    /// 去重不是好看：那一栏是把这张表念给人听的，重复项会让人以为放行过两次。
    #[test]
    fn policy_merges_the_three_sources_without_duplicates() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let base = std::env::temp_dir().join(format!("nbs-control-merge-{}", std::process::id()));
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // a 同时由旗标和设置页给出（设置页那条写成带 `.` 的同一个目录），只该出现一次。
        let a_dotted = a.join(".").display().to_string();
        h.state.prefs.control_dirs = vec![b.display().to_string(), a_dotted];
        sync_policy(&h.state, std::slice::from_ref(&a), false);

        let policy = h.state.control.as_ref().unwrap().policy.lock().unwrap().clone();
        let count = |p: &Path| policy.allowed_dirs.iter().filter(|d| d.as_path() == p).count();
        assert_eq!(count(&a), 1, "同一条目录两处分头给过，表里也只能有一条");
        assert_eq!(count(&b), 1);
        let cfg = nbs_io::prefs::Preferences::config_dir();
        assert!(
            policy.allowed_dirs.iter().any(|d| d.as_path() == cfg.as_path()),
            "配置目录这条种子不能因为重算就丢：发现文件写在那儿，丢了连保存都写不进去"
        );
        assert!(!policy.allow_danger, "重算目录不该顺手把危险动作也打开");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn drain_replies_to_every_queued_request() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        let ping = h.channel.inbox.submit(1, "test", Op::Ping);
        let snapshot = h.channel.inbox.submit(2, "test", Op::GetSnapshot);
        assert_eq!(h.channel.inbox.pending(), 2);

        assert_eq!(drain(&mut h.state, &h.ctx), 2);
        assert!(h.channel.inbox.is_empty());
        assert!(ping.recv().expect("回执应送回").ok);
        assert!(snapshot.recv().expect("回执应送回").ok);
        // 没件可取时 drain 返回 0，宿主据此判断要不要再催一帧。
        assert_eq!(drain(&mut h.state, &h.ctx), 0);
    }

    /// 面板读的是 `state.control.clients`，服务端写的是 `cfg.clients`。这两份必须是一个
    /// 东西，接错了不会报错，只会让「谁连着」那一栏永远空着——所以从真 socket 走一遍。
    #[test]
    fn started_server_writes_into_the_ledger_the_panel_reads() {
        let _prefs = crate::isolate_prefs();
        let ctx = Context::default();
        let mut state = EditorState::for_test();
        let mut start = start(&mut state, &ctx, 0, false, false).expect("回环服务该起得来");
        let board = state
            .control
            .as_ref()
            .expect("start 该把链路挂上")
            .clients
            .clone();
        assert_eq!(board.count(), 0, "起了服务不等于有人连着");

        let client = nbs_control::ControlClient::connect(
            start.port,
            &start.token,
            "probe-ide",
            std::time::Duration::from_secs(2),
        )
        .unwrap();
        // 登记发生在服务端那条线程写完 ack 之后，客户端先到、看板后更新是正常时序。
        for _ in 0..400 {
            if board.count() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let snap = board.snapshot();
        assert_eq!(snap.len(), 1, "握手过了就该在面板那一栏露面");
        assert_eq!(snap[0].client, "probe-ide", "显示的是客户端自报的名字");

        client.close();
        for _ in 0..400 {
            if board.count() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(board.count(), 0, "断开的连接不能留成幽灵");

        start.server.stop();
        assert!(board.snapshot().is_empty(), "停服当场清空");
    }

    /// 播放头停在曲尾之外时，起播即停，回执还说一句「开始播放」——调用方只能靠自己先 seek。
    /// 现在 `play` 自己按真正的曲尾归零，并把「落点动过」写进回执。曲内落点照旧不碰。
    #[test]
    fn play_past_the_end_rewinds_and_says_so() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let painted = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 5, 48), cell(8, 0, 5, 50)],
            label: None,
            expect_rev: None,
        });
        assert!(painted.ok, "{:?}", painted.error);
        let length = h.state.doc.song.song_length() as f64;

        let parked = h.run(&Op::Playback(Playback::Seek {
            tick: length + 4.0,
        }));
        assert!(parked.ok, "{:?}", parked.error);
        let out = h.run(&Op::Playback(Playback::Play));
        assert!(out.ok, "{:?}", out.error);
        assert!(
            // 断言跟着 `say` 走：这条链上没有任何东西该取决于界面语言，测试也不该。
            out.report.message.contains(&say("已归零", "reset to 0")),
            "回执要说清落点被动过：{}",
            out.report.message
        );
        assert!(h.state.marker() < 2.0, "起播点该回到 0，实际 {}", h.state.marker());
        h.state.audio.stop();

        let inside = h.run(&Op::Playback(Playback::Seek { tick: 4.0 }));
        assert!(inside.ok, "{:?}", inside.error);
        let out = h.run(&Op::Playback(Playback::Play));
        assert!(out.ok, "{:?}", out.error);
        assert!(
            !out.report.message.contains(&say("归零", "reset")),
            "曲内落点不该被挪：{}",
            out.report.message
        );
        assert!(h.state.marker() >= 4.0, "从 4 起播：{}", h.state.marker());
        h.state.audio.stop();
    }

    #[test]
    fn ping_and_snapshot_carry_the_current_rev() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        let out = h.run(&Op::Ping);
        let Data::Pong { rev, .. } = out.data.expect("Ping 必带 Pong") else {
            panic!("Ping 应回 Pong");
        };
        assert_eq!(rev, h.state.doc.song.rev);
        let snap = snapshot(&h.state, &h.channel, true, h.state.store.active());
        assert_eq!(snap.rev, h.state.doc.song.rev);
        assert_eq!(snap.layers, h.state.doc.song.layers.len());
    }

    /// 手写 `MidiData` 而不是拿导出器生成：`export_midi_bytes` 把所有音符都塞在通道 0 上，
    /// 测不出通道映射和鼓映射这两张表。这里给两条旋律通道（patch 11 / 48）加一段鼓（36、38）。
    fn midi_fixture() -> nbs_io::midi::MidiData {
        let ev = |pos: i64, channel: u8, note: u8, patch: u16, part: usize| nbs_io::midi::MidiEvent {
            pos,
            until: pos + 240,
            note,
            velocity: 100,
            channel,
            patch,
            part,
        };
        let mut midi = nbs_io::midi::MidiData {
            format: 1,
            division: 480,
            tracks: vec![
                nbs_io::midi::MidiTrack {
                    name: "melody".to_string(),
                    events: vec![ev(0, 0, 60, 11, 0), ev(240, 0, 64, 11, 0), ev(480, 1, 67, 48, 1)],
                },
                nbs_io::midi::MidiTrack {
                    name: "drums".to_string(),
                    events: vec![ev(0, 9, 36, 0, 2), ev(240, 9, 38, 0, 2)],
                },
            ],
            channels: 9,
            perc_notes: vec![36, 38],
            parts: vec![
                nbs_io::midi::MidiPart {
                    track: 0,
                    channel: 0,
                    patch: 11,
                    note_duration: true,
                },
                nbs_io::midi::MidiPart {
                    track: 0,
                    channel: 1,
                    patch: 48,
                    note_duration: true,
                },
                nbs_io::midi::MidiPart {
                    track: 1,
                    channel: 9,
                    patch: 0,
                    note_duration: false,
                },
            ],
            minpos: 0,
            maxpos: 720,
            micsecqn: 500_000,
            ..Default::default()
        };
        midi.channel_patch[0] = 11;
        midi.channel_patch[1] = 48;
        midi
    }

    fn plan_of(out: &Outcome) -> MidiPlan {
        let Data::MidiPlan(plan) = out.data.clone().expect("MIDI 两条工具必带核对单") else {
            panic!("回执不是 midi_plan：{:?}", out.data);
        };
        *plan
    }

    /// 省略选项时用的是「人坐在导入窗前看到的那份」，不是出厂 `default()`。
    /// 出厂那份的鼓表是空的，照它导会把打击乐整段判成静音——这条就是钉住那个 bug。
    #[test]
    fn midi_import_baseline_is_the_window_view_not_the_factory_table() {
        let _prefs = crate::isolate_prefs();
        let h = Harness::new();
        let midi = midi_fixture();
        let (opts, enh) = midi_effective(&h.state, &midi, None, None, false).expect("基线不该有非法覆盖");
        assert!(!enh.enabled, "省略 enhanced 就是传统路径");
        assert!(!opts.remember, "省略 remember 就是别动用户的长期设置");
        let gm = &h.state.midi_gm;
        assert_eq!(opts.channel_ins[0], gm.patch(11).0, "通道 0 按 GM 表现算");
        assert_eq!(opts.channel_ins[1], gm.patch(48).0, "通道 1 按 GM 表现算");
        assert_eq!(opts.perc_ins.len(), 2, "鼓映射按这个文件的鼓音清单长出来");
        assert!(opts.perc_ins.iter().all(|ins| *ins != -1), "基线里鼓不该是静音");
        assert_eq!(opts.perc_pitch[0], gm.drum(36).1 + 33, "鼓键号含 +33，和界面同一把尺");

        let factory = nbs_io::midi::MidiImportOptions::default();
        assert!(factory.perc_ins.is_empty(), "出厂那份压根没有鼓映射");
        let from_baseline = nbs_io::midi::import_midi_data(&midi, &opts).total_blocks;
        let from_factory = nbs_io::midi::import_midi_data(&midi, &factory).total_blocks;
        assert_eq!(
            from_baseline - from_factory,
            2,
            "基线比出厂多出来的应该就是那两个鼓音；对不上说明两份的差不止在鼓表"
        );
    }

    /// 预检和导入必须说同一句话：同一个文件、同一套选项，预测的形状就该是落地的形状，
    /// 而且预检一步都不碰工程。这里用导出器造的临时文件，测的是「两条工具走没走同一条算法」。
    #[test]
    fn inspect_midi_predicts_the_import_and_touches_nothing() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.ensure_layers(2);
        let painted = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 5, 48), cell(4, 0, 5, 50), cell(8, 1, 11, 43)],
            label: None,
            expect_rev: None,
        });
        assert!(painted.ok, "{:?}", painted.error);
        let dir = std::env::temp_dir().join(format!("nbs-control-midi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        sync_policy(&h.state, std::slice::from_ref(&dir), false);
        let path = dir.join("fixture.mid");
        nbs_io::midi::export_midi(&h.state.doc.song, &path).expect("临时 MIDI 写得出去");
        let raw = path.display().to_string();

        let rev_before = h.state.doc.song.rev;
        let preview = h.run(&Op::InspectMidi {
            path: raw.clone(),
            options: None,
            enhanced: None,
        });
        assert!(preview.ok, "{:?}", preview.error);
        assert_eq!(preview.report.rev, rev_before, "预检不能推进 rev");
        assert_eq!(preview.report.cells_changed, 0, "预检一格都没写");
        assert_eq!(preview.report.layers_changed, 0);
        let predicted = plan_of(&preview);

        let import = h.run(&Op::ImportMidi {
            path: raw,
            discard_changes: true,
            options: None,
            enhanced: None,
            remember: None,
        });
        assert!(import.ok, "{:?}", import.error);
        let achieved = plan_of(&import);
        assert_eq!(achieved, predicted, "预测和实得逐字段都得对上");
        assert_eq!(
            h.state.doc.song.total_blocks,
            achieved.notes,
            "回执报的音符数就是工程里的音符数"
        );
        assert_eq!(
            import.report.cells_changed,
            achieved.notes,
            "换整首歌的回执按新工程规模报变更，不是 0"
        );
        assert!(import.report.meta_changed, "速度和出生点也一起换了");
        assert_eq!(h.state.doc.song.layers.len(), achieved.layers);
        assert_eq!(h.state.doc.song.song_length(), achieved.length_ticks);
        let _ = std::fs::remove_file(&path);
    }

    /// 两道闸各管各的：目录白名单挡读得到读不到，`discard_changes` 挡「换歌要不要先认错」。
    /// 两条都在动工程之前，所以失败之后 `rev` 和谱面都得原样。
    #[test]
    fn midi_import_stops_at_the_directory_gate_and_the_acknowledgement_gate() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let dir = std::env::temp_dir().join(format!("nbs-control-midi-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.mid");
        std::fs::write(&path, nbs_io::midi::export_midi_bytes(&h.state.doc.song)).unwrap();
        let raw = path.display().to_string();
        let rev = h.state.doc.song.rev;

        // 文件是真的，但这条目录没放行——报的是「不在允许的目录里」，不是「读不懂」。
        let out = h.run(&Op::InspectMidi {
            path: raw.clone(),
            options: None,
            enhanced: None,
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed, "{:?}", out.error);
        let out = h.run(&Op::ImportMidi {
            path: raw.clone(),
            discard_changes: false,
            options: None,
            enhanced: None,
            remember: None,
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed, "{:?}", out.error);
        assert_eq!(h.state.doc.song.rev, rev, "被目录闸挡下不该动到 rev");

        sync_policy(&h.state, std::slice::from_ref(&dir), false);
        // 放行之后，脏工程上的导入仍要先点头：省略 `discard_changes` 就是「别动我的未存改动」。
        let painted = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 5, 48)],
            label: None,
            expect_rev: None,
        });
        assert!(painted.ok, "{:?}", painted.error);
        assert!(painted.report.dirty);
        let dirty_rev = h.state.doc.song.rev;
        let out = h.run(&Op::ImportMidi {
            path: raw.clone(),
            discard_changes: false,
            options: None,
            enhanced: None,
            remember: None,
        });
        assert_eq!(code(&out), ErrorCode::NotAllowed, "{:?}", out.error);
        // 两道闸用的是同一个错误码，靠话区分：这条要提「未保存改动」。
        let text = format!("{:?}", out.error);
        assert!(
            text.contains("discard_changes"),
            "该指认未保存改动这道闸：{text}"
        );
        assert_eq!(h.state.doc.song.rev, dirty_rev, "没点头就不能换歌");
        assert!(h.state.doc.song.dirty, "被拒的那一步连脏状态都不该动");

        let out = h.run(&Op::InspectMidi {
            path: raw,
            options: None,
            enhanced: None,
        });
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.report.rev, dirty_rev, "预检不放行第二次写，也不推进 rev");
        assert_eq!(h.state.doc.song.total_blocks, 1, "预检之后谱面还是那一个音符");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 整首替换（开档 / 导入 MIDI）会把撤销历史一起清空，于是「按历史算变更量」那条路
    /// 只能算出 0——回执看着和「什么都没动」一模一样。这里换成报新工程的规模。
    #[test]
    fn replacing_the_song_reports_the_incoming_size() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        h.ensure_layers(2);
        let painted = h.run(&Op::SetNotes {
            cells: vec![cell(0, 0, 5, 48), cell(4, 0, 5, 50), cell(8, 1, 11, 43)],
            label: None,
            expect_rev: None,
        });
        assert!(painted.ok, "{:?}", painted.error);
        assert_eq!(painted.report.cells_changed, 3, "普通写入仍按历史算");
        assert_eq!(painted.report.layers_changed, 0, "没动图层结构就是 0");

        let dir = std::env::temp_dir().join(format!("nbs-control-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        sync_policy(&h.state, std::slice::from_ref(&dir), false);
        let path = dir.join("three.nbs");
        let saved = h.run(&Op::SavePath {
            path: path.display().to_string(),
            version: None,
            overwrite: false,
        });
        assert!(saved.ok, "{:?}", saved.error);
        let cleared = h.run(&Op::ClearSong {
            expect_rev: None,
            discard_changes: true,
        });
        assert!(cleared.ok, "{:?}", cleared.error);
        assert_eq!(cleared.report.cells_changed, 3, "清空是改掉那三格，历史算得出来");

        let opened = h.run(&Op::OpenPath {
            path: path.display().to_string(),
            discard_changes: true,
        });
        assert!(opened.ok, "{:?}", opened.error);
        assert_eq!(
            opened.report.cells_changed,
            h.state.doc.song.total_blocks,
            "换歌的回执要说清落了几个音符，不能是 0"
        );
        assert_eq!(opened.report.cells_changed, 3);
        assert_eq!(opened.report.layers_changed, h.state.doc.song.layers.len());
        assert!(opened.report.meta_changed, "整首替换连速度和出生点一起换");
        assert_eq!(opened.report.history_len, 0, "替换不带撤销");
        assert!(!opened.report.dirty, "刚开出来的档和磁盘一致");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 覆盖项既要落到歌上，也要在回执里回显出来——回显的是生效值，不是省略稿。
    #[test]
    fn midi_overrides_land_on_the_song_and_echo_back() {
        let _prefs = crate::isolate_prefs();
        let h = Harness::new();
        let midi = midi_fixture();
        let baseline = midi_compute(&h.state, midi_fixture(), "m".to_string(), None, None, false)
            .expect("基线算得动");
        let with_override = midi_compute(
            &h.state,
            midi.clone(),
            "m".to_string(),
            Some(&ImportMidiOptions {
                channels: vec![ImportChannelMap {
                    channel: 0,
                    instrument: -1,
                    octave: 0,
                }],
                tail_parts: Some(vec![2]),
                precision: Some(3),
                ..Default::default()
            }),
            None,
            false,
        )
        .expect("覆盖算得动");
        let plan = &with_override.plan;
        assert_eq!(plan.applied.precision, Some(3));
        assert_eq!(
            plan.applied.channels.iter().find(|c| c.channel == 0).map(|c| c.instrument),
            Some(-1),
            "回显要写出真正生效的 -1，而不是调用方发过来的那份省略稿"
        );
        assert_eq!(
            plan.channels.iter().find(|c| c.channel == 0).map(|c| c.silent),
            Some(true)
        );
        assert!(plan.parts[2].tail, "通道 9 的声部也在 tail_parts 里点亮了");
        assert!(!plan.parts[0].tail, "没列出的声部以这份清单为准，全关");
        assert!(
            plan.silenced_notes >= 2,
            "整条通道忽略 → 那两个音符都算静音：{:?}",
            plan.silenced_notes
        );
        assert!(
            plan.notes < baseline.plan.notes,
            "关掉一条通道，落地的音符数必须真的少下来：{} vs {}",
            plan.notes,
            baseline.plan.notes
        );
        // 精度 4 倍 → 同一份 MIDI 摊得更长。
        assert!(plan.length_ticks > baseline.plan.length_ticks);
    }

    /// 非法覆盖要在**动工程之前**拒掉，并且点名是哪个字段不对。
    #[test]
    fn bad_midi_options_are_refused_before_the_song_moves() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.empty_song();
        let rev = h.state.doc.song.rev;
        let cases: Vec<(ImportMidiOptions, ErrorCode)> = vec![
            (
                ImportMidiOptions {
                    precision: Some(2),
                    ..Default::default()
                },
                ErrorCode::BadArgument,
            ),
            (
                ImportMidiOptions {
                    max_height: Some(0),
                    ..Default::default()
                },
                ErrorCode::OutOfRange,
            ),
            (
                ImportMidiOptions {
                    note_duration_fade_end: Some(101),
                    ..Default::default()
                },
                ErrorCode::OutOfRange,
            ),
            (
                ImportMidiOptions {
                    channels: vec![
                        ImportChannelMap {
                            channel: 3,
                            instrument: 5,
                            octave: 0,
                        },
                        ImportChannelMap {
                            channel: 3,
                            instrument: 6,
                            octave: 1,
                        },
                    ],
                    ..Default::default()
                },
                ErrorCode::BadArgument,
            ),
            (
                ImportMidiOptions {
                    channels: vec![ImportChannelMap {
                        channel: 9,
                        instrument: 5,
                        octave: 0,
                    }],
                    ..Default::default()
                },
                ErrorCode::BadArgument,
            ),
            (
                ImportMidiOptions {
                    drums: vec![ImportDrumMap {
                        note: 70,
                        instrument: 11,
                        key: 40,
                    }],
                    ..Default::default()
                },
                ErrorCode::BadArgument,
            ),
            (
                ImportMidiOptions {
                    drums: vec![ImportDrumMap {
                        note: 36,
                        instrument: 11,
                        key: 120,
                    }],
                    ..Default::default()
                },
                ErrorCode::OutOfRange,
            ),
            (
                ImportMidiOptions {
                    channels: vec![ImportChannelMap {
                        channel: 0,
                        instrument: 100,
                        octave: 0,
                    }],
                    ..Default::default()
                },
                ErrorCode::OutOfRange,
            ),
            (
                ImportMidiOptions {
                    tail_parts: Some(vec![99]),
                    ..Default::default()
                },
                ErrorCode::OutOfRange,
            ),
        ];
        for (options, want) in cases {
            let out = midi_effective(
                &h.state,
                &midi_fixture(),
                Some(&options),
                None,
                false,
            )
            .expect_err("这份覆盖该被拒");
            assert_eq!(out.code, want, "{options:?} 的错误码不对：{out:?}");
        }
        assert_eq!(h.state.doc.song.rev, rev, "校验不该碰工程");
    }

    /// `remember` 是唯一能动用户长期设置的开关，省略算 false；给了 true 才写回，
    /// 并且写回之后它真的变成下一次的基线。
    #[test]
    fn remembered_overrides_become_the_next_baseline() {
        let _prefs = crate::isolate_prefs();
        let h = Harness::new();
        let midi = midi_fixture();
        let patch = midi.channel_patch[0];
        let options = ImportMidiOptions {
            channels: vec![ImportChannelMap {
                channel: 0,
                instrument: 5,
                octave: -1,
            }],
            ..Default::default()
        };

        let before = h.state.midi_gm.patch(patch as u8);
        let (quiet, _) = midi_effective(&h.state, &midi, Some(&options), None, false).expect("算得动");
        assert_eq!(quiet.channel_ins[0], 5, "覆盖在本次是生效的，只是不写回");
        assert!(!quiet.remember, "省略 remember 就是别动长期设置");
        assert_eq!(h.state.midi_gm.patch(patch as u8), before, "没点头就不该写表");

        let computed = midi_compute(
            &h.state,
            midi.clone(),
            "m".to_string(),
            Some(&options),
            None,
            true,
        )
        .expect("算得动");
        let mut state = Harness::new().state;
        state.midi_gm = h.state.midi_gm.clone();
        state.midi_options = computed.opts;
        state.midi_options.finish_import(&midi, &mut state.midi_gm);
        assert_eq!(
            state.midi_gm.patch(patch as u8),
            (5, -1),
            "写回的是这次真正生效的那一对值"
        );
        // 写回之后，连「省略选项」的下一次基线也跟着变——这正是「记住」二字的含义。
        let next = midi_effective(&state, &midi, None, None, false).expect("算得动");
        assert_eq!((next.0.channel_ins[0], next.0.channel_octave[0]), (5, -1));
    }

    /// 增强导入的选项面只在 `enabled=true` 时存在：只给子字段会被拒，
    /// 免得调用方以为自己在调延音、其实走的是传统路径。
    #[test]
    fn enhanced_import_needs_an_explicit_yes_and_reports_its_own_counts() {
        let _prefs = crate::isolate_prefs();
        let h = Harness::new();
        let midi = midi_fixture();
        let half = ImportEnhancedOptions {
            sustain: Some(true),
            ..Default::default()
        };
        let refused = midi_effective(&h.state, &midi, None, Some(&half), false)
            .expect_err("没给 enabled 就该拒");
        assert_eq!(refused.code, ErrorCode::BadArgument, "{refused:?}");

        let computed = midi_compute(
            &h.state,
            midi,
            "m".to_string(),
            None,
            Some(&ImportEnhancedOptions {
                enabled: Some(true),
                range_mode: Some(ImportRangeMode::Fold),
                sustain: Some(true),
                channels: vec![ImportEnhancedChannel {
                    channel: 0,
                    transpose: Some(12),
                    slots: vec![ImportSlot {
                        instrument: Some(5),
                        volume: Some(80),
                    }],
                }],
                substitutes: vec![ImportSubstitution {
                    instrument: 0,
                    high: Some(5),
                    low: None,
                }],
                ..Default::default()
            }),
            false,
        )
        .expect("增强路径算得动");
        let plan = computed.plan;
        assert_eq!(plan.applied_enhanced.enabled, Some(true));
        assert!(plan.enhanced.is_some(), "走增强路径必带那六项计数");
        let channel = plan
            .applied_enhanced
            .channels
            .iter()
            .find(|c| c.channel == 0)
            .expect("回显含通道 0");
        assert_eq!(channel.transpose, Some(12), "手动移调不能被推荐位移顶掉");
        assert_eq!(channel.slots.len(), 3, "槽位按三个补齐，不是发过来几个回几个");
        assert_eq!(channel.slots[0].instrument, Some(5));
        assert_eq!(channel.slots[0].volume, Some(80));
        assert_eq!(
            channel.slots[1].instrument, None,
            "没给的槽位保持静音，别悄悄补个音色进去"
        );
        assert_eq!(plan.applied_enhanced.range_mode, Some(ImportRangeMode::Fold));
        assert_eq!(
            plan.applied_enhanced.substitutes.iter().find(|s| s.instrument == 0).map(|s| s.high),
            Some(Some(5))
        );
    }

    /// 下载那一下只能人自己在窗口里勾：三条要下载的都得它，报错要说清该谁去勾。
    /// 这条测试同时钉住「参数错排在闸前面」——`model` 写错时回执说的是参数而不是许可，
    /// 调用方才知道自己该改 JSON，而不是坐着等人。
    #[test]
    fn downloads_need_the_persons_own_tick() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        assert!(
            !h.state.transcription.accepted_downloads,
            "许可默认就是没勾，外部调用不许替人勾上"
        );
        for op in [
            Op::TranscribePrepareEnv { cuda: Some(false) },
            Op::TranscribePrepareModel {
                options: TranscribeOptions::default(),
            },
            Op::TranscribeAudio {
                audio: "a/b.wav".into(),
                options: TranscribeOptions::default(),
                save_midi: None,
                overwrite: false,
            },
        ] {
            let out = h.run(&op);
            assert!(!out.ok, "{op:?} 居然起了任务");
            assert_eq!(code(&out), ErrorCode::NotAllowed, "{op:?}");
            let message = out.error.unwrap().message;
            assert!(
                message.contains(&say("许可", "license")),
                "{op:?} 的拒绝没说是许可：{message}"
            );
        }
        let out = h.run(&Op::TranscribePrepareModel {
            options: TranscribeOptions {
                model: Some("giant".into()),
                ..Default::default()
            },
        });
        assert_eq!(code(&out), ErrorCode::BadArgument, "参数错该比闸先说");
        assert!(out.error.unwrap().message.contains("giant"));
        // 被拒的调用一条任务都不许起，不然转录的单飞闸会一直挡着后面合法的调用。
        let listed = match h.run(&Op::JobList).data {
            Some(Data::Jobs { jobs }) => jobs,
            other => panic!("JobList 回来的是 {other:?}"),
        };
        assert!(listed.is_empty(), "被拒的调用起了任务：{listed:?}");
    }

    /// 「省略 = 窗上那一份」是这条链的基线口径，和 MIDI 导入那条一样。
    /// 窗上的乐器组键与权重路径是两行文本，所以搬运必须走 `request_options()` 那一份实现——
    /// 拿出厂默认会得出另一种 device/beam，那正是最难查的一种「没生效」。
    #[test]
    fn omitted_transcribe_fields_keep_the_windows_own_values() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        {
            let t = &mut h.state.transcription;
            t.options.model = "small".into();
            t.options.device = "cpu".into();
            t.options.beam_size = 3;
            t.instruments = "acoustic_piano，voice , ".into();
            t.model_path = " D:/weights/custom.safetensors ".into();
        }
        let kept = transcribe_options(&h.state, &TranscribeOptions::default()).unwrap();
        assert_eq!(kept.model, "small");
        assert_eq!(kept.device, "cpu");
        assert_eq!(kept.beam_size, 3);
        assert_eq!(
            kept.instruments,
            vec!["acoustic_piano", "voice"],
            "逗号分隔（含全角）按窗上那一份解析，空项丢掉"
        );
        assert_eq!(
            kept.model_path.as_deref(),
            Some(Path::new("D:/weights/custom.safetensors")),
            "两侧空格是打字习惯，不是路径的一部分"
        );
        let merged = transcribe_options(
            &h.state,
            &TranscribeOptions {
                dtype: Some("float16".into()),
                device: Some("cuda".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(merged.dtype, "float16");
        assert_eq!(merged.device, "cuda");
        assert_eq!(merged.model, "small", "没给的字段不许被出厂默认顶掉");
        assert_eq!(merged.instruments, kept.instruments);
        // 空串是「显式清掉窗上那行」，和省略是两件事。
        let cleared = transcribe_options(
            &h.state,
            &TranscribeOptions {
                model_path: Some("  ".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cleared.model_path, None, "给了空串就该把窗上那行清掉");
    }

    /// 取值域在这儿钉死，起了子进程之后就晚了：worker 的 ValueError 到不了调用方手上，
    /// 只会变成几十秒后一句「任务失败」。
    #[test]
    fn transcribe_arguments_are_checked_before_a_subprocess() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        let cases: Vec<(TranscribeOptions, &str)> = vec![
            (TranscribeOptions { device: Some("gpu".into()), ..Default::default() }, "gpu"),
            (TranscribeOptions { dtype: Some("fp16".into()), ..Default::default() }, "fp16"),
            (TranscribeOptions { model: Some("tiny".into()), ..Default::default() }, "tiny"),
            (TranscribeOptions { beam_size: Some(0), ..Default::default() }, "beam_size"),
            (TranscribeOptions { beam_size: Some(5), ..Default::default() }, "beam_size"),
            (TranscribeOptions { temperature: Some(2.5), ..Default::default() }, "temperature"),
            (TranscribeOptions { cfg_coef: Some(-1.0), ..Default::default() }, "cfg_coef"),
            (
                TranscribeOptions { model_path: Some("D:/weights/model.bin".into()), ..Default::default() },
                "safetensors",
            ),
        ];
        for (given, wants) in cases {
            let err = transcribe_options(&h.state, &given)
                .err()
                .unwrap_or_else(|| panic!("{given:?} 居然收了"));
            assert!(
                matches!(
                    err.code,
                    ErrorCode::BadArgument | ErrorCode::OutOfRange | ErrorCode::NotFound
                ),
                "{given:?} 的码是 {:?}",
                err.code
            );
            assert!(err.message.contains(wants), "{given:?} → {}", err.message);
        }
        // 窗上已经填着的那行权重不重跑这套检查：那是人自己敲的，AI 没给就该照原样用。
        h.state.transcription.model_path = "D:/weights/model.onnx".into();
        assert!(transcribe_options(&h.state, &TranscribeOptions::default()).is_ok());
    }

    /// `export` 和转录交付共用一条「目标已存在就得认错」的闸，所以字面实现只有一份。
    #[test]
    fn a_delivery_target_needs_its_own_ack() {
        let dir = std::env::temp_dir();
        let taken = dir.join("nbs_transcribe_gate_taken.mid");
        let absent = dir.join("nbs_transcribe_gate_absent.mid");
        let _ = std::fs::remove_file(&absent);
        std::fs::write(&taken, b"MThd").unwrap();
        let err = overwrite_gate(&taken, false).err().expect("目标已存在又没认错");
        assert_eq!(err.code, ErrorCode::NotAllowed);
        assert!(err.message.contains(&say("覆盖", "overwrite")), "{}", err.message);
        assert!(overwrite_gate(&taken, true).is_ok());
        assert!(
            overwrite_gate(&absent, false).is_ok(),
            "不存在的目标不该要求认错"
        );
        let _ = std::fs::remove_file(&taken);
    }

    /// 环境没装好就别去要权重，而且拒绝的话要说清下一步是哪条工具，不是只说「不行」。
    /// 这里刻意把 `runtime.root` 指到临时目录之外：装没装是看真机那份 venv 的，
    /// 测试不许跟着「这台机器跑过转录」变。
    #[test]
    fn the_environment_comes_before_the_weights() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.state.transcription.accepted_downloads = true;
        h.state.transcription.runtime.root = std::env::temp_dir()
            .join("nbs_transcribe_env_gate_absent")
            .join("这一层也不该存在");
        assert!(!h.state.transcription.runtime.installed());
        for op in [
            Op::TranscribePrepareModel {
                options: TranscribeOptions::default(),
            },
            Op::TranscribeAudio {
                audio: String::new(),
                options: TranscribeOptions::default(),
                save_midi: None,
                overwrite: false,
            },
        ] {
            let out = h.run(&op);
            assert_eq!(code(&out), ErrorCode::NotAllowed, "{op:?}");
            let message = out.error.unwrap().message;
            assert!(
                message.contains("transcribe_prepare_env"),
                "{op:?} 的拒绝没点名下一步：{message}"
            );
        }
    }

    /// 「一次只跑一个」数的是任务表里的在册量。窗里那条在跑是另一半，
    /// 它要真的 `TranscriptionTask`，只能在真机验，这里不假装测过。
    #[test]
    fn one_transcription_at_a_time() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        h.state.transcription.accepted_downloads = true;
        // 不碰 python：占一个同 kind 的在册位就够把闸咬住。
        let slot = h
            .channel
            .jobs
            .spawn(JobKind::TranscribeAudio.as_str(), |_| {
                std::thread::sleep(Duration::from_millis(400));
                Ok(json!(null))
            })
            .expect("任务表该收得下这条");
        let out = h.run(&Op::TranscribePrepareEnv { cuda: Some(false) });
        assert_eq!(code(&out), ErrorCode::Busy, "{out:?}");
        assert!(out
            .error
            .unwrap()
            .message
            .contains(&say("一次只跑一个", "only one runs at a time")));
        let _ = slot;
    }
}
