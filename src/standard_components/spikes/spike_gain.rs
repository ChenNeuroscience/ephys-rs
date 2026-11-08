//! The Spike Gain: multiplies each channel's spike waveforms (µV) by its gain, a dumb multiplier, into whatever units
//! what takes them wants (as floats): int8 values for Write LTX, say, or int16, or 0 to 1. Gains can be one for all
//! channels, one for each group (a tetrode's, say), or one each (see `Gains`). They can be chosen from the data when
//! it's triggered ("auto detect", from its window or a Spike Viewer's): its passes sample each channel's spikes for a
//! while, then have a multiple of the biggest reach the output range, which is the only thing that range is for.

use crate::core::engine::{BufferUsage, TreeShake};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Input, Node, NodeInfo, Output, Pass, SettingPort, UiContext};
use crate::core::triggers::Trigger;
use crate::standard_components::auto_detect::{AutoDetect, SAMPLING, auto_detect_ui, detecting};
use crate::standard_components::spikes::{
    Channels, DEFAULT_RANGE_UV, GainScope, Gains, Polarity, RANGES_UV, SpikesBuffer,
};
use crate::standard_components::widgets::{input_summary, named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeGain {
    pub gains: Gains,
    /// Seconds to sample the data for, to choose the gains from.
    pub sampling: f32,
    /// What's to reach the output range, when choosing them: each channel's biggest spike, times this.
    pub multiple: f32,
}

impl Default for SpikeGain {
    fn default() -> Self {
        SpikeGain { gains: Gains::default(), sampling: 20.0, multiple: 2.0 }
    }
}

/// While running: what's been sampled, while choosing the gains; and how that's going, for the window.
#[derive(Default)]
pub struct Gaining {
    sampling: Option<Sampling>,
    /// Which way the spikes go.
    polarity: Polarity,
}

impl crate::core::engine::PassState for Gaining {
    const WHY: &'static str = "While choosing the gains from the data: each channel's spikes sampled so far, which \
                               each pass adds to.";
}

/// What's been sampled so far, to choose the gains from.
struct Sampling {
    elapsed: f64,
    seconds: f64,
    /// Each channel's biggest excursion, the way spikes go.
    peaks: Vec<f32>,
}

impl Node<Edges> for SpikeGain {
    type State = Gaining;

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

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<crate::core::engine::Output<Edges>> {
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

    fn triggers(&self, _setting: usize) -> &'static [crate::core::triggers::TriggerKind] {
        &[AutoDetect::KIND]
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
        if pass.triggered::<AutoDetect>().is_some() {
            let seconds = self.sampling.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
            gaining.sampling = Some(Sampling { elapsed: 0.0, seconds, peaks: vec![0.0; channels] });
        }
        if gaining.sampling.is_some() && pass.trigger_cancelled::<AutoDetect>() {
            gaining.sampling = None;
        }
        if let Some(sampling) = &mut gaining.sampling {
            sampling.add(spikes, gaining.polarity, pass.duration);
            pass.trigger_progress::<AutoDetect>((sampling.elapsed / sampling.seconds) as f32);
            if sampling.elapsed >= sampling.seconds {
                let mut gains = self.gains.clone();
                gains.set_ranges(&sampling.choose(self.multiple, &spikes.thresholds), layout);
                pass.trigger_settings_update(0, Setting::Gains(gains));
                pass.trigger_done::<AutoDetect>();
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

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
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
                    let response = ui
                        .add(field)
                        .on_hover_text("The output range to fill: ±128 for int8, ±32768 for int16, ±1 for floats");
                    self.gains.output_range = output_range;
                    response
                });
                setting(ui, "Biggest spike ×", |ui| {
                    ui.add(egui::DragValue::new(&mut self.multiple).range(0.1..=20.0).speed(0.01)).on_hover_text(
                        "The biggest spike times this fills the output range. It's never less than 1.5 × the threshold",
                    )
                });
                setting(ui, "Sampling for", |ui| {
                    ui.add(egui::DragValue::new(&mut self.sampling).range(SAMPLING).speed(0.2).suffix(" s"))
                        .on_hover_text("How long to sample for")
                });
            })
        });
        auto_detect_ui(ui, context, None, "Sample each channel's spikes, then set its gain");
        ui.add_space(4.0);
        input_summary(ui, context.inputs);
    }
}

impl Sampling {
    fn add(&mut self, spikes: &SpikesBuffer, polarity: Polarity, duration: f64) {
        let (k, samples) = (spikes.group_size, spikes.samples);
        for (i, spike) in spikes.spikes.iter().enumerate() {
            let waveform = spikes.waveform(i);
            for c in 0..k {
                let Some(peak) = self.peaks.get_mut(spike.group as usize * k + c) else { continue };
                let waveform = &waveform[c * samples..(c + 1) * samples];
                *peak = waveform.iter().fold(*peak, |peak, x| peak.max(polarity.extremity(*x)));
            }
        }
        self.elapsed += duration;
    }

    /// Each channel's range (µV), given its threshold: its biggest spike times `multiple`, with room for its threshold
    /// (at most two-thirds of the way up).
    fn choose(&self, multiple: f32, thresholds: &[f32]) -> Vec<f32> {
        self.peaks
            .iter()
            .zip(thresholds)
            .map(|(&peak, &threshold)| {
                let range = (multiple * peak).max(1.5 * threshold);
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
        exchange.trigger(NodeId(0), crate::core::triggers::AnyTrigger::new(AutoDetect));
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
            pass.exchange = exchange;
            gain.process(&mut pass, &mut state);
            assert_eq!(state.sampling.is_some(), sequence + 1 < passes, "samples for 4 s, at pass {sequence}");
        }
        assert_eq!(
            exchange.trigger_progress(NodeId(0), crate::core::triggers::TriggerKind::of::<AutoDetect>()),
            None,
            "done"
        );
        let updates = exchange.direct_triggers().take_settings_updates();
        let Some(Setting::Gains(gains)) = updates[0].2.downcast_ref::<Setting>() else { panic!() };
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(gains.range_uv(0), 200.0), "the first channel's biggest, times 2");
        assert!(close(gains.range_uv(1), 600.0), "and the second's");
        assert!(close(gains.range_uv(2), 120.0), "no spikes on it: room for its threshold");
        assert!(close(gains.range_uv(3), 75.0), "and for the default threshold");
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
        exchange.trigger(NodeId(0), crate::core::triggers::AnyTrigger::new(AutoDetect));
        let mut run = |sequence| {
            let mut pass = Pass::new(sequence, PASS_DURATION, &[], &mut outputs);
            pass.exchange = exchange;
            gain.process(&mut pass, &mut state);
        };
        run(0);
        run(1);
        assert!(
            exchange.trigger_progress(NodeId(0), crate::core::triggers::TriggerKind::of::<AutoDetect>()).is_some(),
            "sampling"
        );
        exchange.cancel(NodeId(0), crate::core::triggers::TriggerKind::of::<AutoDetect>());
        assert_eq!(
            exchange.trigger_progress(NodeId(0), crate::core::triggers::TriggerKind::of::<AutoDetect>()),
            None,
            "stopped, as far as the GUI's concerned"
        );
        for sequence in 2..600 {
            run(sequence);
        }
        assert!(exchange.direct_triggers().take_settings_updates().is_empty(), "nothing chosen");
        assert_eq!(exchange.trigger_progress(NodeId(0), crate::core::triggers::TriggerKind::of::<AutoDetect>()), None);
    }

    /// Its window, a gain for each tetrode. See tests/snapshots/.
    #[test]
    fn settings_window() {
        use crate::Nodes;
        use crate::core::engine::{Graph, NodesEnum};
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
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, false, false, None, |node, context| {
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
