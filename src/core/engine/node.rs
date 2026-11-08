//! The `Node` trait each node type implements: what it is, its ports and settings, how it runs a pass, and what it
//! carries from one to the next. Its window's in `context`, its threads' in `node_threads`, and the list of types a
//! program's built with in `nodes_enum`.

use std::any::Any;
use std::time::Instant;

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Edges, Kind, NodeThread, Pass, PoolStatus, Shared, ThreadContext, UiContext};
use crate::core::global_settings::GlobalSettingsSet;

/// What a node's for, to people: its colour on the graph, and where it's offered. Nothing about how it runs depends
/// on it.
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

/// A data input: its name, and the kind of data stream it takes.
pub struct Input<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::DataStream>,
}

/// A data output: its name, the kind of data stream it makes, and the stream itself, once that can be known.
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

/// What a node says in the tree shake (see `Node::evaluate_tree_shake`): whether it runs, and which inputs' data
/// it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeShake {
    pub runs: bool,
    /// For each input, whether it needs its data. If not, the plan doesn't count it as in use for what feeds it.
    pub needs: Vec<bool>,
}

impl TreeShake {
    /// Running or not, needing each of its `inputs`.
    pub fn needing_all(runs: bool, inputs: usize) -> Self {
        TreeShake { runs, needs: vec![true; inputs] }
    }
}

/// How a node uses its buffers (see `Node::evaluate_buffer_usage`). Leaving it at `none` is always correct, only
/// using more memory, or copying more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferUsage {
    /// For each output, an input it can be made from in place, editing the input's data where it is. The plan
    /// decides, before running, whether it can be: it can if every other node reading that data can run first. The
    /// output must be the same data stream as the input.
    pub in_place: Vec<Option<usize>>,
    /// For each input, whether the node takes its buffer, to do as it likes with the data (swap it for another just
    /// as big, say, to hand to its thread or window), rather than only reading it. Nodes don't copy their inputs:
    /// the plan does, when it needs to. The pass always gives the node a buffer to take (see `Pass::take_input`):
    /// the input's own if no node edits that data in place and every other node reading it can run first, otherwise
    /// a copy the plan makes as soon as the data's made.
    pub takes: Vec<bool>,
}

impl BufferUsage {
    /// Reading each of its `inputs`, and making each of its `outputs` in a buffer of its own.
    pub fn none(inputs: usize, outputs: usize) -> Self {
        BufferUsage { in_place: vec![None; outputs], takes: vec![false; inputs] }
    }
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

    /// What its passes share with the GUI (its window, its work on the GUI's thread, the dev tools): pools of buffers,
    /// usually, or `()` for nothing. See `crate::core::engine::Shared`.
    type Shared: Shared + Default;

    /// Its state on the GUI's side: what its window and its work on the GUI's thread (`gui_background`) keep (what's been
    /// sent them, say), or `()` for nothing. The app keeps it while it has the node, whether its window's shown or not;
    /// it isn't saved. See `UiContext::ui_state`.
    type UiState: Default + 'static;

    /// The global settings it reads (where a vendor's library is, say): `()` for none, a type of them, or a pair or
    /// triple. A build has a tab in Settings… for each type its nodes name. See `crate::core::global_settings`.
    type GlobalSettings: GlobalSettingsSet;

    /// The global settings it reads, as they are now, from any thread: an `Arc` of each.
    fn global_settings() -> <Self::GlobalSettings as GlobalSettingsSet>::Current {
        Self::GlobalSettings::current()
    }

    /// The size its window opens at, floating, by default: a view of its own might want more room than the standard
    /// window.
    const WINDOW_SIZE: [f32; 2] = [340.0, 460.0];

    /// Whether it's listed in the basic view, unless that's been set for it (see `NodeEntry::basic`). By convention,
    /// nodes with a view are; but it's up to each.
    const SHOW_IN_BASIC_DEFAULT: bool = false;

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

    /// Its window. By default the standard one, with nothing of its own to set: its name, what it is, any problems, and its
    /// ports (see `crate::core::gui::node_window::standard`). A node with settings to set there shows them in the
    /// standard window, with `standard(self, ui, context, |node, ui, context| ...)`; a view of its own (a viewer's, say)
    /// draws whatever it likes.
    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<E>) {
        crate::core::gui::node_window::standard(self, ui, context, |_, _, _| {});
    }

    /// Its work on the GUI's thread that isn't drawing, whether its window's shown or not: taking what its passes send
    /// (see `Shared`), say, or timing something. The app calls it every time round, before each frame, and while its
    /// window's hidden (minimised, say) whenever it's woken. It returns when it next needs calling, if it does, and the
    /// app's woken then. It can edit the node's settings, and what its controls see, as a window does. By default,
    /// nothing.
    fn gui_background(&mut self, _context: &mut UiContext<E>) -> Option<Instant> {
        None
    }

    /// What its passes share with the GUI, made for each epoch it runs in, for its inputs' data streams then
    /// (`inputs`). By default, `Default`'s.
    fn shared(&self, _inputs: &[Option<E::DataStream>]) -> Self::Shared {
        Self::Shared::default()
    }

    /// The state for the first pass. `inputs` has the data stream connected to each input, if any.
    fn initial_state(&self, _inputs: &[Option<E::DataStream>]) -> Self::State {
        Self::State::default()
    }

    /// The tree shake: whether the node runs, and which of its inputs' data it needs, given which of its inputs (by
    /// index) have something running into them, and which of its outputs have something running that needs them. The
    /// plan starts with everything that can run running and needing everything, and asks again whenever what a node
    /// would be told changes, until nothing does; so the answer must depend only on what it's told, and its settings.
    /// It settles as long as more in use never makes a node run or need less; one that keeps changing its mind runs,
    /// and needs, whatever either of its last two answers did (see `settle`, in `plan`).
    ///
    /// By default, it runs, needing every input, whatever's in use: leaving it is always correct, only maybe slower. A
    /// node that only makes data for others runs only while something needs it, as the processing nodes do. One whose
    /// running matters beyond the graph says when: a writer only while it's given something to record. A node needn't
    /// need an input (a viewer with part of its view hidden, say): then the plan doesn't count it as in use for what
    /// feeds it. Changing settings changes the plan (a new epoch), so they're for what's switched on and off, not what
    /// changes pass to pass.
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(true, inputs.len())
    }

    /// How the node uses its buffers, given how many inputs and outputs it has: which outputs it can make in place,
    /// and which inputs it takes (see `BufferUsage`). Asked once per plan. By default, it reads its inputs and makes
    /// its outputs in buffers of their own.
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage {
        BufferUsage::none(inputs, outputs)
    }

    /// The triggers of one of the node's settings (by index): what can be sent through it, as well as its value being
    /// set: "auto detect", say. A control connected to it can send one (`UiContext::trigger`, or a `TriggerHandle`), as
    /// can the node's own window. How it's delivered is up to the trigger (see `crate::core::triggers`): a pass
    /// trigger's taken by a pass (`Pass::triggered`), which may update the node's settings when it's done
    /// (`Pass::trigger_settings_update`); a direct trigger goes to `on_trigger`.
    fn triggers(&self, _setting: usize) -> &'static [super::TriggerKind<E>] {
        &[]
    }

    /// One of its direct `triggers`, sent to it, with what its own controls can trigger in turn (directly, from any
    /// thread, whenever: see `ControlTriggers::handle`).
    fn on_trigger(&mut self, _trigger: E::Trigger, _triggers: &crate::core::triggers::ControlTriggers<E::Trigger>) {}

    /// Does the node's work for one pass: reads its inputs and fills its outputs, which are already sized for
    /// the data streams they carry. When an output is being made in place (see `evaluate_buffer_usage`), its input is `None`
    /// in the pass, and the output's buffer already holds the input's data to transform.
    fn process(&self, _pass: &mut Pass<E>, _state: &mut Self::State) {}

    /// Gets ready to record (a writer opening its files, say), given the data streams on its inputs, what its
    /// controls see, the base directory recordings go in, if there is one (see `crate::core::base_directory`), when the
    /// recording starts (to name or date it by: the same for every node, and the settings saved with it), and what its
    /// controls can trigger directly (see `crate::core::triggers`).
    /// Called on a thread of its own when recording's asked for; passes only start recording once every node's
    /// ready, so this can take its time. What it returns is the node's (with `Pass::take_prepared`) on the first
    /// pass that records.
    fn prepare_to_record(
        &self,
        _inputs: &[Option<E::DataStream>],
        _controls: &[Option<E::Setting>],
        _base_directory: Option<&crate::core::base_directory::Place>,
        _start: &chrono::DateTime<chrono::Local>,
        _triggers: &crate::core::triggers::ControlTriggers<E::Trigger>,
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
