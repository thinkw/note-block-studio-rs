//! 界面字体。
//!
//! egui 自带的四款字体（Hack / Ubuntu-Light / NotoEmoji / emoji-icon-font）一个中日韩
//! 字形都没有，所以中文标签会画成空框。原版在 `control_create.gml:76` 按语言换整张字体表
//! （`language == 1` 时改用 `fnt_src_*` 那套位图字体），这里做等价的事：启动时从系统字体
//! 目录找一个含中文字形的字体，追加到 egui 的字体族末尾当兜底。
//!
//! epaint 用 ab_glyph 解析字体，没有 `.ttc` 字体集合（`ttcf`）的取面能力，所以候选表里
//! 只列独立的 `.ttf`/`.otf`；`msyh.ttc`、`simsun.ttc` 打不开，不要往里加。

use std::path::PathBuf;

use egui::{Context, FontData, FontDefinitions, FontFamily};

const FONT_KEY: &str = "nbs-cjk";

/// 各系统字体目录里常见的、独立成文件的中文字体。
const FILE_NAMES: &[&str] = &[
    "simhei.ttf", // Windows 黑体
    "Deng.ttf",   // Windows 等线
    "simkai.ttf",
    "NotoSansSC-VF.ttf",
    "NotoSansSC-Regular.otf",
    "SourceHanSansSC-Regular.otf",
    "WenQuanYi Zen Hei.ttf",
    "Arial Unicode.ttf",
];

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |v: Option<String>, sub: &str| {
        if let Some(root) = v.map(PathBuf::from) {
            let d = if sub.is_empty() { root } else { root.join(sub) };
            dirs.push(d);
        }
    };
    push(std::env::var("WINDIR").ok(), "Fonts");
    push(std::env::var("SYSTEMROOT").ok(), "Fonts");
    push(std::env::var("LOCALAPPDATA").ok(), "Microsoft/Windows/Fonts");
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
    dirs
}

/// 找到一个能显示中文的系统字体文件；`NBS_FONT` 可以直接指定字体文件路径。
pub fn find_cjk_font() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NBS_FONT") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    search_dirs().iter().find_map(|dir| {
        FILE_NAMES
            .iter()
            .map(|name| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// 把中文字体装进 `ctx`，返回实际用的字体文件。找不到就原样返回 `None`，界面退回默认字体。
pub fn install(ctx: &Context) -> Option<PathBuf> {
    let path = find_cjk_font()?;
    let bytes = std::fs::read(&path).ok()?;

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(FONT_KEY.to_owned(), std::sync::Arc::new(FontData::from_owned(bytes)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(FONT_KEY.to_owned());
    }
    ctx.set_fonts(fonts);
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Context::fonts()` 要到下一帧才建好，`set_fonts` 也是下一帧才生效。
    fn pass(ctx: &Context) {
        let _ = ctx.run(egui::RawInput::default(), |_| {});
    }

    /// 装了系统字体之后，egui 才能给出中文字形；没装之前这些字符全是空框。
    /// 找的是本机字体目录，所以没装中文字体的机器上这条只能跳过（原版在中文机器上
    /// 同样只能靠 `fnt_src_*` 那套位图字体，代码这边没有兜底资源）。
    /// 用 `NBS_FONT=<字体文件>` 可以指定要装的字体，跳过时按这个提示补一次覆盖。
    #[test]
    fn installing_the_system_font_makes_cjk_glyphs_available() {
        let id = egui::FontId::proportional(13.0);
        let text = "文件工具播放速度";
        let ctx = Context::default();
        pass(&ctx);
        assert!(
            !ctx.fonts(|f| f.has_glyphs(&id, text)),
            "默认字体不该有中文字形，否则这个用例证明不了什么"
        );

        let Some(path) = install(&ctx) else {
            eprintln!(
                "跳过：本机找不到中文字体（查 %WINDIR%\\Fonts 等目录，或用 NBS_FONT 指定），\
                 没有字体时无法验证字形兜底"
            );
            return;
        };
        assert!(path.is_file());
        pass(&ctx);
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id, text)),
            "装了 {path:?} 之后仍缺字形"
        );
        let mono = egui::FontId::monospace(13.0);
        assert!(ctx.fonts(|f| f.has_glyphs(&mono, text)), "等宽族没兜底");
    }
}
