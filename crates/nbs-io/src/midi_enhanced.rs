// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from MiauNoteBlock contributors (c69dcce), src/static/js/nbs_client.js and modules/28-midi-dialog.js; license: ../MIAU-LICENSE.

use nbs_core::model::{Song, FIRST_CUSTOM_INDEX, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

use crate::midi::{
    import_midi_data, import_resolved, resolve, MidiData, MidiImportOptions, Resolved,
};
use crate::midi_miau::{DEFAULT_SUBSTITUTES, GM_COMBINATIONS, INSTRUMENT_OFFSETS};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedMidiOptions {
    pub enabled: bool,
    pub range_mode: RangeMode,
    pub smart_substitute: bool,
    pub force_fold: bool,
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
    let mut events: Vec<Vec<(Resolved, u16)>> = Vec::with_capacity(midi.tracks.len());
    for track in &midi.tracks {
        let mut expanded = Vec::new();
        for ev in &track.events {
            let channel = ev.channel as usize;
            if channel >= 16 || ev.channel > midi.channels {
                continue;
            }
            // Legacy melody mapping is replaced by the independent per-slot mapping below.
            let mut resolved = resolve(midi, opts, ev, dpt);
            if resolved.pos < 0 {
                continue;
            }
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
    let song = import_resolved(midi, opts, &events);
    finish_report(&song, opts.tempo_changer, &mut report);
    Ok((song, report))
}

#[cfg(test)]
mod tests {
    use super::*;
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
                folded: 1
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
                folded: 0
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
}
