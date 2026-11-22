//! 把一首 .nbs 编成唱片数据箱投影，顺带把报告打到终端。
//!
//! 用法：`cargo run -p nbs-io --offline --example disc_chest_export -- <输入.nbs> <输出.litematic> [20|10] [resample|ticks] [层高]`
//! 目的：与 `.work/ermc-run/upstream` 那份 npm 转换器跑同一首曲子，逐字段对照产物。
//! 语言固定中文，因为上游示例配置 `language: 'zh'`，子区域名要能一一对上。

use nbs_io::disc_chest::{self, DiscChestOptions, Machine, Timing};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("用法: disc_chest_export <输入.nbs> <输出.litematic> [20|10] [resample|ticks] [层高]");
        std::process::exit(2);
    }
    nbs_io::set_language(1);
    let song = nbs_core::nbs::load(std::path::Path::new(&a[0]))
        .unwrap_or_else(|e| panic!("读 {} 失败: {e}", a[0]));
    let opts = DiscChestOptions {
        machine: match a.get(2).map(|s| s.as_str()) {
            Some("10") => Machine::TenHz,
            _ => Machine::TwentyHz,
        },
        timing: match a.get(3).map(|s| s.as_str()) {
            Some("ticks") => Timing::KeepTicks,
            _ => Timing::Resample,
        },
        layer_height: a.get(4).and_then(|s| s.parse().ok()).unwrap_or(8),
        ..Default::default()
    };
    let rep = disc_chest::export(&song, std::path::Path::new(&a[1]), &opts)
        .unwrap_or_else(|e| panic!("{e}"));
    println!("-> {}", a[1]);
    println!(
        "进 {} 颗，编进 {} 颗，移调 {}，量化 {}，无轨道丢弃 {}，合并 {}，子区域 {}，箱子 {}，潜影盒 {}",
        rep.notes_in,
        rep.notes_kept,
        rep.transposed,
        rep.rounded_pitch,
        rep.dropped_unmapped,
        rep.merged,
        rep.regions,
        rep.chests,
        rep.boxes
    );
    for line in rep.lines() {
        println!("  · {line}");
    }
}
