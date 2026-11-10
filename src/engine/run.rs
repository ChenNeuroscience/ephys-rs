//! Running the graph: a small pool of worker threads, each running whole passes over a snapshot of it.
//!
//! Time is cut into passes of `PASS_DURATION`, numbered from 0. A dispatcher thread releases each pass when
//! its time is up, to whichever ready worker is free. A worker runs the pass through every node in order, as
//! the snapshot's plan says (worked out when the snapshot was taken), on buffers of its own made for that plan
//! before it runs any of its passes (so nothing is allocated per pass), with each node reading its inputs
//! from the outputs of the nodes before it.
//!
//! Passes run at the same time on different workers, which is fine for most nodes but not for those that
//! carry state from one pass to the next (a filter's history, say). (A node whose state is only working memory,
//! carrying nothing over, has its own on each worker instead, made with the worker's buffers: see
//! `PassState::CARRIED_OVER`.) Each node's state lives in a slot, on a
//! cache line of its own, with a turn number: pass n waits (spinning briefly, then yielding) for the turn to
//! reach n, uses the state, then sets it to n + 1 to hand over to the next pass. As long as each pass stays
//! behind the one before, there's little waiting. The first pass gets the node's initial state.
//!
//! If every worker is busy and passes queue up, the dispatcher drops passes (counting them) rather than
//! falling ever further behind.
//!
//! Workers report each pass they run to the GUI on a queue of its own, for the stats, never waiting on it.
//!
//! When the graph changes while running, the new snapshot is swapped in between passes if its plan has the
//! same shape (the same nodes and buffers, as after a change of settings): nodes see their new settings on
//! the next pass and adjust (keeping their state). Otherwise it starts a new epoch, with its own node state
//! and pass queue. Workers make buffers for the new plan as they come free (once they've nothing of the old
//! one queued), and the dispatcher only sends passes for it once a worker is ready, to the workers that are;
//! until then, passes carry on with the old one. So a worker is never handed a pass it isn't ready to run.
//! Snapshots and epochs are swapped in atomically (`ArcSwap`), so a pass takes the latest as it starts, waiting on
//! nothing.
//!
//!
//! What each pass is given is in `pass`, and how it's planned in `plan`.

use std::any::Any;
use std::cell::RefCell;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_swap::{ArcSwap, ArcSwapOption};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use crossbeam_utils::CachePadded;

use crate::console::{self, PassId};
use crate::pass_triggers::PassTriggers;
use crate::triggers::DirectTriggers;

use super::pass::*;
use super::plan::*;
use super::{Buffer, DataStreamsEnum, Edges, Graph, NodeId, NodesEnum, SharedByNode, Snapshot};

type DataStream<N> = <<N as NodesEnum>::Edges as Edges>::DataStream;

/// What the workers do with a plan: make its node state, and their buffers for it, and run its passes.
impl<N: NodesEnum> Plan<N> {
    /// Each stateful node's state for the first pass.
    fn initial_states(&self, graph: &Graph<N>) -> Vec<StateSlot> {
        let state = |id: NodeId, inputs: &[Option<DataStream<N>>]| graph.get(id).map(|e| e.node.initial_state(inputs));
        self.state_inputs
            .iter()
            .map(|(id, inputs)| StateSlot::new(state(*id, inputs).unwrap_or_else(|| Box::new(()))))
            .collect()
    }

    /// The buffers a worker runs passes on, and its own working memory for the nodes that have some.
    fn workspace(&self, graph: &Graph<N>) -> Workspace<N> {
        let memory = |id: NodeId, inputs: &[Option<DataStream<N>>]| graph.get(id).map(|e| e.node.initial_state(inputs));
        Workspace {
            buffers: self.buffers.iter().map(|s| Some(s.buffer(PASS_DURATION))).collect(),
            memory: self
                .memory_inputs
                .iter()
                .map(|(id, inputs)| memory(*id, inputs).unwrap_or_else(|| Box::new(())))
                .collect(),
        }
    }

    /// Runs a pass through every step of `snapshot`'s graph, on `workspace`.
    fn run(&self, snapshot: &Snapshot<N>, pass: PassId, workspace: &mut Workspace<N>, run: &RunSlots) {
        let Workspace { buffers: workspace, memory } = workspace;
        let sequence = pass.sequence;
        let start_recording = snapshot.recording.as_ref().map(|r| r.start_effective());
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
            let (work, wait) = {
                let inputs: Vec<Option<&Buffer<N::Edges>>> =
                    step.inputs.iter().map(|b| b.and_then(|b| workspace[b].as_ref())).collect();
                let mut pass_data = Pass {
                    sequence,
                    duration: PASS_DURATION,
                    node: step.id,
                    inputs: &inputs,
                    taken: &mut taken,
                    outputs: &mut outputs,
                    controls,
                    start_acquiring: run.started,
                    start_recording,
                    prepared: snapshot.recording.as_deref(),
                    shared: run.shared.any(step.id).unwrap_or(&()),
                    triggers: &run.triggers,
                    direct: &run.direct,
                };
                let start = Instant::now();
                // When the node's state is handed over, after waiting for it, if it has any.
                let mut began = start;
                console::in_pass(pass, || match (step.state, step.memory) {
                    (Some(slot), _) => run.states[slot].with(sequence, |state| {
                        began = Instant::now();
                        node.process(&mut pass_data, Some(state))
                    }),
                    (None, Some(m)) => node.process(&mut pass_data, Some(memory[m].as_mut())),
                    (None, None) => node.process(&mut pass_data, None),
                });
                (began.elapsed(), began - start)
            };
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
    memory: Vec<Box<dyn Any + Send>>,
}

impl<N: NodesEnum> Workspace<N> {
    /// The memory of its data buffers.
    fn bytes(&self) -> usize {
        self.buffers.iter().flatten().map(<<N::Edges as Edges>::DataStream as DataStreamsEnum>::bytes).sum()
    }
}

/// What the workers share during a run of one plan, besides the graph: the nodes' state, what they share with the GUI,
/// the run's triggers, and how long each step's taking.
struct RunSlots {
    states: Vec<StateSlot>,
    shared: Arc<SharedByNode>,
    triggers: Arc<PassTriggers>,
    direct: Arc<DirectTriggers>,
    times: Vec<StepTimes>,
    started: Instant,
}

impl RunSlots {
    fn new<N: NodesEnum>(plan: &Plan<N>, graph: &Graph<N>, run: &RunCommon) -> Self {
        let times = plan.steps.iter().map(|_| StepTimes::default()).collect();
        let shared = plan
            .steps
            .iter()
            .zip(&plan.step_inputs)
            .filter_map(|(step, inputs)| Some((step.id, graph.get(step.id)?.node.shared(inputs))));
        RunSlots {
            states: plan.initial_states(graph),
            shared: Arc::new(SharedByNode(shared.collect())),
            triggers: run.triggers.clone(),
            direct: run.direct.clone(),
            times,
            started: run.started.0,
        }
    }
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

/// A node's state, handed from each pass to the next in turn (see the module docs).
struct StateSlot {
    /// The pass whose turn it is to use the state.
    turn: CachePadded<AtomicU64>,
    state: UnsafeCell<Box<dyn Any + Send>>,
}

// Safety: the state is only touched by the pass whose turn it is, which `with` waits for.
unsafe impl Sync for StateSlot {}

impl StateSlot {
    fn new(state: Box<dyn Any + Send>) -> Self {
        StateSlot { turn: CachePadded::new(AtomicU64::new(0)), state: UnsafeCell::new(state) }
    }

    /// Waits for pass `sequence`'s turn, runs `f` on the state, then hands it to the next pass.
    fn with<R>(&self, sequence: u64, f: impl FnOnce(&mut (dyn Any + Send)) -> R) -> R {
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
        // Safety: it's this pass's turn, and no other pass's until `_hand_over` is dropped.
        f(unsafe { &mut **self.state.get() })
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

/// What the runner, its workers and its dispatcher share, through every epoch: counts and recent passes for the
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
    /// The pass triggers the GUI sends, and where the passes send direct triggers: the app's.
    triggers: Arc<PassTriggers>,
    direct: Arc<DirectTriggers>,
    /// What each worker is up to.
    workers: Vec<WorkerSlot>,
    /// The next pass the dispatcher will send.
    next_pass: AtomicU64,
    /// Epochs started, which numbers them (from 1).
    epochs: AtomicU64,
    /// The newest version a pass has run against.
    newest_used: AtomicU64,
    /// When it started running, by the monotonic clock (see `Pass::start_acquiring`).
    started: Started,
}

/// When a run started: when it's made.
struct Started(Instant);

impl Default for Started {
    fn default() -> Self {
        Started(Instant::now())
    }
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

/// What a worker is up to, for the dev tools.
#[derive(Default)]
struct WorkerSlot {
    /// The number of the epoch it has buffers for (0 until it has some).
    epoch: AtomicU64,
    /// The pass it's running, or `IDLE`.
    pass: AtomicU64,
    /// The memory of its buffers.
    memory: AtomicUsize,
    passes: AtomicU64,
}

/// What a worker is up to.
#[derive(Clone, Debug)]
pub struct WorkerInfo {
    /// The number of the epoch it has buffers for, once it has some.
    pub epoch: Option<u64>,
    /// The pass it's running, if any.
    pub pass: Option<u64>,
    /// The memory of its buffers, in bytes.
    pub memory: usize,
    /// Passes it has run.
    pub passes: u64,
}

/// A run of one plan (see `Epoch`).
pub struct EpochInfo<N: NodesEnum> {
    /// Epochs are numbered from 1, in the order they start.
    pub number: u64,
    /// The version of the latest snapshot its passes run against.
    pub version: u64,
    pub plan: Arc<Plan<N>>,
    /// Workers with buffers for it.
    pub ready: usize,
    /// Passes waiting for a worker.
    pub queued: usize,
    /// How long each of the plan's steps is taking.
    pub step_times: Vec<StepTime>,
}

/// Everything about a run, as of now, for the dev tools.
pub struct RunInfo<N: NodesEnum> {
    /// The epoch passes are being sent for, once a worker's ready for one.
    pub running: Option<EpochInfo<N>>,
    /// A newer epoch workers are getting ready for, if there is one.
    pub preparing: Option<EpochInfo<N>>,
    pub workers: Vec<WorkerInfo>,
    /// The next pass the dispatcher will send.
    pub next_pass: u64,
    /// How many passes can wait for a worker before they're dropped.
    pub queue_capacity: usize,
    pub stats: Stats,
}

/// A pass on the queue, for a worker to run.
struct Job {
    sequence: u64,
    /// When the pass's data was all due: the end of the time it covers.
    due: Instant,
}

/// A run of one plan: the snapshot its passes run against (swapped for newer snapshots of the same shape), the
/// plan, what the workers share running it, and the queue its passes are dispatched on. A change to the graph
/// that changes the plan starts a new epoch, which the dispatcher moves on to once a worker is ready for it.
struct Epoch<N: NodesEnum> {
    number: u64,
    /// Swapped for a newer one as settings change: each pass runs against the latest when it starts.
    snapshot: ArcSwap<Snapshot<N>>,
    plan: Arc<Plan<N>>,
    slots: RunSlots,
    /// How many workers have a workspace for this epoch, ready to run its passes.
    ready: AtomicUsize,
    /// Taken by the dispatcher when it starts sending this epoch's passes; dropping it lets the workers on this
    /// epoch know there will be no more.
    sender: Mutex<Option<Sender<Job>>>,
    receiver: Receiver<Job>,
    shared: Arc<RunCommon>,
}

impl<N: NodesEnum> Epoch<N> {
    fn new(snapshot: &Snapshot<N>, workers: usize, shared: Arc<RunCommon>) -> Self {
        let plan = snapshot.plan.clone();
        let slots = RunSlots::new(&plan, &snapshot.graph, &shared);
        // A few passes may queue, beyond which they're dropped.
        let (sender, receiver) = crossbeam_channel::bounded(queue_capacity(workers));
        let number = shared.epochs.fetch_add(1, Ordering::Relaxed) + 1;
        Epoch {
            number,
            snapshot: ArcSwap::from_pointee(snapshot.clone()),
            plan,
            slots,
            ready: AtomicUsize::new(0),
            sender: Mutex::new(Some(sender)),
            receiver,
            shared,
        }
    }

    /// Readies the node states for this epoch's first pass, `first`.
    fn begin(&self, first: u64) {
        for slot in &self.slots.states {
            slot.turn.store(first, Ordering::Release);
        }
    }
}

impl<N: NodesEnum> Drop for Epoch<N> {
    /// Passes still queued when no worker is left to run them were never run. The nodes' state is let go of in
    /// the background (see `let_go_of`).
    fn drop(&mut self) {
        self.shared.dropped.fetch_add(self.receiver.len() as u64, Ordering::Relaxed);
        let_go_of(std::mem::take(&mut self.slots.states));
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

/// The epochs a run has: the newest (which workers prepare for), and the one being dispatched.
struct Epochs<N: NodesEnum> {
    wanted: ArcSwap<Epoch<N>>,
    running: ArcSwapOption<Epoch<N>>,
}

/// The graph running: a dispatcher and a pool of workers, until dropped.
pub struct Runner<N: NodesEnum> {
    epochs: Arc<Epochs<N>>,
    shared: Arc<RunCommon>,
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
        let shared = Arc::new(RunCommon {
            workers: (0..workers).map(|_| WorkerSlot::default()).collect(),
            direct,
            ..Default::default()
        });
        let epoch = Arc::new(Epoch::new(snapshot, workers, shared.clone()));
        log::info!("Running on {workers} workers, {} ms passes", PASS_DURATION * 1000.0);
        log::debug!("Epoch {} for version {}: {}", epoch.number, snapshot.version, describe(&epoch.plan));
        let epochs = Arc::new(Epochs { wanted: ArcSwap::new(epoch), running: ArcSwapOption::empty() });

        let mut threads = Vec::new();
        for index in 0..workers {
            let (epochs, shared) = (epochs.clone(), shared.clone());
            let worker = move || work(&epochs, &shared, index);
            threads
                .push(std::thread::Builder::new().name(format!("pass worker {index}")).spawn(worker).expect("worker"));
        }
        let (dispatcher_epochs, dispatcher_shared) = (epochs.clone(), shared.clone());
        let dispatcher = move || dispatch(&dispatcher_epochs, &dispatcher_shared);
        threads.push(std::thread::Builder::new().name("pass dispatcher".into()).spawn(dispatcher).expect("dispatcher"));

        let (version, received) = (AtomicU64::new(snapshot.version), RefCell::default());
        Runner { epochs, shared, threads, workers, version, received }
    }

    /// The graph of the latest snapshot given to the run.
    #[cfg(test)]
    pub fn graph(&self) -> Arc<Graph<N>> {
        self.epochs.wanted.load().snapshot.load().graph.clone()
    }

    /// Where the passes send direct triggers.
    pub fn direct_triggers(&self) -> &DirectTriggers {
        &self.shared.direct
    }

    /// The run's pass triggers, for the GUI to send.
    pub fn pass_triggers(&self) -> &PassTriggers {
        &self.shared.triggers
    }

    /// What the nodes running share with the GUI: the epoch being run's, or, until one is, the one being got ready
    /// for.
    pub fn shared(&self) -> Arc<SharedByNode> {
        let epoch = self.epochs.running.load_full().unwrap_or_else(|| self.epochs.wanted.load_full());
        epoch.slots.shared.clone()
    }

    /// The acquisition clock: seconds of passes sent so far.
    pub fn clock(&self) -> f64 {
        self.shared.next_pass.load(Ordering::Relaxed) as f64 * PASS_DURATION
    }

    /// Moves the run on to a newer snapshot. If it runs the same way (as after a change of settings), passes
    /// carry on with it from the next one, keeping the nodes' state, and this returns true. Otherwise it starts
    /// a new epoch: the workers make buffers for its plan as they come free, and its passes start (with fresh
    /// state) once one is ready; until then, passes carry on with the old one.
    pub fn update(&self, snapshot: &Snapshot<N>) -> bool {
        if self.version.swap(snapshot.version, Ordering::Relaxed) == snapshot.version {
            return true;
        }
        // Only the GUI updates it, so nothing changes it in between.
        let wanted = self.epochs.wanted.load();
        if Arc::ptr_eq(&wanted.plan, &snapshot.plan) || wanted.plan.same_shape(&snapshot.plan) {
            wanted.snapshot.store(Arc::new(snapshot.clone()));
            log::trace!("Version {} carries on epoch {}", snapshot.version, wanted.number);
            true
        } else {
            let epoch = Arc::new(Epoch::new(snapshot, self.workers, self.shared.clone()));
            log::info!(
                "Version {} changes the plan: epoch {}, {}",
                snapshot.version,
                epoch.number,
                describe(&epoch.plan)
            );
            self.epochs.wanted.store(epoch);
            false
        }
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

    /// The plan passes are being dispatched for, once they are.
    #[cfg(test)]
    fn running_plan(&self) -> Option<Arc<Plan<N>>> {
        self.epochs.running.load().as_ref().map(|e| e.plan.clone())
    }

    /// Everything about the run as of now: its epochs, workers and dispatcher, and the stats.
    pub fn inspect(&self) -> RunInfo<N> {
        let info = |epoch: &Epoch<N>| EpochInfo {
            number: epoch.number,
            version: epoch.snapshot.load().version,
            plan: epoch.plan.clone(),
            ready: epoch.ready.load(Ordering::Relaxed),
            queued: epoch.receiver.len(),
            step_times: epoch.slots.times.iter().map(StepTimes::get).collect(),
        };
        let running = self.epochs.running.load().as_deref().map(info);
        let wanted = self.epochs.wanted.load_full();
        let preparing = (running.as_ref().map(|r| r.number) != Some(wanted.number)).then(|| info(&wanted));
        let workers = self
            .shared
            .workers
            .iter()
            .map(|w| {
                let (epoch, pass) = (w.epoch.load(Ordering::Relaxed), w.pass.load(Ordering::Relaxed));
                WorkerInfo {
                    epoch: (epoch > 0).then_some(epoch),
                    pass: (pass != IDLE).then_some(pass),
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
        let (completed, dropped) =
            (self.shared.completed.load(Ordering::Relaxed), self.shared.dropped.load(Ordering::Relaxed));
        log::info!("Stopped after {completed} passes ({dropped} dropped)");
    }
}

/// How long passes' times are kept (see `Runner::pass_times`).
const PASS_TIMES: Duration = Duration::from_secs(10);

/// How long an idle worker waits for a pass before checking for a newer epoch to get ready for.
const IDLE_CHECK: Duration = Duration::from_millis(2);

/// A worker: runs passes of its epoch, and once there's a newer one and nothing of its own queued, makes
/// buffers for that and moves on to it. Ends when the run stops, once every pass dispatched to it is done.
fn work<N: NodesEnum>(epochs: &Epochs<N>, shared: &RunCommon, index: usize) {
    let slot = &shared.workers[index];
    slot.pass.store(IDLE, Ordering::Relaxed);
    let mut mine: Option<(Arc<Epoch<N>>, Workspace<N>)> = None;
    loop {
        let wanted = epochs.wanted.load();
        let behind = match &mine {
            None => true,
            Some((epoch, _)) => !Arc::ptr_eq(epoch, &wanted) && epoch.receiver.is_empty(),
        };
        if behind {
            // Free the old buffers before making the new.
            if let Some((_, workspace)) = mine.take() {
                shared.memory.fetch_sub(workspace.bytes(), Ordering::Relaxed);
            }
            let workspace = wanted.plan.workspace(&wanted.snapshot.load().graph);
            let bytes = workspace.bytes();
            shared.memory.fetch_add(bytes, Ordering::Relaxed);
            slot.memory.store(bytes, Ordering::Relaxed);
            slot.epoch.store(wanted.number, Ordering::Relaxed);
            wanted.ready.fetch_add(1, Ordering::Release);
            log::debug!(
                "Worker {index} ready for epoch {}, with {:.2} MB of buffers",
                wanted.number,
                bytes as f64 / 1e6
            );
            mine = Some((arc_swap::Guard::into_inner(wanted), workspace));
        }
        let Some((epoch, workspace)) = &mut mine else { unreachable!() };
        match epoch.receiver.recv_timeout(IDLE_CHECK) {
            Ok(job) => {
                shared.busy.fetch_add(1, Ordering::Relaxed);
                slot.pass.store(job.sequence, Ordering::Relaxed);
                let start = Instant::now();
                let snapshot = epoch.snapshot.load_full();
                if shared.newest_used.fetch_max(snapshot.version, Ordering::Relaxed) < snapshot.version {
                    let first = FirstUse { version: snapshot.version, sequence: job.sequence, at: start };
                    let _ = shared.reports.0.try_send(Report::FirstUse(first));
                }
                let pass = PassId { epoch: epoch.number, sequence: job.sequence, node: None, node_type: None };
                epoch.plan.run(&snapshot, pass, workspace, &epoch.slots);
                let finished = Instant::now();
                slot.pass.store(IDLE, Ordering::Relaxed);
                slot.passes.fetch_add(1, Ordering::Relaxed);
                shared.busy.fetch_sub(1, Ordering::Relaxed);
                shared.completed.fetch_add(1, Ordering::Relaxed);
                let (time, lag) = (finished - start, finished.saturating_duration_since(job.due));
                let record = Record { sequence: job.sequence, finished, time, lag };
                let _ = shared.reports.0.try_send(Report::Pass(record));
            }
            Err(_) if shared.stop.load(Ordering::Relaxed) => break,
            Err(RecvTimeoutError::Timeout) => {}
            // The dispatcher has moved on from this epoch: get ready for the next.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(IDLE_CHECK),
        }
    }
    if let Some((_, workspace)) = mine {
        shared.memory.fetch_sub(workspace.bytes(), Ordering::Relaxed);
    }
}

/// The dispatcher: releases each pass when its time is up, on the queue of the newest epoch a worker is ready
/// for. The clock starts once the first worker is ready, so making buffers doesn't count against the run.
fn dispatch<N: NodesEnum>(epochs: &Epochs<N>, shared: &RunCommon) {
    let duration = Duration::from_secs_f64(PASS_DURATION);
    let mut current: Option<(Arc<Epoch<N>>, Sender<Job>)> = None;
    let (mut sequence, mut due) = (0, Instant::now());
    // Passes dropped in a row, so a run of them is reported once.
    let mut dropping = 0u64;
    while !shared.stop.load(Ordering::Relaxed) {
        let wanted = epochs.wanted.load_full();
        let dispatching = current.as_ref().is_some_and(|(epoch, _)| Arc::ptr_eq(epoch, &wanted));
        // Move on to the wanted epoch once a worker's ready for it.
        let sender = if !dispatching && wanted.ready.load(Ordering::Acquire) > 0 {
            wanted.sender.lock().unwrap().take()
        } else {
            None
        };
        if let Some(sender) = sender {
            if current.is_none() {
                due = Instant::now() + duration;
            }
            wanted.begin(sequence);
            log::debug!("Epoch {} running from pass {sequence}", wanted.number);
            epochs.running.store(Some(wanted.clone()));
            current = Some((wanted, sender)); // dropping the old epoch's sender ends its passes
        }
        let Some((_, sender)) = &current else {
            std::thread::sleep(IDLE_CHECK);
            continue;
        };
        let now = Instant::now();
        if now < due {
            std::thread::sleep((due - now).min(Duration::from_millis(5)));
            continue;
        }
        match sender.try_send(Job { sequence, due }) {
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
    use crate::engine::NodeTypeEnum;

    use crate::Buffer;
    use crate::Nodes;

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let node_type = NodeType::named(name).unwrap();
        graph.add(Nodes::create(node_type), [0.0, 0.0])
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
                    slot.with(sequence, |state| state.downcast_mut::<Vec<u64>>().unwrap().push(sequence))
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let order = slot.with(8, |state| state.downcast_ref::<Vec<u64>>().unwrap().clone());
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
        let snapshot = Snapshot::new(0, graph, None);
        let plan = &snapshot.plan;
        let slots = RunSlots::new(plan, &snapshot.graph, &RunCommon::default());
        let mut workspace = plan.workspace(&snapshot.graph);
        let filtered = plan.steps.iter().find(|s| s.id == filter).unwrap().outputs[0];
        let copied = plan.steps.iter().find(|s| s.id == writer).unwrap().inputs[0].unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        for sequence in 0.. {
            plan.run(&snapshot, PassId { epoch: 1, sequence, node: None, node_type: None }, &mut workspace, &slots);
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
        let board = add(&mut graph, "Acquisition Board");
        let reference = add(&mut graph, "Reference");
        let filter = add(&mut graph, "Band-pass Filter");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, reference, 0);
        link(&mut graph, reference, 0, filter, 0);
        link(&mut graph, filter, 0, viewer, 0);
        let plan = Plan::new(&graph);
        let step = |id| plan.steps.iter().find(|s| s.id == id).unwrap();
        assert!(step(reference).state.is_none() && step(reference).memory.is_some(), "no sync point");
        assert!(step(filter).state.is_some(), "the filter's history is carried over");
        assert_eq!(plan.workspace(&graph).memory.len(), 1);
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
        let snapshot = Snapshot::new(0, graph, None);
        let plan = &snapshot.plan;
        let slots = RunSlots::new(plan, &snapshot.graph, &RunCommon::default());
        let mut workspace = plan.workspace(&snapshot.graph);
        // A second of samples, as they come from the board in real time.
        let (mut spikes, mut samples) = (0, 0);
        let filter_output = plan.steps.iter().find(|s| s.id == filter).unwrap().outputs[0];
        let detector_output = plan.steps.iter().find(|s| s.id == detector).unwrap().outputs[0];
        let deadline = Instant::now() + Duration::from_secs(5);
        for sequence in 0.. {
            plan.run(&snapshot, PassId { epoch: 1, sequence, node: None, node_type: None }, &mut workspace, &slots);
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
            let snapshot = Snapshot::new(0, crate::example_graph(), None);
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

    /// What nodes share with the GUI is made for each epoch (sized for its data streams), and kept through changes
    /// of settings.
    #[test]
    fn settings_changes_carry_on_and_others_start_a_new_epoch() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let filter = add(&mut graph, "Band-pass Filter");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, viewer, 0);
        let mut snapshot = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start(&snapshot, 2);
        let shared = runner.shared();
        assert!(!shared.pools(viewer).is_empty(), "the viewer's display");
        let mut next = |graph: &Graph<Nodes>| {
            snapshot = Snapshot::new(snapshot.version + 1, graph.clone(), Some(&snapshot));
            snapshot.clone()
        };

        // A new cut-off is a change of settings: the run carries on with it.
        if let Some(Nodes::BandPassFilter(f)) = graph.get_mut(filter).map(|e| &mut e.node) {
            f.high_hz = 3000.0;
        }
        assert!(runner.update(&next(&graph)));

        // A node that isn't connected doesn't run, so doesn't change anything; connected (with something needing
        // its output), it does.
        let gain = add(&mut graph, "Continuous Gain");
        assert!(runner.update(&next(&graph)));
        link(&mut graph, board, 0, gain, 0);
        assert!(runner.update(&next(&graph)), "nothing needs its output");
        assert!(Arc::ptr_eq(&runner.shared(), &shared), "the same epoch's");
        let second = add(&mut graph, "LFP Viewer");
        link(&mut graph, gain, 0, second, 0);
        let latest = next(&graph);
        assert!(!runner.update(&latest));

        // Passes move on to the new plan once a worker has buffers for it, without a pass going undone.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !runner.running_plan().is_some_and(|p| Arc::ptr_eq(&p, &latest.plan)) {
            assert!(Instant::now() < deadline, "passes should move on to the new plan");
            std::thread::sleep(Duration::from_millis(5));
        }
        let now = runner.shared();
        assert!(!Arc::ptr_eq(&now, &shared), "the new epoch's");
        assert!(!now.pools(second).is_empty() && shared.pools(second).is_empty(), "with the new viewer's");
        let completed = runner.stats().completed;
        let stats = wait_for_passes(&runner, completed + 1);
        assert!(stats.completed > completed, "{stats:?}");
        assert_eq!(stats.dropped, 0, "{stats:?}");
    }
}
