//! PNG 谱面导出：把网格画布离线画成图片，便于分享 / 打印。
//! 原版没有此功能（其「图片谱」实为 `.nbp` 片段），这里作为 E 模块的增强补上。

use std::path::Path;

use image::{Rgb, RgbImage};
use nbs_core::instruments::instrument_color;
use nbs_core::model::Song;

#[derive(Clone, Debug)]
pub struct ImageOptions {
    /// 每格像素。
    pub cell: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub dark: bool,
    /// 是否在音符里画力度深浅。
    pub show_velocity: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            cell: 18,
            max_width: 8192,
            max_height: 8192,
            dark: false,
            show_velocity: true,
        }
    }
}

const TIMELINE_H: u32 = 22;
const LAYER_BAR_W: u32 = 8;

const GLYPHS: [([u8; 5], char); 10] = [
    ([0x3E, 0x51, 0x49, 0x45, 0x3E], '0'),
    ([0x00, 0x42, 0x7F, 0x40, 0x00], '1'),
    ([0x42, 0x61, 0x51, 0x49, 0x46], '2'),
    ([0x21, 0x41, 0x45, 0x4B, 0x31], '3'),
    ([0x18, 0x14, 0x12, 0x7F, 0x10], '4'),
    ([0x27, 0x45, 0x45, 0x45, 0x39], '5'),
    ([0x3C, 0x4A, 0x49, 0x49, 0x30], '6'),
    ([0x01, 0x71, 0x09, 0x05, 0x03], '7'),
    ([0x36, 0x49, 0x49, 0x49, 0x36], '8'),
    ([0x06, 0x49, 0x49, 0x29, 0x1E], '9'),
];

fn glyph(c: char) -> Option<[u8; 5]> {
    GLYPHS.iter().find(|(_, ch)| *ch == c).map(|(g, _)| *g)
}

fn draw_text(img: &mut RgbImage, text: &str, x: u32, y: u32, color: Rgb<u8>) {
    let mut cx = x;
    for ch in text.chars() {
        if let Some(g) = glyph(ch) {
            for col in 0..5u32 {
                let bits = g[col as usize];
                for row in 0..7u32 {
                    if bits & (1 << row) != 0 && cx + col < img.width() && y + row < img.height() {
                        img.put_pixel(cx + col, y + row, color);
                    }
                }
            }
        }
        cx += 6;
    }
}

pub fn render_image(song: &Song, opts: &ImageOptions) -> RgbImage {
    let ticks = song.song_length().max(1);
    let layers = song.layers.len().max(1) as u32;
    let mut cell = opts.cell.clamp(2, 64);
    // 自动缩小格子，保证不超过像素上限
    while (ticks as u64 * cell as u64 + LAYER_BAR_W as u64) > opts.max_width as u64 && cell > 2 {
        cell -= 1;
    }
    while (layers as u64 * cell as u64 + TIMELINE_H as u64) > opts.max_height as u64 && cell > 2 {
        cell -= 1;
    }
    let w = (ticks as u64 * cell as u64 + LAYER_BAR_W as u64).min(opts.max_width as u64) as u32;
    let h = (layers as u64 * cell as u64 + TIMELINE_H as u64).min(opts.max_height as u64) as u32;

    let bg = if opts.dark { Rgb([26, 28, 34]) } else { Rgb([250, 250, 252]) };
    let line = if opts.dark { Rgb([48, 52, 62]) } else { Rgb([220, 222, 230]) };
    let bar = if opts.dark { Rgb([96, 120, 160]) } else { Rgb([0, 120, 212]) };
    let text_col = if opts.dark { Rgb([200, 208, 220]) } else { Rgb([90, 96, 110]) };
    let mut img = RgbImage::from_pixel(w.max(1), h.max(1), bg);

    let per_bar = song.meta.ticks_per_bar().max(1);
    let visible_ticks = ((w - LAYER_BAR_W) / cell).min(ticks);
    for t in 0..visible_ticks {
        let x = LAYER_BAR_W + t * cell;
        let color = if t % per_bar == 0 { bar } else if t % 4 == 0 { text_col } else { line };
        let thickness = if t % per_bar == 0 { 2 } else { 1 };
        for dy in 0..thickness {
            for y in TIMELINE_H..h {
                if x + dy < w {
                    img.put_pixel(x + dy, y, color);
                }
            }
        }
        if t % per_bar == 0 && cell >= 12 && x + 4 < w {
            draw_text(&mut img, &t.to_string(), x + 2, 3, text_col);
        }
    }

    for layer in 0..layers {
        let y0 = TIMELINE_H + layer * cell;
        if y0 >= h {
            break;
        }
        let (r, g, b) = instrument_color((layer % 16) as u8);
        for yy in y0..(y0 + cell).min(h) {
            for xx in 0..LAYER_BAR_W.min(w) {
                img.put_pixel(xx, yy, Rgb([r, g, b]));
            }
        }
        for yy in [y0, (y0 + cell - 1).min(h - 1)] {
            for xx in LAYER_BAR_W..w {
                img.put_pixel(xx, yy, line);
            }
        }
    }

    // 只遍历全曲一次（离线导出，不受 UI 帧率限制）
    for (tick, layer, note) in song.iter_notes() {
        if tick >= visible_ticks || layer as u32 >= layers {
            continue;
        }
        let x0 = LAYER_BAR_W + tick * cell;
        let y0 = TIMELINE_H + layer as u32 * cell;
        let (r, g, b) = instrument_color(note.instrument);
        let mul = if opts.show_velocity {
            (0.55 + 0.45 * note.velocity as f32 / 100.0).clamp(0.4, 1.0)
        } else {
            1.0
        };
        let fill = Rgb([(r as f32 * mul) as u8, (g as f32 * mul) as u8, (b as f32 * mul) as u8]);
        let edge = if opts.dark { Rgb([16, 18, 22]) } else { Rgb([255, 255, 255]) };
        let pad = if cell >= 8 { 1 } else { 0 };
        for yy in (y0 + pad)..(y0 + cell - pad).min(h) {
            for xx in (x0 + pad)..(x0 + cell - pad).min(w) {
                let border = xx == x0 + pad || yy == y0 + pad || xx + 1 >= (x0 + cell - pad).min(w);
                img.put_pixel(xx, yy, if border && cell >= 8 { edge } else { fill });
            }
        }
        // 微调音分：正蓝负红叠加
        if note.pitch != 0 && cell >= 8 {
            let tint = if note.pitch > 0 { Rgb([80, 120, 230]) } else { Rgb([220, 80, 80]) };
            for yy in (y0 + pad + 1)..(y0 + cell - pad - 1).min(h) {
                let xx = x0 + cell / 2;
                if xx < w {
                    img.put_pixel(xx, yy, tint);
                }
            }
        }
    }
    img
}

pub fn export_png(song: &Song, path: &Path, opts: &ImageOptions) -> Result<(u32, u32), String> {
    let img = render_image(song, opts);
    let dims = (img.width(), img.height());
    img.save(path).map_err(|e| {
        crate::tr(&format!("Failed to write PNG: {e}"), &format!("写入 PNG 失败: {e}")).to_string()
    })?;
    Ok(dims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Song;

    #[test]
    fn png_has_expected_size_and_content() {
        let song = Song::demo();
        let img = render_image(&song, &ImageOptions { cell: 10, ..Default::default() });
        assert!(img.width() > 100);
        assert_eq!(img.height(), TIMELINE_H + song.layers.len() as u32 * 10);
        // 必须有非背景像素（音符方块）
        let bg = Rgb([250, 250, 252]);
        assert!(img.pixels().any(|p| *p != bg));
    }

    #[test]
    fn png_saves_to_disk() {
        let song = Song::demo();
        let p = std::env::temp_dir().join("nbs_rust_chart.png");
        let dims = export_png(&song, &p, &ImageOptions::default()).unwrap();
        assert!(dims.0 > 0 && dims.1 > 0);
        assert!(p.metadata().unwrap().len() > 100);
        let _ = std::fs::remove_file(p);
    }
}
