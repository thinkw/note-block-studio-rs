use std::fs::File;
use std::io::{self, Cursor, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::{Condvar, Mutex};
use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};

const MAX_MIDI_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOUNDFONT_BYTES: usize = 256 * 1024 * 1024;
const MAX_TRACKS: usize = 1024;
const MAX_EVENTS: usize = 2_000_000;
const MAX_NOTES: usize = 1_000_000;
const RENDER_BLOCK: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiPlaybackState {
    Idle,
    Loading,
    Playing,
    Paused,
    Finished,
    Error,
}

#[derive(Clone, Debug)]
pub struct MidiPlaybackSnapshot {
    pub state: MidiPlaybackState,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub error: Option<String>,
}

impl Default for MidiPlaybackSnapshot {
    fn default() -> Self {
        Self {
            state: MidiPlaybackState::Idle,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            error: None,
        }
    }
}

pub struct MidiPreviewPlayer {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    prepare: Arc<Prepare>,
}

impl Default for MidiPreviewPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiPreviewPlayer {
    pub fn new() -> Self {
        Self::with_prepare(Arc::new(prepare_playback))
    }

    fn with_prepare(prepare: Arc<Prepare>) -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            worker: None,
            prepare,
        }
    }

    pub fn play(
        &mut self,
        midi_bytes: Vec<u8>,
        soundfont: PathBuf,
        output_device: Option<String>,
    ) -> Result<(), String> {
        let generation = {
            let mut control = self.shared.control.lock();
            control.generation = control.generation.wrapping_add(1);
            control.pending = None;
            control.paused = false;
            control.snapshot = MidiPlaybackSnapshot {
                state: MidiPlaybackState::Loading,
                ..MidiPlaybackSnapshot::default()
            };
            control.generation
        };
        self.shared.changed.notify_all();
        if midi_bytes.is_empty() || midi_bytes.len() > MAX_MIDI_BYTES {
            let error = format!("MIDI playback requires 1..={MAX_MIDI_BYTES} bytes");
            self.shared.fail(generation, error.clone());
            return Err(error);
        }
        if soundfont.as_os_str().is_empty() {
            let error = "Select a SoundFont (.sf2) before MIDI playback".to_string();
            self.shared.fail(generation, error.clone());
            return Err(error);
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
        if self.worker.is_none() {
            let shared = self.shared.clone();
            let prepare = self.prepare.clone();
            match thread::Builder::new()
                .name("midi-preview".into())
                .spawn(move || {
                    if catch_unwind(AssertUnwindSafe(|| run_worker(&shared, &prepare))).is_err() {
                        let generation = shared.control.lock().generation;
                        shared.fail(generation, "MIDI playback worker panicked".into());
                    }
                }) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    let error = format!("Cannot start MIDI playback worker: {error}");
                    self.shared.fail(generation, error.clone());
                    return Err(error);
                }
            }
        }
        let mut control = self.shared.control.lock();
        control.pending = Some(PlayRequest {
            generation,
            midi_bytes,
            soundfont,
            output_device,
        });
        self.shared.changed.notify_all();
        Ok(())
    }

    pub fn pause(&self) {
        let mut control = self.shared.control.lock();
        match control.snapshot.state {
            MidiPlaybackState::Loading => control.paused = true,
            MidiPlaybackState::Playing => {
                control.paused = true;
                control.snapshot.state = MidiPlaybackState::Paused;
            }
            _ => {}
        }
    }

    pub fn resume(&self) {
        let mut control = self.shared.control.lock();
        match control.snapshot.state {
            MidiPlaybackState::Loading => control.paused = false,
            MidiPlaybackState::Paused => {
                control.paused = false;
                control.snapshot.state = MidiPlaybackState::Playing;
            }
            _ => {}
        }
    }

    pub fn stop(&self) {
        let mut control = self.shared.control.lock();
        control.generation = control.generation.wrapping_add(1);
        control.pending = None;
        control.paused = false;
        control.snapshot = MidiPlaybackSnapshot::default();
        self.shared.changed.notify_all();
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.shared
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MidiPlaybackSnapshot {
        self.shared.control.lock().snapshot.clone()
    }
}

impl Drop for MidiPreviewPlayer {
    fn drop(&mut self) {
        {
            let mut control = self.shared.control.lock();
            control.shutdown = true;
            control.generation = control.generation.wrapping_add(1);
            control.pending = None;
            self.shared.changed.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct PlayRequest {
    generation: u64,
    midi_bytes: Vec<u8>,
    soundfont: PathBuf,
    output_device: Option<String>,
}

#[derive(Default)]
struct Control {
    generation: u64,
    pending: Option<PlayRequest>,
    paused: bool,
    shutdown: bool,
    snapshot: MidiPlaybackSnapshot,
}

struct Shared {
    control: Mutex<Control>,
    changed: Condvar,
    volume: AtomicU32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            control: Mutex::new(Control::default()),
            changed: Condvar::new(),
            volume: AtomicU32::new(1.0f32.to_bits()),
        }
    }
}

impl Shared {
    fn current(&self, generation: u64) -> bool {
        let control = self.control.lock();
        !control.shutdown && control.generation == generation
    }

    fn check_current(&self, generation: u64) -> Result<(), String> {
        if self.current(generation) {
            Ok(())
        } else {
            Err("MIDI playback cancelled".into())
        }
    }

    fn fail(&self, generation: u64, error: String) {
        let mut control = self.control.lock();
        if !control.shutdown && control.generation == generation {
            control.snapshot.state = MidiPlaybackState::Error;
            control.snapshot.error = Some(error);
            self.changed.notify_all();
        }
    }

    fn ready(&self, generation: u64, duration_seconds: f64) -> bool {
        let mut control = self.control.lock();
        if control.shutdown
            || control.generation != generation
            || control.snapshot.state != MidiPlaybackState::Loading
        {
            return false;
        }
        control.snapshot.duration_seconds = duration_seconds;
        control.snapshot.state = if control.paused {
            MidiPlaybackState::Paused
        } else {
            MidiPlaybackState::Playing
        };
        self.changed.notify_all();
        true
    }
}

trait OutputStream {
    fn start(&self) -> Result<(), String>;
}

impl OutputStream for cpal::Stream {
    fn start(&self) -> Result<(), String> {
        self.play()
            .map_err(|e| format!("Cannot start MIDI output stream: {e}"))
    }
}

struct Prepared {
    stream: Box<dyn OutputStream>,
    duration_seconds: f64,
}

type Prepare = dyn Fn(&PlayRequest, &Arc<Shared>) -> Result<Prepared, String> + Send + Sync;

fn run_worker(shared: &Arc<Shared>, prepare: &Arc<Prepare>) {
    let mut active: Option<(u64, Prepared)> = None;
    loop {
        let request = {
            let mut control = shared.control.lock();
            loop {
                if active.as_ref().is_some_and(|(generation, _)| {
                    *generation != control.generation
                        || matches!(
                            control.snapshot.state,
                            MidiPlaybackState::Idle
                                | MidiPlaybackState::Error
                                | MidiPlaybackState::Finished
                        )
                }) {
                    drop(control);
                    active = None;
                    control = shared.control.lock();
                }
                if control.shutdown {
                    return;
                }
                if let Some(request) = control.pending.take() {
                    break request;
                }
                shared.changed.wait(&mut control);
            }
        };
        active = None;
        if !shared.current(request.generation) {
            continue;
        }
        let result =
            catch_unwind(AssertUnwindSafe(|| prepare(&request, shared))).unwrap_or_else(|_| {
                Err("MIDI/SoundFont loading panicked; playback was not started".into())
            });
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                shared.fail(request.generation, error);
                continue;
            }
        };
        if !shared.current(request.generation) {
            continue;
        }
        if let Err(error) = prepared.stream.start() {
            shared.fail(request.generation, error);
            continue;
        }
        if shared.ready(request.generation, prepared.duration_seconds) {
            active = Some((request.generation, prepared));
        }
    }
}

fn prepare_playback(request: &PlayRequest, shared: &Arc<Shared>) -> Result<Prepared, String> {
    validate_midi(&request.midi_bytes)?;
    shared.check_current(request.generation)?;
    let midi = MidiFile::new(&mut CancelReader::new(
        Cursor::new(&request.midi_bytes),
        shared,
        request.generation,
    ))
    .map_err(|e| format!("Cannot load MIDI: {e}"))?;
    let duration_seconds = midi.get_length();
    if !duration_seconds.is_finite() || duration_seconds < 0.0 {
        return Err("MIDI has an invalid playback duration".into());
    }
    let bytes = read_soundfont(&request.soundfont, shared, request.generation)?;
    validate_soundfont(&bytes)?;
    let soundfont = SoundFont::new(&mut CancelReader::new(
        Cursor::new(&bytes),
        shared,
        request.generation,
    ))
    .map_err(|e| format!("Cannot load SoundFont {}: {e}", request.soundfont.display()))?;
    if soundfont.get_presets().is_empty() || soundfont.get_wave_data().is_empty() {
        return Err("SoundFont contains no playable presets or samples".into());
    }
    drop(bytes);
    shared.check_current(request.generation)?;
    let device = open_output_device(request.output_device.as_deref())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Cannot read MIDI output configuration: {e}"))?;
    let config = supported.config();
    if config.channels == 0
        || config.channels > 32
        || !(16_000..=192_000).contains(&config.sample_rate)
    {
        return Err(format!(
            "Unsupported MIDI output configuration: {} channels at {} Hz",
            config.channels, config.sample_rate
        ));
    }
    let mut settings = SynthesizerSettings::new(config.sample_rate as i32);
    settings.maximum_polyphony = 256;
    let synthesizer = Synthesizer::new(&Arc::new(soundfont), &settings)
        .map_err(|e| format!("Cannot initialise GM synthesizer: {e}"))?;
    let mut sequencer = MidiFileSequencer::new(synthesizer);
    sequencer.play(&Arc::new(midi), false);
    let renderer = Renderer::new(sequencer, config.sample_rate, duration_seconds);
    shared.check_current(request.generation)?;
    let stream = match supported.sample_format() {
        cpal::SampleFormat::I8 => {
            build_stream::<i8>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::I24 => {
            build_stream::<cpal::I24>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::I32 => {
            build_stream::<i32>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::I64 => {
            build_stream::<i64>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::U8 => {
            build_stream::<u8>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::U24 => {
            build_stream::<cpal::U24>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::U32 => {
            build_stream::<u32>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::U64 => {
            build_stream::<u64>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, config, renderer, shared, request.generation)
        }
        cpal::SampleFormat::F64 => {
            build_stream::<f64>(&device, config, renderer, shared, request.generation)
        }
        format => Err(format!("Unsupported MIDI output sample format: {format:?}")),
    }?;
    Ok(Prepared {
        stream: Box::new(stream),
        duration_seconds,
    })
}

fn open_output_device(wanted: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let Some(wanted) = wanted.filter(|name| !name.is_empty()) else {
        return host
            .default_output_device()
            .ok_or_else(|| "No default MIDI output device is available".into());
    };
    let matches = |device: &cpal::Device| {
        device.id().is_ok_and(|id| id.to_string() == wanted)
            || device
                .description()
                .is_ok_and(|description| description.name() == wanted)
    };
    if let Some(device) = host.default_output_device().filter(&matches) {
        return Ok(device);
    }
    if let Ok(id) = wanted.parse::<cpal::DeviceId>() {
        if let Some(device) = host.device_by_id(&id) {
            return Ok(device);
        }
    }
    host.output_devices()
        .map_err(|e| format!("Cannot enumerate MIDI output devices: {e}"))?
        .find(matches)
        .ok_or_else(|| format!("Selected MIDI output device is unavailable: {wanted}"))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut renderer: Renderer<MidiFileSequencer>,
    shared: &Arc<Shared>,
    generation: u64,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let callback_shared = shared.clone();
    let error_shared = shared.clone();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data, _| {
                if catch_unwind(AssertUnwindSafe(|| {
                    renderer.render(data, channels, &callback_shared, generation)
                }))
                .is_err()
                {
                    data.fill(T::EQUILIBRIUM);
                    callback_shared.fail(generation, "GM audio rendering failed".into());
                }
            },
            move |error| {
                error_shared.fail(generation, format!("MIDI output device error: {error}"))
            },
            None,
        )
        .map_err(|e| format!("Cannot build MIDI output stream: {e}"))
}

trait Sequence {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]);
    fn stop(&mut self);
}

impl Sequence for MidiFileSequencer {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        MidiFileSequencer::render(self, left, right);
    }

    fn stop(&mut self) {
        MidiFileSequencer::stop(self);
    }
}

struct Renderer<S> {
    sequence: S,
    sample_rate: u32,
    total_frames: u64,
    rendered_frames: u64,
    stopped: bool,
}

impl<S: Sequence> Renderer<S> {
    fn new(sequence: S, sample_rate: u32, duration_seconds: f64) -> Self {
        Self {
            sequence,
            sample_rate,
            total_frames: (duration_seconds * sample_rate as f64).ceil() as u64,
            rendered_frames: 0,
            stopped: false,
        }
    }

    fn stop(&mut self) {
        if !self.stopped {
            self.sequence.stop();
            self.stopped = true;
        }
    }

    fn render<T: cpal::Sample + cpal::FromSample<f32>>(
        &mut self,
        data: &mut [T],
        channels: usize,
        shared: &Shared,
        generation: u64,
    ) {
        data.fill(T::EQUILIBRIUM);
        let mut left = [0.0; RENDER_BLOCK];
        let mut right = [0.0; RENDER_BLOCK];
        for chunk in data.chunks_mut(RENDER_BLOCK * channels) {
            let Some(mut control) = shared.control.try_lock() else {
                return;
            };
            if control.shutdown
                || control.generation != generation
                || matches!(
                    control.snapshot.state,
                    MidiPlaybackState::Idle
                        | MidiPlaybackState::Error
                        | MidiPlaybackState::Finished
                )
            {
                self.stop();
                return;
            }
            if control.snapshot.state != MidiPlaybackState::Playing {
                return;
            }
            let frames = (chunk.len() / channels)
                .min((self.total_frames - self.rendered_frames).min(RENDER_BLOCK as u64) as usize);
            let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
            self.sequence
                .render(&mut left[..frames], &mut right[..frames]);
            for (index, frame) in chunk.chunks_exact_mut(channels).take(frames).enumerate() {
                let l = finite_sample(left[index], volume);
                let r = finite_sample(right[index], volume);
                if channels == 1 {
                    frame[0] = T::from_sample(0.5 * l + 0.5 * r);
                } else {
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                }
            }
            self.rendered_frames += frames as u64;
            control.snapshot.position_seconds = (self.rendered_frames as f64
                / self.sample_rate as f64)
                .min(control.snapshot.duration_seconds);
            if self.rendered_frames == self.total_frames {
                self.stop();
                control.snapshot.state = MidiPlaybackState::Finished;
                shared.changed.notify_all();
                return;
            }
        }
    }
}

fn finite_sample(sample: f32, volume: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    // dasp's I24 conversion requires an upper bound strictly below +1.
    (sample * volume).clamp(-1.0, 1.0 - f32::EPSILON)
}

struct CancelReader<'a, R> {
    reader: R,
    shared: &'a Shared,
    generation: u64,
}

impl<'a, R> CancelReader<'a, R> {
    fn new(reader: R, shared: &'a Shared, generation: u64) -> Self {
        Self {
            reader,
            shared,
            generation,
        }
    }
}

impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.shared.current(self.generation) {
            return Err(io::Error::other("MIDI playback cancelled"));
        }
        let length = buffer.len().min(64 * 1024);
        self.reader.read(&mut buffer[..length])
    }
}

fn read_soundfont(path: &Path, shared: &Shared, generation: u64) -> Result<Vec<u8>, String> {
    shared.check_current(generation)?;
    let file =
        File::open(path).map_err(|e| format!("Cannot open SoundFont {}: {e}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Cannot read SoundFont metadata: {e}"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOUNDFONT_BYTES as u64 {
        return Err(format!(
            "SoundFont must be a regular file of 1..={MAX_SOUNDFONT_BYTES} bytes"
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    CancelReader::new(file, shared, generation)
        .take(MAX_SOUNDFONT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read SoundFont {}: {e}", path.display()))?;
    if bytes.len() > MAX_SOUNDFONT_BYTES {
        return Err(format!("SoundFont exceeds {MAX_SOUNDFONT_BYTES} bytes"));
    }
    Ok(bytes)
}

fn validate_soundfont(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
        return Err("Invalid SF2 RIFF header".into());
    }
    let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if length.checked_add(8) != Some(bytes.len()) {
        return Err("SF2 RIFF length does not match the file".into());
    }
    validate_riff_chunks(&bytes[12..], 0)
}

fn validate_riff_chunks(mut bytes: &[u8], depth: usize) -> Result<(), String> {
    if depth > 2 {
        return Err("SF2 chunk nesting exceeds the playback limit".into());
    }
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            return Err("Truncated SF2 chunk header".into());
        }
        let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let body = bytes
            .get(8..)
            .and_then(|body| body.get(..length))
            .ok_or("Truncated SF2 chunk payload")?;
        if &bytes[..4] == b"LIST" {
            if body.len() < 4 {
                return Err("Invalid SF2 LIST chunk".into());
            }
            validate_riff_chunks(&body[4..], depth + 1)?;
        }
        bytes = bytes
            .get(8 + length + (length & 1)..)
            .ok_or("Truncated SF2 chunk padding")?;
    }
    Ok(())
}

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Result<&'a [u8], String> {
    let value = bytes.get(..count).ok_or("Truncated MIDI data")?;
    *bytes = &bytes[count..];
    Ok(value)
}

fn vlq(bytes: &mut &[u8]) -> Result<u32, String> {
    let mut value = 0;
    for _ in 0..4 {
        let byte = take(bytes, 1)?[0];
        value = (value << 7) | (byte & 0x7f) as u32;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("MIDI variable-length integer exceeds four bytes".into())
}

fn validate_midi(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_MIDI_BYTES {
        return Err(format!("MIDI exceeds {MAX_MIDI_BYTES} bytes"));
    }
    if bytes.len() < 14 || &bytes[..4] != b"MThd" || bytes[4..8] != 6u32.to_be_bytes() {
        return Err("Invalid MIDI header".into());
    }
    let format = u16::from_be_bytes([bytes[8], bytes[9]]);
    if format == 2 {
        return Err(
            "SMF format 2 contains independent sequences and is not supported for GM playback"
                .into(),
        );
    }
    if format > 1 {
        return Err(format!("Unsupported MIDI format {format}"));
    }
    let count = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;
    if count == 0 || count > MAX_TRACKS || (format == 0 && count != 1) {
        return Err(format!("Invalid MIDI track count {count}; playback allows 1..={MAX_TRACKS} tracks and SMF0 requires one"));
    }
    if bytes[12] & 0x80 != 0 {
        return Err("SMPTE MIDI timing is not supported by the GM playback sequencer; note preview remains available".into());
    }
    if bytes[12] == 0 && bytes[13] == 0 {
        return Err("MIDI timing division must be positive".into());
    }
    let mut bytes = &bytes[14..];
    let mut event_count = 0;
    let mut note_count = 0;
    for track in 0..count {
        if take(&mut bytes, 4)? != b"MTrk" {
            return Err("Invalid MIDI track header".into());
        }
        let length = u32::from_be_bytes(take(&mut bytes, 4)?.try_into().unwrap()) as usize;
        let mut events = take(&mut bytes, length)?;
        let mut running = None;
        let mut tick = 0u64;
        let mut ended = false;
        while !events.is_empty() {
            if ended {
                return Err(format!(
                    "MIDI track {} has data after EndOfTrack",
                    track + 1
                ));
            }
            event_count += 1;
            if event_count > MAX_EVENTS {
                return Err(format!(
                    "MIDI exceeds the playback limit of {MAX_EVENTS} events"
                ));
            }
            tick += vlq(&mut events)? as u64;
            if tick >= i32::MAX as u64 {
                return Err("MIDI tick count exceeds the GM sequencer limit (2147483646)".into());
            }
            let first = *events.first().ok_or("Missing MIDI event status")?;
            let status = if first & 0x80 != 0 {
                take(&mut events, 1)?[0]
            } else {
                running.ok_or("MIDI running status has no preceding channel message")?
            };
            match status {
                0x80..=0xef => {
                    running = Some(status);
                    let size = if matches!(status & 0xf0, 0xc0 | 0xd0) {
                        1
                    } else {
                        2
                    };
                    let data = take(&mut events, size)?;
                    if data.iter().any(|byte| *byte >= 0x80) {
                        return Err("MIDI channel data must be in 0..127".into());
                    }
                    if status & 0xf0 == 0x90 && data[1] != 0 {
                        note_count += 1;
                        if note_count > MAX_NOTES {
                            return Err(format!(
                                "MIDI exceeds the playback limit of {MAX_NOTES} notes"
                            ));
                        }
                    }
                }
                0xf0 | 0xf7 | 0xff => {
                    running = None;
                    let meta = if status == 0xff {
                        Some(take(&mut events, 1)?[0])
                    } else {
                        None
                    };
                    let length = vlq(&mut events)? as usize;
                    let data = take(&mut events, length)?;
                    match meta {
                        Some(0x2f) => {
                            if !data.is_empty() {
                                return Err("Invalid MIDI EndOfTrack length".into());
                            }
                            ended = true;
                        }
                        Some(0x51) if data.len() != 3 || data == [0, 0, 0] => {
                            return Err("Invalid MIDI tempo".into())
                        }
                        _ => {}
                    }
                }
                _ => return Err(format!("Invalid SMF status {status:#x}")),
            }
        }
        if !ended {
            return Err(format!(
                "MIDI track {} has no EndOfTrack; the GM sequencer cannot safely play it",
                track + 1
            ));
        }
    }
    if !bytes.is_empty() {
        return Err("Unexpected bytes beyond the declared MIDI tracks".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    fn midi() -> Vec<u8> {
        let track = [
            0, 0xc0, 40, 0, 0x90, 60, 100, 100, 0x90, 60, 0, 0, 0xff, 0x2f, 0,
        ];
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\0\x64MTrk".to_vec();
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&track);
        bytes
    }

    fn wait_state(player: &MidiPreviewPlayer, state: MidiPlaybackState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut control = player.shared.control.lock();
        while control.snapshot.state != state {
            let now = Instant::now();
            assert!(
                now < deadline,
                "expected {state:?}, got {:?}",
                control.snapshot
            );
            player.shared.changed.wait_for(&mut control, deadline - now);
        }
    }

    struct FakeStream {
        events: Sender<(&'static str, u64, thread::ThreadId)>,
        generation: u64,
    }

    impl OutputStream for FakeStream {
        fn start(&self) -> Result<(), String> {
            self.events
                .send(("start", self.generation, thread::current().id()))
                .unwrap();
            Ok(())
        }
    }

    impl Drop for FakeStream {
        fn drop(&mut self) {
            let _ = self
                .events
                .send(("drop", self.generation, thread::current().id()));
        }
    }

    type StreamEvent = (&'static str, u64, thread::ThreadId);

    fn fake_player() -> (MidiPreviewPlayer, Receiver<StreamEvent>) {
        let (sender, receiver) = mpsc::channel();
        let player = MidiPreviewPlayer::with_prepare(Arc::new(move |request, _| {
            sender
                .send(("load", request.generation, thread::current().id()))
                .unwrap();
            Ok(Prepared {
                stream: Box::new(FakeStream {
                    events: sender.clone(),
                    generation: request.generation,
                }),
                duration_seconds: 2.0,
            })
        }));
        (player, receiver)
    }

    fn receive(events: &Receiver<StreamEvent>) -> StreamEvent {
        events.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    struct FakeSequence {
        rendered: usize,
        stops: usize,
        left: f32,
        right: f32,
    }

    impl Default for FakeSequence {
        fn default() -> Self {
            Self {
                rendered: 0,
                stops: 0,
                left: 0.8,
                right: -0.4,
            }
        }
    }

    impl Sequence for FakeSequence {
        fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
            self.rendered += left.len();
            left.fill(self.left);
            right.fill(self.right);
        }
        fn stop(&mut self) {
            self.stops += 1;
        }
    }

    fn render_fixture(duration: f64) -> (MidiPreviewPlayer, Renderer<FakeSequence>) {
        let player = MidiPreviewPlayer::new();
        {
            let mut control = player.shared.control.lock();
            control.generation = 1;
            control.snapshot.state = MidiPlaybackState::Loading;
        }
        assert!(player.shared.ready(1, duration));
        (
            player,
            Renderer::new(FakeSequence::default(), 100, duration),
        )
    }

    #[test]
    fn construction_does_not_start_worker_or_open_device() {
        let (player, events) = fake_player();
        assert!(player.worker.is_none());
        assert_eq!(player.snapshot().state, MidiPlaybackState::Idle);
        player.pause();
        player.resume();
        player.stop();
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn play_stop_reopen_and_drop_own_stream_on_worker_thread() {
        let (mut player, events) = fake_player();
        player
            .play(midi(), "fake.sf2".into(), Some("selected device".into()))
            .unwrap();
        wait_state(&player, MidiPlaybackState::Playing);
        let load = receive(&events);
        let start = receive(&events);
        assert_eq!((load.0, start.0), ("load", "start"));
        assert_ne!(load.2, thread::current().id());
        assert_eq!(load.2, start.2);
        player.stop();
        let dropped = receive(&events);
        assert_eq!(dropped, ("drop", load.1, load.2));
        assert_eq!(player.snapshot().state, MidiPlaybackState::Idle);
        player.play(midi(), "fake.sf2".into(), None).unwrap();
        wait_state(&player, MidiPlaybackState::Playing);
        let next = receive(&events);
        assert_ne!(next.1, load.1);
        assert_eq!(receive(&events).0, "start");
        drop(player);
        assert_eq!(receive(&events), ("drop", next.1, next.2));
    }

    #[test]
    fn completed_playback_releases_its_output_stream() {
        let (mut player, events) = fake_player();
        player.play(midi(), "fake.sf2".into(), None).unwrap();
        wait_state(&player, MidiPlaybackState::Playing);
        let loaded = receive(&events);
        assert_eq!(receive(&events).0, "start");
        player.shared.control.lock().snapshot.state = MidiPlaybackState::Finished;
        player.shared.changed.notify_all();
        assert_eq!(receive(&events), ("drop", loaded.1, loaded.2));
        assert_eq!(player.snapshot().state, MidiPlaybackState::Finished);
    }

    #[test]
    fn pause_freezes_clock_and_voices_resume_without_restart_then_finish() {
        let (player, mut renderer) = render_fixture(1.0);
        let mut output = [0.0f32; 40];
        renderer.render(&mut output, 2, &player.shared, 1);
        assert_eq!(player.snapshot().position_seconds, 0.2);
        player.pause();
        renderer.render(&mut output, 2, &player.shared, 1);
        assert_eq!(player.snapshot().state, MidiPlaybackState::Paused);
        assert_eq!(player.snapshot().position_seconds, 0.2);
        assert_eq!(renderer.sequence.rendered, 20);
        assert!(output.iter().all(|value| *value == 0.0));
        assert_eq!(renderer.sequence.stops, 0);
        player.resume();
        renderer.render(&mut output, 2, &player.shared, 1);
        assert_eq!(player.snapshot().position_seconds, 0.4);
        let mut end = [0.0f32; 160];
        renderer.render(&mut end, 2, &player.shared, 1);
        assert_eq!(player.snapshot().state, MidiPlaybackState::Finished);
        assert_eq!(player.snapshot().position_seconds, 1.0);
        assert_eq!(renderer.sequence.rendered, 100);
        assert_eq!(renderer.sequence.stops, 1);
        assert!(end[120..].iter().all(|value| *value == 0.0));
        player.resume();
        renderer.render(&mut output, 2, &player.shared, 1);
        assert_eq!(player.snapshot().state, MidiPlaybackState::Finished);
        assert!(output.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn volume_is_independent_and_output_is_finite_for_all_pcm_formats() {
        let (player, mut renderer) = render_fixture(10.0);
        let (other, _) = render_fixture(10.0);
        player.set_volume(0.5);
        let mut stereo = [0.0f32; 2];
        renderer.render(&mut stereo, 2, &player.shared, 1);
        assert_eq!(stereo, [0.4, -0.2]);
        assert_eq!(
            f32::from_bits(other.shared.volume.load(Ordering::Relaxed)),
            1.0
        );
        player.set_volume(f32::NAN);
        renderer.render(&mut stereo, 2, &player.shared, 1);
        assert!(stereo.iter().all(|value| *value == 0.0));
        player.set_volume(2.0);
        renderer.sequence.left = f32::INFINITY;
        renderer.sequence.right = f32::NAN;
        renderer.render(&mut stereo, 2, &player.shared, 1);
        assert_eq!(stereo, [0.0; 2]);
        renderer.sequence.left = 9.0;
        renderer.sequence.right = -9.0;
        macro_rules! check_pcm {
            ($($sample:ty),+ $(,)?) => { $(
                let mut output = [<$sample as cpal::Sample>::EQUILIBRIUM; 2];
                renderer.render(&mut output, 2, &player.shared, 1);
                for value in output {
                    let sample: f32 = cpal::Sample::to_sample(value);
                    assert!(sample.is_finite() && (-1.0..=1.0).contains(&sample));
                }
            )+ };
        }
        check_pcm!(
            i8,
            i16,
            cpal::I24,
            i32,
            i64,
            u8,
            u16,
            cpal::U24,
            u32,
            u64,
            f32,
            f64
        );
        player.pause();
        let mut unsigned = [0u16; 2];
        renderer.render(&mut unsigned, 2, &player.shared, 1);
        assert_eq!(unsigned, [32768; 2]);
    }

    #[test]
    fn mono_downmix_extra_channels_and_stop_silence_are_correct() {
        let (player, mut renderer) = render_fixture(1.0);
        let mut mono = [0.0f32; 1];
        renderer.render(&mut mono, 1, &player.shared, 1);
        assert_eq!(mono, [0.2]);
        let mut surround = [0.0f32; 6];
        renderer.render(&mut surround, 6, &player.shared, 1);
        assert_eq!(surround, [0.8, -0.4, 0.0, 0.0, 0.0, 0.0]);
        player.stop();
        renderer.render(&mut surround, 6, &player.shared, 1);
        assert_eq!(renderer.sequence.stops, 1);
        assert_eq!(surround, [0.0; 6]);
        assert_eq!(player.snapshot().position_seconds, 0.0);
    }

    #[test]
    fn late_load_is_discarded_and_latest_request_alone_can_start() {
        let (sender, events) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let mut player = MidiPreviewPlayer::with_prepare(Arc::new(move |request, _| {
            sender
                .send(("load", request.generation, thread::current().id()))
                .unwrap();
            wait.lock().recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(Prepared {
                stream: Box::new(FakeStream {
                    events: sender.clone(),
                    generation: request.generation,
                }),
                duration_seconds: 1.0,
            })
        }));
        player.play(midi(), "first.sf2".into(), None).unwrap();
        let first = receive(&events);
        assert_eq!(player.snapshot().state, MidiPlaybackState::Loading);
        player.stop();
        player.play(midi(), "discarded.sf2".into(), None).unwrap();
        player.play(midi(), "latest.sf2".into(), None).unwrap();
        player.pause();
        let latest_generation = player.shared.control.lock().generation;
        release.send(()).unwrap();
        assert_eq!(receive(&events), ("drop", first.1, first.2));
        let latest = receive(&events);
        assert_eq!((latest.0, latest.1), ("load", latest_generation));
        release.send(()).unwrap();
        wait_state(&player, MidiPlaybackState::Paused);
        assert_eq!(receive(&events).0, "start");
        player.resume();
        assert_eq!(player.snapshot().state, MidiPlaybackState::Playing);
        drop(player);
        assert_eq!(receive(&events).0, "drop");
    }

    #[test]
    fn stale_errors_cannot_overwrite_new_playback() {
        let (player, _) = render_fixture(1.0);
        player.shared.fail(0, "old device failed".into());
        assert_eq!(player.snapshot().state, MidiPlaybackState::Playing);
        player.shared.fail(1, "selected device disconnected".into());
        assert_eq!(player.snapshot().state, MidiPlaybackState::Error);
        assert!(player.snapshot().error.unwrap().contains("disconnected"));
        assert!(!player.shared.ready(1, 4.0));
    }

    #[test]
    fn load_and_device_failures_are_visible_not_silent_playback() {
        for reason in [
            "No output device",
            "Invalid SoundFont",
            "Cannot start stream",
        ] {
            let mut player =
                MidiPreviewPlayer::with_prepare(Arc::new(move |_, _| Err(reason.into())));
            player.play(midi(), "fake.sf2".into(), None).unwrap();
            wait_state(&player, MidiPlaybackState::Error);
            assert_eq!(player.snapshot().error.as_deref(), Some(reason));
            assert_eq!(player.snapshot().position_seconds, 0.0);
        }
        let mut player = MidiPreviewPlayer::new();
        assert!(player.play(midi(), PathBuf::new(), None).is_err());
        assert_eq!(player.snapshot().state, MidiPlaybackState::Error);
        let missing =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("nonexistent-midi-preview-test.sf2");
        player.play(midi(), missing, None).unwrap();
        wait_state(&player, MidiPlaybackState::Error);
        assert!(player
            .snapshot()
            .error
            .unwrap()
            .contains("Cannot open SoundFont"));
    }

    #[test]
    fn stream_start_failure_never_becomes_playing() {
        struct FailedStream;
        impl OutputStream for FailedStream {
            fn start(&self) -> Result<(), String> {
                Err("Selected output failed to start".into())
            }
        }
        let mut player = MidiPreviewPlayer::with_prepare(Arc::new(|request, _| {
            assert_eq!(request.midi_bytes, midi());
            assert_eq!(request.soundfont, PathBuf::from("selected.sf2"));
            assert_eq!(request.output_device.as_deref(), Some("selected device"));
            Ok(Prepared {
                stream: Box::new(FailedStream),
                duration_seconds: 0.5,
            })
        }));
        player
            .play(
                midi(),
                "selected.sf2".into(),
                Some("selected device".into()),
            )
            .unwrap();
        wait_state(&player, MidiPlaybackState::Error);
        assert_eq!(
            player.snapshot().error.as_deref(),
            Some("Selected output failed to start")
        );
        assert_eq!(player.snapshot().position_seconds, 0.0);
    }

    #[test]
    fn cancellation_interrupts_loading_reads_and_drop_joins_worker() {
        let shared = Shared::default();
        shared.control.lock().generation = 1;
        let mut reader = CancelReader::new(Cursor::new([1, 2, 3]), &shared, 1);
        let mut byte = [0];
        assert_eq!(reader.read(&mut byte).unwrap(), 1);
        shared.control.lock().generation = 2;
        assert!(reader.read(&mut byte).is_err());
        let (sender, events) = mpsc::channel();
        let mut player = MidiPreviewPlayer::with_prepare(Arc::new(move |request, shared| {
            sender.send(()).unwrap();
            let mut control = shared.control.lock();
            while !control.shutdown {
                shared.changed.wait(&mut control);
            }
            drop(control);
            shared.check_current(request.generation)?;
            unreachable!()
        }));
        player.play(midi(), "fake.sf2".into(), None).unwrap();
        events.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(player);
    }

    #[test]
    fn original_midi_loads_without_rewriting_and_invalid_data_is_rejected() {
        let bytes = midi();
        let original = bytes.clone();
        validate_midi(&bytes).unwrap();
        let midi = MidiFile::new(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(midi.get_length(), 0.5);
        assert_eq!(bytes, original);
        for end in 0..bytes.len() {
            assert!(
                validate_midi(&bytes[..end]).is_err(),
                "prefix {end} accepted"
            );
        }
        for (offset, value) in [(9, 2), (11, 0), (13, 0), (24, 0xff), (12, 0xe7)] {
            let mut bad = bytes.clone();
            bad[offset] = value;
            assert!(validate_midi(&bad).is_err(), "mutation {offset} accepted");
        }
        let mut smpte = bytes.clone();
        smpte[12] = 0xe7;
        assert!(validate_midi(&smpte).unwrap_err().contains("SMPTE"));
        assert!(validate_soundfont(b"RIFF\xff\xff\xff\xffsfbk").is_err());
        assert!(validate_riff_chunks(b"smpl\xff\xff\xff\xff", 0).is_err());
    }
}
