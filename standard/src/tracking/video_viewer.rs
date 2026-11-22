use eframe::egui::{self, Align2, Color32, FontId, Stroke};
use serde::{Deserialize, Serialize};

use super::tracker_view::{FrameView, show_frame};
use crate::standard::plots;
use crate::standard::pool::{Pool, WINDOW_BUFFERS};
use crate::standard::tracking::overlay::Overlays;
use crate::standard::tracking::{Video, VideoBuffer};
use crate::standard::widgets::require_input;
use ephys_rs::engine::{
    Buffers, Category, CommandContext, CommandSource, DataStreamType, EdgeType, NoPorts, Node, NodeInfo, PASS_DURATION,
    PassContext, UiContext,
};

ephys_rs::ports! {
    /// The viewer takes each pass's frames, and shows the most recent one.
    video: DataStreamIn<Video> take,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoViewer {
    pub crosshair: bool,
}

impl<E: PortEdges> Node<E> for VideoViewer {
    type Ports = Ports;
    const INFO: NodeInfo =
        NodeInfo { name: "Video Viewer", category: Category::Visualisation, description: "The latest video frame." };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    type State = ();
    type WorkerState = ();
    /// The latest frames, for its window: their buffers, swapped in (so a pool of buffers like the pass's).
    type Shared = Pool<VideoBuffer>;
    type UiState = ();
    type GlobalSettings = ();

    fn val_out(&self) -> NoPorts {
        NoPorts
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {}
    }

    fn output_streams(&self, _inputs: &DataStreamIn) -> NoPorts {
        NoPorts
    }

    /// Buffers like the pass's, to swap for them.
    fn shared(&self, inputs: &DataStreamIn) -> Pool<VideoBuffer> {
        let Some(stream) = inputs.video else { return Pool::default() };
        Pool::new((0..WINDOW_BUFFERS).map(|_| stream.buffer(PASS_DURATION)), 0)
    }

    /// Swaps the pass's frames (if it has any) into the pool its window reads from: nothing's copied.
    fn process(&self, pass: &mut PassContext<E>, inputs: DataStreamIn<Buffers>, _: NoPorts, _: &mut (), _: &mut ()) {
        let Some(video) = inputs.video else { return };
        if video.frames == 0 {
            return;
        }
        let (sequence, pool) = (pass.sequence, pass.shared::<Pool<VideoBuffer>>());
        let Some(mut frames) = pool.reuse() else { return };
        std::mem::swap(video, &mut frames);
        pool.send_as_of(sequence, frames);
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        let locked = context.locked;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!locked, |ui| ui.checkbox(&mut self.crosshair, "Crosshair"));
            context.node_toolbar(ui);
        });
        let Some(stream) = context.data_stream_in::<DataStreamIn>().video else {
            plots::unconnected(ui, context.theme, "Connect video to view it");
            return;
        };
        // The last of the latest frames sent.
        let pool = context.shared::<Pool<VideoBuffer>>();
        let Some(frames) = pool.and_then(Pool::newest).filter(|f| f.frames > 0 && f.width > 0) else {
            let message = if context.running { "Waiting for frames…" } else { "Press Play to see the video" };
            plots::unconnected(ui, context.theme, message);
            return;
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
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        require_input(inputs.any_connected(), "video")
    }
}
