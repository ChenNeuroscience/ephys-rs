use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, EdgeSet, Input, Node, NodeInfo, Output};
use crate::standard_components::widgets::{combo, require_input, setting, settings_grid};
use crate::{DataStream, Edges, Kind};

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
    type State = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Writer",
        category: Category::Recording,
        description: "Records whatever is connected: raw continuous data, events, spikes, video and positions.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![
            Input { name: "continuous", kind: Kind::Continuous },
            Input { name: "events", kind: Kind::Events },
            Input { name: "spikes", kind: Kind::Spikes },
            Input { name: "video", kind: Kind::Video },
            Input { name: "positions", kind: Kind::Positions },
        ]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
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
        for (input, stream) in self.inputs().iter().zip(context.inputs) {
            if let Some(stream) = stream {
                ui.weak(format!("  {}: {}", input.name, stream.summary()));
                any = true;
            }
        }
        if !any {
            ui.weak("  nothing yet");
        }
    }
}
