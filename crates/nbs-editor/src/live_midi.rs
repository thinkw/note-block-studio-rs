//! 实时 MIDI 输入：原版 GMSSimpleMIDI 扩展（`rtmidi_*`）+ `midi_input_*` 脚本族
//! + 设备管理器（`draw_window_mididevices.gml`）的移植。
//!
//! 原版管线：`control_step.gml:104-141` 每步把 winmidi 消息泵进全局
//! `midi_keypresses` / `midi_keyreleases` 两个列表（**所有设备共用一份**，
//! 取值的 `midi_input_key_press_note` 干脆忽略设备号），`draw_piano.gml:152`
//! 的按键循环对 0..87 每个琴键调 `midi_input_set_key`，帧尾清空。指派表
//! `mididevice_instrument[0..10]`：-3 禁用 / -2 无声 / -1 使用当前选中音色 /
//! >=0 具体音色。设备枚举与开关由平台后端提供（Windows 上是 `nbs-app` 的
//! winmm 实现，对应原版扩展里的 rtmidi），无头测试注入假后端。
//!
//! 与原版的偏差（记账）：
//! - 指派 -2（无声）时原版仍会走录音写入，`add_block_manual` 拿到 -2 的音色
//!   实例在 GML 里行为未定；我们的音色字段是 u8 表示不了 -2，跳过写入。
//! - 原版设备数超过 11 时越界指派按 GML 数组默认值读成 0（Harp）；我们只
//!   处理前 11 个设备。
//! - 原版把刚录的块写进 `song_played` 供渐亮效果；回放的渐亮我们本来就没
//!   移植（`control_draw.gml:332` 那段），所以这里没有对应物。

use nbs_core::model::Note;

use crate::commands::record_note_at;
use crate::state::EditorState;

/// 原版 `mididevice_instrument` 的槽数（设备窗口最多画 11 行）。
pub const DEVICE_SLOTS: usize = 11;
/// 原版 `midi_input_set_key` 的 `note == key + 21`：MIDI 音号比琴键号大 21。
const NOTE_OFFSET: u8 = 21;

/// 一条原始 MIDI 消息，由平台后端塞进队列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawMidiMsg {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

/// 平台后端。Windows 下 nbs-app 用 winmm 实现；无头测试用假实现。
pub trait MidiInBackend: Send + Sync {
    /// 原版 `midi_input_devices()` + `midi_input_device_name(a)` 的合并。
    fn devices(&self) -> Vec<String>;
    /// 原版 `rtmidi_set_inport(device)`：true 成功，false 设备不存在。
    fn open(&self, index: usize) -> bool;
    /// 关掉当前口（换设备、退出时）。
    fn close(&self);
    /// 原版 `rtmidi_check_message()` / `rtmidi_get_message(i)` 的批量版：
    /// 取走自上次调用以来的全部消息。
    fn take_messages(&self) -> Vec<RawMidiMsg>;
}

/// 原版实时 MIDI 那一堆全局变量的集合。
pub struct LiveMidi {
    /// 平台后端；`None`（无后端）时设备列表恒为空。
    pub backend: Option<std::sync::Arc<dyn MidiInBackend>>,
    /// 原版 `mididevice_instrument[0..10]`。
    pub assignments: [i8; DEVICE_SLOTS],
    /// 原版 `midi_device_current`。
    pub current_device: usize,
    /// 原版 `midi_device_names`。
    pub devices: Vec<String>,
    /// 原版 `midi_devices_old`：`midi_refresh_device` 里设备数变化就重置指派。
    devices_old: usize,
    /// 原版 `midi_keypresses`：本帧收到的 `[note, velocity]`。
    pub presses: Vec<(u8, u8)>,
    /// 原版 `midi_keyreleases`：本帧收到的 note。
    pub releases: Vec<u8>,
    /// 原版 `key_midipress[a]`：MIDI 按住态（和 PC 键的 `keys_held` 并存）。
    pub key_midipress: [bool; 88],
}

impl LiveMidi {
    pub fn new() -> Self {
        Self {
            backend: None,
            // 原版 `control_create.gml:350`：11 个槽全部初始化为 -3。
            assignments: [-3; DEVICE_SLOTS],
            current_device: 0,
            devices: Vec::new(),
            devices_old: 0,
            presses: Vec::new(),
            releases: Vec::new(),
            key_midipress: [false; 88],
        }
    }

    /// 原版 `control_step.gml:104-141` 的泵：状态字节砍掉通道号（16 通道都认），
    /// 0x90 且力度 0 当作 0x80 松键；只认 128/144 两种（控制号 / 补丁 / 弯音
    /// 在原版里整段注释掉了）。
    pub fn pump(&mut self) {
        let Some(backend) = &self.backend else {
            return;
        };
        for msg in backend.take_messages() {
            let mut status = msg.status & 0xF0;
            if status == 0x90 && msg.data2 == 0 {
                status = 0x80;
            }
            match status {
                0x80 => self.releases.push(msg.data1),
                0x90 => self.presses.push((msg.data1, msg.data2)),
                _ => {}
            }
        }
    }

    /// 原版 `midi_refresh_device`：重列设备名；设备数和上次不一样就
    /// `midi_set_device(0)`。
    pub fn refresh_devices(&mut self) {
        self.devices = self
            .backend
            .as_ref()
            .map(|b| b.devices())
            .unwrap_or_default();
        if self.devices_old != self.devices.len() {
            self.set_device(0);
        }
        self.devices_old = self.devices.len();
    }

    /// 原版 `midi_set_device`：开新口失败返回 false；成功则把全部指派重置为
    /// -3、本设备置 -1（使用选择的）。
    pub fn set_device(&mut self, device: usize) -> bool {
        let Some(backend) = &self.backend else {
            return false;
        };
        if !backend.open(device) {
            return false;
        }
        self.current_device = device;
        self.assignments = [-3; DEVICE_SLOTS];
        if device < DEVICE_SLOTS {
            self.assignments[device] = -1;
        }
        true
    }
}

impl Default for LiveMidi {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorState {
    /// 原版 `midi_input_set_key(key, totalcols)`：设备窗口里每个指派到
    /// 「非禁用」的设备都扫同一份按下/松开队列，命中 `key + 21` 就发声 /
    /// 录音。`totalcols` 只喂给原版 `toggle_playing`，我们的起播路径
    /// （[`EditorState::begin_playback`]）不需要。
    pub fn midi_input_set_key(&mut self, key: u8) {
        let devices = self.live_midi.devices.len().min(DEVICE_SLOTS);
        // 取本帧消息的快照：下面会改 doc / keys_held，原版列表在帧尾统一清空，
        // 快照不影响语义。
        let presses = self.live_midi.presses.clone();
        let releases = self.live_midi.releases.clone();
        for d in 0..devices {
            let mut ins = self.live_midi.assignments[d];
            if ins <= -3 {
                continue;
            }
            if !self.live_midi.key_midipress[key as usize] {
                for k in 0..presses.len() {
                    if presses[k].0 != key + NOTE_OFFSET {
                        continue;
                    }
                    let vel = presses[k].1.min(100);
                    // 原版 `if (ins = -1) ins = songs[song].instrument`。
                    if ins == -1 {
                        ins = self.current.instrument as i8;
                    }
                    if ins >= 0 {
                        // 原版 `play_sound(ins, key, vel, 100, 0)`。
                        let note = Note {
                            instrument: ins as u8,
                            key,
                            velocity: vel,
                            panning: 100,
                            pitch: 0,
                        };
                        self.audio.preview_note(&note);
                    }
                    if self.prefs.select_lastpressed {
                        self.current.key = key;
                        self.current.velocity = 100;
                        self.current.panning = 100;
                        self.current.pitch = 0;
                    }
                    // 原版 `if (playing = 0.25) toggle_playing(totalcols)`：
                    // 录音待命下第一个按键起播，与 Command::KeyPress 同款序列。
                    if self.record_armed {
                        self.record_armed = false;
                        self.prepare_audio();
                        self.begin_playback();
                    }
                    // 原版 `if (playing && record) { while (!add_block_manual(
                    // ceil(marker_pos), b, ins, key, vel, 100, 0)) b += 1; ... }`。
                    if self.is_playing() && self.record {
                        if let Some(note) = (ins >= 0).then(|| Note {
                            instrument: ins as u8,
                            key,
                            velocity: vel,
                            panning: 100,
                            pitch: 0,
                        }) {
                            record_note_at(self, note);
                        }
                    }
                    self.live_midi.key_midipress[key as usize] = true;
                    self.keys_held[key as usize] = true;
                    break;
                }
            }
            if self.live_midi.key_midipress[key as usize] {
                for k in 0..releases.len() {
                    if releases[k] != key + NOTE_OFFSET {
                        continue;
                    }
                    self.live_midi.key_midipress[key as usize] = false;
                    self.keys_held[key as usize] = false;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EditorState;
    use nbs_audio::AudioEngine;
    use nbs_core::model::Note;
    use std::sync::{Arc, Mutex};

    /// 假后端：固定设备表（可中途增减）+ 可注入消息。
    struct FakeBackend {
        devices: Mutex<Vec<String>>,
        queue: Mutex<Vec<RawMidiMsg>>,
    }
    impl FakeBackend {
        fn new(devices: usize) -> Self {
            Self {
                devices: Mutex::new((0..devices).map(|i| format!("Fake Device {i}")).collect()),
                queue: Mutex::new(Vec::new()),
            }
        }
        fn push(&self, status: u8, data1: u8, data2: u8) {
            self.queue.lock().unwrap().push(RawMidiMsg {
                status,
                data1,
                data2,
            });
        }
    }
    impl MidiInBackend for FakeBackend {
        fn devices(&self) -> Vec<String> {
            self.devices.lock().unwrap().clone()
        }
        fn open(&self, index: usize) -> bool {
            index < self.devices.lock().unwrap().len()
        }
        fn close(&self) {}
        fn take_messages(&self) -> Vec<RawMidiMsg> {
            std::mem::take(&mut self.queue.lock().unwrap())
        }
    }

    /// 计数音频：包一层 SilentEngine，记下试听的音符。
    struct CountingAudio {
        inner: nbs_audio::SilentEngine,
        previews: Arc<Mutex<Vec<Note>>>,
    }
    impl AudioEngine for CountingAudio {
        fn name(&self) -> &str {
            "counting"
        }
        fn prepare(&mut self, song: &nbs_core::model::Song) {
            self.inner.prepare(song);
        }
        fn preview_note_vp(&mut self, note: &Note, vol: u8, stereo: u8) {
            self.previews.lock().unwrap().push(note.clone());
            self.inner.preview_note_vp(note, vol, stereo);
        }
        fn preview_metronome(&mut self, key: u8) {
            self.inner.preview_metronome(key);
        }
        fn start(&mut self, from_tick: f64) {
            self.inner.start(from_tick);
        }
        fn stop(&mut self) {
            self.inner.stop();
        }
        fn is_playing(&self) -> bool {
            self.inner.is_playing()
        }
        fn position_ticks(&self) -> f64 {
            self.inner.position_ticks()
        }
        fn advance(&mut self, dt: f64) {
            self.inner.advance(dt);
        }
        fn active_voices(&self) -> usize {
            self.inner.active_voices()
        }
        fn clock(&self) -> &nbs_audio::scheduler::TransportClock {
            self.inner.clock()
        }
        fn clock_mut(&mut self) -> &mut nbs_audio::scheduler::TransportClock {
            self.inner.clock_mut()
        }
        fn device_info(&self) -> String {
            self.inner.device_info()
        }
        fn bank(&self) -> Arc<nbs_audio::SoundBank> {
            self.inner.bank()
        }
        fn sample_status(&self) -> String {
            self.inner.sample_status()
        }
    }

    fn state_with_midi(
        devices: usize,
    ) -> (
        EditorState,
        Arc<FakeBackend>,
        Arc<Mutex<Vec<Note>>>,
    ) {
        let mut state = EditorState::for_test();
        let backend = Arc::new(FakeBackend::new(devices));
        let previews = Arc::new(Mutex::new(Vec::new()));
        state.audio = Box::new(CountingAudio {
            inner: nbs_audio::SilentEngine::new(),
            previews: Arc::clone(&previews),
        });
        state.live_midi.backend = Some(Arc::clone(&backend) as Arc<dyn MidiInBackend>);
        state.live_midi.refresh_devices();
        (state, backend, previews)
    }

    fn previews_of(previews: &Arc<Mutex<Vec<Note>>>) -> Vec<Note> {
        previews.lock().unwrap().clone()
    }

    fn note_of(note: &Note) -> u8 {
        note.key
    }

    /// 原版 control_step 泵：0x90/力度 0 当松键、通道号砍掉、控制号丢弃。
    #[test]
    fn pump_classifies_press_release_and_ignores_controls() {
        let backend = Arc::new(FakeBackend::new(1));
        let mut lm = LiveMidi::new();
        lm.backend = Some(backend.clone() as Arc<dyn MidiInBackend>);
        backend.push(0x90, 60, 100);
        backend.push(0x91, 62, 64);
        backend.push(0x90, 64, 0);
        backend.push(0x80, 66, 0);
        backend.push(0xB0, 64, 127);
        lm.pump();
        assert_eq!(lm.presses, vec![(60, 100), (62, 64)]);
        assert_eq!(lm.releases, vec![64, 66]);
    }

    /// 原版 `midi_set_device`：成功后清空全部指派、本设备置 -1；开不出的口
    /// 返回失败且不动指派。
    #[test]
    fn set_device_resets_assignments_and_reports_failure() {
        let (mut state, _backend, _previews) = state_with_midi(2);
        assert!(state.live_midi.set_device(1), "有 2 台设备，开 1 号该成功");
        assert_eq!(state.live_midi.assignments[1], -1, "本设备置为使用选择的");
        assert_eq!(state.live_midi.assignments[0], -3, "其他槽全部清空");
        let before = state.live_midi.assignments;
        assert!(!state.live_midi.set_device(5), "越界的口开不出");
        assert_eq!(state.live_midi.assignments, before, "失败不动指派");
    }

    /// 原版 `midi_refresh_device`：设备数变化时自动 `midi_set_device(0)`。
    #[test]
    fn refresh_devices_reassigns_when_count_changes() {
        let (mut state, backend, _previews) = state_with_midi(1);
        assert_eq!(state.live_midi.assignments[0], -1, "首台设备自动接到 0 号槽");
        backend.devices.lock().unwrap().push("Late Device".into());
        state.live_midi.refresh_devices();
        assert_eq!(state.live_midi.devices.len(), 2);
        assert_eq!(state.live_midi.assignments[0], -1, "设备数变了重新接到 0 号");
    }

    /// `midi_input_set_key` 发声路径：指派 -1 走当前音色，力度截到 100，
    /// 松键后按住态清掉。
    #[test]
    fn press_plays_and_release_clears_hold() {
        let (mut state, backend, previews) = state_with_midi(1);
        state.current.instrument = 7;
        backend.push(0x90, 61, 130);
        state.live_midi.pump();
        state.midi_input_set_key(40);
        assert!(state.live_midi.key_midipress[40], "按下后按住态置位");
        assert!(state.keys_held[40], "琴键画按下态");
        let previews = previews_of(&previews);
        assert_eq!(previews.len(), 1, "按下只发一声");
        assert_eq!(note_of(&previews[0]), 40);
        assert_eq!(previews[0].instrument, 7, "指派 -1 用当前音色");
        assert_eq!(previews[0].velocity, 100, "力度截到 100");

        backend.push(0x80, 61, 0);
        state.live_midi.pump();
        state.midi_input_set_key(40);
        assert!(!state.live_midi.key_midipress[40]);
        assert!(!state.keys_held[40]);
    }
    /// 指派成具体音色（内置 2 号）时发声用指派的音色；禁用（-3）不发声。
    #[test]
    fn assigned_instrument_is_used_and_disabled_device_is_silent() {
        let (mut state, backend, previews) = state_with_midi(1);
        state.live_midi.set_device(0);
        state.live_midi.assignments[0] = 2;
        backend.push(0x90, 45, 80);
        state.live_midi.pump();
        state.midi_input_set_key(24);
        let got = previews_of(&previews);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].instrument, 2, "用指派的音色");
        assert_eq!(got[0].velocity, 80);

        state.live_midi.assignments[0] = -3;
        state.live_midi.key_midipress[24] = false;
        backend.push(0x80, 45, 0);
        backend.push(0x90, 47, 80);
        state.live_midi.pump();
        let before = previews_of(&previews).len();
        state.midi_input_set_key(26);
        assert_eq!(previews_of(&previews).len(), before, "禁用设备不出声");
    }

    /// 录音路径：开着录制并播放时，MIDI 按键按原版 `add_block_manual` 语义
    /// 从第 0 层往上找空格写入。
    #[test]
    fn press_records_block_while_playing_and_recording() {
        let (mut state, backend, _previews) = state_with_midi(1);
        // for_test 自带演示曲，录音断言要空歌（原版 ConfirmNew 同款）。
        state.doc.replace_song(nbs_core::model::Song::new());
        state.record = true;
        state.record_armed = false;
        // 造一个正在播放的时钟：先建事件表再起播。
        state.prepare_audio();
        state.begin_playback();
        assert!(state.is_playing(), "SilentEngine 时钟应已起播");
        backend.push(0x90, 61, 70);
        state.live_midi.pump();
        state.midi_input_set_key(40);
        let note = state.doc.song.get(0, 0);
        assert!(note.is_some(), "0 列 0 层应写入录音的音符");
        let note = note.unwrap();
        assert_eq!(note.key, 40);
        assert_eq!(note.velocity, 70, "录音带上 MIDI 力度");
        assert_eq!(note.instrument, state.current.instrument, "指派 -1 走当前音色");
        // 同一列 0 层已占，再按一格落在 1 层（原版 while (!add_block_manual) b+=1）。
        backend.push(0x90, 62, 60);
        state.live_midi.pump();
        state.midi_input_set_key(41);
        assert!(state.doc.song.get(0, 1).is_some(), "0 层占了落 1 层");
    }

    /// 录音待命：第一个 MIDI 按键起播（原版 `playing = 0.25 → toggle_playing`）。
    #[test]
    fn first_press_starts_playback_when_record_armed() {
        let (mut state, backend, _previews) = state_with_midi(1);
        state.doc.replace_song(nbs_core::model::Song::new());
        state.record = true;
        state.record_armed = true;
        backend.push(0x90, 61, 70);
        state.live_midi.pump();
        state.midi_input_set_key(40);
        assert!(!state.record_armed, "起播后待命态清掉");
        assert!(state.is_playing(), "待命下按键起播");
        assert!(state.doc.song.get(0, 0).is_some(), "第一个音落在播放起点");
    }

    /// 无声（-2）：不出声；原版此时仍会写块（拿到 -2 音色实例，行为未定），
    /// 我们表示不了 -2 音色，跳过写入（记账偏差）。
    #[test]
    fn none_assignment_is_silent_and_skips_recording() {
        let (mut state, backend, previews) = state_with_midi(1);
        state.doc.replace_song(nbs_core::model::Song::new());
        state.live_midi.set_device(0);
        state.live_midi.assignments[0] = -2;
        state.record = true;
        state.record_armed = false;
        state.prepare_audio();
        state.begin_playback();
        backend.push(0x90, 61, 70);
        state.live_midi.pump();
        let before = previews_of(&previews).len();
        state.midi_input_set_key(40);
        assert_eq!(previews_of(&previews).len(), before, "无声指派不出声");
        assert!(state.doc.song.get(0, 0).is_none(), "无声指派不写块");
        assert!(state.live_midi.key_midipress[40], "按住态照常维护");
    }
}
