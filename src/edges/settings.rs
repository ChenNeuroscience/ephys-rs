//! The settings edge types: values one node can edit on another.

use eframe::egui::Color32;

use crate::engine::{EdgeInfo, EdgeType};
use crate::tracking::{CameraSettings, Crop, Detection};

/// A spike detection threshold.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Threshold {
    /// In standard deviations of the noise.
    pub sd: f32,
}

impl EdgeType for Threshold {
    const INFO: EdgeInfo = EdgeInfo {
        name: "threshold",
        colour: Color32::from_rgb(245, 215, 80),
        description: "A spike detection threshold, in standard deviations of the noise.",
    };

    fn summary(&self) -> String {
        format!("{:.2} SD", self.sd)
    }
}

/// A scale factor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Gain {
    pub factor: f32,
}

impl EdgeType for Gain {
    const INFO: EdgeInfo =
        EdgeInfo { name: "gain", colour: Color32::from_rgb(80, 210, 220), description: "A scale factor." };

    fn summary(&self) -> String {
        format!("×{:.2}", self.factor)
    }
}

impl EdgeType for CameraSettings {
    const INFO: EdgeInfo = EdgeInfo {
        name: "camera",
        colour: Color32::from_rgb(150, 160, 255),
        description: "A camera's frame rate, exposure and gain.",
    };

    fn summary(&self) -> String {
        format!("{} fps", self.frame_rate)
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
