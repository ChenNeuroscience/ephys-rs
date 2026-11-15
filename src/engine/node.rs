//! The `Node` trait each node type implements: what the node is, its ports and settings, how it gets ready for a
//! snapshot, and how it runs a pass. The node's window is in `ui_context`, its threads in `node_threads`, and the list
//! of types a program is built with in `nodes_enum`.

use std::time::Instant;

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Edges, Kind, NodeThread, PassContext, PoolStatus, Shared, ThreadContext, UiContext};
use crate::global_settings::GlobalSettingsSet;

/// Each node type has a category, which sets the node's colour on the graph and where the add-node menu offers it.
/// Nothing about how the node runs depends on its category.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Source,
    Processing,
    Visualisation,
    Recording,
    /// For nodes that aren't about data as it's acquired, or only use the data to control the app.
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

/// The static facts about a node type: its name, category and description.
pub struct NodeInfo {
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
}

/// A node's window can respond to keys (see `Node::SHORTCUTS`). For the keyboard shortcuts' help, this says which key,
/// written as people see it ("G", "↑ ↓"), and what pressing it does.
pub struct Shortcut {
    pub keys: &'static str,
    pub does: &'static str,
}

/// A node takes in data through its inputs. This describes one: its name, and the kind of data stream it takes.
pub struct Input<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::DataStream>,
}

/// A node sends out data through its outputs. This describes one: its name, the kind of data stream it carries, and
/// the stream itself, once that's known.
pub struct Output<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::DataStream>,
    /// `None` until the stream can be known (while the node's input isn't connected, say).
    pub stream: Option<E::DataStream>,
}

/// Other nodes can edit some of a node's settings, through settings edges. This describes one such setting, with its
/// current value.
pub struct SettingPort<E: Edges> {
    pub name: &'static str,
    pub value: E::Setting,
    /// Usually on the right of the node on the graph.
    pub on_right: bool,
}

/// A node can see other nodes' settings, and edit them, through its controls, each connected to a setting by a settings
/// edge. This describes one control.
pub struct Control<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::Setting>,
    /// The engine ignores changes the node makes to the setting: a writer recording a setting, say, only reads it.
    pub read_only: bool,
    /// Usually on the left of the node on the graph.
    pub on_right: bool,
}

/// A node can carry state from one pass to the next (see `Node::State`). Passes hand the state on in order, so they wait
/// for each other at a node that has some (a sync point), and the dev tools show there why the node needs it.
pub trait PassState: Send + 'static {
    /// Precisely what the state carries over, and what for. Empty for `()` (no state).
    const WHY: &'static str;
}

impl PassState for () {
    const WHY: &'static str = "";
}

/// In the tree shake, each node says whether it runs, and which of its inputs' data it needs (see
/// `Node::evaluate_tree_shake`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeShake {
    pub runs: bool,
    /// For each input, whether the node needs its data. If it doesn't, the plan doesn't count that input as using what
    /// feeds it.
    pub needs: Vec<bool>,
}

impl TreeShake {
    /// The node runs or not, needing all `inputs`.
    pub fn needing_all(runs: bool, inputs: usize) -> Self {
        TreeShake { runs, needs: vec![true; inputs] }
    }
}

/// A node can say how it uses its buffers (see `Node::evaluate_buffer_usage`), so the plan can save memory, or
/// copying. `none` is always correct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferUsage {
    /// For each output, an input the node can make that output from in place, editing the input's data where it is.
    /// The output must be the same data stream as the input. The plan decides, before running, whether the node gets
    /// to: only if every other node reading that data can run first.
    pub in_place: Vec<Option<usize>>,
    /// For each input, whether the node takes the input's buffer to do as it likes with (swapping it for another just
    /// as big, say, to hand to the node's thread or window), rather than only reading it. Nodes never copy their
    /// inputs; the plan does when it has to. The pass always has a buffer for the node to take (see
    /// `PassContext::take_input`): the input's own, if nothing edits that data in place and every other reader can
    /// run first, and otherwise a copy the plan makes as soon as the data's made.
    pub takes: Vec<bool>,
}

impl BufferUsage {
    /// The node only reads its `inputs`, and makes each of its `outputs` in a buffer of its own.
    pub fn none(inputs: usize, outputs: usize) -> Self {
        BufferUsage { in_place: vec![None; outputs], takes: vec![false; inputs] }
    }
}

/// A node type. Implementors are plain settings structs, saved with the graph.
///
/// A node's ports can depend on its settings, and its output data streams on what's connected to its inputs: the graph
/// works these out in order, from the sources down (see `Graph::resolve`).
pub trait Node<E: Edges>: Default + Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    const INFO: NodeInfo;

    /// The keys the node's window answers to, for the keyboard shortcuts' help.
    const SHORTCUTS: &'static [Shortcut] = &[];

    /// What the node carries from one pass to the next (a filter's history, say), or `()` for nothing. Passes run on
    /// several threads at once, so a node with state runs for one pass at a time, in order; a node without can run for
    /// several passes at once.
    type State: PassState;

    /// What each worker has of its own for the node (scratch space, kept so it isn't remade every pass), or `()` for
    /// nothing. Nothing in it is carried from pass to pass, so passes needn't wait for each other for it. As a snapshot
    /// takes over, the new worker state the node made ahead of time simply replaces the old (see
    /// `prepare_for_snapshot`).
    type WorkerState: Send + 'static;

    /// What the node's passes share with the GUI (its window, its work on the GUI's thread, the dev tools): pools of
    /// buffers, usually, or `()` for nothing. See `crate::engine::Shared`.
    type Shared: Shared + Default;

    /// What the GUI keeps for the node's window and its work on the GUI's thread (`gui_background`): what's been sent
    /// them, say, or `()` for nothing. The app keeps it as long as it has the node, whether or not the window's shown,
    /// and doesn't save it. See `UiContext::ui_state`.
    type UiState: Default + 'static;

    /// The global settings the node reads (where a vendor's library is, say): `()` for none, a type of them, or a pair
    /// or triple. Each type a build's nodes name gets a tab in Settings… . See `crate::global_settings`.
    type GlobalSettings: GlobalSettingsSet;

    /// The global settings the node reads, as they are now, from any thread: an `Arc` of each.
    fn global_settings() -> <Self::GlobalSettings as GlobalSettingsSet>::Current {
        Self::GlobalSettings::current()
    }

    /// The size the node's window opens at, floating. A view of the node's own might want more room than the
    /// standard window has.
    const WINDOW_SIZE: [f32; 2] = [340.0, 460.0];

    /// Whether the basic view lists the node, unless that's been set for it (see `NodeEntry::basic`). By convention,
    /// nodes with a view are listed, but each node type decides.
    const SHOW_IN_BASIC_DEFAULT: bool = false;

    fn inputs(&self) -> Vec<Input<E>> {
        Vec::new()
    }

    /// `inputs` has an entry for each of `self.inputs()`: the data stream connected to it, if any.
    fn outputs(&self, inputs: &[Option<E::DataStream>]) -> Vec<Output<E>>;

    /// Problems with how the node's set up, shown on it.
    fn warnings(&self, _inputs: &[Option<E::DataStream>]) -> Vec<String> {
        Vec::new()
    }

    /// The settings other nodes can edit, with their current values.
    fn settings(&self) -> Vec<SettingPort<E>> {
        Vec::new()
    }

    /// Sets one of `self.settings()` to a new value of the same kind.
    fn set_setting(&mut self, _index: usize, _value: E::Setting) {}

    fn controls(&self) -> Vec<Control<E>> {
        Vec::new()
    }

    /// Draws the node's window. The default is the standard window with nothing of the node's own to set: its name,
    /// what it is, any problems, and its ports (see `crate::gui::node_window::standard`). A node with settings shows
    /// them in the standard window, with `standard(self, ui, context, |node, ui, context| ...)`; a node with a view of
    /// its own (a viewer, say) draws whatever it likes.
    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<E>) {
        crate::gui::node_window::standard(self, ui, context, |_, _, _| {});
    }

    /// Does the node's work on the GUI's thread that isn't drawing, whether or not the node's window is shown: taking
    /// what the node's passes send (see `Shared`), say, or timing something. The app calls this every frame, before
    /// drawing, and while the app's window is hidden (minimised, say) whenever the app's woken. It returns when it next
    /// needs calling, if it does, and the app wakes then. Like the node's window, it can edit the node's settings and
    /// what the node's controls see, and trigger things (one thing once another's done, say). By default, nothing.
    fn gui_background(&mut self, _context: &mut UiContext<E>) -> Option<Instant> {
        None
    }

    /// Makes what the node's passes share with the GUI, for the node's input data streams (`inputs`). The engine makes
    /// it again whenever those change, and otherwise hands it on from snapshot to snapshot. By default, `Default`'s.
    fn shared(&self, _inputs: &[Option<E::DataStream>]) -> Self::Shared {
        Self::Shared::default()
    }

    /// Gets the node ready for a snapshot (see `Prepared`). This is where the node makes its state: ahead of time, as
    /// proto-states (shared, and one per worker: `context.workers`), which become the node's state and each worker's as
    /// the snapshot takes over (see `on_snapshot_change`; a worker's simply replaces the worker's old one). The node
    /// also says what about its hardware someone should decide (`UserDecision`). It goes by its data streams
    /// (`context.inputs`), its controls, the snapshot's recording, if the snapshot is recording (a writer opens its
    /// files, say), and how the node was in the snapshot before (`context.previous`). If the node makes no proto-state,
    /// its state carries on as it is, which only fits if its data streams are the same (`context.same_streams()`): so
    /// a node new to the run, or whose data streams have changed, must make its state. By default, the node makes
    /// nothing.
    ///
    /// The engine asks the nodes one after another, on the thread preparing the snapshot, before the snapshot runs:
    /// passes only run against the snapshot once every node has prepared, the snapshot before carrying on meanwhile.
    /// Anything slow (opening a device) is best started (on a thread of the node's own, or with `Prepare::on_thread`)
    /// and waited for (`Prepare::Wait`) once every node's been asked, so the slow parts overlap. Once a node has
    /// prepared, that's it: hardware it couldn't open isn't tried again, and hardware that fails later just stops
    /// giving the node data, which the node says.
    fn prepare_for_snapshot(
        &self,
        _context: &super::PrepareContext<E, Self>,
    ) -> super::Prepare<Self::State, Self::WorkerState, Self> {
        super::Prepared::nothing().into()
    }

    /// Makes the node's state from the shared proto-state the node prepared (see `prepare_for_snapshot`), as the
    /// snapshot takes over from one where the node had state (`state`). By default the engine swaps the proto-state in,
    /// but a node can take what it likes across from the old state instead (a device it has open, say). This runs on
    /// the worker running the snapshot's first pass, once the pass before has finished with the old state, so it must
    /// be quick. Whatever's left in `proto_state` (the old state, by default) is let go of in the background.
    fn on_snapshot_change(&self, state: &mut Self::State, proto_state: &mut Self::State) {
        std::mem::swap(state, proto_state);
    }

    /// The tree shake: whether the node runs, and which of its inputs' data it needs. The plan tells the node which of
    /// its inputs (by index) have something running into them, which of its outputs have something running that needs
    /// them, and whether the snapshot's recording (a recording's plan is made when Record's pressed, so its nodes
    /// prepare for it). The plan starts with everything that can run running and needing everything, and asks again
    /// whenever what a node would be told changes, until nothing does; so a node's answer must depend only on what
    /// it's told, and its settings. That settles as long as more in use never makes a node run or need less. A node
    /// that keeps changing its answer runs and needs whatever either of its last two answers did (see `settle`, in
    /// `plan`).
    ///
    /// By default the node runs, needing every input, whatever's in use: that's always correct, only maybe slower.
    /// Nodes that only make data for others run only while something needs it, as the processing nodes do. A node
    /// whose running matters beyond the graph says when it runs: a writer only while recording, and given something to
    /// record. A node needn't need an input (a viewer with part of its view hidden, say), and then the plan doesn't
    /// count that input as using what feeds it. A change of settings means a new plan (new buffers for it), so tree
    /// shaking is for what's switched on and off, not what changes from pass to pass.
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool], _recording: bool) -> TreeShake {
        TreeShake::needing_all(true, inputs.len())
    }

    /// Which of the node's outputs it can make in place, and which inputs it takes (see `BufferUsage`), given how many
    /// of each it has. The plan asks once per plan. By default the node only reads its inputs, and makes its outputs
    /// in buffers of their own.
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage {
        BufferUsage::none(inputs, outputs)
    }

    /// The triggers one of the node's settings (by index) takes: what can be sent through the setting besides a new
    /// value ("auto detect", say). A control connected to the setting can send one (`UiContext::trigger`, or a
    /// `TriggerHandle`), as can the node's own window. Each trigger says how it's delivered (see `crate::triggers`): a
    /// pass takes a pass trigger (`PassContext::triggered`), and may update the node's settings when it's done
    /// (`PassContext::trigger_settings_update`); a direct trigger goes to `on_trigger`.
    fn triggers(&self, _setting: usize) -> &'static [super::TriggerKind<E>] {
        &[]
    }

    /// The triggers one of the node's controls (by index) sends. The control can connect to a setting taking any of
    /// them, whatever the setting's kind, as well as to a setting of the control's own kind, since what the control
    /// sends is all that passes between them (a writer's "after recording" control sends files to whatever takes them,
    /// say).
    fn sends(&self, _control: usize) -> &'static [super::TriggerKind<E>] {
        &[]
    }

    /// Handles one of the node's direct `triggers`, on the GUI's thread, with the same context as the node's window
    /// (so the node's `UiState` can keep work the node starts that isn't a setting), and what the node's own controls
    /// can trigger in turn (directly, from any thread, at any time: see `ControlTriggers::handle`).
    fn on_trigger(
        &mut self,
        _trigger: E::Trigger,
        _context: &mut UiContext<E>,
        _triggers: &crate::triggers::ControlTriggers<E::Trigger>,
    ) {
    }

    /// Does the node's work for one pass: reads its inputs and fills its outputs, which are already sized for their
    /// data streams. An output the node makes in place (see `evaluate_buffer_usage`) already holds its input's data to
    /// transform, and that input is `None` in the pass.
    fn process(&self, _pass: &mut PassContext<E>, _state: &mut Self::State, _worker_state: &mut Self::WorkerState) {}

    /// The threads of its own the node runs while running (started with its state, say), for the dev tools.
    fn threads(&self) -> Vec<NodeThread> {
        Vec::new()
    }

    /// Draws, briefly, what one of the node's `threads()` (by index) is up to, in the dev tools.
    fn thread_ui(&self, _thread: usize, _ui: &mut egui::Ui, _context: &ThreadContext) {}

    /// Where the buffers are that one of the node's `threads()` (by index) swaps with the passes, if it does, for the
    /// dev tools to show between them.
    fn thread_pool(&self, _thread: usize, _context: &ThreadContext) -> Option<PoolStatus> {
        None
    }
}
