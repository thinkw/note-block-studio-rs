//! 自家 20Hz 单程直轨：沿用用户样板的 3 格音槽、5 中继器、15 槽车道。
//!
//! 两条 10Hz 臂共用一对输入总线；车道数按最大复音自动增长，道间留 1 格并放中继器。
//! 内部按左向右构建，输出前整体镜像 x 轴；朝南看时开关在左、车道向右，补偿关系不变。

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use nbs_core::model::Song;

use crate::machine::{by_game_tick, Event, EventPlane};
use crate::schematic::INS_NAMES;
use crate::sponge::{SpongeSchematic, DATA_VERSION_4903};

const VOICES_PER_LANE: usize = 15;
const LANE_WIDTH: i32 = 15;
const LANE_GAP: i32 = 1;
const LANE_PITCH: i32 = LANE_WIDTH + LANE_GAP;
const ROW_EVEN_Y: i32 = 2;
const ROW_ODD_Y: i32 = 6;
const INPUT_Z: i32 = 5;
const FIRST_REPEATER_Z: i32 = 6;
const FIRST_NOTE_Z: i32 = 7;
const DUST_BUDGET: usize = 15;
const CENTERS: [i32; 5] = [1, 4, 7, 10, 13];

const STONE: &str = "minecraft:smooth_stone";
const NOTE_SUPPORT: &str = "minecraft:stone";
const SWITCH_BUTTON: &str = "minecraft:oak_button[face=wall,facing=west,powered=false]";
const SWITCH_PISTON: &str = "minecraft:sticky_piston[facing=south,extended=false]";
const SWITCH_REDSTONE_BLOCK: &str = "minecraft:redstone_block";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HalfTickOptions {
    pub include_locked: bool,
}

impl Default for HalfTickOptions {
    fn default() -> Self {
        Self { include_locked: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HalfTickReport {
    pub size: [i32; 3],
    pub placed: usize,
    pub palette_len: usize,
    pub notes: usize,
    pub lanes: usize,
    pub voices: usize,
    pub compensation_cells: usize,
    pub lead_in_gt: i64,
    pub lane_width: i32,
    pub lane_gap: i32,
    pub lane_pitch: i32,
    pub input_y: [i32; 2],
    pub input_z: i32,
    pub input_x: i32,
    pub bus_repeaters: usize,
    pub switch_buttons: usize,
    pub switch_pistons: usize,
    pub switch_redstone_blocks: usize,
    pub switch_repeaters: usize,
    pub wire_cells: usize,
    pub longest_wire: usize,
    pub repeaters: usize,
    pub notes_with_trigger: usize,
    pub notes_missing_trigger: usize,
    pub notes_with_instrument: usize,
    pub notes_missing_instrument: usize,
    pub notes_with_support: usize,
    pub notes_missing_support: usize,
    pub dropped_out_of_range: usize,
    pub dropped_locked: usize,
    pub mapped_custom: usize,
    pub pitched_ignored: usize,
}

impl HalfTickReport {
    pub fn lines(&self) -> Vec<String> {
        let mut zh = vec![
            format!("20Hz 三槽直轨：{}×{}×{}，{} 个非空气方块，{} 种状态。", self.size[0], self.size[1], self.size[2], self.placed, self.palette_len),
            format!("自动 {} 条道，每道 {} 槽，道间距 {}；最大复音 {}。", self.lanes, VOICES_PER_LANE, self.lane_pitch, self.voices),
            format!("共用输入 x={}、y={}/{}、z={}，朝南视角从左向右（世界 -x）；段间中继器 {}，等长轨道补偿 {} 格（{} game tick）。", self.input_x, self.input_y[0], self.input_y[1], self.input_z, self.bus_repeaters, self.compensation_cells, self.lead_in_gt),
            format!("开关：按钮 {}、粘性活塞 {}、红石块 {}、开关中继器 {}。", self.switch_buttons, self.switch_pistons, self.switch_redstone_blocks, self.switch_repeaters),
            format!("放下音符盒 {} 颗；中继器 {}。", self.notes, self.repeaters),
            format!("红石粉 {} 格，最长连续段 {} 格（预算 {}）。", self.wire_cells, self.longest_wire, DUST_BUDGET),
            format!("触发链自检：{} / {}，缺 {}；乐器块 {} / {}，缺 {}；石头支撑 {} / {}，缺 {}。", self.notes_with_trigger, self.notes, self.notes_missing_trigger, self.notes_with_instrument, self.notes, self.notes_missing_instrument, self.notes_with_support, self.notes, self.notes_missing_support),
            "第一版三槽直轨：开关已按用户样板加入，真机时序仍需验收。".into(),
        ];
        let mut en = vec![
            format!("20Hz three-slot straight layout: {}x{}x{}, {} non-air blocks, {} states.", self.size[0], self.size[1], self.size[2], self.placed, self.palette_len),
            format!("{} automatic lanes, {} slots per lane, lane pitch {}; maximum polyphony {}.", self.lanes, VOICES_PER_LANE, self.lane_pitch, self.voices),
            format!("Shared input at x={}, y={}/{}, z={}, left to right when facing south (world -x); {} bus repeaters; equal-length track compensation {} cells ({} game ticks).", self.input_x, self.input_y[0], self.input_y[1], self.input_z, self.bus_repeaters, self.compensation_cells, self.lead_in_gt),
            format!("Switch: {} buttons, {} sticky pistons, {} redstone blocks, {} switch repeaters.", self.switch_buttons, self.switch_pistons, self.switch_redstone_blocks, self.switch_repeaters),
            format!("{} note blocks; {} repeaters.", self.notes, self.repeaters),
            format!("{} redstone dust cells; longest continuous run {} (budget {}).", self.wire_cells, self.longest_wire, DUST_BUDGET),
            format!("Trigger-chain check: {} / {}, missing {}; instrument blocks {} / {}, missing {}; stone supports {} / {}, missing {}.", self.notes_with_trigger, self.notes, self.notes_missing_trigger, self.notes_with_instrument, self.notes, self.notes_missing_instrument, self.notes_with_support, self.notes, self.notes_missing_support),
            "First three-slot straight version: the user's switch is included; in-game timing still needs a real test.".into(),
        ];
        if self.longest_wire > DUST_BUDGET {
            zh.push(format!("警告：最长红石粉段 {} 格，超过 {} 格预算。", self.longest_wire, DUST_BUDGET));
            en.push(format!("Warning: longest dust run is {}, above the {}-cell budget.", self.longest_wire, DUST_BUDGET));
        }
        if self.notes_missing_trigger + self.notes_missing_instrument + self.notes_missing_support > 0 {
            zh.push(format!("警告：触发链缺 {}、乐器块缺 {}、支撑缺 {}。", self.notes_missing_trigger, self.notes_missing_instrument, self.notes_missing_support));
            en.push(format!("Warning: missing trigger paths {}, instrument blocks {}, supports {}.", self.notes_missing_trigger, self.notes_missing_instrument, self.notes_missing_support));
        }
        if self.dropped_out_of_range + self.dropped_locked + self.mapped_custom + self.pitched_ignored > 0 {
            zh.push(format!("输入取舍：音域越界丢弃 {}、锁定层丢弃 {}、自定义乐器按 harp {}、微调取整 {}。", self.dropped_out_of_range, self.dropped_locked, self.mapped_custom, self.pitched_ignored));
            en.push(format!("Input changes: {} out-of-range dropped, {} locked-layer dropped, {} custom instruments mapped to harp, {} pitch offsets rounded.", self.dropped_out_of_range, self.dropped_locked, self.mapped_custom, self.pitched_ignored));
        }
        if crate::lang() == 1 { zh } else { en }
    }
}

pub fn build(song: &Song, opts: &HalfTickOptions) -> Result<(HalfTickReport, SpongeSchematic), String> {
    let plane = EventPlane::build(song, opts.include_locked);
    let ticks = by_game_tick(&plane.events);
    let voices = ticks.iter().map(|(_, notes)| notes.len()).max().unwrap_or(0);
    let lanes = voices.div_ceil(VOICES_PER_LANE).max(1);
    let compensation_cells = lanes - 1;
    let total_cells = (plane.last_gt as usize / 2 + 1).max(1);
    let cells = total_cells + compensation_cells;
    let width = lanes as i32 * LANE_PITCH - LANE_GAP;
    let height = ROW_ODD_Y + 1;
    let last_z = FIRST_NOTE_Z + 2 * (cells as i32 - 1);
    let length = last_z + 1;
    let mut schem = SpongeSchematic::new(width, length, height)?
        .with_data_version(DATA_VERSION_4903);

    build_input_buses(&mut schem, lanes)?;
    build_switch(&mut schem)?;
    for lane in 0..lanes {
        build_lane_backbone(&mut schem, lane, cells)?;
    }

    for (gt, notes) in &ticks {
        let orig_cell = (*gt / 2) as usize;
        let odd = *gt % 2 == 1;
        for (voice, note) in notes.iter().enumerate() {
            let lane = voice / VOICES_PER_LANE;
            let slot = voice % VOICES_PER_LANE;
            let delay_cells = lanes - 1 - lane;
            let c = orig_cell + delay_cells;
            let x = lane as i32 * LANE_PITCH + slot as i32;
            let y = if odd { ROW_ODD_Y } else { ROW_EVEN_Y };
            let z = FIRST_NOTE_Z + 2 * c as i32;
            set(&mut schem, Pos::new(x, y, z), &note_state(note))?;
            set(&mut schem, Pos::new(x, y - 1, z), instrument_block(note.instrument))?;
            set(&mut schem, Pos::new(x, y - 2, z), NOTE_SUPPORT)?;
        }
    }

    support_wires(&mut schem)?;
    let stats = census(&schem);
    let mut schem = schem.mirrored_x();
    set(&mut schem, Pos::new(width - 3, ROW_EVEN_Y, 1), SWITCH_BUTTON)?;
    let report = HalfTickReport {
        size: [width, height, length],
        placed: schem.placed(),
        palette_len: schem.palette_len(),
        notes: plane.events.len(),
        lanes,
        voices,
        compensation_cells,
        lead_in_gt: (compensation_cells * 2) as i64,
        lane_width: LANE_WIDTH,
        lane_gap: LANE_GAP,
        lane_pitch: LANE_PITCH,
        input_y: [ROW_EVEN_Y, ROW_ODD_Y],
        input_z: INPUT_Z,
        input_x: width - 1,
        bus_repeaters: stats.bus_repeaters,
        switch_buttons: stats.switch_buttons,
        switch_pistons: stats.switch_pistons,
        switch_redstone_blocks: stats.switch_redstone_blocks,
        switch_repeaters: stats.switch_repeaters,
        wire_cells: stats.wire_cells,
        longest_wire: stats.longest_wire,
        repeaters: stats.repeaters,
        notes_with_trigger: stats.notes_with_trigger,
        notes_missing_trigger: stats.notes_missing_trigger,
        notes_with_instrument: stats.notes_with_instrument,
        notes_missing_instrument: stats.notes_missing_instrument,
        notes_with_support: stats.notes_with_support,
        notes_missing_support: stats.notes_missing_support,
        dropped_out_of_range: plane.dropped_out_of_range,
        dropped_locked: plane.dropped_locked,
        mapped_custom: plane.mapped_custom,
        pitched_ignored: plane.pitched_ignored,
    };
    Ok((report, schem))
}

pub fn preview(song: &Song, opts: &HalfTickOptions) -> Result<HalfTickReport, String> {
    build(song, opts).map(|(report, _)| report)
}

pub fn export(song: &Song, path: &Path, opts: &HalfTickOptions) -> Result<HalfTickReport, String> {
    let (report, schem) = build(song, opts)?;
    schem.write_to(path)?;
    Ok(report)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Pos {
    x: i32,
    y: i32,
    z: i32,
}

impl Pos {
    const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

fn set(s: &mut SpongeSchematic, p: Pos, state: &str) -> Result<(), String> {
    s.set(crate::sponge::Pos::new(p.x, p.y, p.z), state)
}

fn repeater(facing: &str) -> String {
    format!("minecraft:repeater[delay=1,facing={facing},locked=false,powered=false]")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Link {
    None,
    Side,
    Up,
}

impl Link {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Side => "side",
            Self::Up => "up",
        }
    }
}

fn wire_links(east: Link, north: Link, south: Link, west: Link) -> String {
    format!(
        "minecraft:redstone_wire[east={},north={},south={},west={},power=0]",
        east.as_str(),
        north.as_str(),
        south.as_str(),
        west.as_str()
    )
}

fn build_input_buses(s: &mut SpongeSchematic, lanes: usize) -> Result<(), String> {
    for y in [ROW_EVEN_Y, ROW_ODD_Y] {
        for lane in 0..lanes {
            let base = lane as i32 * LANE_PITCH;
            for x in base..base + LANE_WIDTH {
                set(s, Pos::new(x, y - 1, INPUT_Z), STONE)?;
                let west = if x > base || lane > 0 { Link::Side } else { Link::None };
                let east = if x + 1 < base + LANE_WIDTH || lane + 1 < lanes { Link::Side } else { Link::None };
                set(s, Pos::new(x, y, INPUT_Z), &wire_links(east, Link::None, Link::None, west))?;
            }
        }
        for gap in 0..lanes - 1 {
            let x = (gap + 1) as i32 * LANE_PITCH - LANE_GAP;
            set(s, Pos::new(x, y - 1, INPUT_Z), STONE)?;
            set(s, Pos::new(x, y, INPUT_Z), &repeater("east"))?;
        }
    }
    Ok(())
}

fn switch_pos(p: Pos) -> Pos {
    Pos::new(14 - p.x, p.y + 1, p.z)
}

fn put_switch(s: &mut SpongeSchematic, p: Pos, state: &str) -> Result<(), String> {
    set(s, switch_pos(p), state)
}

fn put_switch_wire(
    s: &mut SpongeSchematic,
    p: Pos,
    east: Link,
    north: Link,
    south: Link,
    west: Link,
) -> Result<(), String> {
    // 样板的 Region.Size.x 为负，转 Sponge 正坐标时只需交换东西连接。
    set(s, switch_pos(p), &wire_links(west, north, south, east))
}

fn build_switch(s: &mut SpongeSchematic) -> Result<(), String> {
    for z in 0..5 {
        put_switch(s, Pos::new(14, 0, z), STONE)?;
    }
    for x in 12..=14 {
        put_switch(s, Pos::new(x, 0, INPUT_Z), STONE)?;
        put_switch(s, Pos::new(x, 4, INPUT_Z), STONE)?;
    }
    put_switch(s, Pos::new(12, 1, 1), SWITCH_BUTTON)?;
    put_switch(s, Pos::new(13, 1, 1), STONE)?;
    put_switch_wire(s, Pos::new(14, 1, 1), Link::None, Link::None, Link::Side, Link::Up)?;
    put_switch_wire(s, Pos::new(14, 1, 2), Link::None, Link::Side, Link::Side, Link::None)?;
    put_switch_wire(s, Pos::new(14, 1, 3), Link::None, Link::Side, Link::Side, Link::None)?;
    put_switch(s, Pos::new(14, 1, 4), &repeater("north"))?;
    put_switch_wire(s, Pos::new(12, 1, 5), Link::Side, Link::None, Link::None, Link::Side)?;
    put_switch_wire(s, Pos::new(13, 1, 5), Link::Side, Link::None, Link::Side, Link::Side)?;
    put_switch_wire(s, Pos::new(14, 1, 5), Link::None, Link::Side, Link::None, Link::Side)?;

    put_switch(s, Pos::new(13, 2, 0), STONE)?;
    put_switch_wire(s, Pos::new(13, 2, 1), Link::Side, Link::Up, Link::None, Link::None)?;
    put_switch_wire(s, Pos::new(13, 3, 0), Link::Up, Link::None, Link::Side, Link::None)?;
    put_switch(s, Pos::new(14, 3, 0), STONE)?;
    put_switch_wire(s, Pos::new(14, 4, 0), Link::None, Link::None, Link::Up, Link::Side)?;
    for z in 1..5 {
        put_switch(s, Pos::new(14, 4, z), STONE)?;
    }

    put_switch_wire(s, Pos::new(14, 5, 1), Link::None, Link::Side, Link::Side, Link::None)?;
    put_switch(s, Pos::new(14, 5, 2), SWITCH_PISTON)?;
    put_switch(s, Pos::new(14, 5, 3), SWITCH_REDSTONE_BLOCK)?;
    put_switch_wire(s, Pos::new(12, 5, 5), Link::Side, Link::None, Link::None, Link::Side)?;
    put_switch_wire(s, Pos::new(13, 5, 5), Link::Side, Link::None, Link::Side, Link::Side)?;
    put_switch_wire(s, Pos::new(14, 5, 5), Link::Side, Link::None, Link::None, Link::Side)?;
    Ok(())
}

fn build_lane_backbone(s: &mut SpongeSchematic, lane: usize, cells: usize) -> Result<(), String> {
    let base = lane as i32 * LANE_PITCH;
    for c in 0..cells {
        let repeater_z = FIRST_REPEATER_Z + 2 * c as i32;
        let note_z = FIRST_NOTE_Z + 2 * c as i32;
        for y in [ROW_EVEN_Y, ROW_ODD_Y] {
            for center in CENTERS {
                let x = base + center;
                set(s, Pos::new(x, y - 1, repeater_z), STONE)?;
                set(s, Pos::new(x, y, repeater_z), &repeater("north"))?;
                set(s, Pos::new(x, y, note_z), STONE)?;
            }
        }
    }
    Ok(())
}

fn note_state(note: &Event) -> String {
    let instrument = INS_NAMES.get(note.instrument as usize).copied().unwrap_or("harp");
    format!("minecraft:note_block[note={},powered=false,instrument={instrument}]", note.key - 33)
}

fn instrument_block(instrument: u8) -> &'static str {
    match instrument {
        0 => "minecraft:dirt",
        1 => "minecraft:oak_planks",
        2 => "minecraft:stone",
        3 => "minecraft:sand",
        4 => "minecraft:glass",
        5 => "minecraft:white_wool",
        6 => "minecraft:clay",
        7 => "minecraft:gold_block",
        8 => "minecraft:packed_ice",
        9 => "minecraft:bone_block",
        10 => "minecraft:iron_block",
        11 => "minecraft:soul_sand",
        12 => "minecraft:pumpkin",
        13 => "minecraft:emerald_block",
        14 => "minecraft:hay_block",
        15 => "minecraft:glowstone",
        16..=19 => crate::schematic::COPPER_INS_BLOCKS[(instrument - 16) as usize],
        _ => "minecraft:dirt",
    }
}

fn instrument_block_name(instrument: &str) -> &'static str {
    match instrument {
        "harp" => "minecraft:dirt",
        "bass" => "minecraft:oak_planks",
        "basedrum" => "minecraft:stone",
        "snare" => "minecraft:sand",
        "hat" => "minecraft:glass",
        "guitar" => "minecraft:white_wool",
        "flute" => "minecraft:clay",
        "bell" => "minecraft:gold_block",
        "chime" => "minecraft:packed_ice",
        "xylophone" => "minecraft:bone_block",
        "iron_xylophone" => "minecraft:iron_block",
        "cow_bell" => "minecraft:soul_sand",
        "didgeridoo" => "minecraft:pumpkin",
        "bit" => "minecraft:emerald_block",
        "banjo" => "minecraft:hay_block",
        "pling" => "minecraft:glowstone",
        "trumpet" => crate::schematic::COPPER_INS_BLOCKS[0],
        "trumpet_exposed" => crate::schematic::COPPER_INS_BLOCKS[1],
        "trumpet_weathered" => crate::schematic::COPPER_INS_BLOCKS[2],
        "trumpet_oxidized" => crate::schematic::COPPER_INS_BLOCKS[3],
        _ => "minecraft:dirt",
    }
}

fn support_wires(s: &mut SpongeSchematic) -> Result<(), String> {
    let (w, h, l) = s.size();
    let mut wires = Vec::new();
    for y in 0..h {
        for z in 0..l {
            for x in 0..w {
                if s.state_at(crate::sponge::Pos::new(x, y, z))
                    .is_some_and(|v| v.starts_with("minecraft:redstone_wire"))
                {
                    wires.push((x, y, z));
                }
            }
        }
    }
    for (x, y, z) in wires {
        if y > 0 && s.state_at(crate::sponge::Pos::new(x, y - 1, z)) == Some("minecraft:air") {
            set(s, Pos::new(x, y - 1, z), STONE)?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct Census {
    bus_repeaters: usize,
    switch_buttons: usize,
    switch_pistons: usize,
    switch_redstone_blocks: usize,
    switch_repeaters: usize,
    wire_cells: usize,
    longest_wire: usize,
    repeaters: usize,
    notes_with_trigger: usize,
    notes_missing_trigger: usize,
    notes_with_instrument: usize,
    notes_missing_instrument: usize,
    notes_with_support: usize,
    notes_missing_support: usize,
}

fn prop_value<'a>(state: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let (_, rest) = state.split_once(&needle)?;
    Some(rest.split(',').next()?.trim_end_matches(']'))
}

fn link_from_state(state: &str, dir: &str) -> Link {
    match prop_value(state, dir).unwrap_or("none") {
        "side" => Link::Side,
        "up" => Link::Up,
        _ => Link::None,
    }
}

fn census(s: &SpongeSchematic) -> Census {
    let (w, h, l) = s.size();
    let mut out = Census::default();
    let mut wire_props = HashMap::new();
    let mut notes = Vec::new();
    for y in 0..h {
        for z in 0..l {
            for x in 0..w {
                let Some(state) = s.state_at(crate::sponge::Pos::new(x, y, z)) else { continue };
                let name = state.split('[').next().unwrap_or(state);
                match name {
                    "minecraft:redstone_wire" => {
                        out.wire_cells += 1;
                        wire_props.insert((x, y, z), [
                            link_from_state(state, "north"),
                            link_from_state(state, "south"),
                            link_from_state(state, "east"),
                            link_from_state(state, "west"),
                        ]);
                    }
                    "minecraft:repeater" => {
                        out.repeaters += 1;
                        if z == INPUT_Z && prop_value(state, "facing") == Some("east") {
                            out.bus_repeaters += 1;
                        }
                        if x == 0 && y == ROW_EVEN_Y && z == INPUT_Z - 1
                            && prop_value(state, "facing") == Some("north")
                        {
                            out.switch_repeaters += 1;
                        }
                    }
                    "minecraft:oak_button" => out.switch_buttons += 1,
                    "minecraft:sticky_piston" => out.switch_pistons += 1,
                    "minecraft:redstone_block" => out.switch_redstone_blocks += 1,
                    "minecraft:note_block" => notes.push((x, y, z, state)),
                    _ => {}
                }
            }
        }
    }

    let keys: Vec<(i32, i32, i32)> = wire_props.keys().copied().collect();
    let index: HashMap<(i32, i32, i32), usize> = keys.iter().copied().enumerate().map(|(i, p)| (p, i)).collect();
    let mut adj = vec![Vec::new(); keys.len()];
    for (i, &(x, y, z)) in keys.iter().enumerate() {
        for (dir, (dx, dz)) in [(0, -1), (0, 1), (1, 0), (-1, 0)].into_iter().enumerate() {
            let (nx, ny, nz) = match wire_props[&(x, y, z)][dir] {
                Link::Side => (x + dx, y, z + dz),
                Link::Up => (x + dx, y + 1, z + dz),
                Link::None => continue,
            };
            if let Some(&j) = index.get(&(nx, ny, nz)) {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
    }
    let mut seen = vec![false; keys.len()];
    for start in 0..keys.len() {
        if seen[start] { continue; }
        let mut q = VecDeque::from([start]);
        seen[start] = true;
        let mut n = 0;
        while let Some(i) = q.pop_front() {
            n += 1;
            for &j in &adj[i] {
                if !seen[j] {
                    seen[j] = true;
                    q.push_back(j);
                }
            }
        }
        out.longest_wire = out.longest_wire.max(n);
    }

    for (x, y, z, state) in notes {
        let lane = x / LANE_PITCH;
        let local = x - lane * LANE_PITCH;
        let group = local / 3;
        let center_x = lane * LANE_PITCH + 1 + group * 3;
        let repeater_state = s.state_at(crate::sponge::Pos::new(center_x, y, z - 1));
        let center_state = s.state_at(crate::sponge::Pos::new(center_x, y, z));
        let trigger = repeater_state.is_some_and(|v| {
            v.starts_with("minecraft:repeater[") && prop_value(v, "facing") == Some("north")
        }) && center_state.is_some_and(|v| {
            v.starts_with("minecraft:note_block[") || v == "minecraft:smooth_stone"
        });
        if trigger {
            out.notes_with_trigger += 1;
        } else {
            out.notes_missing_trigger += 1;
        }

        let instrument = prop_value(state, "instrument").unwrap_or("harp");
        let below = s.state_at(crate::sponge::Pos::new(x, y - 1, z));
        if below == Some(instrument_block_name(instrument)) {
            out.notes_with_instrument += 1;
        } else {
            out.notes_missing_instrument += 1;
        }
        if s.state_at(crate::sponge::Pos::new(x, y - 2, z)) == Some(NOTE_SUPPORT) {
            out.notes_with_support += 1;
        } else {
            out.notes_missing_support += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::{Layer, Note};

    fn song(tempo: f32, notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::new();
        s.meta.tempo = tempo;
        let max_layer = notes.iter().map(|n| n.1).max().unwrap_or(0) as usize;
        s.layers = vec![Layer::default(); max_layer + 1];
        for &(t, l, ins, key) in notes {
            s.set(t, l, Note::new(ins, key));
        }
        s.recalculate();
        s
    }

    fn chord(n: usize) -> Song {
        let notes: Vec<_> = (0..n).map(|i| (0, i as u32, (i % 16) as u8, 33 + (i % 25) as u8)).collect();
        song(20.0, &notes)
    }

    fn note_positions(s: &SpongeSchematic) -> Vec<(i32, i32, i32)> {
        let (w, h, l) = s.size();
        let mut out = Vec::new();
        for y in 0..h {
            for z in 0..l {
                for x in 0..w {
                    if s.state_at(crate::sponge::Pos::new(x, y, z))
                        .is_some_and(|v| v.starts_with("minecraft:note_block["))
                    {
                        out.push((x, y, z));
                    }
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_20hz_pair_lands_on_consecutive_ticks() {
        let s = song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 46)]);
        let (r, schem) = build(&s, &HalfTickOptions::default()).unwrap();
        assert_eq!(r.notes, 2);
        assert_eq!(r.lanes, 1);
        assert_eq!(r.compensation_cells, 0);
        let p = note_positions(&schem);
        assert_eq!(p.len(), 2);
        assert!(p.iter().any(|q| q.1 == ROW_EVEN_Y));
        assert!(p.iter().any(|q| q.1 == ROW_ODD_Y));
        assert!(p.iter().all(|q| q.2 == FIRST_NOTE_Z));
    }

    #[test]
    fn fifteen_slots_use_five_repeaters_and_three_note_columns() {
        let (r, schem) = build(&chord(15), &HalfTickOptions::default()).unwrap();
        assert_eq!(r.lanes, 1);
        assert_eq!(r.notes, 15);
        assert_eq!(r.repeaters, 11);
        assert_eq!(r.switch_buttons, 1);
        assert_eq!(r.switch_pistons, 1);
        assert_eq!(r.switch_redstone_blocks, 1);
        assert_eq!(r.switch_repeaters, 1);
        assert_eq!(r.notes_missing_trigger, 0);
        let p = note_positions(&schem);
        assert_eq!(p.len(), 15);
        assert!(p.iter().all(|q| q.0 >= 0 && q.0 < 15 && q.1 == ROW_EVEN_Y && q.2 == FIRST_NOTE_Z));
        for center in CENTERS {
            let x = r.size[0] - 1 - center;
            assert!(schem.state_at(crate::sponge::Pos::new(x, ROW_EVEN_Y, FIRST_REPEATER_Z))
                .is_some_and(|v| v.starts_with("minecraft:repeater[")));
            assert!(schem.state_at(crate::sponge::Pos::new(x, ROW_EVEN_Y, FIRST_NOTE_Z))
                .is_some_and(|v| v.starts_with("minecraft:note_block[")));
        }
    }

    #[test]
    fn the_user_switch_is_at_screen_left_when_facing_south() {
        let (r, schem) = build(&chord(1), &HalfTickOptions::default()).unwrap();
        assert_eq!(r.input_x, r.size[0] - 1);
        assert_eq!(r.switch_buttons, 1);
        assert_eq!(r.switch_pistons, 1);
        assert_eq!(r.switch_redstone_blocks, 1);
        assert_eq!(r.switch_repeaters, 1);
        let button = schem.state_at(crate::sponge::Pos::new(12, ROW_EVEN_Y, 1)).unwrap();
        assert!(button.starts_with("minecraft:oak_button[") && prop_value(button, "facing") == Some("west"));
        assert!(schem.state_at(crate::sponge::Pos::new(14, ROW_EVEN_Y, INPUT_Z - 1))
            .is_some_and(|v| v.starts_with("minecraft:repeater[") && prop_value(v, "facing") == Some("north")));
        assert_eq!(schem.state_at(crate::sponge::Pos::new(14, ROW_ODD_Y, 2)), Some(SWITCH_PISTON));
        assert_eq!(schem.state_at(crate::sponge::Pos::new(14, ROW_ODD_Y, 3)), Some(SWITCH_REDSTONE_BLOCK));
        for y in [ROW_EVEN_Y, ROW_ODD_Y] {
            assert!(schem.state_at(crate::sponge::Pos::new(14, y, INPUT_Z))
                .is_some_and(|v| v.starts_with("minecraft:redstone_wire")));
        }
    }

    #[test]
    fn sixteenth_note_opens_a_second_lane_and_compensates_one_cell() {
        let (r, schem) = build(&chord(16), &HalfTickOptions::default()).unwrap();
        assert_eq!(r.lanes, 2);
        assert_eq!(r.compensation_cells, 1);
        assert_eq!(r.lead_in_gt, 2);
        assert_eq!(r.bus_repeaters, 2);
        let p = note_positions(&schem);
        assert!(p.iter().any(|q| q.0 >= 16 && q.2 == FIRST_NOTE_Z + 2));
        assert!(p.iter().any(|q| q.0 < 15 && q.2 == FIRST_NOTE_Z));
        for y in [ROW_EVEN_Y, ROW_ODD_Y] {
            assert!(schem.state_at(crate::sponge::Pos::new(15, y, INPUT_Z))
                .is_some_and(|v| v.starts_with("minecraft:repeater[") && prop_value(v, "facing") == Some("west")));
        }
    }

    #[test]
    fn three_lanes_are_compact_and_share_one_input_pair() {
        let (r, schem) = build(&chord(31), &HalfTickOptions::default()).unwrap();
        assert_eq!(r.lanes, 3);
        assert_eq!(r.size[0], 47);
        assert_eq!(r.compensation_cells, 2);
        assert_eq!(r.lead_in_gt, 4);
        assert_eq!(r.bus_repeaters, 4);
        let p = note_positions(&schem);
        for (min_x, max_x, z) in [
            (32, 47, FIRST_NOTE_Z + 4),
            (16, 31, FIRST_NOTE_Z + 2),
            (0, 16, FIRST_NOTE_Z),
        ] {
            assert!(p.iter().any(|q| q.0 >= min_x && q.0 < max_x && q.2 == z));
        }
        for y in [ROW_EVEN_Y, ROW_ODD_Y] {
            for x in [15, 31] {
                assert!(schem.state_at(crate::sponge::Pos::new(x, y, INPUT_Z))
                    .is_some_and(|v| v.starts_with("minecraft:repeater[") && prop_value(v, "facing") == Some("west")));
            }
        }
    }

    #[test]
    fn every_note_has_trigger_instrument_and_stone_support() {
        let s = song(20.0, &[(0, 0, 0, 45), (0, 2, 3, 50), (1, 1, 7, 55)]);
        let (r, schem) = build(&s, &HalfTickOptions::default()).unwrap();
        assert_eq!(r.notes, 3);
        assert_eq!(r.notes_missing_trigger, 0);
        assert_eq!(r.notes_missing_instrument, 0);
        assert_eq!(r.notes_missing_support, 0);
        for (x, y, z) in note_positions(&schem) {
            assert!(schem.state_at(crate::sponge::Pos::new(x, y - 1, z)).is_some());
            assert_eq!(schem.state_at(crate::sponge::Pos::new(x, y - 2, z)), Some(NOTE_SUPPORT));
        }
    }

    #[test]
    fn every_wire_run_stays_inside_the_dust_budget() {
        let (r, _) = build(&chord(45), &HalfTickOptions::default()).unwrap();
        assert_eq!(r.lanes, 3);
        assert!(r.wire_cells >= 90);
        assert!(r.longest_wire <= DUST_BUDGET, "longest {}", r.longest_wire);
        assert_eq!(r.notes, 45);
    }

    #[test]
    fn the_product_is_sponge_v3_with_the_user_data_version() {
        let (_, schem) = build(&chord(1), &HalfTickOptions::default()).unwrap();
        let root = crate::nbt::parse(&schem.to_bytes()).unwrap();
        let inner = root.get("Schematic").unwrap();
        assert_eq!(inner.get("Version").and_then(crate::nbt::Value::as_i32), Some(3));
        assert_eq!(inner.get("DataVersion").and_then(crate::nbt::Value::as_i32), Some(DATA_VERSION_4903));
    }

    #[test]
    fn the_report_is_honest_about_first_version_status() {
        let r = preview(&chord(1), &HalfTickOptions::default()).unwrap();
        let text = r.lines().join("\n");
        assert!(text.contains("真机") || text.contains("in-game"));
        assert!(text.contains("共用输入") || text.contains("Shared input"));
        assert!(text.contains("开关") || text.contains("Switch"));
        assert!(!text.contains("贴下去就能响"));
    }
}
