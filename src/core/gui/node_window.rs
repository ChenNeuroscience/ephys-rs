//! The standard window, for a node to show (`Node::gui_ui`, as it does by default): its name (to edit), its toolbar,
//! what it is, any problems, its settings, and what its ports carry and are connected to.

use eframe::egui::{self, RichText};

use super::style;
use crate::core::engine::{Edges, Node, NodeState, PortRole, UiContext};

/// The standard window, with `settings` (what of the node's own there is to set, if anything) in the middle: disabled
/// while recording, as settings are locked.
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
            (PortRole::Input, "Inputs"),
            (PortRole::Output, "Outputs"),
            (PortRole::Setting, "Settings"),
            (PortRole::Control, "Controls"),
        ] {
            if state.count(role) == 0 {
                continue;
            }
            ui.label(RichText::new(title).small().weak());
            for index in 0..state.count(role) {
                // What it carries (or a setting's value); a control, whether it's connected.
                let detail = match role {
                    PortRole::Input | PortRole::Output => {
                        state.summary(role, index).unwrap_or_else(|| "not connected".into())
                    }
                    PortRole::Setting => state.summary(role, index).unwrap_or_default(),
                    PortRole::Control if context.control_connected(index) => "connected".into(),
                    PortRole::Control => "not connected".into(),
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
    use crate::Nodes;
    use crate::core::engine::Graph;
    use crate::core::engine::NodesEnum;
    use crate::core::gui::docking::{PANE_MARGIN, node_ui};
    use crate::standard_components::themes::THEMES;

    /// Stopped, what the last run shared is there to read, but there's no run for a trigger to go to.
    #[test]
    fn triggers_need_a_run_not_just_what_one_shared() {
        use egui_kittest::kittest::{NodeT as _, Queryable as _};
        let disabled = |running: bool| {
            let mut graph = Graph::<Nodes>::default();
            let index = Nodes::TYPES.iter().position(|t| t.name == "Spike Gain").unwrap();
            let id = graph.add(Nodes::create(index), [0.0, 0.0]);
            let resolved = graph.resolve();
            let triggers = crate::core::pass_triggers::PassTriggers::default();
            let mut harness = egui_kittest::Harness::builder().with_size([520.0, 500.0]).wgpu().build_ui_state(
                move |ui, graph: &mut Graph<Nodes>| {
                    let mut events = Vec::new();
                    let live = running.then_some(&triggers);
                    let colours = &THEMES[0].colours;
                    node_ui(ui, graph, &resolved, colours, live, None, None, None, false, true, id, &mut events);
                },
                graph,
            );
            harness.run_steps(2);
            harness.get_by_role_and_label(egui::accesskit::Role::Button, "Auto detect").accesskit_node().is_disabled()
        };
        assert!(!disabled(true));
        assert!(disabled(false));
    }

    /// Settings narrower than the pane, and too long for it: the bar's at the pane's edge, not theirs. See tests/snapshots/.
    #[test]
    fn the_scroll_bar_is_at_the_edge_of_the_pane() {
        let mut graph = Graph::<Nodes>::default();
        let index = Nodes::TYPES.iter().position(|t| t.name == "Reference").unwrap();
        let id = graph.add(Nodes::create(index), [0.0, 0.0]);
        let resolved = graph.resolve();
        let mut harness = egui_kittest::Harness::builder().with_size([520.0, 300.0]).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                let mut events = Vec::new();
                egui::Frame::new().inner_margin(PANE_MARGIN).show(ui, |ui| {
                    let colours = &THEMES[0].colours;
                    node_ui(ui, graph, &resolved, colours, None, None, None, None, false, true, id, &mut events);
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
