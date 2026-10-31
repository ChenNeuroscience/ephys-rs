use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{combo, setting, settings_grid};
use crate::edges::signals::{Continuous, Events};
use crate::edges::{Edges, Kind, Signal};
use crate::engine::{Category, Context, Node, NodeInfo, Output};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AcquisitionBoard {
    pub channels: u32,
    pub sample_rate: f32,
    pub ttl_lines: u32,
}

impl Default for AcquisitionBoard {
    fn default() -> Self {
        AcquisitionBoard { channels: 64, sample_rate: 30000.0, ttl_lines: 8 }
    }
}

impl Node<Edges> for AcquisitionBoard {
    const INFO: NodeInfo = NodeInfo {
        name: "Acquisition Board",
        category: Category::Source,
        description: "Headstage channels and digital inputs from an acquisition board (simulated for now).",
    };

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        vec![
            Output {
                name: "continuous",
                kind: Kind::Continuous,
                signal: Some(Signal::Continuous(Continuous { channels: self.channels, sample_rate: self.sample_rate })),
            },
            Output {
                name: "events",
                kind: Kind::Events,
                signal: Some(Signal::Events(Events { lines: self.ttl_lines })),
            },
        ]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Channels", |ui| {
                combo(
                    ui,
                    "channels",
                    &mut self.channels,
                    &[(16, "16"), (32, "32"), (64, "64"), (128, "128"), (256, "256")],
                )
            });
            setting(ui, "Sample rate", |ui| {
                combo(
                    ui,
                    "rate",
                    &mut self.sample_rate,
                    &[(20000.0, "20 kHz"), (25000.0, "25 kHz"), (30000.0, "30 kHz")],
                )
            });
            setting(ui, "TTL lines", |ui| ui.add(egui::DragValue::new(&mut self.ttl_lines).range(0..=16)));
        });
    }
}
