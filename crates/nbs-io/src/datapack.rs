//! Minecraft 数据包导出。
//!
//! 逐条对齐原版的 `datapack_export.gml`（固定文件）、`dat_generate.gml`（notes/
//! 与二分 tree）、`dat_makefolders.gml`（目录结构）、`dat_getpath.gml` 与
//! `string_path.gml`（路径、名称/命名空间消毒，都是项目自带的脚本）、
//! `dat_pitch.gml`、`dat_instrument.gml`、`dat_reset.gml`（默认值）。
//! 命令文本按原版的字符串拼接逐字符生成，包括原版自身的不对称之处
//! （tree 的第二子区间用 `(max2+2)*80+160`、Fountain 的组队选择器少 1 等），
//! 目的是让原版能加载的包这里也能加载，而不是「看起来合理」。

use std::io::Write;
use std::path::Path;

use flate2::write::DeflateEncoder;
use flate2::Compression;
use nbs_core::model::Song;

use crate::schematic::{legacy_block_name, INSTRUMENT_BLOCKS};

/// 原版 `dat_source` 的 10 个取值（窗口里两列的顺序）。
pub const SOUND_SOURCES: [&str; 10] = [
    "ambient", "block", "hostile", "master", "music", "neutral", "player", "record", "voice",
    "weather",
];

/// 原版 `dat_instrument`：前 16 个内置音色对应的 `minecraft:block.note_block.*` 事件。
const NOTE_BLOCK_SOUNDS: [&str; 16] = [
    "harp",
    "bass",
    "basedrum",
    "snare",
    "hat",
    "guitar",
    "flute",
    "bell",
    "chime",
    "xylophone",
    "iron_xylophone",
    "cow_bell",
    "didgeridoo",
    "bit",
    "banjo",
    "pling",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisType {
    Arc,
    Fall,
    Rise,
    Bounce,
    PianoRoll,
    Fountain,
    RittaiOnkyou,
}

impl VisType {
    /// 原版窗口从上到下的顺序。
    pub const ALL: [VisType; 7] = [
        Self::Arc,
        Self::Fall,
        Self::Rise,
        Self::Bounce,
        Self::PianoRoll,
        Self::Fountain,
        Self::RittaiOnkyou,
    ];

    /// 原版内部与 settings.ini 里用的字符串。
    pub fn gml_name(self) -> &'static str {
        match self {
            Self::Arc => "Arc",
            Self::Fall => "Fall",
            Self::Rise => "Rise",
            Self::Bounce => "Bounce",
            Self::PianoRoll => "Piano Roll",
            Self::Fountain => "Fountain",
            Self::RittaiOnkyou => "Rittai Onkyou",
        }
    }

    pub fn from_gml_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.gml_name() == name)
    }
}

#[derive(Clone, Debug)]
pub struct DatapackOptions {
    /// `dat_name`：唯一名称，同时决定 objective 与默认命名空间。
    pub name: String,
    pub namespace: String,
    pub path: String,
    pub source: String,
    /// 0 = 1.13-1.20（pack_format 41、`functions`），1 = 1.21+（48、`function`）。
    pub mc_version: u8,
    pub use_zip: bool,
    pub include_locked: bool,
    pub include_out_of_range: bool,
    pub enable_radius: bool,
    /// 16..=100000，实际音量 `1 + (radius-16)*0.06`。
    pub radius: u32,
    pub enable_looping: bool,
    pub visualizer: bool,
    pub vis_type: VisType,
    pub glow: bool,
    pub y_val: f32,
    pub x_val: f32,
    pub z_val: f32,
}

impl Default for DatapackOptions {
    fn default() -> Self {
        // 与 dat_reset(0) 一致；loopstart 不属于 dat_*，原版用的是歌曲自身的循环起点。
        Self {
            name: String::new(),
            namespace: String::new(),
            path: String::new(),
            source: "record".into(),
            mc_version: 1,
            use_zip: true,
            include_locked: false,
            include_out_of_range: false,
            enable_radius: false,
            radius: 16,
            enable_looping: false,
            visualizer: false,
            vis_type: VisType::Arc,
            glow: true,
            y_val: 100.0,
            x_val: 0.0,
            z_val: 0.0,
        }
    }
}

impl DatapackOptions {
    /// 原版：`dat_radiusvalue = 1 + (dat_radius - 16) * 0.06`。
    pub fn radius_value(&self) -> f64 {
        1.0 + (self.radius.max(16) as f64 - 16.0) * 0.06
    }

    pub fn pack_format(&self) -> u32 {
        if self.mc_version == 0 {
            41
        } else {
            48
        }
    }

    pub fn function_registry(&self) -> &'static str {
        if self.mc_version == 0 {
            "functions"
        } else {
            "function"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// 包内相对路径，一律用 `/` 分隔。
    pub path: String,
    pub content: String,
}

pub struct Plan {
    pub files: Vec<GeneratedFile>,
    /// 原版 `functionpath`，例如 `nyan_cat:song/`；游戏内命令要拼在它后面。
    pub functionpath: String,
    pub objective: String,
}

/// 原版 `string_lettersdigits`。
fn letters_digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// 原版自带的 `string_path` 脚本（不是 GML 内建的那个）：小写、空格转下划线、
/// 只留字母数字与 `_`，因此斜杠等分隔符会被直接吃掉。
fn gml_string_path(s: &str) -> String {
    let lower = s.to_ascii_lowercase().replace(' ', "_");
    lower
        .chars()
        .filter(|c| *c == '_' || c.is_ascii_alphanumeric())
        .collect()
}

/// 原版 `dat_getpath`：小写、空格转下划线、只留字母数字与单个 `/`，去掉首尾 `/`。
pub fn sanitize_path(s: &str) -> String {
    let lower = s.to_ascii_lowercase().replace(' ', "_");
    let mut out = String::new();
    let mut prev = '\0';
    for (i, c) in lower.chars().enumerate() {
        let keep = c == '_'
            || (c == '/' && prev != '/' && i > 0)
            || c.is_ascii_alphanumeric();
        if keep {
            out.push(c);
        }
        prev = c;
    }
    while out.ends_with('/') {
        out.pop();
    }
    out
}

/// 原版 `dat_preview`：导出窗口里实时显示的播放命令。
/// 原版最后套了一层 `string_truncate(preview, 150, true)`，那是按像素宽度给固定尺寸
/// 窗口做的省略号截断（`...` 在开头），文本内容本身与这里一致，布局交给 egui。
pub fn command_preview(name: &str, namespace: &str, path: &str) -> String {
    if letters_digits(name).is_empty() {
        return "<enter name>".into();
    }
    if letters_digits(namespace).is_empty() {
        return format!("/function {}:play", gml_string_path(name));
    }
    if letters_digits(path).is_empty() {
        return format!(
            "/function {}:{}/play",
            gml_string_path(namespace),
            gml_string_path(name)
        );
    }
    format!(
        "/function {}:{}/{}/play",
        gml_string_path(namespace),
        sanitize_path(path),
        gml_string_path(name)
    )
}

/// 原版 `dat_pitch`：把音符折进两个八度内，再 `0.5 * 2^(key/12)`，固定 6 位小数。
pub fn dat_pitch(key: f64) -> String {
    let mut k = key;
    if k < 33.0 {
        k -= 9.0;
    } else if k > 57.0 {
        k -= 57.0;
    } else {
        k -= 33.0;
    }
    format!("{:.6}", 0.5 * 2f64.powf(k / 12.0))
}

/// GML `string()` 对实数的输出：整数不带小数点，非整数按 6 位有效数字并去掉尾零。
fn gml_num(x: f64) -> String {
    if !x.is_finite() {
        return "0".into();
    }
    if x == x.trunc() && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let before = x.abs().log10().floor() as i32 + 1;
    let decimals = (6 - before).clamp(0, 9) as usize;
    let mut s = format!("{:.*}", decimals, x);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// 原版 `dat_instrument`。
fn sound_event(song: &Song, index: usize) -> String {
    if index < 16 {
        format!("minecraft:block.note_block.{}", NOTE_BLOCK_SOUNDS[index])
    } else {
        let name = song
            .custom_instruments
            .get(index - 16)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        format!("minecraft:{name}")
    }
}

/// 原版 `block_get_namespaced_id(sch_exp_ins_block[i])`（不含 `minecraft:` 前缀）。
fn visual_block(index: usize) -> &'static str {
    legacy_block_name(INSTRUMENT_BLOCKS.get(index).copied().unwrap_or(1))
}

/// 生成整个包的文本文件。`output_stem` 是原版 `filename_name(fn)`，
/// 只出现在 uninstall.mcfunction 的 `datapack disable` 与 tellraw 里。
pub fn generate(song: &Song, opts: &DatapackOptions, output_stem: &str) -> Plan {
    let name = gml_string_path(&opts.name);
    let mut namespace = gml_string_path(&opts.namespace);
    let mut path = sanitize_path(&opts.path);
    let objective = format!("nbs_{}", letters_digits(&opts.name).chars().take(10).collect::<String>());
    let tag = objective.clone();

    // 原版三分支：命名空间为空时用歌名当命名空间且不建子路径；
    // 路径为空时用歌名当路径；否则把歌名追加到路径后面。
    let functionpath = if namespace.is_empty() {
        path = String::new();
        namespace = name.clone();
        format!("{namespace}:")
    } else if path.is_empty() {
        path = name.clone();
        format!("{namespace}:{path}/")
    } else {
        path = format!("{path}/{name}");
        format!("{namespace}:{path}/")
    };

    let playspeed = ((song.meta.tempo * 4.0).round() as i64).min(120);
    let enda = song.enda as i64;
    let rootfunction = format!(
        "0_{}",
        gml_num(2f64.powf((enda as f64).log2().floor() + 1.0) - 1.0)
    );
    let add_teams = opts.visualizer && opts.glow;
    let registry = opts.function_registry();

    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut push = |path: String, content: String| files.push(GeneratedFile { path, content });

    // ---- pack.mcmeta（原版直接字符串拼接，dat_name 原样写入）
    push(
        "pack.mcmeta".into(),
        format!(
            "{{\n\t\"pack\": {{\n\t\t\"pack_format\": {},\n\t\t\"description\": \"{}\\nMade with Note Block Studio\"\n\t}}\n}}",
            opts.pack_format(),
            opts.name
        ),
    );

    // ---- minecraft tags
    push(
        format!("data/minecraft/tags/{registry}/load.json"),
        format!("{{\"values\": [\"{functionpath}load\"]}}"),
    );
    push(
        format!("data/minecraft/tags/{registry}/tick.json"),
        format!("{{\"values\": [\"{functionpath}tick\"]}}"),
    );

    // 原版 `dat_makefolders`：函数文件写在 path 的每一段构成的子目录里。
    let mut fdir = format!("data/{namespace}/{registry}");
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        fdir.push('/');
        fdir.push_str(seg);
    }

    // ---- load / tick / play / pause / stop
    push(
        format!("{fdir}/load.mcfunction"),
        format!(
            "scoreboard objectives add {objective} dummy\nscoreboard objectives add {objective}_t dummy\nscoreboard players set speed {objective} {playspeed}"
        ),
    );
    let tree_ref = if opts.enable_radius {
        format!("execute as @a[tag={tag}] run function {functionpath}tree/{rootfunction}")
    } else {
        format!("execute as @a[tag={tag}] at @s run function {functionpath}tree/{rootfunction}")
    };
    push(
        format!("{fdir}/tick.mcfunction"),
        format!(
            "execute as @a[tag={tag}] run scoreboard players operation @s {objective} += speed {objective}\n{tree_ref}"
        ),
    );
    let mut play = format!("tag @s add {tag}\nscoreboard players set @s {objective}_t -1");
    if add_teams {
        play.push_str(&format!("\nfunction {functionpath}add_teams"));
    }
    push(format!("{fdir}/play.mcfunction"), play);
    push(
        format!("{fdir}/pause.mcfunction"),
        format!("tag @s remove {tag}"),
    );
    let mut stop = format!(
        "tag @s remove {tag}\nscoreboard players reset @s {objective}\nscoreboard players reset @s {objective}_t"
    );
    if add_teams {
        stop.push_str(&format!("\nfunction {functionpath}remove_teams"));
    }
    push(format!("{fdir}/stop.mcfunction"), stop);

    // ---- uninstall
    let mut uninstall = format!(
        "tag @e remove {tag}\nscoreboard objectives remove {objective}\nscoreboard objectives remove {objective}_t\n"
    );
    if add_teams {
        uninstall.push_str("kill @e[type=falling_block,tag=nbs]\n");
        uninstall.push_str(&format!("function {functionpath}remove_teams\n"));
    }
    if opts.use_zip {
        uninstall.push_str(&format!("datapack disable \"file/{output_stem}\"\n"));
    } else {
        uninstall.push_str(&format!("datapack disable \"{output_stem}\"\n"));
    }
    uninstall.push_str(&format!(
        "tellraw @s [\"\",{{\"text\":\"[NBS] \",\"color\":\"gold\",\"bold\":true}},{{\"text\":\"Data pack \",\"color\":\"yellow\"}},{{\"text\":\"{output_stem}\",\"color\":\"gold\",\"underlined\":true}},{{\"text\":\" uninstalled successfully. You may now remove it from your data pack folder.\",\"color\":\"yellow\"}}]"
    ));
    push(format!("{fdir}/uninstall.mcfunction"), uninstall);

    if add_teams {
        // 原版写死 17 个队：先 17 行 team add，再 17 行 team modify，颜色表照抄。
        let colors = [
            "dark_gray", "red", "black", "yellow", "light_purple", "green", "dark_red",
            "dark_aqua", "dark_green", "blue", "aqua", "dark_blue", "dark_gray", "dark_green",
            "gray", "gold", "white",
        ];
        let mut add = String::new();
        let mut rem = String::new();
        for i in 0..colors.len() {
            add.push_str(&format!("team add nbs_{}\n", i + 1));
            rem.push_str(&format!("team remove nbs_{}\n", i + 1));
        }
        for (i, color) in colors.iter().enumerate() {
            add.push_str(&format!("team modify nbs_{} color {color}", i + 1));
            if i + 1 < colors.len() {
                add.push('\n');
            }
        }
        rem.pop(); // 原版最后一行不带 br
        push(format!("{fdir}/add_teams.mcfunction"), add);
        push(format!("{fdir}/remove_teams.mcfunction"), rem);
    }

    // ---- notes
    let locked = song.locked_layers();
    let mut by_tick: std::collections::BTreeMap<i64, Vec<(usize, nbs_core::model::Note)>> =
        std::collections::BTreeMap::new();
    for (tick, layer, note) in song.iter_notes() {
        by_tick.entry(tick as i64).or_default().push((layer as usize, note));
    }
    let has_notes = |tick: i64| -> bool {
        tick >= 0 && tick <= enda && by_tick.get(&tick).map(|v| !v.is_empty()).unwrap_or(false)
    };

    for tick in 0..=enda {
        let Some(cells) = by_tick.get(&tick) else { continue };
        let mut str = String::new();
        for (layer, note) in cells {
            if locked.get(*layer).copied().unwrap_or(false) && !opts.include_locked {
                continue;
            }
            let key = note.key as f64 + note.pitch as f64 / 100.0;
            let in_range = (33.0..=57.0).contains(&key);
            let extra = opts.include_out_of_range && (9.0..=81.0).contains(&key);
            if !(in_range || extra) {
                continue;
            }
            let index = note.instrument as usize;
            let mut soundname = sound_event(song, index);
            if key < 33.0 {
                soundname.push_str("_-1");
            } else if key > 57.0 {
                soundname.push_str("_1");
            }
            let pitch = dat_pitch(key);
            let layer_obj = song.layers.get(*layer);
            let vol = layer_obj.map(|l| l.volume as f64).unwrap_or(100.0);
            let stereo = layer_obj.map(|l| l.stereo as f64).unwrap_or(100.0);
            // 原版：layervol/100/100 * velocity
            let blockvolume = vol / 100.0 / 100.0 * note.velocity as f64;
            let s = (stereo + note.panning as f64) / 2.0;
            // 原版三个分支合起来就是 (100 - s) / 100
            let blockposition = if s > 100.0 {
                (s - 100.0) / -100.0
            } else if s == 100.0 {
                0.0
            } else {
                ((s - 100.0) * -1.0) / 100.0
            };

            if opts.enable_radius {
                str.push_str(&format!(
                    "execute at @s run playsound {soundname} {} @a ~ ~ ~ {} {pitch}\n",
                    opts.source,
                    gml_num(opts.radius_value())
                ));
            } else {
                str.push_str(&format!(
                    "playsound {soundname} {} @s ^{} ^ ^ {} {pitch} 1\n",
                    opts.source,
                    gml_num(blockposition * 2.0),
                    gml_num(blockvolume)
                ));
            }

            if opts.visualizer {
                str.push_str(&visualizer_line(opts, index, key, blockposition, blockvolume));
            }
        }
        if tick < enda {
            str.push_str(&format!("scoreboard players set @s {objective}_t {tick}"));
        } else if opts.enable_looping {
            let loop_start = song.meta.loop_start as f64;
            str.push_str(&format!(
                "scoreboard players set @s {objective} {}\nscoreboard players set @s {objective}_t {}",
                gml_num(loop_start * 80.0),
                gml_num(loop_start - 1.0)
            ));
        } else {
            str.push_str(&format!("function {functionpath}stop"));
        }
        push(format!("{fdir}/notes/{tick}.mcfunction"), str);
    }

    // ---- 二分查找树
    let steps_f = (enda as f64).log2().floor() + 1.0;
    let steps = if steps_f.is_finite() && steps_f > 0.0 { steps_f as i64 } else { 0 };
    let pow = 2i64.pow(steps.max(0) as u32);
    for step in 0..steps {
        let searchrange = pow / 2i64.pow(step as u32);
        let segments = pow / searchrange;
        let half = searchrange / 2;
        for segment in 0..segments {
            let mut str = String::new();
            let lower = searchrange * segment;
            let min1 = lower;
            let max1 = lower + half - 1;
            let min2 = lower + half;
            let max2 = lower + searchrange - 1;
            if min1 > enda {
                break;
            }
            if step == steps - 1 {
                if has_notes(min1) {
                    str.push_str(&format!(
                        "execute as @s[scores={{{objective}={}..{},{objective}_t=..{}}}] run function {functionpath}notes/{min1}\n",
                        min1 * 80,
                        (max1 + 1) * 80 + 160,
                        min1 - 1
                    ));
                }
                if min2 <= enda && has_notes(min2) {
                    str.push_str(&format!(
                        "execute as @s[scores={{{objective}={}..{},{objective}_t=..{}}}] run function {functionpath}notes/{min2}\n",
                        min2 * 80,
                        (max2 + 1) * 80 + 160,
                        min2 - 1
                    ));
                }
            } else {
                for i in min1..=max1.min(enda) {
                    if has_notes(i) {
                        str.push_str(&format!(
                            "execute as @s[scores={{{objective}={}..{}}}] run function {functionpath}tree/{min1}_{max1}\n",
                            min1 * 80,
                            (max1 + 1) * 80 + 160
                        ));
                        break;
                    }
                }
                for i in min2..=max2.min(enda) {
                    if has_notes(i) {
                        str.push_str(&format!(
                            "execute as @s[scores={{{objective}={}..{}}}] run function {functionpath}tree/{min2}_{max2}\n",
                            min2 * 80,
                            (max2 + 2) * 80 + 160
                        ));
                        break;
                    }
                }
            }
            if !str.is_empty() {
                push(format!("{fdir}/tree/{min1}_{max2}.mcfunction"), str);
            }
        }
    }

    Plan { files, functionpath, objective }
}

/// 原版 `dat_generate` 里效果器的 summon 行，逐分支照抄（含原版自身的坐标/选择器差异）。
fn visualizer_line(
    opts: &DatapackOptions,
    index: usize,
    key: f64,
    blockposition: f64,
    blockvolume: f64,
) -> String {
    let team_number = index + 1;
    let block_id = visual_block(index);
    let y = gml_num(opts.y_val as f64);
    let x = opts.x_val as f64;
    let z = opts.z_val as f64;
    let glowing = if opts.glow {
        format!("Tags:[\"nbs\",\"nbs_{team_number}\"],Glowing:1,")
    } else {
        String::new()
    };
    let state = format!("{{BlockState:{{Name:\"minecraft:{block_id}\"}},");
    let mut s = String::new();
    match opts.vis_type {
        VisType::Arc => {
            s.push_str(&format!(
                "summon minecraft:falling_block {} {} {} {state}{}",
                gml_num((key - 45.0) * -1.0 + x),
                y,
                gml_num((index as f64 * 2.0) + z),
                glowing
            ));
            s.push_str("Time:-120,DropItem:0,Motion:[0.0d,1.0d,1.0d]}\n");
            if opts.glow {
                s.push_str(&format!("team join nbs_{team_number} @e[tag=nbs_{team_number}]\n"));
            }
        }
        VisType::Fall => {
            s.push_str(&format!(
                "summon minecraft:falling_block {} {} {} {state}{}",
                gml_num(key - 45.0 + x),
                y,
                gml_num((index as f64 * 2.0) + z),
                glowing
            ));
            s.push_str("Time:-80,DropItem:0,Motion:[0.0d,-1.3d,0.0d]}\n");
            if opts.glow {
                s.push_str(&format!("team join nbs_{team_number} @e[tag=nbs_{team_number}]\n"));
            }
        }
        VisType::Rise => {
            s.push_str(&format!(
                "summon minecraft:falling_block {} {} {} {state}{}",
                gml_num(key - 45.0 + x),
                y,
                gml_num((index as f64 * 2.0) + z),
                glowing
            ));
            s.push_str("Time:-50,DropItem:0,Glowing:1,NoGravity:1,Motion:[0.0d,1.0d,0.0d]}\n");
            if opts.glow {
                s.push_str(&format!("team join nbs_{team_number} @e[tag=nbs_{team_number}]\n"));
            }
        }
        VisType::Bounce => {
            s.push_str(&format!(
                "summon minecraft:falling_block {team_number} {} {} {state}{}",
                y,
                gml_num((index as f64 * 2.0) + z),
                glowing
            ));
            s.push_str("Time:-80,DropItem:0,Motion:[0.0d,1.3d,0.0d]}\n");
            if opts.glow {
                s.push_str(&format!("team join nbs_{team_number} @e[tag=nbs_{team_number}]\n"));
            }
        }
        VisType::PianoRoll => {
            s.push_str(&format!(
                "summon minecraft:falling_block {} {} {} {state}{}",
                gml_num((key - 45.0) * -1.0 + x),
                y,
                gml_num(z),
                glowing
            ));
            s.push_str("Time:-50,DropItem:0,NoGravity:1,Motion:[0.0d,0.0d,2.5d]}\n");
            if opts.glow {
                s.push_str(&format!("team join nbs_{team_number} @e[tag=nbs_{team_number}]\n"));
            }
        }
        VisType::Fountain => {
            s.push_str(&format!(
                "summon minecraft:falling_block {team_number} {} {} {state}{}",
                y,
                gml_num((index as f64 * 2.0) + z),
                glowing
            ));
            if key > 45.0 {
                s.push_str("Time:-80,DropItem:0,Motion:[0.5d,1.5d,0.0d]}\n");
            } else {
                s.push_str("Time:-80,DropItem:0,Motion:[-0.5d,1.5d,0.0d]}\n");
            }
            if opts.glow {
                // 原版这里用的是 ins_index（比队名少 1），照抄。
                s.push_str(&format!("team join nbs_{team_number} @e[tag=nbs_{index}]\n"));
            }
        }
        VisType::RittaiOnkyou => {
            s.push_str(&format!(
                "summon minecraft:falling_block {} 90 {} \"{{\"Tags\":[\"nbs\"],BlockState:{{Name:\"minecraft:{block_id}\"}},Time:-80,DropItem:0,Motion:[0.0d,-1.3d,0.0d]}}\n",
                gml_num(blockposition * 48.0),
                gml_num(blockvolume * 48.0)
            ));
            s.push_str(&format!(
                "summon minecraft:falling_block {} 90 {} \"{{\"Tags\":[\"nbs\"],BlockState:{{Name:\"minecraft:note_block\"}},Time:-80,DropItem:0,Motion:[0.0d,-1.3d,0.0d]}}\n",
                gml_num(blockposition * 48.0),
                gml_num(blockvolume * 48.0 - 1.0)
            ));
            s.push_str(&format!(
                "particle minecraft:note {} 90 {} 0 0 0 1 1 force @p\n",
                gml_num(blockposition * 48.0),
                gml_num(blockvolume * 48.0 - 2.0)
            ));
        }
    }
    s
}

// ---------------------------------------------------------------- 落盘

/// 写成目录（原版 `move.bat` 那条分支）。
pub fn write_folder(dest: &Path, files: &[GeneratedFile]) -> Result<(), String> {
    for f in files {
        let path = dest.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        std::fs::write(&path, bytes_of(f))
            .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

fn bytes_of(f: &GeneratedFile) -> Vec<u8> {
    // 原版 dat_writefile 用 buffer_text，写出的就是 UTF-8 字节且不带结尾 NUL。
    f.content.as_bytes().to_vec()
}

/// 打成 ZIP（原版调 7za，这里自己写：deflate + CRC32，条目名走 UTF-8）。
pub fn write_zip(dest: &Path, files: &[GeneratedFile]) -> Result<(), String> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let mut count = 0u16;
    // 固定 1980-01-01 00:00:00，让同一个包可复现比对。
    const DOS_TIME: u16 = 0;
    const DOS_DATE: u16 = 0x21;

    for f in files {
        let name = f.path.as_bytes();
        let data = bytes_of(f);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&data);
        let crc = hasher.finalize();

        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&data).map_err(|e| format!("压缩失败: {e}"))?;
        let comp = enc.finish().map_err(|e| format!("压缩结束失败: {e}"))?;

        let offset = out.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 名称
        out.extend_from_slice(&8u16.to_le_bytes()); // method: deflate
        out.extend_from_slice(&DOS_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name);
        out.extend_from_slice(&comp);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&8u16.to_le_bytes());
        central.extend_from_slice(&DOS_TIME.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 4]); // extra len, comment len
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
        count += 1;
    }

    let dir_offset = out.len() as u32;
    out.extend_from_slice(&central);
    let dir_len = central.len() as u32;
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]); // disk numbers
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&dir_len.to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    std::fs::write(dest, &out).map_err(|e| format!("写入 {} 失败: {e}", dest.display()))?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DatapackInfo {
    pub files: usize,
    pub bytes: u64,
    pub zip: bool,
    pub functionpath: String,
    pub objective: String,
}

/// 导出到 `dest`：扩展名是 `zip` 就打包，否则按原版非 ZIP 分支写成目录。
/// 注意 `opts.use_zip` 只照原样决定 uninstall 里的引用形式（`file/名` 还是 `名`），
/// 调用方必须保证它和 `dest` 的扩展名一致，否则包内文本会和实际容器对不上。
pub fn export(song: &Song, opts: &DatapackOptions, dest: &Path) -> Result<DatapackInfo, String> {
    if letters_digits(&opts.name).is_empty() {
        return Err("数据包需要一个有效的唯一名称".into());
    }
    if sanitize_path(&opts.path).matches('/').count() >= 5 {
        return Err("路径最多只能套 5 个子文件夹".into());
    }
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let plan = generate(song, opts, &stem);
    let is_zip = dest.extension().and_then(|e| e.to_str()) == Some("zip");
    let bytes: u64 = plan.files.iter().map(|f| f.content.len() as u64).sum();
    if is_zip {
        write_zip(dest, &plan.files)?;
    } else {
        write_folder(dest, &plan.files)?;
    }
    Ok(DatapackInfo {
        files: plan.files.len(),
        bytes,
        zip: is_zip,
        functionpath: plan.functionpath,
        objective: plan.objective,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::{Layer, LockMode, Note};

    fn opts(name: &str) -> DatapackOptions {
        DatapackOptions {
            name: name.into(),
            ..Default::default()
        }
    }

    /// tick 0 与 tick 1 各一个音符，enda = 1。
    fn two_tick_song() -> Song {
        let mut song = Song::new();
        song.set(0, 0, Note::new(0, 45));
        song.set(1, 0, Note::new(0, 45));
        song
    }

    fn content(plan: &Plan, path: &str) -> String {
        plan.files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("未生成 {path}"))
            .content
            .clone()
    }

    fn line0(plan: &Plan, path: &str) -> String {
        content(plan, path)
            .lines()
            .next()
            .unwrap_or("<空文件>")
            .to_string()
    }

    /// 解析一条 `execute as @s[scores={...}] run function X` 的区间与目标。
    fn pred(line: &str) -> (i64, i64, i64, i64, String) {
        let open = line.find("scores={").expect("scores 谓词") + "scores={".len();
        let close = open + line[open..].find('}').expect("scores 结束");
        let mut lo = i64::MIN;
        let mut hi = i64::MAX;
        let mut tlo = i64::MIN;
        let mut thi = i64::MAX;
        for part in line[open..close].split(',') {
            let (name, range) = part.split_once('=').expect("objective=区间");
            let mut it = range.split("..");
            let a = it.next().unwrap_or("");
            let b = it.next().unwrap_or("");
            let (l, h) = match (a.is_empty(), b.is_empty()) {
                (true, false) => (i64::MIN, b.parse().unwrap()),
                (false, true) => (a.parse().unwrap(), i64::MAX),
                _ => (a.parse().unwrap(), b.parse().unwrap()),
            };
            if name.ends_with("_t") {
                tlo = l;
                thi = h;
            } else {
                lo = l;
                hi = h;
            }
        }
        (lo, hi, tlo, thi, function_ref(line).expect("function 目标"))
    }

    /// 取出一行里被调用的函数引用（`run function X` 或裸 `function X`）。
    fn function_ref(line: &str) -> Option<String> {
        if let Some(i) = line.rfind(" run function ") {
            return Some(line[i + " run function ".len()..].to_string());
        }
        line.strip_prefix("function ").map(|s| s.to_string())
    }

    /// 函数目录（原版 `dat_makefolders` 返回的 functiondir 的包内形式）。
    fn func_dir(plan: &Plan, registry: &str) -> String {
        let (ns, rest) = plan.functionpath.split_once(':').expect("functionpath 带命名空间");
        let sub = rest.trim_end_matches('/');
        if sub.is_empty() {
            format!("data/{ns}/{registry}/")
        } else {
            format!("data/{ns}/{registry}/{sub}/")
        }
    }

    fn path_of(dir: &str, target: &str) -> String {
        let rel = target.split_once(':').map(|(_, r)| r).unwrap_or(target);
        format!("{dir}{rel}.mcfunction")
    }

    fn by_path(plan: &Plan) -> std::collections::HashMap<String, String> {
        plan.files
            .iter()
            .map(|f| (f.path.clone(), f.content.clone()))
            .collect()
    }

    /// 模拟游戏里的一次函数遍历：分数 `score`、已播放到 `_t = seen` 时，
    /// 从 `root` 出发会命中哪些 notes 函数。
    fn walk(map: &std::collections::HashMap<String, String>, dir: &str, root: &str, score: i64, seen: i64) -> Vec<String> {
        let mut fired = Vec::new();
        let mut stack = vec![root.to_string()];
        while let Some(p) = stack.pop() {
            let text = map.get(&p).unwrap_or_else(|| panic!("引用了不存在的函数文件 {p}"));
            for line in text.lines() {
                let (lo, hi, tlo, thi, target) = pred(line);
                if lo <= score && score <= hi && tlo <= seen && seen <= thi {
                    if target.contains(":notes/") {
                        fired.push(target);
                    } else {
                        stack.push(path_of(dir, &target));
                    }
                }
            }
        }
        fired
    }

    /// 包内每一条函数引用都必须落在真实生成的文件上。
    fn assert_no_dangling_refs(plan: &Plan, dir: &str) {
        let have: std::collections::HashSet<&str> =
            plan.files.iter().map(|f| f.path.as_str()).collect();
        for f in &plan.files {
            for line in f.content.lines() {
                if let Some(target) = function_ref(line) {
                    let p = path_of(dir, &target);
                    assert!(have.contains(p.as_str()), "{} 引用了不存在的 {target}", f.path);
                }
            }
        }
    }

    #[test]
    fn gml_number_format_matches_gml_string() {
        assert_eq!(gml_num(1.0), "1");
        assert_eq!(gml_num(0.0), "0");
        assert_eq!(gml_num(-1.0), "-1");
        assert_eq!(gml_num(-2.0), "-2");
        assert_eq!(gml_num(0.5), "0.5");
        assert_eq!(gml_num(1.0 / 3.0), "0.333333");
        assert_eq!(gml_num(7.0), "7");
        assert_eq!(gml_num(320.0), "320");
    }

    #[test]
    fn pitch_uses_the_original_six_decimals() {
        assert_eq!(dat_pitch(33.0), "0.500000");
        assert_eq!(dat_pitch(45.0), "1.000000");
        assert_eq!(dat_pitch(57.0), "2.000000");
        assert_eq!(dat_pitch(31.0), "1.781797");
        assert_eq!(dat_pitch(30.0), "1.681793");
        assert_eq!(dat_pitch(47.0), "1.122462");
        assert_eq!(dat_pitch(45.12), "1.006956");
        assert_eq!(dat_pitch(58.0), "0.529732");
    }

    #[test]
    fn name_namespace_and_path_use_the_original_sanitizers() {
        // 项目自带的 string_path：小写、空格转下划线、吃掉除 _ 与字母数字外的一切。
        assert_eq!(gml_string_path("Test Song"), "test_song");
        assert_eq!(gml_string_path("My Song!! Vol.2"), "my_song_vol2");
        assert_eq!(gml_string_path("a/b:c"), "abc");
        // dat_getpath：额外允许单个 `/`，去掉结尾的一个 `/`。
        assert_eq!(sanitize_path("Songs//Other/"), "songs/other");
        assert_eq!(sanitize_path("/leading"), "leading");
        assert_eq!(sanitize_path("My Song"), "my_song");
        assert_eq!(sanitize_path(""), "");
        assert_eq!(letters_digits("Test Song 1.0!"), "TestSong10");
    }

    #[test]
    fn command_preview_matches_dat_preview() {
        assert_eq!(command_preview("", "", ""), "<enter name>");
        assert_eq!(command_preview("!!!", "ns", ""), "<enter name>");
        assert_eq!(command_preview("Nyan Cat", "", ""), "/function nyan_cat:play");
        assert_eq!(command_preview("Nyan Cat", "///", ""), "/function nyan_cat:play");
        assert_eq!(
            command_preview("Nyan Cat", "MyNS", ""),
            "/function myns:nyan_cat/play"
        );
        assert_eq!(
            command_preview("Nyan Cat", "MyNS", "Songs//Cat/"),
            "/function myns:songs/cat/nyan_cat/play"
        );
        assert_eq!(
            command_preview("Nyan Cat", "MyNS", "!!!"),
            "/function myns:nyan_cat/play"
        );
    }

    #[test]
    fn mc_version_selects_pack_format_and_registry() {
        let mut o = opts("Song");
        assert_eq!(o.pack_format(), 48);
        assert_eq!(o.function_registry(), "function");
        o.mc_version = 0;
        assert_eq!(o.pack_format(), 41);
        assert_eq!(o.function_registry(), "functions");
        let plan = generate(&two_tick_song(), &o, "song");
        assert!(plan
            .files
            .iter()
            .any(|f| f.path == "data/minecraft/tags/functions/load.json"));
        assert!(content(&plan, "pack.mcmeta").contains("\"pack_format\": 41"));
    }

    #[test]
    fn fixed_files_reproduce_the_original_text() {
        let plan = generate(&two_tick_song(), &opts("Test Song"), "test-song");
        assert_eq!(plan.functionpath, "test_song:");
        assert_eq!(plan.objective, "nbs_TestSong");
        assert_eq!(
            content(&plan, "pack.mcmeta"),
            "{\n\t\"pack\": {\n\t\t\"pack_format\": 48,\n\t\t\"description\": \"Test Song\\nMade with Note Block Studio\"\n\t}\n}"
        );
        assert_eq!(
            content(&plan, "data/minecraft/tags/function/load.json"),
            "{\"values\": [\"test_song:load\"]}"
        );
        assert_eq!(
            content(&plan, "data/minecraft/tags/function/tick.json"),
            "{\"values\": [\"test_song:tick\"]}"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/load.mcfunction"),
            "scoreboard objectives add nbs_TestSong dummy\nscoreboard objectives add nbs_TestSong_t dummy\nscoreboard players set speed nbs_TestSong 40"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/tick.mcfunction"),
            "execute as @a[tag=nbs_TestSong] run scoreboard players operation @s nbs_TestSong += speed nbs_TestSong\nexecute as @a[tag=nbs_TestSong] at @s run function test_song:tree/0_1"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/play.mcfunction"),
            "tag @s add nbs_TestSong\nscoreboard players set @s nbs_TestSong_t -1"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/pause.mcfunction"),
            "tag @s remove nbs_TestSong"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/stop.mcfunction"),
            "tag @s remove nbs_TestSong\nscoreboard players reset @s nbs_TestSong\nscoreboard players reset @s nbs_TestSong_t"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/notes/0.mcfunction"),
            "playsound minecraft:block.note_block.harp record @s ^0 ^ ^ 1 1.000000 1\nscoreboard players set @s nbs_TestSong_t 0"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/notes/1.mcfunction"),
            "playsound minecraft:block.note_block.harp record @s ^0 ^ ^ 1 1.000000 1\nfunction test_song:stop"
        );
        assert_eq!(
            content(&plan, "data/test_song/function/tree/0_1.mcfunction"),
            "execute as @s[scores={nbs_TestSong=0..240,nbs_TestSong_t=..-1}] run function test_song:notes/0\nexecute as @s[scores={nbs_TestSong=80..320,nbs_TestSong_t=..0}] run function test_song:notes/1\n"
        );
    }

    #[test]
    fn uninstall_differs_only_by_the_disable_form() {
        let stem = "my pack";
        let zip = content(
            &generate(&two_tick_song(), &opts("Test Song"), stem),
            "data/test_song/function/uninstall.mcfunction",
        );
        let mut o = opts("Test Song");
        o.use_zip = false;
        let folder = content(
            &generate(&two_tick_song(), &o, stem),
            "data/test_song/function/uninstall.mcfunction",
        );
        assert!(
            zip.starts_with("tag @e remove nbs_TestSong\nscoreboard objectives remove nbs_TestSong\nscoreboard objectives remove nbs_TestSong_t\n"),
            "{zip}"
        );
        assert!(zip.contains("datapack disable \"file/my pack\"\n"), "{zip}");
        assert!(folder.contains("datapack disable \"my pack\"\n"), "{folder}");
        assert_eq!(zip.replace("file/my pack", "my pack"), folder);
        assert!(zip.ends_with("uninstalled successfully. You may now remove it from your data pack folder.\",\"color\":\"yellow\"}]"));
    }

    #[test]
    fn namespace_and_path_build_the_three_original_shapes() {
        // 无命名空间：命名空间取消毒后的歌名，函数放在命名空间根下。
        let p = generate(&two_tick_song(), &opts("Test Song"), "t");
        assert_eq!(p.functionpath, "test_song:");
        assert!(p
            .files
            .iter()
            .any(|f| f.path == "data/test_song/function/load.mcfunction"));

        // 只有命名空间：路径变成歌名。
        let mut o = opts("Test Song");
        o.namespace = "MyNS".into();
        let p = generate(&two_tick_song(), &o, "t");
        assert_eq!(p.functionpath, "myns:test_song/");
        assert!(p
            .files
            .iter()
            .any(|f| f.path == "data/myns/function/test_song/load.mcfunction"));

        // 命名空间 + 路径：路径后追加歌名，目录一层层建出来。
        let mut o = opts("Test Song");
        o.namespace = "MyNS".into();
        o.path = "Songs//Other/".into();
        let p = generate(&two_tick_song(), &o, "t");
        assert_eq!(p.functionpath, "myns:songs/other/test_song/");
        assert_eq!(
            line0(&p, "data/myns/function/songs/other/test_song/play.mcfunction"),
            "tag @s add nbs_TestSong"
        );
        assert!(p
            .files
            .iter()
            .any(|f| f.path == "data/myns/function/songs/other/test_song/notes/0.mcfunction"));
    }

    #[test]
    fn single_tick_song_gets_the_original_degenerate_tree() {
        let mut song = Song::new();
        song.set(0, 0, Note::new(0, 45));
        let plan = generate(&song, &opts("one"), "one");
        // 原版 length=0 → steps=-inf → 一个 tree 文件都不写，tick.mcfunction 仍指向 0_-1。
        assert_eq!(
            content(&plan, "data/one/function/tick.mcfunction")
                .lines()
                .last()
                .unwrap(),
            "execute as @a[tag=nbs_one] at @s run function one:tree/0_-1"
        );
        assert!(!plan
            .files
            .iter()
            .any(|f| f.path.starts_with("data/one/function/tree/")));
        assert_eq!(
            content(&plan, "data/one/function/notes/0.mcfunction"),
            "playsound minecraft:block.note_block.harp record @s ^0 ^ ^ 1 1.000000 1\nfunction one:stop"
        );
    }

    #[test]
    fn looping_rewrites_the_score_instead_of_stopping() {
        let mut song = two_tick_song();
        song.meta.loop_start = 2;
        let mut o = opts("loop");
        o.enable_looping = true;
        let plan = generate(&song, &o, "loop");
        let n1 = content(&plan, "data/loop/function/notes/1.mcfunction");
        assert_eq!(
            n1,
            "playsound minecraft:block.note_block.harp record @s ^0 ^ ^ 1 1.000000 1\nscoreboard players set @s nbs_loop 160\nscoreboard players set @s nbs_loop_t 1"
        );
        assert!(!n1.contains("function loop:stop"));
        assert!(content(&plan, "data/loop/function/notes/0.mcfunction").ends_with("nbs_loop_t 0"));
    }

    #[test]
    fn pan_and_volume_follow_the_original_formula() {
        let mut song = Song::new();
        song.layers[0].volume = 50;
        song.layers[0].stereo = 200;
        let mut n = Note::new(0, 45);
        n.panning = 200;
        song.set(0, 0, n);
        song.set(1, 0, n);
        let plan = generate(&song, &opts("pan"), "pan");
        // s = (200+200)/2 = 200 → blockposition = -1 → ^-2；50/100/100*100 = 0.5
        assert_eq!(
            line0(&plan, "data/pan/function/notes/0.mcfunction"),
            "playsound minecraft:block.note_block.harp record @s ^-2 ^ ^ 0.5 1.000000 1"
        );

        // 共享音乐：改成 @a 相对坐标 + dat_radiusvalue。
        let mut o = opts("pan");
        o.enable_radius = true;
        o.radius = 116;
        assert_eq!(o.radius_value(), 7.0);
        let plan = generate(&song, &o, "pan");
        assert_eq!(
            line0(&plan, "data/pan/function/notes/0.mcfunction"),
            "execute at @s run playsound minecraft:block.note_block.harp record @a ~ ~ ~ 7 1.000000"
        );
        assert_eq!(
            content(&plan, "data/pan/function/tick.mcfunction")
                .lines()
                .last()
                .unwrap(),
            "execute as @a[tag=nbs_pan] run function pan:tree/0_1"
        );
    }

    #[test]
    fn locked_layers_are_skipped_unless_requested() {
        let mut song = Song::new();
        song.layers.push(Layer::default());
        song.layers[1].lock = LockMode::Locked;
        song.set(0, 0, Note::new(0, 45));
        song.set(0, 1, Note::new(1, 47));
        let plan = generate(&song, &opts("mix"), "mix");
        let n0 = content(&plan, "data/mix/function/notes/0.mcfunction");
        assert_eq!(n0.matches("playsound").count(), 1, "{n0}");
        assert!(!n0.contains("bass"), "{n0}");

        let mut o = opts("mix");
        o.include_locked = true;
        let plan = generate(&song, &o, "mix");
        let n0 = content(&plan, "data/mix/function/notes/0.mcfunction");
        assert_eq!(n0.matches("playsound").count(), 2, "{n0}");
        assert!(n0.contains("playsound minecraft:block.note_block.bass record @s ^0 ^ ^ 1 1.122462 1"), "{n0}");
    }

    #[test]
    fn out_of_range_notes_need_the_extra_option() {
        let mut song = Song::new();
        song.set(0, 0, Note::new(0, 30));
        song.set(1, 0, Note::new(0, 58));
        let plan = generate(&song, &opts("oor"), "oor");
        // 原版 colamount 只看有没有音符，所以文件照样写，只是里面没有 playsound。
        let n0 = content(&plan, "data/oor/function/notes/0.mcfunction");
        assert!(!n0.contains("playsound"), "{n0}");
        assert_eq!(n0, "scoreboard players set @s nbs_oor_t 0");

        let mut o = opts("oor");
        o.include_out_of_range = true;
        let plan = generate(&song, &o, "oor");
        assert_eq!(
            line0(&plan, "data/oor/function/notes/0.mcfunction"),
            "playsound minecraft:block.note_block.harp_-1 record @s ^0 ^ ^ 1 1.681793 1"
        );
        assert_eq!(
            line0(&plan, "data/oor/function/notes/1.mcfunction"),
            "playsound minecraft:block.note_block.harp_1 record @s ^0 ^ ^ 1 0.529732 1"
        );
    }

    #[test]
    fn custom_instrument_uses_its_sound_id() {
        let mut song = Song::new();
        song.custom_instruments
            .push(nbs_core::model::CustomInstrument {
                name: "piano".into(),
                filename: String::new(),
                key: 45,
                press: true,
            });
        song.set(0, 0, Note::new(16, 45));
        song.set(1, 0, Note::new(16, 45));
        let plan = generate(&song, &opts("cus"), "cus");
        assert_eq!(
            line0(&plan, "data/cus/function/notes/0.mcfunction"),
            "playsound minecraft:piano record @s ^0 ^ ^ 1 1.000000 1"
        );
    }

    #[test]
    fn visualizer_arc_matches_the_original_summon() {
        let mut o = opts("vis");
        o.visualizer = true;
        o.glow = true;
        let plan = generate(&two_tick_song(), &o, "vis");
        let n0 = content(&plan, "data/vis/function/notes/0.mcfunction");
        assert_eq!(
            n0.lines().nth(1).unwrap(),
            "summon minecraft:falling_block 0 100 0 {BlockState:{Name:\"minecraft:dirt\"},Tags:[\"nbs\",\"nbs_1\"],Glowing:1,Time:-120,DropItem:0,Motion:[0.0d,1.0d,1.0d]}"
        );
        assert_eq!(n0.lines().nth(2).unwrap(), "team join nbs_1 @e[tag=nbs_1]");
        let add = content(&plan, "data/vis/function/add_teams.mcfunction");
        assert!(add.starts_with("team add nbs_1\nteam add nbs_2\n"), "{add}");
        assert!(add.contains("team add nbs_17\nteam modify nbs_1 color dark_gray\n"), "{add}");
        assert!(add.ends_with("team modify nbs_17 color white"), "{add}");
        assert_eq!(add.lines().count(), 34);
        let rem = content(&plan, "data/vis/function/remove_teams.mcfunction");
        assert_eq!(rem.lines().count(), 17);
        assert!(rem.ends_with("team remove nbs_17"), "{rem}");
        assert!(content(&plan, "data/vis/function/play.mcfunction").ends_with("function vis:add_teams"));
        assert!(content(&plan, "data/vis/function/stop.mcfunction").ends_with("function vis:remove_teams"));
        assert!(content(&plan, "data/vis/function/uninstall.mcfunction")
            .contains("kill @e[type=falling_block,tag=nbs]\nfunction vis:remove_teams\n"));

        // 关掉发光就没有 Tags/Glowing，也不生成建队脚本。
        let mut o = opts("vis");
        o.visualizer = true;
        o.glow = false;
        o.vis_type = VisType::Fall;
        let plan = generate(&two_tick_song(), &o, "vis");
        assert_eq!(
            content(&plan, "data/vis/function/notes/0.mcfunction")
                .lines()
                .nth(1)
                .unwrap(),
            "summon minecraft:falling_block 0 100 0 {BlockState:{Name:\"minecraft:dirt\"},Time:-80,DropItem:0,Motion:[0.0d,-1.3d,0.0d]}"
        );
        assert!(!plan
            .files
            .iter()
            .any(|f| f.path.ends_with("add_teams.mcfunction")));
    }

    /// 断言「每个 tick 恰好响一次」——原版靠 `_t` 挡板防止重叠区间重复播放。
    fn assert_one_fire_per_tick(song: &Song, plan: &Plan, registry: &str) {
        let dir = func_dir(plan, registry);
        let root_target = {
            let tick_file = content(plan, &format!("{dir}tick.mcfunction"));
            function_ref(tick_file.lines().last().unwrap()).expect("tick 里引用根函数")
        };
        let map = by_path(plan);
        let root = path_of(&dir, &root_target);
        let mut with_notes = 0;
        for tick in 0..=song.enda as i64 {
            let has = song.columns.contains_key(&(tick as u32));
            if has {
                with_notes += 1;
            }
            let fired = walk(&map, &dir, &root, tick * 80, tick - 1);
            let want: Vec<String> = if has {
                vec![format!("{}notes/{tick}", plan.functionpath)]
            } else {
                Vec::new()
            };
            assert_eq!(fired, want, "tick {tick} 命中的音符文件不对（根 {root_target}）");
        }
        assert!(with_notes > 0, "样本曲要有音符");
    }

    #[test]
    fn every_tick_fires_exactly_once_through_the_tree() {
        let mut song = Song::new();
        for t in [0u32, 1, 2, 4, 5, 9, 13, 16, 20] {
            song.set(t, 0, Note::new(0, 45));
        }
        let o = opts("walk");
        let plan = generate(&song, &o, "walk");
        // enda=20 → 根是 0_31。
        assert_eq!(
            function_ref(
                content(&plan, "data/walk/function/tick.mcfunction")
                    .lines()
                    .last()
                    .unwrap()
            )
            .unwrap(),
            "walk:tree/0_31"
        );
        assert_no_dangling_refs(&plan, "data/walk/function/");
        assert_one_fire_per_tick(&song, &plan, o.function_registry());
    }

    /// 用原版自带曲目做真实规模的一致性检查：全部函数引用可达，
    /// 且每一 tick 只命中一次音符函数。
    #[test]
    fn real_song_datapack_walks_cleanly() {
        let songs = [
            "Nyan Cat.nbs",
            "Megalovania - Super Smash Bros. Ultimate.nbs",
            "The Ground's Colour Is Yellow.nbs",
        ];
        for name in songs {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../NoteBlockStudio/datafiles/Songs")
                .join(name);
            if !path.exists() {
                continue;
            }
            let song = nbs_core::nbs::load(&path).expect("能加载原版自带曲目");
            let mut o = opts(name);
            o.visualizer = true;
            o.include_out_of_range = true;
            o.enable_looping = song.meta.loop_enabled;
            o.include_locked = true;
            let plan = generate(&song, &o, "real");
            let dir = func_dir(&plan, o.function_registry());
            assert_no_dangling_refs(&plan, &dir);
            assert_one_fire_per_tick(&song, &plan, o.function_registry());
            // 有音符的 tick 都要有对应文件。
            let map = by_path(&plan);
            for tick in song.columns.keys() {
                assert!(
                    map.contains_key(format!("{dir}notes/{tick}.mcfunction").as_str()),
                    "{name} 缺 notes/{tick}"
                );
            }
            println!(
                "  {name}: {} ticks（enda {}）→ {} 个文件",
                song.columns.len(),
                song.enda,
                plan.files.len()
            );
        }
    }

    #[test]
    fn export_checks_the_original_guards() {
        let song = two_tick_song();
        let dir = std::env::temp_dir().join(format!("nbs-datapack-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut o = opts("  !!  ");
        assert!(export(&song, &o, &dir.join("a.zip")).is_err());

        o = opts("ok");
        o.namespace = "okns".into();
        o.path = "a/b/c/d/e/f".into();
        assert!(export(&song, &o, &dir.join("b.zip")).is_err());

        o.path = "a/b/c/d/e".into();
        let info = export(&song, &o, &dir.join("b.zip")).expect("五个子文件夹应通过");
        assert!(info.zip);
        assert!(info.files > 3);
        assert_eq!(info.functionpath, "okns:a/b/c/d/e/ok/");
        assert!(dir.join("b.zip").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zip_and_folder_write_the_same_bytes() {
        let plan = generate(&two_tick_song(), &opts("Test Song"), "out");
        let dir = std::env::temp_dir().join(format!("nbs-datapack-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("pack");
        let zipfile = dir.join("pack.zip");
        write_folder(&folder, &plan.files).expect("写目录");
        write_zip(&zipfile, &plan.files).expect("写 zip");

        for f in &plan.files {
            let p = folder.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
            assert_eq!(std::fs::read(&p).unwrap(), f.content.as_bytes(), "{}", f.path);
        }

        let entries = read_zip(&zipfile);
        assert_eq!(entries.len(), plan.files.len());
        for (name, data) in &entries {
            let want = plan
                .files
                .iter()
                .find(|f| &f.path == name)
                .unwrap_or_else(|| panic!("zip 里没有 {name}"));
            assert_eq!(data, want.content.as_bytes(), "{name} 内容不符");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 最小 ZIP 读取器：走中央目录并 deflate 解压，用来验收自己写的打包器。
    fn read_zip(path: &Path) -> Vec<(String, Vec<u8>)> {
        use std::io::Read;
        let bytes = std::fs::read(path).expect("读取 zip");
        let eocd = bytes
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("EOCD");
        let u16 = |i: usize| -> u16 { u16::from_le_bytes([bytes[i], bytes[i + 1]]) };
        let u32 = |i: usize| -> u32 {
            u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
        };
        let count = u16(eocd + 10) as usize;
        let mut off = u32(eocd + 16) as usize;
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(&bytes[off..off + 4], &[0x50, 0x4b, 0x01, 0x02], "中央目录签名");
            let method = u16(off + 10);
            let crc = u32(off + 16);
            let csize = u32(off + 20) as usize;
            let dsize = u32(off + 24) as usize;
            let nlen = u16(off + 28) as usize;
            let elen = u16(off + 30) as usize;
            let clen = u16(off + 32) as usize;
            let local = u32(off + 42) as usize;
            let name = String::from_utf8(bytes[off + 46..off + 46 + nlen].to_vec()).unwrap();
            let lnlen = u16(local + 26) as usize;
            let lelen = u16(local + 28) as usize;
            let start = local + 30 + lnlen + lelen;
            let comp = &bytes[start..start + csize];
            let data = if method == 0 {
                comp.to_vec()
            } else {
                let mut d = Vec::new();
                flate2::read::DeflateDecoder::new(comp).read_to_end(&mut d).unwrap();
                d
            };
            assert_eq!(data.len(), dsize, "{name} 解压长度不符");
            let mut h = crc32fast::Hasher::new();
            h.update(&data);
            assert_eq!(h.finalize(), crc, "{name} CRC 不符");
            out.push((name, data));
            off += 46 + nlen + elen + clen;
        }
        out
    }
}
