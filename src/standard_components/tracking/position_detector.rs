//! Finds LEDs in video: in each frame, the two biggest bright spots within the crop (see `tracking::pipeline`). There
//! can also be a timing LED, in a spot of its own: its coming on and going off are the `pos ttl` output's ups and downs,
//! timed by the camera's clock, as the positions are.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Input, Node, NodeInfo, Output, Pass, SettingPort, UiContext};
use crate::core::gui::widgets::{Icon, small_icon_button};
use crate::standard_components::ephys::{Event, Events};
use crate::standard_components::tracking::pipeline::{Pipeline, timing_led_on};
use crate::standard_components::tracking::{CAMERA_TRACKER, PositionSample, Positions};
use crate::standard_components::tracking::{Crop, Detection, Ruler, TimingLed};
use crate::standard_components::widgets::{named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PositionDetector {
    pub detection: Detection,
}

/// The pipeline's working memory, and whether the timing LED was on in the last frame, if there's been one.
#[derive(Default)]
pub struct Finding {
    pipeline: Pipeline,
    timing_led: Option<bool>,
}

impl crate::core::engine::PassState for Finding {
    const WHY: &'static str = "Whether the timing LED was on in the last frame, as it's its changes that are output.";
}

/// Whether `on` is a change from the last frame, for the first frame only taking note of it: it wasn't seen coming on.
fn changed(last: &mut Option<bool>, on: bool) -> bool {
    last.replace(on).is_some_and(|was| was != on)
}

impl Node<Edges> for PositionDetector {
    const INFO: NodeInfo = NodeInfo {
        name: "Position Detector",
        category: Category::Processing,
        description: "Finds the two biggest bright spots (LEDs) in each video frame, within a crop, and outputs \
                      their positions. A timing LED, in a spot of its own, can be output as a TTL.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn runs_given(&self, inputs: &[bool], outputs: &[bool]) -> bool {
        inputs.iter().any(|&used| used) && outputs.iter().any(|&used| used)
    }

    type State = Finding;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "video", kind: Kind::Video }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let (positions, ttl) = match inputs[0] {
            Some(DataStream::Video(video)) => (
                Some(DataStream::Positions(Positions {
                    fps: video.fps,
                    width: video.width,
                    height: video.height,
                    source: CAMERA_TRACKER,
                })),
                Some(DataStream::Events(Events { lines: 1, sample_rate: video.fps })),
            ),
            _ => (None, None),
        };
        vec![
            Output { name: "positions", kind: Kind::Positions, stream: positions },
            Output { name: "pos ttl", kind: Kind::Events, stream: ttl },
        ]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "video")
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "detection", value: Setting::Detection(self.detection.clone()), on_right: true }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Detection(detection) = value {
            self.detection = detection;
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, finding: &mut Finding) {
        let Some(Some(Buffer::Video(video))) = pass.inputs.first() else { return };
        let [Buffer::Positions(output), Buffer::Events(ttl)] = &mut *pass.outputs else { return };
        output.samples.clear();
        ttl.events.clear();
        ttl.pass_samples = video.frames;
        let pixels_per_metre = self.detection.pixels_per_metre().unwrap_or(f32::NAN);
        let (width, height) = (video.width as u32, video.height as u32);
        for frame in 0..video.frames.min(output.samples.capacity()) {
            let pixels = video.frame(frame);
            let blobs = finding.pipeline.process(pixels, width, height, &self.detection);
            let leds = [blobs.first().copied(), blobs.get(1).copied()];
            let timing_led = timing_led_on(pixels, width, height, &self.detection);
            output.samples.push(PositionSample {
                frame: video.ids[frame],
                time: video.times[frame],
                leds,
                timing_led,
                pixels_per_metre,
            });
            match timing_led {
                Some(on) if changed(&mut finding.timing_led, on) && ttl.events.len() < ttl.events.capacity() => {
                    let time = Some(video.times[frame]);
                    ttl.events.push(Event { sample: frame as u32, line: 0, high: on, time });
                }
                Some(_) => {}
                None => finding.timing_led = None,
            }
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let size = match context.inputs[0] {
            Some(DataStream::Video(video)) => [video.width as f32, video.height as f32],
            _ => [640.0, 480.0],
        };
        all_detection_ui(ui, &mut self.detection, size);
    }
}

/// The detection settings: threshold, sizes, crop and calibration rulers. `size` is the frame size, for placing
/// new crops and rulers.
fn all_detection_ui(ui: &mut egui::Ui, detection: &mut Detection, size: [f32; 2]) {
    ui.label(RichText::new("Detection").strong());
    detection_ui(ui, detection);

    ui.add_space(6.0);
    ui.label(RichText::new("Crop").strong());
    crop_ui(ui, &mut detection.crop, size);

    ui.add_space(6.0);
    ui.label(RichText::new("Calibration").strong());
    calibration_ui(ui, detection, size, false);

    ui.add_space(6.0);
    ui.label(RichText::new("Timing LED").strong());
    timing_led_ui(ui, detection, size, None);
}

/// Whether there's a timing LED, and how it's told to be on (`on`, if it's known, says whether it is). `size` is the
/// frame size, for placing it the first time. Returns whether it was switched on.
pub(super) fn timing_led_ui(ui: &mut egui::Ui, detection: &mut Detection, size: [f32; 2], on: Option<bool>) -> bool {
    let (threshold, min_area) = (detection.threshold, detection.min_area);
    let led = &mut detection.timing_led;
    let was_enabled = led.enabled;
    ui.checkbox(&mut led.enabled, "Use a timing LED").on_hover_text(
        "An LED in a spot of its own, usually outside the crop: it coming on is an up on the pos ttl output, and it \
         going off a down. Drag the circle over it. The spot isn't looked at for the other LEDs.",
    );
    let switched_on = led.enabled && !was_enabled;
    if switched_on && led.center == [0.0; 2] {
        led.center = [size[0] - 40.0, 40.0];
    }
    if !led.enabled {
        return false;
    }
    named_settings_grid(ui, "timing led", |ui| {
        setting(ui, "Centre", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut led.center[0]).speed(1.0).prefix("x "));
                ui.add(egui::DragValue::new(&mut led.center[1]).speed(1.0).prefix("y "));
            })
            .response
        });
        setting(ui, "Radius", |ui| {
            ui.add(egui::DragValue::new(&mut led.radius).speed(0.25).range(TimingLed::RADII).suffix(" px"))
        });
        setting(ui, "Threshold", |ui| {
            ui.horizontal(|ui| {
                let was_separate = led.separate;
                ui.selectable_value(&mut led.separate, false, "Same as the LEDs");
                ui.selectable_value(&mut led.separate, true, "Its own");
                if led.separate && !was_separate {
                    (led.threshold, led.min_area) = (threshold, min_area);
                }
            })
            .response
        });
        if led.separate {
            setting(ui, "Its threshold", |ui| ui.add(egui::Slider::new(&mut led.threshold, 0..=255)));
            setting(ui, "Its min area", |ui| {
                ui.add(egui::DragValue::new(&mut led.min_area).range(1..=100_000).suffix(" px"))
            });
        }
    });
    match on {
        Some(true) => ui.label(RichText::new("On").strong()),
        Some(false) => ui.weak("Off"),
        None => ui.weak("Press Play to see whether it's on"),
    };
    switched_on
}

/// The threshold and the sizes of LED to look for.
pub(super) fn detection_ui(ui: &mut egui::Ui, detection: &mut Detection) {
    named_settings_grid(ui, "detection", |ui| {
        setting(ui, "Threshold", |ui| ui.add(egui::Slider::new(&mut detection.threshold, 0..=255)));
        setting(ui, "Min area", |ui| {
            ui.add(egui::DragValue::new(&mut detection.min_area).range(0..=100_000).suffix(" px"))
        });
        setting(ui, "Max area", |ui| {
            ui.horizontal(|ui| {
                let mut bounded = detection.max_area.is_some();
                ui.checkbox(&mut bounded, "at most");
                detection.max_area = bounded.then(|| {
                    let mut v = detection.max_area.unwrap_or(1000);
                    ui.add(egui::DragValue::new(&mut v).range(detection.min_area..=1_000_000).suffix(" px"));
                    v
                });
            })
            .response
        });
    });
    if detection.crop != Crop::None {
        let edge = &mut detection.edge;
        ui.horizontal(|ui| {
            ui.checkbox(&mut edge.enabled, "Threshold edge").on_hover_text(
                "A higher threshold for pixels near the crop boundary, so LED reflections off the walls don't count. \
                 The dashed line shows where it stops.",
            );
            if edge.enabled {
                edge.threshold = edge.threshold.max(detection.threshold);
                ui.add(egui::DragValue::new(&mut edge.threshold).range(detection.threshold..=255).prefix("≥ "))
                    .on_hover_text("Threshold within the band (at least the main threshold)");
                ui.label("within");
                ui.add(egui::DragValue::new(&mut edge.width).range(1.0..=1000.0).speed(0.5).suffix(" px"));
            }
        });
    }
}

/// The crop's shape and where it is. Returns whether the shape was changed.
pub(super) fn crop_ui(ui: &mut egui::Ui, crop: &mut Crop, size: [f32; 2]) -> bool {
    let kinds = ["None", "Circle", "Square", "Rectangle"];
    let current = match crop {
        Crop::None => 0,
        Crop::Circle { .. } => 1,
        Crop::Square { .. } => 2,
        Crop::Rectangle { .. } => 3,
    };
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for (kind, name) in kinds.iter().enumerate() {
            if ui.selectable_label(current == kind, *name).clicked() && current != kind {
                *crop = convert(crop, kind, size);
                changed = true;
            }
        }
    });
    named_settings_grid(ui, "crop", |ui| {
        let centre = |ui: &mut egui::Ui, center: &mut [f32; 2]| {
            setting(ui, "Centre", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut center[0]).speed(1.0).prefix("x "));
                    ui.add(egui::DragValue::new(&mut center[1]).speed(1.0).prefix("y "));
                })
                .response
            });
        };
        match crop {
            Crop::None => {}
            Crop::Circle { center, radius } => {
                centre(ui, center);
                setting(ui, "Radius", |ui| ui.add(px(radius)));
            }
            Crop::Square { center, size, angle } => {
                centre(ui, center);
                setting(ui, "Size", |ui| ui.add(px(size)));
                setting(ui, "Angle", |ui| ui.drag_angle(angle));
            }
            Crop::Rectangle { center, width, height, angle } => {
                centre(ui, center);
                setting(ui, "Width", |ui| ui.add(px(width)));
                setting(ui, "Height", |ui| ui.add(px(height)));
                setting(ui, "Angle", |ui| ui.drag_angle(angle));
            }
        }
    });
    changed
}

fn px(v: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(v).speed(1.0).range(1.0..=f32::MAX).suffix(" px")
}

/// `crop` as another kind of shape (by index into None, Circle, Square, Rectangle), keeping its centre, angle
/// and rough size where possible.
fn convert(crop: &Crop, kind: usize, image: [f32; 2]) -> Crop {
    let center = crop.center().unwrap_or([image[0] / 2.0, image[1] / 2.0]);
    let angle = crop.angle();
    let [hw, hh] = crop.half_extents().unwrap_or([0.4 * image[0].min(image[1]); 2]);
    match kind {
        1 => Crop::Circle { center, radius: hw.min(hh) },
        2 => Crop::Square { center, size: 2.0 * hw.min(hh), angle },
        3 if hw == hh => Crop::Rectangle { center, width: 2.0 * hw, height: 1.5 * hh, angle },
        3 => Crop::Rectangle { center, width: 2.0 * hw, height: 2.0 * hh, angle },
        _ => Crop::None,
    }
}

/// The calibration, and the rulers it's from: the rulers can't be edited while `locked`. Returns whether a ruler
/// was added.
pub(super) fn calibration_ui(ui: &mut egui::Ui, detection: &mut Detection, size: [f32; 2], locked: bool) -> bool {
    match detection.pixels_per_metre() {
        Some(ppm) => {
            let measured = detection.rulers.iter().filter(|r| r.length_cm > 0.0).count();
            ui.label(RichText::new(format!("{ppm:.1} px/m")).size(18.0).strong());
            ui.weak(format!("{:.2} mm/px, the mean of {measured} ruler(s)", 1000.0 / ppm));
        }
        None => {
            ui.weak("Not calibrated: add a ruler, drag its ends onto something of known size and enter its length.");
        }
    }
    // The readout above stays readable when locked.
    if locked {
        ui.disable();
    }
    let rulers = &mut detection.rulers;
    let added = ui.button("Add ruler").clicked();
    if added {
        let y = size[1] / 2.0 + 20.0 * (rulers.len() % 8) as f32;
        rulers.push(Ruler { a: [size[0] / 3.0, y], b: [size[0] * 2.0 / 3.0, y], length_cm: 0.0 });
    }
    let mut remove = None;
    egui::Grid::new("rulers").num_columns(4).striped(true).show(ui, |ui| {
        for (i, ruler) in rulers.iter_mut().enumerate() {
            let name = ui.label(format!("R{}", i + 1));
            ui.label(format!("{:.1} px", ruler.pixels()));
            ui.add(egui::DragValue::new(&mut ruler.length_cm).speed(0.5).range(0.0..=10_000.0).suffix(" cm"))
                .labelled_by(name.id)
                .on_hover_text("Physical length; 0 leaves this ruler out of the calibration");
            if small_icon_button(ui, Icon::Delete, "Delete ruler", "Delete this ruler").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        rulers.remove(i);
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changes_of_the_timing_led_count_and_not_the_first_frame() {
        let mut last = None;
        let changes: Vec<bool> = [true, true, false, false, true].map(|on| changed(&mut last, on)).to_vec();
        assert_eq!(changes, [false, false, true, false, true]);
    }

    #[test]
    fn says_what_it_outputs() {
        let video = crate::standard_components::tracking::Video { width: 640, height: 480, fps: 50.0 };
        let outputs = PositionDetector::default().outputs(&[Some(DataStream::Video(video))]);
        assert_eq!(outputs[1].name, "pos ttl");
        assert!(matches!(outputs[1].stream, Some(DataStream::Events(Events { lines: 1, .. }))), "a single line");
    }
}
