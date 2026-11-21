use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, ViewportCommand};
use nbs_editor::{commands::Command, EditorState, WindowKind};
use nbs_io::midi::{MidiData, MidiEvent, MidiImportOptions, MidiPart, MidiTrack};
use nbs_io::midi_enhanced::{EnhancedMidiOptions, MidiSlot};

#[derive(PartialEq)]
enum Phase {
    Dialog,
    WaitingDialog,
    Imported,
    WaitingImported,
    Done,
}

struct Probe {
    state: EditorState,
    output: PathBuf,
    phase: Phase,
    frames: usize,
    started: Instant,
}

impl Probe {
    fn new(cc: &eframe::CreationContext<'_>, output: PathBuf) -> Self {
        let mut state = EditorState::new();
        state.prefs.language = 1;
        state.prefs.check_update = false;
        state.theme.apply(&cc.egui_ctx);
        nbs_editor::fonts::install(&cc.egui_ctx);
        let midi = MidiData {
            division: 480,
            minpos: 0,
            maxpos: 480,
            micsecqn: 500_000,
            tracks: vec![MidiTrack {
                name: "Synthetic melody".into(),
                events: [60, 90]
                    .into_iter()
                    .enumerate()
                    .map(|(index, note)| MidiEvent {
                        pos: index as i64 * 240,
                        channel: 0,
                        note,
                        velocity: 100,
                        patch: 0,
                        part: 0,
                        until: index as i64 * 240 + 240,
                    })
                    .collect(),
            }],
            parts: vec![MidiPart {
                track: 0,
                channel: 0,
                patch: 0,
                note_duration: true,
            }],
            ..MidiData::default()
        };
        state.midi_options = MidiImportOptions::for_midi(&midi, &state.midi_gm);
        state.midi_enhanced_options = state.midi_options.clone();
        state.midi_enhanced = EnhancedMidiOptions::for_midi(&midi);
        state.midi_enhanced.enabled = true;
        state.midi_enhanced.channels[0].slots = [
            MidiSlot {
                instrument: Some(0),
                volume: 100,
            },
            MidiSlot {
                instrument: Some(6),
                volume: 50,
            },
            MidiSlot {
                instrument: Some(5),
                volume: 25,
            },
        ];
        state.midi_enhanced.recommend_offsets(&midi);
        state.midi_data = Some(midi);
        state.midi_file = "synthetic.mid".into();
        state.window = Some(WindowKind::MidiImport);
        Self {
            state,
            output,
            phase: Phase::Dialog,
            frames: 0,
            started: Instant::now(),
        }
    }

    fn save_screenshot(&self, image: &egui::ColorImage, name: &str) {
        let pixels: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|color| color.to_array())
            .collect();
        image::save_buffer(
            self.output.join(name),
            &pixels,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ColorType::Rgba8,
        )
        .expect("save native screenshot");
    }
}

impl eframe::App for Probe {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        assert!(
            self.started.elapsed() < Duration::from_secs(30),
            "native screenshot timed out"
        );
        let images = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| {
                    if let Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        for image in images {
            if self.phase == Phase::WaitingDialog {
                self.save_screenshot(&image, "enhanced-dialog.png");
                self.state.emit(Command::MidiImportRun);
                self.phase = Phase::Imported;
                self.frames = 0;
            } else if self.phase == Phase::WaitingImported {
                self.save_screenshot(&image, "enhanced-song.png");
                self.phase = Phase::Done;
                println!(
                    "Native import passed; screenshots and synthetic.nbs: {}",
                    self.output.display()
                );
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
        nbs_editor::show(ctx, &mut self.state);
        self.frames += 1;
        if self.phase == Phase::Dialog && self.frames >= 5 {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            self.phase = Phase::WaitingDialog;
        } else if self.phase == Phase::Imported && self.frames >= 5 {
            assert!(self.state.window.is_none());
            assert_eq!(self.state.doc.song.total_blocks, 6);
            assert!(self
                .state
                .doc
                .song
                .iter_notes()
                .all(|(_, _, note)| (33..=57).contains(&note.key)));
            let mut volumes: Vec<u8> = self
                .state
                .doc
                .song
                .iter_notes()
                .map(|(_, _, note)| note.velocity)
                .collect();
            volumes.sort_unstable();
            assert_eq!(volumes, [20, 20, 39, 39, 78, 78]);
            nbs_core::nbs::save(
                &self.output.join("synthetic.nbs"),
                &self.state.doc.song,
                nbs_core::nbs::NBS_VERSION,
            )
            .expect("save synthetic NBS");
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            self.phase = Phase::WaitingImported;
        }
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

fn main() -> eframe::Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("Usage: midi_import_probe <existing output directory>"),
    );
    assert!(output.is_dir());
    let config = PathBuf::from(
        std::env::var_os("NBS_RUST_CONFIG_DIR")
            .expect("Set NBS_RUST_CONFIG_DIR to an isolated verification directory"),
    );
    assert!(
        config.is_absolute() && config.starts_with(&output),
        "config must be inside the output directory"
    );
    eframe::run_native(
        "Enhanced MIDI import verification",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 900.0]),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(Probe::new(cc, output)))),
    )
}
