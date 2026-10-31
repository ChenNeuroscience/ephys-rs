use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{combo, setting, settings_grid};
use crate::edges::signals::Video;
use crate::edges::{Buffer, Edges, Kind, Signal};
use crate::engine::{Category, Context, Node, NodeInfo, Output, Pass};

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

    type State = ();

    /// Simulated: an LED wandering round a dark arena.
    fn process(&self, pass: &mut Pass<Edges>, _state: &mut ()) {
        let Some(Buffer::Video(out)) = pass.outputs.get_mut(0) else { return };
        let frame_at = |sequence: u64| (sequence as f64 * pass.duration * self.fps as f64).floor() as u64;
        let first = frame_at(pass.sequence);
        out.frames = ((frame_at(pass.sequence + 1) - first) as usize).min(out.capacity);
        let (width, height) = (out.width, out.height);
        for frame in 0..out.frames {
            let t = (first + frame as u64) as f32 / self.fps;
            let pixels = out.frame_mut(frame);
            pixels.fill(40);
            let (cx, cy) =
                (width as f32 * (0.5 + 0.35 * (t * 0.7).sin()), height as f32 * (0.5 + 0.35 * (t * 0.53).cos()));
            let radius = 6.0f32;
            let rows = (cy - radius).max(0.0) as usize..((cy + radius) as usize + 1).min(height);
            let columns = (cx - radius).max(0.0) as usize..((cx + radius) as usize + 1).min(width);
            for y in rows {
                for x in columns.clone() {
                    if (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2) <= radius * radius {
                        pixels[y * width + x] = 255;
                    }
                }
            }
        }
    }

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
