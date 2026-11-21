use nbs_core::model::Song;
use nbs_io::datapack::{generate, DatapackOptions};
use nbs_io::midi::{parse_midi, MidiImportOptions};
use nbs_io::midi_enhanced::{import_midi_enhanced, EnhancedMidiOptions, MidiSlot, RangeMode};
use nbs_io::MidiGmTable;

fn import(note: u8) -> Song {
    let track = [
        0, 255, 81, 3, 7, 161, 32, 0, 192, 0, 0, 144, note, 100, 131, 96, 128, note, 0, 0, 255, 47,
        0,
    ];
    let mut bytes = b"MThd".to_vec();
    bytes.extend_from_slice(&[0, 0, 0, 6, 0, 0, 0, 1, 1, 224]);
    bytes.extend_from_slice(b"MTrk");
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&track);
    let midi = parse_midi(&bytes).unwrap();
    let options = MidiImportOptions::for_midi(&midi, &MidiGmTable::default());
    let mut enhanced = EnhancedMidiOptions::for_midi(&midi);
    enhanced.enabled = true;
    enhanced.range_mode = RangeMode::Keep;
    enhanced.channels[0].transpose = 0;
    enhanced.channels[0].slots = [
        MidiSlot {
            instrument: Some(0),
            volume: 100,
        },
        MidiSlot {
            instrument: Some(6),
            volume: 50,
        },
        MidiSlot {
            instrument: Some(5),
            volume: 25,
        },
    ];
    import_midi_enhanced(&midi, &options, &enhanced).unwrap().0
}

fn sound_lines(song: &Song, options: &DatapackOptions) -> Vec<String> {
    generate(song, options, "enhanced_test")
        .files
        .into_iter()
        .filter(|file| file.path.contains("/notes/"))
        .flat_map(|file| {
            file.content
                .lines()
                .filter(|line| line.contains("playsound "))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn volumes(lines: &[String]) -> Vec<f64> {
    lines
        .iter()
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let index = words.iter().position(|word| *word == "playsound").unwrap();
            words[index + 7].parse().unwrap()
        })
        .collect()
}

#[test]
fn non_radius_datapack_preserves_slot_and_layer_volumes() {
    let song = import(60);
    let options = DatapackOptions::default();
    let full = volumes(&sound_lines(&song, &options));
    assert_eq!(full.len(), 3);
    for (actual, expected) in full.iter().zip([0.78, 0.39, 0.20]) {
        assert!((actual - expected).abs() < 1e-6);
    }
    let mut quiet = song.clone();
    for layer in &mut quiet.layers {
        layer.volume = 50;
    }
    let half = volumes(&sound_lines(&quiet, &options));
    for (actual, full) in half.iter().zip(&full) {
        assert!((actual - full / 2.0).abs() < 1e-6);
    }
}

#[test]
fn radius_datapack_ignores_note_volumes() {
    let song = import(60);
    let options = DatapackOptions {
        enable_radius: true,
        ..DatapackOptions::default()
    };
    let expected = sound_lines(&song, &options);
    assert_eq!(volumes(&expected), [1.0, 1.0, 1.0]);
    let mut quiet = song.clone();
    for column in quiet.columns.values_mut() {
        for note in column.notes.values_mut() {
            note.velocity = 1;
        }
    }
    assert_eq!(sound_lines(&quiet, &options), expected);
}

#[test]
fn stored_nbs_preserves_combination_and_volume() {
    let song = import(60);
    let bytes = nbs_core::nbs::save_bytes(&song, nbs_core::nbs::NBS_VERSION).unwrap();
    let loaded = nbs_core::nbs::load_bytes(&bytes).unwrap();
    assert_eq!(
        song.iter_notes().collect::<Vec<_>>(),
        loaded.iter_notes().collect::<Vec<_>>()
    );
    assert_eq!(
        sound_lines(&song, &DatapackOptions::default()),
        sound_lines(&loaded, &DatapackOptions::default())
    );
}

#[test]
fn out_of_range_requires_explicit_extended_sound_export() {
    let song = import(90);
    assert_eq!(song.total_blocks, 3);
    assert!(song.iter_notes().all(|(_, _, note)| note.key == 69));
    assert!(sound_lines(&song, &DatapackOptions::default()).is_empty());
    let options = DatapackOptions {
        include_out_of_range: true,
        ..DatapackOptions::default()
    };
    let lines = sound_lines(&song, &options);
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|line| line.contains("_1 ")));
}
