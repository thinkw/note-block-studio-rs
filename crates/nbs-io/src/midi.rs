//! Standard MIDI File (SMF) 导入 / 导出。
//!
//! 导入量化对齐原版 `import_midi.gml`：
//!   `deltapertick = (PPQ & 0x7FFF) / (4 * (precision + 1))`
//!   `nbs_tick = floor((midi_tick - minpos * remove_silent) / deltapertick)`
//!   `key = midi_note - 21`，channel 9 打击乐走独立映射表。
//! 导出为 format 1（每条 layer 一个 track），PPQ=480，1 NBS tick = 120 MIDI tick。

use std::path::Path;

use nbs_core::model::{CustomInstrument, Note, Song, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN};

const PPQ: u16 = 480;
const MIDI_TICKS_PER_NBS_TICK: u32 = PPQ as u32 / 4;

#[derive(Clone, Debug)]
pub struct MidiImportOptions {
    /// 0/1/3/7 → 精度倍率 1/2/4/8。
    pub precision: u8,
    pub remove_silent: bool,
    pub use_velocity: bool,
    /// 把音高折叠到 MC 可播放的 33..57。
    pub fold_octaves: bool,
    /// 用 Tempo Changer 自定义乐器写入分段速度。
    pub tempo_changer: bool,
    /// 同 tick 和弦最多占用的层数（原版 1..20，20 表示不限）。
    pub max_height: u8,
    pub octave_shift: i32,
    pub split_channels: bool,
}

impl Default for MidiImportOptions {
    fn default() -> Self {
        Self {
            precision: 1,
            remove_silent: true,
            use_velocity: true,
            fold_octaves: false,
            tempo_changer: false,
            max_height: 20,
            octave_shift: 0,
            split_channels: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MidiNote {
    pub abs_tick: u32,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    pub program: u8,
}

#[derive(Clone, Debug)]
pub struct MidiData {
    pub format: u16,
    pub ppq: u16,
    pub notes: Vec<MidiNote>,
    pub tempos: Vec<(u32, f32)>,
    pub max_tick: u32,
    pub track_count: usize,
}

#[derive(Clone, Debug)]
enum RawKind {
    NoteOn(u8, u8, u8), // channel, note, velocity
    Program(u8, u8),    // channel, program
    Tempo(f32),         // BPM
}

#[derive(Clone, Debug)]
struct RawEvent {
    abs_tick: u32,
    order: usize,
    kind: RawKind,
}

// ---------------------------------------------------------------- 解析

fn be_u16(b: &[u8], p: usize) -> Result<u16, String> {
    if p + 2 > b.len() {
        return Err("文件意外结束 (u16)".into());
    }
    Ok(u16::from_be_bytes([b[p], b[p + 1]]))
}
fn be_u32(b: &[u8], p: usize) -> Result<u32, String> {
    if p + 4 > b.len() {
        return Err("文件意外结束 (u32)".into());
    }
    Ok(u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]))
}

fn read_varlen(data: &[u8], pos: &mut usize) -> Result<u32, String> {
    let mut value: u32 = 0;
    for _ in 0..4 {
        if *pos >= data.len() {
            return Err("文件意外结束 (varlen)".into());
        }
        let b = data[*pos];
        *pos += 1;
        value = (value << 7) | (b & 0x7F) as u32;
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("非法的变长整数".into())
}

/// 解析 SMF 字节流。
pub fn parse_midi(data: &[u8]) -> Result<MidiData, String> {
    if data.len() < 14 || &data[0..4] != b"MThd" {
        return Err("不是有效的 MIDI 文件（缺少 MThd）".into());
    }
    let header_len = be_u32(data, 4)? as usize;
    let format = be_u16(data, 8)?;
    let tracks = be_u16(data, 10)? as usize;
    let division = be_u16(data, 12)?;
    let ppq = division & 0x7FFF;
    let mut pos = 8 + header_len;
    let mut raw: Vec<RawEvent> = Vec::new();
    let mut order = 0usize;
    let mut max_tick = 0u32;

    for _track in 0..tracks {
        if pos + 8 > data.len() {
            break;
        }
        if &data[pos..pos + 4] != b"MTrk" {
            // 跳过未知 chunk
            let len = be_u32(data, pos + 4)? as usize;
            pos = (pos + 8 + len).min(data.len());
            continue;
        }
        let len = be_u32(data, pos + 4)? as usize;
        let start = pos + 8;
        let end = (start + len).min(data.len());
        pos = end;
        let mut p = start;
        let mut abs: u32 = 0;
        let mut running: u8 = 0;
        let mut last_status_at = 0u32;

        while p < end {
            abs = abs.saturating_add(read_varlen(data, &mut p)?);
            if p >= end {
                break;
            }
            let first = data[p];
            let (status, consumed) = if first & 0x80 != 0 {
                running = first;
                last_status_at = abs;
                p += 1;
                (first, true)
            } else if running != 0 && abs == last_status_at {
                (running, false)
            } else {
                p += 1;
                continue;
            };
            let _ = consumed;
            let high = status & 0xF0;
            let channel = status & 0x0F;
            match high {
                0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => {
                    if p + 2 > end {
                        break;
                    }
                    let d1 = data[p];
                    let d2 = data[p + 1];
                    p += 2;
                    if high == 0x90 && d2 > 0 {
                        raw.push(RawEvent {
                            abs_tick: abs,
                            order,
                            kind: RawKind::NoteOn(channel, d1, d2),
                        });
                        order += 1;
                    }
                }
                0xC0 | 0xD0 => {
                    if p >= end {
                        break;
                    }
                    let d = data[p];
                    p += 1;
                    if high == 0xC0 {
                        raw.push(RawEvent { abs_tick: abs, order, kind: RawKind::Program(channel, d) });
                        order += 1;
                    }
                }
                0xFF => {
                    if p >= end {
                        break;
                    }
                    let meta = data[p];
                    p += 1;
                    let mlen = read_varlen(data, &mut p)? as usize;
                    if p + mlen > end {
                        break;
                    }
                    if meta == 0x51 && mlen >= 3 {
                        let uspq = ((data[p] as u32) << 16)
                            | ((data[p + 1] as u32) << 8)
                            | data[p + 2] as u32;
                        if uspq > 0 {
                            let bpm = 60_000_000.0 / uspq as f32;
                            raw.push(RawEvent { abs_tick: abs, order, kind: RawKind::Tempo(bpm) });
                            order += 1;
                        }
                    }
                    p += mlen;
                }
                0xF0 | 0xF7 => {
                    let slen = read_varlen(data, &mut p)? as usize;
                    p = (p + slen).min(end);
                }
                _ => {
                    // 未知 status：跳过一个字节避免死循环
                    p += 1;
                }
            }
            max_tick = max_tick.max(abs);
        }
    }

    raw.sort_by_key(|e| (e.abs_tick, e.order));
    let mut programs = [0u8; 16];
    let mut notes = Vec::new();
    let mut tempos = Vec::new();
    for e in raw {
        match e.kind {
            RawKind::Program(ch, p) => programs[ch as usize] = p,
            RawKind::NoteOn(ch, n, v) => notes.push(MidiNote {
                abs_tick: e.abs_tick,
                channel: ch,
                note: n,
                velocity: v,
                program: programs[ch as usize],
            }),
            RawKind::Tempo(bpm) => tempos.push((e.abs_tick, bpm)),
        }
    }
    Ok(MidiData { format, ppq, notes, tempos, max_tick, track_count: tracks })
}

// ---------------------------------------------------------------- 导入

/// GM program (0..127) → NBS 内置乐器索引（0..15）。
pub fn gm_program_to_instrument(program: u8) -> u8 {
    match program / 8 {
        0 => 0,  // Piano → Harp
        1 => 9,  // Chromatic Percussion → Xylophone
        2 => 7,  // Organ → Bell
        3 => 5,  // Guitar
        4 => 1,  // Bass → Double Bass
        5 => 0,  // Strings → Harp
        6 => 0,  // Ensemble
        7 => 7,  // Brass → Bell
        8 => 6,  // Reed → Flute
        9 => 6,  // Pipe → Flute
        10 => 13, // Synth Lead → Bit
        11 => 12, // Synth Pad → Didgeridoo
        12 => 13, // Synth Effects → Bit
        13 => 14, // Ethnic → Banjo
        14 => 10, // Percussive → Iron Xylophone
        _ => 4,   // Sound Effects → Click
    }
}

/// MIDI 打击乐 note → (NBS 乐器, NBS key)。channel 9 专用。
pub fn drum_mapping(note: u8) -> (u8, u8) {
    match note {
        35 | 36 => (2, 45),       // Bass Drum
        38 | 40 => (3, 45),       // Snare
        42 | 44 | 46 => (4, 45),  // Hi-hat → Click
        41 | 43 | 45 | 47 | 48 | 50 => (2, 45), // Toms
        49 | 51 | 57 | 59 => (3, 50), // Cymbal
        56 => (11, 45),          // Cowbell
        54 => (4, 45),           // Tambourine
        _ => (3, 45),
    }
}

pub fn import_midi_bytes(data: &[u8], opts: &MidiImportOptions) -> Result<Song, String> {
    let midi = parse_midi(data)?;
    Ok(import_midi_data(&midi, opts))
}

pub fn import_midi_path(path: &Path, opts: &MidiImportOptions) -> Result<Song, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取 MIDI 失败: {e}"))?;
    import_midi_bytes(&data, opts)
}

pub fn import_midi_data(midi: &MidiData, opts: &MidiImportOptions) -> Song {
    let mut song = Song::new();
    let mult = (opts.precision as u32 + 1).max(1);
    let deltapertick = (midi.ppq as u32 / 4 / mult).max(1);
    let minpos = if opts.remove_silent {
        midi.notes.iter().map(|n| n.abs_tick).min().unwrap_or(0)
    } else {
        0
    };

    let mut occupied: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut max_layer = 0u32;
    for n in &midi.notes {
        let abs = n.abs_tick.saturating_sub(minpos);
        let tick = abs / deltapertick;
        if tick > 200_000 {
            continue;
        }
        let (instrument, mut key_i) = if n.channel == 9 {
            if n.note < 24 || n.note > 84 {
                continue;
            }
            let (ins, key) = drum_mapping(n.note);
            (ins, key as i32)
        } else {
            (
                gm_program_to_instrument(n.program),
                n.note as i32 - 21 + opts.octave_shift,
            )
        };
        if opts.fold_octaves {
            while key_i < MC_KEY_MIN as i32 {
                key_i += 12;
            }
            while key_i > MC_KEY_MAX as i32 {
                key_i -= 12;
            }
        }
        let key = key_i.clamp(0, KEY_MAX as i32) as u8;
        let velocity = if opts.use_velocity {
            n.velocity.min(100)
        } else {
            100
        };
        let mut layer = if opts.split_channels {
            (n.channel as u32 % 8) * 2
        } else {
            0
        };
        let limit = if opts.max_height >= 20 { 200 } else { opts.max_height as u32 };
        let mut tries = 0u32;
        while occupied.contains(&(tick, layer)) && tries < limit {
            layer += 1;
            tries += 1;
        }
        occupied.insert((tick, layer));
        max_layer = max_layer.max(layer);
        song.set(
            tick,
            layer,
            Note { instrument, key, velocity, panning: 100, pitch: 0 },
        );
    }

    while song.layers.len() <= max_layer as usize {
        song.layers.push(Default::default());
    }
    for (i, l) in song.layers.iter_mut().enumerate() {
        l.name = format!("MIDI 层 {}", i + 1);
    }

    // 速度：优先用 tempo meta，BPM → tps = BPM/15
    let bpm = midi.tempos.first().map(|(_, b)| *b).unwrap_or(150.0);
    song.meta.tempo = (bpm / 15.0).clamp(0.25, 60.0);
    song.meta.name = "MIDI 导入".into();

    if opts.tempo_changer && !midi.tempos.is_empty() {
        song.custom_instruments.push(CustomInstrument {
            name: "Tempo Changer".into(),
            filename: String::new(),
            key: 45,
            press: false,
        });
        let ins = 16u8;
        for (abs, b) in &midi.tempos {
            let tick = abs.saturating_sub(minpos) / deltapertick;
            let mut n = Note::new(ins, 39);
            n.pitch = (b * mult as f32) as i16;
            song.set(tick, 0, n);
        }
    }
    song.recalculate();
    song.dirty = true;
    song
}

// ---------------------------------------------------------------- 导出

fn write_varlen(out: &mut Vec<u8>, mut value: u32) {
    let mut bytes = [0u8; 4];
    let mut i = 4;
    bytes[i - 1] = (value & 0x7F) as u8;
    i -= 1;
    value >>= 7;
    while value > 0 {
        bytes[i - 1] = ((value & 0x7F) | 0x80) as u8;
        i -= 1;
        value >>= 7;
    }
    out.extend_from_slice(&bytes[i..]);
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
}

/// 导出为 SMF format 1：track 0 是速度/拍号，之后每条 layer 一个 track。
pub fn export_midi_bytes(song: &Song) -> Vec<u8> {
    let bpm = (song.meta.tempo * 15.0).round().clamp(1.0, 1000.0);
    let uspq = (60_000_000.0 / bpm as f64).round() as u32;
    let mut t0 = Vec::new();
    write_varlen(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x51, 0x03]);
    t0.extend_from_slice(&[(uspq >> 16) as u8, (uspq >> 8) as u8, uspq as u8]);
    write_varlen(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x58, 0x04, song.meta.time_signature, 2, 24, 8]);
    write_varlen(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    let mut out = Vec::new();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes()); // format 1
    header.extend_from_slice(&((song.layers.len() as u16) + 1).to_be_bytes());
    header.extend_from_slice(&PPQ.to_be_bytes());
    write_chunk(&mut out, b"MThd", &header);
    write_chunk(&mut out, b"MTrk", &t0);

    for layer in 0..song.layers.len() as u32 {
        let mut events: Vec<(u32, Vec<u8>)> = Vec::new();
        for (tick, l, note) in song.iter_notes() {
            if l != layer {
                continue;
            }
            let midi_tick = tick * MIDI_TICKS_PER_NBS_TICK;
            let midi_note = (note.key as i32 + 21).clamp(0, 127) as u8;
            let vel = note.velocity.min(127).max(1);
            events.push((midi_tick, vec![0x90, midi_note, vel]));
            events.push((
                midi_tick + (MIDI_TICKS_PER_NBS_TICK / 2).max(1),
                vec![0x80, midi_note, 0],
            ));
        }
        events.sort_by_key(|e| e.0);
        let mut track = Vec::new();
        let mut prev = 0u32;
        for (tick, ev) in &events {
            write_varlen(&mut track, tick - prev);
            track.extend_from_slice(ev);
            prev = *tick;
        }
        write_varlen(&mut track, 0);
        track.extend_from_slice(&[0xFF, 0x2F, 0x00]);
        write_chunk(&mut out, b"MTrk", &track);
    }
    out
}

pub fn export_midi(song: &Song, path: &Path) -> Result<(), String> {
    std::fs::write(path, export_midi_bytes(song)).map_err(|e| format!("写入 MIDI 失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_roundtrip_preserves_note_count() {
        let song = Song::demo();
        let bytes = export_midi_bytes(&song);
        assert_eq!(&bytes[0..4], b"MThd");
        let midi = parse_midi(&bytes).unwrap();
        assert_eq!(midi.format, 1);
        assert_eq!(midi.notes.len(), song.total_blocks);
        let back = import_midi_data(&midi, &MidiImportOptions::default());
        assert_eq!(back.total_blocks, song.total_blocks);
    }

    #[test]
    fn import_options_quantize() {
        let midi = MidiData {
            format: 0,
            ppq: 480,
            notes: vec![
                MidiNote { abs_tick: 0, channel: 0, note: 60, velocity: 90, program: 0 },
                MidiNote { abs_tick: 60, channel: 0, note: 64, velocity: 90, program: 0 },
                MidiNote { abs_tick: 120, channel: 0, note: 67, velocity: 90, program: 0 },
            ],
            tempos: vec![(0, 150.0)],
            max_tick: 120,
            track_count: 1,
        };
        let opts = MidiImportOptions { precision: 1, use_velocity: true, ..Default::default() };
        let song = import_midi_data(&midi, &opts);
        assert_eq!(song.total_blocks, 3);
        // precision=1 → deltapertick = 480/(4*2) = 60 → tick 0,1,2
        assert_eq!(song.song_length(), 3);
        assert_eq!(song.get(0, 0).unwrap().key, 39);
        assert_eq!(song.get(0, 0).unwrap().velocity, 90);
    }

    #[test]
    fn varlen_roundtrip() {
        for v in [0u32, 1, 127, 128, 8192, 0x0FFFFF] {
            let mut out = Vec::new();
            write_varlen(&mut out, v);
            let mut p = 0;
            assert_eq!(read_varlen(&out, &mut p).unwrap(), v);
        }
    }
}
