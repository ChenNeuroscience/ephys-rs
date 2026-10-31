use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{input_summary, require_input, setting, settings_grid};
use crate::edges::signals::format_rate;
use crate::edges::{Edges, Kind, Signal};
use crate::engine::{Category, Context, Input, Node, NodeInfo, Output};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandPassFilter {
    pub low_hz: f32,
    pub high_hz: f32,
}

impl Default for BandPassFilter {
    fn default() -> Self {
        BandPassFilter { low_hz: 300.0, high_hz: 6000.0 }
    }
}

impl Node<Edges> for BandPassFilter {
    const INFO: NodeInfo = NodeInfo {
        name: "Band-pass Filter",
        category: Category::Processing,
        description: "Keeps frequencies between the low and high cut-offs, on every channel.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        vec![Output { name: "filtered", kind: Kind::Continuous, signal: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous signal");
        if self.low_hz >= self.high_hz {
            warnings.push("The low cut-off must be below the high cut-off".into());
        }
        if let Some(Signal::Continuous(c)) = inputs[0]
            && self.high_hz >= c.sample_rate / 2.0
        {
            warnings.push(format!(
                "The high cut-off must be below {} (half the sample rate)",
                format_rate(c.sample_rate / 2.0)
            ));
        }
        warnings
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Low cut-off", |ui| {
                ui.add(egui::DragValue::new(&mut self.low_hz).range(0.1..=20000.0).speed(1.0).suffix(" Hz"))
            });
            setting(ui, "High cut-off", |ui| {
                ui.add(egui::DragValue::new(&mut self.high_hz).range(1.0..=20000.0).speed(10.0).suffix(" Hz"))
            });
        });
        input_summary(ui, context.inputs);
    }
}
