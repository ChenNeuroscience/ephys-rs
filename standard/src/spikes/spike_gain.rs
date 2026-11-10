//! The Spike Gain: multiplies each channel's spike waveforms (µV) by its gain, a dumb multiplier, into whatever units
//! what takes them wants (as floats): int8 values for Write LTX, say, or int16, or 0 to 1. Gains can be one for all
//! channels, one for each group (a tetrode's, say), or one each (see `Gains`). They can be chosen from the data when
//! it's triggered ("auto detect", from its window or a Spike Viewer's): its passes sample each channel's spikes for a
//! while, then choose what's to reach the output range (which is the only thing that range is for) by the channel's
//! threshold, its spikes' peaks, or both (see `Fit`).
//!
//! The thresholds come with the spikes, as the detector has them. If they change while it's sampling (moved by hand, or
//! detected just before, and only now applied), it starts again, as the spikes it had were found by the old ones. A
//! Spike Viewer detects the thresholds first, then the gains.

use eframe::egui;
use ephys_rs::engine::{BufferUsage, TreeShake};
use serde::{Deserialize, Serialize};

use crate::standard::auto_detect::{Distribution, SAMPLING, auto_detect_ui, detecting};
use crate::standard::spikes::{Channels, DEFAULT_RANGE_UV, GainScope, Gains, Polarity, RANGES_UV, SpikesBuffer};
use crate::standard::widgets::{input_summary, named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};
use ephys_rs::engine::{Category, Input, Node, NodeInfo, Output, Pass, SettingPort, UiContext};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeGain {
    pub gains: Gains,
    /// Seconds to sample the data for, to choose the gains from.
    pub sampling: f32,
    /// What's to reach the output range, when choosing them.
    pub fit: Fit,
}

impl Default for SpikeGain {
    fn default() -> Self {
        SpikeGain { gains: Gains::default(), sampling: 20.0, fit: Fit::default() }
    }
}

/// What's to reach the output range, for each channel, when choosing the gains: its threshold times a factor, its
/// spikes' peaks (a percentile of them) times a factor, or the two together. Never less than 1.5 × the threshold, so it's in
/// view; and a channel with no spikes goes by its threshold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fit {
    pub algorithm: Algorithm,
    pub threshold_factor: f32,
    /// Which percentile of the spikes' peaks (each spike's on the channel, the way spikes go): 100 for the biggest.
    pub peak_percent: f32,
    pub peak_factor: f32,
}

impl Default for Fit {
    fn default() -> Self {
        Fit { algorithm: Algorithm::GeometricMean, threshold_factor: 4.0, peak_percent: 95.0, peak_factor: 1.2 }
    }
}

/// How a channel's range is chosen: from its threshold, its spikes' peaks, or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Algorithm {
    Threshold,
    Peaks,
    /// The larger of the threshold's and the peaks'.
    Larger,
    Mean,
    #[default]
    GeometricMean,
}

impl Algorithm {
    const ALL: [Algorithm; 5] =
        [Algorithm::Threshold, Algorithm::Peaks, Algorithm::Larger, Algorithm::Mean, Algorithm::GeometricMean];

    fn name(self) -> &'static str {
        match self {
            Algorithm::Threshold => "Threshold × factor",
            Algorithm::Peaks => "Peak percentile × factor",
            Algorithm::Larger => "Larger of threshold & peaks × factors",
            Algorithm::Mean => "Mean of threshold & peaks × factors",
            Algorithm::GeometricMean => "Geometric mean of threshold & peaks × factors",
        }
    }

    fn uses_threshold(self) -> bool {
        self != Algorithm::Peaks
    }

    fn uses_peaks(self) -> bool {
        self != Algorithm::Threshold
    }
}

impl Fit {
    /// A channel's range (µV), given its threshold, and its spikes' peaks, if it had any.
    fn range(self, threshold: f32, peaks: Option<f32>) -> f32 {
        let by_threshold = self.threshold_factor * threshold;
        let range = match peaks.map(|peaks| self.peak_factor * peaks) {
            None => by_threshold,
            Some(by_peaks) => match self.algorithm {
                Algorithm::Threshold => by_threshold,
                Algorithm::Peaks => by_peaks,
                Algorithm::Larger => by_threshold.max(by_peaks),
                Algorithm::Mean => (by_threshold + by_peaks) / 2.0,
                Algorithm::GeometricMean => (by_threshold * by_peaks).sqrt(),
            },
        };
        let range = range.max(1.5 * threshold);
        if range > 0.0 { range.clamp(*RANGES_UV.start(), *RANGES_UV.end()) } else { DEFAULT_RANGE_UV }
    }
}

/// While running: what's been sampled, while choosing the gains; and how that's going, for the window.
#[derive(Default)]
pub struct Gaining {
    sampling: Option<Sampling>,
    /// Which way the spikes go.
    polarity: Polarity,
}

impl ephys_rs::engine::PassState for Gaining {
    const WHY: &'static str = "While choosing the gains from the data: each channel's spikes sampled so far, which \
                               each pass adds to.";
}

/// What's been sampled so far, to choose the gains from.
struct Sampling {
    elapsed: f64,
    seconds: f64,
    /// The thresholds the spikes were found by.
    thresholds: Vec<f32>,
    /// Each channel's spikes' peaks: each spike's biggest excursion on it, the way spikes go.
    peaks: Vec<Distribution>,
}

impl Node<Edges> for SpikeGain {
    type State = Gaining;
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Spike Gain",
        category: Category::Processing,
        description: "Multiplies spikes' waveforms by each channel's gain, into the units what takes them wants (int8 \
                      values for Write LTX, say): set by hand, or chosen from the data.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "spikes", kind: Kind::Spikes }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<ephys_rs::engine::Output<Edges>> {
        vec![Output { name: "spikes", kind: Kind::Spikes, stream: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "spikes")
    }

    /// Its output's made in place, from its input.
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage {
        BufferUsage { in_place: vec![Some(0); outputs], ..BufferUsage::none(inputs, outputs) }
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "gains", value: self.gains.clone().into(), on_right: true }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Gains(gains) = value {
            self.gains = gains;
        }
    }

    fn triggers(&self, _setting: usize) -> &'static [crate::TriggerKind] {
        &[crate::TriggerKind::AutoDetect]
    }

    /// Multiplies each channel's waveforms by its gain (saying so on the spikes); and, when triggered, samples the
    /// spikes (in µV, before that) for a while, then triggers an update to gains from them.
    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Gaining {
        let polarity = match inputs.first() {
            Some(Some(DataStream::Spikes(spikes))) => spikes.polarity,
            _ => Polarity::default(),
        };
        Gaining { sampling: None, polarity }
    }

    fn process(&self, pass: &mut Pass<Edges>, gaining: &mut Gaining) {
        if let Some(Some(Buffer::Spikes(input))) = pass.inputs.first()
            && let Some(Buffer::Spikes(output)) = pass.outputs.get_mut(0)
        {
            copy(input, output);
        }
        let Some(Buffer::Spikes(spikes)) = pass.outputs.first() else { return };
        let channels = spikes.gains.len();
        let layout = Channels { count: channels, group_size: spikes.group_size };
        let seconds = self.sampling.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
        let start = || Sampling::new(seconds, &spikes.thresholds);
        if pass.triggered(crate::TriggerKind::AutoDetect).is_some() {
            gaining.sampling = Some(start());
        }
        if gaining.sampling.is_some() && pass.trigger_cancelled(crate::TriggerKind::AutoDetect) {
            gaining.sampling = None;
        }
        // Again, if the thresholds have changed: the spikes so far were found by the old ones.
        if gaining.sampling.as_ref().is_some_and(|s| s.thresholds != spikes.thresholds) {
            gaining.sampling = Some(start());
        }
        if let Some(sampling) = &mut gaining.sampling {
            sampling.add(spikes, gaining.polarity, pass.duration);
            pass.trigger_progress(crate::TriggerKind::AutoDetect, (sampling.elapsed / sampling.seconds) as f32);
            if sampling.elapsed >= sampling.seconds {
                let mut gains = self.gains.clone();
                gains.set_ranges(&sampling.choose(self.fit), layout);
                pass.trigger_settings_update(0, Setting::Gains(gains));
                pass.trigger_done(crate::TriggerKind::AutoDetect);
                gaining.sampling = None;
            }
        }

        // The gains, applied: a dumb multiplier, into whatever units what takes the spikes wants.
        let Some(Buffer::Spikes(spikes)) = pass.outputs.first_mut() else { return };
        for (channel, gain) in spikes.gains.iter_mut().enumerate() {
            *gain = self.gains.get(channel);
        }
        spikes.output_range = self.gains.output_range;
        let (k, samples, size) = (spikes.group_size, spikes.samples, spikes.group_size * spikes.samples);
        for (i, spike) in spikes.spikes.iter().enumerate() {
            let waveform = &mut spikes.waveforms[i * size..(i + 1) * size];
            for c in 0..k {
                let gain = spikes.gains.get(spike.group as usize * k + c).copied().unwrap_or(1.0);
                waveform[c * samples..(c + 1) * samples].iter_mut().for_each(|v| *v *= gain);
            }
        }
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let channels = match context.inputs[0] {
                Some(DataStream::Spikes(s)) => {
                    Channels { count: s.channels as usize, group_size: s.group_size as usize }
                }
                _ => Channels { count: 0, group_size: 4 },
            };
            // The gains: how they're shared out, then each one.
            ui.strong("Gains");
            named_settings_grid(ui, "gain scope", |ui| {
                setting(ui, "One for", |ui| {
                    let mut scope = node.gains.scope;
                    let response = egui::ComboBox::from_id_salt("gain scope")
                        .selected_text(scope.name(channels.group_size))
                        .show_ui(ui, |ui| {
                            for option in GainScope::ALL {
                                ui.selectable_value(&mut scope, option, option.name(channels.group_size));
                            }
                        })
                        .response
                        .on_hover_text("Whether the channels share a gain: all of them, each group's, or none");
                    if scope != node.gains.scope {
                        node.gains.set_scope(scope, channels);
                    }
                    response
                });
            });
            if channels.count > 0 {
                let units = node.gains.units(channels);
                let kind = match node.gains.scope {
                    GainScope::All => "All",
                    GainScope::Group => "Group",
                    GainScope::Channel => "Channel",
                };
                egui::ScrollArea::vertical().max_height(260.0).id_salt("gains").show(ui, |ui| {
                    egui::Grid::new("gains").num_columns(3).striped(true).show(ui, |ui| {
                        ui.strong(kind);
                        ui.strong("Gain");
                        ui.strong("Fits").on_hover_text("The µV that reach the output range, at this gain");
                        ui.end_row();
                        for (index, unit) in units.iter().enumerate() {
                            let name = match node.gains.scope {
                                GainScope::All => format!("1–{}", channels.count),
                                GainScope::Group => format!("{} ({}–{})", index + 1, unit.start + 1, unit.end),
                                GainScope::Channel => format!("{}", unit.start + 1),
                            };
                            ui.label(name);
                            let mut gain = node.gains.get(unit.start);
                            let speed = gain * 0.01;
                            let field = egui::DragValue::new(&mut gain)
                                .range(1e-6..=1e6)
                                .speed(speed)
                                .max_decimals(3)
                                .prefix("×");
                            if ui.add(field).changed() {
                                node.gains.set(unit.start, gain, channels);
                            }
                            ui.label(format!("±{:.0} µV", node.gains.range_uv(unit.start)));
                            ui.end_row();
                        }
                    });
                });
            }

            // Choosing them from the data.
            ui.add_space(8.0);
            ui.strong("Auto detect");
            let detecting = detecting(context);
            ui.add_enabled_ui(!detecting, |ui| {
                named_settings_grid(ui, "auto detect", |ui| {
                    setting(ui, "Fit to", |ui| {
                        let mut output_range = node.gains.output_range;
                        let field = egui::DragValue::new(&mut output_range).range(1e-6..=1e9).speed(1.0).prefix("±");
                        let response = ui
                            .add(field)
                            .on_hover_text("The output range to fill: ±128 for int8, ±32768 for int16, ±1 for floats");
                        node.gains.output_range = output_range;
                        response
                    });
                    let fit = &mut node.fit;
                    setting(ui, "Algorithm", |ui| {
                        egui::ComboBox::from_id_salt("algorithm")
                            .selected_text(fit.algorithm.name())
                            .show_ui(ui, |ui| {
                                for option in Algorithm::ALL {
                                    ui.selectable_value(&mut fit.algorithm, option, option.name());
                                }
                            })
                            .response
                            .on_hover_text(
                                "What fills the output range: each channel's threshold times a factor, a percentile \
                                 of its spikes' peaks times a factor, or both together. Never less than 1.5 × the \
                                 threshold; a channel without spikes goes by its threshold",
                            )
                    });
                    if fit.algorithm.uses_threshold() {
                        setting(ui, "Threshold factor", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut fit.threshold_factor)
                                    .range(1.5..=20.0)
                                    .speed(0.01)
                                    .prefix("×"),
                            )
                            .on_hover_text("The threshold times this")
                        });
                    }
                    if fit.algorithm.uses_peaks() {
                        setting(ui, "Peak percentile", |ui| {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::DragValue::new(&mut fit.peak_percent)
                                        .range(50.0..=100.0)
                                        .speed(0.1)
                                        .suffix(" %"),
                                )
                                .on_hover_text("The percentile of the spikes' peaks: 100 % for the biggest");
                                ui.label("factor");
                                ui.add(
                                    egui::DragValue::new(&mut fit.peak_factor)
                                        .range(0.1..=20.0)
                                        .speed(0.01)
                                        .prefix("×"),
                                )
                                .on_hover_text("The percentile times this");
                            })
                            .response
                        });
                    }
                    setting(ui, "Sampling for", |ui| {
                        ui.add(egui::DragValue::new(&mut node.sampling).range(SAMPLING).speed(0.2).suffix(" s"))
                            .on_hover_text("How long to sample for")
                    });
                })
            });
            auto_detect_ui(ui, context, None, "Sample each channel's spikes, then set its gain");
            ui.add_space(4.0);
            input_summary(ui, context.inputs);
        });
    }
}

impl Sampling {
    fn new(seconds: f64, thresholds: &[f32]) -> Self {
        let peaks = vec![Distribution::default(); thresholds.len()];
        Sampling { elapsed: 0.0, seconds, thresholds: thresholds.to_vec(), peaks }
    }

    fn add(&mut self, spikes: &SpikesBuffer, polarity: Polarity, duration: f64) {
        let (k, samples) = (spikes.group_size, spikes.samples);
        for (i, spike) in spikes.spikes.iter().enumerate() {
            let waveform = spikes.waveform(i);
            for c in 0..k {
                let Some(peaks) = self.peaks.get_mut(spike.group as usize * k + c) else { continue };
                let waveform = &waveform[c * samples..(c + 1) * samples];
                peaks.add(waveform.iter().fold(0.0, |peak: f32, x| peak.max(polarity.extremity(*x))));
            }
        }
        self.elapsed += duration;
    }

    /// Each channel's range (µV), by `fit`.
    fn choose(&self, fit: Fit) -> Vec<f32> {
        let peaks = |peaks: &Distribution| (!peaks.is_empty()).then(|| peaks.signed_percentile(fit.peak_percent));
        self.peaks.iter().zip(&self.thresholds).map(|(p, &threshold)| fit.range(threshold, peaks(p))).collect()
    }
}

/// Copies `input`'s spikes to `output` (when it can't pass them through in place).
fn copy(input: &SpikesBuffer, output: &mut SpikesBuffer) {
    output.spikes.clone_from(&input.spikes);
    output.waveforms.clone_from(&input.waveforms);
    (output.group_size, output.samples, output.pass_samples) = (input.group_size, input.samples, input.pass_samples);
    output.thresholds.clone_from(&input.thresholds);
    output.noise.clone_from(&input.noise);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeType;
    use crate::standard::spikes::{DEFAULT_THRESHOLD_UV, Spike};
    use ephys_rs::engine::{DataStreamType, NodeId, PASS_DURATION};

    #[test]
    fn a_channels_range_goes_by_its_threshold_its_peaks_or_both() {
        let fit = |algorithm| Fit { algorithm, threshold_factor: 4.0, peak_percent: 95.0, peak_factor: 2.0 };
        // A threshold of 50 µV, so 200 by it; peaks of 200, so 400 by them.
        let ranges: Vec<f32> =
            Algorithm::ALL.into_iter().map(|algorithm| fit(algorithm).range(50.0, Some(200.0))).collect();
        let expected = [200.0, 400.0, 400.0, 300.0, 80_000f32.sqrt()];
        for ((range, expected), algorithm) in ranges.iter().zip(expected).zip(Algorithm::ALL) {
            assert!((range - expected).abs() < 0.01, "{algorithm:?}: {range}, not {expected}");
        }
        assert_eq!(fit(Algorithm::Peaks).range(50.0, None), 200.0, "no spikes: by the threshold");
        assert_eq!(fit(Algorithm::Peaks).range(50.0, Some(10.0)), 75.0, "never less than 1.5 × the threshold");
        assert_eq!(fit(Algorithm::Threshold).range(0.0, None), DEFAULT_RANGE_UV, "nothing to go on");
    }

    #[test]
    fn chooses_gains_from_the_spikes_when_triggered() {
        let fit = Fit { algorithm: Algorithm::Peaks, peak_percent: 100.0, peak_factor: 2.0, ..Default::default() };
        let gain = SpikeGain { sampling: 4.0, fit, ..Default::default() };
        let stream = crate::standard::spikes::Spikes {
            channels: 4,
            sample_rate: 30000.0,
            group_size: 4,
            pre: 8,
            post: 32,
            polarity: Default::default(),
        };
        let mut state = gain.initial_state(&[Some(DataStream::Spikes(stream))]);
        let mut buffer = stream.buffer(PASS_DURATION);
        buffer.noise = vec![10.0, 10.0, 20.0, 0.0];
        buffer.thresholds = vec![DEFAULT_THRESHOLD_UV, DEFAULT_THRESHOLD_UV, 80.0, DEFAULT_THRESHOLD_UV];
        let passes = (4.0 / PASS_DURATION).round() as u64;
        let mut outputs = [Buffer::Spikes(buffer)];
        let (triggers, direct) =
            (ephys_rs::pass_triggers::PassTriggers::default(), ephys_rs::triggers::DirectTriggers::default());
        triggers.trigger(
            NodeId(0),
            ephys_rs::triggers::AnyTrigger::new(crate::Trigger::AutoDetect(crate::standard::auto_detect::AutoDetect)),
        );
        for sequence in 0..passes {
            let Buffer::Spikes(spikes) = &mut outputs[0] else { unreachable!() };
            spikes.clear();
            // Negative-going: peaking at 100 µV on the first channel, 300 on the second (now and then). What goes the
            // other way isn't how spikes go.
            if let Some(waveform) = spikes.push(Spike { sample: 0, group: 0, channel: 0 }) {
                (waveform[8], waveform[9]) = (-100.0, 900.0);
                waveform[40 + 8] = if sequence % 4 == 0 { -300.0 } else { -50.0 };
            }
            let mut pass = Pass::new(sequence, PASS_DURATION, &[], &mut outputs);
            (pass.triggers, pass.direct) = (&triggers, &direct);
            gain.process(&mut pass, &mut state);
            assert_eq!(state.sampling.is_some(), sequence + 1 < passes, "samples for 4 s, at pass {sequence}");
        }
        assert_eq!(triggers.trigger_progress(NodeId(0), crate::TriggerKind::AutoDetect), None, "done");
        let updates = direct.take_settings_updates();
        let Some(Setting::Gains(gains)) = updates[0].2.downcast_ref::<Setting>() else { panic!() };
        // To within a bin of the peaks' distribution: about 1.5%.
        let close = |a: f32, b: f32| (a - b).abs() < 0.02 * b;
        assert!(close(gains.range_uv(0), 200.0), "the first channel's biggest, times 2: {}", gains.range_uv(0));
        assert!(close(gains.range_uv(1), 600.0), "and the second's: {}", gains.range_uv(1));
        assert!(close(gains.range_uv(2), 120.0), "no spikes on it: room for its threshold");
        assert!(close(gains.range_uv(3), 75.0), "and for the default threshold");
        assert_eq!(gains.output_range, 128.0, "for 8-bit samples, to begin with");
        // A percentile of them leaves out the second channel's few big ones: 3 in 4 are 50 µV.
        let mut sampling = Sampling::new(4.0, &[50.0; 4]);
        let Buffer::Spikes(spikes) = &mut outputs[0] else { unreachable!() };
        for sequence in 0..8 {
            spikes.clear();
            if let Some(waveform) = spikes.push(Spike { sample: 0, group: 0, channel: 0 }) {
                waveform[40 + 8] = if sequence % 4 == 0 { -300.0 } else { -50.0 };
            }
            sampling.add(spikes, Polarity::default(), PASS_DURATION);
        }
        let range = sampling.choose(Fit { peak_percent: 50.0, ..fit })[1];
        assert!(close(range, 100.0), "half of them are at or below 50 µV: {range}");
    }

    #[test]
    fn it_samples_again_if_the_thresholds_change_while_it_is() {
        let gain = SpikeGain { sampling: 4.0, ..Default::default() };
        let stream = crate::standard::spikes::Spikes {
            channels: 4,
            sample_rate: 30000.0,
            group_size: 4,
            pre: 8,
            post: 32,
            polarity: Default::default(),
        };
        let mut state = gain.initial_state(&[Some(DataStream::Spikes(stream))]);
        let mut buffer = stream.buffer(PASS_DURATION);
        buffer.thresholds = vec![DEFAULT_THRESHOLD_UV; 4];
        let mut outputs = [Buffer::Spikes(buffer)];
        let (triggers, direct) =
            (ephys_rs::pass_triggers::PassTriggers::default(), ephys_rs::triggers::DirectTriggers::default());
        triggers.trigger(
            NodeId(0),
            ephys_rs::triggers::AnyTrigger::new(crate::Trigger::AutoDetect(crate::standard::auto_detect::AutoDetect)),
        );
        let passes = (4.0 / PASS_DURATION).round() as u64;
        let mut run = |sequence, outputs: &mut [Buffer]| {
            let mut pass = Pass::new(sequence, PASS_DURATION, &[], outputs);
            (pass.triggers, pass.direct) = (&triggers, &direct);
            gain.process(&mut pass, &mut state);
        };
        for sequence in 0..passes / 2 {
            run(sequence, &mut outputs);
        }
        // The detector's chosen its thresholds.
        let Buffer::Spikes(spikes) = &mut outputs[0] else { unreachable!() };
        spikes.thresholds = vec![30.0, 40.0, 60.0, 70.0];
        // From the pass the thresholds changed on, it samples for 4 s again.
        let done = passes / 2 + passes - 1;
        for sequence in passes / 2..done {
            run(sequence, &mut outputs);
            assert!(direct.take_settings_updates().is_empty(), "sampling again, at pass {sequence}");
        }
        run(done, &mut outputs);
        let updates = direct.take_settings_updates();
        let Some(Setting::Gains(gains)) = updates[0].2.downcast_ref::<Setting>() else { panic!() };
        // No spikes: each by its new threshold, times 4.
        assert_eq!((0..4).map(|c| gains.range_uv(c).round()).collect::<Vec<_>>(), [120.0, 160.0, 240.0, 280.0]);
    }

    #[test]
    fn stopping_auto_detect_abandons_it() {
        let gain = SpikeGain::default();
        let stream = crate::standard::spikes::Spikes {
            channels: 4,
            sample_rate: 30000.0,
            group_size: 4,
            pre: 8,
            post: 32,
            polarity: Default::default(),
        };
        let mut state = gain.initial_state(&[Some(DataStream::Spikes(stream))]);
        let mut outputs = [Buffer::Spikes(stream.buffer(PASS_DURATION))];
        let (triggers, direct) =
            (ephys_rs::pass_triggers::PassTriggers::default(), ephys_rs::triggers::DirectTriggers::default());
        triggers.trigger(
            NodeId(0),
            ephys_rs::triggers::AnyTrigger::new(crate::Trigger::AutoDetect(crate::standard::auto_detect::AutoDetect)),
        );
        let mut run = |sequence| {
            let mut pass = Pass::new(sequence, PASS_DURATION, &[], &mut outputs);
            (pass.triggers, pass.direct) = (&triggers, &direct);
            gain.process(&mut pass, &mut state);
        };
        run(0);
        run(1);
        assert!(triggers.trigger_progress(NodeId(0), crate::TriggerKind::AutoDetect).is_some(), "sampling");
        triggers.cancel(NodeId(0), crate::TriggerKind::AutoDetect);
        assert_eq!(
            triggers.trigger_progress(NodeId(0), crate::TriggerKind::AutoDetect),
            None,
            "stopped, as far as the GUI's concerned"
        );
        for sequence in 2..600 {
            run(sequence);
        }
        assert!(direct.take_settings_updates().is_empty(), "nothing chosen");
        assert_eq!(triggers.trigger_progress(NodeId(0), crate::TriggerKind::AutoDetect), None);
    }

    /// Its window, a gain for each tetrode. See tests/snapshots/.
    #[test]
    fn settings_window() {
        use crate::Nodes;
        use crate::standard::spikes::Spikes;
        use crate::standard::themes::THEMES;
        use ephys_rs::engine::{Graph, NodesEnum};
        let mut graph = Graph::<Nodes>::default();
        let node_type = NodeType::SpikeGain;
        let id = graph.add(Nodes::create(node_type), [0.0, 0.0]);
        let spikes =
            Spikes { channels: 8, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32, polarity: Default::default() };
        if let Some(Nodes::SpikeGain(gain)) = graph.get_mut(id).map(|e| &mut e.node) {
            let channels = Channels { count: 8, group_size: 4 };
            gain.gains.set_scope(GainScope::Group, channels);
            gain.gains.set_range(5, 120.0, channels);
        }
        let inputs = [Some(DataStream::Spikes(spikes))];
        let mut harness = egui_kittest::Harness::builder().with_size([420.0, 440.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, None, false, false, None, |node, context| {
                    node.gui_ui(ui, context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("spike_gain_settings");
    }
}
