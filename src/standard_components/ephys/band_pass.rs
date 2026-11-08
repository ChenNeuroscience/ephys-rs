//! The Band-pass Filter: a Butterworth band-pass on every channel, as Open Ephys's filter node has, designed and
//! run as second-order sections by sci-rs (a port of SciPy's `butter` and `sosfilt`).

use crate::core::engine::{BufferUsage, TreeShake};
use eframe::egui;
use sci_rs::signal::filter::design::{DigitalFilter, FilterBandType, FilterOutputType, Sos, butter_dyn};
use sci_rs::signal::filter::sosfilt_item;
use serde::{Deserialize, Serialize};

use crate::Buffer;
use crate::core::engine::{Category, Input, Node, NodeInfo, Output, Pass, UiContext};
use crate::standard_components::ephys::ContinuousBuffer;
use crate::standard_components::widgets::format_rate;
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
use crate::{DataStream, Edges, Kind};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandPassFilter {
    pub low_hz: f32,
    pub high_hz: f32,
    /// The Butterworth design's order: each step steepens the roll-off (by 6 dB an octave each side).
    pub order: u32,
}

impl Default for BandPassFilter {
    fn default() -> Self {
        BandPassFilter { low_hz: 300.0, high_hz: 6000.0, order: 2 }
    }
}

impl Node<Edges> for BandPassFilter {
    const INFO: NodeInfo = NodeInfo {
        name: "Band-pass Filter",
        category: Category::Processing,
        description: "Keeps frequencies between the low and high cut-offs, on every channel (a Butterworth \
                      band-pass).",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    type State = Filters;

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Filters {
        let Some(DataStream::Continuous(stream)) = inputs[0] else { return Filters::default() };
        let mut filters = Filters { channels: vec![Vec::new(); stream.channels as usize], ..Default::default() };
        filters.tune(self, stream.sample_rate);
        filters
    }

    /// Its output's made in place, from its input.
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage {
        BufferUsage { in_place: vec![Some(0); outputs], ..BufferUsage::none(inputs, outputs) }
    }

    fn process(&self, pass: &mut Pass<Edges>, filters: &mut Filters) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        if filters.channels.len() < output.channels {
            return;
        }
        // The cut-offs can change while running: keep the history, with new coefficients.
        if filters.design != (self.low_hz, self.high_hz, self.order) {
            let rate = filters.rate;
            filters.tune(self, rate);
        }
        filters.run(output, input);
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        vec![Output { name: "filtered", kind: Kind::Continuous, stream: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if self.low_hz >= self.high_hz {
            warnings.push("The low cut-off must be below the high cut-off".into());
        }
        if let Some(DataStream::Continuous(c)) = inputs[0]
            && self.high_hz >= c.sample_rate / 2.0
        {
            warnings.push(format!(
                "The high cut-off must be below {} (half the sample rate)",
                format_rate(c.sample_rate / 2.0)
            ));
        }
        warnings
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Low cut-off", |ui| {
                ui.add(egui::DragValue::new(&mut self.low_hz).range(0.1..=20000.0).speed(1.0).suffix(" Hz"))
            });
            setting(ui, "High cut-off", |ui| {
                ui.add(egui::DragValue::new(&mut self.high_hz).range(1.0..=20000.0).speed(10.0).suffix(" Hz"))
            });
            setting(ui, "Order", |ui| ui.add(egui::DragValue::new(&mut self.order).range(1..=8)));
        });
        input_summary(ui, context.inputs);
    }
}

/// Each channel's filter (its sections, with their history carried between passes), and what it's designed for.
#[derive(Default)]
pub struct Filters {
    channels: Vec<Vec<Sos<f64>>>,
    design: (f32, f32, u32),
    rate: f32,
}

impl crate::core::engine::PassState for Filters {
    const WHY: &'static str = "Each channel's filter history (its sections' state): each pass's filtering \
                               carries on where the last pass's left off, so there's no step where passes \
                               join.";
}

/// Channels filtered together: each's sections wait on each other, sample by sample, but different channels' don't,
/// so the processor works on several at once rather than waiting.
const LANES: usize = 8;

impl Filters {
    /// Filters `input` into `output`, or (when it's made in place, and `input` is `None`) `output` itself.
    fn run(&mut self, output: &mut ContinuousBuffer, input: Option<&ContinuousBuffer>) {
        if input.is_some() {
            output.transform(input, |_, x| x);
        }
        let (len, capacity) = (output.len, output.capacity);
        for (data, sections) in output.data.chunks_mut(LANES * capacity).zip(self.channels.chunks_mut(LANES)) {
            for t in 0..len {
                for (lane, sections) in sections.iter_mut().enumerate() {
                    let x = &mut data[lane * capacity + t];
                    *x = sosfilt_item(*x as f64, sections) as f32;
                }
            }
        }
    }

    /// Designs the filter for `settings` at `rate`. Every channel takes the new coefficients, keeping its
    /// history if it has as many sections as before.
    fn tune(&mut self, settings: &BandPassFilter, rate: f32) {
        let nyquist = rate as f64 / 2.0;
        let high = (settings.high_hz as f64).clamp(1e-3, nyquist * 0.999);
        let low = (settings.low_hz as f64).clamp(1e-3, high * 0.999);
        let order = settings.order.clamp(1, 8) as usize;
        let design = butter_dyn(
            order,
            vec![low, high],
            Some(FilterBandType::Bandpass),
            Some(false),
            Some(FilterOutputType::Sos),
            Some(rate as f64),
        );
        let DigitalFilter::Sos(design) = design else { unreachable!("asked for sections") };
        for sections in &mut self.channels {
            if sections.len() == design.sos.len() {
                for (section, new) in sections.iter_mut().zip(&design.sos) {
                    (section.b, section.a) = (new.b, new.a);
                }
            } else {
                *sections = design.sos.clone();
            }
        }
        (self.design, self.rate) = ((settings.low_hz, settings.high_hz, settings.order), rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain at `hz` of the filter, from a sine's amplitude once it's settled.
    fn gain(hz: f64) -> f64 {
        let settings = BandPassFilter::default();
        let mut filters = Filters { channels: vec![Vec::new()], ..Default::default() };
        filters.tune(&settings, 30000.0);
        let mut peak: f64 = 0.0;
        for n in 0..60000 {
            let x = (std::f64::consts::TAU * hz * n as f64 / 30000.0).sin();
            let y = sosfilt_item(x, &mut filters.channels[0]);
            if n > 30000 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    /// A pass's worth of 64 channels, several ways, for timing: `cargo test --release filtering_speed -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn filtering_speed() {
        let (channels, len) = (64, 600);
        let settings = BandPassFilter::default();
        let mut filters = Filters { channels: vec![Vec::new(); channels], ..Default::default() };
        filters.tune(&settings, 30000.0);
        let input = ContinuousBuffer {
            channels,
            capacity: 750,
            len,
            data: (0..channels * 750).map(|i| (i as f32 * 0.37).sin() * 100.0).collect(),
        };
        let mut output = ContinuousBuffer { channels, capacity: 750, len: 0, data: vec![0.0; channels * 750] };
        let passes = 500;
        let start = std::time::Instant::now();
        for _ in 0..passes {
            output.transform(Some(&input), |c, x| sosfilt_item(x as f64, &mut filters.channels[c]) as f32);
        }
        let one_by_one = start.elapsed().as_secs_f64() / passes as f64 * 1e6;
        let start = std::time::Instant::now();
        for _ in 0..passes {
            filters.run(&mut output, Some(&input));
        }
        let together = start.elapsed().as_secs_f64() / passes as f64 * 1e6;
        eprintln!("a channel at a time {one_by_one:.0} us a pass; {LANES} at a time {together:.0} us");
    }

    #[test]
    fn filters_channels_together_as_one_by_one() {
        let (channels, len) = (7, 300);
        let settings = BandPassFilter::default();
        let mut one = Filters { channels: vec![Vec::new(); channels], ..Default::default() };
        one.tune(&settings, 30000.0);
        let mut together = Filters { channels: vec![Vec::new(); channels], ..Default::default() };
        together.tune(&settings, 30000.0);
        let input = ContinuousBuffer {
            channels,
            capacity: 400,
            len,
            data: (0..channels * 400).map(|i| (i as f32 * 0.37).sin() * 100.0).collect(),
        };
        let mut expected = ContinuousBuffer { channels, capacity: 400, len: 0, data: vec![0.0; channels * 400] };
        let mut got = ContinuousBuffer { channels, capacity: 400, len: 0, data: vec![0.0; channels * 400] };
        for _ in 0..3 {
            expected.transform(Some(&input), |c, x| sosfilt_item(x as f64, &mut one.channels[c]) as f32);
            together.run(&mut got, Some(&input));
            for c in 0..channels {
                assert_eq!(expected.channel(c), got.channel(c), "channel {c}");
            }
        }
        // In place, too.
        let mut in_place = ContinuousBuffer { channels, capacity: 400, len, data: input.data.clone() };
        together.run(&mut in_place, None);
        expected.transform(Some(&input), |c, x| sosfilt_item(x as f64, &mut one.channels[c]) as f32);
        assert_eq!(expected.channel(3), in_place.channel(3));
    }

    #[test]
    fn passes_the_band_and_stops_the_rest() {
        assert!((gain(1500.0) - 1.0).abs() < 0.02, "{}", gain(1500.0));
        assert!((gain(300.0) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.03, "-3 dB at the cut-off");
        assert!(gain(10.0) < 0.01, "{}", gain(10.0));
        assert!(gain(14000.0) < 0.1, "{}", gain(14000.0));
    }
}
