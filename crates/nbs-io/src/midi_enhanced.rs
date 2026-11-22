// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from MiauNoteBlock contributors (c69dcce), src/static/js/nbs_client.js and modules/28-midi-dialog.js; license: ../MIAU-LICENSE.

use nbs_core::model::{Song, FIRST_CUSTOM_INDEX, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::midi::{
    import_midi_data, import_resolved, resolve, MidiData, MidiImportOptions, Resolved, SustainTail,
};
use crate::midi_miau::{DEFAULT_SUBSTITUTES, GM_COMBINATIONS, INSTRUMENT_OFFSETS};
use crate::midi_voices::{plan_all_channels, VoiceNote};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeMode {
    /// Apply manual transpose only; no substitution or folding.
    Keep,
    /// Apply manual transpose, then fold each note by octaves.
    Fold,
    /// Substitute first; recommend shifts in whole octaves.
    Octave,
    /// Substitute first; recommend shifts in semitones.
    Semitone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiSlot {
    /// Vanilla NBS instrument 0..19; None is silent.
    pub instrument: Option<u8>,
    /// Independent NBS volume percentage (0..100), not timbre/velocity sampling.
    pub volume: u8,
}

impl Default for MidiSlot {
    fn default() -> Self {
        Self {
            instrument: None,
            volume: 100,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelCombination {
    pub slots: [MidiSlot; 3],
    /// Semitones; importing never overwrites a manual value with a recommendation.
    pub transpose: i8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstrumentSubstitution {
    pub high: Option<u8>,
    pub low: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SustainMode {
    /// 不动尾音，沿用传统导入的「每 tick 一块」。
    #[default]
    Off,
    /// 所有轨道的长音都按包络重敲。Miau 的 `'all'` 只保持时长、不套包络，这里相当于 `'all'` 再加渐弱。
    All,
    /// 只有 `sustain_tracks` 列出的 MIDI 轨按包络重敲，对应 Miau 的 `'sustain'`。
    Tracks,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SustainDensity {
    /// 重敲间隔 = 该乐器采样实测余音换算的 tick 数。
    #[default]
    Measured,
    /// 每 tick 一块，和 Miau 的 `for (t = 1; t < noteLength; t++)` 同一步长。
    EveryTick,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedMidiOptions {
    pub enabled: bool,
    pub range_mode: RangeMode,
    pub smart_substitute: bool,
    pub force_fold: bool,
    /// 哪些长音改按实测衰减重敲；Miau 的 `keep_note_length` 三态。
    pub sustain: SustainMode,
    /// `SustainMode::Tracks` 的白名单，存 MIDI 轨下标。
    pub sustain_tracks: Vec<usize>,
    /// Taper those retriggers along the measured envelope; off holds them at one level.
    pub sustain_fade: bool,
    /// 重敲排多密；`EveryTick` 是为了和 Miau 对齐而留的档位。
    pub sustain_density: SustainDensity,
    /// Split one MIDI channel into two NBS voices, following Miau's `separate_voices`.
    pub separate_voices: bool,
    /// Ignore MIDI track metadata in that split and fall back to pitch clustering (`voice_ignore_metadata`).
    pub voice_ignore_metadata: bool,
    /// Drop notes that repeat instrument+key on the same tick, keeping the first (`dedupe_notes`).
    pub dedupe_notes: bool,
    pub channels: [ChannelCombination; 16],
    pub substitutes: [InstrumentSubstitution; 20],
}

impl Default for EnhancedMidiOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            range_mode: RangeMode::Octave,
            smart_substitute: true,
            force_fold: false,
            sustain: SustainMode::default(),
            sustain_tracks: Vec::new(),
            sustain_fade: true,
            sustain_density: SustainDensity::default(),
            separate_voices: false,
            voice_ignore_metadata: false,
            dedupe_notes: false,
            channels: std::array::from_fn(|_| ChannelCombination::default()),
            substitutes: DEFAULT_SUBSTITUTES.map(|[high, low]| InstrumentSubstitution {
                high: u8::try_from(high).ok(),
                low: u8::try_from(low).ok(),
            }),
        }
    }
}

impl EnhancedMidiOptions {
    /// Per-file presets never enable enhanced import implicitly.
    pub fn for_midi(midi: &MidiData) -> Self {
        let mut options = Self::default();
        options.reset_combinations(midi, 2);
        options
    }

    /// Percussion remains independent of melody presets.
    pub fn reset_combinations(&mut self, midi: &MidiData, slot_count: usize) {
        let slot_count = slot_count.clamp(1, 3);
        for (channel, combination) in self.channels.iter_mut().enumerate() {
            *combination = ChannelCombination::default();
            if channel == 9 {
                continue;
            }
            if let Some(preset) = GM_COMBINATIONS.get(midi.channel_patch[channel] as usize) {
                let mut next_slot = 0;
                for &instrument in preset {
                    let Ok(instrument) = u8::try_from(instrument) else {
                        continue;
                    };
                    if combination
                        .slots
                        .iter()
                        .any(|slot| slot.instrument == Some(instrument))
                    {
                        continue;
                    }
                    combination.slots[next_slot].instrument = Some(instrument);
                    next_slot += 1;
                    if next_slot == slot_count {
                        break;
                    }
                }
            }
        }
        self.recommend_offsets(midi);
    }

    /// All active slots count equally; ties prefer the smallest shift, then the negative candidate.
    pub fn recommend_offsets(&mut self, midi: &MidiData) {
        let (limit, step) = match self.range_mode {
            RangeMode::Keep | RangeMode::Fold => {
                for (channel, combination) in self.channels.iter_mut().enumerate() {
                    if channel != 9 {
                        combination.transpose = 0;
                    }
                }
                return;
            }
            RangeMode::Octave => (72i32, 12),
            RangeMode::Semitone => (36i32, 1),
        };
        let mut keys: [Vec<i32>; 16] = std::array::from_fn(|_| Vec::new());
        for ev in midi.tracks.iter().flat_map(|track| &track.events) {
            let channel = ev.channel as usize;
            if channel >= 16 || channel == 9 {
                continue;
            }
            for slot in &self.channels[channel].slots {
                if let Some(instrument) = slot.instrument.filter(|&i| i < 20 && slot.volume > 0) {
                    let (_, key, _) = self.substitute(instrument, ev.note as i32 - 21);
                    keys[channel].push(key);
                }
            }
        }
        for (channel, keys) in keys.iter().enumerate() {
            if channel == 9 {
                continue;
            }
            let mut best = (usize::MAX, i32::MAX);
            let mut best_offset = 0;
            for offset in (-limit..=limit).step_by(step) {
                let score = (
                    keys.iter().filter(|&&key| !in_range(key + offset)).count(),
                    offset.abs(),
                );
                if score < best {
                    best = score;
                    best_offset = offset;
                }
            }
            self.channels[channel].transpose = best_offset as i8;
        }
    }

    /// Failed chains leave both the original key and instrument intact.
    fn substitute(&self, instrument: u8, key: i32) -> (u8, i32, bool) {
        if !self.smart_substitute
            || !matches!(self.range_mode, RangeMode::Octave | RangeMode::Semitone)
            || in_range(key)
        {
            return (instrument, key, false);
        }
        let (mut current, mut current_key) = (instrument, key);
        for _ in 0..4 {
            let Some(config) = self.substitutes.get(current as usize) else {
                break;
            };
            let target = if current_key > MC_KEY_MAX as i32 {
                config.high
            } else {
                config.low
            };
            let Some(target) = target.filter(|&i| i < 20) else {
                break;
            };
            current_key +=
                INSTRUMENT_OFFSETS[current as usize] - INSTRUMENT_OFFSETS[target as usize];
            current = target;
            if in_range(current_key) {
                return (current, current_key, true);
            }
        }
        (instrument, key, false)
    }
}

/// 一条 MIDI 轨的音符时值分布，对应 Miau 解析信息里 `trackInfoList` 的那几项。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackLengthStats {
    /// MIDI 轨下标，与白名单、`import_resolved` 的事件分组同口径。
    pub track: usize,
    /// 该轨 note-on 条数；没配对到 note-off 的也算，按时值 0 计入分母。
    pub notes: usize,
    /// 时值达到一拍的音符数，即 Miau 的 `trackLongNoteCount`。
    pub long_notes: usize,
    /// 音符时值总和，单位是文件 tick（未量化），对应 `trackDurationSum`。
    pub total_ticks: i64,
}

impl TrackLengthStats {
    /// Miau `long_note_ratio`。
    pub fn long_ratio(&self) -> f64 {
        if self.notes == 0 {
            0.0
        } else {
            self.long_notes as f64 / self.notes as f64
        }
    }

    /// Miau `avg_duration_ticks`（原版取整，这里留小数给界面显示）。
    pub fn avg_ticks(&self) -> f64 {
        if self.notes == 0 {
            0.0
        } else {
            self.total_ticks as f64 / self.notes as f64
        }
    }

    /// Miau `28-midi-dialog.js:1797-1800`：长音占比 ≥0.75 且平均时值 ≥2 拍。
    pub fn is_sustain_candidate(&self, beat_ticks: i64) -> bool {
        self.long_ratio() >= 0.75 && self.avg_ticks() >= beat_ticks.max(1) as f64 * 2.0
    }
}

/// 每轨时值统计；没有音符的轨道不出现，同 Miau 只列 `event_count > 0` 的轨。
pub fn track_length_stats(midi: &MidiData) -> Vec<TrackLengthStats> {
    let beat = i64::from(midi.ppq().max(1));
    midi.tracks
        .iter()
        .enumerate()
        .filter_map(|(track, data)| {
            let mut stats = TrackLengthStats {
                track,
                ..TrackLengthStats::default()
            };
            for ev in &data.events {
                stats.notes += 1;
                let duration = (ev.until - ev.pos).max(0);
                stats.total_ticks += duration;
                if duration >= beat {
                    stats.long_notes += 1;
                }
            }
            (stats.notes > 0).then_some(stats)
        })
        .collect()
}

/// Miau「智能识别延音轨」里按时值的那半边。
/// 按 SF2 音色的那半边没搬：本项目只有 20 个内置乐器，实测最长的采样余音 440 ms，
/// 那份「哪些 GM 音色本身能持续」的清单在这里没有对应数据。
pub fn sustain_candidate_tracks(midi: &MidiData) -> Vec<usize> {
    let beat = i64::from(midi.ppq().max(1));
    track_length_stats(midi)
        .into_iter()
        .filter(|stats| stats.is_sustain_candidate(beat))
        .map(|stats| stats.track)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnhancedImportReport {
    /// Final Song.total_blocks, including duration tails and tempo events.
    pub notes: usize,
    /// Final stored notes outside 33..57, excluding generated Tempo Changers.
    pub out_of_range: usize,
    /// Processed melody slot events, before max_height allocation, not tails.
    pub substituted: usize,
    /// Processed melody slot events actually shifted by folding, not tails.
    pub folded: usize,
    /// MIDI 音符里拿到声部标记的条数（一音符一条，不含槽位复制与尾音块）。
    pub voiced_notes: usize,
    /// 同 tick 同乐器同音高被删掉的块数（尾音块一起算）。
    pub deduped: usize,
}

fn in_range(key: i32) -> bool {
    (MC_KEY_MIN as i32..=MC_KEY_MAX as i32).contains(&key)
}

fn base_volume(raw: u8, read_velocity: bool) -> u8 {
    if read_velocity {
        (raw.min(127) as u16 * 100 / 127) as u8
    } else {
        100
    }
}

fn finish_report(song: &Song, tempo_changer: bool, report: &mut EnhancedImportReport) {
    report.notes = song.total_blocks;
    report.out_of_range = song
        .iter_notes()
        .filter(|(_, _, note)| {
            !(in_range(note.key as i32) || tempo_changer && note.instrument == FIRST_CUSTOM_INDEX)
        })
        .count();
}

/// Miau `dedupe_notes`：同 tick、同乐器、同音高只留第一条。
/// Miau 在图层压实前删，这里在落块后删，所以被清空的层会留着名字继续占位，
/// 也不做 `dedupe_reorder` 的向上回填（本项目不迁那套）。速度变化块不是音符，跳过。
fn dedupe_notes(song: &mut Song) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut drops: Vec<(u32, u32)> = Vec::new();
    for (tick, layer, note) in song.iter_notes() {
        if note.instrument == FIRST_CUSTOM_INDEX as u8 {
            continue;
        }
        if !seen.insert((tick, note.instrument, note.key)) {
            drops.push((tick, layer));
        }
    }
    for (tick, layer) in &drops {
        song.remove(*tick, *layer);
    }
    song.recalculate();
    drops.len()
}

/// 三态延音翻成尾音方案。白名单外的轨道仍是传统那套「每 tick 一块」的尾音：
/// 「要不要保持时长」在本项目是传统导入就有的 `note_duration` 开关，不像 Miau 由这三态独占。
fn sustain_plan(enhanced: &EnhancedMidiOptions) -> Option<SustainTail> {
    let tracks = match enhanced.sustain {
        SustainMode::Off => return None,
        SustainMode::All => None,
        SustainMode::Tracks => Some(enhanced.sustain_tracks.clone()),
    };
    Some(SustainTail {
        fade: enhanced.sustain_fade,
        tracks,
        every_tick: enhanced.sustain_density == SustainDensity::EveryTick,
    })
}

/// Unsupported NBS keys fail without clamping; native-range outliers remain in the song.
pub fn import_midi_enhanced(
    midi: &MidiData,
    opts: &MidiImportOptions,
    enhanced: &EnhancedMidiOptions,
) -> Result<(Song, EnhancedImportReport), String> {
    let mut report = EnhancedImportReport::default();
    if !enhanced.enabled {
        let song = import_midi_data(midi, opts);
        finish_report(&song, opts.tempo_changer, &mut report);
        return Ok((song, report));
    }
    let dpt = midi.deltapertick(opts.precision).max(1e-9);
    // Miau 的声部规划看到的是原始音符事件：一音符一条、按 MIDI tick 分单元，槽位复制发生在拆分之后。
    let mut voice_notes: Vec<VoiceNote> = Vec::new();
    let mut voice_at: Vec<(usize, usize)> = Vec::new();
    for (t, track) in midi.tracks.iter().enumerate() {
        for (e, ev) in track.events.iter().enumerate() {
            let channel = ev.channel as usize;
            if channel >= 16 || ev.channel > midi.channels {
                continue;
            }
            if midi.quantize(ev.pos, opts.remove_silent, dpt) < 0 {
                continue;
            }
            voice_notes.push(VoiceNote {
                track: t,
                channel,
                tick: ev.pos,
                note: ev.note as i32,
            });
            voice_at.push((t, e));
        }
    }
    let marks =
        plan_all_channels(&voice_notes, enhanced.separate_voices, enhanced.voice_ignore_metadata);
    report.voiced_notes = marks.iter().filter(|mark| mark.is_some()).count();
    let mut voices: Vec<Vec<Option<u8>>> = midi
        .tracks
        .iter()
        .map(|track| vec![None; track.events.len()])
        .collect();
    for ((t, e), mark) in voice_at.iter().zip(marks) {
        voices[*t][*e] = mark;
    }
    let mut events: Vec<Vec<(Resolved, u16)>> = Vec::with_capacity(midi.tracks.len());
    for (t, track) in midi.tracks.iter().enumerate() {
        let mut expanded = Vec::new();
        for (e, ev) in track.events.iter().enumerate() {
            let channel = ev.channel as usize;
            if channel >= 16 || ev.channel > midi.channels {
                continue;
            }
            // Legacy melody mapping is replaced by the independent per-slot mapping below.
            let mut resolved = resolve(midi, opts, ev, dpt);
            if resolved.pos < 0 {
                continue;
            }
            resolved.voice = voices[t][e];
            let base = base_volume(ev.velocity, opts.use_velocity);
            if channel == 9 {
                resolved.vel = base;
                expanded.push((resolved, ev.patch));
                continue;
            }
            let combination = &enhanced.channels[channel];
            for slot in &combination.slots {
                let Some(instrument) = slot.instrument.filter(|_| slot.volume > 0) else {
                    continue;
                };
                if instrument >= 20 {
                    return Err(crate::tr(
                        "Enhanced MIDI slots require NBS instruments 0..19.",
                        "增强 MIDI 槽位仅支持 NBS 乐器 0..19。",
                    )
                    .into());
                }
                // Equal keys intentionally retain Miau's cross-octave layering.
                let (instrument, mut key, substituted) =
                    enhanced.substitute(instrument, ev.note as i32 - 21);
                key += combination.transpose as i32;
                let before_fold = key;
                if enhanced.range_mode == RangeMode::Fold
                    || enhanced.force_fold
                        && matches!(enhanced.range_mode, RangeMode::Octave | RangeMode::Semitone)
                {
                    while key < MC_KEY_MIN as i32 {
                        key += 12;
                    }
                    while key > MC_KEY_MAX as i32 {
                        key -= 12;
                    }
                }
                if !(0..=KEY_MAX as i32).contains(&key) {
                    return Err(crate::tr(
                        &format!("MIDI channel {}, note {} produces unsupported NBS key {} (0..87). Adjust transpose or enable octave folding.", channel + 1, ev.note, key),
                        &format!("MIDI 通道 {}，音符 {} 生成不支持的 NBS 音高 {}（范围 0..87）。请调整移调或启用八度折叠。", channel + 1, ev.note, key),
                    ).into());
                }
                report.substituted += usize::from(substituted);
                report.folded += usize::from(key != before_fold);
                expanded.push((
                    Resolved {
                        note: key,
                        ins: instrument as i8,
                        stop: false,
                        // floor(raw/127*100), then round(base*slot/100).
                        vel: ((base as u16 * slot.volume.min(100) as u16 + 50) / 100) as u8,
                        ..resolved
                    },
                    ev.patch,
                ));
            }
        }
        events.push(expanded);
    }
    let sustain = sustain_plan(enhanced);
    let mut song = import_resolved(midi, opts, &events, sustain);
    if enhanced.dedupe_notes {
        report.deduped = dedupe_notes(&mut song);
    }
    finish_report(&song, opts.tempo_changer, &mut report);
    Ok((song, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_decay::gain_at;
    use crate::{parse_midi, MidiEvent, MidiGmTable, MidiPart, MidiTrack};

    fn midi_keys(keys: &[i32]) -> MidiData {
        let events = keys
            .iter()
            .enumerate()
            .map(|(i, &key)| MidiEvent {
                pos: i as i64 * 120,
                until: i as i64 * 120 + 480,
                channel: 0,
                note: (key + 21) as u8,
                velocity: 100,
                patch: 0,
                part: 0,
            })
            .collect();
        MidiData {
            division: 480,
            minpos: 0,
            maxpos: keys.len() as i64 * 120 + 360,
            micsecqn: 500_000,
            tracks: vec![MidiTrack {
                name: "Synthetic".into(),
                events,
            }],
            parts: vec![MidiPart {
                track: 0,
                channel: 0,
                patch: 0,
                note_duration: true,
            }],
            ..Default::default()
        }
    }

    fn options() -> MidiImportOptions {
        MidiImportOptions {
            precision: 0,
            part_durations: vec![true],
            ..Default::default()
        }
    }

    fn slot(instrument: u8, volume: u8) -> MidiSlot {
        MidiSlot {
            instrument: Some(instrument),
            volume,
        }
    }

    fn enhanced() -> EnhancedMidiOptions {
        EnhancedMidiOptions {
            enabled: true,
            range_mode: RangeMode::Keep,
            channels: std::array::from_fn(|_| ChannelCombination {
                slots: [slot(0, 100), MidiSlot::default(), MidiSlot::default()],
                transpose: 0,
            }),
            ..Default::default()
        }
    }

    fn three_slots(config: &mut EnhancedMidiOptions) {
        config.channels[0].slots = [slot(0, 100), slot(6, 50), slot(7, 25)];
    }

    fn notes(song: &Song) -> Vec<(u32, u32, nbs_core::model::Note)> {
        song.iter_notes().collect()
    }

    fn assert_same_song(a: &Song, b: &Song) {
        assert_eq!(notes(a), notes(b));
        assert_eq!(a.layers, b.layers);
        assert_eq!(a.meta, b.meta);
        assert_eq!(a.custom_instruments, b.custom_instruments);
        assert_eq!(
            (a.enda, a.endb, a.total_blocks, a.dirty),
            (b.enda, b.endb, b.total_blocks, b.dirty)
        );
    }

    #[test]
    fn defaults_and_gm_resets_are_per_file_and_deduplicated() {
        let defaults = EnhancedMidiOptions::default();
        assert!(!defaults.enabled);
        assert_eq!(defaults.range_mode, RangeMode::Octave);
        assert!(defaults.smart_substitute);
        assert!(!defaults.force_fold);
        assert_eq!(
            MidiSlot::default(),
            MidiSlot {
                instrument: None,
                volume: 100
            }
        );
        assert_eq!(
            defaults.substitutes[0],
            InstrumentSubstitution {
                high: Some(6),
                low: Some(5)
            }
        );
        let mut midi = midi_keys(&[45]);
        midi.channel_patch[1] = 8; // Bell, absent high, Harp low
        midi.channel_patch[2] = 40; // Harp is a real main instrument, not missing
        midi.channel_patch[3] = 128;
        let mut config = EnhancedMidiOptions::for_midi(&midi);
        assert!(!config.enabled);
        assert_eq!(
            config.channels[0].slots,
            [slot(15, 100), slot(7, 100), MidiSlot::default()]
        );
        assert_eq!(
            config.channels[1].slots,
            [slot(7, 100), slot(0, 100), MidiSlot::default()]
        );
        assert_eq!(config.channels[2].slots[0], slot(0, 100));
        assert!(config.channels[3]
            .slots
            .iter()
            .all(|s| s.instrument.is_none()));
        assert!(config.channels[9]
            .slots
            .iter()
            .all(|s| s.instrument.is_none()));
        config.reset_combinations(&midi, 3);
        assert_eq!(
            config.channels[0].slots,
            [slot(15, 100), slot(7, 100), slot(1, 100)]
        );
        config.channels[0].slots[0].volume = 0;
        config.reset_combinations(&midi, 1);
        assert_eq!(
            config.channels[0].slots,
            [slot(15, 100), MidiSlot::default(), MidiSlot::default()]
        );
        for patch in 0..128 {
            midi.channel_patch[0] = patch;
            config.reset_combinations(&midi, 3);
            let active: Vec<_> = config.channels[0]
                .slots
                .iter()
                .filter_map(|s| s.instrument)
                .collect();
            assert!(!active.is_empty());
            assert!(active.iter().all(|&i| i < 20));
            for (index, instrument) in active.iter().enumerate() {
                assert!(!active[..index].contains(instrument));
            }
        }
    }

    #[test]
    fn synthetic_smf_to_three_slots_and_read_velocity_switch() {
        // Format 0, tempo, program 40, note-on velocity 100, note-off at 480.
        let track = [
            0, 0xff, 0x51, 3, 7, 0xa1, 0x20, 0, 0xc0, 40, 0, 0x90, 60, 100, 0x83, 0x60, 0x80, 60,
            0, 0, 0xff, 0x2f, 0,
        ];
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&track);
        let midi = parse_midi(&bytes).unwrap();
        assert_eq!(midi.tracks[0].events[0].until, 480);
        assert_eq!(midi.channel_patch[0], 40);
        assert_eq!(midi.parts.len(), 1);
        let mut config = enhanced();
        three_slots(&mut config);
        let mut opts = options();
        opts.channel_ins = [-1; 16];
        opts.channel_octave = [7; 16];
        opts.keep_octave_range = true;
        let original_config = config.clone();
        for read_velocity in [true, false] {
            opts.use_velocity = read_velocity;
            let original_opts = opts.clone();
            let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
            assert_eq!(
                report,
                EnhancedImportReport {
                    notes: 3,
                    ..Default::default()
                }
            );
            assert_eq!(song.layers.len(), 3);
            let expected = if read_velocity {
                [78, 39, 20]
            } else {
                [100, 50, 25]
            };
            for (layer, (&instrument, &volume)) in [0, 6, 7].iter().zip(&expected).enumerate() {
                let note = song.get(0, layer as u32).unwrap();
                assert_eq!(
                    (note.instrument, note.key, note.velocity, note.pitch),
                    (instrument, 39, volume, 0)
                );
            }
            assert_eq!(opts, original_opts);
        }
        assert_eq!(config, original_config);
    }

    #[test]
    fn disabled_is_exactly_traditional_for_precision_tempo_duration_and_drums() {
        let mut midi = midi_keys(&[15, 39, 75]);
        midi.tempo_changers = vec![(0, 120.0), (120, 60.0)];
        midi.channels = 9;
        midi.perc_notes = vec![36];
        midi.tracks[0].events.push(MidiEvent {
            channel: 9,
            note: 36,
            pos: 120,
            until: 960,
            velocity: 110,
            patch: 0,
            part: 0,
        });
        let mut config = enhanced();
        config.enabled = false;
        config.channels[0].transpose = 127; // would error if enhanced handling ran
        for precision in [0, 1, 3] {
            for tempo_changer in [false, true] {
                for note_duration in [false, true] {
                    for drum in [-1, 2] {
                        let opts = MidiImportOptions {
                            precision,
                            tempo_changer,
                            note_duration,
                            perc_ins: vec![drum],
                            perc_pitch: vec![60],
                            ..options()
                        };
                        let legacy = import_midi_data(&midi, &opts);
                        let (actual, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
                        assert_same_song(&actual, &legacy);
                        assert_eq!(report.notes, legacy.total_blocks);
                        assert_eq!((report.substituted, report.folded), (0, 0));
                        assert_eq!(
                            report.out_of_range,
                            legacy
                                .iter_notes()
                                .filter(|(_, _, n)| !in_range(n.key as i32))
                                .count()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn zero_volume_and_none_slots_do_not_generate_or_reserve_layers() {
        let midi = midi_keys(&[39]);
        let mut config = enhanced();
        config.channels[0].slots = [slot(0, 0), MidiSlot::default(), slot(6, 100)];
        let opts = MidiImportOptions {
            max_height: 1,
            ..options()
        };
        let (song, _) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.total_blocks, 1);
        assert_eq!(song.layers.len(), 1);
        assert_eq!(song.get(0, 0).unwrap().instrument, 6);
        config.channels[0].slots = [MidiSlot::default(); 3];
        config.channels[0].transpose = 127; // silent slots never trigger a key error
        let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.total_blocks, 0);
        assert_eq!(report, EnhancedImportReport::default());
    }

    #[test]
    fn duplicate_manual_slots_and_source_unisons_are_not_deduplicated() {
        let mut midi = midi_keys(&[39, 39]);
        midi.tracks[0].events[1].pos = 0;
        let mut config = enhanced();
        config.channels[0].slots = [slot(0, 100); 3];
        let (song, _) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(song.total_blocks, 6);
        assert_eq!(song.layers.len(), 6);
    }

    #[test]
    fn all_sixteen_channels_expand_but_drums_remain_single_slot() {
        let mut midi = midi_keys(&[39]);
        midi.channels = 15;
        midi.perc_notes = vec![36];
        let event = midi.tracks[0].events[0].clone();
        midi.tracks[0].events = (0..16)
            .map(|channel| MidiEvent {
                channel,
                note: if channel == 9 { 36 } else { 60 },
                ..event.clone()
            })
            .collect();
        let mut config = enhanced();
        for channel in &mut config.channels {
            channel.slots = [slot(0, 100), slot(6, 50), slot(7, 25)];
        }
        config.channels[9].transpose = 127; // ignored for percussion
        let mut opts = MidiImportOptions {
            note_duration: true,
            perc_ins: vec![2],
            perc_pitch: vec![40],
            ..options()
        };
        let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.layers.len(), 46);
        assert_eq!(report.notes, 15 * 3 * 4 + 1);
        assert_eq!(song.get(0, 27).unwrap().instrument, 2);
        assert_eq!(song.get(0, 27).unwrap().velocity, 78);
        assert_eq!(song.get(0, 27).unwrap().key, 40);
        assert!(song.get(1, 27).is_none());
        assert_eq!(song.get(0, 45).unwrap().instrument, 7);
        opts.use_velocity = false;
        let (song, _) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.get(0, 27).unwrap().velocity, 100);
    }

    #[test]
    fn three_slots_sustain_share_channel_max_height_and_release_layers() {
        let mut midi = midi_keys(&[39, 43, 46]);
        midi.tracks[0].events[2].pos = 480;
        midi.tracks[0].events[2].until = 960;
        let mut config = enhanced();
        three_slots(&mut config);
        for (max_height, count, layers) in [(2, 16, 2), (3, 24, 3), (4, 28, 4), (20, 36, 6)] {
            let opts = MidiImportOptions {
                max_height,
                note_duration: true,
                ..options()
            };
            let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
            assert_eq!(song.total_blocks, count, "max height {max_height}");
            assert_eq!(song.layers.len(), layers);
            assert_eq!(song.get(4, 0).unwrap().key, 46); // exact interval end reuses layer
            assert_eq!(song.get(1, 0).unwrap().velocity, 39);
            assert_eq!(song.get(1, 0).unwrap().panning, 50);
            assert_eq!(song.get(2, 0).unwrap().panning, 150);
            assert_eq!(report.notes, count);
        }
    }

    #[test]
    fn substitution_preserves_relative_acoustic_pitch_at_twelve_and_twenty_four() {
        let mut config = enhanced();
        config.range_mode = RangeMode::Octave;
        for (instrument, key, expected_instrument, expected_key) in [
            (0, 60, 6, 48),
            (0, 72, 8, 48),
            (0, 30, 5, 42),
            (0, 18, 1, 42),
            (12, 72, 0, 48),
            (7, 9, 0, 33),
        ] {
            let (actual_instrument, actual_key, substituted) = config.substitute(instrument, key);
            assert!(substituted);
            assert_eq!(
                (actual_instrument, actual_key),
                (expected_instrument, expected_key)
            );
            assert_eq!(
                key + INSTRUMENT_OFFSETS[instrument as usize],
                actual_key + INSTRUMENT_OFFSETS[actual_instrument as usize]
            );
        }
    }

    #[test]
    fn failed_cyclic_and_overlong_chains_never_commit_intermediate_state() {
        let mut config = enhanced();
        config.range_mode = RangeMode::Octave;
        assert_eq!(config.substitute(0, 100), (0, 100, false)); // reaches Chime but still too high
        assert_eq!(config.substitute(0, -10), (0, -10, false));
        config.substitutes[0].high = Some(13);
        config.substitutes[13].high = Some(0); // cycle, four hops only
        assert_eq!(config.substitute(0, 60), (0, 60, false));
        config.substitutes[13].high = Some(14);
        config.substitutes[14].high = Some(15);
        config.substitutes[15].high = Some(16);
        config.substitutes[16].high = Some(6); // fifth hop would succeed
        assert_eq!(config.substitute(0, 60), (0, 60, false));
        config.substitutes[15].high = Some(6); // fourth hop succeeds
        assert_eq!(config.substitute(0, 60), (6, 48, true));
        config.substitutes[0].high = Some(255);
        assert_eq!(config.substitute(0, 60), (0, 60, false));
    }

    #[test]
    fn all_modes_apply_manual_shift_and_only_selected_modes_substitute_or_fold() {
        let midi = midi_keys(&[60]);
        let mut config = enhanced();
        config.channels[0].transpose = 12;
        for (mode, force_fold, instrument, key, substituted, folded) in [
            (RangeMode::Keep, true, 0, 72, 0, 0),
            (RangeMode::Fold, false, 0, 48, 0, 1),
            (RangeMode::Octave, false, 6, 60, 1, 0),
            (RangeMode::Semitone, false, 6, 60, 1, 0),
            (RangeMode::Octave, true, 6, 48, 1, 1),
            (RangeMode::Semitone, true, 6, 48, 1, 1),
        ] {
            config.range_mode = mode;
            config.force_fold = force_fold;
            let (song, report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
            let note = song.get(0, 0).unwrap();
            assert_eq!((note.instrument, note.key), (instrument, key));
            assert_eq!((report.substituted, report.folded), (substituted, folded));
            assert_eq!(report.out_of_range, usize::from(!in_range(key as i32)));
            assert_eq!(config.channels[0].transpose, 12);
        }
        config.smart_substitute = false;
        config.force_fold = false;
        let (song, report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(song.get(0, 0).unwrap().key, 72);
        assert_eq!(report.substituted, 0);
    }

    #[test]
    fn recommendations_include_harp_zero_and_every_active_slot_after_substitution() {
        let midi = midi_keys(&[60]);
        let mut config = enhanced();
        config.range_mode = RangeMode::Octave;
        config.channels[0].slots = [slot(0, 100), slot(7, 100), slot(7, 100)];
        // Bells cannot go higher (60); the main Harp substitutes to Flute (48).
        // -12 fits all three; considering only the main slot would recommend zero.
        config.recommend_offsets(&midi);
        assert_eq!(config.channels[0].transpose, -12);
        config.channels[0].slots = [slot(0, 100), slot(7, 0), MidiSlot::default()];
        config.recommend_offsets(&midi);
        assert_eq!(config.channels[0].transpose, 0); // Harp correctly substitutes
        config.smart_substitute = false;
        config.recommend_offsets(&midi);
        assert_eq!(config.channels[0].transpose, -12);
        config.range_mode = RangeMode::Semitone;
        config.recommend_offsets(&midi);
        assert_eq!(config.channels[0].transpose, -3);
    }

    #[test]
    fn recommendation_minimizes_count_then_absolute_shift_with_stable_ties() {
        let mut config = enhanced();
        config.smart_substitute = false;
        config.range_mode = RangeMode::Octave;
        config.recommend_offsets(&midi_keys(&[21, 69]));
        assert_eq!(config.channels[0].transpose, -12); // +/-12 each fixes one; negative wins tie
        config.recommend_offsets(&midi_keys(&[21, 21, 69]));
        assert_eq!(config.channels[0].transpose, 12); // count beats sign
        config.recommend_offsets(&midi_keys(&[33, 57]));
        assert_eq!(config.channels[0].transpose, 0);
        config.recommend_offsets(&midi_keys(&[-21]));
        assert_eq!(config.channels[0].transpose, 60);
        config.range_mode = RangeMode::Semitone;
        config.recommend_offsets(&midi_keys(&[-21]));
        assert_eq!(config.channels[0].transpose, 0); // no candidate within +/-36 succeeds
        config.channels[9].transpose = 17;
        for mode in [RangeMode::Keep, RangeMode::Fold] {
            config.range_mode = mode;
            config.channels[0].transpose = 12;
            config.recommend_offsets(&midi_keys(&[69]));
            assert_eq!(config.channels[0].transpose, 0);
            assert_eq!(config.channels[9].transpose, 17); // drums skipped
        }
    }

    #[test]
    fn reports_count_final_notes_but_substitution_and_folding_count_slot_events() {
        let midi = midi_keys(&[60]);
        let mut config = enhanced();
        three_slots(&mut config);
        config.range_mode = RangeMode::Octave;
        config.force_fold = true;
        let opts = MidiImportOptions {
            note_duration: true,
            max_height: 2,
            ..options()
        };
        let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.total_blocks, 8); // only two slots admitted, four ticks each
        assert_eq!(
            report,
            EnhancedImportReport {
                notes: 8,
                out_of_range: 0,
                substituted: 2,
                folded: 1,
                voiced_notes: 0,
                deduped: 0,
            }
        );
        config.range_mode = RangeMode::Keep;
        let (_, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(
            report,
            EnhancedImportReport {
                notes: 8,
                out_of_range: 8,
                substituted: 0,
                folded: 0,
                voiced_notes: 0,
                deduped: 0,
            }
        );
    }

    #[test]
    fn valid_extended_keys_survive_unsupported_keys_error_and_fold_can_recover() {
        let config = enhanced();
        let (song, report) =
            import_midi_enhanced(&midi_keys(&[0, 32, 33, 57, 58, 87]), &options(), &config)
                .unwrap();
        assert_eq!(
            song.iter_notes().map(|(_, _, n)| n.key).collect::<Vec<_>>(),
            [0, 32, 33, 57, 58, 87]
        );
        assert_eq!(report.out_of_range, 4);
        for key in [-1, 88, 106] {
            let midi = midi_keys(&[key]);
            let before = config.clone();
            let error = import_midi_enhanced(&midi, &options(), &config).unwrap_err();
            assert!(error.contains("0..87"));
            assert!(error.contains(&key.to_string()));
            assert_eq!(config, before);
            let mut folded = config.clone();
            folded.range_mode = RangeMode::Fold;
            let (song, report) = import_midi_enhanced(&midi, &options(), &folded).unwrap();
            assert!(in_range(song.get(0, 0).unwrap().key as i32));
            assert_eq!((report.out_of_range, report.folded), (0, 1));
        }
    }

    #[test]
    fn empty_midi_and_filtered_events_are_safe() {
        for midi in [
            MidiData::default(),
            MidiData {
                division: 480,
                tracks: vec![MidiTrack::default()],
                ..Default::default()
            },
        ] {
            let mut config = EnhancedMidiOptions::for_midi(&midi);
            config.enabled = true;
            config.recommend_offsets(&midi);
            let opts = MidiImportOptions {
                tempo_changer: true,
                ..options()
            };
            let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
            assert_eq!(song.total_blocks, 0);
            assert_eq!(report, EnhancedImportReport::default());
            assert!(config.channels.iter().all(|c| c.transpose == 0));
        }
        let mut midi = midi_keys(&[106, 106, 39]);
        midi.tracks[0].events[0].pos = -120;
        midi.tracks[0].events[1].channel = 1; // beyond midi.channels
        let (song, report) = import_midi_enhanced(&midi, &options(), &enhanced()).unwrap();
        assert_eq!(song.total_blocks, 1);
        assert_eq!(report.out_of_range, 0);
    }

    #[test]
    fn percussion_mapping_range_and_muting_are_legacy_except_volume() {
        let mut midi = midi_keys(&[39]);
        midi.channels = 9;
        midi.perc_notes = vec![36, 38];
        midi.tracks[0].events = [36, 38]
            .map(|note| MidiEvent {
                channel: 9,
                note,
                velocity: 100,
                ..midi.tracks[0].events[0].clone()
            })
            .to_vec();
        let config = enhanced();
        let mut opts = MidiImportOptions {
            perc_ins: vec![2, -1],
            perc_pitch: vec![100, 45],
            ..options()
        };
        let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.total_blocks, 1);
        assert_eq!(
            (
                song.get(0, 0).unwrap().key,
                song.get(0, 0).unwrap().velocity
            ),
            (87, 78)
        );
        assert_eq!(report.out_of_range, 1);
        opts.keep_octave_range = true;
        let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.get(0, 0).unwrap().key, 51);
        assert_eq!(
            report,
            EnhancedImportReport {
                notes: 1,
                ..Default::default()
            }
        );
        opts.perc_pitch.clear();
        assert_eq!(
            import_midi_enhanced(&midi, &opts, &config)
                .unwrap()
                .0
                .total_blocks,
            0
        );
    }

    #[test]
    fn velocity_uses_floor_then_round_without_loudness_normalization() {
        for raw in 0..=127 {
            assert_eq!(
                base_volume(raw, true),
                (raw as f64 / 127.0 * 100.0).floor() as u8
            );
            assert_eq!(base_volume(raw, false), 100);
        }
        let mut midi = midi_keys(&[39]);
        midi.tracks[0].events[0].velocity = 127;
        let mut config = enhanced();
        config.channels[0].slots = [slot(0, 100), slot(0, 100), slot(0, 100)];
        let (song, _) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert!(song.iter_notes().all(|(_, _, n)| n.velocity == 100));
    }

    #[test]
    fn tempo_changes_trim_and_nbs_save_read_roundtrip() {
        let mut midi = midi_keys(&[39, 60]);
        for event in &mut midi.tracks[0].events {
            event.pos += 240;
            event.until += 240;
        }
        midi.minpos = 240;
        midi.maxpos += 240;
        midi.tempo_changers = vec![(0, 120.0), (240, 90.0), (480, 60.0)];
        let mut config = enhanced();
        config.channels[0].slots = [slot(16, 100), slot(17, 50), slot(19, 25)];
        for remove_silent in [false, true] {
            for precision in [0, 1, 3] {
                let opts = MidiImportOptions {
                    tempo_changer: true,
                    note_duration: true,
                    remove_silent,
                    precision,
                    ..options()
                };
                let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
                let factor = precision as u32 + 1;
                let first = if remove_silent { 0 } else { 2 * factor };
                assert_eq!(song.get(first, 0).unwrap().pitch, 90 * factor as i16);
                assert_eq!(song.get(first, 1).unwrap().instrument, 16);
                assert_eq!(song.meta.tempo, 8.0 * factor as f32);
                assert_eq!(report.out_of_range, 3 * 4 * factor as usize);
                let tempo_count = if remove_silent { 2 } else { 3 };
                assert_eq!(report.notes, 6 * 4 * factor as usize + tempo_count);
                let bytes = nbs_core::nbs::save_bytes(&song, 6).unwrap();
                let loaded = nbs_core::nbs::load_bytes(&bytes).unwrap();
                assert_eq!(notes(&song), notes(&loaded));
                assert_eq!(song.layers, loaded.layers);
                assert_eq!(song.meta.tempo, loaded.meta.tempo);
                assert_eq!(song.custom_instruments, loaded.custom_instruments);
                assert_eq!(song.total_blocks, loaded.total_blocks);
            }
        }
    }

    #[test]
    fn enhancement_does_not_mutate_legacy_gm_tables_or_options() {
        let midi = midi_keys(&[39]);
        let gm = MidiGmTable::default();
        let opts = MidiImportOptions::for_midi(&midi, &gm);
        let before_opts = opts.clone();
        let before_gm = gm.clone();
        let mut config = EnhancedMidiOptions::for_midi(&midi);
        config.enabled = true;
        import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(opts, before_opts);
        assert_eq!(gm, before_gm);
    }

    /// 同通道、一低一高交替：12 音、span 46，聚类必然分成两个声部。
    fn two_hands() -> MidiData {
        midi_keys(&[39, 79, 41, 81, 43, 83, 45, 85, 40, 80, 42, 82])
    }

    /// 同通道两条轨道，音域合计只有 13 个半音且各自错开 tick：聚类必然被 poly/span 双门槛拦住。
    fn two_tracks(low: &[i32], high: &[i32]) -> MidiData {
        let track = |index: usize, shift: i64, keys: &[i32]| MidiTrack {
            name: format!("Track {index}"),
            events: keys
                .iter()
                .enumerate()
                .map(|(i, &key)| MidiEvent {
                    pos: (shift + i as i64) * 120,
                    until: (shift + i as i64) * 120 + 120,
                    channel: 0,
                    note: (key + 21) as u8,
                    velocity: 100,
                    patch: 0,
                    part: index,
                })
                .collect(),
        };
        let part = |track: usize| MidiPart {
            track,
            channel: 0,
            patch: 0,
            note_duration: true,
        };
        MidiData {
            division: 480,
            maxpos: (low.len() + high.len()) as i64 * 120,
            micsecqn: 500_000,
            tracks: vec![track(0, 0, low), track(1, low.len() as i64, high)],
            parts: vec![part(0), part(1)],
            ..Default::default()
        }
    }

    fn layer_names(song: &Song) -> Vec<String> {
        song.layers.iter().map(|layer| layer.name.clone()).collect()
    }

    fn layer_of(song: &Song, tick: u32, key: u8) -> u32 {
        song.iter_notes()
            .find(|(x, _, note)| *x == tick && note.key == key)
            .map(|(_, layer, _)| layer)
            .unwrap_or_else(|| panic!("tick {tick} key {key} 没有音符"))
    }

    #[test]
    fn voice_split_gives_each_hand_its_own_layer_named_by_the_piano_convention() {
        let midi = two_hands();
        let mut config = enhanced();
        let (plain, plain_report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(plain.layers.len(), 1, "未拆分时单通道一层");
        assert_eq!(plain_report.voiced_notes, 0);

        config.separate_voices = true;
        let (song, report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(report.voiced_notes, 12);
        // 高声部排在前面；GM 0（钢琴族）用「右手/左手」而非「高声部/低声部」。
        assert_eq!(
            layer_names(&song),
            ["Piano 1 · 右手", "Piano 1 · 左手"],
            "层名与层序"
        );
        assert_eq!(layer_of(&song, 1, 79), 0, "高音应在第 0 层");
        assert_eq!(layer_of(&song, 0, 39), 1, "低音应在第 1 层");
        assert_eq!(notes(&song).len(), notes(&plain).len(), "拆分不该增删音符");
    }

    #[test]
    fn voice_split_uses_track_metadata_when_pitch_clustering_would_refuse() {
        // 整体 span 14 < 15 且 poly = 1：聚类分支被双门槛拦住，只能靠轨道元数据分。
        let midi = two_tracks(&[39, 40, 41, 42, 43, 44, 45, 46], &[47, 48, 49, 50, 51, 52, 53]);
        let mut config = enhanced();
        config.separate_voices = true;
        let (auto, _) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(
            layer_names(&auto),
            ["Piano 1 · 右手", "Piano 1 · 左手"],
            "轨道元数据优先"
        );

        config.voice_ignore_metadata = true;
        let (blind, blind_report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(blind_report.voiced_notes, 0, "无视元数据后应被门槛拒绝");
        assert_eq!(layer_names(&blind), ["Piano 1"]);
        assert_same_song(&blind, &import_midi_enhanced(&midi, &options(), &enhanced()).unwrap().0);
    }

    #[test]
    fn voice_split_and_combination_slots_share_the_layer_block_by_tick() {
        // 两个槽位 × 两个声部：同 tick 的两条槽位不能撞层，各自再占一层。
        let midi = two_hands();
        let mut config = enhanced();
        three_slots(&mut config); // 槽位 0/6/7，这里只取前两个够用
        config.channels[0].slots = [slot(0, 100), slot(6, 50), MidiSlot::default()];
        config.separate_voices = true;
        let (song, report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(report.voiced_notes, 12);
        assert_eq!(
            layer_names(&song),
            [
                "Piano 1 · 右手",
                "Piano 1 · 右手 2",
                "Piano 1 · 左手",
                "Piano 1 · 左手 2"
            ],
            "声部组内按 tick 压紧，第二层带序号"
        );
        // 高声部在前的两层，同 tick 的两个槽位各占一层；低声部接在后两层。
        let group = |tick: u32| {
            let mut layers: Vec<_> = song
                .iter_notes()
                .filter(move |(x, _, _)| *x == tick)
                .map(|(_, layer, note)| (layer, note.instrument))
                .collect();
            layers.sort();
            layers
        };
        assert_eq!(group(1), [(0, 0), (1, 6)], "tick 1 的高音：两个槽位两层");
        assert_eq!(group(0), [(2, 0), (3, 6)], "tick 0 的低音：后两层");
        assert_eq!(layer_of(&song, 0, 39), 2);
    }

    #[test]
    fn dedupe_keeps_the_first_note_per_tick_instrument_and_key() {
        let mut midi = midi_keys(&[39, 39]);
        midi.tracks[0].events[1].pos = 0;
        let mut config = enhanced();
        config.channels[0].slots = [slot(0, 100); 3];
        let (plain, _) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(plain.total_blocks, 6);

        config.dedupe_notes = true;
        let (song, report) = import_midi_enhanced(&midi, &options(), &config).unwrap();
        assert_eq!(report.deduped, 5, "六块里同 tick 同乐器同音高的五块被删");
        assert_eq!(song.total_blocks, 1);
        assert_eq!(notes(&song)[0].2.key, 39);
        assert_eq!(song.layers.len(), 6, "清空的不回收，层名还占位");
    }

    #[test]
    fn dedupe_collapses_duplicated_tails_and_leaves_tempo_blocks_alone() {
        let mut midi = midi_keys(&[39, 39]);
        midi.tracks[0].events[1].pos = 0;
        midi.tempo_changers = vec![(0, 120.0), (480, 60.0)];
        let opts = MidiImportOptions {
            note_duration: true,
            tempo_changer: true,
            ..options()
        };
        let mut config = enhanced();
        config.channels[0].slots = [slot(0, 100); 3];
        config.dedupe_notes = true;
        let (song, report) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        // 未去重：三个槽位 ×（4 块 + 5 块）= 27 块；去重后 tick 0..4 各留一块。
        assert_eq!(song.iter_notes().filter(|(_, _, n)| n.instrument != 20).count(), 5);
        assert_eq!(report.deduped, 22);
        assert_eq!(
            song.iter_notes()
                .filter(|(_, _, n)| n.instrument == FIRST_CUSTOM_INDEX as u8)
                .count(),
            2,
            "速度变化块不是音符，不能被当成重复删掉"
        );
    }

    /// 一条轨的音符：`(文件 tick 起点, 文件 tick 松开，-1 = 没配对到 note-off)`。
    fn notes_at(notes: &[(i64, i64)]) -> Vec<MidiEvent> {
        notes
            .iter()
            .map(|&(pos, until)| MidiEvent {
                pos,
                until,
                channel: 0,
                note: 60,
                velocity: 100,
                patch: 0,
                part: 0,
            })
            .collect()
    }

    fn stats_midi(tracks: Vec<Vec<MidiEvent>>) -> MidiData {
        MidiData {
            division: 480,
            tracks: tracks
                .into_iter()
                .map(|events| MidiTrack {
                    name: String::new(),
                    events,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn track_length_stats_counts_per_beat_and_skips_noteless_tracks() {
        // 一拍 = 480 tick。第 0 轨四个整拍音，第 1 轨没有音符。
        let midi = stats_midi(vec![
            notes_at(&[(0, 480), (480, 960), (960, 1440), (1440, 1920)]),
            Vec::new(),
        ]);
        let stats = track_length_stats(&midi);
        assert_eq!(stats.len(), 1, "没有音符的轨道不列出，同 Miau 只收 event_count > 0");
        assert_eq!(
            stats[0],
            TrackLengthStats {
                track: 0,
                notes: 4,
                long_notes: 4,
                total_ticks: 1920
            }
        );
        assert_eq!(stats[0].long_ratio(), 1.0);
        assert_eq!(stats[0].avg_ticks(), 480.0);
        assert!(
            !stats[0].is_sustain_candidate(480),
            "长音占比 100% 但平均只有一拍，Miau 的 ≥2 拍门槛要拦住"
        );
    }

    #[test]
    fn unpaired_notes_count_in_the_denominator_with_zero_length() {
        // Miau 的 duration_ticks 初值就是 0，没松开也算一条音符。
        let midi = stats_midi(vec![notes_at(&[(0, 1920), (2000, 3920), (4000, -1)])]);
        let stats = track_length_stats(&midi);
        assert_eq!(stats[0].notes, 3);
        assert_eq!(stats[0].long_notes, 2);
        assert_eq!(stats[0].total_ticks, 3840);
        assert!(stats[0].long_ratio() < 0.75, "占比应低于门槛");
        assert!(
            stats[0].avg_ticks() > 960.0,
            "平均时值反而够，实得 {}",
            stats[0].avg_ticks()
        );
        assert!(
            !stats[0].is_sustain_candidate(480),
            "两条门槛是「且」，平均够不代表能预选"
        );
    }

    #[test]
    fn sustain_candidate_tracks_uses_both_miau_thresholds() {
        // 第 0 轨八分音符跑动，第 1 轨四小节铺底，第 2 轨正好踩在 0.75 占比的边界上。
        let midi = stats_midi(vec![
            notes_at(&[(0, 240), (240, 480), (480, 720), (720, 960)]),
            notes_at(&[(0, 1920), (1920, 3840), (3840, 5760), (5760, 7680)]),
            notes_at(&[(0, 1920), (2000, 3920), (4000, 5920), (6000, -1)]),
        ]);
        let stats = track_length_stats(&midi);
        assert_eq!(stats[0].long_ratio(), 0.0, "八分音符不该算长音");
        assert_eq!(stats[2].long_ratio(), 0.75);
        assert_eq!(stats[2].avg_ticks(), 1440.0);
        assert_eq!(
            sustain_candidate_tracks(&midi),
            vec![1, 2],
            "Miau 两处都用 >= 比较，占比恰好 0.75 的边界轨应入选"
        );
    }

    fn held_note(key: i32, ticks: i64) -> MidiData {
        let mut midi = midi_keys(&[key]);
        midi.tracks[0].events[0].until = ticks * 120;
        midi.maxpos = ticks * 120;
        midi
    }

    fn placed(song: &Song) -> Vec<(u32, u8)> {
        notes(song).iter().map(|(x, _, n)| (*x, n.velocity)).collect()
    }

    /// 原版尾音和延音都挂在 `note_duration` 下面，默认值是关的。
    fn held_opts() -> MidiImportOptions {
        MidiImportOptions {
            note_duration: true,
            ..options()
        }
    }

    #[test]
    fn sustain_retriggers_at_measured_decay_period_instead_of_every_tick() {
        let midi = held_note(39, 40);
        let opts = held_opts();
        let (dense, _) = import_midi_enhanced(&midi, &opts, &enhanced()).unwrap();
        // enda 是最后一个音头所在的 tick：39 tick 播满 5 秒 → 7.8 TPS。
        assert_eq!(dense.enda, 39);
        assert!(
            (f64::from(dense.meta.tempo) - 7.8).abs() < 0.01,
            "实得 {}",
            dense.meta.tempo
        );
        assert_eq!(placed(&dense).len(), 40, "原版尾音每 tick 一块");
        let head = placed(&dense)[0].1;

        let mut config = enhanced();
        config.sustain = SustainMode::All;
        config.sustain_fade = false;
        let (song, _) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        assert_eq!(song.meta.tempo, dense.meta.tempo, "延音不许改速度");
        // 重敲间隔由「实测衰减毫秒 ÷ 每 tick 毫秒」四舍五入得到，测试用同一个公式反推，
        // 免得曲速微调就把断言写死在 2 还是 3 上。
        let ticks: Vec<u32> = placed(&song).iter().map(|(x, _)| *x).collect();
        let ms_per_tick = 1000.0 / f64::from(song.meta.tempo);
        let step = (f64::from(crate::midi_decay::SAMPLE_DECAYS[0].decay_ms) / ms_per_tick)
            .round()
            .max(1.0) as usize;
        assert_eq!(
            ticks,
            (0..40u32).step_by(step).collect::<Vec<_>>(),
            "每 {step} tick 重敲一次（{ms_per_tick:.1}ms/tick，实测衰减 320ms）"
        );
        assert!(
            40 - (*ticks.last().unwrap_or(&0) as usize) <= step,
            "关掉渐弱应敲满整个音长"
        );
        assert!(
            placed(&song).iter().all(|(_, vel)| *vel == head),
            "关渐弱时每次重敲保持音头音量 {head}"
        );
    }

    #[test]
    fn every_tick_density_uses_miau_step_with_the_envelope_volume() {
        let midi = held_note(39, 40);
        let opts = held_opts();
        let mut config = enhanced();
        config.sustain = SustainMode::All;
        config.sustain_density = SustainDensity::EveryTick;
        config.sustain_fade = false;
        let (song, _) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        let placed = placed(&song);
        assert_eq!(
            placed.iter().map(|(x, _)| *x).collect::<Vec<_>>(),
            (0..40u32).collect::<Vec<_>>(),
            "密度档选每 tick 时步长就该是 1，和 Miau 的 for 循环一致"
        );
        let head = placed[0].1;
        assert!(
            placed.iter().all(|(_, vel)| *vel == head),
            "关渐弱时保持音头音量 {head}，而不是传统尾音的半音量：{placed:?}"
        );
        assert!(
            notes(&song).iter().all(|(_, _, note)| note.panning == 100),
            "包络延音不参与原版的左右交替，否则试听里听不出是包络在起作用"
        );
    }

    #[test]
    fn sustain_fade_tapers_along_the_measured_envelope_and_stops_at_silence() {
        let midi = held_note(39, 40);
        let opts = held_opts();
        let mut config = enhanced();
        config.sustain = SustainMode::All;
        let (song, _) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        let placed = placed(&song);
        let head = placed[0].1;
        assert_eq!(placed[0], (0, head), "音头保持原音量");
        assert!(
            placed.windows(2).all(|w| w[1].1 <= w[0].1),
            "包络只许单调不增：{placed:?}"
        );
        assert!(
            placed.last().unwrap().1 > 0,
            "最后一敲必须还听得见：{placed:?}"
        );
        // 实测 harp 320ms 衰减、音长 39 tick：渐弱比「敲满音长」早收，且贴着包络走。
        let step = (f64::from(crate::midi_decay::SAMPLE_DECAYS[0].decay_ms)
            / (1000.0 / f64::from(song.meta.tempo)))
            .round()
            .max(1.0) as usize;
        let full = (0..40u32).step_by(step).count();
        assert!(
            placed.len() < full,
            "渐弱应在音长结束前收声：敲满应 {} 块，实得 {}",
            full,
            placed.len()
        );
        let harp = crate::midi_decay::SAMPLE_DECAYS[0];
        for (i, (tick, vel)) in placed.iter().enumerate() {
            let expected = (f64::from(head) * gain_at(&harp.env, f64::from(*tick) / 39.0)).round() as i32;
            assert!(
                (i32::from(*vel) - expected).abs() <= 1,
                "第 {i} 敲 tick={tick} 音量 {vel}，包络应给 {expected}"
            );
        }
    }

    /// 两轨各持音 40 tick，音高、通道、patch 全一样，只差轨道号。
    fn two_held_tracks() -> (MidiData, MidiImportOptions) {
        let mut midi = held_note(39, 40);
        let mut second = midi.tracks[0].events.clone();
        for ev in &mut second {
            ev.part = 1;
        }
        midi.tracks.push(MidiTrack {
            name: "Pad".into(),
            events: second,
        });
        midi.parts.push(MidiPart {
            track: 1,
            channel: 0,
            patch: 0,
            note_duration: true,
        });
        let opts = MidiImportOptions {
            part_durations: vec![true, true],
            ..held_opts()
        };
        (midi, opts)
    }

    #[test]
    fn sustain_tracks_mode_leaves_other_tracks_on_the_legacy_tail() {
        let (midi, opts) = two_held_tracks();
        let mut config = enhanced();
        config.sustain = SustainMode::Tracks;
        config.sustain_tracks = vec![1];
        config.sustain_fade = false;
        let (song, _) = import_midi_enhanced(&midi, &opts, &config).unwrap();
        let step = (f64::from(crate::midi_decay::SAMPLE_DECAYS[0].decay_ms)
            / (1000.0 / f64::from(song.meta.tempo)))
            .round()
            .max(1.0) as usize;
        let by_layer = |layer: u32| -> Vec<(u32, u8)> {
            notes(&song)
                .into_iter()
                .filter(|(_, y, _)| *y == layer)
                .map(|(x, _, n)| (x, n.velocity))
                .collect()
        };
        let unlisted = by_layer(0);
        let listed = by_layer(1);
        let head = unlisted[0].1;
        assert_eq!(
            unlisted.iter().map(|(x, _)| *x).collect::<Vec<_>>(),
            (0..40u32).collect::<Vec<_>>(),
            "白名单外的第 0 轨仍是每 tick 一块"
        );
        assert!(
            unlisted[1..].iter().all(|(_, vel)| *vel == head / 2),
            "原版尾音固定在音头的一半音量（{head}→{}）：{unlisted:?}",
            head / 2
        );
        assert_eq!(
            listed.iter().map(|(x, _)| *x).collect::<Vec<_>>(),
            (0..40u32).step_by(step).collect::<Vec<_>>(),
            "白名单里的第 1 轨按实测衰减每 {step} tick 重敲"
        );
        assert_eq!(
            listed[0].1, head,
            "两条轨的音头音量应一致，否则这个对照不成立：{listed:?}"
        );
        assert!(
            listed.iter().all(|(_, vel)| *vel == head),
            "关渐弱时白名单轨保持音头音量 {head}：{listed:?}"
        );
    }

    #[test]
    fn gain_at_interpolates_the_table_and_clamps_both_ends() {
        let harp = crate::midi_decay::SAMPLE_DECAYS[0];
        assert_eq!(gain_at(&harp.env, -1.0), 1.0);
        assert!((gain_at(&harp.env, 0.5) - (0.1783 + 0.1139) / 2.0).abs() < 1e-6);
        assert!((gain_at(&harp.env, 1.0) - 0.0001).abs() < 1e-6);
        assert!((gain_at(&harp.env, 9.0) - 0.0001).abs() < 1e-6);
    }
}

