//! Open Note Block Studio — Rust 重写版桌面入口 + 无界面批处理 CLI。
//!
//! ```text
//! cargo run -p nbs-app                                 # 打开演示曲（GUI）
//! cargo run -p nbs-app -- path/to/song.nbs            # 直接打开 .nbs
//! cargo run -p nbs-app -- --info song.nbs             # 打印歌曲信息
//! cargo run -p nbs-app -- --export-wav song.nbs out.wav
//! cargo run -p nbs-app -- --export-midi song.nbs out.mid
//! cargo run -p nbs-app -- --export-png song.nbs out.png
//! cargo run -p nbs-app -- --export-schematic song.nbs out.schematic
//! cargo run -p nbs-app -- song.nbs --control            # 界面 + 外部控制（127.0.0.1）
//! ```
//! 默认使用 glow (OpenGL) 后端；在根 Cargo.toml 的
//! `workspace.dependencies.eframe.features` 改成 `wgpu` 即可切换，
//! 网格画布只依赖 `egui::Painter`，无需改动任何编辑器代码。

// 双击 exe 不该先闪一个黑色命令行框，所以 Windows 上按 GUI 子系统编译。
// 命令行用法不受影响：从终端/cargo 启动时 stdout、stderr 句柄是继承来的，
// 打印照样回到那个终端，只是 shell 不会等 GUI 退出。
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use eframe::egui;
use nbs_editor::commands::Command;
use nbs_editor::state::QuitMode;
use nbs_editor::EditorState;

#[cfg(windows)]
mod mc_fetch;
#[cfg(windows)]
mod midi_win;
#[cfg(windows)]
mod native_dialog;
#[cfg(windows)]
mod update;
#[cfg(windows)]
mod window_icon;

struct NbsApp {
    state: EditorState,
    last_title: String,
    /// `--control` 起起来的回环服务。留着的唯一理由是要在退出时 `stop()`：
    /// 发现文件（端口 + token）必须跟着进程一起消失，否则桥接进程会去连一个死端口。
    /// 设置页上的开关也改这个字段——那一趟的同意是命令行给的，收走得由这里下手。
    control: Option<nbs_editor::control::ControlStart>,
    /// 启动旗标 `--control-dir` 给的那些目录。设置页反复开关服务时要把它们带回去：
    /// 那是命令行上的一次同意，不该因为人手滑关了一次通路就消失。
    control_cli_dirs: Vec<PathBuf>,
    /// Windows 上「旧图标」真正落到任务栏的那两枚句柄，见 `window_icon` 模块。
    #[cfg(windows)]
    icon_handles: [Option<usize>; 2],
    /// 已经交给窗口的图标档位；`None` = 还没成功过，下一帧重试。
    #[cfg(windows)]
    icon_applied: Option<bool>,
}

impl NbsApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 语言要在建状态之前定好：演示曲描述、下面这条启动就写进状态栏的提示，
        // 都是「生成那一刻」定型的（原版同样在 load_settings 里先把 language 读好）。
        nbs_editor::set_language(nbs_io::prefs::Preferences::load().language);
        let mut state = EditorState::new();
        // 文件选择交给系统组件（原版的选框就是 Win32 的），编辑器只提请求。
        #[cfg(windows)]
        {
            state.picker = Some(std::sync::Arc::new(native_dialog::WinFileDialogPicker));
            // 实时 MIDI：winmm 后端 + 开机枚举一遍设备（原版
            // control_create.gml:398 的 midi_refresh_device()）。
            let midi = std::sync::Arc::new(midi_win::WinMidiIn::new());
            state.live_midi.backend = Some(midi);
            state.live_midi.refresh_devices();
            // 导入音效向导的 HTTP 腿：WinINet（原版 http_get / http_get_file 同底层）。
            state.sound_import.fetcher = Some(std::sync::Arc::new(mc_fetch::WinMinecraftFetcher));
        }
        state.theme.apply(&cc.egui_ctx);
        // 必须在首帧渲染前装入：set_fonts 下一帧生效，而首帧就会画到粗体族
        // （黑键标签），默认字体表没有这个族，epaint 会直接 panic。
        // creator 在第一次 pass 之前运行，这里调用正好赶上首帧。
        if !nbs_editor::fonts::install(&cc.egui_ctx) {
            eprintln!("警告: 本机找不到中文字体，界面里的中文会显示成空框");
        }

        // 原版 create 事件的更新检查（control_create.gml:593-616）：开关开着才发请求，
        // check_prerelease 决定拉预发布列表还是正式版；后台线程写共享态，UI 帧轮询。
        #[cfg(windows)]
        if state.prefs.check_update {
            update::spawn(
                state.prefs.check_prerelease,
                nbs_editor::VERSION.to_string(),
                state.update_check.clone(),
            );
        }

        if let Some(arg) = std::env::args().nth(1) {
            let path = PathBuf::from(arg);
            if path.extension().and_then(|s| s.to_str()) == Some("nbs") {
                // 原版启动文件走 load_song(fn, 0, 1)：替换第 1 个标签，取消则保留原曲。
                nbs_editor::commands::dispatch(
                    &mut state,
                    Command::OpenReplacingFirst(Some(path)),
                    &cc.egui_ctx,
                );
            }
        }
        // 外部控制：命令行、环境变量、设置页三层都没点名就一行代码都不多跑
        // （`state.control` 保持 None，每帧的 `control::drain` 等于空判断）。
        // 服务只绑 127.0.0.1，发现文件里的 token 就是全部的访问控制。
        let launch = resolve_control(
            &std::env::args().skip(1).collect::<Vec<_>>(),
            |key| std::env::var(key).ok(),
            &state.prefs,
        );
        let cli_dirs = launch.as_ref().map(|req| req.cli_dirs.clone()).unwrap_or_default();
        let control = launch
            .as_ref()
            .and_then(|req| start_control(&mut state, &cc.egui_ctx, req, true));

        Self {
            state,
            last_title: String::new(),
            control,
            control_cli_dirs: cli_dirs,
            #[cfg(windows)]
            icon_handles: [None, None],
            #[cfg(windows)]
            icon_applied: None,
        }
    }
}

/// 起服务，并把这一串的副作用（白名单、状态栏日志、面板要显示的那几项）落到 `state` 上。
/// 起不来返回 `None`：链路没挂上，界面照常能用，但原因必须写出来——面板上
/// 「设置开着却没在听」那一行就是靠它解释的。
/// `announce_source` = false 用在人刚在面板上点「启用」的那一次：那一下不必回报来源。
fn start_control(
    state: &mut EditorState,
    ctx: &egui::Context,
    req: &ControlRequest,
    announce_source: bool,
) -> Option<nbs_editor::control::ControlStart> {
    match nbs_editor::control::start(state, ctx, req.port, true, req.allow_danger) {
        Ok(start) => {
            let info = nbs_editor::control::ControlInfo {
                port: start.port,
                discovery_path: start.discovery_path.clone(),
                source: source_label(req.source),
                allow_danger: req.allow_danger,
                cli_dirs: req
                    .cli_dirs
                    .iter()
                    .map(|d| d.to_string_lossy().into_owned())
                    .collect(),
            };
            // 白名单在这一处一次算全（种子目录 + 旗标 + 设置页）；面板改目录时走同一条函数。
            nbs_editor::control::sync_policy(state, &req.cli_dirs, info.allow_danger);
            state.log_strs.push(start.log.clone());
            // 「是谁让我开的」只有这一层知道（三层判定的落点都在 nbs-app），
            // 所以补在编辑器那条监听日志后面，而不是往 control::start 的签名里塞来源。
            // 命令行那条路不补：旗标就是敲命令的人自己打的，再复述一遍是噪音。
            let why = match req.source {
                ControlSource::Cli => None,
                ControlSource::Env => Some(nbs_editor::tr(
                    "Source: the NBS_RUST_CONTROL environment variable. Untick it in Settings, or set it to 0.",
                    "来源：环境变量 NBS_RUST_CONTROL。想关掉就在窗口里取消勾选，或把它设成 0。",
                )),
                ControlSource::Prefs => Some(nbs_editor::tr(
                    "Source: Settings - External control (MCP). Whether dangerous actions are callable is decided by that same page.",
                    "来源：设置页「外部控制（MCP）」。危险动作是否可调同样只看那一栏的勾选。",
                )),
            };
            if announce_source {
                if let Some(why) = why {
                    state.log_strs.push(why.to_string());
                }
            }
            state.control_info = Some(info);
            Some(start)
        }
        Err(e) => {
            // GUI 启动时终端未必接得上，两条路都写一遍。
            state.set_message(e.clone());
            eprintln!("{e}");
            state.control_info = None;
            None
        }
    }
}

/// 外部控制的开启是谁给的。设置页要说这句话，排查「本机为什么在监听」时靠它定位
/// 该改哪一层——三层都能开，关掉的地方和开的地方不是同一个地方时，症状是关不掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlSource {
    /// 命令行旗标。`main` 里判断「这条命令是给 CLI 还是给 GUI」时只认这一层。
    Cli,
    /// 环境变量 `NBS_RUST_CONTROL`：只回答开不开，给不出端口、目录，也永远给不出危险动作。
    Env,
    /// 设置页里存下来的开关（`Preferences::control_enabled`）。
    Prefs,
}

/// `--control[=端口]` 与 `--control-danger`：端口自加的外部控制入口，原版没有对应物。
struct ControlRequest {
    /// 0 = 让系统挑端口，真实端口由发现文件带出来。
    port: u16,
    /// 已经判好的**生效值**：`--control-danger` 或设置页那条勾选，有一个就算。
    /// 环境变量永远进不了这一项（A1 拍板：它能开通路，能给读写目录，不能给「替你退出」）。
    allow_danger: bool,
    /// `--control-dir=<目录>` 给的那些，可重复。设置页那栏目录不在这里——白名单由
    /// `control::sync_policy` 现场汇总（种子目录 + 这里 + prefs），两处各拼一份就会有两份真相。
    cli_dirs: Vec<PathBuf>,
    source: ControlSource,
}

/// 标记出现在参数列表的任意位置都算，和原版 `-player` 的判定方式一致。
/// 只看命令行，不看环境变量和 prefs：CLI 分支要用它判断「`--control` 不是条子命令」，
/// 那里要是掺进 prefs，装了设置页开关之后 `--export` 会去打窗口而不是干活。
fn control_request(args: &[String]) -> Option<ControlRequest> {
    let mut port = 0;
    let mut allow_danger = false;
    let mut dirs = Vec::new();
    let mut wanted = false;
    for arg in args {
        if arg == "--control" {
            wanted = true;
        } else if let Some(text) = arg.strip_prefix("--control=") {
            wanted = true;
            match text.parse::<u16>() {
                Ok(p) => port = p,
                Err(e) => eprintln!(
                    "警告: --control={text} 不是端口号（{e}），改用系统分配的端口，以发现文件里的为准"
                ),
            }
        } else if arg == "--control-danger" {
            allow_danger = true;
        } else if let Some(text) = arg.strip_prefix("--control-dir=") {
            dirs.push(std::path::PathBuf::from(text));
        }
    }
    // 只给 --control-danger / --control-dir 不开 --control 等于没服务可连，这里不当作开启。
    wanted.then_some(ControlRequest {
        port,
        allow_danger,
        cli_dirs: dirs,
        source: ControlSource::Cli,
    })
}

/// 这一趟到底开不开外部控制，以及按哪些口径开。优先级：命令行 > 环境变量 > prefs。
///
/// 环境变量排在前是为了给「临时按住」留口子：设置页开着时给一次 `NBS_RUST_CONTROL=0`
/// 就不必先进窗口取消勾选。代价是它只能授予「启用」——危险动作（退出程序、
/// 替你答确认框、开外链）只在命令行旗标和设置页那一条勾选上说话，
/// 环境变量够不到（进程环境常常是从 shell 配置文件里带出来的，改那里的门槛比在窗口里
/// 点一下同意低得多，能给的话等于把同意面架空了）。
///
/// 这里只算「要不要起、按什么口径起」。白名单目录不在这里拼：那一份由
/// `control::sync_policy` 在起服务时现场汇总（种子目录 + 旗标 + 设置页），
/// 因为设置页改目录要当场生效，启动时拼一次会拼出第二份真相。
fn resolve_control(
    args: &[String],
    mut env: impl FnMut(&str) -> Option<String>,
    prefs: &nbs_io::prefs::Preferences,
) -> Option<ControlRequest> {
    let mut req = match control_request(args) {
        Some(cli) => cli,
        None => {
            // 环境变量排在前：它能把设置页的同意临时顶掉（`NBS_RUST_CONTROL=0` 就是
            // 「这次别开」的应急口）。值认不出来时已经吼过一声，这里退到 prefs。
            let switched = env_control_switch(env("NBS_RUST_CONTROL"));
            if !switched.unwrap_or(prefs.control_enabled) {
                return None;
            }
            ControlRequest {
                port: 0,
                allow_danger: false,
                cli_dirs: Vec::new(),
                source: match switched {
                    Some(_) => ControlSource::Env,
                    None => ControlSource::Prefs,
                },
            }
        }
    };
    // danger 是机器级的一次同意：设置页勾了，任何一层开起来的会话都带上它。
    req.allow_danger |= prefs.control_danger;
    Some(req)
}

/// 面板上「来源：…」那半句。三层各给一种说法，排查时不用再猜。
fn source_label(source: ControlSource) -> String {
    match source {
        ControlSource::Cli => nbs_editor::tr("--control on the command line", "命令行旗标 --control"),
        ControlSource::Env => nbs_editor::tr(
            "the NBS_RUST_CONTROL environment variable",
            "环境变量 NBS_RUST_CONTROL",
        ),
        ControlSource::Prefs => nbs_editor::tr("this Settings page", "设置页这一栏"),
    }
    .to_string()
}

/// `NBS_RUST_CONTROL` 的值 → 开 / 关 / 不作数。认 1、true、yes、on 和 0、false、no、off
/// （大小写和首尾空格随意）。认不出值时返回 `None` 并吼一声：静悄悄退回 prefs 会让
/// 拼错的人以为环境变量生效了，回头又去查设置页。
fn env_control_switch(value: Option<String>) -> Option<bool> {
    let raw = value?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        other => {
            eprintln!("警告: NBS_RUST_CONTROL={other:?} 不是 0/1 这类值，按没给这个变量处理");
            None
        }
    }
}


/// 把当前 GL 上下文的换帧间隔改成 `on` 要的档（= 垂直同步）。
/// Windows 上取 `wglSwapIntervalEXT`：它要求调用线程有可用的 GL 上下文，
/// eframe 的 glow 后端正是在 UI 线程上建上下文、且 `App::update` 期间一直是当前上下文。
/// 拿不到入口（非 Windows、驱动不支持）返回 `false`，调用方据此说明「下次启动生效」。
#[cfg(windows)]
fn set_swap_interval(on: bool) -> bool {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    type SwapIntervalFn = unsafe extern "system" fn(i32) -> i32;

    #[link(name = "opengl32")]
    extern "system" {
        fn wglGetProcAddress(name: *const i8) -> *const c_void;
    }

    fn lookup() -> Option<SwapIntervalFn> {
        let name = b"wglSwapIntervalEXT\0";
        let raw = unsafe { wglGetProcAddress(name.as_ptr().cast::<i8>()) };
        // 失败时返回 NULL 或 (HPROC)-1，MSDN 规定小于 4096 的返回值一律算无效。
        if raw.is_null() || (raw as usize) < 4096 {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*const c_void, SwapIntervalFn>(raw) })
        }
    }

    // 只缓存成功那一次：首帧之前没有上下文，缓存 None 会把后面所有切换都判死。
    static FN: OnceLock<Option<SwapIntervalFn>> = OnceLock::new();
    let f = match FN.get() {
        Some(cached) => *cached,
        None => {
            let found = lookup();
            if found.is_some() {
                let _ = FN.set(found);
            }
            found
        }
    };
    match f {
        Some(f) => unsafe { f(i32::from(on)) != 0 },
        None => false,
    }
}

/// 非 Windows 没有 `wglSwapIntervalEXT`：只走 `NativeOptions.vsync`（下次启动生效）。
#[cfg(not(windows))]
fn set_swap_interval(_on: bool) -> bool {
    false
}

impl Drop for NbsApp {
    fn drop(&mut self) {
        // 退出时持久化主题、音频参数、最近文件等偏好
        self.state.save_prefs();
        // 显式 stop 而不只是丢句柄：要把 accept 线程收干净，发现文件才会保证不骗人。
        if let Some(start) = &mut self.control {
            start.server.stop();
        }
    }
}

impl eframe::App for NbsApp {
    /// 原生选框在自己的调用里泵 Win32 消息循环，绝不能开在 egui 帧中间；
    /// eframe 在 `ctx.run` 之前调这条钩子，正好用来把上一帧攒下的选框请求变成系统对话框。
    /// 对话框是模态的，函数返回时用户已经选完或取消，下一条命令进 `state.pending`。
    fn raw_input_hook(&mut self, ctx: &egui::Context, _raw_input: &mut egui::RawInput) {
        // 对话框在 `Show` 里自己把输入吃掉了，egui 这边看不到任何事件，
        // 选完之后不会自然来一帧 —— 主动催一次，`DialogConfirm` 才有人处理。
        if self.state.run_pending_dialog() {
            ctx.request_repaint();
        }
    }

    /// 原版 `control_draw.gml:153` 的 `draw_clear(window_background)`。
    /// eframe 默认清成透明，glow 后端下就是一整片黑，凡是自己没重绘的面板都露黑底。
    /// theme 3 开了背景强调时先铺一整窗 `accent[3]`（`control_draw.gml:69`）。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = if self.state.background_accent
            && self.state.theme.kind == nbs_editor::ThemeKind::Fluent
        {
            self.state.theme.accent(3)
        } else {
            self.state.theme.window_bg
        };
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 原生窗口关闭先拦住：原版 control_end 不可取消，但逐标签问完仍要保留失败恢复文件。
        // eframe 允许用 CancelClose 把退出推迟到这些决定办完；已确认的退出不再拦。
        if ctx.input(|i| i.viewport().close_requested()) && !self.state.quit_confirmed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.state.emit(Command::BeginQuit(QuitMode::NativeClose));
        }
        // 「旧图标」开关切换的那一帧，`EditorState` 才把 egui 的图标命令发出去，而这条命令要等
        // 帧末才冲刷给平台层；winit 收到后只写 ICON_SMALL，还是没缩放的 256x256。所以这里以
        // `applied_window_icon`（= 上一帧确实发过命令）为准，晚一帧再写 WM_SETICON，
        // 这样大小图标都是我们按系统度量缩放过的版本。
        #[cfg(windows)]
        {
            if let Some(want) = self.state.applied_window_icon {
                if self.icon_applied != Some(want) {
                    if let Some(icon) = self.state.icons.window_icon(want) {
                        if window_icon::apply(&icon, &mut self.icon_handles) {
                            self.icon_applied = Some(want);
                        }
                    }
                }
            }
        }

        let title = self.state.song_title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        nbs_editor::show(ctx, &mut self.state);
        // 面板这一帧提的起停要求要画完之后再办：`control::start` 吃 `&mut EditorState`，
        // 而帧内它正被借去画界面；起 socket、写发现文件也不该发生在绘制路径里。
        if let Some(change) = self.state.control_change.take() {
            self.apply_control_change(change, ctx);
        }
        // 用户要求 #119：首选项那颗「垂直同步」。eframe 只在建窗时读
        // `NativeOptions.vsync`，运行中没有任何公开入口，所以这里直接改当前 GL
        // 上下文的换帧间隔；改不动（非 Windows、驱动不认）时设置也已经落盘，
        // 下次启动由上面的 NativeOptions 接上——状态栏那句话按实际结果说。
        if let Some(on) = self.state.vsync_request.take() {
            let done = set_swap_interval(on);
            self.state.set_message(if done {
                nbs_editor::tr(
                    if on {
                        "VSync on: each frame waits for the display refresh."
                    } else {
                        "VSync off: frames reach the screen as soon as they are drawn."
                    },
                    if on {
                        "垂直同步已打开：每帧等显示器刷新后再上屏。"
                    } else {
                        "垂直同步已关闭：画完即上屏，节拍只由帧率限制决定。"
                    },
                )
            } else {
                nbs_editor::tr(
                    "VSync setting saved; it applies to the next window this program opens.",
                    "垂直同步设置已保存；这台机器改不了当前窗口的同步，下次启动生效。",
                )
            });
            ctx.request_repaint();
        }
    }
}

impl NbsApp {
    /// 设置页那三个动作的落点。编辑器只提要求，起停在这里做：服务端句柄归本进程管，
    /// 退出时要在 `Drop` 里 `stop()`，发现文件才不会留下骗人的旧端口。
    fn apply_control_change(
        &mut self,
        change: nbs_editor::control::ControlChange,
        ctx: &egui::Context,
    ) {
        use nbs_editor::control::ControlChange as C;
        match change {
            C::Disable => {
                if let Some(mut start) = self.control.take() {
                    start.server.stop();
                    self.state.log_strs.push(
                        nbs_editor::tr(
                            "External control stopped listening (discovery file removed).",
                            "外部控制已停止监听（发现文件已删）。",
                        )
                        .to_string(),
                    );
                }
                // 没在听的时候关掉句柄不存在，显示位也要跟着清：留着就是面板上
                // 「端口 1234」和「没监听」同时出现的那种假状态。
                self.state.control_info = None;
            }
            C::Enable => {
                if self.control.is_some() {
                    // 已经在听了。这条只会在「勾选框本来就该是勾着」时到来，重起一遍
                    // 只会换一个端口号，把正连着的那个客户端踢掉。
                    return;
                }
                let req = ControlRequest {
                    port: 0,
                    // 面板碰过之后 danger 以设置页为准：命令行那一次同意不该盖过人亲手的一下。
                    allow_danger: self.state.prefs.control_danger,
                    cli_dirs: self.control_cli_dirs.clone(),
                    source: ControlSource::Prefs,
                };
                self.control = start_control(&mut self.state, ctx, &req, false);
            }
            C::Reapply => {
                let danger = self.state.prefs.control_danger;
                nbs_editor::control::sync_policy(&self.state, &self.control_cli_dirs, danger);
                if let Some(info) = &mut self.state.control_info {
                    info.allow_danger = danger;
                }
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 原版 `control_create.gml:28-34`：任意位置的 `-player` / `--protocol-launcher`
    // 都置位 isplayer。后者以 `--` 开头，必须先排除出 CLI 分支。
    let is_player = args
        .iter()
        .any(|a| a == "-player" || a == "--protocol-launcher");
    // `--control` 是给 GUI 开的开关（无界面进程没有编辑器状态可操控），
    // 和 `-player` 一样要排除在 CLI 分支之外，否则它会被当成一条子命令。
    let has_control = control_request(&args).is_some();
    if !is_player && !has_control && args.first().is_some_and(|a| a.starts_with("--")) {
        if let Err(e) = run_cli(&args) {
            // 前缀跟着语言走；`{e}` 本身（NbsError 的 Display）在 run_cli 里已经钉过语言了。
            let msg = if nbs_core::lang() == 1 {
                format!("错误: {e}")
            } else {
                format!("Error: {e}")
            };
            eprintln!("{msg}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // 原版启动时的窗口图标就是 exe 内嵌的那张（勾了「旧图标」才换成 Data\icon.ico），
    // 我们这个 exe 没有内嵌资源，不显式指定的话 eframe 会塞它自己的 egui 标志。
    // 启动走 `NativeOptions`（eframe 自己会同时设大小图标），运行中切换走帧首的
    // `window_icon::apply` + `EditorState` 发的 egui 窗口命令，两条路都要按当前设置选图。
    let prefs = nbs_io::prefs::Preferences::load();
    let mut viewport = egui::ViewportBuilder::default().with_title("nbs-rust");
    // 原版 `control_create.gml:707-720`：播放器档窗口 800×500×window_scale，
    // 最小尺寸相同；编辑器档 min 也是 800×500×scale，端口沿用自定的 960×620。
    // 端口 `window_scale` 只缩画布不缩窗口（#80 定案），这里取基准尺寸。
    if is_player {
        viewport = viewport
            .with_inner_size([800.0, 500.0])
            .with_min_inner_size([800.0, 500.0]);
    } else {
        viewport = viewport
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 620.0]);
    }
    if let Some(icon) =
        nbs_editor::icons::load_window_icon(if prefs.window_icon { "old" } else { "new" })
    {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        // 用户要求 #119：垂直同步跟着首选项走（eframe 出厂为 true，出厂值一致）。
        vsync: prefs.vsync,
        ..Default::default()
    };
    eframe::run_native(
        "nbs-rust",
        options,
        Box::new(move |cc| {
            let mut app = NbsApp::new(cc);
            app.state.player = is_player;
            Ok(Box::new(app))
        }),
    )
}

/// 无界面批处理：便于 CI / 脚本批量转换，不需要音频设备或显示器。
fn run_cli(args: &[String]) -> Result<(), String> {
    // 报错文字（`NbsError` 的 Display 等）按语言开关出词，先按 prefs 把本线程钉好，
    // 别让设了中文的用户看到半句英文。CLI 自己的用法说明是移植附带的工具，原版没有，
    // 保持中文一种。
    nbs_editor::set_language(nbs_io::prefs::Preferences::load().language);
    let cmd = args[0].as_str();
    if cmd == "--help" || cmd == "-h" {
        println!(
            "nbs-rust CLI\n\
             --info <song.nbs>                          打印歌曲信息\n\
             --export-wav <song.nbs> <out.wav>         离线导出 PCM WAV\n\
             --export-audio <song.nbs> <out.mp3|aiff|flac> [--rate N] [--channels N]  经 ffmpeg 转码导出\n\
             --export-midi <song.nbs> <out.mid>        导出 SMF MIDI\n\
             --export-png <song.nbs> <out.png>         导出 PNG 谱面\n\
             --export-schematic <song.nbs> <out.schematic>  导出原理图\n\
             --export-datapack <song.nbs> <out.zip|目录名>  导出 Minecraft 数据包\n\
             --export-nbp <song.nbs> <out.nbp>         导出选区片段\n\
             --import-midi <in.mid> <out.nbs>          导入 MIDI 为 NBS\n\
             --import-schematic <in.schematic> <out.nbs>  导入 Minecraft 原理图为 NBS\n\
             --audio-info                             显示音频设备与采样加载状态\n\
             \n\
             GUI 附加开关（任意位置出现都算，不触发上面的批处理分支）：\n\
             --control[=端口]      开界面同时监听 127.0.0.1 外部控制，端口省略则由系统分配\n\
             --control-dir=<目录>  允许外部在这个目录下读写，可重复\n\
             --control-danger      允许「替人按按钮」那一类动作（退出、开外链、答确认框）\n\
             \n\
             不打旗标也能接：设置页「外部控制（MCP）」勾上后每次开窗口就监听；\n\
             环境变量 NBS_RUST_CONTROL=0/1 临时摁住或顶开那个勾选（只给「启用」，\n\
             危险动作仍然只认 --control-danger 和设置页那条独立勾选）。"
        );
        return Ok(());
    }
    if cmd == "--audio-info" {
        use nbs_audio::AudioEngine as _;
        let engine = nbs_audio::CpalEngine::new();
        println!("引擎: {}", engine.name());
        println!("设备: {}", engine.device_info());
        println!("采样: {}", engine.sample_status());
        println!("采样目录: {:?}", nbs_audio::find_sounds_dir(None));
        return Ok(());
    }
    if cmd == "--info" {
        if args.len() < 2 {
            return Err("用法: --info <song.nbs>".into());
        }
        let song = nbs_core::nbs::load(Path::new(&args[1])).map_err(|e| e.to_string())?;
        println!(
            "名称: {}\n作者: {}\n版本: NBS v{}\n音符: {}\n图层: {}\n长度: {} ticks\n速度: {:.2} t/s ({:.0} BPM)",
            song.meta.name, song.meta.author, song.meta.save_version,
            song.total_blocks, song.layers.len(), song.song_length(),
            song.meta.tempo, song.meta.bpm()
        );
        return Ok(());
    }
    if args.len() < 3 {
        return Err(format!("用法: {cmd} <输入> <输出>（--help 查看全部）"));
    }
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);

    if cmd == "--import-midi" {
        // precision=0 → deltapertick = PPQ/4 = 120，与本程序导出的 MIDI 1:1 对应
        let opts = nbs_io::midi::MidiImportOptions {
            precision: 0,
            ..Default::default()
        };
        let song = nbs_io::midi::import_midi_path(input, &opts)?;
        nbs_core::nbs::save(output, &song, nbs_core::nbs::NBS_VERSION)
            .map_err(|e| e.to_string())?;
        println!("已导入 {} 个音符 → {}", song.total_blocks, output.display());
        return Ok(());
    }

    if cmd == "--import-schematic" {
        let info = nbs_io::schematic::import_schematic(input).map_err(|e| e.to_string())?;
        nbs_core::nbs::save(output, &info.song, nbs_core::nbs::NBS_VERSION)
            .map_err(|e| e.to_string())?;
        println!(
            "图样 {}×{}×{}：{} 个输入 / {} 个音符盒 / 落子 {} → {}",
            info.width,
            info.length,
            info.height,
            info.inputs,
            info.note_blocks,
            info.added_notes,
            output.display()
        );
        return Ok(());
    }

    let song = nbs_core::nbs::load(input).map_err(|e| e.to_string())?;
    match cmd {
        "--export-wav" => {
            let mut bank = nbs_audio::SoundBank::discover();
            bank.extend_for_song(&song);
            let st =
                nbs_audio::export_wav(&song, &bank, output, &nbs_audio::ExportOptions::default())?;
            println!(
                "已导出 {} 音符 / {:.1}s / {} 线程 / 峰值 {:.2} → {}",
                st.notes,
                st.duration_seconds,
                st.threads,
                st.peak,
                output.display()
            );
        }
        // 与原版一样：先渲染 WAV，再交给 ffmpeg 转目标格式。格式由输出扩展名决定。
        "--export-audio" => {
            let format = nbs_io::audio_export::AudioFormat::from_path(output)
                .ok_or_else(|| format!("无法从 {} 判断音频格式", output.display()))?;
            let mut opts = nbs_audio::ExportOptions::default();
            for (i, a) in args.iter().enumerate() {
                match a.as_str() {
                    "--rate" => {
                        opts.sample_rate = args[i + 1].parse().map_err(|_| "采样率不是数字")?
                    }
                    "--channels" => {
                        opts.channels = args[i + 1].parse().map_err(|_| "声道数不是数字")?
                    }
                    _ => {}
                }
            }
            if !nbs_io::audio_export::SAMPLE_RATES.contains(&opts.sample_rate) {
                return Err(format!("采样率 {} 不在原版列表中", opts.sample_rate));
            }
            let tmp = output.with_extension("nbs-tmp.wav");
            let mut bank = nbs_audio::SoundBank::discover();
            bank.extend_for_song(&song);
            let st = nbs_audio::export_wav(&song, &bank, &tmp, &opts)?;
            let enc = nbs_io::audio_export::EncodeOptions {
                format,
                sample_rate: opts.sample_rate,
                channels: opts.channels,
            };
            let res = nbs_io::audio_export::encode_from_wav(&tmp, output, &enc);
            let _ = std::fs::remove_file(&tmp);
            res.map_err(|e| e.to_string())?;
            println!(
                "已导出 {} 音符 / {:.1}s → {}（{}，{} Hz，{} 声道）",
                st.notes,
                st.duration_seconds,
                output.display(),
                format.name(),
                opts.sample_rate,
                opts.channels
            );
        }
        "--export-midi" => {
            nbs_io::midi::export_midi(&song, output)?;
            println!("已导出 MIDI → {}", output.display());
        }
        "--export-png" => {
            let (w, h) = nbs_io::image_export::export_png(
                &song,
                output,
                &nbs_io::image_export::ImageOptions::default(),
            )?;
            println!("已导出 PNG 谱面 {w}×{h} → {}", output.display());
        }
        "--export-schematic" => {
            let info = nbs_io::schematic::export_schematic(
                &song,
                output,
                &nbs_io::schematic::SchematicOptions::default(),
                None,
            )?;
            println!(
                "已导出原理图 {}×{}×{} / {} 音符盒 → {}",
                info.length,
                info.width,
                info.height,
                info.note_blocks,
                output.display()
            );
        }
        "--export-datapack" => {
            let mut opts = nbs_io::datapack::DatapackOptions::default();
            // GUI 打开窗口时把 dat_name 预填成歌曲名，CLI 保持一致；没有名字就退回输出文件名。
            let stem = output
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            opts.name = song.meta.name.trim().to_string();
            if opts.name.is_empty() {
                opts.name = stem;
            }
            // 原版用 dat_usezip 同时决定容器和 uninstall.mcfunction 里的引用形式；
            // CLI 没有这个开关，就以输出名 itself 为准，两者不许各说各话。
            opts.use_zip = output.extension().and_then(|e| e.to_str()) == Some("zip");
            let info = nbs_io::datapack::export(&song, &opts, output)?;
            println!(
                "已导出数据包 {} 个文件（{}）→ {}\n播放命令：/function {}play",
                info.files,
                if info.zip { "ZIP" } else { "目录" },
                output.display(),
                info.functionpath
            );
        }
        "--export-nbp" => {
            let items: Vec<_> = song.iter_notes().collect();
            let sel = nbs_core::Selection::from_absolute(0, 0, &items);
            nbs_io::pattern::export_pattern(&sel, &song, output)?;
            println!("已导出片段 {} 音符 → {}", sel.len(), output.display());
        }
        other => return Err(format!("未知命令: {other}，用 --help 查看")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_io::prefs::Preferences;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 只认一个键的假环境：其它键一律「没设过」，和 `std::env::var` 报 `Err` 同口径。
    fn env_with(value: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let value = value.map(str::to_string);
        move |key| {
            (key == "NBS_RUST_CONTROL")
                .then(|| value.clone())
                .flatten()
        }
    }

    /// 什么都没设过。
    fn no_env() -> impl Fn(&str) -> Option<String> {
        |_| None
    }

    fn prefs(enabled: bool, danger: bool, dirs: &[&str]) -> Preferences {
        Preferences {
            control_enabled: enabled,
            control_danger: danger,
            control_dirs: dirs.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn cli_flags_still_say_everything() {
        // 命令行那层必须原样好使：冒烟脚本就是靠 `--control` + `--control-dir` 起的服务。
        // 这里同时钉住「环境变量摁不住命令行」——`NBS_RUST_CONTROL=0` 是给设置页
        // 那个开关留的应急口，不是用来废掉用户自己敲的旗标的。
        let req = resolve_control(
            &argv(&["--control=1234", "--control-danger", "--control-dir=/a"]),
            env_with(Some("0")),
            &prefs(false, false, &[]),
        )
        .expect("给了 --control 就该起");
        assert_eq!(req.source, ControlSource::Cli);
        assert_eq!(req.port, 1234);
        assert!(req.allow_danger);
        assert_eq!(req.cli_dirs, vec![PathBuf::from("/a")]);
    }

    #[test]
    fn env_and_prefs_decide_when_cli_stays_silent() {
        let p = prefs(true, false, &[]);
        // 设置页开着 → 起，端口交系统挑（真实端口在发现文件里）。
        let req = resolve_control(&argv(&["song.nbs"]), no_env(), &p).expect("prefs 开着就该起");
        assert_eq!(req.source, ControlSource::Prefs);
        assert_eq!(req.port, 0);
        assert!(!req.allow_danger);
        // 同一份 prefs，环境变量给 0 就摁住：不用进窗口取消勾选也能临时不开。
        assert!(resolve_control(&argv(&[]), env_with(Some("0")), &p).is_none());
        // 反过来：prefs 关着，环境变量给个认得出的真值也能开（只授予启用）。
        let req = resolve_control(&argv(&[]), env_with(Some("yes")), &prefs(false, false, &[]))
            .expect("环境变量给了真值就该起");
        assert_eq!(req.source, ControlSource::Env);
        assert!(!req.allow_danger, "环境变量只给启用，不给危险动作");
    }

    #[test]
    fn danger_only_comes_from_flag_or_settings() {
        // 危险动作的两个来源单独测：这条是「AI 能不能替你退出程序」的闸，
        // 最坏的写法是它跟着「启用」一起被默认打开。
        for env_value in [None, Some("1"), Some("true"), Some("on")] {
            let req = resolve_control(&argv(&[]), env_with(env_value), &prefs(true, false, &[]))
                .expect("开着就该起");
            assert!(!req.allow_danger, "env={env_value:?} 却把 danger 带起来了");
        }
        // danger 独立持久化：只勾它、不勾「启用」→ 服务根本不起。
        assert!(resolve_control(&argv(&[]), no_env(), &prefs(false, true, &[])).is_none());
        // 勾了启用 + 勾了 danger → 才给。
        let req = resolve_control(&argv(&[]), no_env(), &prefs(true, true, &[])).unwrap();
        assert!(req.allow_danger);
        // 只给 --control-danger 不给 --control：原版口径，等于没服务可连，不算开启。
        assert!(resolve_control(&argv(&["--control-danger"]), no_env(), &prefs(false, false, &[]))
            .is_none());
    }

    #[test]
    fn launch_request_only_carries_the_flag_directories() {
        // 白名单由 `control::sync_policy` 现场汇总（种子目录 + 旗标 + 设置页），
        // 启动判定这一层不替它拼。两处各拼一次会有两份真相，而且面板上
        // 「启动旗标额外放行的（这一趟删不掉）」那一栏会把设置页的目录误报成旗标给的。
        let req = resolve_control(
            &argv(&["--control", "--control-dir=/a"]),
            no_env(),
            &prefs(false, false, &["/b"]),
        )
        .unwrap();
        assert_eq!(req.cli_dirs, vec![PathBuf::from("/a")]);
        // 没给旗标时这里就是空的：设置页那栏目录仍然生效，但那是策略层的事。
        let req = resolve_control(&argv(&[]), env_with(Some("1")), &prefs(false, false, &["/c"]))
            .unwrap();
        assert!(req.cli_dirs.is_empty());
    }

    #[test]
    fn garbage_env_value_defers_to_settings_page() {
        // 拼错值时不能默默当成「关」，也不能当成「开」：吼一声（stderr），按没给处理。
        let req = resolve_control(&argv(&[]), env_with(Some("ture")), &prefs(true, false, &[]))
            .expect("值认不出来就该退回 prefs，而 prefs 是开的");
        assert_eq!(req.source, ControlSource::Prefs);
        assert!(resolve_control(&argv(&[]), env_with(Some("ture")), &prefs(false, false, &[]))
            .is_none());
        // 空串也归「认不出来」：它既不是真值也不是假值，猜哪边都可能骗人。
        assert!(resolve_control(&argv(&[]), env_with(Some("")), &prefs(true, false, &[])).is_some());
    }
}
