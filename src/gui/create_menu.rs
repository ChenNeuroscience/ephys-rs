//! People add a node with a pop-up menu, where they clicked on the graph. Opened from a port, the menu lists only the
//! node types that could connect there, plus the ports already in the graph that could. Node types hidden in Settings
//! aren't listed.

use eframe::egui::{self, Id, Key, Order, Pos2, RichText};

use crate::engine::{Category, Graph, NodeId, NodeTypeEnum, NodesEnum, PortAddress, PortRole, Resolved};

/// Someone chooses either a new node from the menu, of a type (`NodesEnum::Type`), or a port already in the graph.
pub enum Choice<T> {
    New(T),
    Existing(PortAddress),
}

/// The app keeps the menu here while it's open: where it was opened (on screen, and in the scene, for the new node),
/// the port it was opened from, if any, and what's being searched for.
pub struct CreateMenu {
    pub screen: Pos2,
    pub scene: Pos2,
    pub from: Option<PortAddress>,
    search: String,
    focused: bool,
    /// The menu's above everything else on this layer: the quick action editor's, above its window.
    order: Order,
}

/// Opened from port `from`, the menu offers a node type only if a new node of that type has a port `from` would
/// connect to. This finds that port, if any; the port's node is a placeholder.
pub fn port_on_new<N: NodesEnum>(
    from: PortAddress,
    resolved: &Resolved<N::Edges>,
    node_type: N::Type,
) -> Option<PortAddress> {
    let kind = resolved.get(&from.node)?.kind(from.role, from.port_idx)?;
    let node = N::create(node_type);
    let fitting = |&role: &PortRole| {
        let port_idx = node.ports_of(role).iter().position(|port| port.kind() == kind)?;
        Some(PortAddress { node: NodeId(0), role, port_idx })
    };
    from.role.partners().iter().find_map(fitting)
}

impl CreateMenu {
    pub fn new(screen: Pos2, scene: Pos2, from: Option<PortAddress>) -> Self {
        CreateMenu { screen, scene, from, search: String::new(), focused: false, order: Order::Foreground }
    }

    /// The menu, above a window shown as a modal (which is itself in front of the app).
    pub fn above_modal(mut self) -> Self {
        self.order = Order::Tooltip;
        self
    }

    /// The app shows the menu with this, which returns what was chosen, if anything, and whether the menu stays open.
    pub fn show<N: NodesEnum>(
        &mut self,
        ctx: &egui::Context,
        graph: &Graph<N>,
        resolved: &Resolved<N::Edges>,
        hidden: &std::collections::BTreeSet<String>,
    ) -> (Option<Choice<N::Type>>, bool) {
        let needle = self.search.to_lowercase();
        let matches = |name: &str| name.to_lowercase().contains(&needle);
        let from = self.from;
        let types: Vec<N::Type> = (N::Type::ALL.iter().copied())
            .filter(|t| matches(t.info().name) && !hidden.contains(t.info().name))
            .filter(|&t| from.is_none_or(|from| port_on_new::<N>(from, resolved, t).is_some()))
            .collect();
        let existing: Vec<(PortAddress, String)> = from
            .map(|from| graph.targets(from))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let label = &graph.get(t.node)?.label;
                Some((t, format!("{label} › {}", resolved.get(&t.node)?.name(t.role, t.port_idx)?)))
            })
            .filter(|(_, label)| matches(label))
            .collect();

        let mut choice = None;
        let area = egui::Area::new(Id::unique("create-menu"))
            .order(self.order)
            .fixed_pos(self.screen)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_width(240.0);
                    let title = match from {
                        Some(port) => {
                            let label = graph.get(port.node).map_or("", |e| e.label.as_str());
                            let name =
                                resolved.get(&port.node).and_then(|s| s.name(port.role, port.port_idx)).unwrap_or("?");
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
                            let in_category: Vec<N::Type> =
                                types.iter().copied().filter(|t| t.info().category == category).collect();
                            if in_category.is_empty() {
                                continue;
                            }
                            ui.label(RichText::new(category.name()).small().weak());
                            for t in in_category {
                                let info = t.info();
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

/// What's chosen from the menu, opened at `scene` (from the port `from`, if it was), is done to `graph` with this: a new
/// node beside the port it's connected to, on the side its edge leaves from (or centred where the menu was opened); or
/// the port chosen connected. This returns the new node, if there is one, or why the edge couldn't be made.
pub fn add_chosen<N: NodesEnum>(
    graph: &mut Graph<N>,
    resolved: &Resolved<N::Edges>,
    choice: Choice<N::Type>,
    scene: Pos2,
    from: Option<PortAddress>,
) -> (Option<NodeId>, Result<(), String>) {
    use super::canvas::{HEADER, NODE_WIDTH};
    let connect = |graph: &mut Graph<N>, from: PortAddress, to: PortAddress| {
        graph.connect(from, to).map(|_| ()).map_err(|e| format!("Can't connect: {e}"))
    };
    match choice {
        Choice::New(node_type) => {
            let right = |port: PortAddress| {
                resolved.get(&port.node).is_some_and(|s| s.side(port.role, port.port_idx) == crate::engine::Side::Right)
            };
            let pos = match from {
                Some(port) if right(port) => [scene.x + 40.0, scene.y - HEADER],
                Some(_) => [scene.x - NODE_WIDTH - 40.0, scene.y - HEADER],
                None => [scene.x - NODE_WIDTH / 2.0, scene.y - HEADER / 2.0],
            };
            let id = graph.add(N::create(node_type), pos);
            let connected = match from.and_then(|from| Some((from, port_on_new::<N>(from, resolved, node_type)?))) {
                Some((from, port)) => connect(graph, from, PortAddress { node: id, ..port }),
                None => Ok(()),
            };
            (Some(id), connected)
        }
        Choice::Existing(port) => (None, from.map_or(Ok(()), |from| connect(graph, from, port))),
    }
}
