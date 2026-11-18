//! 底部状态栏：字段顺序、推进量、分隔线位置全部照原版 `control_draw.gml` 2040-2159。
//!
//! 原版右端那一列是 MIDI 设备名（`rw - 6` 右对齐，没设备时写「无 MIDI 设备」）。
//! 实时 MIDI 输入我们还没有，那一列整列不画；状态栏消息（「已复制 3 个音符」这类）
//! 借用这个位置显示，这样左边每一格都还落在原版坐标上。

use egui::{pos2, vec2, Align2, Color32, Painter, Rect, Ui};

use crate::fonts::main_font;
use crate::icons::Bank;
use crate::state::EditorState;
use crate::theme::{Theme, ThemeKind};
use crate::tr;

/// 原版文本顶边在 `rh - 18`，本条高 24 → 条内 y = 6。
const TEXT_Y: f32 = 6.0;
/// 原版分隔线在 `rh - 20`、长 18 → 比文本再高 2px。
const SEP_Y: f32 = 4.0;

/// 走动的 `xx`：一格文本 + 推进量 + 分隔线 + 4px 间距，和原版每格的写法一一对应。
struct Fields<'a> {
    p: &'a Painter,
    th: &'a Theme,
    x: f32,
    y: f32,
}

impl<'a> Fields<'a> {
    /// 画完把游标推到下一格的起点。
    fn text(&mut self, color: Color32, s: String, advance: f32) {
        self.draw(color, s);
        self.x += advance;
        Bank::separator(self.p, pos2(self.x - 1.0, self.y + SEP_Y - TEXT_Y), self.th);
        self.x += 4.0;
    }

    /// 最后一格：原版画完既不推进也不画分隔线。
    fn last(&mut self, color: Color32, s: String) {
        self.draw(color, s);
    }

    fn draw(&self, color: Color32, s: String) {
        self.p.text(
            pos2(self.x, self.y),
            Align2::LEFT_TOP,
            s,
            main_font(),
            color,
        );
    }
}

pub fn show_status_bar(state: &EditorState, ui: &mut Ui) {
    let th = state.theme.clone();
    let area = ui.max_rect();
    let p = ui.painter();
    p.rect_filled(area, 0.0, th.panel);
    // 原版 2041-2044：灰色 1px 线在 `rh - 24`；非 Fluent（theme != 3）再补一条白线。
    p.rect_filled(
        Rect::from_min_size(area.min, vec2(area.width(), 1.0)),
        0.0,
        Color32::from_gray(128),
    );
    if th.kind != ThemeKind::Fluent {
        p.rect_filled(
            Rect::from_min_size(pos2(area.min.x, area.min.y + 1.0), vec2(area.width(), 1.0)),
            0.0,
            Color32::WHITE,
        );
    }

    let mut f = Fields { p: &p, th: &th, x: area.min.x + 4.0, y: area.min.y + TEXT_Y };
    let cur = &state.current;
    f.text(th.text, format!("{}{}", tr("Instrument: ", "音色: "), instrument_label(state, cur.instrument)), 180.0);
    f.text(th.text, format!("{}{}", tr("Key: ", "音: "), key_name(state, cur.key)), 75.0);
    // 原版只在画笔属性被改过之后才显示这三格。
    if cur.velocity != 100 || cur.panning != 100 || cur.pitch != 0 {
        f.text(th.text, format!("{}{}", tr("Velocity: ", "音量: "), cur.velocity), 100.0);
        f.text(th.text, format!("{}{}", tr("Panning: ", "声道: "), cur.panning as i32 - 100), 95.0);
        f.text(th.text, format!("{}{}", tr("Pitch: ", "音高: "), cur.pitch), 85.0);
    }

    let hover = state.hover_cell;
    f.text(th.text, format!("{}{}", tr("Tick: ", "位置: "), hover.map_or(tr("None", "无").to_string(), |(t, _)| t.to_string())), 90.0);
    f.text(th.text, format!("{}{}", tr("Layer: ", "层: "), hover.map_or(tr("None", "无").to_string(), |(_, l)| (l + 1).to_string())), 90.0);
    let selected = state.doc.selection.notes.len();
    f.text(
        th.text,
        format!("{}{selected} / {}", tr("Selected: ", "已选择: "), state.doc.song.total_blocks + selected),
        160.0,
    );

    // 悬停格里有音符时后面四格换成那一格的属性（原版 `hovernote`）；
    // 选中的音符已从歌曲里摘出，所以此时悬停格读不到东西，和原版一致。
    let hovered = hover.and_then(|(t, l)| state.doc.song.get(t, l));
    if let Some(n) = hovered {
        f.text(th.text, format!("{}{}", tr("Key: ", "音: "), key_name(state, n.key)), 90.0);
        f.text(th.text, format!("{}{}", tr("Velocity: ", "音量: "), n.velocity), 110.0);
        f.text(th.text, format!("{}{}", tr("Panning: ", "声道: "), pan_text(n.panning)), 120.0);
        let sign = if n.pitch > 0 { "+" } else { "" };
        f.last(th.text, format!("{}{sign}{}", tr("Pitch: ", "音高: "), if crate::lang() == 1 { format!("{}微分", n.pitch) } else { format!("{} cents", n.pitch) }));
    } else {
        if let Some(m) = autosave_in(state) {
            // 英文复数按整除后的值判（原版 `condstr(ceil(tonextsave)<>1, "s")`）。
            let word = tr("minute", "分钟");
            f.text(
                th.text,
                format!("{}{m} {}{}", tr("Next auto-save: ", "下次自动保存: "), word, if crate::lang() == 0 && m != 1 { "s" } else { "" }),
                210.0,
            );
        }
        if state.prefs.show_sound_count {
            let channels = if state.prefs.extended_voices { 1024 } else { 256 };
            let sounds = state.audio.active_voices();
            let color = if sounds > channels {
                Color32::from_rgb(255, 0, 0)
            } else {
                th.text
            };
            f.last(color, format!("{}{sounds} / {channels}", tr("Sounds: ", "声音数: ")));
        }
    }

    if let Some(msg) = state.active_message() {
        p.text(
            pos2(area.max.x - 6.0, area.min.y + TEXT_Y),
            Align2::RIGHT_TOP,
            msg.to_string(),
            main_font(),
            th.accent,
        );
    }
}

/// 原版 `get_keyname(key, 1)`：状态栏永远带八度。
fn key_name(state: &EditorState, key: u8) -> String {
    nbs_core::model::key_name(key, true, state.flags.flats)
}

/// 原版 `condstr(pan < 100, "L ", "R ") + " " + abs(pan - 100)`，100 写中央。
fn pan_text(pan: u8) -> String {
    if pan == 100 {
        tr("Center", "中央").to_string()
    } else {
        let side = if pan < 100 { "L" } else { "R" };
        format!("{side}  {}", (pan as i32 - 100).abs())
    }
}

/// 原版 `instrument.name`：自定义乐器要用歌里的名字，空名字回落编号
/// （编号样式取自原版新增自定义乐器的默认名 "Custom instrument #N"）。
fn instrument_label(state: &EditorState, idx: u8) -> String {
    if let Some(ins) = nbs_core::instruments::instrument(idx) {
        return ins.name.to_string();
    }
    let j = idx as usize - nbs_core::model::FIRST_CUSTOM_INDEX as usize;
    match state.doc.song.custom_instruments.get(j) {
        Some(ci) if !ci.name.trim().is_empty() => ci.name.clone(),
        _ => format!("{}{}", tr("Custom instrument #", "自定义音色 "), j + 1),
    }
}

/// 原版 `if (autosave && filename_ext(filename) = ".nbs")` 的剩余分钟数。
fn autosave_in(state: &EditorState) -> Option<u32> {
    let mins = state.prefs.autosave_interval() as f64;
    let is_nbs = state
        .doc
        .song
        .filename
        .as_ref()
        .and_then(|p| p.extension().map(|e| e == "nbs"))
        .unwrap_or(false);
    if mins <= 0.0 || !is_nbs {
        return None;
    }
    let left = mins * 60.0 - state.last_autosave.elapsed().as_secs_f64();
    Some((left / 60.0).ceil().max(0.0) as u32)
}
