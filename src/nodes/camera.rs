use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{combo, setting, settings_grid};
use crate::edges::signals::Video;
use crate::edges::{Edges, Kind, Signal};
use crate::engine::{Category, Context, Node, NodeInfo, Output};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub resolution: [u32; 2],
    pub fps: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { resolution: [640, 480], fps: 30.0 }
    }
}

impl Node<Edges> for Camera {
    const INFO: NodeInfo = NodeInfo {
        name: "Camera",
        category: Category::Source,
        description: "Video from a camera (simulated for now).",
    };

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        let [width, height] = self.resolution;
        vec![Output {
            name: "video",
            kind: Kind::Video,
            signal: Some(Signal::Video(Video { width, height, fps: self.fps })),
        }]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Resolution", |ui| {
                combo(
                    ui,
                    "resolution",
                    &mut self.resolution,
                    &[([640, 480], "640×480"), ([1280, 1024], "1280×1024"), ([1440, 1080], "1440×1080")],
                )
            });
            setting(ui, "Frame rate", |ui| {
                ui.add(egui::DragValue::new(&mut self.fps).range(1.0..=240.0).speed(0.5).suffix(" fps"))
            });
        });
    }
}
