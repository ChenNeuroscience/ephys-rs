//! The Continuous Gain: multiplies each channel of continuous data (µV) by its gain, as an amplifier does (×2 doubles
//! it), into amplified µV: what's recorded (Write LTX's EEG's int8 values cover ±250 µV of them). Gains can be one
//! for all channels, or one each (see `Gains`). They can be chosen from the data when it's triggered ("auto detect",
//! from its window): its passes sample each channel for a while, then have its mean, plus a multiple of a percentile's
//! distance from it, fill what's recorded (`Gains::fill_uv`). Sign doesn't count.

use eframe::egui;
use ephys_rs::engine::{BufferUsage, TreeShake};
use serde::{Deserialize, Serialize};

use crate::standard::auto_detect::{Distribution, PercentMultiple, SAMPLING, auto_detect_ui, detecting};
use crate::standard::spikes::{Channels, DEFAULT_RANGE_UV, GainScope, Gains, RANGES_UV, fill_field, gain_field};

use crate::standard::widgets::{input_summary, named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};
use ephys_rs::engine::{Category, Input, Node, NodeInfo, Output, Pass, SettingPort, UiContext};
use ephys_rs::gui::widgets::info_icon;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContinuousGain {
    pub gains: Gains,
    /// Seconds to sample the data for, to choose the gains from.
    pub sampling: f32,
    /// What's to fill what's recorded, when choosing them: this, of each channel's samples.
    pub level: PercentMultiple,
}

impl Default for ContinuousGain {
    fn default() -> Self {
        ContinuousGain {
            gains: Gains { scope: GainScope::All, ..Default::default() },
            sampling: 10.0,
            level: PercentMultiple { percent: 98.0, multiple: 2.0 },
        }
    }
}

/// While choosing the gains from the data: what's been added up of each channel so far.
#[derive(Default)]
pub struct Gaining {
    sampling: Option<Sampling>,
}

impl ephys_rs::engine::PassState for Gaining {
    const WHY: &'static str = "While choosing the gains from the data: each channel's samples added up so far, which \
                               each pass adds to.";
}

struct Sampling {
    elapsed: f64,
    seconds: f64,
    /// Each channel's samples.
    data: Vec<Distribution>,
}

impl Sampling {
    /// Each channel's range (µV): `level` of what's sampled.
    fn choose(&self, level: PercentMultiple) -> Vec<f32> {
        self.data
            .iter()
            .map(|data| {
                let range = level.either_way(data);
                if range > 0.0 { range.clamp(*RANGES_UV.start(), *RANGES_UV.end()) } else { DEFAULT_RANGE_UV }
            })
            .collect()
    }
}

impl Node<Edges> for ContinuousGain {
    type State = Gaining;
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Continuous Gain",
        category: Category::Processing,
        description: "Amplifies continuous data by each channel's gain (or one for all; ×2 doubles it), to fill what's \
                      recorded (Write LTX's EEG, say): set by hand, or chosen from the data.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        vec![Output { name: "scaled", kind: Kind::Continuous, stream: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "a continuous data stream")
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

    /// When triggered, samples each channel (in µV) for a while, then triggers an update to gains from it; and multiplies each
    /// channel by its gain.
    fn process(&self, pass: &mut Pass<Edges>, gaining: &mut Gaining) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        output.transform(input, |_, x| x);
        let channels = output.channels;
        if pass.triggered(crate::TriggerKind::AutoDetect).is_some() {
            let seconds = self.sampling.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
            gaining.sampling = Some(Sampling { elapsed: 0.0, seconds, data: vec![Distribution::default(); channels] });
        }
        if gaining.sampling.is_some() && pass.trigger_cancelled(crate::TriggerKind::AutoDetect) {
            gaining.sampling = None;
        }
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        if let Some(sampling) = &mut gaining.sampling {
            for (channel, data) in sampling.data.iter_mut().enumerate() {
                output.channel(channel).iter().for_each(|x| data.add(*x));
            }
            sampling.elapsed += pass.duration;
            let progress = (sampling.elapsed / sampling.seconds) as f32;
            let done = sampling.elapsed >= sampling.seconds;
            let layout = Channels { count: channels, group_size: 1 };
            let chosen = done.then(|| {
                let mut gains = self.gains.clone();
                gains.set_ranges(&sampling.choose(self.level), layout);
                gains
            });
            pass.trigger_progress(crate::TriggerKind::AutoDetect, progress);
            if let Some(gains) = chosen {
                pass.trigger_settings_update(0, Setting::Gains(gains));
                pass.trigger_done(crate::TriggerKind::AutoDetect);
                gaining.sampling = None;
            }
        }

        // The gains, applied.
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        for channel in 0..output.channels {
            let gain = self.gains.get(channel);
            output.channel_mut(channel).iter_mut().for_each(|x| *x *= gain);
        }
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let channels = match context.inputs[0] {
                Some(DataStream::Continuous(c)) => Channels { count: c.channels as usize, group_size: 1 },
                _ => Channels { count: 0, group_size: 1 },
            };
            // The gains: one for all, or one each.
            ui.strong("Gains");
            named_settings_grid(ui, "gain scope", |ui| {
                setting(ui, "One for", |ui| {
                    let mut scope = node.gains.scope;
                    let options = [(GainScope::All, "All channels together"), (GainScope::Channel, "Each channel")];
                    let name = options.iter().find(|(s, _)| *s == scope).map_or("Each channel", |(_, n)| n);
                    let response = egui::ComboBox::from_id_salt("gain scope")
                        .selected_text(name)
                        .show_ui(ui, |ui| {
                            for (option, name) in options {
                                ui.selectable_value(&mut scope, option, name);
                            }
                        })
                        .response
                        .on_hover_text("Whether the channels share a gain");
                    if scope != node.gains.scope {
                        node.gains.set_scope(scope, channels);
                    }
                    response
                });
            });
            if channels.count > 0 {
                egui::ScrollArea::vertical().max_height(260.0).id_salt("gains").show(ui, |ui| {
                    egui::Grid::new("gains").num_columns(2).striped(true).show(ui, |ui| {
                        ui.strong("Channel");
                        ui.strong("Gain");
                        ui.end_row();
                        for unit in node.gains.units(channels) {
                            let name = if unit.len() > 1 {
                                format!("1–{}", channels.count)
                            } else {
                                format!("{}", unit.start + 1)
                            };
                            ui.label(name);
                            let mut gain = node.gains.get(unit.start);
                            if gain_field(ui, &mut gain, false).changed() {
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
            ui.add_enabled_ui(!detecting(context), |ui| {
                named_settings_grid(ui, "auto detect", |ui| {
                    setting(ui, "Fit range", |ui| fill_field(ui, &mut node.gains));
                    setting(ui, "Level", |ui| {
                        ui.horizontal(|ui| {
                            node.level.ui(
                                ui,
                                "The percentile of the samples, ignoring sign",
                                "Its distance from the mean, times this, fills what's recorded",
                            );
                            info_icon(
                                ui,
                                "For each channel, computes the given percentile of its voltage (ignoring sign) over \
                                 the sampling time, takes the mean plus the factor times that percentile's distance \
                                 from the mean, and maps that value to the upper limit of the fit range.",
                            );
                        })
                        .response
                    });
                    setting(ui, "Sampling for", |ui| {
                        ui.add(egui::DragValue::new(&mut node.sampling).range(SAMPLING).speed(0.2).suffix(" s"))
                            .on_hover_text("How long to sample for")
                    });
                });
            });
            auto_detect_ui(ui, context, None, "Sample each channel, then set its gain");
            ui.add_space(4.0);
            input_summary(ui, context.inputs);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::ephys::Continuous;
    use ephys_rs::engine::{DataStreamType, NodeId, PASS_DURATION};

    #[test]
    fn multiplies_and_chooses_gains_from_the_data() {
        let level = PercentMultiple { percent: 98.0, multiple: 3.0 };
        let gain = ContinuousGain { sampling: 4.0, level, ..Default::default() };
        let stream = Continuous { channels: 2, sample_rate: 1000.0 };
        let mut state = gain.initial_state(&[Some(DataStream::Continuous(stream))]);
        let (triggers, direct) =
            (ephys_rs::pass_triggers::PassTriggers::default(), ephys_rs::triggers::DirectTriggers::default());
        triggers.trigger(
            NodeId(0),
            ephys_rs::triggers::AnyTrigger::new(crate::Trigger::AutoDetect(crate::standard::auto_detect::AutoDetect)),
        );
        let passes = (4.0 / PASS_DURATION).round() as u64;
        for sequence in 0..passes {
            // 100 µV either side of zero on the first channel, 300 on the second.
            let mut input = stream.buffer(PASS_DURATION);
            input.len = 20;
            for (c, size) in [(0, 100.0), (1, 300.0)] {
                for (t, x) in input.channel_mut(c).iter_mut().enumerate() {
                    *x = if t % 2 == 0 { -size } else { size };
                }
            }
            let input = Buffer::Continuous(input);
            let inputs = [Some(&input)];
            let mut outputs = [Buffer::Continuous(stream.buffer(PASS_DURATION))];
            let mut pass = Pass::new(sequence, PASS_DURATION, &inputs, &mut outputs);
            (pass.triggers, pass.direct) = (&triggers, &direct);
            gain.process(&mut pass, &mut state);
            let Buffer::Continuous(out) = &outputs[0] else { unreachable!() };
            let expected = -100.0 * gain.gains.get(0);
            assert_eq!(out.channel(0)[10], expected, "multiplied by the gain");
        }
        let updates = direct.take_settings_updates();
        let Some(Setting::Gains(gains)) = updates[0].2.downcast_ref::<Setting>() else { panic!() };
        // One for all, to begin with: the widest of theirs, so neither clips.
        assert!((gains.range_uv(0) - 900.0).abs() < 20.0 && (gains.range_uv(1) - 900.0).abs() < 20.0);
        // Wider than a quarter of a gain fits, it's the widest it can be.
        let mut gains = gains.clone();
        gains.set_ranges(&[1200.0, 1200.0], Channels { count: 2, group_size: 1 });
        assert_eq!(gains.range_uv(0), 1000.0);
    }
}
