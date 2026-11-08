//! What a node's window works with (`UiContext`): its inputs, the settings its controls see, what its passes share,
//! and the buttons every node has (`NodeToolbar`).

use std::any::Any;
use std::cell::{RefCell, RefMut};

use eframe::egui;

use super::{Edges, NodeId, Shared, SharedByNode};
use crate::core::theme::Colours;
use crate::core::triggers::{AnyTrigger, TriggerKey};

/// A node's state on the GUI's side (its `Node::UiState`), as the app keeps it.
pub type UiStateCell = RefCell<Box<dyn Any>>;

/// What a node's UI gets to work with besides its own settings.
pub struct UiContext<'a, E: Edges> {
    /// For each input, the data stream connected to it, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// For each control, the current value of the setting it's connected to, if any. Changing a value
    /// edits that setting.
    pub controls: &'a mut [Option<E::Setting>],
    /// For each setting, the labels of the nodes with a control connected to it, for as long as the edge is live.
    pub controlled_by: &'a [Vec<String>],
    /// The colours to draw with, by role. A node can derive more of its own from these.
    pub theme: &'a Colours,
    /// The node.
    pub node: NodeId,
    /// What the nodes' passes share with the GUI (see `crate::core::engine::Shared`): the run's, or the last run's, once
    /// it's stopped.
    pub shared_by_node: Option<&'a SharedByNode>,
    /// The node's state on the GUI's side (see `ui_state`).
    pub ui_state: &'a UiStateCell,
    /// The base directory recordings go in (see `crate::core::base_directory`): the next one's, or the one going
    /// on's. For a writer to say where it'll write.
    pub base_directory: Option<&'a crate::core::base_directory::Place>,
    /// Whether the graph is running.
    pub running: bool,
    /// Whether settings are locked (while recording): a view should show its settings disabled, as changes to
    /// them are undone.
    pub locked: bool,
    /// The buttons every node has (copying and pasting its settings, showing it in the basic view, enabling it,
    /// deleting it), for a view to put where it likes: all together (`node_toolbar`), or one by one. None where
    /// there's no GUI to act on them (tests, say).
    pub tools: Option<&'a mut dyn NodeToolbar>,
}

impl<'a, E: Edges> UiContext<'a, E> {
    /// What the node's passes share (its `Node::Shared`, which is `T`), if it runs, or ran in the run just stopped (so
    /// what's shown stays: `running` says which).
    pub fn shared<T: Shared>(&self) -> Option<&'a T> {
        self.shared_by_node?.get(self.node)
    }

    /// The node's state on the GUI's side (its `Node::UiState`, which is `T`). Borrowed from the app, not the context,
    /// so the context can still be used while it's held.
    pub fn ui_state<T: 'static>(&self) -> RefMut<'a, T> {
        RefMut::map(self.ui_state.borrow_mut(), |state| state.downcast_mut().expect("the node's own Node::UiState"))
    }

    /// The node's toolbar (see `NodeToolbar::toolbar`), if there is one here.
    pub fn node_toolbar(&mut self, ui: &mut egui::Ui) {
        self.node_toolbar_with(ui, &mut |_| {});
    }

    /// The node's toolbar, with `buttons` of the pane's own at its left end, so at the right of the pane with the rest.
    /// They're laid out right to left. They're shown whether there's a toolbar here or not.
    pub fn node_toolbar_buttons(&mut self, ui: &mut egui::Ui, buttons: &mut dyn FnMut(&mut egui::Ui)) {
        match &mut self.tools {
            Some(tools) => tools.toolbar_buttons(ui, buttons, &mut |_| {}),
            None => {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), buttons);
            }
        }
    }

    /// The node's toolbar, with `settings` added to its settings menu (in advanced mode): options of the pane's own,
    /// such as what it offers in basic mode.
    pub fn node_toolbar_with(&mut self, ui: &mut egui::Ui, settings: &mut dyn FnMut(&mut egui::Ui)) {
        if let Some(tools) = &mut self.tools {
            tools.toolbar_with(ui, settings);
        }
    }

    /// Sends `trigger` (see `Node::triggers`) to the setting that control `control` connects to, or (with `None`) to
    /// the node's own, however it's delivered. Nothing, if it can't be (see `can_trigger`).
    pub fn trigger(&mut self, control: Option<usize>, trigger: impl Into<E::Trigger>) {
        if let Some(tools) = &mut self.tools {
            tools.trigger(control, AnyTrigger::new(trigger.into()));
        }
    }

    /// Whether a trigger of kind `kind` can be sent: to a setting that takes it (for a control, the one it's connected
    /// to), and for a pass trigger, the graph running.
    pub fn can_trigger(&self, control: Option<usize>, kind: super::TriggerKind<E>) -> bool {
        self.tools.as_ref().is_some_and(|t| t.can_trigger(control, kind.into()))
    }

    /// How far along the pass trigger of kind `kind` is (0 to 1), if it's been sent and isn't done. Never, for a direct
    /// trigger.
    pub fn trigger_progress(&self, control: Option<usize>, kind: super::TriggerKind<E>) -> Option<f32> {
        self.tools.as_ref().and_then(|t| t.trigger_progress(control, kind.into()))
    }

    /// Stops the pass trigger of kind `kind`, if it's been sent and isn't done: the node abandons what it was doing for
    /// it.
    pub fn cancel_trigger(&mut self, control: Option<usize>, kind: super::TriggerKind<E>) {
        if let Some(tools) = &mut self.tools {
            tools.cancel_trigger(control, kind.into());
        }
    }

    /// Opens the window of the node control `control` connects to (to change its settings there, say).
    pub fn open_control_target(&mut self, control: usize) {
        if let Some(tools) = &mut self.tools {
            tools.open_control_target(control);
        }
    }

    /// Whether control `control` is connected.
    pub fn control_connected(&self, control: usize) -> bool {
        self.controls.get(control).is_some_and(Option::is_some)
    }

    /// Whether the app's in advanced mode (see `crate::core::layout::Mode`), rather than basic: what a pane
    /// shows in each is up to it, but by convention, basic mode is simpler, and its settings menu (see
    /// `node_toolbar_with`) offers what to show in basic mode. Advanced, where there's no toolbar (in tests, say).
    pub fn advanced(&self) -> bool {
        self.tools.as_ref().is_none_or(|t| t.advanced())
    }
}

/// What the GUI does for a node's window: the buttons every node has, each on its own or all together (those that
/// change the graph, its settings, enabling and deleting it, only there in advanced mode, giving `None` otherwise);
/// and sending triggers (see `Node::triggers`), and opening other nodes' windows. A view uses these through its
/// `UiContext`.
pub trait NodeToolbar {
    /// Sends a trigger (see `UiContext::trigger`).
    fn trigger(&mut self, control: Option<usize>, trigger: AnyTrigger);
    fn can_trigger(&self, control: Option<usize>, kind: TriggerKey) -> bool;
    fn trigger_progress(&self, control: Option<usize>, kind: TriggerKey) -> Option<f32>;
    /// Stops a trigger sent (see `UiContext::cancel_trigger`).
    fn cancel_trigger(&mut self, control: Option<usize>, kind: TriggerKey);
    fn open_control_target(&mut self, control: usize);

    /// Whether the app's in advanced mode.
    fn advanced(&self) -> bool;
    /// Copy its settings.
    fn copy_button(&mut self, ui: &mut egui::Ui) -> egui::Response;
    /// Paste settings onto it.
    fn paste_button(&mut self, ui: &mut egui::Ui) -> egui::Response;
    /// Its settings, in a menu: whether it shows in basic mode, then `more` (the pane's own options).
    fn settings_menu(&mut self, ui: &mut egui::Ui, more: &mut dyn FnMut(&mut egui::Ui)) -> Option<egui::Response>;
    /// Whether it's enabled: on or off.
    fn enabled_toggle(&mut self, ui: &mut egui::Ui) -> Option<egui::Response>;
    /// Delete it.
    fn delete_button(&mut self, ui: &mut egui::Ui) -> Option<egui::Response>;

    /// All of them (that are there), in a row, laid out right to left (for the right of a toolbar): copy and paste;
    /// settings, and enabled; delete. In a wrapping toolbar, on a row of their own if there isn't room left.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        self.toolbar_with(ui, &mut |_| {});
    }

    /// All of them, with `settings` added to the settings menu.
    fn toolbar_with(&mut self, ui: &mut egui::Ui, settings: &mut dyn FnMut(&mut egui::Ui)) {
        self.toolbar_buttons(ui, &mut |_| {}, settings);
    }

    /// All of them, with `settings` added to the settings menu, and `buttons` at the left end.
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
