//! 对照工具：同一个 .mid，用我们的导入 vs 原版导出的 .nbs，逐格比差异。
//! 用法：cargo run -p nbs-io --offline --example midi_compare -- <a.mid> <原版a.nbs>

use std::collections::BTreeMap;

use nbs_core::model::Song;
use nbs_io::midi::{import_midi_data, parse_midi, MidiImportOptions};
use nbs_io::MidiGmTable;

/// 把装机版 `settings_dev.ini` 的 [midi_import] 读进 GM 表，用来验证
/// 「装机版默认映射」和「仓库 GML 默认映射」到底哪套能复现原版的导出。
fn apply_installed_ini(gm: &mut MidiGmTable, path: &str) {
    let text = std::fs::read_to_string(path).expect("读不到 ini");
    let sec = text
        .split("[midi_import]")
        .nth(1)
        .and_then(|s| s.split("\n[").next())
        .expect("ini 里没有 [midi_import]");
    let mut map = std::collections::HashMap::new();
    for line in sec.lines() {
        if let Some((k, v)) = line
            .trim()
            .split_once('=')
            .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        {
            map.insert(k, v);
        }
    }
    let num = |k: &str| -> Option<i8> { map.get(k).and_then(|v| v.parse::<i8>().ok()) };
    for p in 0..128u8 {
        if let (Some(ins), Some(oct)) = (num(&format!("ins_{p}")), num(&format!("ins_{p}_oct"))) {
            gm.set_patch(p, ins, oct);
        }
    }
    for d in 24..88u8 {
        if let (Some(ins), Some(key)) =
            (num(&format!("drum_{d}")), num(&format!("drum_{d}_key")))
        {
            gm.set_drum(d, ins, key);
        }
    }
}


fn histogram(song: &Song) -> BTreeMap<u8, usize> {
    let mut m = BTreeMap::new();
    for (_, _, n) in song.iter_notes() {
        *m.entry(n.instrument).or_insert(0) += 1;
    }
    m
}

fn key_hist(song: &Song) -> BTreeMap<u8, usize> {
    let mut m = BTreeMap::new();
    for (_, _, n) in song.iter_notes() {
        *m.entry(n.key).or_insert(0) += 1;
    }
    m
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mid = args.next().expect("需要 .mid 路径");
    let refn = args.next().expect("需要原版 .nbs 路径");
    let mut gm = MidiGmTable::default();
    if let Some(ini) = args.next() {
        apply_installed_ini(&mut gm, &ini);
    }
    let data = std::fs::read(&mid).expect("读不到 mid");
    let midi = parse_midi(&data).expect("解析失败");
    let opts = MidiImportOptions::for_midi(&midi, &gm);
    println!(
        "解析：{} 轨道，{} 事件，通道 0..={}，鼓音 {} 个，PPQ={}，minpos={} maxpos={}，时长={:.3}s",
        midi.tracks.len(),
        midi.tracks.iter().map(|t| t.events.len()).sum::<usize>(),
        midi.channels,
        midi.perc_notes.len(),
        midi.ppq(),
        midi.minpos,
        midi.maxpos,
        midi.song_length(false, opts.remove_silent)
    );
    println!(
        "tempo 依据：micsecqn={}，tempo meta={:?}",
        midi.micsecqn, midi.tempo_changers
    );
    let cache = midi.duration_cache();
    println!(
        "时长缓存：无速度变化 [{:.3}, {:.3}]，有速度变化 [{:.3}, {:.3}]",
        cache[0][0], cache[0][1], cache[1][0], cache[1][1]
    );
    println!(
        "声部 midi_parts={}，鼓音表条目={}，配到 note-off 的音={} / {}",
        midi.parts.len(),
        midi.perc_notes.len(),
        midi.tracks.iter().flat_map(|t| t.events.iter()).filter(|e| e.until != -1).count(),
        midi.tracks.iter().map(|t| t.events.len()).sum::<usize>()
    );
    let ours = import_midi_data(&midi, &opts);
    let theirs = nbs_core::nbs::load(std::path::Path::new(&refn)).expect("读取原版失败");

    let row = |tag: &str, a: &str, b: &str| {
        println!("{tag:<16}{:>24}{:>24}", a, b);
    };
    println!("{:<16}{:>24}{:>24}", "", "我们的导入", "原版导出");
    row("tempo", &ours.meta.tempo.to_string(), &theirs.meta.tempo.to_string());
    row("ticks", &ours.song_length().to_string(), &theirs.song_length().to_string());
    row("layers", &ours.layers.len().to_string(), &theirs.layers.len().to_string());
    row("blocks", &ours.total_blocks.to_string(), &theirs.total_blocks.to_string());
    row("name", &format!(" {:?}", ours.meta.name), &format!(" {:?}", theirs.meta.name));
    row(
        "custom",
        &ours.custom_instruments.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(","),
        &theirs.custom_instruments.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(","),
    );

    println!("\n乐器直方图（下标=音色）");
    let (ha, hb) = (histogram(&ours), histogram(&theirs));
    for k in ha.keys().chain(hb.keys()).collect::<std::collections::BTreeSet<_>>() {
        println!("  ins {k:>3}: {:>6} / {:<6}", ha.get(k).copied().unwrap_or(0), hb.get(k).copied().unwrap_or(0));
    }

    println!("\n音高直方图（key 0..87，只列不一致的）");
    let (ka, kb) = (key_hist(&ours), key_hist(&theirs));
    for k in ka.keys().chain(kb.keys()).collect::<std::collections::BTreeSet<_>>() {
        let (x, y) = (ka.get(k).copied().unwrap_or(0), kb.get(k).copied().unwrap_or(0));
        if x != y {
            println!("  key {k:>3}: {x:>6} / {y:<6}  差 {}", x as i64 - y as i64);
        }
    }

    let mut same = 0usize;
    let mut only_ours = 0usize;
    let mut only_theirs = 0usize;
    let mut diff_content = 0usize;
    for (t, l, n) in ours.iter_notes() {
        match theirs.get(t, l) {
            Some(o) => {
                if o.instrument == n.instrument && o.key == n.key && o.velocity == n.velocity {
                    same += 1
                } else {
                    diff_content += 1
                }
            }
            None => only_ours += 1,
        }
    }
    for (t, l, _) in theirs.iter_notes() {
        if ours.get(t, l).is_none() {
            only_theirs += 1;
        }
    }
    println!(
        "\n逐格：完全相同 {same}，同格不同内容 {diff_content}，只有我们有 {only_ours}，只有原版有 {only_theirs}"
    );

    // 内存里的速度是原版的 real_tempo，而 .nbs 头存的是 `real_tempo * 100` 截断后的
    // 整数（原版 `buffer_write_short`）。真正要比的是存盘再读回来的结果。
    let bytes = nbs_core::nbs::save_bytes(&ours, 6).expect("存盘失败");
    let ours_saved = nbs_core::nbs::load_bytes(&bytes).expect("回读失败");
    println!(
        "\n存盘回读：速度 {:.2} / {:.2}，ticks {} / {}，层数 {} / {}",
        ours_saved.meta.tempo,
        theirs.meta.tempo,
        ours_saved.song_length(),
        theirs.song_length(),
        ours_saved.layers.len(),
        theirs.layers.len()
    );
    let mut saved_same = 0usize;
    let mut saved_only_ours = 0usize;
    let mut saved_only_theirs = 0usize;
    let mut saved_diff = 0usize;
    for (t, l, n) in ours_saved.iter_notes() {
        match theirs.get(t, l) {
            Some(o) => {
                if o.instrument == n.instrument && o.key == n.key && o.velocity == n.velocity {
                    saved_same += 1
                } else {
                    saved_diff += 1
                }
            }
            None => saved_only_ours += 1,
        }
    }
    for (t, l, _) in theirs.iter_notes() {
        if ours_saved.get(t, l).is_none() {
            saved_only_theirs += 1;
        }
    }
    println!(
        "存盘后逐格：完全相同 {saved_same}，同格不同内容 {saved_diff}，只有我们有 {saved_only_ours}，只有原版有 {saved_only_theirs}"
    );

    println!("\n层：名字 / 方块数（我们的 | 原版的）");
    fn cnt(s: &Song, i: usize) -> usize {
        s.iter_notes().filter(|(_, l, _)| *l == i as u32).count()
    }
    fn name(s: &Song, i: usize) -> String {
        s.layers.get(i).map(|l| l.name.clone()).unwrap_or_else(|| "<无>".into())
    }
    let n = ours.layers.len().max(theirs.layers.len());
    for i in 0..n {
        println!(
            "  {:>2} {:<20}{:>6} | {:<20}{:>6}",
            i + 1,
            name(&ours, i),
            cnt(&ours, i),
            name(&theirs, i),
            cnt(&theirs, i)
        );
    }
}
