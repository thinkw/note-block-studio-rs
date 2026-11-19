//! 界面字体 —— 用 egui 自带字体，只补一层中文字形。
//!
//! 原版 `font_table`（`control_create.gml:115-135`）每类字体三列：EN / wslui（仅 Fluent
//! 主题）/ CJK。端口一度把 EN 列的 Open Sans / Liberation Sans 和 CJK 列的思源黑体原样
//! 搬进来，按 GM 的磅→px（1pt = 4/3 px）换算字号，想逐字对上原版（#74/#79）。
//! **#98 用户定案：放弃这条对齐路线，界面改用默认字体**，打包字体不再进入字体表。
//!
//! 唯一必须补的是中文：epaint 内置字体表（`text/fonts.rs` 的 `FontDefinitions::default()`
//! = Ubuntu-Light / Hack / 两套 emoji）一个 CJK 字形都没有，中文界面会整屏画成空框。
//! 所以优先用系统里现成的黑体/等线/思源（`NBS_FONT` 可以直接指定字体文件），本机一个都
//! 找不到时才退回仓库自带的 `assets/fonts/SourceHanSansSC-Normal.otf`。
//!
//! epaint 没有字重概念（`TODO(emilk): weight (bold)`），[`bold_family`]/[`small_family`]
//! 两个自定义族仍要绑进字体表 —— 绑空了画到就 panic（当年 release 闪退，commit 5aa9279）。
//! 字号沿用现状不随字体回退：网格/窗口的对齐实测都是按这几档量出来的。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::{Context, FontData, FontDefinitions, FontFamily, FontId};

/// `fnt_main` 9pt（GM 里 1pt = 4/3 px）：网格标签、控制栏数字、状态栏、菜单栏。
pub const MAIN_SIZE: f32 = 12.0;
/// `fnt_small` 8pt（×4/3 = 10.67px，圆整成 11）：分数 em 现栅格化会把笔画压在分数像素上
/// 发虚，字宽差 ~0.3px/字，肉眼不可见。标尺数字、层名、页签、歌曲时长。
pub const SMALL_SIZE: f32 = 11.0;
/// `fnt_info_med` 11pt（×4/3 = 14.67px，圆整理由同上）。
pub const INFO_MED_SIZE: f32 = 15.0;
/// `fnt_info_big` 18pt。
pub const INFO_BIG_SIZE: f32 = 24.0;

const CJK_KEY: &str = "nbs-hansans-normal";
const SYS_CJK_KEY: &str = "nbs-cjk";

const CJK_FILE: &str = "SourceHanSansSC-Normal.otf";

/// `font_main_bold` 一族：黑键标签、Compatible 徽标、控制栏时间、播放器标题。
pub fn bold_family() -> FontFamily {
    FontFamily::Name(Arc::from("nbs_bold"))
}

/// `font_small` / `font_info_med` 一族：标尺数字、层名、页签、歌曲时长。
pub fn small_family() -> FontFamily {
    FontFamily::Name(Arc::from("nbs_small"))
}

/// `font_main`：网格标签、控制栏数字、状态栏、菜单栏。
pub fn main_font() -> FontId {
    FontId::new(MAIN_SIZE, FontFamily::Proportional)
}
/// `font_main_bold`。
pub fn bold_font() -> FontId {
    FontId::new(MAIN_SIZE, bold_family())
}
/// `font_small`。
pub fn small_font() -> FontId {
    FontId::new(SMALL_SIZE, small_family())
}
/// `font_info_med`。
pub fn info_med_font() -> FontId {
    FontId::new(INFO_MED_SIZE, small_family())
}
/// `font_info_med_bold`。
pub fn info_med_bold_font() -> FontId {
    FontId::new(INFO_MED_SIZE, bold_family())
}
/// `font_info_big`（EN 是 Liberation Bold 18pt，CJK 是思源 Medium 18pt，两列都偏粗）。
pub fn info_big_font() -> FontId {
    FontId::new(INFO_BIG_SIZE, bold_family())
}

/// 画布缩放下的字号：网格 / 图层面板里的字随 `Viewport::scale` 放大，
/// chrome（工具栏、钢琴栏、窗口）保持原字号不缩放。
pub fn scaled(id: FontId, scale: f32) -> FontId {
    FontId::new(id.size * scale, id.family)
}

/// 打包字体所在目录，只在系统里找不到中文字体时才会用到。
/// 与图标同一套搜索顺序：可执行文件旁 → 工作目录，按相对层级往上找 `assets/fonts`，
/// 不把编译期绝对路径写进二进制。
fn font_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for up in ["", "..", "../..", "../../.."] {
                candidates.push(dir.join(up).join("assets/fonts"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for up in ["", "..", "../.."] {
            candidates.push(cwd.join(up).join("assets/fonts"));
        }
    }
    candidates.into_iter().find(|p| p.join(CJK_FILE).is_file())
}

/// 找一个含中文字形的系统字体文件；`NBS_FONT` 可以直接指定字体文件路径。
/// 这是中文的主力来源（#98 之后界面主体用 egui 自带字体，中文靠这里补）。
fn find_system_cjk() -> Option<PathBuf> {
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
    if let Ok(p) = std::env::var("NBS_FONT") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |v: Option<String>, sub: &str| {
        if let Some(root) = v.map(PathBuf::from) {
            let d = if sub.is_empty() { root } else { root.join(sub) };
            dirs.push(d);
        }
    };
    push(std::env::var("WINDIR").ok(), "Fonts");
    push(std::env::var("SYSTEMROOT").ok(), "Fonts");
    push(
        std::env::var("LOCALAPPDATA").ok(),
        "Microsoft/Windows/Fonts",
    );
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
    dirs.iter().find_map(|dir| {
        FILE_NAMES
            .iter()
            .map(|name| dir.join(name))
            .find(|p| p.is_file())
    })
}

fn load_into(fonts: &mut FontDefinitions, dir: &Path, key: &str, file: &str) -> bool {
    match std::fs::read(dir.join(file)) {
        Ok(bytes) => {
            fonts
                .font_data
                .insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
            true
        }
        Err(_) => false,
    }
}

/// 构建字体表：egui 默认那套打底，中文字形垫在族尾。
///
/// 中文排在默认字体**后面**（不是像之前那样按语言排到族首）：拉丁字形继续由
/// Ubuntu-Light 出，只有它缺的字才落到思源/系统黑体，界面不会整屏换成黑体。
/// 返回的 `bool` = 中文有没有字形来源；`false` 时中文是空框，调用方要提示用户。
/// 无论有没有中文字体，两个自定义族都会绑好，画到就 panic 的老问题不会回来。
fn build(bundled: Option<&Path>, sys: Option<&Path>) -> (FontDefinitions, bool) {
    let mut fonts = FontDefinitions::default();
    let mut cjk: Vec<String> = Vec::new();

    if let Some(path) = sys {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                SYS_CJK_KEY.to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            cjk.push(SYS_CJK_KEY.to_owned());
        }
    }
    // 本机一个中文字体都没有（干净虚拟机）才用仓库自带的那款 otf。
    // 只补常规字重：这一层的目标是「不缺字」，不再管字重。
    if cjk.is_empty() {
        if let Some(dir) = bundled {
            if load_into(&mut fonts, dir, CJK_KEY, CJK_FILE) {
                cjk.push(CJK_KEY.to_owned());
            }
        }
    }

    let has_cjk = !cjk.is_empty();
    // 端口自己的文本都走 Proportional，但 egui 内置控件（tooltip、debug 覆盖层）
    // 会用到 Monospace —— 两边都垫上，缺字不至于变空框。
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .extend(cjk.iter().cloned());
    }
    let prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts.families.insert(bold_family(), prop.clone());
    fonts.families.insert(small_family(), prop);

    (fonts, has_cjk)
}

/// 构建并缓存字体表：`FontData` 内部是 `Arc`，克隆只复制那几棵小映射。
/// 首次调用读盘（系统中文字体可能十几 MB），之后（每帧 `warm_up`、测试反复新建
/// `Context`）零 IO。第二个返回值 = 中文有没有字形来源。
fn cached() -> (FontDefinitions, bool) {
    static CACHE: OnceLock<(FontDefinitions, bool)> = OnceLock::new();
    let pair = CACHE.get_or_init(|| build(font_root().as_deref(), find_system_cjk().as_deref()));
    (pair.0.clone(), pair.1)
}

/// 把字体表装进 `ctx`，返回界面里的中文是否真有字形来源：`false` 表示本机既没有
/// 现成的中文字体、也找不到打包的思源 otf，中文会画成空框（调用方据此提示用户）。
/// `set_fonts` 要到下一次 pass 开头才生效，所以必须在**首帧渲染前**调用
/// （eframe 的 creator / 测试的第一次 `ctx.run` 之前），否则首帧画到
/// [`bold_family`] 就会 panic。
pub fn install(ctx: &Context) -> bool {
    let (fonts, has_cjk) = cached();
    ctx.set_fonts(fonts);
    has_cjk
}

/// 同 [`install`]，但只装字体不返回结果：给测试 helper 在每次 `ctx.run` 前调用，
/// 幂等（epaint 用 TTF 等值比较跳过重复重建）。
pub fn warm_up(ctx: &Context) {
    let (fonts, _) = cached();
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(ctx: &Context) {
        let _ = ctx.run(egui::RawInput::default(), |_| {});
    }

    /// 默认字体打头，中文只垫尾兜底 —— 这是 #98「改用默认字体」的契约：
    /// 字体表里不许再出现打包的 Open Sans / Liberation Sans。
    #[test]
    fn default_faces_lead_and_cjk_is_only_a_fallback() {
        // 一个中文字体都找不到时的下限：中文缺字，但族表必须完整。
        let (none, has_cjk) = build(None, None);
        assert_eq!(
            none.families[&FontFamily::Proportional][0],
            "Ubuntu-Light",
            "主字体是 egui 自带的 Ubuntu-Light，不是打包字体"
        );
        assert!(
            none.families[&FontFamily::Proportional]
                .iter()
                .all(|f| !f.starts_with("nbs-")),
            "打包字体不该进字体表"
        );
        assert!(!has_cjk, "没有来源时不该谎报有中文");
        // 部署环境丢字体曾经直接闪退：族必须绑好，哪怕绑的是默认字体。
        for fam in [bold_family(), small_family()] {
            assert!(
                !none.families[&fam].is_empty(),
                "{fam:?} 没绑字体，画到就 panic"
            );
        }

        // 系统里找到中文字体：垫在 Ubuntu-Light 之后，拉丁字形不受影响。
        let sys = match find_system_cjk() {
            Some(p) => p,
            None => {
                eprintln!("跳过：本机找不到系统中文字体");
                return;
            }
        };
        let (fonts, has_cjk) = build(None, Some(&sys));
        assert!(has_cjk, "本机有中文字体却说没有");
        let prop = &fonts.families[&FontFamily::Proportional];
        assert_eq!(prop[0], "Ubuntu-Light", "中文不许抢在主字体前面");
        assert_eq!(prop.last().map(String::as_str), Some(SYS_CJK_KEY));
        assert!(
            fonts.families[&FontFamily::Monospace]
                .iter()
                .any(|f| f == SYS_CJK_KEY),
            "egui 内置控件走 Monospace，中文也得兜住"
        );
        assert_eq!(
            fonts.families[&bold_family()].last().map(String::as_str),
            Some(SYS_CJK_KEY),
            "粗体族也得有中文兜底"
        );
    }

    /// 系统里没有中文字体时才轮到打包的思源 otf（干净虚拟机的场景）。
    #[test]
    fn the_bundled_face_is_the_last_resort_for_cjk() {
        let Some(dir) = font_root() else {
            eprintln!("跳过：本机找不到 assets/fonts（测试需在仓库里跑）");
            return;
        };
        let (fonts, has_cjk) = build(Some(&dir), None);
        assert!(has_cjk, "仓库自带的思源应能兜住中文");
        assert_eq!(
            fonts.families[&FontFamily::Proportional].last(),
            Some(&CJK_KEY.to_owned())
        );
        assert!(
            fonts.font_data.contains_key(CJK_KEY),
            "兜底字体必须真读进内存"
        );
    }

    /// 装入后 egui 必须真能画出三档字号的拉丁与中文字形。
    /// `set_fonts` 下一帧生效，所以中间隔一次 `ctx.run`。
    #[test]
    fn installing_makes_all_three_families_render() {
        let id_main = FontId::new(MAIN_SIZE, FontFamily::Proportional);
        let id_bold = FontId::new(MAIN_SIZE, bold_family());
        let id_small = FontId::new(SMALL_SIZE, small_family());
        let ctx = Context::default();
        pass(&ctx);
        assert!(
            !ctx.fonts(|f| f.has_glyphs(&id_main, "A音符")),
            "默认字体不该全有这些字形，否则这个用例证明不了什么"
        );

        let has_cjk = install(&ctx);
        pass(&ctx);
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_main, "BPM = 123")),
            "默认字体的拉丁字形缺失"
        );
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_bold, "13:37")),
            "粗体族没有字形"
        );
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_small, "12:34/ 56:78")),
            "small 族没有字形"
        );
        if has_cjk {
            assert!(
                ctx.fonts(|f| f.has_glyphs(&id_main, "音符速度")),
                "字体表说有中文来源，画出来却是空框"
            );
        } else {
            eprintln!("提示：本机既无系统中文字体也无 assets/fonts，跳过中文断言");
        }
    }
}
