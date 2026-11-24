//! 原版歌曲标签条（`draw_song_tabs.gml` / `draw_song_tab.gml`）。
//!
//! 标签条和工具栏在 y=24..58 重叠：布局上另留 `get_tab_offset()` 的高度把画布往下推，
//! 绘制区则固定在窗口 y=24，保持原版像素。拖拽重排留到刀 3，本文件先实现单击、
//! 关闭、加号、右键菜单和 0.18 秒让位动画。

use egui::{pos2, Align2, Color32, Id, Rect, Sense, Vec2};

use crate::commands::Command;
use crate::fonts::main_font;
use crate::icons::Bank;
use crate::state::EditorState;
use crate::tabs::SongId;
use crate::theme::{Theme, ThemeKind};
use crate::tr;

/// 原版 `get_tab_offset()`：只有多标签且非全屏才让位。
pub fn height(state: &EditorState) -> f32 {
    if state.store.len() > 1 && !state.fullscreen {
        return match state.theme.kind {
            ThemeKind::Aqua => 35.0,
            ThemeKind::Retro | ThemeKind::Dark => 30.0,
            ThemeKind::Fluent => 40.0,
        };
    }
    0.0
}

fn span(window_width: f32, count: usize) -> f32 {
    ((window_width - 44.0) / count as f32).clamp(1.0, 179.0)
}

fn theme_index(kind: ThemeKind) -> usize {
    match kind {
        ThemeKind::Aqua => 0,
        ThemeKind::Retro => 1,
        ThemeKind::Dark => 2,
        ThemeKind::Fluent => 3,
    }
}

fn gm(c: u32) -> Color32 {
    Color32::from_rgb(
        (c & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        ((c >> 16) & 0xff) as u8,
    )
}

fn alpha(c: Color32, a: u8) -> Color32 {
    let (r, g, b, _) = c.to_tuple();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn tab_title(state: &EditorState, id: SongId) -> String {
    let song = if state.active_tab() == Some(id) {
        &state.doc.song
    } else {
        &state.store.parked_doc(id).expect("标签存在").song
    };
    if let Some(path) = &song.filename {
        return path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    if state.player && !state.midi_file.is_empty() {
        return state.midi_file.clone();
    }
    tr("Unsaved song", "新文件").to_string()
}

fn truncate_title(p: &egui::Painter, title: &str, suffix: &str, width: f32) -> String {
    let font = main_font();
    let full = format!("{title}{suffix}");
    if p.layout_no_wrap(full, font.clone(), Color32::WHITE)
        .size()
        .x
        <= width
    {
        return format!("{title}{suffix}");
    }
    let ellipsis = "...";
    let reserved = p
        .layout_no_wrap(ellipsis.to_string(), font.clone(), Color32::WHITE)
        .size()
        .x
        + p.layout_no_wrap(suffix.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x;
    let mut out = title.to_string();
    while !out.is_empty()
        && p.layout_no_wrap(out.clone(), font.clone(), Color32::WHITE)
            .size()
            .x
            > width - reserved
    {
        out.pop();
    }
    format!("{out}{ellipsis}{suffix}")
}

struct TabVisual {
    id: SongId,
    x: f32,
    title: String,
    dirty: bool,
    selected: bool,
}

pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    let count = state.store.len();
    if count < 2 || state.fullscreen {
        state.store.reset_tab_layout();
        state.tab_menu = None;
        return;
    }
    let Some(bank) = state.icons.bank() else {
        return;
    };
    let th = state.theme.clone();
    let rw = ctx.screen_rect().width();
    let span = span(rw, count);
    let elapsed = ctx.input(|i| i.stable_dt).clamp(0.0, 0.05);
    let active = state.active_tab();
    let ids = state.store.order().to_vec();
    let mut tabs = Vec::with_capacity(count);
    for (index, id) in ids.iter().copied().enumerate() {
        let x = state
            .store
            .advance_tab_layout(id, 7.0 + index as f32 * span, elapsed, 0.18);
        let title = tab_title(state, id);
        let dirty = if active == Some(id) {
            state.doc.song.dirty
        } else {
            state
                .store
                .parked_doc(id)
                .map(|d| d.song.dirty)
                .unwrap_or(false)
        };
        tabs.push(TabVisual {
            id,
            x,
            title,
            dirty,
            selected: active == Some(id),
        });
    }

    let area = Rect::from_min_size(pos2(0.0, 24.0), Vec2::new(rw, 34.0));
    egui::Area::new(Id::new("nbs_song_tabs"))
        .order(egui::Order::Middle)
        .fixed_pos(area.min)
        .default_size(area.size())
        .sense(Sense::hover())
        .show(ctx, |ui| {
            let p = ui.painter().clone();
            let input_open = state.window.is_none()
                && state.modals.is_empty()
                && state.pending_close_set.is_none();
            let held = ui.input(|i| i.pointer.any_down());
            let top = if th.kind == ThemeKind::Fluent {
                24.0
            } else if matches!(th.kind, ThemeKind::Retro | ThemeKind::Dark) {
                19.0
            } else {
                24.0
            };
            if th.kind != ThemeKind::Fluent {
                bank.songtab(
                    &p,
                    pos2(0.0, top),
                    9 + 10 * theme_index(th.kind),
                    Vec2::new(rw, 31.0),
                    Color32::WHITE,
                );
            }

            for tab in &tabs {
                let tab_rect = Rect::from_min_size(
                    pos2(tab.x - 1.0, 26.0),
                    Vec2::new(
                        span - 1.0,
                        24.0 + 5.0 * (th.kind == ThemeKind::Fluent) as i32 as f32,
                    ),
                );
                let close_width = if th.kind == ThemeKind::Fluent {
                    30.0
                } else {
                    16.0
                };
                let close_height = if th.kind == ThemeKind::Fluent {
                    22.0
                } else {
                    15.0
                };
                let close_inset = if th.kind == ThemeKind::Fluent {
                    35.0
                } else {
                    21.0
                };
                let close_y = match th.kind {
                    ThemeKind::Fluent => 29.0,
                    ThemeKind::Aqua => 31.0,
                    ThemeKind::Retro | ThemeKind::Dark => 29.0,
                };
                let close_rect = Rect::from_min_size(
                    pos2(tab.x + span - close_inset, close_y),
                    Vec2::new(close_width, close_height),
                );
                let tab_resp = ui.interact(tab_rect, Id::new(("songtab", tab.id)), Sense::click());
                let close_resp = ui.interact(
                    close_rect,
                    Id::new(("songtab-close", tab.id)),
                    Sense::click(),
                );
                let hover = tab_resp.hovered() && input_open;
                let close_hover = close_resp.hovered() && input_open;
                let hover_state = if hover && held {
                    2
                } else if hover {
                    1
                } else {
                    0
                };
                let close_state = if close_hover && held {
                    2
                } else if close_hover {
                    1
                } else {
                    0
                };
                draw_tab(&bank, &p, &th, tab, span, hover_state, close_state);

                if input_open && close_resp.clicked() {
                    state.emit(Command::CloseTab(tab.id));
                } else if input_open && tab_resp.clicked() {
                    state.emit(Command::SwitchTab(tab.id));
                }
                if input_open && tab_resp.secondary_clicked() {
                    state.tab_menu = Some(crate::state::TabMenu {
                        id: tab.id,
                        pos: ui.ctx().pointer_hover_pos().unwrap_or(tab_rect.min),
                    });
                }
            }

            let new_x = 7.0 + count as f32 * span;
            let new_rect = Rect::from_min_size(
                pos2(new_x - 1.0, 26.0),
                Vec2::new(
                    30.0,
                    24.0 + 5.0 * (th.kind == ThemeKind::Fluent) as i32 as f32,
                ),
            );
            let new_resp = ui.interact(new_rect, Id::new("songtab-new"), Sense::click());
            let new_hover = new_resp.hovered() && input_open;
            draw_new(&bank, &p, &th, new_x, new_hover);
            if input_open && new_resp.clicked() {
                state.emit(Command::New);
            }

            if th.kind == ThemeKind::Fluent {
                for tab in &tabs {
                    p.line(
                        vec![
                            pos2(tab.x + span + 1.0, 26.0),
                            pos2(tab.x + span + 1.0, 57.0),
                        ],
                        egui::Stroke::new(1.0_f32, alpha(th.text_dim, 128)),
                    );
                }
                p.line(
                    vec![pos2(0.0, 58.0), pos2(rw, 58.0)],
                    egui::Stroke::new(1.0_f32, alpha(th.text_dim, 204)),
                );
            }
        });

    show_menu(state, ctx);
}

fn draw_tab(
    bank: &Bank,
    p: &egui::Painter,
    th: &Theme,
    tab: &TabVisual,
    span: f32,
    hover: usize,
    close_hover: usize,
) {
    let tabwidth = span + 1.0;
    if th.kind != ThemeKind::Fluent {
        let theme = theme_index(th.kind);
        let top = if matches!(th.kind, ThemeKind::Retro | ThemeKind::Dark) {
            19.0
        } else {
            24.0
        };
        let frame =
            3 * (hover == 1 && !tab.selected) as usize + 6 * tab.selected as usize + 10 * theme;
        bank.songtab(
            p,
            pos2(tab.x - 2.0, top),
            frame,
            Vec2::new(4.0, 31.0),
            Color32::WHITE,
        );
        bank.songtab(
            p,
            pos2(tab.x + 2.0, top),
            frame + 1,
            Vec2::new(tabwidth - 5.0, 31.0),
            Color32::WHITE,
        );
        bank.songtab(
            p,
            pos2(tab.x + span - 3.0, top),
            frame + 2,
            Vec2::new(4.0, 31.0),
            Color32::WHITE,
        );
        let close_x = tab.x + span - 21.0;
        let close_y = top + 7.0 + 3.0 * (th.kind != ThemeKind::Aqua) as i32 as f32;
        bank.frame4_blit(
            p,
            pos2(close_x, close_y),
            close_hover,
            Vec2::new(17.0, 16.0),
            th,
            false,
        );
        let pressed = (close_hover == 2 && th.kind != ThemeKind::Aqua) as i32 as f32;
        let frame = (close_hover > 0 && th.kind == ThemeKind::Aqua) as usize;
        let tint = if th.kind == ThemeKind::Retro {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        bank.closetab(
            p,
            pos2(close_x + 4.0 + pressed, close_y + 4.0 + pressed),
            frame,
            tint,
        );
    } else {
        let dark_fill = th.fdark;
        let hover_color = if dark_fill {
            gm(0x002d2d2d)
        } else {
            gm(0x00e9e9e9)
        };
        let selected_color = if dark_fill {
            gm(0x00282828)
        } else {
            gm(0x00f9f9f9)
        };
        if hover == 1 || tab.selected {
            let y = if tab.selected { 24.0 } else { 26.0 };
            p.rect_filled(
                Rect::from_min_size(pos2(tab.x, y), Vec2::new(span, 57.0 - y)),
                6.0,
                if tab.selected {
                    selected_color
                } else {
                    hover_color
                },
            );
        }
        if close_hover > 0 {
            let color = if dark_fill {
                if tab.selected {
                    if close_hover == 2 {
                        gm(3158064)
                    } else {
                        gm(3487029)
                    }
                } else if close_hover == 2 {
                    gm(3487029)
                } else {
                    gm(3750201)
                }
            } else if tab.selected {
                if close_hover == 2 {
                    gm(15987699)
                } else {
                    gm(15790320)
                }
            } else if close_hover == 2 {
                gm(15000804)
            } else {
                gm(14803425)
            };
            p.rect_filled(
                Rect::from_min_size(pos2(tab.x + span - 35.0, 29.0), Vec2::new(31.0, 23.0)),
                6.0,
                color,
            );
        }
        let tint = if th.fdark {
            Color32::WHITE
        } else {
            Color32::BLACK
        };
        bank.closetab(
            p,
            pos2(tab.x + span - 22.0, 37.0),
            2 + tab.dirty as usize,
            tint,
        );
    }

    let suffix = if th.kind != ThemeKind::Fluent && tab.dirty {
        "*"
    } else {
        ""
    };
    let text_width = span - 40.0 - 15.0 * (th.kind == ThemeKind::Fluent) as i32 as f32;
    let title = truncate_title(p, &tab.title, suffix, text_width.max(0.0));
    let text_y = match th.kind {
        ThemeKind::Aqua => 32.0,
        ThemeKind::Retro | ThemeKind::Dark => 30.0,
        ThemeKind::Fluent => 34.0,
    };
    p.text(
        pos2(tab.x + 8.0, text_y),
        Align2::LEFT_TOP,
        title,
        main_font(),
        th.text,
    );
}

fn draw_new(bank: &Bank, p: &egui::Painter, th: &Theme, x: f32, hover: bool) {
    if th.kind != ThemeKind::Fluent {
        let theme = theme_index(th.kind);
        let top = if matches!(th.kind, ThemeKind::Retro | ThemeKind::Dark) {
            19.0
        } else {
            24.0
        };
        let frame = 3 * hover as usize + 10 * theme;
        bank.songtab(
            p,
            pos2(x - 2.0, top),
            frame,
            Vec2::new(4.0, 31.0),
            Color32::WHITE,
        );
        bank.songtab(
            p,
            pos2(x + 2.0, top),
            frame + 1,
            Vec2::new(27.0, 31.0),
            Color32::WHITE,
        );
        bank.songtab(
            p,
            pos2(x + 28.0, top),
            frame + 2,
            Vec2::new(4.0, 31.0),
            Color32::WHITE,
        );
    } else {
        if hover {
            p.rect_filled(
                Rect::from_min_size(pos2(x, 26.0), Vec2::new(31.0, 31.0)),
                6.0,
                if th.fdark {
                    gm(0x002d2d2d)
                } else {
                    gm(0x00e9e9e9)
                },
            );
        }
        bank.newtab(
            p,
            pos2(x + 9.0, 34.0),
            if th.fdark {
                Color32::WHITE
            } else {
                Color32::BLACK
            },
        );
    }
}

fn show_menu(state: &mut EditorState, ctx: &egui::Context) {
    let Some(menu) = state.tab_menu else {
        return;
    };
    if !state.store.contains(menu.id) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.tab_menu = None;
        return;
    }
    let count = state.store.len();
    let index = state.store.index_of(menu.id).unwrap_or(0);
    let mut chosen = None;
    let response = egui::Window::new("Song tab")
        .id(Id::new("nbs_songtab_menu"))
        .title_bar(false)
        .resizable(false)
        .fixed_pos(menu.pos)
        .show(ctx, |ui| {
            if ui.button(tr("New song", "新文件")).clicked() {
                chosen = Some(Command::NewTabAfter(menu.id));
            }
            ui.separator();
            if ui
                .button(tr("Close song  Ctrl+W", "关闭歌曲  Ctrl+W"))
                .clicked()
            {
                chosen = Some(Command::CloseTab(menu.id));
            }
            ui.add_enabled(
                count > 1,
                egui::Button::new(tr("Close other songs", "关闭其他歌曲")),
            )
            .clicked()
            .then(|| chosen = Some(Command::CloseOtherTabs(menu.id)));
            ui.add_enabled(
                index + 1 < count,
                egui::Button::new(tr("Close songs to the right", "关闭右侧歌曲")),
            )
            .clicked()
            .then(|| chosen = Some(Command::CloseTabsRight(menu.id)));
        });
    if let Some(cmd) = chosen {
        state.tab_menu = None;
        state.emit(cmd);
        return;
    }
    let menu_rect = response.as_ref().map(|r| r.response.rect);
    if ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .latest_pos()
                .is_some_and(|pos| menu_rect.is_some_and(|rect| !rect.contains(pos)))
    }) {
        state.tab_menu = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_sprites_are_available() {
        let ctx = egui::Context::default();
        let mut icons = crate::icons::Icons::default();
        icons.ensure(&ctx);
        assert!(icons.bank().is_some(), "标签条精灵必须能加载");
    }

    #[test]
    fn offset_and_span_match_the_original() {
        let mut state = EditorState::for_test();
        state.store.reset_tab_layout();
        assert_eq!(height(&state), 0.0);
        state.push_tab(nbs_core::model::Song::new());
        assert_eq!(height(&state), 40.0);
        state.theme.kind = ThemeKind::Aqua;
        assert_eq!(height(&state), 35.0);
        state.theme.kind = ThemeKind::Retro;
        assert_eq!(height(&state), 30.0);
        state.fullscreen = true;
        assert_eq!(height(&state), 0.0);
        assert_eq!(span(1440.0, 2), 179.0);
        assert_eq!(span(400.0, 4), 89.0);
        assert_eq!(span(10.0, 4), 1.0);
    }
}
