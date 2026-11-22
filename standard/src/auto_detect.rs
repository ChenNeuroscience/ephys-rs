//! Choosing settings from the data ("auto detect"). The setting to choose (gains, or thresholds) has its auto detect's
//! own settings in `detect`: how long it samples the data for (`seconds`), and when it was started (`started`), while
//! it's going. `started` is the node's transient state, not saved: the node starts it from its window, or when it's
//! sent `Detect` (holding a `DetectRequest`, from a viewer, through a ValCmd edge), and clears it when it's stopped, or
//! told to clear its transient state (see `ephys_rs::engine::Node::clear_transient_state`). While it's started, the node runs, with room to
//! sample into that it made as it prepared, which its passes sample the data into for that long, then send the node
//! the setting they found (`PassContext::cmd_self`), which clears `started`. Whatever can see the setting (the node's
//! window, or a viewer's, through the value the node publishes) can tell how far along it is from those two
//! (`estimated_progress`). The Spike Gain, Continuous Gain and Spike Detector have one; these are the pieces they
//! share.

use std::time::Instant;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::Edges;
use ephys_rs::engine::UiContext;

/// A viewer has a node choose a setting from the data (see the module's doc), or stop, by sending it this, as the
/// `Detect` command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetectRequest {
    Start,
    Stop,
}

/// A node that can detect its setting from the data (see the module's docs) is asked to start or stop with this
/// command.
#[derive(Debug)]
pub struct Detect(pub DetectRequest);

impl DetectRequest {
    /// What the request does to an auto detect started at `started`, if it is.
    pub fn apply(self, started: &mut Option<Instant>) {
        *started = match self {
            DetectRequest::Start => Some(Instant::now()),
            DetectRequest::Stop => None,
        };
    }
}

/// Whether a setting's auto detect (`started`, when it was started) is another than in the snapshot before (`before`,
/// if the node was in it): started, started again, or cleared (it's done, or stopped). If not, what the node made for
/// it still stands.
pub fn detect_changed(started: Option<Instant>, before: Option<Option<Instant>>) -> bool {
    before != Some(started)
}

/// How long the passes can sample for, in seconds.
pub const SAMPLING: std::ops::RangeInclusive<f32> = 4.0..=60.0;

/// How finely values are binned: the bits of a float's mantissa kept, so 32 bins to each doubling, each about 3% wide.
const MANTISSA_BITS: u32 = 5;
/// Values below this are all in the first bin (2^-3, so 0.125 µV).
const SMALLEST_BITS: u32 = (127 - 3) << 23;
/// How many doublings up from there it goes, to about 130,000 µV.
const DOUBLINGS: usize = 20;
const BINS: usize = DOUBLINGS << MANTISSA_BITS;
/// The bits of a float below those kept.
const SHIFT: u32 = 23 - MANTISSA_BITS;

/// What's come in, as a histogram with bins of about the same relative width (a float's own bits are its bin), either
/// side of zero. It counts every value in little space and time, whatever order they come in, so it can say what's in a
/// distribution's tail to within a bin: about 1.5% of the value.
#[derive(Clone)]
pub struct Distribution {
    positive: Vec<u32>,
    negative: Vec<u32>,
    count: u64,
    sum: f64,
}

impl Default for Distribution {
    fn default() -> Self {
        Distribution { positive: vec![0; BINS], negative: vec![0; BINS], count: 0, sum: 0.0 }
    }
}

impl Distribution {
    /// Back to having had nothing, keeping its room.
    pub fn clear(&mut self) {
        self.positive.fill(0);
        self.negative.fill(0);
        (self.count, self.sum) = (0, 0.0);
    }

    pub fn add(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        let bin = ((value.abs().to_bits().saturating_sub(SMALLEST_BITS) >> SHIFT) as usize).min(BINS - 1);
        let bins = if value < 0.0 { &mut self.negative } else { &mut self.positive };
        bins[bin] += 1;
        self.count += 1;
        self.sum += value as f64;
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn mean(&self) -> f32 {
        if self.count == 0 { 0.0 } else { (self.sum / self.count as f64) as f32 }
    }

    /// The values there are, each bin's middle with how many are in it, from the lowest.
    fn bins(&self) -> impl Iterator<Item = (f32, u32)> + '_ {
        let middle = |bin: usize| f32::from_bits(SMALLEST_BITS + ((bin as u32) << SHIFT) + (1 << (SHIFT - 1)));
        let below = (0..BINS).rev().map(move |bin| (-middle(bin), self.negative[bin]));
        let above = (0..BINS).map(move |bin| (middle(bin), self.positive[bin]));
        below.chain(above).filter(|(_, count)| *count > 0)
    }

    /// The first of `bins`' values with `percent` of them at or below it.
    fn percentile(&self, percent: f32, bins: impl Iterator<Item = (f32, u32)>) -> f32 {
        let target = (self.count as f64 * percent as f64 / 100.0).ceil() as u64;
        let mut seen = 0;
        let mut last = 0.0;
        for (value, count) in bins {
            seen += count as u64;
            last = value;
            if seen >= target {
                break;
            }
        }
        last
    }

    /// What `percent` of the values are at or below.
    pub fn signed_percentile(&self, percent: f32) -> f32 {
        self.percentile(percent, self.bins())
    }

    /// What `percent` of the values are at or below, ignoring their sign.
    fn unsigned_percentile(&self, percent: f32) -> f32 {
        let mut values: Vec<(f32, u32)> = self.bins().map(|(value, count)| (value.abs(), count)).collect();
        values.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        self.percentile(percent, values.into_iter())
    }
}

/// A level chosen from the data, from a percentile of it and a multiple: a spike detector's threshold (`either_side`),
/// or a continuous gain's range (`either_way`).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PercentMultiple {
    pub percent: f32,
    pub multiple: f32,
}

impl PercentMultiple {
    /// The level, given data either side of zero: its percentile on each side (above zero, and as far below it), the
    /// further from zero of the two, times the multiple (as thresholds are, from zero either way).
    pub fn either_side(self, data: &Distribution) -> f32 {
        let above = data.signed_percentile(self.percent);
        let below = -data.signed_percentile(100.0 - self.percent);
        above.max(below) * self.multiple
    }

    /// The level, given data that can go either way: its mean, plus a multiple of a percentile's distance from it, both
    /// ignoring sign.
    pub fn either_way(self, data: &Distribution) -> f32 {
        let mean = data.mean().abs();
        mean + (data.unsigned_percentile(self.percent) - mean) * self.multiple
    }

    /// Its two fields, for a row of a settings grid.
    pub fn ui(&mut self, ui: &mut egui::Ui, percent_tip: &str, multiple_tip: &str) -> egui::Response {
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.percent).range(50.0..=100.0).speed(0.1).suffix(" %"))
                .on_hover_text(percent_tip);
            ui.label("factor");
            let factor = egui::DragValue::new(&mut self.multiple).range(0.1..=20.0).speed(0.05).prefix("×");
            ui.add(factor).on_hover_text(multiple_tip);
        })
        .response
    }
}

/// How far along detecting a setting is, from 0 to 1, from when it was started (`started`), and how many seconds it
/// samples for (`seconds`). The node doesn't say, so this is near enough: its passes start sampling once the snapshot
/// starting it takes over, and the Spike Gain starts again if the thresholds change meanwhile.
pub fn estimated_progress(started: Instant, seconds: f32) -> f32 {
    (started.elapsed().as_secs_f32() / seconds.clamp(*SAMPLING.start(), *SAMPLING.end())).min(1.0)
}

/// Auto detect, for a setting that samples for `seconds`, started at `started`, if it is (the node's own, or one a
/// viewer reads): a button to start it, while acquiring (it needs data) and not recording, and while it's going, how
/// far along it is, and a button to stop it. For the node's window, and a viewer's. This returns what was clicked: the
/// node's window applies it (`DetectRequest::apply`), and a viewer sends it, as the `Detect` command.
pub fn auto_detect_ui(
    ui: &mut egui::Ui,
    context: &UiContext<Edges>,
    started: Option<Instant>,
    seconds: f32,
    tip: &str,
) -> Option<DetectRequest> {
    ui.horizontal(|ui| {
        if let Some(at) = started {
            let stop = ui.button("Stop").on_hover_text("Stop detecting, leaving things as they were").clicked();
            let progress = estimated_progress(at, seconds);
            ui.add(egui::ProgressBar::new(progress).desired_width(80.0).show_percentage());
            return stop.then_some(DetectRequest::Stop);
        }
        let can = context.running && !context.locked;
        let why = if context.locked { "Not while recording" } else { "Only while acquiring: it needs data" };
        let start = ui
            .add_enabled(can, egui::Button::new("Auto detect"))
            .on_hover_text(tip)
            .on_disabled_hover_text(why)
            .clicked();
        start.then_some(DetectRequest::Start)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(values: impl Iterator<Item = i32>) -> Distribution {
        let mut data = Distribution::default();
        values.for_each(|value| data.add(value as f32));
        data
    }

    fn near(value: f32, expected: f32, within: f32) {
        assert!((value - expected).abs() <= within, "{value}, not {expected}");
    }

    /// A threshold is from zero, either side of it, whichever's further out: the mean doesn't count.
    #[test]
    fn a_level_either_side_is_the_further_percentile_from_zero_times_the_multiple() {
        let level = PercentMultiple { percent: 98.0, multiple: 2.0 };
        // All above zero: 98% are at or below 980, so it's twice that.
        let data = of(1..=1000);
        near(data.mean(), 500.5, 0.001);
        near(level.either_side(&data), 2.0 * 980.0, 30.0);
        // Below zero, further out than above: 2% are at or below -1960, which is further from zero than 98% are at
        // or below (980).
        let data = of((-2000..=-1).chain(1..=1000));
        near(level.either_side(&data), 2.0 * 1940.0, 60.0);
    }

    #[test]
    fn data_can_be_taken_either_way() {
        let level = PercentMultiple { percent: 98.0, multiple: 2.0 };
        let data = of(-1000..=1000);
        near(data.mean(), 0.0, 0.001);
        near(data.unsigned_percentile(98.0), 980.0, 15.0);
        near(level.either_way(&data), 1960.0, 30.0);

        // Below zero, it's as far from zero.
        let data = of((-1000..=-1).map(|value| value - 1000));
        near(data.mean(), -1500.5, 0.001);
        near(data.unsigned_percentile(98.0), 1980.0, 30.0);
        near(level.either_way(&data), 1500.5 + 2.0 * 479.5, 60.0);
    }

    #[test]
    fn one_way_it_is_the_percentile_that_counts() {
        // Mostly at 10, with a tail of 100s: 3% of them, so the 98th percentile's in it.
        let data = of((0..1000).map(|i| if i < 970 { 10 } else { 100 }));
        near(data.signed_percentile(50.0), 10.0, 0.5);
        near(data.signed_percentile(98.0), 100.0, 2.0);
    }

    #[test]
    fn with_nothing_to_go_on_a_level_is_zero() {
        let level = PercentMultiple { percent: 98.0, multiple: 2.0 };
        assert_eq!(level.either_side(&Distribution::default()), 0.0);
        assert_eq!(level.either_way(&Distribution::default()), 0.0);
    }

    #[test]
    fn every_value_counts_however_small_or_large() {
        let mut data = Distribution::default();
        [0.0, 1e-9, 1e9, f32::NAN].into_iter().for_each(|value| data.add(value));
        assert_eq!(data.count, 3, "all but what isn't a number");
    }
}
