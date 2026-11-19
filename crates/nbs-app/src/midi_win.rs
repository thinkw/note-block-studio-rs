//! winmm 实时 MIDI 输入后端，对应原版 GMSSimpleMIDI 扩展里的 rtmidi 一族
//! （`rtmidi_init` / `midi_input_devices` / `rtmidi_set_inport` /
//! `rtmidi_check_message` / `rtmidi_get_message`）。消息由 winmm 的回调线程
//! 推进共享队列，编辑器每帧取走（原版 control_step 的泵）。

use std::sync::{Arc, Mutex};

use nbs_editor::live_midi::{MidiInBackend, RawMidiMsg};

/// `MIDIINCAPSW`（mmdeviceapi 的 winmm 部分）：名称最长 31 个 UTF-16 码元 + NUL。
#[repr(C)]
struct MidiInCapsW {
    manufacturer_id: u16,
    product_id: u16,
    driver_version: u32,
    name: [u16; 32],
    support: u32,
}

#[link(name = "winmm")]
extern "system" {
    fn midiInGetNumDevs() -> u32;
    fn midiInGetDevCapsW(devid: u32, caps: *mut MidiInCapsW, size: u32) -> u32;
    fn midiInOpen(
        handle: *mut usize,
        devid: usize,
        callback: usize,
        instance: usize,
        flags: u32,
    ) -> u32;
    fn midiInStart(handle: usize) -> u32;
    fn midiInStop(handle: usize) -> u32;
    fn midiInClose(handle: usize) -> u32;
}

/// `MIM_DATA`：`dwParam1` 低三字节依次是状态、data1、data2。
const MIM_DATA: u32 = 0x03C0;
/// `CALLBACK_FUNCTION`：winmm 在自己的线程上回调。
const CALLBACK_FUNCTION: u32 = 0x0003_0000;

/// winmm 后端。同一时刻只开一个输入口（原版 `rtmidi_set_inport` 的语义），
/// 回调把 MIM_DATA 解包后塞进共享队列。
pub struct WinMidiIn {
    inbox: Arc<Mutex<Vec<RawMidiMsg>>>,
    /// 打开的输入口句柄（0 = 没开）。
    handle: Mutex<usize>,
    /// 传给回调的堆指针（`Box<Arc<Mutex<Vec<RawMidiMsg>>>>`），关闭时回收。
    instance: Mutex<usize>,
}

impl WinMidiIn {
    pub fn new() -> Self {
        Self {
            inbox: Arc::new(Mutex::new(Vec::new())),
            handle: Mutex::new(0),
            instance: Mutex::new(0),
        }
    }
}

impl Default for WinMidiIn {
    fn default() -> Self {
        Self::new()
    }
}

extern "system" fn on_message(_handle: usize, msg: u32, instance: usize, p1: u32, _p2: u32) {
    if msg != MIM_DATA {
        return;
    }
    // 回调线程上绝不 panic：锁被毒了就丢这条消息。
    let queue = unsafe { &*(instance as *const Arc<Mutex<Vec<RawMidiMsg>>>) };
    if let Ok(mut q) = queue.lock() {
        q.push(RawMidiMsg {
            status: p1 as u8,
            data1: (p1 >> 8) as u8,
            data2: (p1 >> 16) as u8,
        });
    }
}

impl MidiInBackend for WinMidiIn {
    fn devices(&self) -> Vec<String> {
        let count = unsafe { midiInGetNumDevs() };
        (0..count)
            .filter_map(|i| {
                // SAFETY：caps 结构按 winmm 要求的尺寸传入，名称在返回的缓冲里。
                let mut caps: MidiInCapsW = unsafe { std::mem::zeroed() };
                let rc = unsafe {
                    midiInGetDevCapsW(i, &mut caps, std::mem::size_of::<MidiInCapsW>() as u32)
                };
                if rc != 0 {
                    return None;
                }
                let len = caps
                    .name
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(caps.name.len());
                let name = String::from_utf16_lossy(&caps.name[..len]);
                if name.trim().is_empty() {
                    None
                } else {
                    Some(name)
                }
            })
            .collect()
    }

    fn open(&self, index: usize) -> bool {
        if index >= self.devices().len() {
            // 原版 rtmidi_set_inport 对不存在的口返回失败。
            return false;
        }
        self.close();
        let instance = Box::into_raw(Box::new(Arc::clone(&self.inbox))) as usize;
        let mut handle = 0usize;
        let rc = unsafe {
            midiInOpen(
                &mut handle,
                index,
                on_message as extern "system" fn(usize, u32, usize, u32, u32) as usize,
                instance,
                CALLBACK_FUNCTION,
            )
        };
        if rc != 0 {
            drop(unsafe { Box::from_raw(instance as *mut Arc<Mutex<Vec<RawMidiMsg>>>) });
            return false;
        }
        unsafe { midiInStart(handle) };
        *self.handle.lock().unwrap_or_else(|e| e.into_inner()) = handle;
        *self.instance.lock().unwrap_or_else(|e| e.into_inner()) = instance;
        true
    }

    fn close(&self) {
        let mut handle = self.handle.lock().unwrap_or_else(|e| e.into_inner());
        if *handle != 0 {
            unsafe {
                midiInStop(*handle);
                midiInClose(*handle);
            }
            *handle = 0;
            let instance = *self.instance.lock().unwrap_or_else(|e| e.into_inner());
            if instance != 0 {
                drop(unsafe { Box::from_raw(instance as *mut Arc<Mutex<Vec<RawMidiMsg>>>) });
            }
            *self.instance.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        }
    }

    fn take_messages(&self) -> Vec<RawMidiMsg> {
        std::mem::take(&mut self.inbox.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for WinMidiIn {
    fn drop(&mut self) {
        self.close();
    }
}
