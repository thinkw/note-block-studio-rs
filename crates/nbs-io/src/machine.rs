//! 机器布局的公共底层：时间轴侧。
//!
//! 这一层只回答两个问题，跟方块长什么样无关：
//! 1. 一首 `.nbs` 换算到 game tick 平面后，音符落在哪些整数刻上（Tempo Changer
//!    也在这一步消化，走的是 [`crate::maestro`] 那套已验证的换算，不另写一份）；
//! 2. 给定某种延迟元件的最小间距，这些事件要几条平行道才铺得下（道数就是这台机器的
//!    多声部预算）。
//!
//! 「同一时刻能同时响几个音」在这里变成一个可测的整数，而它正是各家用不同布局换来的
//! 那个东西：中继器机器用 2 game tick 的间隔换简单，1 game tick 的元件用道数换密度。

use nbs_core::model::{Song, FIRST_CUSTOM_INDEX, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

/// 一种布局能兑现的最小信号间隔，决定这首歌铺不铺得开。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeBase {
    /// 只有红石中继器：最小延迟 1 redstone tick = 2 game tick，所以封顶 10 notes/s。
    /// 仓库现有的三种布局与直轨都在这一档。
    Repeater,
    /// 能把信号挪到奇数 game tick 的元件（超能力靠「奇偶两层各配一套定时器」，
    /// 不是靠单个中继器）：封顶 20 notes/s。
    /// 注意：这一档至今没有可引用的方块延迟依据——wiki 只给了中继器 2 gt 的整数倍，
    /// 以及「粘性活塞推红石块 = 1.5 redstone tick = 3 gt」这一个亚中继器元件；
    /// 观察者脉冲长 2 gt、但当前版本正文没写它的响应延迟是 0 还是 1 gt。
    /// 所以本档只用于「这首歌铺不开」的判定，还没有布局兑现它。
    HalfTick,
}

impl TimeBase {
    /// 相邻两格信号能拉开的最小 game tick 数。
    pub fn min_gap_gt(self) -> i64 {
        match self {
            TimeBase::Repeater => 2,
            TimeBase::HalfTick => 1,
        }
    }

    /// 该时间基铺得下的最高歌曲速度（t/s）；等于每秒 game tick 数除以最小间隔。
    pub fn max_tempo(self) -> f64 {
        20.0 / self.min_gap_gt() as f64
    }

    /// 换算后的 game tick 平面上最快的相邻间隔是否越得过这条线。
    pub fn accepts(self, gt_per_tick: f64) -> bool {
        gt_per_tick + f64::EPSILON >= self.min_gap_gt() as f64
    }
}

/// game tick 平面上的一条音符事件。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    /// 绝对 game tick（从 0 起）。
    pub gt: i64,
    /// 原 NBS tick，保留它是为了让报告能指回谱面。
    pub tick: u32,
    pub layer: u32,
    pub instrument: u8,
    pub key: u8,
    pub velocity: u8,
}

/// 换算结果：事件流 + 换算期间的取舍计数。
///
/// 计数口径与 [`crate::schematic::SchematicStats`] 的三问对齐，但这里的「丢弃」是
/// 真丢弃：`out_of_range` 的音符不进事件流，因为音符盒只有 25 档音高，摆进去是错音
/// 而不是少音。
#[derive(Clone, Debug)]
pub struct EventPlane {
    /// 按 `gt` 升序；同一 `gt` 内按图层升序。
    pub events: Vec<Event>,
    /// 最后一个有音符的 game tick，空曲为 0。
    pub last_gt: i64,
    /// 全曲最小的「每 NBS tick 占几 game tick」， Tempo Changer 会让它逐段变。
    pub min_gt_per_tick: f64,
    /// 音域在音符盒 25 档之外而丢弃的音符数。
    pub dropped_out_of_range: usize,
    /// 图层锁定而丢弃的音符数（`include_locked` 为真时恒为 0）。
    pub dropped_locked: usize,
    /// 自定义乐器：留下来了，但落到方块上只能按内置第一种（harp）发声。
    pub mapped_custom: usize,
    /// 带微调音分的音符数；音符盒装不下，导出时按原音高取整。
    pub pitched_ignored: usize,
}

impl EventPlane {
    /// 换算一首歌。`include_locked` 与 [`crate::schematic::SchematicOptions`] 的
    /// 同名选项同义：为假时非独奏层的音符一律不进事件流。
    pub fn build(song: &Song, include_locked: bool) -> Self {
        let changers = crate::maestro::tempo_changer_ids(song);
        let seg = crate::maestro::tempo_segments(song, &changers);
        let map = crate::maestro::tick_map(song, &seg);
        let locked = song.locked_layers();
        let mut events: Vec<Event> = Vec::new();
        let mut plane = Self {
            events: Vec::new(),
            last_gt: 0,
            min_gt_per_tick: f64::INFINITY,
            dropped_out_of_range: 0,
            dropped_locked: 0,
            mapped_custom: 0,
            pitched_ignored: 0,
        };
        for (tick, layer, note) in song.iter_notes() {
            let Some(gt) = map.get(tick as usize).copied() else {
                continue;
            };
            let gt = gt.round() as i64;
            if gt > plane.last_gt {
                plane.last_gt = gt;
            }
            if tick > 0 {
                if let Some(&prev) = map.get(tick as usize - 1) {
                    let step = gt as f64 - prev;
                    if step < plane.min_gt_per_tick {
                        plane.min_gt_per_tick = step;
                    }
                }
            }
            if !include_locked && locked.get(layer as usize).copied().unwrap_or(false) {
                plane.dropped_locked += 1;
                continue;
            }
            if note.key < MC_KEY_MIN || note.key > MC_KEY_MAX || note.key > KEY_MAX {
                plane.dropped_out_of_range += 1;
                continue;
            }
            if note.instrument >= FIRST_CUSTOM_INDEX {
                plane.mapped_custom += 1;
            }
            if note.pitch != 0 {
                plane.pitched_ignored += 1;
            }
            events.push(Event {
                gt,
                tick,
                layer,
                instrument: note.instrument,
                key: note.key,
                velocity: note.velocity,
            });
        }
        events.sort_by(|a, b| (a.gt, a.layer).cmp(&(b.gt, b.layer)));
        if plane.min_gt_per_tick.is_infinite() {
            // 单 tick 的曲子（或空曲）没有相邻间隔可测，按出厂速度算。
            plane.min_gt_per_tick = if song.meta.tempo > 0.0 {
                20.0 / song.meta.tempo as f64
            } else {
                0.0
            };
        }
        plane.events = events;
        plane
    }

    /// 该平面在当前时间基下铺不铺得开，铺不开时给出行得通的最低速度档。
    pub fn fit_check(&self, base: TimeBase) -> Result<(), String> {
        if base.accepts(self.min_gt_per_tick) {
            return Ok(());
        }
        Err(crate::tr(
            &format!(
                "The notes come closer than one {} apart ({} notes per second at the fastest point), which this machine cannot build.",
                if base == TimeBase::Repeater { "repeater tick (2 game ticks)" } else { "game tick" },
                (20.0 / self.min_gt_per_tick).floor().max(1.0),
            ),
            &format!(
                "最密处相邻音符只隔 {:.2} 个 game tick（约每秒 {:.1} 音），比{}还近，这台机器铺不开。",
                self.min_gt_per_tick,
                (20.0 / self.min_gt_per_tick).floor().max(1.0),
                if base == TimeBase::Repeater { "中继器最小延迟（2 game tick）" } else { "一个 game tick" },
            ),
        )
        .to_string())
    }
}

/// 把事件按「同道内两格至少隔 `spacing_gt`」分到平行道里。
///
/// 按 `gt` 升序贪心：放进第一条已经空出位置的道，放不下才新开一条。这样得到的道数
/// 等于「任意 `spacing_gt` 窗口内同时存在的最多事件数」，也就是理论下界，不用再搜。
pub fn assign_lanes(events: &[Event], spacing_gt: i64) -> Vec<Vec<Event>> {
    assert!(spacing_gt > 0, "延迟元件的间距得是正数");
    let mut lanes: Vec<Vec<Event>> = Vec::new();
    for e in events {
        let free = lanes.iter().position(|lane| {
            lane.last()
                .map(|prev| e.gt - prev.gt >= spacing_gt)
                .unwrap_or(true)
        });
        match free {
            Some(i) => lanes[i].push(*e),
            None => lanes.push(vec![*e]),
        }
    }
    lanes
}

/// [`assign_lanes`] 的道数，供尺寸计算与报告直接取用。
pub fn lane_count(events: &[Event], spacing_gt: i64) -> usize {
    assign_lanes(events, spacing_gt).len()
}

/// 同一 `gt` 上的事件（和弦）。布局拿它决定「这一拍要挂几个音符盒」。
pub fn by_game_tick(events: &[Event]) -> Vec<(i64, Vec<&Event>)> {
    let mut out: Vec<(i64, Vec<&Event>)> = Vec::new();
    for e in events {
        match out.last_mut() {
            Some((gt, group)) if *gt == e.gt => group.push(e),
            _ => out.push((e.gt, vec![e])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::{Layer, LockMode, Note};

    fn song(tempo: f32, notes: &[(u32, u32, u8, u8)]) -> Song {
        let mut s = Song::new();
        s.meta.tempo = tempo;
        let max_layer = notes.iter().map(|n| n.1).max().unwrap_or(0) as usize;
        s.layers = vec![Layer::default(); max_layer + 1];
        for (t, l, ins, key) in notes {
            s.set(*t, *l, Note::new(*ins, *key));
        }
        s.recalculate();
        s
    }

    fn ev(gt: i64, layer: u32) -> Event {
        Event {
            gt,
            tick: 0,
            layer,
            instrument: 0,
            key: 45,
            velocity: 100,
        }
    }

    #[test]
    fn tempo_sets_the_game_tick_stride() {
        // 出厂 10 t/s 就是一 tick 两格；20 t/s 才是一 tick 一格。
        let slow = EventPlane::build(&song(10.0, &[(0, 0, 0, 45), (1, 0, 0, 45), (2, 0, 0, 45)]), false);
        assert_eq!(slow.min_gt_per_tick, 2.0);
        assert_eq!(
            slow.events.iter().map(|e| e.gt).collect::<Vec<_>>(),
            vec![0, 2, 4]
        );
        let fast = EventPlane::build(&song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 45), (2, 0, 0, 45)]), false);
        assert_eq!(fast.min_gt_per_tick, 1.0);
        assert_eq!(
            fast.events.iter().map(|e| e.gt).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn half_tick_base_only_opens_at_twenty_notes_per_second() {
        let s = song(20.0, &[(0, 0, 0, 45), (1, 0, 0, 45)]);
        let plane = EventPlane::build(&s, false);
        assert!(plane.fit_check(TimeBase::Repeater).is_err());
        assert!(plane.fit_check(TimeBase::HalfTick).is_ok());
        // 中继器档在 10 t/s 上刚好够用，再快就该拒。
        let ok = EventPlane::build(&song(10.0, &[(0, 0, 0, 45), (1, 0, 0, 45)]), false);
        assert!(ok.fit_check(TimeBase::Repeater).is_ok());
        assert_eq!(TimeBase::Repeater.max_tempo(), 10.0);
        assert_eq!(TimeBase::HalfTick.max_tempo(), 20.0);
    }

    #[test]
    fn fractional_tempo_rounds_without_drifting() {
        // 12 t/s 时每 tick 占 1.667 格：换算得用累计值取整，不能每步各取一次整，
        // 否则后半曲会越拖越靠前。
        let plane = EventPlane::build(
            &song(12.0, &(0..12).map(|t| (t, 0, 0, 45)).collect::<Vec<_>>()),
            false,
        );
        let got: Vec<i64> = plane.events.iter().map(|e| e.gt).collect();
        assert_eq!(got, vec![0, 2, 3, 5, 7, 8, 10, 12, 13, 15, 17, 18]);
        assert_eq!(got.last(), Some(&18), "12 tick 该落在第 18 格附近");
    }

    #[test]
    fn out_of_range_notes_are_dropped_and_counted() {
        // 音符盒只有 MC_KEY_MIN..=MC_KEY_MAX 那 25 档，外面的音在现有导出里是丢弃。
        let plane = EventPlane::build(
            &song(10.0, &[(0, 0, 0, 32), (1, 0, 0, 33), (2, 0, 0, 57), (3, 0, 0, 58)]),
            false,
        );
        assert_eq!(plane.events.len(), 2);
        assert_eq!(plane.dropped_out_of_range, 2);
    }

    #[test]
    fn custom_instruments_survive_as_harp_notes() {
        // 与现有结构导出同口径：自定义乐器不丢音，但方块上只能按 harp 发声，
        // 所以这里计数、不丢弃。
        let plane = EventPlane::build(&song(10.0, &[(0, 0, FIRST_CUSTOM_INDEX, 45)]), false);
        assert_eq!(plane.events.len(), 1);
        assert_eq!(plane.mapped_custom, 1);
        assert_eq!(plane.dropped_out_of_range, 0);
    }

    #[test]
    fn locked_layers_only_drop_when_the_option_is_off() {
        // 图层锁：solo 之外的层在 include_locked=false 时不进事件流。
        let mut s = song(10.0, &[(0, 0, 0, 45), (0, 1, 0, 47)]);
        s.layers[1].lock = LockMode::Locked;
        s.recalculate();
        let off = EventPlane::build(&s, false);
        let on = EventPlane::build(&s, true);
        assert_eq!(off.events.len(), 1, "锁住的第二层不该进来：{:?}", off.events);
        assert_eq!(off.dropped_locked, 1);
        assert_eq!(on.events.len(), 2);
        assert_eq!(on.dropped_locked, 0);
    }

    #[test]
    fn lanes_keep_every_pair_on_one_lane_apart() {
        let events = vec![ev(0, 0), ev(0, 1), ev(1, 0), ev(2, 0), ev(3, 0), ev(3, 1)];
        let lanes = assign_lanes(&events, 2);
        for lane in &lanes {
            for pair in lane.windows(2) {
                assert!(pair[1].gt - pair[0].gt >= 2, "{lane:?}");
            }
        }
        // 事件一个都不能少，顺序也不能乱。
        let flat: Vec<Event> = lanes.into_iter().flatten().collect();
        assert_eq!(flat.len(), events.len());
    }

    #[test]
    fn greedy_lane_count_hits_the_window_lower_bound() {
        // 下界：任意长度等于间距的窗口里最多同时有几个事件，就至少要几条道。
        // 对密集小样本穷举，确认贪心没多开支票。
        for seed in 0..64u64 {
            let mut state = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
            let mut next = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let n = 3 + (next() % 9) as usize;
            let mut events: Vec<Event> = (0..n)
                .map(|i| ev((next() % 12) as i64, i as u32))
                .collect();
            events.sort_by_key(|e| (e.gt, e.layer));
            for spacing in 1..=5i64 {
                let got = lane_count(&events, spacing);
                let want = events
                    .iter()
                    .map(|e| {
                        events
                            .iter()
                            .filter(|o| o.gt >= e.gt && o.gt < e.gt + spacing)
                            .count()
                    })
                    .max()
                    .unwrap_or(1);
                assert_eq!(got, want, "spacing {spacing}, events {events:?}");
            }
        }
    }

    #[test]
    fn same_game_tick_notes_group_as_one_chord() {
        let events = vec![ev(4, 0), ev(4, 1), ev(6, 0)];
        let chords = by_game_tick(&events);
        assert_eq!(chords.len(), 2);
        assert_eq!(chords[0].1.len(), 2);
        assert_eq!(chords[1].1.len(), 1);
        assert_eq!(chords[0].0, 4);
    }

    #[test]
    fn empty_song_yields_an_empty_plane_without_panicking() {
        let plane = EventPlane::build(&song(10.0, &[]), false);
        assert!(plane.events.is_empty());
        assert_eq!(plane.last_gt, 0);
        assert_eq!(plane.min_gt_per_tick, 2.0);
        assert!(plane.fit_check(TimeBase::Repeater).is_ok());
    }
}
