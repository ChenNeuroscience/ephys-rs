use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{combo, require_input, setting, settings_grid};
use crate::edges::{Edges, Kind, Signal};
use crate::engine::{Category, Context, EdgeSet, Input, Node, NodeInfo, Output};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Format {
    Binary,
    Nwb,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Writer {
    pub directory: String,
    pub prefix: String,
    pub format: Format,
}

impl Default for Writer {
    fn default() -> Self {
        Writer { directory: "~/recordings".into(), prefix: "session".into(), format: Format::Binary }
    }
}

impl Node<Edges> for Writer {
    const INFO: NodeInfo = NodeInfo {
        name: "Writer",
        category: Category::Recording,
        description: "Records whatever is connected: raw continuous data, events, spikes and video.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![
            Input { name: "continuous", kind: Kind::Continuous },
            Input { name: "events", kind: Kind::Events },
            Input { name: "spikes", kind: Kind::Spikes },
            Input { name: "video", kind: Kind::Video },
        ]
    }

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        require_input(inputs, "something to record")
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Directory", |ui| ui.text_edit_singleline(&mut self.directory));
            setting(ui, "File prefix", |ui| ui.text_edit_singleline(&mut self.prefix));
            setting(ui, "Format", |ui| {
                combo(ui, "format", &mut self.format, &[(Format::Binary, "Binary"), (Format::Nwb, "NWB")])
            });
        });
        ui.add_space(4.0);
        ui.label("Will record:");
        let mut any = false;
        for (input, signal) in self.inputs().iter().zip(context.inputs) {
            if let Some(signal) = signal {
                ui.weak(format!("  {}: {}", input.name, signal.summary()));
                any = true;
            }
        }
        if !any {
            ui.weak("  nothing yet");
        }
    }
}
