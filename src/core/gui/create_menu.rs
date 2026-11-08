//! The pop-up for adding a node where the user clicked. Opened from a port, it lists only the
//! node types that could connect there, plus the ports already in the graph that could.

use eframe::egui::{self, Id, Key, Order, Pos2, RichText};

use crate::core::engine::{Category, Graph, NodeId, NodeSet, NodeState, Port, Resolved};

/// What's chosen in the menu: a new node, of a type (by index), or a port already in the graph.
pub enum Choice {
    New(usize),
    Existing(Port),
}

/// The menu, open: where it was opened (on screen, and in the scene, for the new node), the port it was opened
/// from, if any, and what's being searched for.
pub struct CreateMenu {
    pub screen: Pos2,
    pub scene: Pos2,
    pub from: Option<Port>,
    search: String,
    focused: bool,
}

/// The port on a new node of type `type_index` that `from` would connect to, if any. The port's node is a
/// placeholder.
pub fn port_on_new<N: NodeSet>(from: Port, resolved: &Resolved<N::Edges>, type_index: usize) -> Option<Port> {
    let kind = resolved.get(&from.node)?.kind(from.role, from.index)?;
    NodeState::unconnected(&N::create(type_index)).find(NodeId(0), from.role.partner(), kind)
}

impl CreateMenu {
    pub fn new(screen: Pos2, scene: Pos2, from: Option<Port>) -> Self {
        CreateMenu { screen, scene, from, search: String::new(), focused: false }
    }

    /// Returns what was chosen, if anything, and whether the menu stays open.
    pub fn show<N: NodeSet>(
        &mut self,
        ctx: &egui::Context,
        graph: &Graph<N>,
        resolved: &Resolved<N::Edges>,
    ) -> (Option<Choice>, bool) {
        let needle = self.search.to_lowercase();
        let matches = |name: &str| name.to_lowercase().contains(&needle);
        let from = self.from;
        let types: Vec<usize> = (0..N::TYPES.len())
            .filter(|&t| matches(N::TYPES[t].name))
            .filter(|&t| from.is_none_or(|from| port_on_new::<N>(from, resolved, t).is_some()))
            .collect();
        let existing: Vec<(Port, String)> = from
            .map(|from| graph.targets(from))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let label = &graph.get(t.node)?.label;
                Some((t, format!("{label} › {}", resolved.get(&t.node)?.name(t.role, t.index)?)))
            })
            .filter(|(_, label)| matches(label))
            .collect();

        let mut choice = None;
        let area = egui::Area::new(Id::unique("create-menu"))
            .order(Order::Foreground)
            .fixed_pos(self.screen)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_width(240.0);
                    let title = match from {
                        Some(port) => {
                            let label = graph.get(port.node).map_or("", |e| e.label.as_str());
                            let name =
                                resolved.get(&port.node).and_then(|s| s.name(port.role, port.index)).unwrap_or("?");
                            format!("Connect to {label} › {name}")
                        }
                        None => "Add a node".into(),
                    };
                    ui.label(RichText::new(title).strong());
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.search).hint_text("Search").desired_width(f32::INFINITY),
                    );
                    if !self.focused {
                        search.request_focus();
                        self.focused = true;
                    }
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        choice = types
                            .first()
                            .map(|t| Choice::New(*t))
                            .or(existing.first().map(|(p, _)| Choice::Existing(*p)));
                    }

                    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                        for category in Category::ALL {
                            let in_category: Vec<usize> =
                                types.iter().copied().filter(|t| N::TYPES[*t].category == category).collect();
                            if in_category.is_empty() {
                                continue;
                            }
                            ui.label(RichText::new(category.name()).small().weak());
                            for t in in_category {
                                let info = &N::TYPES[t];
                                if ui.selectable_label(false, info.name).on_hover_text(info.description).clicked() {
                                    choice = Some(Choice::New(t));
                                }
                            }
                        }
                        if !existing.is_empty() {
                            ui.separator();
                            ui.label(RichText::new("Already in the graph").small().weak());
                            for (port, label) in &existing {
                                if ui.selectable_label(false, label).clicked() {
                                    choice = Some(Choice::Existing(*port));
                                }
                            }
                        }
                        if types.is_empty() && existing.is_empty() {
                            ui.weak("Nothing fits");
                        }
                    });
                });
            });

        let clicked_outside = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| !area.response.rect.contains(p))
        });
        let escaped = ctx.input(|i| i.key_pressed(Key::Escape));
        let keep = choice.is_none() && !clicked_outside && !escaped;
        (choice, keep)
    }
}
