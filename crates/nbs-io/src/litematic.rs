//! 自写的 Litematica 写出器（`.litematic`）：多子区域 + `Long_Array` 位流 + 带容器内容的箱子。
//!
//! 路线 A（[`crate::fractal`]）用的是上游 `litematica` crate，它只出**单区域**、`TileEntities`
//! 恒是空表，写不出「箱子里装了哪些唱片」这一维。唱片工坊那一档要 40 个固定尺寸子区域、
//! 每格箱子挂一只装满足够潜影盒的容器，所以这一份自己写。
//!
//! 口径来自 Litematica 的存档格式与 minecraft.wiki 的 NBT 规范：
//! - 根标签：`Version` / `SubVersion` / `MinecraftDataVersion` / `Metadata` / `Regions`；
//!   26.1 那一代是 `Version=7`、`DataVersion=4786`，1.20.4 是 `Version=6`、`DataVersion=3700`。
//! - 每个子区域：`Position` / `Size` / `BlockStatePalette` / `BlockStates` / `TileEntities` /
//!   `Entities` / `PendingBlockTicks` / `PendingFluidTicks`，字段顺序照上游转换器写，
//!   这样两边产物能逐字节对照。
//! - `BlockStates` 是 `TAG_Long_Array`（编号 12），位宽 `max(2, ceil(log2(palette)))`，
//!   LSB 优先、**允许跨 long 边界**；格子线性下标是 `((y * sizeZ) + z) * sizeX + x`。
//! - 箱内物品在两代 MC 里形状不同：26.1 用组件 `components`/`minecraft:container`，
//!   1.20.4 用 `tag`/`BlockEntityTag`/`Items`；数量字段也一个是大写 `Count`(byte)、一个是小写 `count`(int)。
//! - 唯一有意的不同：`Metadata.TotalBlocks` 这里写真实放置的方块数，上游转换器写的是箱子数。
//!   这一档只进投影模组的界面，读取方按 `BlockStates` 自己重算，不影响摆放。

use std::collections::HashMap;
use std::io::Write;

use flate2::{write::GzEncoder, Compression};

use crate::schematic::Nbt;

/// 目标 MC 世代：一次决定 `Version`、`DataVersion` 和物品 NBT 的形状。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Generation {
    /// 1.20.4 那一代：`Version=6`，箱子用 `Count`(byte) + `tag.BlockEntityTag.Items`。
    Legacy,
    /// 26.1 那一代：`Version=7`，箱子用 `count`(int) + `components["minecraft:container"]`。
    #[default]
    Modern,
}

impl Generation {
    pub const ALL: [Self; 2] = [Self::Legacy, Self::Modern];

    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy => "1.20.4",
            Self::Modern => "26.1",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Legacy => nbs_core::tr(
                "Version 6 projection; chest contents use the old Item form (capital `Count`, `BlockEntityTag`).",
                "Version 6 投影；箱内物品用旧写法（大写 Count 与 BlockEntityTag）。",
            ),
            Self::Modern => nbs_core::tr(
                "Version 7 projection; chest contents use item components (`minecraft:container`), which 1.20.4 readers ignore.",
                "Version 7 投影；箱内物品用组件写法（minecraft:container），1.20.4 的读取方不认这一份。",
            ),
        }
    }

    fn version(self) -> i32 {
        match self {
            Self::Legacy => 6,
            Self::Modern => 7,
        }
    }

    fn data_version(self) -> i32 {
        match self {
            Self::Legacy => 3700,
            Self::Modern => 4786,
        }
    }
}

/// 一个方块状态：名字加属性。属性顺序固定写出来才好逐字节对照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub name: String,
    pub props: Vec<(String, String)>,
}

impl State {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), props: Vec::new() }
    }

    pub fn with(name: impl Into<String>, props: &[(&str, &str)]) -> Self {
        Self {
            name: name.into(),
            props: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn key(&self) -> String {
        let mut s = self.name.clone();
        for (k, v) in &self.props {
            s.push('|');
            s.push_str(k);
            s.push('=');
            s.push_str(v);
        }
        s
    }
}

/// 一件物品：物品 id 与数量。装进潜影盒时数量按世代换成 `count`(int) 或 `Count`(byte)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub count: i32,
}

impl Item {
    pub fn new(id: impl Into<String>, count: i32) -> Self {
        Self { id: id.into(), count }
    }
}

/// 一格容器方块：局部坐标、朝向、左右箱，以及它挂的潜影盒（每只最多 27 件，最多 54 只）。
#[derive(Clone, Debug)]
pub struct Chest {
    pub at: [i32; 3],
    pub facing: String,
    pub half: String,
    pub boxes: Vec<Vec<Item>>,
}

/// 一个子区域：固定包围盒 + palette + 逐格下标 + 箱子。
#[derive(Clone, Debug)]
pub struct Region {
    pub name: String,
    pub min: [i32; 3],
    pub size: [i32; 3],
    palette: Vec<State>,
    index: HashMap<String, u16>,
    cells: Vec<u16>,
    chests: Vec<Chest>,
}

impl Region {
    pub fn new(name: impl Into<String>, min: [i32; 3], size: [i32; 3]) -> Self {
        let volume = (size[0] * size[1] * size[2]).max(0) as usize;
        let air = State::new("minecraft:air");
        let mut index = HashMap::new();
        index.insert(air.key(), 0u16);
        Self {
            name: name.into(),
            min,
            size,
            palette: vec![air],
            index,
            cells: vec![0; volume],
            chests: Vec::new(),
        }
    }

    /// 区域局部坐标（0 起）的线性下标，顺序 y -> z -> x，与 `PalettedContainer` 一致。
    pub fn index_of(&self, x: i32, y: i32, z: i32) -> usize {
        ((y * self.size[2] + z) * self.size[0] + x) as usize
    }

    /// 局部坐标是否还在包围盒里。
    pub fn inside(&self, x: i32, y: i32, z: i32) -> bool {
        [x, y, z].iter().zip(self.size).all(|(v, n)| *v >= 0 && *v < n)
    }

    fn palette_put(&mut self, state: &State) -> u16 {
        let key = state.key();
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.palette.len() as u16;
        self.palette.push(state.clone());
        self.index.insert(key, i);
        i
    }

    /// 往局部坐标放一个方块状态，返回它在 palette 里的编号。
    pub fn put(&mut self, x: i32, y: i32, z: i32, state: &State) -> u16 {
        let id = self.palette_put(state);
        let at = self.index_of(x, y, z);
        if at < self.cells.len() {
            self.cells[at] = id;
        }
        id
    }

    /// 放一格箱子并给它挂容器内容：`facing` 是箱子朝向，`half` 是 `left` / `right`。
    pub fn chest(&mut self, x: i32, y: i32, z: i32, facing: &str, half: &str, boxes: Vec<Vec<Item>>) {
        let state = State::with(
            "minecraft:chest",
            &[("facing", facing), ("type", half), ("waterlogged", "false")],
        );
        self.put(x, y, z, &state);
        self.chests.push(Chest {
            at: [x, y, z],
            facing: facing.to_string(),
            half: half.to_string(),
            boxes,
        });
    }

    /// 非空气格数，也就是真放进去的方块数。
    pub fn placed(&self) -> usize {
        self.cells.iter().filter(|&&c| c != 0).count()
    }

    /// 这一臂上的容器格，按放入顺序。
    pub fn chests(&self) -> &[Chest] {
        &self.chests
    }

    /// palette 长度；空区域（只有空气）也要写得出。
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// 位宽：`max(2, ceil(log2(palette)))`。上游转换器也是这个下限，所以哪怕只用两种状态
    /// 也按 2 bit 存——这是与它逐字节对齐的前提。
    pub fn bits(&self) -> u32 {
        let raw = if self.palette.len() <= 1 {
            1
        } else {
            let n = self.palette.len() as u64 - 1;
            (64 - n.leading_zeros()) as usize
        };
        raw.max(2) as u32
    }
}

/// 一整份投影文件。
#[derive(Clone, Debug)]
pub struct Litematic {
    pub gen: Generation,
    pub name: String,
    pub author: String,
    pub description: String,
    pub regions: Vec<Region>,
    /// 毫秒时间戳；写死一个值就能让产物逐字节比对不受运行时间影响。
    pub now_ms: i64,
}

impl Litematic {
    pub(crate) fn volume(&self) -> i64 {
        self.regions
            .iter()
            .map(|r| r.size[0] as i64 * r.size[1] as i64 * r.size[2] as i64)
            .sum()
    }

    pub(crate) fn enclosing(&self) -> ([i32; 3], [i32; 3]) {
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        for r in &self.regions {
            for a in 0..3 {
                min[a] = min[a].min(r.min[a]);
                max[a] = max[a].max(r.min[a] + r.size[a] - 1);
            }
        }
        if min[0] == i32::MAX {
            return ([0; 3], [0; 3]);
        }
        (min, max)
    }
}

/// 逐格下标打包成 long 数组：LSB 优先、可跨 long 边界，跨出去的那一半高位写进下一个 long。
pub fn pack_states(indices: &[u16], bits: u32) -> Vec<i64> {
    let words = (indices.len() * bits as usize).div_ceil(64);
    let mut longs = vec![0u64; words];
    let mask = (1u64 << bits) - 1;
    for (i, &value) in indices.iter().enumerate() {
        let start = i * bits as usize;
        let word = start / 64;
        let shift = (start % 64) as u32;
        let value = (value as u64) & mask;
        longs[word] |= value << shift;
        if shift + bits > 64 {
            longs[word + 1] |= value >> (64 - shift);
        }
    }
    longs.into_iter().map(|v| v as i64).collect()
}

/// 写成 gzip 后的 `.litematic` 字节。
pub fn write(doc: &Litematic) -> Result<Vec<u8>, String> {
    if doc.regions.is_empty() {
        return Err(crate::maestro::msg(
            "There is nothing to project: the layout produced no subregion.".to_string(),
            "没有可投影的内容：布局一个子区域都没产出。".to_string(),
        ));
    }
    let mut n = Nbt::default();
    n.compound("");
    n.int("Version", doc.gen.version());
    n.int("SubVersion", 1);
    n.int("MinecraftDataVersion", doc.gen.data_version());

    let (emin, emax) = doc.enclosing();
    n.compound("Metadata");
    n.string("Name", &doc.name);
    n.string("Author", &doc.author);
    n.string("Description", &doc.description);
    n.long("TimeCreated", doc.now_ms);
    n.long("TimeModified", doc.now_ms);
    n.int("RegionCount", doc.regions.len() as i32);
    n.int("TotalVolume", doc.volume() as i32);
    n.int(
        "TotalBlocks",
        doc.regions.iter().map(|r| r.placed() as i64).sum::<i64>() as i32,
    );
    n.compound("EnclosingSize");
    for (k, v) in [("x", 0), ("y", 1), ("z", 2)] {
        n.int(k, emax[v] - emin[v] + 1);
    }
    n.end(); // EnclosingSize
    n.end(); // Metadata

    n.compound("Regions");
    let mut used: HashMap<String, u32> = HashMap::new();
    for region in &doc.regions {
        // 子区域的名字就是投影模组界面里列出的分区名；重名时补 _2、_3，别覆盖掉前一个。
        let mut name = region.name.clone();
        let mut suffix = 2u32;
        while used.contains_key(&name) {
            name = format!("{}_{}", region.name, suffix);
            suffix += 1;
        }
        used.insert(name.clone(), 1);

        n.compound(&name);
        n.compound("Position");
        for (k, v) in [("x", 0), ("y", 1), ("z", 2)] {
            n.int(k, region.min[v]);
        }
        n.end();
        n.compound("Size");
        for (k, v) in [("x", 0), ("y", 1), ("z", 2)] {
            n.int(k, region.size[v]);
        }
        n.end();

        n.list("BlockStatePalette", region.palette.len() as i32, 10);
        for state in &region.palette {
            n.string("Name", &state.name);
            if !state.props.is_empty() {
                n.compound("Properties");
                for (k, v) in &state.props {
                    n.string(k, v);
                }
                n.end();
            }
            n.end(); // palette 元素
        }

        n.long_array("BlockStates", &pack_states(&region.cells, region.bits()));

        n.list("TileEntities", region.chests.len() as i32, 10);
        for chest in &region.chests {
            n.string("id", "minecraft:chest");
            n.int("x", chest.at[0]);
            n.int("y", chest.at[1]);
            n.int("z", chest.at[2]);
            n.list("Items", chest.boxes.len() as i32, 10);
            for (slot, box_items) in chest.boxes.iter().enumerate() {
                write_shulker(&mut n, doc.gen, slot as u8, box_items);
            }
            n.end(); // TileEntity 元素
        }

        for key in ["Entities", "PendingBlockTicks", "PendingFluidTicks"] {
            n.list(key, 0, 10); // 空列表没有元素，也就没有 TAG_End。
        }
        n.end(); // region
    }
    n.end(); // Regions
    n.end(); // root

    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&n.buf)
        .map_err(|e| crate::maestro::msg(format!("gzip failed: {e}"), format!("gzip 失败：{e}")))?;
    enc.finish()
        .map_err(|e| crate::maestro::msg(format!("gzip finish failed: {e}"), format!("gzip 收尾失败：{e}")))
}

/// 一格里挂的一只潜影盒：盒本身占一个 `Slot`，里面装的才是唱片与填充物。
fn write_shulker(n: &mut Nbt, gen: Generation, slot: u8, items: &[Item]) {
    match gen {
        Generation::Modern => {
            n.string("id", "minecraft:shulker_box");
            n.int("count", 1);
            n.byte("Slot", slot);
            n.compound("components");
            n.list("minecraft:container", items.len() as i32, 10);
            for (i, item) in items.iter().enumerate() {
                // 顺序照上游：先 `slot` 后 `item`，这样两份产物能逐字节对照。
                n.int("slot", i as i32);
                n.compound("item");
                n.string("id", &item.id);
                n.int("count", item.count);
                n.end();
                n.end(); // container 元素
            }
            n.end(); // components
            n.end(); // Items 元素
        }
        Generation::Legacy => {
            n.string("id", "minecraft:shulker_box");
            n.byte("Count", 1);
            n.byte("Slot", slot);
            n.compound("tag");
            n.compound("BlockEntityTag");
            n.string("id", "minecraft:shulker_box");
            n.list("Items", items.len() as i32, 10);
            for (i, item) in items.iter().enumerate() {
                n.string("id", &item.id);
                n.byte("Count", item.count as u8);
                n.byte("Slot", i as u8);
                n.end(); // 盒内物品元素
            }
            n.end(); // BlockEntityTag
            n.end(); // tag
            n.end(); // Items 元素
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{parse, Value};
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn inflate(bytes: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        GzDecoder::new(bytes).read_to_end(&mut raw).unwrap();
        raw
    }

    /// 按位宽把 long 数组还原成逐格下标。故意写成与 [`pack_states`] 不同的逐位读法，
    /// 这样两侧一起写错时这条断言会先炸。
    fn unpack(longs: &[i64], bits: u32, count: usize) -> Vec<u16> {
        (0..count)
            .map(|i| {
                (0..bits)
                    .map(|b| {
                        let bit = i * bits as usize + b as usize;
                        let word = (longs[bit / 64] as u64) >> (bit % 64) as u32;
                        ((word & 1) as u16) << b
                    })
                    .sum()
            })
            .collect()
    }

    /// 一份两区域、一只双箱的样例：一个子区域有内容，另一个只有空气也要写得出。
    fn sample(gen: Generation) -> Litematic {
        let mut main = Region::new("module", [0, 64, 0], [2, 1, 2]);
        main.put(1, 0, 0, &State::new("minecraft:note_block"));
        main.chest(
            0,
            0,
            1,
            "north",
            "right",
            vec![
                vec![Item::new("minecraft:music_disc_pigstep", 1)],
                vec![
                    Item::new("minecraft:music_disc_stal", 1),
                    Item::new("minecraft:light_blue_carpet", 62),
                ],
            ],
        );
        let mut empty = Region::new("module", [0, 64, 64], [1, 1, 1]);
        empty.put(0, 0, 0, &State::new("minecraft:air"));
        Litematic {
            gen,
            name: "sample".to_string(),
            author: "nbs-rs".to_string(),
            description: "two subregions, one double chest".to_string(),
            regions: vec![main, empty],
            now_ms: 1_700_000_000_000,
        }
    }

    fn write_doc(gen: Generation) -> Value {
        parse(&inflate(&write(&sample(gen)).unwrap())).unwrap()
    }

    fn region<'a>(root: &'a Value, name: &str) -> &'a Value {
        root.get("Regions").unwrap().get(name).unwrap()
    }

    /// 一格箱子里挂的潜影盒列表（两代都是 `Items`）。
    fn boxes(chest: &Value) -> Vec<Value> {
        chest.get("Items").unwrap().as_list().unwrap().1.to_vec()
    }

    /// 一只潜影盒里装的物品，按世代取各自的存放位置。
    fn inner(shulker: &Value, gen: Generation) -> Vec<Value> {
        let list = match gen {
            Generation::Modern => shulker
                .get("components")
                .unwrap()
                .get("minecraft:container")
                .unwrap(),
            Generation::Legacy => shulker
                .get("tag")
                .unwrap()
                .get("BlockEntityTag")
                .unwrap()
                .get("Items")
                .unwrap(),
        };
        list.as_list().unwrap().1.to_vec()
    }

    /// 盒内某件物品的数量：26.1 藏在 `item.count`，1.20.4 直接是 `Count`。
    fn inner_count(entry: &Value, gen: Generation) -> Option<i64> {
        match gen {
            Generation::Modern => entry.get("item")?.get("count")?.as_i32().map(i64::from),
            Generation::Legacy => entry.get("Count")?.as_i8().map(i64::from),
        }
    }

    fn inner_id(entry: &Value, gen: Generation) -> Option<&str> {
        match gen {
            Generation::Modern => entry.get("item")?.get("id")?.as_str(),
            Generation::Legacy => entry.get("id")?.as_str(),
        }
    }

    #[test]
    fn bit_width_keeps_the_upstream_floor_of_two() {
        // palette 只有 1..4 个状态时上游也按 2 bit 存，逐字节对照的前提。
        let mut r = Region::new("r", [0, 0, 0], [5, 1, 1]);
        assert_eq!((r.palette_len(), r.bits()), (1, 2));
        r.put(0, 0, 0, &State::new("minecraft:stone"));
        assert_eq!((r.palette_len(), r.bits()), (2, 2));
        r.put(1, 0, 0, &State::with("minecraft:chest", &[("facing", "north"), ("type", "left"), ("waterlogged", "false")]));
        r.put(2, 0, 0, &State::new("minecraft:air")); // 空气出厂就在表里，不该新增
        r.put(3, 0, 0, &State::new("minecraft:dirt"));
        assert_eq!((r.palette_len(), r.bits()), (4, 2));
        r.put(4, 0, 0, &State::new("minecraft:sandstone"));
        assert_eq!((r.palette_len(), r.bits()), (5, 3));
    }

    #[test]
    fn packed_indices_survive_a_long_boundary() {
        // 5 bit × 13 格 = 65 位，最后一格正好跨进第二个 long。
        let idx: Vec<u16> = (0..13).map(|i| ((i * 7) % 32) as u16).collect();
        let longs = pack_states(&idx, 5);
        assert_eq!(longs.len(), 2);
        assert_eq!(unpack(&longs, 5, idx.len()), idx);
    }

    #[test]
    fn packed_bitstream_matches_the_golden_samples() {
        // 期望值不是自家算法推出来的：一边是 node 跑上游转换器的 `packBlockStates`
        // （.work/ermc-run/upstream/src/litematic.js），一边是 python 的逐位拼串
        // （.work/ermc-run/golden_bits.py），两条独立路子里相同的样本才写进来。
        // 覆盖 2 / 3 / 8 bit 三档位宽，含跨 long 边界与多字长的序列。
        let cases: &[(&str, &[u16], usize, &[i64])] = &[
            ("air/note/chest 三态 4 格", &[0, 1, 2, 0], 3, &[36]),
            ("256 态 8 bit，最大下标夹在中间", &[1, 255, 1], 256, &[130817]),
            (
                "4 态循环 68 格，跨到第三个 long",
                &(0..68).map(|i| (i % 4) as u16).collect::<Vec<u16>>(),
                4,
                &[-1953184666628070172, -1953184666628070172, 228],
            ),
            (
                "5 态循环 66 格（3 bit）",
                &(0..65).map(|i| (i % 5) as u16).chain(std::iter::once(3u16)).collect::<Vec<u16>>(),
                5,
                &[
                    -8588063626344774008,
                    -8607204508894477208,
                    7532500250442024006,
                    28,
                ],
            ),
        ];
        for (label, cells, palette_len, want) in cases {
            assert_eq!(
                &pack_states(cells, bits_for(*palette_len)),
                want,
                "{label}：位流与上游/python 的样本不一致"
            );
        }
    }

    /// 造一个 palette 恰好 `n` 项的区域，只为借它的 `bits()`——位宽不该由测试再算一遍。
    fn bits_for(palette_len: usize) -> u32 {
        let mut r = Region::new("r", [0, 0, 0], [palette_len.max(1) as i32, 1, 1]);
        for i in 1..palette_len {
            r.put(i as i32, 0, 0, &State::new(format!("minecraft:s{i}")));
        }
        assert_eq!(r.palette_len(), palette_len.max(1));
        r.bits()
    }

    #[test]
    fn written_document_decodes_back_to_the_same_cells_and_chests() {
        let root = write_doc(Generation::Modern);
        assert_eq!(root.get("Version").and_then(Value::as_i32), Some(7));
        assert_eq!(root.get("SubVersion").and_then(Value::as_i32), Some(1));
        assert_eq!(root.get("MinecraftDataVersion").and_then(Value::as_i32), Some(4786));

        let names: Vec<&str> = root
            .get("Regions")
            .unwrap()
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(names, ["module", "module_2"], "重名子区域要补后缀，不能盖掉前一个");

        let meta = root.get("Metadata").unwrap();
        assert_eq!(meta.get("RegionCount").and_then(Value::as_i32), Some(2));
        assert_eq!(meta.get("TotalVolume").and_then(Value::as_i32), Some(5));
        assert_eq!(meta.get("TotalBlocks").and_then(Value::as_i32), Some(2));
        let enclosing = meta.get("EnclosingSize").unwrap();
        assert_eq!(
            [
                enclosing.get("x").and_then(Value::as_i32),
                enclosing.get("y").and_then(Value::as_i32),
                enclosing.get("z").and_then(Value::as_i32),
            ],
            [Some(2), Some(1), Some(65)],
            "外包尺寸按 min..max 算，含只有空气的那个子区域"
        );
        assert_eq!(meta.get("TimeCreated").and_then(Value::as_i64), Some(1_700_000_000_000));

        let reg = region(&root, "module");
        let pos = reg.get("Position").unwrap();
        assert_eq!(pos.get("y").and_then(Value::as_i32), Some(64));
        assert_eq!(reg.get("Size").unwrap().get("x").and_then(Value::as_i32), Some(2));

        let palette = reg.get("BlockStatePalette").unwrap().as_list().unwrap();
        assert_eq!(palette.0, 10);
        let ids: Vec<&str> = palette.1.iter().filter_map(|s| s.get("Name").and_then(Value::as_str)).collect();
        assert_eq!(ids, ["minecraft:air", "minecraft:note_block", "minecraft:chest"]);
        let chest_state = &palette.1[2];
        let props: Vec<&str> = chest_state
            .get("Properties")
            .unwrap()
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(props, ["facing", "type", "waterlogged"]);

        let cells = reg.get("BlockStates").unwrap().as_list().unwrap();
        assert_eq!(cells.0, 4, "BlockStates 必须是 long 数组（元素类型 4），不能写成 int");
        let longs: Vec<i64> = cells.1.iter().map(|v| v.as_i64().unwrap()).collect();
        // 线性下标 y -> z -> x：(1,0,0) 落在 1，箱子 (0,0,1) 落在 2。
        assert_eq!(unpack(&longs, 2, 4), vec![0u16, 1, 2, 0]);

        let tiles = reg.get("TileEntities").unwrap().as_list().unwrap();
        assert_eq!((tiles.0, tiles.1.len()), (10, 1));
        let chest = &tiles.1[0];
        assert_eq!(chest.get("id").and_then(Value::as_str), Some("minecraft:chest"));
        assert_eq!(
            [
                chest.get("x").and_then(Value::as_i32),
                chest.get("y").and_then(Value::as_i32),
                chest.get("z").and_then(Value::as_i32),
            ],
            [Some(0), Some(0), Some(1)],
            "TileEntity 的坐标是子区域局部坐标，不是世界坐标"
        );

        let bs = boxes(chest);
        assert_eq!(bs.len(), 2, "一只双箱挂两只潜影盒");
        assert_eq!(bs[0].get("id").and_then(Value::as_str), Some("minecraft:shulker_box"));
        assert_eq!(bs[0].get("count").and_then(Value::as_i32), Some(1));
        assert_eq!(bs[0].get("Count"), None, "26.1 用小写 count(int)，不该混进大写 Count(byte)");
        assert_eq!(bs[0].get("Slot").and_then(Value::as_i8), Some(0));
        assert_eq!(bs[1].get("Slot").and_then(Value::as_i8), Some(1));

        let inners = inner(&bs[1], Generation::Modern);
        assert_eq!(inner_id(&inners[0], Generation::Modern), Some("minecraft:music_disc_stal"));
        assert_eq!(inner_count(&inners[1], Generation::Modern), Some(62));
        assert_eq!(inners[0].get("slot").and_then(Value::as_i32), Some(0));
        assert_eq!(inners[1].get("slot").and_then(Value::as_i32), Some(1));

        for key in ["Entities", "PendingBlockTicks", "PendingFluidTicks"] {
            let list = reg.get(key).unwrap().as_list().unwrap();
            assert!(list.1.is_empty(), "{key} 应为空表");
        }

        // 只有空气的子区域也得写得出，且 palette 里就只有 air。
        let empty = region(&root, "module_2");
        assert_eq!(empty.get("BlockStatePalette").unwrap().as_list().unwrap().1.len(), 1);
        assert_eq!(empty.get("TileEntities").unwrap().as_list().unwrap().1.len(), 0);
    }

    #[test]
    fn legacy_generation_uses_the_old_item_shape() {
        let root = write_doc(Generation::Legacy);
        assert_eq!(root.get("Version").and_then(Value::as_i32), Some(6));
        assert_eq!(root.get("MinecraftDataVersion").and_then(Value::as_i32), Some(3700));
        let chest = &region(&root, "module").get("TileEntities").unwrap().as_list().unwrap().1[0];
        let bs = boxes(chest);
        assert_eq!(bs[0].get("count"), None);
        assert_eq!(bs[0].get("Count").and_then(Value::as_i8), Some(1));
        assert!(bs[0].get("components").is_none(), "1.20.4 的读取方不认组件写法");
        let inners = inner(&bs[1], Generation::Legacy);
        assert_eq!(inner_id(&inners[0], Generation::Legacy), Some("minecraft:music_disc_stal"));
        assert_eq!(inners[0].get("Slot").and_then(Value::as_i8), Some(0));
        assert_eq!(inner_count(&inners[1], Generation::Legacy), Some(62));
    }

    #[test]
    fn only_the_item_shape_differs_between_generations() {
        // 两档之间只该差物品 NBT 的形状，几何部分逐字节同形，否则对照实验不成立。
        let m = write_doc(Generation::Modern);
        let l = write_doc(Generation::Legacy);
        for key in ["Position", "Size", "BlockStatePalette", "BlockStates", "Entities", "PendingBlockTicks", "PendingFluidTicks"] {
            assert_eq!(
                region(&m, "module").get(key),
                region(&l, "module").get(key),
                "{key} 两代写法该一样"
            );
        }
        assert_ne!(
            region(&m, "module").get("TileEntities"),
            region(&l, "module").get("TileEntities")
        );
    }

    #[test]
    fn an_empty_document_is_refused() {
        let doc = Litematic {
            gen: Generation::Modern,
            name: "x".to_string(),
            author: String::new(),
            description: String::new(),
            regions: Vec::new(),
            now_ms: 0,
        };
        let err = write(&doc).unwrap_err();
        assert!(!err.is_empty(), "没有子区域时要有一句能看的报错");
    }

    #[test]
    fn both_generation_labels_and_hints_are_filled_in() {
        for gen in Generation::ALL {
            assert!(!gen.label().is_empty());
            assert!(!gen.hint().is_empty());
        }
        assert_eq!(Generation::default(), Generation::Modern);
    }
}
