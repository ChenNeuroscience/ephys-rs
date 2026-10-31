//! The data edge types: what each kind of signal looks like, as known when configuring the graph.

use eframe::egui::Color32;

use crate::engine::{EdgeInfo, EdgeType};

/// Regularly sampled voltages.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Continuous {
    pub channels: u32,
    pub sample_rate: f32,
}

impl EdgeType for Continuous {
    const INFO: EdgeInfo = EdgeInfo {
        name: "continuous",
        colour: Color32::from_rgb(90, 160, 255),
        description: "Regularly sampled voltages, one or more channels.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {}", self.channels, format_rate(self.sample_rate))
    }
}

/// TTL-style digital events.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Events {
    pub lines: u32,
}

impl EdgeType for Events {
    const INFO: EdgeInfo = EdgeInfo {
        name: "events",
        colour: Color32::from_rgb(190, 130, 255),
        description: "Digital events, e.g. TTL inputs.",
    };

    fn summary(&self) -> String {
        format!("{} lines", self.lines)
    }
}

/// Detected spikes: a timestamp, channel and waveform each.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spikes {
    pub channels: u32,
    pub sample_rate: f32,
}

impl EdgeType for Spikes {
    const INFO: EdgeInfo = EdgeInfo {
        name: "spikes",
        colour: Color32::from_rgb(255, 160, 60),
        description: "Detected spikes, each with a timestamp, channel and waveform.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {}", self.channels, format_rate(self.sample_rate))
    }
}

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

/// "30 kHz", "2.5 kHz", "500 Hz".
pub fn format_rate(hz: f32) -> String {
    if hz >= 1000.0 { format!("{} kHz", hz / 1000.0) } else { format!("{hz} Hz") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        assert_eq!(Continuous { channels: 32, sample_rate: 30000.0 }.summary(), "32 ch · 30 kHz");
        assert_eq!(Spikes { channels: 4, sample_rate: 2500.0 }.summary(), "4 ch · 2.5 kHz");
        assert_eq!(Video { width: 640, height: 480, fps: 30.0 }.summary(), "640×480 · 30 fps");
    }
}
