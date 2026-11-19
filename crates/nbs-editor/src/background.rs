//! 原版壁纸子系统：`wallpaper_init.gml`（读图 + 定 `wpaperanchor`）、
//! `wallpaper_prepare.gml`（Draw 期把图缩到 `min(720/h, 4096/w)` 再糊一遍）、
//! `control_draw.gml:48-67`（画在 `draw_clear` 之后、所有面板之前，居中铺满）、
//! 暗化层则分两处：编辑器 `:1176-1191`（整窗除网格区，亮色纯白）、
//! 播放器 `:73-83`（整窗，亮色 #E7E7E7），alpha 都是 0.875。
//!
//! 端口把图片和暗化层都压在整帧最底下、铺满整窗，各面板随后盖上来 ——
//! 与原版「暗化层铺满、网格区自己填回底色」等价。
//!
//! 三处有意偏离，都记在这里：
//! - 原版那道门是 `theme = 3 && acrylic && wpaperexist && can_draw_mica`。端口没有系统
//!   Mica/acrylic 背景（图片完全由应用自己铺），`can_draw_mica` 在桌面端恒真，
//!   所以这里只剩「Fluent 主题 + 已导入图片」两个条件。
//! - 原版取消对话框会走 `wallpaper_init("")`：跑 `wallpaper.bat` 抓桌面壁纸、
//!   anchor=0 平铺。端口没有抓桌面壁纸的子系统，取消时保持现状。
//! - 原版的 `wpapernoblur` / `wpapernodim` 两个开关藏在首选项「高级设置」区
//!   （整个区端口还没移植），这里按出厂默认值固定为「虚化 + 暗化」。
//!
//! 图片不进 .nbs、不落配置，原版也只在运行时持有，端口一致。

use egui::{Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use std::path::{Path, PathBuf};

/// 原版 `wallpaper_blur_radius`（`control_create.gml:148`）。
const BLUR_RADIUS: f32 = 300.0;
/// 原版 `wallpaper_prepare.gml:6`：高按 720、宽按 4096 取小的那个作为预缩放。
const MAX_HEIGHT: f32 = 720.0;
const MAX_WIDTH: f32 = 4096.0;
/// 原版 `blur_surface.gml:14` 的 `min(1, 6 / radius)`：先缩到 2% 再糊，最后放大。
const KERNEL_SPAN: f32 = 6.0;
/// 原版 `draw_set_alpha(0.875)` 换算成 8 位 alpha。
const DIM_ALPHA: u8 = (0.875 * 255.0 + 0.5) as u8;
/// 原版暗化层的底色分两套：编辑器 `control_draw.gml:1181` 用纯白，
/// 播放器 `:78` 用 `15198183`（GameMaker 按 BGR 存，即 #E7E7E7）；
/// 深色两档都是 `1315860` = #140D14（`:1183` / `:80`）。
pub const DIM_EDITOR_LIGHT: Color32 = Color32::WHITE;
pub const DIM_PLAYER_LIGHT: Color32 = Color32::from_rgb(231, 231, 231);
pub const DIM_DARK: Color32 = Color32::from_rgb(20, 13, 20);

/// 已导入的背景图：贴图 + 原版 `sprite_get_width/height(wpaper)` 那份尺寸。
#[derive(Clone)]
pub struct Wallpaper {
    pub path: PathBuf,
    pub texture: TextureHandle,
    pub size: Vec2,
}

/// 原版 `wallpaper_prepare.gml:6` 的预缩放系数（小图会被放大，照抄不夹到 1）。
pub fn bake_scale(width: u32, height: u32) -> f32 {
    (MAX_HEIGHT / height as f32).min(MAX_WIDTH / width as f32)
}

/// 原版 `blur_surface.gml:14` 的降采样系数。
pub fn reduction(radius: f32) -> f32 {
    if radius <= 0.0 {
        return 1.0;
    }
    (KERNEL_SPAN / radius).min(1.0)
}

/// 原版 `control_step.gml:303` 的 `wpaperside` 加上 `control_draw.gml:61` 的缩放：
/// 图片相对窗口更宽就按高铺满，否则按宽铺满 —— 短边填满、长边溢出，即 cover。
pub fn cover_scale(window: Vec2, image: Vec2) -> f32 {
    let side = (window.x / window.y) < (image.x / image.y);
    if side {
        window.y / image.y
    } else {
        window.x / image.x
    }
}

/// 原版 `control_draw.gml:62-65`：`(rw - 图宽 * scale) / 2` 居中，不裁剪。
pub fn cover_rect(window: Rect, image: Vec2) -> Rect {
    let size = image * cover_scale(window.size(), image);
    Rect::from_min_size(
        Pos2::new(
            window.min.x + (window.width() - size.x) / 2.0,
            window.min.y + (window.height() - size.y) / 2.0,
        ),
        size,
    )
}

/// 原版 `wallpaper_init` + `wallpaper_prepare`：读图、预缩放、降采样后糊化。
/// 糊化结果只有原来的 2%，绘制时靠线性过滤放大回窗口大小，与原版的
/// 「小表面高斯 + 拉伸拷贝」同构。
pub fn bake(ctx: &Context, path: &Path) -> Result<Wallpaper, String> {
    let decoded = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rgba = decoded.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err(format!("{}: 空图片", path.display()));
    }
    let scale = bake_scale(w, h);
    let wide = image::imageops::resize(
        &rgba,
        (w as f64 * scale as f64).max(1.0).round() as u32,
        (h as f64 * scale as f64).max(1.0).round() as u32,
        image::imageops::FilterType::Triangle,
    );
    let red = reduction(BLUR_RADIUS);
    let tiny = image::imageops::resize(
        &wide,
        (wide.width() as f64 * red as f64).max(1.0).ceil() as u32,
        (wide.height() as f64 * red as f64).max(1.0).ceil() as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let blurred = image::imageops::blur(&tiny, 1.0);
    let (tw, th) = blurred.dimensions();
    let mut img = ColorImage::new([tw as usize, th as usize], Color32::TRANSPARENT);
    for (i, px) in blurred.pixels().enumerate() {
        img.pixels[i] = Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]);
    }
    let texture = ctx.load_texture("nbs_wallpaper", img, TextureOptions::LINEAR);
    Ok(Wallpaper {
        path: path.to_path_buf(),
        texture,
        size: Vec2::new(tw as f32, th as f32),
    })
}

/// 铺在所有面板底下的那两层：糊过的图（cover 居中）+ Mica 暗化层。
/// 画在背景层的最前面，菜单条 / 工具栏 / 状态栏 / 琴键 / 图层栏各自会盖上来，
/// 所以只有原版那几条「编辑区四周空白」会露出图片，
/// 与原版 `control_draw.gml:1185-1191` 的可见范围一致。
pub fn paint(state: &crate::state::EditorState, ctx: &Context) {
    if state.theme.kind != crate::theme::ThemeKind::Fluent {
        return;
    }
    let Some(wp) = state.wallpaper.as_ref() else {
        return;
    };
    let window = ctx.screen_rect();
    if window.width() <= 0.0 || window.height() <= 0.0 {
        return;
    }
    let p = ctx.layer_painter(egui::LayerId::background());
    p.image(
        wp.texture.id(),
        cover_rect(window, wp.size),
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    let mica = if state.theme.fdark {
        DIM_DARK
    } else if state.player {
        DIM_PLAYER_LIGHT
    } else {
        DIM_EDITOR_LIGHT
    };
    p.rect_filled(
        window,
        0.0,
        Color32::from_rgba_unmultiplied(mica.r(), mica.g(), mica.b(), DIM_ALPHA),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))
    }

    #[test]
    fn bake_scale_takes_the_smaller_of_the_two_limits() {
        // 原版 `min(720 / h, 4096 / w)`：1920×1080 卡在高度上。
        assert_eq!(bake_scale(1920, 1080), 720.0 / 1080.0);
        // 超宽全景图卡在宽度上。
        assert_eq!(bake_scale(8192, 1080), 4096.0 / 8192.0);
        // 小图原版是放大的，这里不夹到 1。
        assert_eq!(bake_scale(100, 100), 7.2);
    }

    #[test]
    fn blur_reduction_matches_the_original() {
        assert_eq!(reduction(BLUR_RADIUS), 0.02);
        // 半径小于 6 时原版夹到 1，等于不降采样。
        assert_eq!(reduction(3.0), 1.0);
        assert_eq!(reduction(0.0), 1.0);
    }

    #[test]
    fn cover_fills_the_short_side_and_overflows_the_long_one() {
        // 窗口 1000×500、方图 200×200：原版 `wpaperside` 为假 ⇒ 按宽铺满，高度溢出。
        let r = cover_rect(rect(1000.0, 500.0), Vec2::new(200.0, 200.0));
        assert_eq!(r.width(), 1000.0);
        assert_eq!(r.height(), 1000.0);
        assert_eq!(r.min.x, 0.0);
        assert_eq!(r.min.y, -250.0, "原版 (rh - 图高 * scale) / 2 居中");
        // 窗口比图更「竖」时换成按高铺满，横向溢出。
        let r = cover_rect(rect(400.0, 800.0), Vec2::new(200.0, 100.0));
        assert_eq!(r.height(), 800.0);
        assert_eq!(r.width(), 1600.0);
        assert_eq!(r.min.x, -600.0);
        assert_eq!(r.min.y, 0.0);
    }

    #[test]
    fn dim_alpha_is_the_original_0875() {
        assert_eq!(DIM_ALPHA, 223);
        // 编辑器 `:1181` 是 c_white，播放器 `:78` 才是 GM 的 15198183（按 BGR 解）。
        assert_eq!(DIM_EDITOR_LIGHT, Color32::WHITE);
        assert_eq!(DIM_PLAYER_LIGHT, Color32::from_rgb(0xE7, 0xE7, 0xE7));
        // 深色两档同一个值：1315860。
        assert_eq!(DIM_DARK, Color32::from_rgb(0x14, 0x0D, 0x14));
    }

    /// 独立临时目录，避免并行用例互踩。
    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("nbs_background_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 真读一张图走一遍烘焙：贴图存在、尺寸是原图的 2% 量级。
    #[test]
    fn bake_loads_blurs_and_uploads() {
        let ctx = Context::default();
        let dir = scratch("bake");
        let file = dir.join("tile.png");
        let img = image::RgbaImage::from_fn(800, 600, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 199) as u8, 128, 255])
        });
        img.save(&file).unwrap();

        let wp = bake(&ctx, &file).expect("烘焙背景图");
        assert_eq!(wp.path, file);
        // 预缩放 720/600 = 1.2 ⇒ 960×720，再按 2% 降采样 ⇒ 20×15。
        assert_eq!(wp.size, Vec2::new(20.0, 15.0));

        // 读不到的文件要报错，而不是悄悄什么都不画。
        assert!(bake(&ctx, &dir.join("missing.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
