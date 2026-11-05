//! Choosing settings from the data ("auto detect"): a node's trigger (see `Node::triggers`), which its passes take,
//! sample the data for a while, then propose settings from. The Spike Gain, Continuous Gain and Spike Detector have one,
//! fired from their windows or a viewer's; these are the pieces they share.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::Edges;
use crate::core::engine::Context;

/// The trigger.
pub const AUTO_DETECT: &str = "auto detect";
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

/// What's come in, as a histogram with bins of about the same relative width (a float's own bits are its bin), either side
/// of zero. It counts every value in little space and time, whatever order they come in, so it can say what's in a
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

    /// How far above the mean `percent` of the values are at or below.
    pub fn above_mean(&self, percent: f32) -> f32 {
        self.percentile(percent, self.bins()) - self.mean()
    }

    /// How far from the mean, either way, `percent` of the values are within.
    pub fn from_mean(&self, percent: f32) -> f32 {
        let mean = self.mean();
        let mut distances: Vec<(f32, u32)> = self.bins().map(|(value, count)| ((value - mean).abs(), count)).collect();
        distances.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        self.percentile(percent, distances.into_iter())
    }
}

/// A level chosen from the data: its mean, plus the distance from it that takes in so much of the data, multiplied.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PercentMultiple {
    pub percent: f32,
    pub multiple: f32,
}

impl PercentMultiple {
    /// The level, given data that goes one way: `percent` of it is at or below that distance above the mean.
    pub fn above(self, data: &Distribution) -> f32 {
        data.mean() + data.above_mean(self.percent) * self.multiple
    }

    /// The level, given data that can go either way: `percent` of it is within that distance of the mean.
    pub fn either_way(self, data: &Distribution) -> f32 {
        data.mean().abs() + data.from_mean(self.percent) * self.multiple
    }

    /// "98% × 2.5".
    pub fn summary(self) -> String {
        format!("{}% × {}", self.percent, self.multiple)
    }

    /// Its two fields, for a row of a settings grid.
    pub fn ui(&mut self, ui: &mut egui::Ui, tip: &str) -> egui::Response {
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.percent).range(50.0..=100.0).speed(0.1).suffix(" %"))
                .on_hover_text(tip);
            ui.label("×");
            ui.add(egui::DragValue::new(&mut self.multiple).range(0.1..=20.0).speed(0.05)).on_hover_text(tip);
        })
        .response
    }
}

/// A button to fire the auto-detect trigger (of the node's own settings, or of the setting `control` connects to),
/// and how it's going: for this node's window, and a viewer's.
pub fn auto_detect_ui(ui: &mut egui::Ui, context: &mut Context<Edges>, control: Option<usize>, tip: &str) {
    ui.horizontal(|ui| {
        // While detecting, it can be stopped.
        if let Some(progress) = context.trigger_progress(control, AUTO_DETECT) {
            if ui.button("Stop").on_hover_text("Stop detecting, leaving things as they were").clicked() {
                context.cancel_trigger(control, AUTO_DETECT);
            }
            ui.add(egui::ProgressBar::new(progress).desired_width(80.0).show_percentage());
            return;
        }
        let can = context.can_trigger(control, AUTO_DETECT) && !context.locked;
        let why = if context.locked { "Not while recording" } else { "Only while acquiring: it needs data" };
        if ui
            .add_enabled(can, egui::Button::new("Auto detect"))
            .on_hover_text(tip)
            .on_disabled_hover_text(why)
            .clicked()
        {
            context.trigger(control, AUTO_DETECT);
        }
    });
}

/// Whether the node's auto-detect is going (its settings for it can't change meanwhile).
pub fn detecting(context: &Context<Edges>) -> bool {
    context.trigger_progress(None, AUTO_DETECT).is_some()
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

    #[test]
    fn a_level_is_the_mean_plus_the_distance_taking_in_so_much_of_the_data_multiplied() {
        let level = PercentMultiple { percent: 98.0, multiple: 2.0 };
        // A mean of 500.5: 98% are at or below 980, which is 479.5 above it.
        let data = of(1..=1000);
        near(data.mean(), 500.5, 0.001);
        near(data.above_mean(98.0), 479.5, 15.0);
        near(level.above(&data), 500.5 + 2.0 * 479.5, 30.0);
    }

    #[test]
    fn data_can_be_taken_either_way() {
        let level = PercentMultiple { percent: 98.0, multiple: 2.0 };
        let data = of(-1000..=1000);
        near(data.mean(), 0.0, 0.001);
        near(data.from_mean(98.0), 980.0, 15.0);
        near(level.either_way(&data), 1960.0, 30.0);

        // Below zero, it's as far from zero.
        let data = of((-1000..=-1).map(|value| value - 1000));
        near(data.mean(), -1500.5, 0.001);
        near(data.from_mean(98.0), 490.0, 30.0);
        near(level.either_way(&data), 1500.5 + 980.0, 60.0);
    }

    #[test]
    fn what_goes_above_the_mean_is_what_counts_one_way() {
        // Mostly at 10, with a tail of 100s: 3% of them, so the 98th percentile's in it.
        let data = of((0..1000).map(|i| if i < 970 { 10 } else { 100 }));
        near(data.above_mean(50.0), 10.0 - 12.7, 0.5);
        near(data.above_mean(98.0), 100.0 - 12.7, 2.0);
    }

    #[test]
    fn with_nothing_to_go_on_a_level_is_zero() {
        let level = PercentMultiple { percent: 98.0, multiple: 2.0 };
        assert_eq!(level.above(&Distribution::default()), 0.0);
        assert_eq!(level.either_way(&Distribution::default()), 0.0);
    }

    #[test]
    fn every_value_counts_however_small_or_large() {
        let mut data = Distribution::default();
        [0.0, 1e-9, 1e9, f32::NAN].into_iter().for_each(|value| data.add(value));
        assert_eq!(data.count, 3, "all but what isn't a number");
    }
}
