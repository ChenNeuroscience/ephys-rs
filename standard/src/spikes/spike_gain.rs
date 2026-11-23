//! The Spike Gain: multiplies each channel's spike waveforms (µV) by its gain, as an amplifier does (×2 doubles them),
//! into amplified µV: what's recorded (Write LTX's int8 values cover ±250 µV of them). Gains can be one for all
//! channels, one for each group (a tetrode's, say), or one each (see `Gains`). They can be chosen from the data when
//! their auto detect's started (from its window or a Spike Viewer's: see `Gains::detect`): its passes sample each
//! channel's spikes for a while, then choose what's to fill what's recorded (`Gains::fill_uv`) by the channel's
//! threshold, its spikes' peaks, or both (see `Fit`).
//!
//! The gain reads the thresholds from the Spike Detector's (its `thresholds` input, connected to the detector's
//! `thresholds`), as they are in the snapshot the detector's finding spikes by; unconnected, it goes by the default
//! threshold. If they change while it's sampling (moved by hand, or
//! detected just before, and only now applied), it starts again, as the spikes it had were found by the old ones. A
//! Spike Viewer detects the thresholds first, then the gains.

use eframe::egui;
use ephys_rs::engine::{Buffers, Prepare, PrepareContext, Prepared, TreeShake};
use serde::{Deserialize, Serialize};

use crate::standard::auto_detect::{Detect, Distribution, SAMPLING, auto_detect_ui, detect_changed};
use crate::standard::spikes::Spikes;
use crate::standard::spikes::{
    Channels, DEFAULT_RANGE_UV, GainScope, Gains, RANGES_UV, SetGains, SpikesBuffer, Thresholds, fill_field, gain_field,
};
use crate::standard::widgets::{input_summary, named_settings_grid, require_input, setting};
use ephys_rs::engine::{Category, CommandContext, CommandSource, Node, NodeInfo, PassContext, UiContext};
use ephys_rs::gui::widgets::info_icon;

ephys_rs::ports! {
    spikes: DataStreamIn<Spikes>,
    spikes: DataStreamOut<Spikes> in_place_of spikes,
    gains: ValOutCmdIn<Gains, (SetGains, Detect)>,
    /// The Spike Detector's thresholds, which the gains' auto detect can go by. The gain only reads them.
    thresholds: ValIn<Thresholds>,
}

#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeGain {
    pub gains: Gains,
    /// What's to fill what's recorded (`Gains::fill_uv`), when choosing them.
    pub fit: Fit,
}

/// What's to fill what's recorded, for each channel, when choosing the gains: its threshold times a factor, its spikes'
/// peaks (a percentile of them) times a factor, or the two together. Never less than 1.5 × the threshold, so it's in
/// view; and a channel with no spikes goes by its threshold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fit {
    pub algorithm: Algorithm,
    pub threshold_factor: f32,
    /// Which percentile of the spikes' peaks (each spike's furthest from zero on the channel): 100 for the biggest.
    pub peak_percent: f32,
    pub peak_factor: f32,
    /// Whether to turn each channel's spikes to go down, as most do (a Spike Viewer shows them pointing up): a
    /// negative gain where they go up.
    pub auto_invert: bool,
}

impl Default for Fit {
    fn default() -> Self {
        Fit {
            algorithm: Algorithm::GeometricMean,
            threshold_factor: 3.0,
            peak_percent: 95.0,
            peak_factor: 0.8,
            auto_invert: true,
        }
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

    /// How the threshold's and the peaks' ranges are combined, if they are.
    fn combined(self) -> Option<&'static str> {
        match self {
            Algorithm::Threshold | Algorithm::Peaks => None,
            Algorithm::Larger => Some("the larger of the two"),
            Algorithm::Mean => Some("their mean"),
            Algorithm::GeometricMean => Some("their geometric mean"),
        }
    }

    /// What the threshold factor does, as its info says.
    fn threshold_info(self) -> String {
        let start = "For each channel, multiplies the spike threshold by the given factor";
        match self.combined() {
            None => format!("{start} and maps that value to the upper limit of the fit range."),
            Some(how) => format!(
                "{start}. That value and the one from the peak percentile are combined by taking {how}, which is \
                 mapped to the upper limit of the fit range."
            ),
        }
    }

    /// What the peak percentile and its factor do, as their info says.
    fn peaks_info(self) -> String {
        let start = "For each channel, computes the given percentile of its spikes' peaks over the sampling time";
        match self.combined() {
            None => format!(
                "{start}, multiplies it by the given factor, and maps that value to the upper limit of the fit range."
            ),
            Some(how) => format!(
                "{start} and multiplies it by the given factor. That value and the one from the threshold factor are \
                 combined by taking {how}, which is mapped to the upper limit of the fit range."
            ),
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
    /// Only while the gains are being detected (see `crate::standard::auto_detect`).
    sampling: Option<Sampling>,
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
    /// Each channel's spikes' peaks: each spike's biggest excursion on it, from zero either way.
    peaks: Vec<Distribution>,
    /// Which way each channel's spikes go: their extrema (each's furthest from zero, signed), added up.
    extrema: Vec<f64>,
    /// Whether the gains have been chosen: `detect`'s still set in the snapshot, till the update choosing them (which
    /// also clears it) takes over.
    done: bool,
}

impl<E: PortEdges> Node<E> for SpikeGain {
    type Ports = Ports;
    type State = Gaining;
    type WorkerState = ();
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Spike Gain",
        category: Category::Processing,
        description: "Amplifies spikes' waveforms by each channel's gain (×2 doubles them), to fill what's recorded: \
                      set by hand, or chosen from the data.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], _recording: bool) -> TreeShake {
        // And while detecting, whatever needs what it makes: its gains are wanted before recording, say.
        let wanted = outputs.contains(&true) || self.gains.detect.started.is_some();
        TreeShake::needing_all(inputs.contains(&true) && wanted, inputs.len())
    }

    fn data_stream_out(&self, inputs: &DataStreamIn) -> DataStreamOut {
        DataStreamOut { spikes: inputs.spikes }
    }

    fn val_out(&self) -> ValOutCmdIn {
        ValOutCmdIn { gains: self.gains.clone() }
    }

    /// New gains: from its passes, those auto detect chose, with it done; from a viewer, an edit, leaving auto detect
    /// as it is.
    fn on_command(&mut self, from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {
            CmdIn::SetGains(SetGains(gains)) => {
                let started = self.gains.detect.started;
                self.gains = gains;
                if from != CommandSource::Itself {
                    self.gains.detect.started = started;
                }
            }
            CmdIn::Detect(Detect(request)) => request.apply(&mut self.gains.detect.started),
        }
    }

    fn clear_transient_state(&mut self) {
        self.gains.detect.started = None;
    }

    /// While the gains are being detected, each channel's room to sample into.
    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Gaining, (), Self> {
        let before = context.previous.as_ref().map(|previous| previous.node.gains.detect.started);
        if context.same_streams() && !detect_changed(self.gains.detect.started, before) {
            return Prepared::nothing().into();
        }
        let channels = context.data_stream_in::<DataStreamIn>().spikes.map_or(0, |stream| stream.channels as usize);
        let seconds = self.gains.detect.seconds.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
        let sampling = self.gains.detect.started.map(|_| Sampling::new(seconds, channels));
        Prepared::shared(Gaining { sampling }).into()
    }

    /// Multiplies each channel's waveforms by its gain; and, while the gains are being detected, samples the spikes (in
    /// µV, before that) for a while, then sends itself gains chosen from them, with `detect` cleared.
    fn process(
        &self,
        pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        outputs: DataStreamOut<Buffers>,
        gaining: &mut Gaining,
        _: &mut (),
    ) {
        let spikes = outputs.spikes;
        if let Some(input) = inputs.spikes {
            copy(input, spikes);
        }
        if let Some(sampling) = gaining.sampling.as_mut().filter(|s| !s.done) {
            let channels = sampling.peaks.len();
            let layout = Channels { count: channels, group_size: spikes.group_size };
            let default = Thresholds::default();
            let thresholds = pass.val_in(ValIn::thresholds).unwrap_or(&default);
            // From the start, and again if the thresholds change: the spikes so far were found by the old ones.
            if !sampling.found_by(thresholds) {
                sampling.restart(thresholds, channels);
            }
            sampling.add(spikes, pass.duration);
            if sampling.elapsed >= sampling.seconds {
                let mut gains = self.gains.clone();
                // Done: no longer detecting.
                gains.detect.started = None;
                gains.set_ranges(&sampling.choose(self.fit), layout);
                if self.fit.auto_invert {
                    sampling.turn_down(&mut gains, layout);
                }
                pass.cmd_self(CmdIn::SetGains(SetGains(gains)));
                *sampling = Sampling { done: true, ..Sampling::new(sampling.seconds, 0) };
            }
        }

        // The gains, applied: amplified µV.
        let (k, samples, size) = (spikes.group_size, spikes.samples, spikes.group_size * spikes.samples);
        for (i, spike) in spikes.spikes.iter().enumerate() {
            let waveform = &mut spikes.waveforms[i * size..(i + 1) * size];
            for c in 0..k {
                let gain = self.gains.get(spike.group as usize * k + c);
                waveform[c * samples..(c + 1) * samples].iter_mut().for_each(|v| *v *= gain);
            }
        }
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let channels = match context.data_stream_in::<DataStreamIn>().spikes {
                Some(s) => Channels { count: s.channels as usize, group_size: s.group_size as usize },
                None => Channels { count: 0, group_size: 4 },
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
                    egui::Grid::new("gains").num_columns(2).striped(true).show(ui, |ui| {
                        ui.strong(kind);
                        ui.strong("Gain");
                        ui.end_row();
                        for (index, unit) in units.iter().enumerate() {
                            let name = match node.gains.scope {
                                GainScope::All => format!("1–{}", channels.count),
                                GainScope::Group => format!("{} ({}–{})", index + 1, unit.start + 1, unit.end),
                                GainScope::Channel => format!("{}", unit.start + 1),
                            };
                            ui.label(name);
                            let mut gain = node.gains.get(unit.start);
                            if gain_field(ui, &mut gain, true).changed() {
                                node.gains.set(unit.start, gain, channels);
                            }
                            ui.end_row();
                        }
                    });
                });
            }

            // Choosing them from the data.
            ui.add_space(8.0);
            ui.strong("Auto detect");
            let detecting = node.gains.detect.started.is_some();
            ui.add_enabled_ui(!detecting, |ui| {
                named_settings_grid(ui, "auto detect", |ui| {
                    setting(ui, "Fit range", |ui| fill_field(ui, &mut node.gains));
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
                                "What fills what's recorded: each channel's threshold times a factor, a percentile \
                                 of its spikes' peaks times a factor, or both together. Never less than 1.5 × the \
                                 threshold; a channel without spikes goes by its threshold",
                            )
                    });
                    if fit.algorithm.uses_threshold() {
                        setting(ui, "Threshold factor", |ui| {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::DragValue::new(&mut fit.threshold_factor)
                                        .range(1.5..=20.0)
                                        .speed(0.01)
                                        .prefix("×"),
                                )
                                .on_hover_text("The threshold times this");
                                info_icon(ui, &fit.algorithm.threshold_info());
                            })
                            .response
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
                                info_icon(ui, &fit.algorithm.peaks_info());
                            })
                            .response
                        });
                    }
                    setting(ui, "Auto invert", |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut node.fit.auto_invert, "")
                                .on_hover_text("Turn each channel's spikes to go down, as most do");
                            info_icon(
                                ui,
                                "For each channel, adds up its spikes' extrema (each spike's furthest point from \
                                 zero) over the sampling time. If the total is positive, it uses a negative gain for \
                                 that channel, so its spikes have negative voltage (rendered in the up direction in \
                                 the Spike Viewer).",
                            );
                        })
                        .response
                    });
                    setting(ui, "Sampling for", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut node.gains.detect.seconds)
                                .range(SAMPLING)
                                .speed(0.2)
                                .suffix(" s"),
                        )
                        .on_hover_text("How long to sample for")
                    });
                })
            });
            let detect = auto_detect_ui(
                ui,
                context,
                node.gains.detect.started,
                node.gains.detect.seconds,
                "Sample each channel's spikes, then set its gain",
            );
            if let Some(request) = detect {
                request.apply(&mut node.gains.detect.started);
            }
            ui.add_space(4.0);
            input_summary(ui, context);
        });
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        require_input(inputs.any_connected(), "spikes")
    }
}

impl Sampling {
    /// Room for `channels`' spikes, found by thresholds yet to be seen.
    fn new(seconds: f64, channels: usize) -> Self {
        let (peaks, extrema) = (vec![Distribution::default(); channels], vec![0.0; channels]);
        Sampling { elapsed: 0.0, seconds, thresholds: Vec::new(), peaks, extrema, done: false }
    }

    /// Whether the spikes sampled so far were found by `thresholds` (none have been, before the first `restart`).
    fn found_by(&self, thresholds: &Thresholds) -> bool {
        !self.thresholds.is_empty() && self.thresholds.iter().enumerate().all(|(c, &t)| t == thresholds.get(c))
    }

    /// Starts again, in the room it has, with `channels`' spikes found by `thresholds`.
    fn restart(&mut self, thresholds: &Thresholds, channels: usize) {
        self.elapsed = 0.0;
        self.thresholds.clear();
        self.thresholds.extend((0..channels).map(|c| thresholds.get(c)));
        self.peaks.resize_with(channels, Distribution::default);
        self.peaks.iter_mut().for_each(Distribution::clear);
        self.extrema.clear();
        self.extrema.resize(channels, 0.0);
    }

    fn add(&mut self, spikes: &SpikesBuffer, duration: f64) {
        let (k, samples) = (spikes.group_size, spikes.samples);
        for (i, spike) in spikes.spikes.iter().enumerate() {
            let waveform = spikes.waveform(i);
            for c in 0..k {
                let channel = spike.group as usize * k + c;
                let (Some(peaks), Some(extrema)) = (self.peaks.get_mut(channel), self.extrema.get_mut(channel)) else {
                    continue;
                };
                let waveform = &waveform[c * samples..(c + 1) * samples];
                // Its biggest excursion, either way.
                let peak =
                    waveform.iter().copied().fold(0.0_f32, |peak, x| if x.abs() > peak.abs() { x } else { peak });
                peaks.add(peak.abs());
                *extrema += peak as f64;
            }
        }
        self.elapsed += duration;
    }

    /// Turns each channel's spikes to go down, as most extracellular spikes do: a negative gain where their extrema
    /// add up to above zero, a positive one where below; those sharing a gain by all theirs together. Those without
    /// spikes stay as they are.
    fn turn_down(&self, gains: &mut Gains, layout: Channels) {
        for unit in gains.units(layout) {
            let total: f64 = unit.clone().filter_map(|c| self.extrema.get(c)).sum();
            if total != 0.0 && gains.inverted(unit.start) != (total > 0.0) {
                gains.invert(unit.start, layout);
            }
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edges;
    use crate::standard::spikes::Spike;
    use crate::{Command, NodeType};
    use ephys_rs::engine::DataStreamInPorts;
    use ephys_rs::engine::HasValue;
    use ephys_rs::engine::{DataStreamType, PASS_DURATION};

    #[test]
    fn a_channels_range_goes_by_its_threshold_its_peaks_or_both() {
        let fit = |algorithm| Fit {
            algorithm,
            threshold_factor: 4.0,
            peak_percent: 95.0,
            peak_factor: 2.0,
            auto_invert: true,
        };
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
    fn chooses_gains_from_the_spikes_when_detecting() {
        let fit = Fit { algorithm: Algorithm::Peaks, peak_percent: 100.0, peak_factor: 2.0, ..Default::default() };
        let mut gain = SpikeGain { fit, ..Default::default() };
        gain.gains.detect.seconds = 4.0;
        let stream =
            crate::standard::spikes::Spikes { channels: 4, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32 };
        let started = std::time::Instant::now();
        let inputs = DataStreamIn { spikes: Some(stream) };
        gain.gains.detect.started = Some(started);
        let mut state = ephys_rs::engine::fresh_state::<Edges, _>(&gain, &inputs);
        let mut spikes = stream.buffer(PASS_DURATION);
        let mut thresholds = Thresholds::default();
        thresholds.set(2, 80.0);
        let val_ins = [Some(<Edges as HasValue<Thresholds>>::wrap(thresholds))];
        let passes = (4.0 / PASS_DURATION).round() as u64;
        let commands = ephys_rs::commands::Commands::default();
        for sequence in 0..passes {
            spikes.clear();
            // Going down to 100 µV on the first channel (and back up past zero, not as far), and up to 300 on the
            // second (now and then).
            if let Some(waveform) = spikes.push(Spike { sample: 0, group: 0, channel: 0 }) {
                (waveform[8], waveform[9]) = (-100.0, 50.0);
                waveform[40 + 8] = if sequence % 4 == 0 { 300.0 } else { 50.0 };
            }
            let mut pass = PassContext::<Edges>::new(sequence, PASS_DURATION);
            (pass.commands, pass.val_ins) = (&commands, &val_ins);
            // Made in place: the output already holds the spikes.
            gain.process(
                &mut pass,
                DataStreamIn { spikes: None },
                DataStreamOut { spikes: &mut spikes },
                &mut state,
                &mut (),
            );
            let sampling = state.sampling.as_ref().is_some_and(|s| !s.done);
            assert_eq!(sampling, sequence + 1 < passes, "samples for 4 s, at pass {sequence}");
        }
        let updates = commands.take_commands::<Command>();
        let [(_, Command::SetGains(SetGains(gains)))] = &updates[..] else { panic!("{updates:?}") };
        assert_eq!(gains.detect.started, None, "with auto detect cleared");
        // To within a bin of the peaks' distribution: about 1.5%.
        let close = |a: f32, b: f32| (a - b).abs() < 0.02 * b;
        assert!(close(gains.range_uv(0), 200.0), "the first channel's biggest, times 2: {}", gains.range_uv(0));
        assert!(close(gains.range_uv(1), 600.0), "and the second's: {}", gains.range_uv(1));
        assert!(close(gains.range_uv(2), 120.0), "no spikes on it: room for its threshold");
        assert!(close(gains.range_uv(3), 75.0), "and for the default threshold");
        assert_eq!(gains.fill_uv, 250.0, "Write LTX's full scale, to begin with");
        // The second channel's spikes go up, so it's turned upside down, to go down as the first's do.
        assert!(!gains.inverted(0) && gains.inverted(1), "{:?}", gains.gains);
        assert!(!gains.inverted(2), "no spikes: as it was");
        // A percentile of them leaves out the second channel's few big ones: 3 in 4 are 50 µV.
        let mut sampling = Sampling::new(4.0, 4);
        sampling.restart(&Thresholds::default(), 4);
        for sequence in 0..8 {
            spikes.clear();
            if let Some(waveform) = spikes.push(Spike { sample: 0, group: 0, channel: 0 }) {
                waveform[40 + 8] = if sequence % 4 == 0 { -300.0 } else { -50.0 };
            }
            sampling.add(&spikes, PASS_DURATION);
        }
        let range = sampling.choose(Fit { peak_percent: 50.0, ..fit })[1];
        assert!(close(range, 100.0), "half of them are at or below 50 µV: {range}");
    }

    #[test]
    fn it_samples_again_if_the_thresholds_change_while_it_is() {
        let mut gain = SpikeGain::default();
        gain.gains.detect.seconds = 4.0;
        let stream =
            crate::standard::spikes::Spikes { channels: 4, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32 };
        let started = std::time::Instant::now();
        let inputs = DataStreamIn { spikes: Some(stream) };
        gain.gains.detect.started = Some(started);
        let mut state = ephys_rs::engine::fresh_state::<Edges, _>(&gain, &inputs);
        let mut spikes = stream.buffer(PASS_DURATION);
        let commands = ephys_rs::commands::Commands::default();
        let passes = (4.0 / PASS_DURATION).round() as u64;
        let mut run = |sequence, spikes: &mut SpikesBuffer, thresholds: &Thresholds| {
            let val_ins = [Some(<Edges as HasValue<Thresholds>>::wrap(thresholds.clone()))];
            let mut pass = PassContext::<Edges>::new(sequence, PASS_DURATION);
            (pass.commands, pass.val_ins) = (&commands, &val_ins);
            gain.process(&mut pass, DataStreamIn { spikes: None }, DataStreamOut { spikes }, &mut state, &mut ());
        };
        let mut thresholds = Thresholds::default();
        for sequence in 0..passes / 2 {
            run(sequence, &mut spikes, &thresholds);
        }
        // The detector's chosen its thresholds.
        for (channel, uv) in [30.0, 40.0, 60.0, 70.0].into_iter().enumerate() {
            thresholds.set(channel, uv);
        }
        // From the pass the thresholds changed on, it samples for 4 s again.
        let done = passes / 2 + passes - 1;
        for sequence in passes / 2..done {
            run(sequence, &mut spikes, &thresholds);
            assert!(commands.take_commands::<Command>().is_empty(), "sampling again, at pass {sequence}");
        }
        run(done, &mut spikes, &thresholds);
        let updates = commands.take_commands::<Command>();
        let [(_, Command::SetGains(SetGains(gains)))] = &updates[..] else { panic!("{updates:?}") };
        // No spikes: each by its new threshold, times the factor.
        let factor = Fit::default().threshold_factor;
        let expected: Vec<f32> = [30.0, 40.0, 60.0, 70.0].iter().map(|t| (t * factor).round()).collect();
        assert_eq!((0..4).map(|c| gains.range_uv(c).round()).collect::<Vec<_>>(), expected);
    }

    #[test]
    fn stopping_auto_detect_lets_go_of_what_it_sampled() {
        use ephys_rs::engine::{Node as _, PrepareContext, Previous};
        let mut gain = SpikeGain::default();
        let stream =
            crate::standard::spikes::Spikes { channels: 4, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32 };
        let started = std::time::Instant::now();
        let inputs = DataStreamIn { spikes: Some(stream) };
        gain.gains.detect.started = Some(started);
        let mut state = ephys_rs::engine::fresh_state::<Edges, _>(&gain, &inputs);
        let mut spikes = stream.buffer(PASS_DURATION);
        let commands = ephys_rs::commands::Commands::default();
        for sequence in 0..2 {
            let mut pass = PassContext::<Edges>::new(sequence, PASS_DURATION);
            pass.commands = &commands;
            gain.process(
                &mut pass,
                DataStreamIn { spikes: None },
                DataStreamOut { spikes: &mut spikes },
                &mut state,
                &mut (),
            );
        }
        assert!(state.sampling.as_ref().is_some_and(|s| s.elapsed > 0.0), "sampling");
        // Stopped: auto detect cleared, the node makes its state without the sampling.
        let was = gain.clone();
        gain.gains.detect.started = None;
        let streams = DataStreamInPorts::<Edges>::streams(&inputs);
        let previous: Previous<Edges, _> = Previous { node: &was, inputs: &streams, same_streams: true };
        let context = PrepareContext { previous: Some(previous), ..PrepareContext::trying(&streams, None) };
        let mut proto = gain.prepare_for_snapshot(&context).wait().proto_shared_state.expect("made afresh");
        Node::<Edges>::adopt_snapshot(&gain, &mut state, &mut proto);
        assert!(state.sampling.is_none(), "let go of");
        for sequence in 2..600 {
            let mut pass = PassContext::<Edges>::new(sequence, PASS_DURATION);
            pass.commands = &commands;
            gain.process(
                &mut pass,
                DataStreamIn { spikes: None },
                DataStreamOut { spikes: &mut spikes },
                &mut state,
                &mut (),
            );
        }
        assert!(commands.take_commands::<Command>().is_empty(), "nothing chosen");
    }

    /// Its window, a gain for each tetrode. See tests/snapshots/.
    /// Spikes whose extrema add up above zero are turned down, by a negative gain; those below, the right way up;
    /// those sharing a gain, by all theirs together; those without spikes, as they were.
    #[test]
    fn auto_invert_turns_spikes_to_go_down() {
        let mut sampling = Sampling::new(1.0, 8);
        sampling.restart(&Thresholds::default(), 8);
        sampling.extrema = vec![-300.0, 200.0, 0.0, 0.0, -100.0, 150.0, 0.0, -10.0];
        let (tetrodes, each) = (Channels { count: 8, group_size: 4 }, Channels { count: 8, group_size: 1 });
        let mut gains = Gains::default();
        gains.invert(1, each);
        gains.invert(3, each);
        sampling.turn_down(&mut gains, each);
        let inverted: Vec<bool> = (0..8).map(|c| gains.inverted(c)).collect();
        assert_eq!(
            inverted,
            [false, true, false, true, false, true, false, false],
            "the third and fourth, as they were"
        );
        // A tetrode each: the first's add up to -100, the second's to 40.
        let mut gains = Gains { scope: GainScope::Group, ..Default::default() };
        sampling.turn_down(&mut gains, tetrodes);
        assert!((0..4).all(|c| !gains.inverted(c)) && (4..8).all(|c| gains.inverted(c)));
    }

    /// The factors' info says how they're used, as the algorithm has it: on their own, or combined.
    #[test]
    fn the_factors_info_follows_the_algorithm() {
        assert!(
            Algorithm::Threshold
                .threshold_info()
                .ends_with("by the given factor and maps that value to the upper limit of the fit range.")
        );
        assert!(Algorithm::Peaks.peaks_info().contains("multiplies it by the given factor, and maps that value"));
        for (algorithm, how) in [
            (Algorithm::Larger, "the larger of the two"),
            (Algorithm::Mean, "their mean"),
            (Algorithm::GeometricMean, "their geometric mean"),
        ] {
            let both = [algorithm.threshold_info(), algorithm.peaks_info()];
            assert!(both.iter().all(|info| info.contains(&format!("combined by taking {how}, which is mapped"))));
        }
    }

    #[test]
    fn settings_window() {
        use crate::Nodes;
        use crate::standard::themes::THEMES;
        use ephys_rs::engine::{Graph, NodesEnum};
        let mut graph = Graph::<Nodes>::default();
        let node_type = NodeType::SpikeGain;
        let id = graph.add(Nodes::create(node_type), [0.0, 0.0]);
        let spikes = Spikes { channels: 8, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32 };
        if let Some(Nodes::SpikeGain(gain)) = graph.get_mut(id).map(|e| &mut e.node) {
            let channels = Channels { count: 8, group_size: 4 };
            gain.gains.set_scope(GainScope::Group, channels);
            gain.gains.set_range(5, 120.0, channels);
        }
        let inputs = DataStreamInPorts::<Edges>::streams(&DataStreamIn { spikes: Some(spikes) });
        let mut harness = egui_kittest::Harness::builder().with_size([420.0, 440.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, None, false, false, None, |node, context| {
                    node.on_gui_tick(Some(ui), context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("spike_gain_settings");
    }
}
