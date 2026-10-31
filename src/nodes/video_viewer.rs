use eframe::egui::{self, Align2, Color32, FontId, Rect, Stroke};
use serde::{Deserialize, Serialize};

use super::{preview, require_input};
use crate::edges::{Edges, Kind, Signal};
use crate::engine::{Category, Context, EdgeType, Input, Node, NodeInfo, Output};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoViewer {
    pub crosshair: bool,
}

impl Node<Edges> for VideoViewer {
    const INFO: NodeInfo =
        NodeInfo { name: "Video Viewer", category: Category::Visualisation, description: "The latest video frame." };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "video", kind: Kind::Video }]
    }

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        require_input(inputs, "video")
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        ui.checkbox(&mut self.crosshair, "Crosshair");
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let Some(Signal::Video(signal)) = context.inputs[0] else {
            return preview::unconnected(ui, "Connect video to view it");
        };
        let area = preview::plot_area(ui);
        let aspect = signal.width as f32 / signal.height as f32;
        let size = if area.width() / area.height() > aspect {
            egui::vec2(area.height() * aspect, area.height())
        } else {
            egui::vec2(area.width(), area.width() / aspect)
        };
        let frame = Rect::from_center_size(area.center(), size);
        let painter = ui.painter_at(area);
        painter.rect_filled(frame, 0.0, Color32::from_gray(32));
        // Simulated: an LED wandering round the arena.
        let t = ui.input(|i| i.time) as f32;
        let led = frame.center() + egui::vec2((t * 0.7).sin() * 0.35 * size.x, (t * 0.53).cos() * 0.35 * size.y);
        painter.circle_filled(led, 6.0, Color32::from_rgb(120, 255, 140));
        if self.crosshair {
            let stroke = Stroke::new(1.0, Color32::from_gray(90));
            painter.hline(frame.x_range(), frame.center().y, stroke);
            painter.vline(frame.center().x, frame.y_range(), stroke);
        }
        painter.text(
            frame.left_top() + egui::vec2(6.0, 4.0),
            Align2::LEFT_TOP,
            signal.summary(),
            FontId::proportional(11.0),
            Color32::from_gray(150),
        );
        ui.ctx().request_repaint();
    }
}
