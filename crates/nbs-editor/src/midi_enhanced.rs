use egui::{ComboBox, DragValue, RichText, Ui};
use nbs_core::instruments::instrument_name;
use nbs_io::midi::{MidiData, MidiImportOptions};
use nbs_io::midi_enhanced::{
    import_midi_enhanced, EnhancedImportReport, EnhancedMidiOptions, RangeMode,
};

use crate::{commands::Command, tr};

fn range_name(mode: RangeMode) -> &'static str {
    match mode {
        RangeMode::Keep => tr("Keep range / manual transpose", "保留音域／手动移调"),
        RangeMode::Fold => tr("Fold each note by octaves", "逐音八度折叠"),
        RangeMode::Octave => tr("Recommend octave shift", "推荐整体八度偏移"),
        RangeMode::Semitone => tr("Recommend semitone shift", "推荐整体半音偏移"),
    }
}

fn instrument_choice(ui: &mut Ui, id: impl std::hash::Hash, value: &mut Option<u8>) -> bool {
    let before = *value;
    ComboBox::from_id_salt(id)
        .width(126.0)
        .selected_text(
            value
                .map(instrument_name)
                .unwrap_or_else(|| tr("None", "不生成").into()),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, tr("None", "不生成"));
            for instrument in 0..20 {
                ui.selectable_value(value, Some(instrument), instrument_name(instrument));
            }
        });
    before != *value
}

pub(crate) fn show(
    ui: &mut Ui,
    midi: &MidiData,
    options: &mut MidiImportOptions,
    enhanced: &mut EnhancedMidiOptions,
    checked: &mut Option<Result<EnhancedImportReport, String>>,
) -> Option<Command> {
    ui.set_min_width(560.0);
    let before = (options.clone(), enhanced.clone());
    let mut recommend = false;
    let mut action = None;
    let max_height = (ui.ctx().screen_rect().height() - 210.0).clamp(200.0, 570.0);
    egui::ScrollArea::vertical()
        .id_salt("enhanced_midi_options")
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.label(tr(
                "Melody combinations + native note-block range (NBS key 33..57)",
                "旋律组合配器＋原生音符盒音域（NBS key 33..57）",
            ));
            ui.label(RichText::new(tr(
                "This file only. Traditional GM mappings stay unchanged; drums use the traditional percussion page.",
                "配置仅用于本次文件，不写回传统 GM 表；鼓组沿用传统页的打击乐映射。",
            )).weak());
            ui.collapsing(tr("Timing and note generation", "时序与音符生成"), |ui| {
                common_options(ui, midi, options);
            });
            if options.max_height < 20 {
                ui.colored_label(ui.visuals().warn_fg_color, format!(
                    "{} {}{}", tr("Per-channel layer limit:", "每通道层数上限："), options.max_height,
                    tr("; excess voices may be omitted. Adjust above.", "；超限声部可能被省略，可展开上方设置调整。"),
                ));
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Range strategy", "音域策略"));
                let old_mode = enhanced.range_mode;
                ComboBox::from_id_salt("enhanced_range_mode")
                    .selected_text(range_name(enhanced.range_mode))
                    .show_ui(ui, |ui| {
                        for mode in [RangeMode::Keep, RangeMode::Fold, RangeMode::Octave, RangeMode::Semitone] {
                            ui.selectable_value(&mut enhanced.range_mode, mode, range_name(mode));
                        }
                    });
                recommend |= old_mode != enhanced.range_mode;
                if ui.button(tr("Recommend shifts", "重新推荐偏移")).clicked() {
                    recommend = true;
                }
            });
            let supports_substitution = matches!(enhanced.range_mode, RangeMode::Octave | RangeMode::Semitone);
            ui.add_enabled_ui(supports_substitution, |ui| {
                ui.horizontal_wrapped(|ui| {
                    recommend |= ui.checkbox(&mut enhanced.smart_substitute, tr("Substitute out-of-range instruments", "越界时换乐器接续")).changed();
                    ui.checkbox(&mut enhanced.force_fold, tr("Fold remaining out-of-range notes", "残余越界音强制八度折叠"));
                });
            });
            ui.label(RichText::new(tr(
                "Substitute first, then shift, then optionally fold. Substitution compensates instrument octave offsets; shifting/folding transposes notes.",
                "先替代、再整体偏移、最后可选折叠；替代按内置乐器八度差补偿，偏移／折叠会改变音高。",
            )).small());
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Reset combinations", "重设组合"));
                for (count, label) in [(1, tr("One voice", "单槽")), (2, tr("Two voices", "双槽")), (3, tr("Three voices", "三槽"))] {
                    if ui.button(label).clicked() {
                        enhanced.reset_combinations(midi, count);
                    }
                }
            });
            let mut used = [false; 16];
            for track in &midi.tracks {
                for event in &track.events {
                    if event.channel < 16 && event.channel != 9 {
                        used[event.channel as usize] = true;
                    }
                }
            }
            if !used.iter().any(|used| *used) {
                ui.label(tr("No melodic notes in this file.", "此文件没有旋律通道音符。"));
            }
            for (channel, used) in used.into_iter().enumerate() {
                if !used {
                    continue;
                }
                ui.push_id(channel, |ui| {
                    ui.group(|ui| {
                        let combination = &mut enhanced.channels[channel];
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(format!("{} {} · {}", tr("Channel", "通道"), channel + 1,
                                nbs_io::MidiGmTable::patch_name(midi.channel_patch[channel] as u8)));
                            ui.label(tr("Shift", "偏移"));
                            ui.add(DragValue::new(&mut combination.transpose).range(-72..=72).suffix(tr(" st", " 半音")));
                        });
                        ui.horizontal_wrapped(|ui| {
                            for (slot_index, slot) in combination.slots.iter_mut().enumerate() {
                                ui.vertical(|ui| {
                                    ui.label(format!("{} {}", tr("Slot", "槽位"), slot_index + 1));
                                    recommend |= instrument_choice(ui, ("slot", slot_index), &mut slot.instrument);
                                    ui.add_enabled_ui(slot.instrument.is_some(), |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(tr("Volume", "音量"));
                                            recommend |= ui.add(DragValue::new(&mut slot.volume).range(0..=100).suffix("%")).changed();
                                        });
                                    });
                                });
                            }
                        });
                    });
                });
            }
            ui.label(RichText::new(tr(
                "Zero-volume/empty slots generate no notes. Equal keys on different instruments may intentionally sound in different octaves.",
                "零音量／空槽不生成音符；不同乐器的同一 key 可能形成有意的跨八度叠加。",
            )).small());
            ui.collapsing(tr("Instrument substitution chains", "高低音替代链"), |ui| {
                ui.add_enabled_ui(supports_substitution && enhanced.smart_substitute, |ui| {
                    egui::Grid::new("enhanced_substitution_grid").striped(true).show(ui, |ui| {
                        ui.label(tr("Instrument", "乐器"));
                        ui.label(tr("Above range", "高音越界"));
                        ui.label(tr("Below range", "低音越界"));
                        ui.end_row();
                        for (instrument, rule) in enhanced.substitutes.iter_mut().enumerate() {
                            ui.label(instrument_name(instrument as u8));
                            recommend |= instrument_choice(ui, ("high", instrument), &mut rule.high);
                            recommend |= instrument_choice(ui, ("low", instrument), &mut rule.low);
                            ui.end_row();
                        }
                    });
                });
            });
            ui.collapsing(tr("Minecraft output and volume", "Minecraft 输出与音量"), |ui| {
                ui.label(tr(
                    "Note volume = MIDI volume / 127 × slot volume. Layer volume is applied during playback/export; no velocity-dependent timbre model is used.",
                    "音符音量由 MIDI 音量按 127 归一化后乘槽位音量；播放／导出时再乘层音量，不模拟击键力度音色。",
                ));
                ui.label(tr(
                    "Physical note blocks have no volume field. Non-radius datapacks preserve note volume; radius mode ignores it. This import does not turn volume into spatial placement.",
                    "实体音符盒没有音量字段；数据包非半径模式保留逐音音量，半径模式忽略它；本导入不将音量转换成空间摆放距离。",
                ));
            });
        });
    if recommend {
        enhanced.recommend_offsets(midi);
    }
    if before != (options.clone(), enhanced.clone()) {
        *checked = None;
    }
    ui.separator();
    if let Some(result) = checked.as_ref() {
        match result {
            Ok(report) => {
                ui.label(format!(
                    "{} {} · {} {}",
                    tr("Generated notes:", "生成音符："),
                    report.notes,
                    tr("Outside key 33..57:", "超出 key 33..57："),
                    report.out_of_range
                ));
                ui.label(format!(
                    "{} {} · {} {}",
                    tr("Substituted slot events:", "替代槽位事件："),
                    report.substituted,
                    tr("Folded slot events:", "折叠槽位事件："),
                    report.folded
                ));
                if report.out_of_range > 0 {
                    ui.colored_label(ui.visuals().warn_fg_color, tr(
                        "Out-of-range notes remain in NBS but are not playable by native note blocks.",
                        "残余越界音保留在 NBS 中，但原生音符盒无法播放。",
                    ));
                }
            }
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button(tr("Check result", "检查结果")).clicked() {
            *checked =
                Some(import_midi_enhanced(midi, options, enhanced).map(|(_, report)| report));
        }
        if ui
            .button(tr("Reset combinations and range", "重置配器和音域"))
            .clicked()
        {
            *enhanced = EnhancedMidiOptions::for_midi(midi);
            enhanced.enabled = true;
            *checked = None;
        }
        if ui.button(tr("Cancel", "取消")).clicked() {
            action = Some(Command::MidiImportCancel);
        }
        if ui.button(tr("Import", "导入")).clicked() {
            action = Some(Command::MidiImportRun);
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
    use nbs_io::midi::{MidiEvent, MidiPart, MidiTrack};

    fn fixture() -> MidiData {
        MidiData {
            division: 480,
            minpos: 0,
            maxpos: 480,
            micsecqn: 500_000,
            tracks: vec![MidiTrack {
                name: "Piano".into(),
                events: vec![MidiEvent {
                    pos: 0,
                    channel: 0,
                    note: 60,
                    velocity: 100,
                    patch: 0,
                    part: 0,
                    until: 480,
                }],
            }],
            parts: vec![MidiPart {
                track: 0,
                channel: 0,
                patch: 0,
                note_duration: true,
            }],
            ..MidiData::default()
        }
    }

    struct Harness {
        context: Context,
        midi: MidiData,
        options: MidiImportOptions,
        enhanced: EnhancedMidiOptions,
        checked: Option<Result<EnhancedImportReport, String>>,
    }

    impl Harness {
        fn new() -> Self {
            let midi = fixture();
            let options = MidiImportOptions::for_midi(&midi, &nbs_io::MidiGmTable::default());
            let mut enhanced = EnhancedMidiOptions::for_midi(&midi);
            enhanced.enabled = true;
            Self {
                context: Context::default(),
                midi,
                options,
                enhanced,
                checked: None,
            }
        }

        fn frame(&mut self, events: Vec<Event>) -> (FullOutput, Option<Command>) {
            let mut command = None;
            let output = self.context.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 900.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        command = show(
                            ui,
                            &self.midi,
                            &mut self.options,
                            &mut self.enhanced,
                            &mut self.checked,
                        );
                    });
                },
            );
            (output, command)
        }

        fn click(&mut self, text: &str) -> Option<Command> {
            self.frame(vec![]);
            let (output, _) = self.frame(vec![]);
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| text_position(&shape.shape, text))
                .unwrap_or_else(|| panic!("missing widget: {text}"));
            self.frame(vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ]);
            self.frame(vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }])
            .1
        }
    }

    fn text_position(shape: &egui::epaint::Shape, expected: &str) -> Option<Pos2> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == expected => {
                Some(text.pos + text.galley.rect.center().to_vec2())
            }
            egui::epaint::Shape::Vec(shapes) => shapes
                .iter()
                .find_map(|shape| text_position(shape, expected)),
            _ => None,
        }
    }

    #[test]
    fn buttons_emit_import_and_cancel_commands() {
        let _lock = crate::frame_lock();
        let _lang = crate::LangGuard::set(0);
        let mut harness = Harness::new();
        assert!(matches!(
            harness.click("Import"),
            Some(Command::MidiImportRun)
        ));
        assert!(matches!(
            harness.click("Cancel"),
            Some(Command::MidiImportCancel)
        ));
    }

    #[test]
    fn one_voice_preset_and_check_use_the_live_options() {
        let _lock = crate::frame_lock();
        let _lang = crate::LangGuard::set(0);
        let mut harness = Harness::new();
        harness.click("One voice");
        assert_eq!(
            harness.enhanced.channels[0]
                .slots
                .iter()
                .filter(|slot| slot.instrument.is_some())
                .count(),
            1
        );
        harness.click("Check result");
        let report = harness.checked.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(report.notes, 1);
        assert_eq!(report.out_of_range, 0);
        harness.click("Reset combinations and range");
        assert!(harness.checked.is_none());
        assert!(harness.enhanced.enabled);
        assert_eq!(
            harness.enhanced.channels[0]
                .slots
                .iter()
                .filter(|slot| slot.instrument.is_some())
                .count(),
            2
        );
    }

    #[test]
    fn many_channels_keep_import_action_visible() {
        let _lock = crate::frame_lock();
        let _lang = crate::LangGuard::set(0);
        let mut harness = Harness::new();
        let event = harness.midi.tracks[0].events[0].clone();
        harness.midi.channels = 15;
        harness.midi.tracks[0].events = (0..16)
            .map(|channel| MidiEvent {
                channel,
                ..event.clone()
            })
            .collect();
        harness.enhanced = EnhancedMidiOptions::for_midi(&harness.midi);
        harness.enhanced.enabled = true;
        assert!(matches!(
            harness.click("Import"),
            Some(Command::MidiImportRun)
        ));
    }

    #[test]
    fn empty_midi_can_be_checked() {
        let _lock = crate::frame_lock();
        let _lang = crate::LangGuard::set(0);
        let mut harness = Harness::new();
        harness.midi.tracks.clear();
        harness.midi.parts.clear();
        harness.click("Check result");
        assert_eq!(harness.checked.as_ref().unwrap().as_ref().unwrap().notes, 0);
    }

    #[test]
    fn invalid_pitch_is_reported_without_mutating_input() {
        let _lock = crate::frame_lock();
        let _lang = crate::LangGuard::set(0);
        let mut harness = Harness::new();
        harness.midi.tracks[0].events[0].note = 0;
        harness.enhanced.range_mode = RangeMode::Keep;
        harness.enhanced.channels[0].transpose = 0;
        harness.click("Check result");
        assert!(harness.checked.as_ref().unwrap().is_err());
        assert_eq!(harness.midi.tracks[0].events[0].note, 0);
    }
}

fn common_options(ui: &mut Ui, midi: &MidiData, options: &mut MidiImportOptions) {
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(
            &mut options.remove_silent,
            tr("Trim leading silence", "去除开头静音"),
        );
        ui.checkbox(
            &mut options.use_velocity,
            tr("Read MIDI volume", "读取 MIDI 音量"),
        );
        ui.checkbox(&mut options.name_layers, tr("Name layers", "命名图层"));
        ui.checkbox(
            &mut options.name_after_patch,
            tr("Use GM names", "使用 GM 名称"),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut options.tempo_from_file, tr("File tempo", "文件速度"));
        ui.checkbox(
            &mut options.tempo_changer,
            tr("Tempo changes", "保留速度变化"),
        );
        ComboBox::from_id_salt("enhanced_precision")
            .selected_text(format!("{}×", options.precision as u16 + 1))
            .show_ui(ui, |ui| {
                for precision in [0, 1, 3, 7] {
                    ui.selectable_value(
                        &mut options.precision,
                        precision,
                        format!("{}×", precision + 1),
                    );
                }
            });
        ui.label(tr(
            "Max layers/channel (20 = unlimited)",
            "通道层数上限（20 为不限）",
        ));
        ui.add(DragValue::new(&mut options.max_height).range(1..=20));
    });
    ui.checkbox(
        &mut options.note_duration,
        tr("Repeat notes to retain duration", "重复音符以保留时值"),
    );
    ui.add_enabled_ui(options.note_duration, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut options.note_duration_fade,
                tr("Custom tail volume", "自定义尾音音量"),
            );
            ui.add_enabled_ui(options.note_duration_fade, |ui| {
                ui.add(
                    DragValue::new(&mut options.note_duration_fade_start)
                        .range(0..=100)
                        .prefix(tr("Start ", "起始 "))
                        .suffix("%"),
                );
                ui.add(
                    DragValue::new(&mut options.note_duration_fade_end)
                        .range(0..=100)
                        .prefix(tr("End ", "结束 "))
                        .suffix("%"),
                );
            });
        });
        ui.collapsing(
            tr("Parts with note duration", "保留时值的声部"),
            |ui| {
                for (index, part) in midi.parts.iter().enumerate() {
                    if let Some(enabled) = options.part_durations.get_mut(index) {
                        ui.add_enabled_ui(part.channel != 9, |ui| {
                            ui.checkbox(
                                enabled,
                                format!(
                                    "{} {} / {} {} · {}",
                                    tr("Track", "轨道"),
                                    part.track + 1,
                                    tr("Channel", "通道"),
                                    part.channel + 1,
                                    nbs_io::MidiGmTable::patch_name(part.patch as u8)
                                ),
                            );
                        });
                    }
                }
            },
        );
    });
}
