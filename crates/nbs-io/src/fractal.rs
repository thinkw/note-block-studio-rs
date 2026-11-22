//! nbs-fractal 机体投影导出（`.litematic`，**自带红石的整机**）。
//!
//! 和 [`crate::maestro`] 那条正好互补：MAESTRO 只出错码箱，贴到已建好的机体上才响；
//! 这里产出的是能独立运行的机体，贴下去就响。几何、延迟元件、方块选择全部来自上游
//! `nbs-fractal`（AGPL-3.0-or-later，本仓 AGPL-3.0-only 的接收方可取 3.0；它的
//! `rsnbs` / `litematica` 依赖是 MIT OR Apache-2.0），我们不重写它任何一格几何。
//!
//! 20 t/s（每 game tick 一音）走上游那两档：`linear` 的 Scale1（奇刻落支道、活塞压）
//! 与 `compact` 的 halves（偶奇各拆一条 10 Hz 道）。三家 20 Hz 生态的机制同形——都是
//! 「两条 10 Hz 单元反相 1 刻」，没有谁把单个延迟元件压到 1 刻，所以这里不需要自拍常数。
//!
//! 音符取舍沿用 MAESTRO 那条口径（不是上游的口径）：上游的 `playable()` 对超窗音高与
//! 自定义乐器是**静默丢弃**，且布局侧拿不到音符盒状态时回落到链条方块，产物里看不出少了
//! 音。这里改成 ±12 拉回 33..=57 + 自定义乐器按采样文件名映射怪物头 + 映射不到的丢弃，
//! 三者都计数并写进 [`FractalReport::lines`]。
//!
//! 时间轴换算刻意保持上游 `rsnbs` 的做法（见 [`tempo_step`]）：它把 `target / tempo`
//! 近似成 `{n, 1/n}` 再整除，只有速度是 20 的整数比时才精确；不近似到整的档位会当场
//! 报错而不是悄悄改变播放时长。

use std::io::Cursor;
use std::num::NonZero;
use std::path::Path;

use nbs_core::model::{Song, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN, PAN_CENTER};
use nbs_core::nbs::{save_bytes, MAX_NBS_VERSION};
use nbs_core::tr;
use nbs_fractal::layout::compact::MultiCompactLayout;
use nbs_fractal::layout::linear::{MultiLinearLayout, ScaleMode, StackedLinearLayout};
use nbs_fractal::layout::{Layout, LayoutExt, Metadata};
use rsnbs::note::{Instrument as UpInstrument, Note as UpNote, NoteStreamExt, Notes};
use rsnbs::song::Song as Upstream;
use rsnbs::types::TimeAnchor;

use crate::maestro::{msg, MOB_SOUNDS};

// ==================================================== 选项

/// 布局族。上游有 9 个布局模块，这里只接两族能兑现 20 t/s 的。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FractalLayout {
    /// 主干＋支道的时间格；档位由时间戳自动选，最细一格一个 game tick。
    #[default]
    Linear,
    /// 变长紧凑延迟线，每组音符一条道；20 t/s 需要奇偶分道。
    Compact,
}

impl FractalLayout {
    pub const ALL: [Self; 2] = [Self::Linear, Self::Compact];

    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => tr("Linear (backbone + branch)", "Linear（主干＋支道）"),
            Self::Compact => tr("Compact (variable-length packed)", "Compact（变长紧凑）"),
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Linear => tr(
                "Cells step by 1, 2, 3 or 4 game ticks, whichever is the coarsest that divides every note timestamp; odd game ticks land on the branch line, driven by a piston. The speed comes from the song, not from a switch.",
                "每格走 1/2/3/4 个 game tick，取能整除所有音符时刻的最粗档；奇数刻落在支道上，由活塞压发。速度是这首歌本身的性质，不由开关决定。",
            ),
            Self::Compact => tr(
                "Packed delay lines, one track per note group; 20 t/s needs the even/odd split.",
                "变长紧凑延迟线，每组音符一条道；20 t/s 要靠奇偶分道。",
            ),
        }
    }

    /// 这一族能不能被「每 game tick 一音」开关影响。
    pub fn honors_half_ticks(self) -> bool {
        matches!(self, Self::Compact)
    }
}

/// 导出选项。默认值对齐上游 CLI 的出厂档。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FractalOptions {
    pub layout: FractalLayout,
    /// 每 game tick 一音（上游 `--halves`）。`Linear` 本来就按时间戳自动选档，此项只对
    /// `Compact` 生效。
    pub twenty_hz: bool,
    /// 每行最多几列后换行，0 = 不换行（上游 Compact 的出厂值是 16）。
    pub wrap: usize,
    /// 相邻道之间的方块间距，0 = 啮合。
    pub gap: u32,
    /// 底部铺整层地板。
    pub full_floor: bool,
    /// 继电器延迟粗度 0..=4，0 = 不限（仅 `Compact` 用得到）。
    pub coarse: u32,
    /// 按空层拆分声道。
    pub breaks: bool,
}

impl Default for FractalOptions {
    fn default() -> Self {
        Self {
            layout: FractalLayout::Linear,
            twenty_hz: true,
            wrap: 0,
            gap: 0,
            full_floor: false,
            coarse: 0,
            breaks: true,
        }
    }
}

impl FractalOptions {
    /// 校验：把上游 CLI 的取值范围搬过来，越界当场拒，而不是让它 `NonZero` 崩。
    fn validate(&self, tempo: f32) -> Result<(), String> {
        if self.coarse > 4 {
            return Err(msg(
                format!(
                    "Repeater coarseness must be 0-4 (0 = unlimited), got {}.",
                    self.coarse
                ),
                format!("继电器粗度只能是 0-4（0 = 不限），当前是 {}。", self.coarse),
            ));
        }
        let target = if self.twenty_hz { 20 } else { 10 };
        // 上游在 tempo 落在 (0, 30) 之外时把 NBS tick 当作 game tick，只按 target/20 折叠，
        // 于是 40 t/s 的歌会原样铺到 20 t/s 网格上、播成两倍长。这条不报错但时长变了。
        if !(0.0..30.0).contains(&tempo) {
            return Err(msg(
                format!("Tempo {tempo} t/s is at or above the {target} t/s grid's ceiling; a note block machine cannot play faster than one note per game tick. Export the classic structure instead."),
                format!("速度 {tempo} t/s 不低于 {target} t/s 网格的上限，音符盒机器最快只能每 game tick 一音，装不下。请改走经典过道导出。"),
            ));
        }
        let (num, den) = tempo_step(tempo, target);
        let ideal = ideal_step(tempo, target);
        let exact = (ideal - num as f32 / den as f32).abs() < 1e-4;
        if !exact {
            return Err(msg(
                format!(
                    "Tempo {tempo} t/s is not a whole-number ratio of the {target} t/s grid ({ideal:.3}); the upstream resampler would round it and change the song's duration. Pick 20/10 t/s-tempo songs, or export the classic structure instead."
                ),
                format!(
                    "速度 {tempo} t/s 不是 {target} t/s 网格的整数比（比值 {ideal:.3}）；上游换算会取整并改变乐曲时长。请改用 20 或 10 的整数比速度，或走经典过道导出。"
                ),
            ));
        }
        Ok(())
    }
}

// ==================================================== 报告

/// 导出过程中对歌曲做过的改写与产出统计。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FractalReport {
    pub layout: FractalLayout,
    /// 机体每格实际间隔几个 game tick——也就是这台机器最快多久发一声。
    /// `Linear` 由上游按音符时刻自动选档（1/2/3/4），`Compact` 跟 `twenty_hz` 一致。
    pub cell_gt: u32,
    /// 每个原 NBS tick 折算成的目标 tick 数（[`tempo_step`] 的 `num / den`）。
    pub step_num: u32,
    pub step_den: u32,
    /// 歌曲原速度（ticks/秒）。
    pub tempo_from: f32,
    /// 输入音符总数。
    pub notes_in: usize,
    /// 实际进机体的音符数。
    pub notes_kept: usize,
    /// 被 ±12 移调拉回 33..=57 的音符数。
    pub transposed: usize,
    /// 自定义乐器命中怪物头、成功改写的音符数。
    pub remapped: usize,
    /// 自定义乐器命中不了怪物头、被丢弃的音符数。
    pub dropped_custom: usize,
    /// 携带力度/声像/音分偏移、但这些信息在音符盒里无处安放的音符数。
    pub fx_ignored: usize,
    /// 拆出来的道数（`Compact` 开奇偶分道时会翻倍）。
    pub tracks: usize,
    /// 用了铜块小号一族（NBS 16..19）的音符数：音符盒的 `instrument=trumpet*` 属性要 26.1 才有。
    pub trumpets: usize,
    /// 机体包围盒（方块）。
    pub size: [i32; 3],
    /// 用到的方块状态种数（palette 长度），给「铺不铺得开」那类核对留个抓手。
    pub palette_len: usize,
}

impl FractalReport {
    /// 这台机器的发声频率（音/秒）= 20 game tick ÷ 每格间隔。
    pub fn notes_per_sec(&self) -> f32 {
        if self.cell_gt == 0 {
            0.0
        } else {
            20.0 / self.cell_gt as f32
        }
    }

    /// 变化说明：只列真正发生过的改写，给导出对话框直接显示。
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        let grid = crate::maestro::trim_num(self.notes_per_sec() as f64);
        let gt = self.cell_gt;
        if self.layout == FractalLayout::Linear {
            v.push(msg(
                format!("Linear puts one cell every {gt} game tick(s), so it plays at {grid} notes/s. The scale is picked from the note timestamps: {gt} divides every one of them."),
                format!("Linear 每 {gt} 个 game tick 一格，也就是 {grid} 音/秒。档位由音符时刻自动选：这些时刻都能被 {gt} 整除。"),
            ));
        } else {
            v.push(msg(
                format!(
                    "Compact uses {} track(s), one cell every {gt} game tick(s), {grid} notes/s.",
                    self.tracks
                ),
                format!(
                    "Compact 用 {} 条道，每 {gt} 个 game tick 一格，{grid} 音/秒。",
                    self.tracks
                ),
            ));
        }
        if self.step_num != self.step_den {
            let ratio = self.step_num as f64 / self.step_den as f64;
            let collapse = if self.step_num < self.step_den {
                msg(
                    " Notes from the merged ticks sound together at the same instant.",
                    " 被并到一起的相邻两刻，音符会在同一刻同时响。",
                )
            } else {
                String::new()
            };
            v.push(msg(
                format!("Tempo {} t/s is rescaled by {ratio} onto this machine's grid, so the song keeps its length.{collapse}", crate::maestro::trim_num(self.tempo_from as f64)),
                format!("速度 {} t/s 按 {ratio} 折算到这台机器的网格（每 {gt} game tick 一格），乐曲时长保持不变。{collapse}", crate::maestro::trim_num(self.tempo_from as f64)),
            ));
        }
        if self.transposed > 0 {
            v.push(msg(
                format!(
                    "{} notes outside F#3-F#5 were moved by octaves.",
                    self.transposed
                ),
                format!("{} 个音符超出 F♯3–F♯5，已按八度移调。", self.transposed),
            ));
        }
        if self.remapped > 0 {
            v.push(msg(
                format!(
                    "{} notes on custom instruments were rewritten onto mob heads.",
                    self.remapped
                ),
                format!(
                    "{} 个自定义乐器音符按采样文件名改写到怪物头。",
                    self.remapped
                ),
            ));
        }
        if self.dropped_custom > 0 {
            v.push(msg(
                format!(
                    "{} notes could not be rendered by a note block and were dropped.",
                    self.dropped_custom
                ),
                format!(
                    "{} 个音符音符盒发不出来（自定义乐器映射不到怪物头），已丢弃。",
                    self.dropped_custom
                ),
            ));
        }
        if self.trumpets > 0 {
            v.push(msg(
                format!("{} notes use the copper trumpet family (NBS indexes 16-19); that note-block instrument property only exists from 26.1 on, so an older machine will not sound them.", self.trumpets),
                format!("{} 个音符用了铜块小号一族（NBS 索引 16-19）；音符盒的 instrument=trumpet* 属性要到 26.1 才有，老机体贴上去这些不会响。", self.trumpets),
            ));
        }
        if self.fx_ignored > 0 {
            v.push(msg(
                format!("Velocity, panning and pitch offsets are not carried by note blocks; {} notes are affected.", self.fx_ignored),
                format!("音符盒不承载力度/声像/音分，{} 个音符的这些信息不会生效。", self.fx_ignored),
            ));
        }
        v.push(msg(
            format!(
                "Bounding box {}x{}x{} blocks, {} block states; this projection is a working machine, it needs no prebuilt structure.",
                self.size[0], self.size[1], self.size[2], self.palette_len
            ),
            format!(
                "包围盒 {}×{}×{} 格，{} 种方块状态；这份投影是自带红石的整机，贴下去就能响，不需要先建机体。",
                self.size[0], self.size[1], self.size[2], self.palette_len
            ),
        ));
        v
    }
}

// ==================================================== 时间换算（照抄上游 rsnbs 的口径）

/// 上游 `rescale_to_tick_rate` 里 `target / tempo` 的理想值。
fn ideal_step(tempo: f32, target: u32) -> f32 {
    if (0.0..30.0).contains(&tempo) {
        target as f32 / tempo
    } else {
        // 上游：速度落在 (0, 30) 之外时当作 NBS tick 已等于 game tick。
        target as f32 / 20.0
    }
}

/// 上游把那比值近似成 `{n, 1/n}` 以便整除；这里复刻同一近似，好在报告里如实说明。
pub(crate) fn tempo_step(tempo: f32, target: u32) -> (u32, u32) {
    let scale = ideal_step(tempo, target);
    if scale >= 1.0 {
        (scale.round() as u32, 1)
    } else {
        (1, (1.0 / scale).round() as u32)
    }
}

// ==================================================== 核心

/// 把音符表搬成上游认得的 `.nbs` 字节，顺手做掉音域与乐器改写并记账。
fn prepare(song: &Song) -> Result<(Vec<u8>, FractalReport), String> {
    let mut rep = FractalReport {
        notes_in: song.total_blocks,
        tempo_from: song.meta.tempo,
        ..Default::default()
    };
    let customs = song.custom_instruments.clone();
    // 官方 `mobInstruments` 查表：自定义乐器 -> 怪物头；查不到就丢。同 MAESTRO 一张表。
    let remap_target = |instrument: u8| -> Option<u8> {
        let custom = (instrument as usize)
            .checked_sub(FIRST_CUSTOM_INDEX as usize)
            .and_then(|i| customs.get(i))?;
        MOB_SOUNDS
            .iter()
            .find(|(file, _)| *file == custom.filename)
            .map(|(_, id)| *id)
    };

    let mut out = Song::default();
    out.meta = song.meta.clone();
    out.layers = song.layers.clone();
    out.custom_instruments = customs.clone();

    for (&tick, col) in &song.columns {
        for (&layer, note) in &col.notes {
            let mut note = *note;
            if note.instrument >= FIRST_CUSTOM_INDEX {
                match remap_target(note.instrument) {
                    Some(id) => {
                        rep.remapped += 1;
                        note.instrument = id;
                    }
                    None => {
                        rep.dropped_custom += 1;
                        continue;
                    }
                }
            }
            // 音高拉回 33..=57：±12 反复，直到落进窗内。
            let mut key = note.key as i32;
            while key < MC_KEY_MIN as i32 {
                key += 12;
            }
            while key > MC_KEY_MAX as i32 {
                key -= 12;
            }
            if key != note.key as i32 {
                rep.transposed += 1;
                note.key = key as u8;
            }
            if note.velocity != 100 || note.panning != PAN_CENTER || note.pitch != 0 {
                rep.fx_ignored += 1;
            }
            rep.notes_kept += 1;
            out.set(tick, layer, note);
        }
    }
    if out.total_blocks == 0 {
        return Err(msg(
            "No note left can be rendered by a note block.",
            "没有任何音符是音符盒能发出来的。",
        ));
    }
    let bytes = save_bytes(&out, MAX_NBS_VERSION).map_err(|e| {
        msg(
            format!("Failed to write the intermediate .nbs: {e}"),
            format!("中间 .nbs 写入失败：{e}"),
        )
    })?;
    Ok((bytes, rep))
}

/// Compact 的一条道：格号（机器刻）+ 音符，外加继电器粗度。
type CompactedTrack = (Vec<(u32, UpNote)>, Option<NonZero<u32>>);

/// `(机体, 道数, 折算目标网格 t/s, 每格几个 game tick, 小号一族颗数)`。
type Assembled = (Box<dyn Layout<()>>, usize, u32, u32, usize);

/// 转成上游布局对象。返回机体、道数与实际时间基。
fn assemble(
    bytes: &[u8],
    opts: &FractalOptions,
) -> Result<Assembled, String> {
    let parsed = Upstream::parse(&mut Cursor::new(bytes)).map_err(|e| {
        msg(
            format!("The upstream NBS reader refused the song: {e}"),
            format!("上游 NBS 解析器拒绝这份曲子：{e}"),
        )
    })?;
    let tempo = parsed.header.tempo;
    let target = if opts.twenty_hz { 20 } else { 10 };
    // 铜块小号一族（NBS v6 的 16..19）是 26.1 才有的音符盒乐器属性，老机体的
    // note_block 状态里没有这一项；颗数如实报，不替用户的 MC 版本做决定。
    let trumpets = parsed
        .notes
        .iter()
        .filter(|(_, n)| {
            matches!(
                n.tone.instrument,
                UpInstrument::Trumpet
                    | UpInstrument::TrumpetExposed
                    | UpInstrument::TrumpetWeathered
                    | UpInstrument::TrumpetOxidized
            )
        })
        .count();

    let (layout, n_tracks, rate, cell_gt) = match opts.layout {
        FractalLayout::Linear => {
            // 上游 Linear 一律落 game tick 域，档位（Scale4/3/2/1）由时间戳自动选。
            let notes: Notes = parsed.notes.rescale_to_game_tick(tempo).collect();
            let song_length = notes.tick_count();
            let tracks: Vec<Notes> = if opts.breaks {
                notes.split_by_layer_gaps()
            } else {
                vec![notes]
            };
            let count = tracks.len();
            // 上游按「能整除所有音符时刻的最粗档」选 ScaleMode，所以档位是这首歌的性质，
            // 不是我们的设置；这里量出来如实报，别替他喊 20 t/s。
            let cell_gt = ScaleMode::new(
                tracks
                    .iter()
                    .flat_map(|t| t.iter().map(|(pos, _)| pos.into_tick())),
            )
            .scale();
            let layout: Box<dyn Layout<()>> = match NonZero::new(opts.wrap) {
                Some(wrap) => Box::new(StackedLinearLayout::new(
                    tracks,
                    Some(wrap),
                    opts.gap,
                    opts.full_floor,
                    song_length,
                )),
                None => Box::new(MultiLinearLayout::new(tracks, opts.gap, song_length)),
            };
            (layout, count, 20, cell_gt)
        }
        FractalLayout::Compact => {
            let coarse = NonZero::new(opts.coarse);
            let groups: Vec<Notes> = if opts.breaks {
                parsed.notes.split_by_layer_gaps()
            } else {
                vec![parsed.notes]
            };
            let mut tracks: Vec<CompactedTrack> = Vec::new();
            for group in groups {
                let stream = group
                    .rescale_to_tick_rate(tempo, target)
                    .map(|(pos, note)| (pos.into_tick(), note));
                if opts.twenty_hz {
                    // 上游 split_halves：共用一个红石刻的两颗音符拆到两条道上。
                    let mut lanes: [Vec<(u32, UpNote)>; 2] = Default::default();
                    for (tick, note) in stream {
                        lanes[(tick % 2) as usize].push((tick / 2, note));
                    }
                    tracks.extend(
                        lanes
                            .into_iter()
                            .filter(|l| !l.is_empty())
                            .map(|l| (l, coarse)),
                    );
                } else {
                    let line: Vec<(u32, UpNote)> = stream.collect();
                    if !line.is_empty() {
                        tracks.push((line, coarse));
                    }
                }
            }
            let count = tracks.len();
            let layout: Box<dyn Layout<()>> = Box::new(MultiCompactLayout::new(
                tracks,
                NonZero::new(opts.wrap),
                opts.gap,
                opts.full_floor,
            ));
            // Compact 的一条道恒是 2 game tick 一格；开奇偶分道后两条道错开 1 刻，
            // 相邻发声间隔才降到 1 刻。
            (layout, count, target, if opts.twenty_hz { 1 } else { 2 })
        }
    };
    Ok((layout, n_tracks, rate, cell_gt, trumpets))
}

/// 产出 `(litematic 字节, 报告)`，不落盘。
pub fn build_bytes(song: &Song, opts: &FractalOptions) -> Result<(Vec<u8>, FractalReport), String> {
    opts.validate(song.meta.tempo)?;
    let (bytes, mut rep) = prepare(song)?;
    let (layout, tracks, rate, cell_gt, trumpets) = assemble(&bytes, opts)?;

    rep.layout = opts.layout;
    rep.cell_gt = cell_gt;
    let (num, den) = tempo_step(song.meta.tempo, rate);
    rep.step_num = num;
    rep.step_den = den;
    rep.tracks = tracks;
    rep.trumpets = trumpets;
    let size = layout.size();
    rep.size = [size.x, size.y, size.z];
    rep.palette_len = layout.block_palette().len();

    let name = if song.meta.name.is_empty() {
        song.filename
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("nbs-rs projection")
            .to_owned()
    } else {
        song.meta.name.clone()
    };
    let author = song.meta.author.clone();
    let description = msg(
        format!(
            "{} layout from nbs-rs, built with nbs-fractal.",
            opts.layout.label()
        ),
        format!(
            "由 nbs-rs 生成的 {} 布局，几何取自 nbs-fractal。",
            opts.layout.label()
        ),
    );
    let metadata = Metadata {
        name: &name,
        description: &description,
        author: &author,
    };
    let mut buf = Vec::new();
    layout.as_litematic(metadata, &mut buf).map_err(|e| {
        msg(
            format!("The litematic writer failed: {e}"),
            format!("投影写出失败：{e}"),
        )
    })?;
    Ok((buf, rep))
}

/// 只算报告不写文件，给导出窗口的预览用。
pub fn preview(song: &Song, opts: &FractalOptions) -> Result<FractalReport, String> {
    build_bytes(song, opts).map(|(_, report)| report)
}

/// 导出到 `path`，返回报告。
pub fn export(song: &Song, path: &Path, opts: &FractalOptions) -> Result<FractalReport, String> {
    let (bytes, report) = build_bytes(song, opts)?;
    std::fs::write(path, bytes).map_err(|e| {
        msg(
            format!("Failed to write the file: {e}"),
            format!("写入文件失败：{e}"),
        )
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Note;

    fn song(tempo: f32, notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::default();
        s.meta.tempo = tempo;
        s.layers = vec![nbs_core::model::Layer::default(); 4];
        for &(tick, layer, instrument, key) in notes {
            s.set(tick, layer, Note::new(instrument, key));
        }
        s
    }

    #[test]
    fn linear_export_is_a_gzip_litematic_and_keeps_every_note() {
        let s = song(
            20.0,
            &[
                (0, 0, 0, 45),
                (1, 0, 1, 47),
                (2, 1, 2, 48),
                (3, 1, 3, 50),
                (4, 0, 0, 45),
            ],
        );
        let opts = FractalOptions::default();
        let (bytes, rep) = build_bytes(&s, &opts).unwrap();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "产物该是 gzip");
        assert_eq!(rep.notes_in, 5);
        assert_eq!(rep.notes_kept, 5);
        assert_eq!(
            (rep.transposed, rep.remapped, rep.dropped_custom),
            (0, 0, 0)
        );
        assert_eq!((rep.cell_gt, rep.notes_per_sec()), (1, 20.0));
        assert!(rep.size[0] > 0 && rep.size[1] > 0 && rep.size[2] > 0);
        assert!(rep.palette_len > 1);
    }

    #[test]
    fn the_upstream_parser_reads_what_our_writer_writes() {
        // 用第三方解析器验自家 .nbs 写出器：颗数与速度必须一字不差。
        let s = song(10.0, &[(0, 0, 0, 45), (2, 1, 5, 47), (7, 2, 9, 52)]);
        let (bytes, rep) = prepare(&s).unwrap();
        let up = Upstream::parse(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(up.notes.iter().count(), rep.notes_kept);
        assert!((up.header.tempo - 10.0).abs() < f32::EPSILON);
        // 音高要一字不差地回来：Key 存的是 MC note 0..=24，加回 33 才是 NBS 音高。
        let mut got: Vec<(u32, u8)> = up
            .notes
            .iter()
            .map(|(pos, note)| {
                (
                    pos.into_tick(),
                    note.tone.key.minecraft_note().unwrap() + 33,
                )
            })
            .collect();
        got.sort();
        assert_eq!(got, vec![(0, 45), (2, 47), (7, 52)]);
    }

    #[test]
    fn out_of_range_keys_move_by_octaves() {
        let s = song(20.0, &[(0, 0, 0, 21), (1, 0, 0, 69), (2, 0, 0, 45)]);
        let (_, rep) = build_bytes(&s, &FractalOptions::default()).unwrap();
        assert_eq!(rep.transposed, 2);
        assert_eq!(rep.notes_kept, 3);
    }

    #[test]
    fn custom_instrument_without_a_mob_head_sample_is_dropped() {
        let mut s = song(20.0, &[(0, 0, FIRST_CUSTOM_INDEX, 45), (1, 0, 0, 45)]);
        s.custom_instruments = vec![nbs_core::model::CustomInstrument {
            name: "Pad".into(),
            filename: "not_a_mob_head".into(),
            key: 45,
            press: true,
        }];
        let (_, rep) = build_bytes(&s, &FractalOptions::default()).unwrap();
        assert_eq!(rep.dropped_custom, 1);
        assert_eq!(rep.notes_kept, 1);
        assert!(rep
            .lines()
            .iter()
            .any(|l| l.contains("丢弃") || l.contains("dropped")));
    }

    #[test]
    fn compact_halves_doubles_the_track_count() {
        // 同一首曲子只差 `twenty_hz` 这一个因子：10 Hz 一条道，20 Hz 拆成奇偶两条。
        let s = song(
            20.0,
            &[(0, 0, 0, 45), (1, 0, 0, 46), (2, 0, 0, 47), (3, 0, 0, 48)],
        );
        let opts = FractalOptions {
            layout: FractalLayout::Compact,
            breaks: false,
            ..FractalOptions::default()
        };
        let (_, ten) = build_bytes(
            &s,
            &FractalOptions {
                twenty_hz: false,
                ..opts
            },
        )
        .unwrap();
        let (_, twenty) = build_bytes(&s, &opts).unwrap();
        assert_eq!((ten.tracks, ten.cell_gt), (1, 2));
        assert_eq!((twenty.tracks, twenty.cell_gt), (2, 1));
        assert_ne!(ten.size, twenty.size, "两条档位的机体形状本来就不同");
        assert!(
            twenty.size[1] > ten.size[1],
            "两条道竖直堆叠，高度该更大：{:?} vs {:?}",
            ten.size,
            twenty.size
        );
    }

    #[test]
    fn a_tempo_that_is_not_a_whole_ratio_is_refused() {
        // 上游会把 20/15 取整成 1，等于把时长改掉 1.33 倍；我们当场拒绝。
        let s = song(15.0, &[(0, 0, 0, 45)]);
        let err = build_bytes(&s, &FractalOptions::default()).unwrap_err();
        assert!(err.contains("15"), "{err}");
    }

    #[test]
    fn tempo_step_matches_the_upstream_approximation() {
        assert_eq!(tempo_step(20.0, 20), (1, 1));
        assert_eq!(tempo_step(10.0, 20), (2, 1));
        assert_eq!(tempo_step(15.0, 20), (1, 1), "1.33 会被上游取整成 1");
        // 上游对 (0,30) 之外的速度按「NBS tick 已等于 game tick」处理：40 折到 20 是 1，
        // 折到 10 才是 1/2。这条分支等于把 40 t/s 当 20 t/s 用，时长会变，所以我们拒掉。
        assert_eq!(tempo_step(40.0, 20), (1, 1));
        assert_eq!(tempo_step(40.0, 10), (1, 2));
    }

    #[test]
    fn a_tempo_at_or_above_the_grid_is_refused() {
        // 每 game tick 一音已经是最快的一档，比它更快的源歌铺不进去。
        let s = song(40.0, &[(0, 0, 0, 45), (1, 0, 0, 46)]);
        let err = build_bytes(&s, &FractalOptions::default()).unwrap_err();
        assert!(err.contains("40"), "{err}");
    }

    #[test]
    fn linear_ignores_the_half_tick_switch() {
        let s = song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 46)]);
        let (_, a) = build_bytes(&s, &FractalOptions::default()).unwrap();
        let (_, b) = build_bytes(
            &s,
            &FractalOptions {
                twenty_hz: false,
                ..FractalOptions::default()
            },
        )
        .unwrap();
        assert_eq!((a.cell_gt, b.cell_gt), (1, 1));
        assert_eq!(a.size, b.size);
    }

    /// 逐格读布局，收集所有音符盒的坐标与状态——时间轴是哪根轴不靠猜。
    fn note_blocks(layout: Box<dyn Layout<()>>) -> Vec<(i32, i32, i32, String, String)> {
        use nbs_fractal::BlockPos;
        let size = layout.size();
        let mut out = Vec::new();
        for x in 0..size.x {
            for y in 0..size.y {
                for z in 0..size.z {
                    if let Some(b) = layout.get_block(BlockPos::new(x, y, z)) {
                        if &b.name[..] == "minecraft:note_block" {
                            out.push((
                                x,
                                y,
                                z,
                                b.properties
                                    .get("instrument")
                                    .map(|c| c.to_string())
                                    .unwrap_or_default(),
                                b.properties
                                    .get("note")
                                    .map(|c| c.to_string())
                                    .unwrap_or_default(),
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    fn compact_notes(ticks: &[(u32, u8)], hz: u32) -> Vec<(i32, i32, i32, String, String)> {
        let s = song(
            20.0,
            &ticks
                .iter()
                .map(|(t, k)| (*t, 0u32, 0u8, *k))
                .collect::<Vec<_>>(),
        );
        let opts = FractalOptions {
            layout: FractalLayout::Compact,
            twenty_hz: hz == 20,
            breaks: false,
            ..FractalOptions::default()
        };
        let bytes = prepare(&s).unwrap().0;
        let (layout, _, _, _, _) = assemble(&bytes, &opts).unwrap();
        note_blocks(layout)
    }

    #[test]
    fn linear_scale_is_a_property_of_the_timestamps() {
        // 上游 Linear 取「能整除所有音符时刻的最粗档」：全偶刻的歌只能出 2 gt 一格（10 音/秒），
        // 加一颗奇刻才降到 1 gt。所以 20 音/秒不是我们设出来的，是这首歌本来达不到的。
        let even = song(20.0, &[(0, 0, 0, 45), (2, 0, 0, 47), (4, 0, 0, 49)]);
        let (_, rep) = build_bytes(&even, &FractalOptions::default()).unwrap();
        assert_eq!((rep.cell_gt, rep.notes_per_sec()), (2, 10.0));
        let mixed = song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 47), (2, 0, 0, 49)]);
        let (_, rep) = build_bytes(&mixed, &FractalOptions::default()).unwrap();
        assert_eq!((rep.cell_gt, rep.notes_per_sec()), (1, 20.0));
    }

    #[test]
    fn compact_lanes_realize_the_game_tick_offset() {
        // 同一份四刻输入，只差 hz 这一个因子，逐格读音符盒落在哪儿。
        let notes = [(0u32, 45u8), (1, 47), (2, 49), (3, 51)];
        let twenty = compact_notes(&notes, 20);
        let ten = compact_notes(&notes, 10);
        assert_eq!((twenty.len(), ten.len()), (4, 4), "并刻不许丢音");

        // 20 Hz：奇偶两颗进两条道（y 不同），各自每 2 个 z 一格；那 1 刻的差在道的相位上。
        let lanes: Vec<i32> = {
            let mut v: Vec<i32> = twenty.iter().map(|b| b.1).collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(lanes.len(), 2, "奇偶分道后该有两条道：{twenty:?}");
        for y in lanes.clone() {
            let mut in_lane: Vec<i32> = twenty.iter().filter(|b| b.1 == y).map(|b| b.2).collect();
            in_lane.sort();
            assert_eq!(in_lane.len(), 2);
            assert_eq!(in_lane[1] - in_lane[0], 2, "一条道内相邻格隔 2 刻");
        }

        // 10 Hz：只剩一条道，被折到一起的两颗同处一格（同 y 同 z），于是一起响而不是排成两拍。
        assert_eq!(
            ten.iter()
                .map(|b| b.1)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );
        let mut cells: std::collections::BTreeMap<i32, Vec<String>> = Default::default();
        for b in &ten {
            cells.entry(b.2).or_default().push(b.4.clone());
        }
        let mut cells: Vec<(i32, Vec<String>)> = cells
            .into_iter()
            .map(|(z, mut ns)| {
                ns.sort();
                (z, ns)
            })
            .collect();
        cells.sort();
        assert_eq!(
            cells,
            vec![
                (0, vec!["12".into(), "14".into()]),
                (2, vec!["16".into(), "18".into()])
            ],
            "tick0+1 与 tick2+3 各自并到同一格"
        );
    }
}
