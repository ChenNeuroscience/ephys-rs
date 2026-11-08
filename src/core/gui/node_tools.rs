//! The buttons every node has, for its window (see `NodeToolbar`): copying and pasting its settings; and in advanced
//! mode, its settings menu (whether it shows in basic mode, and the pane's own options), enabling it, and deleting
//! it. A node's default window has them beside its name; a view puts them
//! where it likes, through its `UiContext`.

use eframe::egui;

use super::Event;
use super::icons::Icon;
use super::toolbar;
use crate::core::engine::{Graph, NodeId, NodeToolbar, NodesEnum, Port, PortRole};
use crate::core::exchange::Exchange;
use crate::core::triggers::{AnyTrigger, Delivery, TriggerKind};

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
    pub own_triggers: Vec<TriggerKind>,
    pub controls: Vec<Option<(NodeId, &'static [TriggerKind])>>,
    /// Where pass triggers go, while running. Direct triggers go through `events`.
    pub exchange: Option<&'e Exchange>,
    /// What's clicked becomes these.
    pub events: &'e mut Vec<Event>,
}

impl<'e> NodeTools<'e> {
    /// For node `id`'s window: what it can trigger, through its own settings and its controls.
    pub fn new<N: NodesEnum>(
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

    /// The node whose trigger of kind `kind` is meant, if it takes one: the node itself, or the one a control connects
    /// to.
    fn trigger_node(&self, control: Option<usize>, kind: TriggerKind) -> Option<NodeId> {
        match control {
            None => self.own_triggers.contains(&kind).then_some(self.id),
            Some(control) => {
                let (node, triggers) = self.controls.get(control).copied().flatten()?;
                triggers.contains(&kind).then_some(node)
            }
        }
    }
}

impl NodeToolbar for NodeTools<'_> {
    fn trigger(&mut self, control: Option<usize>, trigger: AnyTrigger) {
        let Some(node) = self.trigger_node(control, trigger.kind()) else { return };
        match (trigger.kind().delivery, self.exchange) {
            (Delivery::Pass, Some(exchange)) => exchange.trigger(node, trigger),
            (Delivery::Pass, None) => {}
            (Delivery::Direct, _) => self.events.push(Event::Trigger(node, trigger)),
        }
    }

    fn can_trigger(&self, control: Option<usize>, kind: TriggerKind) -> bool {
        let running = self.exchange.is_some() || kind.delivery == Delivery::Direct;
        running && self.trigger_node(control, kind).is_some()
    }

    fn trigger_progress(&self, control: Option<usize>, kind: TriggerKind) -> Option<f32> {
        if kind.delivery != Delivery::Pass {
            return None;
        }
        self.exchange?.trigger_progress(self.trigger_node(control, kind)?, kind)
    }

    fn cancel_trigger(&mut self, control: Option<usize>, kind: TriggerKind) {
        if let (Some(node), Some(exchange)) = (self.trigger_node(control, kind), self.exchange) {
            exchange.cancel(node, kind);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nodes;
    use crate::core::triggers::Trigger;
    use crate::standard_components::auto_detect::AutoDetect;
    use crate::standard_components::other::run_command::RunOnFiles;

    /// A direct trigger is sent whether or not anything's running, to the app (as an event); a pass trigger only to a
    /// run.
    #[test]
    fn a_trigger_goes_the_way_it_is_delivered() {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), [0.0, 0.0])
        };
        let (command, gain) = (add(&mut graph, "Run Command"), add(&mut graph, "Spike Gain"));
        let exchange = Exchange::default();
        for running in [false, true] {
            let live = running.then_some(&exchange);
            let mut events = Vec::new();
            let mut tools = NodeTools::new(&graph, command, true, live, &mut events).unwrap();
            assert!(tools.can_trigger(None, RunOnFiles::KIND), "running or not");
            assert!(!tools.can_trigger(None, AutoDetect::KIND), "not what it takes");
            tools.trigger(None, AnyTrigger::new(RunOnFiles(Vec::new())));
            assert_eq!(tools.trigger_progress(None, RunOnFiles::KIND), None, "no progress to say");
            assert!(matches!(events.as_slice(), [Event::Trigger(node, _)] if *node == command));

            let mut events = Vec::new();
            let mut tools = NodeTools::new(&graph, gain, true, live, &mut events).unwrap();
            assert_eq!(tools.can_trigger(None, AutoDetect::KIND), running, "only to a run");
            tools.trigger(None, AnyTrigger::new(AutoDetect));
            assert!(events.is_empty());
            assert_eq!(exchange.trigger_progress(gain, AutoDetect::KIND), running.then_some(0.0));
        }
    }
}
