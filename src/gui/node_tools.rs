//! Every node's window has the same buttons (see `NodeToolbar`): copying and pasting its settings; and in advanced
//! mode, its settings menu (whether it shows in basic mode, and the pane's own options), enabling it, and deleting it.
//! The standard window has them beside its name; a view puts them where it likes, through its `UiContext`.

use eframe::egui;

use super::Event;
use super::icons::Icon;
use super::toolbar;
use crate::engine::{Graph, NodeId, NodeToolbar, NodesEnum, Port, PortRole};
use crate::triggers::{AnyTrigger, TriggerKey};

/// The app makes a node's toolbar each frame: what it shows, and where what's clicked goes.
pub struct NodeTools<'e> {
    pub id: NodeId,
    /// Whether the node's in the basic view, and enabled, now.
    pub show_in_basic: bool,
    pub enabled: bool,
    /// In basic mode, the buttons that change the graph are left out; this says whether the app's in advanced mode.
    pub advanced: bool,
    /// The triggers the node's own settings take; and for each of its controls, the node and the triggers of the
    /// setting it's connected to, if it is.
    pub own_triggers: Vec<TriggerKey>,
    pub controls: Vec<Option<(NodeId, Vec<TriggerKey>)>>,
    /// What's clicked becomes events, here, which the app handles once the panes are drawn.
    pub events: &'e mut Vec<Event>,
}

impl<'e> NodeTools<'e> {
    /// The app makes node `id`'s toolbar with this, knowing what the node can trigger, through its own settings and its
    /// controls.
    pub fn new<N: NodesEnum>(graph: &Graph<N>, id: NodeId, advanced: bool, events: &'e mut Vec<Event>) -> Option<Self> {
        let entry = graph.get(id)?;
        let node = &entry.node;
        let keys = |kinds: &[crate::engine::TriggerKind<N::Edges>]| kinds.iter().map(|&k| k.into()).collect();
        let own_triggers = (0..node.settings().len()).flat_map(|s| keys(node.triggers(s))).collect::<Vec<_>>();
        let controls = (0..node.controls().len())
            .map(|index| {
                let edge = graph.live_edges_from(Port { node: id, role: PortRole::Control, index }).next()?;
                Some((edge.to.node, keys(graph.get(edge.to.node)?.node.triggers(edge.to.port))))
            })
            .collect();
        let (show_in_basic, enabled) = (entry.shown_in_basic(), !entry.disabled);
        Some(NodeTools { id, show_in_basic, enabled, advanced, own_triggers, controls, events })
    }

    /// A trigger of kind `kind` goes to the node itself, or the one a control connects to. This finds that node, if it
    /// takes such triggers.
    fn trigger_node(&self, control: Option<usize>, kind: TriggerKey) -> Option<NodeId> {
        match control {
            None => self.own_triggers.contains(&kind).then_some(self.id),
            Some(control) => {
                let (node, triggers) = self.controls.get(control)?.as_ref()?;
                triggers.contains(&kind).then_some(*node)
            }
        }
    }
}

impl NodeToolbar for NodeTools<'_> {
    fn trigger(&mut self, control: Option<usize>, trigger: AnyTrigger) {
        if let Some(node) = self.trigger_node(control, trigger.key()) {
            self.events.push(Event::Trigger(node, trigger));
        }
    }

    fn can_trigger(&self, control: Option<usize>, kind: TriggerKey) -> bool {
        self.trigger_node(control, kind).is_some()
    }

    fn open_control_target(&mut self, control: usize) {
        if let Some(Some((node, _))) = self.controls.get(control) {
            let node = *node;
            self.events.push(Event::OpenWindow(node, Some(crate::layout::Pane::Node(self.id))));
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
    use crate::NodeType;
    use crate::Nodes;
    use crate::TriggerKind;
    use crate::engine::NodeTypeEnum;
    use crate::standard::other::run_command::RunOnFiles;

    /// A trigger goes to the app (as an event), running or not, if the node takes it.
    #[test]
    fn a_trigger_goes_to_the_app() {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let node_type = NodeType::named(name).unwrap();
            graph.add(Nodes::create(node_type), [0.0, 0.0])
        };
        let (command, gain) = (add(&mut graph, "Run Command"), add(&mut graph, "Spike Gain"));
        let mut events = Vec::new();
        let mut tools = NodeTools::new(&graph, command, true, &mut events).unwrap();
        assert!(tools.can_trigger(None, TriggerKind::RunOnFiles.into()));
        tools.trigger(None, AnyTrigger::new(crate::Trigger::from(RunOnFiles(Vec::new()))));
        assert!(matches!(events.as_slice(), [Event::Trigger(node, _)] if *node == command));

        let mut events = Vec::new();
        let mut tools = NodeTools::new(&graph, gain, true, &mut events).unwrap();
        assert!(!tools.can_trigger(None, TriggerKind::RunOnFiles.into()), "not what it takes");
        tools.trigger(None, AnyTrigger::new(crate::Trigger::from(RunOnFiles(Vec::new()))));
        assert!(events.is_empty());
    }
}
