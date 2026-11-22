//! 用 nbs-fractal 的几何出一架整机投影，顺带把报告原样打出来。
//!
//! 运行：`cargo run -p nbs-io --offline --example fractal_export -- <输入.nbs> <输出.litematic> [linear|compact] [20|10]`
//! 目的：产物交给独立解析脚本数方块，核对「进去多少音符、机体里落了多少」。

use nbs_io::fractal::{self, FractalLayout, FractalOptions};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("用法: fractal_export <输入.nbs> <输出.litematic> [linear|compact] [20|10]");
        std::process::exit(2);
    }
    let in_path = std::path::Path::new(&a[0]);
    // 下面那行总结写的是中文，报告文案也跟着走中文，别一次输出两种语言。
    nbs_io::set_language(1);
    let song = nbs_core::nbs::load(in_path).unwrap_or_else(|e| panic!("读 {} 失败: {e}", a[0]));
    let layout = match a.get(2).map(|s| s.as_str()) {
        Some("compact") => FractalLayout::Compact,
        _ => FractalLayout::Linear,
    };
    let opts = FractalOptions {
        layout,
        twenty_hz: a.get(3).map_or(true, |s| s != "10"),
        ..FractalOptions::default()
    };
    let out_path = std::path::Path::new(&a[1]);
    let rep = fractal::export(&song, out_path, &opts).unwrap_or_else(|e| panic!("{e}"));
    println!("-> {}", a[1]);
    println!(
        "进 {} 颗，留 {} 颗，移调 {}，改写怪物头 {}，丢弃 {}，道数 {}，每格 {} game tick（{} 音/秒）",
        rep.notes_in, rep.notes_kept, rep.transposed, rep.remapped, rep.dropped_custom, rep.tracks, rep.cell_gt, rep.notes_per_sec()
    );
    for line in rep.lines() {
        println!("  · {line}");
    }
}
