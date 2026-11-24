//! Each node type implements the `Node` trait below, which describes the node, its ports and settings, how the node
//! gets ready for a snapshot, and what the node does in a pass. Related modules:
//!
//! - `ports`: how a node declares its ports (with the details in `data_ports` and `val_cmd_ports`);
//! - `ui_context`: the context a node's window receives;
//! - `node_threads`: how a node describes threads of its own;
//! - `nodes_enum`: the list of node types a program is built with.

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    CmdInOf, CommandContext, DataStreamInOf, DataStreamInPorts, DataStreamOutOf, DataStreamOutPorts, DevTools, Edges,
    NodeId, PassContext, PortSet, Shared, UiContext, ValOutCmdInOf,
};
use crate::global_settings::GlobalSettingsSet;

/// The graph colours each node by its type's category, and the add-node menu groups node types by category. Nothing
/// about how a node runs depends on its category.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Source,
    Processing,
    Visualisation,
    Recording,
    /// Nodes that aren't about data as it's acquired, or that only use the data to control the app, belong here.
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

/// The add-node menu and each node's window describe the node's type. This holds what they show, which never changes:
/// the type's name, category and description.
pub struct NodeInfo {
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
}

/// A node's window can respond to keys (see `Node::SHORTCUTS`), and the keyboard shortcuts' help lists them. This says
/// which key, written as people see it ("G", "↑ ↓"), and what pressing that key does.
pub struct Shortcut {
    pub keys: &'static str,
    pub does: &'static str,
}

/// Where a command sent to a node came from (see `Node::on_command`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandSource {
    /// Through an edge into the node's `ValOutCmdIn` port number `n`. The node can compare `n` with the constants
    /// that `ports!` generates (such as `ValOutCmdIn::files`).
    Port(usize),
    /// From the node itself: its passes (auto detect's result, say) or a thread of its own. The node's own window has
    /// `&mut self`, so it calls the node's methods directly rather than commanding itself.
    Itself,
}

/// A node can carry state from one pass to the next (see `Node::State`). Passes hand the state on in order, so passes
/// wait for each other at a node that has state (a sync point). The dev tools show, at that sync point, why the node
/// needs its state, which each type of state says by implementing this trait.
pub trait PassState: Send + 'static {
    /// The dev tools show this at the node's sync point: precisely what the state carries over, and what for. For `()`
    /// (no state), there's nothing to say, so the text is empty.
    const WHY: &'static str;
}

impl PassState for () {
    const WHY: &'static str = "";
}

/// Before a graph runs, the plan works out which nodes run, and which of their inputs' data each node needs: the tree
/// shake (see `Node::evaluate_tree_shake`). This is one node's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeShake {
    pub runs: bool,
    /// A node needn't need every input's data. For each input, this says whether the node needs it, and the plan
    /// doesn't count an input the node doesn't need as using what feeds that input.
    pub needs: Vec<bool>,
}

impl TreeShake {
    /// Most nodes need all their inputs' data whenever they run. This answers that the node runs or not (`runs`),
    /// needing all of its `inputs`.
    pub fn needing_all(runs: bool, inputs: usize) -> Self {
        TreeShake { runs, needs: vec![true; inputs] }
    }
}

/// How a node uses its buffers, which lets the plan save memory or copying. The engine builds this from the node's
/// data port declarations: inputs declared `take`, and outputs declared `in_place_of` an input (see `ports!`). `none`
/// is always correct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferUsage {
    /// A node can sometimes make an output by editing one of its input's data where the data is, rather than filling a
    /// buffer of the output's own. For each output, this names the input the node can do that with, if any; the output
    /// must be the same data stream as the input. The plan decides, before running, whether the node gets to: only if
    /// every other node reading that data can run first.
    pub in_place: Vec<Option<usize>>,
    /// A node can take an input's buffer to do as the node likes with (swapping the buffer for another just as big,
    /// say, to hand to the node's thread or window), rather than only reading it. For each input, this says whether the
    /// node does. Nodes never copy their inputs; the plan does when it has to. The pass always has a buffer for the
    /// node to take, if the input's connected: the input's own, if nothing edits that data in place and every other
    /// reader can run first, and otherwise a copy the plan makes as soon as the data's made.
    pub takes: Vec<bool>,
}

impl BufferUsage {
    /// A node that says nothing about its buffers only reads its `inputs`, and makes each of its `outputs` in a buffer
    /// of the output's own. This is that answer.
    pub fn none(inputs: usize, outputs: usize) -> Self {
        BufferUsage { in_place: vec![None; outputs], takes: vec![false; inputs] }
    }
}

/// Each kind of node on the graph (a filter, a viewer, a device) is a type implementing this trait. The type itself is
/// a plain struct of the node's settings, which the app saves with the graph.
///
/// A node's ports are fixed by its type (see `Node::Ports`), but the data streams on its outputs can depend on its
/// settings and on what's connected to its inputs. The graph works the streams out in order, from the sources
/// downstream (see `Graph::resolve`).
pub trait Node<E: Edges>: Default + Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The add-node menu and the node's window describe the node's type with this (see `NodeInfo`).
    const INFO: NodeInfo;

    /// A node's window can respond to keys. The keyboard shortcuts' help lists the keys a node type gives here.
    const SHORTCUTS: &'static [Shortcut] = &[];

    /// The engine runs passes on several threads at once. A node that carries something from one pass to the next (a
    /// filter's history, say) keeps it in its state, and then runs for one pass at a time, in order. A node whose state
    /// is `()` carries nothing, and can run for several passes at once.
    type State: PassState;

    /// Each worker (a thread running passes) can keep something of its own for the node, such as scratch space that
    /// would otherwise be remade every pass; `()` is for nothing. Nothing in a worker's state is carried from pass to
    /// pass, so passes needn't wait for each other for it. As a snapshot takes over, the worker state the node made
    /// ahead of time simply replaces the old (see `prepare_for_snapshot`). A node that's new to the run, or whose data
    /// streams have changed, and that made none, gets `Default`'s in each worker.
    type WorkerState: Default + Send + 'static;

    /// The node's passes run apart from the GUI (the node's window, the node's work on the GUI's thread, the dev
    /// tools), and share with the GUI only what's in here: pools of buffers, usually, or `()` for nothing. See
    /// `crate::engine::Shared`.
    type Shared: Shared + Default;

    /// The GUI can keep things for the node's window and the node's work on the GUI's thread (see `on_gui_tick`), such
    /// as what the node's passes have sent; `()` is for nothing. The app keeps this as long as the app has the node,
    /// whether or not the window's shown, and doesn't save it. See `UiContext::ui_state`.
    type UiState: Default + 'static;

    /// Some settings belong to the app rather than to any one node (where a vendor's library is, say). A node type
    /// names here the global settings its nodes read: `()` for none, a type of them, or a pair or triple. Each type a
    /// build's nodes name gets a tab in Settings… . See `crate::global_settings`.
    type GlobalSettings: GlobalSettingsSet;

    /// A node's window can float, and opens at this size when it does. A node with a view of its own might want more
    /// room than the standard window needs.
    const WINDOW_SIZE: [f32; 2] = [340.0, 460.0];

    /// The basic view lists only some of the graph's nodes. Unless someone has set whether a node is listed (see
    /// `NodeEntry::basic`), this decides. By convention, nodes with a view are listed, but each node type decides.
    const SHOW_IN_BASIC_DEFAULT: bool = false;

    /// The node's ports. Each node declares its ports with `ports!` (see `crate::engine::ports`), which generates a
    /// `Ports` type for this; a node with no ports uses `NoPorts`. The method signatures below use the other types
    /// that `ports!` generates: `DataStreamIn`, `DataStreamOut`, `ValOutCmdIn` and `CmdIn`. A node with no ports of a
    /// role uses `NoPorts` in place of that role's type, or `NoCommands` in place of `CmdIn`.
    type Ports: PortSet<E>;

    /// Returns the data stream on each of the node's `DataStreamOut` ports, given the streams on its `DataStreamIn`
    /// ports (`inputs`, where an unconnected input is `None`). The graph calls this to work out the streams. The node
    /// only runs once every output's stream is known, so the node should return `None` for an output it can't produce
    /// from what it has, such as when a required input isn't connected.
    ///
    /// The node's `id` is for anything in a stream that names where the stream comes from: a source names the clock its
    /// data's timed by from it, say. Nothing else should depend on it.
    fn data_stream_out(&self, id: NodeId, inputs: &DataStreamInOf<Self, E>) -> DataStreamOutOf<Self, E>;

    /// Returns the current values at the node's `ValOutCmdIn` ports, which other nodes read. These are usually copies
    /// of some of the node's settings. The app reads them each frame for the nodes' windows, and each snapshot keeps a
    /// copy for passes. A node with no `ValOutCmdIn` ports returns `NoPorts`.
    fn val_out(&self) -> ValOutCmdInOf<Self, E>;

    /// Handles a command sent to the node. The app calls this on the GUI's thread, whether or not the graph is
    /// running, with commands in the order they were sent. `command` is the node's `CmdIn`, the enum of the commands
    /// its `ValOutCmdIn` ports accept, and `from` says where the command came from (most nodes ignore it).
    ///
    /// A command that sets a value (such as `SetGains`) usually just replaces the setting that the value copies. Other
    /// commands usually start something, such as a job kept in the node's `UiState`; if the node is busy, it queues the
    /// command or reports it as missed. A node that accepts no commands implements this as `match command {}`.
    fn on_command(&mut self, from: CommandSource, command: CmdInOf<Self, E>, context: &mut CommandContext<E>);

    /// Clears the node's transient state: things the node has been asked to do that only apply for now, such as an
    /// auto detect in progress. A node keeps such state in fields that aren't saved (`#[serde(skip)]`). The app calls
    /// this on every node when it restores the graph from history (undo or redo), starts recording or stops running,
    /// and on a node it duplicates. By default, there is nothing to clear.
    fn clear_transient_state(&mut self) {}

    /// While running, the app tries to draw a frame 50 times a second; while stopped, it draws only when something
    /// changes (input, say). The app calls this for every node once a frame. If the node's window is shown, `ui` is
    /// there to draw the window in; if not, `ui` is `None`, and the app still calls this so the node can do work that
    /// carries on regardless (taking what its passes send, say). With or without a `ui`, the node can edit its
    /// settings, read the values its ports see, and send commands (one thing once another's done, say).
    ///
    /// By default, the node draws the standard window, with nothing of its own to set: its name, what it is, any
    /// problems, and its ports (see `crate::gui::node_window::standard`). A node with settings shows them in the
    /// standard window, with `standard(self, ui, context, |node, ui, context| ...)`; a node with a view of its own (a
    /// viewer, say) draws whatever it likes.
    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        if let Some(ui) = ui {
            crate::gui::node_window::standard(self, ui, context, |_, _, _| {});
        }
    }

    /// The node's passes and the GUI share what's in `Self::Shared`. The engine asks the node for it here, for the
    /// node's input data streams (`inputs`), and asks again whenever those change, otherwise handing what it has on
    /// from snapshot to snapshot. By default, the node gives `Default`'s.
    fn shared(&self, _inputs: &DataStreamInOf<Self, E>) -> Self::Shared {
        Self::Shared::default()
    }

    /// Whenever the graph changes while running (a setting's edited, say, or Record's pressed), the engine makes a new
    /// snapshot of the graph, and the passes switch to the new snapshot once every node is ready for it. The engine
    /// asks each node to get ready here (see `Prepared`).
    ///
    /// A node makes its state here, ahead of time, as proto-states: a shared one, and one per worker
    /// (`context.workers`). These become the node's state and each worker's as the snapshot takes over (see
    /// `adopt_snapshot`; a worker's proto-state simply replaces the worker's old state). The node also reports anything
    /// someone should decide (`UserDecision`), such as hardware that doesn't match the node's settings. To decide what
    /// to make, the node has:
    ///
    /// - its data streams (`context.inputs`);
    /// - its settings (`self`): an auto detect in progress needs room to sample into, for example;
    /// - the values its `ValInCmdOut` and `ValIn` ports read;
    /// - whether the snapshot is recording: a writer opens its files then, for example;
    /// - how the node was in the snapshot before (`context.previous`, including its settings then).
    ///
    /// A node that makes no proto-state keeps its state as it is, which only works if the node's data streams are
    /// unchanged (`context.same_streams()`). So a node that is new to the run, or whose data streams have changed, must
    /// make its shared state (each worker's state defaults to `Default` if the node makes none). By default, the node
    /// makes nothing.
    ///
    /// The engine asks the nodes one after another, on the thread preparing the snapshot, while passes carry on with
    /// the snapshot before. A node is best to start anything slow (opening a device) on a thread of the node's own, or
    /// with `Prepare::on_thread`, and have the engine wait for it (`Prepare::Wait`) once every node's been asked, so
    /// the slow parts overlap. Once a node has prepared, that's it: the node doesn't try again to open hardware it
    /// couldn't, and hardware that fails later just stops giving the node data, which the node says.
    fn prepare_for_snapshot(
        &self,
        _context: &super::PrepareContext<E, Self>,
    ) -> super::Prepare<Self::State, Self::WorkerState, Self> {
        super::Prepared::nothing().into()
    }

    /// When the passes switch to a new snapshot, a node that had state in the snapshot before and prepared a new shared
    /// proto-state (`new_proto_state`, see `prepare_for_snapshot`) gets to make its new state from the two. The node's
    /// `current_state` holds the old state when the engine calls this, and whatever the node leaves there becomes the
    /// new state. By default, the node swaps the proto-state in, but a node can take what it likes across from the old
    /// state instead (a device it has open, say). The engine calls this on the worker
    /// running the snapshot's first pass, once the pass before has finished with the old state, so the node must be
    /// quick. The engine lets go of whatever's left in `new_proto_state` (the old state, by default) in the background.
    fn adopt_snapshot(&self, current_state: &mut Self::State, new_proto_state: &mut Self::State) {
        std::mem::swap(current_state, new_proto_state);
    }

    /// Before a graph runs, the plan works out which nodes run, and which of their inputs' data each node needs (the
    /// tree shake), so nothing works on data that nothing uses. The plan asks each node here, telling the node which of
    /// its inputs (by index) have something running into them, which of its outputs have something running that needs
    /// them, and whether the snapshot's recording (a recording's plan is made when Record's pressed, so its nodes
    /// prepare for it); and the node has its settings (`self`). The plan starts with everything that can run running
    /// and needing everything, and asks again whenever what a node would be told changes, until nothing does; so a
    /// node's answer must depend only on what the node's told, and its settings. The answers settle as long as more in
    /// use never makes a node run or need less. A node that keeps changing its answer runs and needs whatever either of
    /// its last two answers did (see `settle`, in `plan`).
    ///
    /// By default the node runs, needing every input, whatever's in use: that's always correct, only maybe slower.
    /// Nodes that only make data for others run only while something needs it, as the processing nodes do. A node whose
    /// running matters beyond the graph says when it runs: a writer only while recording, and given something to
    /// record. A node asked to do something with the data (an auto detect, kept as its transient state: see
    /// `Node::clear_transient_state`) can only do it by running, so it should run while it's asked, even if
    /// nothing needs what it makes yet: a gain feeding only a writer, say, has its gains detected before recording. A
    /// node needn't need an input (a viewer with part of its view hidden, say), and then the plan doesn't count that
    /// input as using what feeds it. A change of settings means a new plan (new buffers for it), so tree shaking is for
    /// what's switched on and off, not what changes from pass to pass.
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool], _recording: bool) -> TreeShake {
        TreeShake::needing_all(true, inputs.len())
    }

    /// The engine runs the graph in passes, each taking in a short stretch of data and sending it through every node
    /// that runs. In each pass, the node does its work here: the node reads its inputs and fills its outputs, which are
    /// already sized for their data streams. The buffers arrive as the node's own `DataStreamIn` and
    /// `DataStreamOut` structs, in the `Buffers` form. An output declared `in_place_of` an input may be written in
    /// place: if the plan allows that, the output's buffer already holds the input's data to transform, and the input
    /// itself is `None` for the pass.
    fn process(
        &self,
        _pass: &mut PassContext<E>,
        _inputs: <DataStreamInOf<Self, E> as DataStreamInPorts<E>>::View<'_>,
        _outputs: <DataStreamOutOf<Self, E> as DataStreamOutPorts<E>>::View<'_>,
        _state: &mut Self::State,
        _worker_state: &mut Self::WorkerState,
    ) {
    }

    /// Some nodes run threads of their own while running (started with their state, say). Each time the dev tools draw,
    /// they ask the node here, and the node describes each of its threads to `dev` (see `DevTools::thread`): what the
    /// thread is, where its pool's buffers are, and what to draw to show, briefly, what the thread's up to.
    fn on_dev_tools_tick<'a>(&'a self, _dev: &mut DevTools<'a>) {}

    /// The graph and the node's window show any problems with how the node's set up, which the node lists here, given
    /// what's connected to its inputs.
    fn warnings(&self, _inputs: &DataStreamInOf<Self, E>) -> Vec<String> {
        Vec::new()
    }
}
