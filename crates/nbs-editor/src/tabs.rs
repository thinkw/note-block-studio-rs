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

use nbs_core::Document;
use slotmap::{new_key_type, SlotMap};

new_key_type! {
    /// 一个歌曲标签的身份，等价于原版 `obj_song` 的实例引用。
    /// 关掉标签后这个 id 就作废，不会被复用。
    pub struct SongId;
}

/// 一首歌在标签条上的一份记录。
///
/// `doc` 是 `Option`：台前那份被 `EditorState.doc` 持有，对应槽位留 `None`。
/// 原版挂在 `obj_song` 上的其余每标签标量（`marker_*`、`section_*`、`sb_sel`、
/// 画笔乐器、滚动位置……）在刀 1 逐个搬进来，本刀先只有文档与身份。
pub struct SongTab {
    /// 缓存身份，全局单调、永不复用。见 [`crate::state::EditorState::song_session`]。
    pub session: u64,
    /// `None` = 已取出到台前。
    pub doc: Option<Document>,
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
    pub fn insert(&mut self, doc: Document) -> SongId {
        let session = self.next_session();
        let id = self.slots.insert(SongTab {
            session,
            doc: Some(doc),
        });
        self.order.push(id);
        id
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
}
