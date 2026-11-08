//! The Log Viewer: log lines as they come, each with its time, in seconds by its source's clock, just as it came. The
//! passes send each line to the window through a pool (see `Pool::send_pending`); the window keeps them for the run,
//! up to `KEPT`, and draws only those in view.

use std::collections::VecDeque;
use std::sync::Mutex;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Input, Node, NodeInfo, Output, Pass, PassState, PoolStatus, Shared, UiContext};
use crate::standard_components::pool::{Pool, WINDOW_BUFFERS};
use crate::standard_components::preview;
use crate::standard_components::widgets::require_input;
use crate::{Buffer, DataStream, Edges, Kind};

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

/// The lines sent to the window, and those it's kept.
pub struct LogHistory {
    sent: Pool<Vec<Entry>>,
    /// Only the window takes this, so passes never wait on it.
    kept: Mutex<VecDeque<Entry>>,
}

impl Default for LogHistory {
    fn default() -> Self {
        LogHistory { sent: Pool::new((0..WINDOW_BUFFERS).map(|_| Vec::new()), 0), kept: Mutex::default() }
    }
}

impl Shared for LogHistory {
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        vec![("log lines", self.sent.status())]
    }
}

impl LogHistory {
    /// For the window: the lines kept, with those sent since it last looked.
    fn kept(&self) -> std::sync::MutexGuard<'_, VecDeque<Entry>> {
        let mut kept = self.kept.lock().unwrap();
        self.sent.receive_all(|entries| {
            kept.extend(entries);
            let excess = kept.len().saturating_sub(KEPT);
            kept.drain(..excess);
        });
        kept
    }

    /// Lines in the order they came, for tests.
    #[cfg(test)]
    pub fn lines(&self) -> Vec<String> {
        self.kept().iter().map(|e| e.line.clone()).collect()
    }
}

/// While running: the lines not sent yet.
#[derive(Default)]
pub struct Viewing {
    pending: Vec<Entry>,
}

impl PassState for Viewing {
    const WHY: &'static str = "The lines its window hasn't been sent yet, which each pass adds to, in order.";
}

impl Node<Edges> for LogViewer {
    type State = Viewing;
    /// The lines, for its window.
    type Shared = LogHistory;

    const INFO: NodeInfo = NodeInfo {
        name: "Log Viewer",
        category: Category::Visualisation,
        description: "Log lines as they come, each with its time.",
    };
    const HAS_CUSTOM_UI: bool = true;
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
        let (pending, history) = (&mut viewing.pending, pass.shared::<LogHistory>());
        if let Some(Some(Buffer::LogLines(log))) = pass.inputs.first() {
            pending.extend(
                log.lines().map(|(time, line)| Entry { time, line: String::from_utf8_lossy(line).into_owned() }),
            );
            // At most `KEPT` waiting, if the window isn't taking them.
            if pending.len() > 2 * KEPT {
                pending.drain(..pending.len() - KEPT);
            }
        }
        history.sent.send_pending(pending);
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut UiContext<Edges>) {
        ui.weak("Nothing to set.");
    }

    fn custom_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let history = context.shared::<LogHistory>();
        let mut entries = history.as_ref().map(|history| history.kept());
        ui.horizontal(|ui| {
            let count = entries.as_ref().map_or(0, |entries| entries.len());
            ui.weak(format!("{count} lines"));
            if ui.add_enabled(count > 0, egui::Button::new("Clear")).clicked()
                && let Some(entries) = &mut entries
            {
                entries.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| context.node_toolbar(ui));
        });
        let Some(entries) = entries else {
            return preview::unconnected(ui, context.theme, "Press Play to see the log");
        };
        if entries.is_empty() {
            return preview::unconnected(ui, context.theme, "No log lines yet");
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
        drop(entries);
        preview::repaint_with_data(ui, context.running);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nodes;
    use crate::core::engine::SharedByNode;
    use crate::core::engine::{Graph, NodeId, NodesEnum};
    use crate::standard_components::themes::THEMES;
    use egui_kittest::kittest::Queryable;
    use std::sync::Arc;

    /// Runs a pass of the viewer, sharing `history`, given `lines`.
    fn pass(viewing: &mut Viewing, history: &LogHistory, lines: &[(f64, String)]) {
        use crate::core::engine::DataStreamType;
        let mut log = crate::standard_components::logs::LogLines { width: 64 }.buffer(0.02);
        for (time, line) in lines {
            assert!(log.push(*time, line));
        }
        let input = Buffer::LogLines(log);
        let inputs = [Some(&input)];
        let mut pass = Pass::new(0, 0.02, &inputs, &mut []);
        pass.shared = history;
        LogViewer::default().process(&mut pass, viewing);
    }

    /// While the window isn't taking them (it's closed, say), lines wait for it: none are lost but the oldest, beyond
    /// `KEPT`.
    #[test]
    fn lines_wait_for_the_window_which_keeps_the_latest() {
        let (history, mut viewing) = (LogHistory::default(), Viewing::default());
        let per_pass = crate::standard_components::logs::LINES_PER_PASS;
        let passes = 2 * KEPT / per_pass;
        for p in 0..passes {
            let lines: Vec<(f64, String)> = (0..per_pass).map(|i| (0.0, (p * per_pass + i).to_string())).collect();
            pass(&mut viewing, &history, &lines);
        }
        // What was sent before the pool ran out, and the rest with the next pass.
        assert_eq!(history.lines().len(), WINDOW_BUFFERS * per_pass);
        pass(&mut viewing, &history, &[]);
        let kept = history.lines();
        let last = passes * per_pass - 1;
        assert_eq!(kept.len(), KEPT);
        assert_eq!((kept[0].clone(), kept[KEPT - 1].clone()), ((last + 1 - KEPT).to_string(), last.to_string()));
    }

    fn harness(shared: Arc<SharedByNode>) -> egui_kittest::Harness<'static, Graph<Nodes>> {
        let mut graph = Graph::default();
        let viewer = graph.add(Nodes::LogViewer(LogViewer::default()), [0.0, 0.0]);
        let mut harness = egui_kittest::Harness::builder().with_size([560.0, 200.0]).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    viewer,
                    &[None],
                    &THEMES[0].colours,
                    Some(&shared),
                    None,
                    true,
                    false,
                    None,
                    |node, context| node.custom_ui(ui, context),
                );
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness
    }

    #[test]
    fn lines_are_shown_with_their_times_and_cleared() {
        let (history, viewer, mut viewing) = (LogHistory::default(), NodeId(1), Viewing::default());
        let lines = |lines: &[(f64, &str)]| lines.iter().map(|(t, l)| (*t, l.to_string())).collect::<Vec<_>>();
        pass(&mut viewing, &history, &lines(&[(1520.25, "session started"), (1520.5, "trial 1 started")]));
        pass(&mut viewing, &history, &lines(&[(1526.125, "trial 1 rewarded at the north arm")]));
        let shared = Arc::new(SharedByNode::default().with(viewer, history));
        let history = shared.clone();
        let history = || history.get::<LogHistory>(viewer).unwrap();
        let mut harness = harness(shared);
        harness.snapshot("log_viewer");
        harness.get_by_label("Clear").click();
        harness.run_steps(2);
        assert!(history().lines().is_empty());
        assert!(harness.query_by_label("0 lines").is_some());
    }
}
