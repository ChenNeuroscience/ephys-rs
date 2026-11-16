//! The Continuous Gain: multiplies each channel of continuous data (µV) by its gain, as an amplifier does (×2 doubles
//! it), into amplified µV: what's recorded (Write LTX's EEG's int8 values cover ±250 µV of them). Gains can be one for
//! all channels, or one each (see `Gains`). They can be chosen from the data when their auto detect's started (from its
//! window or a viewer's: see `Gains::detect`): its passes sample each channel for a while, then have its mean, plus a
//! multiple of a percentile's distance from it, fill what's recorded (`Gains::fill_uv`). Sign doesn't count.

use eframe::egui;
use ephys_rs::engine::{BufferUsage, Prepare, PrepareContext, Prepared, TreeShake};
use serde::{Deserialize, Serialize};

use crate::standard::auto_detect::{Distribution, PercentMultiple, SAMPLING, auto_detect_ui, detect_changed};
use crate::standard::spikes::{
    Channels, DEFAULT_RANGE_UV, DetectGains, GainScope, Gains, RANGES_UV, fill_field, gain_field,
};

use crate::standard::widgets::{input_summary, named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind, Setting};
use ephys_rs::engine::{Category, Input, Node, NodeInfo, Output, PassContext, SettingPort, UiContext};
use ephys_rs::gui::widgets::info_icon;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContinuousGain {
    pub gains: Gains,
    /// What's to fill what's recorded, when choosing them: this, of each channel's samples.
    pub level: PercentMultiple,
}

impl Default for ContinuousGain {
    fn default() -> Self {
        ContinuousGain {
            gains: Gains {
                scope: GainScope::All,
                detect: DetectGains { seconds: 10.0, ..Default::default() },
                ..Default::default()
            },
            level: PercentMultiple { percent: 98.0, multiple: 2.0 },
        }
    }
}

/// While choosing the gains from the data (an auto detect: see `crate::standard::auto_detect`): what's been added up of
/// each channel so far. Only while the gains are being detected.
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
    /// Each channel's samples (none, once it's done).
    data: Vec<Distribution>,
    /// Whether the gains have been chosen: `detect`'s still set in the snapshot, till the update choosing them (which
    /// also clears it) takes over.
    done: bool,
}

impl Sampling {
    fn new(seconds: f64, channels: usize) -> Self {
        Sampling { elapsed: 0.0, seconds, data: vec![Distribution::default(); channels], done: false }
    }

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
    type WorkerState = ();
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
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], _recording: bool) -> TreeShake {
        // And while detecting, whatever needs what it makes: its gains are wanted before recording, say.
        let wanted = outputs.contains(&true) || self.gains.detect.started.is_some();
        TreeShake::needing_all(inputs.contains(&true) && wanted, inputs.len())
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

    /// While the gains are being detected, each channel's room to sample into.
    fn prepare_for_snapshot(&self, context: &PrepareContext<Edges, Self>) -> Prepare<Gaining, (), Self> {
        let before = context.previous.as_ref().map(|previous| previous.node.gains.detect.started);
        if context.same_streams() && !detect_changed(self.gains.detect.started, before) {
            return Prepared::nothing().into();
        }
        let channels = match context.inputs[0] {
            Some(DataStream::Continuous(stream)) => stream.channels as usize,
            _ => 0,
        };
        let seconds = self.gains.detect.seconds.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
        let sampling = self.gains.detect.started.map(|_| Sampling::new(seconds, channels));
        Prepared::shared(Gaining { sampling }).into()
    }

    /// While the gains are being detected, samples each channel (in µV) for a while, then triggers an update to
    /// gains from it, with `detect` cleared; and multiplies each channel by its gain.
    fn process(&self, pass: &mut PassContext<Edges>, gaining: &mut Gaining, _: &mut ()) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        output.transform(input, |_, x| x);
        let channels = output.channels;
        if let Some(sampling) = gaining.sampling.as_mut().filter(|s| !s.done) {
            for (channel, data) in sampling.data.iter_mut().enumerate() {
                output.channel(channel).iter().for_each(|x| data.add(*x));
            }
            sampling.elapsed += pass.duration;
            let done = sampling.elapsed >= sampling.seconds;
            let layout = Channels { count: channels, group_size: 1 };
            let chosen = done.then(|| {
                let mut gains = self.gains.clone();
                // Done: no longer detecting.
                gains.detect.started = None;
                gains.set_ranges(&sampling.choose(self.level), layout);
                gains
            });
            if let Some(gains) = chosen {
                pass.trigger_settings_update(0, Setting::Gains(gains));
                *sampling = Sampling { done: true, ..Sampling::new(sampling.seconds, 0) };
            }
        }

        // The gains, applied.
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        for channel in 0..output.channels {
            let gain = self.gains.get(channel);
            output.channel_mut(channel).iter_mut().for_each(|x| *x *= gain);
        }
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<Edges>) {
        let Some(ui) = ui else { return };
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
            ui.add_enabled_ui(node.gains.detect.started.is_none(), |ui| {
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
                        ui.add(
                            egui::DragValue::new(&mut node.gains.detect.seconds)
                                .range(SAMPLING)
                                .speed(0.2)
                                .suffix(" s"),
                        )
                        .on_hover_text("How long to sample for")
                    });
                });
            });
            auto_detect_ui(
                ui,
                context,
                &mut node.gains.detect.started,
                node.gains.detect.seconds,
                "Sample each channel, then set its gain",
            );
            ui.add_space(4.0);
            input_summary(ui, context.inputs);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::ephys::Continuous;
    use ephys_rs::engine::{DataStreamType, PASS_DURATION};

    #[test]
    fn multiplies_and_chooses_gains_from_the_data() {
        let level = PercentMultiple { percent: 98.0, multiple: 3.0 };
        let mut gain = ContinuousGain { level, ..Default::default() };
        gain.gains.detect.seconds = 4.0;
        let stream = Continuous { channels: 2, sample_rate: 1000.0 };
        let started = std::time::Instant::now();
        let inputs = [Some(DataStream::Continuous(stream))];
        gain.gains.detect.started = Some(started);
        let mut state = ephys_rs::engine::fresh_state(&gain, &inputs);
        let triggers = ephys_rs::triggers::Triggers::default();
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
            let mut pass = PassContext::new(sequence, PASS_DURATION, &inputs, &mut outputs);
            pass.triggers = &triggers;
            gain.process(&mut pass, &mut state, &mut ());
            let Buffer::Continuous(out) = &outputs[0] else { unreachable!() };
            let expected = -100.0 * gain.gains.get(0);
            assert_eq!(out.channel(0)[10], expected, "multiplied by the gain");
        }
        let updates = triggers.take_settings_updates();
        let Some(Setting::Gains(gains)) = updates[0].2.downcast_ref::<Setting>() else { panic!() };
        // One for all, to begin with: the widest of theirs, so neither clips.
        assert!((gains.range_uv(0) - 900.0).abs() < 20.0 && (gains.range_uv(1) - 900.0).abs() < 20.0);
        // Wider than a quarter of a gain fits, it's the widest it can be.
        let mut gains = gains.clone();
        gains.set_ranges(&[1200.0, 1200.0], Channels { count: 2, group_size: 1 });
        assert_eq!(gains.range_uv(0), 1000.0);
    }
}
