//! Spikes: detected in groups of channels (tetrodes, say) of continuous data against each channel's threshold
//! (`spike_detector`), given each channel's gain, which they're recorded and viewed at (`spike_gain`), and viewed
//! (`spike_viewer`, where the thresholds and gains can be adjusted, and detected from the data).

pub mod spike_detector;
pub mod spike_gain;
pub mod spike_viewer;

use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

use super::per_pass;
use super::widgets::format_rate;
use ephys_rs::engine::{DataStreamType, EdgeInfo, EdgeType};

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
    /// A value as it's plotted: spikes go up, so negative-going ones are shown upside down.
    pub fn plotted(self, x: f32) -> f32 {
        if self == Polarity::Negative { -x } else { x }
    }

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
    /// Each channel's threshold and noise (a standard deviation), in µV, as the pass ended.
    pub thresholds: Vec<f32>,
    /// The gain each channel's waveforms have been multiplied by: 1 until a Spike Gain has (they're then amplified µV,
    /// see `Gains`).
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
            gains: vec![1.0; self.channels as usize],
            noise: vec![0.0; self.channels as usize],
            pass_samples: 0,
        }
    }

    fn bytes(buffer: &SpikesBuffer) -> usize {
        buffer.spikes.capacity() * size_of::<Spike>() + buffer.waveforms.capacity() * size_of::<f32>()
    }

    fn copy(from: &SpikesBuffer, to: &mut SpikesBuffer) {
        to.spikes.clone_from(&from.spikes);
        to.waveforms.clone_from(&from.waveforms);
        (to.group_size, to.samples, to.pass_samples) = (from.group_size, from.samples, from.pass_samples);
        to.thresholds.clone_from(&from.thresholds);
        to.gains.clone_from(&from.gains);
        to.noise.clone_from(&from.noise);
    }
}

/// What Write LTX's −128 to 127 cover, of amplified µV, until it's set: ±250 µV, as ephys-rs has always recorded. What
/// auto detect fills, to begin with, so a channel's chosen range is what's recorded.
pub const RECORDED_FULL_SCALE_UV: f32 = DEFAULT_RANGE_UV;
/// The sizes a gain can be, either way: a quarter to four times.
pub const GAIN_SIZES: std::ops::RangeInclusive<f32> = 0.25..=4.0;
/// A channel's range until it's set: µV either side of zero.
pub const DEFAULT_RANGE_UV: f32 = 250.0;
/// A channel's threshold until it's set, in µV.
pub const DEFAULT_THRESHOLD_UV: f32 = 50.0;
/// The ranges gains can give, in µV either side of zero.
pub const RANGES_UV: std::ops::RangeInclusive<f32> = 10.0..=30_000.0;

/// The next round number down (`smaller`) or up from `value`, in steps of 1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8 (×10ⁿ,
/// from 0.01 to 10⁷).
pub fn nice_step(value: f32, smaller: bool) -> f32 {
    const STEPS: [f32; 10] = [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0];
    let steps = (-2..=7).flat_map(|n| STEPS.map(|s| s * 10f32.powi(n)));
    let next = if smaller {
        steps.filter(|&s| s < value / 1.001).reduce(f32::max)
    } else {
        steps.filter(|&s| s > value * 1.001).reduce(f32::min)
    };
    next.unwrap_or(value)
}

/// The next range down (`smaller`) or up from `range_uv`, a round number (see `nice_step`) within `RANGES_UV`.
pub fn range_step(range_uv: f32, smaller: bool) -> f32 {
    nice_step(range_uv, smaller).clamp(*RANGES_UV.start(), *RANGES_UV.end())
}

/// The next threshold up or down from `uv`: on to the next round number, in steps of a hundredth to a tenth of it
/// (50 to 51, 100 to 110, 100 down to 99).
pub fn threshold_step(uv: f32, up: bool) -> f32 {
    // Going down from 100, it's the steps below 100 that count.
    let from = if up { uv } else { uv * 0.999 };
    let step = 10f32.powf(from.max(f32::MIN_POSITIVE).log10().floor() - 1.0);
    let steps = uv / step;
    let next = if up { (steps + 1e-3).floor() + 1.0 } else { (steps - 1e-3).ceil() - 1.0 };
    next * step
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

/// How spike gains are shared out: one for all the channels, one for each group (a tetrode's, say), or one each.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum GainScope {
    All,
    Group,
    #[default]
    Channel,
}

impl GainScope {
    pub const ALL: [GainScope; 3] = [GainScope::All, GainScope::Group, GainScope::Channel];

    /// "Each tetrode", say, for groups of `group_size`.
    pub fn name(self, group_size: usize) -> &'static str {
        match self {
            GainScope::All => "All channels together",
            GainScope::Group => match group_size {
                1 => "Each channel",
                2 => "Each stereotrode",
                4 => "Each tetrode",
                8 => "Each octrode",
                _ => "Each group",
            },
            GainScope::Channel => "Each channel",
        }
    }
}

/// Each channel's gain: what a Spike Gain or Continuous Gain multiplies its µV by, as an amplifier does (×2 doubles
/// them), into amplified µV: all a gain does. Choosing gains from the data, a channel's bigger values are made to fill
/// ±`fill_uv` (what's recorded: Write LTX's full scale, to begin with), so its range, what it fits, is
/// ±`fill_uv / gain` µV of what comes in. Negative turns them upside down. They're shared out as `scope` says: setting
/// one channel's sets its group's, or all.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Gains {
    /// What auto detect fills, and what a gain's range is of (see `range_uv`): nothing else reads it.
    pub fill_uv: f32,
    pub scope: GainScope,
    /// Each channel's gain (channels past the end have the default range's).
    pub gains: Vec<f32>,
}

impl Default for Gains {
    fn default() -> Self {
        Gains { fill_uv: RECORDED_FULL_SCALE_UV, scope: GainScope::default(), gains: Vec::new() }
    }
}

/// Channels, and how many to a group: what gains are shared out over.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Channels {
    pub count: usize,
    pub group_size: usize,
}

impl Gains {
    /// A channel's gain: ×1 until it's set.
    pub fn get(&self, channel: usize) -> f32 {
        per_channel(&self.gains, channel, 1.0)
    }

    /// A channel's range: µV either side of zero that its gain makes ±`fill_uv` (whichever its sign).
    pub fn range_uv(&self, channel: usize) -> f32 {
        self.fill_uv / self.get(channel).abs().max(f32::MIN_POSITIVE)
    }

    /// Whether a channel's gain is negative: its waveforms turned upside down.
    pub fn inverted(&self, channel: usize) -> bool {
        self.get(channel) < 0.0
    }

    /// Turns a channel's waveforms upside down, or back, and those of the channels sharing its gain: its range stays.
    pub fn invert(&mut self, channel: usize, channels: Channels) {
        self.set(channel, -self.get(channel), channels);
    }

    /// The channels sharing a gain, unit by unit, as the scope has it.
    pub fn units(&self, channels: Channels) -> Vec<std::ops::Range<usize>> {
        let size = match self.scope {
            GainScope::All => channels.count.max(1),
            GainScope::Group => channels.group_size.max(1),
            GainScope::Channel => 1,
        };
        (0..channels.count).step_by(size).map(|start| start..(start + size).min(channels.count)).collect()
    }

    /// Sets a channel's range (so its gain, keeping its sign), and those of the channels sharing its gain.
    pub fn set_range(&mut self, channel: usize, range_uv: f32, channels: Channels) {
        let sign = if self.inverted(channel) { -1.0 } else { 1.0 };
        self.set(channel, sign * self.fill_uv / range_uv.max(f32::MIN_POSITIVE), channels);
    }

    /// Sets a channel's gain, and those of the channels sharing it, its size within `GAIN_SIZES`. Negative turns their
    /// waveforms upside down.
    pub fn set(&mut self, channel: usize, gain: f32, channels: Channels) {
        let gain = gain.signum() * gain.abs().clamp(*GAIN_SIZES.start(), *GAIN_SIZES.end());
        let unit = self.units(channels).into_iter().find(|u| u.contains(&channel)).unwrap_or(channel..channel + 1);
        for channel in unit {
            set_per_channel(&mut self.gains, channel, gain, 1.0);
        }
    }

    /// Sets each channel's range, those sharing a gain taking the widest of theirs (so none's spikes clip).
    pub fn set_ranges(&mut self, ranges: &[f32], channels: Channels) {
        for unit in self.units(channels) {
            let widest = ranges[unit.clone()].iter().copied().fold(0.0, f32::max);
            if widest > 0.0 {
                self.set_range(unit.start, widest, channels);
            }
        }
    }

    /// Changes how the gains are shared out, those now sharing one taking the widest range of theirs.
    pub fn set_scope(&mut self, scope: GainScope, channels: Channels) {
        let ranges: Vec<f32> = (0..channels.count).map(|c| self.range_uv(c)).collect();
        self.scope = scope;
        self.set_ranges(&ranges, channels);
    }
}

/// A value to two decimals, without those it doesn't need: 4, 8.33, 0.5.
pub fn trimmed(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A field for a gain: what the µV are multiplied by, its size within `GAIN_SIZES`, either way if `negative` (upside
/// down).
pub fn gain_field(ui: &mut egui::Ui, gain: &mut f32, negative: bool) -> egui::Response {
    let (smallest, biggest) = (*GAIN_SIZES.start(), *GAIN_SIZES.end());
    let low = if negative { -biggest } else { smallest };
    let speed = gain.abs() * 0.01;
    let field =
        egui::DragValue::new(gain).range(low..=biggest).speed(speed).custom_formatter(|v, _| trimmed(v)).prefix("×");
    let tip = "What the µV are multiplied by, as an amplifier does (×2 doubles them): ×¼ to ×4";
    let tip =
        if negative { format!("{tip}. Negative turns them upside down, fitting the same range") } else { tip.into() };
    ui.add(field).on_hover_text(tip)
}

/// A field for what auto detect fills.
pub fn fill_field(ui: &mut egui::Ui, gains: &mut Gains) -> egui::Response {
    let speed = gains.fill_uv * 0.01;
    let field = egui::DragValue::new(&mut gains.fill_uv)
        .range(1.0..=1_000_000.0)
        .speed(speed)
        .custom_formatter(|v, _| trimmed(v))
        .prefix("±")
        .suffix(" µV");
    ui.add(field).on_hover_text(
        "What each channel's chosen range is made to fill, of the amplified µV: what's recorded, so Write LTX's full \
         scale (±250 µV, to begin with). A gain fits this ÷ the gain of what comes in.",
    )
}

impl EdgeType for Gains {
    const INFO: EdgeInfo = EdgeInfo {
        name: "gains",
        colour: Color32::from_rgb(110, 215, 150),
        description: "Each channel's gain for spikes: what their waveforms' values (µV) are multiplied by, for \
                      whatever takes them, so the range they fit within.",
    };

    /// The channels' ranges, which is what the gains mean to people.
    fn summary(&self) -> String {
        let ranges: Vec<f32> = (0..self.gains.len()).map(|c| self.range_uv(c)).collect();
        spread(&ranges, DEFAULT_RANGE_UV, |r| format!("±{r:.0} µV"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephys_rs::engine::EdgeType;

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
    fn thresholds_step_to_round_numbers() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let steps = [(50.0, true, 51.0), (50.0, false, 49.0), (99.0, true, 100.0), (100.0, true, 110.0)];
        let steps = steps.into_iter().chain([(100.0, false, 99.0), (50.3, true, 51.0), (50.3, false, 50.0)]);
        for (from, up, to) in steps.chain([(5.0, true, 5.1), (1000.0, true, 1100.0)]) {
            assert!(near(threshold_step(from, up), to), "{from}, up {up}: {}", threshold_step(from, up));
        }
    }

    #[test]
    fn gains_are_shared_out_by_scope() {
        assert_eq!(range_step(250.0, true), 200.0);
        assert_eq!(range_step(250.0, false), 300.0);
        assert_eq!(range_step(220.0, true), 200.0, "from between steps, to the next");
        let tetrodes = Channels { count: 8, group_size: 4 };
        let mut gains = Gains::default();
        assert_eq!(gains.range_uv(0), DEFAULT_RANGE_UV);
        gains.set_range(2, 500.0, tetrodes);
        assert_eq!(gains.get(2), RECORDED_FULL_SCALE_UV / 500.0, "how much it amplifies");
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(gains.range_uv(1), DEFAULT_RANGE_UV) && near(gains.range_uv(2), 500.0), "each channel its own");
        // A tetrode each: the widest of each's.
        gains.set_scope(GainScope::Group, tetrodes);
        assert!((0..4).all(|c| near(gains.range_uv(c), 500.0)));
        assert!((4..8).all(|c| near(gains.range_uv(c), DEFAULT_RANGE_UV)));
        gains.set_range(5, 100.0, tetrodes);
        assert!((4..8).all(|c| near(gains.range_uv(c), 100.0)), "the whole tetrode");
        // All together.
        gains.set_scope(GainScope::All, tetrodes);
        assert!((0..8).all(|c| near(gains.range_uv(c), 500.0)));
    }

    #[test]
    fn a_negative_gain_turns_spikes_upside_down_and_keeps_its_range() {
        let tetrodes = Channels { count: 8, group_size: 4 };
        let mut gains = Gains { scope: GainScope::Group, ..Default::default() };
        gains.set_range(0, 500.0, tetrodes);
        gains.invert(1, tetrodes);
        assert!((0..4).all(|c| gains.inverted(c)), "the whole tetrode, which shares its gain");
        assert!(!gains.inverted(4));
        assert_eq!(gains.get(0), -RECORDED_FULL_SCALE_UV / 500.0);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(gains.range_uv(0), 500.0), "the range is the gain's size");
        // Setting the range, by hand or from the data, keeps it upside down.
        gains.set_range(2, 100.0, tetrodes);
        gains.set_ranges(&[300.0; 8], tetrodes);
        assert!(gains.inverted(0) && near(gains.range_uv(0), 300.0));
        gains.set(0, -1e-30, tetrodes);
        assert_eq!(gains.get(0), -0.25, "never zero: a quarter, at least");
        gains.set(0, 10.0, tetrodes);
        assert_eq!(gains.get(0), 4.0, "and four times, at most");
        gains.set(0, -2.5, tetrodes);
        gains.invert(0, tetrodes);
        assert_eq!(gains.get(3), 2.5, "and back");
    }
}
