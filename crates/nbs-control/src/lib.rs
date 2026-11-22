//! nbs-control — 让外部进程（MCP 桥、脚本）操控**正在运行的编辑器**。
//!
//! 分成三块，谁都不依赖 egui/eframe/cpal，这样协议可以单独测：
//!
//! ```text
//! op.rs        能力清单（显式参数，不吃鼠标 / 悬停 / 窗口草稿态）
//! outcome.rs   结构化回执：每个调用必带 rev / 会话号 / 变更量，失败必带错误码
//! bus.rs       UI 线程收件箱：别的线程投递请求，主线程逐帧取走并回执
//! clients.rs   连接看板：谁连着、最后一次调用距今多久
//! server.rs    127.0.0.1 回环服务：行分隔 JSON + token 握手
//! client.rs    桥接进程用的客户端
//! discovery.rs 端口 / token 的发现文件
//! task.rs      重活（导出、方案重建）的任务表：立即回句柄，之后轮询
//! midi.rs      MIDI 导入的入参与核对单
//! transcribe.rs 音频转 MIDI 的入参
//! ```
//!
//! 硬约束：本 crate 的日志**绝不写 stdout**（stdio MCP 会把 stdout 当协议流），
//! 一律走 stderr 或由调用方决定。

pub mod bus;
pub mod client;
pub mod clients;
pub mod discovery;
pub mod frame;
pub mod midi;
pub mod op;
pub mod outcome;
pub mod server;
pub mod task;
pub mod transcribe;

pub use bus::{Inbox, PendingRequest};
pub use client::{ClientError, ControlClient};
pub use clients::{ClientBoard, ClientSession};
pub use discovery::{Discovery, DISCOVERY_FILE};
pub use frame::{read_json_line, write_json_line, MAX_FRAME_BYTES};
pub use op::{
    BatchField, CellNote, ExportFormat, LayerSet, Op, Playback, Scope, SelectTarget, SetMeta,
};
pub use midi::{
    ImportChannelMap, ImportDrumMap, ImportEnhancedChannel, ImportEnhancedOptions, ImportMidiOptions,
    ImportRangeMode, ImportSlot, ImportSubstitution, MidiChannelPlan, MidiDrumPlan,
    MidiEnhancedReport, MidiPartPlan, MidiPlan, MidiTrackInfo,
};
pub use outcome::{
    ActionInfo, BarOverview, ChangeReport, Data, Diagnostics, ErrorCode, ErrorInfo, HistoryInfo,
    InstrumentInfo, JobInfo, JobStatus, LayerInfo, NoteCell, Outcome, SelectionInfo, Snapshot,
};
pub use transcribe::TranscribeOptions;
pub use server::{ControlServer, ServerConfig};
pub use task::{JobHandle, JobKind, JobRegistry};

/// 线协议版本：桥接进程与服务端不一致就直接拒绝，别拿半懂不懂的字段猜。
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests;
