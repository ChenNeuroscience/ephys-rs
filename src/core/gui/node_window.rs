//! A node's window, for a node without a view of its own (see `Node::HAS_VIEW`): its name (to edit), what it is,
//! any problems, its settings, and what its ports carry and are connected to.

use eframe::egui::{self, RichText};

use super::node_tools::NodeTools;
use super::{Event, style};
use crate::core::engine::NodeToolbar;
use crate::core::engine::{Graph, NodeId, NodeSet, Port, Resolved, Role};
use crate::core::theme::Colours;

/// The window of a node without a view of its own: what it is, its settings, and its connections. `exchange` is what
/// its passes share with its window (see `crate::core::exchange`), while running.
#[allow(clippy::too_many_arguments)]
pub fn show<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &mut Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    exchange: Option<&crate::core::exchange::Exchange>,
    locked: bool,
    advanced: bool,
    id: NodeId,
    events: &mut Vec<Event>,
) {
    let Some(state) = resolved.get(&id) else { return };
    let running = exchange.is_some();
    let Some(mut tools) = NodeTools::new(graph, id, advanced, exchange, events) else { return };
    let Some(entry) = graph.get_mut(id) else { return };
    let info = entry.node.info();
    // Its name, and its toolbar (see `node_tools`). Then its type.
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 160.0).clamp(80.0, 260.0);
        let name = egui::TextEdit::singleline(&mut entry.label)
            .font(egui::TextStyle::Heading)
            .hint_text("Name")
            .desired_width(width);
        ui.add(name).on_hover_text("Its name");
        tools.toolbar(ui);
    });
    ui.horizontal_wrapped(|ui| {
        let (chip, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
        ui.painter().rect_filled(chip, 2.0, style::category_colour(info.category));
        ui.label(RichText::new(info.name).small().strong());
        ui.label(RichText::new(info.description).small());
    });
    for warning in &state.warnings {
        ui.label(RichText::new(warning.as_str()).color(colours.warning));
    }
    ui.separator();

    if locked {
        ui.label(RichText::new("Settings are locked while recording.").color(colours.warning));
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_enabled_ui(!locked, |ui| {
            let tools = Some(&mut tools as &mut dyn crate::core::engine::NodeToolbar);
            graph.edit(id, &state.input_streams, colours, exchange, running, locked, tools, |node: &mut N, context| {
                node.settings_ui(ui, context)
            });
        });
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
                        // Each control linked to it, editing it or only reading it.
                        let (mut set, mut read) = (Vec::new(), Vec::new());
                        for edge in graph.edges().iter().filter(|e| e.target() == port) {
                            let read_only = graph
                                .get(edge.from.node)
                                .is_some_and(|e| e.node.controls().get(edge.from.port).is_some_and(|c| c.read_only));
                            if read_only { &mut read } else { &mut set }.push(label(edge.from.node));
                        }
                        let mut detail = value;
                        if !set.is_empty() {
                            detail += &format!(", set by {}", set.join(", "));
                        }
                        if !read.is_empty() {
                            detail += &format!(", read by {}", read.join(", "));
                        }
                        detail
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
    });
}
