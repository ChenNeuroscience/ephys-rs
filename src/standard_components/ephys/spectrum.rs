//! The Spectrum: each channel's power spectrum over a band of interest (theta, 4–12 Hz, to begin with), for the LFP
//! Viewer to show beside its traces.
//!
//! It keeps about four seconds of each channel, averaged down to about 1 kHz, and works out a few channels' spectra
//! each pass (by real FFT, Hann windowed), so each is fresh every few passes, smoothed over the last few. Each pass
//! passes on the latest of them all. It only runs while something needs them (see `Node::evaluate_tree_shake`): the LFP Viewer,
//! with its spectrum on.

use crate::core::engine::TreeShake;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Color32};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};

use crate::core::engine::{
    Category, DataStreamType, EdgeInfo, EdgeType, Input, Node, NodeInfo, Output, Pass, UiContext,
};
use crate::standard_components::ephys::{Continuous, ContinuousBuffer};
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};

/// About how many samples a second the spectra are worked out from.
const SPECTRUM_RATE: f32 = 1000.0;
/// Samples each spectrum's worked out from: about four seconds.
pub const SPECTRUM_SAMPLES: usize = 4096;
/// Each channel's spectrum is worked out again every so many passes (a few channels a pass).
const SPECTRUM_PASSES: usize = 10;
/// How much of each new spectrum goes into the smoothed one.
const SMOOTHING: f32 = 0.3;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spectrum {
    /// The band, in Hz.
    pub band: [f32; 2],
}

impl Default for Spectrum {
    fn default() -> Self {
        Spectrum { band: [4.0, 12.0] }
    }
}

/// Each channel's power in each of a band's frequency bins: the first bin's frequency, and the step to each next.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spectra {
    pub channels: u32,
    pub bins: u32,
    pub first_hz: f32,
    pub step_hz: f32,
}

impl Spectra {
    /// Of `input`'s channels, over `band`.
    pub fn of(input: Continuous, band: [f32; 2]) -> Spectra {
        let bins = Analysis::bins(input.sample_rate, band);
        let step_hz = input.sample_rate / Analysis::factor(input.sample_rate) as f32 / SPECTRUM_SAMPLES as f32;
        Spectra { channels: input.channels, bins: bins.len() as u32, first_hz: bins.start as f32 * step_hz, step_hz }
    }

    /// The band's top, in Hz.
    pub fn last_hz(&self) -> f32 {
        self.first_hz + self.bins.saturating_sub(1) as f32 * self.step_hz
    }
}

impl EdgeType for Spectra {
    const INFO: EdgeInfo = EdgeInfo {
        name: "spectra",
        colour: Color32::from_rgb(230, 190, 90),
        description: "Each channel's power spectrum over a band of frequencies.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {:.1}–{:.1} Hz", self.channels, self.first_hz, self.last_hz())
    }
}

/// The latest spectra: each channel's power in each bin (channel after channel), and whether it's been worked out yet.
#[derive(Default)]
pub struct SpectraBuffer {
    pub power: Vec<f32>,
    pub done: Vec<bool>,
    pub bins: usize,
}

impl DataStreamType for Spectra {
    type Buffer = SpectraBuffer;

    fn buffer(&self, _duration: f64) -> SpectraBuffer {
        let (channels, bins) = (self.channels as usize, self.bins as usize);
        SpectraBuffer { power: vec![0.0; channels * bins], done: vec![false; channels], bins }
    }

    fn bytes(buffer: &SpectraBuffer) -> usize {
        buffer.power.capacity() * size_of::<f32>() + buffer.done.capacity()
    }

    fn copy(from: &SpectraBuffer, to: &mut SpectraBuffer) {
        to.power.clone_from(&from.power);
        to.done.clone_from(&from.done);
        to.bins = from.bins;
    }
}

impl crate::core::engine::PassState for Analysis {
    const WHY: &'static str = "Each channel's last few seconds (averaged down), which each pass adds to, and its \
                               spectrum, smoothed over passes.";
}

impl Node<Edges> for Spectrum {
    type State = Analysis;
    type Shared = ();
    type UiState = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Spectrum",
        category: Category::Processing,
        description: "Each channel's power spectrum over a band (theta, say), for the LFP Viewer to show: worked out \
                      only while something needs it.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Continuous(c)) => Some(DataStream::Spectra(Spectra::of(c, self.band))),
            _ => None,
        };
        vec![Output { name: "spectra", kind: Kind::Spectra, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "a continuous data stream")
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Analysis {
        let mut analysis = Analysis::default();
        if let Some(DataStream::Continuous(c)) = inputs[0] {
            analysis.reshape(c.channels as usize, c.sample_rate, self.band);
            analysis.rate = c.sample_rate;
        }
        analysis
    }

    fn process(&self, pass: &mut Pass<Edges>, analysis: &mut Analysis) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let rate = analysis.rate;
        analysis.reshape(input.channels, rate, self.band);
        analysis.add(input, 0);
        analysis.work_out();
        if let Some(Buffer::Spectra(out)) = pass.outputs.get_mut(0) {
            out.power.clone_from(&analysis.power);
            out.done.clone_from(&analysis.done);
            out.bins = analysis.shape.2.len();
        }
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        crate::core::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let rate = match context.inputs[0] {
                Some(DataStream::Continuous(c)) => c.sample_rate,
                _ => 30000.0,
            };
            let nyquist = rate / Analysis::factor(rate) as f32 / 2.0;
            settings_grid(ui, |ui| {
                setting(ui, "Band", |ui| {
                    ui.horizontal(|ui| {
                        let [low, high] = &mut node.band;
                        ui.add(egui::DragValue::new(low).range(0.0..=nyquist).speed(0.1)).on_hover_text("From (Hz)");
                        ui.label("–");
                        ui.add(egui::DragValue::new(high).range(0.5..=nyquist).speed(0.1).suffix(" Hz"))
                            .on_hover_text("To (Hz)");
                        if *high <= *low {
                            *high = (*low + 0.5).min(nyquist);
                        }
                    })
                    .response
                });
            });
            input_summary(ui, context.inputs);
        });
    }
}

/// The channels' spectra over the band: from the last `SPECTRUM_SAMPLES` of each, averaged down from the input.
#[derive(Default)]
pub struct Analysis {
    /// What they're for: channels, input samples to each kept, and the band's bins.
    shape: (usize, usize, Range<usize>),
    /// For each channel, the sum of the input samples so far toward the next kept, and how many.
    sums: Vec<f32>,
    summed: usize,
    /// For each channel, `SPECTRUM_SAMPLES` kept, round a ring; where the next goes, and how many there are.
    kept: Vec<f32>,
    head: usize,
    count: usize,
    /// For each channel, the power in each of the band's bins, smoothed; and whether it's been worked out yet.
    power: Vec<f32>,
    done: Vec<bool>,
    /// The next channel to work out.
    next: usize,
    /// A real FFT (the samples being real, half the work of a complex one), and what it works in: the windowed
    /// samples, the spectrum (to half the rate kept), and its scratch.
    fft: Option<Arc<dyn RealToComplex<f32>>>,
    /// The input's sample rate.
    rate: f32,
    window: Vec<f32>,
    samples: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Analysis {
    /// Input samples to each kept, for a rate.
    pub fn factor(rate: f32) -> usize {
        (rate / SPECTRUM_RATE).round().max(1.0) as usize
    }

    /// The bins of a band, at a rate.
    pub fn bins(rate: f32, band: [f32; 2]) -> Range<usize> {
        let step = rate / Self::factor(rate) as f32 / SPECTRUM_SAMPLES as f32;
        let low = (band[0].max(0.0) / step).ceil() as usize;
        let high = ((band[1] / step).floor() as usize + 1).min(SPECTRUM_SAMPLES / 2);
        low.min(high)..high
    }

    fn reshape(&mut self, channels: usize, rate: f32, band: [f32; 2]) {
        let shape = (channels, Self::factor(rate), Self::bins(rate, band));
        if self.shape == shape {
            return;
        }
        let bins = shape.2.len();
        let n = SPECTRUM_SAMPLES;
        let fft = self.fft.take().unwrap_or_else(|| RealFftPlanner::new().plan_fft_forward(n));
        let (samples, spectrum, scratch) = (fft.make_input_vec(), fft.make_output_vec(), fft.make_scratch_vec());
        // Keep the samples so far, if they're for the same channels.
        let (kept, head, count) = if self.shape.0 == channels && self.shape.1 == shape.1 {
            (std::mem::take(&mut self.kept), self.head, self.count)
        } else {
            (vec![0.0; channels * n], 0, 0)
        };
        *self = Analysis {
            shape,
            sums: vec![0.0; channels],
            summed: 0,
            kept,
            head,
            count,
            power: vec![0.0; channels * bins],
            done: vec![false; channels],
            next: 0,
            fft: Some(fft),
            rate: self.rate,
            window: (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect(),
            samples,
            spectrum,
            scratch,
        };
    }

    /// Keeps a pass's samples of the channels shown (from `first`).
    /// A run of samples at a time, up to where the next is kept, channel by channel (as they're laid out).
    pub fn add(&mut self, input: &ContinuousBuffer, first: usize) {
        let (channels, factor, _) = self.shape.clone();
        let n = SPECTRUM_SAMPLES;
        let mut t = 0;
        while t < input.len {
            let run = (factor - self.summed).min(input.len - t);
            for (c, sum) in self.sums.iter_mut().enumerate().take(channels) {
                *sum += input.channel(first + c)[t..t + run].iter().sum::<f32>();
            }
            (t, self.summed) = (t + run, self.summed + run);
            if self.summed == factor {
                for (c, sum) in self.sums.iter_mut().enumerate() {
                    self.kept[c * n + self.head] = *sum / factor as f32;
                    *sum = 0.0;
                }
                (self.head, self.summed, self.count) = ((self.head + 1) % n, 0, (self.count + 1).min(n));
            }
        }
    }

    /// Works out the next few channels' spectra, once there are enough samples.
    fn work_out(&mut self) {
        let (channels, _, bins) = self.shape.clone();
        let n = SPECTRUM_SAMPLES;
        let Some(fft) = self.fft.clone() else { return };
        if self.count < n || channels == 0 {
            return;
        }
        for _ in 0..channels.div_ceil(SPECTRUM_PASSES) {
            let c = self.next;
            self.next = (self.next + 1) % channels;
            // Oldest first, less the mean, windowed.
            let kept = &self.kept[c * n..(c + 1) * n];
            let mean = kept.iter().sum::<f32>() / n as f32;
            for (i, value) in self.samples.iter_mut().enumerate() {
                *value = (kept[(self.head + i) % n] - mean) * self.window[i];
            }
            // Only fails if the vectors are the wrong lengths, which they're made not to be.
            let _ = fft.process_with_scratch(&mut self.samples, &mut self.spectrum, &mut self.scratch);
            let power = &mut self.power[c * bins.len()..(c + 1) * bins.len()];
            let first = !self.done[c];
            for (p, bin) in power.iter_mut().zip(bins.clone()) {
                let new = self.spectrum[bin].norm_sqr() / n as f32;
                *p = if first { new } else { *p + SMOOTHING * (new - *p) };
            }
            self.done[c] = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::PASS_DURATION;

    #[test]
    fn the_spectrum_peaks_at_the_rhythm() {
        let spectrum = Spectrum::default();
        let stream = Continuous { channels: 2, sample_rate: 30000.0 };
        let mut analysis = spectrum.initial_state(&[Some(DataStream::Continuous(stream))]);
        let mut input = stream.buffer(PASS_DURATION);
        let per_pass = 600;
        input.len = per_pass;
        // 8 Hz on the first channel, 10 Hz on the second, for five seconds.
        for sequence in 0..250u64 {
            for (c, hz) in [(0, 8.0), (1, 10.0)] {
                for (s, x) in input.channel_mut(c).iter_mut().enumerate() {
                    let t = (sequence as usize * per_pass + s) as f32 / 30000.0;
                    *x = 100.0 * (std::f32::consts::TAU * hz * t).sin();
                }
            }
            let buffer = Buffer::Continuous(input);
            let inputs = [Some(&buffer)];
            let mut pass = Pass::new(sequence, PASS_DURATION, &inputs, &mut []);
            spectrum.process(&mut pass, &mut analysis);
            let Buffer::Continuous(b) = buffer else { unreachable!() };
            input = b;
        }
        let spectra = &analysis;
        assert!(spectra.done.iter().all(|d| *d));
        let bins = spectra.shape.2.clone();
        let step = 30000.0 / 30.0 / SPECTRUM_SAMPLES as f32;
        for (c, hz) in [(0, 8.0), (1, 10.0)] {
            let power = &spectra.power[c * bins.len()..(c + 1) * bins.len()];
            let peak = power.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
            let peak_hz = (bins.start + peak) as f32 * step;
            assert!((peak_hz - hz).abs() < step, "channel {c}: {peak_hz} Hz");
        }
    }
}
