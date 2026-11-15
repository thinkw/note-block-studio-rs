//! 把一个目录里的 PNG 帧横向拼成一张对照图，白底、可放大。
//! 用法: cargo run -p nbs-editor --example sheet -- <帧目录> <输出.png> [放大倍数]

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("用法: sheet <帧目录> <输出.png> [放大倍数]");
        return;
    };
    let scale: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);

    let mut paths: Vec<_> = std::fs::read_dir(Path::new(dir))
        .expect("目录不存在")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        eprintln!("没有 png");
        return;
    }

    let imgs: Vec<_> = paths
        .iter()
        .map(|p| image::open(p).expect("打开失败").to_rgba8())
        .collect();
    let w = imgs.iter().map(|i| i.width()).max().unwrap() * scale;
    let h = imgs.iter().map(|i| i.height()).max().unwrap() * scale;

    let mut sheet = image::RgbaImage::from_pixel(w * paths.len() as u32, h + 14, [255, 255, 255, 255].into());
    for (i, img) in imgs.iter().enumerate() {
        for y in 0..img.height() {
            for x in 0..img.width() {
                let p = img.get_pixel(x, y);
                for dy in 0..scale {
                    for dx in 0..scale {
                        sheet.put_pixel(i as u32 * w + x * scale + dx, y * scale + dy, *p);
                    }
                }
            }
        }
        // 帧与帧之间画一条红色分隔线，第 i 格即 subimg i。
        if i > 0 {
            for y in 0..h {
                sheet.put_pixel(i as u32 * w, y, [220, 0, 0, 255].into());
            }
        }
    }
    sheet.save(out).expect("保存失败");
    println!("{} 帧 -> {out} ({}x{})", paths.len(), sheet.width(), sheet.height());
}
