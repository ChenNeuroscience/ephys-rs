//! The tracker's window: the camera's video with the LEDs found, and the crop and calibration rulers over it to
//! drag; beside it, the camera's and the detector's settings. It edits those nodes through its controls.

use eframe::egui::{self, Color32, ColorImage, Rect, RichText, Sense, TextureHandle, TextureOptions};
use serde::{Deserialize, Serialize};

use super::camera::node::camera_settings_ui;
use super::position_detector::detection_ui;
use crate::core::engine::{Category, Context, Control, Input, Node, NodeInfo, Output, Pass};
use crate::core::theme::Colours;
use crate::standard_components::preview;
use crate::standard_components::tracking::VideoBuffer;
use crate::standard_components::tracking::overlay::{self, Drag, Overlay, Overlays, Xform};
use crate::standard_components::tracking::pipeline::{Blob, row_spans};
use crate::standard_components::tracking::{Crop, Detection};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Above-threshold pixels, when detection's shown.
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 70, 70);

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackerView {
    pub overlays: Overlays,
}

/// A video frame, copied for display.
#[derive(Default)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    /// The camera's frame id.
    pub id: u64,
}

impl Frame {
    /// Copies frame `index` of `video`, reusing this one's memory.
    pub fn copy_from(&mut self, video: &VideoBuffer, index: usize) {
        (self.width, self.height, self.id) = (video.width, video.height, video.ids[index]);
        self.pixels.clear();
        self.pixels.extend_from_slice(video.frame(index));
    }
}

/// The latest frame and the LEDs found in it.
#[derive(Default)]
pub struct TrackerDisplay {
    frame: Frame,
    leds: Vec<Blob>,
}

impl Node<Edges> for TrackerView {
    const INFO: NodeInfo = NodeInfo {
        name: "Tracker View",
        category: Category::Visualisation,
        description: "The camera's video with the LEDs found, and the crop and calibration rulers over it to \
                      adjust. Through its controls, adjusts the camera and the position detector too.",
    };
    const HAS_VIEW: bool = true;

    type State = ();

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "video", kind: Kind::Video }, Input { name: "positions", kind: Kind::Positions }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        if inputs[0].is_none() { vec!["Connect video".into()] } else { Vec::new() }
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "camera", kind: SettingKind::Camera },
            Control { name: "detection", kind: SettingKind::Detection },
        ]
    }

    fn display(&self) -> Option<Box<dyn std::any::Any + Send>> {
        Some(Box::new(TrackerDisplay::default()))
    }

    /// Keeps the pass's last frame, and the LEDs found in it.
    fn process(&self, pass: &mut Pass<Edges>, _state: &mut ()) {
        let Some(Buffer::Video(video)) = pass.inputs[0] else { return };
        if video.frames == 0 {
            return;
        }
        let last = video.frames - 1;
        let id = video.ids[last];
        let leds = match pass.inputs[1] {
            Some(Buffer::Positions(positions)) => positions.samples.iter().find(|s| s.frame == id).map(|s| s.leds),
            _ => None,
        };
        let Some(display) = pass.display::<TrackerDisplay>() else { return };
        display.frame.copy_from(video, last);
        display.leds.clear();
        display.leds.extend(leds.into_iter().flatten().flatten());
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        overlays_ui(ui, &mut self.overlays);
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let size = match context.inputs[0] {
            Some(DataStream::Video(video)) => [video.width as f32, video.height as f32],
            _ => [640.0, 480.0],
        };
        egui::Panel::left(ui.scope_id().with("tracker controls")).resizable(true).default_size(280.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls_ui(ui, context, size));
        });

        // Through the context's reference, so the context itself is free to change below.
        let display = context.display.and_then(|d| d.downcast_ref::<TrackerDisplay>());
        let Some(display) = display.filter(|d| d.frame.width > 0) else {
            let message =
                if context.display.is_some() { "Waiting for frames…" } else { "Press Play to see the camera" };
            return preview::unconnected(ui, context.theme, message);
        };
        let detection = match &context.controls[1] {
            Some(Setting::Detection(detection)) => Some(detection.clone()),
            _ => None,
        };
        let (rect, response) = show_frame(ui, &display.frame, detection.as_ref(), &self.overlays, context.theme);
        let xf = Xform { origin: rect.min, scale: rect.width() / display.frame.width as f32 };

        // Dragging the crop and rulers edits the detector's settings, if it's connected.
        if let Some(mut detection) = detection {
            let drag_id = ui.scope_id().with("tracker drag");
            let mut drag = ui.data(|d| d.get_temp::<Option<Drag>>(drag_id)).flatten();
            if overlay::interact(ui, &response, xf, &mut detection, &self.overlays, &mut drag) {
                context.controls[1] = Some(Setting::Detection(detection.clone()));
            }
            ui.data_mut(|d| d.insert_temp(drag_id, drag));
            overlay::draw(&ui.painter_at(rect), xf, &detection, &self.overlays, &display.leds);
        } else {
            overlay::draw(&ui.painter_at(rect), xf, &Detection::default(), &self.overlays, &display.leds);
        }
        ui.ctx().request_repaint();
    }
}

impl TrackerView {
    /// The camera's and the detector's settings (through the controls), and what's shown over the video.
    fn controls_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>, size: [f32; 2]) {
        ui.label(RichText::new("Camera").strong());
        match context.controls[0].clone() {
            Some(Setting::Camera(mut settings)) => {
                camera_settings_ui(ui, &mut settings, None);
                context.controls[0] = Some(Setting::Camera(settings));
            }
            _ => {
                ui.weak("Connect the camera control to a Camera to adjust it here.");
            }
        }
        ui.separator();
        match context.controls[1].clone() {
            Some(Setting::Detection(mut detection)) => {
                detection_ui(ui, &mut detection, size);
                context.controls[1] = Some(Setting::Detection(detection));
            }
            _ => {
                ui.weak("Connect the detection control to a Position Detector to adjust it here.");
            }
        }
        ui.separator();
        overlays_ui(ui, &mut self.overlays);
    }
}

/// Whether the crop, detection and rulers are shown over the video, and whether they can be dragged.
fn overlays_ui(ui: &mut egui::Ui, overlays: &mut Overlays) {
    ui.label(RichText::new("Over the video").strong());
    egui::Grid::new("overlays").num_columns(3).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, name: &str, overlay: &mut Overlay, tip: &str| {
            ui.label(name).on_hover_text(tip);
            ui.toggle_value(&mut overlay.visible, "Show");
            ui.toggle_value(&mut overlay.locked, "Lock");
            ui.end_row();
        };
        row(
            ui,
            "Crop",
            &mut overlays.crop,
            "Shown: the whole frame with the crop's outline. Hidden: outside it masked out.",
        );
        row(ui, "Detection", &mut overlays.detection, "Pixels over the threshold highlighted, and the LEDs circled");
        row(ui, "Rulers", &mut overlays.rulers, "The calibration rulers (which count even when hidden)");
    });
}

/// Shows `frame` fitted to the space left, keeping its shape; with `detection`, above-threshold pixels are
/// highlighted (if the overlays say) and the outside of the crop masked (if it's hidden). Returns where it's
/// shown, and the response there.
pub fn show_frame(
    ui: &mut egui::Ui,
    frame: &Frame,
    detection: Option<&Detection>,
    overlays: &Overlays,
    theme: &Colours,
) -> (Rect, egui::Response) {
    let area = preview::plot_area(ui, theme);
    let image = egui::vec2(frame.width as f32, frame.height as f32);
    let scale = (area.width() / image.x).min(area.height() / image.y);
    let rect = Rect::from_center_size(area.center(), image * scale);
    let response = ui.interact(rect, ui.scope_id().with("frame"), Sense::click_and_drag());

    // Only redrawn when the frame or how it's shown changes.
    let shown = Shown {
        frame: (frame.id, frame.width, frame.height),
        detection: detection.map(|d| (d.threshold, d.edge_threshold(), d.crop, d.edge_width())),
        overlays: (overlays.crop.visible, overlays.detection.visible),
    };
    let texture_id = ui.scope_id().with("frame texture");
    let cached: Option<(TextureHandle, Shown)> = ui.data(|d| d.get_temp(texture_id));
    let texture = match cached {
        Some((texture, previous)) if previous == shown => texture,
        cached => {
            let image = colour_image(frame, detection, overlays);
            let texture = match cached {
                Some((mut texture, _)) => {
                    texture.set(image, TextureOptions::LINEAR);
                    texture
                }
                None => ui.ctx().load_texture("frame", image, TextureOptions::LINEAR),
            };
            ui.data_mut(|d| d.insert_temp(texture_id, (texture.clone(), shown)));
            texture
        }
    };
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
    (rect, response)
}

/// What a frame's texture shows, so it's only redrawn when that changes.
#[derive(Clone, PartialEq)]
struct Shown {
    frame: (u64, usize, usize),
    /// Threshold, edge threshold, crop and edge width.
    detection: Option<(u8, u8, Crop, f32)>,
    /// Whether the crop and detection are shown.
    overlays: (bool, bool),
}

/// The frame in colour: grey, with above-threshold pixels in the crop highlighted (when detection's shown),
/// and the outside of the crop black (when the crop's hidden).
fn colour_image(frame: &Frame, detection: Option<&Detection>, overlays: &Overlays) -> ColorImage {
    let (width, height) = (frame.width, frame.height);
    let mut pixels = Vec::with_capacity(width * height);
    for (y, row) in frame.pixels.chunks_exact(width.max(1)).enumerate() {
        let spans = detection.map(|d| (d, row_spans(&d.crop, d.edge_width(), y as u32, width as u32)));
        for (x, &value) in row.iter().enumerate() {
            let x = x as u32;
            let colour = match spans {
                None => Color32::from_gray(value),
                Some((detection, spans)) => {
                    let inside = x >= spans.outer.0 && x < spans.outer.1;
                    let in_band = !(x >= spans.inner.0 && x < spans.inner.1);
                    let threshold = if in_band { detection.edge_threshold() } else { detection.threshold };
                    if !inside && !overlays.crop.visible && detection.crop != Crop::None {
                        Color32::BLACK
                    } else if inside && overlays.detection.visible && value > threshold {
                        HIGHLIGHT
                    } else {
                        Color32::from_gray(value)
                    }
                }
            };
            pixels.push(colour);
        }
    }
    ColorImage::new([width, height], pixels)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Nodes;
    use crate::core::engine::{Graph, NodeId, NodeSet, Port, Role, Runner, Snapshot};

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
        graph.add(Nodes::create(index), [0.0, 0.0])
    }

    /// The LEDs in the view's latest display, once it has a frame.
    fn leds_shown(runner: &Runner<Nodes>, view: NodeId) -> Option<usize> {
        runner
            .with_display(view, |d| {
                d.downcast_ref::<TrackerDisplay>().filter(|d| d.frame.width > 0).map(|d| d.leds.len())
            })
            .flatten()
    }

    fn wait_for(mut done: impl FnMut() -> bool) -> bool {
        (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(30));
            done()
        })
    }

    #[test]
    fn tracks_the_camera_and_adjusts_while_running() {
        let mut graph = Graph::default();
        let camera = add(&mut graph, "Camera");
        let address = super::super::camera::serve_test_scene(640, 480);
        if let Some(Nodes::Camera(c)) = graph.get_mut(camera).map(|e| &mut e.node) {
            c.source = super::super::camera::Source::Fake { address };
        }
        let detector = add(&mut graph, "Position Detector");
        let view = add(&mut graph, "Tracker View");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(camera, Role::Output, 0), port(detector, Role::Input, 0)).unwrap();
        graph.connect(port(camera, Role::Output, 0), port(view, Role::Input, 0)).unwrap();
        graph.connect(port(detector, Role::Output, 0), port(view, Role::Input, 1)).unwrap();
        let first = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start(&first, 2);

        // The test scene has a big LED at 255 and a small one at 240, both over the default threshold.
        assert!(wait_for(|| leds_shown(&runner, view) == Some(2)), "both LEDs found: {:?}", leds_shown(&runner, view));

        // Raising the threshold between them carries on running (a change of settings), and only one is found.
        if let Some(Nodes::PositionDetector(d)) = graph.get_mut(detector).map(|e| &mut e.node) {
            d.detection.threshold = 245;
        }
        assert!(runner.update(&Snapshot::new(1, graph, Some(&first))));
        assert!(wait_for(|| leds_shown(&runner, view) == Some(1)), "one LED found: {:?}", leds_shown(&runner, view));
    }
}
