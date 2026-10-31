use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{input_summary, require_input, setting, settings_grid};
use crate::edges::settings;
use crate::edges::{Edges, Kind, Setting, Signal};
use crate::engine::{Category, Context, Input, Node, NodeInfo, Output, SettingPort};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gain {
    pub gain: f32,
    pub invert: bool,
}

impl Default for Gain {
    fn default() -> Self {
        Gain { gain: 1.0, invert: false }
    }
}

impl Node<Edges> for Gain {
    const INFO: NodeInfo =
        NodeInfo { name: "Gain", category: Category::Processing, description: "Scales every channel by a constant." };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        vec![Output { name: "scaled", kind: Kind::Continuous, signal: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        require_input(inputs, "a continuous signal")
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "gain", value: settings::Gain { factor: self.gain }.into() }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Gain(g) = value {
            self.gain = g.factor.clamp(0.0, 1000.0);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Gain", |ui| {
                ui.add(egui::DragValue::new(&mut self.gain).range(0.0..=1000.0).speed(0.01).prefix("×"))
            });
            setting(ui, "Invert", |ui| ui.checkbox(&mut self.invert, ""));
        });
        input_summary(ui, context.inputs);
    }
}
