//! 手工冒烟：连上**正在运行的编辑器**，按顺序跑一串有代表性的调用，把每条回执打全。
//! 这是给人在真机上验证链路用的，不是单测：它会打开窗口、真的落盘。
//!
//! ```text
//! NBS_RUST_CONFIG_DIR=<配置目录> nbs-rust.exe <song.nbs> --control --control-dir=<工作目录> --control-danger
//! cargo run -p nbs-control --example smoke -- <配置目录> <工作目录>
//! ```
//!
//! 结尾几步专门挑了条最绕的路：`transpose_to_minecraft` 落在原版的「确定分支」，转完会留一个
//! 只读的提示框，框挂着时写操作要被挡下、`answer_modal` 要能把框答掉；`exit` 是危险动作，
//! 而且脏着的时候要先拒、认错（`discard_changes=true`）才真关。
//! 一次跑完「危险开关 + 弹窗闸 + 主线程取件 + 停服删发现文件」四件事。

use std::path::{Path, PathBuf};

use nbs_control::{BatchField, CellNote, ControlClient, Discovery, Op, Outcome, Scope, SetMeta};

fn show(label: &str, result: Result<Outcome, nbs_control::ClientError>) {
    print!("{label:<28} ");
    match result {
        Err(e) => println!("传输失败: {e}"),
        Ok(out) => {
            let report = &out.report;
            print!(
                "{} code={:?} rev={} cells={} hist={}/{} dirty={}",
                if out.ok { "ok  " } else { "FAIL" },
                out.error_code(),
                report.rev,
                report.cells_changed,
                report.history_cursor,
                report.history_len,
                report.dirty,
            );
            // 成功的那句话也要看见：闸为什么放行、批量跳过了几格全在里面。
            let text = if out.ok {
                report.message.clone()
            } else {
                out.error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_default()
            };
            if !text.is_empty() {
                print!("  「{}」", truncate(&text, 140));
            }
            if let Some(data) = &out.data {
                let json = serde_json::to_string(data).unwrap_or_default();
                print!("  data={}", truncate(&json, 200));
            }
            println!();
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    let flat = text.replace('\n', "⏎");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

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

fn main() {
    let mut argv = std::env::args().skip(1);
    let config = PathBuf::from(
        argv.next()
            .expect("用法: smoke <配置目录> <工作目录>（工作目录要给 --control-dir）"),
    );
    let work = PathBuf::from(argv.next().expect("同上"));
    std::process::exit(run(&config, &work));
}

fn run(config: &Path, work: &Path) -> i32 {
    println!("发现文件目录: {}", config.display());
    let found = match Discovery::read_from(config) {
        Ok(found) => found,
        Err(e) => {
            println!("  读不到: {e} —— 编辑器没起，或没带 --control\n");
            return 1;
        }
    };
    println!("  {found:?}\n");
    let port = found.port;

    let mut client = match ControlClient::from_discovery_dir(config, "smoke-example") {
        Ok(client) => client,
        Err(e) => {
            println!("握手失败: {e}");
            return 1;
        }
    };
    println!("连上: {}\n", client.server_app());

    show("Ping", client.request(Op::Ping));
    show("GetSnapshot", client.request(Op::GetSnapshot));
    show("GetLayers", client.request(Op::GetLayers));
    show("GetInstruments", client.request(Op::GetInstruments));
    show("GetDiagnostics", client.request(Op::GetDiagnostics));
    show("ListActions", client.request(Op::ListActions));

    // 写：一次调用 = 一次撤销。
    let rev = match client.request(Op::GetSnapshot) {
        Ok(out) => out.report.rev,
        Err(_) => 0,
    };
    show(
        "SetNotes(2)",
        client.request(Op::SetNotes {
            cells: vec![cell(0, 0, 1, 36), cell(1, 0, 1, 40)],
            label: Some("冒烟写入".into()),
            expect_rev: Some(rev),
        }),
    );
    show(
        "GetNotes(0..2)",
        client.request(Op::GetNotes {
            tick_from: Some(0),
            tick_to: Some(2),
            layer_from: Some(0),
            layer_to: Some(0),
            ticks_per_bar: None,
            bars: None,
        }),
    );
    show(
        "GetNotes(一个参数都不给=全曲)",
        client.request(Op::GetNotes {
            tick_from: None,
            tick_to: None,
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: None,
        }),
    );
    show(
        "GetNotes(按小节读前两节)",
        client.request(Op::GetNotes {
            tick_from: None,
            tick_to: None,
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: Some([0, 1]),
        }),
    );
    show(
        "拿旧 rev 再写(应冲突)",
        client.request(Op::SetNotes {
            cells: vec![cell(2, 0, 1, 45)],
            label: None,
            expect_rev: Some(rev),
        }),
    );
    show(
        "BatchField velocity +10",
        client.request(Op::BatchField {
            field: BatchField::Velocity,
            mode: "add".into(),
            value: 10,
            scope: Scope::Rect {
                tick_from: 0,
                layer_from: 0,
                tick_to: 2,
                layer_to: 0,
            },
            label: None,
            expect_rev: None,
        }),
    );
    show("Undo", client.request(Op::Undo { steps: None }));
    show("Redo", client.request(Op::Redo { steps: None }));
    show(
        "SetMeta tempo+name",
        client.request(Op::SetMeta {
            meta: SetMeta {
                tempo: Some(150.0),
                name: Some("冒烟曲".into()),
                ..Default::default()
            },
            expect_rev: None,
        }),
    );

    // 文件：路径必须落在 --control-dir 里。
    let outside = work
        .join("..")
        .join("nbs-smoke-outside.nbs")
        .display()
        .to_string();
    show(
        "SavePath(白名单外)",
        client.request(Op::SavePath {
            path: outside,
            version: None,
            overwrite: false,
        }),
    );
    let inside = work.join("nbs-smoke.nbs").display().to_string();
    show(
        "SavePath(白名单内)",
        client.request(Op::SavePath {
            path: inside.clone(),
            version: None,
            overwrite: true,
        }),
    );
    println!(
        "  落盘检查: {} → {}",
        truncate(&inside, 90),
        Path::new(&inside).is_file()
    );
    show(
        "OpenPath(刚存的那份)",
        client.request(Op::OpenPath {
            path: inside.clone(),
            discard_changes: true,
        }),
    );
    // 原版开一首有名字的歌会自己弹出 Song Info 那一格（`open_nbs_at` 的行为，界面测试也钉着）。
    // 它不攥草稿，所以不挡写操作，但它会一直挂在屏幕上 —— `close_window` 就是替人点掉它。
    show(
        "RunAction close_window",
        client.request(Op::RunAction {
            action: "close_window".into(),
            args: None,
        }),
    );
    show("GetHistory", client.request(Op::GetHistory));

    // 播放：起一秒钟再看位置。
    show(
        "Playback play",
        client.request(Op::Playback(nbs_control::Playback::Play)),
    );
    std::thread::sleep(std::time::Duration::from_millis(700));
    show("GetSnapshot(播放中)", client.request(Op::GetSnapshot));
    show(
        "Playback stop",
        client.request(Op::Playback(nbs_control::Playback::Stop)),
    );

    // 危险 + 弹窗那条最绕的路。`transpose_to_minecraft` 走的是原版的确定分支，转完留一个
    // 「已编辑 N 个音符」的提示框；框挂着的时候写操作必须被挡下来，
    // 而不是回一个「其实什么都没发生」的成功。
    show(
        "RunAction window_scale_up",
        client.request(Op::RunAction {
            action: "window_scale_up".into(),
            args: None,
        }),
    );
    show(
        "RunAction 未知动作",
        client.request(Op::RunAction {
            action: "not_a_real_action".into(),
            args: None,
        }),
    );
    // 先弄脏：`exit` 那道闸门只在有未保存改动时才拦人，脏没脏看回执里的 dirty。
    // 这条放在 transpose 之前，是因为它被拒之后程序必须还活着，后面的步骤才有的跑。
    show(
        "SetNotes(弄脏)",
        client.request(Op::SetNotes {
            cells: vec![cell(30, 0, 1, 48)],
            label: None,
            expect_rev: None,
        }),
    );
    show(
        "RunAction exit(脏,应拒)",
        client.request(Op::RunAction {
            action: "exit".into(),
            args: None,
        }),
    );
    show(
        "RunAction 转 MC 音域",
        client.request(Op::RunAction {
            action: "transpose_to_minecraft".into(),
            args: None,
        }),
    );
    match client.request(Op::GetSnapshot) {
        Ok(out) => match &out.data {
            Some(nbs_control::Data::Snapshot(snap)) => println!(
                "  界面现状: 确认框 {} 个，窗口 {:?}，脏 {}，播放头 {:.1}",
                snap.modal_count, snap.open_window, snap.dirty, snap.playhead
            ),
            other => println!("  快照形态不对: {other:?}"),
        },
        Err(e) => println!("  快照失败: {e}"),
    }
    show(
        "SetNotes(应被弹窗挡)",
        client.request(Op::SetNotes {
            cells: vec![cell(31, 0, 1, 48)],
            label: None,
            expect_rev: None,
        }),
    );
    show(
        "RunAction answer_modal no",
        client.request(Op::RunAction {
            action: "answer_modal".into(),
            args: Some(serde_json::json!({ "yes": false })),
        }),
    );
    show(
        "SetNotes(答完应能写)",
        client.request(Op::SetNotes {
            cells: vec![cell(31, 0, 1, 48)],
            label: None,
            expect_rev: None,
        }),
    );
    // 音频转 MIDI：三条要下载的先说「该人自己去勾」，probe 不需要许可、起的是任务。
    show(
        "TranscribePrepareEnv(没勾许可应被拒)",
        client.request(Op::TranscribePrepareEnv { cuda: Some(false) }),
    );
    show(
        "TranscribeAudio(没勾许可时不该先报路径)",
        client.request(Op::TranscribeAudio {
            audio: "C:/nope/there.wav".into(),
            options: Default::default(),
            save_midi: None,
            overwrite: false,
        }),
    );
    show(
        "TranscribeProbe(不需要许可)",
        client.request(Op::TranscribeProbe),
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    show("JobList(转录任务在册)", client.request(Op::JobList));

    show(
        "RunAction exit(认错丢弃)",
        client.request(Op::RunAction {
            action: "exit".into(),
            args: Some(serde_json::json!({ "discard_changes": true })),
        }),
    );

    // 窗口关干净之后：服务该停，发现文件该没了。
    std::thread::sleep(std::time::Duration::from_secs(2));
    let file = config.join(nbs_control::DISCOVERY_FILE);
    let again = Discovery::read_from(config);
    println!(
        "\n停服后再读发现文件: {:?}",
        again.as_ref().err().map(|e| e.to_string())
    );
    let port_free = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
        std::time::Duration::from_millis(500),
    );
    println!(
        "原端口 {port} 再连: {:?}",
        port_free.as_ref().err().map(|e| e.kind())
    );
    if !file.exists() {
        println!("结论: 退出清理已生效（发现文件不在，端口也不再接受）");
        0
    } else {
        println!("结论: 发现文件还在原地 —— 服务没停干净");
        1
    }
}
