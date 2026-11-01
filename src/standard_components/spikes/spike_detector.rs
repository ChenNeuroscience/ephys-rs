use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::standard_components::spikes::Threshold;
use crate::standard_components::spikes::{Spike, Spikes};
use crate::standard_components::widgets::{combo, input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Polarity {
    Negative,
    Positive,
    Both,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeDetector {
    /// In standard deviations of the noise.
    pub threshold: f32,
    pub polarity: Polarity,
    /// Waveform samples kept around each threshold crossing.
    pub waveform_samples: u32,
}

impl Default for SpikeDetector {
    fn default() -> Self {
        SpikeDetector { threshold: 4.0, polarity: Polarity::Negative, waveform_samples: 40 }
    }
}

impl Node<Edges> for SpikeDetector {
    const INFO: NodeInfo = NodeInfo {
        name: "Spike Detector",
        category: Category::Processing,
        description: "Finds threshold crossings on each channel and cuts out the waveform around them.",
    };

    type State = Detector;

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Detector {
        let Some(DataStream::Continuous(stream)) = inputs[0] else { return Detector::default() };
        let channels = stream.channels as usize;
        Detector {
            // A guess, refined as it runs.
            noise: vec![20.0; channels],
            wait: vec![0; channels],
            dead_time: (stream.sample_rate / 1000.0) as u32,
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, detector: &mut Detector) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let Some(Buffer::Spikes(output)) = pass.outputs.get_mut(0) else { return };
        output.spikes.clear();
        for channel in 0..input.channels.min(detector.noise.len()) {
            let (noise, wait) = (&mut detector.noise[channel], &mut detector.wait[channel]);
            for (sample, &x) in input.channel(channel).iter().enumerate() {
                // The noise's standard deviation, from its mean absolute value, adapting slowly.
                *noise += 0.0005 * (1.2533 * x.abs() - *noise);
                let threshold = self.threshold * *noise;
                let crossed = match self.polarity {
                    Polarity::Negative => x < -threshold,
                    Polarity::Positive => x > threshold,
                    Polarity::Both => x.abs() > threshold,
                };
                if *wait > 0 {
                    *wait -= 1;
                } else if crossed && output.spikes.len() < output.spikes.capacity() {
                    output.spikes.push(Spike { sample: sample as u32, channel: channel as u32 });
                    *wait = detector.dead_time;
                }
            }
        }
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Continuous(c)) => {
                Some(DataStream::Spikes(Spikes { channels: c.channels, sample_rate: c.sample_rate }))
            }
            _ => None,
        };
        vec![Output { name: "spikes", kind: Kind::Spikes, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "a (filtered) continuous data stream")
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "threshold", value: Threshold { sd: self.threshold }.into() }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Threshold(t) = value {
            self.threshold = t.sd.clamp(1.0, 20.0);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Threshold", |ui| {
                ui.add(egui::DragValue::new(&mut self.threshold).range(1.0..=20.0).speed(0.05).suffix(" SD"))
            });
            setting(ui, "Polarity", |ui| {
                combo(
                    ui,
                    "polarity",
                    &mut self.polarity,
                    &[(Polarity::Negative, "Negative"), (Polarity::Positive, "Positive"), (Polarity::Both, "Both")],
                )
            });
            setting(ui, "Waveform", |ui| {
                ui.add(egui::DragValue::new(&mut self.waveform_samples).range(8..=128).suffix(" samples"))
            });
        });
        input_summary(ui, context.inputs);
    }
}

/// Each channel's running estimate of its noise, and how many samples until it can spike again.
#[derive(Default)]
pub struct Detector {
    noise: Vec<f32>,
    wait: Vec<u32>,
    /// Samples after a spike before the next can be detected.
    dead_time: u32,
}
