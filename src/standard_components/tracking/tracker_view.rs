//! The tracker's window, laid out as in the Open Ephys tracker plugin: the camera's video with the LEDs found,
//! and the crop and calibration rulers over it to drag; beside it, collapsible sections of the camera's and the
//! detector's settings (which it edits through its controls), with eye and padlock toggles in their headers for
//! what's shown over the video and whether it can be changed; below it, a chart of the LEDs' sizes.

use eframe::egui::{self, Color32, ColorImage, Rect, RichText, Sense, TextureHandle, TextureOptions};
use serde::{Deserialize, Serialize};

use super::camera::CaptureStats;
use super::camera::capture::State;
use super::camera::node::camera_settings_ui;
use super::led_chart::{self, History};
use super::position_detector::{calibration_ui, crop_ui, detection_ui};
use crate::core::engine::{Category, Context, Control, CopyTo, Input, Node, NodeInfo, Output, Pass};
use crate::core::exchange::Latest;
use crate::core::theme::Colours;
use crate::standard_components::preview;
use crate::standard_components::tracking::VideoBuffer;
use crate::standard_components::tracking::overlay::{self, Drag, Overlay, Overlays, Xform};
use crate::standard_components::tracking::pipeline::{Blob, row_spans};
use crate::standard_components::tracking::{Crop, Detection};
use crate::standard_components::widgets::{named_settings_grid, section};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Above-threshold pixels, when detection's shown.
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 70, 70);

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackerView {
    pub overlays: Overlays,
    /// Whether the camera's settings are locked against changes here.
    pub camera_locked: bool,
}

/// The tracker's state while running: the LEDs' recent sizes, and what it last copied for its window.
#[derive(Default)]
pub struct Tracking {
    history: History,
    shown: Latest<TrackerDisplay>,
}

impl crate::core::engine::PassState for Tracking {
    const WHY: &'static str = "The LEDs' sizes over the last few seconds, for the chart, which each pass adds \
                               to.";
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

/// The latest frame and the LEDs found in it, the LEDs' recent sizes, and the camera's stats (if they're
/// connected).
#[derive(Default)]
pub struct TrackerDisplay {
    frame: Frame,
    leds: Vec<Blob>,
    history: History,
    stats: Option<CaptureStats>,
}

impl Node<Edges> for TrackerView {
    const INFO: NodeInfo = NodeInfo {
        name: "Tracker View",
        category: Category::Visualisation,
        description: "The camera's video with the LEDs found, and the crop and calibration rulers over it to \
                      adjust. Through its controls, adjusts the camera and the position detector too.",
    };
    const HAS_VIEW: bool = true;

    type State = Tracking;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![
            Input { name: "video", kind: Kind::Video },
            Input { name: "positions", kind: Kind::Positions },
            Input { name: "stats", kind: Kind::CameraStats },
        ]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        if inputs[0].is_none() { vec!["Connect video".into()] } else { Vec::new() }
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "camera", kind: SettingKind::Camera, read_only: false },
            Control { name: "detection", kind: SettingKind::Detection, read_only: false },
        ]
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Window)
    }

    /// Room for the LEDs' sizes over `led_chart::SECONDS`, so it isn't allocated while running.
    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Tracking {
        let fps = match inputs[1] {
            Some(DataStream::Positions(positions)) => positions.fps as f64,
            _ => 0.0,
        };
        let history = History::with_capacity((fps * led_chart::SECONDS) as usize + 16);
        Tracking { history, shown: Latest::default() }
    }

    /// Records the LEDs' sizes, and keeps the pass's last frame, the LEDs found in it and the camera's stats. A
    /// pass without a frame is only shown if the camera isn't running, to say why.
    fn process(&self, pass: &mut Pass<Edges>, tracking: &mut Tracking) {
        let Tracking { history, shown } = tracking;
        if let Some(Buffer::Positions(positions)) = pass.inputs[1] {
            for sample in &positions.samples {
                let area = |led: Option<Blob>| led.map_or(0, |b| b.area);
                led_chart::record(history, sample.time, [area(sample.leds[0]), area(sample.leds[1])]);
            }
        }
        let video = match pass.inputs[0] {
            Some(Buffer::Video(video)) if video.frames > 0 => Some(video),
            _ => None,
        };
        let stats = match pass.inputs[2] {
            Some(Buffer::CameraStats(stats)) => Some(stats),
            _ => None,
        };
        if video.is_none() && !stats.is_some_and(|s| s.status.state != State::Running) {
            return;
        }
        let positions = match pass.inputs[1] {
            Some(Buffer::Positions(positions)) => Some(positions),
            _ => None,
        };
        shown.send(pass, |display| {
            display.leds.clear();
            match video {
                Some(video) => {
                    let last = video.frames - 1;
                    display.frame.copy_from(video, last);
                    let found = positions.and_then(|p| p.samples.iter().find(|s| s.frame == video.ids[last]));
                    display.leds.extend(found.into_iter().flat_map(|s| s.leds).flatten());
                }
                None => display.frame.width = 0,
            }
            match (stats, &mut display.stats) {
                (Some(stats), Some(shown)) => shown.copy_from(stats),
                (Some(stats), shown) => *shown = Some(stats.clone()),
                (None, shown) => *shown = None,
            }
            display.history.clear();
            display.history.extend(history.iter().copied());
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let size = frame_size(context);
        self.controls_ui(ui, context, size);
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let size = frame_size(context);
        let area = ui.available_rect_before_wrap();
        let arrangement = Arrangement::of(area);
        if arrangement == Arrangement::Rows {
            let rest = self.controls_above(ui, context, size, area);
            ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| self.camera_ui(ui, context, arrangement));
        } else {
            // As wide as it's set, however much of it the sections take while some are closed.
            let panel = egui::Panel::left(ui.scope_id().with("tracker controls")).resizable(true).default_size(290.0);
            panel.show(ui, |ui| {
                let scroll = egui::ScrollArea::vertical().auto_shrink([false, true]);
                scroll.show(ui, |ui| self.controls_ui(ui, context, size));
            });
            self.camera_ui(ui, context, arrangement);
        }
    }
}

impl TrackerView {
    /// The settings above the camera, as tall as they are (so closing sections makes room for the camera), up
    /// to a height that dragging their bottom edge sets. Returns the room left below.
    fn controls_above(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>, size: [f32; 2], area: Rect) -> Rect {
        // The most it's to take, and how tall the settings were last time.
        let fit_id = ui.scope_id().with("tracker controls above");
        let (most, tall) = ui.data(|d| d.get_temp::<(f32, f32)>(fit_id)).unwrap_or((area.height() * 0.4, f32::MAX));
        let height = most.min(tall).clamp(40.0, (area.height() - 120.0).max(40.0));
        let top = Rect::from_min_size(area.min, egui::vec2(area.width(), height));
        let mut content = tall;
        ui.scope_builder(egui::UiBuilder::new().max_rect(top.shrink2(egui::vec2(8.0, 0.0))), |ui| {
            let scroll = egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                self.controls_ui(ui, context, size);
            });
            content = scroll.content_size.y + 6.0;
        });

        let edge = Rect::from_x_y_ranges(area.x_range(), top.bottom() - 3.0..=top.bottom() + 3.0);
        let drag = ui.interact(edge, fit_id.with("edge"), Sense::drag());
        let mut most = most;
        if let Some(pointer) = drag.interact_pointer_pos().filter(|_| drag.dragged()) {
            most = (pointer.y - area.top()).max(40.0);
        }
        let colour = if drag.hovered() || drag.dragged() { context.theme.accent } else { context.theme.divider };
        ui.painter().hline(area.x_range(), top.bottom(), egui::Stroke::new(1.0, colour));
        drag.on_hover_cursor(egui::CursorIcon::ResizeVertical);
        ui.data_mut(|d| d.insert_temp(fit_id, (most, content)));
        if (content - height).abs() > 0.5 {
            ui.ctx().request_repaint(); // to fit the settings as they now are
        }
        Rect::from_min_max(egui::pos2(area.left(), top.bottom() + 4.0), area.max)
    }

    /// The camera, with what's detected drawn over it, and the LED chart beside or below it.
    fn camera_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>, arrangement: Arrangement) {
        if self.overlays.detection.visible
            && let Some(display) = context.display.and_then(|d| d.downcast_ref::<TrackerDisplay>())
        {
            led_panel(ui, &display.history, arrangement);
        }

        // Through the context's reference, so the context itself is free to change below.
        let display = context.display.and_then(|d| d.downcast_ref::<TrackerDisplay>());
        let Some(display) = display.filter(|d| d.frame.width > 0) else {
            let state = display.and_then(|d| d.stats.as_ref()).map(|s| &s.status.state);
            let message = match (context.display, state) {
                (None, _) => "Press Play to see the camera".to_string(),
                (_, Some(State::Starting)) => "Opening the camera…".to_string(),
                (_, Some(State::Problem(problem))) => format!("No video: {problem}"),
                _ => "Waiting for frames…".to_string(),
            };
            return preview::unconnected(ui, context.theme, &message);
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

    /// The camera's and the detector's settings (through the controls), in sections whose headers say what's
    /// shown over the video and lock what can be changed.
    fn controls_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>, size: [f32; 2]) {
        let stats = context.display.and_then(|d| d.downcast_ref::<TrackerDisplay>()).and_then(|d| d.stats.as_ref());
        let declared = match context.inputs[0] {
            Some(DataStream::Video(video)) => Some(video.fps as f64),
            _ => None,
        };
        let connected = matches!(context.inputs[2], Some(DataStream::CameraStats(_)));
        egui::CollapsingHeader::new(RichText::new("Stats").strong())
            .default_open(true)
            .show(ui, |ui| stats_ui(ui, stats, connected, declared, context.theme));

        // What the camera supports, once it's running.
        let supports = stats.filter(|s| s.status.state == State::Running).map(|s| &s.status.controls);
        let mut camera_locked = self.camera_locked;
        section(ui, "Camera", None, (&mut camera_locked, "Lock the camera settings"), |ui, locked| match &mut context
            .controls[0]
        {
            Some(Setting::Camera(settings)) => {
                if locked {
                    ui.disable();
                }
                camera_settings_ui(ui, settings, supports);
            }
            _ => {
                ui.weak("Connect the camera control to a Camera to adjust it here.");
            }
        });
        self.camera_locked = camera_locked;

        let mut detection = match &context.controls[1] {
            Some(Setting::Detection(detection)) => Some(detection.clone()),
            _ => None,
        };
        let unconnected = |ui: &mut egui::Ui| {
            ui.weak("Connect the detection control to a Position Detector to adjust it here.");
        };
        let Overlays { crop, detection: found, rulers } = &mut self.overlays;

        let mut reshaped = false;
        section(
            ui,
            "Crop",
            Some((
                &mut crop.visible,
                "Show the whole frame with the crop outline, or hide it and mask out everything outside the crop",
            )),
            (
                &mut crop.locked,
                "Lock the crop. Unlocked: drag inside to move, drag the square handles to resize, and the round \
                 handle on the stalk to rotate (hold shift to snap to 15°)",
            ),
            |ui, locked| match &mut detection {
                Some(detection) => {
                    if locked {
                        ui.disable();
                    }
                    reshaped = crop_ui(ui, &mut detection.crop, size);
                }
                None => unconnected(ui),
            },
        );
        // A new shape is for adjusting.
        if reshaped {
            *crop = Overlay { visible: true, locked: false };
        }

        section(
            ui,
            "Detection",
            Some((&mut found.visible, "Highlight pixels above the threshold and circle the LEDs found")),
            (&mut found.locked, "Lock the detection settings"),
            |ui, locked| match &mut detection {
                Some(detection) => {
                    if locked {
                        ui.disable();
                    }
                    detection_ui(ui, detection);
                }
                None => unconnected(ui),
            },
        );

        let mut added = false;
        section(
            ui,
            "Calibration",
            Some((&mut rulers.visible, "Show or hide the rulers (they still count towards px/m)")),
            (&mut rulers.locked, "Lock the rulers so they can't be dragged or edited"),
            |ui, locked| match &mut detection {
                Some(detection) => added = calibration_ui(ui, detection, size, locked),
                None => unconnected(ui),
            },
        );
        if added {
            rulers.visible = true;
        }

        if let Some(detection) = detection {
            context.controls[1] = Some(Setting::Detection(detection));
        }
    }
}

/// How the camera's doing, from its stats: what it is, frames, rate and drops, and problems.
fn stats_ui(ui: &mut egui::Ui, stats: Option<&CaptureStats>, connected: bool, declared: Option<f64>, theme: &Colours) {
    let Some(stats) = stats else {
        ui.weak(if connected { "Press Play to see how the camera's doing." } else { "Connect the camera's stats." });
        return;
    };
    let status = &stats.status;
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
    named_settings_grid(ui, "stats", |ui| {
        let row = |ui: &mut egui::Ui, name: &str, value: String| {
            ui.label(name);
            ui.label(value);
            ui.end_row();
        };
        row(ui, "Frames", status.frames.to_string());
        row(ui, "Rate", format!("{:.1} fps", status.fps));
        row(ui, "Camera dropped", status.camera_dropped.to_string());
        row(ui, "Not taken in time", (status.queue_dropped + stats.missed).to_string());
    });
    // Once a couple of seconds have run, so the rate has settled.
    if let Some(declared) = declared
        && status.state == State::Running
        && status.frames as f64 > 2.0 * declared
        && (status.fps - declared).abs() > 0.05 * declared
    {
        let advice = if status.controls.frame_rate_set {
            "Frames will be dropped or missing. The camera may be triggered externally, or not keeping up \
             (e.g. exposure too long for this rate)."
        } else {
            "Frames will be dropped or missing. This camera won't take a frame rate: set the Camera section's to \
             match it."
        };
        let warning = format!("Camera running at {:.1} fps, video declared at {declared} fps", status.fps);
        ui.label(RichText::new(warning).color(theme.warning).strong()).on_hover_text(advice);
    }
    if stats.wrong_size > 0 {
        let [w, h] = stats.last_wrong_size;
        let warning =
            format!("{} frames skipped for their size ({w}×{h}): set the camera's to match", stats.wrong_size);
        ui.label(RichText::new(warning).color(theme.warning));
    }
    for warning in &status.warnings {
        ui.label(RichText::new(warning).small().color(theme.warning));
    }
}

/// The size of the frames coming in, or a likely one if none are.
fn frame_size(context: &Context<Edges>) -> [f32; 2] {
    match context.inputs[0] {
        Some(DataStream::Video(video)) => [video.width as f32, video.height as f32],
        _ => [640.0, 480.0],
    }
}

/// The strip along the bottom, with the chart of the LEDs' sizes at its right (which can be closed).
/// How the view's three parts (settings, camera, LED chart) are laid out, from the shape of its window.
#[derive(Clone, Copy, PartialEq)]
enum Arrangement {
    /// Much wider than tall: settings, camera and chart side by side.
    Columns,
    /// Taller than wide: one above another.
    Rows,
    /// In between: the settings beside the camera, with the chart below it.
    Square,
}

impl Arrangement {
    fn of(rect: Rect) -> Self {
        let aspect = rect.width() / rect.height().max(1.0);
        if aspect > 2.2 {
            Arrangement::Columns
        } else if aspect < 0.9 {
            Arrangement::Rows
        } else {
            Arrangement::Square
        }
    }
}

/// The chart of LED sizes, which can be closed down to its toggle: a column at the right, or a strip along the
/// bottom (in the bottom right corner, if the settings are beside the camera).
fn led_panel(ui: &mut egui::Ui, history: &History, arrangement: Arrangement) {
    let open_id = ui.scope_id().with("led chart open");
    let open = ui.data_mut(|d| *d.get_persisted_mut_or(open_id, true));
    if arrangement == Arrangement::Columns {
        // Open, a third of the width to start with; closed, a strip just wide enough for the toggle. Each is a
        // panel of its own, so closing doesn't forget how wide it was open.
        let panel = if open {
            let third = ui.available_width() / 3.0;
            egui::Panel::right(ui.scope_id().with("led column")).resizable(true).default_size(third)
        } else {
            egui::Panel::right(ui.scope_id().with("led column closed")).exact_size(30.0)
        };
        panel.show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                if led_chart::side_toggle(ui, "LED sizes", open).clicked() {
                    ui.data_mut(|d| d.insert_persisted(open_id, !open));
                }
                if open {
                    led_chart::chart(ui, history);
                }
            });
        });
        return;
    }
    let height = if open { 170.0 } else { 26.0 };
    egui::Panel::bottom(ui.scope_id().with("led strip")).exact_size(height).resizable(false).show(ui, |ui| {
        let rect = ui.available_rect_before_wrap();
        let width = if arrangement == Arrangement::Rows { rect.width() } else { 440.0 };
        let chart = Rect::from_min_max(egui::pos2((rect.right() - width).max(rect.left()), rect.top()), rect.max);
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(chart).layout(egui::Layout::top_down(egui::Align::Max)),
            |ui| {
                if led_chart::toggle(ui, "LED sizes", open).clicked() {
                    ui.data_mut(|d| d.insert_persisted(open_id, !open));
                }
                if open {
                    led_chart::chart(ui, history);
                }
            },
        );
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

    #[test]
    fn lays_out_by_the_window_shape() {
        let shape = |w: f32, h: f32| Arrangement::of(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h)));
        assert!(shape(1100.0, 260.0) == Arrangement::Columns);
        assert!(shape(800.0, 520.0) == Arrangement::Square);
        assert!(shape(400.0, 520.0) == Arrangement::Rows);
    }

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
        graph.add(Nodes::create(index), [0.0, 0.0])
    }

    /// The LEDs in the view's latest display, once it has a frame.
    fn leds_shown(runner: &Runner<Nodes>, view: NodeId) -> Option<usize> {
        runner
            .exchange()
            .with_latest(view, |d| {
                d.downcast_ref::<TrackerDisplay>().filter(|d| d.frame.width > 0).map(|d| d.leds.len())
            })
            .flatten()
    }

    /// Waits (ten seconds at most, on a busy machine) for `done`.
    fn wait_for(mut done: impl FnMut() -> bool) -> bool {
        (0..330).any(|_| {
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

    /// The window, laid out as in the tracker plugin, on a made-up frame. See tests/snapshots/.
    #[test]
    fn window() {
        use crate::standard_components::themes::THEMES;
        use crate::standard_components::tracking::camera::capture::Status;
        use crate::standard_components::tracking::camera::{CameraControls, Range};
        use crate::standard_components::tracking::{Crop, Ruler};

        let mut graph = Graph::default();
        let camera = add(&mut graph, "Camera");
        let detector = add(&mut graph, "Position Detector");
        let view = add(&mut graph, "Tracker View");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(camera, Role::Output, 0), port(detector, Role::Input, 0)).unwrap();
        graph.connect(port(camera, Role::Output, 0), port(view, Role::Input, 0)).unwrap();
        graph.connect(port(detector, Role::Output, 0), port(view, Role::Input, 1)).unwrap();
        graph.connect(port(camera, Role::Output, 1), port(view, Role::Input, 2)).unwrap();
        graph.connect(port(view, Role::Control, 0), port(camera, Role::Setting, 0)).unwrap();
        graph.connect(port(view, Role::Control, 1), port(detector, Role::Setting, 0)).unwrap();
        if let Some(Nodes::PositionDetector(d)) = graph.get_mut(detector).map(|e| &mut e.node) {
            d.detection.crop = Crop::Rectangle { center: [320.0, 240.0], width: 420.0, height: 300.0, angle: 0.3 };
            d.detection.rulers.push(Ruler { a: [110.0, 420.0], b: [470.0, 420.0], length_cm: 100.0 });
        }

        // A lit arena on a dark floor, and the two LEDs.
        let (width, height) = (640, 480);
        let mut pixels = vec![25u8; width * height];
        for y in 90..390 {
            for x in 110..530 {
                pixels[y * width + x] = 70 + ((x * 7 + y * 13) % 11) as u8;
            }
        }
        let leds = [Blob { x: 400.0, y: 200.0, area: 30 }, Blob { x: 380.0, y: 215.0, area: 12 }];
        for (led, value) in leds.iter().zip([255, 240]) {
            let r = (led.area as f32 / std::f32::consts::PI).sqrt();
            for y in (led.y - r) as usize..=(led.y + r) as usize {
                for x in (led.x - r) as usize..=(led.x + r) as usize {
                    if (x as f32 - led.x).powi(2) + (y as f32 - led.y).powi(2) <= r * r {
                        pixels[y * width + x] = value;
                    }
                }
            }
        }
        let range = |min, max, current| Some(Range { min, max, current });
        let status = Status {
            state: State::Running,
            description: "Fake camera (synthetic_streams/fake_camera.py)".into(),
            controls: CameraControls {
                frame_rate_set: true,
                frame_rate: range(1.0, 200.0, 50.0),
                exposure: range(100.0, 50_000.0, 10_000.0),
                gain: range(0.0, 24.0, 0.0),
            },
            frames: 2961,
            camera_dropped: 2,
            fps: 50.0,
            ..Default::default()
        };
        let mut display = TrackerDisplay {
            frame: Frame { width, height, pixels, id: 1 },
            leds: leds.to_vec(),
            stats: Some(CaptureStats { status, ..Default::default() }),
            ..Default::default()
        };
        for frame in 0..500 {
            let t = frame as f64 / 50.0;
            let wobble = (t * 2.0).sin();
            let small = if (300..312).contains(&frame) { 0 } else { (12.0 + wobble) as u32 };
            led_chart::record(&mut display.history, t, [(30.0 + 2.0 * wobble) as u32, small]);
        }

        let resolved = graph.resolve();
        let inputs = resolved[&view].input_streams.clone();
        let mut harness = egui_kittest::Harness::builder().with_size([1150.0, 650.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                let display: &(dyn std::any::Any + Send) = &display;
                graph.edit(view, &inputs, &THEMES[0].colours, Some(display), true, |node, context| {
                    node.view_ui(ui, context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("tracker_view");
    }
}
