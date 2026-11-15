//! General MIDI 乐器表与鼓组表，逐条抄自原版
//! `scripts/midi_instruments/midi_instruments.gml`（tag `v3.12.0-rc.1`，即装机版
//! Note Block Studio 3.12.0-rc.1，2026-11-11）。
//!
//! 注意别用仓库 `main` 那份：main（2026-07-21）比装机版旧，铜块一族（乐器 16..19）
//! 还没接进 GM 映射，鼓组表也还是 24..87 的旧对应，用它导出来的歌曲和装机版对不上。
//!
//! 列含义与原版一致：`name` = 列 0，`ins` = 列 1（`instrument_list` 下标，-1 表示不出声），
//! `octave` = 列 2（八度偏移），`short` = 列 3（层名简称，空串表示用全名）。
//! 鼓组：`ins` = 列 1，`pitch` = 列 2（写进歌曲时原版会 `+ 33`）；下标即 MIDI 音号，
//! 覆盖 0..=127，没有对应音效的行原版就是 `"----"` + `ins = -1`，导入时视作静音。
//!
//! 已核对：本表 128 行 patch + 128 行 drum 与装机版生效值（
//! `%APPDATA%\Minecraft_Note_Block_Studio\settings_dev.ini` 的 `[midi_import]`）逐条一致。

pub struct MidiPatch {
    pub name: &'static str,
    pub ins: i8,
    pub octave: i8,
    pub short: &'static str,
}

pub struct MidiDrum {
    pub name: &'static str,
    pub ins: i8,
    pub pitch: i8,
}

pub static MIDI_PATCHES: [MidiPatch; 128] = [
    MidiPatch { name: "Acoustic Grand Piano", ins: 0, octave: 0, short: "Piano 1" },
    MidiPatch { name: "Bright Acoustic Piano", ins: 15, octave: 0, short: "Piano 2" },
    MidiPatch { name: "Electric Grand Piano", ins: 15, octave: 0, short: "Piano 3" },
    MidiPatch { name: "Honky-tonk Piano", ins: 15, octave: 0, short: "Honky-tonk" },
    MidiPatch { name: "Electric Piano 1", ins: 0, octave: 0, short: "E.Piano 1" },
    MidiPatch { name: "Electric Piano 2", ins: 0, octave: 0, short: "E.Piano 2" },
    MidiPatch { name: "Harpsichord", ins: 5, octave: 1, short: "" },
    MidiPatch { name: "Clavinet", ins: 14, octave: 0, short: "" },
    MidiPatch { name: "Celesta", ins: 7, octave: -2, short: "" },
    MidiPatch { name: "Glockenspiel", ins: 7, octave: -2, short: "" },
    MidiPatch { name: "Music Box", ins: 7, octave: -2, short: "" },
    MidiPatch { name: "Vibraphone", ins: 10, octave: 0, short: "" },
    MidiPatch { name: "Marimba", ins: 10, octave: 0, short: "" },
    MidiPatch { name: "Xylophone", ins: 9, octave: -2, short: "" },
    MidiPatch { name: "Tubular Bells", ins: 7, octave: -2, short: "TubularBells" },
    MidiPatch { name: "Dulcimer", ins: 5, octave: 1, short: "" },
    MidiPatch { name: "Drawbar Organ", ins: 6, octave: -1, short: "Organ 1" },
    MidiPatch { name: "Percussive Organ", ins: 10, octave: 0, short: "Organ 2" },
    MidiPatch { name: "Rock Organ", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Church Organ", ins: 6, octave: -1, short: "ChurchOrgan" },
    MidiPatch { name: "Reed Organ", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Accordion", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Harmonica", ins: 16, octave: 0, short: "" },
    MidiPatch { name: "Bandoneon", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Acoustic Guitar (nylon)", ins: 5, octave: 1, short: "Nylon-str.Gt" },
    MidiPatch { name: "Acoustic Guitar (steel)", ins: 5, octave: 1, short: "Steel-str.Gt" },
    MidiPatch { name: "Electric Guitar (jazz)", ins: 0, octave: 0, short: "Jazz Guitar" },
    MidiPatch { name: "Electric Guitar (clean)", ins: 5, octave: 1, short: "Clean Guitar" },
    MidiPatch { name: "Electric Guitar (muted)", ins: 1, octave: 2, short: "Muted Guitar" },
    MidiPatch { name: "Overdriven Guitar", ins: 12, octave: 2, short: "Overdrive Gt" },
    MidiPatch { name: "Distortion Guitar", ins: 5, octave: 1, short: "DistortionGt" },
    MidiPatch { name: "Guitar Harmonics", ins: 5, octave: 3, short: "Gt.Harmonics" },
    MidiPatch { name: "Acoustic Bass", ins: 1, octave: 2, short: "AcousticBass" },
    MidiPatch { name: "Electric Bass (finger)", ins: 1, octave: 2, short: "FingeredBass" },
    MidiPatch { name: "Electric Bass (pick)", ins: 1, octave: 2, short: "Picked Bass" },
    MidiPatch { name: "Fretless Bass", ins: 1, octave: 2, short: "FretlessBass" },
    MidiPatch { name: "Slap Bass 1", ins: 5, octave: 1, short: "" },
    MidiPatch { name: "Slap Bass 2", ins: 5, octave: 1, short: "" },
    MidiPatch { name: "Synth Bass 1", ins: 1, octave: 2, short: "" },
    MidiPatch { name: "Synth Bass 2", ins: 15, octave: 0, short: "" },
    MidiPatch { name: "Violin", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Viola", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Cello", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Contrabass", ins: 12, octave: 2, short: "" },
    MidiPatch { name: "Tremolo Strings", ins: 6, octave: -1, short: "Tremolo Str." },
    MidiPatch { name: "Pizzicato Strings", ins: 1, octave: 2, short: "Pizzicato" },
    MidiPatch { name: "Orchestral Harp", ins: 0, octave: 0, short: "Harp" },
    MidiPatch { name: "Timpani", ins: 3, octave: 0, short: "" },
    MidiPatch { name: "String Ensemble 1", ins: 6, octave: -1, short: "Strings 1" },
    MidiPatch { name: "String Ensemble 2", ins: 6, octave: -1, short: "Strings 2" },
    MidiPatch { name: "Synth Strings 1", ins: 6, octave: -1, short: "Syn.Strings1" },
    MidiPatch { name: "Synth Strings 2", ins: 6, octave: -1, short: "Syn.Strings2" },
    MidiPatch { name: "Choir Aahs", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Voice Oohs", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Synth Voice", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Orchestra Hit", ins: 3, octave: 0, short: "OrchestraHit" },
    MidiPatch { name: "Trumpet", ins: 16, octave: 0, short: "" },
    MidiPatch { name: "Trombone", ins: 17, octave: 0, short: "" },
    MidiPatch { name: "Tuba", ins: 19, octave: 2, short: "" },
    MidiPatch { name: "Muted Trumpet", ins: 18, octave: 1, short: "Muted Tp." },
    MidiPatch { name: "French Horn", ins: 17, octave: 0, short: "" },
    MidiPatch { name: "Brass Section", ins: 17, octave: 0, short: "Brass" },
    MidiPatch { name: "Synth Brass 1", ins: 17, octave: 0, short: "Synth Brass1" },
    MidiPatch { name: "Synth Brass 2", ins: 16, octave: 0, short: "Synth Brass2" },
    MidiPatch { name: "Soprano Sax", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Alto Sax", ins: 16, octave: 0, short: "" },
    MidiPatch { name: "Tenor Sax", ins: 16, octave: 0, short: "" },
    MidiPatch { name: "Baritone Sax", ins: 17, octave: 0, short: "" },
    MidiPatch { name: "Oboe", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "English Horn", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Bassoon", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Clarinet", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Piccolo", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Flute", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Recorder", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Pan Flute", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Blown Bottle", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Shakuhachi", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Whistle", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Ocarina", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Lead 1 (square)", ins: 13, octave: 0, short: "Square Lead" },
    MidiPatch { name: "Lead 2 (sawtooth)", ins: 6, octave: -1, short: "Saw Lead" },
    MidiPatch { name: "Lead 3 (calliope)", ins: 6, octave: -1, short: "Calliope" },
    MidiPatch { name: "Lead 4 (chiff)", ins: 6, octave: -1, short: "Chiff Lead" },
    MidiPatch { name: "Lead 5 (charang)", ins: 5, octave: 1, short: "Charang" },
    MidiPatch { name: "Lead 6 (voice)", ins: 6, octave: -1, short: "Voice Lead" },
    MidiPatch { name: "Lead 7 (fifths)", ins: 6, octave: -1, short: "Fifth Lead" },
    MidiPatch { name: "Lead 8 (bass + lead)", ins: 1, octave: 2, short: "Bass+Lead" },
    MidiPatch { name: "Pad 1 (new age)", ins: 7, octave: -2, short: "Fantasia" },
    MidiPatch { name: "Pad 2 (warm)", ins: 6, octave: -1, short: "Warm Pad" },
    MidiPatch { name: "Pad 3 (polysynth)", ins: 6, octave: -1, short: "Polysynth" },
    MidiPatch { name: "Pad 4 (choir)", ins: 6, octave: -1, short: "Space Choir" },
    MidiPatch { name: "Pad 5 (bowed)", ins: 6, octave: -1, short: "Bowed Glass" },
    MidiPatch { name: "Pad 6 (metallic)", ins: 6, octave: -1, short: "Metal Pad" },
    MidiPatch { name: "Pad 7 (halo)", ins: 6, octave: -1, short: "Halo Pad" },
    MidiPatch { name: "Pad 8 (sweep)", ins: 8, octave: -2, short: "Sweep Pad" },
    MidiPatch { name: "FX 1 (rain)", ins: 8, octave: -2, short: "Rain Drop" },
    MidiPatch { name: "FX 2 (soundtrack)", ins: 6, octave: -1, short: "Soundtrack" },
    MidiPatch { name: "FX 3 (crystal)", ins: 8, octave: -2, short: "Crystal" },
    MidiPatch { name: "FX 4 (atmosphere)", ins: 5, octave: 1, short: "Atmosphere" },
    MidiPatch { name: "FX 5 (brightness)", ins: 15, octave: 0, short: "Brightness" },
    MidiPatch { name: "FX 6 (goblins)", ins: 6, octave: -1, short: "Goblins" },
    MidiPatch { name: "FX 7 (echoes)", ins: 6, octave: -1, short: "Echoes" },
    MidiPatch { name: "FX 8 (sci-fi)", ins: 5, octave: 1, short: "SF" },
    MidiPatch { name: "Sitar", ins: 14, octave: 0, short: "" },
    MidiPatch { name: "Banjo", ins: 14, octave: 0, short: "" },
    MidiPatch { name: "Shamisen", ins: 14, octave: 0, short: "" },
    MidiPatch { name: "Koto", ins: 5, octave: 1, short: "" },
    MidiPatch { name: "Kalimba", ins: 10, octave: 0, short: "" },
    MidiPatch { name: "Bag pipe", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Fiddle", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Shanai", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Tinkle Bell", ins: 8, octave: -2, short: "" },
    MidiPatch { name: "Agogo", ins: 11, octave: -1, short: "" },
    MidiPatch { name: "Steel Drums", ins: 10, octave: 0, short: "" },
    MidiPatch { name: "Woodblock", ins: 9, octave: -2, short: "" },
    MidiPatch { name: "Taiko Drum", ins: 2, octave: 0, short: "" },
    MidiPatch { name: "Melodic Tom", ins: 2, octave: 0, short: "" },
    MidiPatch { name: "Synth Drum", ins: 2, octave: 0, short: "" },
    MidiPatch { name: "Reverse Cymbal", ins: 3, octave: 0, short: "Reverse Cym." },
    MidiPatch { name: "Guitar Fret Noise", ins: 4, octave: 1, short: "Gt FretNoise" },
    MidiPatch { name: "Breath Noise", ins: 6, octave: -1, short: "" },
    MidiPatch { name: "Seashore", ins: 8, octave: -2, short: "" },
    MidiPatch { name: "Bird Tweet", ins: 6, octave: 1, short: "Bird" },
    MidiPatch { name: "Telephone Ring", ins: 7, octave: 2, short: "Telephone" },
    MidiPatch { name: "Helicopter", ins: 2, octave: 0, short: "" },
    MidiPatch { name: "Applause", ins: 3, octave: 0, short: "" },
    MidiPatch { name: "Gunshot", ins: 3, octave: 0, short: "" },
];

/// 原版 `midi_drum`，下标即 MIDI 音号，128 行覆盖 0..=127。
pub static MIDI_DRUMS: [MidiDrum; 128] = [
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "[GS] Concert SD", ins: 3, pitch: 12 },
    MidiDrum { name: "[GS] Snare Roll", ins: 3, pitch: 5 },
    MidiDrum { name: "[GS] Finger Snap", ins: 4, pitch: 18 },
    MidiDrum { name: "High Q", ins: 3, pitch: 2 },
    MidiDrum { name: "Slap", ins: 3, pitch: 9 },
    MidiDrum { name: "Scratch Push", ins: 4, pitch: 6 },
    MidiDrum { name: "Scratch Pull", ins: 4, pitch: 2 },
    MidiDrum { name: "Sticks", ins: 4, pitch: 13 },
    MidiDrum { name: "Square Click", ins: 4, pitch: 9 },
    MidiDrum { name: "Metronome Click", ins: 4, pitch: 15 },
    MidiDrum { name: "Metronome Bell", ins: 8, pitch: 18 },
    MidiDrum { name: "Acoustic Bass Drum", ins: 2, pitch: 4 },
    MidiDrum { name: "Electric Bass Drum", ins: 2, pitch: 8 },
    MidiDrum { name: "Side Stick", ins: 4, pitch: 8 },
    MidiDrum { name: "Acoustic Snare", ins: 3, pitch: 15 },
    MidiDrum { name: "Hand Clap", ins: 3, pitch: 19 },
    MidiDrum { name: "Electric Snare", ins: 3, pitch: 16 },
    MidiDrum { name: "Low Floor Tom", ins: 2, pitch: 6 },
    MidiDrum { name: "Closed Hi-hat", ins: 4, pitch: 21 },
    MidiDrum { name: "High Floor Tom", ins: 2, pitch: 9 },
    MidiDrum { name: "Pedal Hi-hat", ins: 4, pitch: 23 },
    MidiDrum { name: "Low Tom", ins: 2, pitch: 14 },
    MidiDrum { name: "Open Hi-hat", ins: 3, pitch: 22 },
    MidiDrum { name: "Low-Mid Tom", ins: 2, pitch: 17 },
    MidiDrum { name: "High-Mid Tom", ins: 2, pitch: 20 },
    MidiDrum { name: "Crash Cymbal 1", ins: 3, pitch: 20 },
    MidiDrum { name: "High Tom", ins: 2, pitch: 23 },
    MidiDrum { name: "Ride Cymbal 1", ins: 3, pitch: 17 },
    MidiDrum { name: "Chinese Cymbal", ins: 3, pitch: 14 },
    MidiDrum { name: "Ride Bell", ins: 7, pitch: 17 },
    MidiDrum { name: "Tambourine", ins: 3, pitch: 23 },
    MidiDrum { name: "Splash Cymbal", ins: 3, pitch: 18 },
    MidiDrum { name: "Cowbell", ins: 11, pitch: 6 },
    MidiDrum { name: "Crash Cymbal 2", ins: 3, pitch: 21 },
    MidiDrum { name: "Vibraslap", ins: 4, pitch: 17 },
    MidiDrum { name: "Ride Cymbal 2", ins: 3, pitch: 24 },
    MidiDrum { name: "High Bongo", ins: 11, pitch: 16 },
    MidiDrum { name: "Low Bongo", ins: 11, pitch: 9 },
    MidiDrum { name: "Mute Hi Conga", ins: 4, pitch: -3 },
    MidiDrum { name: "Open Hi Conga", ins: 11, pitch: -1 },
    MidiDrum { name: "Low Conga", ins: 11, pitch: -9 },
    MidiDrum { name: "High Timbale", ins: 11, pitch: 5 },
    MidiDrum { name: "Low Timbale", ins: 11, pitch: -4 },
    MidiDrum { name: "High Agogo", ins: 9, pitch: 12 },
    MidiDrum { name: "Low Agogo", ins: 9, pitch: 5 },
    MidiDrum { name: "Cabasa", ins: 4, pitch: 35 },
    MidiDrum { name: "Maracas", ins: 4, pitch: 32 },
    MidiDrum { name: "Short Whistle", ins: 6, pitch: 34 },
    MidiDrum { name: "Long Whistle", ins: 6, pitch: 33 },
    MidiDrum { name: "Short Guiro", ins: 4, pitch: 19 },
    MidiDrum { name: "Long Guiro", ins: 4, pitch: 20 },
    MidiDrum { name: "Claves", ins: 9, pitch: 19 },
    MidiDrum { name: "Hi Wood Block", ins: 9, pitch: 7 },
    MidiDrum { name: "Low Wood Block", ins: 9, pitch: 1 },
    MidiDrum { name: "Mute Cuica", ins: 12, pitch: 22 },
    MidiDrum { name: "Open Cuica", ins: 12, pitch: 13 },
    MidiDrum { name: "Mute Triangle", ins: 7, pitch: 19 },
    MidiDrum { name: "Open Triangle", ins: 8, pitch: 19 },
    MidiDrum { name: "Shaker", ins: 4, pitch: 36 },
    MidiDrum { name: "Jingle Bell", ins: 7, pitch: 21 },
    MidiDrum { name: "Bell Tree", ins: 8, pitch: 17 },
    MidiDrum { name: "Castanets", ins: 9, pitch: 15 },
    MidiDrum { name: "Mute Surdo", ins: 2, pitch: 12 },
    MidiDrum { name: "Open Surdo", ins: 2, pitch: 7 },
    MidiDrum { name: "[GS] Applause", ins: 3, pitch: 10 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
    MidiDrum { name: "----", ins: -1, pitch: 0 },
];

/// 原版 `midi_fadein` / `midi_fadeout`：按 patch 名判断延长音该做渐入还是渐出。
pub static MIDI_FADE_IN: [&'static str; 1] = ["Reverse Cymbal"];
pub static MIDI_FADE_OUT: [&'static str; 3] = ["Gunshot", "Applause", "Seashore"];


/// 对应 `midi_is_note_fade(name, fadetype)`，`fadetype` 0 = 渐入、1 = 渐出。
pub fn is_note_fade(name: &str, fade_out: bool) -> bool {
    let list = if fade_out { &MIDI_FADE_OUT[..] } else { &MIDI_FADE_IN[..] };
    list.iter().any(|n| *n == name)
}

/// 原版 `midi_ins[*, 1..2]` / `midi_drum[*, 1..2]` 的可变副本。出厂值取自上面的静态表，
/// 用户在导入窗口里的改动只有在「记住我的更改」勾上时才写回这里（原版
/// `import_midi.gml` 尾部的 `w_midi_remember` 分支），并由 `save_settings` 持久化。
///
/// 鼓组两个数组下标即 MIDI 音号（0..=127），与原版 `midi_drum` 行数一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiGmTable {
    pub patch_ins: [i8; 128],
    pub patch_oct: [i8; 128],
    pub drum_ins: [i8; 128],
    pub drum_key: [i8; 128],
}

impl Default for MidiGmTable {
    fn default() -> Self {
        let mut t = Self {
            patch_ins: [-1; 128],
            patch_oct: [0; 128],
            drum_ins: [-1; 128],
            drum_key: [0; 128],
        };
        for (i, p) in MIDI_PATCHES.iter().enumerate() {
            t.patch_ins[i] = p.ins;
            t.patch_oct[i] = p.octave;
        }
        for (i, d) in MIDI_DRUMS.iter().enumerate() {
            t.drum_ins[i] = d.ins;
            t.drum_key[i] = d.pitch;
        }
        t
    }
}

impl MidiGmTable {
    pub fn patch(&self, p: u8) -> (i8, i8) {
        (self.patch_ins[p as usize], self.patch_oct[p as usize])
    }

    pub fn set_patch(&mut self, p: u8, ins: i8, octave: i8) {
        self.patch_ins[p as usize] = ins;
        self.patch_oct[p as usize] = octave;
    }

    /// 鼓音的列 1 / 列 2，下标即 MIDI 音号；`ins = -1` 的行原版写作 `"----"`，导入时静音。
    pub fn drum(&self, note: u8) -> (i8, i8) {
        (self.drum_ins[note as usize], self.drum_key[note as usize])
    }

    pub fn set_drum(&mut self, note: u8, ins: i8, key: i8) {
        self.drum_ins[note as usize] = ins;
        self.drum_key[note as usize] = key;
    }

    pub fn patch_name(p: u8) -> &'static str {
        MIDI_PATCHES.get(p as usize).map(|x| x.name).unwrap_or("")
    }

    pub fn patch_short(p: u8) -> &'static str {
        MIDI_PATCHES.get(p as usize).map(|x| x.short).unwrap_or("")
    }

    pub fn drum_name(note: u8) -> &'static str {
        MIDI_DRUMS.get(note as usize).map(|d| d.name).unwrap_or("")
    }

    /// 存成偏好里的 `[midi_import]` 等价物（原版 `save_settings` 写的 512 个数字）。
    pub fn to_persist(&self) -> MidiGmPersist {
        MidiGmPersist {
            patch_ins: self.patch_ins.to_vec(),
            patch_oct: self.patch_oct.to_vec(),
            drum_ins: self.drum_ins.to_vec(),
            drum_key: self.drum_key.to_vec(),
        }
    }

    /// 读回。向量比 128 短（老偏好文件、手改过的 JSON）时，缺的那些保持出厂值。
    pub fn from_persist(p: &MidiGmPersist) -> Self {
        let mut t = Self::default();
        for i in 0..128 {
            if let Some(v) = p.patch_ins.get(i) {
                t.patch_ins[i] = *v;
            }
            if let Some(v) = p.patch_oct.get(i) {
                t.patch_oct[i] = *v;
            }
            if let Some(v) = p.drum_ins.get(i) {
                t.drum_ins[i] = *v;
            }
            if let Some(v) = p.drum_key.get(i) {
                t.drum_key[i] = *v;
            }
        }
        t
    }
}

/// 原版 `settings.ini` 的 `[midi_import]` 段：`ins_<patch>` / `ins_<patch>_oct` /
/// `drum_<音号>` / `drum_<音号>_key`。装机版把 128 + 128 两组全写出来，这里同样用
/// 四个 `Vec` 存全部下标；偏好里没有这一段（`None`）就代表从没记住过，用出厂表。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MidiGmPersist {
    pub patch_ins: Vec<i8>,
    pub patch_oct: Vec<i8>,
    pub drum_ins: Vec<i8>,
    pub drum_key: Vec<i8>,
}
