//! A node's window, and its work on the GUI's thread (see `Node::on_gui_tick`), is given a context (`UiContext`): the
//! node's inputs, the settings its controls see, what its passes share, and the buttons every node has
//! (`NodeToolbar`).

use std::any::Any;
use std::cell::{RefCell, RefMut};

use eframe::egui;

use super::{Edges, NodeId, Shared, SharedByNode};
use crate::theme::Colours;
use crate::triggers::{AnyTrigger, TriggerKey};

/// The app keeps each node's state on the GUI's side (its `Node::UiState`) in one of these, whatever its type.
pub type UiStateCell = RefCell<Box<dyn Any>>;

/// Besides its own settings, a node's window gets this to work with (see `Node::on_gui_tick`).
pub struct UiContext<'a, E: Edges> {
    /// For each input, the data stream connected to it, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// For each control, the current value of the setting it's connected to, if any. The window edits that setting by
    /// changing the value.
    pub controls: &'a mut [Option<E::Setting>],
    /// For each setting, the names of the nodes with a control connected to it, while the edge is live.
    pub controlled_by: &'a [Vec<String>],
    /// The theme's colours to draw with, by role. A node can derive more of its own from these.
    pub theme: &'a Colours,
    /// The node, and its name, which the window can edit.
    pub node: NodeId,
    pub label: &'a mut String,
    /// The nodes' passes share some things with the GUI (see `crate::engine::Shared`): the run's, or once it's stopped,
    /// the last run's.
    pub shared_by_node: Option<&'a SharedByNode>,
    /// The node's state on the GUI's side (see `ui_state`).
    pub ui_state: &'a UiStateCell,
    /// Recordings go in a base directory (see `crate::base_directory`): the next one's, or the one going on's. A
    /// writer's window says where it'll write.
    pub base_directory: Option<&'a crate::base_directory::Place>,
    /// If the graph is running, this is set.
    pub running: bool,
    /// While recording, settings are locked, and changes to them are undone, so a view should show its settings
    /// disabled.
    pub locked: bool,
    /// Every node has the same buttons (copying and pasting its settings, showing it in the basic view, enabling it,
    /// deleting it), which a view puts where it likes: all together (`node_toolbar`), or one by one. There are none
    /// where there's no GUI to act on them (tests, say).
    pub tools: Option<&'a mut dyn NodeToolbar>,
}

impl<'a, E: Edges> UiContext<'a, E> {
    /// A node's passes share its `Node::Shared` (which is `T`) with the window. The window finds it here if the node
    /// runs, or ran in the run just stopped, so what's shown stays (`running` says which).
    pub fn shared<T: Shared>(&self) -> Option<&'a T> {
        self.shared_by_node?.get(self.node)
    }

    /// The window finds the node's state on the GUI's side (its `Node::UiState`, which is `T`) here. It's borrowed from
    /// the app, not the context, so the context can still be used while it's held.
    pub fn ui_state<T: 'static>(&self) -> RefMut<'a, T> {
        RefMut::map(self.ui_state.borrow_mut(), |state| state.downcast_mut().expect("the node's own Node::UiState"))
    }

    /// A view draws the node's toolbar (see `NodeToolbar::toolbar`) with this, if there's one here.
    pub fn node_toolbar(&mut self, ui: &mut egui::Ui) {
        self.node_toolbar_with(ui, &mut |_| {});
    }

    /// A view draws the node's toolbar with this, with `buttons` of the pane's own at the toolbar's left end, so at the
    /// right of the pane with the rest. The buttons are laid out right to left, and shown whether there's a toolbar
    /// here or not.
    pub fn node_toolbar_buttons(&mut self, ui: &mut egui::Ui, buttons: &mut dyn FnMut(&mut egui::Ui)) {
        match &mut self.tools {
            Some(tools) => tools.toolbar_buttons(ui, buttons, &mut |_| {}),
            None => {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), buttons);
            }
        }
    }

    /// A view draws the node's toolbar with this, with `settings` added to the toolbar's settings menu (in advanced
    /// mode): options of the pane's own, such as what it offers in basic mode.
    pub fn node_toolbar_with(&mut self, ui: &mut egui::Ui, settings: &mut dyn FnMut(&mut egui::Ui)) {
        if let Some(tools) = &mut self.tools {
            tools.toolbar_with(ui, settings);
        }
    }

    /// A window sends `trigger` (see `Node::triggers`) with this, to the setting that control `control` connects to, or
    /// (with `None`) to the node's own. If it can't be sent (see `can_trigger`), nothing happens.
    pub fn trigger(&mut self, control: Option<usize>, trigger: impl Into<E::Trigger>) {
        if let Some(tools) = &mut self.tools {
            tools.trigger(control, AnyTrigger::new(trigger.into()));
        }
    }

    /// A trigger of kind `kind` can only be sent to a setting that takes it (for a control, the one it's connected to).
    /// This says whether there's one.
    pub fn can_trigger(&self, control: Option<usize>, kind: super::TriggerKind<E>) -> bool {
        self.tools.as_ref().is_some_and(|t| t.can_trigger(control, kind.into()))
    }

    /// A view can open the window of the node control `control` connects to (to change its settings there, say).
    pub fn open_control_target(&mut self, control: usize) {
        if let Some(tools) = &mut self.tools {
            tools.open_control_target(control);
        }
    }

    /// This says whether control `control` is connected.
    pub fn control_connected(&self, control: usize) -> bool {
        self.controls.get(control).is_some_and(Option::is_some)
    }

    /// The app is in advanced mode or basic (see `crate::layout::Mode`). What a pane shows in each is up to the pane,
    /// but by convention, basic mode is simpler, and the pane's settings menu (see `node_toolbar_with`) offers what to
    /// show in basic mode. This says whether it's advanced, which it is where there's no toolbar (in tests, say).
    pub fn advanced(&self) -> bool {
        self.tools.as_ref().is_none_or(|t| t.advanced())
    }
}

/// The GUI does some things for every node's window: it has the buttons every node has, each on its own or all together
/// (those that change the graph, its settings, enabling and deleting it, only in advanced mode, giving `None`
/// otherwise), it sends triggers (see `Node::triggers`), and it opens other nodes' windows. A view uses these through
/// its `UiContext`.
pub trait NodeToolbar {
    /// See `UiContext::trigger`.
    fn trigger(&mut self, control: Option<usize>, trigger: AnyTrigger);
    /// See `UiContext::can_trigger`.
    fn can_trigger(&self, control: Option<usize>, kind: TriggerKey) -> bool;
    /// See `UiContext::open_control_target`.
    fn open_control_target(&mut self, control: usize);

    /// See `UiContext::advanced`.
    fn advanced(&self) -> bool;
    /// The button that copies the node's settings.
    fn copy_button(&mut self, ui: &mut egui::Ui) -> egui::Response;
    /// The button that pastes settings onto the node.
    fn paste_button(&mut self, ui: &mut egui::Ui) -> egui::Response;
    /// The node's settings menu: whether the node shows in basic mode, then `more` (the pane's own options).
    fn settings_menu(&mut self, ui: &mut egui::Ui, more: &mut dyn FnMut(&mut egui::Ui)) -> Option<egui::Response>;
    /// The switch that turns the node on or off.
    fn enabled_toggle(&mut self, ui: &mut egui::Ui) -> Option<egui::Response>;
    /// The button that deletes the node.
    fn delete_button(&mut self, ui: &mut egui::Ui) -> Option<egui::Response>;

    /// The toolbar draws all the buttons there are, in a row, laid out right to left (for the right of a toolbar): copy
    /// and paste; settings, and enabled; delete. In a wrapping toolbar, they go on a row of their own if there isn't
    /// room left.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        self.toolbar_with(ui, &mut |_| {});
    }

    /// The toolbar, with `settings` added to the settings menu.
    fn toolbar_with(&mut self, ui: &mut egui::Ui, settings: &mut dyn FnMut(&mut egui::Ui)) {
        self.toolbar_buttons(ui, &mut |_| {}, settings);
    }

    /// The toolbar, with `settings` added to the settings menu, and `buttons` at the left end.
    fn toolbar_buttons(
        &mut self,
        ui: &mut egui::Ui,
        buttons: &mut dyn FnMut(&mut egui::Ui),
        settings: &mut dyn FnMut(&mut egui::Ui),
    ) {
        let width_id = ui.scope_id().with("node toolbar width");
        let width: f32 = ui.data(|d| d.get_temp(width_id)).unwrap_or(0.0);
        if ui.available_size_before_wrap().x < width {
            ui.end_row();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if self.delete_button(ui).is_some() {
                ui.separator();
            }
            let enabled = self.enabled_toggle(ui);
            if self.settings_menu(ui, settings).is_some() || enabled.is_some() {
                ui.separator();
            }
            self.paste_button(ui);
            self.copy_button(ui);
            buttons(ui);
            let width = ui.min_rect().width();
            ui.data_mut(|d| d.insert_temp(width_id, width));
        });
    }
}
