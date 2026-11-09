//! The basic view, the home pane's first mode, for people who'd rather not work with the graph: the program's version
//! as a title, with a link to the About window; while the graph's empty, the recent files, and otherwise the components
//! shown in it (viewers, unless set otherwise: see `NodeEntry::shown_in_basic`), large, to open their windows (beside it); and the
//! quick actions that apply to the graph, in grey (blue is for processing). Disabled components are faded, as on the
//! graph. The graph is in advanced mode (the toolbar's graph button), a click away.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke};

use super::Event;
use super::actions::Action;
use super::icons::Icon;
use super::style;
use crate::engine::{Category, EdgeTypesEnum, Graph, NodeId, NodesEnum, Resolved};
use crate::quick_action::QuickAction;
use crate::theme::Colours;

/// The cards' widest.
const WIDTH: f32 = 560.0;
/// How opaque a disabled component's card is: as faded as a disabled node on the graph.
const DISABLED_OPACITY: f32 = 0.35;

#[allow(clippy::too_many_arguments)]
pub fn show<N: NodesEnum>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    quick_actions: &[QuickAction<N>],
    // Those offered, by index (see `App::offered`).
    offered: &[usize],
    open_windows: &HashSet<NodeId>,
    recent: &[PathBuf],
    locked: bool,
    events: &mut Vec<Event>,
) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 16)).show(ui, |ui| {
            ui.set_max_width(WIDTH.min(ui.available_width()));
            ui.label(RichText::new(crate::full_version()).size(26.0).color(colours.text_primary));
            if ui.link(RichText::new("About").size(12.0)).clicked() {
                events.push(Event::Perform(Action::About, None));
            }
            ui.add_space(18.0);
            let empty = graph.iter().next().is_none();
            if empty && !recent.is_empty() {
                ui.label(RichText::new("Recent files").size(22.0).color(colours.text_primary));
                ui.add_space(8.0);
                for (index, path) in recent.iter().enumerate() {
                    let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(14.0), Sense::hover());
                        Icon::New.paint(ui.painter(), rect, colours.text_secondary, true);
                        let link = ui.link(name).on_hover_text(path.display().to_string());
                        if link.clicked() {
                            events.push(Event::Perform(Action::OpenRecent(index), None));
                        }
                    });
                }
                ui.add_space(18.0);
            }
            if !empty {
                ui.label(RichText::new("Components").size(22.0).color(colours.text_primary));
                ui.add_space(8.0);
                let mut views: Vec<(NodeId, &str, Category)> = graph
                    .iter()
                    .filter(|(_, e)| e.shown_in_basic())
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
                                let summary = EdgeTypesEnum::summary(stream.as_ref()?);
                                Some(if summary.is_empty() {
                                    input.name.to_string()
                                } else {
                                    format!("{}: {summary}", input.name)
                                })
                            })
                            .collect();
                        if !connected.is_empty() {
                            return Some(connected.join(" · "));
                        }
                        // A source has nothing to connect: what it gives is what's worth saying.
                        let outputs = state.outputs.iter().filter_map(|output| {
                            let summary = EdgeTypesEnum::summary(output.stream.as_ref()?);
                            Some(format!("{}: {summary}", output.name))
                        });
                        state.inputs.is_empty().then(|| outputs.collect::<Vec<_>>().join(" · "))
                    });
                    let disabled = graph.get(id).is_some_and(|e| e.disabled);
                    let (detail, detail_colour) = match detail.flatten() {
                        _ if disabled => ("Disabled".to_owned(), colours.text_secondary),
                        Some(detail) => (detail, colours.text_secondary),
                        None => ("Nothing connected yet".to_owned(), colours.warning),
                    };
                    let colour = style::category_colour(category);
                    // Faded, as on the graph.
                    let card = ui
                        .scope(|ui| {
                            if disabled {
                                ui.multiply_opacity(DISABLED_OPACITY);
                            }
                            card(ui, colours, colour, label, &detail, detail_colour)
                        })
                        .inner;
                    let tip = if open { "Its window's open" } else { "Open its window beside this view" };
                    let card = card.on_hover_text(format!("{tip} (right-click to hide it from here)"));
                    if card.clicked() {
                        events.push(Event::OpenWindow(id, Some(crate::layout::Pane::Home)));
                    }
                    card.context_menu(|ui| {
                        if ui.button("Hide from the basic view").clicked() {
                            events.push(Event::ShowInBasic(id, false));
                        }
                    });
                    ui.add_space(6.0);
                }

                ui.add_space(18.0);
            }
            ui.label(RichText::new("Quick actions").size(22.0).color(colours.text_primary));
            ui.add_space(8.0);
            let offered: Vec<(usize, &QuickAction<N>)> =
                offered.iter().filter_map(|&i| Some((i, quick_actions.get(i)?))).collect();
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

/// A large, clickable card: a stripe of `colour` at the left, a title and a line of detail.
fn card(
    ui: &mut egui::Ui,
    colours: &Colours,
    colour: Color32,
    title: &str,
    detail: &str,
    detail_colour: Color32,
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
            let ink = if enabled { colours.text_primary } else { colours.text_disabled };
            ui.label(RichText::new(title).size(17.0).color(ink));
            ui.label(RichText::new(detail).size(12.0).color(detail_colour));
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
