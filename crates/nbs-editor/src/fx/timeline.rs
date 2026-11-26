//! 特效用的时间轴：tick → 秒，外加按 tick 排序的音符表。
//!
//! 为什么不直接读 `SharedState`：实时那份 tick→秒 换算分散在音频推进里（每帧按
//! 当时的 `params.tempo` 走），导出那份又带离线起点对齐，两边都不适合特效按
//! 「某一列在第几秒」来查。这里按 `tempo_at_tick` 的同一套规则重算一份纯时间的表：
//! 变速点那一列仍走旧速度（原版先派发本列、再改速度），新速度从下一列起生效。
//!
//! 缓存键是 `song.rev` 加「是否播放锁定层」（见 `crate::state::EditorState::fx`）：
//! 曲子一改、或那批会出声的音符一换档就重烘，播放中每帧只查表。

use nbs_audio::scheduler::{build_event_table, EventKind};
use nbs_core::model::{Song, FIRST_CUSTOM_INDEX};

/// 一枚会发出声音的音符（事件乐器与锁定层都不在这里）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FxNote {
    pub tick: u32,
    pub layer: u32,
    pub key: u8,
    pub instrument: u8,
    pub velocity: u8,
    /// 音符自己的声像 0..=200（100 = 中）。
    pub panning: u8,
    /// 所在轨的声道 0..=200。它和 `panning` 一起才决定听到哪只耳朵，见 [`Self::audible_pan`]。
    pub layer_stereo: u8,
}

impl FxNote {
    /// 真正听到的声像 0..=200，直接取音频侧那条公式（轨声道非正中时把音符 pan 往中
    /// 间拉一半），所以按它摆位置的画面和耳朵对得上。
    ///
    /// 不能图省事用裸 `panning`：轨声道拨到一边时，音符的 pan 会被半路吃掉。
    pub fn audible_pan(&self) -> u8 {
        nbs_audio::mixer::Voice::effective_pan(self.panning, self.layer_stereo)
    }
}

/// 每列起点的时刻（秒，相对播放头 0）。
pub struct FxTimeline {
    notes: Vec<FxNote>,
    cum: Vec<f32>,
    ticks_per_bar: u32,
    /// 乐器号 → 乐器名（内置 20 个 + 这首曲子的自定义乐器）。
    /// 特效要靠它区分「这是哪件乐器」：方块表没配的自定义乐器在表里全是同一块石头，
    /// 只有名字能把它们分开。
    instruments: Vec<String>,
    /// 音符表 + 秒表的内容指纹（烘的时候顺手算的）。特效想「对全曲做一次统计、结果缓存起来」
    /// 需要一个能认「曲子还是不是那一首、速度还是不是那个速度」的凭据，见 [`Self::signature`]。
    signature: u64,
}

impl Default for FxTimeline {
    fn default() -> Self {
        Self::new()
    }
}

/// FNV-1a 的一步。指纹只当「重算触发器」用，不当校验和：同一份输入两次算出同一个数就够。
fn mix(signature: &mut u64, field: u64) {
    *signature = (*signature ^ field).wrapping_mul(0x100_0000_01b3);
}

impl FxTimeline {
    pub fn new() -> Self {
        Self {
            notes: Vec::new(),
            cum: vec![0.0, 1.0],
            ticks_per_bar: 16,
            instruments: Vec::new(),
            signature: 0,
        }
    }

    /// 原版 tempo 就是「每秒 tick 数」，所以一列的长度 = 1 / tempo。
    fn seconds_per_tick(tempo: f32) -> f32 {
        1.0 / tempo.max(0.25)
    }

    /// 展开事件表并烘出查表用的时间轴。`include_locked` 和音频那条路同一个参数：
    /// 特效画的必须是真会出声的那批音符，否则锁定层会有胶囊没声音。
    pub fn bake(song: &Song, include_locked: bool) -> Self {
        let events = build_event_table(song, include_locked);
        let ticks_per_bar = 4 * u32::from(song.meta.time_signature.max(1));
        let last_tick = (song.song_length().max(1) as usize) + 1;
        // 变速点：同列多枚取最后一枚（tempo_at_tick 是遍历到底的那个语义）。
        let mut changers: Vec<(u32, f32)> = Vec::new();
        let mut notes: Vec<FxNote> = Vec::new();
        for ev in &events {
            match ev.kind {
                EventKind::Tempo(pitch) => {
                    let tempo = (pitch.unsigned_abs() as f32 / 15.0).max(0.25);
                    match changers.last_mut() {
                        Some(last) if last.0 == ev.tick => last.1 = tempo,
                        _ => changers.push((ev.tick, tempo)),
                    }
                }
                EventKind::Note => notes.push(FxNote {
                    tick: ev.tick,
                    layer: ev.layer,
                    key: ev.note.key,
                    instrument: ev.note.instrument,
                    velocity: ev.note.velocity,
                    panning: ev.note.panning,
                    layer_stereo: ev.layer_stereo,
                }),
                _ => {}
            }
        }
        let mut cum = Vec::with_capacity(last_tick + 1);
        cum.push(0.0);
        let mut t = 0.0_f32;
        let mut idx = 0usize;
        let mut tempo = song.meta.tempo.max(0.25);
        for tick in 0..last_tick {
            while idx < changers.len() && changers[idx].0 as usize <= tick {
                tempo = changers[idx].1;
                idx += 1;
            }
            t += Self::seconds_per_tick(tempo);
            cum.push(t);
        }
        notes.sort_by_key(|n| (n.tick, n.layer));
        // 内容指纹：见 [`Self::signature`]。喂进去的都是「声道方块那套全曲统计」看得见
        // 的东西 —— 时刻和轨决定窗口滑到哪，乐器号决定合并成几格，两个声像决定落哪一槽，
        // 出厂 tempo 和变速点决定「隔多久算断一段」（两张表都按秒判，不看列数）。
        let mut signature = 0xcbf2_9ce4_8422_2325u64;
        for note in &notes {
            for field in [
                note.tick,
                note.layer,
                u32::from(note.instrument),
                u32::from(note.panning),
                u32::from(note.layer_stereo),
            ] {
                mix(&mut signature, u64::from(field));
            }
        }
        mix(&mut signature, notes.len() as u64);
        mix(&mut signature, u64::from(f32::to_bits(song.meta.tempo)));
        for (tick, tempo) in &changers {
            mix(&mut signature, u64::from(*tick));
            mix(&mut signature, u64::from(f32::to_bits(*tempo)));
        }
        // 乐器名表：内置那 20 个 + 这首曲子挂的自定义乐器。越界的号后面查不到，特效按空名处理。
        let instruments = (0..(FIRST_CUSTOM_INDEX as usize + song.custom_instruments.len()))
            .map(|i| song.instrument_name(i as u8).to_string())
            .collect();
        Self {
            notes,
            cum,
            ticks_per_bar,
            signature,
            instruments,
        }
    }

    /// 整份音符表 + 秒表的指纹（FNV-1a：喂时刻/轨/乐器号/两个声像，再喂出厂 tempo 和每个变速点）。
    ///
    /// 键号、力度、乐器名都不进哈希：改这些不会挪动「哪一槽有几件乐器」的统计结果，
    /// 也没改「多少列算一段」，没必要让特效那边的缓存作废。tempo 和变速点**要**进：
    /// 声道方块那两张表（峰值、段起点）都按秒断，光折音符的话改完速度指纹不变，
    /// 缓存会一直沿用旧速度算出来的段。**同一首曲子两次烘出来必须得同一个数**，
    /// 所以这条只当「重算一次」的触发器，不当校验和用。
    pub fn signature(&self) -> u64 {
        self.signature
    }

    /// 音符总数，也用来判断「烘出来的表是不是空的」。
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    /// 某个乐器号在这首曲子里叫什么。内置乐器给英文名（和原版乐器表同源），
    /// 自定义乐器给用户起的名字；号越界返回空串，调用方自己兜底。
    pub fn instrument_name(&self, instrument: u8) -> &str {
        self.instruments
            .get(instrument as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// 表里有几个乐器（内置 20 + 自定义数）。
    pub fn instrument_count(&self) -> usize {
        self.instruments.len()
    }

    pub fn ticks_per_bar(&self) -> u32 {
        self.ticks_per_bar
    }

    /// 表覆盖到的最后一列。
    pub fn last_tick(&self) -> usize {
        self.cum.len().saturating_sub(1)
    }

    /// 把任意（含播放中浮点）tick 换成秒。越界夹到表的两端：特效拿到的是
    /// 「停在头/尾」，而不是越界索引或 NaN。
    pub fn seconds_at(&self, tick: f64) -> f32 {
        // 只有 NaN 需要特判：±inf 走下面的 `clamp`，正好夹到表头/表尾。
        if self.cum.len() < 2 || tick.is_nan() {
            return 0.0;
        }
        let t = tick.clamp(0.0, (self.last_tick() - 1) as f64);
        let whole = t.floor() as usize;
        let frac = (t - whole as f64) as f32;
        let a = self.cum[whole.min(self.cum.len() - 1)];
        let b = self.cum[(whole + 1).min(self.cum.len() - 1)];
        a + (b - a) * frac
    }

    /// 第 `bar` 小节起点的 tick（原版一小节 = `4 * time_signature` 列）。
    pub fn tick_of_bar(&self, bar: u32) -> u32 {
        bar.saturating_mul(self.ticks_per_bar)
    }

    /// 第 `seconds` 秒落在哪一列。`seconds_at` 单调不减，所以二分即可；
    /// 有变速点时不能用 `position + 秒 * tempo` 那种线性换算。
    pub fn tick_at_seconds(&self, seconds: f64) -> f64 {
        if self.cum.len() < 2 || seconds.is_nan() {
            return 0.0;
        }
        let mut lo = 0.0_f64;
        let mut hi = (self.last_tick() - 1) as f64;
        if seconds <= self.seconds_at(0.0) as f64 {
            return 0.0;
        }
        if seconds >= self.seconds_at(hi) as f64 {
            return hi;
        }
        for _ in 0..48 {
            let mid = (lo + hi) / 2.0;
            if (self.seconds_at(mid) as f64) < seconds {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// `[from, to)` 这段 tick 区间对应的音符下标范围，两端都夹到表内。
    /// 特效每帧只扫可见窗口，不遍历整首歌。
    pub fn window(&self, from: f64, to: f64) -> (usize, usize) {
        if self.notes.is_empty() || !from.is_finite() || !to.is_finite() {
            return (0, 0);
        }
        let lo = from.max(0.0).floor() as i64;
        let hi = to.max(0.0).ceil() as i64;
        let start = self.notes.partition_point(|n| (n.tick as i64) < lo);
        let end = self.notes.partition_point(|n| (n.tick as i64) < hi);
        (start, end.max(start))
    }

    pub fn notes(&self) -> &[FxNote] {
        &self.notes
    }

    /// [`Self::window`] 那一段的切片。
    pub fn notes_in(&self, from: f64, to: f64) -> &[FxNote] {
        let (a, b) = self.window(from, to);
        &self.notes[a..b]
    }

    /// 播放头从 `from` 走到 `to` 这一帧**真正跨过**的音符：`from ≤ tick ≤ to`，
    /// 两端都不取整放宽。`notes_in` 的 floor/ceil 放宽是给画胶囊用的（窗口宽一列
    /// 只是多画几个，反正有 ahead 过滤）；击中判定必须精确，不然同一枚音符在
    /// 它所在的那个整列里每帧都被当成「刚跨过」，一帧一束连撒好几帧。
    /// 相邻两帧的窗口首尾相接（上一帧的 to 就是这一帧的 from），所以一枚音符
    /// 至多中一次；恰好压在起播位置上的那枚也算跨过（原版起播先派发落点列）。
    pub fn notes_crossed(&self, from: f64, to: f64) -> &[FxNote] {
        if self.notes.is_empty() || !from.is_finite() || !to.is_finite() {
            return &[];
        }
        let start = self.notes.partition_point(|n| (n.tick as f64) < from);
        let end = self.notes.partition_point(|n| (n.tick as f64) <= to);
        &self.notes[start..end.max(start)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::{CustomInstrument, Layer, LockMode, Note};

    fn note(instrument: u8, key: u8, pitch: i16) -> Note {
        Note {
            instrument,
            key,
            velocity: 80,
            panning: 100,
            pitch,
        }
    }

    /// 3 t/s、4/4，0..40 列每列一枚 harp 音。
    fn song() -> Song {
        let mut s = Song::new();
        s.meta.tempo = 3.0;
        s.meta.time_signature = 4;
        for t in 0..40u32 {
            s.set(t, 0, note(0, 39, 0));
        }
        s.recalculate();
        s
    }

    /// 挂上「Tempo Changer」自定义乐器（只有索引 >= 20 的同名乐器才算变速点）。
    fn changer(s: &mut Song, tick: u32, layer: u32, pitch: i16) {
        if s.custom_instruments.is_empty() {
            s.custom_instruments.push(CustomInstrument {
                name: nbs_core::TEMPO_CHANGER_NAME.into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        }
        s.set(tick, layer, note(20, 45, pitch));
    }

    #[test]
    fn constant_tempo_is_one_tick_over_tempo() {
        let tl = FxTimeline::bake(&song(), false);
        // 3 t/s ⇒ 一列 0.333… 秒。
        assert!((tl.seconds_at(3.0) - 1.0).abs() < 1e-4);
        assert!((tl.seconds_at(1.5) - 0.5).abs() < 1e-4);
        assert_eq!(tl.seconds_at(0.0), 0.0);
        assert_eq!(tl.note_count(), 40);
    }

    /// 指纹只认「全曲统计看得见的」字段：动声像/乐器要重算，只动音高不该。
    #[test]
    fn the_signature_tracks_what_the_layout_counts() {
        let base = FxTimeline::bake(&song(), false).signature();
        assert_eq!(FxTimeline::bake(&song(), false).signature(), base);

        let mut retuned = song();
        for t in 0..40u32 {
            retuned.set(t, 0, note(0, 60, 0));
        }
        retuned.recalculate();
        assert_eq!(
            FxTimeline::bake(&retuned, false).signature(),
            base,
            "只改音高：声道方块统计不出差别，不该重扫一遍全曲"
        );

        let mut panned = song();
        let mut swapped = song();
        for t in 0..40u32 {
            let mut n = note(0, 39, 0);
            n.panning = 150;
            panned.set(t, 0, n);
            swapped.set(t, 0, note(1, 39, 0));
        }
        panned.recalculate();
        swapped.recalculate();
        assert_ne!(FxTimeline::bake(&panned, false).signature(), base);
        assert_ne!(FxTimeline::bake(&swapped, false).signature(), base);

        // 秒表也进指纹：声道方块那两张表都按**秒**断段/开窗，改速度不改音符。
        let mut faster = song();
        faster.meta.tempo = 6.0;
        assert_ne!(
            FxTimeline::bake(&faster, false).signature(),
            base,
            "改出厂 tempo 会挪动「隔多久算一段」，不该沿用旧速度烘出来的段起点表"
        );

        // 变速点自己不进音符表（`tempo_changer_applies_from_its_own_column_on` 数过 40 枚），
        // 所以这一条只可能由秒表那几笔哈希区分开。
        let mut shifted = song();
        changer(&mut shifted, 20, 1, 30); // 30/15 = 2 t/s
        assert_eq!(FxTimeline::bake(&shifted, false).note_count(), 40);
        assert_ne!(
            FxTimeline::bake(&shifted, false).signature(),
            base,
            "加一个变速点同理"
        );
    }

    #[test]
    fn tempo_changer_applies_from_its_own_column_on() {
        // 原版先派发本列再改速度：第 10 列本身按新速度走完，才轮到第 11 列。
        let mut s = song();
        changer(&mut s, 10, 1, 15); // 15/15 = 1 t/s
        let tl = FxTimeline::bake(&s, false);
        // 前 10 列仍按 3 t/s，第 10 列起每列 1 秒。
        assert!((tl.seconds_at(10.0) - 10.0 / 3.0).abs() < 1e-4);
        assert!((tl.seconds_at(15.0) - (10.0 / 3.0 + 5.0)).abs() < 1e-4);
        // 变速点自己不算音符。
        assert_eq!(tl.note_count(), 40);
    }

    #[test]
    fn built_in_instruments_never_become_a_tempo_changer() {
        // 19 号是内置 Tin Whistle：若被当成变速器，pitch=0 会把全曲钉死在 0.25 t/s。
        let mut s = song();
        for t in 0..10u32 {
            s.set(t, 1, note(19, 45, 0));
        }
        let tl = FxTimeline::bake(&s, false);
        assert!((tl.seconds_at(3.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn same_column_changer_last_one_wins() {
        let mut s = song();
        changer(&mut s, 4, 0, 15); // 1 t/s
        changer(&mut s, 4, 1, 45); // 3 t/s，同列后者生效
        let tl = FxTimeline::bake(&s, false);
        // 生效的是 3 t/s ⇒ 第 34 列比第 31 列晚 1 秒，而不是晚 3 秒。
        assert!((tl.seconds_at(34.0) - tl.seconds_at(31.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn notes_are_sorted_and_windowed() {
        let mut s = Song::new();
        s.meta.tempo = 3.0;
        for tick in [7u32, 2, 40, 2] {
            s.set(tick, 1, note(1, 45, 0));
            s.set(tick, 0, note(0, 39, 0));
        }
        s.recalculate();
        let tl = FxTimeline::bake(&s, false);
        // 第 2 列写了两遍，后者覆盖前者 ⇒ 3 个有音的列 × 2 层。
        assert_eq!(tl.note_count(), 6);
        assert_eq!(
            tl.notes()
                .iter()
                .map(|n| (n.tick, n.layer))
                .collect::<Vec<_>>(),
            vec![(2, 0), (2, 1), (7, 0), (7, 1), (40, 0), (40, 1)],
            "按列排，同列按层号排"
        );
        // 窗口左闭右开：`to = 7.0` 时第 7 列还归下一帧。
        assert_eq!(
            tl.notes_in(2.0, 7.0)
                .iter()
                .map(|n| n.tick)
                .collect::<Vec<_>>(),
            vec![2, 2]
        );
        assert_eq!(
            tl.notes_in(2.0, 7.5)
                .iter()
                .map(|n| n.tick)
                .collect::<Vec<_>>(),
            vec![2, 2, 7, 7],
            "带小数的播放头要能把自己那一列算进去"
        );
        // 窗口越界返回空，不 panic。
        assert!(tl.notes_in(1e6, 1e6 + 1.0).is_empty());
        assert!(tl.notes_in(-5.0, -1.0).is_empty());
        // from > to（往回拖）也是空，不是反向切片。
        assert!(tl.notes_in(9.0, 2.0).is_empty());
    }

    /// 击中判定用的精确窗口：`from ≤ tick ≤ to`，不做 floor/ceil 放宽。
    /// 播放头在一列里要走上好几帧，放宽的窗口会把同一枚音符每帧再报一次。
    #[test]
    fn crossed_notes_use_the_exact_window() {
        let tl = FxTimeline::bake(&song(), false); // 0..39 每列一枚
        assert_eq!(
            tl.notes_crossed(2.9, 3.1)
                .iter()
                .map(|n| n.tick)
                .collect::<Vec<_>>(),
            vec![3],
            "跨过第 3 列的那一帧只报这一枚"
        );
        // 播放头还在同一列里走的后续帧：没有「刚跨过」的音符。
        assert!(tl.notes_crossed(3.1, 3.5).is_empty());
        assert!(tl.notes_crossed(3.5, 3.9).is_empty());
        // 两端都含：恰好压在起播位置上的那枚算跨过；窗口挪过之后不再重复报。
        assert_eq!(tl.notes_crossed(3.0, 3.0).len(), 1);
        assert!(tl.notes_crossed(3.000_001, 3.5).is_empty());
        // 往回走不是「跨过」。
        assert!(tl.notes_crossed(9.0, 2.0).is_empty());
    }

    /// `tick_at_seconds` 是 `seconds_at` 的反函数：特效算「可见窗口到哪一列」全靠它，
    /// 变速之后不能再用 `tick + 秒 × tempo` 那种线性换算。
    #[test]
    fn seconds_and_ticks_convert_both_ways() {
        let tl = FxTimeline::bake(&song(), false);
        for tick in [0.0, 3.0, 12.5, 38.0] {
            let back = tl.tick_at_seconds(tl.seconds_at(tick) as f64);
            assert!((back - tick).abs() < 0.01, "{tick} → {back}");
        }
        // 这首曲子 3 t/s ⇒ 2 秒走 6 列。
        assert!((tl.tick_at_seconds(2.0) - 6.0).abs() < 0.01);

        // 有变速点：变速之后同样的秒数走的列数不一样。
        let mut s = song();
        changer(&mut s, 8, 0, 30); // 第 8 列起 2 t/s
        let tl = FxTimeline::bake(&s, false);
        assert!((tl.tick_at_seconds(tl.seconds_at(8.0) as f64) - 8.0).abs() < 0.01);
        // 第 8 列之后每秒 2 列 ⇒ 再多 4 秒到 8 + 8 = 16 列。
        let at = tl.seconds_at(8.0) as f64 + 4.0;
        assert!((tl.tick_at_seconds(at) - 16.0).abs() < 0.05);
        // 单调不减：秒数越大，列号不会倒退。
        let mut prev = tl.tick_at_seconds(0.0);
        for step in 0..30i32 {
            let sec = step as f64;
            let now = tl.tick_at_seconds(sec);
            assert!(now >= prev - 1e-6, "{sec} 秒处倒退了");
            prev = now;
        }
        // 越界夹到表尾/表头，非法值不炸。
        assert_eq!(tl.tick_at_seconds(-99.0), 0.0);
        assert_eq!(tl.tick_at_seconds(f64::NAN), 0.0);
        assert!(tl.tick_at_seconds(f64::INFINITY) > 0.0);
        // 空曲子的表本来就没内容：换算结果只要落在表内就行。
        let quiet = FxTimeline::bake(&Song::new(), false);
        let got = quiet.tick_at_seconds(3.0);
        assert!(got.is_finite() && (0.0..=quiet.last_tick() as f64).contains(&got), "{got}");
    }

    #[test]
    fn out_of_range_ticks_clamp_instead_of_nan() {
        let tl = FxTimeline::bake(&song(), false);
        let far = tl.seconds_at(1e9);
        assert!(far.is_finite() && far > 0.0, "越界要夹到表尾，实测 {far}");
        assert_eq!(tl.seconds_at(-3.0), 0.0);
        assert_eq!(tl.seconds_at(f64::NAN), 0.0);
        assert_eq!(tl.seconds_at(f64::INFINITY), far);
    }

    /// 「哪些音符会出声」由锁定/独奏那一档说了算，特效表跟着它走：
    /// 否则锁定层要么落胶囊不出声，要么出声了却不落下来。
    #[test]
    fn locked_layers_appear_only_when_they_are_played() {
        let mut s = Song::new();
        s.meta.tempo = 3.0;
        s.layers.push(Layer {
            lock: LockMode::Locked,
            ..Layer::default()
        });
        s.set(1, 0, note(0, 39, 0));
        s.set(2, 1, note(0, 45, 0));
        s.recalculate();
        assert_eq!(FxTimeline::bake(&s, false).note_count(), 1);
        assert_eq!(FxTimeline::bake(&s, true).note_count(), 2);
        // 独奏层反过来：非独奏的那层变成锁定层。
        let mut solo = s.clone();
        solo.layers[0].lock = LockMode::Solo;
        solo.layers[1].lock = LockMode::Normal;
        assert_eq!(FxTimeline::bake(&solo, false).note_count(), 1);
    }

    /// 特效得知道「这枚音符是哪件乐器」：自定义乐器在导出方块表里全是同一块石头，
    /// 只有名字能把它们分开，所以名字必须一起烘进表里。
    #[test]
    fn instrument_names_cover_builtin_and_custom() {
        let mut s = song();
        for name in ["S080", "VGE"] {
            s.custom_instruments.push(CustomInstrument {
                name: name.into(),
                filename: String::new(),
                key: 45,
                press: false,
            });
        }
        let tl = FxTimeline::bake(&s, false);
        assert_eq!(tl.instrument_count(), 22, "内置 20 + 自定义 2");
        assert_eq!(tl.instrument_name(0), "Harp");
        assert_eq!(tl.instrument_name(19), "Oxidized Trumpet");
        assert_eq!(tl.instrument_name(20), "S080");
        assert_eq!(tl.instrument_name(21), "VGE");
        // 号越界：空串，不是 panic，也不是把上一个名字顶上去。
        assert_eq!(tl.instrument_name(22), "");
        assert_eq!(tl.instrument_name(u8::MAX), "");
    }

    #[test]
    fn bars_follow_time_signature() {
        let mut s = song();
        s.meta.time_signature = 3;
        let tl = FxTimeline::bake(&s, false);
        assert_eq!(tl.ticks_per_bar(), 12);
        assert_eq!(tl.tick_of_bar(2), 24);
    }

    /// 声像要一起烘进来：声道方块按「听到哪只耳朵」摆位置，裸的 `note.panning` 会摆错。
    #[test]
    fn pan_survives_the_bake() {
        let mut s = Song::new();
        s.meta.tempo = 3.0;
        // `Song::new()` 自带一条轨，声道改在它身上（再 push 就成第二条了）。
        s.layers[0].stereo = 60;
        let mut n = note(0, 45, 0);
        n.panning = 140;
        s.set(3, 0, n);
        s.recalculate();
        let tl = FxTimeline::bake(&s, false);
        let baked = &tl.notes()[0];
        assert_eq!(baked.panning, 140);
        assert_eq!(baked.layer_stereo, 60);
        // 轨不在正中 ⇒ 往中间拉一半：(60 + 140) / 2 = 100。
        assert_eq!(baked.audible_pan(), 100);
        // 轨在正中 ⇒ 原样。
        let mut flat = s.clone();
        flat.layers[0].stereo = 100;
        assert_eq!(FxTimeline::bake(&flat, false).notes()[0].audible_pan(), 140);
    }

    #[test]
    fn empty_song_bakes_a_usable_table() {
        let tl = FxTimeline::bake(&Song::new(), false);
        assert_eq!(tl.note_count(), 0);
        assert!(tl.seconds_at(12.0).is_finite());
        assert!(tl.notes_in(0.0, 100.0).is_empty());
    }
}
