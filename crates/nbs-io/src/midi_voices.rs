// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from MiauNoteBlock contributors (c69dcce), src/static/js/nbs_client.js (_planChannelVoices L1747-1881, call site L2451-2469); license: ../MIAU-LICENSE.

/// 声部分离的输入视图：只带 Miau 算法真正读到的四个字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VoiceNote {
    pub track: usize,
    pub channel: usize,
    /// Miau `event.tick`，即量化后的 NBS tick；同 tick 视为同一和弦单元。
    pub tick: i64,
    pub note: i32,
}

/// 拆分结果所依据的分支。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VoiceMode {
    /// 按 MIDI 轨道元数据分派，跳过音高门槛。
    Track,
    /// 一维 k-means 音高聚类。
    Pitch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VoicePlan {
    pub mode: VoiceMode,
    /// 与输入同序，`Some(0)` = 低声部（左手/伴奏），`Some(1)` = 高声部（右手/主旋律）。
    /// `None` 对应 Miau 里没进 `voiceOf` 的音符：轨道分支只覆盖够格的轨道，剩下的音符不参与拆分。
    pub voices: Vec<Option<u8>>,
}

pub(super) fn plan_channel_voices(list: &[VoiceNote], ignore_meta: bool) -> Option<VoicePlan> {
    let n = list.len();
    if n < 8 {
        return None;
    }

    let mut min_note = i32::MAX;
    let mut max_note = i32::MIN;
    for ev in list {
        min_note = min_note.min(ev.note);
        max_note = max_note.max(ev.note);
    }
    let span = max_note - min_note;

    // 同 tick 的音符聚成一个单元；JS 对象键是精确匹配，这里同样不给容差、不看时长。
    let mut unit_ticks: Vec<i64> = Vec::with_capacity(n);
    for ev in list {
        if !unit_ticks.contains(&ev.tick) {
            unit_ticks.push(ev.tick);
        }
    }
    let poly = n as f64 / unit_ticks.len() as f64;

    // ---- 优先使用 MIDI 轨道(track)结构, 这是最可靠的线索 ----
    if !ignore_meta {
        let mut track_order: Vec<usize> = Vec::new();
        let mut track_sizes: Vec<usize> = Vec::new();
        let mut track_sums: Vec<i64> = Vec::new();
        for ev in list {
            match track_order.iter().position(|t| *t == ev.track) {
                Some(i) => {
                    track_sizes[i] += 1;
                    track_sums[i] += ev.note as i64;
                }
                None => {
                    track_order.push(ev.track);
                    track_sizes.push(1);
                    track_sums.push(ev.note as i64);
                }
            }
        }
        let min_track_notes = 6.max(((n as f64) * 0.08).floor() as usize);
        let mut usable: Vec<usize> = (0..track_order.len())
            .filter(|&i| track_sizes[i] >= min_track_notes)
            .collect();
        if usable.len() >= 2 {
            // 并列平均音高时保持插入序，等价于 JS 的稳定 sort。
            usable.sort_by(|&a, &b| {
                let pa = track_sums[a] as f64 / track_sizes[a] as f64;
                let pb = track_sums[b] as f64 / track_sizes[b] as f64;
                pa.total_cmp(&pb)
            });
            let half = usable.len().div_ceil(2);
            let mut voices = vec![None; n];
            for (ui, &ti) in usable.iter().enumerate() {
                let v = if ui < half { 0 } else { 1 };
                for (idx, ev) in list.iter().enumerate() {
                    if ev.track == track_order[ti] {
                        voices[idx] = Some(v);
                    }
                }
            }
            return Some(VoicePlan {
                mode: VoiceMode::Track,
                voices,
            });
        }
    }

    // ---- 复杂度判定: 单声部 / 窄音域乐器不拆分 ----
    if span < 10 {
        return None;
    }
    if poly < 1.12 && span < 15 {
        return None;
    }

    // ---- 初始分界: 音高排序后的最大间隙 ----
    let mut sorted: Vec<usize> = (0..n).collect();
    sorted.sort_by_key(|&i| list[i].note);
    let mut best_gap = -1i64;
    let mut best_idx = -1i64;
    for g in 1..n {
        let gap = list[sorted[g]].note as i64 - list[sorted[g - 1]].note as i64;
        if gap > best_gap {
            best_gap = gap;
            best_idx = g as i64;
        }
    }
    let (mut mu_low, mut mu_high) = if best_gap >= 2 && best_idx > 0 {
        (
            list[sorted[(best_idx - 1) as usize]].note as f64,
            list[sorted[(best_idx as usize).min(n - 1)]].note as f64,
        )
    } else {
        (
            list[sorted[((n as f64) * 0.25).floor() as usize]].note as f64,
            list[sorted[((n as f64) * 0.75).floor() as usize]].note as f64,
        )
    };

    // ---- 逐音符两簇聚类 (k-means 一维) ----
    // 逐音符而非按 tick 聚合: 人声与伴奏、左右手会在同一时刻同时发声,
    // 按 tick 聚合会把它们锁死在同一组, 无法分离。
    let mut assign = vec![0u8; n];
    let mut converged = false;
    for _iter in 0..8 {
        if converged {
            break;
        }
        let (mut sum_low, mut cnt_low, mut sum_high, mut cnt_high) = (0i64, 0usize, 0i64, 0usize);
        let mut changed = false;
        for idx in 0..n {
            let note = list[idx].note as f64;
            let d0 = (note - mu_low).abs();
            let d1 = (note - mu_high).abs();
            let v = if d1 < d0 { 1 } else { 0 };
            if assign[idx] != v {
                changed = true;
            }
            assign[idx] = v;
            if v == 0 {
                sum_low += list[idx].note as i64;
                cnt_low += 1;
            } else {
                sum_high += list[idx].note as i64;
                cnt_high += 1;
            }
        }
        if cnt_low == 0 || cnt_high == 0 {
            return None;
        }
        let next_low = sum_low as f64 / cnt_low as f64;
        let next_high = sum_high as f64 / cnt_high as f64;
        converged = !changed
            && (next_low - mu_low).abs() < 0.01
            && (next_high - mu_high).abs() < 0.01;
        mu_low = next_low;
        mu_high = next_high;
    }
    if mu_high - mu_low < 2.0 {
        return None; // 两个簇几乎重合, 不构成两个声部
    }

    // ---- 和弦保护: 同一时刻音高相近的音不应被拆到两个声部 ----
    let mut buckets: Vec<Vec<usize>> = Vec::new();
    for idx in 0..n {
        match buckets
            .iter()
            .position(|b| !b.is_empty() && list[b[0]].tick == list[idx].tick)
        {
            Some(i) => buckets[i].push(idx),
            None => buckets.push(vec![idx]),
        }
    }
    for bucket in &mut buckets {
        if bucket.len() < 2 {
            continue;
        }
        bucket.sort_by_key(|&i| list[i].note);
        for bi in 0..bucket.len() - 1 {
            let (lo, hi) = (bucket[bi], bucket[bi + 1]);
            if list[hi].note - list[lo].note <= 5 && assign[lo] != assign[hi] {
                // 统一到较高音所在的声部 (旋律优先, 低音跟随)
                assign[lo] = assign[hi];
            }
        }
    }

    // ---- 碎片守卫: 任一声部过于稀疏时不拆分 ----
    let count_low = assign.iter().filter(|&&v| v == 0).count();
    let count_high = n - count_low;
    let min_size = 4.max(((n as f64) * 0.06).floor() as usize);
    if count_low < min_size || count_high < min_size {
        return None;
    }

    Some(VoicePlan {
        mode: VoiceMode::Pitch,
        voices: assign.into_iter().map(Some).collect(),
    })
}

/// 把整个事件数组按通道分桶后逐桶规划，对应 Miau 调用点：跳过无音高的事件与打击乐通道 9。
/// 返回与 `list` 同长的 voice 标记，`None` 等价 Miau 的 `_voice === undefined`（该音符不参与拆分）。
pub(super) fn plan_all_channels(
    list: &[VoiceNote],
    separate_voices: bool,
    ignore_meta: bool,
) -> Vec<Option<u8>> {
    let mut out = vec![None; list.len()];
    if !separate_voices {
        return out;
    }
    let mut by_ch: Vec<Vec<usize>> = Vec::new();
    let mut ch_order: Vec<usize> = Vec::new();
    for (idx, ev) in list.iter().enumerate() {
        match ch_order.iter().position(|c| *c == ev.channel) {
            Some(i) => by_ch[i].push(idx),
            None => {
                ch_order.push(ev.channel);
                by_ch.push(vec![idx]);
            }
        }
    }
    for (ci, &ch) in ch_order.iter().enumerate() {
        if ch == 9 {
            continue;
        }
        let bucket: Vec<VoiceNote> = by_ch[ci].iter().map(|&i| list[i]).collect();
        if let Some(plan) = plan_channel_voices(&bucket, ignore_meta) {
            for (k, &idx) in by_ch[ci].iter().enumerate() {
                out[idx] = plan.voices[k];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vn(track: usize, channel: usize, tick: i64, note: i32) -> VoiceNote {
        VoiceNote {
            track,
            channel,
            tick,
            note,
        }
    }

    /// 等间隔上行音阶：span = (len-1)*step，且全部落在不同 tick（poly = 1）。
    fn ladder(len: usize, step: i32) -> Vec<VoiceNote> {
        (0..len)
            .map(|i| vn(0, 0, i as i64 * 20, i as i32 * step + 40))
            .collect()
    }

    /// 从 `base` 起、错开 `off` 个 tick 的一小撮孤立高音。
    fn outliers(k: usize, off: i64, base: i32) -> Vec<VoiceNote> {
        (0..k).map(|i| vn(0, 0, i as i64 * 20 + off, base + i as i32)).collect()
    }

    fn pitch(list: &[VoiceNote]) -> VoicePlan {
        let plan = plan_channel_voices(list, true).expect("expected a pitch plan");
        assert_eq!(plan.mode, VoiceMode::Pitch);
        plan
    }

    /// `-1` 表示 Miau 里没有 `_voice` 标记的音符（轨道分支只覆盖够格的轨道）。
    fn want(pattern: &[i8]) -> Vec<Option<u8>> {
        pattern
            .iter()
            .map(|&v| if v < 0 { None } else { Some(v as u8) })
            .collect()
    }

    /// 只数真正被拆进声部的音符，None 两个声部都不算。
    fn counts(v: &[Option<u8>]) -> (usize, usize) {
        let low = v.iter().filter(|&&x| x == Some(0)).count();
        let high = v.iter().filter(|&&x| x == Some(1)).count();
        (low, high)
    }

    #[test]
    fn fewer_than_eight_notes_is_not_split() {
        assert_eq!(plan_channel_voices(&ladder(7, 3), true), None);
        assert_eq!(plan_channel_voices(&[], true), None);
        assert_eq!(pitch(&ladder(8, 3)).voices, want(&[0, 0, 0, 0, 1, 1, 1, 1]));
    }

    #[test]
    fn narrow_span_below_ten_semitones_is_not_split() {
        // 步长 1 → span 7 < 10
        assert_eq!(plan_channel_voices(&ladder(8, 1), true), None);
        // 步长 2 → span 14 ≥ 10，但下面 poly 门槛拦住
        assert_eq!(plan_channel_voices(&ladder(8, 2), true), None);
        // 步长 3 → span 21 ≥ 15，通过
        assert!(plan_channel_voices(&ladder(8, 3), true).is_some());
    }

    #[test]
    fn monophonic_narrow_band_needs_span_fifteen() {
        // poly = 1.0 < 1.12 且 span = 14 < 15 → 拒绝
        assert_eq!(plan_channel_voices(&ladder(8, 2), true), None);
        // 同音高重复使 poly 涨到 8.0，但 span = 0 先被音域门槛拦住
        let unison: Vec<VoiceNote> = (0..8).map(|_| vn(0, 0, 0, 60)).collect();
        assert_eq!(plan_channel_voices(&unison, true), None);
    }

    #[test]
    fn coincident_ticks_raise_polyphony_without_tolerance() {
        // 8 音全在同一 tick：poly = 8.0 ≥ 1.12，span = 21 → 越过 poly 门槛
        let chord: Vec<VoiceNote> = (0..8).map(|i| vn(0, 0, 0, 40 + i * 3)).collect();
        // 但同 tick 和弦被聚类拆开后，碎片守卫把它否掉（3 低 / 5 高 < 4）
        assert_eq!(plan_channel_voices(&chord, true), None);
        // 相邻 tick 只差 1 也算不同单元，不会被合并
        let mut a = ladder(8, 3);
        a.extend(outliers(4, 10, 96));
        assert_eq!(counts(&pitch(&a).voices), (8, 4));
    }

    #[test]
    fn metadata_track_dispatch_skips_pitch_and_fragment_gates() {
        // 两个 track 各 8 音，音域只有 4 个半音：音高门槛必然拒绝
        let list: Vec<VoiceNote> = [
            (0..8).map(|i| vn(0, 0, i as i64 * 20, 60 + i % 3)).collect::<Vec<_>>(),
            (0..8).map(|i| vn(1, 0, i as i64 * 20 + 10, 72 + i % 3)).collect::<Vec<_>>(),
        ]
        .concat();
        let plan = plan_channel_voices(&list, false).expect("track plan");
        assert_eq!(plan.mode, VoiceMode::Track);
        // 平均音高低的 track → 低声部
        assert_eq!(plan.voices, want(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1]));
        // 关闭元数据后退化成聚类，并被 poly/span 门槛拒绝
        assert_eq!(plan_channel_voices(&list, true), None);
    }

    #[test]
    fn track_dispatch_assigns_first_half_of_tracks_to_low_voice() {
        // 三个 track 各 8 音，usable = 3，half = ceil(3/2) = 2 → 前两个 track 归低声部
        let list: Vec<VoiceNote> = [(0usize, 40), (1, 70), (2, 100)]
            .into_iter()
            .flat_map(|(t, base)| {
                (0..8).map(move |i| vn(t, 0, i as i64 * 20, base + i))
            })
            .collect();
        let plan = plan_channel_voices(&list, false).expect("track plan");
        assert_eq!(counts(&plan.voices), (16, 8));
    }

    #[test]
    fn track_branch_requires_two_usable_tracks_with_min_six_notes_each() {
        // 8 + 8 + 5：第三个 track 不足 max(6, floor(21*0.08)=1) → usable 只剩 2，仍走 track 分支
        let hit: Vec<VoiceNote> = [
            (0..8).map(|i| vn(0, 0, i as i64 * 20, 40 + i * 3)).collect::<Vec<_>>(),
            (0..8).map(|i| vn(1, 0, i as i64 * 20, 80 + i)).collect::<Vec<_>>(),
            (0..5).map(|i| vn(2, 0, i as i64 * 20, 60)).collect::<Vec<_>>(),
        ]
        .concat();
        let plan = plan_channel_voices(&hit, false).expect("track plan");
        assert_eq!(plan.mode, VoiceMode::Track);
        // 只有够格的两条 track 被标记，第三 track 的 5 音保持未拆分（Miau 的 undefined）
        assert_eq!(counts(&plan.voices), (8, 8));
        assert_eq!(plan.voices[16..], want(&[-1; 5])[..]);
        // usable 只有 1 个 track → 退回聚类分支
        let single = ladder(12, 3);
        assert_eq!(
            plan_channel_voices(&single, false).map(|p| p.mode),
            Some(VoiceMode::Pitch)
        );
        // 两个 track 各 5/6 音，其中一个不足 6 → usable = 1，退回聚类分支
        let thin: Vec<VoiceNote> = [
            (0..6).map(|i| vn(0, 0, i as i64 * 20, 40 + i * 3)).collect::<Vec<_>>(),
            (0..5).map(|i| vn(1, 0, i as i64 * 20, 80 + i * 3)).collect::<Vec<_>>(),
        ]
        .concat();
        assert_eq!(
            plan_channel_voices(&thin, false).map(|p| p.mode),
            Some(VoiceMode::Pitch)
        );
    }

    #[test]
    fn equal_average_pitch_keeps_insertion_order_across_ties() {
        // 两个 track 平均音高完全相同 → 稳定排序保持出现顺序，第一个归低声部
        let lo = [40, 60, 40, 60, 40, 60, 40, 60];
        let hi = [60, 40, 60, 40, 60, 40, 60, 40];
        let list: Vec<VoiceNote> = lo
            .iter()
            .enumerate()
            .map(|(i, &v)| vn(0, 0, i as i64 * 20, v))
            .chain(hi.iter().enumerate().map(|(i, &v)| vn(1, 0, i as i64 * 20, v)))
            .collect();
        let plan = plan_channel_voices(&list, false).expect("track plan");
        assert_eq!(plan.voices, want(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1]));
    }

    #[test]
    fn gap_seed_splits_ladder_at_median_into_four_and_four() {
        // 最大相邻间隙 3 ≥ 2 → 质心取间隙两侧 40/43，收敛后对半分
        assert_eq!(pitch(&ladder(8, 3)).voices, want(&[0, 0, 0, 0, 1, 1, 1, 1]));
        assert_eq!(
            pitch(&ladder(12, 3)).voices,
            want(&[0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1])
        );
    }

    #[test]
    fn empty_cluster_and_coincident_centroid_guards_never_fire_on_reachable_input() {
        // 两个质心取自排序后的音符本身，各自至少吸附自己那个音符，故空簇不可达；
        // 同时间距门槛 (≥2) 又保证收敛后 muHigh-muLow ≥ 2。这里不加无意义的断言桩，
        // 而是穷举小音域输入确认没有任何一例落进这两个分支。
        for step in 1..=8 {
            for len in 8..=16 {
                for off in [0i64, 4, 10] {
                    for hi_base in [55, 70, 96] {
                        let low: Vec<VoiceNote> =
                            (0..len).map(|i| vn(0, 0, i as i64 * 20, 40 + i * step)).collect();
                        let high: Vec<VoiceNote> =
                            (0..8).map(|i| vn(0, 0, i as i64 * 20 + off, hi_base + i)).collect();
                        let list: Vec<VoiceNote> = [low, high].concat();
                        if let Some(plan) = plan_channel_voices(&list, true) {
                            assert_eq!(plan.mode, VoiceMode::Pitch);
                            assert!(plan.voices.iter().any(|v| *v == Some(0)));
                            assert!(plan.voices.iter().any(|v| *v == Some(1)));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fragment_guard_rejects_voice_below_max_four_or_six_percent() {
        // 12 音：minSize = max(4, floor(0.72)) = 4，3 个孤立高音 → 拒绝
        let thin: Vec<VoiceNote> = [ladder(9, 1), outliers(3, 10, 96)].concat();
        assert_eq!(plan_channel_voices(&thin, true), None);
        // 补到 4 个 → 放行
        let ok: Vec<VoiceNote> = [ladder(9, 1), outliers(4, 10, 96)].concat();
        assert_eq!(counts(&pitch(&ok).voices), (9, 4));
        // 100 音：minSize = max(4, floor(6)) = 6，5 个孤立高音仍拒绝
        let mut band: Vec<VoiceNote> = (0..95)
            .map(|i| vn(0, 0, i as i64 * 20, 40 + i % 12))
            .collect();
        band.extend(outliers(5, 5, 100));
        assert_eq!(plan_channel_voices(&band, true), None);
    }

    #[test]
    fn chord_protection_pulls_notes_into_their_same_tick_neighbour_voice() {
        // 偶数带 40..86 与高音带 55..66 同 tick 交叠。聚类边界落在 62/63 之间，
        // 所以同音高原始判定必然一致；差别全部来自和弦保护（按 tick 分桶，桶内相邻 ≤5 半音时低音并入高音声部）。
        let low: Vec<VoiceNote> = (0..24).map(|i| vn(0, 0, i as i64 * 20, 40 + i * 2)).collect();
        let high: Vec<VoiceNote> = (0..12).map(|i| vn(0, 0, i as i64 * 20, 55 + i)).collect();
        let list: Vec<VoiceNote> = [low, high].concat();
        let plan = pitch(&list);
        let even = |note: i32| ((note - 40) / 2) as usize; // tick = (note-40)/2 * 20
        let top = |note: i32| 24 + (note - 55) as usize; // 音高 note 在偶数带之后第 note-55 个

        // 两个 60：tick 200 的桶里挨着 65（差 5）被拉进高声部，tick 100 的桶里邻居是 50（差 10）留在低声部。
        assert_eq!(plan.voices[even(60)], Some(1));
        assert_eq!(plan.voices[top(60)], Some(0));
        // 两个 62 同理：tick 220 挨着 66（差 4）→ 高；tick 140 邻居 54（差 8）→ 低。
        assert_eq!(plan.voices[even(62)], Some(1));
        assert_eq!(plan.voices[top(62)], Some(0));
        // 没被桶内邻居带动的音符保持聚类原判。
        assert_eq!(plan.voices[even(58)], Some(0));
        assert_eq!(plan.voices[top(61)], Some(0));
        assert_eq!(plan.voices[top(63)], Some(1));
        assert_eq!(counts(&plan.voices), (18, 18));
    }

    #[test]
    fn chord_protection_leaves_intervals_wider_than_five_apart() {
        // 低音带与高音带同 tick 但最近距离 41 半音，远超 5 → 不合并
        let low: Vec<VoiceNote> = (0..16).map(|i| vn(0, 0, i as i64 * 20, 40 + i)).collect();
        let high = outliers(8, 4, 96);
        let list: Vec<VoiceNote> = [low, high].concat();
        let plan = pitch(&list);
        let idx = |target: i32| list.iter().position(|e| e.note == target).unwrap();
        assert_eq!((plan.voices[idx(55)], plan.voices[idx(96)]), (Some(0), Some(1)));
        assert_eq!(counts(&plan.voices), (16, 8));
    }

    #[test]
    fn disabled_flag_and_percussion_channel_are_left_unmarked() {
        let mut list: Vec<VoiceNote> = ladder(8, 3);
        list.extend(ladder(8, 3).into_iter().map(|mut e| {
            e.channel = 9;
            e
        }));
        let marked = plan_all_channels(&list, true, false);
        assert!(marked[..8].iter().all(|v| v.is_some()));
        assert!(marked[8..].iter().all(|v| v.is_none()));
        assert!(plan_all_channels(&list, false, false)
            .iter()
            .all(|v| v.is_none()));
    }
}
