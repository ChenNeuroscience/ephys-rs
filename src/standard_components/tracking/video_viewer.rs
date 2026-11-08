use crate::core::engine::BufferUsage;
use eframe::egui::{self, Align2, Color32, FontId, Stroke};
use serde::{Deserialize, Serialize};

use super::tracker_view::{FrameView, show_frame};
use crate::core::engine::{
    Category, DataStreamType, EdgeType, Input, Node, NodeInfo, Output, PASS_DURATION, Pass, UiContext,
};
use crate::standard_components::pool::{Pool, WINDOW_BUFFERS};
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

impl Node<Edges> for VideoViewer {
    const INFO: NodeInfo =
        NodeInfo { name: "Video Viewer", category: Category::Visualisation, description: "The latest video frame." };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    type State = ();
    /// The latest frames, for its window: their buffers, swapped in (so a pool of buffers like the pass's).
    type Shared = Pool<VideoBuffer>;
    type UiState = ();

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
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage {
        BufferUsage { takes: vec![true; inputs], ..BufferUsage::none(inputs, outputs) }
    }

    /// Buffers like the pass's, to swap for them.
    fn shared(&self, inputs: &[Option<DataStream>]) -> Pool<VideoBuffer> {
        let Some(DataStream::Video(stream)) = inputs[0] else { return Pool::default() };
        Pool::new((0..WINDOW_BUFFERS).map(|_| stream.buffer(PASS_DURATION)), 0)
    }

    /// Swaps the pass's frames (if it has any) into the pool its window reads from: nothing's copied.
    fn process(&self, pass: &mut Pass<Edges>, _: &mut ()) {
        let Some(Buffer::Video(video)) = pass.input(0) else { return };
        if video.frames == 0 {
            return;
        }
        let (sequence, pool) = (pass.sequence, pass.shared::<Pool<VideoBuffer>>());
        let Some(mut frames) = pool.reuse() else { return };
        if let Some(Buffer::Video(video)) = pass.take_input(0) {
            std::mem::swap(video, &mut frames);
        }
        pool.send_as_of(sequence, frames);
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
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
        let Some(frames) = pool.and_then(Pool::newest).filter(|f| f.frames > 0 && f.width > 0) else {
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
