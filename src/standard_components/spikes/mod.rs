//! Spikes: detected in continuous data against a threshold, and viewed (where the threshold can be adjusted).

pub mod spike_detector;
pub mod spike_viewer;

use eframe::egui::Color32;

use super::per_pass;
use super::widgets::format_rate;
use crate::core::engine::{DataStreamType, EdgeInfo, EdgeType};

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

/// A detected spike, at a sample within the pass.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // read by viewers and writers, once they're fed real data
pub struct Spike {
    pub sample: u32,
    pub channel: u32,
}

pub struct SpikesBuffer {
    /// Room for up to one spike per millisecond per channel; any beyond that are lost.
    pub spikes: Vec<Spike>,
}

impl DataStreamType for Spikes {
    type Buffer = SpikesBuffer;

    fn buffer(&self, duration: f64) -> SpikesBuffer {
        let capacity = self.channels as usize * per_pass(1000.0, duration);
        SpikesBuffer { spikes: Vec::with_capacity(capacity) }
    }

    fn bytes(buffer: &SpikesBuffer) -> usize {
        buffer.spikes.capacity() * size_of::<Spike>()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::EdgeType;

    #[test]
    fn summary() {
        assert_eq!(Spikes { channels: 4, sample_rate: 2500.0 }.summary(), "4 ch · 2.5 kHz");
    }
}
