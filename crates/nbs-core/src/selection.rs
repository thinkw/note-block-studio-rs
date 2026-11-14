//! 浮动选区（floating selection）。
//!
//! 原版的一个关键语义：音符一旦被选中，就**从歌曲网格里摘出来**放到浮动层，
//! 拖拽时只移动浮动层，`selection_place()` 时才写回歌曲。粘贴/复制/批量修改
//! 全都建立在同一个浮动层之上。这里保留该语义，但用 `Vec` + 稀疏坐标代替
//! 原版的 8 个平行二维数组。

use crate::model::{Note, Song, DEFAULT_KEY};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SelectionNote {
    /// 相对选区原点的 tick 偏移。
    pub dx: u32,
    /// 相对选区原点的 layer 偏移。
    pub dy: u32,
    pub note: Note,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Selection {
    /// 浮动选区在歌曲中的原点，允许为负（拖出左/上边界）。
    pub x: i64,
    pub y: i64,
    pub notes: Vec<SelectionNote>,
    pub width: u32,
    pub height: u32,
}

/// 用于历史快照的别名，语义上更清晰。
pub type SelectionSnapshot = Selection;

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// 选区在歌曲坐标下的包围盒（含端点）。
    pub fn bounds(&self) -> Option<(i64, i64, i64, i64)> {
        if self.is_empty() {
            return None;
        }
        Some((
            self.x,
            self.y,
            self.x + self.width as i64 - 1,
            self.y + self.height as i64 - 1,
        ))
    }

    pub fn contains(&self, tick: i64, layer: i64) -> bool {
        let dx = tick - self.x;
        let dy = layer - self.y;
        if dx < 0 || dy < 0 {
            return false;
        }
        self.notes
            .iter()
            .any(|n| n.dx as i64 == dx && n.dy as i64 == dy)
    }

    pub fn at(&self, dx: u32, dy: u32) -> Option<Note> {
        self.notes
            .iter()
            .find(|n| n.dx == dx && n.dy == dy)
            .map(|n| n.note)
    }

    pub fn recalc_size(&mut self) {
        let w = self.notes.iter().map(|n| n.dx + 1).max().unwrap_or(0);
        let h = self.notes.iter().map(|n| n.dy + 1).max().unwrap_or(0);
        self.width = w;
        self.height = h;
    }

    /// 把浮动选区整体平移到 `(x, y)`（不改变内部相对坐标）。
    pub fn set_origin(&mut self, x: i64, y: i64) {
        self.x = x;
        self.y = y;
    }

    /// 用一组绝对坐标重建选区。
    pub fn from_absolute(x: i64, y: i64, items: &[(u32, u32, Note)]) -> Self {
        let mut notes = Vec::with_capacity(items.len());
        for &(t, l, note) in items {
            let dx = t as i64 - x;
            let dy = l as i64 - y;
            if dx >= 0 && dy >= 0 {
                notes.push(SelectionNote {
                    dx: dx as u32,
                    dy: dy as u32,
                    note,
                });
            }
        }
        let mut s = Self { x, y, notes, ..Default::default() };
        s.recalc_size();
        s
    }

    /// 合并一组绝对坐标音符进已有选区（Ctrl 加选 / 框选叠加）。
    /// 新音符若与已有位置重合则覆盖。
    pub fn merge_absolute(&mut self, items: &[(u32, u32, Note)]) {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<(i64, i64), Note> = BTreeMap::new();
        for n in &self.notes {
            map.insert((self.x + n.dx as i64, self.y + n.dy as i64), n.note);
        }
        for &(t, l, note) in items {
            map.insert((t as i64, l as i64), note);
        }
        let minx = map.keys().map(|k| k.0).min().unwrap_or(self.x);
        let miny = map.keys().map(|k| k.1).min().unwrap_or(self.y);
        self.x = minx;
        self.y = miny;
        self.notes = map
            .into_iter()
            .map(|((ax, ay), note)| SelectionNote {
                dx: (ax - minx) as u32,
                dy: (ay - miny) as u32,
                note,
            })
            .collect();
        self.recalc_size();
    }

    /// 从选区中移除落在矩形内的音符，返回被移除的绝对坐标音符。
    pub fn remove_rect(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<(u32, u32, Note)> {
        let mut removed = Vec::new();
        self.notes.retain(|n| {
            let ax = self.x + n.dx as i64;
            let ay = self.y + n.dy as i64;
            let inside = ax >= x0 as i64
                && ax <= x1 as i64
                && ay >= y0 as i64
                && ay <= y1 as i64;
            if inside && ax >= 0 && ay >= 0 {
                removed.push((ax as u32, ay as u32, n.note));
            }
            !inside
        });
        // 保持原点不动（原版 selection_remove 亦不移动选区原点），仅重算尺寸。
        self.recalc_size();
        if self.notes.is_empty() {
            self.width = 0;
            self.height = 0;
        }
        removed
    }

    /// 把选区落点夹取到非负坐标（松开拖拽时调用）。
    pub fn clamp_origin(&mut self) {
        // 若原点是负数，需要把内容整体右移/下移，避免音符丢失。
        if self.x < 0 {
            let shift = (-self.x) as u32;
            for n in &mut self.notes {
                n.dx += shift;
            }
            self.x = 0;
        }
        if self.y < 0 {
            let shift = (-self.y) as u32;
            for n in &mut self.notes {
                n.dy += shift;
            }
            self.y = 0;
        }
    }

    /// 批量修改选区内音符属性。
    pub fn change_all<F: FnMut(&mut Note)>(&mut self, mut f: F) {
        for n in &mut self.notes {
            f(&mut n.note);
        }
    }

    /// 编码为 OpenNBS 兼容的 `selection_code` 文本格式：
    /// `列偏移|行偏移|ins|key|vel|pan|pit|...|-1|`。
    /// 空列与空行用相对偏移跳过，因此大选区也不会产生巨长字符串。
    pub fn encode_rle(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        use std::collections::BTreeMap;
        let mut cols: BTreeMap<u32, BTreeMap<u32, Note>> = BTreeMap::new();
        for n in &self.notes {
            cols.entry(n.dx).or_default().insert(n.dy, n.note);
        }
        let mut out = String::new();
        let mut prev_col: i64 = 0;
        for (dx, rows) in cols {
            let gap = if prev_col == 0 && dx == 0 {
                0
            } else {
                dx as i64 - prev_col
            };
            out.push_str(&gap.to_string());
            out.push('|');
            let mut prev_row: i64 = 0;
            for (dy, note) in rows {
                let row_gap = dy as i64 - prev_row;
                out.push_str(&row_gap.to_string());
                out.push('|');
                out.push_str(&note.instrument.to_string());
                out.push('|');
                out.push_str(&note.key.to_string());
                out.push('|');
                out.push_str(&note.velocity.to_string());
                out.push('|');
                out.push_str(&note.panning.to_string());
                out.push('|');
                out.push_str(&note.pitch.to_string());
                out.push('|');
                prev_row = dy as i64;
            }
            out.push_str("-1|");
            prev_col = dx as i64;
        }
        out
    }

    /// 解码 OpenNBS `selection_code`（增量语义，列/行偏移均累加），
    /// 返回相对坐标选区（原点 0,0）。
    pub fn decode_rle(code: &str) -> Self {
        let tokens: Vec<&str> = code.split('|').filter(|s| !s.is_empty()).collect();
        let mut i = 0;
        let mut notes = Vec::new();
        let mut col: i64 = 0;
        while i < tokens.len() {
            col += tokens[i].trim().parse::<i64>().unwrap_or(0);
            i += 1;
            let mut row: i64 = 0;
            while i < tokens.len() {
                let val = tokens[i].trim().parse::<i64>().unwrap_or(-1);
                i += 1;
                if val == -1 {
                    break;
                }
                row += val;
                if i + 5 > tokens.len() {
                    break;
                }
                let ins = tokens[i].trim().parse::<u8>().unwrap_or(0);
                let key = tokens[i + 1].trim().parse::<u8>().unwrap_or(DEFAULT_KEY);
                let vel = tokens[i + 2].trim().parse::<u8>().unwrap_or(100);
                let pan = tokens[i + 3].trim().parse::<u8>().unwrap_or(100);
                let pit = tokens[i + 4].trim().parse::<i16>().unwrap_or(0);
                i += 5;
                if col >= 0 && row >= 0 {
                    notes.push(SelectionNote {
                        dx: col as u32,
                        dy: row as u32,
                        note: Note { instrument: ins, key, velocity: vel, panning: pan, pitch: pit },
                    });
                }
            }
        }
        let mut s = Self { x: 0, y: 0, notes, ..Default::default() };
        s.recalc_size();
        s
    }
}

/// 把歌曲矩形内的音符取出（跳过锁定层），返回绝对坐标列表。
pub fn collect_rect(
    song: &Song,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    include_locked: bool,
) -> Vec<(u32, u32, Note)> {
    let locked = song.locked_layers();
    song.notes_in_rect(x0, y0, x1, y1)
        .into_iter()
        .filter(|(_, l, _)| {
            include_locked || !locked.get(*l as usize).copied().unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_roundtrip() {
        let s = Selection::from_absolute(0, 0, &[
            (0, 0, Note::new(0, 45)),
            (0, 3, Note::new(1, 40)),
            (5, 1, Note::new(2, 50)),
        ]);
        let code = s.encode_rle();
        let back = Selection::decode_rle(&code);
        assert_eq!(back.len(), 3);
        assert_eq!(back.at(0, 0), Some(Note::new(0, 45)));
        assert_eq!(back.at(0, 3), Some(Note::new(1, 40)));
        assert_eq!(back.at(5, 1), Some(Note::new(2, 50)));
    }

    #[test]
    fn merge_keeps_absolute_positions() {
        let mut s = Selection::from_absolute(10, 2, &[(10, 2, Note::new(0, 45))]);
        s.merge_absolute(&[(20, 5, Note::new(1, 50))]);
        assert_eq!(s.x, 10);
        assert_eq!(s.y, 2);
        assert_eq!(s.at(0, 0), Some(Note::new(0, 45)));
        assert_eq!(s.at(10, 3), Some(Note::new(1, 50)));
    }
}
