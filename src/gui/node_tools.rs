//! Every node's window has the same buttons (see `NodeToolbar`): copying and pasting its settings; and in advanced
//! mode, its settings menu (whether it shows in basic mode, and the pane's own options), enabling it, and deleting it.
//! The standard window has them beside its name; a view puts them where it likes, through its `UiContext`.

use eframe::egui;

use super::Event;
use super::icons::Icon;
use super::toolbar;
use crate::engine::{Graph, NodeId, NodeToolbar, NodesEnum, PortRole};

/// The app makes a node's toolbar each frame: what it shows, and where what's clicked goes.
pub struct NodeTools<'e> {
    pub id: NodeId,
    /// Whether the node's in the basic view, and enabled, now.
    pub show_in_basic: bool,
    pub enabled: bool,
    /// In basic mode, the buttons that change the graph are left out; this says whether the app's in advanced mode.
    pub advanced: bool,
    /// For each of the node's `ValInCmdOut` and `ValIn` ports, the node it's connected to, if it is.
    pub controls: Vec<Option<NodeId>>,
    /// What's clicked becomes events, here, which the app handles once the panes are drawn.
    pub events: &'e mut Vec<Event>,
}

impl<'e> NodeTools<'e> {
    /// The app makes node `id`'s toolbar with this, knowing what its `ValInCmdOut` and `ValIn` ports connect to.
    pub fn new<N: NodesEnum>(graph: &Graph<N>, id: NodeId, advanced: bool, events: &'e mut Vec<Event>) -> Option<Self> {
        let entry = graph.get(id)?;
        let count = entry.node.val_cmd_ports().iter().filter(|p| p.role() != PortRole::ValOutCmdIn).count();
        let controls = (0..count).map(|port| Some(graph.val_in_target(id, port)?.node)).collect();
        let (show_in_basic, enabled) = (entry.shown_in_basic(), !entry.disabled);
        Some(NodeTools { id, show_in_basic, enabled, advanced, controls, events })
    }
}

impl NodeToolbar for NodeTools<'_> {
    fn open_control_target(&mut self, val_in_idx: usize) {
        if let Some(Some(node)) = self.controls.get(val_in_idx) {
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
