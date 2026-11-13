//! Video from a camera, captured on a thread of its own (see `capture`) into a pool of buffers, which each pass swaps
//! for its own.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::capture::{Capture, State};
use super::{CameraControls, CameraSettings, CameraStats, CaptureStats, Control, Range, Source};
use crate::standard::pool::Pool;
use crate::standard::tracking::Video;
use crate::standard::widgets::{setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};
use ephys_rs::engine::{
    Category, Flow, Node, NodeInfo, NodeThread, Output, Pass, PoolStatus, SettingPort, ThreadContext, UiContext,
};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub source: Source,
    pub settings: CameraSettings,
}

/// Capturing, while running.
#[derive(Default)]
pub struct Capturing {
    capture: Option<Capture>,
    /// The wrong frame size last acted on (triggering an update to the right one, or saying it can't be, while recording).
    triggered_size: Option<[u32; 2]>,
    complained: Option<[u32; 2]>,
    /// The stats as last copied from the capture thread (a pass doesn't wait for it, if it's updating them just then).
    stats: CaptureStats,
}

impl ephys_rs::engine::PassState for Capturing {
    const WHY: &'static str = "The camera's capture thread and its pool of buffers: each pass takes the frames \
                               that have come since the last, in order.";
}

impl Node<Edges> for Camera {
    const INFO: NodeInfo = NodeInfo {
        name: "Camera",
        category: Category::Source,
        description: "Video from a camera: a FLIR camera (via the Spinnaker SDK), or the fake camera in \
                      synthetic_streams/.",
    };

    type State = Capturing;
    /// What it shows, for its window.
    type Shared = Pool<CaptureStats>;
    type UiState = ();
    type GlobalSettings = super::SpinnakerSettings;

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let [width, height] = self.settings.frame_size;
        let stream = Video { width, height, fps: self.settings.frame_rate as f32 };
        vec![
            Output { name: "video", kind: Kind::Video, stream: Some(DataStream::Video(stream)) },
            Output { name: "stats", kind: Kind::CameraStats, stream: Some(DataStream::CameraStats(CameraStats)) },
        ]
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "camera", value: Setting::Camera(self.settings), on_right: true }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Camera(settings) = value {
            self.settings = settings;
        }
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "Camera capture",
            link: "buffer pool",
            flow: Flow::ToPasses,
            why: "On a thread of its own so frames are taken from the camera as they come, whatever the passes \
                  are doing.",
            // The video: the stats are made by the passes.
            swapped_outputs: vec![0],
        }]
    }

    fn thread_ui(&self, _thread: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        let small = |text: String| RichText::new(text).size(11.0);
        let pool = context.shared::<Pool<CaptureStats>>();
        let Some(stats) = pool.and_then(Pool::newest) else {
            ui.label(small("Takes frames as they arrive; starts when running.".into()).weak());
            return;
        };
        let status = &stats.status;
        match &status.state {
            State::Starting => ui.label(small("Opening the camera…".into()).weak()),
            State::Running => ui.label(small(status.description.clone())),
            State::Problem(problem) => ui.label(small(problem.clone()).color(context.theme.warning)),
        };
        ui.label(small(format!("{:.1} frames/s · {} frames", status.fps, status.frames)).weak());
        if status.camera_dropped + status.not_taken > 0 {
            let text =
                format!("{} dropped by the camera, {} not taken in time", status.camera_dropped, status.not_taken);
            ui.label(small(text).color(context.theme.warning));
        }
    }

    fn thread_pool(&self, _thread: usize, context: &ThreadContext) -> Option<PoolStatus> {
        context.shared::<Pool<CaptureStats>>().and_then(|l| l.newest().map(|s| s.pool))
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Capturing {
        let [width, height] = self.settings.frame_size;
        let video = Video { width, height, fps: self.settings.frame_rate as f32 };
        Capturing { capture: Some(Capture::start(self.source.clone(), self.settings, video)), ..Default::default() }
    }

    /// Takes the frames that have arrived since the last pass, swapping the buffer they're in for the pass's own
    /// (which goes back to the capture thread's pool to fill), so nothing's copied.
    fn process(&self, pass: &mut Pass<Edges>, capturing: &mut Capturing) {
        let Some(capture) = &capturing.capture else { return };
        capture.apply(&self.settings); // exposure and gain can change while running
        if let Some(Buffer::Video(out)) = pass.outputs.get_mut(0) {
            match capture.pool().take(|video| video.frames == 0) {
                Some(mut video) => {
                    std::mem::swap(out, &mut video);
                    capture.pool().give_back(video);
                }
                None => out.frames = 0,
            }
        }
        // The stats, on their output and in the node's window: as last copied, if the capture thread's updating them
        // just then.
        if let Some(status) = capture.try_status() {
            capturing.stats.status.copy_from(&status);
        }
        // Of the video, not the stats.
        capturing.stats.pool = PoolStatus { ports: 1, ..capture.pool().status() };
        let stats = &capturing.stats;
        if let Some(Buffer::CameraStats(out)) = pass.outputs.get_mut(1) {
            out.copy_from(stats);
        }
        pass.shared::<Pool<CaptureStats>>().send_newest(pass.sequence, |shown| shown.copy_from(stats));
        // Frames of the wrong size are skipped. An update to the right size is triggered, but while recording settings are locked.
        if let Some(size @ [w, h]) = capturing.stats.status.last_wrong_size {
            if !pass.recording() && capturing.triggered_size != Some(size) {
                log::info!("Camera: setting its Frame size to the {w}×{h} it's sending");
                pass.trigger_settings_update(0, Setting::Camera(CameraSettings { frame_size: size, ..self.settings }));
                capturing.triggered_size = Some(size);
            } else if pass.recording() && capturing.complained != Some(size) {
                let [set_w, set_h] = self.settings.frame_size;
                log::error!(
                    "Camera: frames are {w}×{h}, not the {set_w}×{set_h} set, so this recording has no video. It can't be \
                     changed while recording: stop, and it'll change to match"
                );
                capturing.complained = Some(size);
            }
        }
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            settings_grid(ui, |ui| {
                setting(ui, "Camera", |ui| source_ui(ui, &mut node.source));
                match &mut node.source {
                    Source::Fake { address } => {
                        setting(ui, "Address", |ui| ui.text_edit_singleline(address));
                    }
                    Source::Spinnaker { serial } => {
                        setting(ui, "Serial", |ui| ui.add(egui::TextEdit::singleline(serial).hint_text("first found")));
                    }
                }
                setting(ui, "Frame size", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut node.settings.frame_size[0]).range(16..=8192).suffix(" px"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut node.settings.frame_size[1]).range(16..=8192).suffix(" px"));
                    })
                    .response
                });
            });
            let pool = context.shared::<Pool<CaptureStats>>();
            let display = pool.and_then(Pool::newest);
            camera_settings_ui(ui, &mut node.settings, display.as_ref().map(|d| &d.status.controls));
            ui.separator();
            if !context.running {
                ui.weak("Press Play to start capturing.");
            }
            let Some(display) = display else { return };
            let theme = context.theme;
            let status = &display.status;
            match &status.state {
                State::Starting if !context.running => {}
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
            let rate = if context.running { format!("{:.1} fps · ", status.fps) } else { String::new() };
            ui.weak(format!(
                "{rate}{} frames · {} dropped by the camera · {} not taken in time",
                status.frames, status.camera_dropped, status.not_taken
            ));
            if let Some([w, h]) = status.last_wrong_size {
                let [set_w, set_h] = node.settings.frame_size;
                let message = format!(
                    "{} frames skipped: they're {w}×{h}, not the {set_w}×{set_h} set. It'll change to match, unless \
                     recording.",
                    status.wrong_size
                );
                ui.label(RichText::new(message).color(theme.warning));
            }
            for warning in &status.warnings {
                ui.label(RichText::new(warning).small().color(theme.warning));
            }
        });
    }
}

fn source_ui(ui: &mut egui::Ui, source: &mut Source) -> egui::Response {
    let name = |source: &Source| match source {
        Source::Fake { .. } => "Fake camera (synthetic_streams/)",
        Source::Spinnaker { .. } => "FLIR (Spinnaker)",
    };
    let choices = [Source::default(), Source::Spinnaker { serial: String::new() }];
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
pub(in super::super) fn camera_settings_ui(
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
            .on_hover_text("Also the rate declared for the video, whatever a triggered camera does")
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
