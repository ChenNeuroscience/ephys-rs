//! The buttons every node has, for its window (see `NodeToolbar`): copying and pasting its settings; and in advanced
//! mode, its settings menu (whether it shows in basic mode, and the pane's own options), enabling it, and deleting
//! it. A node's default window has them beside its name; a view puts them
//! where it likes, through its `UiContext`.

use eframe::egui;

use super::Event;
use super::icons::Icon;
use super::toolbar;
use crate::core::engine::{Graph, NodeId, NodeSet, NodeToolbar, Port, PortRole};
use crate::core::exchange::Exchange;

/// A node's toolbar, for a frame: what it shows, and where what's clicked goes.
pub struct NodeTools<'e> {
    pub id: NodeId,
    /// Whether it's in the basic view, and enabled, now.
    pub show_in_basic: bool,
    pub enabled: bool,
    /// Whether the app's in advanced mode (otherwise, what changes the graph is left out).
    pub advanced: bool,
    /// The triggers of the node's own settings; and for each of its controls, the node and the triggers of the setting
    /// it's connected to, if it is.
    pub own_triggers: Vec<&'static str>,
    pub controls: Vec<Option<(NodeId, &'static [&'static str])>>,
    /// Where pass triggers go, while running.
    pub exchange: Option<&'e Exchange>,
    /// What's clicked becomes these.
    pub events: &'e mut Vec<Event>,
}

impl<'e> NodeTools<'e> {
    /// For node `id`'s window: what it can trigger, through its own settings and its controls.
    pub fn new<N: NodeSet>(
        graph: &Graph<N>,
        id: NodeId,
        advanced: bool,
        exchange: Option<&'e Exchange>,
        events: &'e mut Vec<Event>,
    ) -> Option<Self> {
        let entry = graph.get(id)?;
        let node = &entry.node;
        let own_triggers = (0..node.settings().len()).flat_map(|s| node.triggers(s).iter().copied()).collect();
        let controls = (0..node.controls().len())
            .map(|index| {
                let edge = graph.live_edges_from(Port { node: id, role: PortRole::Control, index }).next()?;
                Some((edge.to.node, graph.get(edge.to.node)?.node.triggers(edge.to.port)))
            })
            .collect();
        let (show_in_basic, enabled) = (entry.shown_in_basic(), !entry.disabled);
        Some(NodeTools { id, show_in_basic, enabled, advanced, own_triggers, controls, exchange, events })
    }

    /// The node whose trigger `name` is meant, if it has one: the node's own, or the one a control connects to.
    fn trigger_node(&self, control: Option<usize>, name: &'static str) -> Option<NodeId> {
        match control {
            None => self.own_triggers.contains(&name).then_some(self.id),
            Some(control) => {
                let (node, triggers) = self.controls.get(control).copied().flatten()?;
                triggers.contains(&name).then_some(node)
            }
        }
    }
}

impl NodeToolbar for NodeTools<'_> {
    fn trigger(&mut self, control: Option<usize>, name: &'static str) {
        if let (Some(node), Some(exchange)) = (self.trigger_node(control, name), self.exchange) {
            exchange.trigger(node, name);
        }
    }

    fn can_trigger(&self, control: Option<usize>, name: &'static str) -> bool {
        self.exchange.is_some() && self.trigger_node(control, name).is_some()
    }

    fn trigger_progress(&self, control: Option<usize>, name: &'static str) -> Option<f32> {
        self.exchange?.trigger_progress(self.trigger_node(control, name)?, name)
    }

    fn cancel_trigger(&mut self, control: Option<usize>, name: &'static str) {
        if let (Some(node), Some(exchange)) = (self.trigger_node(control, name), self.exchange) {
            exchange.cancel(node, name);
        }
    }

    fn open_control_target(&mut self, control: usize) {
        if let Some((node, _)) = self.controls.get(control).copied().flatten() {
            self.events.push(Event::OpenWindow(node, Some(crate::core::layout::Pane::Node(self.id))));
        }
    }

    fn copy_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let button = toolbar::small_icon_button(ui, Icon::Copy, "Copy settings", "Copy this node's settings");
        if button.clicked() {
            self.events.push(Event::CopySettings(vec![self.id]));
        }
        button
    }

    fn paste_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let tip = "Paste settings onto this node (if one node's were copied; otherwise choose where each goes)";
        let button = toolbar::small_icon_button(ui, Icon::Paste, "Paste settings", tip);
        if button.clicked() {
            self.events.push(Event::PasteSettings(Some(self.id)));
        }
        button
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui, more: &mut dyn FnMut(&mut egui::Ui)) -> Option<egui::Response> {
        if !self.advanced {
            return None;
        }
        let button = toolbar::small_icon_button(ui, Icon::Settings, "Settings", "This window's settings");
        egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            let mut show_in_basic = self.show_in_basic;
            let tick = ui.checkbox(&mut show_in_basic, "Show in basic mode");
            if tick.on_hover_text("Shown among the components in basic mode").changed() {
                self.events.push(Event::ShowInBasic(self.id, show_in_basic));
            }
            more(ui);
        });
        Some(button)
    }

    fn advanced(&self) -> bool {
        self.advanced
    }

    fn enabled_toggle(&mut self, ui: &mut egui::Ui) -> Option<egui::Response> {
        if !self.advanced {
            return None;
        }
        let tip = if self.enabled {
            "Enabled: click to disable it (run the graph as if it weren't there, keeping it)"
        } else {
            "Disabled: click to enable it"
        };
        let button = toolbar::small_toggle_button(ui, Icon::Power, "Enabled", tip, self.enabled);
        if button.clicked() {
            self.events.push(Event::SetDisabled(self.id, self.enabled));
        }
        Some(button)
    }

    fn delete_button(&mut self, ui: &mut egui::Ui) -> Option<egui::Response> {
        if !self.advanced {
            return None;
        }
        let button = toolbar::small_icon_button(ui, Icon::Delete, "Delete", "Delete this node");
        if button.clicked() {
            self.events.push(Event::Delete(self.id));
        }
        Some(button)
    }
}
