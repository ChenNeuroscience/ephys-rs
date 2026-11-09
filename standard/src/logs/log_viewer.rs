//! The Log Viewer: log lines as they come, each with its time, in seconds by its source's clock, just as it came. The
//! passes send each line to the GUI through a pool (see `Pool::send_pending`); its work on the GUI's thread takes them
//! as they come, window open or not, keeping up to `KEPT` (until they're cleared), and the window draws only those in
//! view.

use std::collections::VecDeque;
use std::time::Instant;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::standard::plots;
use crate::standard::pool::Pool;
use crate::standard::widgets::require_input;
use crate::{Buffer, DataStream, Edges, Kind};
use ephys_rs::engine::{Category, Input, Node, NodeInfo, Output, Pass, PassState, PoolStatus, Shared, UiContext};

/// The most lines kept: the oldest go.
const KEPT: usize = 10_000;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogViewer {}

/// A line, and its time.
pub struct Entry {
    pub time: f64,
    pub line: String,
}

/// The lines its passes send the GUI, every one, in order.
#[derive(Default)]
pub struct SentLines(Pool<Vec<Entry>>);

impl Shared for SentLines {
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        vec![("log lines", self.0.status())]
    }
}

/// The lines kept, on the GUI's side.
#[derive(Default)]
pub struct KeptLines(VecDeque<Entry>);

impl KeptLines {
    /// Lines in the order they came, for tests.
    #[cfg(test)]
    pub fn lines(&self) -> Vec<String> {
        self.0.iter().map(|e| e.line.clone()).collect()
    }
}

/// While running: the lines not sent yet.
#[derive(Default)]
pub struct Viewing {
    pending: Vec<Entry>,
}

impl PassState for Viewing {
    const WHY: &'static str = "The lines the GUI hasn't been sent yet, which each pass adds to, in order.";
}

impl Node<Edges> for LogViewer {
    type State = Viewing;
    type Shared = SentLines;
    type UiState = KeptLines;
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Log Viewer",
        category: Category::Visualisation,
        description: "Log lines as they come, each with its time.",
    };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "log", kind: Kind::LogLines }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "log lines")
    }

    fn process(&self, pass: &mut Pass<Edges>, viewing: &mut Viewing) {
        let pending = &mut viewing.pending;
        if let Some(Some(Buffer::LogLines(log))) = pass.inputs.first() {
            pending.extend(
                log.lines().map(|(time, line)| Entry { time, line: String::from_utf8_lossy(line).into_owned() }),
            );
            // At most `KEPT` waiting, if the GUI isn't taking them.
            if pending.len() > 2 * KEPT {
                pending.drain(..pending.len() - KEPT);
            }
        }
        pass.shared::<SentLines>().0.send_pending(pending);
    }

    /// Takes the lines sent, keeping the latest.
    fn gui_background(&mut self, context: &mut UiContext<Edges>) -> Option<Instant> {
        let sent = context.shared::<SentLines>()?;
        let mut kept = context.ui_state::<KeptLines>();
        sent.0.receive_all(|entries| {
            kept.0.extend(entries);
            let excess = kept.0.len().saturating_sub(KEPT);
            kept.0.drain(..excess);
        });
        None
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let mut kept = context.ui_state::<KeptLines>();
        let entries = &mut kept.0;
        ui.horizontal(|ui| {
            ui.weak(format!("{} lines", entries.len()));
            if ui.add_enabled(!entries.is_empty(), egui::Button::new("Clear")).clicked() {
                entries.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| context.node_toolbar(ui));
        });
        if entries.is_empty() {
            let message = if context.running { "No log lines yet" } else { "Press Play to see the log" };
            return plots::unconnected(ui, context.theme, message);
        }
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let scroll = egui::ScrollArea::both().auto_shrink(false).stick_to_bottom(true);
        scroll.show_rows(ui, row_height, entries.len(), |ui, rows| {
            for entry in entries.range(rows) {
                ui.horizontal(|ui| {
                    let time = format!("{:.3}", entry.time);
                    ui.label(RichText::new(time).monospace().color(context.theme.text_secondary));
                    ui.add(egui::Label::new(RichText::new(&entry.line).monospace()).extend());
                });
            }
        });
        plots::repaint_with_data(ui, context.running);
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::RefCell;

    use super::*;
    use crate::Nodes;
    use crate::standard::pool::WINDOW_BUFFERS;
    use crate::standard::themes::THEMES;
    use egui_kittest::kittest::Queryable;
    use ephys_rs::engine::{Graph, NodeId, NodesEnum, SharedByNode};

    /// Runs a pass of the viewer, sending `lines` to `sent`.
    fn pass(viewing: &mut Viewing, sent: &SentLines, lines: &[(f64, String)]) {
        use ephys_rs::engine::DataStreamType;
        let mut log = crate::standard::logs::LogLines { width: 64 }.buffer(0.02);
        for (time, line) in lines {
            assert!(log.push(*time, line));
        }
        let input = Buffer::LogLines(log);
        let inputs = [Some(&input)];
        let mut pass = Pass::new(0, 0.02, &inputs, &mut []);
        pass.shared = sent;
        LogViewer::default().process(&mut pass, viewing);
    }

    /// A viewer on the GUI's side: its graph, what its passes share, and its state.
    struct Gui {
        graph: Graph<Nodes>,
        viewer: NodeId,
        shared: SharedByNode,
        state: RefCell<Box<dyn Any>>,
    }

    impl Gui {
        fn new(sent: SentLines) -> Self {
            let mut graph = Graph::default();
            let viewer = graph.add(Nodes::LogViewer(LogViewer::default()), [0.0, 0.0]);
            let shared = SharedByNode::default().with(viewer, sent);
            Gui { graph, viewer, shared, state: RefCell::new(Box::new(KeptLines::default())) }
        }

        /// Runs `f` on the viewer, as the app does.
        fn run(&mut self, f: impl FnOnce(&mut Nodes, &mut UiContext<Edges>)) {
            let Gui { graph, viewer, shared, state } = self;
            let colours = &THEMES[0].colours;
            graph.edit(*viewer, &[None], colours, Some(shared), Some(state), None, true, false, None, f);
        }

        fn sent(&self) -> &SentLines {
            self.shared.get::<SentLines>(self.viewer).unwrap()
        }

        fn lines(&self) -> Vec<String> {
            self.state.borrow().downcast_ref::<KeptLines>().unwrap().lines()
        }
    }

    /// While the GUI isn't taking them, lines wait for it: none are lost but the oldest, beyond `KEPT`.
    #[test]
    fn lines_wait_for_the_gui_which_keeps_the_latest() {
        let (mut gui, mut viewing) = (Gui::new(SentLines::default()), Viewing::default());
        let per_pass = crate::standard::logs::LINES_PER_PASS;
        let passes = 2 * KEPT / per_pass;
        for p in 0..passes {
            let lines: Vec<(f64, String)> = (0..per_pass).map(|i| (0.0, (p * per_pass + i).to_string())).collect();
            pass(&mut viewing, gui.sent(), &lines);
        }
        // What was sent before the pool ran out, and the rest with the next pass.
        gui.run(|node, context| _ = node.gui_background(context));
        assert_eq!(gui.lines().len(), WINDOW_BUFFERS * per_pass);
        pass(&mut viewing, gui.sent(), &[]);
        gui.run(|node, context| _ = node.gui_background(context));
        let kept = gui.lines();
        let last = passes * per_pass - 1;
        assert_eq!(kept.len(), KEPT);
        assert_eq!((kept[0].clone(), kept[KEPT - 1].clone()), ((last + 1 - KEPT).to_string(), last.to_string()));
    }

    #[test]
    fn lines_are_shown_with_their_times_and_cleared() {
        let (gui, mut viewing) = (Gui::new(SentLines::default()), Viewing::default());
        let lines = |lines: &[(f64, &str)]| lines.iter().map(|(t, l)| (*t, l.to_string())).collect::<Vec<_>>();
        pass(&mut viewing, gui.sent(), &lines(&[(1520.25, "session started"), (1520.5, "trial 1 started")]));
        pass(&mut viewing, gui.sent(), &lines(&[(1526.125, "trial 1 rewarded at the north arm")]));
        let mut harness = egui_kittest::Harness::builder().with_size([560.0, 200.0]).wgpu().build_ui_state(
            |ui, gui: &mut Gui| {
                gui.run(|node, context| _ = node.gui_background(context));
                gui.run(|node, context| node.gui_ui(ui, context));
            },
            gui,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("log_viewer");
        harness.get_by_label("Clear").click();
        harness.run_steps(2);
        assert!(harness.state().lines().is_empty());
        assert!(harness.query_by_label("0 lines").is_some());
    }
}
