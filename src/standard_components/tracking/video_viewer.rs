use eframe::egui::{self, Align2, Color32, FontId, Stroke};
use serde::{Deserialize, Serialize};

use super::tracker_view::{FrameView, show_frame};
use crate::core::engine::{
    Category, Context, DataStreamType, EdgeType, Input, Node, NodeInfo, Output, PASS_DURATION, Pass,
};
use crate::standard_components::pool::{Pool, SharedPool, WINDOW_BUFFERS};
use crate::standard_components::preview;
use crate::standard_components::tracking::VideoBuffer;
use crate::standard_components::tracking::overlay::Overlays;
use crate::standard_components::widgets::require_input;
use crate::{Buffer, DataStream, Edges, Kind};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoViewer {
    pub crosshair: bool,
}

/// Where it sends the latest frames to its window: their buffers, swapped in (so a pool of buffers like the pass's).
#[derive(Default)]
pub struct Showing(Option<SharedPool<VideoBuffer>>);

impl crate::core::engine::PassState for Showing {
    const WHY: &'static str = "Only where it sends the latest frames to its window, kept so it's found once: \
                               working memory, so each worker has its own.";
    const CARRIED_OVER: bool = false;
}

impl Node<Edges> for VideoViewer {
    const INFO: NodeInfo =
        NodeInfo { name: "Video Viewer", category: Category::Visualisation, description: "The latest video frame." };
    const HAS_VIEW: bool = true;
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    type State = Showing;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "video", kind: Kind::Video }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "video")
    }

    /// Takes the pass's frames, to show the last.
    fn takes(&self, _input: usize) -> bool {
        true
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Showing {
        let Some(DataStream::Video(stream)) = inputs[0] else { return Showing::default() };
        Showing(Some(SharedPool::new(WINDOW_BUFFERS, move || stream.buffer(PASS_DURATION))))
    }

    /// Swaps the pass's frames (if it has any) into the pool its window reads from: nothing's copied.
    fn process(&self, pass: &mut Pass<Edges>, showing: &mut Showing) {
        let (Some(sender), Some(Buffer::Video(video))) = (&mut showing.0, pass.input(0)) else { return };
        if video.frames == 0 {
            return;
        }
        let sequence = pass.sequence;
        let pool = sender.pool(pass);
        let Some(mut frames) = pool.reuse() else { return };
        if let Some(Buffer::Video(video)) = pass.take_input(0) {
            std::mem::swap(video, &mut frames);
        }
        pool.send_as_of(sequence, frames);
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        ui.checkbox(&mut self.crosshair, "Crosshair");
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let locked = context.locked;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!locked, |ui| ui.checkbox(&mut self.crosshair, "Crosshair"));
            context.node_toolbar(ui);
        });
        let Some(DataStream::Video(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect video to view it");
        };
        // The last of the latest frames sent.
        let pool = context.shared::<Pool<VideoBuffer>>();
        let Some(frames) = pool.as_deref().and_then(Pool::newest).filter(|f| f.frames > 0 && f.width > 0) else {
            let message = if context.running { "Waiting for frames…" } else { "Press Play to see the video" };
            return preview::unconnected(ui, context.theme, message);
        };
        let last = frames.frames - 1;
        let frame =
            FrameView { width: frames.width, height: frames.height, pixels: frames.frame(last), id: frames.ids[last] };
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
        preview::repaint_with_data(ui, context.running);
    }
}
