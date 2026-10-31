//! The inspector: the settings and connections of whatever is selected.

use std::collections::HashSet;

use eframe::egui::{self, RichText};

use super::annotations::{AnnotationId, AnnotationKind, Annotations, NoteColour};
use super::canvas::Selection;
use super::{Event, style};
use crate::engine::{Dimension, Graph, NodeId, NodeSet, Port, Resolved, Role};

pub fn show<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &mut Graph<N>,
    annotations: &mut Annotations,
    resolved: &Resolved<N::Edges>,
    selection: &Selection,
    open_windows: &HashSet<NodeId>,
    events: &mut Vec<Event>,
) {
    let count = selection.nodes.len() + selection.annotations.len();
    if let Some(id) = selection.single() {
        node(ui, graph, resolved, id, open_windows.contains(&id), events);
    } else if let (Some(&id), true) = (selection.annotations.first(), count == 1) {
        annotation(ui, annotations, id, events);
    } else if count > 1 {
        ui.heading(format!("{count} selected"));
        if ui.button("Delete").clicked() {
            events.push(Event::DeleteSelection);
        }
    } else if let Some(edge) = selection.edge {
        let describe = |port: Port| {
            let label = graph.get(port.node).map_or("?", |e| e.label.as_str());
            let name = resolved.get(&port.node).and_then(|s| s.name(port.role, port.index)).unwrap_or("?");
            format!("{label} › {name}")
        };
        ui.heading(match edge.dimension {
            Dimension::Data => "Connection",
            Dimension::Settings => "Settings link",
        });
        ui.label(describe(edge.source()));
        ui.label(format!(
            "{} {}",
            if edge.dimension == Dimension::Data { "→" } else { "edits" },
            describe(edge.target())
        ));
        let source = edge.source();
        if let Some(summary) = resolved.get(&source.node).and_then(|s| s.summary(source.role, source.index)) {
            ui.weak(summary);
        }
        ui.add_space(6.0);
        if ui.button("Delete").clicked() {
            events.push(Event::DeleteSelection);
        }
    } else {
        tips(ui);
    }
}

fn tips(ui: &mut egui::Ui) {
    ui.heading("Nothing selected");
    ui.add_space(6.0);
    for tip in [
        "Add nodes and notes with the toolbar's Add menu, or double-click, right-click or press Tab on the canvas.",
        "Drag from a port to connect it. Compatible ports light up; drop on empty space to add a node that fits.",
        "Diamond ports are settings and controls: link a control to a setting to let one node edit another, even \
         one upstream of it.",
        "Click a port to see what could connect there. Drag a connected input away to move or remove its edge.",
        "Drag on empty canvas to select. Middle- or right-drag, or scroll, to pan; pinch or Ctrl-scroll to zoom; \
         F frames everything.",
        "Double-click a node, or its window button, to open its window. Dock it from there; drag tabs to arrange.",
    ] {
        ui.label(RichText::new(tip).weak());
        ui.add_space(4.0);
    }
}

fn node<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &mut Graph<N>,
    resolved: &Resolved<N::Edges>,
    id: NodeId,
    window_open: bool,
    events: &mut Vec<Event>,
) {
    let Some(state) = resolved.get(&id) else { return };
    let Some(entry) = graph.get_mut(id) else { return };
    let info = entry.node.info();
    let name = ui.label(RichText::new("Name").small().weak());
    ui.add(egui::TextEdit::singleline(&mut entry.label).font(egui::TextStyle::Heading).desired_width(f32::INFINITY))
        .labelled_by(name.id);
    ui.horizontal(|ui| {
        let (chip, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
        ui.painter().rect_filled(chip, 2.0, style::category_colour(info.category));
        ui.weak(info.name);
    });
    ui.label(RichText::new(info.description).small());
    for warning in &state.warnings {
        ui.label(RichText::new(warning.as_str()).color(style::AMBER));
    }
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        graph.edit(id, &state.input_signals, |node, context| node.settings_ui(ui, context));
        ui.add_space(8.0);
        for (role, title) in [
            (Role::Input, "Inputs"),
            (Role::Output, "Outputs"),
            (Role::Setting, "Settings"),
            (Role::Control, "Controls"),
        ] {
            if state.count(role) == 0 {
                continue;
            }
            ui.label(RichText::new(title).small().weak());
            for index in 0..state.count(role) {
                let port = Port { node: id, role, index };
                // What it carries, and for settings links, what's at the other end.
                let label = |node: NodeId| graph.get(node).map_or("?", |e| e.label.as_str()).to_string();
                let detail = match role {
                    Role::Input | Role::Output => state.summary(role, index).unwrap_or_else(|| "not connected".into()),
                    Role::Setting => {
                        let value = state.summary(role, index).unwrap_or_default();
                        match graph.edge_into(port) {
                            Some(edge) => format!("{value}, set by {}", label(edge.from.node)),
                            None => value,
                        }
                    }
                    Role::Control => {
                        let targets: Vec<String> = graph.edges_from(port).map(|e| label(e.to.node)).collect();
                        if targets.is_empty() {
                            "not connected".into()
                        } else {
                            format!("edits {}", targets.join(", "))
                        }
                    }
                };
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    let colour = state.info(role, index).map_or(egui::Color32::GRAY, |i| i.colour);
                    ui.painter().circle_filled(dot.center(), 4.0, colour);
                    ui.label(state.name(role, index).unwrap_or("?"));
                    ui.weak(detail);
                });
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(if window_open { "Close window" } else { "Open window" }).clicked() {
                events.push(Event::ToggleWindow(id));
            }
            if ui.button("Delete").clicked() {
                events.push(Event::Delete(id));
            }
        });
    });
}

fn annotation(ui: &mut egui::Ui, annotations: &mut Annotations, id: AnnotationId, events: &mut Vec<Event>) {
    let Some(annotation) = annotations.get_mut(id) else { return };
    let AnnotationKind::StickyNote(note) = &mut annotation.kind;
    ui.heading("Sticky note");
    ui.horizontal(|ui| {
        for colour in NoteColour::ALL {
            ui.selectable_value(&mut note.colour, colour, colour.name());
        }
    });
    ui.add_space(4.0);
    let label = ui.label(RichText::new("Markdown").small().weak());
    ui.add(egui::TextEdit::multiline(&mut note.text).code_editor().desired_rows(12).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    ui.add_space(6.0);
    if ui.button("Delete").clicked() {
        events.push(Event::DeleteAnnotation(id));
    }
}
