//! 原版 15 个宏（macro_* / draw_window_macro_*）的 Rust 实现。
//!
//! 原版把选区导成扁平数组做指针算术：每列先是列偏移，然后每个音符 6 个槽
//! （行偏移、乐器、音高键、力度、声道、微调），列尾一个 -1
//! （`selection_to_array_ext.gml`）；这里直接在 `Vec<(i64,i64,Note)>` 上按列
//! 分组变换，语义等价但不易越界。所有宏都通过一次事务提交，撤销只占一条历史。

use std::collections::BTreeMap;

use crate::document::Document;
use crate::model::{Note, KEY_MAX, PAN_MAX, PIT_MAX, VEL_MAX};
use crate::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TremoloFade {
    None,
    In,
    Out,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Macro {
    FadeIn,
    FadeOut,
    VelocityLfo,
    Vibrato,
    Reset,
    ReplaceKey,
    Tremolo {
        spacing: u32,
        fade: TremoloFade,
        amount: i32,
    },
    Stereo {
        width: u8,
        reverse: bool,
    },
    Arpeggio {
        pattern: Vec<i32>,
    },
    Portamento {
        cents: i32,
        reverse: bool,
    },
    Stagger {
        pattern: Vec<u32>,
    },
    Chorus {
        detune: i32,
    },
    SetVelocity {
        value: u8,
        percent: bool,
    },
    SetPanning {
        value: i16,
    },
    SetPitch {
        value: i16,
        additive: bool,
    },
    SetInstrument {
        value: u8,
    },
}

impl Macro {
    /// 英文宏名，逐字取自原版 `lang_en_us.gml:136-149`（`name_macro_*`）。
    pub fn name(&self) -> &'static str {
        match self {
            Self::FadeIn => "Fade in",
            Self::FadeOut => "Fade out",
            Self::VelocityLfo => "Volume LFO",
            Self::Vibrato => "Vibrato",
            Self::Reset => "Reset all properties",
            Self::ReplaceKey => "Replace key",
            Self::Tremolo { .. } => "Tremolo",
            Self::Stereo { .. } => "Stereo",
            Self::Arpeggio { .. } => "Arpeggio",
            Self::Portamento { .. } => "Portamento",
            Self::Stagger { .. } => "Stagger",
            Self::Chorus { .. } => "Chorus",
            Self::SetVelocity { .. } => "Set velocity",
            Self::SetPanning { .. } => "Set panning",
            Self::SetPitch { .. } => "Set pitch",
            Self::SetInstrument { .. } => "Set instrument",
        }
    }

    /// 中文宏名，取自原版中文菜单（`control_draw.gml:1848`）：
    /// 只有 淡入/淡出/替换音/设定音量/设定声道/设定音高/重置所有属性 这七条有
    /// 中文，其余（Tremolo、Stereo、Arpeggio、Portamento、Vibrato、Stagger、
    /// Chorus、Volume LFO）原版在中文界面里也保持英文，这里如实照抄。
    /// Set instrument 是移植自建的宏，按同一条「设定〇〇」构词补齐。
    pub fn name_zh(&self) -> &'static str {
        match self {
            Self::FadeIn => "淡入",
            Self::FadeOut => "淡出",
            Self::VelocityLfo => "Volume LFO",
            Self::Vibrato => "Vibrato",
            Self::Reset => "重置所有属性",
            Self::ReplaceKey => "替换音",
            Self::Tremolo { .. } => "Tremolo",
            Self::Stereo { .. } => "Stereo",
            Self::Arpeggio { .. } => "Arpeggio",
            Self::Portamento { .. } => "Portamento",
            Self::Stagger { .. } => "Stagger",
            Self::Chorus { .. } => "Chorus",
            Self::SetVelocity { .. } => "设定音量",
            Self::SetPanning { .. } => "设定声道",
            Self::SetPitch { .. } => "设定音高",
            Self::SetInstrument { .. } => "设定音色",
        }
    }

    /// 按当前语言取宏名（原版 `condstr(language != 1, …)` 的写法）。
    pub fn label(&self) -> &'static str {
        crate::tr(self.name(), self.name_zh())
    }

    pub fn all() -> Vec<Macro> {
        vec![
            Self::FadeIn,
            Self::FadeOut,
            Self::VelocityLfo,
            Self::Vibrato,
            Self::Reset,
            Self::ReplaceKey,
            Self::Tremolo {
                spacing: 2,
                fade: TremoloFade::None,
                amount: 20,
            },
            Self::Stereo {
                width: 60,
                reverse: false,
            },
            Self::Arpeggio {
                pattern: vec![0, 4, 7, 12],
            },
            Self::Portamento {
                cents: 300,
                reverse: false,
            },
            Self::Stagger {
                pattern: vec![0, 1, 2],
            },
            Self::Chorus { detune: 40 },
            Self::SetVelocity {
                value: 100,
                percent: false,
            },
            Self::SetPanning { value: 0 },
            Self::SetPitch {
                value: 0,
                additive: false,
            },
            Self::SetInstrument { value: 0 },
        ]
    }
}

/// 原版三个带规律的窗口（琶音 / 错开）解析输入框的写法：
/// `string_digits_symbol(pattern, "|")` 只留数字和 `|`（负号、小数点、空格、
/// 字母全被丢掉），补一个尾部 `|` 后交给 `selection_to_array` 切段，而它
/// 只收非空段（`if (element_size > 0)`）。所以 `"0 | -4 |7"` → `[0, 4, 7]`。
pub fn parse_pattern(raw: &str) -> Vec<i32> {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '|')
        .collect();
    kept.split('|')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f64>().unwrap_or(0.0) as i32)
        .collect()
}

/// 按列（tick）分组，返回排序后的列号与各列音符。
fn group_columns(items: &[(i64, i64, Note)]) -> BTreeMap<i64, Vec<(i64, i64, Note)>> {
    let mut cols: BTreeMap<i64, Vec<(i64, i64, Note)>> = BTreeMap::new();
    for &(x, y, n) in items {
        cols.entry(x).or_default().push((x, y, n));
    }
    for v in cols.values_mut() {
        v.sort_by_key(|t| t.1);
    }
    cols
}

/// 纯函数变换：输入绝对坐标音符，输出（可能数量/坐标变化的）新音符列表。
pub fn transform(items: Vec<(i64, i64, Note)>, m: &Macro) -> Vec<(i64, i64, Note)> {
    if items.is_empty() {
        return items;
    }
    let cols = group_columns(&items);
    let col_keys: Vec<i64> = cols.keys().copied().collect();
    let ncol = col_keys.len().max(1);
    let mut out: Vec<(i64, i64, Note)> = Vec::with_capacity(items.len());

    match m {
        Macro::FadeIn | Macro::FadeOut => {
            let fade_in = matches!(m, Macro::FadeIn);
            for (ci, &x) in col_keys.iter().enumerate() {
                let factor = if ncol <= 1 {
                    if fade_in {
                        0.0
                    } else {
                        1.0
                    }
                } else if fade_in {
                    ci as f32 / (ncol - 1) as f32
                } else {
                    1.0 - ci as f32 / (ncol - 1) as f32
                };
                for &(xx, y, mut n) in &cols[&x] {
                    n.velocity =
                        ((n.velocity as f32 * factor).ceil() as i32).clamp(1, VEL_MAX as i32) as u8;
                    out.push((xx, y, n));
                }
            }
        }
        Macro::VelocityLfo => {
            const WAVE: [u8; 4] = [100, 50, 25, 50];
            for (ci, &x) in col_keys.iter().enumerate() {
                for &(xx, y, mut n) in &cols[&x] {
                    n.velocity = WAVE[ci % 4];
                    if n.panning == 0 {
                        n.panning = 100;
                    }
                    out.push((xx, y, n));
                }
            }
        }
        Macro::Vibrato => {
            const WAVE: [i16; 4] = [40, 0, -40, 0];
            for (ci, &x) in col_keys.iter().enumerate() {
                for &(xx, y, mut n) in &cols[&x] {
                    n.pitch = WAVE[ci % 4];
                    out.push((xx, y, n));
                }
            }
        }
        Macro::Reset => {
            for (x, y, mut n) in items {
                n.velocity = VEL_MAX;
                n.panning = 100;
                n.pitch = 0;
                out.push((x, y, n));
            }
        }
        Macro::ReplaceKey => {
            let key = items[0].2.key;
            for (x, y, mut n) in items {
                n.key = key;
                out.push((x, y, n));
            }
        }
        Macro::Stereo { width, reverse } => {
            let w = (*width).min(100) as i32;
            for (ci, &x) in col_keys.iter().enumerate() {
                let left = (ci % 2 == 0) != *reverse;
                let pan = if left { 100 - w } else { 100 + w };
                for &(xx, y, mut n) in &cols[&x] {
                    n.panning = pan.clamp(0, PAN_MAX as i32) as u8;
                    out.push((xx, y, n));
                }
            }
        }
        Macro::Arpeggio { pattern } => {
            if pattern.is_empty() {
                return items;
            }
            // 原版那条指针走查是按列推进规律下标的（`for (i = 0; i < arplen; i++)`
            // 在外层 while 里面，一轮走完一整列），所以同一列的所有音符拿同一个
            // 偏移，相邻列之间才换规律里的下一个数。
            for (ci, &x) in col_keys.iter().enumerate() {
                let d = pattern[ci % pattern.len()];
                for &(xx, y, mut n) in &cols[&x] {
                    // 原版的越界判断写成 `> 0 || < 88`，恒为真，等于不夹；这里
                    // 保留夹取，因为端口浮动选区的 key 是 u8，越界会直接写坏音符。
                    n.key = (n.key as i32 + d).clamp(0, KEY_MAX as i32) as u8;
                    out.push((xx, y, n));
                }
            }
        }
        Macro::Portamento { cents, reverse } => {
            let step = *cents as f32 / ncol as f32;
            for (ci, &x) in col_keys.iter().enumerate() {
                // 原版每列后 `decr += inc`（反转是 `decr -= inc`），`inc` 就是首列
                // 的 `step`，所以第 ci 列拿到 `step * (ci + 1)`；反转那一支是
                // `arr_data[val] = port_cent + decr`（直接赋值，不看原音高），
                // decr 从 step 起步往下减，即 `step * (1 - ci)`。
                let delta = if *reverse {
                    *cents as f32 + step * (1 - ci as i32) as f32
                } else {
                    step * (ci + 1) as f32
                };
                for &(xx, y, mut n) in &cols[&x] {
                    n.pitch = if *reverse {
                        delta as i16
                    } else {
                        (n.pitch as f32 + delta) as i16
                    }
                    .clamp(-PIT_MAX, PIT_MAX);
                    out.push((xx, y, n));
                }
            }
        }
        Macro::Stagger { pattern } => {
            if pattern.is_empty() {
                return items;
            }
            // 与琶音同一套走查：规律下标按列推进，一列内的音符整体下移同样的行数。
            for (ci, &x) in col_keys.iter().enumerate() {
                let dy = pattern[ci % pattern.len()] as i64;
                for &(xx, y, n) in &cols[&x] {
                    out.push((xx, y + dy, n));
                }
            }
        }
        Macro::Chorus { detune } => {
            for &x in &col_keys {
                let col = &cols[&x];
                for (k, &(xx, y, mut n)) in col.iter().enumerate() {
                    if k % 2 == 0 {
                        n.panning = 60;
                        n.pitch = -(detune.abs() as i16);
                        n.velocity = (n.velocity as i32 + 40).min(VEL_MAX as i32) as u8;
                    } else {
                        n.panning = 140;
                        n.pitch = detune.abs() as i16;
                    }
                    out.push((xx, y, n));
                }
            }
        }
        Macro::SetVelocity { value, percent } => {
            for (x, y, mut n) in items {
                n.velocity = if *percent {
                    ((n.velocity as f32) * (*value as f32 / 100.0))
                        .round()
                        .clamp(0.0, VEL_MAX as f32) as u8
                } else {
                    (*value).min(VEL_MAX)
                };
                out.push((x, y, n));
            }
        }
        Macro::SetPanning { value } => {
            let p = (*value + 100).clamp(0, PAN_MAX as i16) as u8;
            for (x, y, mut n) in items {
                n.panning = p;
                out.push((x, y, n));
            }
        }
        Macro::SetPitch { value, additive } => {
            for (x, y, mut n) in items {
                n.pitch = if *additive {
                    (n.pitch as i32 + *value as i32).clamp(-PIT_MAX as i32, PIT_MAX as i32) as i16
                } else {
                    (*value).clamp(-PIT_MAX, PIT_MAX)
                };
                out.push((x, y, n));
            }
        }
        Macro::SetInstrument { value } => {
            for (x, y, mut n) in items {
                n.instrument = *value;
                out.push((x, y, n));
            }
        }
        Macro::Tremolo {
            spacing,
            fade,
            amount,
        } => {
            let sp = (*spacing).clamp(1, 8) as i64;
            let total_out = ncol as i64 * sp;
            for (ci, &x) in col_keys.iter().enumerate() {
                for (k, &(xx, y, mut n)) in cols[&x].iter().enumerate() {
                    let _ = k;
                    for j in 0..sp {
                        let nx = xx * sp + j;
                        let progress = if total_out <= 1 {
                            0.0
                        } else {
                            (ci as f64 * sp as f64 + j as f64) / total_out as f64
                        };
                        let env = match fade {
                            TremoloFade::None => 0,
                            TremoloFade::In => (amount.abs() as f64 * progress) as i32,
                            TremoloFade::Out => -(amount.abs() as f64 * progress) as i32,
                        };
                        n.velocity = (n.velocity as i32 + env).clamp(1, VEL_MAX as i32) as u8;
                        out.push((nx, y, n));
                    }
                }
            }
        }
    }

    // 坐标归一化：不允许负 tick / 负 layer
    let minx = out.iter().map(|t| t.0).min().unwrap_or(0);
    let miny = out.iter().map(|t| t.1).min().unwrap_or(0);
    if minx < 0 || miny < 0 {
        for t in &mut out {
            t.0 -= minx.min(0);
            t.1 -= miny.min(0);
        }
    }
    out
}

impl Document {
    /// 对浮动选区执行一次宏，记录一条紧凑 diff 历史。空选区直接忽略 ——
    /// 原版 macro_fadein / macro_vibrato / macro_tremolo 等每个宏开头都是
    /// `if (songs[song].selected = 0) return 0`（macro_chorus 传空数组等效）。
    pub fn run_macro(&mut self, m: &Macro) {
        if self.selection.is_empty() {
            return;
        }
        let sel = self.selection.clone();
        let abs: Vec<(i64, i64, Note)> = sel
            .notes
            .iter()
            .map(|n| (sel.x + n.dx as i64, sel.y + n.dy as i64, n.note))
            .collect();
        let result = transform(abs, m);
        let mut tx = self.begin(format!("{}: {}", crate::tr("Macro", "宏"), m.label()));
        tx.track_selection();
        let new_sel = selection_at_origin(sel.x, sel.y, &result);
        *tx.selection() = new_sel;
        tx.commit();
    }
}

/// 宏结果写回浮动选区。原版每个宏收尾都是
/// `selection_load_from_array(songs[song].selection_x, songs[song].selection_y, arr)`
/// ——原点沿用选区自己的，不跟着重新取左上角；否则错开把音符整体下移后，
/// 选区会莫名往上跳。
fn selection_at_origin(x: i64, y: i64, items: &[(i64, i64, Note)]) -> Selection {
    if items.is_empty() {
        return Selection::new();
    }
    let abs: Vec<(u32, u32, Note)> = items
        .iter()
        .map(|&(ax, ay, n)| (ax as u32, ay as u32, n))
        .collect();
    Selection::from_absolute(x, y, &abs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Song;

    fn doc_with_column() -> Document {
        let mut d = Document::new();
        for i in 0..8u32 {
            d.place_note(i, 0, Note::new(0, 45));
        }
        d
    }

    #[test]
    fn fade_in_ramps_velocity() {
        let mut d = doc_with_column();
        d.select_rect(0, 0, 7, 0, false, false, false);
        d.run_macro(&Macro::FadeIn);
        let first = d.selection.at(0, 0).unwrap().velocity;
        let last = d.selection.at(7, 0).unwrap().velocity;
        assert!(first <= 1, "首列应最轻: {first}");
        assert_eq!(last, 100);
        assert!(d.history.can_undo());
        d.undo();
        assert_eq!(d.selection.at(0, 0).unwrap().velocity, 100);
    }

    #[test]
    fn vibrato_writes_pitch_wave() {
        let mut d = doc_with_column();
        d.select_rect(0, 0, 2, 0, false, false, false);
        d.run_macro(&Macro::Vibrato);
        assert_eq!(d.selection.at(0, 0).unwrap().pitch, 40);
        assert_eq!(d.selection.at(1, 0).unwrap().pitch, 0);
        assert_eq!(d.selection.at(2, 0).unwrap().pitch, -40);
    }

    #[test]
    fn tremolo_expands_notes() {
        let mut d = doc_with_column();
        d.select_rect(0, 0, 7, 0, false, false, false);
        d.run_macro(&Macro::Tremolo {
            spacing: 2,
            fade: TremoloFade::None,
            amount: 0,
        });
        assert_eq!(d.selection.len(), 16);
    }

    #[test]
    fn macro_on_selection_is_one_undo_step() {
        let mut d = doc_with_column();
        d.select_rect(0, 0, 3, 0, false, false, false);
        assert_eq!(d.selection.len(), 4);
        let before = d.history.len();
        d.run_macro(&Macro::Vibrato);
        assert_eq!(d.history.len(), before + 1);
        assert_eq!(d.selection.len(), 4);
        assert_eq!(d.selection.at(0, 0).unwrap().pitch, 40);
    }

    #[test]
    fn stereo_alternates_pan() {
        let mut d = doc_with_column();
        d.select_rect(0, 0, 7, 0, false, false, false);
        d.run_macro(&Macro::Stereo {
            width: 60,
            reverse: false,
        });
        assert_eq!(d.selection.at(0, 0).unwrap().panning, 40);
        assert_eq!(d.selection.at(1, 0).unwrap().panning, 160);
    }

    /// 原版每个宏开头都是 `if (songs[song].selected = 0) return 0`：没有选区
    /// 时宏必须什么都不做（不改歌、不进历史），绝不退化成「对全曲生效」。
    #[test]
    fn macro_without_selection_is_a_noop() {
        let mut d = doc_with_column();
        let before = d.history.len();
        d.run_macro(&Macro::Vibrato);
        d.run_macro(&Macro::FadeIn);
        d.run_macro(&Macro::Stereo {
            width: 60,
            reverse: false,
        });
        assert_eq!(d.history.len(), before);
        assert!(d.selection.is_empty());
        let n = d.song.get(0, 0).unwrap();
        assert_eq!(n.pitch, 0);
        assert_eq!(n.velocity, 100);
        assert_eq!(n.panning, 100);
    }

    #[test]
    fn arpeggio_applies_pattern_per_column() {
        let mut d = Document::with_song(Song::new());
        // 原版规律下标按列推进：同一列两个音符拿同一个偏移，下一列才换下一个数。
        for x in 0..3u32 {
            d.place_note(x, 0, Note::new(0, 45));
            d.place_note(x, 1, Note::new(0, 45));
        }
        d.select_rect(0, 0, 2, 1, false, false, false);
        d.run_macro(&Macro::Arpeggio {
            pattern: vec![0, 4, 7],
        });
        for (x, key) in [(0u32, 45u8), (1, 49), (2, 52)] {
            assert_eq!(d.selection.at(x, 0).unwrap().key, key, "第 {x} 列上行");
            assert_eq!(d.selection.at(x, 1).unwrap().key, key, "第 {x} 列下行");
        }
    }

    #[test]
    fn stagger_shifts_each_column_by_pattern() {
        let mut d = Document::with_song(Song::new());
        for x in 0..3u32 {
            d.place_note(x, 0, Note::new(0, 45));
            d.place_note(x, 1, Note::new(0, 45));
        }
        d.select_rect(0, 0, 2, 1, false, false, false);
        d.run_macro(&Macro::Stagger {
            pattern: vec![0, 1, 2],
        });
        // 每列整体下移 pattern[列号]，列内两音符的间距不变。
        assert!(d.selection.at(0, 0).is_some());
        assert!(d.selection.at(1, 1).is_some());
        assert!(d.selection.at(2, 2).is_some());
        assert!(d.selection.at(2, 3).is_some());
        // 原点不跟着重新取左上角：第一列没动，选区仍锚在 (0, 0)。
        assert_eq!((d.selection.x, d.selection.y), (0, 0));
    }

    /// 原点保持的另一半：选区不在 (0,0) 时，错开也不要把选区往上拽。
    #[test]
    fn stagger_keeps_the_selection_origin() {
        let mut d = Document::with_song(Song::new());
        d.place_note(5, 7, Note::new(0, 45));
        d.place_note(6, 7, Note::new(0, 45));
        d.select_rect(5, 7, 6, 7, false, false, false);
        d.run_macro(&Macro::Stagger {
            pattern: vec![2, 3],
        });
        assert_eq!((d.selection.x, d.selection.y), (5, 7));
        assert_eq!(d.selection.at(0, 2).unwrap().key, 45);
        assert_eq!(d.selection.at(1, 3).unwrap().key, 45);
    }

    #[test]
    fn portamento_reverse_is_one_step_higher_than_cent() {
        // 原版反转那一支：第 0 列是 `port_cent + decr`（decr 初值 = cent/列数），
        // 之后每列再减一个 step，所以起步比要求的 cent 还高一个 step。
        let out = transform(
            vec![(0, 0, Note::new(0, 45)), (1, 0, Note::new(0, 45))],
            &Macro::Portamento {
                cents: 400,
                reverse: true,
            },
        );
        assert_eq!(out[0].2.pitch, 600);
        assert_eq!(out[1].2.pitch, 400);
    }

    #[test]
    fn parse_pattern_keeps_only_digits_and_pipes() {
        assert_eq!(parse_pattern("0 | -4 |7"), vec![0, 4, 7]);
        // 空段整段丢掉（原版 `element_size > 0`），小数点被吃掉只留下数字。
        assert_eq!(parse_pattern("1||2|"), vec![1, 2]);
        assert_eq!(parse_pattern("0.5|abc|"), vec![5]);
        assert_eq!(parse_pattern(""), Vec::<i32>::new());
    }
}
