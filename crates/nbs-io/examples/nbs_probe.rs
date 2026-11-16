//! 把 .nbs 的头部与乐器索引分布打出来，用来核对原版把铜块一族算在内置（0..19）还是自定义里。
//!
//! 运行：`cargo run -p nbs-io --example nbs_probe -- <文件…>`
//! 注意：原版 v3.12 的保存/加载都不压缩 .nbs（buffer_save/buffer_load 裸读写），
//! 生态库 rsnbs/nbs-rs 也不解压，所以这里只处理裸格式。

use std::collections::BTreeMap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: cargo run -p nbs-io --example nbs_probe -- <文件.nbs…>");
        std::process::exit(2);
    }
    for path in args {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let (version, first_custom) = if bytes.len() > 4 && &bytes[0..2] == b"\x00\x00" {
            (bytes[2], bytes[3])
        } else {
            (0, 0)
        };
        print!("{}  版本 {version}  首个自定义索引 {first_custom}", path.rsplit(['/', '\\']).next().unwrap_or(&path));
        match nbs_core::nbs::load_bytes(&bytes) {
            Ok(song) => {
                let mut hist: BTreeMap<u8, usize> = BTreeMap::new();
                for (_, _, note) in song.iter_notes() {
                    *hist.entry(note.instrument).or_default() += 1;
                }
                let used = song
                    .iter_notes()
                    .map(|(_, l, _)| l)
                    .max()
                    .map_or(0, |m| m as usize + 1);
                println!(
                    "  层 {}（有音符 {}，尾空 {}）  长度 {}  音符 {}  自定义乐器 {} 个\n    曲名 {:?} 作者 {:?} 速度 {:.2}\n    索引分布 {hist:?}",
                    song.layers.len(),
                    used,
                    song.layers.len().saturating_sub(used),
                    song.song_length(),
                    song.total_blocks,
                    song.custom_instruments.len(),
                    song.meta.name,
                    song.meta.author,
                    song.meta.tempo
                );
            }
            Err(e) => println!("  —— 本移植读不了：{e}"),
        }
    }
}

