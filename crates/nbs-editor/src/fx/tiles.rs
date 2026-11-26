//! 方块贴图：乐器 → 方块名 → 图上的一颗方块。
//!
//! 名称来自导出用的那张表（`SchematicOptions::ins_block` / `ins_data` 经
//! [`nbs_io::schematic::namespaced_id`]），和机器里实际摆的方块同源 —— 特效画出来的
//! 方块必然和导出的 .nbt 一致，用户改「乐器对应的方块」时两边一起变。
//!
//! 两个目录，同一个方块名，先查哪一层由消费方说（[`TileLook`]）：声道方块的摞塔先等距图，
//! 方块排和瀑布的「乐器方块」先平面；首选那层没命中仍退到另一层。
//!
//! * `assets/BlocksIso/` —— Minecraft Wiki 的机体渲染图（300×300，一颗立着的方块，
//!   四角透明）。20 个内置乐器各一张，文件主名和 `resolve_file` 算出来的名字一致。
//! * `assets/Blocks/` —— 从客户端 jar 扒的 16×16 方块**面**贴图：方块排和瀑布画的
//!   就是这一层，等距图那边缺的方块（含用户自己配的自定义乐器方块）也退到这里。
//!
//! 两边都是 Mojang 的版权素材：仓库只入「20 个内置乐器真的会画到的那几张」（白名单写在
//! `.gitignore`，由测试拿 `INSTRUMENT_BLOCKS` / `COPPER_INS_BLOCKS` 现算名单核对，改了
//! 出厂表就会红），其余不入库；平面那 1000 多张跟乐器无关，需要时由
//! `tools/fetch_block_textures.py` 从客户端 jar 里现扒（来路记在
//! `assets/Blocks/PROVENANCE.json`，等距图记在 `assets/BlocksIso/PROVENANCE.json`）。
//! 找不到目录、或某个方块在 1.21 资源里换了名，都退回按名字哈希出的稳定色块：画面会失真，
//! 但不会缺格，也不会因为素材没扒就整个特效不能用。自定义乐器在出厂表里还是默认的石头，
//! 走的是乐器色那条路，不欠贴图。
//!
//! 这几张图不是我们的著作物：归属、Mojang 条款原文、无素材发行包和下架承诺写在
//! `assets/Blocks/MOJANG-NOTICE.txt`、`assets/BlocksIso/WIKI-NOTICE.txt` 与仓库根的
//! `THIRD-PARTY-NOTICES.md`，那几份声明有没有跟着代码走由
//! `tests::the_third_party_notices_ship_with_the_textures` 核。

use egui::{Color32, Context, TextureFilter, TextureHandle, TextureOptions};
use nbs_io::schematic::{namespaced_id, SchematicOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 平面贴图目录名（相对可执行文件/仓库根），与 `tools/fetch_block_textures.py` 一致。
pub const BLOCK_DIR: &str = "Blocks";
/// 等距方块图目录名（同一个 `assets/` 根下的兄弟目录）。
pub const ISO_DIR: &str = "BlocksIso";

/// 一条特效要的是哪一种方块图：**先查哪一层**由它自己说。
///
/// 两层都入库、都常驻，但用途分开了：方块排和瀑布的「乐器方块」用 [`TileLook::Face`]
/// （16×16 的方块面，用户：方块排不要用 wiki 上下载的方块，要用回之前的那种方块）；
/// 声道方块的摞塔用 [`TileLook::Iso`]（300 px 等距图，摞起来才有「上面盖住下面」的层次）。
/// 首选那层没命中（素材没扒、或自定义方块只在另一层有图）仍退到另一层，比只剩色块好。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileLook {
    /// 客户端 jar 里扒的 16×16 方块面。
    Face,
    /// Wiki 那批 300×300 等距方块图。
    Iso,
}
/// 贴图的原始边长（Minecraft 方块贴图 16×16）。
pub const TILE_PX: f32 = 16.0;

/// 1.21 资源里改了名或拆了面的方块：`namespaced_id` 吐出的名字 → 实际文件名候选。
/// 顺序即优先级，第一个存在的文件胜出。
const ALIASES: &[(&str, &[&str])] = &[
    ("golden_rail", &["activator_rail", "detector_rail"]),
    ("rail", &["rail"]),
    ("powered_rail", &["powered_rail"]),
    ("detector_rail", &["detector_rail"]),
    ("melon_block", &["melon_side"]),
    ("brick_block", &["bricks"]),
    ("pumpkin", &["carved_pumpkin_side", "pumpkin_side"]),
    ("id_block", &["hardened_clay"]),
    ("mossstone", &["mossy_cobblestone"]),
    ("nether_brick", &["nether_bricks"]),
    ("purpur_block_2", &["purpur_block"]),
    ("wood", &["oak_log"]),
    ("wood2", &["birch_log"]),
    ("log", &["oak_log"]),
    ("web", &["cobweb"]),
    ("record", &["music_disc_13"]),
];

/// 名字在目录里找不到时，依次补的这些后缀（方块常见的多面贴图）。
const SUFFIXES: &[&str] = &[
    "", "_side", "_top", "_front", "_end", "_bottom", "_0", "_1", "_plain",
];

/// 16..19 这四格（小号一族）在导出表里没有数字 id，方块名由 [`nbs_io::schematic::COPPER_INS_BLOCKS`]
/// 直接定死 —— 导出那一路连 `ins_block` 都不查，特效跟着同一张表才对得上机体里真摆的方块。
fn copper(i: usize) -> Option<&'static str> {
    let k = i.checked_sub(nbs_io::schematic::INSTRUMENT_BLOCKS.len())?;
    nbs_io::schematic::COPPER_INS_BLOCKS.get(k).copied()
}

/// FNV-1a：字符串 → 稳定的 32 位散列。兜底色和乐器色都靠它，
/// 同一名字在任何机器上算出同一个数，画面才不会每次重启换个样。
fn fnv1a(s: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for b in s.bytes() {
        hash = (hash ^ b as u32).wrapping_mul(16777619);
    }
    hash
}

/// 找一个方块名能用的贴图文件。返回的是不带扩展名的文件主名。
pub fn resolve_file(available: &impl Fn(&str) -> bool, name: &str) -> Option<String> {
    let base = name.strip_prefix("minecraft:").unwrap_or(name);
    if base.is_empty() {
        return None;
    }
    let mut tried: Vec<&str> = Vec::new();
    if let Some((_, targets)) = ALIASES.iter().find(|(from, _)| *from == base) {
        tried.extend(targets.iter().copied());
    }
    tried.push(base);
    // 打蜡只是「不再氧化」，方块外观没换：MC 的 blockstate 里 waxed_* 直接复用没打蜡
    // 那张贴图，jar 里根本没有 waxed_copper_block.png，所以剥掉前缀再找一遍。
    if let Some(bare) = base.strip_prefix("waxed_") {
        tried.push(bare);
    }
    for candidate in tried {
        for suffix in SUFFIXES {
            let stem = format!("{candidate}{suffix}");
            if available(&stem) {
                return Some(stem);
            }
        }
    }
    None
}

/// 缺贴图时的兜底色：同一个名字在任何机器上都是同一个颜色，
/// 免得用户每次看到的方块排颜色都在换。
pub fn fallback_color(name: &str) -> Color32 {
    let hash = fnv1a(name);
    let r = (hash & 0xFF) as u8;
    let g = ((hash >> 8) & 0xFF) as u8;
    let b = ((hash >> 16) & 0xFF) as u8;
    // 拉向中灰，避免哈希出的颜色有的近黑有的刺眼。
    let mix = |c: u8| (c as u16 * 3 / 5 + 60) as u8;
    Color32::from_rgb(mix(r), mix(g), mix(b))
}

/// 乐器 → 稳定颜色：按乐器名哈希出色相，同一件乐器在任何曲子、任何机器上同色。
///
/// 自定义乐器在导出用的方块表里默认全是同一块石头（`ins_block[16..]` 没人动过），
/// 光靠方块分不出「现在响的是 S080 还是 VGE」，颜色是它们唯一分得开的地方。
/// 空名（号越界、曲子没挂这层乐器）退到按乐器号上色，至少不同号不同色。
pub fn instrument_color(name: &str, index: u8) -> Color32 {
    let hue = if name.is_empty() {
        fnv1a(&format!("#{index}"))
    } else {
        fnv1a(name)
    } % 360;
    // 只动色相：饱和与明度钉死，免得有的乐器刺眼、有的像蒙了层灰。
    let [r, g, b] = crate::theme::Theme::hsb_to_rgb(hue as f32, 55.0, 92.0);
    Color32::from_rgb(r, g, b)
}

/// 主界面画音符那行精灵（`assets/icons/block_color/00..37.png`，原版
/// `spr_block_color`）的平均色。瀑布胶囊的底色取这里，颜色就和编辑区同一个来源：
/// 素材换了色特效跟着换，不存在第二张需要维护的表。
///
/// 素材目录找不到时表是空的，调用方（`fx::synthesia`）退回 [`instrument_color`]
/// 的按名哈希 —— 观感变差，但不会没颜色。
#[derive(Clone, Default)]
pub struct NotePalette {
    frames: Vec<Option<Color32>>,
}

impl NotePalette {
    /// 读装机素材。只在「第一条特效开启」那帧调一次（`FxState::refresh`），
    /// 38 张 32×32 的 PNG 摊成一次性的开销，不进播放循环。
    pub fn open() -> Self {
        crate::icons::asset_root()
            .map(|root| Self::from_dir(&root.join("block_color")))
            .unwrap_or_default()
    }

    /// 取帧规则和 `icons::Strip::load` 一致（只要 `.png`、按文件名排序），
    /// 这样第 i 帧和主界面 blit 的第 i 帧是同一个文件。
    pub fn from_dir(dir: &Path) -> Self {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
            .collect();
        paths.sort();
        Self {
            frames: paths.iter().map(|p| average_color(p)).collect(),
        }
    }

    /// 乐器号 → 主界面给它用的那帧精灵的色。帧号口径见 `icons::block_frame`。
    pub fn color(&self, instrument: u8) -> Option<Color32> {
        let frame = crate::icons::block_frame(
            instrument as usize,
            nbs_core::model::FIRST_CUSTOM_INDEX as usize,
        );
        // `Strip::blit` 把越界帧号静默夹到最后一帧，主界面就是这么画的：
        // 第 19 个自定义乐器往后全都同一块色。跟着夹才对得上，返回 None 反而更错。
        let idx = frame.min(self.frames.len().saturating_sub(1));
        self.frames.get(idx).copied().flatten()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// 一帧精灵的平均色：只算不透明像素，透明边会把颜色冲淡。
/// 读失败或整帧透明返回 `None`，调用方按「这帧没有色」处理。
fn average_color(path: &Path) -> Option<Color32> {
    let rgba = image::open(path).ok()?.to_rgba8();
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in rgba.pixels() {
        if px.0[3] == 0 {
            continue;
        }
        r += px.0[0] as u64;
        g += px.0[1] as u64;
        b += px.0[2] as u64;
        n += 1;
    }
    (n > 0).then(|| Color32::from_rgb((r / n) as u8, (g / n) as u8, (b / n) as u8))
}

/// 乐器 → 方块名 的那张表。导出选项（`ins_block` / `ins_data`）一改就重建，
/// 每帧只查内存，不在音符循环里分配字符串。
pub struct InstrumentBlocks {
    names: Vec<String>,
    /// 这一格的方块是不是「没人配过的默认值」。默认表只有前 16 格是真方块，
    /// 往后（内置 16..19 加全部自定义乐器）清一色是石头 —— 特效据此改用乐器色。
    untouched: Vec<bool>,
}

impl Default for InstrumentBlocks {
    fn default() -> Self {
        Self::from_options(&SchematicOptions::default())
    }
}

impl InstrumentBlocks {
    pub fn from_options(opts: &SchematicOptions) -> Self {
        let count = opts.ins_block.len().max(16);
        let fallback = nbs_io::schematic::default_ins_block();
        let names = (0..count)
            .map(|i| match copper(i) {
                // 铜块一族（小号 16..19）：导出那一路直接写名字，压根不查 ins_block，
                // 特效跟着它才对得上机体里真摆的方块。
                Some(c) => c.to_string(),
                None => {
                    let id = opts
                        .ins_block
                        .get(i)
                        .or_else(|| fallback.get(i))
                        .copied()
                        .unwrap_or(1);
                    let data = opts.ins_data.get(i).copied().unwrap_or(0);
                    namespaced_id(id, data)
                }
            })
            .collect();
        let untouched = (0..count)
            .map(|i| {
                // 前 16 格默认就是各自的真方块（harp → 泥土），铜块一族也有自己的方块，
                // 都不算「没配过」。
                copper(i).is_none()
                    && i >= nbs_io::schematic::INSTRUMENT_BLOCKS.len()
                    && opts.ins_block.get(i) == fallback.get(i)
                    && opts.ins_data.get(i).copied().unwrap_or(0) == 0
            })
            .collect();
        Self { names, untouched }
    }

    /// 这件乐器在导出表里有没有一个配好的方块（而不是默认的石头）。
    /// 没有时方块排改用 [`instrument_color`]：否则一整排自定义乐器全是同一种石头，
    /// 看着就像「特效不随乐器变」。
    pub fn has_block(&self, instrument: u8) -> bool {
        !self
            .untouched
            .get(instrument as usize)
            .copied()
            .unwrap_or(false)
    }

    /// 该乐器的方块名；越界（自定义乐器号超出表长）落到最后一格。
    pub fn name(&self, instrument: u8) -> &str {
        self.names
            .get(instrument as usize)
            .or_else(|| self.names.last())
            .map(String::as_str)
            .unwrap_or("stone")
    }

    /// 表有多长（内置 20 + 自定义 236 = 256 格）。
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// 一个贴图目录：文件主名 → 路径。只 `read_dir` 一次，之后全查内存。
#[derive(Default)]
struct TileSet {
    dir: Option<PathBuf>,
    files: HashMap<String, PathBuf>,
}

impl TileSet {
    fn open(dir: Option<PathBuf>) -> Self {
        let mut set = Self {
            dir,
            files: HashMap::new(),
        };
        if let Some(dir) = &set.dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("png") {
                        continue;
                    }
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        set.files.insert(stem.to_owned(), path);
                    }
                }
            }
        }
        set
    }

    /// 方块名在这张目录表里对应的文件；没有则 `None`（去下一层或退回色块）。
    fn file(&self, name: &str) -> Option<&Path> {
        resolve_file(&|stem| self.files.contains_key(stem), name)
            .and_then(|stem| self.files.get(&stem))
            .map(PathBuf::as_path)
    }

    fn len(&self) -> usize {
        self.files.len()
    }
}

/// 已加载的贴图集合。贴图按名字惰性加载并缓存，`None` 表示「查过，没有」。
///
/// 缓存用 `Mutex`：主窗与分离浮窗共享同一个 `FxState`，两边都可能按需加载贴图；
/// `RefCell` 只够单线程借用，跨视口共享后会把缓存变成 `!Sync`。
#[derive(Default)]
pub struct BlockTiles {
    /// 16×16 的方块面（客户端 jar 里扒的）。
    face: TileSet,
    /// 300×300 的等距方块图（Wiki 机体渲染）。只有声道方块的摞塔把它排在前面。
    iso: TileSet,
    cache: Mutex<HashMap<String, Option<Arc<TextureHandle>>>>,
}

impl BlockTiles {
    /// 候选目录：和 `nbs_audio::samples::find_sounds_dir` 同一套约定 ——
    /// 环境变量、可执行文件同级、开发期的 `target/debug/../../assets`、当前目录。
    fn locate_dir(env: &str, sub: &str) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(p) = std::env::var(env) {
            candidates.push(PathBuf::from(p));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(sub));
                candidates.push(dir.join("assets").join(sub));
                candidates.push(dir.join("datafiles").join("Data").join(sub));
                for up in 2..=4 {
                    let mut dev = dir.to_path_buf();
                    for _ in 0..up {
                        dev.push("..");
                    }
                    candidates.push(dev.join("assets").join(sub));
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(sub));
            candidates.push(cwd.join("assets").join(sub));
            candidates.push(cwd.join("../assets").join(sub));
        }
        // 认 `dirt.png` 而不是认目录：空目录（刚 clone 完还没扒素材）不算找到了。
        candidates.retain(|c| c.join("dirt.png").is_file());
        candidates
            .into_iter()
            .next()
            .map(|c| c.canonicalize().unwrap_or(c))
    }

    /// 方块面目录（`assets/Blocks`）。
    pub fn locate() -> Option<PathBuf> {
        Self::locate_dir("NBS_BLOCKS_DIR", BLOCK_DIR)
    }

    /// 等距方块图目录（`assets/BlocksIso`）。
    pub fn locate_iso() -> Option<PathBuf> {
        Self::locate_dir("NBS_BLOCKS_ISO_DIR", ISO_DIR)
    }

    /// 打开装机素材：两层各自找目录，缺哪层就少哪层。
    pub fn open() -> Self {
        Self::open_at(Self::locate(), Self::locate_iso())
    }

    /// 只给一个目录时（测试、或 `NBS_BLOCKS_DIR` 指到别处），等距图按「它的兄弟目录
    /// `BlocksIso`」找 —— 出厂布局就是 `assets/Blocks` 与 `assets/BlocksIso` 并列。
    /// 传 `None` 则两层都不读盘，留给「素材缺失」那条退化路径。
    pub fn open_in(dir: Option<PathBuf>) -> Self {
        let iso = dir
            .as_deref()
            .and_then(Path::parent)
            .map(|parent| parent.join(ISO_DIR))
            .filter(|p| p.join("dirt.png").is_file());
        Self::open_at(dir, iso)
    }

    pub fn open_at(face: Option<PathBuf>, iso: Option<PathBuf>) -> Self {
        Self {
            face: TileSet::open(face),
            iso: TileSet::open(iso),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.face.dir.as_deref()
    }

    pub fn iso_dir(&self) -> Option<&Path> {
        self.iso.dir.as_deref()
    }

    /// 两层加起来有几张贴图（素材没扒时为 0，特效自动退回色块）。
    pub fn len(&self) -> usize {
        self.face.len() + self.iso.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 方块名 → 贴图：按 `prefer` 给的那一层先查，缺了才退到另一层，两层都没有才 `None`
    /// （调用方用 [`fallback_color`]）。每一层单独记「查过，没有」，播放循环里不再重复摸盘。
    pub fn texture(&self, ctx: &Context, name: &str, prefer: TileLook) -> Option<Arc<TextureHandle>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let sets: [(&str, &TileSet); 2] = match prefer {
            TileLook::Iso => [("iso", &self.iso), ("face", &self.face)],
            TileLook::Face => [("face", &self.face), ("iso", &self.iso)],
        };
        for (kind, set) in sets {
            let key = format!("{kind}\u{1}{name}");
            if let Some(hit) = cache.get(&key) {
                if hit.is_some() {
                    return hit.clone();
                }
                continue;
            }
            let loaded = set.file(name).and_then(|p| load_png(ctx, p)).map(Arc::new);
            cache.insert(key, loaded.clone());
            if loaded.is_some() {
                return loaded;
            }
        }
        None
    }

    /// 某个乐器现在的贴图（可能为 `None`，只剩色块）。`prefer` 决定先查哪一层，
    /// 见 [`TileLook`]。
    pub fn texture_for(
        &self,
        ctx: &Context,
        blocks: &InstrumentBlocks,
        instrument: u8,
        prefer: TileLook,
    ) -> Option<Arc<TextureHandle>> {
        self.texture(ctx, blocks.name(instrument), prefer)
    }

    /// 已经缓存到贴图的名字（素材没扒时是 0）。两层各记一条，按名字去重。
    pub fn cached(&self) -> usize {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<&str> = cache
            .iter()
            .filter(|(_, v)| v.is_some())
            .map(|(k, _)| k.split('\u{1}').nth(1).unwrap_or(k.as_str()))
            .collect();
        names.sort_unstable();
        names.dedup();
        names.len()
    }
}

fn load_png(ctx: &Context, path: &Path) -> Option<TextureHandle> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let mut color = egui::ColorImage::new([w as usize, h as usize], Color32::TRANSPARENT);
    for (i, px) in img.pixels().enumerate() {
        color.pixels[i] = Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]);
    }
    // 16×16 的方块面是像素画：放大必须用最近邻，否则糊成一片。等距图有 300 px，
    // 画进 45 px 的格子是缩小，得线性 + mipmap，不然棱边全是锯齿。
    let options = if w.max(h) > TILE_PX as u32 * 2 {
        TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear))
    } else {
        TextureOptions::NEAREST
    };
    Some(ctx.load_texture(
        format!("nbs_block_{}", path.file_stem()?.to_string_lossy()),
        color,
        options,
    ))
}

/// 方块在屏幕上摆多大：16 的整数倍，最近邻放大才不会看成模糊的 1.7 倍。
pub fn tile_size(scale: f32) -> f32 {
    (TILE_PX * 3.0 * scale).round().max(TILE_PX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_and_suffix_pick_the_first_existing_file() {
        let have = |stem: &str| matches!(stem, "bricks" | "melon_side" | "white_wool");
        assert_eq!(resolve_file(&have, "brick_block").as_deref(), Some("bricks"));
        assert_eq!(
            resolve_file(&have, "melon_block").as_deref(),
            Some("melon_side")
        );
        assert_eq!(resolve_file(&have, "white_wool").as_deref(), Some("white_wool"));
        // 认不出来的名字不给色块以外的假象。
        assert_eq!(resolve_file(&have, "not_a_block"), None);
        assert_eq!(resolve_file(&have, ""), None);
        assert_eq!(
            resolve_file(&have, "minecraft:white_wool").as_deref(),
            Some("white_wool"),
            "namespaced 前缀要吃掉"
        );
    }

    #[test]
    fn multi_face_blocks_fall_back_to_side_then_top() {
        let have = |stem: &str| stem == "pumpkin_side" || stem == "hay_block_side";
        assert_eq!(resolve_file(&have, "pumpkin").as_deref(), Some("pumpkin_side"));
        assert_eq!(
            resolve_file(&have, "hay_block").as_deref(),
            Some("hay_block_side")
        );
        let only_top = |stem: &str| stem == "bone_block_top";
        assert_eq!(
            resolve_file(&only_top, "bone_block").as_deref(),
            Some("bone_block_top")
        );
    }

    #[test]
    fn fallback_color_is_stable_and_not_extreme() {
        let a = fallback_color("dirt");
        let b = fallback_color("dirt");
        assert_eq!(a, b, "同一个方块每次颜色要一样");
        assert_ne!(a, fallback_color("glass"), "不同方块要能看出区别");
        // 兜底色不能黑到看不见，也不能花到刺眼。
        assert!(a.r() > 40 && a.g() > 40 && a.b() > 40);
        assert!(a.r() < 216 && a.g() < 216 && a.b() < 216);
    }

    /// 自定义乐器在默认表里全是石头：特效得知道「这格没人配过」，才能改用乐器色。
    #[test]
    fn only_configured_instruments_have_a_block() {
        let blocks = InstrumentBlocks::default();
        for ins in 0..20u8 {
            assert!(blocks.has_block(ins), "{ins} 号内置乐器默认就有自己的方块");
        }
        // 20 往后是自定义乐器：出厂清一色石头，谁都没配过。
        for ins in [20u8, 100, 255] {
            assert!(!blocks.has_block(ins), "{ins} 号没配方块");
            assert_eq!(blocks.name(ins), "stone");
        }
        // 用户配过一格，就该认出来。
        let mut opts = SchematicOptions::default();
        opts.ins_block[20] = nbs_io::schematic::INSTRUMENT_BLOCKS[0];
        let tuned = InstrumentBlocks::from_options(&opts);
        assert!(tuned.has_block(20), "配了 dirt 就不算默认值了");
        assert!(!tuned.has_block(21), "邻格没动，还是默认的石头");
    }

    /// 小号一族（16..19）的方块不是 ins_block 里那块默认石头，而是导出那一路写死的
    /// waxed 铜块 —— 特效跟着它，画面上才认得出「响的是铜块一族」而不是清一色石头。
    /// 用户把 ins_block[16] 改成别的也不生效，因为 .nbt 导出压根不查那格。
    #[test]
    fn the_copper_family_uses_the_export_tables_blocks() {
        let blocks = InstrumentBlocks::default();
        assert_eq!(blocks.name(16), "waxed_copper_block");
        assert_eq!(blocks.name(17), "waxed_exposed_copper");
        assert_eq!(blocks.name(18), "waxed_weathered_copper");
        assert_eq!(blocks.name(19), "waxed_oxidized_copper");
        let mut opts = SchematicOptions::default();
        opts.ins_block[16] = nbs_io::schematic::INSTRUMENT_BLOCKS[0];
        assert_eq!(
            InstrumentBlocks::from_options(&opts).name(16),
            "waxed_copper_block",
            "铜块一族不吃 ins_block，改了也得跟导出一致"
        );
    }

    /// waxed 铜块在 jar 里没有自己的贴图，blockstate 复用未打蜡那张。
    #[test]
    fn waxed_blocks_resolve_to_the_unwaxed_texture() {
        let have = |stem: &str| stem == "copper_block" || stem == "oxidized_copper";
        assert_eq!(
            resolve_file(&have, "waxed_copper_block").as_deref(),
            Some("copper_block")
        );
        assert_eq!(
            resolve_file(&have, "waxed_oxidized_copper").as_deref(),
            Some("oxidized_copper")
        );
        // 没打蜡那张也没有 → 不给假方块。
        let none = |_stem: &str| false;
        assert_eq!(resolve_file(&none, "waxed_copper_block"), None);
    }

    #[test]
    fn instrument_colors_are_stable_and_distinct() {
        assert_eq!(
            instrument_color("S080", 20),
            instrument_color("S080", 21),
            "同一件乐器不管在这首曲子里排第几号，颜色都得一样"
        );
        assert_ne!(instrument_color("S080", 20), instrument_color("VGE", 21));
        assert_ne!(instrument_color("Harp", 0), instrument_color("Bell", 7));
        // 没名字（号越界）按号上色，不同号还是分得开。
        assert_ne!(instrument_color("", 21), instrument_color("", 22));
        assert_eq!(instrument_color("", 21), instrument_color("", 21));
        // 上色要看得清：不能暗到贴图上看不见，也不能亮到一片白。
        for name in ["Harp", "Bell", "S080", "VGE", "idle", "第26层", ""] {
            let c = instrument_color(name, 3);
            let (lo, hi) = (c.r().min(c.g().min(c.b())), c.r().max(c.g().max(c.b())));
            assert!(hi > 150 && lo < 210, "{name:?} → {c:?} 太暗或太白");
            assert!(hi - lo > 30, "{name:?} → {c:?} 灰到分不出色相");
        }
    }

    #[test]
    fn the_sixteen_classic_instruments_have_named_blocks() {
        let blocks = InstrumentBlocks::from_options(&SchematicOptions::default());
        for ins in 0..16u8 {
            let name = blocks.name(ins);
            assert!(!name.is_empty(), "{ins} 号乐器没有方块名");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{ins} 号乐器 → {name}：不是小写加下划线，1.21 资源里对不上文件名"
            );
        }
        // 经典十六个的名字要真的对应 INSTRUMENT_BLOCKS 那张表。
        assert_eq!(blocks.name(0), "dirt", "Harp");
        assert_eq!(blocks.name(7), "gold_block", "Bell");
        assert_eq!(blocks.name(15), "glowstone", "Pling");
        // 没配过方块的乐器号落到表尾，而不是空字符串。
        assert!(!blocks.name(255).is_empty());
    }

    /// 入库的那几张贴图 = 20 个内置乐器会画到的那几张，一张不多一张不少。
    /// 名单不是手抄的：出厂表（`INSTRUMENT_BLOCKS` + 铜块一族）现算，再走一遍
    /// [`resolve_file`]。这里把 `.gitignore` 的白名单当成「新克隆机器上
    /// `assets/Blocks` 的全部内容」来解，所以测的是克隆后对不对得上，
    /// 而不是本机有没有扒全 —— 本机扒了 1000 张也算不出缺哪张。
    #[test]
    fn the_shipped_textures_are_exactly_the_builtin_instrument_blocks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("仓库根")
            .to_path_buf();
        let ignore = std::fs::read_to_string(root.join(".gitignore")).expect("读不到 .gitignore");
        let listed: std::collections::BTreeSet<String> = ignore
            .lines()
            .filter_map(|l| l.trim().strip_prefix("!assets/Blocks/"))
            .filter(|f| f.ends_with(".png"))
            .map(str::to_owned)
            .collect();
        assert!(!listed.is_empty(), ".gitignore 里没有 !assets/Blocks/*.png 的白名单");

        let blocks = InstrumentBlocks::from_options(&SchematicOptions::default());
        let has = |stem: &str| listed.contains(&format!("{stem}.png"));
        let mut used: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for ins in 0..20u8 {
            let name = blocks.name(ins);
            match resolve_file(&has, name) {
                Some(stem) => {
                    used.insert(format!("{stem}.png"));
                }
                None => panic!(
                    "{ins} 号乐器 → {name}：入库白名单里没有这张贴图。\
                     跑了 tools/fetch_block_textures.py 之后把 !assets/Blocks/<文件主名>.png 补进 .gitignore。"
                ),
            }
        }
        let extra: Vec<&String> = listed.iter().filter(|f| !used.contains(*f)).collect();
        assert!(
            extra.is_empty(),
            "入库贴图里有乐器用不到的：{extra:?} —— 口径是只入乐器会用到的那几张"
        );
    }

    /// 等距图那 20 张的口径和方块面一样：入库名单 = 20 个内置乐器会画到的那几颗，
    /// 由出厂表现算 + [`resolve_file`] 走一遍。这条同时钉住「文件名的改法」——
    /// Wiki 原名（`Block_of_Gold_JE6_BE3.png`）入库时统一改成了 `resolve_file` 吐出的
    /// 主名（`gold_block.png`），谁手滑按原名拷进来就会在这儿红。
    #[test]
    fn the_shipped_iso_textures_are_exactly_the_builtin_instrument_blocks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("仓库根")
            .to_path_buf();
        let ignore = std::fs::read_to_string(root.join(".gitignore")).expect("读不到 .gitignore");
        let listed: std::collections::BTreeSet<String> = ignore
            .lines()
            .filter_map(|l| l.trim().strip_prefix("!assets/BlocksIso/"))
            .filter(|f| f.ends_with(".png"))
            .map(str::to_owned)
            .collect();
        assert!(
            !listed.is_empty(),
            ".gitignore 里没有 !assets/BlocksIso/*.png 的白名单"
        );

        let blocks = InstrumentBlocks::from_options(&SchematicOptions::default());
        let has = |stem: &str| listed.contains(&format!("{stem}.png"));
        let mut used: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for ins in 0..20u8 {
            let name = blocks.name(ins);
            match resolve_file(&has, name) {
                Some(stem) => {
                    used.insert(format!("{stem}.png"));
                }
                None => panic!(
                    "{ins} 号乐器 → {name}：入库的等距图里没有这张。\
                     从 Wiki 存下来后按 {name}.png 改名，并把 !assets/BlocksIso/{name}.png 补进 .gitignore。"
                ),
            }
        }
        let extra: Vec<&String> = listed.iter().filter(|f| !used.contains(*f)).collect();
        assert!(
            extra.is_empty(),
            "入库等距图里有乐器用不到的：{extra:?} —— 口径是只入乐器会用到的那几张"
        );
    }

    /// 同一个方块名两层都有图时，**先查哪一层由调用方说了算**：声道方块（[`TileLook::Iso`]）
    /// 拿 Wiki 那颗立方体，方块排/瀑布（[`TileLook::Face`]）拿 16×16 的方块面（用户：方块排
    /// 不要用 wiki 上下载的方块，要用回之前的那种方块）。首选那层没覆盖到的名字（用户自己配的
    /// 方块）还得退到另一层，别因为「这层没命中」就整个没图。用两张不同颜色的合成 PNG 比对，
    /// 不依赖本机素材扒没扒。
    #[test]
    fn each_consumer_asks_for_its_own_layer() {
        let ctx = Context::default();
        let base = std::env::temp_dir().join(format!("nbs_iso_{}", std::process::id()));
        let face = base.join("Blocks");
        let iso = base.join("BlocksIso");
        std::fs::create_dir_all(&face).expect("建临时目录");
        std::fs::create_dir_all(&iso).expect("建临时目录");
        // 16×16 走最近邻那条路，64×64 走线性 + mipmap 那条路，两个分支各测一次。
        image::RgbaImage::from_pixel(16, 16, image::Rgba([10, 20, 30, 255]))
            .save(face.join("dirt.png"))
            .expect("写平面 dirt");
        image::RgbaImage::from_pixel(16, 16, image::Rgba([7, 7, 7, 255]))
            .save(face.join("stone.png"))
            .expect("写平面 stone");
        image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 210, 220, 255]))
            .save(iso.join("dirt.png"))
            .expect("写等距 dirt");

        let tiles = BlockTiles::open_at(Some(face.clone()), Some(iso.clone()));
        let cube = tiles
            .texture(&ctx, "dirt", TileLook::Iso)
            .expect("两层都有 dirt，总该读到一张");
        assert_eq!(
            cube.size(),
            [64, 64],
            "声道方块该拿等距图那张（64），不是 16×16 的方块面"
        );
        assert_eq!(
            tiles
                .texture(&ctx, "dirt", TileLook::Face)
                .expect("方块面就在首选那层")
                .size(),
            [16, 16],
            "方块排该拿方块面那张，不是 wiki 的立方体"
        );
        let flat = tiles
            .texture(&ctx, "stone", TileLook::Iso)
            .expect("等距层没有 stone，要退到方块面");
        assert_eq!(flat.size(), [16, 16], "退到的是方块面那张");
        assert!(tiles
            .texture(&ctx, "not_a_block", TileLook::Face)
            .is_none());
        assert_eq!(tiles.cached(), 2, "只有真读到贴图的名字才算数");

        // `open_in` 走的是同一套：给定 Blocks 目录，等距图按兄弟目录找。
        let via_sibling = BlockTiles::open_in(Some(face.clone()));
        assert_eq!(
            via_sibling.iso_dir(),
            Some(iso.as_path()),
            "open_in 要认出 assets/Blocks 旁边的 BlocksIso"
        );
        assert_eq!(
            via_sibling
                .texture(&ctx, "dirt", TileLook::Iso)
                .map(|t| t.size()),
            Some([64, 64]),
            "open_in 认出来的那层要真的用上报给画面"
        );
        // 旁边没 BlocksIso 时不能凭空长出一层。
        let lonely = base.join("nested").join("Blocks");
        std::fs::create_dir_all(&lonely).expect("建临时目录");
        std::fs::copy(iso.join("dirt.png"), lonely.join("dirt.png")).expect("拷 dirt");
        assert_eq!(
            BlockTiles::open_in(Some(lonely)).iso_dir(),
            None,
            "旁边没有 BlocksIso 就不该有第二层"
        );
        assert_eq!(BlockTiles::open_in(None).len(), 0, "传 None 就该完全不读盘");
        std::fs::remove_dir_all(&base).ok();
    }

    /// 入库的等距图得真是「一颗方块」：正方形、比方块面大得多（所以走线性那条路）、
    /// 四角透明中心不透明。顺带证明这些 PNG 解得开 —— 提交时被截断会红在这儿。
    #[test]
    fn the_shipped_iso_images_are_cube_sprites() {
        let dir = match BlockTiles::locate_iso() {
            Some(d) => d,
            None => {
                eprintln!("跳过：找不到 assets/BlocksIso");
                return;
            }
        };
        let tiles = TileSet::open(Some(dir.clone()));
        let blocks = InstrumentBlocks::default();
        for ins in 0..20u8 {
            let name = blocks.name(ins);
            let file = match tiles.file(name) {
                Some(f) => f.to_path_buf(),
                None => panic!("{name}（{ins} 号乐器）在等距目录里没有对应文件：{dir:?}"),
            };
            let img = match image::open(&file) {
                Ok(img) => img.to_rgba8(),
                Err(e) => panic!("{} 解不开：{e}", file.display()),
            };
            let (w, h) = (img.width(), img.height());
            assert_eq!(w, h, "{}：等距图要正方", file.display());
            assert!(w >= 64, "{}：{} px 太小，缩到格子会糊", file.display(), w);
            let alpha = |x: u32, y: u32| img.get_pixel(x, y).0[3];
            let side = w - 1;
            assert!(
                alpha(0, 0) == 0 && alpha(side, 0) == 0 && alpha(0, side) == 0 && alpha(side, side) == 0,
                "{}：四角该透明（立方体外面的空白）",
                file.display()
            );
            assert!(alpha(w / 2, h / 2) > 200, "{}：中心该是方块本体", file.display());
        }
    }

    /// 声明要跟着素材走：贴图入库那一刻，「这几张不是我们的」那几句话必须在仓库里，
    /// 而且要真的随包分发（`.gitignore` 放行、被 LICENSE 指到）。这条测试拦的是
    /// 「图加了、声明没加」和「声明写了却被误 ignore」两种飘。
    #[test]
    fn the_third_party_notices_ship_with_the_textures() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("仓库根")
            .to_path_buf();
        for rel in [
            "assets/Blocks/MOJANG-NOTICE.txt",
            "assets/Blocks/PROVENANCE.json",
            "assets/BlocksIso/WIKI-NOTICE.txt",
            "assets/BlocksIso/PROVENANCE.json",
            "THIRD-PARTY-NOTICES.md",
        ] {
            assert!(
                root.join(rel).is_file(),
                "{rel} 不见了：入库素材的归属声明不能缺"
            );
        }
        let ignore = std::fs::read_to_string(root.join(".gitignore")).expect("读不到 .gitignore");
        for rel in [
            "!assets/Blocks/MOJANG-NOTICE.txt",
            "!assets/BlocksIso/WIKI-NOTICE.txt",
            "!assets/BlocksIso/PROVENANCE.json",
        ] {
            assert!(
                ignore.lines().any(|l| l.trim() == rel),
                "{rel} 没在 .gitignore 里放行，克隆的人看不到它"
            );
        }
        let license = std::fs::read_to_string(root.join("LICENSE")).expect("读不到 LICENSE");
        assert!(
            license.contains("THIRD-PARTY-NOTICES.md"),
            "LICENSE 里没有指向第三方素材声明的那段"
        );
        for rel in [
            "assets/Blocks/PROVENANCE.json",
            "assets/BlocksIso/PROVENANCE.json",
        ] {
            let prov = std::fs::read_to_string(root.join(rel)).expect("读不到 PROVENANCE.json");
            let v: serde_json::Value =
                serde_json::from_str(&prov).expect("PROVENANCE.json 不是合法 JSON");
            assert_eq!(
                v["not_under_agpl"].as_bool(),
                Some(true),
                "{rel} 要写明这些贴图不在 AGPL 授权范围内"
            );
        }
    }

    /// 帧号口径：内置按序号，自定义在那 18 帧里循环。主界面的 `push_note` 和
    /// 瀑布取色共用这一个函数，改这里等于同时改两边 —— 不会飘。
    #[test]
    fn block_frame_wraps_custom_instruments() {
        assert_eq!(crate::icons::block_frame(3, 20), 3, "内置就是自己的帧");
        assert_eq!(crate::icons::block_frame(19, 20), 19, "19 号还没到自定义区");
        assert_eq!(crate::icons::block_frame(20, 20), 20, "第一个自定义");
        assert_eq!(crate::icons::block_frame(37, 20), 37, "第 18 个自定义正好到表尾");
        assert_eq!(
            crate::icons::block_frame(38, 20),
            20,
            "第 19 个自定义循环回那 18 帧的第一帧，跟主界面一样"
        );
    }

    /// 配色对齐主界面：`assets/icons/block_color` 的 38 帧都要有色，自定义序号
    /// 在那 18 帧里循环（主界面就是这么选的），素材缺帧时才夹到现有表尾。
    #[test]
    fn the_palette_covers_the_frames_the_grid_draws() {
        let p = NotePalette::open();
        if p.is_empty() {
            eprintln!("跳过：assets/icons/block_color 没素材");
            return;
        }
        assert_eq!(p.len(), 38, "block_color 是 38 帧");
        for ins in 0..40u8 {
            assert!(p.color(ins).is_some(), "{ins} 号乐器没色");
        }
        assert_ne!(p.color(1), p.color(3), "不同帧该有不同颜色，否则等于没对齐");
        assert_eq!(
            p.color(38),
            p.color(20),
            "第 19 个自定义循环回首帧，主界面画的也是它"
        );
        // 空表不能假装有色：调用方要靠 None 退回按名哈希。
        assert_eq!(NotePalette::default().color(1), None);
        assert_eq!(NotePalette::default().len(), 0);
    }
}
