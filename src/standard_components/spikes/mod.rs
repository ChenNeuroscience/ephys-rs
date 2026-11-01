//! Spikes: detected in groups of channels (tetrodes, say) of continuous data against a threshold, and viewed
//! (where the threshold can be adjusted).

pub mod spike_detector;
pub mod spike_viewer;

use eframe::egui::Color32;

use super::per_pass;
use super::widgets::format_rate;
use crate::core::engine::{DataStreamType, EdgeInfo, EdgeType};

/// Detected spikes, from groups of channels (tetrodes, say): each with when it was, its group, and its waveform
/// on each of the group's channels. A group's spikes don't overlap (each waits for the last's waveform to end),
/// which bounds how many a pass can have, so a buffer always has room for them all.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spikes {
    pub channels: u32,
    pub sample_rate: f32,
    /// Channels in each group (the last may have fewer).
    pub group_size: u32,
    /// Waveform samples before and after each spike's peak.
    pub pre: u32,
    pub post: u32,
}

impl Spikes {
    pub fn groups(&self) -> u32 {
        self.channels.div_ceil(self.group_size.max(1))
    }

    /// Samples in each waveform.
    pub fn waveform_samples(&self) -> u32 {
        self.pre + self.post
    }

    /// The channels of group `group`.
    pub fn group_channels(&self, group: u32) -> std::ops::Range<u32> {
        let size = self.group_size.max(1);
        group * size..((group + 1) * size).min(self.channels)
    }
}

impl EdgeType for Spikes {
    const INFO: EdgeInfo = EdgeInfo {
        name: "spikes",
        colour: Color32::from_rgb(255, 160, 60),
        description: "Detected spikes, from groups of channels (tetrodes, say): each with its time, group and \
                      waveforms.",
    };

    fn summary(&self) -> String {
        let groups = match self.group_size {
            1 => "channels",
            2 => "stereotrodes",
            4 => "tetrodes",
            _ => "groups",
        };
        format!("{} {groups} · {}", self.groups(), format_rate(self.sample_rate))
    }
}

/// A detected spike.
#[derive(Clone, Copy, Debug)]
pub struct Spike {
    /// Its peak, as a sample of the pass (negative if it was in an earlier pass: a spike is only found once its
    /// whole waveform has come).
    pub sample: i32,
    pub group: u32,
    /// The channel (of all of them) on which it crossed the threshold.
    pub channel: u32,
}

pub struct SpikesBuffer {
    pub spikes: Vec<Spike>,
    /// Each spike's waveforms, one after another: for each of its group's channels (`group_size` of them, the
    /// absent ones of a smaller last group being zero), `samples` values, in µV.
    pub waveforms: Vec<f32>,
    pub group_size: usize,
    pub samples: usize,
    /// Each channel's threshold as the pass ended, in µV (negative for negative-going spikes).
    pub thresholds: Vec<f32>,
    /// How many samples of continuous data the pass covers (for timing spikes across passes).
    pub pass_samples: usize,
}

impl SpikesBuffer {
    /// Spike `index`'s waveforms.
    pub fn waveform(&self, index: usize) -> &[f32] {
        let size = self.group_size * self.samples;
        &self.waveforms[index * size..(index + 1) * size]
    }

    pub fn clear(&mut self) {
        self.spikes.clear();
        self.waveforms.clear();
    }

    /// Adds a spike, returning room for its waveforms; `None` if the buffer is full (which, with the detector's
    /// spikes not overlapping, it never is).
    pub fn push(&mut self, spike: Spike) -> Option<&mut [f32]> {
        if self.spikes.len() == self.spikes.capacity() {
            return None;
        }
        self.spikes.push(spike);
        let size = self.group_size * self.samples;
        let start = self.waveforms.len();
        self.waveforms.resize(start + size, 0.0);
        Some(&mut self.waveforms[start..])
    }
}

impl DataStreamType for Spikes {
    type Buffer = SpikesBuffer;

    /// Room for as many spikes as a pass can have: a pass of continuous data is at most `per_pass(...) * 5/4`
    /// samples (see `Continuous`), and a group's spikes are at least a waveform apart.
    fn buffer(&self, duration: f64) -> SpikesBuffer {
        let samples = per_pass(self.sample_rate, duration) * 5 / 4;
        let waveform = self.waveform_samples().max(1) as usize;
        let capacity = self.groups() as usize * (samples.div_ceil(waveform) + 1);
        let group_size = self.group_size.max(1) as usize;
        SpikesBuffer {
            spikes: Vec::with_capacity(capacity),
            waveforms: Vec::with_capacity(capacity * group_size * waveform),
            group_size,
            samples: waveform,
            thresholds: vec![0.0; self.channels as usize],
            pass_samples: 0,
        }
    }

    fn bytes(buffer: &SpikesBuffer) -> usize {
        buffer.spikes.capacity() * size_of::<Spike>() + buffer.waveforms.capacity() * size_of::<f32>()
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
        let spikes = Spikes { channels: 64, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32 };
        assert_eq!(spikes.summary(), "16 tetrodes · 30 kHz");
        assert_eq!(spikes.group_channels(15), 60..64);
    }
}
