//! Tracking LEDs on an animal in video, ported from the Open Ephys camera plugin: the Camera node and its
//! backends (`camera`), finding the LEDs in each frame (`pipeline`), and drawing and editing the crop and
//! rulers over the video (`overlay`); and the nodes using them: the Position Detector, Tracker View and Video
//! Viewer.
//!
//! Image coordinates are continuous pixels: pixel (i, j) covers [i, i+1) × [j, j+1).

pub mod camera;
pub mod led_chart;
pub mod overlay;
pub mod pipeline;
pub mod position_detector;
pub mod tracker_view;
pub mod video_viewer;

use std::collections::BTreeMap;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use super::per_pass;
use crate::engine::{DataStreamType, EdgeInfo, EdgeType};
use pipeline::Blob;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

impl EdgeType for Video {
    const INFO: EdgeInfo =
        EdgeInfo { name: "video", colour: Color32::from_rgb(90, 210, 130), description: "Video frames." };

    fn summary(&self) -> String {
        format!("{}×{} · {} fps", self.width, self.height, self.fps)
    }
}

/// Where positions tracked in video come from, as `Positions::source`.
pub const CAMERA_TRACKER: &str = "camera_tracker";

/// The positions of LEDs tracked in video, for each frame the biggest two found; or positions from elsewhere, as if they
/// had been.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Positions {
    /// Frames per second.
    pub fps: f32,
    /// The frames' size, in pixels.
    pub width: u32,
    pub height: u32,
    /// Where they come from, as a name for headers: `CAMERA_TRACKER`, say.
    pub source: &'static str,
}

impl EdgeType for Positions {
    const INFO: EdgeInfo = EdgeInfo {
        name: "positions",
        colour: Color32::from_rgb(240, 100, 160),
        description: "LED positions tracked in video, frame by frame.",
    };

    fn summary(&self) -> String {
        format!("{} fps", self.fps)
    }
}

/// Where the LEDs were in one frame.
#[derive(Clone, Copy, Debug)]
pub struct PositionSample {
    /// The camera's frame id and timestamp (seconds, in the camera's clock).
    pub frame: u64,
    pub time: f64,
    /// The biggest LED found, then the second biggest, in pixels.
    pub leds: [Option<Blob>; 2],
    /// Whether the timing LED was on, if there is one.
    pub timing_led: Option<bool>,
    /// The calibration when it was found; NaN if there wasn't one.
    pub pixels_per_metre: f32,
}

pub struct PositionsBuffer {
    pub samples: Vec<PositionSample>,
}

impl DataStreamType for Positions {
    type Buffer = PositionsBuffer;

    fn buffer(&self, duration: f64) -> PositionsBuffer {
        PositionsBuffer { samples: Vec::with_capacity(per_pass(self.fps, duration) + 1) }
    }

    fn bytes(buffer: &PositionsBuffer) -> usize {
        buffer.samples.capacity() * size_of::<PositionSample>()
    }

    fn copy(from: &PositionsBuffer, to: &mut PositionsBuffer) {
        to.samples.clone_from(&from.samples);
    }
}

/// A pass's video frames (often none or one), each `width` × `height` bytes of grey.
#[derive(Default)]
pub struct VideoBuffer {
    pub width: usize,
    pub height: usize,
    pub frames: usize,
    pub capacity: usize,
    pub pixels: Vec<u8>,
    /// For each frame, the camera's frame id and timestamp (seconds, in the camera's clock).
    pub ids: Vec<u64>,
    pub times: Vec<f64>,
}

impl VideoBuffer {
    pub fn frame(&self, frame: usize) -> &[u8] {
        let size = self.width * self.height;
        &self.pixels[frame * size..(frame + 1) * size]
    }

    pub fn frame_mut(&mut self, frame: usize) -> &mut [u8] {
        let size = self.width * self.height;
        &mut self.pixels[frame * size..(frame + 1) * size]
    }
}

impl DataStreamType for Video {
    type Buffer = VideoBuffer;

    fn buffer(&self, duration: f64) -> VideoBuffer {
        // A frame more than strictly needed, as cameras' timing jitters.
        let (width, height, capacity) = (self.width as usize, self.height as usize, per_pass(self.fps, duration) + 1);
        VideoBuffer {
            width,
            height,
            frames: 0,
            capacity,
            pixels: vec![0; width * height * capacity],
            ids: vec![0; capacity],
            times: vec![0.0; capacity],
        }
    }

    fn bytes(buffer: &VideoBuffer) -> usize {
        buffer.pixels.capacity()
    }

    fn copy(from: &VideoBuffer, to: &mut VideoBuffer) {
        (to.width, to.height, to.frames) = (from.width, from.height, from.frames.min(to.capacity));
        let size = from.width * from.height * to.frames;
        to.pixels[..size].copy_from_slice(&from.pixels[..size]);
        to.ids[..to.frames].copy_from_slice(&from.ids[..to.frames]);
        to.times[..to.frames].copy_from_slice(&from.times[..to.frames]);
    }
}

impl EdgeType for Detection {
    const INFO: EdgeInfo = EdgeInfo {
        name: "detection",
        colour: Color32::from_rgb(255, 130, 110),
        description: "How LEDs are found in video: threshold, sizes, crop and calibration.",
    };

    fn summary(&self) -> String {
        let crop = match self.crop {
            Crop::None => "",
            Crop::Circle { .. } => ", circle",
            Crop::Square { .. } => ", square",
            Crop::Rectangle { .. } => ", rectangle",
        };
        format!("threshold {}{crop}", self.threshold)
    }
}

/// How LEDs are found in each frame: bright regions of a size within limits, inside the crop. Also the
/// calibration, from rulers laid over the video.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detection {
    /// Pixels strictly brighter than this are foreground.
    pub threshold: u8,
    pub min_area: u32,
    /// `None` means unbounded.
    pub max_area: Option<u32>,
    /// Only pixels inside this region are considered.
    pub crop: Crop,
    /// A higher threshold near the crop boundary, to ignore LED reflections off the walls.
    pub edge: EdgeThreshold,
    pub rulers: Vec<Ruler>,
    /// An LED of its own for timing, whose spot's left out of finding the others.
    pub timing_led: TimingLed,
    /// Anything else to record with the positions, by name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub custom: BTreeMap<String, String>,
}

impl Default for Detection {
    fn default() -> Self {
        Detection {
            threshold: 200,
            min_area: 5,
            max_area: None,
            crop: Crop::None,
            edge: EdgeThreshold::default(),
            rulers: Vec::new(),
            timing_led: TimingLed::default(),
            custom: BTreeMap::new(),
        }
    }
}

impl Detection {
    /// The threshold for pixels within the edge band; never below `threshold` (the pipeline's quick
    /// rejection of empty rows relies on that). 255 means nothing there can count.
    pub fn edge_threshold(&self) -> u8 {
        if self.edge_width() <= 0.0 {
            return self.threshold;
        }
        self.edge.threshold.max(self.threshold)
    }

    /// Width of the edge band in effect: 0 when it's switched off or there's no crop.
    pub fn edge_width(&self) -> f32 {
        if self.edge.enabled && self.crop != Crop::None { self.edge.width.max(0.0) } else { 0.0 }
    }

    /// The timing LED's spot, if it's in use.
    pub fn timing_spot(&self) -> Option<Crop> {
        let TimingLed { enabled, center, radius, .. } = self.timing_led;
        enabled.then_some(Crop::Circle { center, radius })
    }

    /// What pixels in the timing LED's spot have to be brighter than, to count.
    pub fn timing_threshold(&self) -> u8 {
        if self.timing_led.separate { self.timing_led.threshold } else { self.threshold }
    }

    /// How many pixels in the timing LED's spot have to count, for it to be on.
    pub fn timing_min_area(&self) -> u32 {
        (if self.timing_led.separate { self.timing_led.min_area } else { self.min_area }).max(1)
    }

    /// The mean over rulers that have a physical length entered.
    pub fn pixels_per_metre(&self) -> Option<f32> {
        let values: Vec<f32> = self.rulers.iter().filter_map(Ruler::pixels_per_metre).collect();
        (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
    }
}

/// When enabled, pixels within `width` of the crop boundary use `threshold` instead of the main threshold.
/// The values are kept while it's switched off.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeThreshold {
    pub enabled: bool,
    /// Pixels.
    pub width: f32,
    pub threshold: u8,
}

impl Default for EdgeThreshold {
    fn default() -> Self {
        EdgeThreshold { enabled: false, width: 25.0, threshold: 250 }
    }
}

/// A spot in the frame, likely outside the crop, where an LED comes on and goes off for timing. The values are kept
/// while it's switched off.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimingLed {
    pub enabled: bool,
    /// Pixels.
    pub center: [f32; 2],
    pub radius: f32,
    /// Whether it has a threshold and area of its own, rather than the other LEDs'.
    pub separate: bool,
    pub threshold: u8,
    pub min_area: u32,
}

impl TimingLed {
    pub const RADII: std::ops::RangeInclusive<f32> = 2.0..=60.0;
}

impl Default for TimingLed {
    fn default() -> Self {
        TimingLed { enabled: false, center: [0.0; 2], radius: 10.0, separate: false, threshold: 200, min_area: 5 }
    }
}

/// Region of interest. Angles are radians, clockwise on screen (since y points down).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Crop {
    #[default]
    None,
    Circle {
        center: [f32; 2],
        radius: f32,
    },
    Square {
        center: [f32; 2],
        size: f32,
        angle: f32,
    },
    Rectangle {
        center: [f32; 2],
        width: f32,
        height: f32,
        angle: f32,
    },
}

impl Crop {
    pub fn center(&self) -> Option<[f32; 2]> {
        match *self {
            Crop::None => None,
            Crop::Circle { center, .. } | Crop::Square { center, .. } | Crop::Rectangle { center, .. } => Some(center),
        }
    }

    pub fn angle(&self) -> f32 {
        match *self {
            Crop::Square { angle, .. } | Crop::Rectangle { angle, .. } => angle,
            _ => 0.0,
        }
    }

    /// Half width and half height in the shape's own (rotated) frame; for a circle, the radius twice.
    pub fn half_extents(&self) -> Option<[f32; 2]> {
        match *self {
            Crop::None => None,
            Crop::Circle { radius, .. } => Some([radius, radius]),
            Crop::Square { size, .. } => Some([size / 2.0, size / 2.0]),
            Crop::Rectangle { width, height, .. } => Some([width / 2.0, height / 2.0]),
        }
    }

    /// Its bounding box (min x, min y, max x, max y), if it has a shape.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let ([cx, cy], [hx, hy]) = (self.center()?, self.half_extents()?);
        let (sin, cos) = self.angle().sin_cos();
        let (ex, ey) = ((hx * cos).abs() + (hy * sin).abs(), (hx * sin).abs() + (hy * cos).abs());
        Some([cx - ex, cy - ey, cx + ex, cy + ey])
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        let (Some(center), Some([hw, hh])) = (self.center(), self.half_extents()) else { return true };
        let [x, y] = to_local(center, self.angle(), p);
        match self {
            Crop::Circle { .. } => x * x + y * y <= hw * hw,
            _ => x.abs() <= hw && y.abs() <= hh,
        }
    }

    /// The points at least `d` inside the boundary, which for these shapes is the same shape shrunk by `d`.
    /// `None` if nothing is left (or there's no crop).
    pub fn inset(&self, d: f32) -> Option<Crop> {
        match *self {
            Crop::None => None,
            Crop::Circle { center, radius } => (radius > d).then_some(Crop::Circle { center, radius: radius - d }),
            Crop::Square { center, size, angle } => {
                (size > 2.0 * d).then_some(Crop::Square { center, size: size - 2.0 * d, angle })
            }
            Crop::Rectangle { center, width, height, angle } => (width > 2.0 * d && height > 2.0 * d)
                .then_some(Crop::Rectangle { center, width: width - 2.0 * d, height: height - 2.0 * d, angle }),
        }
    }

    pub fn set_center(&mut self, c: [f32; 2]) {
        match self {
            Crop::None => {}
            Crop::Circle { center, .. } | Crop::Square { center, .. } | Crop::Rectangle { center, .. } => *center = c,
        }
    }
}

/// Rotates `p` about `center` by -angle, giving coordinates in a shape's own frame.
pub fn to_local(center: [f32; 2], angle: f32, p: [f32; 2]) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
    [dx * c + dy * s, -dx * s + dy * c]
}

/// Inverse of [`to_local`].
pub fn from_local(center: [f32; 2], angle: f32, l: [f32; 2]) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [center[0] + l[0] * c - l[1] * s, center[1] + l[0] * s + l[1] * c]
}

/// A line laid over the video between two points, with the physical length it spans, for calibration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ruler {
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Physical length; 0 means not entered yet.
    pub length_cm: f32,
}

impl Ruler {
    pub fn pixels(&self) -> f32 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }

    pub fn pixels_per_metre(&self) -> Option<f32> {
        (self.length_cm > 0.0).then(|| self.pixels() / (self.length_cm / 100.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary() {
        assert_eq!(Video { width: 640, height: 480, fps: 30.0 }.summary(), "640×480 · 30 fps");
    }

    #[test]
    fn rotated_rectangle_contains() {
        let crop =
            Crop::Rectangle { center: [50.0, 50.0], width: 40.0, height: 10.0, angle: std::f32::consts::FRAC_PI_2 };
        // Rotated by 90 degrees the long side runs vertically.
        assert!(crop.contains([50.0, 68.0]));
        assert!(!crop.contains([68.0, 50.0]));
    }

    #[test]
    fn local_round_trip() {
        let (c, a, p) = ([3.0, -2.0], 0.7, [10.0, 4.0]);
        let back = from_local(c, a, to_local(c, a, p));
        assert!((back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4);
    }

    #[test]
    fn pixels_per_metre_averages_measured_rulers() {
        let detection = Detection {
            rulers: vec![
                Ruler { a: [0.0, 0.0], b: [300.0, 0.0], length_cm: 100.0 },
                Ruler { a: [0.0, 0.0], b: [0.0, 160.0], length_cm: 50.0 },
                Ruler { a: [0.0, 0.0], b: [10.0, 0.0], length_cm: 0.0 }, // not measured: ignored
            ],
            ..Default::default()
        };
        assert_eq!(detection.pixels_per_metre(), Some(310.0));
    }
}
