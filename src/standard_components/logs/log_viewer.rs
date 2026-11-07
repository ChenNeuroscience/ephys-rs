//! The Log Viewer: log lines as they come, each with its time, in seconds by its source's clock, just as it came. The
//! lines are kept for the run, up to `KEPT`, in the run's exchange (so they carry on from epoch to epoch), and the
//! window draws only those in view.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, PassState};
use crate::core::exchange::Shared;
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

/// The lines so far, shared with the window.
#[derive(Default)]
pub struct LogHistory {
    entries: Mutex<VecDeque<Entry>>,
}

impl Shared for LogHistory {
    fn name(&self) -> &'static str {
        "log lines"
    }
}

impl LogHistory {
    fn add<'a>(&self, lines: impl Iterator<Item = (f64, &'a [u8])>) {
        let mut entries = self.entries.lock().unwrap();
        for (time, line) in lines {
            if entries.len() == KEPT {
                entries.pop_front();
            }
            entries.push_back(Entry { time, line: String::from_utf8_lossy(line).into_owned() });
        }
    }

    /// Lines in the order they came, for tests.
    #[cfg(test)]
    pub fn lines(&self) -> Vec<String> {
        self.entries.lock().unwrap().iter().map(|e| e.line.clone()).collect()
    }
}

/// While running: the lines so far, once there are any.
#[derive(Default)]
pub struct Viewing {
    history: Option<Arc<LogHistory>>,
}

impl PassState for Viewing {
    const WHY: &'static str = "Where the lines so far are, which each pass adds to, in order.";
}

impl Node<Edges> for LogViewer {
    type State = Viewing;

    const INFO: NodeInfo = NodeInfo {
        name: "Log Viewer",
        category: Category::Visualisation,
        description: "Log lines as they come, each with its time.",
    };
    const HAS_VIEW: bool = true;
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
        let history = viewing.history.get_or_insert_with(|| pass.shared(LogHistory::default));
        if let Some(Some(Buffer::LogLines(log))) = pass.inputs.first() {
            history.add(log.lines());
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        ui.weak("Nothing to set.");
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let history = context.shared::<LogHistory>();
        let mut entries = history.as_ref().map(|history| history.entries.lock().unwrap());
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
    use crate::core::engine::{Graph, NodeSet};
    use crate::core::exchange::Exchange;
    use crate::standard_components::themes::THEMES;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn it_keeps_the_latest_lines() {
        let history = LogHistory::default();
        let lines: Vec<String> = (0..KEPT + 2).map(|i| i.to_string()).collect();
        history.add(lines.iter().map(|line| (0.0, line.as_bytes())));
        let kept = history.lines();
        assert_eq!(kept.len(), KEPT);
        assert_eq!((kept[0].as_str(), kept[KEPT - 1].as_str()), ("2", lines[KEPT + 1].as_str()));
    }

    fn harness(exchange: Exchange) -> egui_kittest::Harness<'static, Graph<Nodes>> {
        let mut graph = Graph::default();
        let viewer = graph.add(Nodes::LogViewer(LogViewer::default()), [0.0, 0.0]);
        let mut harness = egui_kittest::Harness::builder().with_size([560.0, 200.0]).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    viewer,
                    &[None],
                    &THEMES[0].colours,
                    Some(&exchange),
                    None,
                    true,
                    false,
                    None,
                    |node, context| {
                        // As the run's window has: it's made for it by its passes.
                        context.exchange.unwrap().shared(viewer, LogHistory::default);
                        node.view_ui(ui, context)
                    },
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
        let exchange = Exchange::default();
        let viewer = crate::core::engine::NodeId(1);
        let history = exchange.shared(viewer, LogHistory::default);
        history.add([(1520.25, &b"session started"[..]), (1520.5, b"trial 1 started")].into_iter());
        history.add([(1526.125, &b"trial 1 rewarded at the north arm"[..])].into_iter());
        let mut harness = harness(exchange);
        harness.snapshot("log_viewer");
        harness.get_by_label("Clear").click();
        harness.run_steps(2);
        assert!(history.lines().is_empty());
        assert!(harness.query_by_label("0 lines").is_some());
    }
}
