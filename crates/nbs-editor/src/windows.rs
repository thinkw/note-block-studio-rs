//! 弹窗集合：歌曲信息 / 属性 / 统计 / 首选项 / 音色 / 宏窗口 / 文件对话框 /
//! 图层重命名 / 快捷键帮助 / 关于。原版 47 个 `w_*` 窗口状态收敛成一个枚举。

use egui::{Context, RichText, Ui};
use nbs_core::instruments::{instrument_name, INSTRUMENTS};
use nbs_core::model::{key_name, SongMeta};
use nbs_core::Macro;

use crate::commands::Command;
use crate::shortcuts::SHORTCUT_HELP;
use crate::state::{EditorState, Flag, WindowKind};
use crate::theme::ThemeKind;

pub fn show_windows(state: &mut EditorState, ctx: &Context) {
    let Some(kind) = state.window.clone() else { return; };
    let mut open = true;
    let title = kind.title().to_string();
    let default_width = match kind {
        WindowKind::Preferences | WindowKind::Shortcuts => 460.0,
        WindowKind::FileOpen | WindowKind::FileSave => 520.0,
        WindowKind::ClipEditor => 500.0,
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
            WindowKind::PngExport => png_export_window(state, ui),
            WindowKind::MacroEditor => macro_editor_window(state, ui),
            WindowKind::Autosaves => autosaves_window(state, ui),
        });
    if !open {
        state.window = None;
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
        Flag::ShowLayers, Flag::ShowPiano, Flag::ShowInstruments,
        Flag::UseColors, Flag::UseShapes, Flag::UseIcons,
        Flag::ShowNumbers, Flag::ShowOctaves, Flag::ShowKeyNames,
        Flag::ShowIncompatible, Flag::ShowGridLines, Flag::Flats, Flag::Blackout,
    ] {
        let mut v = Flag::get(flag, &state.flags);
        if ui.checkbox(&mut v, flag.label()).changed() {
            state.emit(Command::ToggleFlag(flag));
        }
    }
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
    let action = state
        .file_dialog
        .as_ref()
        .map(|d| d.action)
        .unwrap_or(crate::state::DialogAction::OpenSong);
    ui.label(
        RichText::new(format!(
            "文件类型：*.{}   {}",
            action.extension(),
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
                    path.with_extension(d.action.extension())
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
        "离线多线程 WAV 混音导出",
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
    ui.label("按与原版相同的 pitch/gain/pan 公式离线混音，输出 PCM s16 WAV。");
    if let Some(s) = &state.export_status {
        ui.label(RichText::new(s).small().color(egui::Color32::from_rgb(20, 130, 60)));
    }
    ui.separator();
    egui::Grid::new("audio_export_grid").num_columns(2).show(ui, |ui| {
        ui.label("采样率");
        let mut rate = state.prefs.export_sample_rate;
        egui::ComboBox::from_id_salt("export_rate")
            .selected_text(format!("{} Hz", rate))
            .show_ui(ui, |ui| {
                for r in [22050u32, 44100, 48000, 88200, 96000] {
                    ui.selectable_value(&mut rate, r, format!("{r} Hz"));
                }
            });
        if rate != state.prefs.export_sample_rate {
            state.prefs.export_sample_rate = rate;
        }
        ui.end_row();
        ui.label("声道");
        ui.horizontal(|ui| {
            if ui.selectable_label(state.prefs.export_channels == 1, "单声道").clicked() {
                state.prefs.export_channels = 1;
            }
            if ui.selectable_label(state.prefs.export_channels == 2, "立体声").clicked() {
                state.prefs.export_channels = 2;
            }
        });
        ui.end_row();
    });
    ui.checkbox(&mut state.prefs.export_normalize, "导出时峰值归一化");
    ui.checkbox(&mut state.prefs.include_locked_in_export, "包含锁定图层");
    ui.add(egui::Slider::new(&mut state.prefs.export_tail_seconds, 0.0..=8.0).text("尾部留白 (秒)"));
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
    let mut mono = state.prefs.mono;
    if ui.checkbox(&mut mono, "强制单声道").changed() {
        state.emit(Command::ToggleMono);
    }
    let mut extended = state.prefs.extended_voices;
    if ui.checkbox(&mut extended, "更多音频通道（1024 voices）").changed() {
        state.emit(Command::ToggleExtendedVoices);
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

fn schematic_export_window(state: &mut EditorState, ui: &mut Ui) {
    ui.label("生成 Minecraft 原理图：音符盒 + 下方乐器方块 + 红石线/中继器。");
    ui.label(RichText::new(format!(
        "中继器延迟由 tempo 自动映射：{:.2} t/s → {} 档",
        state.doc.song.meta.tempo,
        nbs_io::schematic::tempo_to_delay(state.doc.song.meta.tempo)
    )).small().weak());
    ui.checkbox(&mut state.schematic_options.structure, "1.13+ 结构方块格式 (.nbt，palette + blocks)");
    ui.label(RichText::new("只包含音高 33..57 的音符（Minecraft 原生音域）").small().weak());
    ui.separator();
    if ui.button("选择输出路径并导出…").clicked() {
        state.emit(Command::OpenDialog(crate::state::DialogAction::ExportSchematic));
    }
    footer(state, ui);
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
