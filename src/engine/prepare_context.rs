//! Preparing a node for a snapshot (`Node::prepare_for_snapshot`): what the node's told, including what it was in the
//! snapshot before, and what it answers. The answer has any state it needs made ahead of time (proto-states, which
//! become its state as the snapshot takes over: see `Node::on_snapshot_change`), and anything about its hardware
//! someone should decide (`UserDecision`). It can answer at once, or once something it's started (opening a device,
//! say) is done.

use super::{Edges, NodeId, Recording};
use crate::triggers::ControlTriggers;

/// A node's answer as it prepares for a snapshot (see `Node::prepare_for_snapshot`). `S` is its state, `W` each worker's
/// own, and `N` the node.
pub struct Prepared<S, W, N> {
    /// New state, made ahead of time, to become the node's state as the snapshot takes over (see
    /// `Node::on_snapshot_change`). Without it, the state carries on as it is, which only fits if the node's data
    /// streams are the same as in the snapshot before.
    pub proto_shared_state: Option<S>,
    /// The same for each worker's own state: one per worker (`PrepareContext::workers`).
    pub proto_worker_state: Option<Vec<W>>,
    /// Anything about its hardware someone should decide.
    pub decisions: Vec<UserDecision<N>>,
}

impl<S, W, N> Prepared<S, W, N> {
    /// Nothing new: the node's state carries on as it is.
    pub fn nothing() -> Self {
        Prepared { proto_shared_state: None, proto_worker_state: None, decisions: Vec::new() }
    }

    /// New shared state, to become the node's.
    pub fn shared(proto_state: S) -> Self {
        Prepared { proto_shared_state: Some(proto_state), ..Self::nothing() }
    }

    /// New state for each worker (one each: `PrepareContext::workers`).
    pub fn worker(proto_states: Vec<W>) -> Self {
        Prepared { proto_worker_state: Some(proto_states), ..Self::nothing() }
    }

    /// And new state for each worker, too.
    pub fn and_worker(self, proto_states: Vec<W>) -> Self {
        Prepared { proto_worker_state: Some(proto_states), ..self }
    }

    /// And `decisions`.
    pub fn with_decisions(self, decisions: Vec<UserDecision<N>>) -> Self {
        Prepared { decisions, ..self }
    }

    /// The same, its states and node turned into something else (as the engine keeps them, whatever the node's type).
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

/// Something about a node's hardware that isn't as its settings say, for someone to decide about: take on `fix`, if
/// there is one, carry on without the node, or stop. The snapshot runs meanwhile, the node doing what it can (a board
/// whose headstages aren't on the ports set acquires none of them, say). See `Runner::user_decisions`.
#[derive(Clone, Debug)]
pub struct UserDecision<N> {
    /// What isn't as set, for people: "the board's headstages are D1 32, D2 32, not the A1 32, B1 32 set", say.
    pub what: String,
    /// The node with settings matching the hardware, if there are such settings. A node with several decisions gives
    /// the whole fix with each.
    pub fix: Option<N>,
}

impl<N> UserDecision<N> {
    pub fn map<M>(self, f: impl Fn(N) -> M) -> UserDecision<M> {
        UserDecision { what: self.what, fix: self.fix.map(f) }
    }
}

/// A node's answer as it prepares (see `Node::prepare_for_snapshot`): given now, or once something it's started is
/// done.
pub enum Prepare<S, W, N> {
    Now(Prepared<S, W, N>),
    /// Run on the thread preparing the snapshot once every node's been asked, so what it waits for (a device the node's
    /// own thread is opening, say) can take its time while the others' do too.
    Wait(Box<dyn FnOnce() -> Prepared<S, W, N> + Send>),
}

impl<S: Send + 'static, W: Send + 'static, N: Send + 'static> Prepare<S, W, N> {
    /// Runs `prepare` on a thread of its own, called `name`, and waits for it (see `Wait`): for a node without a thread
    /// of its own to do the slow part of preparing on.
    pub fn on_thread(name: &str, prepare: impl FnOnce() -> Prepared<S, W, N> + Send + 'static) -> Self {
        let thread = std::thread::Builder::new().name(name.into()).spawn(prepare).expect("a thread to prepare on");
        Prepare::Wait(Box::new(move || match thread.join() {
            Ok(prepared) => prepared,
            Err(panic) => std::panic::resume_unwind(panic),
        }))
    }

    /// The same, its states and node turned into something else (see `Prepared::map`).
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
    /// The answer, waiting for it if need be.
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

/// How a node was in the snapshot before.
pub struct Previous<'a, E: Edges, N> {
    /// The node, with its settings then.
    pub node: &'a N,
    /// The data stream connected to each input then, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// Whether its data streams, in and out, are the same now, so state made for them still fits.
    pub same_streams: bool,
}

/// What a node's told as it prepares for a snapshot (see `Node::prepare_for_snapshot`). `N` is the node.
pub struct PrepareContext<'a, E: Edges, N> {
    pub node: NodeId,
    /// The data stream connected to each input, if any.
    pub inputs: &'a [Option<E::DataStream>],
    /// What its controls see.
    pub controls: &'a [Option<E::Setting>],
    /// The snapshot's recording, if it's recording.
    pub recording: Option<&'a Recording>,
    /// What its controls can trigger directly (see `crate::triggers`).
    pub triggers: &'a ControlTriggers<E::Trigger>,
    /// How it was in the snapshot before, if it was in it.
    pub previous: Option<Previous<'a, E, N>>,
    /// Whether the snapshot before was recording something else, or nothing: recording starts (or stops) with this
    /// one. A writer doesn't carry its files on into another recording.
    pub recording_changed: bool,
    /// How many workers there are, each needing its own state (see `Prepared::proto_worker_state`).
    pub workers: usize,
}

impl<'a, E: Edges, N> PrepareContext<'a, E, N> {
    /// For trying a node out: with inputs `inputs`, recording `recording` if there is one, new to the run, with no
    /// controls connected, triggering `triggers` (usually none: `ControlTriggers::none`), on one worker.
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

    /// Whether it was in the snapshot before with the same data streams, so the state it has still fits.
    pub fn same_streams(&self) -> bool {
        self.previous.as_ref().is_some_and(|previous| previous.same_streams)
    }

    /// Whether it was in the snapshot before with the same data streams and settings (`node` being itself now).
    pub fn unchanged(&self, node: &N) -> bool
    where
        N: PartialEq,
    {
        self.previous.as_ref().is_some_and(|previous| previous.same_streams && previous.node == node)
    }

    /// The same context, for node type `T` (the engine's dispatching to it), with `previous` finding how it was before
    /// in `N` (the program's enum of nodes).
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

/// The states `node` makes for `inputs` as if new to the run, not recording, on one worker (for trying nodes out):
/// its shared state, and its worker's.
pub fn fresh_states<E: Edges, N: super::Node<E>>(
    node: &N,
    inputs: &[Option<E::DataStream>],
) -> (N::State, N::WorkerState) {
    let triggers = ControlTriggers::none();
    let context = PrepareContext::trying(inputs, None, &triggers);
    let prepared = node.prepare_for_snapshot(&context).wait();
    let shared = or_unit(prepared.proto_shared_state).expect("a node with state makes it when it's new");
    let worker = or_unit(prepared.proto_worker_state.and_then(|mut states| states.pop()))
        .expect("a node with worker state makes it when it's new");
    (shared, worker)
}

/// The shared state `node` makes for `inputs` (see `fresh_states`).
pub fn fresh_state<E: Edges, N: super::Node<E>>(node: &N, inputs: &[Option<E::DataStream>]) -> N::State {
    fresh_states(node, inputs).0
}

/// `value`, or if there's none and `T` is `()` (no state of the kind), that.
pub(super) fn or_unit<T: 'static>(value: Option<T>) -> Option<T> {
    value.or_else(|| (Box::new(()) as Box<dyn std::any::Any>).downcast::<T>().ok().map(|unit| *unit))
}
