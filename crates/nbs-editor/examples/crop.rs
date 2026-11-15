//! 裁剪 / 放大一张 PNG，用来把对照图切成能看清的小块。
//!
//! ```text
//! cargo run -p nbs-editor --example crop -- <in.png> <out.png> <x> <y> <w> <h> [整数倍放大]
//! ```

use image::{GenericImageView, RgbaImage};

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 6 {
        return Err("用法: crop <in.png> <out.png> <x> <y> <w> <h> [scale]".into());
    }
    let n = |i: usize| -> Result<u32, String> {
        a[i].parse().map_err(|_| format!("第 {} 个参数不是数字", i + 1))
    };
    let (x, y, w, h) = (n(2)?, n(3)?, n(4)?, n(5)?);
    let scale = if a.len() > 6 { n(6)? } else { 1 }.max(1);

    let src = image::open(&a[0]).map_err(|e| format!("读取 {} 失败: {e}", a[0]))?;
    let (sw, sh) = src.dimensions();
    if x + w > sw || y + h > sh {
        return Err(format!("裁剪框越界：图是 {sw}×{sh}，要 ({x},{y})+{w}×{h}"));
    }
    let mut out = RgbaImage::new(w * scale, h * scale);
    for j in 0..h {
        for i in 0..w {
            let p = src.get_pixel(x + i, y + j);
            for sy in 0..scale {
                for sx in 0..scale {
                    out.put_pixel(i * scale + sx, j * scale + sy, p);
                }
            }
        }
    }
    out.save(&a[1]).map_err(|e| format!("写入失败: {e}"))?;
    println!("({x},{y}) {w}×{h} ×{scale} → {}", a[1]);
    Ok(())
}
