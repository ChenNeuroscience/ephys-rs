//! The basic view, the home pane's first mode, for people who'd rather not work with the graph: the components shown
//! in it (viewers, unless set otherwise: see `NodeEntry::in_basic`), large, to open their windows (beside it); and the
//! quick actions that apply to the graph, in grey (blue is for processing). The graph's a click away (or in Home's
//! menu).

use std::collections::HashSet;

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke};

use super::Event;
use super::style;
use crate::core::engine::{Category, EdgeSet, Graph, NodeId, NodeSet, Resolved};
use crate::core::quick_action::{QuickAction, QuickState};
use crate::core::theme::Colours;

/// The cards' widest.
const WIDTH: f32 = 560.0;

#[allow(clippy::too_many_arguments)]
pub fn show<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    quick_actions: &[QuickAction<N>],
    open_windows: &HashSet<NodeId>,
    locked: bool,
    events: &mut Vec<Event>,
) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 16)).show(ui, |ui| {
            ui.set_max_width(WIDTH.min(ui.available_width()));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Components").size(22.0).color(colours.text_primary));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let graph_button = ui.button("Open the graph").on_hover_text(
                        "The graph: every node, and how data flows between them, to build and change it node by node",
                    );
                    if graph_button.clicked() {
                        events.push(Event::ShowGraph);
                    }
                });
            });
            ui.add_space(8.0);
            let mut views: Vec<(NodeId, &str, Category)> = graph
                .iter()
                .filter(|(_, e)| e.in_basic())
                .map(|(id, e)| (id, e.label.as_str(), e.node.info().category))
                .collect();
            views.sort_by_key(|(id, label, _)| (graph.get(*id).map(|e| e.pos[1] as i64), *label));
            if views.is_empty() {
                ui.label(
                    RichText::new("None yet: add some with a quick action below.").color(colours.text_secondary),
                );
            }
            for (id, label, category) in views {
                let open = open_windows.contains(&id);
                let detail = resolved.get(&id).map(|state| {
                    let connected: Vec<String> = state
                        .inputs
                        .iter()
                        .zip(&state.input_streams)
                        .filter_map(|(input, stream)| {
                            let summary = EdgeSet::summary(stream.as_ref()?);
                            Some(if summary.is_empty() {
                                input.name.to_string()
                            } else {
                                format!("{}: {summary}", input.name)
                            })
                        })
                        .collect();
                    if connected.is_empty() { None } else { Some(connected.join(" · ")) }
                });
                let (detail, detail_colour) = match detail.flatten() {
                    Some(detail) => (detail, colours.text_secondary),
                    None => ("Nothing connected yet".to_owned(), colours.warning),
                };
                let colour = style::category_colour(category);
                let card = card(ui, colours, colour, label, &detail, detail_colour, None);
                let tip = if open { "Its window's open" } else { "Open its window beside this view" };
                let card = card.on_hover_text(format!("{tip} (right-click to hide it from here)"));
                if card.clicked() {
                    events.push(Event::OpenWindow(id));
                }
                card.context_menu(|ui| {
                    if ui.button("Hide from the basic view").clicked() {
                        events.push(Event::InBasic(id, false));
                    }
                });
                ui.add_space(6.0);
            }

            ui.add_space(18.0);
            ui.label(RichText::new("Quick actions").size(22.0).color(colours.text_primary));
            ui.add_space(8.0);
            let state = QuickState { graph, open_windows };
            let offered: Vec<(usize, &QuickAction<N>)> =
                quick_actions.iter().enumerate().filter(|(_, a)| (a.run)(&state).is_some()).collect();
            if offered.is_empty() {
                ui.label(RichText::new("Nothing to add: the graph does all of these.").color(colours.text_secondary));
            }
            for (index, action) in offered {
                ui.add_enabled_ui(!locked, |ui| {
                    let card = card(
                        ui,
                        colours,
                        colours.text_secondary,
                        action.name,
                        action.description,
                        colours.text_secondary,
                        Some("Apply"),
                    );
                    if card.on_disabled_hover_text("Not while recording").clicked() {
                        events.push(Event::QuickAction(index));
                    }
                });
                ui.add_space(6.0);
            }
        });
    });
}

/// A large, clickable card: a stripe of `colour` at the left, a title, a line of detail, and what clicking does at
/// the right.
fn card(
    ui: &mut egui::Ui,
    colours: &Colours,
    colour: Color32,
    title: &str,
    detail: &str,
    detail_colour: Color32,
    action: Option<&str>,
) -> egui::Response {
    let enabled = ui.is_enabled();
    let width = ui.available_width();
    let response = egui::Frame::new()
        .fill(colours.surface_raised)
        .stroke(Stroke::new(1.0, colours.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin { left: 18, right: 14, top: 12, bottom: 12 })
        .show(ui, |ui| {
            ui.set_width(width - 34.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    let ink = if enabled { colours.text_primary } else { colours.text_disabled };
                    ui.label(RichText::new(title).size(17.0).color(ink));
                    ui.label(RichText::new(detail).size(12.0).color(detail_colour));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(action) = action {
                        ui.label(RichText::new(action).color(colours.text_primary));
                    }
                });
            });
        })
        .response;
    let response = ui.interact(response.rect, response.id.with("card"), Sense::click());
    let painter = ui.painter();
    let stripe = egui::Rect::from_min_size(response.rect.min, egui::vec2(5.0, response.rect.height()));
    painter.rect_filled(stripe, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, colour);
    if response.hovered() && enabled {
        painter.rect_stroke(response.rect, 6.0, Stroke::new(1.5, colours.accent), egui::StrokeKind::Inside);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, title));
    response
}
