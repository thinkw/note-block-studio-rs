use std::collections::VecDeque;

use midly::{Format, MetaMessage, MidiMessage, Timing, TrackEventKind};

const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TRACKS: usize = 1024;
const MAX_EVENTS: usize = 2_000_000;
const MAX_NOTES: usize = 1_000_000;
const MAX_NAME_BYTES: usize = 4096;
const NBS_NOTES_PER_TRACK: usize = 31_999;

#[derive(Clone, Debug)]
pub struct MidiPreview {
    pub tracks: Vec<MidiPreviewTrack>,
    pub notes: Vec<MidiPreviewNote>,
    pub duration_seconds: f64,
    pub warnings: Vec<String>,
    pub import_blocked_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MidiPreviewTrack {
    pub name: String,
    pub note_count: usize,
    pub programs: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct MidiPreviewNote {
    pub track: usize,
    pub channel: u8,
    pub program: u8,
    pub key: u8,
    pub velocity: u8,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

struct TimedEvent {
    tick: u64,
    track: usize,
    order: usize,
    kind: TrackEventKind<'static>,
}

pub fn parse_midi_preview(bytes: &[u8]) -> Result<MidiPreview, String> {
    let chunks = track_chunks(bytes)?;
    let (header, _) = midly::parse(bytes).map_err(|e| format!("Invalid MIDI: {e}"))?;
    if header.format == Format::Sequential {
        return Err(
            "SMF format 2 contains independent sequences and cannot be merged for preview".into(),
        );
    }
    let mut preview = MidiPreview {
        tracks: Vec::with_capacity(chunks.len()),
        notes: Vec::new(),
        duration_seconds: 0.0,
        warnings: Vec::new(),
        import_blocked_reason: None,
    };
    let mut events = Vec::new();
    let mut event_count = 0;
    let mut note_count = 0;
    let mut last_tick = 0;
    for (track, chunk) in chunks.into_iter().enumerate() {
        let mut info = MidiPreviewTrack {
            name: format!("Track {}", track + 1),
            note_count: 0,
            programs: Vec::new(),
        };
        let mut named = false;
        let mut ended = false;
        let mut tick = 0u64;
        let mut iter = midly::EventIter::new(chunk);
        while !iter.unread().is_empty() {
            if ended {
                return Err(format!(
                    "Track {} contains data after EndOfTrack",
                    track + 1
                ));
            }
            event_count += 1;
            if event_count > MAX_EVENTS {
                return Err(format!(
                    "MIDI exceeds the preview limit of {MAX_EVENTS} events"
                ));
            }
            // midly without its strict feature can silently accept truncated events.
            validate_event(iter.unread(), iter.running_status())
                .map_err(|e| format!("Track {}: {e}", track + 1))?;
            let event = iter
                .next()
                .ok_or_else(|| format!("Malformed MIDI event in track {}", track + 1))?
                .map_err(|e| format!("Invalid MIDI event in track {}: {e}", track + 1))?;
            tick = tick
                .checked_add(event.delta.as_int() as u64)
                .ok_or("MIDI tick count overflow")?;
            match event.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(name)) if !named => {
                    if name.len() > MAX_NAME_BYTES {
                        return Err(format!(
                            "Track {} name exceeds {MAX_NAME_BYTES} bytes",
                            track + 1
                        ));
                    }
                    info.name = String::from_utf8_lossy(name).into_owned();
                    named = true;
                }
                TrackEventKind::Meta(MetaMessage::EndOfTrack) => ended = true,
                TrackEventKind::Meta(MetaMessage::Tempo(tempo)) => {
                    if tempo.as_int() == 0 {
                        return Err("MIDI tempo must be greater than zero".into());
                    }
                    events.push(TimedEvent {
                        tick,
                        track,
                        order: event_count,
                        kind: event.kind.to_static(),
                    });
                }
                TrackEventKind::Midi { message, .. } => {
                    if matches!(message, MidiMessage::NoteOn { vel, .. } if vel.as_int() != 0) {
                        note_count += 1;
                        if note_count > MAX_NOTES {
                            return Err(format!(
                                "MIDI exceeds the preview limit of {MAX_NOTES} notes"
                            ));
                        }
                        info.note_count += 1;
                    }
                    if matches!(
                        message,
                        MidiMessage::NoteOn { .. }
                            | MidiMessage::NoteOff { .. }
                            | MidiMessage::ProgramChange { .. }
                    ) {
                        events.push(TimedEvent {
                            tick,
                            track,
                            order: event_count,
                            kind: event.kind.to_static(),
                        });
                    }
                }
                _ => {}
            }
        }
        if !ended {
            preview.warnings.push(format!(
                "Track {} has no EndOfTrack; its chunk boundary is used",
                track + 1
            ));
        }
        last_tick = last_tick.max(tick);
        preview.tracks.push(info);
    }
    events.sort_unstable_by_key(|event| (event.tick, event.track, event.order));
    let mut seconds = 0.0;
    let mut tick = 0;
    let mut tempo = 500_000;
    let mut programs = [0u8; 16];
    let mut melodic_programs = 0u128;
    let mut channel_programs = [0u128; 16];
    let mut active: Vec<VecDeque<usize>> = (0..16 * 128).map(|_| VecDeque::new()).collect();
    let mut unmatched_off = 0;
    let mut overlapping = 0;
    for event in events {
        seconds += tick_seconds(event.tick - tick, header.timing, tempo);
        tick = event.tick;
        match event.kind {
            TrackEventKind::Meta(MetaMessage::Tempo(value)) => tempo = value.as_int(),
            TrackEventKind::Midi { channel, message } => {
                let channel = channel.as_int();
                match message {
                    MidiMessage::ProgramChange { program } => {
                        programs[channel as usize] = program.as_int();
                        add_program(&mut preview.tracks[event.track], program.as_int());
                    }
                    MidiMessage::NoteOn { key, vel } if vel.as_int() != 0 => {
                        let program = programs[channel as usize];
                        add_program(&mut preview.tracks[event.track], program);
                        if channel != 9 {
                            melodic_programs |= 1u128 << program;
                            channel_programs[channel as usize] |= 1u128 << program;
                        }
                        let queue = &mut active[channel as usize * 128 + key.as_int() as usize];
                        if !queue.is_empty() {
                            overlapping += 1;
                        }
                        queue.push_back(preview.notes.len());
                        preview.notes.push(MidiPreviewNote {
                            track: event.track,
                            channel,
                            program,
                            key: key.as_int(),
                            velocity: vel.as_int(),
                            start_seconds: seconds,
                            end_seconds: seconds,
                        });
                    }
                    MidiMessage::NoteOff { key, .. } | MidiMessage::NoteOn { key, .. } => {
                        let queue = &mut active[channel as usize * 128 + key.as_int() as usize];
                        if let Some(index) = queue.pop_front() {
                            preview.notes[index].end_seconds = seconds;
                        } else {
                            unmatched_off += 1;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    seconds += tick_seconds(last_tick - tick, header.timing, tempo);
    preview.duration_seconds = seconds;
    let mut missing_off = 0;
    for index in active.into_iter().flatten() {
        missing_off += 1;
        let note = &mut preview.notes[index];
        note.end_seconds = seconds.max(note.start_seconds + 0.25);
        preview.duration_seconds = preview.duration_seconds.max(note.end_seconds);
    }
    if missing_off != 0 {
        preview.warnings.push(format!("{missing_off} notes have no note-off; preview ends them at the file boundary (at least 0.25 seconds after note-on). The source MIDI is unchanged"));
    }
    if unmatched_off != 0 {
        preview.warnings.push(format!(
            "{unmatched_off} note-off events have no matching note-on"
        ));
    }
    if overlapping != 0 {
        preview.warnings.push(format!("{overlapping} overlapping same-channel, same-key note-ons are paired with note-offs in FIFO order across tracks"));
    }
    let count = melodic_programs.count_ones();
    if count > 15 {
        let reused: Vec<_> = channel_programs
            .iter()
            .enumerate()
            .filter(|(channel, mask)| *channel != 9 && mask.count_ones() > 1)
            .map(|(channel, _)| channel.to_string())
            .collect();
        preview.warnings.push(format!("MIDI uses {count} melodic programs but GM has only 15 melodic channels. Reused channels (0-based): {}. Program changes are channel-wide, including across tracks; no channels or programs were remapped", reused.join(", ")));
    }
    let blocked: Vec<_> = preview
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.note_count > NBS_NOTES_PER_TRACK)
        .map(|(index, track)| format!("track {}: {} note-ons", index + 1, track.note_count))
        .collect();
    if !blocked.is_empty() {
        preview.import_blocked_reason = Some(format!("NBS import blocked: the existing importer retains only {NBS_NOTES_PER_TRACK} note-ons per track ({}). Preview contains every note; importing would silently truncate the MIDI", blocked.join("; ")));
    }
    Ok(preview)
}

fn add_program(track: &mut MidiPreviewTrack, program: u8) {
    if !track.programs.contains(&program) {
        track.programs.push(program);
    }
}

fn tick_seconds(ticks: u64, timing: Timing, tempo: u32) -> f64 {
    match timing {
        Timing::Metrical(ppq) => ticks as f64 * tempo as f64 / (ppq.as_int() as f64 * 1_000_000.0),
        Timing::Timecode(fps, subframes) => {
            let fps = match fps {
                midly::Fps::Fps29 => 30_000.0 / 1001.0,
                other => other.as_int() as f64,
            };
            ticks as f64 / (fps * subframes as f64)
        }
    }
}

fn track_chunks(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!(
            "MIDI exceeds the preview limit of {MAX_FILE_BYTES} bytes"
        ));
    }
    if bytes.len() < 14 || &bytes[..4] != b"MThd" || bytes[4..8] != 6u32.to_be_bytes() {
        return Err("Invalid SMF header (expected MThd with length 6)".into());
    }
    let format = u16::from_be_bytes([bytes[8], bytes[9]]);
    if format == 2 {
        return Err(
            "SMF format 2 contains independent sequences and cannot be merged for preview".into(),
        );
    }
    if format > 1 {
        return Err(format!("Unsupported SMF format {format}"));
    }
    let count = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;
    if count == 0 || count > MAX_TRACKS || (format == 0 && count != 1) {
        return Err(format!("Invalid MIDI track count {count}; preview allows 1..={MAX_TRACKS} tracks and SMF0 requires one"));
    }
    let division = u16::from_be_bytes([bytes[12], bytes[13]]);
    if division == 0
        || (division & 0x8000 != 0
            && (!matches!(bytes[12] as i8, -24 | -25 | -29 | -30) || bytes[13] == 0))
    {
        return Err("Invalid MIDI timing division".into());
    }
    let mut chunks = Vec::with_capacity(count);
    let mut rest = &bytes[14..];
    for _ in 0..count {
        if rest.len() < 8 || &rest[..4] != b"MTrk" {
            return Err("Missing or invalid MIDI track chunk".into());
        }
        let length = u32::from_be_bytes(rest[4..8].try_into().unwrap()) as usize;
        rest = &rest[8..];
        let chunk = rest.get(..length).ok_or("Truncated MIDI track chunk")?;
        chunks.push(chunk);
        rest = &rest[length..];
    }
    if !rest.is_empty() {
        return Err(
            "Unexpected MIDI chunks or trailing bytes beyond the declared track count".into(),
        );
    }
    Ok(chunks)
}

fn read_vlq(bytes: &mut &[u8]) -> Result<u32, String> {
    let mut value = 0;
    for _ in 0..4 {
        let (&byte, rest) = bytes
            .split_first()
            .ok_or("Truncated MIDI variable-length integer")?;
        *bytes = rest;
        value = (value << 7) | (byte & 0x7f) as u32;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("MIDI variable-length integer exceeds four bytes".into())
}

fn validate_event(mut bytes: &[u8], running: Option<u8>) -> Result<(), String> {
    read_vlq(&mut bytes)?;
    let first = *bytes.first().ok_or("Missing MIDI event status")?;
    let status = if first < 0x80 {
        running.ok_or("MIDI running status has no preceding channel message")?
    } else {
        bytes = &bytes[1..];
        first
    };
    match status {
        0x80..=0xef => {
            let size = if matches!(status & 0xf0, 0xc0 | 0xd0) {
                1
            } else {
                2
            };
            let data = bytes.get(..size).ok_or("Truncated MIDI channel message")?;
            if data.iter().any(|byte| *byte >= 0x80) {
                return Err("MIDI channel data must be in 0..127".into());
            }
        }
        0xf0 | 0xf7 | 0xff => {
            let meta = if status == 0xff {
                let (&kind, rest) = bytes.split_first().ok_or("Missing MIDI meta type")?;
                bytes = rest;
                Some(kind)
            } else {
                None
            };
            let length = read_vlq(&mut bytes)? as usize;
            bytes
                .get(..length)
                .ok_or("Truncated MIDI meta/SysEx payload")?;
            let expected = match meta {
                Some(0x00) => Some(2),
                Some(0x20 | 0x21) => Some(1),
                Some(0x2f) => Some(0),
                Some(0x51) => Some(3),
                Some(0x54) => Some(5),
                Some(0x58) => Some(4),
                Some(0x59) => Some(2),
                _ => None,
            };
            if expected.is_some_and(|size| size != length) {
                return Err("Invalid MIDI meta-event payload length".into());
            }
        }
        _ => return Err(format!("Invalid SMF event status {status:#x}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::{num::u28, Header, Smf, TrackEvent};

    fn event(delta: u32, kind: TrackEventKind<'static>) -> TrackEvent<'static> {
        TrackEvent {
            delta: u28::new(delta),
            kind,
        }
    }

    fn message(delta: u32, channel: u8, message: MidiMessage) -> TrackEvent<'static> {
        event(
            delta,
            TrackEventKind::Midi {
                channel: channel.into(),
                message,
            },
        )
    }

    fn on(delta: u32, channel: u8, key: u8, vel: u8) -> TrackEvent<'static> {
        message(
            delta,
            channel,
            MidiMessage::NoteOn {
                key: key.into(),
                vel: vel.into(),
            },
        )
    }

    fn off(delta: u32, channel: u8, key: u8) -> TrackEvent<'static> {
        message(
            delta,
            channel,
            MidiMessage::NoteOff {
                key: key.into(),
                vel: 0.into(),
            },
        )
    }

    fn program(delta: u32, channel: u8, value: u8) -> TrackEvent<'static> {
        message(
            delta,
            channel,
            MidiMessage::ProgramChange {
                program: value.into(),
            },
        )
    }

    fn tempo(delta: u32, value: u32) -> TrackEvent<'static> {
        event(
            delta,
            TrackEventKind::Meta(MetaMessage::Tempo(value.into())),
        )
    }

    fn encode(timing: Timing, mut tracks: Vec<Vec<TrackEvent<'static>>>) -> Vec<u8> {
        let format = if tracks.len() == 1 {
            Format::SingleTrack
        } else {
            Format::Parallel
        };
        for track in &mut tracks {
            track.push(event(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)));
        }
        let smf = Smf {
            header: Header::new(format, timing),
            tracks,
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }

    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn cross_track_programs_and_tempos_follow_ticks_not_track_iteration() {
        let bytes = encode(
            Timing::Metrical(480.into()),
            vec![
                vec![
                    program(0, 0, 40),
                    tempo(480, 1_000_000),
                    program(480, 0, 73),
                ],
                vec![
                    on(240, 0, 60, 93),
                    off(480, 0, 60),
                    on(240, 0, 62, 91),
                    off(480, 0, 62),
                ],
                vec![tempo(720, 250_000)],
            ],
        );
        let original = bytes.clone();
        let parsed = parse_midi_preview(&bytes).unwrap();
        assert_eq!(bytes, original);
        assert_eq!(parsed.notes[0].program, 40);
        assert_eq!(parsed.notes[1].program, 73);
        near(parsed.notes[0].start_seconds, 0.25);
        near(parsed.notes[0].end_seconds, 1.0);
        near(parsed.notes[1].start_seconds, 1.125);
        near(parsed.notes[1].end_seconds, 1.375);
        assert_eq!(parsed.tracks[1].programs, [40, 73]);
    }

    #[test]
    fn chords_long_notes_drums_and_zero_velocity_keep_original_values() {
        let bytes = encode(
            Timing::Metrical(100.into()),
            vec![vec![
                event(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Original"))),
                program(0, 0, 127),
                on(0, 0, 0, 1),
                on(0, 0, 127, 127),
                on(0, 9, 35, 64),
                off(10, 9, 35),
                on(190, 0, 127, 0),
                off(19_800, 0, 0),
            ]],
        );
        let parsed = parse_midi_preview(&bytes).unwrap();
        assert_eq!(parsed.tracks[0].name, "Original");
        assert_eq!(parsed.tracks[0].note_count, 3);
        assert_eq!(parsed.notes.len(), 3);
        assert_eq!(
            (
                parsed.notes[0].key,
                parsed.notes[0].velocity,
                parsed.notes[0].program
            ),
            (0, 1, 127)
        );
        assert_eq!((parsed.notes[1].key, parsed.notes[1].velocity), (127, 127));
        assert_eq!(parsed.notes[2].channel, 9);
        near(parsed.notes[0].end_seconds, 100.0);
        near(parsed.notes[1].end_seconds, 1.0);
        near(parsed.notes[2].end_seconds, 0.05);
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn repeated_keys_use_fifo_and_note_off_can_be_in_another_track() {
        let bytes = encode(
            Timing::Metrical(100.into()),
            vec![
                vec![on(0, 0, 60, 80), on(10, 0, 60, 90), off(10, 0, 60)],
                vec![off(30, 0, 60)],
            ],
        );
        let parsed = parse_midi_preview(&bytes).unwrap();
        near(parsed.notes[0].end_seconds, 0.1);
        near(parsed.notes[1].end_seconds, 0.15);
        assert!(parsed.warnings.iter().any(|w| w.contains("FIFO")));
    }

    #[test]
    fn same_tick_order_is_track_then_event_order() {
        let bytes = encode(
            Timing::Metrical(100.into()),
            vec![
                vec![program(0, 0, 10), on(0, 0, 60, 80), off(100, 0, 60)],
                vec![program(0, 0, 20), on(0, 0, 62, 80), off(100, 0, 62)],
            ],
        );
        let parsed = parse_midi_preview(&bytes).unwrap();
        assert_eq!(
            parsed.notes.iter().map(|n| n.program).collect::<Vec<_>>(),
            [10, 20]
        );
    }

    #[test]
    fn smpte_uses_real_frame_rates_and_ignores_tempo() {
        for (fps, expected) in [
            (midly::Fps::Fps24, 1.25),
            (midly::Fps::Fps25, 1.2),
            (midly::Fps::Fps29, 1.001),
            (midly::Fps::Fps30, 1.0),
        ] {
            let bytes = encode(
                Timing::Timecode(fps, 80),
                vec![vec![
                    on(0, 0, 60, 99),
                    tempo(1200, 900_000),
                    off(1200, 0, 60),
                ]],
            );
            let parsed = parse_midi_preview(&bytes).unwrap();
            near(parsed.duration_seconds, expected);
            near(parsed.notes[0].end_seconds, expected);
        }
    }

    #[test]
    fn missing_note_off_is_finite_and_reported_without_changing_source() {
        let bytes = encode(
            Timing::Metrical(100.into()),
            vec![vec![off(0, 0, 61), on(100, 0, 60, 77)]],
        );
        let parsed = parse_midi_preview(&bytes).unwrap();
        near(parsed.notes[0].start_seconds, 0.5);
        near(parsed.notes[0].end_seconds, 0.75);
        near(parsed.duration_seconds, 0.75);
        assert!(parsed.warnings.iter().any(|w| w.contains("no note-off")));
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("no matching note-on")));
    }

    #[test]
    fn nbs_limit_blocks_import_but_never_truncates_preview() {
        for count in [NBS_NOTES_PER_TRACK, NBS_NOTES_PER_TRACK + 1] {
            let track: Vec<_> = (0..count)
                .flat_map(|_| [on(0, 0, 60, 80), off(1, 0, 60)])
                .collect();
            let bytes = encode(Timing::Metrical(100.into()), vec![track]);
            let parsed = parse_midi_preview(&bytes).unwrap();
            assert_eq!(parsed.notes.len(), count);
            assert_eq!(parsed.tracks[0].note_count, count);
            assert_eq!(
                parsed.import_blocked_reason.is_some(),
                count > NBS_NOTES_PER_TRACK
            );
            if let Some(reason) = parsed.import_blocked_reason {
                assert!(reason.contains("31999") && reason.contains("32000"));
            }
        }
    }

    #[test]
    fn more_than_fifteen_melodic_programs_warns_about_channel_reuse() {
        let track = (0..16)
            .flat_map(|p| [program(0, 0, p), on(0, 0, 60, 80), off(1, 0, 60)])
            .collect();
        let parsed =
            parse_midi_preview(&encode(Timing::Metrical(100.into()), vec![track])).unwrap();
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("16 melodic programs") && w.contains("channel-wide")));
        assert_eq!(
            parsed.notes.iter().map(|n| n.program).collect::<Vec<_>>(),
            (0..16).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rejects_invalid_headers_chunks_events_and_format_two() {
        let valid = encode(
            Timing::Metrical(100.into()),
            vec![vec![on(0, 0, 60, 80), off(100, 0, 60)]],
        );
        for end in 0..valid.len() {
            assert!(
                parse_midi_preview(&valid[..end]).is_err(),
                "accepted prefix {end}"
            );
        }
        for (offset, value) in [(9, 2), (13, 0), (12, 0x80), (11, 0), (23, 0x70), (24, 0xff)] {
            let mut corrupt = valid.clone();
            corrupt[offset] = value;
            assert!(
                parse_midi_preview(&corrupt).is_err(),
                "accepted mutation at {offset}"
            );
        }
        let mut format_two = valid.clone();
        format_two[9] = 2;
        assert!(parse_midi_preview(&format_two)
            .unwrap_err()
            .contains("format 2"));
        for event in [
            vec![0, 0xff, 1, 10, b'x'],
            vec![0x80, 0x80, 0x80, 0x80, 0x90, 60, 80],
            vec![0, 0xff, 0x51, 2, 1, 1],
        ] {
            assert!(validate_event(&event, None).is_err());
        }
    }

    #[test]
    fn file_and_note_limits_fail_explicitly() {
        assert!(parse_midi_preview(&vec![0; MAX_FILE_BYTES + 1])
            .unwrap_err()
            .contains("bytes"));
        let mut track = vec![on(0, 0, 60, 80); MAX_NOTES + 1];
        track.push(off(100, 0, 60));
        let bytes = encode(Timing::Metrical(100.into()), vec![track]);
        assert!(parse_midi_preview(&bytes)
            .unwrap_err()
            .contains("1000000 notes"));
    }
}
