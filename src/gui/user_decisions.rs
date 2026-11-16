//! As nodes prepare for a snapshot, they may say something about their hardware someone should decide (see
//! `engine::UserDecision`). The snapshot runs meanwhile, and the app asks, for each node, whether to take on the
//! hardware's settings or carry on without it, or to stop.

use eframe::egui::{self, RichText};

use super::dialog;
use crate::engine::{Graph, NodeId, NodesEnum, UserDecision};
use crate::theme::Colours;

/// For each node whose hardware isn't as its settings say, someone chooses one of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    /// The node takes on the hardware's settings (the decisions' fix).
    UseHardware,
    /// The node's disabled, and the run carries on without it.
    Disable,
}

/// The dialog lists what each node says someone should decide, in the order it said, with the fix (its last: each is
/// the whole node).
type NodeDecisions<N> = (NodeId, Vec<String>, Option<N>);

/// The dialog groups the decisions by node (see `NodeDecisions`).
fn by_node<N: NodesEnum>(decisions: Vec<(NodeId, UserDecision<N>)>) -> Vec<NodeDecisions<N>> {
    let mut nodes: Vec<NodeDecisions<N>> = Vec::new();
    for (id, decision) in decisions {
        let at = match nodes.iter().position(|(node, ..)| *node == id) {
            Some(at) => at,
            None => {
                nodes.push((id, Vec::new(), None));
                nodes.len() - 1
            }
        };
        nodes[at].1.push(decision.what);
        if decision.fix.is_some() {
            nodes[at].2 = decision.fix;
        }
    }
    nodes
}

/// The app keeps the dialog here while it's asking what to do about nodes whose hardware isn't as their settings say,
/// for a snapshot (by version) the run's holding.
pub struct Asking<N: NodesEnum> {
    pub version: u64,
    nodes: Vec<NodeDecisions<N>>,
    /// For each node, what's chosen.
    choices: Vec<Choice>,
}

/// Showing the dialog says what's been decided, if anything.
pub enum Outcome {
    Open,
    /// Apply what's chosen for each node, running on.
    Apply,
    Stop,
}

impl<N: NodesEnum> Asking<N> {
    pub fn new(version: u64, decisions: Vec<(NodeId, UserDecision<N>)>) -> Self {
        let nodes = by_node(decisions);
        // Taking on the hardware's settings where there are some, otherwise doing without the node.
        let choices =
            nodes.iter().map(|(_, _, fix)| if fix.is_some() { Choice::UseHardware } else { Choice::Disable }).collect();
        Asking { version, nodes, choices }
    }

    pub fn show(&mut self, ctx: &egui::Context, graph: &Graph<N>, colours: &Colours) -> Outcome {
        let mut outcome = Outcome::Open;
        let (nodes, choices) = (&self.nodes, &mut self.choices);
        let body = |ui: &mut egui::Ui| {
            ui.label("Before running, these nodes found their hardware isn't as their settings say:");
            ui.add_space(6.0);
            for ((id, whats, fix), choice) in nodes.iter().zip(choices.iter_mut()) {
                let label = graph.get(*id).map_or("(removed)", |entry| entry.label.as_str());
                ui.label(RichText::new(label).strong());
                for what in whats {
                    ui.label(RichText::new(what).color(colours.warning));
                }
                ui.horizontal(|ui| {
                    if fix.is_some() {
                        ui.radio_value(choice, Choice::UseHardware, "Use the hardware's settings");
                    }
                    ui.radio_value(choice, Choice::Disable, "Carry on without it (disabled)");
                });
                ui.add_space(6.0);
            }
        };
        let footer = |ui: &mut egui::Ui| {
            if ui.button("Stop").on_hover_text("Stop running, changing nothing").clicked() {
                outcome = Outcome::Stop;
            }
            let apply = dialog::at_right(ui, |ui| {
                ui.button("Apply").on_hover_text("Do what's chosen for each node, and run on").clicked()
            });
            if apply {
                outcome = Outcome::Apply;
            }
        };
        // It stays until it's decided: clicking outside or Esc doesn't dismiss it.
        let _ = dialog::show(ctx, "The hardware isn't as set", 520.0, colours, body, footer);
        outcome
    }

    /// The app does what's been decided to `graph` with this: each node's settings changed to the hardware's, or the
    /// node disabled.
    pub fn apply(self, graph: &mut Graph<N>) {
        for ((id, _, fix), choice) in self.nodes.into_iter().zip(self.choices) {
            let Some(entry) = graph.get_mut(id) else { continue };
            match (choice, fix) {
                (Choice::UseHardware, Some(fix)) => {
                    log::info!("{}: taking on the hardware's settings", entry.label);
                    entry.node = fix;
                }
                (Choice::UseHardware, None) | (Choice::Disable, _) => {
                    log::warn!("{}: disabled, to carry on without it", entry.label);
                    entry.disabled = true;
                }
            }
        }
    }
}
