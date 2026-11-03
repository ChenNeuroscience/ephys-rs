//! The Spike Gain: multiplies each channel's spike waveforms (µV) by its gain, a dumb multiplier, into whatever units
//! what takes them wants (as floats): int8 values for Write LTX, say, or int16, or 0 to 1. Gains can be one for all
//! channels, one for each group (a tetrode's, say), or one each (see `Gains`). They can be chosen from the data when
//! it's triggered ("auto detect", from its window or a Spike Viewer's): its passes sample each channel's spikes for a
//! while, then have the bigger of them reach the output range, which is the only thing that range is for.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::core::exchange::Latest;
use crate::standard_components::spikes::{Channels, DEFAULT_RANGE_UV, GainScope, Gains, RANGES_UV, SpikesBuffer};
use crate::standard_components::widgets::{input_summary, named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

/// Its trigger, to choose the gains from the data.
pub const AUTO_DETECT: &str = "auto detect";
/// How long the passes can sample for, in seconds.
pub const SAMPLING: std::ops::RangeInclusive<f32> = 4.0..=60.0;
/// The most spikes sampled on each channel.
const SAMPLED_SPIKES: usize = 4000;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeGain {
    pub gains: Gains,
    /// Seconds to sample the data for, to choose the gains from.
    pub sampling: f32,
}

impl Default for SpikeGain {
    fn default() -> Self {
        SpikeGain { gains: Gains::default(), sampling: 10.0 }
    }
}

/// While running: what's been sampled, while choosing the gains; and how that's going, for the window.
#[derive(Default)]
pub struct Gaining {
    sampling: Option<Sampling>,
    shown: Latest<Option<(f32, f32)>>,
}

impl crate::core::engine::PassState for Gaining {
    const WHY: &'static str = "While choosing the gains from the data: each channel's spikes and noise sampled so \
                               far, which each pass adds to.";
}

/// What's been sampled so far, to choose the gains from.
struct Sampling {
    elapsed: f64,
    seconds: f64,
    /// For each channel: its noise summed over the passes so far, and each spike's biggest excursion on it.
    noise: Vec<f64>,
    passes: u32,
    peaks: Vec<Vec<f32>>,
}

impl Node<Edges> for SpikeGain {
    type State = Gaining;

    const INFO: NodeInfo = NodeInfo {
        name: "Spike Gain",
        category: Category::Processing,
        description: "Multiplies spikes' waveforms by each channel's gain, into the units what takes them wants (int8 \
                      values for Write LTX, say): set by hand, from a Spike Viewer, or chosen from the data.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "spikes", kind: Kind::Spikes }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<crate::core::engine::Output<Edges>> {
        vec![Output { name: "spikes", kind: Kind::Spikes, stream: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "spikes")
    }

    fn in_place(&self, _output: usize) -> Option<usize> {
        Some(0)
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "gains", value: self.gains.clone().into() }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Gains(gains) = value {
            self.gains = gains;
        }
    }

    fn triggers(&self, _setting: usize) -> &'static [&'static str] {
        &[AUTO_DETECT]
    }

    /// Multiplies each channel's waveforms by its gain (saying so on the spikes); and, when triggered, samples the
    /// spikes (in µV, before that) for a while, then proposes gains from them.
    fn process(&self, pass: &mut Pass<Edges>, gaining: &mut Gaining) {
        if let Some(Some(Buffer::Spikes(input))) = pass.inputs.first()
            && let Some(Buffer::Spikes(output)) = pass.outputs.get_mut(0)
        {
            copy(input, output);
        }
        let Some(Buffer::Spikes(spikes)) = pass.outputs.first() else { return };
        let channels = spikes.gains.len();
        let layout = Channels { count: channels, group_size: spikes.group_size };
        if pass.triggered(AUTO_DETECT) {
            let seconds = self.sampling.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
            gaining.sampling = Some(Sampling {
                elapsed: 0.0,
                seconds,
                noise: vec![0.0; channels],
                passes: 0,
                peaks: (0..channels).map(|_| Vec::with_capacity(SAMPLED_SPIKES)).collect(),
            });
        }
        if gaining.sampling.is_some() && pass.trigger_cancelled(AUTO_DETECT) {
            gaining.sampling = None;
        }
        if let Some(sampling) = &mut gaining.sampling {
            sampling.add(spikes, pass.duration);
            pass.trigger_progress(AUTO_DETECT, (sampling.elapsed / sampling.seconds) as f32);
            if sampling.elapsed >= sampling.seconds {
                let mut gains = self.gains.clone();
                gains.set_ranges(&sampling.choose(&spikes.thresholds), layout);
                pass.propose(0, Setting::Gains(gains));
                pass.trigger_done(AUTO_DETECT);
                gaining.sampling = None;
            }
        }
        let progress = gaining.sampling.as_ref().map(|s| (s.elapsed as f32, s.seconds as f32));
        gaining.shown.send(pass, |shown| *shown = progress);

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

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let channels = match context.inputs[0] {
            Some(DataStream::Spikes(s)) => Channels { count: s.channels as usize, group_size: s.group_size as usize },
            _ => Channels { count: 0, group_size: 4 },
        };
        // The gains: how they're shared out, then each one.
        ui.strong("Gains");
        named_settings_grid(ui, "gain scope", |ui| {
            setting(ui, "One for", |ui| {
                let mut scope = self.gains.scope;
                let response = egui::ComboBox::from_id_salt("gain scope")
                    .selected_text(scope.name(channels.group_size))
                    .show_ui(ui, |ui| {
                        for option in GainScope::ALL {
                            ui.selectable_value(&mut scope, option, option.name(channels.group_size));
                        }
                    })
                    .response
                    .on_hover_text("Whether the channels share a gain: all of them, each group's, or none");
                if scope != self.gains.scope {
                    self.gains.set_scope(scope, channels);
                }
                response
            });
        });
        if channels.count > 0 {
            let units = self.gains.units(channels);
            let kind = match self.gains.scope {
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
                        let name = match self.gains.scope {
                            GainScope::All => format!("1–{}", channels.count),
                            GainScope::Group => format!("{} ({}–{})", index + 1, unit.start + 1, unit.end),
                            GainScope::Channel => format!("{}", unit.start + 1),
                        };
                        ui.label(name);
                        let mut gain = self.gains.get(unit.start);
                        let speed = gain * 0.01;
                        let field =
                            egui::DragValue::new(&mut gain).range(1e-6..=1e6).speed(speed).max_decimals(3).prefix("×");
                        if ui.add(field).changed() {
                            self.gains.set(unit.start, gain, channels);
                        }
                        ui.label(format!("±{:.0} µV", self.gains.range_uv(unit.start)));
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
                    let mut output_range = self.gains.output_range;
                    let field = egui::DragValue::new(&mut output_range).range(1e-6..=1e9).speed(1.0).prefix("±");
                    let response = ui.add(field).on_hover_text(
                        "Gains chosen from the data have each channel's bigger spikes (the 98th percentile's biggest \
                     excursion) reach this, in the units of what takes them: ±128 for Write LTX's int8 values, say \
                     (or less, for room above them), ±32768 for int16, ±1 for floats",
                    );
                    self.gains.output_range = output_range;
                    response
                });
                setting(ui, "Sampling for", |ui| {
                    ui.add(egui::DragValue::new(&mut self.sampling).range(SAMPLING).speed(0.2).suffix(" s"))
                        .on_hover_text("How long to sample the spikes for")
                });
            })
        });
        let tip = format!(
            "Sample the spikes, then set the gains so the bigger ones reach ±{}, sharing them out as above",
            self.gains.output_range
        );
        auto_detect_ui(ui, context, None, &tip);
        ui.add_space(4.0);
        input_summary(ui, context.inputs);
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

impl Sampling {
    fn add(&mut self, spikes: &SpikesBuffer, duration: f64) {
        for (sum, noise) in self.noise.iter_mut().zip(&spikes.noise) {
            *sum += *noise as f64;
        }
        self.passes += 1;
        let (k, samples) = (spikes.group_size, spikes.samples);
        for (i, spike) in spikes.spikes.iter().enumerate() {
            let waveform = spikes.waveform(i);
            for c in 0..k {
                let channel = spike.group as usize * k + c;
                let Some(peaks) = self.peaks.get_mut(channel) else { continue };
                let peak = waveform[c * samples..(c + 1) * samples].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                if peaks.len() < SAMPLED_SPIKES {
                    peaks.push(peak);
                }
            }
        }
        self.elapsed += duration;
    }

    /// Each channel's range (µV), given its threshold: the bigger of its spikes (the 98th percentile's biggest
    /// excursion) reach it, with room for its threshold (at most two-thirds of the way up); with too few spikes to
    /// tell, it's eight standard deviations of its noise.
    fn choose(&mut self, thresholds: &[f32]) -> Vec<f32> {
        let passes = self.passes.max(1) as f64;
        self.peaks
            .iter_mut()
            .zip(&self.noise)
            .zip(thresholds)
            .map(|((peaks, &noise), &threshold)| {
                let noise = (noise / passes) as f32;
                let biggest = if peaks.len() >= 10 {
                    peaks.sort_unstable_by(f32::total_cmp);
                    peaks[(peaks.len() - 1) * 98 / 100]
                } else {
                    8.0 * noise
                };
                let range = biggest.max(1.5 * threshold);
                if range > 0.0 { range.clamp(*RANGES_UV.start(), *RANGES_UV.end()) } else { DEFAULT_RANGE_UV }
            })
            .collect()
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
    use crate::core::engine::{DataStreamType, NodeId, PASS_DURATION};
    use crate::standard_components::spikes::{DEFAULT_THRESHOLD_UV, Spike};

    #[test]
    fn chooses_gains_from_the_spikes_when_triggered() {
        let gain = SpikeGain { sampling: 4.0, ..Default::default() };
        let stream = crate::standard_components::spikes::Spikes {
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
        let exchange = Pass::<Edges>::new(0, PASS_DURATION, &[], &mut []).exchange;
        exchange.fire(NodeId(0), AUTO_DETECT);
        for sequence in 0..passes {
            let Buffer::Spikes(spikes) = &mut outputs[0] else { unreachable!() };
            spikes.clear();
            // A spike each pass, peaking at 100 µV on the first channel, 500 µV on the second (now and then).
            if let Some(waveform) = spikes.push(Spike { sample: 0, group: 0, channel: 0 }) {
                waveform[8] = -100.0;
                waveform[40 + 8] = if sequence % 4 == 0 { -500.0 } else { -50.0 };
            }
            let mut pass = Pass::new(sequence, PASS_DURATION, &[], &mut outputs);
            pass.exchange = exchange;
            gain.process(&mut pass, &mut state);
            assert_eq!(state.sampling.is_some(), sequence + 1 < passes, "samples for 4 s, at pass {sequence}");
        }
        assert_eq!(exchange.trigger_progress(NodeId(0), AUTO_DETECT), None, "done");
        let proposals = exchange.take_proposals();
        let Some(Setting::Gains(gains)) = proposals[0].2.downcast_ref::<Setting>() else { panic!() };
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(gains.range_uv(0), 100.0), "the first channel's spikes reach the output range");
        assert!(close(gains.range_uv(1), 500.0), "the second's biggest");
        assert!(close(gains.range_uv(2), 120.0), "spikes no bigger than 0: room for its threshold");
        assert!(close(gains.range_uv(3), 75.0), "and for the default threshold, with no noise");
        assert_eq!(gains.output_range, 128.0, "for 8-bit samples, to begin with");
    }

    #[test]
    fn stopping_auto_detect_abandons_it() {
        let gain = SpikeGain::default();
        let stream = crate::standard_components::spikes::Spikes {
            channels: 4,
            sample_rate: 30000.0,
            group_size: 4,
            pre: 8,
            post: 32,
            polarity: Default::default(),
        };
        let mut state = gain.initial_state(&[Some(DataStream::Spikes(stream))]);
        let mut outputs = [Buffer::Spikes(stream.buffer(PASS_DURATION))];
        let exchange = Pass::<Edges>::new(0, PASS_DURATION, &[], &mut []).exchange;
        exchange.fire(NodeId(0), AUTO_DETECT);
        let mut run = |sequence| {
            let mut pass = Pass::new(sequence, PASS_DURATION, &[], &mut outputs);
            pass.exchange = exchange;
            gain.process(&mut pass, &mut state);
        };
        run(0);
        run(1);
        assert!(exchange.trigger_progress(NodeId(0), AUTO_DETECT).is_some(), "sampling");
        exchange.cancel(NodeId(0), AUTO_DETECT);
        assert_eq!(exchange.trigger_progress(NodeId(0), AUTO_DETECT), None, "stopped, as far as the GUI's concerned");
        for sequence in 2..600 {
            run(sequence);
        }
        assert!(exchange.take_proposals().is_empty(), "nothing chosen");
        assert_eq!(exchange.trigger_progress(NodeId(0), AUTO_DETECT), None);
    }

    /// Its window, a gain for each tetrode. See tests/snapshots/.
    #[test]
    fn settings_window() {
        use crate::Nodes;
        use crate::core::engine::{Graph, NodeSet};
        use crate::standard_components::spikes::Spikes;
        use crate::standard_components::themes::THEMES;
        let mut graph = Graph::<Nodes>::default();
        let index = Nodes::TYPES.iter().position(|t| t.name == "Spike Gain").unwrap();
        let id = graph.add(Nodes::create(index), [0.0, 0.0]);
        let spikes =
            Spikes { channels: 8, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32, polarity: Default::default() };
        if let Some(Nodes::SpikeGain(gain)) = graph.get_mut(id).map(|e| &mut e.node) {
            let channels = Channels { count: 8, group_size: 4 };
            gain.gains.set_scope(GainScope::Group, channels);
            gain.gains.set_range(5, 120.0, channels);
        }
        let inputs = [Some(DataStream::Spikes(spikes))];
        let mut harness = egui_kittest::Harness::builder().with_size([420.0, 380.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(id, &inputs, &THEMES[0].colours, None, false, false, None, |node, context| {
                    node.settings_ui(ui, context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("spike_gain_settings");
    }
}
