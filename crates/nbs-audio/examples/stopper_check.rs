//! 对真实 .nbs 数一遍事件乐器：消音器（Sound Stopper）层区间、变速器（Tempo Changer）。
//!
//! 运行：`cargo run -p nbs-audio --example stopper_check -- <文件.nbs>`
//! 对照 `control_step.gml:283-291`（事件乐器派发）与 `remove_emitters_all.gml`（消音区间）。

use nbs_audio::{build_event_table, EventKind};
use nbs_core::nbs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: cargo run -p nbs-audio --example stopper_check -- <文件.nbs>");
        std::process::exit(2);
    }
    for path in args {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let song = match nbs::load_bytes(&bytes) {
            Ok(s) => s,
            Err(e) => {
                println!("{path}: 打不开 {e}");
                continue;
            }
        };
        for include_locked in [false, true] {
            let events = build_event_table(&song, include_locked);
            let stoppers: Vec<&nbs_audio::NoteEvent> = events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::Stopper(..)))
                .collect();
            let tempos: Vec<&nbs_audio::NoteEvent> = events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::Tempo(..)))
                .collect();
            let notes = events
                .iter()
                .filter(|e| e.kind == EventKind::Note)
                .count();
            println!(
                "{}  include_locked={}  事件 {} 条：音符 {}、消音 {}、变速 {}",
                path.rsplit(['/', '\\']).next().unwrap_or(&path),
                include_locked,
                events.len(),
                notes,
                stoppers.len(),
                tempos.len()
            );
            for e in stoppers.iter().take(3) {
                if let EventKind::Stopper(first, last) = e.kind {
                    println!("    首个消音器：tick {} 层 {} → 掐 [{}, {}]", e.tick, e.layer, first, last);
                }
            }
            for e in tempos.iter().take(3) {
                if let EventKind::Tempo(pitch) = e.kind {
                    println!(
                        "    首个变速器：tick {} 层 {} → pitch {}（BPM {} → tempo {:.2} t/s）",
                        e.tick,
                        e.layer,
                        pitch,
                        pitch.unsigned_abs(),
                        pitch.unsigned_abs() as f32 / 15.0
                    );
                }
            }
        }
    }
}
