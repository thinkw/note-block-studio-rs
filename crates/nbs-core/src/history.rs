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

/// 一步事务真正碰过的格子，供 UI 做「撤销后定位」。
/// 图层/元数据那两类改动没有格坐标，所以 [`Edit::scope`] 对它们返回 `None`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditScope {
    /// 去重并按 `(tick, layer)` 升序排好的受影响格子。
    pub cells: Vec<(u32, u32)>,
    /// 外接矩形，闭区间 `(tick0, layer0, tick1, layer1)`。
    pub bounds: (u32, u32, u32, u32),
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

    /// 这一步改过的格子范围。`before == after` 的占位 diff（例如事务里顺手记了一次
    /// 没有真正变化的格子）不算，因为高亮它们会误导「这里被动过」。
    pub fn scope(&self) -> Option<EditScope> {
        let mut cells: Vec<(u32, u32)> = self
            .cells
            .iter()
            .filter(|d| !d.is_noop())
            .map(|d| (d.tick, d.layer))
            .collect();
        if cells.is_empty() {
            return None;
        }
        cells.sort_unstable();
        cells.dedup();
        let tick0 = cells.first()?.0;
        let tick1 = cells.last()?.0;
        let mut layer0 = u32::MAX;
        let mut layer1 = 0;
        for (_, layer) in &cells {
            layer0 = layer0.min(*layer);
            layer1 = layer1.max(*layer);
        }
        Some(EditScope {
            cells,
            bounds: (tick0, layer0, tick1, layer1),
        })
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

    /// 按下标取条目（0 = 最早一条）。超出栈容量后下标会整体前移，调用方不要把下标存久。
    pub fn get(&self, index: usize) -> Option<&Edit> {
        self.entries.get(index)
    }

    /// 刚被 [`undo`](Self::undo) / [`redo`](Self::redo) 应用掉的那一条的下标，供 UI 取标签和定位范围。
    /// `undo` 先把 cursor 退一位再应用 `entries[cursor]`，`redo` 应用完才自增，
    /// 所以撤销侧的下标就是 cursor，重做侧是 cursor-1。
    /// **只在一次撤销 / 重做成功之后立刻调用**：平时这两个下标指的是别的条目。
    pub fn applied_index(&self, undo_side: bool) -> usize {
        if undo_side {
            self.cursor
        } else {
            self.cursor.saturating_sub(1)
        }
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

    /// 端口自加（历史列表点选）：回退或前进到「前 `target` 条已应用」的状态，
    /// 取值口径与 [`Self::cursor`] 一致（`0` = 全撤完，`len()` = 全部应用）。
    /// 逐条走 [`Edit::apply`]，不另开第二条状态迁移路径：选区恢复、图层/元数据回滚、
    /// `song.rev` 自增都和单步撤销/重做完全同路。
    /// 返回最后一次真正应用的条目下标，供 UI 报动作名和定位；已经在位或下标越界时返回
    /// `None`，此时歌曲一个字节都不动。跳转不丢掉任何条目，redo 分支照样留着。
    pub fn jump_to(
        &mut self,
        target: usize,
        song: &mut Song,
        selection: &mut Selection,
    ) -> Option<usize> {
        if target > self.entries.len() || target == self.cursor {
            return None;
        }
        if target < self.cursor {
            while self.cursor > target {
                self.cursor -= 1;
                self.entries[self.cursor].apply(song, selection, true);
            }
            Some(target)
        } else {
            while self.cursor < target {
                self.entries[self.cursor].apply(song, selection, false);
                self.cursor += 1;
            }
            Some(target - 1)
        }
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

    #[test]
    fn scope_wraps_the_cells_a_step_touched() {
        let mut doc = Document::new();
        // 一个事务里改两颗离得很远的音，历史只有一条，范围要横跨它们。
        let mut tx = doc.begin("一次改两颗");
        tx.watch(7, 2);
        tx.song().set(7, 2, Note::new(0, 45));
        tx.watch(3, 5);
        tx.song().set(3, 5, Note::new(0, 45));
        tx.commit();
        let last = doc.history.entries().last().unwrap().1;
        let scope = last.scope().unwrap();
        assert_eq!(
            scope.cells,
            vec![(3, 5), (7, 2)],
            "格子按 (tick, layer) 升序"
        );
        assert_eq!(scope.bounds, (3, 2, 7, 5), "外接矩形取闭区间");
    }

    #[test]
    fn scope_ignores_cells_that_did_not_change() {
        let edit = Edit {
            label: String::new(),
            cells: vec![CellDelta {
                tick: 4,
                layer: 1,
                before: None,
                after: None,
            }],
            layers: None,
            meta: None,
            selection: None,
        };
        assert!(edit.scope().is_none(), "没真变化的占位 diff 不该高亮");
        let meta_only = Edit {
            label: String::new(),
            cells: Vec::new(),
            layers: None,
            meta: None,
            selection: None,
        };
        assert!(meta_only.scope().is_none());
    }

    #[test]
    fn applied_index_points_at_the_step_just_reverted() {
        let mut doc = Document::new();
        doc.place_note(1, 0, Note::new(0, 40));
        doc.place_note(2, 0, Note::new(0, 42));
        assert!(doc.undo());
        let index = doc.history.applied_index(true);
        assert_eq!(
            doc.history.get(index).unwrap().cells[0].tick,
            2,
            "撤销后要指到刚退掉的那一步"
        );
        assert!(doc.redo());
        let index = doc.history.applied_index(false);
        assert_eq!(
            doc.history.get(index).unwrap().cells[0].tick,
            2,
            "重做后要指到刚应用的那一步"
        );
    }

    #[test]
    fn jump_to_walks_the_stack_in_both_directions() {
        let mut doc = Document::new();
        doc.place_note(1, 0, Note::new(0, 40));
        doc.place_note(2, 0, Note::new(0, 42));
        doc.place_note(3, 0, Note::new(0, 44));

        assert_eq!(doc.jump_history(1), Some(1), "往回跳报最后退掉的那条");
        assert_eq!(doc.history.cursor(), 1);
        assert_eq!(doc.song.get(1, 0), Some(Note::new(0, 40)));
        assert!(doc.song.get(2, 0).is_none(), "第 2 步没退掉");
        assert!(doc.song.get(3, 0).is_none(), "第 3 步没退掉");

        assert_eq!(doc.jump_history(3), Some(2), "往前跳报最后应用的那条");
        assert_eq!(doc.song.get(3, 0), Some(Note::new(0, 44)));

        assert_eq!(doc.jump_history(3), None, "已经在位不该有动作");
        assert_eq!(doc.jump_history(99), None, "越界不该有动作");
        assert_eq!(doc.history.cursor(), 3);

        assert_eq!(doc.jump_history(0), Some(0), "全撤完报第一条");
        assert!(doc.song.get(1, 0).is_none());

        assert_eq!(doc.history.len(), 3, "跳转不该丢掉任何条目");
        assert!(doc.jump_history(2).is_some());
        assert_eq!(doc.song.get(2, 0), Some(Note::new(0, 42)));
        assert!(doc.song.get(3, 0).is_none());
    }
}
