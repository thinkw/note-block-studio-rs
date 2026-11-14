//! 文档 = 歌曲 + 浮动选区 + 历史 + 剪贴板。
//! 所有编辑命令都通过 `Tx`（事务）执行：事务声明要监视的单元格，
//! 命令随意改动歌曲/图层/元数据，提交时自动求出紧凑 diff 并压入历史。

use std::collections::BTreeSet;

use crate::history::{CellDelta, Edit, History, LayerDelta, MetaDelta};
use crate::model::{Layer, Note, Song, SongMeta};
use crate::selection::{collect_rect, Selection, SelectionNote, SelectionSnapshot};

pub struct Document {
    pub song: Song,
    pub selection: Selection,
    pub history: History,
    pub clipboard: Vec<SelectionNote>,
    pub clipboard_text: String,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Tx<'a> {
    doc: &'a mut Document,
    label: String,
    keys: BTreeSet<(u32, u32)>,
    before: Vec<(u32, u32, Option<Note>)>,
    layers_before: Option<Vec<Layer>>,
    meta_before: Option<SongMeta>,
    selection_before: SelectionSnapshot,
    track_sel: bool,
}

impl<'a> Tx<'a> {
    /// 声明一个会被改动的格子；重复声明自动去重。
    pub fn watch(&mut self, tick: u32, layer: u32) {
        if self.keys.insert((tick, layer)) {
            self.before
                .push((tick, layer, self.doc.song.get(tick, layer)));
        }
    }

    pub fn watch_notes(&mut self, notes: &[(u32, u32, Note)]) {
        for (t, l, _) in notes {
            self.watch(*t, *l);
        }
    }

    /// 只监视矩形内**已有音符**的格子（避免全选时遍历 10 万 × 100 个空格）。
    pub fn watch_rect_existing(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        if x1 < x0 || y1 < y0 {
            return;
        }
        let cells: Vec<(u32, u32, Note)> = self.doc.song.notes_in_rect(x0, y0, x1, y1);
        self.watch_notes(&cells);
    }

    pub fn watch_rect_existing_i(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        self.watch_rect_existing(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
    }

    pub fn track_layers(&mut self) {
        self.layers_before = Some(self.doc.song.layers.clone());
    }

    pub fn track_meta(&mut self) {
        self.meta_before = Some(self.doc.song.meta.clone());
    }

    pub fn track_selection(&mut self) {
        self.track_sel = true;
    }

    pub fn song(&mut self) -> &mut Song {
        &mut self.doc.song
    }

    pub fn read_song(&self) -> &Song {
        &self.doc.song
    }

    pub fn selection(&mut self) -> &mut Selection {
        &mut self.doc.selection
    }

    pub fn read_selection(&self) -> &Selection {
        &self.doc.selection
    }

    pub fn commit(self) {
        let cells: Vec<CellDelta> = self
            .before
            .iter()
            .map(|&(t, l, before)| CellDelta {
                tick: t,
                layer: l,
                before,
                after: self.doc.song.get(t, l),
            })
            .filter(|d| !d.is_noop())
            .collect();

        let layers = self.layers_before.map(|before| LayerDelta {
            before,
            after: self.doc.song.layers.clone(),
        });
        let meta = self.meta_before.map(|before| MetaDelta {
            before,
            after: self.doc.song.meta.clone(),
        });
        let selection = self.track_sel.then(|| {
            (self.selection_before.clone(), self.doc.selection.clone())
        });

        let edit = Edit {
            label: self.label,
            cells,
            layers,
            meta,
            selection,
        };
        if !edit.is_empty() {
            self.doc.song.rev += 1;
        }
        self.doc.history.push(edit);
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            song: Song::new(),
            selection: Selection::new(),
            history: History::default(),
            clipboard: Vec::new(),
            clipboard_text: String::new(),
        }
    }

    pub fn with_song(song: Song) -> Self {
        Self { song, ..Self::new() }
    }

    pub fn begin(&mut self, label: impl Into<String>) -> Tx<'_> {
        Tx {
            selection_before: self.selection.clone(),
            doc: self,
            label: label.into(),
            keys: BTreeSet::new(),
            before: Vec::new(),
            layers_before: None,
            meta_before: None,
            track_sel: false,
        }
    }

    // ---------------------------------------------------------------- 音符

    pub fn place_note(&mut self, tick: u32, layer: u32, note: Note) {
        let mut tx = self.begin("放置音符");
        tx.watch(tick, layer);
        tx.song().set(tick, layer, note.clamped());
        tx.commit();
    }

    pub fn remove_note(&mut self, tick: u32, layer: u32) {
        if self.song.get(tick, layer).is_none() {
            return;
        }
        let mut tx = self.begin("删除音符");
        tx.watch(tick, layer);
        tx.song().remove(tick, layer);
        tx.commit();
    }

    /// 点击已有音符：用当前画笔覆盖其属性。
    pub fn change_note(&mut self, tick: u32, layer: u32, note: Note) {
        let mut tx = self.begin("修改音符");
        tx.watch(tick, layer);
        tx.song().set(tick, layer, note.clamped());
        tx.commit();
    }

    pub fn clear_all_notes(&mut self) {
        if self.song.total_blocks == 0 {
            return;
        }
        let mut tx = self.begin("清空所有音符");
        let all: Vec<_> = tx.read_song().iter_notes().collect();
        tx.watch_notes(&all);
        for (t, l, _) in all {
            tx.song().remove(t, l);
        }
        tx.commit();
    }

    // ---------------------------------------------------------------- 浮动选区

    /// 把浮动选区落回歌曲（目标格原有音符会被覆盖）。
    pub fn place_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let mut tx = self.begin("落定选区");
        tx.track_selection();
        place_selection_in_tx(&mut tx);
        tx.commit();
    }

    pub fn deselect(&mut self) {
        self.place_selection();
    }

    /// 矩形框选。`copy = true` 时只复制不摘出（Alt 框选）。
    /// `additive = true` 时合并进已有选区（Ctrl 加选）。
    pub fn select_rect(
        &mut self,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        copy: bool,
        additive: bool,
        include_locked: bool,
    ) {
        let (ax, ay, bx, by) = (
            x0.min(x1),
            y0.min(y1),
            x0.max(x1).min(self.song.enda),
            y0.max(y1).min(self.song.endb.max(self.song.layer_count().saturating_sub(1))),
        );
        if ax > bx || ay > by {
            return;
        }
        let mut tx = self.begin(if copy { "复制框选" } else { "框选" });
        tx.track_selection();

        // 非加选时，先把旧浮动选区落回歌曲，保证新框选的坐标是对的。
        // 顺序与原版一致：selection_place() 之后再扫描矩形。
        if !additive && !tx.read_selection().is_empty() {
            place_selection_in_tx(&mut tx);
        }
        tx.watch_rect_existing(ax, ay, bx, by);
        let collected = collect_rect(tx.read_song(), ax, ay, bx, by, include_locked);

        if collected.is_empty() {
            // 空矩形：仅落定已有选区（等价「取消选择」），仍要提交事务。
            tx.commit();
            return;
        }

        if !copy {
            for (t, l, _) in &collected {
                tx.song().remove(*t, *l);
            }
        }
        tx.selection().merge_absolute(&collected);
        tx.commit();
    }

    pub fn select_point(&mut self, tick: u32, layer: u32, additive: bool) {
        self.select_rect(tick, layer, tick, layer, false, additive, false);
    }

    pub fn select_all(&mut self) {
        if self.song.total_blocks == 0 {
            return;
        }
        let enda = self.song.enda;
        let endb = self.song.endb;
        self.select_rect(0, 0, enda, endb, false, false, true);
    }

    /// 按乐器选择全部音符。`invert = true` 对应原版的「选择除该乐器之外的全部」。
    /// 语义与 select_rect 一致：选中即摘出为浮动选区。
    pub fn select_by_instrument(&mut self, instrument: u8, invert: bool) {
        let mut tx = self.begin(if invert { "选择除该乐器" } else { "按乐器选择" });
        tx.track_selection();
        if !tx.read_selection().is_empty() {
            place_selection_in_tx(&mut tx);
        }
        let keep: Vec<(u32, u32, Note)> = tx
            .read_song()
            .iter_notes()
            .filter(|(_, _, n)| (n.instrument == instrument) != invert)
            .collect();
        if keep.is_empty() {
            tx.commit();
            return;
        }
        tx.watch_notes(&keep);
        for (t, l, _) in &keep {
            tx.song().remove(*t, *l);
        }
        tx.selection().merge_absolute(&keep);
        tx.commit();
    }

    pub fn select_invert(&mut self) {
        let old_abs: BTreeSet<(i64, i64)> = self
            .selection
            .notes
            .iter()
            .map(|n| (self.selection.x + n.dx as i64, self.selection.y + n.dy as i64))
            .collect();
        let mut tx = self.begin("反选");
        tx.track_selection();
        // 先把旧选区落回，再重新摘出「不在旧集合」的音符。
        if !tx.read_selection().is_empty() {
            place_selection_in_tx(&mut tx);
        }
        let all: Vec<(u32, u32, Note)> = tx.read_song().iter_notes().collect();
        let keep: Vec<(u32, u32, Note)> = all
            .iter()
            .copied()
            .filter(|(t, l, _)| !old_abs.contains(&(*t as i64, *l as i64)))
            .collect();
        tx.watch_notes(&keep);
        for (t, l, _) in &keep {
            tx.song().remove(*t, *l);
        }
        *tx.selection() = Selection::from_absolute(0, 0, &keep);
        tx.commit();
    }

    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let mut tx = self.begin("删除选区");
        tx.track_selection();
        *tx.selection() = Selection::new();
        tx.commit();
    }

    /// 拖拽过程中调用：只移动浮动层，不写历史。
    pub fn set_selection_origin(&mut self, x: i64, y: i64) {
        self.selection.set_origin(x, y);
    }

    /// 松开鼠标时调用：把拖拽前/后的选区状态记成一条历史。
    pub fn commit_selection_move(&mut self, before: SelectionSnapshot) {
        self.selection.clamp_origin();
        let after = self.selection.clone();
        let mut tx = self.begin("移动选区");
        tx.track_selection();
        // 覆盖事务捕获的 before（begin 时捕获的是当前值）。
        tx.selection_before = before;
        *tx.selection() = after;
        tx.commit();
    }

    /// 从选区中减去一个矩形，被减掉的音符写回歌曲。
    pub fn selection_remove_rect(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        if self.selection.is_empty() {
            return;
        }
        let mut tx = self.begin("减少选区");
        tx.track_selection();
        let (ax, ay, bx, by) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
        let removed = tx.selection().remove_rect(ax, ay, bx, by);
        for (t, l, _) in &removed {
            tx.watch(*t, *l);
        }
        for (t, l, n) in removed {
            tx.song().set(t, l, n);
        }
        tx.commit();
    }

    // ---------------------------------------------------------------- 剪贴板

    pub fn copy_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.clipboard = self.selection.notes.clone();
        self.clipboard_text = self.selection.encode_rle();
    }

    pub fn cut_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.copy_selection();
        self.delete_selection();
    }

    /// 粘贴到 `(x, y)`：先落定旧选区，再生成一个新的浮动选区。
    pub fn paste(&mut self, x: i64, y: i64) {
        let items = self.clipboard.clone();
        if items.is_empty() {
            return;
        }
        let mut tx = self.begin("粘贴");
        tx.track_selection();
        place_selection_in_tx(&mut tx);

        // 目标格原有音符会被新选区覆盖：先监视再移除。
        for n in &items {
            let ax = (x + n.dx as i64).max(0) as u32;
            let ay = (y + n.dy as i64).max(0) as u32;
            tx.watch(ax, ay);
        }
        for n in &items {
            let ax = (x + n.dx as i64).max(0) as u32;
            let ay = (y + n.dy as i64).max(0) as u32;
            tx.song().remove(ax, ay);
        }
        let mut sel = Selection { x, y, notes: items, ..Default::default() };
        sel.recalc_size();
        sel.clamp_origin();
        *tx.selection() = sel;
        tx.commit();
    }

    /// 从 OpenNBS 文本粘贴（跨程序互通）。
    pub fn paste_text(&mut self, code: &str, x: i64, y: i64) {
        let decoded = Selection::decode_rle(code);
        if decoded.is_empty() {
            return;
        }
        self.clipboard = decoded.notes;
        self.clipboard_text = code.to_string();
        self.paste(x, y);
    }

    // ---------------------------------------------------------------- 批量属性

    /// 对浮动选区（无选区时对全部音符）批量修改属性，等价于原版各类宏。
    pub fn batch_change(&mut self, label: impl Into<String>, mut f: impl FnMut(&mut Note)) {
        let label = label.into();
        if !self.selection.is_empty() {
            let mut tx = self.begin(label);
            tx.track_selection();
            tx.selection().change_all(|n| f(n));
            tx.commit();
        } else {
            let mut tx = self.begin(label);
            let all: Vec<_> = tx.read_song().iter_notes().collect();
            tx.watch_notes(&all);
            for (t, l, n) in all {
                let mut n = n;
                f(&mut n);
                tx.song().set(t, l, n.clamped());
            }
            tx.commit();
        }
    }

    pub fn transpose(&mut self, semitones: i32) {
        self.batch_change("移调", |n| {
            n.key = (n.key as i32 + semitones).clamp(0, 87) as u8;
        });
    }

    pub fn set_instrument_all(&mut self, instrument: u8) {
        self.batch_change("更改音色", |n| n.instrument = instrument);
    }

    // ---------------------------------------------------------------- 图层

    pub fn insert_layer(&mut self, at: u32) {
        let at = at.min(self.song.layers.len() as u32);
        let mut tx = self.begin("添加图层");
        tx.track_layers();
        remap_layers(&mut tx, at, |l| Some(l + 1));
        tx.song().layers.insert(at as usize, Layer::default());
        tx.commit();
    }

    pub fn append_layer(&mut self) {
        let at = self.song.layers.len() as u32;
        self.insert_layer(at);
    }

    pub fn remove_layer(&mut self, at: u32) {
        if self.song.layers.len() <= 1 || at as usize >= self.song.layers.len() {
            return;
        }
        let mut tx = self.begin("删除图层");
        tx.track_layers();
        // 先把浮动选区落定，避免层号变化后选区错位。
        place_selection_in_tx(&mut tx);
        tx.track_selection();
        remap_layers(&mut tx, at, |l| {
            if l == at {
                None
            } else {
                Some(l - 1)
            }
        });
        tx.song().layers.remove(at as usize);
        tx.commit();
    }

    pub fn swap_layers(&mut self, a: u32, b: u32) {
        if a == b || a as usize >= self.song.layers.len() || b as usize >= self.song.layers.len() {
            return;
        }
        let from = a.min(b);
        let mut tx = self.begin("移动图层");
        tx.track_layers();
        remap_layers(&mut tx, from, |l| {
            if l == a {
                Some(b)
            } else if l == b {
                Some(a)
            } else {
                Some(l)
            }
        });
        tx.song().layers.swap(a as usize, b as usize);
        tx.commit();
    }

    pub fn set_layer(&mut self, index: u32, f: impl Fn(&mut Layer)) {
        if index as usize >= self.song.layers.len() {
            return;
        }
        let mut tx = self.begin("修改图层");
        tx.track_layers();
        if let Some(layer) = tx.song().layers.get_mut(index as usize) {
            f(layer);
        }
        tx.commit();
    }

    // ---------------------------------------------------------------- 元数据

    pub fn edit_meta(&mut self, label: impl Into<String>, f: impl Fn(&mut SongMeta)) {
        let mut tx = self.begin(label);
        tx.track_meta();
        f(&mut tx.song().meta);
        tx.commit();
    }

    /// 原版「循环结束刻」：`.nbs` 里没有 loop_end 字段，循环终点就是歌曲长度，
    /// 所以把结尾延长到 `loop_end` 靠的是在 (loop_end-1, 0) 放一个 0 音量音符
    /// （原版 `add_block(loopend - 1, 0, instrument_list[| 0], 45, 0, 100, 0)`）。
    /// 只延长、不缩短：缩短需要先删掉结尾音符，原版同样不允许。
    pub fn extend_loop_end(&mut self, loop_end: u32) {
        if loop_end == 0 || self.song.song_length() >= loop_end {
            return;
        }
        let tick = loop_end - 1;
        let mut tx = self.begin("延长循环结尾");
        tx.watch(tick, 0);
        tx.song().set(
            tick,
            0,
            Note {
                instrument: 0,
                key: 45,
                velocity: 0,
                panning: 100,
                pitch: 0,
            },
        );
        tx.commit();
    }

    // ---------------------------------------------------------------- 历史

    pub fn undo(&mut self) -> bool {
        self.history.undo(&mut self.song, &mut self.selection)
    }

    pub fn redo(&mut self) -> bool {
        self.history.redo(&mut self.song, &mut self.selection)
    }

    pub fn replace_song(&mut self, song: Song) {
        self.song = song;
        self.selection = Selection::new();
        self.history.clear();
    }
}

/// 事务内落定浮动选区。
fn place_selection_in_tx(tx: &mut Tx<'_>) {
    let sel = tx.read_selection().clone();
    if sel.is_empty() {
        return;
    }
    for n in &sel.notes {
        let ax = (sel.x + n.dx as i64).max(0) as u32;
        let ay = (sel.y + n.dy as i64).max(0) as u32;
        tx.watch(ax, ay);
    }
    for n in &sel.notes {
        let ax = (sel.x + n.dx as i64).max(0) as u32;
        let ay = (sel.y + n.dy as i64).max(0) as u32;
        tx.song().set(ax, ay, n.note);
    }
    *tx.selection() = Selection::new();
}

/// 把 `l >= from` 的音符按 `map` 重新映射到新层号；None 表示丢弃。
fn remap_layers(tx: &mut Tx<'_>, from: u32, map: impl Fn(u32) -> Option<u32>) {
    let all: Vec<(u32, u32, Note)> = tx
        .read_song()
        .iter_notes()
        .filter(|(_, l, _)| *l >= from)
        .collect();
    tx.watch_notes(&all);
    for (t, l, _) in &all {
        if let Some(nl) = map(*l) {
            tx.watch(*t, nl);
        }
    }
    let mut moved = Vec::with_capacity(all.len());
    for (t, l, n) in all {
        tx.song().remove(t, l);
        if let Some(nl) = map(l) {
            moved.push((t, nl, n));
        }
    }
    for (t, l, n) in moved {
        tx.song().set(t, l, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_loop_end_adds_one_silent_note_like_original() {
        let mut doc = Document::new();
        doc.place_note(3, 0, Note::new(0, 45));
        assert_eq!(doc.song.song_length(), 4);
        doc.extend_loop_end(10);
        assert_eq!(doc.song.song_length(), 10);
        assert_eq!(
            doc.song.get(9, 0),
            Some(Note { instrument: 0, key: 45, velocity: 0, panning: 100, pitch: 0 })
        );
        // 不缩短、不重复加：与原版 median(enda + 1, …) 的单向语义一致。
        doc.extend_loop_end(6);
        assert_eq!(doc.song.song_length(), 10);
        doc.extend_loop_end(10);
        assert_eq!(doc.song.total_blocks, 2);
        assert!(doc.undo(), "延长结尾应是一条可撤销的历史");
        assert_eq!(doc.song.song_length(), 4);
    }

    #[test]
    fn floating_selection_roundtrip() {
        let mut doc = Document::new();
        doc.place_note(5, 0, Note::new(0, 45));
        doc.place_note(6, 0, Note::new(0, 47));
        doc.select_rect(5, 0, 6, 0, false, false, false);
        assert_eq!(doc.selection.len(), 2);
        assert_eq!(doc.song.total_blocks, 0, "选中后音符应从歌曲摘出");
        doc.set_selection_origin(10, 0);
        doc.commit_selection_move(Selection::from_absolute(5, 0, &[
            (5, 0, Note::new(0, 45)),
            (6, 0, Note::new(0, 47)),
        ]));
        doc.place_selection();
        assert_eq!(doc.song.get(10, 0), Some(Note::new(0, 45)));
        assert_eq!(doc.song.get(11, 0), Some(Note::new(0, 47)));
    }

    #[test]
    fn copy_paste_does_not_destroy_original() {
        let mut doc = Document::new();
        doc.place_note(2, 0, Note::new(0, 45));
        doc.select_rect(2, 0, 2, 0, true, false, false);
        doc.copy_selection();
        doc.place_selection();
        doc.paste(20, 0);
        doc.place_selection();
        assert_eq!(doc.song.get(2, 0), Some(Note::new(0, 45)));
        assert_eq!(doc.song.get(20, 0), Some(Note::new(0, 45)));
    }

    #[test]
    fn insert_layer_shifts_blocks_and_undo_restores() {
        let mut doc = Document::new();
        doc.place_note(3, 0, Note::new(0, 45));
        doc.place_note(3, 1, Note::new(1, 40));
        doc.insert_layer(0);
        assert_eq!(doc.song.get(3, 1), Some(Note::new(0, 45)));
        assert_eq!(doc.song.get(3, 2), Some(Note::new(1, 40)));
        assert_eq!(doc.song.layers.len(), 2);
        assert!(doc.undo());
        assert_eq!(doc.song.get(3, 0), Some(Note::new(0, 45)));
        assert_eq!(doc.song.get(3, 1), Some(Note::new(1, 40)));
        assert_eq!(doc.song.layers.len(), 1);
    }

    #[test]
    fn batch_change_is_one_history_entry() {
        let mut doc = Document::new();
        for i in 0..50u32 {
            doc.place_note(i, 0, Note::new(0, 45));
        }
        let before = doc.history.len();
        doc.batch_change("升八度", |n| n.key += 12);
        assert_eq!(doc.history.len(), before + 1);
        assert_eq!(doc.song.get(0, 0).unwrap().key, 57);
    }
}
