//! 歌曲标签页的槽位表（刀 0）。
//!
//! 原版的多开是一张全局数组 `global.songs[]`，里面装的是 `obj_song` 实例，
//! 再加一个标量 `global.song` 指出「当前是哪首」（取证见
//! `.work/songtabs-20261123/gml/set_song.gml`、`open_song_nbs.gml`）。
//! 端口对应到这个类型：`SongStore` 就是那张数组，`SongId` 是下标，
//! `active` 就是 `global.song`。
//!
//! 用槽位表（generational arena）而不是 `Vec<SongTab>`：原版重排标签是
//! `array_delete` + `array_insert`，下标会跟着挪，而 MCP 那边要长期持有引用。
//! 这里把「身份」和「位置」分开——`SongId` 只在标签被真正关掉时失效
//! （MCP 侧返回 `tab_gone`），拖拽重排只动 [`SongStore::order`]。
//!
//! 本刀只做状态拆层，不画任何界面，用户看不出差别：台前仍然是
//! `EditorState.doc` 那一份 `Document`，槽位表里当前标签的 `doc` 恒为 `None`
//! （表示「已取出到台前」）。这个形状同时防住「同一个文档被两处同时持有」，
//! 取用只能走 [`SongStore::lift`] / [`SongStore::swap_active`] 这两条会挪动
//! 所有权的入口。

use std::path::PathBuf;

use nbs_core::model::Note;
use nbs_core::Document;
use slotmap::{new_key_type, SlotMap};

new_key_type! {
    /// 一个歌曲标签的身份，等价于原版 `obj_song` 的实例引用。
    /// 关掉标签后这个 id 就作废，不会被复用。
    pub struct SongId;
}

/// 每标签一份的界面状态（刀 1）。
///
/// 字段逐条对照原版 `obj_song/Create_0.gml`：`instrument`、`starta`/`startb`、
/// `marker_pos`、`section_exists`/`section_start`/`section_end`、`insbox_start`、
/// `midifile`（取证见 `.work/songtabs-20261123/gml/obj_song_Create_0.gml`）。
/// 原版把这些直接写在 `current_song` 上，端口集中到切标签那一刻收/装一次。
///
/// 刻意做成一个独立结构、而不是散进 [`SongTab`]：收和装这两端都走解构赋值
/// （`state.rs` 的 `TabUi::grab` / `TabUi::restore`），以后往这里加字段会在
/// 两端同时编译不过，不会只改一半（用户要求 #131）。
#[derive(Clone, Debug, Default)]
pub struct TabUi {
    /// 原版 `instrument`：画笔当前乐器（含 key/vel/pan/pit 默认值）。
    pub brush: Note,
    /// 原版 `starta`：视口左上角那一列。界面缩放是全局的，滚动位置不是。
    pub scroll_tick: u32,
    /// 原版 `startb`：视口左上角那一层。
    pub scroll_layer: u32,
    /// 原版 `marker_pos`：播放头所在刻。端口真身在播放时钟里，切标签时收/装。
    pub playhead: f64,
    /// 原版 `section_exists`。
    pub section_exists: bool,
    /// 原版 `section_start`。
    pub section_start: i64,
    /// 原版 `section_end`。
    pub section_end: i64,
    /// 原版 `insbox_start`：乐器面板滚到第几个色块。写它的那几行用的是
    /// `current_song.insbox_start`（`control_draw.gml:2007-2012`），所以每首一份。
    pub insbox_start: usize,
    /// 原版 `midifile`：MIDI 导入窗左下角显示的那个文件名。
    pub midi_file: String,
}

/// 原版 `obj_song` 上的参考音频配置；不写进 `.nbs`，只随本次会话的标签保留。
#[derive(Clone, Debug)]
pub struct ReferenceConfig {
    pub path: Option<PathBuf>,
    /// 0 静音 / 1 独奏 / 2 正常。
    pub option: u8,
    pub offset_ms: i64,
    pub volume: u8,
}

impl Default for ReferenceConfig {
    fn default() -> Self {
        Self {
            path: None,
            option: 2,
            offset_ms: 0,
            volume: 100,
        }
    }
}

/// 一首歌在标签条上的一份记录。
///
/// `doc` 是 `Option`：台前那份被 `EditorState.doc` 持有，对应槽位留 `None`。
pub struct SongTab {
    /// 缓存身份，全局单调、永不复用。见 [`crate::state::EditorState::song_session`]。
    pub session: u64,
    /// `None` = 已取出到台前。
    pub doc: Option<Document>,
    /// 这一首的界面状态；台前那份在 `EditorState` 上，收装时机见
    /// [`crate::state::EditorState::switch_tab`]。
    pub ui: TabUi,
    /// 原版 `song_backup_path`：只删这一首确实写过的恢复文件，不凭文件名认领别人的备份。
    pub backup_path: Option<PathBuf>,
    pub reference: ReferenceConfig,
    /// 原版 `tab_draw_x` / `tab_target_x` / `tab_from_x` / `tab_progress`。
    pub tab_draw_x: f32,
    pub tab_target_x: f32,
    pub tab_from_x: f32,
    pub tab_progress: f32,
}

/// [`SongStore::close`] 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed {
    /// 关掉之后站在台前的标签；表被关空了是 `None`。
    pub active: Option<SongId>,
    /// 关的是不是台前那一个（决定调用方要不要重挂音频、清选区）。
    pub was_active: bool,
}

/// 标签槽位表：原版 `global.songs[]` + `global.song`。
pub struct SongStore {
    slots: SlotMap<SongId, SongTab>,
    /// 标签条顺序，等价于原版 `songs[]` 的数组序。重排只动这里。
    order: Vec<SongId>,
    /// 台前那个标签（原版 `global.song`）。
    active: Option<SongId>,
    /// 代次计数器：只增不减，标签关掉也不回收号码。
    next_session: u64,
}

impl Default for SongStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SongStore {
    pub fn new() -> Self {
        Self {
            slots: SlotMap::with_key(),
            order: Vec::new(),
            active: None,
            next_session: 0,
        }
    }

    fn next_session(&mut self) -> u64 {
        self.next_session += 1;
        self.next_session
    }

    /// 新增一个标签槽（后台，不动台前）。id 在标签关掉之前一直有效。
    /// 界面状态取默认值：原版 `new_song()` / `open_song_nbs.gml` 建的都是
    /// 刚 `create(obj_song)` 出来的那份出厂值。
    pub fn insert(&mut self, doc: Document) -> SongId {
        self.insert_at(self.order.len(), doc)
    }

    /// 在原版 `array_insert(songs, index, create(obj_song))` 的位置插入一个标签。
    pub fn insert_at(&mut self, index: usize, doc: Document) -> SongId {
        let session = self.next_session();
        let id = self.slots.insert(SongTab {
            session,
            doc: Some(doc),
            ui: TabUi::default(),
            backup_path: None,
            reference: ReferenceConfig::default(),
            tab_draw_x: -1.0,
            tab_target_x: -1.0,
            tab_from_x: -1.0,
            tab_progress: 1.0,
        });
        self.order.insert(index.min(self.order.len()), id);
        id
    }

    /// 存某个标签的界面状态（台前那份收进槽里）。标签不存在返回 `false`。
    pub fn set_ui(&mut self, id: SongId, ui: TabUi) -> bool {
        match self.slots.get_mut(id) {
            Some(tab) => {
                tab.ui = ui;
                true
            }
            None => false,
        }
    }

    /// 读某个标签的界面状态。
    pub fn ui(&self, id: SongId) -> Option<&TabUi> {
        self.slots.get(id).map(|t| &t.ui)
    }

    /// 记下这一首最近一次成功写出的备份路径；失败时保留上一份。
    pub fn set_backup_path(&mut self, id: SongId, path: Option<PathBuf>) -> bool {
        match self.slots.get_mut(id) {
            Some(tab) => {
                tab.backup_path = path;
                true
            }
            None => false,
        }
    }

    /// 读这一首拥有的备份路径；标签不存在或从未备份时返回 `None`。
    pub fn backup_path(&self, id: SongId) -> Option<&PathBuf> {
        self.slots.get(id).and_then(|t| t.backup_path.as_ref())
    }

    pub fn set_reference(&mut self, id: SongId, config: ReferenceConfig) -> bool {
        match self.slots.get_mut(id) {
            Some(tab) => {
                tab.reference = config;
                true
            }
            None => false,
        }
    }

    pub fn reference(&self, id: SongId) -> Option<&ReferenceConfig> {
        self.slots.get(id).map(|tab| &tab.reference)
    }

    /// 原版标签条让位动画：目标槽位变化时重设起点，按三次缓出推进。
    pub fn advance_tab_layout(
        &mut self,
        id: SongId,
        target: f32,
        elapsed: f32,
        duration: f32,
    ) -> f32 {
        let Some(tab) = self.slots.get_mut(id) else {
            return target;
        };
        if tab.tab_draw_x < 0.0 {
            tab.tab_draw_x = target;
            tab.tab_target_x = target;
            tab.tab_from_x = target;
            tab.tab_progress = 1.0;
        }
        if tab.tab_target_x != target {
            tab.tab_from_x = tab.tab_draw_x;
            tab.tab_target_x = target;
            tab.tab_progress = 0.0;
        }
        if tab.tab_progress < 1.0 {
            tab.tab_progress = (tab.tab_progress + elapsed / duration.max(0.001)).min(1.0);
            let eased = 1.0 - (1.0 - tab.tab_progress).powi(3);
            tab.tab_draw_x = tab.tab_from_x + (target - tab.tab_from_x) * eased;
        }
        tab.tab_draw_x
    }

    /// 标签条隐藏或重排前，把动画位置清回哨兵，下一次显示重新铺槽。
    pub fn reset_tab_layout(&mut self) {
        for tab in self.slots.values_mut() {
            tab.tab_draw_x = -1.0;
            tab.tab_target_x = -1.0;
            tab.tab_from_x = -1.0;
            tab.tab_progress = 1.0;
        }
    }

    /// 把某个标签的文档取到台前（原版 `set_song` 的「装入」那一半）。
    ///
    /// 返回 `None` 表示这次取用会出事，且不动任何状态：
    /// - 台上还有一份没写回（当前槽 `doc` 是 `None`）——取走会把正在编辑的文档丢在半路；
    /// - 标签不存在，或它的文档本来就在台前。
    pub fn lift(&mut self, id: SongId) -> Option<Document> {
        if let Some(prev) = self.active {
            if self.slots[prev].doc.is_none() {
                return None;
            }
        }
        let tab = self.slots.get_mut(id)?;
        let doc = tab.doc.take()?;
        self.active = Some(id);
        Some(doc)
    }

    /// 切标签：台前这份文档写回它自己的槽位，目标标签的文档换到台前。
    ///
    /// 一次调用完成，中间不留「两处都没有文档」的窗口；失败返回 `false` 且
    /// `current` 原样不动。台上没东西可写回（`active` 为空）时不走这里，
    /// 那是 [`Self::lift`] 的场景。
    pub fn swap_active(&mut self, id: SongId, current: &mut Document) -> bool {
        if self.active == Some(id) {
            return false;
        }
        let Some(prev) = self.active else {
            return false;
        };
        if !self.slots.contains_key(id) || self.slots[id].doc.is_none() {
            return false;
        }
        let incoming = self.slots[id].doc.take().expect("上面刚查过是 Some");
        self.slots[prev].doc = Some(std::mem::replace(current, incoming));
        self.active = Some(id);
        true
    }

    /// 关掉一个标签：槽位与标签条顺序拿掉，返回关掉之后台前是哪一首。
    ///
    /// 只搬状态，不问保存、不删备份文件——那是 [`crate::state::EditorState::close_tab`]
    /// 和退出流程的事（原版把这两件事都放在 `close_song` 的前半段，取证见
    /// `.work/songtabs-20261123/gml/close_song.gml`）。
    ///
    /// 关后台标签不动台前（id 稳定，这正是槽位表相对数组的好处）；关台前标签时
    /// `stage` 就地换成邻居的文档，邻居按原版的下标规则挑：
    /// `if (song > sid || song >= array_length(songs)) song--`（`close_song.gml:24`）——
    /// 落在被关标签后面那一首，关的是最后一个就退到新的最后一个。表被关空了
    /// 就把 `stage` 换成一首空曲并返回 `active: None`，由调用方决定补不补标签
    /// （原版交互路径 `close_song(sid)` 和退出路径 `close_song(i, 1, 1)` 的
    /// `!nonew || noconfirm` 都成立；只有欢迎窗替换首标签的 `close_song(0, 1)` 不补）。
    /// id 不存在时不动任何状态。
    pub fn close(&mut self, id: SongId, stage: &mut Document) -> Closed {
        let Some(idx) = self.index_of(id) else {
            return Closed {
                active: self.active,
                was_active: false,
            };
        };
        let was_active = self.active == Some(id);
        let active_id = self.active;
        self.slots.remove(id);
        self.order.remove(idx);
        if !was_active {
            // 台前那份还在台上（它自己的槽位是 `None`），不能去 lift 它。
            return Closed {
                active: active_id,
                was_active,
            };
        }
        let next = if self.order.is_empty() {
            None
        } else {
            self.order.get(idx.min(self.order.len() - 1)).copied()
        };
        self.active = None;
        *stage = match next {
            Some(n) => self.lift(n).expect("刚关掉的不是它，槽里的文档一定取得到"),
            None => Document::new(),
        };
        Closed {
            active: next,
            was_active,
        }
    }

    /// 台前标签的 id（原版 `global.song`，没有标签时 `None`）。
    pub fn active(&self) -> Option<SongId> {
        self.active
    }

    /// 标签条顺序（原版 `songs[]` 下标序），刀 2 画标签条时按这个遍历。
    pub fn order(&self) -> &[SongId] {
        &self.order
    }

    /// 标签在标签条上的位置（原版 `global.song` 那个下标）。
    pub fn index_of(&self, id: SongId) -> Option<usize> {
        self.order.iter().position(|&x| x == id)
    }

    /// 原版拖拽松手后的 `array_delete` + `array_insert`：只动顺序，不动 id、文档和台前选中项。
    pub fn reorder(&mut self, id: SongId, target: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let target = target.min(self.order.len().saturating_sub(1));
        if from == target {
            return true;
        }
        let moved = self.order.remove(from);
        self.order.insert(target, moved);
        true
    }

    pub fn contains(&self, id: SongId) -> bool {
        self.slots.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// 某个标签的缓存身份；标签已关掉返回 `None`。
    pub fn session(&self, id: SongId) -> Option<u64> {
        self.slots.get(id).map(|t| t.session)
    }

    /// 台前的缓存身份，等价于原来的 `EditorState::song_session` 读值。
    /// 一个标签都没有时返回 0——发号从 1 开始，0 不会撞上任何真实身份。
    pub fn active_session(&self) -> u64 {
        self.active.and_then(|id| self.session(id)).unwrap_or(0)
    }

    /// 读一个后台标签的文档（刀 5 的 MCP 跨标签读走这里）。
    /// 台前的那份不在槽里，得读 `EditorState::doc`。
    pub fn parked_doc(&self, id: SongId) -> Option<&Document> {
        self.slots.get(id).and_then(|t| t.doc.as_ref())
    }

    /// 内容被整首替换（新建 / 载入 / 导入）时给台前标签换一个新的缓存身份。
    /// 没有台前标签时什么都不做，返回 `None`，也不推进计数器。
    ///
    /// 原版每首曲子是一个独立实例，`string(o.songs[o.song])` 天然不同；
    /// 端口在同一个标签里换歌，得手动发号，否则按 `rev` 缓存的派生态
    /// 会把上一首的结果认成这一首的（#116 那次徽标滞后的根因）。
    pub fn reissue_session(&mut self) -> Option<u64> {
        let id = self.active?;
        let session = self.next_session();
        self.slots.get_mut(id)?.session = session;
        Some(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbs_core::model::Note;
    use slotmap::Key as _;

    #[test]
    fn tab_layout_animates_and_resets() {
        let mut store = SongStore::new();
        let a = store.insert(Document::new());
        let b = store.insert(Document::new());
        let _ = store.lift(a);
        assert_eq!(store.advance_tab_layout(a, 7.0, 0.016, 0.18), 7.0);
        assert_eq!(store.advance_tab_layout(b, 186.0, 0.016, 0.18), 186.0);
        let moved = store.advance_tab_layout(a, 100.0, 0.09, 0.18);
        assert!(moved > 7.0 && moved < 100.0, "让位动画应处于中间: {moved}");
        assert_eq!(store.advance_tab_layout(a, 100.0, 0.09, 0.18), 100.0);
        store.reset_tab_layout();
        assert_eq!(store.advance_tab_layout(a, 7.0, 0.016, 0.18), 7.0);
    }

    #[test]
    fn sessions_are_unique_and_never_reused() {
        let mut store = SongStore::new();
        let a = store.insert(Document::new());
        let b = store.insert(Document::new());
        assert_eq!(store.session(a), Some(1));
        assert_eq!(store.session(b), Some(2));
        // 换号是对「台前那一首」做的，先把它取到台上。
        store.lift(a).unwrap();
        assert_eq!(store.active_session(), 1);
        assert_eq!(store.reissue_session(), Some(3));
        assert_eq!(store.active_session(), 3);
        assert_eq!(store.session(a), Some(3));
        // 旧号不会被别的标签撞上：代次只增不减。
        assert_eq!(store.session(b), Some(2));
        // 没有台前标签时不发号（原版 `global.song` 无值）。
        let mut empty = SongStore::new();
        assert_eq!(empty.reissue_session(), None);
        assert_eq!(empty.active_session(), 0);
    }

    #[test]
    fn first_lift_needs_no_parked_doc() {
        let mut store = SongStore::new();
        let id = store.insert(Document::new());
        assert_eq!(store.active(), None);
        store.lift(id).expect("启动时台上是空的，取得到");
        assert_eq!(store.active(), Some(id));
        assert!(store.parked_doc(id).is_none());
        // 已在台前的标签不能再取一次（防双持有）。
        assert!(store.lift(id).is_none());
    }

    #[test]
    fn swap_moves_docs_and_refuses_to_lose_one() {
        let mut store = SongStore::new();
        let mut a = Document::new();
        a.place_note(3, 0, Note::new(0, 45));
        let mut b = Document::new();
        b.place_note(9, 1, Note::new(2, 60));
        let ta = store.insert(a);
        let tb = store.insert(b);
        let mut current = store.lift(ta).unwrap();
        assert_eq!(current.song.get(3, 0), Some(Note::new(0, 45)));

        // 台上这份还没进槽，切到 id 相同的目标会被拒；切到别的前必须先 swap。
        assert!(!store.swap_active(ta, &mut current));
        assert!(store.swap_active(tb, &mut current));
        assert_eq!(store.active(), Some(tb));
        assert_eq!(current.song.get(9, 1), Some(Note::new(2, 60)));
        // 上一首整份留在自己的槽里，没丢。
        assert_eq!(
            store.parked_doc(ta).unwrap().song.get(3, 0),
            Some(Note::new(0, 45))
        );

        assert!(store.swap_active(ta, &mut current));
        assert_eq!(store.active(), Some(ta));
        assert_eq!(current.song.get(3, 0), Some(Note::new(0, 45)));
        assert_eq!(
            store.parked_doc(tb).unwrap().song.get(9, 1),
            Some(Note::new(2, 60))
        );
    }

    #[test]
    fn unknown_or_closed_id_is_refused() {
        let mut store = SongStore::new();
        let kept = store.insert(Document::new());
        let dropped = store.insert(Document::new());
        let mut current = store.lift(kept).unwrap();
        // 模拟关标签：槽位拿掉，id 立刻作废且不会被新标签复用。
        let closed = store.slots.remove(dropped).unwrap();
        store.order.retain(|&x| x != dropped);
        assert!(!store.contains(dropped));
        assert!(!store.swap_active(dropped, &mut current));
        assert_eq!(store.active(), Some(kept));
        assert_eq!(store.len(), 1);
        assert_eq!(store.session(kept), Some(1));
        // 关掉的那个号不会被新标签复用：槽位下标可以回收，代次不一样。
        let fresh = store.insert(Document::new());
        assert!(!store.contains(dropped));
        assert!(store.session(fresh).unwrap() > closed.session);
    }

    #[test]
    fn order_is_the_tab_strip_and_survives_reindex() {
        let mut store = SongStore::new();
        let a = store.insert(Document::new());
        let b = store.insert(Document::new());
        let c = store.insert(Document::new());
        assert_eq!(store.index_of(b), Some(1));
        // 刀 3 的拖拽重排只改 order：身份与缓存号都不动。
        let session_b = store.session(b);
        store.order.remove(1);
        store.order.insert(0, b);
        assert_eq!(store.order(), &[b, a, c]);
        assert_eq!(store.index_of(b), Some(0));
        assert_eq!(store.session(b), session_b);
    }

    /// 造一首只在 (tick, layer) 有一格的曲子，用来确认「台前摆的是哪一首」。
    fn marked(tick: u32, layer: u32) -> Document {
        let mut doc = Document::new();
        doc.place_note(tick, layer, Note::new(0, 45));
        doc
    }

    fn note_at(doc: &Document, tick: u32, layer: u32) -> bool {
        doc.song.get(tick, layer) == Some(Note::new(0, 45))
    }

    #[test]
    fn tab_ui_roundtrips_through_the_slot() {
        let mut store = SongStore::new();
        let a = store.insert(marked(1, 0));
        let b = store.insert(marked(2, 0));
        // 出厂值 = 原版 obj_song Create 里那份（画笔第 0 号乐器、C4、滚到左上）。
        let fresh = store.ui(a).unwrap();
        assert_eq!(fresh.brush, Note::default());
        assert_eq!((fresh.scroll_tick, fresh.scroll_layer), (0, 0));
        assert_eq!(fresh.playhead, 0.0);
        assert!(!fresh.section_exists);
        assert!(fresh.midi_file.is_empty());

        let mut stage = store.lift(a).unwrap();
        let mut stage_ui = TabUi {
            brush: Note::new(7, 60),
            scroll_tick: 40,
            scroll_layer: 3,
            playhead: 12.5,
            section_exists: true,
            section_start: 4,
            section_end: 9,
            insbox_start: 21,
            midi_file: "song.mid".into(),
        };
        // 切到 b：a 的那份存回槽，b 的默认值装上台。
        assert!(store.swap_active(b, &mut stage));
        store.set_ui(a, std::mem::take(&mut stage_ui));
        let on_b = store.ui(b).unwrap().clone();
        assert_eq!((on_b.scroll_tick, on_b.scroll_layer), (0, 0));
        assert_eq!(on_b.brush, Note::default());
        // 切回 a：原样回来，一格不差。
        assert!(store.swap_active(a, &mut stage));
        store.set_ui(b, on_b);
        let back = store.ui(a).unwrap().clone();
        assert_eq!(back.brush, Note::new(7, 60));
        assert_eq!((back.scroll_tick, back.scroll_layer), (40, 3));
        assert_eq!(back.playhead, 12.5);
        assert_eq!(
            (back.section_exists, back.section_start, back.section_end),
            (true, 4, 9)
        );
        assert_eq!(back.insbox_start, 21);
        assert_eq!(back.midi_file, "song.mid");
        // 不存在的 id 既存不进也读不出。
        let ghost = SongId::null();
        assert!(!store.set_ui(ghost, back.clone()));
        assert!(store.ui(ghost).is_none());
    }

    #[test]
    fn reorder_moves_only_the_slot_and_keeps_the_active_id() {
        let mut store = SongStore::new();
        let a = store.insert(marked(1, 0));
        let b = store.insert(marked(2, 0));
        let c = store.insert(marked(3, 0));
        let stage = store.lift(a).unwrap();
        assert!(store.reorder(a, 2));
        assert_eq!(store.order(), &[b, c, a]);
        assert_eq!(store.active(), Some(a));
        assert!(note_at(&stage, 1, 0));
        assert!(note_at(store.parked_doc(b).unwrap(), 2, 0));
        assert!(note_at(store.parked_doc(c).unwrap(), 3, 0));
        assert!(store.reorder(c, 0));
        assert_eq!(store.order(), &[c, b, a]);
        assert!(store.reorder(a, 2), "拖回自己的位置也算成功");
        assert_eq!(store.order(), &[c, b, a]);
        assert!(!store.reorder(SongId::null(), 0));
    }

    #[test]
    fn closing_a_background_tab_leaves_the_stage_alone() {
        let mut store = SongStore::new();
        let a = store.insert(marked(1, 0));
        let b = store.insert(marked(2, 0));
        let c = store.insert(marked(3, 0));
        let mut stage = store.lift(a).unwrap();
        let out = store.close(b, &mut stage);
        assert_eq!(
            out,
            Closed {
                active: Some(a),
                was_active: false
            }
        );
        // 台前完全没动，顺序里少了中间那首。
        assert!(note_at(&stage, 1, 0));
        assert_eq!(store.order(), &[a, c]);
        assert_eq!(store.len(), 2);
        assert!(store.parked_doc(c).is_some());
        assert!(!store.contains(b));
        // a 的缓存身份不受影响（关别人的号不会顶掉自己）。
        assert_eq!(store.session(a), Some(1));
        assert_eq!(store.active_session(), 1);
    }

    #[test]
    fn closing_the_active_tab_lands_like_the_original() {
        let mut store = SongStore::new();
        let a = store.insert(marked(1, 0));
        let b = store.insert(marked(2, 0));
        let c = store.insert(marked(3, 0));
        let mut stage = store.lift(b).unwrap();
        // 三首、台前是中间那首：关掉它落在「原来后面那一首」（close_song 的 song 不变 = 新下标 1）。
        let out = store.close(b, &mut stage);
        assert_eq!(
            out,
            Closed {
                active: Some(c),
                was_active: true
            }
        );
        assert!(note_at(&stage, 3, 0));
        assert_eq!(store.active_session(), 3);
        // 排在被关那首前面的不受影响，文档还留在自己槽里。
        assert!(store.parked_doc(a).is_some());

        // 关最后一个：下标越界要退一位，落在新的末尾。
        let d = store.insert(marked(4, 0));
        assert!(store.swap_active(d, &mut stage));
        let out = store.close(d, &mut stage);
        assert_eq!(
            out,
            Closed {
                active: Some(c),
                was_active: true
            }
        );
        assert!(note_at(&stage, 3, 0));

        // 再关倒数第二首：表里还剩最前面那首，落在它身上（下标退一位）。
        let out = store.close(c, &mut stage);
        assert_eq!(
            out,
            Closed {
                active: Some(a),
                was_active: true
            }
        );
        assert!(note_at(&stage, 1, 0));
        assert_eq!(store.order(), &[a]);

        // 关掉最后一首：表空，台前换成空曲。
        let out = store.close(a, &mut stage);
        assert_eq!(
            out,
            Closed {
                active: None,
                was_active: true
            }
        );
        assert!(store.is_empty());
        assert_eq!(store.active_session(), 0);
        assert_eq!(stage.song.total_blocks, 0);
        // 空表上再关同一个 id：不动状态。
        let out = store.close(a, &mut stage);
        assert_eq!(
            out,
            Closed {
                active: None,
                was_active: false
            }
        );
    }

    #[test]
    fn closed_id_stays_closed() {
        let mut store = SongStore::new();
        let a = store.insert(marked(1, 0));
        let b = store.insert(marked(2, 0));
        let mut stage = store.lift(a).unwrap();
        store.close(b, &mut stage);
        // MCP 长期持有的那个号：关掉的标签再也切不过去、也读不出来。
        assert!(!store.swap_active(b, &mut stage));
        assert!(store.lift(b).is_none());
        assert!(store.parked_doc(b).is_none());
        assert!(store.ui(b).is_none());
        // 新标签拿的是新号，不会复用被关掉那个。
        let fresh = store.insert(marked(5, 0));
        assert_eq!(store.session(fresh), Some(3));
    }
}
