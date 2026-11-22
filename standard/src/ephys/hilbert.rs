//! The Hilbert Transform makes the analytic signal of continuous data, on every channel: each sample becomes a complex
//! value whose real part is the sample itself and whose imaginary part is the signal's Hilbert transform. The analytic
//! signal's argument is the signal's phase and its magnitude is the signal's envelope, which is what a Phase Viewer
//! shows. Phase only means something for a narrow band, so the input is usually band-passed first (to theta, say).
//!
//! An offline tool (such as SciPy's `hilbert`) transforms a whole recording at once with an FFT. A pass only has a few
//! milliseconds of data, so this node uses a streaming FIR Hilbert transformer instead: the ideal Hilbert impulse
//! response (`2 / πk` at odd offsets `k`, zero at even ones), cut to `half` samples either side and shaped by a
//! Blackman window. The filter is accurate (gain within 0.1% of 1) from the **Lowest frequency** up to the same
//! distance below half the sample rate, if it's `1.5 × rate / lowest` samples long either side. The filter needs that
//! many samples after each one, so the output comes `half` samples late: the real part is the input, delayed to match.
//!
//! The filter's length grows with the sample rate, so the node is meant for data downsampled to about 1 kHz (where a
//! 4 Hz lowest frequency takes 375 samples either side, and a delay of 375 ms). The length is capped at `MAX_HALF`
//! samples either side, and the node warns when the cap raises the lowest frequency it's accurate at.

use eframe::egui;
use ephys_rs::engine::{Buffers, CommandContext, CommandSource, NoPorts, Prepare, PrepareContext, Prepared, TreeShake};
use serde::{Deserialize, Serialize};

use crate::standard::ephys::{ComplexContinuous, ComplexContinuousBuffer, Continuous, ContinuousBuffer};
use crate::standard::widgets::{format_rate, input_summary, require_input, setting, settings_grid};
use ephys_rs::engine::{Category, Node, NodeInfo, PassContext, UiContext};

ephys_rs::ports! {
    continuous: DataStreamIn<Continuous>,
    analytic: DataStreamOut<ComplexContinuous>,
}

/// The most samples the filter reaches either side of each sample, whatever the sample rate.
const MAX_HALF: usize = 4096;

/// How many samples either side of each sample the filter needs, per cycle of the lowest frequency, to be accurate at
/// that frequency (see the module's docs).
const SAMPLES_PER_LOWEST_CYCLE: f32 = 1.5;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HilbertTransform {
    /// The lowest frequency the transform is accurate at, in Hz. Lower means a longer filter and a longer delay.
    pub lowest_hz: f32,
}

impl Default for HilbertTransform {
    fn default() -> Self {
        HilbertTransform { lowest_hz: 4.0 }
    }
}

impl HilbertTransform {
    /// How many samples either side of each sample the filter reaches, at `rate`.
    fn half(&self, rate: f32) -> usize {
        let wanted = (SAMPLES_PER_LOWEST_CYCLE * rate / self.lowest_hz.max(0.01)).ceil();
        (wanted as usize).clamp(1, MAX_HALF)
    }

    /// The lowest frequency the transform is accurate at, at `rate`: the setting, unless the filter's length is capped.
    fn accurate_from(&self, rate: f32) -> f32 {
        SAMPLES_PER_LOWEST_CYCLE * rate / self.half(rate) as f32
    }
}

impl<E: PortEdges> Node<E> for HilbertTransform {
    type Ports = Ports;
    const INFO: NodeInfo = NodeInfo {
        name: "Hilbert Transform",
        category: Category::Processing,
        description: "The analytic signal of continuous data, on every channel: its phase and envelope, as complex \
                      values. Band-pass and downsample the data first.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], _recording: bool) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    type State = Transforming;
    type WorkerState = ();
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Transforming, (), Self> {
        if context.unchanged(self) {
            return Prepared::nothing().into();
        }
        let transforming = match context.data_stream_in::<DataStreamIn>().continuous {
            Some(stream) => Transforming::new(self.half(stream.sample_rate), stream, PASS_ROOM),
            None => Transforming::default(),
        };
        Prepared::shared(transforming).into()
    }

    fn process(
        &self,
        _pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        outputs: DataStreamOut<Buffers>,
        transforming: &mut Transforming,
        _: &mut (),
    ) {
        let output = outputs.analytic;
        output.len = 0;
        if let Some(input) = inputs.continuous {
            transforming.run(input, output);
        }
    }

    fn val_out(&self) -> NoPorts {
        NoPorts
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {}
    }

    fn output_streams(&self, inputs: &DataStreamIn) -> DataStreamOut {
        let analytic =
            inputs.continuous.map(|c| ComplexContinuous { channels: c.channels, sample_rate: c.sample_rate });
        DataStreamOut { analytic }
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let rate = context.data_stream_in::<DataStreamIn>().continuous.map(|c| c.sample_rate);
            settings_grid(ui, |ui| {
                setting(ui, "Lowest frequency", |ui| {
                    let field = egui::DragValue::new(&mut node.lowest_hz).range(0.5..=200.0).speed(0.1).suffix(" Hz");
                    ui.add(field)
                        .on_hover_text("The lowest frequency the transform is accurate at. Lower means a longer delay.")
                });
                if let Some(rate) = rate {
                    setting(ui, "Delay", |ui| {
                        let delay = node.half(rate) as f32 / rate;
                        ui.label(format!("{:.0} ms", delay * 1000.0)).on_hover_text(
                            "The filter needs this much data after each sample, so its output comes this late",
                        )
                    });
                }
            });
            input_summary(ui, context);
        });
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        let mut warnings = require_input(inputs.any_connected(), "a continuous data stream");
        if let Some(c) = inputs.continuous {
            let from = self.accurate_from(c.sample_rate);
            if from > self.lowest_hz * 1.01 {
                warnings.push(format!(
                    "At {}, the transform is only accurate above {from:.1} Hz: downsample first (to about 1 kHz)",
                    format_rate(c.sample_rate)
                ));
            }
            if self.lowest_hz >= c.sample_rate / 4.0 {
                warnings.push(format!(
                    "The lowest frequency must be well below {} (half the sample rate)",
                    format_rate(c.sample_rate / 2.0)
                ));
            }
        }
        warnings
    }
}

/// How much more than a pass's worth of samples each channel's working buffer has room for, so a pass that's a
/// little longer than usual doesn't need more memory (as `Continuous` buffers have room for a quarter more).
const PASS_ROOM: f64 = ephys_rs::engine::PASS_DURATION * 1.5;

/// The filter, and each channel's recent input, which each pass's filtering carries on from.
#[derive(Default)]
pub struct Transforming {
    /// How many samples either side of each sample the filter reaches (the output's delay, in samples).
    half: usize,
    /// The filter's weights at offsets 1, 3, 5... up to `half`. The weight at offset `-k` is minus the weight at `k`,
    /// and the weights at even offsets are zero.
    taps: Vec<f32>,
    /// For each channel, the last `2 × half` samples of input, followed by room for a pass's samples.
    recent: Vec<Vec<f32>>,
}

impl ephys_rs::engine::PassState for Transforming {
    const WHY: &'static str = "Each channel's recent input: the filter reaches a fixed number of samples either side \
                               of each sample, so each pass needs the end of the passes before it.";
}

impl Transforming {
    /// The filter reaching `half` samples either side, for `stream`, with room for passes of up to `room` seconds.
    fn new(half: usize, stream: Continuous, room: f64) -> Self {
        let window_length = (2 * half) as f64;
        let blackman = |i: usize| {
            let x = std::f64::consts::TAU * i as f64 / window_length;
            0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
        };
        let taps = (1..=half)
            .step_by(2)
            .map(|k| (2.0 / (std::f64::consts::PI * k as f64) * blackman(half + k)) as f32)
            .collect();
        let pass = (stream.sample_rate as f64 * room).ceil() as usize;
        let recent = (0..stream.channels).map(|_| Vec::with_capacity(2 * half + pass)).collect();
        let mut transforming = Transforming { half, taps, recent };
        for recent in &mut transforming.recent {
            recent.resize(2 * half, 0.0);
        }
        transforming
    }

    /// Fills `output` with the analytic signal of `input`, `half` samples late.
    fn run(&mut self, input: &ContinuousBuffer, output: &mut ComplexContinuousBuffer) {
        let half = self.half;
        output.len = input.len.min(output.capacity);
        let len = output.len;
        for (channel, recent) in self.recent.iter_mut().enumerate().take(output.channels.min(input.channels)) {
            recent.extend_from_slice(&input.channel(channel)[..len]);
            let (re, im) = output.channel_mut(channel);
            // The output's sample `i` is the input's `half` samples before it: `recent[half + i]`.
            re.copy_from_slice(&recent[half..half + len]);
            im.fill(0.0);
            for (j, &weight) in self.taps.iter().enumerate() {
                let k = 2 * j + 1;
                let before = &recent[half - k..half - k + len];
                let after = &recent[half + k..half + k + len];
                for ((y, b), a) in im.iter_mut().zip(before).zip(after) {
                    *y += weight * (b - a);
                }
            }
            // Keep the last `2 × half` samples for the next pass.
            recent.drain(..len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephys_rs::engine::DataStreamType;

    const RATE: f32 = 1000.0;

    /// Runs `transforming` on `signal` for one channel, in passes of the lengths in `passes`, round and round, and
    /// returns the analytic signal.
    fn run(transforming: &mut Transforming, signal: &[f32], passes: &[usize]) -> Vec<(f32, f32)> {
        let stream = Continuous { channels: 1, sample_rate: RATE };
        let mut input = stream.buffer(0.1);
        let mut output = ComplexContinuous { channels: 1, sample_rate: RATE }.buffer(0.1);
        let mut analytic = Vec::new();
        let mut at = 0;
        for &len in passes.iter().cycle() {
            if at >= signal.len() {
                break;
            }
            let len = len.min(signal.len() - at);
            input.len = len;
            input.channel_mut(0).copy_from_slice(&signal[at..at + len]);
            transforming.run(&input, &mut output);
            let (re, im) = output.channel(0);
            analytic.extend(re.iter().copied().zip(im.iter().copied()));
            at += len;
        }
        analytic
    }

    fn sine(hz: f32, samples: usize) -> Vec<f32> {
        (0..samples).map(|n| (std::f32::consts::TAU * hz * n as f32 / RATE).cos() * 100.0).collect()
    }

    /// A cosine's analytic signal is a complex exponential: its real part the cosine and its imaginary part the sine,
    /// both `half` samples late. That holds from the lowest frequency up, across passes of any length.
    #[test]
    fn a_cosine_becomes_a_complex_exponential_half_late() {
        let node = HilbertTransform::default();
        let half = node.half(RATE);
        assert_eq!(half, 375, "1.5 cycles of 4 Hz at 1 kHz");
        let stream = Continuous { channels: 1, sample_rate: RATE };
        for hz in [4.0, 8.0, 40.0, 200.0] {
            let mut transforming = Transforming::new(half, stream, 0.1);
            let samples = 4000;
            let analytic = run(&mut transforming, &sine(hz, samples), &[20, 21, 7, 60]);
            assert_eq!(analytic.len(), samples);
            // Once the filter's full (after `2 × half` samples), the output at `n` is the input at `n - half`.
            for (n, &(re, im)) in analytic.iter().enumerate().skip(2 * half) {
                let phase = std::f32::consts::TAU * hz * (n - half) as f32 / RATE;
                assert!((re - 100.0 * phase.cos()).abs() < 1e-3, "{hz} Hz: real part at {n}");
                assert!((im - 100.0 * phase.sin()).abs() < 0.2, "{hz} Hz: imaginary part at {n}: {im}");
            }
        }
    }

    #[test]
    fn the_filter_is_capped_and_says_so() {
        let node = HilbertTransform::default();
        assert_eq!(node.half(30000.0), MAX_HALF);
        assert!(node.accurate_from(30000.0) > 10.0);
        let inputs = DataStreamIn { continuous: Some(Continuous { channels: 4, sample_rate: 30000.0 }) };
        let warnings = Node::<crate::Edges>::warnings(&node, &inputs);
        assert!(warnings.iter().any(|w| w.contains("downsample first")), "{warnings:?}");
        let inputs = DataStreamIn { continuous: Some(Continuous { channels: 4, sample_rate: RATE }) };
        assert!(Node::<crate::Edges>::warnings(&node, &inputs).is_empty());
    }

    /// The output is complex continuous data, with the input's channels and rate.
    #[test]
    fn the_output_matches_the_input() {
        let node = HilbertTransform::default();
        let inputs = DataStreamIn { continuous: Some(Continuous { channels: 4, sample_rate: RATE }) };
        let outputs = Node::<crate::Edges>::output_streams(&node, &inputs);
        assert_eq!(outputs.analytic, Some(ComplexContinuous { channels: 4, sample_rate: RATE }));
    }
}
