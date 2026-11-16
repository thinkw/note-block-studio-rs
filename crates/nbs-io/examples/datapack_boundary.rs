//! 临时探针：数据包超音域导出链的音高对齐核对。不属于产品代码。
use nbs_core::model::{CustomInstrument, Note, Song};
use nbs_io::datapack::{generate, DatapackOptions};

fn opts(name: &str, extra: bool) -> DatapackOptions {
    let mut o = DatapackOptions::default();
    o.name = name.into();
    o.namespace = name.into();
    o.include_out_of_range = extra;
    o
}

fn first_line(plan: &nbs_io::datapack::Plan, suffix: &str) -> String {
    plan.files
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .map(|f| f.content.lines().next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// 命令里的倍率 -> 相对基准采样的半音。
fn semitones_from_ratio(r: f64) -> f64 {
    12.0 * r.log2()
}

/// 从 "playsound <event> <src> <target> ... <volume> <pitch> 1" 里取事件名与倍率。
fn parse_playsound(line: &str) -> Option<(String, f64)> {
    let mut it = line.split_whitespace();
    if it.next() != Some("playsound") {
        return None;
    }
    let event = it.next()?.to_string();
    let pitch: f64 = line.split_whitespace().rev().nth(1)?.parse().ok()?;
    Some((event, pitch))
}

/// 探针实测的资源包位移：`x_-1` 比基准低 24 半音，`x_1` 比基准高 24 半音。
fn content_shift(event: &str) -> Option<f64> {
    if event.ends_with("_-1") {
        Some(-24.0)
    } else if event.ends_with("_1") {
        Some(24.0)
    } else {
        Some(0.0)
    }
}

fn main() {
    let cases: [(u8, &str); 5] = [
        (0, "harp"),
        (1, "bass"),
        (2, "basedrum"),
        (16, "trumpet"),
        (20, "custom:piano"),
    ];
    let keys = [8i32, 9, 10, 32, 33, 45, 56, 57, 58, 80, 81, 82];

    println!(
        "乐器        key  默认导出  扩展导出事件名                                   倍率     位移   游戏内反推  编辑器公式反推"
    );
    for (ins, label) in cases {
        for k in keys {
            let mut s = Song::new();
            s.layers.push(Default::default());
            s.custom_instruments.push(CustomInstrument {
                name: "piano".into(),
                filename: String::new(),
                key: 45,
                press: true,
            });
            s.set(0, 0, Note::new(ins, k as u8));

            let def = generate(&s, &opts("p", false), "p");
            let def_line = first_line(&def, "notes/0.mcfunction");
            let def_played = def_line.contains("playsound");

            let ext = generate(&s, &opts("p", true), "p");
            let ext_line = first_line(&ext, "notes/0.mcfunction");
            let parsed = parse_playsound(&ext_line);

            let (ev_cell, ratio_cell, shift_cell, ok_cell) = match &parsed {
                None => ("—丢弃—".to_string(), "—".to_string(), "—".to_string(), "—".to_string()),
                Some((event, r)) => {
                    let shift = content_shift(event).unwrap_or(0.0);
                    let implied = 45.0 + shift + semitones_from_ratio(*r);
                    let ev = format!("{event}");
                    let custom = ins >= 20;
                    let covered = !custom && (shift == 0.0 || event.contains("_-1") || event.contains("_1"));
                    let mark = if (implied - k as f64).abs() < 0.02 {
                        if custom && shift != 0.0 {
                            "音高对/事件缺失".to_string()
                        } else {
                            "对".to_string()
                        }
                    } else {
                        format!("错({:+.2})", implied - k as f64)
                    };
                    let _ = covered;
                    (ev, format!("{r:.6}"), format!("{shift:+.0}"), mark)
                }
            };

            // 编辑器侧：mixer.rs 的 playback_ratio = 0.5 * 2^((key-33)/24... ) 按公式复算
            let ed_ratio = 0.5 * 2f64.powf((k as f64 - 33.0) / 12.0);
            let ed_implied = 45.0 + semitones_from_ratio(ed_ratio);
            let ed_mark = if (ed_implied - k as f64).abs() < 0.02 {
                "对".to_string()
            } else {
                format!("错({:+.2})", ed_implied - k as f64)
            };

            println!(
                "{label:11} {k:>3}  {:<7}  {ev_cell:44} {ratio_cell:9} {shift_cell:>5}  {ok_cell:10}  {ed_mark}",
                if def_played { "有命令" } else { "丢弃" }
            );
        }
        println!();
    }

    // 下游成本：可视化列/team 跟乐器还是跟层
    println!("== 可视化列数与命令数：同乐器多层 vs 异乐器多层 ==");
    let mk = |specs: &[(u8, u8)]| {
        let mut s = Song::new();
        for _ in 0..specs.len() {
            s.layers.push(Default::default());
        }
        for (i, (ins, k)) in specs.iter().enumerate() {
            s.set(0, i as u32, Note::new(*ins, *k));
        }
        s
    };
    let report = |tag: &str, s: &Song| {
        let mut o = opts("v", true);
        o.visualizer = true;
        let plan = generate(s, &o, "v");
        let all: String = plan
            .files
            .iter()
            .map(|f| f.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let summons = all.matches("summon minecraft:falling_block").count();
        let teams = all.matches("team join").count();
        let plays = all.lines().filter(|l| l.contains("playsound")).count();
        let mut cols: Vec<String> = all
            .lines()
            .filter(|l| l.contains("summon minecraft:falling_block"))
            .map(|l| {
                let at = l.split(" run ").last().unwrap_or(l);
                at.split(" Time:").next().unwrap_or(at).to_string()
            })
            .collect();
        cols.sort();
        cols.dedup();
        println!(
            "{tag}: playsound {} 条 | summon {} 条 | 去重后排布位置 {} | team join {} | 音符 {}",
            plays,
            summons,
            cols.len(),
            teams,
            s.iter_notes().count()
        );
    };
    report("4 层全用 harp(0) ", &mk(&[(0, 45), (0, 47), (0, 49), (0, 51)]));
    report("4 层各异乐器      ", &mk(&[(0, 45), (1, 47), (2, 49), (16, 51)]));
    report("2 层，同音同乐器  ", &mk(&[(0, 45), (0, 45)]));
}
