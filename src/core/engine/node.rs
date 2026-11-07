//! The `Node` trait implemented by each node type, and `NodeSet`, the list of types a program is built with.

use std::any::{Any, TypeId};
use std::sync::Arc;

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Edges, Kind, NodeId, Pass};
use crate::core::exchange::{Exchange, Shared};
use crate::core::theme::Colours;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Source,
    Processing,
    Visualisation,
    Recording,
    /// Nodes that aren't about data as it's acquired.
    Other,
}

impl Category {
    pub const ALL: [Category; 5] =
        [Category::Source, Category::Processing, Category::Visualisation, Category::Recording, Category::Other];

    pub fn name(self) -> &'static str {
        match self {
            Category::Source => "Sources",
            Category::Processing => "Processing",
            Category::Visualisation => "Visualisation",
            Category::Recording => "Recording",
            Category::Other => "Other",
        }
    }
}

/// Static facts about a node type.
pub struct NodeInfo {
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
}

pub struct Input<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::DataStream>,
}

pub struct Output<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::DataStream>,
    /// What the output produces, or `None` while that can't be known (e.g. its input isn't connected).
    pub stream: Option<E::DataStream>,
}

/// A setting that other nodes can edit through a settings edge, with its current value.
pub struct SettingPort<E: Edges> {
    pub name: &'static str,
    pub value: E::Setting,
    /// Which side of the node it's on, on the graph: usually the right.
    pub on_right: bool,
}

/// A setting of some other node that this one can see, and edit unless it's read-only, through a settings edge.
pub struct Control<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::Setting>,
    /// Only reads the setting (a writer recording it, say): changes the node makes to it are ignored.
    pub read_only: bool,
    /// Which side of the node it's on, on the graph: usually the left.
    pub on_right: bool,
}

/// What a node's UI gets to work with besides its own settings.
pub struct Context<'a, E: Edges> {
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
    /// What the node's passes and window share (see `crate::core::exchange`): the run's, or the last run's, while paused.
    pub exchange: Option<&'a Exchange>,
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

impl<E: Edges> Context<'_, E> {
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

    /// Triggers the pass trigger `name` (see `Node::triggers`) of the setting that control `control` connects to, or (with
    /// `None`) of the node's own. Nothing, if it can't be (see `can_trigger`).
    pub fn trigger(&mut self, control: Option<usize>, name: &'static str) {
        if let Some(tools) = &mut self.tools {
            tools.trigger(control, name);
        }
    }

    /// Whether the pass trigger can be triggered: the graph running, and (for a control) connected to a setting that has it.
    pub fn can_trigger(&self, control: Option<usize>, name: &'static str) -> bool {
        self.tools.as_ref().is_some_and(|t| t.can_trigger(control, name))
    }

    /// How far along the trigger is (0 to 1), if it's been triggered and isn't done.
    pub fn trigger_progress(&self, control: Option<usize>, name: &'static str) -> Option<f32> {
        self.tools.as_ref().and_then(|t| t.trigger_progress(control, name))
    }

    /// Stops the trigger, if it's been triggered and isn't done: the node abandons what it was doing for it.
    pub fn cancel_trigger(&mut self, control: Option<usize>, name: &'static str) {
        if let Some(tools) = &mut self.tools {
            tools.cancel_trigger(control, name);
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
/// and triggering pass triggers (see `Node::triggers`), and opening other nodes' windows. A view uses these through its
/// `Context`.
pub trait NodeToolbar {
    /// Triggers a pass trigger (see `Context::trigger`).
    fn trigger(&mut self, control: Option<usize>, name: &'static str);
    fn can_trigger(&self, control: Option<usize>, name: &'static str) -> bool;
    fn trigger_progress(&self, control: Option<usize>, name: &'static str) -> Option<f32>;
    /// Stops a trigger triggered (see `Context::cancel_trigger`).
    fn cancel_trigger(&mut self, control: Option<usize>, name: &'static str);
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

impl<E: Edges> Context<'_, E> {
    /// The node's shared `T` (see `crate::core::exchange`), if a run's passes have made one. A run stopped leaves it as it
    /// was: that's a pause, so what's shown stays, and `running` says which it is.
    pub fn shared<T: Shared>(&self) -> Option<Arc<T>> {
        self.exchange?.get(self.node)
    }
}

/// A thread a node runs of its own while the graph runs (acquiring from a board, say), shown in the dev tools.
pub struct NodeThread {
    pub name: &'static str,
    /// How the thread and the node's passes hand data to each other, e.g. "block queue".
    pub link: &'static str,
    /// Which way the data goes: to the passes (from a board, say) or from them (to disk).
    pub flow: Flow,
    /// Why it's a thread of its own, rather than done in the passes.
    pub why: &'static str,
    /// Whether the data's buffers are swapped with the passes' own (through a pool: see `Node::thread_pool`), rather
    /// than copied.
    pub swaps: bool,
}

/// Where the buffers are that a node's thread swaps with its passes, rather than copying (see `Node::thread_pool`):
/// so many in the pool, of which some are empty (ready to fill), one's maybe being filled (or, by a writer,
/// written), and some are full (waiting to be taken by a pass, or written). Swaps move buffers between the pool
/// and the workers', which aren't counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStatus {
    pub size: usize,
    /// Each buffer's memory (for a source's, all its outputs for a pass, together).
    pub bytes: usize,
    /// Which of the node's ports the buffers are like, a bit each: its outputs (for a source) or inputs (for a
    /// writer).
    pub ports: u64,
    pub empty: usize,
    pub filling: bool,
    pub full: usize,
    /// Swaps with the passes so far.
    pub swaps: u64,
    /// Data lost because the pool ran out of buffers: the passes, or the disk, not keeping up.
    pub overflows: u64,
}

/// Which way data goes between a node's thread and its passes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    ToPasses,
    FromPasses,
}

/// What a node's drawing of one of its threads (in the dev tools) gets to work with.
pub struct ThreadContext<'a> {
    pub theme: &'a Colours,
    pub node: NodeId,
    /// What the node's passes and window share (see `crate::core::exchange`), while running.
    pub exchange: Option<&'a Exchange>,
}

impl ThreadContext<'_> {
    /// The node's shared `T` (see `crate::core::exchange`), if it's running and its passes have made one.
    pub fn shared<T: Shared>(&self) -> Option<Arc<T>> {
        self.exchange?.get(self.node)
    }
}

/// What a node carries from one pass to the next. Passes wait for each other at a node with state, handing it on
/// in order (a sync point), so each kind of state has to say why it's needed: the dev tools show it there.
pub trait PassState: Default + Send + 'static {
    /// Why the node can't do without it, precisely: what it carries over, and what for. Empty for `()` (none).
    const WHY: &'static str;

    /// Whether it carries anything over from pass to pass. If it doesn't (it's only working memory, kept so it
    /// isn't remade every pass), each worker has its own instead, and passes needn't wait for each other.
    const CARRIED_OVER: bool = true;
}

impl PassState for () {
    const WHY: &'static str = "";
}

/// A node type. Implementors are plain settings structs, saved with the graph.
///
/// The ports may depend on the settings, and the output data streams on what is connected to the inputs; the
/// graph works these out in order from the sources down (see `Graph::resolve`).
pub trait Node<E: Edges>: Default + Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    const INFO: NodeInfo;

    /// What the node carries from one pass to the next (a filter's history, say), or `()` for nothing.
    /// Passes run on several threads at once, so a node with state is run for one pass at a time, in order;
    /// nodes without it can run for several passes at once.
    type State: PassState;

    /// Whether the node's window is a view of its own (a viewer, say) rather than just its settings.
    const HAS_VIEW: bool = false;

    fn inputs(&self) -> Vec<Input<E>> {
        Vec::new()
    }

    /// `inputs` has an entry for each of `self.inputs()`: the data stream connected to it, if any.
    fn outputs(&self, inputs: &[Option<E::DataStream>]) -> Vec<Output<E>>;

    /// Problems with the configuration, shown on the node.
    fn warnings(&self, _inputs: &[Option<E::DataStream>]) -> Vec<String> {
        Vec::new()
    }

    /// Settings that other nodes may edit, with their current values.
    fn settings(&self) -> Vec<SettingPort<E>> {
        Vec::new()
    }

    /// Called with a new value for one of `self.settings()`, of the same kind.
    fn set_setting(&mut self, _index: usize, _value: E::Setting) {}

    fn controls(&self) -> Vec<Control<E>> {
        Vec::new()
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<E>);

    fn view_ui(&mut self, _ui: &mut egui::Ui, _context: &mut Context<E>) {}

    /// The state for the first pass. `inputs` has the data stream connected to each input, if any.
    fn initial_state(&self, _inputs: &[Option<E::DataStream>]) -> Self::State {
        Self::State::default()
    }

    /// An input that an output can be made from in place, editing the input's data where it is. The plan
    /// decides, before running, whether it can be: it can if every other node reading that data can run
    /// first. The output must be the same data stream as the input.
    fn in_place(&self, _output: usize) -> Option<usize> {
        None
    }

    /// Whether the node needs an input's data now, given its settings (a viewer with part of its view hidden might
    /// not). If not, the plan doesn't give it the data; and a node whose outputs nothing needs doesn't run at all
    /// (except sources, which run regardless). Changing what's needed changes the plan (a new epoch), so it's for
    /// what's switched on and off, not what changes pass to pass.
    fn needs(&self, _input: usize) -> bool {
        true
    }

    /// The pass triggers of one of the node's settings (by index): what can be triggered through it, as well as set,
    /// while running: "auto detect", say. A control connected to it can trigger one (`Context::trigger`), as can the
    /// node's own window; its passes take it (`Pass::triggered`), and may update its settings when they're done
    /// (`Pass::trigger_settings_update`).
    fn triggers(&self, _setting: usize) -> &'static [&'static str] {
        &[]
    }

    /// The direct triggers of one of the node's settings (by index): what can be triggered through it without a pass, or
    /// a run, delivered to `on_trigger` on the GUI's thread. See `crate::core::triggers`.
    fn direct_triggers(&self, _setting: usize) -> &'static [&'static str] {
        &[]
    }

    /// One of its `direct_triggers`, with what was sent with it.
    fn on_trigger(&mut self, _name: &'static str, _payload: Box<dyn Any + Send>) {}

    /// Whether the node takes an input's buffer, to do as it likes with the data (swap it for another just as big,
    /// say, to hand to its thread or window), rather than only reading it. Nodes don't copy their inputs: the plan
    /// does, when it needs to. The pass always gives the node a buffer to take (see `Pass::take_input`): the input's
    /// own if no node edits that data in place and every other node reading it can run first, otherwise a copy the
    /// plan makes as soon as the data's made.
    fn takes(&self, _input: usize) -> bool {
        false
    }

    /// Does the node's work for one pass: reads its inputs and fills its outputs, which are already sized for
    /// the data streams they carry. When an output is being made in place (see `in_place`), its input is `None`
    /// in the pass, and the output's buffer already holds the input's data to transform.
    fn process(&self, _pass: &mut Pass<E>, _state: &mut Self::State) {}

    /// Gets ready to record (a writer opening its files, say), given the data streams on its inputs, what its
    /// controls see, the base directory recordings go in, if there is one (see `crate::core::base_directory`), and what
    /// its controls can trigger directly (see `crate::core::triggers`).
    /// Called on a thread of its own when recording's asked for; passes only start recording once every node's
    /// ready, so this can take its time. What it returns is the node's (with `Pass::take_prepared`) on the first
    /// pass that records.
    fn prepare_to_record(
        &self,
        _inputs: &[Option<E::DataStream>],
        _controls: &[Option<E::Setting>],
        _base_directory: Option<&crate::core::base_directory::Place>,
        _triggers: &crate::core::triggers::ControlTriggers,
    ) -> Option<Box<dyn Any + Send>> {
        None
    }

    /// Threads the node runs of its own while running (started with its state, say), for the dev tools.
    fn threads(&self) -> Vec<NodeThread> {
        Vec::new()
    }

    /// Draws what one of `threads()` (by index) is up to, in the dev tools: briefly, a few lines.
    fn thread_ui(&self, _thread: usize, _ui: &mut egui::Ui, _context: &ThreadContext) {}

    /// Where the buffers are that one of `threads()` (by index) swaps with the passes, if it does, for the dev tools
    /// to show between them.
    fn thread_pool(&self, _thread: usize, _context: &ThreadContext) -> Option<PoolStatus> {
        None
    }
}

/// The closed set of node types a program is built with, as an enum with one variant per type. Implemented
/// by `node_set!`, and forwards each `Node` method to the variant's type.
#[allow(clippy::type_complexity)]
pub trait NodeSet: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    type Edges: Edges;

    /// Indexed by type index.
    const TYPES: &'static [NodeInfo];

    /// A node of the given type, with default settings.
    fn create(type_index: usize) -> Self;
    fn type_index(&self) -> usize;
    fn has_view(&self) -> bool;
    fn inputs(&self) -> Vec<Input<Self::Edges>>;
    fn outputs(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<Output<Self::Edges>>;
    fn warnings(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<String>;
    fn settings(&self) -> Vec<SettingPort<Self::Edges>>;
    fn set_setting(&mut self, index: usize, value: <Self::Edges as Edges>::Setting);
    fn controls(&self) -> Vec<Control<Self::Edges>>;
    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Self::Edges>);
    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Self::Edges>);
    fn in_place(&self, output: usize) -> Option<usize>;
    fn takes(&self, input: usize) -> bool;
    fn needs(&self, input: usize) -> bool;
    fn triggers(&self, setting: usize) -> &'static [&'static str];
    fn direct_triggers(&self, setting: usize) -> &'static [&'static str];
    fn on_trigger(&mut self, name: &'static str, payload: Box<dyn Any + Send>);
    /// Whether the node carries state from one pass to the next.
    fn has_state(&self) -> bool;
    /// Why it does (see `PassState::WHY`).
    fn state_reason(&self) -> &'static str;
    /// Whether it carries its state over, or it's only working memory (see `PassState::CARRIED_OVER`).
    fn state_carried_over(&self) -> bool;
    fn initial_state(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Box<dyn Any + Send>;
    /// `state` is the node's state, if it has any (as made by `initial_state`).
    fn process(&self, pass: &mut Pass<Self::Edges>, state: Option<&mut (dyn Any + Send)>);
    fn prepare_to_record(
        &self,
        inputs: &[Option<<Self::Edges as Edges>::DataStream>],
        controls: &[Option<<Self::Edges as Edges>::Setting>],
        base_directory: Option<&crate::core::base_directory::Place>,
        triggers: &crate::core::triggers::ControlTriggers,
    ) -> Option<Box<dyn Any + Send>>;
    fn threads(&self) -> Vec<NodeThread>;
    fn thread_ui(&self, thread: usize, ui: &mut egui::Ui, context: &ThreadContext);
    fn thread_pool(&self, thread: usize, context: &ThreadContext) -> Option<PoolStatus>;

    fn info(&self) -> &'static NodeInfo {
        &Self::TYPES[self.type_index()]
    }
}

/// Whether `S` is real state, rather than `()` for none.
pub fn is_state<S: 'static>() -> bool {
    TypeId::of::<S>() != TypeId::of::<()>()
}

/// Declares the enum of node types a program is built with, and implements `NodeSet` for it:
///
/// ```ignore
/// node_set! {
///     edges: Edges;
///     pub enum Nodes {
///         Gain(gain::Gain),
///         Writer(writer::Writer),
///     }
/// }
/// ```
///
/// where `Edges` was declared with `edge_types!`. Saved graphs name each node by its variant, so renaming a
/// variant breaks existing files.
#[macro_export]
macro_rules! node_set {
    (edges: $edges:ty; $(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident($ty:ty)),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "settings")]
        $vis enum $name {
            $($variant($ty)),+
        }

        impl $crate::core::engine::NodeSet for $name {
            type Edges = $edges;

            const TYPES: &'static [$crate::core::engine::NodeInfo] =
                &[$(<$ty as $crate::core::engine::Node<$edges>>::INFO),+];

            #[allow(unused_assignments)]
            fn create(type_index: usize) -> Self {
                let mut i = 0;
                $(
                    if i == type_index {
                        return $name::$variant(<$ty>::default());
                    }
                    i += 1;
                )+
                panic!("no node type {type_index}")
            }

            #[allow(unused_assignments)]
            fn type_index(&self) -> usize {
                let mut i = 0;
                $(
                    if let $name::$variant(_) = self {
                        return i;
                    }
                    i += 1;
                )+
                unreachable!()
            }

            fn has_view(&self) -> bool {
                match self { $($name::$variant(_) => <$ty as $crate::core::engine::Node<$edges>>::HAS_VIEW),+ }
            }

            fn inputs(&self) -> Vec<$crate::core::engine::Input<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::inputs(n)),+ }
            }

            fn outputs(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
            ) -> Vec<$crate::core::engine::Output<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::outputs(n, inputs)),+ }
            }

            fn warnings(&self, inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>]) -> Vec<String> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::warnings(n, inputs)),+ }
            }

            fn settings(&self) -> Vec<$crate::core::engine::SettingPort<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::settings(n)),+ }
            }

            fn set_setting(&mut self, index: usize, value: <$edges as $crate::core::engine::Edges>::Setting) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::set_setting(n, index, value)),+ }
            }

            fn controls(&self) -> Vec<$crate::core::engine::Control<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::controls(n)),+ }
            }

            fn settings_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::core::engine::Context<$edges>) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::settings_ui(n, ui, context)),+ }
            }

            fn view_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::core::engine::Context<$edges>) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::view_ui(n, ui, context)),+ }
            }

            fn in_place(&self, output: usize) -> Option<usize> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::in_place(n, output)),+ }
            }

            fn needs(&self, input: usize) -> bool {
                match self { $($name::$variant(n) => $crate::core::engine::Node::needs(n, input)),+ }
            }

            fn triggers(&self, setting: usize) -> &'static [&'static str] {
                match self { $($name::$variant(n) => $crate::core::engine::Node::triggers(n, setting)),+ }
            }

            fn direct_triggers(&self, setting: usize) -> &'static [&'static str] {
                match self { $($name::$variant(n) => $crate::core::engine::Node::direct_triggers(n, setting)),+ }
            }

            fn on_trigger(&mut self, name: &'static str, payload: Box<dyn std::any::Any + Send>) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::on_trigger(n, name, payload)),+ }
            }

            fn takes(&self, input: usize) -> bool {
                match self { $($name::$variant(n) => $crate::core::engine::Node::takes(n, input)),+ }
            }

            fn has_state(&self) -> bool {
                match self {
                    $($name::$variant(_) => {
                        $crate::core::engine::is_state::<<$ty as $crate::core::engine::Node<$edges>>::State>()
                    }),+
                }
            }

            fn state_carried_over(&self) -> bool {
                match self {
                    $($name::$variant(_) => {
                        <<$ty as $crate::core::engine::Node<$edges>>::State as $crate::core::engine::PassState>::CARRIED_OVER
                    }),+
                }
            }

            fn state_reason(&self) -> &'static str {
                match self {
                    $($name::$variant(_) => {
                        <<$ty as $crate::core::engine::Node<$edges>>::State as $crate::core::engine::PassState>::WHY
                    }),+
                }
            }

            fn initial_state(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
            ) -> Box<dyn std::any::Any + Send> {
                match self { $($name::$variant(n) => Box::new($crate::core::engine::Node::initial_state(n, inputs))),+ }
            }

            fn process(
                &self,
                pass: &mut $crate::core::engine::Pass<$edges>,
                state: Option<&mut (dyn std::any::Any + Send)>,
            ) {
                match self {
                    $($name::$variant(n) => {
                        let state = state.and_then(|s| s.downcast_mut::<<$ty as $crate::core::engine::Node<$edges>>::State>());
                        match state {
                            Some(state) => $crate::core::engine::Node::process(n, pass, state),
                            None => $crate::core::engine::Node::process(n, pass, &mut Default::default()),
                        }
                    }),+
                }
            }

            fn prepare_to_record(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
                controls: &[Option<<$edges as $crate::core::engine::Edges>::Setting>],
                base_directory: Option<&$crate::core::base_directory::Place>,
                triggers: &$crate::core::triggers::ControlTriggers,
            ) -> Option<Box<dyn std::any::Any + Send>> {
                match self {
                    $($name::$variant(n) => $crate::core::engine::Node::prepare_to_record(n, inputs, controls, base_directory, triggers)),+
                }
            }

            fn threads(&self) -> Vec<$crate::core::engine::NodeThread> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::<$edges>::threads(n)),+ }
            }

            fn thread_ui(
                &self,
                thread: usize,
                ui: &mut eframe::egui::Ui,
                context: &$crate::core::engine::ThreadContext,
            ) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::<$edges>::thread_ui(n, thread, ui, context)),+ }
            }

            fn thread_pool(
                &self,
                thread: usize,
                context: &$crate::core::engine::ThreadContext,
            ) -> Option<$crate::core::engine::PoolStatus> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::<$edges>::thread_pool(n, thread, context)),+ }
            }
        }
    };
}
