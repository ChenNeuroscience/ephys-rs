//! Running the graph: a small pool of worker threads, each running whole passes over a snapshot of it.
//!
//! Time is cut into passes of `PASS_DURATION`, numbered from 0. A dispatcher thread releases each pass when
//! its time is up, to whichever ready worker is free. A worker runs the pass through every node in order, as
//! the snapshot's plan says (worked out when the snapshot was taken), on buffers of its own made for that plan
//! before it runs any of its passes (so nothing is allocated per pass), with each node reading its inputs
//! from the outputs of the nodes before it.
//!
//! Passes run at the same time on different workers, which is fine for most nodes but not for those that carry state
//! from one pass to the next (a filter's history, say). Each node's state lives in a slot, on a cache line of its own,
//! with a turn number: pass n waits (spinning briefly, then yielding) for the turn to reach n, uses the state, then sets
//! it to n + 1 to hand over to the next pass. As long as each pass stays behind the one before, there's little
//! waiting. State that's each worker's own (scratch space: see `Node::WorkerState`) needs no turns: each worker keeps
//! its own, by node.
//!
//! If every worker is busy and passes queue up, the dispatcher drops passes (counting them) rather than
//! falling ever further behind.
//!
//! Workers report each pass they run to the GUI on a queue of its own, for the stats, never waiting on it.
//!
//! Every node with state prepares for each snapshot the run's given, one snapshot at a time, on a thread of the run's
//! (`Node::prepare_for_snapshot`, see `prepare_snapshots`): making its state ahead of time (proto-states), or making
//! nothing new, so its state carries on (a node with the same data streams in and out sees its new settings on the
//! next pass, and adjusts). The new state's made from the proto-state at the new snapshot's first pass, once the pass
//! before has finished with the old (`Node::on_snapshot_change`), so passes see it in order across snapshots too; each
//! worker takes its own, replacing what it had, at its first pass of the snapshot. What a node shares with the GUI is handed on likewise, or made afresh. The dispatcher attaches the
//! snapshot to each pass it sends, so snapshots only move forward pass by pass. It switches to a new snapshot once
//! it's ready: its nodes have prepared, and its plan has the same shape as the running one's (the same steps and
//! buffers, as after a change of settings) or every worker has made buffers for it, as they come free; until then,
//! passes carry on with the one before. So a worker is never handed a pass it isn't ready to run. A
//! recording takes effect with its snapshot's first pass (`Recording::take_effect`).
//!
//!
//! What each pass is given is in `pass_context`, and how it's planned in `plan`.

use std::any::Any;
use std::cell::RefCell;
use std::cell::UnsafeCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_swap::{ArcSwap, ArcSwapOption};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use crossbeam_utils::CachePadded;

use crate::console::{self, PassId};
use crate::triggers::DirectTriggers;

use super::pass_context::*;
use super::plan::*;
use super::{
    Buffer, DataStreamsEnum, Edges, NodeId, NodesEnum, Prepare, PrepareContext, Prepared, Previous, SharedByNode,
    Snapshot, SnapshotLite, UserDecision,
};

type DataStream<N> = <<N as NodesEnum>::Edges as Edges>::DataStream;

/// What the workers do with a plan: make their buffers for it, and run its passes.
impl<N: NodesEnum> Plan<N> {
    /// The buffers a worker runs passes on.
    fn workspace(&self) -> Workspace<N> {
        Workspace { buffers: self.buffers.iter().map(|s| Some(s.buffer(PASS_DURATION))).collect() }
    }

    /// Runs a pass through every step of `snapshot`'s graph, on `workspace`, with the worker's own state for the nodes
    /// with some (`worker_states`). A node without the state it needs (it failed to prepare) doesn't run, and its
    /// outputs are left empty.
    fn run(
        &self,
        snapshot: &Arc<Snapshot<N>>,
        pass: PassId,
        workspace: &mut Workspace<N>,
        worker_states: &mut WorkerStates,
        run: &RunSlots,
    ) {
        // As nodes can see it, to keep (see `PassContext::snapshot`).
        let baked: Arc<dyn SnapshotLite> = snapshot.clone();
        let Workspace { buffers: workspace } = workspace;
        let sequence = pass.sequence;
        for (step, times) in self.steps.iter().zip(&run.times) {
            let Some(node) = snapshot.graph.get(step.id).map(|e| &e.node) else { continue };
            let pass = PassId { node: Some(step.id), node_type: Some(node.info().name), ..pass };
            let controls = snapshot.controls.get(&step.id).map_or(&[][..], Vec::as_slice);
            // Take the outputs out of the pool, so the rest of it can be read as inputs.
            let mut outputs: Vec<Buffer<N::Edges>> =
                step.outputs.iter().map(|&b| workspace[b].take().expect("buffer in pool")).collect();
            // And the inputs it takes.
            let mut taken: Vec<Option<Buffer<N::Edges>>> = step
                .inputs
                .iter()
                .zip(&step.takes)
                .map(|(b, &takes)| if takes { b.and_then(|b| workspace[b].take()) } else { None })
                .collect();
            let worker_state = worker_states.get_mut(&step.id);
            let (work, wait, ran) = {
                let inputs: Vec<Option<&Buffer<N::Edges>>> =
                    step.inputs.iter().map(|b| b.and_then(|b| workspace[b].as_ref())).collect();
                let mut pass_data = PassContext {
                    sequence,
                    duration: PASS_DURATION,
                    node: step.id,
                    inputs: &inputs,
                    taken: &mut taken,
                    outputs: &mut outputs,
                    controls,
                    snapshot: &baked,
                    shared: run.shared.any(step.id).unwrap_or(&()),
                    direct: &run.direct,
                };
                let start = Instant::now();
                // When the node's state is handed over, after waiting for it, if it has any.
                let mut began = start;
                let worker_state = worker_state.map(|state| state.as_mut());
                let ran = console::in_pass(pass, || match step.state {
                    Some(slot) => {
                        let change = |state: &mut (dyn Any + Send), proto_state: &mut (dyn Any + Send)| {
                            node.on_snapshot_change(state, proto_state)
                        };
                        run.states[slot].with(sequence, &change, |state| {
                            began = Instant::now();
                            node.process(&mut pass_data, Some(state), worker_state)
                        })
                    }
                    None => node.process(&mut pass_data, None, worker_state),
                });
                (began.elapsed(), began - start, ran)
            };
            if !ran {
                outputs.iter_mut().for_each(<DataStream<N> as DataStreamsEnum>::empty);
            }
            for (&b, buffer) in step.outputs.iter().zip(outputs) {
                workspace[b] = Some(buffer);
            }
            for (b, buffer) in step.inputs.iter().zip(taken) {
                if let (Some(b), Some(buffer)) = (b, buffer) {
                    workspace[*b] = Some(buffer);
                }
            }
            // Copies of what it made, for nodes taking it that can't be the last to read it (counted as its work).
            let copying = Instant::now();
            for copy in &step.copies {
                let mut to = workspace[copy.to].take().expect("buffer in pool");
                if let Some(from) = &workspace[copy.from] {
                    <DataStream<N> as DataStreamsEnum>::copy(from, &mut to);
                }
                workspace[copy.to] = Some(to);
            }
            let copied = if step.copies.is_empty() { Duration::ZERO } else { copying.elapsed() };
            StepTimes::add(&times.work, work + copied);
            if step.state.is_some() {
                StepTimes::add(&times.wait, wait);
            }
        }
    }
}

/// A worker's buffers, for every step. Made once per worker per plan, before the worker runs any of the plan's
/// passes.
struct Workspace<N: NodesEnum> {
    buffers: Vec<Option<Buffer<N::Edges>>>,
}

/// What a node prepared, as the engine keeps it, whatever the node's type.
type PreparedAny<N> = Prepared<Box<dyn Any + Send>, Box<dyn Any + Send>, N>;

/// What about their hardware nodes say someone should decide, by node (see `Runner::user_decisions`).
pub type UserDecisions<N> = Vec<(NodeId, UserDecision<N>)>;

/// A worker's own state for each node (see `Node::WorkerState`), kept from snapshot to snapshot whatever
/// their plans.
type WorkerStates = HashMap<NodeId, Box<dyn Any + Send>>;

/// What each worker's to make its state for a node from, as it first runs a snapshot (see `Node::WorkerState`): the
/// proto-state the node made for each worker, taken by the first of its snapshots the worker runs. Handed on from
/// snapshot to snapshot while the node makes no new ones, so a worker that runs none of one snapshot's passes still
/// takes its own.
type WorkerProtoStates = Arc<Mutex<Vec<Option<Box<dyn Any + Send>>>>>;

impl<N: NodesEnum> Workspace<N> {
    /// The memory of its data buffers.
    fn bytes(&self) -> usize {
        self.buffers.iter().flatten().map(<<N::Edges as Edges>::DataStream as DataStreamsEnum>::bytes).sum()
    }
}

/// What the workers share running one snapshot, besides its graph: the nodes' state, what they share with the GUI, and
/// how long each step's taking (each handed on from the snapshot before, by node: see `Scheduled::new`), and the run's
/// triggers.
struct RunSlots {
    /// By the plan's state slot (`Step::state`).
    states: Vec<Arc<StateSlot>>,
    shared: Arc<SharedByNode>,
    direct: Arc<DirectTriggers>,
    /// By step.
    times: Vec<Arc<StepTimes>>,
}

/// How long a step's taking, on average over its last few dozen passes (in µs, as `f64` bits): working, and
/// waiting for the pass before to hand over the node's state. Workers update them without waiting for each other,
/// so an update's occasionally lost, which doesn't matter for an average.
#[derive(Default)]
struct StepTimes {
    work: AtomicU64,
    wait: AtomicU64,
}

impl StepTimes {
    /// How much of each pass's time goes into the average.
    const WEIGHT: f64 = 0.05;

    fn add(average: &AtomicU64, time: Duration) {
        let micros = time.as_secs_f64() * 1e6;
        let old = f64::from_bits(average.load(Ordering::Relaxed));
        let new = if old == 0.0 { micros } else { old + Self::WEIGHT * (micros - old) };
        average.store(new.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> StepTime {
        let read = |average: &AtomicU64| f64::from_bits(average.load(Ordering::Relaxed));
        StepTime { work_us: read(&self.work), wait_us: read(&self.wait) }
    }
}

/// How long a step's taking, on average (see `StepTimes`).
#[derive(Clone, Copy, Debug, Default)]
pub struct StepTime {
    pub work_us: f64,
    /// Waiting at its sync point, for the pass before to hand over the node's state.
    pub wait_us: f64,
}

/// A node's state, handed from each pass to the next in turn (see the module docs). Each snapshot has its own slot for
/// each node with state, empty until the node's prepared it (`fill`), maybe taking its state from the slot it had in the
/// snapshot before as the new one takes over (see `Prepared`).
struct StateSlot {
    /// The pass whose turn it is to use the state.
    turn: CachePadded<AtomicU64>,
    /// Whether its turns have begun: at the first pass of its snapshot.
    begun: AtomicBool,
    /// Whether the node's prepared it.
    filled: AtomicBool,
    /// While it's still to take the state from the slot before, at its first turn (`from`).
    pending: AtomicBool,
    from: Mutex<Option<From>>,
    state: UnsafeCell<Box<dyn Any + Send>>,
}

/// Where a slot takes its state from, as its snapshot takes over: the slot the node had in the snapshot before, and
/// whether it takes the state as it is (the node kept it) or makes it from the proto-state the node prepared, which
/// the slot holds till then (see `Node::on_snapshot_change`).
struct From {
    slot: Arc<StateSlot>,
    carry: bool,
}

/// What a slot's node does to make its state from the proto-state it prepared (`Node::on_snapshot_change`).
type ChangeFn<'a> = &'a dyn Fn(&mut (dyn Any + Send), &mut (dyn Any + Send));

// Safety: the state is only touched by the pass whose turn it is, which `with` waits for, or before its turns begin by
// whatever has it alone: the node preparing it (`fill`, before it's ready, so before any pass runs against it), or the
// pass taking it from a slot that never ran (`take`).
unsafe impl Sync for StateSlot {}

impl StateSlot {
    /// Holding `state`, prepared.
    #[cfg(test)]
    fn new(state: Box<dyn Any + Send>) -> Self {
        let slot = Self::empty();
        slot.fill(state, None);
        slot
    }

    /// For the node to prepare.
    fn empty() -> Self {
        StateSlot {
            turn: CachePadded::new(AtomicU64::new(0)),
            begun: AtomicBool::new(false),
            filled: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            from: Mutex::new(None),
            state: UnsafeCell::new(Box::new(())),
        }
    }

    /// The node's prepared it: `state`, maybe to take the state from another slot at its first turn (`from`).
    fn fill(&self, state: Box<dyn Any + Send>, from: Option<From>) {
        assert!(!self.filled.load(Ordering::Acquire), "a slot's filled once");
        // Safety: nothing else touches it until it's filled (see `Sync`).
        unsafe { *self.state.get() = state };
        self.pending.store(from.is_some(), Ordering::Relaxed);
        *self.from.lock().unwrap() = from;
        self.filled.store(true, Ordering::Release);
    }

    /// Whether passes can run against it: it's prepared, and so is any slot it's to take its state from (or that
    /// slot's in use).
    fn ready(&self) -> bool {
        if !self.filled.load(Ordering::Acquire) {
            return false;
        }
        let from = self.from.lock().unwrap();
        from.as_ref().is_none_or(|from| from.slot.begun.load(Ordering::Acquire) || from.slot.ready())
    }

    /// Begins its turns at pass `first`.
    fn begin(&self, first: u64) {
        if !self.begun.swap(true, Ordering::AcqRel) {
            self.turn.store(first, Ordering::Release);
        }
    }

    /// Waits for pass `sequence`'s turn, takes the state from the slot before if it's still to (see `From`), runs `f`
    /// on the state, then hands it to the next pass.
    fn with<R>(&self, sequence: u64, change: ChangeFn, f: impl FnOnce(&mut (dyn Any + Send)) -> R) -> R {
        self.with_box(sequence, change, |state| f(state.as_mut()))
    }

    /// The same, giving `f` the box the state's in.
    fn with_box<R>(&self, sequence: u64, change: ChangeFn, f: impl FnOnce(&mut Box<dyn Any + Send>) -> R) -> R {
        let mut spins = 0u32;
        while self.turn.load(Ordering::Acquire) != sequence {
            if spins < 1000 {
                spins += 1;
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
        // Hands over even if `f` panics, so later passes don't wait forever.
        /// Sets the turn to the next pass's when dropped.
        struct HandOver<'a>(&'a AtomicU64, u64);
        impl Drop for HandOver<'_> {
            fn drop(&mut self) {
                self.0.store(self.1, Ordering::Release);
            }
        }
        let _hand_over = HandOver(&self.turn, sequence + 1);
        self.take_from_before(sequence, change);
        // Safety: it's this pass's turn, and no other pass's until `_hand_over` is dropped.
        f(unsafe { &mut *self.state.get() })
    }

    /// Takes the state from the slot before, if it's still to, at pass `sequence` (its first). Only by the pass whose
    /// turn it is, or the one taking the state from it if it never ran.
    fn take_from_before(&self, sequence: u64, change: ChangeFn) {
        if !self.pending.swap(false, Ordering::AcqRel) {
            return;
        }
        let Some(from) = self.from.lock().unwrap().take() else { return };
        // Safety: as the caller's.
        let state = unsafe { &mut *self.state.get() };
        from.slot.lend(sequence, change, |old| {
            if from.carry {
                *state = std::mem::replace(old, Box::new(()));
            } else {
                // The old state, made the new from the proto-state this slot holds; what's left of that goes back in
                // the old slot, to be let go of in the background.
                let mut current = std::mem::replace(old, Box::new(()));
                change(current.as_mut(), state.as_mut());
                *old = std::mem::replace(state, current);
            }
        });
    }

    /// Lends its state to `f`, for the slot after it, at that slot's first pass (`sequence`): once the pass before's
    /// done with it, if it's run; otherwise at once, once it's taken its own from the slot before it.
    fn lend(&self, sequence: u64, change: ChangeFn, f: impl FnOnce(&mut Box<dyn Any + Send>)) {
        if self.begun.load(Ordering::Acquire) {
            self.with_box(sequence, change, f);
        } else {
            // Its snapshot never ran, so nothing else touches it.
            self.take_from_before(sequence, change);
            // Safety: as above.
            f(unsafe { &mut *self.state.get() });
        }
    }

    /// Whether it holds nothing (its state taken, or none).
    fn is_empty(&mut self) -> bool {
        self.state.get_mut().is::<()>()
    }
}

/// How the run is going, over the last second or so.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub workers: usize,
    /// Workers running a pass right now.
    pub busy: usize,
    pub completed: u64,
    /// Passes skipped because the workers couldn't keep up.
    pub dropped: u64,
    pub passes_per_second: usize,
    /// How long passes took to run.
    pub mean_time: Duration,
    pub max_time: Duration,
    /// The longest a pass finished after its data was due.
    pub max_lag: Duration,
    /// Memory allocated for the workers' buffers, in bytes. While workers move on to a new plan, it counts
    /// both their old buffers and their new.
    pub memory: usize,
}

/// A pass that's run, for the stats: when it finished, how long it took, and how late.
struct Record {
    sequence: u64,
    finished: Instant,
    time: Duration,
    lag: Duration,
}

/// What the runner, its workers and its dispatcher share, through every snapshot: counts and recent passes for the
/// stats, the triggers, and what each worker's doing.
#[derive(Default)]
struct RunCommon {
    stop: AtomicBool,
    memory: AtomicUsize,
    busy: AtomicUsize,
    completed: AtomicU64,
    dropped: AtomicU64,
    /// Where workers report to the GUI.
    reports: Reports,
    /// Where the passes send direct triggers: the app's.
    direct: Arc<DirectTriggers>,
    /// What each worker is up to.
    workers: Vec<WorkerSlot>,
    /// The next pass the dispatcher will send.
    next_pass: AtomicU64,
    /// The newest version a pass has run against.
    newest_used: AtomicU64,
}

/// What a worker tells the GUI: a pass it's run, or that it's the first to run against a version.
enum Report {
    Pass(Record),
    FirstUse(FirstUse),
}

/// The queue workers report on, for the GUI to take off (see `Received`). A worker never waits on it: if the GUI
/// hasn't taken them (it's not drawing, say), reports are dropped.
struct Reports(Sender<Report>, Receiver<Report>);

impl Default for Reports {
    fn default() -> Self {
        // A good few seconds' worth.
        let (sender, receiver) = crossbeam_channel::bounded(1024);
        Reports(sender, receiver)
    }
}

/// What the GUI's taken off the reports queue: the passes run over the last `PASS_TIMES`, and the versions first
/// run against since it last asked.
#[derive(Default)]
struct Received {
    recent: VecDeque<Record>,
    first_uses: Vec<FirstUse>,
}

/// The first pass to run against a version (a later one, if passes never ran against those before it).
#[derive(Clone, Copy, Debug)]
pub struct FirstUse {
    pub version: u64,
    pub sequence: u64,
    /// When the pass started.
    pub at: Instant,
}

/// What a pass in `WorkerSlot::pass` is when the worker isn't running one.
const IDLE: u64 = u64::MAX;

/// What a worker's snapshot (`WorkerSlot::version`) is before it's run a pass.
const NO_VERSION: u64 = u64::MAX;

/// What a worker is up to, for the dev tools.
#[derive(Default)]
struct WorkerSlot {
    /// The version of the snapshot of the pass it's running, or last ran (`NO_VERSION` before its first).
    version: AtomicU64,
    /// The pass it's running, or `IDLE`.
    pass: AtomicU64,
    /// Whether it's making buffers for a newer snapshot's plan.
    preparing: AtomicBool,
    /// The memory of its buffers.
    memory: AtomicUsize,
    passes: AtomicU64,
}

/// What a worker is up to.
#[derive(Clone, Debug)]
pub struct WorkerInfo {
    /// The version of the snapshot of the pass it's running, or last ran.
    pub version: Option<u64>,
    /// The pass it's running, if any.
    pub pass: Option<u64>,
    /// Whether it's making buffers for a newer snapshot's plan.
    pub preparing: bool,
    /// The memory of its buffers, in bytes.
    pub memory: usize,
    /// Passes it has run.
    pub passes: u64,
}

/// A snapshot the run's been given (see `Scheduled`).
pub struct ScheduledInfo<N: NodesEnum> {
    pub version: u64,
    pub plan: Arc<Plan<N>>,
    /// Workers with buffers for its plan.
    pub ready: usize,
    /// The nodes still preparing their state for it (see `Node::prepare_for_snapshot`).
    pub preparing: Vec<NodeId>,
    /// What about their hardware its nodes say someone should decide (see `Runner::user_decisions`).
    pub decisions: usize,
    /// How long each of the plan's steps is taking.
    pub step_times: Vec<StepTime>,
}

/// Everything about a run, as of now, for the dev tools.
pub struct RunInfo<N: NodesEnum> {
    /// The snapshot passes are being sent for, once there is one.
    pub running: Option<ScheduledInfo<N>>,
    /// A newer one, not yet ready (workers making buffers for its plan, or its recording's nodes preparing), if
    /// there is one.
    pub preparing: Option<ScheduledInfo<N>>,
    pub workers: Vec<WorkerInfo>,
    /// The next pass the dispatcher will send.
    pub next_pass: u64,
    /// Passes waiting for a worker, and how many can before they're dropped.
    pub queued: usize,
    pub queue_capacity: usize,
    pub stats: Stats,
}

/// A pass on the queue, for a worker to run, with the snapshot it runs: given it as it's sent, so passes only ever see
/// snapshots in order.
struct Job<N: NodesEnum> {
    sequence: u64,
    /// When the pass's data was all due: the end of the time it covers.
    due: Instant,
    scheduled: Arc<Scheduled<N>>,
}

/// A snapshot the run's been given, and what its passes share as they run (`RunSlots`). Each node with state prepares it
/// for the snapshot (`Node::prepare_for_snapshot`), on a thread of its own (see `prepare`): carrying on with the state it had in the
/// snapshot before (when it's the same node with the same data streams in and out, so its state still fits: as within
/// a snapshot, it adapts to any change of settings that leaves them the same), or a fresh one, maybe handed the old.
/// What it shares with the GUI, and its step's times, are handed on likewise, or made afresh. The dispatcher runs it
/// once it's ready: every node has prepared, and its plan's laid out as the one running's, or else every worker has
/// buffers for it (meanwhile, the one before carries on).
struct Scheduled<N: NodesEnum> {
    snapshot: Arc<Snapshot<N>>,
    slots: RunSlots,
    /// Workers with buffers for its plan.
    ready: AtomicUsize,
    /// Whether the snapshot before recorded something else, or nothing (see `PrepareContext::recording_changed`).
    recording_changed: bool,
    /// How many workers the run has.
    workers: usize,
    /// How many of its nodes haven't prepared yet.
    unprepared: AtomicUsize,
    /// What about their hardware its nodes say someone should decide (see `Runner::user_decisions`).
    decisions: Mutex<Vec<(NodeId, UserDecision<N>)>>,
    /// For each node that prepared, what the workers are to make theirs from.
    worker_protos: Mutex<HashMap<NodeId, WorkerProtoStates>>,
    /// What's left of workers' old states as they changed to this snapshot's, to let go of in the background.
    leftovers: Mutex<Vec<Box<dyn Any + Send>>>,
}

/// A node to prepare for a snapshot (see `Scheduled::prepare`): one with state, shared or each worker's.
struct ToPrepare<N: NodesEnum> {
    id: NodeId,
    /// Its step in the plan.
    step: usize,
    /// Its shared state's slot, if it has shared state.
    slot: Option<usize>,
    /// How it was in the snapshot before, if it was in it.
    before: Option<Before<N>>,
}

/// How a node was in the snapshot before (see `ToPrepare`).
struct Before<N: NodesEnum> {
    snapshot: Arc<Snapshot<N>>,
    /// Its step in that snapshot's plan.
    step: usize,
    same_streams: bool,
    /// Its shared state's slot there, if it had shared state.
    slot: Option<Arc<StateSlot>>,
    /// What the workers were to make their states from.
    worker_protos: Option<WorkerProtoStates>,
    /// What it said someone should decide (kept, if it makes nothing new).
    decisions: Vec<UserDecision<N>>,
}

impl<N: NodesEnum> Scheduled<N> {
    /// For `snapshot`, after `before` (if there was one), with what its nodes are to prepare (see `prepare`).
    fn new(snapshot: Arc<Snapshot<N>>, before: Option<&Scheduled<N>>, run: &RunCommon) -> (Self, Vec<ToPrepare<N>>) {
        let (plan, graph) = (&snapshot.plan, &snapshot.graph);
        // The step before of the same node, if there is one, and whether it had the same data streams.
        let step_before = |i: usize| {
            let before = before?;
            let then = &before.snapshot.plan;
            let id = plan.steps[i].id;
            let j = then.steps.iter().position(|step| step.id == id)?;
            let same = then.step_inputs[j] == plan.step_inputs[i] && then.step_outputs[j] == plan.step_outputs[i];
            Some((before, j, same))
        };
        let mut to_prepare = Vec::new();
        let (mut shared, mut times) = (HashMap::new(), Vec::with_capacity(plan.steps.len()));
        for (i, step) in plan.steps.iter().enumerate() {
            let then = step_before(i);
            let before = then.map(|(before, j, same_streams)| {
                let decisions = before.decisions.lock().unwrap();
                Before {
                    snapshot: before.snapshot.clone(),
                    step: j,
                    same_streams,
                    slot: before.snapshot.plan.steps[j].state.map(|state| before.slots.states[state].clone()),
                    worker_protos: before.worker_protos.lock().unwrap().get(&step.id).cloned(),
                    decisions: decisions.iter().filter(|(id, _)| *id == step.id).map(|(_, d)| d.clone()).collect(),
                }
            });
            to_prepare.push(ToPrepare { id: step.id, step: i, slot: step.state, before });
            let Some(entry) = graph.get(step.id) else {
                times.push(Arc::default());
                continue;
            };
            let carried = then
                .filter(|(_, _, same)| *same)
                .and_then(|(before, _, _)| before.slots.shared.0.get(&step.id).cloned());
            shared.insert(step.id, carried.unwrap_or_else(|| Arc::from(entry.node.shared(&plan.step_inputs[i]))));
            // A step's times go with its node, whatever its streams.
            times.push(then.map(|(before, j, _)| before.slots.times[j].clone()).unwrap_or_default());
        }
        let slots = RunSlots {
            states: (0..plan.state_inputs.len()).map(|_| Arc::new(StateSlot::empty())).collect(),
            shared: Arc::new(SharedByNode(shared)),
            direct: run.direct.clone(),
            times,
        };
        let recording_changed = match (before.and_then(|b| b.snapshot.recording.as_ref()), &snapshot.recording) {
            (Some(was), Some(is)) => !Arc::ptr_eq(was, is),
            (was, is) => was.is_some() || is.is_some(),
        };
        let scheduled = Scheduled {
            snapshot,
            slots,
            ready: AtomicUsize::new(0),
            recording_changed,
            workers: run.workers.len(),
            unprepared: AtomicUsize::new(to_prepare.len()),
            decisions: Mutex::default(),
            worker_protos: Mutex::default(),
            leftovers: Mutex::default(),
        };
        (scheduled, to_prepare)
    }

    /// Has each node in `to_prepare` prepare for it (see `Node::prepare_for_snapshot`), one after another, then waits
    /// for what any of them started (`Prepare::Wait`), filling in what each made once it's done. Slots no node
    /// prepares (for a node that's gone) hold nothing. On the thread preparing snapshots (see `prepare_snapshots`), or a
    /// test's.
    fn prepare(&self, to_prepare: Vec<ToPrepare<N>>) {
        let prepared: std::collections::HashSet<usize> = to_prepare.iter().filter_map(|p| p.slot).collect();
        for (slot, state) in self.slots.states.iter().enumerate() {
            if !prepared.contains(&slot) {
                state.fill(Box::new(()), None);
            }
        }
        let snapshot = &self.snapshot;
        let workers = self.workers;
        let mut waiting = Vec::new();
        for node in to_prepare {
            let Some(entry) = snapshot.graph.get(node.id) else {
                self.fill(node, None);
                continue;
            };
            let triggers = snapshot.graph.control_triggers(node.id, &self.slots.direct);
            let previous = node.before.as_ref().and_then(|before| {
                Some(Previous {
                    node: &before.snapshot.graph.get(node.id)?.node,
                    inputs: &before.snapshot.plan.step_inputs[before.step],
                    same_streams: before.same_streams,
                })
            });
            let context = PrepareContext {
                node: node.id,
                inputs: &snapshot.plan.step_inputs[node.step],
                controls: snapshot.controls.get(&node.id).map_or(&[][..], Vec::as_slice),
                recording: snapshot.recording.as_deref(),
                triggers: &triggers,
                previous,
                recording_changed: self.recording_changed,
                workers,
            };
            let prepare = || entry.node.prepare_for_snapshot(&context);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(prepare)) {
                Ok(Prepare::Now(prepared)) => self.fill(node, Some(prepared)),
                Ok(Prepare::Wait(wait)) => waiting.push((node, wait)),
                Err(_) => self.fill(node, None),
            }
        }
        for (node, wait) in waiting {
            self.fill(node, std::panic::catch_unwind(std::panic::AssertUnwindSafe(wait)).ok());
        }
    }

    /// Fills in what a node prepared (`None` if it panicked): its shared state's slot, and what the workers are to make
    /// theirs from, keeping what it says someone should decide (and what it said before, if it makes nothing new). A
    /// node that didn't make the state it needs (new to the run, or its data streams changed) failed to prepare: it
    /// doesn't run, and someone's to decide about it.
    fn fill(&self, node: ToPrepare<N>, prepared: Option<PreparedAny<N>>) {
        let ToPrepare { id, slot, before, .. } = node;
        let entry = self.snapshot.graph.get(id);
        let failed = |why: &str| (Box::new(()) as Box<dyn Any + Send>, None, Some(why.to_owned()));
        let (mut shared, mut worker_protos, mut problem) = (None, None, None);
        let mut decisions = Vec::new();
        match prepared {
            None => problem = Some("it failed (it panicked)".to_owned()),
            Some(Prepared { proto_shared_state, proto_worker_state, decisions: said }) => {
                decisions = said;
                let same_streams = before.as_ref().is_some_and(|before| before.same_streams);
                if proto_shared_state.is_none() && proto_worker_state.is_none() && same_streams {
                    // Nothing new: what it said before still stands.
                    let mut kept = before.as_ref().map_or_else(Vec::new, |before| before.decisions.clone());
                    kept.append(&mut decisions);
                    decisions = kept;
                }
                if slot.is_some() {
                    shared = Some(match (proto_shared_state, before.as_ref().and_then(|b| b.slot.clone())) {
                        (Some(proto), Some(old)) => (proto, Some(From { slot: old, carry: false }), None),
                        (Some(proto), None) => (proto, None, None),
                        (None, Some(old)) if same_streams => {
                            (Box::new(()) as Box<dyn Any + Send>, Some(From { slot: old, carry: true }), None)
                        }
                        (None, _) => failed("it didn't make its state, which it needs (it's new, or its data changed)"),
                    });
                }
                let workers = self.workers;
                let fresh =
                    |protos: Vec<Box<dyn Any + Send>>| Arc::new(Mutex::new(protos.into_iter().map(Some).collect()));
                match (proto_worker_state, before.as_ref().and_then(|b| b.worker_protos.clone())) {
                    (Some(protos), _) if protos.len() == workers => worker_protos = Some(fresh(protos)),
                    (Some(protos), _) => {
                        problem = Some(format!("it made {} workers' states, not {workers}", protos.len()))
                    }
                    (None, Some(protos)) if same_streams => worker_protos = Some(protos),
                    // New to the run, or its data streams changed: each worker starts with the default.
                    (None, _) => {
                        let node = &entry.expect("a node that prepared").node;
                        worker_protos = Some(fresh((0..workers).map(|_| node.new_worker_state()).collect()));
                    }
                }
            }
        }
        if let Some((_, _, Some(why))) = &shared {
            problem.get_or_insert_with(|| why.clone());
        }
        if let Some(why) = problem {
            // It doesn't run: no shared state, and no worker state, to run with.
            let name = entry.map_or("A node", |entry| entry.node.info().name);
            log::error!("{name} couldn't prepare for snapshot v{}: {why}", self.snapshot.version);
            decisions.push(UserDecision { what: format!("couldn't prepare: {why}"), fix: None });
            shared = slot.map(|_| failed(&why));
            worker_protos = None;
        }
        if let Some(entry) = entry {
            let mut found = self.decisions.lock().unwrap();
            for decision in decisions {
                log::warn!("{}: {}", entry.label, decision.what);
                found.push((id, decision));
            }
        }
        if let Some(protos) = worker_protos {
            self.worker_protos.lock().unwrap().insert(id, protos);
        }
        if let (Some(slot), Some((state, from, _))) = (slot, shared) {
            self.slots.states[slot].fill(state, from);
        } else if let Some(slot) = slot {
            self.slots.states[slot].fill(Box::new(()), None);
        }
        self.unprepared.fetch_sub(1, Ordering::Release);
    }

    /// Changes worker `worker`'s own states (`states`) to this snapshot's, as it first runs one of its passes: each the
    /// proto-state its node made for the worker, if it made one, and none for nodes that no longer have any here. The
    /// old states are let go of in the background, with this.
    fn change_worker_states(&self, worker: usize, states: &mut WorkerStates) {
        let protos = self.worker_protos.lock().unwrap();
        let mut leftovers = Vec::new();
        states.retain(|id, state| {
            let kept = protos.contains_key(id);
            if !kept {
                leftovers.push(std::mem::replace(state, Box::new(())));
            }
            kept
        });
        for (id, node_protos) in protos.iter() {
            let Some(proto) = node_protos.lock().unwrap().get_mut(worker).and_then(Option::take) else { continue };
            if let Some(old) = states.insert(*id, proto) {
                leftovers.push(old);
            }
        }
        drop(protos);
        self.leftovers.lock().unwrap().append(&mut leftovers);
    }

    /// Whether every node's prepared its state for it (see `prepare`).
    fn prepared(&self) -> bool {
        self.unprepared.load(Ordering::Acquire) == 0 && self.slots.states.iter().all(|slot| slot.ready())
    }

    /// The nodes still preparing their state for it.
    fn preparing(&self) -> Vec<NodeId> {
        let plan = &self.snapshot.plan;
        plan.steps
            .iter()
            .filter(|step| step.state.is_some_and(|slot| !self.slots.states[slot].ready()))
            .map(|step| step.id)
            .collect()
    }

    /// Readies it to run from pass `first`, which covers the time from `at`: the turns of the node state made for it
    /// begin there (what's handed on carries on), and its recording, if it has one, takes effect then.
    fn begin(&self, first: u64, at: Instant) {
        for slot in &self.slots.states {
            slot.begin(first);
        }
        if let Some(recording) = &self.snapshot.recording {
            recording.take_effect(at);
        }
    }
}

impl<N: NodesEnum> Drop for Scheduled<N> {
    /// The nodes' state it had that isn't handed on (to a newer snapshot) is let go of in the background (see
    /// `let_go_of`).
    fn drop(&mut self) {
        let states = std::mem::take(&mut self.slots.states);
        let_go_of(
            states
                .into_iter()
                .filter_map(Arc::into_inner)
                .filter_map(|mut slot| (!slot.is_empty()).then_some(slot))
                .collect(),
        );
        // What's left of workers' old states, and the proto-states no worker took.
        let mut rest = std::mem::take(self.leftovers.get_mut().unwrap());
        for protos in std::mem::take(self.worker_protos.get_mut().unwrap()).into_values() {
            if let Some(protos) = Arc::into_inner(protos) {
                rest.extend(protos.into_inner().unwrap().into_iter().flatten());
            }
        }
        if !rest.is_empty() {
            let dropping = std::thread::Builder::new().name("dropping worker state".into()).spawn(move || drop(rest));
            if let Ok(thread) = dropping {
                crate::background::finish_later(thread);
            }
        }
    }
}

/// Lets go of nodes' state in the background, each on a thread of its own: it can take a while (a capture thread
/// stopping, waiting on its device, say), and this may be on a worker, whose passes mustn't wait, or the GUI. Each its
/// own, so one slow to stop doesn't keep another node's device going.
fn let_go_of(states: Vec<StateSlot>) {
    for state in states {
        let dropping = std::thread::Builder::new().name("dropping node state".into()).spawn(move || drop(state));
        if let Ok(thread) = dropping {
            crate::background::finish_later(thread);
        }
    }
}

/// What the runner, its workers and its dispatcher share about the snapshots: the newest given to the run (ready or
/// not), the one passes are being sent for, and the pass queue.
struct Schedule<N: NodesEnum> {
    latest: ArcSwap<Scheduled<N>>,
    running: ArcSwapOption<Scheduled<N>>,
    /// Taken by the dispatcher; dropping it, when it stops, lets the workers know there'll be no more.
    sender: Mutex<Option<Sender<Job<N>>>>,
    receiver: Receiver<Job<N>>,
}

/// The snapshot waiting its turn to be prepared, if there is one (see `prepare_snapshots`).
struct Waiting<N: NodesEnum> {
    snapshot: Mutex<Option<Snapshot<N>>>,
    given: Condvar,
}

/// Prepares the snapshots the run's given, one at a time: `first`'s nodes, for the first, then each that comes, after
/// the latest (see `Scheduled::prepare`). One waits its turn while another's prepared, and a newer one given the run
/// meanwhile replaces it, so a burst of changes (a slider dragged) has the latest prepared next, not each in turn. Ends
/// once the run stops.
fn prepare_snapshots<N: NodesEnum>(
    schedule: &Schedule<N>,
    shared: &RunCommon,
    waiting: &Waiting<N>,
    first: Vec<ToPrepare<N>>,
) {
    schedule.latest.load_full().prepare(first);
    loop {
        let snapshot = {
            let mut snapshot = waiting.snapshot.lock().unwrap();
            loop {
                if shared.stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(snapshot) = snapshot.take() {
                    break snapshot;
                }
                snapshot = waiting.given.wait_timeout(snapshot, Duration::from_millis(50)).unwrap().0;
            }
        };
        let before = schedule.latest.load_full();
        if before.snapshot.plan.same_shape(&snapshot.plan) {
            log::trace!("Snapshot v{} carries on with the same buffers", snapshot.version);
        } else {
            log::info!("Snapshot v{} needs new buffers: {}", snapshot.version, describe(&snapshot.plan));
        }
        let (scheduled, to_prepare) = Scheduled::new(Arc::new(snapshot), Some(&before), shared);
        drop(before);
        let scheduled = Arc::new(scheduled);
        schedule.latest.store(scheduled.clone());
        scheduled.prepare(to_prepare);
    }
}

/// The graph running: a dispatcher and a pool of workers, until dropped.
pub struct Runner<N: NodesEnum> {
    schedule: Arc<Schedule<N>>,
    shared: Arc<RunCommon>,
    /// The snapshot waiting its turn to be prepared (see `prepare_snapshots`).
    waiting: Arc<Waiting<N>>,
    threads: Vec<JoinHandle<()>>,
    workers: usize,
    /// The version of the latest snapshot given to the run.
    version: AtomicU64,
    /// The GUI's, from the workers' reports.
    received: RefCell<Received>,
}

impl<N: NodesEnum> Runner<N> {
    pub fn start(snapshot: &Snapshot<N>, workers: usize) -> Self {
        Self::start_with(snapshot, workers, Arc::default())
    }

    /// The same, with its direct triggers going to `direct`.
    pub fn start_with(snapshot: &Snapshot<N>, workers: usize, direct: Arc<DirectTriggers>) -> Self {
        let workers = workers.max(1);
        assert!(snapshot.started.is_some(), "a run's snapshots say when it started");
        let shared = Arc::new(RunCommon {
            workers: (0..workers)
                .map(|_| WorkerSlot { version: AtomicU64::new(NO_VERSION), ..Default::default() })
                .collect(),
            direct,
            ..Default::default()
        });
        let (first, to_prepare) = Scheduled::new(Arc::new(snapshot.clone()), None, &shared);
        let first = Arc::new(first);
        log::info!("Running on {workers} workers, {} ms passes", PASS_DURATION * 1000.0);
        log::debug!("Snapshot v{}: {}", snapshot.version, describe(&snapshot.plan));
        // A few passes may queue, beyond which they're dropped.
        let (sender, receiver) = crossbeam_channel::bounded(queue_capacity(workers));
        let schedule = Arc::new(Schedule {
            latest: ArcSwap::new(first),
            running: ArcSwapOption::empty(),
            sender: Mutex::new(Some(sender)),
            receiver,
        });

        let mut threads = Vec::new();
        for index in 0..workers {
            let (schedule, shared) = (schedule.clone(), shared.clone());
            let worker = move || work(&schedule, &shared, index);
            threads
                .push(std::thread::Builder::new().name(format!("pass worker {index}")).spawn(worker).expect("worker"));
        }
        let (dispatcher_schedule, dispatcher_shared) = (schedule.clone(), shared.clone());
        let dispatcher = move || dispatch(&dispatcher_schedule, &dispatcher_shared);
        threads.push(std::thread::Builder::new().name("pass dispatcher".into()).spawn(dispatcher).expect("dispatcher"));
        // Not joined as the run stops: a node it's waiting for (a device opening) can take a while.
        let waiting = Arc::new(Waiting { snapshot: Mutex::new(None), given: Condvar::new() });
        let (preparer_schedule, preparer_shared, preparer_waiting) =
            (schedule.clone(), shared.clone(), waiting.clone());
        let preparer = move || prepare_snapshots(&preparer_schedule, &preparer_shared, &preparer_waiting, to_prepare);
        std::thread::Builder::new().name("preparing snapshots".into()).spawn(preparer).expect("preparer");

        let (version, received) = (AtomicU64::new(snapshot.version), RefCell::default());
        Runner { schedule, shared, waiting, threads, workers, version, received }
    }

    /// The graph of the latest snapshot given to the run.
    #[cfg(test)]
    pub fn graph(&self) -> Arc<super::Graph<N>> {
        self.schedule.latest.load().snapshot.graph.clone()
    }

    /// Where the passes send direct triggers.
    pub fn direct_triggers(&self) -> &DirectTriggers {
        &self.shared.direct
    }

    /// What the nodes running share with the GUI: the snapshot being run's, or, until one is, the one being got ready.
    pub fn shared(&self) -> Arc<SharedByNode> {
        let scheduled = self.schedule.running.load_full().unwrap_or_else(|| self.schedule.latest.load_full());
        scheduled.slots.shared.clone()
    }

    /// The acquisition clock: seconds of passes sent so far.
    pub fn clock(&self) -> f64 {
        self.shared.next_pass.load(Ordering::Relaxed) as f64 * PASS_DURATION
    }

    /// Moves the run on to a newer snapshot, once its nodes have prepared their state for it, carrying on with what
    /// they had or not (see `Scheduled`): it waits its turn to be prepared, replacing any that's waiting already (see
    /// `prepare_snapshots`). Returns whether its plan's laid out as the latest's given the run before (as after a change
    /// of settings), so it needs no new buffers; otherwise it also waits for the workers to make them. Meanwhile, the
    /// one before carries on.
    pub fn update(&self, snapshot: &Snapshot<N>) -> bool {
        if self.version.swap(snapshot.version, Ordering::Relaxed) == snapshot.version {
            return true;
        }
        let mut waiting = self.waiting.snapshot.lock().unwrap();
        let before =
            waiting.as_ref().map_or_else(|| self.schedule.latest.load().snapshot.plan.clone(), |w| w.plan.clone());
        let same_buffers = before.same_shape(&snapshot.plan);
        if let Some(replaced) = waiting.replace(snapshot.clone()) {
            log::trace!("Snapshot v{} replaced v{} waiting to be prepared", snapshot.version, replaced.version);
        }
        drop(waiting);
        self.waiting.given.notify_one();
        same_buffers
    }

    /// What about their hardware the nodes of the latest snapshot given to the run say someone should decide, once they
    /// have all prepared, with its version. It runs meanwhile; the app deals with them by giving the run another (with
    /// the nodes' settings fixed, or the nodes disabled), or stopping.
    pub fn user_decisions(&self) -> Option<(u64, UserDecisions<N>)> {
        let latest = self.schedule.latest.load();
        if !latest.prepared() {
            return None;
        }
        let decisions = latest.decisions.lock().unwrap();
        (!decisions.is_empty()).then(|| (latest.snapshot.version, decisions.clone()))
    }

    /// The versions passes have started running against since this was last asked.
    pub fn first_uses(&self) -> Vec<FirstUse> {
        std::mem::take(&mut self.received().first_uses)
    }

    /// What the workers have reported, taking what's waiting off the queue, and forgetting passes older than
    /// `PASS_TIMES`.
    fn received(&self) -> std::cell::RefMut<'_, Received> {
        let mut received = self.received.borrow_mut();
        for report in self.shared.reports.1.try_iter() {
            match report {
                Report::Pass(record) => received.recent.push_back(record),
                Report::FirstUse(first) => received.first_uses.push(first),
            }
        }
        let now = Instant::now();
        while received.recent.front().is_some_and(|r| now - r.finished > PASS_TIMES) {
            received.recent.pop_front();
        }
        received
    }

    /// Everything about the run as of now: the snapshots, the workers and dispatcher, and the stats.
    pub fn inspect(&self) -> RunInfo<N> {
        let info = |scheduled: &Scheduled<N>| ScheduledInfo {
            version: scheduled.snapshot.version,
            plan: scheduled.snapshot.plan.clone(),
            ready: scheduled.ready.load(Ordering::Relaxed),
            preparing: scheduled.preparing(),
            decisions: scheduled.decisions.lock().unwrap().len(),
            step_times: scheduled.slots.times.iter().map(|times| times.get()).collect(),
        };
        let running = self.schedule.running.load().as_deref().map(info);
        let latest = self.schedule.latest.load_full();
        let preparing = (running.as_ref().map(|r| r.version) != Some(latest.snapshot.version)).then(|| info(&latest));
        let workers = self
            .shared
            .workers
            .iter()
            .map(|w| {
                let (version, pass) = (w.version.load(Ordering::Relaxed), w.pass.load(Ordering::Relaxed));
                WorkerInfo {
                    version: (version != NO_VERSION).then_some(version),
                    pass: (pass != IDLE).then_some(pass),
                    preparing: w.preparing.load(Ordering::Relaxed),
                    memory: w.memory.load(Ordering::Relaxed),
                    passes: w.passes.load(Ordering::Relaxed),
                }
            })
            .collect();
        RunInfo {
            running,
            preparing,
            workers,
            next_pass: self.shared.next_pass.load(Ordering::Relaxed),
            queued: self.schedule.receiver.len(),
            queue_capacity: queue_capacity(self.workers),
            stats: self.stats(),
        }
    }

    /// Each pass run over the last few seconds: when its data was due (seconds on the acquisition clock), and how
    /// long it took to run (in seconds), in order.
    pub fn pass_times(&self) -> Vec<[f64; 2]> {
        let mut times: Vec<[f64; 2]> = self
            .received()
            .recent
            .iter()
            .map(|r| [(r.sequence + 1) as f64 * PASS_DURATION, r.time.as_secs_f64()])
            .collect();
        times.sort_by(|a, b| a[0].total_cmp(&b[0]));
        times
    }

    pub fn stats(&self) -> Stats {
        let now = Instant::now();
        let received = self.received();
        let last_second: Vec<&Record> =
            received.recent.iter().filter(|r| now - r.finished <= Duration::from_secs(1)).collect();
        let count = last_second.len();
        let total: Duration = last_second.iter().map(|r| r.time).sum();
        Stats {
            workers: self.workers,
            busy: self.shared.busy.load(Ordering::Relaxed),
            completed: self.shared.completed.load(Ordering::Relaxed),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
            passes_per_second: count,
            mean_time: if count > 0 { total / count as u32 } else { Duration::ZERO },
            max_time: last_second.iter().map(|r| r.time).max().unwrap_or_default(),
            max_lag: last_second.iter().map(|r| r.lag).max().unwrap_or_default(),
            memory: self.shared.memory.load(Ordering::Relaxed),
        }
    }
}

impl<N: NodesEnum> Drop for Runner<N> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        // Passes still queued when the workers stopped were never run.
        self.shared.dropped.fetch_add(self.schedule.receiver.len() as u64, Ordering::Relaxed);
        let (completed, dropped) =
            (self.shared.completed.load(Ordering::Relaxed), self.shared.dropped.load(Ordering::Relaxed));
        log::info!("Stopped after {completed} passes ({dropped} dropped)");
    }
}

/// How long passes' times are kept (see `Runner::pass_times`).
const PASS_TIMES: Duration = Duration::from_secs(10);

/// How long an idle worker waits for a pass before checking for a newer snapshot to get ready for.
const IDLE_CHECK: Duration = Duration::from_millis(2);

/// A worker: runs the passes it's sent, each on buffers for its snapshot's plan; and makes buffers for the newest
/// snapshot's plan, if it hasn't any laid out like it, so it's ready for its passes before they come (keeping those it
/// has until no pass needs them). Ends when the run stops, once every pass sent to it is done.
fn work<N: NodesEnum>(schedule: &Schedule<N>, shared: &RunCommon, index: usize) {
    let slot = &shared.workers[index];
    slot.pass.store(IDLE, Ordering::Relaxed);
    // Its buffers, each for the plans laid out like the one they were made for.
    let mut workspaces: Vec<(Arc<Plan<N>>, Workspace<N>)> = Vec::new();
    let memory = |workspaces: &[(Arc<Plan<N>>, Workspace<N>)]| workspaces.iter().map(|(_, w)| w.bytes()).sum::<usize>();
    let make = |scheduled: &Scheduled<N>, workspaces: &mut Vec<(Arc<Plan<N>>, Workspace<N>)>| {
        let snapshot = &scheduled.snapshot;
        slot.preparing.store(true, Ordering::Relaxed);
        let workspace = snapshot.plan.workspace();
        let bytes = workspace.bytes();
        shared.memory.fetch_add(bytes, Ordering::Relaxed);
        workspaces.push((snapshot.plan.clone(), workspace));
        slot.memory.store(memory(workspaces), Ordering::Relaxed);
        slot.preparing.store(false, Ordering::Relaxed);
        log::debug!(
            "Worker {index} ready for snapshot v{}, with {:.2} MB of buffers",
            snapshot.version,
            bytes as f64 / 1e6
        );
        workspaces.len() - 1
    };
    // The newest snapshot it's got ready for.
    let mut ready_for = None;
    // Its own state for the nodes with some, and the snapshot it's been changed to.
    let (mut worker_states, mut changed_to) = (WorkerStates::new(), None);
    loop {
        let latest = schedule.latest.load_full();
        if ready_for != Some(latest.snapshot.version) {
            if !workspaces.iter().any(|(plan, _)| plan.same_shape(&latest.snapshot.plan)) {
                make(&latest, &mut workspaces);
            }
            latest.ready.fetch_add(1, Ordering::Release);
            ready_for = Some(latest.snapshot.version);
        }
        drop(latest);
        match schedule.receiver.recv_timeout(IDLE_CHECK) {
            Ok(job) => {
                shared.busy.fetch_add(1, Ordering::Relaxed);
                slot.pass.store(job.sequence, Ordering::Relaxed);
                let start = Instant::now();
                let snapshot = &job.scheduled.snapshot;
                slot.version.store(snapshot.version, Ordering::Relaxed);
                if shared.newest_used.fetch_max(snapshot.version, Ordering::Relaxed) < snapshot.version {
                    let first = FirstUse { version: snapshot.version, sequence: job.sequence, at: start };
                    let _ = shared.reports.0.try_send(Report::FirstUse(first));
                }
                // Buffers for its plan: made by now, unless a newer snapshot came along before this one ran.
                let at = workspaces.iter().position(|(plan, _)| plan.same_shape(&snapshot.plan));
                let at = at.unwrap_or_else(|| make(&job.scheduled, &mut workspaces));
                let pass = PassId { version: snapshot.version, sequence: job.sequence, node: None, node_type: None };
                if changed_to != Some(snapshot.version) {
                    job.scheduled.change_worker_states(index, &mut worker_states);
                    changed_to = Some(snapshot.version);
                }
                let workspace = &mut workspaces[at].1;
                snapshot.plan.run(snapshot, pass, workspace, &mut worker_states, &job.scheduled.slots);
                let finished = Instant::now();
                slot.pass.store(IDLE, Ordering::Relaxed);
                slot.passes.fetch_add(1, Ordering::Relaxed);
                shared.busy.fetch_sub(1, Ordering::Relaxed);
                shared.completed.fetch_add(1, Ordering::Relaxed);
                let (time, lag) = (finished - start, finished.saturating_duration_since(job.due));
                let record = Record { sequence: job.sequence, finished, time, lag };
                let _ = shared.reports.0.try_send(Report::Pass(record));
                // Buffers no pass will need: neither this one's plan nor the newest's.
                let newest = schedule.latest.load().snapshot.plan.clone();
                workspaces.retain(|(plan, workspace)| {
                    let kept = plan.same_shape(&snapshot.plan) || plan.same_shape(&newest);
                    if !kept {
                        shared.memory.fetch_sub(workspace.bytes(), Ordering::Relaxed);
                    }
                    kept
                });
                slot.memory.store(memory(&workspaces), Ordering::Relaxed);
            }
            Err(_) if shared.stop.load(Ordering::Relaxed) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    shared.memory.fetch_sub(memory(&workspaces), Ordering::Relaxed);
}

/// The dispatcher: releases each pass when its time is up, with the snapshot it's to run: the one running, until a
/// newer one's ready (see `Scheduled`), which it then begins (its fresh node state's turns, and its recording's
/// effective instant) at that pass. The clock starts once the first snapshot's ready, so making buffers doesn't count
/// against the run.
fn dispatch<N: NodesEnum>(schedule: &Schedule<N>, shared: &RunCommon) {
    let Some(sender) = schedule.sender.lock().unwrap().take() else { return };
    let duration = Duration::from_secs_f64(PASS_DURATION);
    let mut current: Option<Arc<Scheduled<N>>> = None;
    let (mut sequence, mut due) = (0, Instant::now());
    // Passes dropped in a row, so a run of them is reported once.
    let mut dropping = 0u64;
    while !shared.stop.load(Ordering::Relaxed) {
        let latest = schedule.latest.load_full();
        if !current.as_ref().is_some_and(|running| Arc::ptr_eq(running, &latest)) {
            // Ready: laid out as the one running, or with buffers on every worker; and with every node's state
            // prepared.
            let same_buffers = current.as_ref().is_some_and(|c| c.snapshot.plan.same_shape(&latest.snapshot.plan));
            let buffers = same_buffers || latest.ready.load(Ordering::Acquire) >= shared.workers.len();
            if buffers && latest.prepared() {
                if current.is_none() {
                    due = Instant::now() + duration;
                }
                latest.begin(sequence, due - duration);
                log::debug!("Snapshot v{} running from pass {sequence}", latest.snapshot.version);
                schedule.running.store(Some(latest.clone()));
                current = Some(latest);
            }
        }
        let Some(scheduled) = &current else {
            std::thread::sleep(IDLE_CHECK);
            continue;
        };
        let now = Instant::now();
        if now < due {
            std::thread::sleep((due - now).min(Duration::from_millis(5)));
            continue;
        }
        match sender.try_send(Job { sequence, due, scheduled: scheduled.clone() }) {
            Ok(()) => {
                if dropping > 0 {
                    log::warn!("Dropped {dropping} passes: the workers couldn't keep up");
                    dropping = 0;
                }
                sequence += 1;
                shared.next_pass.store(sequence, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                shared.dropped.fetch_add(1, Ordering::Relaxed);
                dropping += 1;
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
        due += duration;
    }
    // Dropping the sender lets the workers finish what's queued, then stop.
}

/// How many passes can queue for the workers before they're dropped.
fn queue_capacity(workers: usize) -> usize {
    2 * workers
}

/// How many workers to use by default: half the cores (leaving the rest for the GUI, nodes' own threads and
/// everything else), up to 4.
pub fn default_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4)
}

#[cfg(test)]
pub mod tests {
    use super::super::{Port, PortRole};
    use super::*;
    use crate::NodeType;
    use crate::engine::Graph;
    use crate::engine::NodeTypeEnum;

    use crate::Buffer;
    use crate::Nodes;

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let node_type = NodeType::named(name).unwrap();
        graph.add(Nodes::create(node_type), [0.0, 0.0])
    }

    /// An Acquisition Board, acquiring from a test board of its own (see `serve_test_board`).
    fn add_board(graph: &mut Graph<Nodes>) -> NodeId {
        use crate::standard::ephys::board::{Source, serve_test_board};
        let board = add(graph, "Acquisition Board");
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        board
    }

    fn link(graph: &mut Graph<Nodes>, from: NodeId, output: usize, to: NodeId, input: usize) {
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(from, PortRole::Output, output), port(to, PortRole::Input, input)).unwrap();
    }

    #[test]
    fn state_is_handed_from_pass_to_pass_in_order() {
        let slot = Arc::new(StateSlot::new(Box::new(Vec::<u64>::new())));
        // Passes run on several threads, started in any order.
        let threads: Vec<_> = (0..8u64)
            .rev()
            .map(|sequence| {
                let slot = slot.clone();
                std::thread::spawn(move || {
                    slot.with(sequence, &|_, _| {}, |state| state.downcast_mut::<Vec<u64>>().unwrap().push(sequence))
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let order = slot.with(8, &|_, _| {}, |state| state.downcast_ref::<Vec<u64>>().unwrap().clone());
        assert_eq!(order, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn copies_for_nodes_taking_are_made_before_the_data_is_edited() {
        use crate::standard::ephys::board::{Source, serve_test_board};

        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        let filter = add(&mut graph, "Band-pass Filter");
        let viewer = add(&mut graph, "LFP Viewer");
        let writer = add(&mut graph, "Write Raw");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, viewer, 0);
        link(&mut graph, board, 0, writer, 0);
        // Recording, so the writer runs; somewhere its files can't be made (in a file), so it doesn't take its copy.
        let file = std::env::temp_dir().join(format!("ephys-rs-not-a-directory-{}", std::process::id()));
        std::fs::write(&file, b"").unwrap();
        let place = crate::base_directory::Place::directory(file.join("recording"));
        let recording = Some(Arc::new(crate::engine::Recording::new(Some(place), chrono::Local::now())));
        let snapshot = Arc::new(Snapshot::bake(0, Arc::new(graph), Some(Instant::now()), recording));
        let plan = &snapshot.plan;
        let (scheduled, to_prepare) = Scheduled::new(snapshot.clone(), None, &RunCommon::default());
        scheduled.prepare(to_prepare);
        let slots = &scheduled.slots;
        let mut workspace = plan.workspace();
        let mut worker_states = WorkerStates::new();
        scheduled.change_worker_states(0, &mut worker_states);
        let filtered = plan.steps.iter().find(|s| s.id == filter).unwrap().outputs[0];
        let copied = plan.steps.iter().find(|s| s.id == writer).unwrap().inputs[0].unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        for sequence in 0.. {
            plan.run(
                &snapshot,
                PassId { version: 0, sequence, node: None, node_type: None },
                &mut workspace,
                &mut worker_states,
                slots,
            );
            let (Some(Buffer::Continuous(filtered)), Some(Buffer::Continuous(copied))) =
                (&workspace.buffers[filtered], &workspace.buffers[copied])
            else {
                panic!("continuous buffers")
            };
            // The writer's copy is the board's data as it was made, not filtered.
            assert_eq!(copied.len, filtered.len);
            if copied.len > 0 && copied.channel(0) != filtered.channel(0) {
                break;
            }
            assert!(Instant::now() < deadline, "no samples came");
            std::thread::sleep(Duration::from_secs_f64(PASS_DURATION));
        }
    }

    #[test]
    fn a_node_slow_to_let_go_doesnt_hold_up_the_others() {
        use std::sync::atomic::AtomicBool;
        /// Takes a while to drop, as a device does to close.
        struct Slow;
        impl Drop for Slow {
            fn drop(&mut self) {
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        /// Says when it's dropped.
        struct Quick(Arc<AtomicBool>);
        impl Drop for Quick {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let states = vec![StateSlot::new(Box::new(Slow)), StateSlot::new(Box::new(Quick(dropped.clone())))];
        let started = Instant::now();
        let_go_of(states);
        assert!(started.elapsed() < Duration::from_millis(500), "not on this thread");
        while !dropped.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(dropped.load(Ordering::Relaxed), "the quick one went without waiting for the slow one");
    }

    #[test]
    fn working_memory_is_each_workers_own() {
        let mut graph = Graph::default();
        let board = add_board(&mut graph);
        let reference = add(&mut graph, "Reference");
        let filter = add(&mut graph, "Band-pass Filter");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, reference, 0);
        link(&mut graph, reference, 0, filter, 0);
        link(&mut graph, filter, 0, viewer, 0);
        let plan = Plan::new(&graph, false);
        let step = |id| plan.steps.iter().find(|s| s.id == id).unwrap();
        assert!(step(reference).state.is_none(), "no sync point");
        assert!(step(filter).state.is_some(), "the filter's history is carried over");
        // Each worker starts with the reference's default worker state.
        let runner = Runner::start(&Snapshot::new(0, graph), 2);
        let stats = wait_for_passes(&runner, 10);
        assert!(stats.completed >= 10, "{stats:?}");
        assert!(runner.user_decisions().is_none(), "the reference had its workers' state");
    }

    #[test]
    fn passes_filter_and_detect_spikes() {
        use crate::standard::ephys::board::{Source, serve_test_board};

        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        let filter = add(&mut graph, "Band-pass Filter");
        let detector = add(&mut graph, "Spike Detector");
        let viewer = add(&mut graph, "Spike Viewer");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, detector, 0);
        link(&mut graph, detector, 0, viewer, 0);
        let snapshot = Arc::new(Snapshot::new(0, graph));
        let plan = &snapshot.plan;
        let (scheduled, to_prepare) = Scheduled::new(snapshot.clone(), None, &RunCommon::default());
        scheduled.prepare(to_prepare);
        let slots = &scheduled.slots;
        let mut workspace = plan.workspace();
        let mut worker_states = WorkerStates::new();
        scheduled.change_worker_states(0, &mut worker_states);
        // A second of samples, as they come from the board in real time.
        let (mut spikes, mut samples) = (0, 0);
        let filter_output = plan.steps.iter().find(|s| s.id == filter).unwrap().outputs[0];
        let detector_output = plan.steps.iter().find(|s| s.id == detector).unwrap().outputs[0];
        let deadline = Instant::now() + Duration::from_secs(5);
        for sequence in 0.. {
            plan.run(
                &snapshot,
                PassId { version: 0, sequence, node: None, node_type: None },
                &mut workspace,
                &mut worker_states,
                slots,
            );
            let Some(Buffer::Spikes(output)) = &workspace.buffers[detector_output] else { panic!("spikes buffer") };
            spikes += output.spikes.len();
            let Some(Buffer::Continuous(filtered)) = &workspace.buffers[filter_output] else { panic!("continuous") };
            assert!(filtered.len <= filtered.capacity);
            assert!(filtered.channel(0)[..filtered.len].iter().all(|x| x.is_finite() && x.abs() < 1000.0));
            samples += filtered.len;
            if samples >= 30000 || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_secs_f64(PASS_DURATION));
        }
        assert!(samples >= 30000, "a second at 30 kHz, got {samples} samples");
        // The test board puts a spike in about 1% of milliseconds on each of 64 channels: about 640 in a second.
        assert!((100..3000).contains(&spikes), "found {spikes} spikes in a second");
    }

    #[test]
    fn runs_the_example_graph() {
        // One worker, as on a two-core machine, and several.
        for workers in [1, 2] {
            let snapshot = Snapshot::new(0, crate::example_graph());
            let runner = Runner::start(&snapshot, workers);
            let stats = wait_for_passes(&runner, 5);
            drop(runner);
            assert!(stats.completed >= 5, "expected passes to run on {workers} workers: {stats:?}");
            assert!(stats.memory > 0);
        }
    }

    /// Waits (a few seconds at most, on a busy machine) for `passes` passes to have run, then the stats.
    pub fn wait_for_passes(runner: &Runner<Nodes>, passes: u64) -> Stats {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let stats = runner.stats();
            if stats.completed >= passes || Instant::now() > deadline {
                return stats;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The snapshot passes are being sent for, once it's `version` (a few seconds at most).
    fn until_running(runner: &Runner<Nodes>, version: u64) -> Arc<Scheduled<Nodes>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(running) = runner.schedule.running.load_full().filter(|r| r.snapshot.version == version) {
                return running;
            }
            assert!(Instant::now() < deadline, "snapshot v{version} should run");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Where a node's state is, in a snapshot the run's been given, once it's running (and has taken its state from the
    /// snapshot before, if it's carried on with it): state carried on is the same, moved from slot to slot.
    fn state_of(runner: &Runner<Nodes>, scheduled: &Scheduled<Nodes>, node: NodeId) -> usize {
        wait_for_passes(runner, runner.stats().completed + 2);
        let step = scheduled.snapshot.plan.steps.iter().find(|step| step.id == node).expect("it runs");
        let slot = &scheduled.slots.states[step.state.expect("it has state")];
        assert!(!slot.pending.load(Ordering::Acquire), "it's taken its state");
        // Safety: only the box's address is read, which only changes as the slot takes its state, which it has.
        let state: &Box<dyn Any + Send> = unsafe { &*slot.state.get() };
        &**state as *const (dyn Any + Send) as *const () as usize
    }

    /// Whether a node's state in a snapshot that's been run was left behind as the next took over: what's left of it
    /// after its new state was made, rather than taken as it was (carried on).
    fn left_behind(scheduled: &Scheduled<Nodes>, node: NodeId) -> bool {
        let step = scheduled.snapshot.plan.steps.iter().find(|step| step.id == node).expect("it ran");
        let slot = &scheduled.slots.states[step.state.expect("it had state")];
        // Safety: nothing uses the slot of a snapshot that's been taken over.
        let state: &Box<dyn Any + Send> = unsafe { &*slot.state.get() };
        !state.is::<()>()
    }

    /// What a node shares with the GUI, in a snapshot the run's been given.
    fn shared_of(scheduled: &Scheduled<Nodes>, node: NodeId) -> Arc<dyn crate::engine::Shared> {
        scheduled.slots.shared.0.get(&node).expect("it runs").clone()
    }

    /// Each node's state, and what it shares with the GUI, are handed on from snapshot to snapshot while its data
    /// streams stay the same, whether the plan's laid out the same (a change of settings) or needs new buffers (another
    /// node added); a snapshot needing new buffers runs once the workers have them, the run carrying on meanwhile.
    #[test]
    fn snapshots_hand_on_each_nodes_state_while_its_streams_are_the_same() {
        let mut graph = Graph::default();
        let board = add_board(&mut graph);
        let filter = add(&mut graph, "Band-pass Filter");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, viewer, 0);
        let mut snapshot = Snapshot::new(0, graph.clone());
        let runner = Runner::start(&snapshot, 2);
        let first = until_running(&runner, 0);
        let filter_state = state_of(&runner, &first, filter);
        assert!(!runner.shared().pools(viewer).is_empty(), "the viewer's display");
        let mut next = |graph: &Graph<Nodes>| {
            snapshot = Snapshot::new(snapshot.version + 1, graph.clone());
            snapshot.clone()
        };

        // A new cut-off is a change of settings: the same buffers, and everything handed on.
        if let Some(Nodes::BandPassFilter(f)) = graph.get_mut(filter).map(|e| &mut e.node) {
            f.high_hz = 3000.0;
        }
        let tuned = next(&graph);
        assert!(runner.update(&tuned));
        let running = until_running(&runner, tuned.version);
        assert_eq!(filter_state, state_of(&runner, &running, filter), "the filter's history");
        assert!(Arc::ptr_eq(&shared_of(&first, viewer), &shared_of(&running, viewer)), "the viewer's display");

        // Another viewer needs new buffers: it runs once they're made, everything that was there handed on.
        let gain = add(&mut graph, "Continuous Gain");
        let second = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, gain, 0);
        link(&mut graph, gain, 0, second, 0);
        let more = next(&graph);
        assert!(!runner.update(&more));
        let running = until_running(&runner, more.version);
        assert_eq!(filter_state, state_of(&runner, &running, filter), "the filter's, still");
        assert!(Arc::ptr_eq(&shared_of(&first, viewer), &shared_of(&running, viewer)), "the viewer's, still");
        assert!(!runner.shared().pools(second).is_empty(), "with the new viewer's");
        let completed = runner.stats().completed;
        let stats = wait_for_passes(&runner, completed + 5);
        assert!(stats.completed >= completed + 5, "passes carry on: {stats:?}");
        assert_eq!(stats.dropped, 0, "none lost: {stats:?}");
    }

    /// A node whose data streams change starts afresh (its state was made for them), and so does what it feeds: here,
    /// the Spike Detector's waveforms getting longer. The rest carries on.
    #[test]
    fn a_node_whose_streams_change_starts_afresh() {
        let mut graph = Graph::default();
        let board = add_board(&mut graph);
        let filter = add(&mut graph, "Band-pass Filter");
        let detector = add(&mut graph, "Spike Detector");
        let spikes = add(&mut graph, "Spike Viewer");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, detector, 0);
        link(&mut graph, detector, 0, spikes, 0);
        let runner = Runner::start(&Snapshot::new(0, graph.clone()), 2);
        let first = until_running(&runner, 0);
        wait_for_passes(&runner, runner.stats().completed + 2);
        if let Some(Nodes::SpikeDetector(d)) = graph.get_mut(detector).map(|e| &mut e.node) {
            d.pre += 4;
        }
        assert!(!runner.update(&Snapshot::new(1, graph)), "longer waveforms need other buffers");
        until_running(&runner, 1);
        wait_for_passes(&runner, runner.stats().completed + 5);
        assert!(!left_behind(&first, filter), "the filter carries on");
        assert!(left_behind(&first, detector), "the detector's afresh");
        assert!(left_behind(&first, spikes), "as is what it feeds");
    }

    /// A recording takes effect as the first pass of the snapshot with it is sent: from when that pass's time starts.
    #[test]
    fn a_recording_takes_effect_with_its_snapshots_first_pass() {
        let graph = Arc::new(Graph::default());
        let started = Instant::now();
        let runner = Runner::start(&Snapshot::bake(0, graph.clone(), Some(started), None), 1);
        until_running(&runner, 0);
        let recording = Arc::new(crate::engine::Recording::default());
        let asked = Instant::now();
        runner.update(&Snapshot::bake(1, graph, Some(started), Some(recording.clone())));
        until_running(&runner, 1);
        let effective = recording.effective().expect("taken effect").at;
        assert!(effective >= asked - Duration::from_secs_f64(PASS_DURATION), "not before it was given the run");
        assert!(effective <= Instant::now());
    }

    /// A snapshot doesn't run until its nodes have prepared their state for it: the one before carries on.
    #[test]
    fn a_snapshot_waits_for_its_nodes_to_prepare() {
        let mut graph = Graph::default();
        let board = add_board(&mut graph);
        let graph = Arc::new(graph);
        let started = Instant::now();
        let runner = Runner::start(&Snapshot::bake(0, graph.clone(), Some(started), None), 1);
        until_running(&runner, 0);
        let snapshot = Arc::new(Snapshot::bake(1, graph, Some(started), None));
        assert!(snapshot.plan.steps.iter().any(|step| step.id == board && step.state.is_some()), "has state");
        // Given the run, but its nodes not yet prepared.
        let before = runner.schedule.latest.load_full();
        let (scheduled, to_prepare) = Scheduled::new(snapshot, Some(&before), &runner.shared);
        let scheduled = Arc::new(scheduled);
        runner.schedule.latest.store(scheduled.clone());
        let passes = runner.clock();
        while runner.clock() < passes + 5.0 * PASS_DURATION {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(runner.schedule.running.load_full().unwrap().snapshot.version, 0, "the one before, meanwhile");
        assert_eq!(runner.inspect().preparing.unwrap().preparing, [board]);
        scheduled.prepare(to_prepare);
        until_running(&runner, 1);
    }

    /// A slot takes its state from the one before at its first pass: as it is (carried on), or handed to the fresh
    /// state its node prepared; through a slot whose snapshot never ran, too.
    #[test]
    fn state_is_carried_on_or_handed_over_at_the_first_pass() {
        // The new state, from the proto-state: what it has, then the old's.
        let change: ChangeFn = &|state, proto_state| {
            let state = state.downcast_mut::<Vec<u64>>().unwrap();
            let proto_state = proto_state.downcast_mut::<Vec<u64>>().unwrap();
            proto_state.append(state);
            std::mem::swap(state, proto_state);
        };
        let first = Arc::new(StateSlot::new(Box::new(vec![1u64])));
        first.begin(0);
        first.with(0, change, |state| state.downcast_mut::<Vec<u64>>().unwrap().push(2));
        // Never run: carrying on from the first.
        let never = Arc::new(StateSlot::empty());
        never.fill(Box::new(()), Some(From { slot: first.clone(), carry: true }));
        // With a proto-state, made the new state with what the one that never ran would have had.
        let last = Arc::new(StateSlot::empty());
        last.fill(Box::new(vec![10u64]), Some(From { slot: never.clone(), carry: false }));
        assert!(last.ready());
        last.begin(1);
        let state = last.with(1, change, |state| state.downcast_ref::<Vec<u64>>().unwrap().clone());
        assert_eq!(state, [10, 1, 2]);
        assert!(Arc::into_inner(first).unwrap().is_empty(), "carried on from");
        let mut never = Arc::into_inner(never).unwrap();
        assert!(!never.is_empty(), "what's left, to let go of in the background");
    }

    /// A slot isn't ready until it's filled, nor while the slot it's to take its state from isn't (and isn't in use).
    #[test]
    fn a_slot_is_ready_once_it_and_what_it_takes_from_are() {
        let before = Arc::new(StateSlot::empty());
        let after = Arc::new(StateSlot::empty());
        assert!(!after.ready());
        after.fill(Box::new(()), Some(From { slot: before.clone(), carry: true }));
        assert!(!after.ready(), "the one before isn't");
        before.fill(Box::new(1u64), None);
        assert!(after.ready());
    }
}
