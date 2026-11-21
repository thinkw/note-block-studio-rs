//! 撤销 / 重做：紧凑 diff 命令栈。
//!
//! 原版用一张 `history[a, 0..15]` 定长二维表 + 13 种命令码 + 选区 RLE 文本，
//! 每种命令在 undo/redo 里各写一遍逆操作。这里改成统一的「单元格前后值 diff」：
//! 每次事务只记录**真正变化的格子**，因此一个 10 万音符的工程里改一个音符，
//! 历史条目只占几十字节，不会像全量快照那样爆炸。
//! 图层/元数据/浮动选区的变化用同样的方式记录前后态。

use std::collections::VecDeque;

use crate::model::{Layer, Note, Song, SongMeta};
use crate::selection::{Selection, SelectionSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellDelta {
    pub tick: u32,
    pub layer: u32,
    pub before: Option<Note>,
    pub after: Option<Note>,
}

impl CellDelta {
    pub fn is_noop(&self) -> bool {
        self.before == self.after
    }
}

#[derive(Clone, Debug)]
pub struct LayerDelta {
    pub before: Vec<Layer>,
    pub after: Vec<Layer>,
}

#[derive(Clone, Debug)]
pub struct MetaDelta {
    pub before: SongMeta,
    pub after: SongMeta,
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub label: String,
    pub cells: Vec<CellDelta>,
    pub layers: Option<LayerDelta>,
    pub meta: Option<MetaDelta>,
    /// (事务前选区, 事务后选区)
    pub selection: Option<(SelectionSnapshot, SelectionSnapshot)>,
}

impl Edit {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
            && self.layers.is_none()
            && self.meta.is_none()
            && self.selection.is_none()
    }

    /// 应用到歌曲。`undo_side = true` 恢复 before，否则应用 after。
    pub fn apply(&self, song: &mut Song, selection: &mut Selection, undo_side: bool) {
        for d in &self.cells {
            let value = if undo_side { d.before } else { d.after };
            match value {
                Some(n) => {
                    song.set(d.tick, d.layer, n);
                }
                None => {
                    song.remove(d.tick, d.layer);
                }
            }
        }
        if let Some(l) = &self.layers {
            song.layers = if undo_side {
                l.before.clone()
            } else {
                l.after.clone()
            };
        }
        if let Some(m) = &self.meta {
            song.meta = if undo_side {
                m.before.clone()
            } else {
                m.after.clone()
            };
        }
        if let Some((before, after)) = &self.selection {
            *selection = if undo_side {
                before.clone()
            } else {
                after.clone()
            };
        }
        song.recalculate();
        song.rev += 1;
    }
}

pub struct History {
    entries: VecDeque<Edit>,
    /// 已应用的条目数（0..=entries.len()）。
    cursor: usize,
    capacity: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(512)
    }
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            cursor: 0,
            capacity: capacity.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    /// 栈容量：超出后最旧的条目会被丢掉，外部调用方用它判断「还能往前退多远」。
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }
    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo().then(|| self.entries[self.cursor - 1].label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.can_redo().then(|| self.entries[self.cursor].label.as_str())
    }
    pub fn entries(&self) -> impl Iterator<Item = (usize, &Edit)> {
        self.entries.iter().enumerate()
    }

    /// 提交一次事务。若当前处于撤销状态，先丢弃 redo 分支。
    pub fn push(&mut self, edit: Edit) {
        if edit.is_empty() {
            return;
        }
        if self.can_redo() {
            self.entries.truncate(self.cursor);
        }
        self.entries.push_back(edit);
        self.cursor = self.entries.len();
        self.trim();
    }

    fn trim(&mut self) {
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn undo(&mut self, song: &mut Song, selection: &mut Selection) -> bool {
        if !self.can_undo() {
            return false;
        }
        self.cursor -= 1;
        let edit = self.entries[self.cursor].clone();
        edit.apply(song, selection, true);
        true
    }

    pub fn redo(&mut self, song: &mut Song, selection: &mut Selection) -> bool {
        if !self.can_redo() {
            return false;
        }
        let edit = self.entries[self.cursor].clone();
        edit.apply(song, selection, false);
        self.cursor += 1;
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn undo_redo_single_note() {
        let mut doc = Document::new();
        doc.place_note(3, 0, Note::new(0, 45));
        assert_eq!(doc.song.get(3, 0), Some(Note::new(0, 45)));
        assert!(doc.undo());
        assert!(doc.song.get(3, 0).is_none());
        assert!(doc.redo());
        assert_eq!(doc.song.get(3, 0), Some(Note::new(0, 45)));
    }

    #[test]
    fn new_edit_discards_redo_branch() {
        let mut doc = Document::new();
        doc.place_note(1, 0, Note::new(0, 40));
        doc.place_note(2, 0, Note::new(0, 42));
        assert!(doc.undo());
        assert!(doc.history.can_redo());
        doc.place_note(5, 0, Note::new(0, 50));
        assert!(!doc.history.can_redo());
        assert!(doc.song.get(2, 0).is_none());
    }

    #[test]
    fn diff_only_records_changed_cells() {
        let mut doc = Document::new();
        for i in 0..1000u32 {
            doc.place_note(i, 0, Note::new(0, 45));
        }
        doc.place_note(500, 0, Note::new(1, 50));
        let last = doc.history.entries().last().unwrap().1;
        assert_eq!(last.cells.len(), 1);
    }
}
