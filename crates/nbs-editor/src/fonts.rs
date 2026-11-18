//! 界面字体 —— 按语言装入原版同款字体。
//!
//! 原版 `font_table`（`control_create.gml:115-135`）每类字体三列：EN / wslui（仅 Fluent
//! 主题）/ CJK。端口不用 Fluent 列，只装 EN 与 CJK 两列：
//!   EN  → Open Sans（`fnt_main*` 9pt）+ Liberation Sans（`fnt_small*` 8pt、`fnt_info_*`
//!         11/18pt），与原版 datafiles 同源的 ttf；
//!   CJK → 思源黑体 CN（`fnt_src*`），Normal/Medium 对应常规/粗体，即原版
//!         `datafiles/Data/Fonts` 自带的两个 otf。
//!
//! GM 的字号单位是磅，运行时 1pt = 4/3 px（`fnt_main` 9pt 的 'A' 步宽 8px ≈ 0.658em×12px）。
//! 端口此前把磅数直接当 px 用（9→9px），字比原版小 25%，再叠加 egui 默认的
//! Hack/Ubuntu-Light 细体，就是「字显示比原版糊」的两个根因。
//!
//! epaint 没有字重概念（`epaint fonts.rs` 里 `TODO(emilk): weight (bold)`），粗体走独立族：
//! `font_main_bold`/`font_info_med_bold`/`font_info_big` → [`bold_family`]，
//! `font_small`/`font_info_med` → [`small_family`]。每个族把另一套语言的字体垫尾兜底，
//! egui 自带四款垫底，最后是系统字体查找（部署时丢了 assets 目录的保险）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::{Context, FontData, FontDefinitions, FontFamily, FontId};

/// `fnt_main` / `fnt_mainbold`：Open Sans 9pt。
pub const MAIN_SIZE: f32 = 12.0;
/// `fnt_small` / `fnt_smallbold`：Liberation Sans 8pt。
pub const SMALL_SIZE: f32 = 32.0 / 3.0;
/// `fnt_info_med` / `fnt_info_med_bold`：Liberation Sans 11pt。
pub const INFO_MED_SIZE: f32 = 44.0 / 3.0;
/// `fnt_info_big`：Liberation Sans 18pt。
pub const INFO_BIG_SIZE: f32 = 24.0;

const MAIN_KEY: &str = "nbs-opensans";
const MAIN_BOLD_KEY: &str = "nbs-opensans-bold";
const SMALL_KEY: &str = "nbs-liberation";
const SMALL_BOLD_KEY: &str = "nbs-liberation-bold";
const CJK_KEY: &str = "nbs-hansans-normal";
const CJK_BOLD_KEY: &str = "nbs-hansans-medium";
const SYS_CJK_KEY: &str = "nbs-cjk";

const MAIN_FILE: &str = "OpenSans-Regular.ttf";
const MAIN_BOLD_FILE: &str = "OpenSans-Bold.ttf";
const SMALL_FILE: &str = "LiberationSans-Regular.ttf";
const SMALL_BOLD_FILE: &str = "LiberationSans-Bold.ttf";
const CJK_FILE: &str = "SourceHanSansSC-Normal.otf";
const CJK_BOLD_FILE: &str = "SourceHanSansSC-Medium.otf";

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
    candidates.into_iter().find(|p| p.join(MAIN_FILE).is_file())
}

/// 找一个含中文字形的系统字体文件；`NBS_FONT` 可以直接指定字体文件路径。
/// 只在打包字体缺字时才有意义（比如部署时丢了 assets 目录）。
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

fn load_into(fonts: &mut FontDefinitions, dir: &Path, key: &str, file: &str) -> Option<()> {
    let bytes = std::fs::read(dir.join(file)).ok()?;
    fonts
        .font_data
        .insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
    Some(())
}

/// 按语言构建字体表：主字体插到族首（原版中文界面连数字都用思源的拉丁字形），
/// 另一套语言的字体垫尾兜底，egui 自带四款垫底，最后是系统字体查找。
fn build(language: u8, dir: &Path, sys: Option<&Path>) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    let mut have = |key: &str, file: &str| load_into(&mut fonts, dir, key, file).is_some();
    let main = have(MAIN_KEY, MAIN_FILE).then(|| MAIN_KEY.to_owned());
    let bold = have(MAIN_BOLD_KEY, MAIN_BOLD_FILE).then(|| MAIN_BOLD_KEY.to_owned());
    let small = have(SMALL_KEY, SMALL_FILE).then(|| SMALL_KEY.to_owned());
    let small_bold = have(SMALL_BOLD_KEY, SMALL_BOLD_FILE).then(|| SMALL_BOLD_KEY.to_owned());
    let cjk = have(CJK_KEY, CJK_FILE).then(|| CJK_KEY.to_owned());
    let cjk_bold = have(CJK_BOLD_KEY, CJK_BOLD_FILE).then(|| CJK_BOLD_KEY.to_owned());
    let sys_key = sys.and_then(|p| {
        std::fs::read(p).ok().map(|bytes| {
            fonts.font_data.insert(
                SYS_CJK_KEY.to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            SYS_CJK_KEY.to_owned()
        })
    });

    let cjk = cjk.clone().or_else(|| sys_key.clone());
    let cjk_bold = cjk_bold.clone().or_else(|| sys_key.clone());
    let (head, tail, small_head, small_tail, bold_head, bold_tail, sys_key) = if language == 1 {
        (
            cjk.clone(),
            main,
            cjk.clone(),
            small,
            cjk_bold.clone(),
            bold,
            sys_key.clone(),
        )
    } else {
        (
            main,
            cjk.clone(),
            small,
            cjk.clone(),
            bold,
            cjk_bold.clone(),
            sys_key.clone(),
        )
    };

    let mut prop: Vec<String> = Vec::new();
    prop.extend(head);
    prop.extend(tail);
    prop.extend(sys_key.iter().cloned());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .splice(0..0, prop);

    // 端口自己的文本都走 Proportional，但 egui 内置控件（tooltip、debug 覆盖层）
    // 会用到 Monospace —— 把中文字形同样垫进去，缺字不至于变空框。
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .extend(cjk.clone());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .extend(sys_key.clone());

    let mut bold_fam: Vec<String> = Vec::new();
    bold_fam.extend(bold_head);
    bold_fam.extend(bold_tail);
    // font_small_bold（属性窗的 [!] 警告标记）：Liberation Bold 挂在 bold 族尾，
    // 用 `FontId::new(SMALL_SIZE, bold_family())` 就能取到。
    bold_fam.extend(small_bold);
    bold_fam.extend(sys_key.iter().cloned());
    fonts.families.insert(bold_family(), bold_fam);

    let mut small_fam: Vec<String> = Vec::new();
    small_fam.extend(small_head);
    small_fam.extend(small_tail);
    small_fam.extend(sys_key);
    fonts.families.insert(small_family(), small_fam);

    fonts
}

/// 构建并缓存两套语言的字体表：`FontData` 内部是 `Arc`，克隆只复制两棵小映射。
/// 首次调用读盘，之后（测试反复新建 `Context`）零 IO。
fn cached(language: u8) -> Option<FontDefinitions> {
    static CACHE: OnceLock<Option<[FontDefinitions; 2]>> = OnceLock::new();
    let pair = CACHE.get_or_init(|| {
        let dir = font_root()?;
        let sys = find_system_cjk();
        Some([
            build(0, &dir, sys.as_deref()),
            build(1, &dir, sys.as_deref()),
        ])
    });
    let pair = pair.as_ref()?;
    Some(pair[usize::from(language == 1)].clone())
}

/// 把界面字体装进 `ctx`，返回资产目录。找不到 `assets/fonts` 时原样返回 `None`，
/// 界面退回 egui 默认字体（此时系统字体兜底也一并失效 —— 资产是主路径）。
/// `set_fonts` 要到下一次 pass 开头才生效，所以必须在**首帧渲染前**调用
/// （eframe 的 creator / 测试的第一次 `ctx.run` 之前），否则首帧画到
/// [`bold_family`] 就会 panic。
pub fn install(ctx: &Context, language: u8) -> Option<PathBuf> {
    ctx.set_fonts(cached(language)?);
    font_root()
}

/// 同 [`install`]，但只装字体不找目录：给测试 helper 在每次 `ctx.run` 前调用，
/// 幂等（epaint 用 TTF 等值比较跳过重复重建）。
pub fn warm_up(ctx: &Context, language: u8) {
    if let Some(defs) = cached(language) {
        ctx.set_fonts(defs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(ctx: &Context) {
        let _ = ctx.run(egui::RawInput::default(), |_| {});
    }

    /// 族首必须按语言排：EN 界面的数字是 Open Sans 的字形，中文界面是思源的
    /// （原版中文界面全部走 `fnt_src*`）。粗体、small 档各自独立成族。
    #[test]
    fn family_order_follows_language() {
        let Some(dir) = font_root() else {
            eprintln!("跳过：本机找不到 assets/fonts（测试需在仓库里跑）");
            return;
        };
        let en = build(0, &dir, None);
        assert_eq!(en.families[&FontFamily::Proportional][0], MAIN_KEY);
        assert_eq!(en.families[&small_family()][0], SMALL_KEY);
        assert_eq!(
            en.families[&bold_family()][0],
            MAIN_BOLD_KEY,
            "EN 的粗体是 Open Sans Bold"
        );

        let cjk = build(1, &dir, None);
        assert_eq!(
            cjk.families[&FontFamily::Proportional][0],
            CJK_KEY,
            "中文界面连主字体都是思源"
        );
        assert_eq!(
            cjk.families[&small_family()][0],
            CJK_KEY,
            "fnt_small 的 CJK 列也是思源"
        );
        assert_eq!(
            cjk.families[&bold_family()][0],
            CJK_BOLD_KEY,
            "CJK 的粗体是思源 Medium"
        );
    }

    /// 装入后 egui 必须真能画出原版的三档字号：主字体、思源中文、粗体族。
    /// `set_fonts` 下一帧生效，所以中间隔一次 `ctx.run`。
    #[test]
    fn installing_makes_all_three_families_render() {
        let id_main = FontId::new(MAIN_SIZE, FontFamily::Proportional);
        let id_cjk = FontId::new(MAIN_SIZE, FontFamily::Proportional);
        let id_bold = FontId::new(MAIN_SIZE, bold_family());
        let id_small = FontId::new(SMALL_SIZE, small_family());
        let ctx = Context::default();
        pass(&ctx);
        assert!(
            !ctx.fonts(|f| f.has_glyphs(&id_main, "A音符")),
            "默认字体不该全有这些字形，否则这个用例证明不了什么"
        );

        let Some(_dir) = install(&ctx, 1) else {
            eprintln!("跳过：本机找不到 assets/fonts，无法验证打包字体");
            return;
        };
        pass(&ctx);
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_main, "BPM = 123")),
            "主字体拉丁字形缺失"
        );
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_cjk, "音符速度")),
            "思源中文字形缺失"
        );
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_bold, "13:37")),
            "粗体族没有字形"
        );
        assert!(
            ctx.fonts(|f| f.has_glyphs(&id_small, "12:34/ 56:78")),
            "small 族没有字形"
        );
    }
}
