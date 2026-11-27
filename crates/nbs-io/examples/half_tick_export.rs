//! 导出自家 20Hz 单程直轨，并把报告原样打出来。

use nbs_io::half_tick::{self, HalfTickOptions};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("用法: half_tick_export <输入.nbs> <输出.schem>");
        std::process::exit(2);
    }
    nbs_io::set_language(1);
    let song = nbs_core::nbs::load(std::path::Path::new(&a[0])).unwrap_or_else(|e| panic!("读 {} 失败: {e}", a[0]));
    let rep = half_tick::export(&song, std::path::Path::new(&a[1]), &HalfTickOptions::default())
        .unwrap_or_else(|e| panic!("{e}"));
    println!("-> {}", a[1]);
    for line in rep.lines() {
        println!("  · {line}");
    }
}
