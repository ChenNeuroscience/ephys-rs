//! Unless a node draws a window of its own, it draws the standard window (see `Node::on_gui_tick`): its name (to edit),
//! its toolbar, what it is, any problems, its settings, and what its ports carry and are connected to.

use eframe::egui::{self, RichText};

use super::style;
use crate::engine::{Edges, Node, NodeState, PortRole, UiContext};

/// A node draws the standard window with this, with `settings` (what of the node's own there is to set, if anything)
/// in the middle, disabled while recording, as settings are locked.
pub fn standard<E: Edges, N: Node<E>>(
    node: &mut N,
    ui: &mut egui::Ui,
    context: &mut UiContext<E>,
    settings: impl FnOnce(&mut N, &mut egui::Ui, &mut UiContext<E>),
) {
    let state = NodeState::of(node, context.inputs.to_vec());
    let info = N::INFO;
    let colours = context.theme;
    // Its name, and its toolbar (see `NodeToolbar`). Then its type.
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 160.0).clamp(80.0, 260.0);
        let name = egui::TextEdit::singleline(&mut *context.label)
            .font(egui::TextStyle::Heading)
            .hint_text("Name")
            .desired_width(width);
        ui.add(name).on_hover_text("Its name");
        context.node_toolbar(ui);
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

    let locked = context.locked;
    if locked {
        ui.label(RichText::new("Settings are locked while recording.").color(colours.warning));
    }
    // Out to the very edge of the pane, however little the settings take, so that's where the bar is, with the pane's margin
    // on the inside. The rest of the pane needn't know, or a window that fits its content would grow.
    let margin = super::docking::PANE_MARGIN;
    let mut pane = ui.available_rect_before_wrap();
    let width = pane.width();
    pane.max.x += margin as f32;
    let mut area = ui.new_child(egui::UiBuilder::new().max_rect(pane));
    let content_margin = egui::Margin { right: margin, ..Default::default() };
    let scrolling = egui::ScrollArea::vertical().auto_shrink([false, true]).content_margin(content_margin);
    scrolling.show(&mut area, |ui| {
        ui.add_enabled_ui(!locked, |ui| settings(node, ui, context));
        ui.add_space(8.0);
        for (role, title) in [
            (PortRole::DataStreamIn, "Inputs"),
            (PortRole::DataStreamOut, "Outputs"),
            (PortRole::ValOutCmdIn, "Settings"),
            (PortRole::ValInCmdOut, "Controls"),
        ] {
            if state.count(role) == 0 {
                continue;
            }
            ui.label(RichText::new(title).small().weak());
            for index in 0..state.count(role) {
                // What it carries (or a setting's value); a control, whether it's connected.
                let detail = match role {
                    PortRole::DataStreamIn | PortRole::DataStreamOut => {
                        state.summary(role, index).unwrap_or_else(|| "not connected".into())
                    }
                    PortRole::ValOutCmdIn => state.summary(role, index).unwrap_or_default(),
                    PortRole::ValInCmdOut | PortRole::ValIn if context.control_connected(index) => "connected".into(),
                    PortRole::ValInCmdOut | PortRole::ValIn => "not connected".into(),
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
    ui.allocate_space(egui::vec2(width, area.min_rect().height()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeType;
    use crate::Nodes;
    use crate::engine::Graph;
    use crate::engine::NodesEnum;
    use crate::gui::docking::{PANE_MARGIN, Ticked, node_ui};
    use crate::standard::themes::THEMES;

    /// Once a run's stopped, what it shared is there to read, but there's no run for auto detect to happen in.
    #[test]
    fn auto_detect_needs_a_run_not_just_what_one_shared() {
        use egui_kittest::kittest::{NodeT as _, Queryable as _};
        let disabled = |running: bool| {
            let mut graph = Graph::<Nodes>::default();
            let node_type = NodeType::SpikeGain;
            let id = graph.add(Nodes::create(node_type), [0.0, 0.0]);
            let resolved = graph.resolve();
            let mut harness = egui_kittest::Harness::builder().with_size([520.0, 500.0]).wgpu().build_ui_state(
                move |ui, graph: &mut Graph<Nodes>| {
                    let mut events = Vec::new();
                    let colours = &THEMES[0].colours;
                    node_ui(
                        ui,
                        graph,
                        &resolved,
                        colours,
                        running,
                        None,
                        None,
                        None,
                        false,
                        true,
                        id,
                        &mut events,
                        &mut Ticked::default(),
                        None,
                    );
                },
                graph,
            );
            harness.run_steps(2);
            harness.get_by_role_and_label(egui::accesskit::Role::Button, "Auto detect").accesskit_node().is_disabled()
        };
        assert!(!disabled(true));
        assert!(disabled(false));
    }

    /// When a node's settings are narrower than the pane, and too long for it, the scroll bar's at the pane's edge, not
    /// theirs. See tests/snapshots/.
    #[test]
    fn the_scroll_bar_is_at_the_edge_of_the_pane() {
        let mut graph = Graph::<Nodes>::default();
        let node_type = NodeType::Reference;
        let id = graph.add(Nodes::create(node_type), [0.0, 0.0]);
        let resolved = graph.resolve();
        let mut harness = egui_kittest::Harness::builder().with_size([520.0, 300.0]).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                let mut events = Vec::new();
                egui::Frame::new().inner_margin(PANE_MARGIN).show(ui, |ui| {
                    let colours = &THEMES[0].colours;
                    node_ui(
                        ui,
                        graph,
                        &resolved,
                        colours,
                        false,
                        None,
                        None,
                        None,
                        false,
                        true,
                        id,
                        &mut events,
                        &mut Ticked::default(),
                        None,
                    );
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        // It only shows with the pointer over it.
        harness.event(egui::Event::PointerMoved(egui::pos2(508.0, 200.0)));
        harness.run_steps(20);
        harness.snapshot("node_window_scroll_bar");
    }
}
