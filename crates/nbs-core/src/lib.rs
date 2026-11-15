//! `nbs-core` — Open Note Block Studio 重写版的数据与编辑内核。
//!
//! 模块地图（对应另一份分析里的 A / D 分层）：
//! - [`model`]      稀疏网格 `Song` / `Note` / `Layer`（A 数据层 + D 的数据基础）
//! - [`nbs`]        `.nbs` v0..v5 二进制编解码（A 数据层）
//! - [`selection`]  浮动选区与 OpenNBS 兼容的 RLE 编码（D）
//! - [`history`]    紧凑 diff 撤销/重做栈（D）
//! - [`document`]   所有编辑命令的事务封装（D）
//! - [`instruments`] 20 种内置乐器与方块配色（D）
//!
//! 设计要点：渲染只读 [`model::Song::notes_in_view`]，它基于 `BTreeMap::range`
//! 只遍历视口内的列，绝不扫描全曲 —— 这是长曲子不掉帧的硬性保证。

pub mod document;
pub mod history;
pub mod instruments;
pub mod macros;
pub mod model;
pub mod nbs;
pub mod selection;

pub use document::Document;
pub use history::{Edit, History};
pub use macros::{transform as transform_notes, Macro, TremoloFade};
pub use model::{
    key_name, key_offset, Column, CustomInstrument, Layer, LockMode, Note, Song, SongMeta,
    FIRST_CUSTOM_INDEX, KEY_MAX, KEY_MIN, MC_KEY_MAX, MC_KEY_MIN, PAN_CENTER, PAN_MAX, PIT_MAX,
    VEL_MAX,
};
pub use selection::{Selection, SelectionNote};
