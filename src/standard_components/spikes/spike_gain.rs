//! The Spike Gain: each channel's gain for spikes (see `Gains`): what their waveforms are multiplied by, for whatever
//! takes them, up to a magnitude (±128, for Write LTX's 8-bit samples; ±1 for floats, say), so the range of µV each
//! channel's waveforms fit in, which the Spike Viewer shows. It passes spikes through, setting their gains, and can
//! choose the gains from the data when it's triggered ("auto detect", from its window or a Spike Viewer's): its passes
//! sample each channel's spikes for a while, then have the bigger of them reach a target share of the range.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, EdgeType, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::core::exchange::Latest;
use crate::standard_components::spikes::{DEFAULT_RANGE_UV, Gains, RANGES_UV, SpikesBuffer};
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
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
    /// How far up the range (in percent) a channel's bigger spikes reach, with gains chosen from the data.
    pub target: f32,
}

impl Default for SpikeGain {
    fn default() -> Self {
        SpikeGain { gains: Gains::default(), sampling: 10.0, target: 95.0 }
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
        description: "Each channel's gain for spikes, up to a magnitude (±128 for 8-bit samples, say), so the range \
                      they're recorded and viewed over: set by hand, from a Spike Viewer, or chosen from the data.",
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

    /// Sets the spikes' gains; and, when triggered, samples the spikes for a while, then proposes gains from them.
    fn process(&self, pass: &mut Pass<Edges>, gaining: &mut Gaining) {
        if let Some(Some(Buffer::Spikes(input))) = pass.inputs.first()
            && let Some(Buffer::Spikes(output)) = pass.outputs.get_mut(0)
        {
            copy(input, output);
        }
        let Some(Buffer::Spikes(spikes)) = pass.outputs.first_mut() else { return };
        for (channel, gain) in spikes.gains.iter_mut().enumerate() {
            *gain = self.gains.get(channel);
        }
        spikes.magnitude = self.gains.magnitude;
        let channels = spikes.gains.len();
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
        let Some(Buffer::Spikes(spikes)) = pass.outputs.first() else { return };
        if let Some(sampling) = &mut gaining.sampling {
            sampling.add(spikes, pass.duration);
            pass.trigger_progress(AUTO_DETECT, (sampling.elapsed / sampling.seconds) as f32);
            if sampling.elapsed >= sampling.seconds {
                let mut gains = self.gains.clone();
                for (channel, range) in sampling.choose(&spikes.thresholds, self.target / 100.0).into_iter().enumerate()
                {
                    gains.set_range(channel, range);
                }
                pass.propose(0, Setting::Gains(gains));
                pass.trigger_done(AUTO_DETECT);
                gaining.sampling = None;
            }
        }
        let progress = gaining.sampling.as_ref().map(|s| (s.elapsed as f32, s.seconds as f32));
        gaining.shown.send(pass, |shown| *shown = progress);
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Gains", |ui| ui.label(self.gains.summary()));
            setting(ui, "Output range", |ui| {
                let mut magnitude = self.gains.magnitude;
                let field = egui::DragValue::new(&mut magnitude).range(1e-6..=1e9).speed(1.0).prefix("±");
                let response = ui.add(field).on_hover_text(
                    "What the gains map each channel's range to, in the units of what takes the spikes: ±128 for \
                         Write LTX's 8-bit samples, ±1 for floats, say. Changing it keeps the ranges",
                );
                if response.changed() {
                    self.gains.set_magnitude(magnitude);
                }
                response
            });
            setting(ui, "Auto detect over", |ui| {
                ui.add(egui::DragValue::new(&mut self.sampling).range(SAMPLING).speed(0.2).suffix(" s"))
                    .on_hover_text("How long to sample the spikes for")
            });
            setting(ui, "Target", |ui| {
                ui.add(egui::DragValue::new(&mut self.target).range(20.0..=100.0).speed(0.5).suffix(" % of range"))
                    .on_hover_text(
                        "How far up the recording's 8-bit range each channel's bigger spikes reach (the 98th \
                         percentile's biggest excursion), with gains chosen from the data",
                    )
            });
        });
        auto_detect_ui(
            ui,
            context,
            None,
            "Sample the spikes, then set each channel's gain so its bigger ones fill the range",
        );
        let channels = match context.inputs[0] {
            Some(DataStream::Spikes(s)) => s.channels as usize,
            _ => 0,
        };
        if channels > 0 {
            egui::CollapsingHeader::new("Each channel")
                .id_salt("each channel")
                .show(ui, |ui| {
                    egui::Grid::new("gains").num_columns(3).striped(true).show(ui, |ui| {
                        ui.strong("Channel");
                        ui.strong("Range");
                        ui.strong("Gain");
                        ui.end_row();
                        for channel in 0..channels {
                            ui.label(format!("{}", channel + 1));
                            let mut range = self.gains.range_uv(channel);
                            let field =
                                egui::DragValue::new(&mut range).range(RANGES_UV).speed(1.0).prefix("±").suffix(" µV");
                            if ui.add(field).changed() {
                                self.gains.set_range(channel, range);
                            }
                            ui.label(format!("×{:.4}", self.gains.get(channel)));
                            ui.end_row();
                        }
                    });
                })
                .header_response
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        input_summary(ui, context.inputs);
    }
}

/// A button to fire the auto-detect trigger (of the node's own settings, or of the setting `control` connects to),
/// and how it's going: for this node's window, and a viewer's.
pub fn auto_detect_ui(ui: &mut egui::Ui, context: &mut Context<Edges>, control: Option<usize>, tip: &str) {
    ui.horizontal(|ui| {
        let progress = context.trigger_progress(control, AUTO_DETECT);
        let can = context.can_trigger(control, AUTO_DETECT) && progress.is_none() && !context.locked;
        let why = if context.locked {
            "Not while recording"
        } else if !context.running {
            "Only while acquiring: it needs data"
        } else {
            "Detecting…"
        };
        if ui
            .add_enabled(can, egui::Button::new("Auto detect"))
            .on_hover_text(tip)
            .on_disabled_hover_text(why)
            .clicked()
        {
            context.trigger(control, AUTO_DETECT);
        }
        if let Some(progress) = progress {
            ui.add(egui::ProgressBar::new(progress).desired_width(80.0).show_percentage());
        }
    });
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
    /// excursion) reach `target` of it, with room for its threshold (at most two-thirds of the way up); with too few
    /// spikes to tell, it's eight standard deviations of its noise.
    fn choose(&mut self, thresholds: &[f32], target: f32) -> Vec<f32> {
        let passes = self.passes.max(1) as f64;
        self.peaks
            .iter_mut()
            .zip(&self.noise)
            .zip(thresholds)
            .map(|((peaks, &noise), &threshold)| {
                let noise = (noise / passes) as f32;
                let biggest = if peaks.len() >= 10 {
                    peaks.sort_unstable_by(f32::total_cmp);
                    peaks[(peaks.len() - 1) * 98 / 100] / target.max(0.01)
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
        assert!(close(gains.range_uv(0), 100.0 / 0.95), "the first channel's spikes reach 95%");
        assert!(close(gains.range_uv(1), 500.0 / 0.95), "the second's biggest");
        assert!(close(gains.range_uv(2), 120.0), "spikes no bigger than 0: room for its threshold");
        assert!(close(gains.range_uv(3), 75.0), "and for the default threshold, with no noise");
        assert_eq!(gains.magnitude, 128.0, "for 8-bit samples, to begin with");
    }
}
