//! Copying nodes' settings, and pasting them onto nodes of the same types. On the clipboard, they're JSON: each
//! node's label, and its type and settings as the graph's file has them. Pasting opens a window to review where
//! each goes (the node of the same type and label, to begin with, or the only one of its type), what it changes,
//! and whether to paste it at all, before pasting.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Graph, NodeId, NodeSet};
use crate::core::theme::Colours;

/// What's on the clipboard.
#[derive(Serialize, Deserialize)]
struct Clip {
    #[serde(rename = "ephys-rs settings")]
    format: u32,
    nodes: Vec<ClipNode>,
}

#[derive(Serialize, Deserialize)]
struct ClipNode {
    label: String,
    node: serde_json::Value,
}

/// The settings of `ids`, for the clipboard.
pub fn copy<N: NodeSet>(graph: &Graph<N>, ids: &[NodeId]) -> String {
    let nodes = ids
        .iter()
        .filter_map(|id| graph.get(*id))
        .filter_map(|e| Some(ClipNode { label: e.label.clone(), node: serde_json::to_value(&e.node).ok()? }))
        .collect();
    serde_json::to_string_pretty(&Clip { format: 1, nodes }).unwrap_or_default()
}

/// A node's settings from the clipboard: where it goes, and whether to paste it.
struct Row<N> {
    label: String,
    node: Result<N, String>,
    target: Option<NodeId>,
    paste: bool,
}

/// The paste window's state.
pub struct PasteSettings<N> {
    rows: Vec<Row<N>>,
}

/// What came of the window this frame.
pub enum Outcome<N> {
    Open,
    Cancelled,
    /// The settings to paste, and what's left out (and why), to log.
    Paste(Vec<Pasted<N>>, Vec<String>),
}

/// Settings to paste: onto which node, from which copied node.
pub struct Pasted<N> {
    pub target: NodeId,
    pub node: N,
    pub from: String,
}

impl<N: NodeSet> PasteSettings<N> {
    /// From the clipboard's text, matching each node to one of the same type: of the same label, or the only one.
    pub fn new(text: &str, graph: &Graph<N>) -> Result<Self, String> {
        let clip: Clip = serde_json::from_str(text).map_err(|_| "There are no copied settings to paste".to_owned())?;
        let rows = clip
            .nodes
            .into_iter()
            .map(|copied| {
                let node = serde_json::from_value::<N>(copied.node).map_err(|e| format!("Can't be read: {e}"));
                let same_type = |id: &NodeId| {
                    let (Ok(node), Some(entry)) = (&node, graph.get(*id)) else { return false };
                    entry.node.info().name == node.info().name
                };
                let candidates: Vec<NodeId> = graph.ids().filter(same_type).collect();
                let named =
                    candidates.iter().copied().find(|id| graph.get(*id).is_some_and(|e| e.label == copied.label));
                let target = named.or_else(|| (candidates.len() == 1).then(|| candidates[0]));
                Row { label: copied.label, node, target, paste: target.is_some() }
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Err("There are no copied settings to paste".into());
        }
        Ok(PasteSettings { rows })
    }

    pub fn show(&mut self, ctx: &egui::Context, graph: &Graph<N>, colours: &Colours, locked: bool) -> Outcome<N> {
        let mut outcome = Outcome::Open;
        let modal = egui::Modal::new(egui::Id::unique("paste settings")).show(ctx, |ui| {
            ui.set_max_width(760.0);
            ui.heading("Paste settings");
            ui.label(
                RichText::new(
                    "Each copied node's settings go to a node of the same type here. Check where, and what changes.",
                )
                .color(colours.text_secondary),
            );
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("paste settings rows").num_columns(4).striped(true).spacing([14.0, 8.0]).show(
                    ui,
                    |ui| {
                        ui.strong("Paste");
                        ui.strong("Copied from");
                        ui.strong("To");
                        ui.strong("Changes");
                        ui.end_row();
                        for (index, row) in self.rows.iter_mut().enumerate() {
                            let node = match &row.node {
                                Ok(node) => node,
                                Err(problem) => {
                                    ui.label("");
                                    ui.label(&row.label);
                                    ui.label(RichText::new(problem).color(colours.warning));
                                    ui.label("");
                                    ui.end_row();
                                    continue;
                                }
                            };
                            let tick =
                                ui.add_enabled(row.target.is_some(), egui::Checkbox::without_text(&mut row.paste));
                            let from = ui.label(format!("{} ({})", row.label, node.info().name));
                            tick.labelled_by(from.id);
                            let name = |id: Option<NodeId>| match id.and_then(|id| graph.get(id)) {
                                Some(entry) => entry.label.clone(),
                                None => "Nowhere".into(),
                            };
                            let candidates: Vec<NodeId> = graph
                                .iter()
                                .filter(|(_, e)| e.node.info().name == node.info().name)
                                .map(|(id, _)| id)
                                .collect();
                            let before = row.target;
                            egui::ComboBox::from_id_salt(("paste to", index))
                                .selected_text(name(row.target))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut row.target, None, "Nowhere");
                                    for id in candidates {
                                        ui.selectable_value(&mut row.target, Some(id), name(Some(id)));
                                    }
                                })
                                .response
                                .labelled_by(from.id);
                            if row.target != before {
                                row.paste = row.target.is_some();
                            }
                            let changes = match row.target.and_then(|id| graph.get(id)) {
                                Some(entry) => changed_settings(&entry.node, node),
                                None if row.target.is_none() => "No node of this type here".into(),
                                None => String::new(),
                            };
                            ui.label(RichText::new(changes).small().color(colours.text_secondary));
                            ui.end_row();
                        }
                    },
                );
            });
            ui.add_space(8.0);
            let (pasting, skipped) = self.plan(graph);
            let mut targets: Vec<NodeId> = pasting.iter().map(|p| p.target).collect();
            targets.sort();
            targets.dedup();
            if targets.len() < pasting.len() {
                ui.label(RichText::new("Two go to the same node: the later is what it gets").color(colours.warning));
            }
            if locked {
                ui.label(RichText::new("Settings are locked while recording.").color(colours.warning));
            }
            ui.horizontal(|ui| {
                let text = format!("Paste to {} node{}", targets.len(), if targets.len() == 1 { "" } else { "s" });
                if ui.add_enabled(!pasting.is_empty() && !locked, egui::Button::new(text)).clicked() {
                    outcome = Outcome::Paste(pasting, skipped);
                }
                if ui.button("Cancel").clicked() {
                    outcome = Outcome::Cancelled;
                }
            });
        });
        if modal.should_close() && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }

    /// What would be pasted, as the rows are now, and what left out, and why. Two going to the same node, the
    /// later is what it gets.
    fn plan(&self, graph: &Graph<N>) -> (Vec<Pasted<N>>, Vec<String>) {
        let mut pasting: Vec<Pasted<N>> = Vec::new();
        let mut skipped = Vec::new();
        for row in &self.rows {
            let node = match &row.node {
                Ok(node) => node,
                Err(problem) => {
                    skipped.push(format!("{}: {problem}", row.label));
                    continue;
                }
            };
            let what = format!("{} ({})", row.label, node.info().name);
            match row.target {
                None => skipped.push(format!("{what}: no node of its type to paste to")),
                Some(_) if !row.paste => skipped.push(format!("{what}: left out")),
                Some(target) => {
                    if let Some(earlier) = pasting.iter().position(|p| p.target == target) {
                        let name = graph.get(target).map_or("?".into(), |e| e.label.clone());
                        skipped.push(format!("{}: {name} got {what}'s instead", pasting[earlier].from));
                        pasting.remove(earlier);
                    }
                    pasting.push(Pasted { target, node: node.clone(), from: what });
                }
            }
        }
        (pasting, skipped)
    }

    /// Pasting straight onto `target`, without asking, if there's just one node's settings: them, if they're of its
    /// type, or why not. Nothing if there are several (to review).
    pub fn onto(&self, target: NodeId, graph: &Graph<N>) -> Option<Result<Pasted<N>, String>> {
        let [row] = self.rows.as_slice() else { return None };
        let entry = graph.get(target)?;
        Some(match &row.node {
            Err(problem) => Err(format!("{}: {problem}", row.label)),
            Ok(node) if node.info().name != entry.node.info().name => Err(format!(
                "The settings copied are of {}, a {}: not of {}, a {}",
                row.label,
                node.info().name,
                entry.label,
                entry.node.info().name
            )),
            Ok(node) => {
                Ok(Pasted { target, node: node.clone(), from: format!("{} ({})", row.label, node.info().name) })
            }
        })
    }
}

/// Which of a node's settings `new` would change, by name.
pub fn changed_settings<N: NodeSet>(old: &N, new: &N) -> String {
    let settings = |n: &N| serde_json::to_value(n).ok().and_then(|v| v.get("settings").cloned()).unwrap_or_default();
    let (old, new) = (settings(old), settings(new));
    let (Some(old), Some(new)) = (old.as_object(), new.as_object()) else {
        return if old == new { "None".into() } else { "Its settings".into() };
    };
    let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    let changed: Vec<String> =
        keys.into_iter().filter(|k| old.get(*k) != new.get(*k)).map(|k| k.replace('_', " ")).collect();
    if changed.is_empty() { "None: they're the same".into() } else { changed.join(", ") }
}
