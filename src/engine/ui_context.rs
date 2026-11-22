//! A node's window, and its work on the GUI's thread (see `Node::on_gui_tick`), is given a context (`UiContext`): the
//! node's inputs, the values its ports read, what its passes share, where it sends commands, and the buttons every node
//! has (`NodeToolbar`). A node handling a command (see `Node::on_command`) is given a smaller one (`CommandContext`).

use std::any::Any;
use std::cell::{RefCell, RefMut};
use std::sync::Arc;

use eframe::egui;

use super::{
    DataStreamInPorts, Edges, HasCommand, HasValue, Includes, NodeId, ReadsValIn, Shared, SharedByNode, ValInCmdOutPort,
};
use crate::commands::{Address, CmdSender, Commands};
use crate::theme::Colours;

/// The app keeps each node's state on the GUI's side (its `Node::UiState`) in one of these, whatever its type.
pub type UiStateCell = RefCell<Box<dyn Any>>;

/// Besides its own settings, a node's window gets this to work with (see `Node::on_gui_tick`).
pub struct UiContext<'a, E: Edges> {
    /// For each input, the data stream connected to it, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// For each of the node's `ValInCmdOut` and `ValIn` ports, what the node at the other end publishes, if the port's
    /// connected (see `val_in`).
    pub val_ins: &'a [Option<E::PublishedValue>],
    /// For each of the node's `ValOutCmdIn` ports, the names of the nodes with a live edge into the port.
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
    /// The app's queue of commands, which senders kept for later send to (see `cmd_sender`), if there is one (there
    /// isn't in tests, say).
    pub commands: Option<&'a Arc<Commands>>,
    /// The commands the window has sent through its `ValInCmdOut` ports (see `cmd_out`), each with the port's number.
    /// The app delivers them once the window has been drawn.
    pub sent: &'a mut Vec<(usize, E::Command)>,
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

    /// The data streams on the node's `DataStreamIn` ports, as the node's own `DataStreamIn` struct (`I`).
    pub fn data_stream_in<I: DataStreamInPorts<E>>(&self) -> I {
        I::of(self.inputs)
    }

    /// The value that `port` reads, or `None` if the port isn't connected. `port` is a handle generated by `ports!`
    /// for one of the node's `ValInCmdOut` or `ValIn` ports.
    pub fn val_in<P: ReadsValIn>(&self, port: P) -> Option<&'a P::Value>
    where
        E: HasValue<P::Value>,
        P::Value: 'a,
    {
        E::from_value(self.val_ins.get(port.index())?.as_ref()?)
    }

    /// Sends `command` through `port` to the node at the other end. The command must be one that the port declares.
    /// The app delivers the command once the window has been drawn, so the window shows the change on the next frame.
    pub fn cmd_out<V, S, C: std::fmt::Debug + Send + 'static, I>(&mut self, port: ValInCmdOutPort<V, S>, command: C)
    where
        E: HasCommand<C>,
        S: Includes<C, I>,
    {
        self.sent.push((port.index(), E::wrap(command)));
    }

    /// Sends an edited copy of the value that `port` reads back to the node at the other end, if the copy (`value`)
    /// differs from what the port reads. The command sent is `set(value)`, and the node at the other end updates the
    /// setting the value copies.
    pub fn cmd_out_if_changed<V, S, C: std::fmt::Debug + Send + 'static, I>(
        &mut self,
        port: ValInCmdOutPort<V, S>,
        value: V,
        set: fn(V) -> C,
    ) where
        E: HasValue<V> + HasCommand<C>,
        V: crate::engine::EdgeType + PartialEq + 'a,
        S: Includes<C, I>,
    {
        if self.val_in(port).is_some_and(|read| *read != value) {
            self.cmd_out(port, set(value));
        }
    }

    /// A sender the node can keep, to send commands later, from any thread, to whichever node `port` connects to at
    /// that time (see `CmdSender`). Returns `None` if the port isn't connected now, or if there's no command queue
    /// (in tests).
    pub fn cmd_sender<V, S>(&self, port: ValInCmdOutPort<V, S>) -> Option<CmdSender<E, S>> {
        let val_in_idx = port.index();
        self.val_ins.get(val_in_idx)?.as_ref()?;
        Some(CmdSender::new(self.commands?.clone(), Address::Port { from: self.node, val_in_idx }))
    }

    /// Opens the window of the node that `port` connects to, for example so that someone can change its settings
    /// there.
    pub fn open_val_in_target(&mut self, port: impl ReadsValIn) {
        if let Some(tools) = &mut self.tools {
            tools.open_val_in_target(port.index());
        }
    }

    /// Whether `port` is connected.
    pub fn val_in_connected(&self, port: impl ReadsValIn) -> bool {
        self.val_ins.get(port.index()).is_some_and(Option::is_some)
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
/// otherwise), and it opens other nodes' windows. A view uses these through its `UiContext`.
pub trait NodeToolbar {
    /// See `UiContext::open_val_in_target`.
    fn open_val_in_target(&mut self, val_in_idx: usize);

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

/// A node handling a command (see `Node::on_command`) gets this to work with: less than its window's `UiContext`, as
/// the app delivers commands whether the node's window is shown or not.
pub struct CommandContext<'a, E: Edges> {
    /// The node, and its name.
    pub node: NodeId,
    pub label: &'a str,
    /// For each of the node's `ValInCmdOut` and `ValIn` ports, what the node at the other end publishes, if it's
    /// connected.
    pub val_ins: &'a [Option<E::PublishedValue>],
    /// The node's state on the GUI's side (its `Node::UiState`), where it keeps work it starts, if the app has it (it
    /// always does for a command a node sent: see `ui_state`).
    pub ui_state: Option<&'a UiStateCell>,
    /// The app's queue of commands, for the node's own commands in turn (see `cmd_sender`), if there is one.
    pub commands: Option<&'a Arc<Commands>>,
    /// If the graph is running, this is set.
    pub running: bool,
}

impl<'a, E: Edges> CommandContext<'a, E> {
    pub fn new(
        node: NodeId,
        label: &'a str,
        val_ins: &'a [Option<E::PublishedValue>],
        ui_state: Option<&'a UiStateCell>,
        commands: Option<&'a Arc<Commands>>,
        running: bool,
    ) -> Self {
        CommandContext { node, label, val_ins, ui_state, commands, running }
    }

    /// The node finds its state on the GUI's side (its `Node::UiState`, which is `T`) here, if the app has it.
    pub fn ui_state<T: 'static>(&self) -> Option<RefMut<'a, T>> {
        let state = self.ui_state?.borrow_mut();
        Some(RefMut::map(state, |state| state.downcast_mut().expect("the node's own Node::UiState")))
    }

    /// The value that `port` reads, or `None` if the port isn't connected (see `UiContext::val_in`).
    pub fn val_in<P: ReadsValIn>(&self, port: P) -> Option<&'a P::Value>
    where
        E: HasValue<P::Value>,
        P::Value: 'a,
    {
        E::from_value(self.val_ins.get(port.index())?.as_ref()?)
    }

    /// See `UiContext::cmd_sender`.
    pub fn cmd_sender<V, S>(&self, port: ValInCmdOutPort<V, S>) -> Option<CmdSender<E, S>> {
        let val_in_idx = port.index();
        self.val_ins.get(val_in_idx)?.as_ref()?;
        Some(CmdSender::new(self.commands?.clone(), Address::Port { from: self.node, val_in_idx }))
    }
}
