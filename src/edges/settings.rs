//! The settings edge types: values one node can edit on another.

use eframe::egui::Color32;

use crate::engine::{EdgeInfo, EdgeType};

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
