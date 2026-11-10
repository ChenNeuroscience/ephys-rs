//! The tracker's window, laid out as in the Open Ephys tracker plugin: the camera's video with the LEDs found,
//! and the crop and calibration rulers over it to drag; beside it, collapsible sections of the camera's and the
//! detector's settings (which it edits through its controls), with eye and padlock toggles in their headers for
//! what's shown over the video and whether it can be changed; below it, a chart of the LEDs' sizes.
//!
//! Without video, given positions from elsewhere, it shows where they are in the window they range over (the detection's
//! crop). There's nothing there to adjust, so its settings are only shown, and nothing's edited.

use eframe::egui::{self, Color32, ColorImage, Rect, RichText, Sense, TextureHandle, TextureOptions};
use serde::{Deserialize, Serialize};

use super::camera::CaptureStats;
use super::camera::capture::State;
use super::camera::node::camera_settings_ui;
use super::led_chart::{self, History};
use super::position_detector::{calibration_ui, crop_ui, detection_ui, timing_led_ui};
use crate::standard::plots;
use crate::standard::pool::Pool;
use crate::standard::tracking::VideoBuffer;
use crate::standard::tracking::overlay::{self, Drag, Overlay, Overlays, Xform};
use crate::standard::tracking::pipeline::{Blob, row_spans};
use crate::standard::tracking::{Crop, Detection, Positions};
use crate::standard::widgets::{named_settings_grid, section};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};
use ephys_rs::engine::{
    Category, Control, EdgeType, Input, Node, NodeInfo, Output, Pass, PoolStatus, Shared, UiContext,
};
use ephys_rs::theme::Colours;

/// Above-threshold pixels, when detection's shown.
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 70, 70);

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackerView {
    pub overlays: Overlays,
    /// Whether the camera's settings are locked against changes here.
    pub camera_locked: bool,
}

/// The tracker's state while running: the LEDs' recent sizes, the path while recording (what's not been sent to the
/// window yet), and what it last copied for its window.
#[derive(Default)]
pub struct Tracking {
    history: History,
    pending: Vec<PathUpdate>,
    recording: bool,
    /// The LEDs in the latest position, for showing without video, when a pass might have none.
    latest: [Option<Blob>; 2],
}

/// Where the animal's been while recording: the middle of the LEDs found in each frame (in pixels), NaN where
/// none were. The window keeps it, and can clear it; recording starting clears it too.
pub type Path = Vec<[f32; 2]>;

/// What a pass sends the window of the path, in order (see `Pool::send_pending`).
pub enum PathUpdate {
    /// Recording's started: the path so far goes.
    Restart,
    Point([f32; 2]),
}

/// What the tracker's passes share with the GUI: what to show, and the path, every point of it, in order.
#[derive(Default)]
pub struct TrackerShared {
    shown: Pool<TrackerDisplay>,
    path: Pool<Vec<PathUpdate>>,
}

impl Shared for TrackerShared {
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        vec![("display", self.shown.status()), ("path", self.path.status())]
    }
}

/// The tracker on the GUI's side: the path so far, which it keeps (window open or not) until recording starts again,
/// or it's cleared.
#[derive(Default)]
pub struct TrackerUi {
    path: Path,
}

impl ephys_rs::engine::PassState for Tracking {
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

/// A frame to show, wherever it is: a `Frame`, or one of a `VideoBuffer`'s.
#[derive(Clone, Copy)]
pub struct FrameView<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a [u8],
    pub id: u64,
}

impl Frame {
    pub fn view(&self) -> FrameView<'_> {
        FrameView { width: self.width, height: self.height, pixels: &self.pixels, id: self.id }
    }

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
    /// Whether the timing LED's on in the frame, if there is one.
    timing_led: Option<bool>,
    history: History,
    stats: Option<CaptureStats>,
}

impl Node<Edges> for TrackerView {
    const INFO: NodeInfo = NodeInfo {
        name: "Tracker View",
        category: Category::Visualisation,
        description: "The camera's video with the LEDs found, and the crop and calibration rulers over it to \
                      adjust. Through its controls, adjusts the camera and the position detector too. Without video, \
                      shows where the positions are.",
    };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    type State = Tracking;
    type Shared = TrackerShared;
    type UiState = TrackerUi;
    type GlobalSettings = ();

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
        if inputs[0].is_none() && inputs[1].is_none() { vec!["Connect video or positions".into()] } else { Vec::new() }
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "camera", kind: SettingKind::Camera, read_only: false, on_right: false },
            Control { name: "detection", kind: SettingKind::Detection, read_only: false, on_right: false },
        ]
    }

    /// Room for the LEDs' sizes over `led_chart::SECONDS`, so it isn't allocated while running.
    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Tracking {
        let fps = match inputs[1] {
            Some(DataStream::Positions(positions)) => positions.fps as f64,
            _ => 0.0,
        };
        let history = History::with_capacity((fps * led_chart::SECONDS) as usize + 16);
        Tracking { history, ..Default::default() }
    }

    /// Records the LEDs' sizes, and keeps the pass's last frame, the LEDs found in it and the camera's stats. A
    /// pass without a frame is only shown if the camera isn't running, to say why.
    fn process(&self, pass: &mut Pass<Edges>, tracking: &mut Tracking) {
        let Tracking { history, pending, recording, latest } = tracking;
        let TrackerShared { shown, path } = pass.shared::<TrackerShared>();
        if pass.recording() && !*recording {
            pending.clear();
            pending.push(PathUpdate::Restart);
        }
        *recording = pass.recording();
        if let Some(Buffer::Positions(positions)) = pass.inputs[1] {
            for sample in &positions.samples {
                let area = |led: Option<Blob>| led.map_or(0, |b| b.area);
                let timing_on = sample.timing_led == Some(true);
                led_chart::record(history, sample.time, [area(sample.leds[0]), area(sample.leds[1])], timing_on);
                *latest = sample.leds;
            }
            if pass.recording() {
                pending.extend(positions.samples.iter().map(|sample| PathUpdate::Point(middle(&sample.leds))));
            }
        }
        path.send_pending(pending);
        let video = match pass.inputs[0] {
            Some(Buffer::Video(video)) if video.frames > 0 => Some(video),
            _ => None,
        };
        let stats = match pass.inputs[2] {
            Some(Buffer::CameraStats(stats)) => Some(stats),
            _ => None,
        };
        let positions_only = pass.inputs[0].is_none() && pass.inputs[1].is_some();
        if video.is_none() && !positions_only && !stats.is_some_and(|s| s.status.state != State::Running) {
            return;
        }
        let positions = match pass.inputs[1] {
            Some(Buffer::Positions(positions)) => Some(positions),
            _ => None,
        };
        shown.send_newest(pass.sequence, |display| {
            display.leds.clear();
            display.timing_led = None;
            match video {
                Some(video) => {
                    let last = video.frames - 1;
                    display.frame.copy_from(video, last);
                    let found = positions.and_then(|p| p.samples.iter().find(|s| s.frame == video.ids[last]));
                    display.leds.extend(found.into_iter().flat_map(|s| s.leds).flatten());
                    display.timing_led = found.and_then(|s| s.timing_led);
                }
                None => {
                    display.frame.width = 0;
                    display.leds.extend(latest.iter().flatten());
                }
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

    /// Takes the path sent.
    fn gui_background(&mut self, context: &mut UiContext<Edges>) -> Option<std::time::Instant> {
        let shared = context.shared::<TrackerShared>()?;
        let path = &mut context.ui_state::<TrackerUi>().path;
        shared.path.receive_all(|updates| {
            for update in updates {
                match update {
                    PathUpdate::Restart => path.clear(),
                    PathUpdate::Point(point) => path.push(point),
                }
            }
        });
        None
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        // The node's toolbar, along the top.
        if context.tools.is_some() {
            ui.horizontal(|ui| context.node_toolbar(ui));
        }
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
    fn controls_above(
        &mut self,
        ui: &mut egui::Ui,
        context: &mut UiContext<Edges>,
        size: [f32; 2],
        area: Rect,
    ) -> Rect {
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
    fn camera_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>, arrangement: Arrangement) {
        let pool = context.shared::<TrackerShared>().map(|shared| &shared.shown);
        let reading = pool.and_then(Pool::newest);
        let display = reading.as_deref();
        if self.overlays.detection.visible
            && let Some(display) = display
        {
            led_panel(ui, &display.history, arrangement);
        }
        if positions_only(context) {
            return positions_ui(ui, context, display);
        }

        let Some(display) = display.filter(|d| d.frame.width > 0) else {
            let state = display.and_then(|d| d.stats.as_ref()).map(|s| &s.status.state);
            let message = match (display, state) {
                _ if display.is_none() || !context.running => "Press Play to see the camera".to_string(),
                (_, Some(State::Starting)) => "Opening the camera…".to_string(),
                (_, Some(State::Problem(problem))) => format!("No video: {problem}"),
                _ => "Waiting for frames…".to_string(),
            };
            return plots::unconnected(ui, context.theme, &message);
        };
        let detection = match &context.controls[1] {
            Some(Setting::Detection(detection)) => Some(detection.clone()),
            _ => None,
        };
        let (rect, response) = show_frame(ui, display.frame.view(), detection.as_ref(), &self.overlays, context.theme);
        let xf = Xform { origin: rect.min, scale: rect.width() / display.frame.width as f32 };

        // Dragging the crop and rulers edits the detector's settings, if it's connected.
        if let Some(mut detection) = detection {
            let drag_id = ui.scope_id().with("tracker drag");
            let mut drag = ui.data(|d| d.get_temp::<Option<Drag>>(drag_id)).flatten();
            if overlay::interact(ui, &response, xf, &mut detection, &self.overlays, &mut drag) {
                context.controls[1] = Some(Setting::Detection(detection.clone()));
            }
            ui.data_mut(|d| d.insert_temp(drag_id, drag));
            let timing_on = display.timing_led == Some(true);
            overlay::draw(&ui.painter_at(rect), xf, &detection, &self.overlays, &display.leds, timing_on);
        } else {
            overlay::draw(&ui.painter_at(rect), xf, &Detection::default(), &self.overlays, &display.leds, false);
        }
        let calibration = match &context.controls[1] {
            Some(Setting::Detection(detection)) => detection.pixels_per_metre(),
            _ => None,
        };
        let length = |length: f32| match calibration {
            Some(ppm) => format!("{:.2} m", length / ppm),
            None => format!("{length:.0} px"),
        };
        path_ui(ui, rect, xf, &mut context.ui_state::<TrackerUi>().path, length, context.theme);
        plots::repaint_with_data(ui, context.running);
    }

    /// The camera's and the detector's settings (through the controls), in sections whose headers say what's
    /// shown over the video and lock what can be changed.
    fn controls_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>, size: [f32; 2]) {
        if positions_only(context) {
            return positions_settings_ui(ui, context);
        }
        let pool = context.shared::<TrackerShared>().map(|shared| &shared.shown);
        let reading = pool.and_then(Pool::newest);
        let stats = reading.as_deref().and_then(|d| d.stats.as_ref());
        let declared = match context.inputs[0] {
            Some(DataStream::Video(video)) => Some(video.fps as f64),
            _ => None,
        };
        let connected = matches!(context.inputs[2], Some(DataStream::CameraStats(_)));
        egui::CollapsingHeader::new(RichText::new("Stats").strong())
            .default_open(true)
            .show(ui, |ui| stats_ui(ui, stats, connected, declared, context.theme))
            .header_response
            .on_hover_cursor(egui::CursorIcon::PointingHand);

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
        let Overlays { crop, detection: found, rulers, timing } = &mut self.overlays;

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

        let timing_on = reading.as_deref().and_then(|d| d.timing_led);
        let mut switched_on = false;
        section(
            ui,
            "Timing LED",
            Some((
                &mut timing.visible,
                "Show the spot and its pixels above the threshold, or nothing on the video (the LED's times stay on the \
                 LED sizes chart, and it's still found)",
            )),
            (&mut timing.locked, "Lock the timing LED, so it can't be moved or changed"),
            |ui, locked| match &mut detection {
                Some(detection) => {
                    if locked {
                        ui.disable();
                    }
                    switched_on = timing_led_ui(ui, detection, size, timing_on);
                }
                None => unconnected(ui),
            },
        );
        // A new spot is for placing.
        if switched_on {
            *timing = Overlay { visible: true, locked: false };
        }

        if let Some(detection) = detection {
            context.controls[1] = Some(Setting::Detection(detection));
        }
    }
}

/// Whether it's only got positions to show, with no video.
fn positions_only(context: &UiContext<Edges>) -> bool {
    context.inputs[0].is_none() && context.inputs[1].is_some()
}

/// The window the positions range over: the crop's, if there's one, otherwise a frame's. Sized so it's never empty.
fn positions_window(context: &UiContext<Edges>) -> [f32; 4] {
    let crop = match &context.controls[1] {
        Some(Setting::Detection(detection)) => detection.crop.bounds(),
        _ => None,
    };
    let size = match context.inputs[1] {
        Some(DataStream::Positions(positions)) => [positions.width as f32, positions.height as f32],
        _ => [1.0, 1.0],
    };
    crop.unwrap_or([0.0, 0.0, size[0], size[1]])
}

/// Where positions are in their window, with the path recorded, looking as they do over video.
fn positions_ui(ui: &mut egui::Ui, context: &UiContext<Edges>, display: Option<&TrackerDisplay>) {
    let Some(display) = display else {
        return plots::unconnected(ui, context.theme, "Press Play to see the positions");
    };
    let [min_x, min_y, max_x, max_y] = positions_window(context);
    let (width, height) = (max_x - min_x, max_y - min_y);
    let room = ui.available_size() - egui::vec2(16.0, 16.0);
    let scale = (room.x / width).min(room.y / height).max(f32::MIN_POSITIVE);
    let rect = Rect::from_center_size(ui.available_rect_before_wrap().center(), egui::vec2(width, height) * scale);
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    let xf = Xform { origin: rect.min - egui::vec2(min_x, min_y) * scale, scale };
    let detection = match &context.controls[1] {
        Some(Setting::Detection(detection)) => detection.clone(),
        _ => Detection::default(),
    };
    // The calibration has rulers, but nobody laid them over anything.
    let shown = Overlay { visible: true, locked: true };
    let hidden = Overlay { visible: false, locked: true };
    let overlays = Overlays { crop: shown, detection: shown, rulers: hidden, timing: hidden };
    overlay::draw(&painter, xf, &detection, &overlays, &display.leds, false);
    let range = format!("x {min_x} to {max_x}, y {min_y} to {max_y}");
    let corner = rect.left_bottom() + egui::vec2(8.0, -8.0);
    painter.text(
        corner,
        egui::Align2::LEFT_BOTTOM,
        range,
        egui::FontId::proportional(11.0),
        context.theme.text_secondary,
    );
    let length = |length: f32| match detection.pixels_per_metre() {
        Some(ppm) => format!("{:.2} m", length / ppm),
        None => format!("{length:.0} px"),
    };
    path_ui(ui, rect, xf, &mut context.ui_state::<TrackerUi>().path, length, context.theme);
    plots::repaint_with_data(ui, context.running);
}

/// What's known of positions without video, which there's nothing to adjust of.
fn positions_settings_ui(ui: &mut egui::Ui, context: &UiContext<Edges>) {
    let source = match context.inputs[1] {
        Some(DataStream::Positions(positions)) => positions.source,
        _ => "",
    };
    ui.label(RichText::new("Positions").strong());
    ui.weak(format!("From {}, without video: there's nothing to adjust here.", source.replace('_', " ")));
    let Some(Setting::Detection(detection)) = &context.controls[1] else { return };
    if detection.custom.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Settings").strong());
    egui::Grid::new("custom settings").num_columns(2).striped(true).show(ui, |ui| {
        for (key, value) in &detection.custom {
            ui.label(key);
            ui.label(value);
            ui.end_row();
        }
    });
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
        row(ui, "Not taken in time", status.not_taken.to_string());
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
    if let Some([w, h]) = status.last_wrong_size {
        let warning = format!(
            "{} frames skipped for their size ({w}×{h}): it'll change to match, unless recording",
            status.wrong_size
        );
        ui.label(RichText::new(warning).color(theme.warning));
    }
    for warning in &status.warnings {
        ui.label(RichText::new(warning).small().color(theme.warning));
    }
}

/// The size of the frames coming in, or a likely one if none are.
fn frame_size(context: &UiContext<Edges>) -> [f32; 2] {
    match context.inputs[0] {
        Some(DataStream::Video(video)) => [video.width as f32, video.height as f32],
        _ => [640.0, 480.0],
    }
}

/// The strip along the bottom, with the chart of the LEDs' sizes at its right (which can be closed).
/// The middle of the LEDs found, or NaN if none were.
fn middle(leds: &[Option<Blob>; 2]) -> [f32; 2] {
    let found: Vec<&Blob> = leds.iter().flatten().collect();
    if found.is_empty() {
        return [f32::NAN; 2];
    }
    let n = found.len() as f32;
    [found.iter().map(|b| b.x).sum::<f32>() / n, found.iter().map(|b| b.y).sum::<f32>() / n]
}

/// The path recorded, over the frame, with a button to clear it and how long it is.
fn path_ui(
    ui: &mut egui::Ui,
    frame: Rect,
    xf: Xform,
    path: &mut Path,
    length_text: impl Fn(f32) -> String,
    theme: &Colours,
) {
    if path.is_empty() {
        return;
    }
    let painter = ui.painter_at(frame);
    let stroke = egui::Stroke::new(1.5, Positions::INFO.colour.gamma_multiply(0.85));
    let mut length = 0.0;
    // Broken where no LED was found.
    for run in path.split(|p| !p[0].is_finite()) {
        length +=
            run.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum::<f32>();
        if run.len() > 1 {
            painter.add(egui::Shape::line(run.iter().map(|p| xf.to_screen(*p)).collect(), stroke));
        }
    }
    let corner = Rect::from_min_size(frame.min + egui::vec2(8.0, 8.0), egui::vec2(220.0, 22.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(corner), |ui| {
        ui.horizontal(|ui| {
            if ui.small_button("Clear path").clicked() {
                path.clear();
            }
            let text = RichText::new(format!("path {}", length_text(length))).small().color(theme.text_secondary);
            ui.label(text).on_hover_text("Where the LEDs have been while recording (cleared when recording starts)");
        });
    });
}

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
                if crate::standard::widgets::side_toggle(ui, "LED sizes", open).clicked() {
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
                if crate::standard::widgets::toggle(ui, "LED sizes", open).clicked() {
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
    frame: FrameView,
    detection: Option<&Detection>,
    overlays: &Overlays,
    theme: &Colours,
) -> (Rect, egui::Response) {
    let area = plots::plot_area(ui, theme);
    let image = egui::vec2(frame.width as f32, frame.height as f32);
    let scale = (area.width() / image.x).min(area.height() / image.y);
    let rect = Rect::from_center_size(area.center(), image * scale);
    let response = ui.interact(rect, ui.scope_id().with("frame"), Sense::click_and_drag());

    // Only redrawn when the frame or how it's shown changes.
    let shown = Shown {
        frame: (frame.id, frame.width, frame.height),
        detection: detection
            .map(|d| (d.threshold, d.edge_threshold(), d.crop, d.edge_width(), d.timing_spot(), d.timing_threshold())),
        overlays: (overlays.crop.visible, overlays.detection.visible, overlays.timing.visible),
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
    /// Threshold, edge threshold, crop, edge width, and the timing LED's spot and threshold.
    detection: Option<(u8, u8, Crop, f32, Option<Crop>, u8)>,
    /// Whether the crop, detection and timing LED are shown.
    overlays: (bool, bool, bool),
}

/// The frame in colour: grey, with above-threshold pixels in the crop highlighted (when detection's shown),
/// and the outside of the crop black (when the crop's hidden). The timing LED's spot, if it's shown, is seen whatever the
/// crop, with its own threshold.
fn colour_image(frame: FrameView, detection: Option<&Detection>, overlays: &Overlays) -> ColorImage {
    let (width, height) = (frame.width, frame.height);
    let mut pixels = Vec::with_capacity(width * height);
    for (y, row) in frame.pixels.chunks_exact(width.max(1)).enumerate() {
        let spans = detection.map(|d| (d, row_spans(d, y as u32, width as u32)));
        for (x, &value) in row.iter().enumerate() {
            let x = x as u32;
            let colour = match spans {
                None => Color32::from_gray(value),
                Some((detection, spans)) => {
                    let inside = x >= spans.outer.0 && x < spans.outer.1;
                    let in_band = !(x >= spans.inner.0 && x < spans.inner.1);
                    let threshold = if in_band { detection.edge_threshold() } else { detection.threshold };
                    let in_timing = x >= spans.timing.0 && x < spans.timing.1;
                    if in_timing && overlays.timing.visible {
                        if value > detection.timing_threshold() { HIGHLIGHT } else { Color32::from_gray(value) }
                    } else if !inside && !overlays.crop.visible && detection.crop != Crop::None {
                        Color32::BLACK
                    } else if inside && !in_timing && overlays.detection.visible && value > threshold {
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
    use crate::NodeType;
    use crate::Nodes;
    use ephys_rs::engine::NodeTypeEnum;
    use ephys_rs::engine::{Graph, NodeId, NodesEnum, Port, PortRole, Runner, Snapshot};

    /// The path's sent to the window while recording, a pass at a time, and goes when recording starts again.
    #[test]
    fn the_path_is_sent_while_recording_and_restarts_with_it() {
        use crate::standard::tracking::{PositionSample, PositionsBuffer};
        let mut graph = Graph::<Nodes>::default();
        let view = graph.add(Nodes::TrackerView(TrackerView::default()), [0.0, 0.0]);
        let by_node = ephys_rs::engine::SharedByNode::default().with(view, TrackerShared::default());
        let (shared, mut tracking) = (by_node.get::<TrackerShared>(view).unwrap(), Tracking::default());
        let ui_state = std::cell::RefCell::new(Box::new(TrackerUi::default()) as Box<dyn std::any::Any>);
        let mut run_pass = |recording: bool, x: f32| {
            let leds = [Some(Blob { x, y: 1.0, area: 9 }), None];
            let sample = PositionSample { frame: 0, time: 0.0, leds, timing_led: None, pixels_per_metre: f32::NAN };
            let positions = Buffer::Positions(PositionsBuffer { samples: vec![sample] });
            let inputs = [None, Some(&positions), None];
            let mut pass = Pass::new(0, 0.02, &inputs, &mut []);
            (pass.shared, pass.node, pass.start_recording) = (shared, view, recording.then(std::time::Instant::now));
            TrackerView::default().process(&mut pass, &mut tracking);
        };
        // What the GUI's side has, once it's taken what's been sent.
        let mut xs = || {
            let colours = &crate::standard::themes::THEMES[0].colours;
            let inputs = [None, None, None];
            graph.edit(view, &inputs, colours, Some(&by_node), Some(&ui_state), None, true, false, None, |n, c| {
                _ = n.gui_background(c)
            });
            let state = ui_state.borrow();
            state.downcast_ref::<TrackerUi>().unwrap().path.iter().map(|p| p[0]).collect::<Vec<_>>()
        };
        run_pass(false, 1.0);
        run_pass(true, 2.0);
        run_pass(true, 3.0);
        assert_eq!(xs(), [2.0, 3.0], "only while recording");
        run_pass(false, 4.0);
        run_pass(true, 5.0);
        assert_eq!(xs(), [5.0], "a new recording, a new path");
    }

    #[test]
    fn lays_out_by_the_window_shape() {
        let shape = |w: f32, h: f32| Arrangement::of(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h)));
        assert!(shape(1100.0, 260.0) == Arrangement::Columns);
        assert!(shape(800.0, 520.0) == Arrangement::Square);
        assert!(shape(400.0, 520.0) == Arrangement::Rows);
    }

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let node_type = NodeType::named(name).unwrap();
        graph.add(Nodes::create(node_type), [0.0, 0.0])
    }

    /// The LEDs in the view's latest display, once it has a frame.
    fn leds_shown(runner: &Runner<Nodes>, view: NodeId) -> Option<usize> {
        let shared = runner.shared();
        let display = shared.get::<TrackerShared>(view)?.shown.newest();
        display.filter(|d| d.frame.width > 0).map(|d| d.leds.len())
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
        graph.connect(port(camera, PortRole::Output, 0), port(detector, PortRole::Input, 0)).unwrap();
        graph.connect(port(camera, PortRole::Output, 0), port(view, PortRole::Input, 0)).unwrap();
        graph.connect(port(detector, PortRole::Output, 0), port(view, PortRole::Input, 1)).unwrap();
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

    #[test]
    fn a_timing_led_takes_its_spot_from_the_others_and_is_seen_coming_on() {
        let mut graph = Graph::default();
        let camera = add(&mut graph, "Camera");
        let address = super::super::camera::serve_test_scene(640, 480);
        if let Some(Nodes::Camera(c)) = graph.get_mut(camera).map(|e| &mut e.node) {
            c.source = super::super::camera::Source::Fake { address };
        }
        let detector = add(&mut graph, "Position Detector");
        let view = add(&mut graph, "Tracker View");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(camera, PortRole::Output, 0), port(detector, PortRole::Input, 0)).unwrap();
        graph.connect(port(camera, PortRole::Output, 0), port(view, PortRole::Input, 0)).unwrap();
        graph.connect(port(detector, PortRole::Output, 0), port(view, PortRole::Input, 1)).unwrap();
        // Over the test scene's big LED, in the middle of the frame.
        if let Some(Nodes::PositionDetector(d)) = graph.get_mut(detector).map(|e| &mut e.node) {
            let center = [320.5, 240.5];
            d.detection.timing_led =
                crate::standard::tracking::TimingLed { enabled: true, center, radius: 12.0, ..Default::default() };
        }
        let runner = Runner::start(&Snapshot::new(0, graph, None), 2);
        let shown = || {
            let shared = runner.shared();
            let display = shared.get::<TrackerShared>(view)?.shown.newest();
            display.filter(|d| d.frame.width > 0).map(|d| (d.leds.len(), d.timing_led))
        };
        assert!(wait_for(|| shown() == Some((1, Some(true)))), "only the small LED's left: {:?}", shown());
    }

    #[test]
    fn a_new_frame_size_carries_on_with_the_camera() {
        // The test scene takes one connection, so LEDs found after the change show it wasn't opened again.
        let mut graph = Graph::default();
        let camera = add(&mut graph, "Camera");
        let address = super::super::camera::serve_test_scene(640, 480);
        if let Some(Nodes::Camera(c)) = graph.get_mut(camera).map(|e| &mut e.node) {
            c.source = super::super::camera::Source::Fake { address };
            c.settings.frame_size = [320, 240];
        }
        let detector = add(&mut graph, "Position Detector");
        let view = add(&mut graph, "Tracker View");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(camera, PortRole::Output, 0), port(detector, PortRole::Input, 0)).unwrap();
        graph.connect(port(camera, PortRole::Output, 0), port(view, PortRole::Input, 0)).unwrap();
        graph.connect(port(detector, PortRole::Output, 0), port(view, PortRole::Input, 1)).unwrap();
        let first = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start(&first, 2);

        let mut update = None;
        assert!(wait_for(|| {
            update = runner.direct_triggers().take_settings_updates().into_iter().next();
            update.is_some()
        }));
        let (_, setting, value) = update.unwrap();
        let value = value.downcast::<crate::Setting>().unwrap();
        graph.get_mut(camera).unwrap().node.set_setting(setting, *value);
        assert!(!runner.update(&Snapshot::new(1, graph, Some(&first))), "a new epoch");
        assert!(wait_for(|| leds_shown(&runner, view) == Some(2)), "both LEDs: {:?}", leds_shown(&runner, view));
    }

    #[test]
    fn the_camera_updates_to_the_size_its_sending_unless_recording() {
        let mut graph = Graph::default();
        let camera = add(&mut graph, "Camera");
        let address = super::super::camera::serve_test_scene(320, 240);
        if let Some(Nodes::Camera(c)) = graph.get_mut(camera).map(|e| &mut e.node) {
            c.source = super::super::camera::Source::Fake { address };
        }
        let idle = Snapshot::new(0, graph, None);
        let recording = idle.with_recording(1, Some(std::sync::Arc::new(ephys_rs::engine::Recording::default())));
        let runner = Runner::start(&recording, 2);
        let updated = || {
            let updates = runner.direct_triggers().take_settings_updates();
            updates.into_iter().find_map(|(_, _, value)| value.downcast::<crate::Setting>().ok()).map(|value| *value)
        };
        std::thread::sleep(Duration::from_millis(1000));
        assert!(updated().is_none(), "settings are locked while recording");

        assert!(runner.update(&recording.with_recording(2, None)));
        let mut value = None;
        assert!(wait_for(|| {
            value = updated();
            value.is_some()
        }));
        let Some(crate::Setting::Camera(settings)) = value else { panic!("the camera's settings") };
        assert_eq!(settings.frame_size, [320, 240]);
        assert_eq!(settings.frame_rate, 50.0, "and the rest as they were");
    }

    /// The window, laid out as in the tracker plugin, on a made-up frame. See tests/snapshots/.
    #[test]
    fn window() {
        use crate::standard::themes::THEMES;
        use crate::standard::tracking::camera::capture::Status;
        use crate::standard::tracking::camera::{CameraControls, Range};
        use crate::standard::tracking::{Crop, Ruler};

        let mut graph = Graph::default();
        let camera = add(&mut graph, "Camera");
        let detector = add(&mut graph, "Position Detector");
        let view = add(&mut graph, "Tracker View");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(camera, PortRole::Output, 0), port(detector, PortRole::Input, 0)).unwrap();
        graph.connect(port(camera, PortRole::Output, 0), port(view, PortRole::Input, 0)).unwrap();
        graph.connect(port(detector, PortRole::Output, 0), port(view, PortRole::Input, 1)).unwrap();
        graph.connect(port(camera, PortRole::Output, 1), port(view, PortRole::Input, 2)).unwrap();
        graph.connect(port(view, PortRole::Control, 0), port(camera, PortRole::Setting, 0)).unwrap();
        graph.connect(port(view, PortRole::Control, 1), port(detector, PortRole::Setting, 0)).unwrap();
        if let Some(Nodes::PositionDetector(d)) = graph.get_mut(detector).map(|e| &mut e.node) {
            d.detection.crop = Crop::Rectangle { center: [320.0, 240.0], width: 420.0, height: 300.0, angle: 0.3 };
            d.detection.rulers.push(Ruler { a: [110.0, 420.0], b: [470.0, 420.0], length_cm: 100.0 });
        }

        // A lit arena on a dark floor, the two LEDs, and a timing LED away from them.
        let (width, height) = (640, 480);
        let mut pixels = vec![25u8; width * height];
        for y in 56..64 {
            pixels[y * width + 586..y * width + 594].fill(255);
        }
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
            timing_led: Some(true),
            stats: Some(CaptureStats { status, ..Default::default() }),
            ..Default::default()
        };
        for frame in 0..500 {
            let t = frame as f64 / 50.0;
            let wobble = (t * 2.0).sin();
            let small = if (300..312).contains(&frame) { 0 } else { (12.0 + wobble) as u32 };
            let timing_on = (200..215).contains(&frame);
            led_chart::record(&mut display.history, t, [(30.0 + 2.0 * wobble) as u32, small], timing_on);
        }

        let resolved = graph.resolve();
        let inputs = resolved[&view].input_streams.clone();
        let shown = TrackerShared { shown: Pool::holding(display), path: Pool::default() };
        let shared = ephys_rs::engine::SharedByNode::default().with(view, shown);
        let mut harness = egui_kittest::Harness::builder().with_size([1150.0, 650.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    view,
                    &inputs,
                    &THEMES[0].colours,
                    Some(&shared),
                    None,
                    None,
                    true,
                    false,
                    None,
                    |node, context| node.gui_ui(ui, context),
                );
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("tracker_view");

        // With a timing LED, on, outside the crop.
        if let Some(Nodes::PositionDetector(d)) = harness.state_mut().get_mut(detector).map(|e| &mut e.node) {
            d.detection.timing_led.enabled = true;
            d.detection.timing_led.center = [590.0, 60.0];
        }
        harness.set_size(egui::vec2(1150.0, 900.0));
        harness.run_steps(3);
        harness.snapshot("tracker_view_timing_led");
    }

    #[test]
    fn without_video_it_shows_positions_in_their_window_with_nothing_to_adjust() {
        use crate::standard::themes::THEMES;

        let mut graph = Graph::default();
        let vr = add(&mut graph, "VR Computer");
        let view = add(&mut graph, "Tracker View");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(vr, PortRole::Output, 0), port(view, PortRole::Input, 1)).unwrap();
        graph.connect(port(view, PortRole::Control, 1), port(vr, PortRole::Setting, 0)).unwrap();
        let window = [
            ("window_min_x", "0"),
            ("window_max_x", "800"),
            ("window_min_y", "0"),
            ("window_max_y", "400"),
            ("pixels_per_metre", "400"),
            ("ttl_channels", "0"),
        ];
        let custom = window.iter().chain(&[("task", "linear")]).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(Nodes::VrComputer(node)) = graph.get_mut(vr).map(|e| &mut e.node) {
            node.custom = custom;
        }
        let resolved = graph.resolve();
        assert!(graph.get(view).unwrap().node.warnings(&resolved[&view].input_streams).is_empty());
        let path: Vec<[f32; 2]> = (0..200)
            .map(|i| [400.0 + 300.0 * (i as f32 / 30.0).cos(), 200.0 + 140.0 * (i as f32 / 20.0).sin()])
            .collect();
        let [x, y] = *path.last().unwrap();
        let display = TrackerDisplay { leds: vec![Blob { x, y, area: 30 }], ..Default::default() };
        let inputs = resolved[&view].input_streams.clone();
        let shown = TrackerShared { shown: Pool::holding(display), path: Pool::default() };
        let shared = ephys_rs::engine::SharedByNode::default().with(view, shown);
        // The path the GUI's side has.
        let ui_state = std::cell::RefCell::new(Box::new(TrackerUi { path }) as Box<dyn std::any::Any>);
        let before = graph.clone();
        let mut harness = egui_kittest::Harness::builder().with_size([900.0, 500.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    view,
                    &inputs,
                    &THEMES[0].colours,
                    Some(&shared),
                    Some(&ui_state),
                    None,
                    true,
                    false,
                    None,
                    |node, context| node.gui_ui(ui, context),
                );
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("tracker_view_positions");
        assert!(harness.state() == &before, "it's only shown");
    }
}
