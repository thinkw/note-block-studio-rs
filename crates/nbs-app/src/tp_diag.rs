//! 滚动输入诊断探针（临时，不是功能代码）。
//!
//! 只观测、只写日志，**不参与任何滚动决策**：不带 `NBS_TPDIAG` 时第一行就返回，默认零开销。
//! 日志落在 `.work/tpdiag/run-<epoch>.log`（`.work/` 已在 .gitignore 里）。
//!
//! 曾经还想用它验证 DirectManipulation 能不能白拿系统动量（`rust_win_trackpad`），实测
//! `ATTACH ok` 之后本机一场真实滑动（1933 帧 / 900 条滚轮事件）手势流为 0 条 —— 那条路
//! 已判死并摘掉依赖。现在它只干一件事：把 Windows 到底给了什么样的滚轮流钉成数字，
//! 供 `grid.rs` 的滚动手势锁存和尾巴放行做前后对照。

use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{Context, Event, MouseWheelUnit, Vec2};

/// 日志目录（相对进程 CWD；`cargo run` 时就是仓库根）。
const LOG_DIR: &str = ".work/tpdiag";

pub fn enabled() -> bool {
    match std::env::var("NBS_TPDIAG") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[derive(Default)]
struct Count {
    /// 本帧有滚轮事件的帧数
    event_frames: u64,
    /// 其中「整格」帧（每个分量都是 ≥1 的整数）—— 像鼠标滚轮
    integral_frames: u64,
    /// 其中带小数格的帧 —— 像精确触控板
    fraction_frames: u64,
    /// 单事件不足一格的次数（触控板的细滑）
    sub_notch_events: u64,
    /// egui 漏桶在动（`smooth_scroll_delta` 非零）但本帧没有事件的帧 = 抬手后的尾巴
    tail_frames: u64,
    tail_segments: u64,
    tail_ms_total: u64,
    tail_px_total: f64,
    tail_px_max: f64,
    tail_ms_max: u64,
    wheel_events: u64,
    notch_units: f64,
    point_units: f64,
    frames_with_raw: u64,
    frames_with_smooth: u64,
}

struct Diag {
    started: Instant,
    path: PathBuf,
    log: Option<File>,
    frame: u64,
    next_beat: u64,
    /// 当前这段尾巴的起点/终点/累计位移
    seg: Option<(Instant, Instant, f64)>,
    c: Count,
}

impl Diag {
    fn new() -> Self {
        let path = Path::new(LOG_DIR).join(format!("run-{}.log", epoch_ms() / 1000));
        let mut diag = Self {
            started: Instant::now(),
            path,
            log: None,
            frame: 0,
            next_beat: 10,
            seg: None,
            c: Count::default(),
        };
        if let Some(parent) = diag.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&diag.path)
        {
            Ok(file) => diag.log = Some(file),
            Err(e) => eprintln!("[tpdiag] 日志打不开 {:?}: {e}", diag.path),
        }
        eprintln!("[tpdiag] 滚动输入探针已开启，日志: {:?}", diag.path);
        diag.line(&format!(
            "# tpdiag start pid={} epoch_ms={} dir={}",
            std::process::id(),
            epoch_ms(),
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ));
        diag
    }

    fn line(&mut self, text: &str) {
        let frame = self.frame;
        let ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let Some(log) = self.log.as_mut() else { return };
        let _ = writeln!(log, "f={:<6} t={:>10.1}ms {}", frame, ms, text);
        let _ = log.flush();
    }

    /// 尾巴段的收口：一段 = 连续若干「只有漏桶、没有事件」的帧。
    fn close_segment(&mut self) {
        let Some((first, last, px)) = self.seg.take() else {
            return;
        };
        let ms = (last - first).as_millis() as u64;
        self.c.tail_segments += 1;
        self.c.tail_ms_total += ms;
        self.c.tail_ms_max = self.c.tail_ms_max.max(ms);
        self.c.tail_px_total += px;
        self.c.tail_px_max = self.c.tail_px_max.max(px);
        self.line(&format!("TAIL-SEG frames={} ms={} px={:.1}", ms, ms, px));
    }

    fn on_frame(&mut self, ctx: &Context) {
        let snap = ctx.input(|i| {
            let mut wheels: Vec<(u8, f32, f32)> = Vec::new();
            for e in i.events.iter() {
                if let Event::MouseWheel { unit, delta, .. } = e {
                    wheels.push((
                        match unit {
                            MouseWheelUnit::Line => 0u8,
                            MouseWheelUnit::Page => 1,
                            MouseWheelUnit::Point => 2,
                        },
                        delta.x,
                        delta.y,
                    ));
                }
            }
            (i.raw_scroll_delta, i.smooth_scroll_delta, wheels)
        });
        let (raw, smooth, wheels) = snap;
        if raw != Vec2::ZERO {
            self.c.frames_with_raw += 1;
        }
        if smooth != Vec2::ZERO {
            self.c.frames_with_smooth += 1;
        }
        let mut integral = true;
        let mut sub_notch = false;
        for (unit, dx, dy) in &wheels {
            self.c.wheel_events += 1;
            match unit {
                0 => self.c.notch_units += (dx.abs() + dy.abs()) as f64,
                2 => self.c.point_units += (dx.abs() + dy.abs()) as f64,
                _ => {}
            }
            for v in [*dx, *dy] {
                let a = v.abs();
                if a == 0.0 {
                    continue;
                }
                if a < 1.0 {
                    sub_notch = true;
                }
                if *unit != 0 || (a - a.round()).abs() >= 0.011 || a < 1.0 {
                    integral = false;
                }
            }
        }
        if !wheels.is_empty() {
            self.c.event_frames += 1;
            if integral {
                self.c.integral_frames += 1;
            } else {
                self.c.fraction_frames += 1;
            }
        }
        self.c.sub_notch_events += u64::from(sub_notch);

        // 尾巴帧 = 有漏桶输出、没滚轮事件。
        if wheels.is_empty() && smooth != Vec2::ZERO {
            let px = smooth.x.abs().max(smooth.y.abs()) as f64;
            let now = Instant::now();
            match self.seg.as_mut() {
                Some((_, last, acc)) => {
                    *last = now;
                    *acc += px;
                }
                None => self.seg = Some((now, now, px)),
            }
            self.c.tail_frames += 1;
        } else {
            self.close_segment();
        }

        if !wheels.is_empty() || raw != Vec2::ZERO || smooth != Vec2::ZERO {
            let sum: Vec2 = wheels
                .iter()
                .fold(Vec2::ZERO, |acc, &(_, x, y)| acc + Vec2::new(x, y));
            self.line(&format!(
                "IN raw=({:+7.2},{:+7.2}) fine=({:+7.2},{:+7.2}) notches=({:+.3},{:+.3}) kind={} tail={}",
                raw.x,
                raw.y,
                smooth.x,
                smooth.y,
                sum.x,
                sum.y,
                if wheels.is_empty() {
                    "tail"
                } else if integral {
                    "wheel"
                } else {
                    "trackpad"
                },
                wheels.is_empty() && smooth != Vec2::ZERO,
            ));
        }
    }

    fn summary(&mut self) {
        self.close_segment();
        let c = &self.c;
        self.line(&format!(
            "SUMMARY epoch_ms={} frames={} eventFrames={} 整格帧={} 小数帧={} 单事件不足一格帧={} tailFrames={} tailSegs={} tailMs(max/总)={}/{} tailPx(max/总)={:.1}/{:.1} wheelEvents={} notchUnits={:.1} pointUnits={:.1} rawFrames={} fineFrames={}",
            epoch_ms(),
            self.frame,
            c.event_frames,
            c.integral_frames,
            c.fraction_frames,
            c.sub_notch_events,
            c.tail_frames,
            c.tail_segments,
            c.tail_ms_max,
            c.tail_ms_total,
            c.tail_px_max,
            c.tail_px_total,
            c.wheel_events,
            c.notch_units,
            c.point_units,
            c.frames_with_raw,
            c.frames_with_smooth,
        ));
    }
}

thread_local! {
    static DIAG: RefCell<Option<Diag>> = const { RefCell::new(None) };
}

/// 每帧调用一次：记录这一帧 egui 收到的滚动输入。
pub fn tick(ctx: &Context) {
    if !enabled() {
        return;
    }
    DIAG.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(Diag::new());
        }
        let Some(diag) = slot.as_mut() else { return };
        diag.frame += 1;
        diag.on_frame(ctx);
        // 心跳：反应式渲染下空闲几乎不产帧，没心跳就分不清「没操作」和「探针死了」。
        // 跨睡眠 elapsed 会一次跳掉几千秒，必须按当前值重新对齐，否则逐帧刷屏。
        let secs = diag.started.elapsed().as_secs();
        if secs >= diag.next_beat {
            diag.summary();
            diag.next_beat = secs + 10;
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        }
    });
}
