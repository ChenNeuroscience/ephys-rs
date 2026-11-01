//! Spikes: detected in groups of channels (tetrodes, say) of continuous data against each channel's threshold,
//! and viewed (where the thresholds, and the gains they're recorded at, can be adjusted).

pub mod spike_detector;
pub mod spike_viewer;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

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
    /// Which way the spikes go.
    pub polarity: Polarity,
}

/// Which way spikes go past their channel's threshold. (Extracellular spikes mostly go negative: near a firing cell
/// the current flows in, leaving the outside more negative.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Polarity {
    #[default]
    Negative,
    Positive,
    Both,
}

impl Polarity {
    /// How far a value is past zero, the way spikes go.
    pub fn extremity(self, x: f32) -> f32 {
        match self {
            Polarity::Negative => -x,
            Polarity::Positive => x,
            Polarity::Both => x.abs(),
        }
    }
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
    /// Each channel's threshold, gain and noise (a standard deviation, µV) as the pass ended.
    pub thresholds: Vec<f32>,
    pub gains: Vec<f32>,
    pub noise: Vec<f32>,
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
            gains: vec![DEFAULT_GAIN; self.channels as usize],
            noise: vec![0.0; self.channels as usize],
            pass_samples: 0,
        }
    }

    fn bytes(buffer: &SpikesBuffer) -> usize {
        buffer.spikes.capacity() * size_of::<Spike>() + buffer.waveforms.capacity() * size_of::<f32>()
    }
}

/// The full scale of a gain of 1, in µV: at gain `g`, spikes' waveforms are recorded (in the LTX format, as 8-bit
/// samples) and viewed over ±`FULL_SCALE_UV / g`. (As in the Axona format the LTX format comes from, whose `.set`
/// files give each channel's gain, and the full scale as `ADC_fullscale_mv`.)
pub const FULL_SCALE_UV: f32 = 1_500_000.0;
/// A channel's gain until it's set: ±250 µV.
pub const DEFAULT_GAIN: f32 = 6000.0;
/// A channel's threshold until it's set, in µV.
pub const DEFAULT_THRESHOLD_UV: f32 = 50.0;
/// The gains that can be set.
pub const GAINS: std::ops::RangeInclusive<f32> = 50.0..=100_000.0;

/// The µV either side of zero a gain shows (and records).
pub fn range_uv(gain: f32) -> f32 {
    FULL_SCALE_UV / gain.max(1.0)
}

/// The next gain up or down from `gain` in steps of 1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8 (×10ⁿ).
pub fn gain_step(gain: f32, up: bool) -> f32 {
    const STEPS: [f32; 10] = [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0];
    let steps = (1..=5).flat_map(|n| STEPS.map(|s| s * 10f32.powi(n)));
    let next = if up {
        steps.filter(|&s| s > gain * 1.001).reduce(f32::min)
    } else {
        steps.filter(|&s| s < gain / 1.001).reduce(f32::max)
    };
    next.unwrap_or(gain).clamp(*GAINS.start(), *GAINS.end())
}

/// A value for each channel; channels past the end have the default.
fn per_channel(values: &[f32], channel: usize, default: f32) -> f32 {
    values.get(channel).copied().unwrap_or(default)
}

fn set_per_channel(values: &mut Vec<f32>, channel: usize, value: f32, default: f32) {
    if values.len() <= channel {
        values.resize(channel + 1, default);
    }
    values[channel] = value;
}

/// The range of some channels' values, for a summary.
fn spread(values: &[f32], default: f32, show: impl Fn(f32) -> String) -> String {
    let (low, high) = values.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), &v| (l.min(v), h.max(v)));
    match values.len() {
        0 => format!("{} (all)", show(default)),
        _ if low == high => show(low),
        _ => format!("{}–{}", show(low), show(high)),
    }
}

/// Each channel's spike detection threshold: µV past zero, the way the spikes go (see `Polarity`).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Thresholds(pub Vec<f32>);

impl Thresholds {
    pub fn get(&self, channel: usize) -> f32 {
        per_channel(&self.0, channel, DEFAULT_THRESHOLD_UV)
    }

    pub fn set(&mut self, channel: usize, uv: f32) {
        set_per_channel(&mut self.0, channel, uv.max(0.1), DEFAULT_THRESHOLD_UV);
    }
}

impl EdgeType for Thresholds {
    const INFO: EdgeInfo = EdgeInfo {
        name: "thresholds",
        colour: Color32::from_rgb(245, 215, 80),
        description: "Each channel's spike detection threshold, in µV past zero, the way the spikes go.",
    };

    fn summary(&self) -> String {
        spread(&self.0, DEFAULT_THRESHOLD_UV, |t| format!("{t:.0} µV"))
    }
}

/// Each channel's gain for spikes: their waveforms are recorded, and viewed, over ±`range_uv(gain)`.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Gains(pub Vec<f32>);

impl Gains {
    pub fn get(&self, channel: usize) -> f32 {
        per_channel(&self.0, channel, DEFAULT_GAIN)
    }

    pub fn set(&mut self, channel: usize, gain: f32) {
        set_per_channel(&mut self.0, channel, gain.clamp(*GAINS.start(), *GAINS.end()), DEFAULT_GAIN);
    }
}

impl EdgeType for Gains {
    const INFO: EdgeInfo = EdgeInfo {
        name: "gains",
        colour: Color32::from_rgb(110, 215, 150),
        description: "Each channel's gain for spikes, as in the LTX format: the range their waveforms are recorded, \
                      and viewed, over.",
    };

    fn summary(&self) -> String {
        spread(&self.0, DEFAULT_GAIN, |g| format!("×{g:.0}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::EdgeType;

    #[test]
    fn summary() {
        let spikes = Spikes {
            channels: 64,
            sample_rate: 30000.0,
            group_size: 4,
            pre: 8,
            post: 32,
            polarity: Polarity::Negative,
        };
        assert_eq!(spikes.summary(), "16 tetrodes · 30 kHz");
        assert_eq!(spikes.group_channels(15), 60..64);
    }

    #[test]
    fn gains_step_and_default() {
        assert_eq!(gain_step(6000.0, true), 8000.0);
        assert_eq!(gain_step(6000.0, false), 5000.0);
        assert_eq!(gain_step(7000.0, false), 6000.0, "from between steps, to the next");
        assert_eq!(range_uv(DEFAULT_GAIN), 250.0);
        let mut gains = Gains::default();
        gains.set(2, 3000.0);
        assert_eq!(gains.0, vec![DEFAULT_GAIN, DEFAULT_GAIN, 3000.0]);
        assert_eq!((gains.get(2), gains.get(9)), (3000.0, DEFAULT_GAIN));
        assert_eq!(gains.summary(), "×3000–×6000");
    }
}
