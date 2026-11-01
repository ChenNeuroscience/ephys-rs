use eframe::egui::{self, Align2, Color32, FontId, Stroke};
use serde::{Deserialize, Serialize};

use super::tracker_view::{Frame, show_frame};
use crate::core::engine::{Category, Context, CopyTo, EdgeType, Input, Node, NodeInfo, Output, Pass};
use crate::core::exchange::Latest;
use crate::standard_components::preview;
use crate::standard_components::tracking::overlay::Overlays;
use crate::standard_components::widgets::require_input;
use crate::{Buffer, DataStream, Edges, Kind};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoViewer {
    pub crosshair: bool,
}

impl Node<Edges> for VideoViewer {
    const INFO: NodeInfo =
        NodeInfo { name: "Video Viewer", category: Category::Visualisation, description: "The latest video frame." };
    const HAS_VIEW: bool = true;

    /// The latest frame, for the window.
    type State = Latest<Frame>;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "video", kind: Kind::Video }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "video")
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Window)
    }

    /// Keeps the pass's last frame.
    fn process(&self, pass: &mut Pass<Edges>, shown: &mut Latest<Frame>) {
        let Some(Buffer::Video(video)) = pass.inputs[0] else { return };
        if video.frames > 0 {
            shown.send(pass, |frame| frame.copy_from(video, video.frames - 1));
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        ui.checkbox(&mut self.crosshair, "Crosshair");
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let Some(DataStream::Video(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect video to view it");
        };
        let Some(frame) = context.display::<Frame>().filter(|f| f.width > 0) else {
            let message = if context.running { "Waiting for frames…" } else { "Press Play to see the video" };
            return preview::unconnected(ui, context.theme, message);
        };
        let (rect, _) = show_frame(ui, frame, None, &Overlays::default(), context.theme);
        let painter = ui.painter_at(rect);
        if self.crosshair {
            let stroke = Stroke::new(1.0, context.theme.text_secondary);
            painter.hline(rect.x_range(), rect.center().y, stroke);
            painter.vline(rect.center().x, rect.y_range(), stroke);
        }
        let text = format!("{} · frame {}", stream.summary(), frame.id);
        painter.text(
            rect.left_top() + egui::vec2(6.0, 4.0),
            Align2::LEFT_TOP,
            text,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
        ui.ctx().request_repaint();
    }
}
