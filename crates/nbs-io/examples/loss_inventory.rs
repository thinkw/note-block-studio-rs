//! 临时探针：转换损失清单（只读分析，不改产品代码）。不属于产品代码。
//! cargo run --release --example loss_inventory -p nbs-io
//!
//! 口径：
//! - MIDI：`for_midi`（GM 默认表 + 出厂标量）导入，另跑一档 `keep_octave_range`；
//!   格子占位直接查真实 `song.has`，结尾用 `placed == song 音符数` 自证复算无漂移。
//! - NBS：原样读入，无导入损失项。
//! - 导出：数据包两档（普通 / 含超范围），逐条 playsound 按事件名归类。
use nbs_core::model::{MC_KEY_MAX, MC_KEY_MIN, Song};
use nbs_core::nbs;
use nbs_io::datapack::{generate, DatapackOptions};
use nbs_io::midi::{import_midi_data, parse_midi_path, MidiData, MidiImportOptions};
use nbs_io::midi_gm::MidiGmTable;
use std::path::{Path, PathBuf};

const BUILTIN: [&str; 20] = [
    "harp", "bass", "basedrum", "snare", "hat", "guitar", "flute", "bell", "chime",
    "xylophone", "iron_xylophone", "cow_bell", "didgeridoo", "bit", "banjo", "pling",
    "trumpet", "trumpet_exposed", "trumpet_weathered", "trumpet_oxidized",
];
/// 资源包内 base 录音与内置采样不同源的 8 件：扩展段游戏内音色 ≠ 编辑器试听。
const TIER_B: [&str; 8] = [
    "guitar", "flute", "bell", "chime", "trumpet", "trumpet_exposed", "trumpet_weathered",
    "trumpet_oxidized",
];
const KEY_MAX: i32 = 87;
const BUILTIN_PREFIX: &str = "minecraft:block.note_block.";

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        " -".into()
    } else {
        format!("{:.1}%", 100.0 * n as f32 / d as f32)
    }
}

// ---------------------------------------------------------------- 导入侧

#[derive(Default)]
struct ImportStats {
    total: usize,
    ch_muted: usize,
    perc_muted: usize,
    neg_start: usize,
    layer_cut: usize,
    clamped: usize,
    folded: usize,
    native_pitch: usize,
    off_native: usize,
    head_all_lost: usize,
    head_part_lost: usize,
    blocked_cells: usize,
    placed: usize,
}

/// 与 midi.rs 的 resolve/两遍扫描逐条对齐的复算。格子占位维护在探针自己的集合里
/// （导入后的 song 已含全部音符，不能再查它），结尾用 `placed == song 音符数` 自证复算无漂移。
fn import_stats(midi: &MidiData, opts: &MidiImportOptions, song: &Song) -> ImportStats {
    use std::collections::HashSet;
    let mut grid: HashSet<(u32, u32)> = HashSet::new();
    let dpt = midi.deltapertick(opts.precision).max(1e-9);
    let last_channel = midi.channels as usize;

    struct EvRef {
        track: usize,
        index: usize,
        start: i64,
        finish: i64,
        order: usize,
    }
    // 每事件的 resolve 结果（确定性，两遍一致）：(pos, until, 终值 key, include_duration)
    let mut resolved: Vec<Vec<Option<(i64, i64, i32, bool)>>> =
        midi.tracks.iter().map(|t| vec![None; t.events.len()]).collect();
    let mut per_channel: Vec<Vec<EvRef>> = (0..last_channel + 1).map(|_| Vec::new()).collect();
    let mut eventlayer: Vec<Vec<i32>> =
        midi.tracks.iter().map(|t| vec![-1i32; t.events.len()]).collect();
    let mut st = ImportStats::default();

    // —— 第一遍：归类 + 登记 ——
    for (t, track) in midi.tracks.iter().enumerate() {
        for (e, ev) in track.events.iter().enumerate() {
            st.total += 1;
            let channel = ev.channel as usize;
            let pos = midi.quantize(ev.pos, opts.remove_silent, dpt);
            let until = midi.quantize(ev.until, opts.remove_silent, dpt);
            let (ins, mut note, raw_hit_edge) = if channel == 9 {
                match midi
                    .perc_notes
                    .iter()
                    .position(|&n| n == ev.note)
                    .and_then(|i| {
                        Some((opts.perc_ins.get(i).copied()?, opts.perc_pitch.get(i).copied()?))
                    }) {
                    Some((perc_ins, pitch)) => {
                        if perc_ins == -1 {
                            st.perc_muted += 1;
                        }
                        let raw = pitch as i32;
                        (perc_ins, raw.clamp(0, KEY_MAX), raw < 0 || raw > KEY_MAX)
                    }
                    None => {
                        st.perc_muted += 1;
                        (-1, 0, false)
                    }
                }
            } else {
                let ci = opts.channel_ins[channel];
                if ci == -1 {
                    st.ch_muted += 1;
                }
                let raw = ev.note as i32 - 21 + 12 * opts.channel_octave[channel] as i32;
                (ci, raw.clamp(0, KEY_MAX), raw < 0 || raw > KEY_MAX)
            };
            if ins < 0 {
                continue; // 停用：原因已按通道/鼓分开计
            }
            if raw_hit_edge {
                st.clamped += 1;
            }
            if opts.keep_octave_range {
                let before = note;
                while note < MC_KEY_MIN as i32 {
                    note += 12;
                }
                while note > MC_KEY_MAX as i32 {
                    note -= 12;
                }
                if note != before {
                    st.folded += 1;
                }
            }
            if pos < 0 {
                st.neg_start += 1;
                continue;
            }
            let include_duration = opts.note_duration
                && channel != 9
                && ev.patch < 128
                && opts.part_durations.get(ev.part).copied().unwrap_or(false)
                && ev.until != -1
                && until > pos;
            let finish = if include_duration { until } else { pos + 1 };
            resolved[t][e] = Some((pos, until, note, include_duration));
            let order = per_channel[channel].len();
            per_channel[channel].push(EvRef {
                track: t,
                index: e,
                start: pos,
                finish,
                order,
            });
        }
    }

    // —— 声部分配（assign_layers 同款贪心）——
    let unlimited = opts.max_height >= 20;
    let mut channel_height = vec![0usize; per_channel.len()];
    for (channel, events) in per_channel.iter_mut().enumerate() {
        events.sort_by_key(|e| (e.start, e.order));
        let mut layer_ends: Vec<i64> = Vec::new();
        for entry in events.iter() {
            let mut slot = 0usize;
            while slot < layer_ends.len() && layer_ends[slot] > entry.start {
                slot += 1;
            }
            if !unlimited && slot >= opts.max_height as usize {
                st.layer_cut += 1;
                continue;
            }
            if slot == layer_ends.len() {
                layer_ends.push(entry.finish);
            } else {
                layer_ends[slot] = entry.finish;
            }
            eventlayer[entry.track][entry.index] = slot as i32;
        }
        channel_height[channel] = layer_ends.len();
    }

    // —— 第二遍：格子占位（真实 song.has 为准）——
    for (t, track) in midi.tracks.iter().enumerate() {
        for (e, ev) in track.events.iter().enumerate() {
            let Some((pos, until, note, include_duration)) = resolved[t][e] else {
                continue;
            };
            if ev.channel as usize > last_channel || eventlayer[t][e] < 0 {
                continue;
            }
            let mut yy = eventlayer[t][e] as usize;
            yy += channel_height[..ev.channel as usize].iter().sum::<usize>();
            let forvalue = if include_duration { until } else { pos + 1 };
            let mut placed_any = false;
            let mut head_blocked = false;
            let mut i = pos;
            while i < forvalue {
                if !grid.insert((i as u32, yy as u32)) {
                    st.blocked_cells += 1;
                    if i == pos {
                        head_blocked = true;
                    }
                } else {
                    st.placed += 1;
                    placed_any = true;
                }
                i += 1;
            }
            if head_blocked {
                if placed_any {
                    st.head_part_lost += 1;
                } else {
                    st.head_all_lost += 1;
                }
            } else if placed_any {
                let k = note.clamp(0, KEY_MAX) as u8;
                if (MC_KEY_MIN..=MC_KEY_MAX).contains(&k) {
                    st.native_pitch += 1;
                } else {
                    st.off_native += 1;
                }
            }
        }
    }
    st
}

fn print_import(tag: &str, st: &ImportStats, song: &Song) {
    let real = song.iter_notes().count();
    let drift = if st.placed == real { "" } else { "  <<复算漂移!" };
    println!(
        "[导入|{tag}] 源事件 {} | 通道停用 {}({}) 鼓停用 {}({}) 负起点 {}({}) 层限 {}({}) | 音头被占 {}({})（全失 {}、截尾 {}）、被占格子 {} | 落格 {}({})：原生音域 {}({}) 界外 {}({})，其中截断 {} 折音 {} | 产出音符 {}{}",
        st.total,
        st.ch_muted + st.perc_muted,
        pct(st.ch_muted + st.perc_muted, st.total),
        st.perc_muted,
        pct(st.perc_muted, st.total),
        st.neg_start,
        pct(st.neg_start, st.total),
        st.layer_cut,
        pct(st.layer_cut, st.total),
        st.head_all_lost + st.head_part_lost,
        pct(st.head_all_lost + st.head_part_lost, st.total),
        st.head_all_lost,
        st.head_part_lost,
        st.blocked_cells,
        st.placed,
        pct(st.placed, st.total),
        st.native_pitch,
        pct(st.native_pitch, st.placed),
        st.off_native,
        pct(st.off_native, st.placed),
        st.clamped,
        st.folded,
        real,
        drift,
    );
}

// ---------------------------------------------------------------- 导出侧

#[derive(Default)]
struct ExportStats {
    notes: usize,
    commands: usize,
    native: usize,
    ext: usize,
    ext_timbre: usize,
    ext_custom: usize,
    custom_native: usize,
}

fn export_stats(song: &Song, include_out_of_range: bool) -> ExportStats {
    let mut o = DatapackOptions::default();
    o.name = "inv".into();
    o.namespace = "inv".into();
    o.include_out_of_range = include_out_of_range;
    let plan = generate(song, &o, "inv");
    let mut es = ExportStats { notes: song.iter_notes().count(), ..Default::default() };
    for f in &plan.files {
        for line in f.content.lines() {
            let toks: Vec<&str> = line.split_whitespace().collect();
            let Some(pi) = toks.iter().position(|t| *t == "playsound") else {
                continue;
            };
            let Some(event) = toks.get(pi + 1) else {
                continue;
            };
            es.commands += 1;
            let (base, is_ext) = match event.strip_suffix("_-1").or(event.strip_suffix("_1")) {
                Some(b) => (b, true),
                None => (*event, false),
            };
            if let Some(name) = base.strip_prefix(BUILTIN_PREFIX) {
                if is_ext {
                    es.ext += 1;
                    if TIER_B.contains(&name) {
                        es.ext_timbre += 1;
                    }
                } else {
                    es.native += 1;
                }
            } else if is_ext {
                es.ext_custom += 1;
            } else {
                es.custom_native += 1;
            }
        }
    }
    es
}

fn print_export(tag: &str, es: &ExportStats) {
    println!(
        "[导出|{tag}] 音符 {} → 命令 {} | 原生 {}({}) | 扩展 {}({})（8 件音色差 {}、自定义无事件 {}）| 自定义原生 {} | 丢 {}({})",
        es.notes,
        es.commands,
        es.native,
        pct(es.native, es.notes),
        es.ext,
        pct(es.ext, es.notes),
        es.ext_timbre,
        es.ext_custom,
        es.custom_native,
        es.notes - es.commands,
        pct(es.notes - es.commands, es.notes),
    );
}

fn print_instruments(song: &Song) {
    let mut counts = [0usize; 256];
    for (_, _, n) in song.iter_notes() {
        counts[n.instrument as usize] += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for (i, c) in counts.iter().enumerate() {
        if *c == 0 {
            continue;
        }
        let label = if (i as u8) < FIRST_CUSTOM_INDEX_FALLBACK {
            BUILTIN[i].to_string()
        } else {
            let ci = i - FIRST_CUSTOM_INDEX_FALLBACK as usize;
            song.custom_instruments
                .get(ci)
                .map(|x| format!("自定义:{}", x.name))
                .unwrap_or_else(|| format!("自定义#{ci}"))
        };
        parts.push(format!("{label}×{c}"));
    }
    println!("[乐器] {}", if parts.is_empty() { "（无音符）".into() } else { parts.join(" ") });
}

const FIRST_CUSTOM_INDEX_FALLBACK: u8 = 20;

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/Songs");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("assets/Songs 不存在")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            matches!(p.extension().and_then(|e| e.to_str()), Some("mid" | "nbs"))
        })
        .collect();
    files.sort();

    for path in &files {
        let _stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        println!("== {} ==", path.file_name().unwrap_or_default().to_string_lossy());
        match path.extension().and_then(|e| e.to_str()) {
            Some("mid") => {
                let midi = match parse_midi_path(path) {
                    Ok(m) => m,
                    Err(e) => {
                        println!("  解析失败: {e}");
                        continue;
                    }
                };
                let gm = MidiGmTable::default();
                let opts = MidiImportOptions::for_midi(&midi, &gm);
                let song = import_midi_data(&midi, &opts);
                let st = import_stats(&midi, &opts, &song);
                print_import("默认", &st, &song);
                print_instruments(&song);
                print_export("普通", &export_stats(&song, false));
                print_export("扩展", &export_stats(&song, true));

                let mut opts2 = opts.clone();
                opts2.keep_octave_range = true;
                let song2 = import_midi_data(&midi, &opts2);
                let st2 = import_stats(&midi, &opts2, &song2);
                print_import("折八度", &st2, &song2);
                print_export("普通", &export_stats(&song2, false));
                print_export("扩展", &export_stats(&song2, true));
            }
            _ => {
                let song = match nbs::load(path) {
                    Ok(s) => s,
                    Err(e) => {
                        println!("  读取失败: {e}");
                        continue;
                    }
                };
                print_instruments(&song);
                print_export("普通", &export_stats(&song, false));
                print_export("扩展", &export_stats(&song, true));
            }
        }
        println!();
    }
    let _ = Path::new("unused");
}
