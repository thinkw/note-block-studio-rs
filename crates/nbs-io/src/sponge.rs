//! 通用 Sponge v3（WorldEdit / Litematica 的 `.schem`）写出器。
//!
//! 与 [`crate::maestro`] 的区别：那边是 MAESTRO 机体固定区域 + 手写死 palette，
//! 这里只留「方块网格 → NBT 字节」这一件与机器布局无关的事，palette 由
//! [`SpongeSchematic::set`] 的调用点逐次登记，尺寸随歌曲变。
//!
//! 两条格式约定不是本模块自定的，各自有出处：
//! - 下标顺序 `x + W * (z + D * y)` 沿用 [`crate::maestro::access_index`]，
//!   那份写法已在真机被投影读进过。
//! - `Data` 是 LEB128 varint 流，一格一个变长整数；palette 超过 128 项时
//!   单元格会占 2 字节以上，所以 `Data.len() != 体积`，测试按 varint 语义解回来数。

use std::collections::HashMap;
use std::path::Path;

use crate::schematic::{gzip, Nbt};

/// 与 [`crate::maestro`] 同一代：Java 1.21.4。
pub const DATA_VERSION: i32 = 4189;

/// Java 1.21.8：20Hz 直轨布局用，认识铜块小号一族的 `instrument` 状态。
pub const DATA_VERSION_1_21_8: i32 = 4440;

/// 用户提供的 26.2 样板版本；20Hz 三槽车道布局跟随这个 DataVersion。
pub const DATA_VERSION_4903: i32 = 4903;

/// Sponge v3 的三个尺寸标签是 `TAG_Short`，所以单轴上限就是这个数。
pub const MAX_DIM: i32 = 32767;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

/// 方块网格 + 动态 palette。空气由 `minecraft:air` 表示，不是「没写过」。
#[derive(Clone, Debug)]
pub struct SpongeSchematic {
    width: i32,
    height: i32,
    length: i32,
    cells: Vec<i32>,
    palette: Vec<String>,
    index: HashMap<String, i32>,
    data_version: i32,
}

impl SpongeSchematic {
    /// 建一份全空气的网格。`width` 是 X、`length` 是 Z、`height` 是 Y。
    pub fn new(width: i32, length: i32, height: i32) -> Result<Self, String> {
        for (name, v) in [("width", width), ("length", length), ("height", height)] {
            if v <= 0 {
                return Err(crate::tr(
                    &format!("Internal error: {name} is {v}, a schematic axis must be positive."),
                    &format!("内部错误：{name} = {v}，schem 的每一条轴都得是正数。"),
                )
                .to_string());
            }
            if v > MAX_DIM {
                return Err(crate::tr(
                    &format!("Internal error: {name} is {v}, above the {MAX_DIM} schematic limit."),
                    &format!("内部错误：{name} = {v}，超过 schem 单轴上限 {MAX_DIM}。"),
                )
                .to_string());
            }
        }
        let cells = (width as i64 * length as i64 * height as i64) as usize;
        let mut out = Self {
            width,
            length,
            height,
            cells: vec![0; cells],
            palette: Vec::new(),
            index: HashMap::new(),
            data_version: DATA_VERSION,
        };
        // palette 0 号必须是空气：整份网格初始就全指向它。
        assert_eq!(out.state("minecraft:air"), 0);
        Ok(out)
    }

    /// 登记一种方块状态并给出 palette 编号；重复登记回同一个编号。
    ///
    /// 状态串按 Sponge 写法 `minecraft:name[key=value,...]`，属性顺序由调用点定；
    /// 本模块不解析也不重排，因为 palette 的键必须与消费方期望的字符串逐字一致。
    pub fn state(&mut self, block_state: &str) -> i32 {
        if let Some(&id) = self.index.get(block_state) {
            return id;
        }
        let id = self.palette.len() as i32;
        self.palette.push(block_state.to_string());
        self.index.insert(block_state.to_string(), id);
        id
    }

    fn cell(&self, p: Pos) -> Option<usize> {
        if p.x < 0 || p.x >= self.width || p.y < 0 || p.y >= self.height || p.z < 0
            || p.z >= self.length
        {
            return None;
        }
        Some(
            (p.x as i64
                + self.width as i64 * p.z as i64
                + p.y as i64 * self.width as i64 * self.length as i64) as usize,
        )
    }

    /// 放置一格；越界返回带坐标的错误，而不是像 [`crate::schematic`] 的网格那样丢弃。
    ///
    /// 这里必须报错而不能丢弃：新式机器的尺寸由布局自己算，静默丢格等于算小了尺寸，
    /// 粘进游戏会缺一块却没有任何提示。
    pub fn set(&mut self, p: Pos, block_state: &str) -> Result<(), String> {
        let id = self.state(block_state);
        let Some(i) = self.cell(p) else {
            return Err(crate::tr(
                &format!(
                    "Internal error: ({}, {}, {}) falls outside the {} × {} × {} schematic.",
                    p.x, p.y, p.z, self.width, self.height, self.length
                ),
                &format!(
                    "内部错误：({}, {}, {}) 落在 {} × {} × {} 区域之外。",
                    p.x, p.y, p.z, self.width, self.height, self.length
                ),
            )
            .to_string());
        };
        self.cells[i] = id;
        Ok(())
    }

    pub fn at(&self, p: Pos) -> Option<i32> {
        self.cell(p).map(|i| self.cells[i])
    }

    /// 改成目标游戏版本的 DataVersion；默认仍是 Java 1.21.4。
    pub fn with_data_version(mut self, data_version: i32) -> Self {
        self.data_version = data_version;
        self
    }

    pub fn state_at(&self, p: Pos) -> Option<&str> {
        let id = self.at(p)? as usize;
        self.palette.get(id).map(String::as_str)
    }

    pub fn size(&self) -> (i32, i32, i32) {
        (self.width, self.height, self.length)
    }

    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// 非空气方块总数，给导出报告的「变更清单」用。
    pub fn placed(&self) -> usize {
        self.cells.iter().filter(|id| **id != 0).count()
    }

    /// 东西轴镜像：位置左右翻转，`east/west` 连接与 `facing=east/west` 一起翻转。
    pub fn mirrored_x(self) -> Self {
        let mut out = Self::new(self.width, self.length, self.height)
            .expect("镜像不会改变已经合法的尺寸")
            .with_data_version(self.data_version);
        for y in 0..self.height {
            for z in 0..self.length {
                for x in 0..self.width {
                    let id = self.cells[self.cell(Pos::new(x, y, z)).unwrap()] as usize;
                    if id == 0 {
                        continue;
                    }
                    let state = mirror_x_state(&self.palette[id]);
                    out.set(Pos::new(self.width - 1 - x, y, z), &state)
                        .expect("镜像坐标仍在原网格内");
                }
            }
        }
        out
    }

    /// 未压缩的 `.schem` NBT 字节。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut n = Nbt::default();
        n.compound("");
        n.compound("Schematic");
        n.int("Version", 3);
        n.int("DataVersion", self.data_version);
        n.short("Width", self.width as i16);
        n.short("Height", self.height as i16);
        n.short("Length", self.length as i16);
        n.int_array("Offset", &[0, 0, 0]);
        n.compound("Blocks");
        n.compound("Palette");
        for (i, name) in self.palette.iter().enumerate() {
            n.int(name, i as i32);
        }
        n.end();
        let mut data = Vec::with_capacity(self.cells.len());
        for &id in &self.cells {
            put_varint(&mut data, id);
        }
        n.byte_array("Data", &data);
        // 本模块的机器不需要方块实体，但空列表照写：与已验证能读进的 MAESTRO 产物一致。
        n.list("BlockEntities", 0, 10);
        n.end();
        n.end();
        n.end();
        n.buf
    }

    /// gzip 后写盘。`.schem` 就是 gzip 过的 NBT，不压的话创世神不认。
    pub fn write_to(&self, path: &Path) -> Result<(), String> {
        let packed = gzip(&self.to_bytes())?;
        std::fs::write(path, packed).map_err(|e| {
            crate::tr(
                &format!("Failed to write schematic: {e}"),
                &format!("写入 schematic 失败: {e}"),
            )
            .to_string()
        })
    }
}

fn mirror_x_state(state: &str) -> String {
    let Some((name, rest)) = state.split_once('[') else {
        return state.to_string();
    };
    let inner = rest.strip_suffix(']').unwrap_or(rest);
    let mut props: Vec<(String, String)> = inner
        .split(',')
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect();
    if let (Some(east), Some(west)) = (
        props.iter().position(|(key, _)| key == "east"),
        props.iter().position(|(key, _)| key == "west"),
    ) {
        let east_value = props[east].1.clone();
        props[east].1 = props[west].1.clone();
        props[west].1 = east_value;
    }
    for (key, value) in &mut props {
        if key == "facing" {
            if value == "east" {
                *value = "west".to_string();
            } else if value == "west" {
                *value = "east".to_string();
            }
        }
    }
    let mut out = format!("{name}[");
    for (i, (key, value)) in props.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(value);
    }
    out.push(']');
    out
}

/// 无符号 LEB128：每字节低 7 位存数据，最高位为续位。
fn put_varint(buf: &mut Vec<u8>, value: i32) {
    let mut value = value as u32;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            return;
        }
    }
}

/// 测试侧的 varint 解码，等价于投影读 sponge 方块数据时的那次转换
/// （`convertVarIntByteArrayToPackedLongArray`：一格一个 varint，读到 volume 个为止）。
#[cfg(test)]
fn get_varints(buf: &[u8], count: usize) -> Result<Vec<i32>, String> {
    let mut out = Vec::with_capacity(count);
    let mut p = 0usize;
    while out.len() < count {
        let mut result = 0u32;
        let mut shift = 0u32;
        loop {
            let Some(&byte) = buf.get(p) else {
                return Err(format!("varint 在第 {} 格处越界", out.len()));
            };
            p += 1;
            result |= ((byte & 0x7f) as u32) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }
        out.push(result as i32);
    }
    if p != buf.len() {
        return Err(format!("varint 流尾部还剩 {} 字节没读完", buf.len() - p));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{parse, Value};

    fn grid(w: i32, l: i32, h: i32) -> SpongeSchematic {
        SpongeSchematic::new(w, l, h).unwrap()
    }

    /// 读出 `Data` 并按 varint 解回每一格。
    fn decode(bytes: &[u8], volume: usize) -> Vec<i32> {
        let root = parse(bytes).unwrap();
        let sch = root.get("Schematic").expect("根里该有 Schematic");
        let data = sch
            .get("Blocks")
            .and_then(|b| b.get("Data"))
            .and_then(Value::as_bytes)
            .expect("Blocks/Data 该是 TAG_Byte_Array");
        get_varints(data, volume).expect("varint 流该刚好解完")
    }

    #[test]
    fn cells_follow_the_verified_index_order() {
        // 3 宽 × 4 深 × 2 高：下标 = x + W * (z + D * y)。
        let mut s = grid(3, 4, 2);
        let p = Pos::new(1, 1, 2);
        s.set(p, "minecraft:stone").unwrap();
        assert_eq!(s.at(p), Some(1));
        let ids = decode(&s.to_bytes(), 3 * 4 * 2);
        let want = 1 + 3 * (2 + 4 * 1);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(*id, if i == want { 1 } else { 0 }, "第 {i} 格");
        }
    }

    #[test]
    fn horizontal_mirror_flips_positions_connections_and_east_west_facing() {
        let mut s = grid(3, 2, 1).with_data_version(DATA_VERSION_4903);
        s.set(Pos::new(0, 0, 0), "minecraft:stone").unwrap();
        s.set(
            Pos::new(1, 0, 0),
            "minecraft:redstone_wire[east=side,north=none,south=none,west=none,power=0]",
        )
        .unwrap();
        s.set(Pos::new(2, 0, 0), "minecraft:repeater[facing=east]")
            .unwrap();
        let s = s.mirrored_x();
        assert_eq!(s.state_at(Pos::new(2, 0, 0)), Some("minecraft:stone"));
        let wire = s.state_at(Pos::new(1, 0, 0)).unwrap();
        assert!(wire.contains("east=none") && wire.contains("west=side"), "{wire}");
        let repeater = s.state_at(Pos::new(0, 0, 0)).unwrap();
        assert!(repeater.contains("facing=west"), "{repeater}");
        let root = parse(&s.to_bytes()).unwrap();
        let sch = root.get("Schematic").unwrap();
        assert_eq!(sch.get("DataVersion").and_then(Value::as_i32), Some(DATA_VERSION_4903));
    }

    #[test]
    fn air_holds_palette_zero_and_repeated_states_reuse_one_id() {
        let mut s = grid(2, 2, 2);
        assert_eq!(s.state("minecraft:air"), 0);
        let a = s.state("minecraft:redstone_wire[power=0]");
        let b = s.state("minecraft:redstone_wire[power=0]");
        assert_eq!(a, b, "同一种状态不该占两个 palette 编号");
        assert_eq!(s.palette_len(), 2);
        assert_eq!(s.placed(), 0, "还没落子时全网格都指向空气");
        s.set(Pos::new(0, 0, 0), "minecraft:redstone_wire[power=0]")
            .unwrap();
        assert_eq!(s.placed(), 1);
    }

    #[test]
    fn palette_above_128_writes_multibyte_varints_that_still_round_trip() {
        // 新式机器的音符盒状态是「乐器 × 音高」，轻松超过 128；此时每格占 2 字节，
        // Data 长度不再等于体积，只能按 varint 语义数格子。
        let mut s = grid(3, 3, 3);
        for n in 0..25 {
            for i in 0..20 {
                s.state(&format!("minecraft:note_block[instrument={i},note={n}]"));
            }
        }
        assert!(s.palette_len() > 128);
        let hot = s.state("minecraft:note_block[instrument=19,note=24]");
        assert!(hot >= 128, "这条状态该落到双字节 varint 区间");
        s.set(Pos::new(2, 2, 2), "minecraft:note_block[instrument=19,note=24]")
            .unwrap();
        let bytes = s.to_bytes();
        let ids = decode(&bytes, 27);
        assert_eq!(ids[26], hot);
        let root = parse(&bytes).unwrap();
        let data = root
            .get("Schematic")
            .unwrap()
            .get("Blocks")
            .unwrap()
            .get("Data")
            .unwrap()
            .as_bytes()
            .unwrap();
        assert!(data.len() > 27, "有双字节编号时 Data 会比格数长");
    }

    #[test]
    fn palette_numbers_are_dense_and_in_range() {
        let mut s = grid(2, 2, 2);
        for k in 0..5 {
            s.set(Pos::new(0, 0, 0), &format!("minecraft:wool[color={k}]"))
                .unwrap();
        }
        let root = parse(&s.to_bytes()).unwrap();
        let pal = root
            .get("Schematic")
            .unwrap()
            .get("Blocks")
            .unwrap()
            .get("Palette")
            .unwrap()
            .fields()
            .unwrap();
        assert_eq!(pal.len(), 6, "空气 + 5 种羊毛");
        for (i, (name, value)) in pal.iter().enumerate() {
            assert_eq!(value.as_i32(), Some(i as i32), "{name}");
        }
    }

    #[test]
    fn bytes_satisfy_the_litematica_sponge_v3_requirements() {
        let mut s = grid(4, 5, 6);
        s.set(Pos::new(3, 5, 4), "minecraft:piston[facing=north]").unwrap();
        let root = parse(&s.to_bytes()).unwrap();
        let sch = root.get("Schematic").expect("根复合标签里该有 Schematic");
        assert!(sch.get("Version").and_then(Value::as_i32).unwrap_or_default() >= 3);
        assert!(sch.get("DataVersion").is_some());
        let size: Vec<i32> = ["Width", "Height", "Length"]
            .iter()
            .map(|k| sch.get(k).and_then(Value::as_i32).unwrap_or_default())
            .collect();
        assert_eq!(size, vec![4, 6, 5], "Width/Height/Length 的对应轴别写反");
        assert!(size.iter().all(|v| *v > 0));
        let blocks = sch.get("Blocks").expect("该有 Blocks 复合标签");
        assert!(blocks.get("Palette").and_then(Value::fields).is_some());
        assert!(blocks.get("Data").and_then(Value::as_bytes).is_some());
        let (ty, bes) = blocks.get("BlockEntities").and_then(Value::as_list).expect("BlockEntities 该是列表");
        assert_eq!(ty, 10);
        assert!(bes.is_empty(), "本模块的机器不写方块实体");
        let (ty, off) = sch.get("Offset").and_then(Value::as_list).expect("Offset");
        assert_eq!(ty, 3, "Offset 得是 TAG_Int_Array");
        assert_eq!(off.len(), 3);
    }

    #[test]
    fn degenerate_and_oversized_axes_are_refused() {
        // 断言只挑两种文案里都出现的轴名，不挑句子——文案跟着语言开关走。
        let cases = [
            (0, 4, 2, "width"),
            (4, 0, 2, "length"),
            (4, 4, 0, "height"),
            (MAX_DIM + 1, 4, 2, "width"),
        ];
        for (w, l, h, name) in cases {
            let e = SpongeSchematic::new(w, l, h).unwrap_err();
            assert!(e.contains(name), "错误里该点名是哪条轴：{e}");
        }
        assert!(SpongeSchematic::new(MAX_DIM, 1, 1).is_ok());
    }

    #[test]
    fn out_of_range_placement_reports_the_cell() {
        let mut s = grid(3, 3, 3);
        for p in [
            Pos::new(3, 0, 0),
            Pos::new(0, 3, 0),
            Pos::new(0, 0, 3),
            Pos::new(-1, 0, 0),
        ] {
            let e = s.set(p, "minecraft:stone").unwrap_err();
            assert!(
                e.contains(&format!("({}, {}, {})", p.x, p.y, p.z)),
                "错误里该点名越界坐标：{e}"
            );
            assert_eq!(s.at(p), None);
        }
    }
}
