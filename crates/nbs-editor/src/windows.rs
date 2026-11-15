//! 弹窗集合：歌曲信息 / 属性 / 统计 / 首选项 / 音色 / 宏窗口 / 文件对话框 /
//! 图层重命名 / 快捷键帮助 / 关于。原版 47 个 `w_*` 窗口状态收敛成一个枚举。

use egui::{Context, RichText, Ui};
use nbs_core::instruments::{instrument_name, INSTRUMENTS};
use nbs_core::model::{key_name, SongMeta};
use nbs_core::Macro;

use crate::commands::Command;
use crate::shortcuts::SHORTCUT_HELP;
use crate::state::{EditorState, Flag, ModalQueue, WindowKind};
use crate::theme::ThemeKind;

pub fn show_windows(state: &mut EditorState, ctx: &Context) {
    show_modal(state, ctx);
    let Some(kind) = state.window.clone() else { return; };
    let mut open = true;
    let title = kind.title().to_string();
    let default_width = match kind {
        WindowKind::Preferences | WindowKind::Shortcuts => 460.0,
        WindowKind::FileOpen | WindowKind::FileSave => 520.0,
        WindowKind::ClipEditor => 500.0,
        // 原版数据包导出窗口 550×430，两列布局。
        WindowKind::DatapackExport => 560.0,
        // 原版 `draw_window_schematic_export` 是 550×400。
        WindowKind::SchematicExport => 560.0,
        _ => 360.0,
    };
    egui::Window::new(title)
        .id(egui::Id::new(format!("nbs_win_{kind:?}")))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(default_width)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| match &kind {
            WindowKind::SongInfo => song_info(state, ui),
            WindowKind::Properties => properties(state, ui),
            WindowKind::Statistics => statistics(state, ui),
            WindowKind::Preferences => preferences(state, ui),
            WindowKind::Instruments => instruments_window(state, ui),
            WindowKind::About => about(ui),
            WindowKind::SetTempo => numeric_window(state, ui, Numeric::Tempo),
            WindowKind::SetVelocity => numeric_window(state, ui, Numeric::Velocity),
            WindowKind::SetPanning => numeric_window(state, ui, Numeric::Panning),
            WindowKind::SetPitch => numeric_window(state, ui, Numeric::Pitch),
            WindowKind::Transpose => numeric_window(state, ui, Numeric::Transpose),
            WindowKind::Tremolo => numeric_window(state, ui, Numeric::Tremolo),
            WindowKind::Stereo => numeric_window(state, ui, Numeric::Stereo),
            WindowKind::ClipEditor => clip_editor(state, ui),
            WindowKind::Shortcuts => shortcuts_help(ui),
            WindowKind::FileOpen => file_dialog(state, ui, false),
            WindowKind::FileSave => file_dialog(state, ui, true),
            WindowKind::LayerRename(layer) => layer_rename(state, ui, *layer),
            WindowKind::ConfirmNew => confirm_new(state, ui),
            WindowKind::AudioExport => audio_export_window(state, ui),
            WindowKind::AudioSettings => audio_settings_window(state, ui),
            WindowKind::MidiImport => midi_import_window(state, ui),
            WindowKind::SchematicExport => schematic_export_window(state, ui),
            WindowKind::DatapackExport => datapack_export_window(state, ui),
            WindowKind::PngExport => png_export_window(state, ui),
            WindowKind::MacroEditor => macro_editor_window(state, ui),
            WindowKind::Autosaves => autosaves_window(state, ui),
        });
    if !open {
        state.window = None;
    }
}

/// 原版的 `message()` / `question()`：一次一个，答完才轮到下一个，
/// 中途答「否」丢掉整个队列。窗口右上角的叉等价于「确定」/「否」。
fn show_modal(state: &mut EditorState, ctx: &Context) {
    let Some(modal) = state.modals.items.front().cloned() else { return };
    let mut answer = None;
    let mut open = true;
    egui::Window::new(modal.title)
        .id(egui::Id::new("nbs_modal"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(&modal.text);
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if modal.yes_no {
                    if ui.button("否").clicked() {
                        answer = Some(false);
                    }
                    if ui.button("是").clicked() {
                        answer = Some(true);
                    }
                } else if ui.button("确定").clicked() {
                    answer = Some(true);
                }
            });
        });
    if answer.is_none() && !open {
        // 点右上角的叉：确认框算「确定」，提问框算「否」。
        answer = Some(!modal.yes_no);
    }
    if let Some(yes) = answer {
        state.emit(Command::AnswerModal(yes));
    }
}

fn footer(state: &mut EditorState, ui: &mut Ui) {
    ui.separator();
    if ui.button("关闭").clicked() {
        state.emit(Command::CloseWindow);
    }
}

fn song_info(state: &mut EditorState, ui: &mut Ui) {
    let mut changed = false;
    egui::Grid::new("song_info_grid").num_columns(2).show(ui, |ui| {
        let m: &mut SongMeta = &mut state.doc.song.meta;
        ui.label("名称");
        if ui.text_edit_singleline(&mut m.name).changed() { changed = true; }
        ui.end_row();
        ui.label("作者");
        if ui.text_edit_singleline(&mut m.author).changed() { changed = true; }
        ui.end_row();
        ui.label("原作者");
        if ui.text_edit_singleline(&mut m.original_author).changed() { changed = true; }
        ui.end_row();
        ui.label("描述");
        if ui.add(egui::TextEdit::multiline(&mut m.description).desired_rows(3)).changed() { changed = true; }
        ui.end_row();
    });
    if changed {
        state.doc.song.dirty = true;
    }
    ui.separator();
    ui.label(RichText::new("这些字段会随 .nbs 一起保存").small().weak());
    footer(state, ui);
}

fn properties(state: &mut EditorState, ui: &mut Ui) {
    let mut tempo = state.doc.song.meta.tempo as f64;
    if ui.add(egui::Slider::new(&mut tempo, 0.25..=60.0).text("速度 (tick/s)")).changed() {
        state.emit(Command::SetTempo(tempo as f32));
    }
    ui.label(RichText::new(format!("= {:.0} BPM", tempo * 15.0)).small().weak());
    let mut ts = state.doc.song.meta.time_signature as f32;
    if ui.add(egui::Slider::new(&mut ts, 2.0..=8.0).text("拍号")).changed() {
        state.emit(Command::SetTimeSignature(ts as u8));
    }
    let mut looping = state.doc.song.meta.loop_enabled;
    if ui.checkbox(&mut looping, "启用循环").changed() {
        state.emit(Command::ToggleLoop);
    }
    let loop_on = state.doc.song.meta.loop_enabled;
    ui.add_enabled_ui(loop_on, |ui| {
        ui.horizontal(|ui| {
            ui.label("循环起点 tick");
            let enda = state.doc.song.enda as f64;
            let mut start = state.doc.song.meta.loop_start as f64;
            if ui.add(egui::DragValue::new(&mut start).range(0.0..=enda)).changed() {
                state.doc.edit_meta("设置循环起点", |m| m.loop_start = start as u16);
                state.sync_loop_params();
            }
        });
        let len = state.doc.song.song_length();
        ui.horizontal(|ui| {
            ui.label("循环终点 tick");
            // 原版没有 loop_end 字段：循环终点就是歌曲长度，所以只能往上调，
            // 上调的效果是延长结尾。
            let mut end = len as f64;
            if ui.add(egui::DragValue::new(&mut end).range(len as f64..=(len + 16) as f64)).changed() {
                state.doc.extend_loop_end(end as u32);
                state.sync_loop_params();
            }
        });
        ui.label(
            RichText::new("超出歌曲长度时，结尾会自动加一个 0 音量音符来延长终点")
                .small()
                .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("循环次数");
            let mut max = state.doc.song.meta.loop_max as i32;
            if ui.add(egui::DragValue::new(&mut max).range(0..=10)).changed() {
                state.doc.edit_meta("设置循环次数", |m| m.loop_max = max as u8);
                state.sync_loop_params();
            }
            if state.doc.song.meta.loop_max == 0 {
                ui.label(RichText::new("（无限）").small().weak());
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label("保存格式版本");
        let mut v = state.doc.song.meta.save_version;
        egui::ComboBox::from_id_salt("save_version")
            .selected_text(format!("NBS v{v}"))
            .show_ui(ui, |ui| {
                for n in 0..=5u8 {
                    ui.selectable_value(&mut v, n, format!("NBS v{n}"));
                }
            });
        if v != state.doc.song.meta.save_version {
            state.emit(Command::SetSaveVersion(v));
        }
    });
    ui.separator();
    ui.label(RichText::new("v0..v5 全部可读可写；v4/v5 才会保存力度/声像/微调与图层锁定").small().weak());
    footer(state, ui);
}

fn statistics(state: &mut EditorState, ui: &mut Ui) {
    let song = &state.doc.song;
    // 自定义乐器的编号从 FIRST_CUSTOM_INDEX 起，所以直方图要覆盖到它们，
    // 否则大工程里最常见的恰恰是被漏掉的（原版统计就包含全部乐器行）。
    let slots = nbs_core::model::FIRST_CUSTOM_INDEX as usize
        + song.custom_instruments.len().max(1);
    let mut per_ins = vec![0usize; slots];
    let mut min_key = 87u8;
    let mut max_key = 0u8;
    for (_, _, n) in song.iter_notes() {
        let i = n.instrument as usize;
        if i < per_ins.len() {
            per_ins[i] += 1;
        }
        min_key = min_key.min(n.key);
        max_key = max_key.max(n.key);
    }
    let seconds = song.song_length() as f32 / song.meta.tempo.max(0.01);
    egui::Grid::new("stats_grid").num_columns(2).show(ui, |ui| {
        ui.label("音符总数"); ui.label(song.total_blocks.to_string()); ui.end_row();
        ui.label("歌曲长度"); ui.label(format!("{} ticks", song.song_length())); ui.end_row();
        ui.label("时长"); ui.label(format!("{seconds:.1} 秒")); ui.end_row();
        ui.label("图层数"); ui.label(song.layers.len().to_string()); ui.end_row();
        ui.label("已锁定图层"); ui.label(song.locked_layers().iter().filter(|x| **x).count().to_string()); ui.end_row();
        ui.label("音高范围"); ui.label(format!("{} ~ {}", key_name(min_key, true, false), key_name(max_key, true, false))); ui.end_row();
        ui.label("自定义音色"); ui.label(song.custom_instruments.len().to_string()); ui.end_row();
        ui.label("历史条目"); ui.label(state.doc.history.len().to_string()); ui.end_row();
    });
    ui.separator();
    ui.label(RichText::new("各乐器音符数").strong());
    let total = song.total_blocks.max(1);
    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
        for (i, c) in per_ins.iter().enumerate() {
            if *c > 0 {
                let pct = *c as f32 * 100.0 / total as f32;
                ui.label(format!("{:>6}  {:>5.1}%  {}", c, pct, instrument_name(i as u8)));
            }
        }
    });
    footer(state, ui);
}

fn preferences(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new("主题").strong());
    ui.horizontal_wrapped(|ui| {
        for kind in ThemeKind::ALL {
            if ui.selectable_label(state.theme.kind == kind, kind.name()).clicked() {
                state.emit(Command::SetTheme(kind));
            }
        }
    });
    ui.separator();
    ui.label(RichText::new("显示选项").strong());
    for flag in [
        Flag::ShowLayers, Flag::ShowPiano,
        Flag::UseColors, Flag::UseShapes, Flag::UseIcons,
        Flag::ShowNumbers, Flag::ShowOctaves, Flag::ShowKeyNames,
        Flag::ShowKeyNumbers,
        Flag::ShowIncompatible, Flag::ShowGridLines, Flag::Flats, Flag::Blackout,
    ] {
        let mut v = Flag::get(flag, &state.flags);
        if ui.checkbox(&mut v, flag.label()).changed() {
            state.emit(Command::ToggleFlag(flag));
        }
    }
    ui.separator();
    // 原版 `keysmax`（首选项 `keys_to_show`，20..50，默认 30）：钢琴最多画几根白键。
    ui.horizontal(|ui| {
        ui.label("琴键数量");
        let mut keys = state.prefs.keys_to_show as i32;
        if ui.add(egui::DragValue::new(&mut keys).range(20..=50)).changed() {
            state.prefs.keys_to_show = keys.clamp(20, 50) as u8;
            state.emit(Command::SavePreferences);
        }
    });
    // 原版 `draw_window_preferences.gml` 549-552「进度条」组。原版有三种跟随模式
    // （按页 / 按刻 / 按帧），后两种都居中，只差亚刻偏移；我们的 `follow_center`
    // 就是「居中 + 亚刻偏移」，所以这里给两格：按页 与 居中逐帧。
    ui.label(RichText::new("进度条").strong());
    if ui.checkbox(&mut state.marker_follow, "播放时跟随进度条").changed() {
        state.emit(Command::SavePreferences);
    }
    ui.add_enabled_ui(state.marker_follow, |ui| {
        if ui.radio(!state.follow_center, "按页").clicked() {
            state.follow_center = false;
            state.emit(Command::SavePreferences);
        }
        if ui.radio(state.follow_center, "居中逐帧").clicked() {
            state.follow_center = true;
            state.emit(Command::SavePreferences);
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("撤销栈容量");
        let mut cap = state.doc.history.capacity() as f64;
        if ui.add(egui::DragValue::new(&mut cap).range(16.0..=4096.0)).changed() {
            state.doc.history.set_capacity(cap as usize);
        }
    });
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui.button("音频与采样设置…").clicked() { state.emit(Command::ShowWindow(WindowKind::AudioSettings)); }
        if ui.button("音频导出设置…").clicked() { state.emit(Command::ShowWindow(WindowKind::AudioExport)); }
        if ui.button("自动保存 / 备份…").clicked() { state.emit(Command::ShowWindow(WindowKind::Autosaves)); }
        if ui.button("保存偏好").clicked() { state.emit(Command::SavePreferences); }
    });
    ui.label(RichText::new("主题、显示开关、音频参数、最近文件自动写入用户配置目录的 prefs.json").small().weak());
    footer(state, ui);
}

fn instruments_window(state: &mut EditorState, ui: &mut Ui) {
    egui::Grid::new("instruments_grid").num_columns(2).show(ui, |ui| {
        for (i, ins) in INSTRUMENTS.iter().enumerate() {
            if ui.selectable_label(state.current.instrument == i as u8, ins.name).clicked() {
                state.emit(Command::SetInstrument(i as u8));
            }
            ui.label(RichText::new(format!("Ctrl+{}", if i < 9 { (i + 1).to_string() } else { format!("Shift+{}", i - 8) })).small().weak());
            ui.end_row();
        }
    });
    ui.separator();
    ui.label(RichText::new("16 种内置乐器；自定义乐器索引 >= 16，随 .nbs 文件读写").small().weak());
    footer(state, ui);
}

#[derive(Clone, Copy)]
enum Numeric {
    Tempo, Velocity, Panning, Pitch, Transpose, Tremolo, Stereo,
}

fn numeric_window(state: &mut EditorState, ui: &mut Ui, which: Numeric) {
    let (label, hint) = match which {
        Numeric::Tempo => ("速度 (tick/s)", "10 t/s = 150 BPM"),
        Numeric::Velocity => ("力度 0..100", "应用到浮动选区；无选区时应用到全曲"),
        Numeric::Panning => ("声像 0..200", "100 = 中央，<100 偏左，>100 偏右"),
        Numeric::Pitch => ("微调 -1200..1200 ¢", "100 ¢ = 1 半音"),
        Numeric::Transpose => ("移调（半音）", "正数为升，负数为降"),
        Numeric::Tremolo => ("颤音强度", "按音高奇偶交替增减力度"),
        Numeric::Stereo => ("立体声展开量", "按音符顺序左右交替偏移声像"),
    };
    ui.label(label);
    ui.label(RichText::new(hint).small().weak());
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut state.macro_amount).speed(1.0));
        ui.label("数值");
    });
    let a = state.macro_amount;
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("应用").clicked() {
            state.emit(match which {
                Numeric::Tempo => Command::SetTempo(a),
                Numeric::Velocity => Command::BatchVelocity(a.clamp(0.0, 100.0) as u8),
                Numeric::Panning => Command::BatchPanning(a.clamp(0.0, 200.0) as u8),
                Numeric::Pitch => Command::BatchPitch(a.clamp(-1200.0, 1200.0) as i16),
                Numeric::Transpose => Command::Transpose(a as i32),
                Numeric::Tremolo => Command::Tremolo(a as i32),
                Numeric::Stereo => Command::Stereo(a as i16),
            });
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn clip_editor(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new("OpenNBS 兼容的选区 RLE 文本，可与原版剪贴板互通").small().weak());
    let mut text = state.doc.clipboard_text.clone();
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .desired_width(f32::INFINITY)
            .desired_rows(6)
            .code_editor(),
    );
    ui.horizontal(|ui| {
        if ui.button("从当前选区生成").clicked() {
            state.doc.copy_selection();
        }
        if ui.button("载入并粘贴").clicked() {
            state.emit(Command::PasteText(text.clone()));
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn shortcuts_help(ui: &mut Ui) {
    ui.label(RichText::new("快捷键对齐原版 OpenNBS，另增音频与导出相关组合键").small().weak());
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        egui::Grid::new("shortcut_grid").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in SHORTCUT_HELP {
                ui.label(RichText::new(*k).monospace().strong());
                ui.label(*v);
                ui.end_row();
            }
        });
    });
}

fn file_dialog(state: &mut EditorState, ui: &mut Ui, save: bool) {
    let mut cmd: Option<Command> = None;
    let mut error: Option<String> = None;
    let (action, ext) = state
        .file_dialog
        .as_ref()
        .map(|d| (d.action, d.ext))
        .unwrap_or((crate::state::DialogAction::OpenSong, "nbs"));
    ui.label(
        RichText::new(format!(
            "文件类型：*.{}   {}",
            ext,
            if action.is_save() { "（保存 / 导出）" } else { "（打开 / 导入）" }
        ))
        .small()
        .weak(),
    );
    if let Some(d) = state.file_dialog.as_mut() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(d.dir.to_string_lossy()).small().weak());
            if ui.small_button("上级").clicked() {
                if let Some(parent) = d.dir.parent() {
                    d.dir = parent.to_path_buf();
                    d.refresh();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("文件名");
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
        });
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            let entries = d.entries.clone();
            for entry in entries {
                let name = entry.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let is_dir = entry.is_dir();
                let label = if is_dir { format!("[目录] {name}") } else { name.clone() };
                if ui.selectable_label(false, label).clicked() {
                    if is_dir {
                        d.dir = entry.clone();
                        d.refresh();
                    } else {
                        d.name = name;
                    }
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(if save { "保存" } else { "打开" }).clicked() {
                let path = d.selected_path();
                let path = if path.extension().is_none() {
                    path.with_extension(ext)
                } else {
                    path
                };
                cmd = Some(Command::DialogConfirm(path));
            }
            if ui.button("取消").clicked() {
                cmd = Some(Command::CloseWindow);
            }
        });
        error = d.error.clone();
    } else {
        cmd = Some(Command::CloseWindow);
    }
    if let Some(e) = error {
        ui.label(RichText::new(e).color(egui::Color32::RED).small());
    }
    if let Some(c) = cmd {
        state.emit(c);
    }
}

fn layer_rename(state: &mut EditorState, ui: &mut Ui, layer: u32) {
    if state.editing_layer_name.is_none() {
        let name = state.doc.song.layers.get(layer as usize).map(|l| l.name.clone()).unwrap_or_default();
        state.editing_layer_name = Some((layer, name));
    }
    let mut cmd: Option<Command> = None;
    if let Some((_, name)) = state.editing_layer_name.as_mut() {
        ui.label("图层名称");
        ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("确定").clicked() {
                cmd = Some(Command::RenameLayer(layer, name.clone()));
            }
            if ui.button("取消").clicked() {
                cmd = Some(Command::CloseWindow);
            }
        });
    }
    if let Some(c) = cmd {
        state.editing_layer_name = None;
        state.emit(c);
        state.emit(Command::CloseWindow);
    }
}

fn confirm_new(state: &mut EditorState, ui: &mut Ui) {
    ui.label("当前歌曲有未保存的修改，确定新建？");
    ui.horizontal(|ui| {
        if ui.button("确定").clicked() {
            state.emit(Command::ConfirmNew);
        }
        if ui.button("取消").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn about(ui: &mut Ui) {
    ui.heading("nbs-rust");
    ui.label("Open Note Block Studio 3.11.0 (GameMaker) 的 Rust 完整重写版");
    ui.separator();
    ui.label(RichText::new("A 数据层").strong());
    for line in [
        "稀疏 BTreeMap 网格、NBS v0–v5 完整读写、自定义乐器",
        "紧凑 diff 事务（撤销只记录变化的格子）",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.label(RichText::new("B 音频 / C 调度").strong());
    for line in [
        "原版 16 个 OGG 采样，44100/48000 统一重采样",
        "pitch = 0.5·2^((key−33+pitch/100)/12)，与原版逐字节对齐",
        "voice 池 256/1024、gain/pan、cpal 实时输出、独立采样时钟",
        "离线多线程混音导出 WAV，MP3/AIFF/FLAC 经 ffmpeg 转码",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.label(RichText::new("D 网格编辑器").strong());
    for line in [
        "视口剔除、批量形状绘制、缩放/平移/边缘自动滚动",
        "框选/减选/浮动选区拖拽/吸管/全量修饰键",
        "图层栏、乐器、时间轴、钢琴、菜单、弹窗、4 套主题、15 个宏",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.label(RichText::new("E 导入导出").strong());
    for line in [
        "MIDI SMF 导入/导出、.nbp 片段、.schematic / 结构 NBT、PNG 谱面",
        "JSON 偏好、最近文件、原子保存、自动备份",
    ] {
        ui.label(RichText::new(line).small());
    }
    ui.separator();
    ui.label(RichText::new("egui 0.31 · glow（可切 wgpu）· cpal · Rust 2021").small().weak());
}

// ---------------------------------------------------------------- B/C/E 弹窗

fn audio_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label("与原版相同：先离线渲染 PCM s16 WAV，MP3/AIFF/FLAC 再交给 ffmpeg 转码。");
    if let Some(s) = &state.export_status {
        ui.label(RichText::new(s).small().color(egui::Color32::from_rgb(20, 130, 60)));
    }
    ui.separator();
    egui::Grid::new("audio_export_grid").num_columns(2).show(ui, |ui| {
        // 原版四个选项：导出格式 / 采样率 / 通道 / 包括已静音的层。
        ui.label("导出格式");
        let mut fmt = state.prefs.export_format.clone();
        egui::ComboBox::from_id_salt("export_format")
            .selected_text(fmt.clone())
            .show_ui(ui, |ui| {
                for f in nbs_io::audio_export::AudioFormat::ALL {
                    ui.selectable_value(&mut fmt, f.name().into(), f.name());
                }
            });
        if fmt != state.prefs.export_format {
            state.prefs.export_format = fmt;
        }
        ui.end_row();
        ui.label("采样率");
        let mut rate = state.prefs.export_sample_rate;
        egui::ComboBox::from_id_salt("export_rate")
            .selected_text(hz_label(rate))
            .show_ui(ui, |ui| {
                for r in nbs_io::audio_export::SAMPLE_RATES {
                    ui.selectable_value(&mut rate, r, hz_label(r));
                }
            });
        if rate != state.prefs.export_sample_rate {
            state.prefs.export_sample_rate = rate;
        }
        ui.end_row();
        ui.label("通道");
        let mut ch = state.prefs.export_channels;
        ui.radio_value(&mut ch, 1, "1（单声道）").on_hover_text("导出单声道的音频。");
        ui.radio_value(&mut ch, 2, "2（立体声）").on_hover_text("导出立体声的音频。");
        if ch != state.prefs.export_channels {
            state.prefs.export_channels = ch;
        }
        ui.end_row();
    });
    let mut locked = state.prefs.include_locked_in_export;
    if ui
        .checkbox(&mut locked, "包括已静音的层")
        .on_hover_text("是否在音频内包含已静音的层。")
        .changed()
    {
        state.prefs.include_locked_in_export = locked;
    }
    let needs_encoder = nbs_io::audio_export::AudioFormat::parse(&state.prefs.export_format)
        .map(|f| f.needs_encoder())
        .unwrap_or(false);
    if needs_encoder {
        // 原版随程序附带 ffmpeg.exe；本程序按 audio_export::find_ffmpeg 的顺序找，
        // 这里显示真实结果，找不到就明说，避免「按了导出却没产物」。
        let line = match nbs_io::audio_export::ffmpeg_version() {
            Some(v) => v,
            None => "未找到 ffmpeg：把它放进程序目录、datafiles\\ 或 PATH，也可设 NBS_FFMPEG".into(),
        };
        ui.label(RichText::new(line).small().weak());
    }
    ui.label(RichText::new(format!(
        "本曲 {} 音符 / {:.1} 秒 / {:.2} t/s",
        state.doc.song.total_blocks,
        state.doc.song.song_length() as f32 / state.doc.song.meta.tempo,
        state.doc.song.meta.tempo
    )).small().weak());
    ui.separator();
    if ui.button("选择输出路径并导出…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ExportAudio));
    }
    footer(state, ui);
}

/// 原版 `format_sample_rate`：44100 → "44,100 Hz"。
fn hz_label(rate: u32) -> String {
    let digits: Vec<char> = rate.to_string().chars().collect();
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    out + " Hz"
}

fn audio_settings_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new(format!("引擎：{}", state.audio.name())).strong());
    ui.label(RichText::new(state.audio.device_info()).small().weak());
    ui.label(RichText::new(state.audio.sample_status()).small().weak());
    ui.label(RichText::new(format!("当前活跃 voice：{}", state.audio.active_voices())).small().weak());
    ui.separator();
    let mut master = state.prefs.master_volume;
    if ui.add(egui::Slider::new(&mut master, 0.0..=2.0).text("主音量")).changed() {
        state.emit(Command::SetMasterVolume(master));
    }
    let mut speed = state.prefs.play_speed;
    if ui.add(egui::Slider::new(&mut speed, 0.25..=4.0).text("播放速度")).changed() {
        state.emit(Command::SetPlaySpeed(speed));
    }
    let mut metronome = state.prefs.metronome;
    if ui.checkbox(&mut metronome, "节拍器（每 4 tick）").changed() {
        state.emit(Command::ToggleMetronome);
    }
    // 以下四格对应原版 `draw_window_preferences.gml` 558-562「播放」组里的四个开关。
    let mut mono = state.prefs.mono;
    if ui.checkbox(&mut mono, "关闭立体声").changed() {
        state.emit(Command::ToggleMono);
    }
    let mut layer_vol = state.prefs.show_layer_volumes;
    if ui.checkbox(&mut layer_vol, "显示每层音量").changed() {
        state.prefs.show_layer_volumes = layer_vol;
        state.emit(Command::SavePreferences);
    }
    let mut soundcount = state.prefs.show_sound_count;
    if ui.checkbox(&mut soundcount, "显示正在播放的声音数").changed() {
        state.prefs.show_sound_count = soundcount;
        state.emit(Command::SavePreferences);
    }
    let mut extended = state.prefs.extended_voices;
    if ui.checkbox(&mut extended, "提升声音限制（1024 voices）").changed() {
        state.emit(Command::ToggleExtendedVoices);
    }
    ui.separator();
    ui.label(RichText::new("速度单位").strong());
    let mut bpm = state.prefs.use_bpm;
    ui.radio_value(&mut bpm, false, "红石刻 / 秒 (t/s)");
    ui.radio_value(&mut bpm, true, "拍数 / 分钟 (BPM)");
    if bpm != state.prefs.use_bpm {
        state.prefs.use_bpm = bpm;
        state.emit(Command::SavePreferences);
    }
    ui.separator();
    ui.label("内置采样目录（留空 = 自动搜索）");
    let mut dir = state.prefs.sounds_dir.clone().unwrap_or_default();
    ui.add(egui::TextEdit::singleline(&mut dir).desired_width(f32::INFINITY).hint_text("自动"));
    if ui.button("应用并重新加载采样").clicked() {
        state.emit(Command::SetSoundsDir(dir));
    }
    ui.separator();
    if ui.button("保存偏好").clicked() {
        state.emit(Command::SavePreferences);
    }
    footer(state, ui);
}

fn midi_import_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label(RichText::new("量化：tick = floor((midi_tick − 起始空白) / (PPQ / (4 × 精度倍率)))").small().weak());
    ui.separator();
    egui::Grid::new("midi_import_grid").num_columns(2).show(ui, |ui| {
        ui.label("时间精度");
        let mut p = state.midi_options.precision;
        egui::ComboBox::from_id_salt("midi_precision")
            .selected_text(format!("{}×", p + 1))
            .show_ui(ui, |ui| {
                for v in [0u8, 1, 3, 7] {
                    ui.selectable_value(&mut p, v, format!("{}×", v + 1));
                }
            });
        state.midi_options.precision = p;
        ui.end_row();
        ui.label("八度偏移");
        ui.add(egui::Slider::new(&mut state.midi_options.octave_shift, -24..=24).suffix(" 半音"));
        ui.end_row();
    });
    ui.checkbox(&mut state.midi_options.remove_silent, "去掉开头空白");
    ui.checkbox(&mut state.midi_options.use_velocity, "使用 MIDI 力度（上限 100）");
    ui.checkbox(&mut state.midi_options.fold_octaves, "折叠到 MC 音域 33..57");
    ui.checkbox(&mut state.midi_options.tempo_changer, "写入 Tempo Changer 分段速度");
    ui.checkbox(&mut state.midi_options.split_channels, "按 MIDI 通道分层");
    ui.separator();
    if ui.button("选择 MIDI 文件并导入…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ImportMidi));
    }
    footer(state, ui);
}

/// 原版 `draw_window_schematic_export`：设计 / 方块两页 + 记住我的更改 + 三个底部按钮。
fn schematic_export_window(state: &mut EditorState, ui: &mut Ui) {
    let mut tab = state.schematic_tab;
    ui.horizontal(|ui| {
        for (i, name) in ["设计", "方块"].iter().enumerate() {
            if ui.selectable_label(tab == i as u8, *name).clicked() {
                tab = i as u8;
            }
        }
    });
    state.schematic_tab = tab;
    ui.separator();

    let stats = state.schematic_stats.clone();
    let names = state.instrument_list();
    let opts = &mut state.schematic_options;
    if tab == 0 {
        design_tab(opts, &stats, ui);
    } else {
        blocks_tab(opts, &names, ui);
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.schematic_remember, "记住我的更改")
            .on_hover_text("下次导出 Schematic 时是否使用同样的设定。");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("使用默认值").clicked() {
                state.modals = ModalQueue::question("确定", "你确定吗？", Command::ResetSchematicOptions);
            }
            if ui.button("取消").clicked() {
                state.emit(Command::CloseWindow);
            }
            if ui.button("导出").clicked() {
                state.emit(Command::ExportSchematicChecked);
            }
        });
    });
}

/// 原版的方块候选菜单：分组标题 + 「id, data — 名称」，当前项高亮。
fn block_menu(
    ui: &mut Ui,
    id: &str,
    current: nbs_io::schematic::McBlock,
    sections: &[nbs_io::schematic::BlockMenuSection],
    label: String,
) -> Option<nbs_io::schematic::McBlock> {
    let mut out = None;
    egui::ComboBox::from_id_salt(id)
        .width(240.0)
        .selected_text(label)
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for sec in sections {
                    if let Some(t) = sec.title {
                        ui.label(RichText::new(t.to_string()).small().strong());
                    }
                    for m in &sec.items {
                        let text =
                            format!("{}, {}  {}", m.id, m.data, nbs_io::schematic::block_display_name(m.id, m.data));
                        if ui.selectable_label(*m == current, text).clicked() {
                            out = Some(*m);
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                }
            });
        });
    out
}

fn design_tab(o: &mut nbs_io::schematic::SchematicOptions, st: &nbs_io::schematic::SchematicStats, ui: &mut Ui) {
    use nbs_io::schematic::{calc_size, export_guard};
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label("分布:");
            if ui.radio(o.layout == 1, "普通过道").on_hover_text("生成过道延伸到歌曲末尾。").clicked() {
                o.layout = 1;
            }
            if ui.radio(o.layout == 0, "环形过道").on_hover_text("生成让玩家来回走的过道。").clicked() {
                o.layout = 0;
            }
            ui.label("导出为 Minecraft 版本:");
            if ui.radio(o.structure, "1.13+").on_hover_text("创建一个兼容 1.13+ 的结构方块文件。").clicked() {
                o.structure = true;
            }
            if ui
                .radio(!o.structure && !o.minecraft_old, "1.11-1.12")
                .on_hover_text("创建一个兼容 1.11 和 1.12 的 Schematic 文件。")
                .clicked()
            {
                o.minecraft_old = false;
                o.structure = false;
            }
            if ui
                .radio(!o.structure && o.minecraft_old, "pre 1.11")
                .on_hover_text("创建一个只兼容 1.11 之前旧版本的 Schematic 文件。")
                .clicked()
            {
                o.minecraft_old = true;
                o.structure = false;
            }
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            let mut n = o.notesperrow as f64;
            ui.horizontal(|ui| {
                ui.label("每行中继器个数:");
                if ui.add(egui::DragValue::new(&mut n).speed(1.0).range(5.0..=100.0)).changed() {
                    o.set_notesperrow(n as usize);
                }
            });
            ui.checkbox(&mut o.include_locked, "包括已静音的层")
                .on_hover_text("是否在 Schematic 内包括已静音的层。");
            ui.checkbox(&mut o.compress, "压缩每层").on_hover_text("压缩每层以节省竖向空间。");
            if o.layout < 2 {
                ui.checkbox(&mut o.minecart, "包括矿车轨道")
                    .on_hover_text("包括一个跟随歌曲进度的矿车轨道。");
                ui.indent("sch_chest", |ui| {
                    ui.add_enabled_ui(o.minecart, |ui| {
                        ui.checkbox(&mut o.chest, "添加矿车箱")
                            .on_hover_text("是否在歌曲开始处添加一个装满矿车的箱子。");
                    });
                });
                if o.layout == 0 {
                    ui.checkbox(&mut o.loop_option, "包括循环选项")
                        .on_hover_text("是否添加开关循环功能的拉杆。");
                }
                if o.structure {
                    ui.checkbox(&mut o.command_block, "使用命令方块")
                        .on_hover_text("是否使用命令方块代替音符盒以获得更广音域。\n（需要更多音符资源包）");
                }
            } else {
                // 原版这里连中文界面都没翻译，照抄。
                ui.checkbox(&mut o.glass, "Create glass floor")
                    .on_hover_text("Whether a glass floor should make all the note blocks visible.");
            }
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            let size = calc_size(st, o);
            let incl = o.include_locked as usize;
            egui::Grid::new("sch_stats").num_columns(2).show(ui, |ui| {
                ui.label("音符盒:");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(st.totalblocks[incl].to_string());
                });
                ui.end_row();
                ui.label("中继器:");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}x{}", st.repeaters[incl][o.tempo_mode as usize], size.layers));
                });
                ui.end_row();
                ui.label("大小:");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}x{}x{}", size.len, size.wid, size.hei));
                });
                ui.end_row();
            });
            ui.label("速度:");
            for (i, name) in ["10 红石刻 / 秒", "5 红石刻 / 秒", "2.5 红石刻 / 秒"].iter().enumerate() {
                if ui.radio(o.tempo_mode == i as u8, *name).on_hover_text("生成对应速度的歌曲").clicked() {
                    o.tempo_mode = i as u8;
                }
            }
            // 超限的红字提示与原版的报错同源。
            if let Err(e) = export_guard(st, o) {
                ui.label(RichText::new(e.replace('\n', " ")).small().color(egui::Color32::from_rgb(200, 90, 90)));
            }
        });
    });
}

fn blocks_tab(
    o: &mut nbs_io::schematic::SchematicOptions,
    names: &[(u8, String)],
    ui: &mut Ui,
) {
    use nbs_io::schematic::{block_display_name, instrument_block_menu, misc_block_menu, McBlock, INS_COUNT};
    let rows = names.len().min(INS_COUNT);
    let mut pick_ins: Option<(usize, McBlock)> = None;
    egui::Grid::new("sch_ins_table").num_columns(3).striped(true).show(ui, |ui| {
        ui.strong("音色");
        ui.strong("方块");
        ui.strong("方块名称");
        ui.end_row();
        for b in 0..rows {
            ui.label(&names[b].1);
            let cur = McBlock {
                id: o.ins_block.get(b).copied().unwrap_or(1),
                data: o.ins_data.get(b).copied().unwrap_or(0),
            };
            let menu = instrument_block_menu(b);
            if let Some(m) = block_menu(
                ui,
                &format!("schexport_insblock_{b}"),
                cur,
                &menu,
                format!("{}, {}", cur.id, cur.data),
            ) {
                pick_ins = Some((b, m));
            }
            ui.label(block_display_name(cur.id, cur.data));
            ui.end_row();
        }
    });
    if let Some((b, m)) = pick_ins {
        // 原版 `menu_click` 的 `schexport_insblock` 分支。
        if b < o.ins_block.len() {
            o.ins_block[b] = m.id;
        }
        if b < o.ins_data.len() {
            o.ins_data[b] = m.data;
        }
    }

    ui.separator();
    let slots = [
        ("过道所用方块:", &mut o.walkway, 0u8, "用于过道和除电路和地面的方块。"),
        ("电路所用方块:", &mut o.circuit, 1, "为电路用的方块。"),
        ("地面所用方块:", &mut o.ground, 2, "为地面用的方块。选草方块会刷一堆生物。"),
    ];
    for (label, slot, which, tip) in slots {
        ui.horizontal(|ui| {
            ui.label(label);
            let cur = *slot;
            let menu = misc_block_menu(which);
            if let Some(m) = block_menu(
                ui,
                &format!("schexport_block_{which}"),
                cur,
                &menu,
                block_display_name(cur.id, cur.data),
            ) {
                *slot = m;
            }
            ui.label(RichText::new(format!("{}, {}", cur.id, cur.data)).small().weak());
        });
        ui.label(RichText::new(tip).small().weak());
    }
}

/// 原版 `draw_window_datapack_export` 里 10 个声音类型的中文标签与悬停说明。
/// 元组第三项写进命令的必须是英文内部值，中文只是界面标签。
const SOUND_SOURCES_ZH: [(&str, &str, &str); 10] = [
    ("ambient", "环境", "由环境音量控制"),
    ("block", "方块", "由方块音量控制"),
    ("hostile", "敌对", "由敌对生物音量控制"),
    ("master", "主源", "由主音量控制"),
    ("music", "音乐", "由音乐音量控制"),
    ("neutral", "中立", "由友好生物音量控制"),
    ("player", "玩家", "由玩家音量控制"),
    ("record", "唱片", "由唱片机/音符盒音量控制"),
    ("voice", "语音", "由语音音量控制"),
    ("weather", "天气", "由天气音量控制"),
];

/// 与 `VisType::ALL` 同序的中文标签。
const VIS_TYPES_ZH: [(&str, &str); 7] = [
    ("弧形", "使用弧形效果器。"),
    ("下落", "使用下落效果器。"),
    ("上升", "使用上升效果器。"),
    ("弹跳", "使用弹跳效果器。"),
    ("钢琴", "使用钢琴式效果器。"),
    ("喷泉", "使用喷泉效果器。"),
    ("立体", "使用立体音效效果器。"),
];

fn datapack_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (i, tab) in ["设置", "效果"].iter().enumerate() {
            if ui.selectable_label(state.datapack_tab == i as u8, *tab).clicked() {
                state.datapack_tab = i as u8;
            }
        }
    });
    ui.separator();

    let tab = state.datapack_tab;
    let enda = state.doc.song.enda as f32;
    // 循环起点属于歌曲自身（原版的拖动条直接改 loopstart），先取到局部再写回。
    let mut loop_start = state.doc.song.meta.loop_start as f32;
    let o = &mut state.datapack_options;
    if tab == 0 {
        egui::Grid::new("datapack_grid").num_columns(2).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label("唯一名称:");
                ui.add(
                    egui::TextEdit::singleline(&mut o.name)
                        .desired_width(145.0)
                        .hint_text("<enter name>"),
                )
                .on_hover_text("该名称会作为在游戏里播放使用的命令名称。");
                ui.label("命名空间:");
                ui.add(
                    egui::TextEdit::singleline(&mut o.namespace)
                        .desired_width(145.0)
                        .hint_text("(可选)"),
                )
                .on_hover_text("（可选）将命令名称更改到自定义命名空间下。如为空，命名空间会设为歌曲名称。");
                ui.label("路径:");
                ui.add(
                    egui::TextEdit::singleline(&mut o.path)
                        .desired_width(145.0)
                        .hint_text("(可选)"),
                )
                .on_hover_text("（可选）从“functions”文件夹到歌曲的路径。用“/”来套文件夹。");
                ui.label("命令预览:");
                let preview = nbs_io::datapack::command_preview(&o.name, &o.namespace, &o.path);
                ui.add(
                    egui::Label::new(RichText::new(preview).monospace().strong())
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Truncate),
                );
                ui.label("声音类型");
                for (id, zh, tip) in &SOUND_SOURCES_ZH[..5] {
                    if ui.radio(o.source == *id, *zh).on_hover_text(*tip).clicked() {
                        o.source = (*id).to_string();
                    }
                }
                ui.label("Minecraft 版本");
                if ui
                    .radio(o.mc_version == 0, "1.13-1.20")
                    .on_hover_text("导出至 Minecraft: Java 版\n版本 1.13 至 1.20.6。")
                    .clicked()
                {
                    o.mc_version = 0;
                }
                if ui
                    .radio(o.mc_version == 1, "1.21+")
                    .on_hover_text("导出至 Minecraft: Java 版\n版本 1.21 及以上。")
                    .clicked()
                {
                    o.mc_version = 1;
                }
                ui.checkbox(&mut o.use_zip, "导出为 ZIP")
                    .on_hover_text("是否导出为 ZIP 格式的文件。如关闭，将导出为文件夹。");
            });
            ui.vertical(|ui| {
                ui.label(" ");
                ui.label(" ");
                ui.label(" ");
                ui.checkbox(&mut o.include_locked, "包括已静音的层")
                    .on_hover_text("是否在数据包内包括已静音的层。");
                ui.checkbox(&mut o.include_out_of_range, "包括超范围音符")
                    .on_hover_text("是否包括超过 Minecraft 的 2 八度限制的音符。此选项需要安装额外的数据包。");
                ui.checkbox(&mut o.enable_radius, "共享音乐")
                    .on_hover_text("是否共享音乐给一定范围内的玩家。");
                ui.horizontal(|ui| {
                    ui.label("范围（方块）:");
                    let mut r = o.radius as f32;
                    ui.add_enabled_ui(o.enable_radius, |ui| {
                        if ui
                            .add(egui::DragValue::new(&mut r).range(16.0..=100000.0).speed(2.0))
                            .changed()
                        {
                            o.radius = (r as i64).clamp(16, 100_000) as u32;
                        }
                    });
                });
                ui.label(RichText::new(format!("实际音量半径 = {:.2}", o.radius_value()))
                    .small()
                    .weak());
                ui.checkbox(&mut o.enable_looping, "启用循环")
                    .on_hover_text("如果开启，歌曲播放到最后将会循环。");
                ui.horizontal(|ui| {
                    ui.label("循环开始:");
                    ui.add_enabled_ui(o.enable_looping, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut loop_start)
                                .range(0.0..=enda)
                                .speed(0.5),
                        )
                        .on_hover_text("播放完循环时回到的时刻。");
                    });
                });
            });
            ui.end_row();
        });
    } else {
        ui.checkbox(&mut o.visualizer, "启用效果器")
            .on_hover_text("注意：请使用一个虚空地图不然方块就会堆起来！");
        ui.label("效果类型");
        for (vis, (zh, tip)) in nbs_io::datapack::VisType::ALL.iter().zip(VIS_TYPES_ZH) {
            if ui.radio(o.vis_type == *vis, zh).on_hover_text(tip).clicked() {
                o.vis_type = *vis;
            }
        }
        ui.checkbox(&mut o.glow, "发光效果")
            .on_hover_text("是否为每个音符盒添加发光效果。");
        ui.label("生成高度:");
        ui.add(egui::DragValue::new(&mut o.y_val).range(0.0..=255.0).speed(0.5));
        ui.label("地图中位置:");
        ui.horizontal(|ui| {
            ui.label("X值:");
            ui.add(egui::DragValue::new(&mut o.x_val).speed(0.5));
        });
        ui.horizontal(|ui| {
            ui.label("Z值:");
            ui.add(egui::DragValue::new(&mut o.z_val).speed(0.5));
        });
        if o.visualizer {
            ui.label(RichText::new(format!(
                "效果器类型：{}，音符盒生成在 y = {}",
                o.vis_type.gml_name(),
                o.y_val
            ))
            .small()
            .weak());
        }
    }

    if loop_start != state.doc.song.meta.loop_start as f32 {
        state.doc.edit_meta("设置循环起点", |m| m.loop_start = loop_start as u16);
        state.sync_loop_params();
    }

    if let Some(s) = &state.export_status {
        ui.separator();
        ui.label(RichText::new(s).small());
    }
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("导出").clicked() {
            state.emit(Command::OpenDialog(crate::state::DialogAction::ExportDatapack));
        }
        if ui.button("使用默认值").clicked() {
            // 原版是 dat_reset(1) + 一个确认框；这里直接重置并给出提示。
            state.datapack_options = nbs_io::datapack::DatapackOptions::default();
            state.export_status = None;
            state.set_message("已恢复数据包导出默认设置");
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn png_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label("把整首歌的网格画成 PNG 图片，颜色对应乐器，亮度对应力度。");
    ui.label(RichText::new("自动缩小格子以适配最大像素尺寸；不受屏幕视口限制。").small().weak());
    ui.separator();
    if let Some(s) = &state.export_status {
        ui.label(RichText::new(s).small().weak());
    }
    if ui.button("选择输出路径并导出…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ExportPng));
    }
    footer(state, ui);
}

fn macro_editor_window(state: &mut EditorState, ui: &mut Ui) {
    let macros = Macro::all();
    ui.label(RichText::new(format!(
        "作用对象：{}（无选区时对全曲生效）",
        if state.doc.selection.is_empty() {
            "全曲".to_string()
        } else {
            format!("浮动选区 {} 音符", state.doc.selection.len())
        }
    )).small().weak());
    ui.separator();
    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
        for (i, m) in macros.iter().enumerate() {
            if ui.selectable_label(state.macro_index == i, m.name()).clicked() {
                state.macro_index = i;
                state.macro_amount = match i {
                    6 => 2.0,   // Tremolo spacing
                    7 => 60.0,  // Stereo width
                    8 => 0.0,   // Arpeggio 用 pattern
                    9 => 300.0, // Portamento cents
                    12 => 100.0,
                    15 => 0.0,
                    _ => 20.0,
                };
            }
        }
    });
    ui.separator();
    ui.add(egui::Slider::new(&mut state.macro_amount, -1200.0..=1200.0).text("参数值"));
    ui.horizontal(|ui| {
        ui.label("序列 (| 分隔)");
        ui.add(egui::TextEdit::singleline(&mut state.macro_pattern).desired_width(160.0));
    });
    ui.checkbox(&mut state.macro_reverse, "反转 / 叠加模式");
    ui.label(RichText::new(format!("当前：{}", state.current_macro().name())).small().weak());
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("应用宏").clicked() {
            state.emit(Command::RunMacro(state.current_macro()));
        }
        if ui.button("关闭").clicked() {
            state.emit(Command::CloseWindow);
        }
    });
}

fn autosaves_window(state: &mut EditorState, ui: &mut Ui) {
    let files = nbs_io::prefs::list_autosaves();
    ui.label(RichText::new(format!("备份目录：{}", nbs_io::prefs::Preferences::backup_dir().display())).small().weak());
    ui.separator();
    if files.is_empty() {
        ui.label("暂无自动备份");
    }
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
        for p in files {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if ui.button(&name).clicked() {
                state.emit(Command::RestoreAutosave(p.clone()));
            }
        }
    });
    ui.separator();
    ui.add(egui::Slider::new(&mut state.prefs.auto_save_minutes, 0..=60).text("自动保存间隔 (分钟，0=关闭)"));
    footer(state, ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_label_inserts_the_original_commas() {
        assert_eq!(hz_label(44100), "44,100 Hz");
        assert_eq!(hz_label(8000), "8,000 Hz");
        assert_eq!(hz_label(384000), "384,000 Hz");
        assert_eq!(hz_label(192000), "192,000 Hz");
    }
}
