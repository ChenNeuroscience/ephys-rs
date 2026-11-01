//! Video from a camera, captured on a thread of its own (see `tracking::capture`) and taken into each pass.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::capture::{Capture, State, Status};
use super::{CameraControls, CameraSettings, Control, Range, Source};
use crate::core::engine::{Category, Context, Node, NodeInfo, Output, Pass, SettingPort};
use crate::standard_components::tracking::Video;
use crate::standard_components::widgets::{setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub source: Source,
    /// The frame size: frames of other sizes are skipped. (The synthetic camera makes this size.)
    pub size: [u32; 2],
    pub settings: CameraSettings,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { source: Source::Synthetic, size: [640, 480], settings: CameraSettings::default() }
    }
}

/// Capturing, while running.
#[derive(Default)]
pub struct Capturing {
    capture: Option<Capture>,
    /// Frames skipped for being the wrong size, and the size of the last of them.
    wrong_size: u64,
    last_wrong_size: [u32; 2],
    /// Frames skipped because more arrived in a pass than it had room for.
    missed: u64,
}

/// How capture is going, for the node's window.
#[derive(Default)]
pub struct CameraDisplay {
    status: Status,
    wrong_size: u64,
    last_wrong_size: [u32; 2],
    missed: u64,
}

impl Node<Edges> for Camera {
    const INFO: NodeInfo = NodeInfo {
        name: "Camera",
        category: Category::Source,
        description: "Video from a camera: a FLIR camera (via the Spinnaker SDK), the fake camera in tools/, or a \
                      synthetic test scene.",
    };

    type State = Capturing;

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let [width, height] = self.size;
        let stream = Video { width, height, fps: self.settings.frame_rate as f32 };
        vec![Output { name: "video", kind: Kind::Video, stream: Some(DataStream::Video(stream)) }]
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "camera", value: Setting::Camera(self.settings) }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Camera(settings) = value {
            self.settings = settings;
        }
    }

    fn display(&self) -> Option<Box<dyn std::any::Any + Send>> {
        Some(Box::new(CameraDisplay::default()))
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Capturing {
        Capturing { capture: Some(Capture::start(self.source.clone(), self.settings, self.size)), ..Default::default() }
    }

    /// Takes the frames that have arrived since the last pass.
    fn process(&self, pass: &mut Pass<Edges>, capturing: &mut Capturing) {
        let Some(capture) = &capturing.capture else { return };
        capture.apply(&self.settings); // exposure and gain can change while running
        if let Some(Buffer::Video(out)) = pass.outputs.get_mut(0) {
            out.frames = 0;
            while let Some(frame) = capture.next() {
                if (frame.width as usize, frame.height as usize) != (out.width, out.height) {
                    capturing.wrong_size += 1;
                    capturing.last_wrong_size = [frame.width, frame.height];
                } else if out.frames == out.capacity {
                    capturing.missed += 1;
                } else {
                    let index = out.frames;
                    out.frame_mut(index).copy_from_slice(&frame.pixels);
                    (out.ids[index], out.times[index]) = (frame.id, frame.timestamp);
                    out.frames += 1;
                }
            }
        }
        if let Some(display) = pass.display::<CameraDisplay>() {
            display.status = capture.status();
            display.wrong_size = capturing.wrong_size;
            display.last_wrong_size = capturing.last_wrong_size;
            display.missed = capturing.missed;
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Camera", |ui| source_ui(ui, &mut self.source));
            match &mut self.source {
                Source::Synthetic => {}
                Source::Fake { address } => {
                    setting(ui, "Address", |ui| ui.text_edit_singleline(address));
                }
                Source::Spinnaker { serial, library } => {
                    setting(ui, "Serial", |ui| ui.add(egui::TextEdit::singleline(serial).hint_text("first found")));
                    setting(ui, "Library", |ui| {
                        ui.add(egui::TextEdit::singleline(library).hint_text("found automatically"))
                    });
                }
            }
            setting(ui, "Frame size", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.size[0]).range(16..=8192).suffix(" px"));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut self.size[1]).range(16..=8192).suffix(" px"));
                })
                .response
            });
        });
        let display = context.display::<CameraDisplay>();
        camera_settings_ui(ui, &mut self.settings, display.map(|d| &d.status.controls));
        ui.separator();
        let Some(display) = display else {
            ui.weak("Press Play to start capturing.");
            return;
        };
        let theme = context.theme;
        let status = &display.status;
        match &status.state {
            State::Starting => {
                ui.weak("Opening the camera…");
            }
            State::Running => {
                ui.label(&status.description);
            }
            State::Problem(problem) => {
                ui.label(RichText::new(problem).color(theme.warning));
            }
        }
        ui.weak(format!(
            "{:.1} fps · {} frames · {} dropped by the camera · {} not taken in time",
            status.fps,
            status.frames,
            status.camera_dropped,
            status.queue_dropped + display.missed
        ));
        if display.wrong_size > 0 {
            let [w, h] = display.last_wrong_size;
            let message = format!(
                "{} frames skipped: they're {w}×{h}, not the {}×{} set. Change the frame size to match.",
                display.wrong_size, self.size[0], self.size[1]
            );
            ui.label(RichText::new(message).color(theme.warning));
        }
        for warning in &status.warnings {
            ui.label(RichText::new(warning).small().color(theme.warning));
        }
    }
}

fn source_ui(ui: &mut egui::Ui, source: &mut Source) -> egui::Response {
    let name = |source: &Source| match source {
        Source::Synthetic => "Synthetic test scene",
        Source::Fake { .. } => "Fake camera (tools/)",
        Source::Spinnaker { .. } => "FLIR (Spinnaker)",
    };
    let choices = [
        Source::Synthetic,
        Source::Fake { address: "127.0.0.1:27030".into() },
        Source::Spinnaker { serial: String::new(), library: String::new() },
    ];
    egui::ComboBox::from_id_salt("source")
        .selected_text(name(source))
        .show_ui(ui, |ui| {
            for choice in choices {
                let selected = std::mem::discriminant(source) == std::mem::discriminant(&choice);
                if ui.selectable_label(selected, name(&choice)).clicked() && !selected {
                    *source = choice;
                }
            }
        })
        .response
}

/// The frame rate, exposure and gain. `controls`, if the camera's running, says what it supports.
pub(in crate::standard_components::tracking) fn camera_settings_ui(
    ui: &mut egui::Ui,
    settings: &mut CameraSettings,
    controls: Option<&CameraControls>,
) {
    egui::Grid::new("camera settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        setting(ui, "Frame rate", |ui| {
            let range = controls.and_then(|c| c.frame_rate).map_or(1.0..=1000.0, |r| r.min..=r.max);
            ui.add(
                egui::DragValue::new(&mut settings.frame_rate).range(range).speed(0.1).max_decimals(2).suffix(" fps"),
            )
            .on_hover_text("Also the video's rate: changing it restarts the run")
        });
        let unknown = |min, max| Range { min, max, current: f64::NAN };
        let exposure = controls.map_or(Some(unknown(10.0, 1e6)), |c| c.exposure);
        let gain = controls.map_or(Some(unknown(0.0, 48.0)), |c| c.gain);
        control_row(ui, "Exposure", &mut settings.exposure, exposure, " µs", true);
        control_row(ui, "Gain", &mut settings.gain, gain, " dB", false);
    });
}

/// A camera setting that may be left as the camera has it, automatic, or set manually.
fn control_row(ui: &mut egui::Ui, name: &str, control: &mut Control, range: Option<Range>, unit: &str, log: bool) {
    let label = ui.label(name);
    ui.vertical(|ui| {
        let Some(range) = range else {
            ui.weak("not supported by this camera");
            return;
        };
        ui.horizontal(|ui| {
            let start = if range.current.is_finite() { range.current } else { range.min };
            let manual = Control::Manual(start.clamp(range.min, range.max));
            for (option, text) in [(Control::Unchanged, "Camera"), (Control::Auto, "Auto"), (manual, "Manual")] {
                let selected = std::mem::discriminant(control) == std::mem::discriminant(&option);
                if ui.selectable_label(selected, text).clicked() && !selected {
                    *control = option;
                }
            }
        })
        .response
        .on_hover_text("Camera: leave it as the camera is configured (e.g. in SpinView)");
        match control {
            Control::Manual(value) => {
                ui.add(egui::Slider::new(value, range.min..=range.max).logarithmic(log).suffix(unit))
                    .labelled_by(label.id);
            }
            _ if range.current.is_finite() => {
                ui.weak(format!("now {:.1}{unit}", range.current));
            }
            _ => {}
        }
    });
    ui.end_row();
}
