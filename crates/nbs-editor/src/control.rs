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

use egui::Context;
use serde_json::{json, Value};

use nbs_control::task::{CancelOutcome, SpawnReject};
use nbs_control::{
    ActionInfo, BatchField, CellNote, ChangeReport, ClientBoard, ControlServer, Data, Diagnostics,
    Discovery, ErrorCode, ErrorInfo, HistoryInfo, Inbox, InstrumentInfo, JobHandle, JobInfo,
    JobRegistry, JobStatus, LayerInfo, NoteCell, Op, Outcome, Playback, Scope, SelectTarget,
    SelectionInfo, ServerConfig, SetMeta, Snapshot, PROTOCOL_VERSION,
};
use nbs_core::model::{Note, Song, FIRST_CUSTOM_INDEX, KEY_MAX, PAN_MAX, PIT_MAX, VEL_MAX};

use crate::commands::Command;
use crate::state::{EditorState, Flag};

/// 一条请求最多写多少格。AI 一次生成上万音符是正常活，但让主线程一帧卡死不值得。
const MAX_CELLS_PER_CALL: usize = 50_000;
/// `GetNotes` 一次最多回多少格，截断会显式标出来。
const MAX_CELLS_RETURNED: usize = 20_000;
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
        let outcome = execute(state, ctx, &channel, &req.op);
        req.reply(outcome);
    }
    n
}

// ------------------------------------------------------------------ 分发

fn execute(state: &mut EditorState, ctx: &Context, ch: &ControlChannel, op: &Op) -> Outcome {
    match op {
        Op::Ping => Outcome::ok(
            Data::Pong {
                app_version: ch.app_version.clone(),
                protocol: ch.protocol,
                rev: state.doc.song.rev,
                song_session: state.song_session,
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
            Data::Snapshot(Box::new(snapshot(state, ch))),
            String::new(),
        ),
        Op::GetNotes {
            tick_from,
            tick_to,
            layer_from,
            layer_to,
        } => match read_notes(state, *tick_from, *tick_to, *layer_from, *layer_to) {
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
                instruments: instrument_infos(state),
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
        other => mutate(state, ctx, ch, other),
    }
}

// ------------------------------------------------------------------ 写路径

fn mutate(state: &mut EditorState, ctx: &Context, ch: &ControlChannel, op: &Op) -> Outcome {
    // 导出只是给歌曲拍快照再交给后台线程，不改状态，所以不受弹窗闸约束。
    if !matches!(op, Op::Export { .. }) {
        if let Err(e) = gate(state, op) {
            return fail(state, e);
        }
    }
    let stamp = Stamp::capture(state);
    match write(state, ctx, ch, op) {
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
                    "界面上还挂着没答完的确认框（{titles}）。外部写操作会被界面丢掉，先读 GetSnapshot，再用 RunAction answer_modal 回答，或者让人处理。"
                ),
                format!(
                    "Unanswered dialog(s) on screen ({titles}); writes would be dropped by the UI. Read GetSnapshot, then RunAction answer_modal, or let a human handle it."
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
}

impl Stamp {
    fn capture(state: &EditorState) -> Self {
        Self {
            history_len: state.doc.history.len(),
            cursor: state.doc.history.cursor(),
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
        let (cells, layers, meta) = edit_totals(state, from, to);
        ChangeReport {
            cells_changed: cells,
            layers_changed: layers,
            meta_changed: meta,
            rev: state.doc.song.rev,
            song_session: state.song_session,
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
        song_session: state.song_session,
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
                format!("工程已经变了：你给的 expect_rev={want}，现在 rev={rev}。重新 GetNotes/GetSnapshot 再决定。"),
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
fn write(state: &mut EditorState, ctx: &Context, ch: &ControlChannel, op: &Op) -> Written {
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
        Op::Select { target } => {
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
            run_command(state, ctx, cmd);
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
            run_command(state, ctx, Command::Deselect);
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
                run_command(state, ctx, cmd);
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
            run_command(state, ctx, Command::RemoveLayer(*index));
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
            state.sync_loop_params();
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
        Op::Undo { steps } => roll(state, ctx, *steps, true),
        Op::Redo { steps } => roll(state, ctx, *steps, false),
        Op::ClearSong {
            expect_rev,
            discard_changes,
        } => {
            rev_check(*expect_rev, state.doc.song.rev)?;
            keep_or_replace(state, *discard_changes, ("清空", "Clearing the song"))?;
            run_command(state, ctx, Command::ClearSong);
            Ok((None, say("音符已全部清空", "All notes cleared")))
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
        } => {
            let target = resolve_path(state, ch, path, &["mid", "midi"], true)?;
            keep_or_replace(state, *discard_changes, ("导入 MIDI", "Importing MIDI"))?;
            // 用 MIDI 导入窗的默认档位（原版 control_create 那一套），
            // 精确度、通道映射都不在这里改，需要的人先 RunAction midi_import_*。
            let opts = nbs_io::midi::MidiImportOptions::default();
            let song = nbs_io::midi::import_midi_path(&target, &opts)
                .map_err(|e| ErrorInfo::new(ErrorCode::Io, e))?;
            let notes = song.total_blocks;
            let layers = song.layers.len();
            apply_replacement(state, song);
            Ok((
                Some(Data::File {
                    path: target.display().to_string(),
                    file_kind: "midi-import".into(),
                    notes,
                }),
                say(
                    format!("MIDI 导入完成：{notes} 个音符、{layers} 层（用的是导入窗默认选项）"),
                    format!("MIDI imported: {notes} notes across {layers} layers (default window options)"),
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
            "音色 {instrument} 不存在：内置 0..{}，自定义从 {FIRST_CUSTOM_INDEX} 起，本工程只有 {customs} 个自定义音色。先 GetInstruments。",
            FIRST_CUSTOM_INDEX - 1
        ),
        format!(
            "Instrument {instrument} does not exist: built-ins 0..{}, customs start at {FIRST_CUSTOM_INDEX} and this song has {customs}. Call GetInstruments first.",
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
    state.song_session += 1;
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

fn roll(state: &mut EditorState, ctx: &Context, steps: Option<u32>, undo: bool) -> Written {
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
        run_command(state, ctx, if undo { Command::Undo } else { Command::Redo });
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
            run_command(state, ctx, Command::PlayPause);
            Ok((None, say("开始播放", "Playback started")))
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
    tick_from: u32,
    tick_to: u32,
    layer_from: Option<u32>,
    layer_to: Option<u32>,
) -> Result<(Data, String), ErrorInfo> {
    let song = &state.doc.song;
    let layers = song.layers.len() as u32;
    if tick_to < tick_from {
        return Err(bad(
            "tick_to 不能小于 tick_from",
            "tick_to must be >= tick_from",
        ));
    }
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
    let raw = song.notes_in_rect(tick_from, lf, tick_to, lt);
    let total = raw.len();
    let truncated = total > MAX_CELLS_RETURNED;
    let cells = raw
        .into_iter()
        .take(MAX_CELLS_RETURNED)
        .map(|(tick, layer, note)| note_cell(tick, layer, note))
        .collect();
    let message = if truncated {
        say(
            format!("范围里共 {total} 格，只回了前 {MAX_CELLS_RETURNED} 格"),
            format!("Range holds {total} cells, returning the first {MAX_CELLS_RETURNED}"),
        )
    } else {
        String::new()
    };
    Ok((
        Data::Notes {
            cells,
            truncated,
            limit: MAX_CELLS_RETURNED,
        },
        message,
    ))
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

fn instrument_infos(state: &EditorState) -> Vec<InstrumentInfo> {
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
            sample_ready: Some(sample_ready(&bank, index as usize)),
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

fn snapshot(state: &EditorState, ch: &ControlChannel) -> Snapshot {
    let song = &state.doc.song;
    Snapshot {
        app_version: ch.app_version.clone(),
        protocol: ch.protocol,
        path: song.filename.as_ref().map(|p| p.display().to_string()),
        dirty: song.dirty,
        rev: song.rev,
        song_session: state.song_session,
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
        playhead: state.audio.position_ticks(),
        playing: state.audio.is_playing(),
        selection: Box::new(selection_info(&state.doc.selection)),
        diagnostics: Box::new(diagnostics(state)),
        modal_count: state.modals.items.len(),
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
            description: "回答界面当前那个确认框（yes=true）。等于替人按按钮，默认关掉",
            args: Some(r#"{"yes":true}"#),
            mutating: false,
            danger: true,
            build: |_state, args| Ok(Some(Command::AnswerModal(arg_bool(args, "yes")?))),
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
            description: "退出程序。界面那颗「退出」是不问就关的，所以这里在有未保存改动时先拒；确认要丢就带 discard_changes=true。默认关掉",
            args: Some(r#"{"discard_changes":true}"#),
            mutating: false,
            danger: true,
            build: |state, args| {
                keep_or_replace(
                    state,
                    arg_bool_or(args, "discard_changes")?,
                    ("退出", "Quitting"),
                )?;
                Ok(Some(Command::Exit))
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
            format!("未知动作 {name:?}。用 ListActions 看可用动作。"),
            format!("Unknown action {name:?}. See ListActions."),
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
    if target.exists() && !overwrite {
        return Err(ErrorInfo::new(
            ErrorCode::NotAllowed,
            say(
                format!(
                    "目标已存在：{}。确认覆盖就带 overwrite=true。",
                    target.display()
                ),
                format!(
                    "Target exists: {}. Pass overwrite=true to replace it.",
                    target.display()
                ),
            ),
        ));
    }
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
        .map_err(|reject| match reject {
            SpawnReject::Busy => ErrorInfo::new(
                ErrorCode::Busy,
                say(
                    "并发任务已满，先用 JobList 看看有什么在跑，或 JobCancel 掉一个。",
                    "All job slots are busy; check JobList or cancel one first.",
                ),
            ),
            SpawnReject::Thread(message) => ErrorInfo::new(ErrorCode::Internal, message),
        })?;
    let info = ch.jobs.status(id).unwrap_or_else(|| JobInfo {
        id,
        kind: "export".into(),
        status: JobStatus::Queued,
        progress: 0.0,
        message: String::new(),
        started_unix: 0,
        finished_unix: None,
        result: None,
    });
    Ok((
        Some(Data::Job(Box::new(info))),
        say(
            format!(
                "已起 {fmt} 导出任务（快照 {notes} 个音符）→ {}；用 JobStatus 轮询",
                target.display(),
                fmt = format_tag(format)
            ),
            format!(
                "Export job started ({notes} notes snapshotted) → {}; poll with JobStatus",
                target.display()
            ),
        ),
    ))
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
    use crate::state::{Modal, WindowKind};
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
            execute(&mut self.state, &self.ctx, &self.channel, op)
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

    fn code(out: &Outcome) -> ErrorCode {
        out.error.as_ref().expect("失败回执必带错误码").code
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

        let out = h.run(&Op::Undo { steps: None });
        assert!(out.ok, "{:?}", out.error);
        assert!(
            h.note(0, 0).is_none() && h.note(1, 0).is_none(),
            "两条音符要一起退回"
        );
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
            yes_no: true,
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

        let out = h.run(&Op::Undo { steps: None });
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

    #[test]
    fn ping_and_snapshot_carry_the_current_rev() {
        let _prefs = crate::isolate_prefs();
        let mut h = Harness::new();
        let out = h.run(&Op::Ping);
        let Data::Pong { rev, .. } = out.data.expect("Ping 必带 Pong") else {
            panic!("Ping 应回 Pong");
        };
        assert_eq!(rev, h.state.doc.song.rev);
        let snap = snapshot(&h.state, &h.channel);
        assert_eq!(snap.rev, h.state.doc.song.rev);
        assert_eq!(snap.layers, h.state.doc.song.layers.len());
    }
}
