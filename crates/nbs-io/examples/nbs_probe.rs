//! 把 .nbs 的头部与乐器索引分布打出来，用来核对原版把铜块一族算在内置（0..19）还是自定义里。
//!
//! 运行：`cargo run -p nbs-io --example nbs_probe -- <文件…>`
//! 压缩过的文件（头 8 字节是 gzip）会先解压再看。

use std::collections::BTreeMap;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: cargo run -p nbs-io --example nbs_probe -- <文件.nbs…>");
        std::process::exit(2);
    }
    for path in args {
        let raw = std::fs::read(&path).unwrap_or_default();
        let bytes = decompress(&raw);
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
                println!(
                    "  层 {}  长度 {}  音符 {}  自定义乐器 {} 个\n    曲名 {:?} 作者 {:?} 速度 {:.2}\n    索引分布 {hist:?}",
                    song.layers.len(),
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

/// 压缩过的 nbs 头两字节是 00 00，接着是 gzip 长度 u32 + zlib 数据；未压缩的原样返回。
fn decompress(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() < 8 || &bytes[0..4] != b"\x00\x00\x1f\x8b" {
        return bytes.to_vec();
    }
    let mut d = flate2::read::ZlibDecoder::new(&bytes[8..]);
    let mut out = Vec::new();
    d.read_to_end(&mut out).unwrap_or_default();
    out
}
