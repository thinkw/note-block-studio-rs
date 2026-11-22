//! 方块贴图：乐器 → 方块名 → 16×16 贴图。
//!
//! 名称来自导出用的那张表（`SchematicOptions::ins_block` / `ins_data` 经
//! [`nbs_io::schematic::namespaced_id`]），和机器里实际摆的方块同源 —— 特效画出来的
//! 方块必然和导出的 .nbt 一致，用户改「乐器对应的方块」时两边一起变。
//!
//! 贴图本身是 Mojang 的版权素材，不入库，由 `tools/fetch_block_textures.py` 从
//! 客户端 jar 里现扒（来路记在 `assets/Blocks/PROVENANCE.json`）。找不到目录、
//! 或某个方块在 1.21 资源里换了名，都退回按名字哈希出的稳定色块：画面会失真，
//! 但不会缺格，也不会因为素材没扒就整个特效不能用。

use egui::{Color32, Context, TextureHandle, TextureOptions};
use nbs_io::schematic::{namespaced_id, SchematicOptions};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 贴图目录名（相对可执行文件/仓库根），与 `tools/fetch_block_textures.py` 一致。
pub const BLOCK_DIR: &str = "Blocks";
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
    let mut hash: u32 = 2166136261;
    for b in name.bytes() {
        hash = (hash ^ b as u32).wrapping_mul(16777619);
    }
    let r = (hash & 0xFF) as u8;
    let g = ((hash >> 8) & 0xFF) as u8;
    let b = ((hash >> 16) & 0xFF) as u8;
    // 拉向中灰，避免哈希出的颜色有的近黑有的刺眼。
    let mix = |c: u8| (c as u16 * 3 / 5 + 60) as u8;
    Color32::from_rgb(mix(r), mix(g), mix(b))
}

/// 乐器 → 方块名 的那张表。导出选项（`ins_block` / `ins_data`）一改就重建，
/// 每帧只查内存，不在音符循环里分配字符串。
pub struct InstrumentBlocks {
    names: Vec<String>,
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
            .map(|i| {
                let id = opts
                    .ins_block
                    .get(i)
                    .or_else(|| fallback.get(i))
                    .copied()
                    .unwrap_or(1);
                let data = opts.ins_data.get(i).copied().unwrap_or(0);
                namespaced_id(id, data)
            })
            .collect();
        Self { names }
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

/// 已加载的贴图集合。贴图按名字惰性加载并缓存，`None` 表示「查过，没有」。
///
/// 缓存用 `RefCell`：特效只从 [`super::FxContext`] 拿到 `&BlockTiles`，
/// 拿不到可变的 `FxState`——不然贴图就得在每帧之前全量预加载一遍。
#[derive(Default)]
pub struct BlockTiles {
    dir: Option<PathBuf>,
    files: HashMap<String, PathBuf>,
    cache: RefCell<HashMap<String, Option<Arc<TextureHandle>>>>,
}

impl BlockTiles {
    /// 候选目录：和 `nbs_audio::samples::find_sounds_dir` 同一套约定 ——
    /// 环境变量、可执行文件同级、开发期的 `target/debug/../../assets`、当前目录。
    pub fn locate() -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(p) = std::env::var("NBS_BLOCKS_DIR") {
            candidates.push(PathBuf::from(p));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(BLOCK_DIR));
                candidates.push(dir.join("assets").join(BLOCK_DIR));
                candidates.push(dir.join("datafiles").join("Data").join(BLOCK_DIR));
                for up in 2..=4 {
                    let mut dev = dir.to_path_buf();
                    for _ in 0..up {
                        dev.push("..");
                    }
                    candidates.push(dev.join("assets").join(BLOCK_DIR));
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(BLOCK_DIR));
            candidates.push(cwd.join("assets").join(BLOCK_DIR));
            candidates.push(cwd.join("../assets").join(BLOCK_DIR));
        }
        // 认 `dirt.png` 而不是认目录：空目录（刚 clone 完还没扒素材）不算找到了。
        candidates.retain(|c| c.join("dirt.png").is_file());
        candidates
            .into_iter()
            .next()
            .map(|c| c.canonicalize().unwrap_or(c))
    }

    /// 打开目录并列出文件名（一次 `read_dir`，之后只查内存）。
    pub fn open() -> Self {
        Self::open_in(Self::locate())
    }

    pub fn open_in(dir: Option<PathBuf>) -> Self {
        let mut tiles = Self {
            dir,
            files: HashMap::new(),
            cache: RefCell::new(HashMap::new()),
        };
        if let Some(dir) = &tiles.dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("png") {
                        continue;
                    }
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        tiles.files.insert(stem.to_owned(), path);
                    }
                }
            }
        }
        tiles
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// 目录里有几张贴图（素材没扒时为 0，特效自动退回色块）。
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// 方块名 → 贴图。查不到返回 `None`，调用方用 [`fallback_color`]。
    pub fn texture(&self, ctx: &Context, name: &str) -> Option<Arc<TextureHandle>> {
        if let Some(hit) = self.cache.borrow().get(name) {
            return hit.clone();
        }
        let found = self.dir.is_some().then(|| {
            resolve_file(&|stem| self.files.contains_key(stem), name)
                .and_then(|stem| self.files.get(&stem).cloned())
        });
        let loaded = match found.flatten() {
            Some(path) => load_png(ctx, &path).map(Arc::new),
            None => None,
        };
        self.cache.borrow_mut().insert(name.to_owned(), loaded.clone());
        loaded
    }

    /// 某个乐器现在的贴图（可能为 `None`，只剩色块）。
    pub fn texture_for(
        &self,
        ctx: &Context,
        blocks: &InstrumentBlocks,
        instrument: u8,
    ) -> Option<Arc<TextureHandle>> {
        self.texture(ctx, blocks.name(instrument))
    }

    /// 已经缓存过贴图的名字（素材没扒时是 0，`note()` 据此决定要不要说明一句）。
    pub fn cached(&self) -> usize {
        self.cache.borrow().values().filter(|t| t.is_some()).count()
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
    // 方块贴图是像素画：放大必须用最近邻，否则糊成一片。
    Some(ctx.load_texture(
        format!("nbs_block_{}", path.file_stem()?.to_string_lossy()),
        color,
        TextureOptions::NEAREST,
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

    /// 素材扒好了才跑：确认 16 个经典乐器的方块在 1.21.4 贴图包里真的存在。
    #[test]
    fn fetched_assets_resolve_every_classic_block() {
        let tiles = BlockTiles::open();
        if tiles.is_empty() {
            eprintln!("跳过：assets/Blocks 没素材（跑 tools/fetch_block_textures.py）");
            return;
        }
        let blocks = InstrumentBlocks::default();
        for ins in 0..16u8 {
            let name = blocks.name(ins);
            let have = |stem: &str| tiles.files.contains_key(stem);
            assert!(
                resolve_file(&have, name).is_some(),
                "{ins} 号乐器 → {name}：扒下来的贴图里没有这个方块"
            );
        }
    }
}
