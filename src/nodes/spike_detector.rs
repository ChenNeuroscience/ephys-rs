use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{combo, input_summary, require_input, setting, settings_grid};
use crate::edges::settings::Threshold;
use crate::edges::signals::Spikes;
use crate::edges::{Edges, Kind, Setting, Signal};
use crate::engine::{Category, Context, Input, Node, NodeInfo, Output, SettingPort};

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

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        let signal = match inputs[0] {
            Some(Signal::Continuous(c)) => {
                Some(Signal::Spikes(Spikes { channels: c.channels, sample_rate: c.sample_rate }))
            }
            _ => None,
        };
        vec![Output { name: "spikes", kind: Kind::Spikes, signal }]
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        require_input(inputs, "a (filtered) continuous signal")
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
