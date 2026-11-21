//! `nbs-core` — Open Note Block Studio 重写版的数据与编辑内核。
//!
//! 模块地图（对应另一份分析里的 A / D 分层）：
//! - [`model`]      稀疏网格 `Song` / `Note` / `Layer`（A 数据层 + D 的数据基础）
//! - [`nbs`]        `.nbs` v0..v5 二进制编解码（A 数据层）
//! - [`selection`]  浮动选区与 OpenNBS 兼容的 RLE 编码（D）
//! - [`history`]    紧凑 diff 撤销/重做栈（D）
//! - [`document`]   所有编辑命令的事务封装（D）
//! - [`instruments`] 20 种内置乐器与方块配色（D）
//! - [`paths`]      配置目录这一条规则（编辑器和 MCP 桥必须算出同一个目录）
//!
//! 设计要点：渲染只读 [`model::Song::notes_in_view`]，它基于 `BTreeMap::range`
//! 只遍历视口内的列，绝不扫描全曲 —— 这是长曲子不掉帧的硬性保证。

pub mod document;
pub mod history;
pub mod instruments;
pub mod macros;
pub mod model;
pub mod nbs;
pub mod paths;
pub mod selection;

// ---- 多语言。原版 GML 只有一个全局 `language`，连最深层的脚本（读档报错、
// 历史标签、宏名）都直接读它。这里把同一份开关放在最底层的 nbs-core，
// 由 nbs-editor 每帧从 prefs 同步；nbs-io / nbs-audio 都用这里的 tr()。
//
// 注意：文案是在「生成那一刻」按当前语言定型的。历史记录标签、音频设备快照
// 这类存下来的字符串不会跟着语言下拉实时翻转（原版 language 也在启动时读
// settings 定一次），切换语言后新产生的内容才是新语言。
// 手上已经有语言值、又不想动全局的（比如演示曲说明），用 tr_at()。

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

/// 进程级的语言：只作为「新线程的初值」和 `set_language` 的落点，读都读线程本地那份。
static LANGUAGE: AtomicU8 = AtomicU8::new(0);

thread_local! {
    /// 本线程看到的语言。为什么要分线程：`show()` / `dispatch()` 每帧都要把开关
    /// 对齐到自己那份 prefs，如果这是唯一一个进程变量，并行跑的单测就会互相把对方
    /// 正在断言的语言翻走（同一份代码在 GUI 里只有一条 UI 线程，不存在这个问题）。
    /// 后台线程第一次读时拿到的是它开工那一刻的进程值，正是它该用的语言。
    static THREAD_LANG: Cell<u8> = const { Cell::new(u8::MAX) };
}

fn current_lang() -> u8 {
    THREAD_LANG.with(|c| {
        let v = c.get();
        if v == u8::MAX {
            // 还没钉过：继承进程值（= UI 线程最后一次同步的语言）。
            let inherited = LANGUAGE.load(Ordering::Relaxed);
            c.set(inherited);
            inherited
        } else {
            v
        }
    })
}

/// 等价原版读 `language`：0 = English，1 = 简体中文。
pub fn lang() -> u8 {
    current_lang()
}

/// 钉住本线程的语言，同时把它记为后续新线程的初值。
pub fn set_language(l: u8) {
    LANGUAGE.store(l, Ordering::Relaxed);
    THREAD_LANG.with(|c| c.set(l));
}

/// 等价原版 `condstr(language != 1, 英文, 中文)`。
pub fn tr<'a>(en: &'a str, zh: &'a str) -> &'a str {
    tr_at(lang(), en, zh)
}

/// `tr` 的显式语言版本。定型的文案（演示曲说明）用它按调用方手上的语言写，
/// 不要为了翻一句词去动全局开关——并行跑的单测会被串台。
pub fn tr_at<'a>(lang: u8, en: &'a str, zh: &'a str) -> &'a str {
    if lang == 1 {
        zh
    } else {
        en
    }
}

pub use document::Document;
pub use history::{Edit, History};
pub use macros::{parse_pattern, transform as transform_notes, Macro, TremoloFade};
pub use model::{
    key_name, panning_velocity_to_short, short_to_panning_velocity, Column, CustomInstrument,
    Layer, LockMode, Note, Song, SongMeta, FIRST_CUSTOM_INDEX, KEY_MAX, MC_KEY_MAX, MC_KEY_MIN,
    PAN_CENTER, PAN_MAX, PIT_MAX, SOUND_STOPPER_NAME, TEMPO_CHANGER_NAME, VEL_MAX,
};
pub use selection::{Selection, SelectionNote};
