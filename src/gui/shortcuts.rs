//! The keyboard shortcuts' help (Help › Keyboard Shortcuts): the app's, from its actions (see `actions`), so it says
//! what the keys do as the program has it; the graph's; and those of node types' windows (`Node::SHORTCUTS`): the
//! types in the graph, or all of them.

use std::collections::HashSet;

use eframe::egui::{self, RichText};

use super::actions::{Action, MENUS, MenuEntry};
use super::dialog;
use crate::engine::{NodeTypeEnum, Shortcut};
use crate::theme::Colours;

/// The graph's keys, over it.
const GRAPH: &[Shortcut] = &[
    Shortcut { keys: "Tab", does: "Add a node, where the pointer is" },
    Shortcut { keys: "Enter", does: "Open the selected node's window, or edit the selected annotation" },
    Shortcut { keys: "Esc", does: "Select nothing, and go back to the select tool" },
];

/// A section: its title, and its keys and what they do.
type Section = (String, Vec<(String, String)>);

/// The help, while it's open.
#[derive(Default)]
pub struct ShortcutsHelp {
    /// Whether it lists every node type's keys, not just those of the types in the graph.
    all_types: bool,
}

/// The app's sections, in its menus' order, then the graph's.
fn app_sections(ctx: &egui::Context) -> Vec<Section> {
    // As they're written on the keys.
    let format = |shortcut| ctx.format_shortcut(&shortcut).replace("Slash", "/").replace("Comma", ",");
    let mut app = Vec::new();
    let mut add = |action: Action| {
        let Some(shortcut) = action.shortcut() else { return };
        let mut keys = format(shortcut);
        if let Some(alternative) = action.alternative() {
            keys = format!("{keys} or {}", format(alternative));
        }
        let name = action.name(&[]).trim_end_matches('…');
        let does = if action.only_when_not_typing() { format!("{name} (not while typing)") } else { name.into() };
        app.push((keys, does));
    };
    fn each(entries: &[MenuEntry], add: &mut dyn FnMut(Action)) {
        for entry in entries {
            match entry {
                MenuEntry::Action(action) => add(*action),
                MenuEntry::Submenu(_, entries) => each(entries, add),
                _ => {}
            }
        }
    }
    for (_, entries) in MENUS {
        each(entries, &mut add);
    }
    vec![("The app".to_string(), app), ("Over the graph".to_string(), rows(GRAPH))]
}

fn rows(shortcuts: &[Shortcut]) -> Vec<(String, String)> {
    shortcuts.iter().map(|s| (s.keys.to_string(), s.does.to_string())).collect()
}

/// Each node type's section, that has keys of its own: of those in `in_graph`, or all.
fn node_sections<T: NodeTypeEnum>(in_graph: &HashSet<T>, all: bool) -> Vec<Section> {
    let shown = T::ALL.iter().filter(|t| !t.shortcuts().is_empty() && (all || in_graph.contains(t)));
    shown.map(|t| (format!("{}'s window", t.info().name), rows(t.shortcuts()))).collect()
}

impl ShortcutsHelp {
    /// Shows the help, for a graph with nodes of the types `in_graph`; returns whether it's still open.
    pub fn show<T: NodeTypeEnum>(&mut self, ctx: &egui::Context, colours: &Colours, in_graph: &HashSet<T>) -> bool {
        let mut close = false;
        let all_types = &mut self.all_types;
        let body = |ui: &mut egui::Ui| {
            body(ui, colours, app_sections(ui.ctx()), all_types, node_sections(in_graph, *all_types));
        };
        let footer = |ui: &mut egui::Ui| close = dialog::at_right(ui, |ui| ui.button("OK").clicked());
        let dismissed = dialog::show(ctx, "Keyboard Shortcuts", 480.0, colours, body, footer);
        !close && !dismissed
    }
}

fn body(ui: &mut egui::Ui, colours: &Colours, app: Vec<Section>, all_types: &mut bool, nodes: Vec<Section>) {
    let section = |ui: &mut egui::Ui, index: usize, (title, rows): &Section| {
        ui.label(RichText::new(title).strong());
        egui::Grid::new(("shortcuts", index)).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            for (keys, does) in rows {
                ui.label(RichText::new(keys).monospace().color(colours.text_primary));
                ui.label(RichText::new(does).color(colours.text_secondary));
                ui.end_row();
            }
        });
        ui.add_space(8.0);
    };
    // Which node types' keys: those in the graph, or every type's. Above the list, so it's always there; the app's
    // and the graph's are always listed.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Node windows' keys:").color(colours.text_secondary));
        ui.selectable_value(all_types, false, "In this graph")
            .on_hover_text("Those of the windows of the nodes in this graph");
        ui.selectable_value(all_types, true, "All types")
            .on_hover_text("Those of every node type's window, in this graph or not");
    });
    ui.add_space(8.0);
    // A fixed size, so the window doesn't move as the scroll bar comes and goes (with what wraps): what's left of
    // the window after the title, the footer and a margin.
    let height = (ui.ctx().content_rect().height() - 330.0).clamp(150.0, 520.0);
    egui::ScrollArea::vertical().auto_shrink(false).max_height(height).show(ui, |ui| {
        for (index, app) in app.iter().enumerate() {
            section(ui, index, app);
        }
        if nodes.is_empty() {
            let none = if *all_types { "No node type" } else { "No node in this graph" };
            ui.label(RichText::new(format!("{none} has keys of its own.")).color(colours.text_secondary));
        }
        for (index, node) in nodes.iter().enumerate() {
            section(ui, app.len() + index, node);
        }
    });
}
