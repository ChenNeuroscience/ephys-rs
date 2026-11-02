//! The buttons every node has, for its window (see `NodeToolbar`): copying and pasting its settings, showing it in
//! the basic view, enabling it, and deleting it. A node's default window has them beside its name; a view puts them
//! where it likes, through its `Context`.

use eframe::egui;

use super::Event;
use super::icons::Icon;
use super::toolbar;
use crate::core::engine::{NodeId, NodeToolbar};

pub struct NodeTools<'e> {
    pub id: NodeId,
    /// Whether it's in the basic view, and enabled, now.
    pub in_basic: bool,
    pub enabled: bool,
    /// What's clicked becomes these.
    pub events: &'e mut Vec<Event>,
}

impl NodeToolbar for NodeTools<'_> {
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

    fn basic_toggle(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let tip = if self.in_basic {
            "In the basic view: click to leave it out"
        } else {
            "Click to show it in the basic view"
        };
        let button = toolbar::small_toggle_button(ui, Icon::Home, "Show in basic view", tip, self.in_basic);
        if button.clicked() {
            self.events.push(Event::InBasic(self.id, !self.in_basic));
        }
        button
    }

    fn enabled_toggle(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let tip = if self.enabled {
            "Enabled: click to disable it (run the graph as if it weren't there, keeping it)"
        } else {
            "Disabled: click to enable it"
        };
        let button = toolbar::small_toggle_button(ui, Icon::Power, "Enabled", tip, self.enabled);
        if button.clicked() {
            self.events.push(Event::SetDisabled(self.id, self.enabled));
        }
        button
    }

    fn delete_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let button = toolbar::small_icon_button(ui, Icon::Delete, "Delete", "Delete this node");
        if button.clicked() {
            self.events.push(Event::Delete(self.id));
        }
        button
    }
}
