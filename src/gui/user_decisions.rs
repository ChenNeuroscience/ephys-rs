//! As nodes prepare for a snapshot, they may say something someone should decide (see `engine::UserDecision`). The
//! snapshot runs meanwhile, and the app asks, for each node, whether to take what the node suggests, if anything, or
//! carry on without it, or to stop.

use eframe::egui::{self, RichText};

use super::dialog;
use crate::engine::{Graph, NodeId, NodesEnum, Suggestion, UserDecision};
use crate::theme::Colours;

/// For each node with something to decide, someone chooses one of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    /// The node's replaced by what it suggests (see `Suggestion`).
    UseSuggestion,
    /// The node's disabled, and the run carries on without it.
    Disable,
    /// The node's left as it is, and the app doesn't ask about the same things again while running.
    Leave,
}

/// Once someone's chosen to leave a node as it is, the app doesn't ask about the same things again while running: these
/// are the nodes left, each with what it said.
pub type LeftAlone = Vec<(NodeId, Vec<String>)>;

/// The dialog lists what each node says someone should decide, in the order it said, with what the node suggests (its
/// last: each is the whole node).
type NodeDecisions<N> = (NodeId, Vec<String>, Option<Suggestion<N>>);

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
        if decision.suggestion.is_some() {
            nodes[at].2 = decision.suggestion;
        }
    }
    nodes
}

/// The app keeps the dialog here while it's asking what to do about nodes with something to decide, for a snapshot (by
/// version) the run's holding.
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
        // Taking what a node suggests where it suggests something, otherwise doing without the node.
        let choice =
            |suggestion: &Option<_>| if suggestion.is_some() { Choice::UseSuggestion } else { Choice::Disable };
        let choices = nodes.iter().map(|(_, _, suggestion)| choice(suggestion)).collect();
        Asking { version, nodes, choices }
    }

    pub fn show(&mut self, ctx: &egui::Context, graph: &Graph<N>, colours: &Colours) -> Outcome {
        let mut outcome = Outcome::Open;
        let (nodes, choices) = (&self.nodes, &mut self.choices);
        let body = |ui: &mut egui::Ui| {
            ui.label("The following decisions need your attention:");
            ui.add_space(6.0);
            for ((id, whats, suggestion), choice) in nodes.iter().zip(choices.iter_mut()) {
                let label = graph.get(*id).map_or("(removed)", |entry| entry.label.as_str());
                ui.label(RichText::new(label).strong());
                for what in whats {
                    ui.label(RichText::new(what).color(colours.warning));
                }
                // One a line, as node names can be long.
                ui.vertical(|ui| {
                    if let Some(suggestion) = suggestion {
                        ui.radio_value(choice, Choice::UseSuggestion, suggestion.label.as_str());
                    }
                    ui.radio_value(choice, Choice::Disable, format!("Disable {label}"));
                    ui.radio_value(choice, Choice::Leave, format!("Don't change {label}"));
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
        let _ = dialog::show(ctx, "Something to decide", 520.0, colours, body, footer);
        outcome
    }

    /// The app does what's been decided to `graph` with this: each node replaced by what it suggests, disabled, or
    /// left as it is. This returns the nodes left, with what each said, not to ask about again (see `LeftAlone`).
    pub fn apply(self, graph: &mut Graph<N>) -> LeftAlone {
        let mut left = Vec::new();
        for ((id, whats, suggestion), choice) in self.nodes.into_iter().zip(self.choices) {
            let Some(entry) = graph.get_mut(id) else { continue };
            match (choice, suggestion) {
                (Choice::Leave, _) => {
                    log::info!("{}: left as it is", entry.label);
                    left.push((id, whats));
                }
                (Choice::UseSuggestion, Some(Suggestion { label, node })) => {
                    log::info!("{}: {label}", entry.label);
                    entry.node = node;
                }
                (Choice::UseSuggestion, None) | (Choice::Disable, _) => {
                    log::warn!("{}: disabled, to carry on without it", entry.label);
                    entry.disabled = true;
                }
            }
        }
        left
    }
}
