//! 原版 `scripts/minecraft_export/minecraft_export.gml` 的移植：命令方块与数据包
//! 导出共用的声音命令计划器。
//!
//! Sound Stopper 按出现实例分配：Minecraft 只能区分最终的声音事件与声源，所以
//! 重叠的音符用「尽量少的声源类别」着色，把互相掐掉的风险压到可证的最小值。
//! 图着色（贪心上界 + 精确搜索 + 牺牲子集搜索）逐条对照原版转录，包括
//! 先到先得的平局规则与工时/时限预算。

use crate::datapack::{dat_pitch, gml_num, instrument_sound};
use nbs_core::model::Song;
use nbs_core::model::{panning_velocity_to_short, CustomInstrument, FIRST_CUSTOM_INDEX};
use nbs_core::model::{SOUND_STOPPER_NAME, TEMPO_CHANGER_NAME};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 原版 `minecraft_export_sources()`。
pub const SOURCES: [&str; 10] = [
    "ambient", "block", "hostile", "master", "music", "neutral", "player", "record", "voice",
    "weather",
];

/// 原版 `minecraft_export_default_allowed_sources()`：Master 需单独勾选，其余默认允许。
pub fn default_allowed_sources() -> [bool; 10] {
    let mut out = [true; 10];
    out[3] = false;
    out
}

/// 原版 `minecraft_export_source_palette(preferred, allowed)`。
/// Master 不勾选就绝不使用（哪怕还是首选）；非 Master 的首选是明确选择，保持可用。
pub fn source_palette(preferred: &str, allowed: &[bool; 10]) -> Vec<&'static str> {
    let mut allowed = *allowed;
    let mut preferred_index = SOURCES.iter().position(|s| *s == preferred).unwrap_or(7);
    if preferred_index == 3 && !allowed[3] {
        preferred_index = 7;
        if !allowed[preferred_index] {
            for i in 0..SOURCES.len() {
                if i != 3 && allowed[i] {
                    preferred_index = i;
                    break;
                }
            }
        }
    }
    if !allowed[preferred_index] {
        allowed[preferred_index] = true;
    }
    let mut result = vec![SOURCES[preferred_index]];
    for i in 0..SOURCES.len() {
        if i != preferred_index && i != 3 && allowed[i] {
            result.push(SOURCES[i]);
        }
    }
    if preferred_index != 3 && allowed[3] {
        result.push("master");
    }
    result
}

/// 原版 `custom_instrument_is_event(instrument_name)`（custom_instruments_resolve.gml）。
pub fn is_event_instrument(instrument_name: &str) -> bool {
    instrument_name == TEMPO_CHANGER_NAME
        || instrument_name == "Toggle Rainbow"
        || instrument_name == SOUND_STOPPER_NAME
        || instrument_name == "Show Save Popup"
        || instrument_name
            .to_lowercase()
            .matches("change color to #")
            .count()
            == 1
        || instrument_name == "Toggle Background Accent"
}

/// 原版 `minecraft_export_sanitize_part(value)`。
fn sanitize_part(value: &str) -> String {
    let value = value.to_lowercase().replace('\\', "/");
    let mut result = String::new();
    let mut previous_separator = false;
    for ch in value.chars() {
        let valid =
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '.' | '-' | '/');
        if valid {
            result.push(ch);
            previous_separator = false;
        } else if !previous_separator {
            result.push('_');
            previous_separator = true;
        }
    }
    while result.starts_with('/') {
        result.remove(0);
    }
    while result.ends_with('/') {
        result.pop();
    }
    result
}

/// 原版 `minecraft_export_resource_location(value)`。
pub fn resource_location(value: &str) -> String {
    let value = value.replace('\\', "/");
    let (namespace, path) = match value.find(':') {
        Some(separator) => (
            value[..separator].to_string(),
            value[separator + 1..].to_string(),
        ),
        None if value.to_lowercase().starts_with("minecraft/") => {
            // GML string_copy 从 1 基第 11 个字符起，即 0 基第 10 字节（"minecraft/" 之后）。
            ("minecraft".to_string(), value[10..].to_string())
        }
        None => ("minecraft".to_string(), value),
    };
    let namespace = sanitize_part(&namespace);
    let path = sanitize_part(&path);
    let namespace = if namespace.is_empty() {
        "minecraft"
    } else {
        &namespace
    };
    let path = if path.is_empty() {
        "unnamed_instrument"
    } else {
        &path
    };
    format!("{namespace}:{path}")
}

/// 乐器映射时「手动指定声音事件」的来源。原版存在音色实例的运行时字段上，
/// 由音色窗口的输入框写入；端口里由调用方注入（无输入框时恒为 None）。
pub type MappingFn<'a> = &'a dyn Fn(usize) -> Option<String>;

/// 采样时长（秒）的来源。原版读加载好的音色实例的 `sound_duration`；
/// 端口由音频引擎的采样库注入。None 或 <=0 视为未加载。
pub type DurationFn<'a> = &'a dyn Fn(usize) -> Option<f64>;

/// 原版 `minecraft_export_ensure_mapping(instrument, instrument_index)`。
/// 原版读音色实例上的运行时字段；端口里手动映射由调用方注入。
fn ensure_mapping(song: &Song, instrument_index: usize, manual: Option<&str>) -> String {
    let name = song.instrument_name(instrument_index as u8);
    if is_event_instrument(name) {
        // 事件乐器的名字是保留字，映射保持空白。
        return String::new();
    }
    if (instrument_index as u8) < FIRST_CUSTOM_INDEX {
        return instrument_sound(song, instrument_index);
    }
    let ins = match custom_of(song, instrument_index) {
        Some(ins) => ins,
        None => return String::new(),
    };
    let sound = match manual {
        Some(s) => s.to_string(),
        None => {
            // 新增音色还没有声音可映射，别把编号占位名变成 custom_instrument_1 事件。
            if ins.filename.is_empty() && ins.name.starts_with("Custom instrument #") {
                return String::new();
            }
            resource_location(&ins.name)
        }
    };
    if sound.is_empty() {
        return String::new();
    }
    resource_location(&sound)
}

/// 原版 `minecraft_export_effective_key(song, tick, layer, instrument_index)`。
pub fn effective_key(song: &Song, tick: u32, layer: u32, instrument_index: usize) -> f64 {
    let note = match song.get(tick, layer) {
        Some(n) => n,
        None => return 0.0,
    };
    let ins_key = match (instrument_index as u8).checked_sub(FIRST_CUSTOM_INDEX) {
        Some(custom) => song
            .custom_instruments
            .get(custom as usize)
            .map(|c| c.key as f64)
            .unwrap_or(45.0),
        None => 45.0,
    };
    note.key as f64 + note.pitch as f64 / 100.0 + ins_key - 45.0
}

/// 原版 `minecraft_export_sound_event(song, tick, layer, instrument_index)`。
fn sound_event(
    song: &Song,
    tick: u32,
    layer: u32,
    instrument_index: usize,
    mappings: MappingFn,
) -> String {
    let event_name = ensure_mapping(
        song,
        instrument_index,
        mappings(instrument_index).as_deref(),
    );
    if event_name.is_empty() {
        return String::new();
    }
    let key = effective_key(song, tick, layer, instrument_index);
    if key < 33.0 {
        format!("{event_name}_-1")
    } else if key > 57.0 {
        format!("{event_name}_1")
    } else {
        event_name
    }
}

fn custom_of(song: &Song, instrument_index: usize) -> Option<CustomInstrument> {
    (instrument_index as u8)
        .checked_sub(FIRST_CUSTOM_INDEX)
        .and_then(|c| song.custom_instruments.get(c as usize).cloned())
}

/// 原版 `minecraft_export_note_duration(song, tick, layer, instrument_index)`。
fn note_duration(
    song: &Song,
    tick: u32,
    layer: u32,
    instrument_index: usize,
    durations: DurationFn,
) -> f64 {
    let sound_duration = match durations(instrument_index) {
        Some(d) if d > 0.0 => d,
        _ => return f64::INFINITY,
    };
    let note = match song.get(tick, layer) {
        Some(n) => n,
        None => return f64::INFINITY,
    };
    let ins_key = match (instrument_index as u8).checked_sub(FIRST_CUSTOM_INDEX) {
        Some(custom) => song
            .custom_instruments
            .get(custom as usize)
            .map(|c| c.key as f64)
            .unwrap_or(45.0),
        None => 45.0,
    };
    let keyshift = note.key as f64 + ins_key + note.pitch as f64 / 100.0 - 78.0;
    let emitter_pitch = 0.5 * 2f64.powf(keyshift / 12.0);
    // 原版此处还会乘 instrument.resourcepack_pitch（更多音符资源包），
    // 端口未接资源包机制，恒为 1。
    sound_duration / emitter_pitch.max(0.000000001)
}

/// 原版 `minecraft_export_stopper_range(song, tick, layer)`：起始层取音高字段的
/// floor，结束层从声像 + 力度解出。空区间必须保留，不能殃及相邻层。
fn stopper_range(song: &Song, tick: u32, layer: u32) -> (u32, u32) {
    let note = match song.get(tick, layer) {
        Some(n) => n,
        None => return (0, 0),
    };
    let start = (note.pitch as f64).floor().max(0.0) as u32;
    let end = panning_velocity_to_short(note.panning, note.velocity);
    (start, end)
}

/// 原版 `minecraft_export_layer_targeted(one_based_layer, layer_range)`。
fn layer_targeted(one_based_layer: u32, layer_range: (u32, u32)) -> bool {
    layer_range.0 == 0 || (one_based_layer >= layer_range.0 && one_based_layer <= layer_range.1)
}

/// 原版 `minecraft_export_snapped_speed(tempo)`：80 分/曲 tick ÷ 20 游戏刻/秒 = tempo*4。
fn snapped_speed(tempo: f64) -> i64 {
    (tempo.abs() * 4.0).round().max(1.0) as i64
}

/// 原版 `minecraft_export_tempo_from_note(song, tick, layer)`。
fn tempo_from_note(song: &Song, tick: u32, layer: u32) -> f64 {
    let pitch = song.get(tick, layer).map(|n| n.pitch).unwrap_or(0) as f64;
    pitch.abs().floor() / 15.0
}

/// 原版 `minecraft_export_tempo_speed_after_tick(song, tick, include_locked, current_speed)`。
fn tempo_speed_after_tick(
    song: &Song,
    tick: u32,
    include_locked: bool,
    locked: &[bool],
    mut current_speed: i64,
) -> i64 {
    if tick > song.enda {
        return current_speed;
    }
    let column = match song.columns.get(&tick) {
        Some(c) => c,
        None => return current_speed,
    };
    for (layer_index, note) in &column.notes {
        if !include_locked && locked.get(*layer_index as usize).copied().unwrap_or(false) {
            continue;
        }
        let name = song.instrument_name(note.instrument);
        if name == TEMPO_CHANGER_NAME {
            current_speed = snapped_speed(tempo_from_note(song, tick, *layer_index));
        }
    }
    current_speed
}

/// 原版 `minecraft_export_datapack_server_grid(song, include_locked, start_tick)`。
fn datapack_server_grid(
    song: &Song,
    include_locked: bool,
    locked: &[bool],
    start_tick: u32,
) -> Vec<i64> {
    let mut result = vec![-1i64; song.enda as usize + 2];
    let mut playback_speed = snapped_speed(song.meta.tempo as f64);
    for tick in 0..=start_tick {
        playback_speed = tempo_speed_after_tick(song, tick, include_locked, locked, playback_speed);
    }
    // 播放先推进再派发列。用与数据包一致的 80 分/曲 tick 空间，让一个 20 TPS
    // 更新跨过的列落进同一个服务端刻桶。
    let mut playback_position = start_tick as f64 * 80.0 + playback_speed as f64;
    let mut server_tick = 1i64;
    for tick in start_tick..=song.enda {
        let threshold = tick as f64 * 80.0;
        if playback_position < threshold {
            let advances = ((threshold - playback_position) / playback_speed as f64).ceil();
            playback_position += advances * playback_speed as f64;
            server_tick += advances as i64;
        }
        result[tick as usize] = server_tick;
        playback_speed = tempo_speed_after_tick(song, tick, include_locked, locked, playback_speed);
    }
    result
}

/// 原版 `minecraft_export_timing(song, mode, include_locked, tempo_grid, direct_tps)`。
#[derive(Clone)]
pub struct Timing {
    /// 每曲 tick 的墙钟秒数。
    pub wall: Vec<f64>,
    /// 输出网格号：命令模式开 tempo_grid 时吸附到红石网格，否则等于曲 tick。
    pub grid: Vec<i64>,
    /// 数据包模式的 20 TPS 服务端刻桶。
    pub server: Vec<i64>,
    /// 数据包循环段的桶。
    pub loop_server: Vec<i64>,
}

fn timing(
    song: &Song,
    mode: &str,
    include_locked: bool,
    locked: &[bool],
    tempo_grid: bool,
    direct_tps: f64,
) -> Timing {
    let mut wall = vec![0.0f64; song.enda as usize + 2];
    let mut grid_ticks = vec![0i64; song.enda as usize + 2];
    let mut tempo_speed = snapped_speed(song.meta.tempo as f64);
    for tick in 0..=song.enda as usize + 1 {
        if tick > 0 {
            if mode == "command" && !tempo_grid {
                wall[tick] = tick as f64 / direct_tps;
            } else {
                wall[tick] = wall[tick - 1] + 4.0 / tempo_speed as f64;
            }
        }
        if tick <= song.enda as usize && (mode == "datapack" || tempo_grid) {
            tempo_speed =
                tempo_speed_after_tick(song, tick as u32, include_locked, locked, tempo_speed);
        }
        grid_ticks[tick] = if mode == "command" && tempo_grid {
            (wall[tick] * direct_tps).round() as i64
        } else {
            tick as i64
        };
    }
    let (server, loop_server) = if mode == "datapack" {
        (
            datapack_server_grid(song, include_locked, locked, 0),
            datapack_server_grid(song, include_locked, locked, song.meta.loop_start as u32),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    Timing {
        wall,
        grid: grid_ticks,
        server,
        loop_server,
    }
}

fn array_contains(values: &[usize], wanted: usize) -> bool {
    values.contains(&wanted)
}

fn array_remove_value(values: &mut Vec<usize>, wanted: usize) {
    values.retain(|v| *v != wanted);
}

/// 原版 `minecraft_export_add_unique_target(stopper, occurrence_index)`。
fn add_unique_target(values: &mut Vec<usize>, occurrence_index: usize) {
    if !values.contains(&occurrence_index) {
        values.push(occurrence_index);
    }
}

/// 原版 `minecraft_export_set_snapshot_survivors(snapshot, values)`。
fn set_snapshot_survivors(snapshots: &mut [Snapshot], index: usize, values: &[usize]) {
    let mut survivors = values.to_vec();
    let targets = snapshots[index].targets.clone();
    for t in targets {
        array_remove_value(&mut survivors, t);
    }
    snapshots[index].survivors = survivors;
}

/// 原版 `minecraft_export_add_snapshot_survivors(snapshot, values)`。
fn add_snapshot_survivors(snapshots: &mut [Snapshot], index: usize, values: &[usize]) {
    let targets = snapshots[index].targets.clone();
    for v in values {
        if targets.contains(v) {
            continue;
        }
        if !snapshots[index].survivors.contains(v) {
            snapshots[index].survivors.push(*v);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Occurrence {
    pub index: usize,
    pub tick: u32,
    pub layer: u32,
    pub instrument: usize,
    pub key: f64,
    pub event: String,
    pub duration: f64,
}

#[derive(Clone, Debug)]
pub struct Stopper {
    pub tick: u32,
    pub layer: u32,
    /// [起始层, 终止层]，1 基；起始层 0 表示「全部层」。
    pub layer_range: (u32, u32),
    pub instrument: usize,
    pub targets: Vec<usize>,
    /// 指向 `Plan::snapshots` 的下标。
    pub snapshots: Vec<usize>,
    pub grid_delay: u32,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub stopper: usize,
    pub active: Vec<usize>,
    pub targets: Vec<usize>,
    pub survivors: Vec<usize>,
    pub absolute_tick: u32,
    pub output_grid: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Play,
    Stop,
    Tempo,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub index: usize,
    pub kind: RowKind,
    pub tick: u32,
    pub grid: i64,
    pub layer: u32,
    pub instrument: usize,
    pub command: String,
    pub event: String,
    pub source: String,
    /// 仅 stop 行。
    pub stopper_index: Option<usize>,
    /// 仅数据包的延迟 stop 行。
    pub delayed: bool,
    /// 仅 tempo 行。
    pub speed: i64,
}

#[derive(Clone, Debug)]
pub struct Risk {
    pub target: usize,
    pub victim: usize,
    pub stopper: usize,
    pub lost: i64,
}

/// 原版 `minecraft_export_add_risk(risks, risk_map, target, victim, stopper, lost_ticks)`。
fn add_risk(
    risks: &mut Vec<Risk>,
    risk_map: &mut HashSet<(usize, usize)>,
    target: usize,
    victim: usize,
    stopper: usize,
    lost_ticks: i64,
) {
    let key = (target, victim);
    if let Some(existing_index) = risk_map.iter().position(|k| *k == key) {
        let lost_ticks = lost_ticks.max(0);
        if lost_ticks > risks[existing_index].lost {
            risks[existing_index].lost = lost_ticks;
            risks[existing_index].stopper = stopper;
        }
        return;
    }
    risk_map.insert(key);
    risks.push(Risk {
        target,
        victim,
        stopper,
        lost: lost_ticks.max(0),
    });
}

/// 原版 `minecraft_export_risk_objective(risks, colors)`：同色牺牲者数量 + 保守损失时长。
fn risk_objective(risks: &[Risk], colors: &[i32]) -> (usize, i64) {
    let mut victims: Vec<(usize, i64)> = Vec::new();
    for risk in risks {
        if colors[risk.target] != colors[risk.victim] {
            continue;
        }
        if let Some(slot) = victims.iter_mut().find(|(v, _)| *v == risk.victim) {
            slot.1 = slot.1.max(risk.lost);
        } else {
            victims.push((risk.victim, risk.lost));
        }
    }
    let lost = victims.iter().map(|(_, l)| *l).sum();
    (victims.len(), lost)
}

fn objective_less(first: (usize, i64), second: (usize, i64)) -> bool {
    first.0 < second.0 || (first.0 == second.0 && first.1 < second.1)
}

/// 工时/时限预算，对应原版 budget 结构。时限取进程起点，语义与 GML get_timer 一致。
struct Budget {
    work: u64,
    maximum_work: u64,
    deadline: Instant,
    limited: bool,
}

impl Budget {
    fn new(full_search: bool) -> Self {
        Budget {
            work: 0,
            maximum_work: if full_search { 150000 } else { 20000 },
            deadline: Instant::now()
                + Duration::from_micros(if full_search { 1500000 } else { 200000 }),
            limited: false,
        }
    }

    fn expired(&self) -> bool {
        self.work > self.maximum_work || Instant::now() > self.deadline
    }
}

/// 原版 `minecraft_export_greedy_colors(vertices, adjacency, maximum_colors)`。
fn greedy_colors(vertices: &[usize], adjacency: &[Vec<usize>], maximum_colors: usize) -> Vec<i32> {
    let mut colors = vec![-1i32; adjacency.len()];
    let mut remaining = vertices.to_vec();
    let mut class_sizes = vec![0usize; maximum_colors.max(1)];
    while !remaining.is_empty() {
        let mut selected_position = 0usize;
        let mut selected_saturation = -1i64;
        let mut selected_degree = -1i64;
        for (p, vertex) in remaining.iter().enumerate() {
            let mut used: HashSet<i32> = HashSet::new();
            for n in &adjacency[*vertex] {
                let neighbor_color = colors[*n];
                if neighbor_color >= 0 {
                    used.insert(neighbor_color);
                }
            }
            let saturation = used.len() as i64;
            if saturation > selected_saturation
                || (saturation == selected_saturation
                    && adjacency[*vertex].len() as i64 > selected_degree)
            {
                selected_position = p;
                selected_saturation = saturation;
                selected_degree = adjacency[*vertex].len() as i64;
            }
        }
        let vertex = remaining[selected_position];
        let mut best_color = 0usize;
        let mut best_conflicts = i64::MAX;
        let color_limit = maximum_colors.max(1);
        for color in 0..color_limit {
            let mut conflicts = 0i64;
            for n in &adjacency[vertex] {
                conflicts += (colors[*n] == color as i32) as i64;
            }
            if conflicts < best_conflicts
                || (conflicts == best_conflicts && class_sizes[color] < class_sizes[best_color])
            {
                best_color = color;
                best_conflicts = conflicts;
            }
        }
        colors[vertex] = best_color as i32;
        class_sizes[best_color] += 1;
        remaining.remove(selected_position);
    }
    colors
}

/// 原版 `minecraft_export_k_color_search(state)`。
fn k_color_search(
    vertices: &[usize],
    adjacency: &[Vec<usize>],
    colors: &mut [i32],
    color_count: usize,
    budget: &mut Budget,
) -> bool {
    budget.work += 1;
    if budget.expired() {
        budget.limited = true;
        return false;
    }
    let mut selected: Option<usize> = None;
    let mut selected_saturation = -1i64;
    let mut selected_degree = -1i64;
    for vertex in vertices {
        if colors[*vertex] >= 0 {
            continue;
        }
        let mut used: HashSet<i32> = HashSet::new();
        for n in &adjacency[*vertex] {
            let neighbor_color = colors[*n];
            if neighbor_color >= 0 {
                used.insert(neighbor_color);
            }
        }
        let saturation = used.len() as i64;
        if saturation > selected_saturation
            || (saturation == selected_saturation
                && adjacency[*vertex].len() as i64 > selected_degree)
        {
            selected = Some(*vertex);
            selected_saturation = saturation;
            selected_degree = adjacency[*vertex].len() as i64;
        }
    }
    let selected = match selected {
        Some(v) => v,
        None => return true,
    };
    for color in 0..color_count {
        let mut valid = true;
        for n in &adjacency[selected] {
            if colors[*n] == color as i32 {
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }
        colors[selected] = color as i32;
        if k_color_search(vertices, adjacency, colors, color_count, budget) {
            return true;
        }
        colors[selected] = -1;
        if budget.limited {
            return false;
        }
    }
    false
}

/// 原版 `minecraft_export_k_color(vertices, adjacency, color_count, budget)`。
fn k_color(
    vertices: &[usize],
    adjacency: &[Vec<usize>],
    color_count: usize,
    budget: &mut Budget,
) -> (bool, Vec<i32>, bool) {
    let mut colors = vec![-1i32; adjacency.len()];
    let mut local = Budget {
        work: budget.work,
        maximum_work: budget.maximum_work,
        deadline: budget.deadline,
        limited: false,
    };
    let found = k_color_search(vertices, adjacency, &mut colors, color_count, &mut local);
    budget.work = local.work;
    budget.limited = budget.limited || local.limited;
    (found, colors, local.limited)
}

/// 原版 `minecraft_export_connected_components(vertices, adjacency)`。
fn connected_components(vertices: &[usize], adjacency: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for start in vertices {
        if seen.contains(start) {
            continue;
        }
        let mut stack = vec![*start];
        let mut component = Vec::new();
        seen.insert(*start);
        while let Some(vertex) = stack.pop() {
            component.push(vertex);
            for neighbor in &adjacency[vertex] {
                if seen.insert(*neighbor) {
                    stack.push(*neighbor);
                }
            }
        }
        components.push(component);
    }
    components
}

/// 原版 `minecraft_export_greedy_clique_lower_bound(vertices, adjacency)`。
fn greedy_clique_lower_bound(vertices: &[usize], adjacency: &[Vec<usize>]) -> usize {
    if vertices.is_empty() {
        return 0;
    }
    let mut best = 1usize;
    let mut starts = vertices.to_vec();
    // 少量高度数起点就能找到重要的反向 stop 团，又不至于让下界捷径卡住界面。
    let start_limit = starts.len().min(12);
    for _ in 0..start_limit {
        let mut start_position = 0usize;
        for i in 1..starts.len() {
            if adjacency[starts[i]].len() > adjacency[starts[start_position]].len() {
                start_position = i;
            }
        }
        let start = starts.remove(start_position);
        let mut candidates = adjacency[start].clone();
        let mut clique_size = 1usize;
        while !candidates.is_empty() {
            let candidate_set: HashSet<usize> = candidates.iter().copied().collect();
            let mut selected_position = 0usize;
            let mut selected_intersections = -1i64;
            let mut selected_degree = -1i64;
            for (i, vertex) in candidates.iter().enumerate() {
                let mut intersections = 0i64;
                for n in &adjacency[*vertex] {
                    intersections += candidate_set.contains(n) as i64;
                }
                if intersections > selected_intersections
                    || (intersections == selected_intersections
                        && adjacency[*vertex].len() as i64 > selected_degree)
                {
                    selected_position = i;
                    selected_intersections = intersections;
                    selected_degree = adjacency[*vertex].len() as i64;
                }
            }
            let selected = candidates[selected_position];
            clique_size += 1;
            let selected_neighbors: HashSet<usize> = adjacency[selected].iter().copied().collect();
            candidates.retain(|c| *c != selected && selected_neighbors.contains(c));
        }
        best = best.max(clique_size);
    }
    best
}

/// 牺牲子集搜索的共享状态，对应原版 lossy state 结构。
struct LossyState<'a> {
    component: &'a [usize],
    risks: &'a [Risk],
    play_count: usize,
    palette_count: usize,
    candidates: Vec<usize>,
    sacrificed: HashSet<usize>,
    best_colors: Vec<i32>,
    best_objective: (usize, i64),
    found_at_count: bool,
}

/// 原版 `minecraft_export_lossy_subset_leaf(state)`。
fn lossy_subset_leaf(state: &mut LossyState, budget: &mut Budget) {
    budget.work += 1;
    if budget.expired() {
        budget.limited = true;
        return;
    }
    let mut adjacency = vec![Vec::<usize>::new(); state.play_count];
    for risk in state.risks {
        if state.sacrificed.contains(&risk.victim) {
            continue;
        }
        if risk.target == risk.victim {
            return;
        }
        if !adjacency[risk.target].contains(&risk.victim) {
            adjacency[risk.target].push(risk.victim);
        }
        if !adjacency[risk.victim].contains(&risk.target) {
            adjacency[risk.victim].push(risk.target);
        }
    }
    let (found, colors, limited) =
        k_color(state.component, &adjacency, state.palette_count, budget);
    if limited || !found {
        return;
    }
    state.found_at_count = true;
    let objective = risk_objective(state.risks, &colors);
    if objective_less(objective, state.best_objective) {
        state.best_objective = objective;
        state.best_colors = colors;
    }
}

/// 原版 `minecraft_export_lossy_subset_search(state, position, remaining)`。
fn lossy_subset_search(
    state: &mut LossyState,
    position: usize,
    remaining: usize,
    budget: &mut Budget,
) {
    if budget.limited {
        return;
    }
    if remaining == 0 {
        lossy_subset_leaf(state, budget);
        return;
    }
    if position >= state.candidates.len() || state.candidates.len() - position < remaining {
        return;
    }
    let victim = state.candidates[position];
    state.sacrificed.insert(victim);
    lossy_subset_search(state, position + 1, remaining - 1, budget);
    state.sacrificed.remove(&victim);
    lossy_subset_search(state, position + 1, remaining, budget);
}

/// 原版 `minecraft_export_lossy_coloring(...)`。
fn lossy_coloring(
    component: &[usize],
    component_risks: &[Risk],
    play_count: usize,
    palette_count: usize,
    budget: &mut Budget,
    initial_colors: Vec<i32>,
    clique_cutoff_lower_bound: usize,
) -> (Vec<i32>, (usize, i64), bool) {
    let mut victims: Vec<usize> = Vec::new();
    let mut sacrificed: HashSet<usize> = HashSet::new();
    let mut forced_count = 0usize;
    for risk in component_risks {
        if !victims.contains(&risk.victim) {
            victims.push(risk.victim);
        }
        if risk.target == risk.victim && !sacrificed.contains(&risk.victim) {
            sacrificed.insert(risk.victim);
            forced_count += 1;
        }
    }
    let candidates: Vec<usize> = victims
        .iter()
        .copied()
        .filter(|v| !sacrificed.contains(v))
        .collect();
    let best_objective = risk_objective(component_risks, &initial_colors);
    let mut state = LossyState {
        component,
        risks: component_risks,
        play_count,
        palette_count,
        candidates,
        sacrificed,
        best_colors: initial_colors,
        best_objective,
        found_at_count: false,
    };
    let cutoff_lower_bound = clique_cutoff_lower_bound.max(forced_count);
    // 达到团/自险下界即可证明主要目标，哪怕剩余预算只能启发式地压缩损失时长。
    let mut proven = state.best_objective.0 == cutoff_lower_bound;
    let maximum_extra = state
        .candidates
        .len()
        .min(state.best_objective.0.saturating_sub(forced_count));
    let minimum_extra = cutoff_lower_bound.saturating_sub(forced_count);
    for extra in minimum_extra..=maximum_extra {
        state.found_at_count = false;
        lossy_subset_search(&mut state, 0, extra, budget);
        if budget.limited {
            break;
        }
        if state.found_at_count {
            proven = true;
            break;
        }
    }
    (state.best_colors, state.best_objective, proven)
}

/// 原版 `minecraft_export_assign_colors(play_count, risks, palette_count, full_search)`。
struct Assignment {
    colors: Vec<i32>,
    exact: bool,
    required: usize,
    minimum_proven: bool,
    cutoff_proven: bool,
    search_limited: bool,
    cutoffs: usize,
    lost_ticks: i64,
}

fn assign_colors(
    play_count: usize,
    risks: &[Risk],
    palette_count: usize,
    full_search: bool,
) -> Assignment {
    let mut adjacency = vec![Vec::<usize>::new(); play_count];
    let mut risk_vertices: Vec<usize> = Vec::new();
    let mut self_risk_vertices: HashSet<usize> = HashSet::new();
    let mut self_risk = false;
    for risk in risks {
        if !risk_vertices.contains(&risk.target) {
            risk_vertices.push(risk.target);
        }
        if !risk_vertices.contains(&risk.victim) {
            risk_vertices.push(risk.victim);
        }
        if risk.target == risk.victim {
            self_risk = true;
            self_risk_vertices.insert(risk.target);
        } else {
            if !adjacency[risk.target].contains(&risk.victim) {
                adjacency[risk.target].push(risk.victim);
            }
            if !adjacency[risk.victim].contains(&risk.target) {
                adjacency[risk.victim].push(risk.target);
            }
        }
    }
    let mut colors = vec![0i32; play_count];
    let mut exact = !self_risk;
    let mut minimum_proven = !self_risk;
    let mut cutoff_proven = true;
    let mut required = 1usize;
    let mut budget = Budget::new(full_search);
    let components = connected_components(&risk_vertices, &adjacency);
    for component in &components {
        let component_set: HashSet<usize> = component.iter().copied().collect();
        let component_risks: Vec<Risk> = risks
            .iter()
            .filter(|r| component_set.contains(&r.target) && component_set.contains(&r.victim))
            .cloned()
            .collect();
        let component_self_risk = component.iter().any(|v| self_risk_vertices.contains(v));
        let unrestricted = greedy_colors(component, &adjacency, component.len());
        let mut upper = 1usize;
        for v in component {
            upper = upper.max(unrestricted[*v].max(0) as usize + 1);
        }
        let mut selected = unrestricted.clone();
        let mut component_required = upper;
        let mut proven = upper <= 2;
        if !component_self_risk && upper > 1 {
            for k in 1..upper {
                let (found, colors_k, limited) = k_color(component, &adjacency, k, &mut budget);
                if limited {
                    proven = false;
                    break;
                }
                if found {
                    selected = colors_k;
                    component_required = k;
                    proven = true;
                    break;
                }
                if k == upper - 1 {
                    proven = true;
                }
            }
        }
        if component_required > palette_count || component_self_risk {
            exact = false;
            selected = greedy_colors(component, &adjacency, palette_count);
            // 先挑可能牺牲的最小声音集合，再选保守损失时长最低的着色。
            let clique_lb =
                greedy_clique_lower_bound(component, &adjacency).saturating_sub(palette_count);
            let (lossy_colors, _, lossy_proven) = lossy_coloring(
                component,
                &component_risks,
                play_count,
                palette_count,
                &mut budget,
                selected,
                clique_lb,
            );
            selected = lossy_colors;
            cutoff_proven = cutoff_proven && lossy_proven;
        }
        for v in component.iter() {
            colors[*v] = selected[*v];
        }
        required = required.max(component_required);
        minimum_proven = minimum_proven && proven;
    }
    let objective = risk_objective(risks, &colors);
    Assignment {
        exact: exact && objective.0 == 0,
        colors,
        required,
        minimum_proven,
        cutoff_proven,
        search_limited: budget.limited,
        cutoffs: objective.0,
        lost_ticks: objective.1,
    }
}

/// 原版 `minecraft_export_build_plan(...)` 的完整移植。
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    song: &Song,
    mode: &str,
    preferred_source: &str,
    allowed_sources: &[bool; 10],
    include_locked: bool,
    include_out_of_range: bool,
    nearby: bool,
    tempo_grid: bool,
    direct_tps: f64,
    looping: bool,
    compress_layers: bool,
    full_search: bool,
    radius: f64,
    mappings: MappingFn,
    durations: DurationFn,
) -> Plan {
    let locked = song.locked_layers();
    let palette = source_palette(preferred_source, allowed_sources);
    let timing = timing(song, mode, include_locked, &locked, tempo_grid, direct_tps);

    let mut occurrences: Vec<Occurrence> = Vec::new();
    let mut stoppers: Vec<Stopper> = Vec::new();
    let mut occupied_ticks: Vec<u32> = Vec::new();
    let mut play_at: std::collections::HashMap<(u32, u32), usize> =
        std::collections::HashMap::new();
    let mut stopper_at: std::collections::HashMap<(u32, u32), usize> =
        std::collections::HashMap::new();
    for tick in 0..=song.enda {
        let column = match song.columns.get(&tick) {
            Some(c) if c.amount() > 0 => c,
            _ => continue,
        };
        let collast = column.last().unwrap_or(0);
        occupied_ticks.push(tick);
        for layer_index in 0..=collast {
            let note = match column.notes.get(&layer_index) {
                Some(n) => n,
                None => continue,
            };
            if !include_locked && locked.get(layer_index as usize).copied().unwrap_or(false) {
                continue;
            }
            let instrument_index = note.instrument as usize;
            if (instrument_index as u8) >= FIRST_CUSTOM_INDEX
                && custom_of(song, instrument_index).is_none()
            {
                continue;
            }
            let name = song.instrument_name(note.instrument);
            if name == SOUND_STOPPER_NAME {
                stopper_at.insert((tick, layer_index), stoppers.len());
                stoppers.push(Stopper {
                    tick,
                    layer: layer_index,
                    layer_range: stopper_range(song, tick, layer_index),
                    instrument: instrument_index,
                    targets: Vec::new(),
                    snapshots: Vec::new(),
                    grid_delay: 0,
                });
                continue;
            }
            if name.is_empty() || is_event_instrument(name) {
                continue;
            }
            let key = effective_key(song, tick, layer_index, instrument_index);
            if !((key >= 33.0 && key <= 57.0)
                || (include_out_of_range && key >= 9.0 && key <= 81.0))
            {
                continue;
            }
            let event_name = sound_event(song, tick, layer_index, instrument_index, mappings);
            if event_name.is_empty() {
                continue;
            }
            let index = occurrences.len();
            play_at.insert((tick, layer_index), index);
            occurrences.push(Occurrence {
                index,
                tick,
                layer: layer_index,
                instrument: instrument_index,
                key,
                event: event_name,
                duration: note_duration(song, tick, layer_index, instrument_index, durations),
            });
        }
    }

    // 吸附把一次发声和它的目标消音器并进同一批输出时，保留发声、把消音器推后一格。
    // 命令结构走所选红石网格；数据包走精确的 20 TPS 累计桶（含曲中速度更改）。
    let mut delayed_stopper_count = 0usize;
    let delay_phase_count = if mode == "datapack" && looping { 2 } else { 1 };
    for delay_phase in 0..delay_phase_count {
        let delay_grid: &Vec<i64> = if mode == "command" {
            &timing.grid
        } else if delay_phase == 0 {
            &timing.server
        } else {
            &timing.loop_server
        };
        let delay_start_tick = if mode == "datapack" && delay_phase > 0 {
            song.meta.loop_start as u32
        } else {
            0
        };
        let mut batch = -1i64;
        let mut local_active: Vec<usize> = Vec::new();
        for tick in &occupied_ticks {
            if *tick < delay_start_tick {
                continue;
            }
            let grid_value = delay_grid.get(*tick as usize).copied().unwrap_or(-1);
            if grid_value != batch {
                batch = grid_value;
                local_active = Vec::new();
            }
            let collast = song.columns.get(tick).and_then(|c| c.last()).unwrap_or(0);
            for layer_index in 0..=collast {
                if let Some(play_index) = play_at.get(&(*tick, layer_index)) {
                    local_active.push(*play_index);
                } else if let Some(stopper_index) = stopper_at.get(&(*tick, layer_index)) {
                    let stop_range = stoppers[*stopper_index].layer_range;
                    let mut delays_this_stopper = false;
                    for i in (0..local_active.len()).rev() {
                        let occurrence = &occurrences[local_active[i]];
                        if layer_targeted(occurrence.layer + 1, stop_range) {
                            delays_this_stopper = true;
                            local_active.remove(i);
                        }
                    }
                    if delays_this_stopper && stoppers[*stopper_index].grid_delay == 0 {
                        stoppers[*stopper_index].grid_delay = 1;
                        delayed_stopper_count += 1;
                    }
                }
            }
        }
    }

    let mut snapshots: Vec<Snapshot> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut expiry = vec![-1.0f64; occurrences.len()];
    let loop_start = if mode == "datapack" {
        song.meta.loop_start as u32
    } else {
        0
    };
    let phases = if looping { 3 } else { 1 };
    let wall_span = timing
        .wall
        .get(song.enda as usize + 1)
        .copied()
        .unwrap_or(0.0)
        - timing.wall.get(loop_start as usize).copied().unwrap_or(0.0);
    let mut datapack_pending: Vec<(usize, usize, i64)> = Vec::new(); // (快照下标, phase, grid)
    for phase in 0..phases {
        let start_tick = if phase == 0 { 0 } else { loop_start };
        let wall_offset = if phase == 0 {
            0.0
        } else {
            timing
                .wall
                .get(song.enda as usize + 1)
                .copied()
                .unwrap_or(0.0)
                + (phase - 1) as f64 * wall_span
        };
        let phase_grid: &Vec<i64> = if mode == "command" {
            &timing.grid
        } else if phase == 0 {
            &timing.server
        } else {
            &timing.loop_server
        };
        let mut pending_snapshots: Vec<usize> = Vec::new();
        let mut occupied_position = 0usize;
        while occupied_position < occupied_ticks.len()
            && occupied_ticks[occupied_position] < start_tick
        {
            occupied_position += 1;
        }
        while occupied_position < occupied_ticks.len() {
            let batch_key = phase_grid
                .get(occupied_ticks[occupied_position] as usize)
                .copied()
                .unwrap_or(0);
            let mut batch_end_position = occupied_position;
            while batch_end_position + 1 < occupied_ticks.len()
                && phase_grid
                    .get(occupied_ticks[batch_end_position + 1] as usize)
                    .copied()
                    .unwrap_or(i64::MIN)
                    == batch_key
            {
                batch_end_position += 1;
            }
            let mut batch_snapshots: Vec<usize> = Vec::new();
            for batch_position in occupied_position..=batch_end_position {
                let source_tick = occupied_ticks[batch_position];
                let now = wall_offset
                    + timing
                        .wall
                        .get(source_tick as usize)
                        .copied()
                        .unwrap_or(0.0)
                    - timing.wall.get(start_tick as usize).copied().unwrap_or(0.0);
                for i in (0..active.len()).rev() {
                    if expiry[active[i]] <= now {
                        active.remove(i);
                    }
                }
                let collast = song
                    .columns
                    .get(&source_tick)
                    .and_then(|c| c.last())
                    .unwrap_or(0);
                for layer_index in 0..=collast {
                    if let Some(play_index) = play_at.get(&(source_tick, layer_index)) {
                        let play_index = *play_index;
                        if !array_contains(&active, play_index) {
                            active.push(play_index);
                        }
                        expiry[play_index] = if occurrences[play_index].duration.is_infinite() {
                            f64::INFINITY
                        } else {
                            now + occurrences[play_index].duration
                        };
                    } else if let Some(stopper_index) = stopper_at.get(&(source_tick, layer_index))
                    {
                        let stopper_index = *stopper_index;
                        let before = active.clone();
                        let mut targets: Vec<usize> = Vec::new();
                        for occurrence_index in &before {
                            let occurrence = &occurrences[*occurrence_index];
                            if layer_targeted(
                                occurrence.layer + 1,
                                stoppers[stopper_index].layer_range,
                            ) {
                                targets.push(occurrence.index);
                                add_unique_target(
                                    &mut stoppers[stopper_index].targets,
                                    occurrence.index,
                                );
                            }
                        }
                        let snapshot_index = snapshots.len();
                        snapshots.push(Snapshot {
                            stopper: stopper_index,
                            active: before,
                            targets: targets.clone(),
                            survivors: Vec::new(),
                            absolute_tick: source_tick
                                + phase as u32 * (song.enda + 1 - loop_start),
                            output_grid: batch_key + stoppers[stopper_index].grid_delay as i64,
                        });
                        batch_snapshots.push(snapshot_index);
                        stoppers[stopper_index].snapshots.push(snapshot_index);
                        for t in &targets {
                            array_remove_value(&mut active, *t);
                        }
                    }
                }
            }
            if mode == "command" {
                // 被新延迟的消音器锁定的声音必须活过本批，避免被这里发出的
                // 其他 stopsound 误伤。
                let mut physical_survivors = active.clone();
                for snapshot_index in &batch_snapshots {
                    if snapshots[*snapshot_index].output_grid > batch_key {
                        for target in &snapshots[*snapshot_index].targets {
                            if !array_contains(&physical_survivors, *target) {
                                physical_survivors.push(*target);
                            }
                        }
                    }
                }
                let mut next_pending: Vec<usize> = Vec::new();
                for snapshot_index in pending_snapshots.iter().copied().collect::<Vec<_>>() {
                    if snapshots[snapshot_index].output_grid <= batch_key {
                        set_snapshot_survivors(&mut snapshots, snapshot_index, &physical_survivors);
                    } else {
                        next_pending.push(snapshot_index);
                    }
                }
                for snapshot_index in batch_snapshots.iter().copied().collect::<Vec<_>>() {
                    if snapshots[snapshot_index].output_grid <= batch_key {
                        set_snapshot_survivors(&mut snapshots, snapshot_index, &physical_survivors);
                    } else {
                        next_pending.push(snapshot_index);
                    }
                }
                pending_snapshots = next_pending;

                // 延迟命令落进空的网格步时，后面没有行能跟它冲突，直接用当前
                // 预期幸存者结算，不等下一批。
                let next_occupied_position = batch_end_position + 1;
                let next_batch_key = if next_occupied_position < occupied_ticks.len() {
                    timing
                        .grid
                        .get(occupied_ticks[next_occupied_position] as usize)
                        .copied()
                        .unwrap_or(i64::MAX)
                } else {
                    i64::MAX
                };
                if next_batch_key > batch_key + 1 {
                    let current_active = active.clone();
                    for snapshot_index in pending_snapshots.iter().copied().collect::<Vec<_>>() {
                        set_snapshot_survivors(&mut snapshots, snapshot_index, &current_active);
                    }
                    pending_snapshots = Vec::new();
                }
            } else {
                // 排定的 1t stop 在下一个服务端边界运行。保守地保护原批的所有
                // 幸存声音，以及下一服务端桶派发的所有声音。
                for pending_index in 0..datapack_pending.len() {
                    let (snapshot_index, entry_phase, entry_grid) = datapack_pending[pending_index];
                    let immediate = (entry_phase == phase && entry_grid == batch_key)
                        || (entry_phase + 1 == phase && batch_key == 1);
                    if immediate {
                        add_snapshot_survivors(&mut snapshots, snapshot_index, &active);
                    }
                }
                datapack_pending = Vec::new();
                for snapshot_index in batch_snapshots.iter().copied().collect::<Vec<_>>() {
                    if stoppers[snapshots[snapshot_index].stopper].grid_delay > 0 {
                        let current_active = active.clone();
                        set_snapshot_survivors(&mut snapshots, snapshot_index, &current_active);
                        datapack_pending.push((snapshot_index, phase, batch_key + 1));
                    }
                }
            }
            occupied_position = batch_end_position + 1;
        }
        if mode == "command" {
            let current_active = active.clone();
            for snapshot_index in pending_snapshots.iter().copied().collect::<Vec<_>>() {
                set_snapshot_survivors(&mut snapshots, snapshot_index, &current_active);
            }
        }
    }

    // 消音器在每次循环都发出同一组静态事件/声源对。冲突要对它在任何模拟轮里
    // 可能有的目标并集构建，包括首轮与稳态循环的差异。
    let mut risks: Vec<Risk> = Vec::new();
    let mut risk_map: HashSet<(usize, usize)> = HashSet::new();
    for stopper_index in 0..stoppers.len() {
        let stopper = &stoppers[stopper_index];
        for snapshot_index in &stopper.snapshots {
            let snapshot = &snapshots[*snapshot_index];
            let possible_victims: &[usize] = if mode == "command" || stopper.grid_delay > 0 {
                &snapshot.survivors
            } else {
                &snapshot.active
            };
            for target in &stopper.targets {
                for victim in possible_victims {
                    if mode == "datapack" && snapshot.targets.contains(victim) {
                        continue;
                    }
                    if occurrences[*target].event == occurrences[*victim].event {
                        add_risk(
                            &mut risks,
                            &mut risk_map,
                            *target,
                            *victim,
                            stopper_index,
                            song.enda as i64 + 1 - stopper.tick as i64,
                        );
                    }
                }
            }
        }
    }

    let assignment = assign_colors(occurrences.len(), &risks, palette.len(), full_search);
    let mut rows: Vec<Row> = Vec::new();
    let grid_len = timing.grid.get(song.enda as usize).copied().unwrap_or(0) + 2;
    let mut rows_by_tick: Vec<Vec<usize>> = vec![Vec::new(); song.enda as usize + 1];
    let mut rows_by_grid: Vec<Vec<usize>> = vec![Vec::new(); grid_len.max(0) as usize];
    let target_selector = if mode == "command" || nearby {
        "@a"
    } else {
        "@s"
    };
    for tick in 0..=song.enda {
        let column = match song.columns.get(&tick) {
            Some(c) if c.amount() > 0 => c,
            _ => continue,
        };
        let collast = column.last().unwrap_or(0);
        for layer_index in 0..=collast {
            if let Some(play_index) = play_at.get(&(tick, layer_index)) {
                let play_index = *play_index;
                let occurrence = &occurrences[play_index];
                let source = palette[assignment.colors[play_index].max(0) as usize].to_string();
                let command = if mode == "command" {
                    format!(
                        "playsound {} {} @a ~ ~ ~ 3 {}",
                        occurrence.event,
                        source,
                        dat_pitch(occurrence.key)
                    )
                } else {
                    let layer = song.layers.get(occurrence.layer as usize);
                    let layervol = layer.map(|l| l.volume as f64).unwrap_or(100.0);
                    let layerstereo = layer.map(|l| l.stereo as f64).unwrap_or(100.0);
                    let vel = song
                        .get(tick, occurrence.layer)
                        .map(|n| n.velocity as f64)
                        .unwrap_or(100.0);
                    let pan = song
                        .get(tick, occurrence.layer)
                        .map(|n| n.panning as f64)
                        .unwrap_or(100.0);
                    let volume = layervol / 100.0 / 100.0 * vel;
                    let stereo = (layerstereo + pan) / 2.0;
                    let position = (stereo - 100.0).abs() / 100.0 * 2.0;
                    if nearby {
                        format!(
                            "execute at @s run playsound {} {} @a ~ ~ ~ {} {}",
                            occurrence.event,
                            source,
                            gml_num(radius),
                            dat_pitch(occurrence.key)
                        )
                    } else {
                        format!(
                            "playsound {} {} @s ^{} ^ ^ {} {} 1",
                            occurrence.event,
                            source,
                            gml_num(position),
                            gml_num(volume),
                            dat_pitch(occurrence.key)
                        )
                    }
                };
                let output_grid = timing.grid[tick as usize];
                let index = rows.len();
                rows_by_tick[tick as usize].push(index);
                if output_grid >= 0 && (output_grid as usize) < rows_by_grid.len() {
                    rows_by_grid[output_grid as usize].push(index);
                }
                rows.push(Row {
                    index,
                    kind: RowKind::Play,
                    tick,
                    grid: output_grid,
                    layer: occurrence.layer,
                    instrument: occurrence.instrument,
                    command,
                    event: occurrence.event.clone(),
                    source,
                    stopper_index: None,
                    delayed: false,
                    speed: 0,
                });
            } else if let Some(stopper_index) = stopper_at.get(&(tick, layer_index)) {
                let stopper_index = *stopper_index;
                let output_grid = timing.grid[tick as usize]
                    + if mode == "command" {
                        stoppers[stopper_index].grid_delay as i64
                    } else {
                        0
                    };
                let mut pairs: HashSet<String> = HashSet::new();
                let mut pair_order: Vec<String> = Vec::new();
                for target in &stoppers[stopper_index].targets {
                    let source = palette[assignment.colors[*target].max(0) as usize].to_string();
                    let pair = format!("{}|{}", occurrences[*target].event, source);
                    if pairs.insert(pair.clone()) {
                        pair_order.push(pair);
                        let command = format!(
                            "stopsound {} {} {}",
                            target_selector, source, occurrences[*target].event
                        );
                        let index = rows.len();
                        rows_by_tick[tick as usize].push(index);
                        if output_grid >= 0 && (output_grid as usize) < rows_by_grid.len() {
                            rows_by_grid[output_grid as usize].push(index);
                        }
                        rows.push(Row {
                            index,
                            kind: RowKind::Stop,
                            tick,
                            grid: output_grid,
                            layer: stoppers[stopper_index].layer,
                            instrument: stoppers[stopper_index].instrument,
                            command,
                            event: occurrences[*target].event.clone(),
                            source,
                            stopper_index: Some(stopper_index),
                            delayed: mode == "datapack" && stoppers[stopper_index].grid_delay > 0,
                            speed: 0,
                        });
                    }
                }
            } else if mode == "datapack" && column.notes.contains_key(&layer_index) {
                let note = &column.notes[&layer_index];
                let name = song.instrument_name(note.instrument);
                if name == TEMPO_CHANGER_NAME
                    && (include_locked
                        || !locked.get(layer_index as usize).copied().unwrap_or(false))
                {
                    let index = rows.len();
                    rows_by_tick[tick as usize].push(index);
                    rows.push(Row {
                        index,
                        kind: RowKind::Tempo,
                        tick,
                        grid: tick as i64,
                        layer: layer_index,
                        instrument: note.instrument as usize,
                        command: String::new(),
                        event: String::new(),
                        source: String::new(),
                        stopper_index: None,
                        delayed: false,
                        speed: snapped_speed(tempo_from_note(song, tick, layer_index)),
                    });
                }
            }
        }
    }

    let mut slots_by_grid: Vec<Vec<usize>> = Vec::with_capacity(rows_by_grid.len());
    let mut maximum_slots = 0usize;
    for grid_tick in 0..rows_by_grid.len() {
        let grid_rows = &rows_by_grid[grid_tick];
        if compress_layers {
            slots_by_grid.push(grid_rows.clone());
        } else {
            let mut base_count = 0usize;
            for row_index in grid_rows {
                base_count = base_count.max(rows[*row_index].layer as usize + 1);
            }
            let mut slots = vec![usize::MAX; base_count];
            let mut overflow: Vec<usize> = Vec::new();
            for row_index in grid_rows {
                let layer = rows[*row_index].layer as usize;
                if slots[layer] == usize::MAX {
                    slots[layer] = *row_index;
                } else {
                    overflow.push(*row_index);
                }
            }
            slots.extend(overflow);
            slots_by_grid.push(slots);
        }
        maximum_slots = maximum_slots.max(slots_by_grid[grid_tick].len());
    }
    let mut used_sources = 0usize;
    for color in &assignment.colors {
        used_sources = used_sources.max((*color).max(0) as usize + 1);
    }
    let mut stopper_commands = 0usize;
    let mut last_command_grid = 0i64;
    for row in &rows {
        if row.kind == RowKind::Stop {
            stopper_commands += 1;
        }
        if row.kind != RowKind::Tempo {
            last_command_grid = last_command_grid.max(row.grid);
        }
    }
    let mut ignored_stoppers = 0usize;
    for stopper in &stoppers {
        if stopper.targets.is_empty() {
            ignored_stoppers += 1;
        }
    }
    let last_grid = timing.grid.get(song.enda as usize).copied().unwrap_or(0);
    Plan {
        mode: if mode == "datapack" {
            "datapack"
        } else {
            "command"
        },
        palette: palette.iter().map(|s| s.to_string()).collect(),
        occurrences,
        stoppers,
        snapshots,
        rows,
        risks,
        colors: assignment.colors,
        rows_by_tick,
        rows_by_grid,
        slots_by_grid,
        timing,
        last_grid,
        last_command_grid,
        maximum_slots,
        exact: assignment.exact,
        required_sources: assignment.required,
        minimum_proven: assignment.minimum_proven,
        search_limited: assignment.search_limited,
        cutoff_proven: assignment.cutoff_proven,
        used_sources,
        cutoff_count: assignment.cutoffs,
        lost_ticks: assignment.lost_ticks,
        stopper_commands,
        ignored_stoppers,
        omitted_count: 0,
        delayed_stoppers: delayed_stopper_count,
        track_rows_by_grid: Vec::new(),
        track_maximum_slots: 0,
    }
}

/// 原版 `build_plan` 的返回结构。
#[derive(Clone)]
pub struct Plan {
    pub mode: &'static str,
    pub palette: Vec<String>,
    pub occurrences: Vec<Occurrence>,
    pub stoppers: Vec<Stopper>,
    pub snapshots: Vec<Snapshot>,
    pub rows: Vec<Row>,
    pub risks: Vec<Risk>,
    pub colors: Vec<i32>,
    pub rows_by_tick: Vec<Vec<usize>>,
    pub rows_by_grid: Vec<Vec<usize>>,
    pub slots_by_grid: Vec<Vec<usize>>,
    pub timing: Timing,
    pub last_grid: i64,
    pub last_command_grid: i64,
    pub maximum_slots: usize,
    pub exact: bool,
    pub required_sources: usize,
    pub minimum_proven: bool,
    pub search_limited: bool,
    pub cutoff_proven: bool,
    pub used_sources: usize,
    pub cutoff_count: usize,
    pub lost_ticks: i64,
    pub stopper_commands: usize,
    pub ignored_stoppers: usize,
    /// 原版默认 0，由上层导出器在裁剪音符时累加。
    pub omitted_count: usize,
    pub delayed_stoppers: usize,
    /// `prepare_track_plan` 填充：每个网格步的 9 条车道（行下标）。
    pub track_rows_by_grid: Vec<[Vec<usize>; 9]>,
    pub track_maximum_slots: usize,
}

/// 原版 `minecraft_export_plan_repeaters(plan)`。
pub fn plan_repeaters(plan: &Plan, tempo_mode: u8) -> usize {
    let mut rep = 0usize;
    let mut repeaters = 0usize;
    let step = match tempo_mode {
        0 => 1,
        1 => 2,
        _ => 4,
    };
    for grid_tick in 0..=plan.last_command_grid.max(0) as usize {
        rep += step;
        if !plan.rows_by_grid[grid_tick].is_empty() || rep >= 4 {
            repeaters += 1;
            rep = 0;
        }
    }
    repeaters
}

/// 原版 `minecraft_export_track_channel(row, song)`：直轨的 9 条车道判定。
pub fn track_channel(row: &Row, song: &Song) -> usize {
    if row.kind != RowKind::Play {
        return 4;
    }
    let layer_index = row.layer as usize;
    let layer = song.layers.get(layer_index);
    let layerstereo = layer.map(|l| l.stereo as f64).unwrap_or(100.0);
    let note = song.get(row.tick, row.layer);
    let song_pan = note.map(|n| n.panning as f64).unwrap_or(100.0);
    let song_vel = note.map(|n| n.velocity as f64).unwrap_or(100.0);
    let pan = if layerstereo == 100.0 {
        song_pan
    } else {
        (layerstereo + song_pan) / 2.0
    };
    let volume = layer.map(|l| l.volume as f64).unwrap_or(100.0) / 100.0 * song_vel;
    let pan_volume = (pan - 100.0) * volume + 100.0;
    if (((pan > 80.0 && pan < 120.0) || (pan > 0.0 && pan <= 80.0))
        && volume > 0.0
        && volume <= 0.2)
        || pan_volume >= 180.0
    {
        return 0;
    }
    if (((pan > 80.0 && pan < 120.0) || (pan > 0.0 && pan <= 80.0))
        && volume > 0.2
        && volume <= 0.4)
        || (pan_volume >= 160.0 && pan_volume < 180.0)
    {
        return 1;
    }
    if (((pan > 80.0 && pan < 120.0) || (pan > 0.0 && pan <= 80.0))
        && volume > 0.4
        && volume <= 0.6)
        || (pan_volume >= 140.0 && pan_volume < 160.0)
    {
        return 2;
    }
    if (((pan > 80.0 && pan < 120.0 && volume <= 0.8) || (pan > 0.0 && pan <= 80.0))
        && volume > 0.6)
        || (pan_volume >= 120.0 && pan_volume < 140.0)
    {
        return 3;
    }
    if pan > 80.0 && pan < 120.0 && volume > 0.8 {
        return 4;
    }
    if (((pan > 80.0 && pan < 120.0 && volume <= 0.8) || (pan > 120.0 && pan <= 200.0))
        && volume > 0.6)
        || (pan_volume > 60.0 && pan_volume <= 80.0)
    {
        return 5;
    }
    if (((pan > 80.0 && pan < 120.0) || (pan > 120.0 && pan <= 200.0))
        && volume > 0.4
        && volume <= 0.6)
        || (pan_volume > 40.0 && pan_volume <= 60.0)
    {
        return 6;
    }
    if (((pan > 80.0 && pan < 120.0) || (pan > 120.0 && pan <= 200.0))
        && volume > 0.2
        && volume <= 0.4)
        || (pan_volume > 20.0 && pan_volume <= 40.0)
    {
        return 7;
    }
    8
}

/// 原版 `minecraft_export_prepare_track_plan(plan)`。
pub fn prepare_track_plan(plan: &mut Plan, song: &Song) {
    if !plan.track_rows_by_grid.is_empty() {
        return;
    }
    let mut track_rows_by_grid: Vec<[Vec<usize>; 9]> =
        vec![Default::default(); plan.rows_by_grid.len()];
    let mut track_maximum_slots = 0usize;
    for (grid_tick, grid_rows) in plan.rows_by_grid.iter().enumerate() {
        let mut channels: [Vec<usize>; 9] = Default::default();
        for row_index in grid_rows {
            let channel = track_channel(&plan.rows[*row_index], song);
            channels[channel].push(*row_index);
        }
        for channel in &channels {
            track_maximum_slots = track_maximum_slots.max(channel.len());
        }
        track_rows_by_grid[grid_tick] = channels;
    }
    plan.track_maximum_slots = track_maximum_slots;
    plan.track_rows_by_grid = track_rows_by_grid;
}

/// 原版 `minecraft_export_summary(plan)`。
pub fn summary(plan: &Plan) -> String {
    let result_quality = if plan.exact {
        if plan.minimum_proven {
            "minimum sources proven"
        } else {
            "bounded source minimum"
        }
    } else if plan.cutoff_proven {
        "minimum cutoff count proven"
    } else {
        "bounded cutoff result"
    };
    format!(
        "{}/{} sources; {} stop commands; {} accidental cutoffs ({})",
        plan.used_sources,
        plan.palette.len(),
        plan.stopper_commands,
        plan.cutoff_count,
        result_quality
    )
}

/// 原版 `minecraft_export_log_report(plan, preferred_source, export_kind)`。
/// 返回一行一条的日志文本，交由调用方落盘或打印。
pub fn log_report(plan: &Plan, preferred_source: &str, export_kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("Minecraft sound export report: {export_kind}"));
    out.push(format!("Minecraft sound export summary: {}", summary(plan)));
    out.push(format!(
        "Minecraft sound export preferred source: {preferred_source}"
    ));
    out.push(format!(
        "Minecraft sound export ignored no-op Sound Stoppers: {}",
        plan.ignored_stoppers
    ));
    out.push(format!(
        "Minecraft sound export Sound Stoppers delayed one grid tick: {}",
        plan.delayed_stoppers
    ));
    if plan.search_limited {
        out.push(
            "Minecraft sound export source optimization reached its safety limit; the exported assignment remains valid but its reported minimum may be bounded.".to_string(),
        );
    }
    let mut seen_victims: HashSet<usize> = HashSet::new();
    for risk in &plan.risks {
        if plan.colors[risk.target] != plan.colors[risk.victim] {
            continue;
        }
        if !seen_victims.insert(risk.victim) {
            continue;
        }
        let victim = &plan.occurrences[risk.victim];
        let stopper = &plan.stoppers[risk.stopper];
        let detail = format!(
            "stopper tick {} layer {}; victim tick {} layer {}; {}",
            stopper.tick,
            stopper.layer + 1,
            victim.tick,
            victim.layer + 1,
            victim.event
        );
        out.push(format!(
            "Minecraft sound export accidental cutoff: {detail}"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::{CustomInstrument, Layer, Note, Song};

    fn no_mappings(_: usize) -> Option<String> {
        None
    }
    fn no_durations(_: usize) -> Option<f64> {
        None
    }

    /// 每个音符的默认力度/声像/微调与原版一致（100/100/0）。
    fn song_with(notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::new();
        let max_layer = notes.iter().map(|n| n.1).max().unwrap_or(0) as usize;
        s.layers = vec![Layer::default(); max_layer + 1];
        for (t, l, ins, key) in notes {
            s.set(*t, *l, Note::new(*ins, *key));
        }
        s.recalculate();
        s
    }

    fn stopper_instrument() -> CustomInstrument {
        CustomInstrument {
            name: SOUND_STOPPER_NAME.to_string(),
            filename: String::new(),
            key: 45,
            press: true,
        }
    }

    fn tempo_changer_instrument() -> CustomInstrument {
        CustomInstrument {
            name: TEMPO_CHANGER_NAME.to_string(),
            filename: String::new(),
            key: 45,
            press: true,
        }
    }

    /// 起始层 2（pitch=2）、终止层 2（pan=102/vel=100 → short 2）的消音器音符。
    fn stopper_note() -> Note {
        let mut note = Note::new(20, 45);
        note.pitch = 2;
        note.panning = 102;
        note
    }

    fn command_plan(song: &Song) -> Plan {
        build_plan(
            song,
            "command",
            "block",
            &default_allowed_sources(),
            false,
            true,
            true,
            false,
            10.0,
            false,
            true,
            true,
            0.0,
            &no_mappings,
            &no_durations,
        )
    }

    #[test]
    fn palette_orders_preferred_first_and_master_last() {
        let p = source_palette("block", &default_allowed_sources());
        assert_eq!(p[0], "block");
        assert_eq!(p.len(), 9);
        assert!(p.contains(&"record"));
        assert!(!p.contains(&"master"));
        let mut allowed = default_allowed_sources();
        allowed[3] = true;
        let p = source_palette("block", &allowed);
        assert_eq!(*p.last().unwrap(), "master");
        // Master 未勾选时即使被选为首选也退到 record。
        let p = source_palette("master", &default_allowed_sources());
        assert_eq!(p[0], "record");
        assert!(!p.contains(&"master"));
    }

    #[test]
    fn resource_location_sanitizes_namespace_and_path() {
        assert_eq!(resource_location("Harp!"), "minecraft:harp_");
        assert_eq!(resource_location("My:Song\\A"), "my:song/a");
        assert_eq!(resource_location("minecraft/Curry"), "minecraft:curry");
        assert_eq!(resource_location(""), "minecraft:unnamed_instrument");
        assert_eq!(resource_location(":x"), "minecraft:x");
    }

    #[test]
    fn builtin_notes_map_to_block_sounds() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let plan = command_plan(&song);
        assert_eq!(plan.rows.len(), 1);
        assert_eq!(
            plan.rows[0].command,
            "playsound minecraft:block.note_block.harp block @a ~ ~ ~ 3 1.000000"
        );
        assert_eq!(plan.rows[0].event, "minecraft:block.note_block.harp");
        assert!(plan.exact);
        assert_eq!(plan.used_sources, 1);
        assert_eq!(plan.slots_by_grid[0].len(), 1);
    }

    #[test]
    fn out_of_range_notes_follow_include_flag() {
        let song = song_with(&[(0, 0, 0, 30)]);
        let excluded = build_plan(
            &song,
            "command",
            "block",
            &default_allowed_sources(),
            false,
            false,
            true,
            false,
            10.0,
            false,
            true,
            true,
            0.0,
            &no_mappings,
            &no_durations,
        );
        assert!(excluded.rows.is_empty());
        let included = command_plan(&song);
        assert_eq!(
            included.rows[0].command,
            "playsound minecraft:block.note_block.harp_-1 block @a ~ ~ ~ 3 1.681793"
        );
    }

    #[test]
    fn placeholder_custom_instruments_stay_unmapped() {
        let mut song = song_with(&[(0, 0, 20, 45)]);
        song.custom_instruments.push(CustomInstrument {
            name: "Custom instrument #1".to_string(),
            filename: String::new(),
            key: 45,
            press: true,
        });
        song.recalculate();
        let plan = command_plan(&song);
        assert!(plan.rows.is_empty());

        let manual = |_: usize| Some("custom:gong".to_string());
        let plan = build_plan(
            &song,
            "command",
            "block",
            &default_allowed_sources(),
            false,
            true,
            true,
            false,
            10.0,
            false,
            true,
            true,
            0.0,
            &manual,
            &no_durations,
        );
        assert_eq!(
            plan.rows[0].command,
            "playsound custom:gong block @a ~ ~ ~ 3 1.000000"
        );

        // 非占位名按名字派生事件。
        let mut named = song_with(&[(0, 0, 20, 45)]);
        named.custom_instruments.push(CustomInstrument {
            name: "Curry".to_string(),
            filename: String::new(),
            key: 45,
            press: true,
        });
        named.recalculate();
        let plan = command_plan(&named);
        assert_eq!(
            plan.rows[0].command,
            "playsound minecraft:curry block @a ~ ~ ~ 3 1.000000"
        );
    }

    #[test]
    fn stopper_cuts_one_layer_and_costs_two_sources() {
        let mut song = song_with(&[(0, 0, 0, 45), (0, 1, 0, 45)]);
        song.custom_instruments.push(stopper_instrument());
        song.set(2, 1, stopper_note());
        song.recalculate();
        let plan = command_plan(&song);
        // 两个同事件发声被同一消音器区间覆盖，需要 2 个声源避免互相掐掉。
        assert_eq!(plan.rows.len(), 3);
        let sources: Vec<&str> = plan.rows.iter().map(|r| r.source.as_str()).collect();
        assert_ne!(sources[0], sources[1]);
        assert_eq!(
            plan.rows[2].command,
            format!(
                "stopsound @a {} minecraft:block.note_block.harp",
                sources[1]
            )
        );
        assert_eq!(plan.rows[2].grid, 2);
        assert_eq!(plan.used_sources, 2);
        assert_eq!(plan.stopper_commands, 1);
        assert!(plan.exact);
        assert!(plan.minimum_proven);
        assert_eq!(plan.maximum_slots, 2);
    }

    #[test]
    fn empty_stopper_range_hits_nothing() {
        let mut song = song_with(&[(0, 0, 0, 45), (0, 2, 0, 45)]);
        song.custom_instruments.push(stopper_instrument());
        song.set(2, 1, stopper_note());
        song.recalculate();
        let plan = command_plan(&song);
        // 区间 [2,2] 只覆盖 1 基层 2，层 1/3 都不受影响。
        assert_eq!(plan.rows.len(), 2);
        assert_eq!(plan.stopper_commands, 0);
        assert_eq!(plan.ignored_stoppers, 1);
        assert_eq!(plan.used_sources, 1);
    }

    #[test]
    fn expired_voices_make_later_stoppers_noops() {
        let mut song = song_with(&[(0, 0, 0, 45)]);
        song.custom_instruments.push(stopper_instrument());
        song.set(10, 0, stopper_note());
        song.recalculate();
        let short = |_: usize| Some(0.1);
        let plan = build_plan(
            &song,
            "command",
            "block",
            &default_allowed_sources(),
            false,
            true,
            true,
            false,
            10.0,
            false,
            true,
            true,
            0.0,
            &no_mappings,
            &short,
        );
        assert_eq!(plan.rows.len(), 1);
        assert_eq!(plan.ignored_stoppers, 1);
    }

    #[test]
    fn snapping_delays_stopper_one_grid_step() {
        // tempo_grid 下 2.5 tps 网格把相邻两曲 tick 收进同一网格步。
        let mut song = song_with(&[(0, 1, 0, 45)]);
        song.custom_instruments.push(stopper_instrument());
        song.set(1, 1, stopper_note());
        song.recalculate();
        let plan = build_plan(
            &song,
            "command",
            "block",
            &default_allowed_sources(),
            false,
            true,
            true,
            true,
            2.5,
            false,
            true,
            true,
            0.0,
            &no_mappings,
            &no_durations,
        );
        assert_eq!(plan.timing.grid[1], 0);
        assert_eq!(plan.delayed_stoppers, 1);
        assert_eq!(plan.stoppers[0].grid_delay, 1);
        assert_eq!(plan.rows[0].grid, 0);
        assert_eq!(plan.rows[1].grid, 1);
        assert_eq!(plan.rows_by_grid[0].len(), 1);
        assert_eq!(plan.rows_by_grid[1].len(), 1);
    }

    #[test]
    fn tempo_changers_retime_snapped_grid() {
        let mut song = song_with(&[(0, 0, 0, 45), (2, 0, 0, 45)]);
        song.custom_instruments.push(tempo_changer_instrument());
        let mut changer = Note::new(20, 45);
        changer.pitch = 75; // floor(75)/15 = 5 t/s
        song.set(1, 1, changer);
        song.recalculate();
        let plan = build_plan(
            &song,
            "command",
            "block",
            &default_allowed_sources(),
            false,
            true,
            true,
            true,
            10.0,
            false,
            true,
            true,
            0.0,
            &no_mappings,
            &no_durations,
        );
        assert_eq!(plan.timing.grid[1], 1);
        // 0.1s + 4/20 = 0.3s → 网格 3。
        assert_eq!(plan.timing.grid[2], 3);
        assert_eq!(plan.rows[1].grid, 3);
    }

    #[test]
    fn datapack_rows_use_relative_listener_and_gml_numbers() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let plan = build_plan(
            &song,
            "datapack",
            "block",
            &default_allowed_sources(),
            false,
            true,
            false,
            false,
            10.0,
            false,
            true,
            true,
            4.0,
            &no_mappings,
            &no_durations,
        );
        assert_eq!(
            plan.rows[0].command,
            "playsound minecraft:block.note_block.harp block @s ^0 ^ ^ 1 1.000000 1"
        );
        let nearby = build_plan(
            &song,
            "datapack",
            "block",
            &default_allowed_sources(),
            false,
            true,
            true,
            false,
            10.0,
            false,
            true,
            true,
            4.0,
            &no_mappings,
            &no_durations,
        );
        assert_eq!(
            nearby.rows[0].command,
            "execute at @s run playsound minecraft:block.note_block.harp block @a ~ ~ ~ 4 1.000000"
        );
    }

    #[test]
    fn coloring_suite_matches_graph_facts() {
        let adj = vec![vec![1, 2], vec![0, 2], vec![0, 1]];
        let colors = greedy_colors(&[0, 1, 2], &adj, 3);
        assert_ne!(colors[0], colors[1]);
        assert_ne!(colors[0], colors[2]);
        assert_ne!(colors[1], colors[2]);
        let mut budget = Budget::new(true);
        let (found_two, _, limited) = k_color(&[0, 1, 2], &adj, 2, &mut budget);
        assert!(!found_two && !limited);
        let (found_three, colors3, _) = k_color(&[0, 1, 2], &adj, 3, &mut budget);
        assert!(found_three);
        assert_ne!(colors3[0], colors3[1]);
        // 路径图 0-1-2 连通；再加两个孤立点凑出不连通图。
        let path = vec![vec![1], vec![0, 2], vec![1]];
        assert_eq!(connected_components(&[0, 1, 2], &path).len(), 1);
        let disjoint = vec![vec![1], vec![0], Vec::<usize>::new(), Vec::<usize>::new()];
        assert_eq!(connected_components(&[0, 1, 2, 3], &disjoint).len(), 3);
        assert_eq!(greedy_clique_lower_bound(&[0, 1, 2], &adj), 3);
        assert_eq!(greedy_clique_lower_bound(&[0, 1, 2], &path), 2);
    }

    fn play_row(tick: u32, layer: u32) -> Row {
        Row {
            index: 0,
            kind: RowKind::Play,
            tick,
            grid: tick as i64,
            layer,
            instrument: 0,
            command: String::new(),
            event: String::new(),
            source: String::new(),
            stopper_index: None,
            delayed: false,
            speed: 0,
        }
    }

    #[test]
    fn track_channel_places_center_low_and_sides() {
        // 原版 volume = 层音量/100 * 音符力度，力度是原始 0-100 字节，
        // 所以车道判定里的 0.2/0.4/0.6/0.8 是「原始 0-100 量纲」上的阈值；
        // 右偏大音量走 pan_volume >= 180 进车道 0。忠实转录，不改原行为。
        let mut song = song_with(&[(0, 0, 0, 45), (0, 1, 0, 45), (0, 2, 0, 45)]);
        assert_eq!(track_channel(&play_row(0, 0), &song), 4);
        song.layers[0].volume = 1;
        let mut note = song.get(0, 0).unwrap();
        note.velocity = 10;
        song.set(0, 0, note);
        assert_eq!(track_channel(&play_row(0, 0), &song), 0);
        let mut note = song.get(0, 1).unwrap();
        note.panning = 50;
        song.set(0, 1, note);
        assert_eq!(track_channel(&play_row(0, 1), &song), 3);
        let mut note = song.get(0, 2).unwrap();
        note.panning = 150;
        song.set(0, 2, note);
        assert_eq!(track_channel(&play_row(0, 2), &song), 0);
        song.layers[0].volume = 1;
        let mut note = song.get(0, 0).unwrap();
        note.panning = 60;
        note.velocity = 50;
        song.set(0, 0, note);
        assert_eq!(track_channel(&play_row(0, 0), &song), 2);
    }

    #[test]
    fn non_play_rows_take_center_channel() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let stop = Row {
            kind: RowKind::Stop,
            ..play_row(0, 0)
        };
        assert_eq!(track_channel(&stop, &song), 4);
    }

    #[test]
    fn prepare_track_plan_buckets_by_channel() {
        let mut song = song_with(&[(0, 0, 0, 45), (0, 1, 0, 45), (0, 2, 0, 45)]);
        let mut note = song.get(0, 1).unwrap();
        note.panning = 50;
        song.set(0, 1, note);
        let mut note = song.get(0, 2).unwrap();
        note.panning = 150;
        song.set(0, 2, note);
        song.recalculate();
        let mut plan = command_plan(&song);
        prepare_track_plan(&mut plan, &song);
        // pan 100 → 车道 4；pan 50 → 车道 3；pan 150 → pan_volume ≥ 180 → 车道 0。
        assert_eq!(plan.track_rows_by_grid[0][4].len(), 1);
        assert_eq!(plan.track_rows_by_grid[0][3].len(), 1);
        assert_eq!(plan.track_rows_by_grid[0][0].len(), 1);
        assert_eq!(plan.track_maximum_slots, 1);
        // 重复调用幂等。
        prepare_track_plan(&mut plan, &song);
        assert_eq!(plan.track_rows_by_grid[0][4].len(), 1);
    }

    #[test]
    fn plan_repeaters_counts_per_tempo_mode() {
        let song = song_with(&[(0, 0, 0, 45), (3, 0, 0, 45)]);
        let plan = command_plan(&song);
        assert_eq!(plan.last_command_grid, 3);
        assert_eq!(plan_repeaters(&plan, 0), 2);
        assert_eq!(plan_repeaters(&plan, 2), 4);
    }

    #[test]
    fn summary_and_report_shape() {
        let song = song_with(&[(0, 0, 0, 45)]);
        let plan = command_plan(&song);
        assert_eq!(
            summary(&plan),
            "1/9 sources; 0 stop commands; 0 accidental cutoffs (minimum sources proven)"
        );
        let report = log_report(&plan, "block", "Track command-block structure");
        assert_eq!(
            report[0],
            "Minecraft sound export report: Track command-block structure"
        );
        assert!(report[2].contains("block"));
        assert_eq!(report.len(), 5);
    }
}
