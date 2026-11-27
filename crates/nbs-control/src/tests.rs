//! 协议层自测：线格式、帧读写、收件箱、任务表、发现文件，以及回环服务的握手与超时。
//! 这里刻意不起真正的编辑器，`echo` 冒充主线程干活。

use std::io::{BufReader, Cursor, ErrorKind};
use std::net::{Ipv4Addr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::bus::{Hello, Request, Response};
use crate::client::{ClientError, ControlClient};
use crate::discovery::{Discovery, DISCOVERY_FILE};
use crate::frame::{read_json_line, write_json_line};
use crate::midi::{
    ImportChannelMap, ImportDrumMap, ImportEnhancedOptions, ImportMidiOptions, ImportRangeMode,
    MidiPlan,
};
use crate::op::{CellNote, ExportFormat, NoteView, Op, Playback, Scope, SelectTarget};
use crate::outcome::{
    BarOverview, ChangeReport, Data, ErrorCode, JobInfo, JobStatus, LayerInfo, NoteCell, NoteRun,
    Outcome,
};
use crate::server::{ControlServer, ServerConfig};
use crate::task::{CancelOutcome, JobKind, JobRegistry, SpawnReject};
use crate::transcribe::TranscribeOptions;
use crate::Inbox;

fn report(rev: u64) -> ChangeReport {
    ChangeReport {
        rev,
        ..Default::default()
    }
}

fn wait_until<F>(what: &str, mut f: F)
where
    F: FnMut() -> bool,
{
    for _ in 0..2_000 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("等不到：{what}");
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nbs-control-{tag}-{}", Discovery::random_token()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------- 线格式

#[test]
fn set_notes_wire_shape() {
    let op = Op::SetNotes {
        cells: vec![CellNote {
            tick: 0,
            layer: 1,
            instrument: 1,
            key: 45,
            velocity: None,
            panning: None,
            pitch: Some(-13),
        }],
        label: None,
        expect_rev: Some(12),
    };
    let value = serde_json::to_value(&op).unwrap();
    assert_eq!(value["kind"], json!("set_notes"));
    assert_eq!(value["expect_rev"], json!(12));
    // 省略的量不上线，回来时仍是 None
    assert_eq!(value["cells"][0].get("velocity"), None);
    assert_eq!(value["cells"][0]["pitch"], json!(-13));
    assert_eq!(serde_json::from_value::<Op>(value).unwrap(), op);
}

#[test]
fn playback_newtype_variant_keeps_both_tags() {
    let op = Op::Playback(Playback::Seek { tick: 12.5 });
    let value = serde_json::to_value(&op).unwrap();
    assert_eq!(value["kind"], json!("playback"));
    assert_eq!(value["action"], json!("seek"));
    assert_eq!(value["tick"], json!(12.5));
    assert_eq!(serde_json::from_value::<Op>(value).unwrap(), op);
}

#[test]
fn nested_scope_and_target_tags() {
    let op = Op::Transpose {
        scope: Scope::Rect {
            tick_from: 4,
            layer_from: 0,
            tick_to: 8,
            layer_to: 2,
        },
        semitones: -2,
        expect_rev: None,
    };
    let value = serde_json::to_value(&op).unwrap();
    assert_eq!(value["scope"]["scope"], json!("rect"));
    assert_eq!(serde_json::from_value::<Op>(value).unwrap(), op);

    let sel = Op::Select {
        target: SelectTarget::Rect {
            tick_from: 0,
            layer_from: 0,
            tick_to: 16,
            layer_to: 3,
            copy: true,
            additive: false,
        },
        expect_rev: None,
    };
    let value = serde_json::to_value(&sel).unwrap();
    assert_eq!(value["target"]["target"], json!("rect"));
    assert_eq!(serde_json::from_value::<Op>(value).unwrap(), sel);
}

#[test]
fn export_format_and_file_data_are_snake_case() {
    assert_eq!(
        serde_json::to_value(ExportFormat::Wav).unwrap(),
        json!("wav")
    );
    let data = Data::File {
        path: "a/b.nbs".to_string(),
        file_kind: "save".to_string(),
        notes: 7,
    };
    let value = serde_json::to_value(&data).unwrap();
    assert_eq!(value["kind"], json!("file"));
    assert_eq!(value["file_kind"], json!("save"));
    assert_eq!(serde_json::from_value::<Data>(value).unwrap(), data);
}

/// 线上跑的是字节，不是 `Value`：`to_value` 把重复键并进同一个 map，
/// 「摊平出两个 kind」这种形状在 Value 里 round-trip 得过去，落到 socket 上
/// 客户端却读不回来（`JobInfo` 自带 `kind`，和 `Data` 的内部 tag 撞名）。
/// 所以这条测试坚持先 `to_string` 再按 `read_json_line` 的路子从字符串读。
#[test]
fn job_data_survives_the_real_wire() {
    let job = JobInfo {
        id: 3,
        kind: "export".to_string(),
        status: JobStatus::Running,
        progress: 0.5,
        message: "离线混音".to_string(),
        started_unix: 1_700_000_000,
        finished_unix: None,
        result: None,
    };
    let cases = [
        Data::Job(Box::new(job.clone())),
        Data::Jobs {
            jobs: vec![job.clone()],
        },
        Data::File {
            path: "a/b.nbs".to_string(),
            file_kind: "save".to_string(),
            notes: 7,
        },
        Data::Pong {
            app_version: "0.1.0".to_string(),
            protocol: crate::PROTOCOL_VERSION,
            rev: 7,
            song_session: 2,
            pid: 4242,
        },
    ];
    for data in cases {
        let outcome = Outcome::ok(data.clone(), report(7));
        let text = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            text.matches("\"kind\"").count(),
            1,
            "线上出现了两个 kind：{text}"
        );
        let back: Outcome =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e} 读不回来：{text}"));
        assert_eq!(back, outcome, "{:?} 的形状发出去和读回来不一样", data);
    }
}

/// MIDI 导入那一面有三条口径只能在线上验：省略的字段是「不出现」而不是 `null`、
/// 预检归只读（否则一个模态框就把看一眼这件事挡死）、核对单摊平进 `Data` 不多出一个 `kind`。
#[test]
fn midi_import_surface_survives_the_wire() {
    let options = ImportMidiOptions {
        precision: Some(3),
        channels: vec![ImportChannelMap {
            channel: 0,
            instrument: 5,
            octave: -1,
        }],
        ..Default::default()
    };
    let enhanced = ImportEnhancedOptions {
        enabled: Some(true),
        range_mode: Some(ImportRangeMode::Fold),
        ..Default::default()
    };
    let import = Op::ImportMidi {
        path: "a/b.mid".to_string(),
        discard_changes: true,
        options: Some(options.clone()),
        enhanced: Some(enhanced.clone()),
        remember: Some(true),
    };
    let inspect = Op::InspectMidi {
        path: "a/b.mid".to_string(),
        options: Some(options.clone()),
        enhanced: None,
    };
    for op in [import.clone(), inspect.clone()] {
        let text = serde_json::to_string(&op).unwrap();
        let back: Op =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e} 读不回来：{text}"));
        assert_eq!(back, op);
    }
    // `null` 会被读成「给了但没值」，和「省略 = 用编辑器基线」是两回事。
    let bare = serde_json::to_string(&Op::InspectMidi {
        path: "a/b.mid".to_string(),
        options: None,
        enhanced: None,
    })
    .unwrap();
    assert!(!bare.contains("options") && !bare.contains("enhanced"), "{bare}");
    let bare_import = serde_json::to_string(&Op::ImportMidi {
        path: "a/b.mid".to_string(),
        discard_changes: false,
        options: None,
        enhanced: None,
        remember: None,
    })
    .unwrap();
    assert!(!bare_import.contains("remember"), "{bare_import}");
    // 预检只是把导入算法跑一遍，不该被写操作的闸门拦住。
    assert!(inspect.is_read_only());
    assert!(!import.is_read_only());

    let plan = MidiPlan {
        path: "a/b.mid".to_string(),
        notes: 12,
        layers: 2,
        layer_names: vec!["Vibraphone".to_string(), "Percussion".to_string()],
        applied: options,
        applied_enhanced: enhanced,
        ..Default::default()
    };
    let outcome = Outcome::ok(Data::MidiPlan(Box::new(plan)), report(9));
    let text = serde_json::to_string(&outcome).unwrap();
    assert_eq!(text.matches("\"kind\"").count(), 1, "线上出现了两个 kind：{text}");
    let back: Outcome = serde_json::from_str(&text).unwrap();
    assert_eq!(back, outcome);
}

/// 拼错的字段名不能读成「这一项没给」：那样回显看起来完全正常、用的却是基线，
/// 而「导入时少勾了一项」这种事没人能从结果反推出来。所以这几个入参结构体不认未知字段。
#[test]
fn a_typo_in_the_import_options_is_rejected_not_ignored() {
    for err in [
        serde_json::from_str::<ImportMidiOptions>(r#"{"percision":3}"#).unwrap_err(),
        serde_json::from_str::<ImportEnhancedOptions>(r#"{"susntain":true}"#).unwrap_err(),
        serde_json::from_str::<ImportChannelMap>(r#"{"channl":0,"instrument":5}"#).unwrap_err(),
        serde_json::from_str::<ImportDrumMap>(r#"{"note":36,"instalment":5,"key":40}"#).unwrap_err(),
    ] {
        assert!(err.to_string().contains("unknown field"), "{err}");
    }
    // 真字段照旧读得进；回显用的是同一批结构体，序列化不受这条属性影响。
    let options = ImportMidiOptions {
        precision: Some(3),
        ..Default::default()
    };
    let text = serde_json::to_string(&options).unwrap();
    assert_eq!(serde_json::from_str::<ImportMidiOptions>(&text).unwrap(), options);
}

#[test]
fn error_codes_have_stable_strings() {
    for code in [
        ErrorCode::BadArgument,
        ErrorCode::NotFound,
        ErrorCode::LockedLayer,
        ErrorCode::RevConflict,
        ErrorCode::ModalOpen,
        ErrorCode::WindowOpen,
        ErrorCode::Busy,
        ErrorCode::Unsupported,
        ErrorCode::NotAllowed,
        ErrorCode::Io,
        ErrorCode::OutOfRange,
        ErrorCode::Timeout,
        ErrorCode::Closed,
        ErrorCode::Internal,
    ] {
        let value = serde_json::to_value(code).unwrap();
        assert_eq!(
            value.as_str(),
            Some(code.as_str()),
            "{code:?} 的字符串漂移了"
        );
        assert_eq!(serde_json::from_value::<ErrorCode>(value).unwrap(), code);
    }
}

#[test]
fn read_only_ops_are_the_safe_subset() {
    assert!(Op::Ping.is_read_only());
    assert!(Op::GetNotes {
        tick_from: Some(0),
        tick_to: Some(1),
        layer_from: None,
        layer_to: None,
        ticks_per_bar: None,
        bars: None,
        limit: None,
        max_bytes: None,
        view: None,
    }
    .is_read_only());
    assert!(Op::JobList.is_read_only());
    // 这些会动歌、动磁盘或动播放，一律不算只读
    assert!(!Op::Save.is_read_only());
    assert!(!Op::Playback(Playback::Play).is_read_only());
    assert!(!Op::Undo {
        steps: None,
        expect_rev: None,
    }
    .is_read_only());
    assert!(!Op::RunAction {
        action: "anything".to_string(),
        args: None
    }
    .is_read_only());
}

/// 断线重发的那份名单判的是「重发一次会不会多出看不见的事」，不是「改不改状态」：
/// `seek` / `stop` 会改走带但幂等，所以进来；`play` 也幂等不了，响没响过几次只有耳朵知道，
/// 所以出去。这两个集合以后会各自长，别图省事合成一个。
#[test]
fn the_resend_list_adds_seek_and_stop_but_keeps_play_out() {
    assert!(Op::Playback(Playback::Seek { tick: 12.0 }).is_resend_safe());
    assert!(Op::Playback(Playback::Stop).is_resend_safe());
    assert!(
        Op::GetSnapshot.is_resend_safe(),
        "只读的那些全都在名单里（它是名单的前半）"
    );
    assert!(!Op::Playback(Playback::Play).is_resend_safe());
    assert!(!Op::Playback(Playback::Pause).is_resend_safe());
    assert!(!Op::Playback(Playback::SetLoop { enabled: true }).is_resend_safe());
    assert!(!Op::Save.is_resend_safe());
    assert!(
        !Op::SavePath {
            path: "x.nbs".to_string(),
            version: None,
            overwrite: true
        }
        .is_resend_safe()
    );
    assert!(
        !Op::ClearSong {
            expect_rev: None,
            discard_changes: true
        }
        .is_resend_safe()
    );
}

/// 小节视图的两端都要过得在线格式上：入参那两个字段是给 AI 填的，`bar_overview` 是
/// 编辑器拼好、桥接原样转出去的——任何一侧悄悄改名都会变成「参数没了」而不是报错。
#[test]
fn the_bar_view_survives_the_wire() {
    let op = Op::GetNotes {
        tick_from: None,
        tick_to: None,
        layer_from: None,
        layer_to: None,
        ticks_per_bar: Some(12),
        bars: Some([0, 255]),
        limit: Some(500),
        max_bytes: Some(8_192),
        view: Some(NoteView::Runs),
    };
    let text = serde_json::to_string(&op).unwrap();
    assert!(text.contains("\"bars\":[0,255]"), "{text}");
    // 预算那三个字段也要在字面上留痕：省略时一个键都不占，给了就得是这三个键名。
    assert!(
        text.contains("\"limit\":500")
            && text.contains("\"max_bytes\":8192")
            && text.contains("\"view\":\"runs\""),
        "{text}"
    );
    assert_eq!(serde_json::from_str::<Op>(&text).unwrap(), op);

    let data = Data::Notes {
        cells: vec![NoteCell {
            tick: 0,
            layer: 0,
            instrument: 1,
            key: 39,
            velocity: 100,
            panning: 100,
            pitch: 0,
            out_of_range: false,
        }],
        truncated: false,
        limit: 20_000,
        total: 1,
        bar_overview: vec![BarOverview {
            bar: 0,
            tick_from: 0,
            tick_to: 11,
            cells: 1,
            text: "0:C4(1)".to_string(),
        }],
    };
    let text = serde_json::to_string(&data).unwrap();
    assert!(text.contains("\"bar_overview\""), "{text}");
    assert_eq!(serde_json::from_str::<Data>(&text).unwrap(), data);

    // 列式与延音段是两种新形状，桥接端只转手不重拼：标签名必须钉在字面上。
    let columns = Data::NoteColumns {
        tick: vec![0, 1],
        layer: vec![0, 0],
        instrument: vec![1, 1],
        key: vec![39, 41],
        velocity: vec![100, 90],
        panning: vec![100, 100],
        pitch: vec![0, -3],
        out_of_range: vec![false, false],
        truncated: false,
        limit: 1_000,
        total: 2,
    };
    let text = serde_json::to_string(&columns).unwrap();
    assert!(text.starts_with("{\"kind\":\"note_columns\""), "{text}");
    assert_eq!(serde_json::from_str::<Data>(&text).unwrap(), columns);

    let runs = Data::NoteRuns {
        runs: vec![NoteRun {
            layer: 0,
            instrument: 1,
            key: 39,
            tick_from: 0,
            tick_to: 3,
            cells: 4,
            velocity_from: 100,
            velocity_to: 80,
        }],
        truncated: true,
        limit: 1,
        total_runs: 9,
        total_cells: 40,
    };
    let text = serde_json::to_string(&runs).unwrap();
    assert!(text.starts_with("{\"kind\":\"note_runs\""), "{text}");
    assert_eq!(serde_json::from_str::<Data>(&text).unwrap(), runs);
}

/// 图层这一面两端都在 wire 上：`get_layers` 的区间是外部调用方填的，`layer` 回声是
/// 编辑器拼好、桥接原样转出去的。顺带钉住向后兼容——老客户端发的还是不带字段的
/// `{"kind":"get_layers"}`，那一版没有区间，读回来必须等于「省略 = 全表」而不是报错。
#[test]
fn the_layer_surface_survives_the_wire() {
    let ranged = Op::GetLayers {
        layer_from: Some(34),
        layer_to: Some(37),
    };
    let text = serde_json::to_string(&ranged).unwrap();
    assert!(text.contains("\"layer_from\":34"), "{text}");
    assert_eq!(serde_json::from_str::<Op>(&text).unwrap(), ranged);

    let legacy: Op = serde_json::from_str(r#"{"kind":"get_layers"}"#).unwrap();
    assert_eq!(legacy, Op::GetLayers { layer_from: None, layer_to: None }, "老形状要读成省略");
    // 省略两头时不该多发两个 null 字段：那是「给了但没值」，和省略不是一回事。
    let bare = serde_json::to_string(&legacy).unwrap();
    assert!(!bare.contains("layer_from") && !bare.contains("layer_to"), "{bare}");

    let echo = Data::Layer {
        layer: LayerInfo {
            index: 34,
            name: "旋律".to_string(),
            volume: 70,
            stereo: 100,
            lock: "normal".to_string(),
            effectively_locked: false,
            notes: 992,
        },
    };
    let text = serde_json::to_string(&echo).unwrap();
    assert!(text.contains("\"kind\":\"layer\""), "{text}");
    assert_eq!(serde_json::from_str::<Data>(&text).unwrap(), echo);
}

/// 身份这一格要钉在字面上：桥接把 `Data` 原样转手，改名不会报错，只会让人拿不到。
/// 同机开好几个编辑器时，`pid` 是唯一不会说谎的那个——`path` 另存就变，
/// `song_session` 只跟着整首替换走，两个都认不出「刚才答话的是哪个进程」。
#[test]
fn the_pid_says_which_process_answered() {
    let pong = Data::Pong {
        app_version: "0.1.0".to_string(),
        protocol: crate::PROTOCOL_VERSION,
        rev: 7,
        song_session: 2,
        pid: 4242,
    };
    let text = serde_json::to_string(&pong).unwrap();
    assert!(text.contains("\"pid\":4242"), "{text}");
    assert_eq!(serde_json::from_str::<Data>(&text).unwrap(), pong);

    // 旧编辑器不回这一格：要读成 0（= 对面没说），不能因为少一个字段把整份回执判死。
    let legacy = r#"{"kind":"pong","app_version":"0.1.0","protocol":2,"rev":7,"song_session":2}"#;
    match serde_json::from_str::<Data>(legacy).unwrap() {
        Data::Pong { pid, .. } => assert_eq!(pid, 0, "缺 pid 的旧帧该读成「没说」"),
        other => panic!("形状不对: {other:?}"),
    }
}

// ---------------------------------------------------------------- 帧

#[test]
fn tab_ref_round_trips_and_requires_exactly_one_locator() {
    let by_id = crate::bus::TabRef {
        id: Some(42),
        index: None,
    };
    let by_index = crate::bus::TabRef {
        id: None,
        index: Some(2),
    };
    assert_eq!(by_id.validate().unwrap(), by_id);
    assert_eq!(by_index.validate().unwrap(), by_index);
    assert!(crate::bus::TabRef {
        id: None,
        index: None
    }
    .validate()
    .is_err());
    assert!(crate::bus::TabRef {
        id: Some(42),
        index: Some(2),
    }
    .validate()
    .is_err());

    let request = Request {
        id: 9,
        tab: Some(by_id),
        op: Op::GetSnapshot,
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["tab"]["id"], 42);
    assert_eq!(serde_json::from_value::<Request>(value).unwrap(), request);
    assert!(Op::ListTabs.is_read_only());
    assert!(!Op::SwitchTab { tab: by_id }.is_read_only());
    assert!(!Op::CloseTab {
        tab: by_id,
        discard: None,
    }
    .is_read_only());
}

#[test]
fn inbox_preserves_the_optional_tab_envelope() {
    let inbox = Inbox::idle();
    let tab = crate::bus::TabRef {
        id: Some(7),
        index: None,
    };
    let rx = inbox.submit_tab(
        1,
        "test",
        Op::GetNotes {
            tick_from: None,
            tick_to: None,
            layer_from: None,
            layer_to: None,
            ticks_per_bar: None,
            bars: None,
            limit: None,
            max_bytes: None,
            view: None,
        },
        Some(tab),
    );
    let pending = inbox.drain();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tab, Some(tab));
    pending[0].reply(Outcome::done(ChangeReport::default()));
    assert!(rx.recv().unwrap().ok);
}
#[test]
fn frame_round_trip_adds_exactly_one_newline() {
    let mut buf = Vec::new();
    write_json_line(
        &mut buf,
        &Request {
            tab: None,
            id: 7,
            op: Op::JobCancel { id: 3 },
        },
    )
    .unwrap();
    assert!(buf.ends_with(b"\n"));
    assert_eq!(
        buf.iter().filter(|b| **b == b'\n').count(),
        1,
        "内嵌换行会把一帧拆成两帧"
    );

    let mut cursor = Cursor::new(buf);
    let got: Option<Request> = read_json_line(&mut cursor).unwrap();
    assert_eq!(
        got.unwrap(),
        Request {
            tab: None,
            id: 7,
            op: Op::JobCancel { id: 3 }
        }
    );
}

#[test]
fn frame_skips_blank_lines_and_reports_clean_eof() {
    let mut cursor = Cursor::new(b"\n\n{\"id\":1,\"op\":{\"kind\":\"ping\"}}".to_vec());
    let got: Option<Request> = read_json_line(&mut cursor).unwrap();
    assert_eq!(
        got.unwrap(),
        Request {
            tab: None,
            id: 1,
            op: Op::Ping
        }
    );
    let eof: Option<Request> = read_json_line(&mut cursor).unwrap();
    assert!(eof.is_none());
}

#[test]
fn frame_tail_without_newline_still_parses() {
    let mut cursor = Cursor::new(b"{\"id\":2,\"op\":{\"kind\":\"job_list\"}}".to_vec());
    let got: Option<Request> = read_json_line(&mut cursor).unwrap();
    assert_eq!(
        got.unwrap(),
        Request {
            tab: None,
            id: 2,
            op: Op::JobList
        }
    );
}

#[test]
fn frame_bad_json_is_invalid_data_not_a_panic() {
    let mut cursor = Cursor::new(b"{\"id\":x}".to_vec());
    let err = read_json_line::<Cursor<Vec<u8>>, Request>(&mut cursor).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

// ---------------------------------------------------------------- 收件箱

#[test]
fn inbox_wakes_submitter_and_replies_only_once() {
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woken);
    let inbox = Inbox::with_waker(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let rx = inbox.submit(1, "test", Op::Ping);
    assert_eq!(inbox.pending(), 1);
    assert_eq!(
        woken.load(Ordering::SeqCst),
        1,
        "投递必须催帧，否则窗口静止时请求会烂在队列里"
    );

    let reqs = inbox.drain();
    assert_eq!(reqs.len(), 1);
    assert!(inbox.is_empty());
    assert_eq!(reqs[0].client, "test");

    assert!(reqs[0].reply(Outcome::done(report(1))), "第一次回执要成功");
    assert!(
        !reqs[0].reply(Outcome::done(report(1))),
        "重复回执必须安静失败"
    );
    assert!(rx.recv().unwrap().ok);
}

#[test]
fn inbox_reply_to_abandoned_request_returns_false() {
    let inbox = Inbox::idle();
    let rx = inbox.submit(1, "test", Op::Ping);
    drop(rx);
    let reqs = inbox.drain();
    assert!(!reqs[0].reply(Outcome::done(report(1))));
}

// ---------------------------------------------------------------- 任务表

#[test]
fn job_reports_progress_then_result() {
    let registry = JobRegistry::new(2, 8);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let id = registry
        .spawn("test", move |job| {
            job.progress(0.5, "干一半");
            let _ = rx.recv();
            Ok(json!({ "cells": 3 }))
        })
        .unwrap();

    wait_until(
        "任务进入 Running",
        || matches!(registry.status(id), Some(info) if info.status == JobStatus::Running),
    );
    assert_eq!(registry.status(id).unwrap().progress, 0.5);
    assert_eq!(registry.status(id).unwrap().message, "干一半");

    tx.send(()).unwrap();
    wait_until(
        "任务收工",
        || matches!(registry.status(id), Some(info) if info.status == JobStatus::Done),
    );
    let info = registry.status(id).unwrap();
    assert_eq!(info.result, Some(json!({ "cells": 3 })));
    assert_eq!(info.progress, 1.0);
    assert!(info.finished_unix.is_some());
}

#[test]
fn job_progress_is_clamped() {
    let registry = JobRegistry::new(1, 2);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let id = registry
        .spawn("test", move |job| {
            job.progress(9.0, "超了");
            let _ = rx.recv();
            Ok(Value::Null)
        })
        .unwrap();
    wait_until("进度被夹住", || {
        registry
            .status(id)
            .map(|info| info.progress == 1.0)
            .unwrap_or(false)
    });
    tx.send(()).unwrap();
}

#[test]
fn job_cancel_is_cooperative() {
    let registry = JobRegistry::new(2, 4);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let id = registry
        .spawn("test", move |job| {
            let _ = rx.recv(); // 等外部把取消位置好
            if job.cancelled() {
                Err("按取消位收手".to_string())
            } else {
                Ok(Value::Null)
            }
        })
        .unwrap();

    wait_until(
        "任务进入 Running",
        || matches!(registry.status(id), Some(info) if info.status == JobStatus::Running),
    );
    assert_eq!(registry.cancel(id), CancelOutcome::Signalled);
    tx.send(()).unwrap();
    wait_until(
        "任务按取消收手",
        || matches!(registry.status(id), Some(info) if info.status == JobStatus::Cancelled),
    );

    // 已结束的任务再取消是无意义的，不能伪装成成功
    assert_eq!(registry.cancel(id), CancelOutcome::Finished);
    assert_eq!(registry.cancel(9_999), CancelOutcome::NotFound);
}

#[test]
fn job_panics_are_recorded_as_failures() {
    let registry = JobRegistry::new(1, 2);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // 别把刻意制造的 panic 打到测试输出里
    let id = registry.spawn("boom", |_| panic!("任务体炸了")).unwrap();
    wait_until(
        "panic 被兜住",
        || matches!(registry.status(id), Some(info) if info.status == JobStatus::Failed),
    );
    std::panic::set_hook(previous);
    assert_eq!(registry.status(id).unwrap().message, "任务线程异常退出");
}

#[test]
fn job_slot_limit_rejects_extra_work() {
    let registry = JobRegistry::new(1, 4);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    registry
        .spawn("block", move |_| {
            let _ = rx.recv();
            Ok(Value::Null)
        })
        .unwrap();
    wait_until("占住唯一的槽", || registry.running() == 1);
    assert_eq!(
        registry.spawn("second", |_| Ok(Value::Null)),
        Err(SpawnReject::Busy)
    );

    tx.send(()).unwrap();
    wait_until("槽位释放", || registry.running() == 0);
    assert!(registry.spawn("third", |_| Ok(Value::Null)).is_ok());
}

#[test]
fn finished_jobs_are_pruned_to_the_recent_ones() {
    let registry = JobRegistry::new(1, 2);
    let mut ids = Vec::new();
    for _ in 0..4 {
        let id = registry.spawn("quick", |_| Ok(Value::Null)).unwrap();
        ids.push(id);
        wait_until(
            "任务收工",
            || matches!(registry.status(id), Some(info) if info.finished_unix.is_some()),
        );
    }
    let kept: Vec<u64> = registry.list().into_iter().map(|info| info.id).collect();
    assert_eq!(
        kept,
        vec![ids[3], ids[2]],
        "完成的历史只留最近两条，新的在前"
    );
}

// ---------------------------------------------------------------- 发现文件

#[test]
fn discovery_writes_reads_and_only_removes_its_own() {
    let dir = temp_dir("discovery");
    let mine = Discovery {
        protocol: crate::PROTOCOL_VERSION,
        app: "nbs-rs".to_string(),
        pid: std::process::id(),
        port: 45_001,
        token: Discovery::random_token(),
        started_unix: 1,
    };
    let written = mine.write_to(&dir).unwrap();
    assert_eq!(written, dir.join(DISCOVERY_FILE));
    assert_eq!(Discovery::read_from(&dir).unwrap(), mine);

    // 别的实例还活着：token 对不上就不许删
    Discovery::remove_matching(&dir, "别人的 token");
    assert!(written.exists());

    Discovery::remove_matching(&dir, &mine.token);
    assert!(!written.exists());
    assert!(matches!(
        Discovery::read_from(&dir),
        Err(crate::discovery::DiscoveryError::Io(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discovery_tokens_differ_across_calls() {
    assert_ne!(Discovery::random_token(), Discovery::random_token());
}

// ---------------------------------------------------------------- 服务 + 客户端

fn test_config(token: &str, inbox: Inbox) -> ServerConfig {
    let mut cfg = ServerConfig::new(token.to_string(), inbox);
    cfg.app_name = "nbs-test".to_string();
    cfg.app_version = "0.0.0-test".to_string();
    cfg
}

#[test]
fn hello_ack_reports_the_live_parked_write_capability() {
    let inbox = Inbox::idle();
    let mut cfg = test_config("tok", inbox);
    cfg.parked_write = Arc::new(AtomicBool::new(true));
    let server = ControlServer::start(0, cfg).unwrap();
    let stream = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    write_json_line(
        &mut writer,
        &Hello {
            token: "tok".into(),
            protocol: crate::PROTOCOL_VERSION,
            client: "test".into(),
        },
    )
    .unwrap();
    let ack: crate::bus::HelloAck = read_json_line(&mut reader).unwrap().unwrap();
    assert!(ack.hello);
    assert!(ack.capabilities.parked_write);
    let client =
        ControlClient::connect(server.port(), "tok", "test", Duration::from_secs(1)).unwrap();
    assert!(client.capabilities().parked_write);
}
/// 冒充 UI 主线程：轮询收件箱，按 `Op` 回一份固定回执。
fn fake_main_thread(inbox: Inbox, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        for req in inbox.drain() {
            let outcome = match &req.op {
                Op::Ping => Outcome::ok(
                    Data::Pong {
                        app_version: "0.0.0-test".to_string(),
                        protocol: crate::PROTOCOL_VERSION,
                        rev: 42,
                        song_session: 9,
                        pid: 4242,
                    },
                    report(42),
                ),
                Op::GetSnapshot => Outcome::ok(Data::Empty, report(42)),
                other => Outcome::ok(
                    Data::Empty,
                    report(if other.is_read_only() { 42 } else { 43 }),
                ),
            };
            let _ = req.reply(outcome);
        }
        std::thread::sleep(Duration::from_millis(2));
    })
}

#[test]
fn server_answers_requests_serially_on_one_connection() {
    let inbox = Inbox::idle();
    let server_inbox = inbox.clone();
    let mut cfg = test_config("tok", server_inbox);
    cfg.discovery_dir = None;
    let mut server = ControlServer::start(0, cfg).unwrap();
    assert_ne!(server.port(), 0);
    assert_eq!(server.token(), "tok");

    let stop = Arc::new(AtomicBool::new(false));
    let main = fake_main_thread(inbox.clone(), Arc::clone(&stop));

    let mut client =
        ControlClient::connect(server.port(), "tok", "test", Duration::from_secs(2)).unwrap();
    assert_eq!(client.server_app(), "nbs-test 0.0.0-test");
    assert!(client.alive());

    let out = client
        .request(Op::SetNotes {
            cells: vec![CellNote {
                tick: 0,
                layer: 0,
                instrument: 1,
                key: 45,
                velocity: None,
                panning: None,
                pitch: None,
            }],
            label: None,
            expect_rev: Some(42),
        })
        .unwrap();
    assert!(out.ok);
    assert_eq!(out.report.rev, 43, "写操作之后 rev 必须前进");

    // 同一连接的第二条请求看得到前一条的结果，id 也对得上
    let out = client.request(Op::GetSnapshot).unwrap();
    assert!(out.ok);
    assert_eq!(out.report.rev, 42);

    client.close();
    stop.store(true, Ordering::Relaxed);
    main.join().unwrap();
    server.stop();
    assert!(inbox.is_empty(), "回执发完不该留待件");
}

#[test]
fn server_refuses_wrong_token() {
    let inbox = Inbox::idle();
    let mut server = ControlServer::start(0, test_config("right", inbox)).unwrap();
    let err = match ControlClient::connect(server.port(), "wrong", "test", Duration::from_secs(2)) {
        Ok(_) => panic!("错误 token 居然连上了"),
        Err(err) => err,
    };
    assert!(matches!(err, ClientError::Refused(_)), "拿到的是 {err:?}");
    server.stop();
}

#[test]
fn server_refuses_protocol_mismatch_with_a_structured_error() {
    let inbox = Inbox::idle();
    let mut server = ControlServer::start(0, test_config("tok", inbox)).unwrap();
    let port = server.port();

    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    write_json_line(
        &mut writer,
        &Hello {
            token: "tok".to_string(),
            protocol: crate::PROTOCOL_VERSION + 1,
            client: "test".to_string(),
        },
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let resp: Response = read_json_line(&mut reader).unwrap().unwrap();
    assert_eq!(resp.id, 0);
    assert!(!resp.outcome.ok);
    assert_eq!(resp.outcome.error_code(), Some(ErrorCode::NotAllowed));
    server.stop();
}

#[test]
fn server_times_out_when_the_main_thread_is_busy() {
    let inbox = Inbox::idle();
    let mut cfg = test_config("tok", inbox.clone());
    cfg.request_timeout = Duration::from_millis(50);
    let mut server = ControlServer::start(0, cfg).unwrap();

    // 没有主线程取件：请求烂在队列里，服务端必须自己给个交代。
    let mut client =
        ControlClient::connect(server.port(), "tok", "test", Duration::from_secs(2)).unwrap();
    let out = client.request(Op::Ping).unwrap();
    assert!(!out.ok);
    assert_eq!(out.error_code(), Some(ErrorCode::Timeout));
    // 连接没被这次超时打死，还能接着用
    assert_eq!(inbox.pending(), 1);
    client.close();
    server.stop();
}

// ---------------------------------------------------------------- 连接看板

#[test]
fn board_only_counts_a_session_once_the_server_is_open() {
    let board = crate::ClientBoard::default();
    // 没人 open_epoch = 服务没在对外，登记这条路根本不该走得通。
    assert!(board.attach(7, "nbs-mcp", "127.0.0.1:1").is_none());
    assert_eq!(board.count(), 0);

    let epoch = board.open_epoch();
    let s = board.attach(epoch, "nbs-mcp", "127.0.0.1:52133").expect("刚开任期");
    let snap = board.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].client, "nbs-mcp");
    assert_eq!(snap[0].requests, 0, "连上还没调用");
    assert_eq!(snap[0].last_request_unix, None);

    s.requested();
    let snap = board.snapshot();
    assert_eq!(snap[0].requests, 1);
    assert!(snap[0].last_request_unix.is_some(), "调用时刻要报得出来");

    drop(s);
    assert_eq!(board.count(), 0, "连接收尾必须销账");
}

#[test]
fn a_stale_server_cannot_wipe_the_current_ledger() {
    let board = crate::ClientBoard::default();
    let old = board.open_epoch();
    board.close_epoch(old);

    let now = board.open_epoch();
    let _s = board.attach(now, "claude", "127.0.0.1:40001").unwrap();
    // 上一趟服务晚到的收尾（线程还在关连接）不能把这一趟刚登记的连接抹掉。
    board.close_epoch(old);
    assert_eq!(board.count(), 1, "旧任期的 close 不该动新账");

    board.close_epoch(now);
    assert_eq!(board.count(), 0);
    assert!(board.attach(now, "late", "127.0.0.1:40002").is_none());
}

#[test]
fn server_board_shows_the_connected_client_and_its_calls() {
    let inbox = Inbox::idle();
    let server_inbox = inbox.clone();
    let mut cfg = test_config("tok", server_inbox);
    cfg.discovery_dir = None;
    let board = cfg.clients.clone();
    let mut server = ControlServer::start(0, cfg).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let main = fake_main_thread(inbox.clone(), Arc::clone(&stop));
    assert!(board.snapshot().is_empty(), "起服不等于有人连着");

    let mut client =
        ControlClient::connect(server.port(), "tok", "probe-ide", Duration::from_secs(2)).unwrap();
    wait_until("看板出现这条连接", || board.count() == 1);
    let s = &board.snapshot()[0];
    assert_eq!(s.client, "probe-ide", "握手自报的名字要原样显示");
    assert!(s.peer.starts_with("127.0.0.1:"), "对端 {}", s.peer);
    assert_eq!(s.requests, 0);

    client.request(Op::Ping).unwrap();
    wait_until("调用次数记上", || board.snapshot()[0].requests == 1);
    let s = &board.snapshot()[0];
    assert!(s.last_request_unix.is_some());
    assert!(s.connected_unix <= s.last_request_unix.unwrap());

    client.close();
    wait_until("断线后销账", || board.snapshot().is_empty());

    stop.store(true, Ordering::Relaxed);
    main.join().unwrap();
    server.stop();
    assert!(board.snapshot().is_empty());
}

#[test]
fn server_board_ignores_connections_that_never_handshook() {
    let inbox = Inbox::idle();
    let mut cfg = test_config("tok", inbox);
    cfg.discovery_dir = None;
    let board = cfg.clients.clone();
    let mut server = ControlServer::start(0, cfg).unwrap();
    let port = server.port();

    // 扫端口 / token 不对的进程不该显示成「有 AI 连着」，否则这一栏就是在骗人。
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    write_json_line(
        &mut writer,
        &Hello {
            token: "wrong".to_string(),
            protocol: crate::PROTOCOL_VERSION,
            client: "scanner".to_string(),
        },
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    // 握手没过时服务端回的是**一条普通回执**（id 0 + ok:false），不是 HelloAck。
    let resp: Response = read_json_line(&mut reader).unwrap().unwrap();
    assert!(!resp.outcome.ok, "token 不对必须先被挡下");
    assert_eq!(resp.outcome.error_code(), Some(ErrorCode::NotAllowed));
    assert_eq!(board.count(), 0, "没过握手不能登记");

    // 光连上就放着（一句都没发）也算没登记：登记发生在握手之后。
    let _idle = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    assert_eq!(board.count(), 0);
    server.stop();
}

#[test]
fn a_stopped_server_stops_serving_connections_that_stayed_open() {
    let inbox = Inbox::idle();
    let server_inbox = inbox.clone();
    let mut cfg = test_config("tok", server_inbox);
    cfg.discovery_dir = None;
    let board = cfg.clients.clone();
    let mut server = ControlServer::start(0, cfg).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let main = fake_main_thread(inbox.clone(), Arc::clone(&stop));
    let mut client =
        ControlClient::connect(server.port(), "tok", "probe-ide", Duration::from_secs(2)).unwrap();
    wait_until("连接登记好", || board.count() == 1);
    client.request(Op::Ping).unwrap();

    // 用户在面板上取消勾选：端口关了还不够，**还连着的那条**也不能再使唤编辑器。
    server.stop();
    assert!(board.snapshot().is_empty(), "停服当场清空看板");
    let err = match client.request(Op::Ping) {
        Ok(out) => panic!("停服之后这条连接居然还能用：{out:?}"),
        Err(err) => err,
    };
    assert!(matches!(err, ClientError::Closed), "拿到的是 {err:?}");
    assert!(inbox.is_empty(), "停服后的请求不该投给主线程执行");

    stop.store(true, Ordering::Relaxed);
    main.join().unwrap();
}

/// 音频转 MIDI 的四条：走字节往返（Value 往返会把重复键静默吞掉，这是阶段一那条教训），
/// 拼错的字段名不许静默变成「省略」，四条都只起任务、不改工程。
#[test]
fn the_transcription_ops_round_trip_and_refuse_typo_fields() {
    let ops = [
        Op::TranscribeProbe,
        Op::TranscribePrepareEnv { cuda: None },
        Op::TranscribePrepareEnv { cuda: Some(true) },
        Op::TranscribePrepareModel {
            options: TranscribeOptions {
                model: Some("large".into()),
                instruments: Some(vec!["acoustic_piano".into(), "voice".into()]),
                ..Default::default()
            },
        },
        Op::TranscribeAudio {
            audio: "a/b.wav".into(),
            options: TranscribeOptions {
                beam_size: Some(4),
                strict_eos: Some(false),
                ..Default::default()
            },
            save_midi: Some("a/b.mid".into()),
            overwrite: true,
        },
        Op::TranscribeAudio {
            audio: "a/b.wav".into(),
            options: TranscribeOptions::default(),
            save_midi: None,
            overwrite: false,
        },
    ];
    for op in &ops {
        let text = serde_json::to_string(op).unwrap();
        let back: Op =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e} 读不回来：{text}"));
        assert_eq!(&back, op);
        // 转录的四条会起本机子进程，不是「读」；但它们都不改工程，弹窗挡路时照样该回答。
        assert!(op.only_spawns_a_job(), "{op:?} 该和 export 一样豁免弹窗闸");
        assert!(!op.is_read_only(), "{op:?} 会动本机，不该按只读放行");
    }
    // 交付路径省略时字节里不该出现它，否则「没给」和「给了空」就分不开。
    let bare = serde_json::to_string(&Op::TranscribeAudio {
        audio: "a.wav".into(),
        options: TranscribeOptions::default(),
        save_midi: None,
        overwrite: false,
    })
    .unwrap();
    assert!(!bare.contains("save_midi"), "{bare}");

    for typo in [
        r#"{"temperture":1.0}"#,
        r#"{"device_1":"cpu"}"#,
        r#"{"beem_size":2}"#,
    ] {
        let err = serde_json::from_str::<TranscribeOptions>(typo)
            .err()
            .unwrap_or_else(|| panic!("{typo} 居然读得动"));
        assert!(
            err.to_string().contains("unknown field"),
            "{typo} 的报错不像字段问题：{err}"
        );
    }
    // 任务 kind 的字符串是闸数在册量的依据，拼错一个字母那道闸就形同虚设。
    assert_eq!(
        JobKind::TranscribePrepareEnv.as_str(),
        "transcribe_prepare_env"
    );
    assert!(JobKind::TranscribeAudio.is_transcription());
    assert!(!JobKind::Export.is_transcription());
    let mut strs: Vec<&str> = JobKind::ALL.iter().map(|kind| kind.as_str()).collect();
    strs.sort();
    strs.dedup();
    assert_eq!(strs.len(), JobKind::ALL.len(), "kind 字符串撞车了：{strs:?}");
}

/// 只讲协议、不碰编辑器的假服务端：认下握手，收到第一问后晚 `delay` 才回。
///
/// `hold_open` = 答完再撑一会儿才关。这一条是给「迟到的回执」那个现场用的：
/// 服务端一答就断开，连接被关掉会替「不作废连接」遮掩住缺陷，测试就咬不动了。
/// `answer` = false 时握完手一句回执都不发就把连接丢掉（真窗口上就是空闲 60s 被服务端关掉）。
fn fake_server(
    answer: bool,
    delay: Duration,
    hold_open: bool,
) -> (u16, Arc<std::sync::Mutex<Vec<u64>>>, std::thread::JoinHandle<()>) {
    use crate::bus::HelloAck;

    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let answered: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let sink = Arc::clone(&answered);
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let _hello: Hello = read_json_line(&mut reader).unwrap().unwrap();
        write_json_line(
            &mut writer,
            &HelloAck {
                hello: true,
                protocol: crate::PROTOCOL_VERSION,
                app: "nbs-fake 0.0.0-test".to_string(),
                capabilities: Default::default(),
                snapshot: None,
            },
        )
        .unwrap();
        let req: Request = read_json_line(&mut reader).unwrap().unwrap();
        if !answer {
            // 一问不答就走：下一问写出去就再没人接。
            return;
        }
        std::thread::sleep(delay);
        sink.lock().unwrap().push(req.id);
        // 本端这时候可能早就不等了：写得成写不成都不算服务端错，别 unwrap。
        let _ = write_json_line(
            &mut writer,
            &Response {
                id: req.id,
                outcome: Outcome::done(report(1)),
            },
        );
        if hold_open {
            std::thread::sleep(Duration::from_millis(1_500));
        }
    });
    (port, answered, server)
}

/// 现场：编辑器主线程排队到 600ms 才回，而本端只等 200ms（真窗口上是「正在跑导出」）。
#[test]
fn a_reply_later_than_the_wait_does_not_answer_the_next_request() {
    let (port, answered, server) = fake_server(true, Duration::from_millis(600), true);
    let mut client = ControlClient::connect_with_reply_wait(
        port,
        "",
        "test",
        Duration::from_secs(2),
        Duration::from_millis(200),
    )
    .unwrap();
    let first = client.request(Op::Ping);
    assert!(
        matches!(first, Err(ClientError::TimedOut)),
        "等满上限要说成「没按时回答」，实际：{first:?}"
    );

    // 等那张回执真的落到本端的接收缓冲里再问第二句。不这么排，第二问会在它到达之前
    // 就自己超时，正好错过要钉的那个缺陷。
    std::thread::sleep(Duration::from_millis(750));

    // 这一问是整条修复的重点：第二问绝不能把第一问那张迟到回执当成自己的答案。
    // 旧 rev、旧计数读起来像「答对了」，比直接报错危险得多。
    let second = client.request(Op::GetSnapshot);
    match second {
        Err(ClientError::Closed) | Err(ClientError::Unreachable(_)) | Err(ClientError::TimedOut) => {}
        other => panic!("第二问不该拿到东西，实际：{other:?}"),
    }
    server.join().unwrap();
    assert_eq!(*answered.lock().unwrap(), vec![1], "服务端只该收到第一问");
}

/// 对端已经不认这条连接了（真窗口上就是空闲 60s 被服务端关掉）。
#[test]
fn a_dropped_link_says_closed_instead_of_bad_reply() {
    let (port, _answered, server) = fake_server(false, Duration::ZERO, false);

    let mut client = ControlClient::connect_with_reply_wait(
        port,
        "",
        "test",
        Duration::from_secs(2),
        Duration::from_millis(200),
    )
    .unwrap();
    let out = client.request(Op::Ping);
    assert!(
        matches!(out, Err(ClientError::Closed)),
        "连接断了要说成「断了」，报成 BadReply 会把人支去改参数：{out:?}"
    );
    server.join().unwrap();
}
