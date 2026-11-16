//! Before passes switch to a new snapshot, the engine asks each node to get ready for it
//! (`Node::prepare_for_snapshot`). This module has what the node is told (`PrepareContext`, including how the node was
//! in the snapshot before) and what the node answers (`Prepared`): any state the node needs, made ahead of time as
//! proto-states that become its state as the snapshot takes over (see `Node::on_snapshot_change`), and anything someone
//! should decide (`UserDecision`: hardware not as the node's settings say, say). A node can answer at once, or once
//! something it's started (opening a device, say) is done (`Prepare`).

use super::{Edges, NodeId, Recording};
use crate::triggers::ControlTriggers;

/// When a node prepares for a snapshot (see `Node::prepare_for_snapshot`), it answers with this: what it made ahead of
/// time, and what someone should decide. `S` is the node's state, `W` each worker's own, and `N` the node.
pub struct Prepared<S, W, N> {
    /// The node can make new state ahead of time, which becomes its state as the snapshot takes over (see
    /// `Node::on_snapshot_change`). Without any, the node's state carries on as it is, which only fits if the node's
    /// data streams are the same as in the snapshot before.
    pub proto_shared_state: Option<S>,
    /// Each worker can have state of its own for the node; the node can make that ahead of time too, one per worker
    /// (`PrepareContext::workers`).
    pub proto_worker_state: Option<Vec<W>>,
    /// The node may find something someone should decide (its hardware not as its settings say, say), which it lists
    /// here.
    pub decisions: Vec<UserDecision<N>>,
}

impl<S, W, N> Prepared<S, W, N> {
    /// A node with nothing new to make answers this, and its state carries on as it is.
    pub fn nothing() -> Self {
        Prepared { proto_shared_state: None, proto_worker_state: None, decisions: Vec::new() }
    }

    /// A node answers this with new shared state, which becomes the node's.
    pub fn shared(proto_state: S) -> Self {
        Prepared { proto_shared_state: Some(proto_state), ..Self::nothing() }
    }

    /// A node answers this with new state for each worker (one each: `PrepareContext::workers`).
    pub fn worker(proto_states: Vec<W>) -> Self {
        Prepared { proto_worker_state: Some(proto_states), ..Self::nothing() }
    }

    /// The answer, with new state for each worker as well.
    pub fn and_worker(self, proto_states: Vec<W>) -> Self {
        Prepared { proto_worker_state: Some(proto_states), ..self }
    }

    /// The answer, with `decisions` for someone to make as well.
    pub fn with_decisions(self, decisions: Vec<UserDecision<N>>) -> Self {
        Prepared { decisions, ..self }
    }

    /// The engine keeps every node's answer alike, whatever the node's type. This turns the states and the node in the
    /// answer into what the engine keeps.
    pub fn map<T, X, M>(
        self,
        shared: impl FnOnce(S) -> T,
        worker: impl Fn(W) -> X,
        node: impl Fn(N) -> M,
    ) -> Prepared<T, X, M> {
        Prepared {
            proto_shared_state: self.proto_shared_state.map(shared),
            proto_worker_state: self.proto_worker_state.map(|states| states.into_iter().map(worker).collect()),
            decisions: self.decisions.into_iter().map(|decision| decision.map(&node)).collect(),
        }
    }
}

/// A node may find, as it prepares, something someone should decide (its hardware not as its settings say, say). This
/// is one such thing: someone takes the node's `suggestion`, if it has one, carries on without the node, or stops. The
/// snapshot runs meanwhile, and the node does what it can (a board whose headstages aren't on the ports set acquires
/// none of them, say). See `Runner::user_decisions`.
#[derive(Clone, Debug)]
pub struct UserDecision<N> {
    /// The app shows people what's to decide, as the node puts it: "the board's headstages are D1 32, D2 32, not the A1
    /// 32, B1 32 set", say.
    pub what: String,
    /// The node can suggest a way to settle it (see `Suggestion`). A node with several decisions gives the whole
    /// suggestion with each.
    pub suggestion: Option<Suggestion<N>>,
}

/// A node can suggest how to settle what's to decide: the node itself, changed (its settings matching its hardware's,
/// say), which the app offers by the node's `label` for it ("Use the hardware's settings", say).
#[derive(Clone, Debug)]
pub struct Suggestion<N> {
    pub label: String,
    pub node: N,
}

impl<N> UserDecision<N> {
    /// The engine keeps every node's decisions alike, whatever the node's type. This turns the suggested node into what
    /// the engine keeps.
    pub fn map<M>(self, f: impl Fn(N) -> M) -> UserDecision<M> {
        let suggestion = self.suggestion.map(|Suggestion { label, node }| Suggestion { label, node: f(node) });
        UserDecision { what: self.what, suggestion }
    }
}

/// A node can answer as it prepares (see `Node::prepare_for_snapshot`) at once, or once something it's started (opening
/// a device, say) is done.
pub enum Prepare<S, W, N> {
    Now(Prepared<S, W, N>),
    /// The engine runs this on the thread preparing the snapshot once every node has been asked, so what the node waits
    /// for (a device the node's own thread is opening, say) can take its time while other nodes' slow parts do too.
    Wait(Box<dyn FnOnce() -> Prepared<S, W, N> + Send>),
}

impl<S: Send + 'static, W: Send + 'static, N: Send + 'static> Prepare<S, W, N> {
    /// A node without a thread of its own can do the slow part of preparing on one made for it: this runs `prepare` on
    /// a thread called `name`, and the engine waits for it (see `Wait`).
    pub fn on_thread(name: &str, prepare: impl FnOnce() -> Prepared<S, W, N> + Send + 'static) -> Self {
        let thread = std::thread::Builder::new().name(name.into()).spawn(prepare).expect("a thread to prepare on");
        Prepare::Wait(Box::new(move || match thread.join() {
            Ok(prepared) => prepared,
            Err(panic) => std::panic::resume_unwind(panic),
        }))
    }

    /// This turns the states and node in the answer into what the engine keeps, as `Prepared::map` does, once the
    /// answer's given.
    pub fn map<T: 'static, X: 'static, M: 'static>(
        self,
        shared: impl FnOnce(S) -> T + Send + 'static,
        worker: impl Fn(W) -> X + Send + 'static,
        node: impl Fn(N) -> M + Send + 'static,
    ) -> Prepare<T, X, M> {
        match self {
            Prepare::Now(prepared) => Prepare::Now(prepared.map(shared, worker, node)),
            Prepare::Wait(wait) => Prepare::Wait(Box::new(move || wait().map(shared, worker, node))),
        }
    }
}

impl<S, W, N> Prepare<S, W, N> {
    /// The engine takes the answer with this, waiting for it if need be.
    pub fn wait(self) -> Prepared<S, W, N> {
        match self {
            Prepare::Now(prepared) => prepared,
            Prepare::Wait(wait) => wait(),
        }
    }
}

impl<S, W, N> From<Prepared<S, W, N>> for Prepare<S, W, N> {
    fn from(prepared: Prepared<S, W, N>) -> Self {
        Prepare::Now(prepared)
    }
}

/// A node preparing for a snapshot is told how it was in the snapshot before, if it was in it.
pub struct Previous<'a, E: Edges, N> {
    /// The node as it was, with its settings then.
    pub node: &'a N,
    /// For each input, the data stream connected to it then, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// If the node's data streams, in and out, are the same now, state made for them still fits.
    pub same_streams: bool,
}

/// When the engine asks a node to prepare for a snapshot (see `Node::prepare_for_snapshot`), it tells the node this.
/// `N` is the node.
pub struct PrepareContext<'a, E: Edges, N> {
    pub node: NodeId,
    /// For each input, the data stream connected to it, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// For each of the node's controls, the setting it's connected to, if it is.
    pub controls: &'a [Option<E::Setting>],
    /// If the snapshot's recording, the recording (where it goes, and when it was asked for).
    pub recording: Option<&'a Recording>,
    /// The node's controls can send triggers to the nodes they're connected to (see `crate::triggers`), through this.
    pub triggers: &'a ControlTriggers<E::Trigger>,
    /// How the node was in the snapshot before, if it was in it.
    pub previous: Option<Previous<'a, E, N>>,
    /// If the snapshot before was recording something else, or nothing, recording starts (or stops) with this one. A
    /// writer doesn't carry its files on into another recording.
    pub recording_changed: bool,
    /// The run has this many workers, each with its own state for the node (see `Prepared::proto_worker_state`).
    pub workers: usize,
}

impl<'a, E: Edges, N> PrepareContext<'a, E, N> {
    /// Tests try nodes out with this: the node new to the run, with inputs `inputs`, recording `recording` if there is
    /// one, no controls connected, sending triggers through `triggers` (usually none: `ControlTriggers::none`), on one
    /// worker.
    pub fn trying(
        inputs: &'a [Option<E::DataStream>],
        recording: Option<&'a Recording>,
        triggers: &'a ControlTriggers<E::Trigger>,
    ) -> Self {
        PrepareContext {
            node: NodeId(0),
            inputs,
            controls: &[],
            recording,
            triggers,
            previous: None,
            recording_changed: recording.is_some(),
            workers: 1,
        }
    }

    /// A node that was in the snapshot before with the same data streams can keep its state, which still fits. This
    /// says whether the node was.
    pub fn same_streams(&self) -> bool {
        self.previous.as_ref().is_some_and(|previous| previous.same_streams)
    }

    /// This says whether the node was in the snapshot before with the same data streams and settings (`node` being the
    /// node now).
    pub fn unchanged(&self, node: &N) -> bool
    where
        N: PartialEq,
    {
        self.previous.as_ref().is_some_and(|previous| previous.same_streams && previous.node == node)
    }

    /// The engine hands each node type its own context, from the program's enum of nodes (`N`). This is that context
    /// for node type `T`, with `previous` finding the node as it was before.
    pub fn of<T>(&self, previous: impl Fn(&'a N) -> Option<&'a T>) -> PrepareContext<'a, E, T> {
        PrepareContext {
            node: self.node,
            inputs: self.inputs,
            controls: self.controls,
            recording: self.recording,
            triggers: self.triggers,
            previous: self.previous.as_ref().and_then(|p| {
                Some(Previous { node: previous(p.node)?, inputs: p.inputs, same_streams: p.same_streams })
            }),
            recording_changed: self.recording_changed,
            workers: self.workers,
        }
    }
}

/// Tests try nodes out with the state a node makes for `inputs` when it's new to the run, not recording, on one worker.
/// This returns the node's shared state and its worker's.
pub fn fresh_states<E: Edges, N: super::Node<E>>(
    node: &N,
    inputs: &[Option<E::DataStream>],
) -> (N::State, N::WorkerState) {
    let triggers = ControlTriggers::none();
    let context = PrepareContext::trying(inputs, None, &triggers);
    let prepared = node.prepare_for_snapshot(&context).wait();
    let shared = or_unit(prepared.proto_shared_state).expect("a node with state makes it when it's new");
    let worker = prepared.proto_worker_state.and_then(|mut states| states.pop()).unwrap_or_default();
    (shared, worker)
}

/// Tests mostly need only the shared state a node makes for `inputs` (see `fresh_states`).
pub fn fresh_state<E: Edges, N: super::Node<E>>(node: &N, inputs: &[Option<E::DataStream>]) -> N::State {
    fresh_states(node, inputs).0
}

/// A node with no state of a kind (`T` being `()`) needn't make any. This gives `value`, or `()` if there's none and
/// `T` is `()`.
pub(super) fn or_unit<T: 'static>(value: Option<T>) -> Option<T> {
    value.or_else(|| (Box::new(()) as Box<dyn std::any::Any>).downcast::<T>().ok().map(|unit| *unit))
}
