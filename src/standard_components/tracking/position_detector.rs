//! Finds LEDs in video: in each frame, the two biggest bright spots within the crop (see `tracking::pipeline`).

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::standard_components::tracking::pipeline::Pipeline;
use crate::standard_components::tracking::{Crop, Detection, Ruler};
use crate::standard_components::tracking::{PositionSample, Positions};
use crate::standard_components::widgets::{named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PositionDetector {
    pub detection: Detection,
}

impl Node<Edges> for PositionDetector {
    const INFO: NodeInfo = NodeInfo {
        name: "Position Detector",
        category: Category::Processing,
        description: "Finds the two biggest bright spots (LEDs) in each video frame, within a crop, and outputs \
                      their positions.",
    };

    /// The pipeline's working memory, kept between frames.
    type State = Pipeline;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "video", kind: Kind::Video }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Video(video)) => Some(DataStream::Positions(Positions { fps: video.fps })),
            _ => None,
        };
        vec![Output { name: "positions", kind: Kind::Positions, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "video")
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "detection", value: Setting::Detection(self.detection.clone()) }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Detection(detection) = value {
            self.detection = detection;
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, pipeline: &mut Pipeline) {
        let Some(Some(Buffer::Video(video))) = pass.inputs.first() else { return };
        let Some(Buffer::Positions(output)) = pass.outputs.get_mut(0) else { return };
        output.samples.clear();
        let pixels_per_metre = self.detection.pixels_per_metre().unwrap_or(f32::NAN);
        for frame in 0..video.frames.min(output.samples.capacity()) {
            let blobs = pipeline.process(video.frame(frame), video.width as u32, video.height as u32, &self.detection);
            output.samples.push(PositionSample {
                frame: video.ids[frame],
                time: video.times[frame],
                leds: [blobs.first().copied(), blobs.get(1).copied()],
                pixels_per_metre,
            });
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
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
            if ui.small_button("×").on_hover_text("Remove ruler").clicked() {
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
