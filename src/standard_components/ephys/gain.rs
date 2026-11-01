use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::standard_components::ephys;
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

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

    type State = ();

    fn in_place(&self, _output: usize) -> Option<usize> {
        Some(0)
    }

    fn process(&self, pass: &mut Pass<Edges>, _state: &mut ()) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        let factor = if self.invert { -self.gain } else { self.gain };
        output.transform(input, |_, x| x * factor);
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

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "gain", value: ephys::Gain { factor: self.gain }.into() }]
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
