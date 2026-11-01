//! Running the graph: a small pool of worker threads, each running whole passes over a snapshot of it.
//!
//! Time is cut into passes of `PASS_DURATION`, numbered from 0. A dispatcher thread releases each pass when
//! its time is up, to whichever ready worker is free. A worker runs the pass through every node in order, as
//! the snapshot's plan says (worked out when the snapshot was taken), on buffers of its own made for that plan
//! before it runs any of its passes (so nothing is allocated per pass), with each node reading its inputs
//! from the outputs of the nodes before it.
//!
//! Passes run at the same time on different workers, which is fine for most nodes but not for those that
//! carry state from one pass to the next (a filter's history, say). Each node's state lives in a slot, on a
//! cache line of its own, with a turn number: pass n waits (spinning briefly, then yielding) for the turn to
//! reach n, uses the state, then sets it to n + 1 to hand over to the next pass. As long as each pass stays
//! behind the one before, there's little waiting. The first pass gets the node's initial state.
//!
//! If every worker is busy and passes queue up, the dispatcher drops passes (counting them) rather than
//! falling ever further behind.
//!
//! When the graph changes while running, the new snapshot is swapped in between passes if its plan has the
//! same shape (the same nodes and buffers, as after a change of settings): nodes see their new settings on
//! the next pass and adjust (keeping their state). Otherwise it starts a new epoch, with its own node state
//! and pass queue. Workers make buffers for the new plan as they come free (once they've nothing of the old
//! one queued), and the dispatcher only sends passes for it once a worker is ready, to the workers that are;
//! until then, passes carry on with the old one. So a worker is never handed a pass it isn't ready to run.
//!
//! Nodes that show live data in their windows (a video frame, say) fill a display buffer of their own on
//! each worker, which is then swapped with the latest one shown, if it's newer. The swap only `try_lock`s,
//! so a worker never waits on the GUI: if the window is reading the latest just then, that pass's display
//! is skipped. The buffers are reused, so nothing is allocated.

use std::any::Any;
use std::cell::UnsafeCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use crossbeam_utils::CachePadded;

use super::{Buffer, DataStreamSet, Edges, Graph, NodeId, NodeSet, Port, Role, Snapshot};

type DataStream<N> = <<N as NodeSet>::Edges as Edges>::DataStream;

/// The length of time each pass covers, in seconds.
pub const PASS_DURATION: f64 = 0.02;

/// What a node works on for one pass.
pub struct Pass<'a, E: Edges> {
    /// Passes are numbered from 0, each covering the next `duration` seconds. (No node needs them just now.)
    #[allow(dead_code)]
    pub sequence: u64,
    #[allow(dead_code)]
    pub duration: f64,
    /// For each input, this pass's data on it, if it's connected.
    pub inputs: &'a [Option<&'a Buffer<E>>],
    /// For each output, a buffer to fill.
    pub outputs: &'a mut [Buffer<E>],
    display: Option<&'a mut (dyn Any + Send)>,
    shown: bool,
}

impl<E: Edges> Pass<'_, E> {
    /// The node's display buffer, if it has one (see `Node::display`) of type `T`. What's left in it is shown
    /// in the node's window after the pass; it may hold an earlier pass's display, to overwrite.
    pub fn display<T: 'static>(&mut self) -> Option<&mut T> {
        let display = self.display.as_deref_mut()?.downcast_mut::<T>()?;
        self.shown = true;
        Some(display)
    }
}

/// A node to run in each pass, and which buffers (in the workspace's pool) it works with.
#[derive(PartialEq)]
struct Step {
    id: NodeId,
    /// For each input, the buffer feeding it. `None` if it's unconnected, or an output is made from it in place.
    inputs: Vec<Option<usize>>,
    /// For each output, its buffer.
    outputs: Vec<usize>,
    /// The node's state, if it has any: an index into the run's state slots.
    state: Option<usize>,
    /// Whether the node shows live data in its window: an index into the run's display slots.
    display: Option<usize>,
}

/// Which nodes run, in what order, on which buffers: worked out when a snapshot is taken (see `Snapshot`), so
/// it's ready before anything runs.
///
/// The nodes that run are those whose outputs are all known (so can have buffers). Each output gets a buffer
/// of its own, except where a node can make an output in place from an input (see `Node::in_place`): then
/// the output takes over the input's buffer, as long as every other node reading that data can run before
/// it. Within a pass everything runs on one thread, in an order of our choosing, so the plan orders the
/// nodes to make that so: the other readers first (they only read, e.g. viewers copying what they show),
/// then the one editing in place. Only if another reader depends on the editor's output can't it be.
pub struct Plan<N: NodeSet> {
    steps: Vec<Step>,
    /// The data stream each buffer in the pool holds.
    buffers: Vec<DataStream<N>>,
    /// For each node with state, the data streams on its inputs (for its initial state).
    state_inputs: Vec<(NodeId, Vec<Option<DataStream<N>>>)>,
    /// The nodes that show live data in their windows.
    displays: Vec<NodeId>,
}

impl<N: NodeSet> Plan<N> {
    pub fn new(graph: &Graph<N>) -> Self {
        let resolved = graph.resolve();
        let runs: Vec<NodeId> = graph
            .topological_order()
            .into_iter()
            .filter(|id| resolved.get(id).is_some_and(|s| s.outputs.iter().all(|o| o.stream.is_some())))
            .collect();
        let data_edges: Vec<_> = graph
            .edges()
            .iter()
            .filter(|e| {
                e.dimension == super::Dimension::Data && runs.contains(&e.from.node) && runs.contains(&e.to.node)
            })
            .collect();
        // Which nodes must run before which: those feeding them, and those reading data another edits in place.
        let mut before: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in &data_edges {
            before.entry(edge.from.node).or_default().push(edge.to.node);
        }

        // Pick the outputs made in place: (node, output) -> its input, and the data each takes over.
        let mut in_place: HashMap<(NodeId, usize), usize> = HashMap::new();
        let mut claimed: Vec<(NodeId, usize)> = Vec::new();
        for &id in &runs {
            let (Some(entry), Some(state)) = (graph.get(id), resolved.get(&id)) else { continue };
            for output in 0..state.outputs.len() {
                let Some(input) = entry.node.in_place(output) else { continue };
                let Some(edge) = data_edges.iter().find(|e| e.to.node == id && e.to.port == input) else { continue };
                let source = (edge.from.node, edge.from.port);
                if claimed.contains(&source) {
                    continue; // another node is already editing this data in place
                }
                let others: Vec<NodeId> = data_edges
                    .iter()
                    .filter(|e| (e.from.node, e.from.port) == source && e.to.node != id)
                    .map(|e| e.to.node)
                    .collect();
                // The other readers must run first, which they can't if they depend on this node.
                if others.iter().any(|&other| reaches(&before, id, other)) {
                    continue;
                }
                for other in others {
                    before.entry(other).or_default().push(id);
                }
                claimed.push(source);
                in_place.insert((id, output), input);
            }
        }

        let (mut steps, mut buffers, mut state_inputs, mut displays) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // The buffer each output (of the nodes so far) is in.
        let mut buffer_of: HashMap<(NodeId, usize), usize> = HashMap::new();
        for id in order(&runs, &before) {
            let (Some(entry), Some(state)) = (graph.get(id), resolved.get(&id)) else { continue };
            let mut inputs: Vec<Option<usize>> = (0..state.inputs.len())
                .map(|index| {
                    let edge = graph.edge_into(Port { node: id, role: Role::Input, index })?;
                    buffer_of.get(&(edge.from.node, edge.from.port)).copied()
                })
                .collect();
            let mut outputs = Vec::new();
            for (index, output) in state.outputs.iter().enumerate() {
                let buffer = match in_place.get(&(id, index)).and_then(|&input| Some((input, inputs[input]?))) {
                    Some((input, buffer)) => {
                        inputs[input] = None; // its data is in the output
                        buffer
                    }
                    None => {
                        buffers.push(output.stream.expect("only nodes with known outputs run"));
                        buffers.len() - 1
                    }
                };
                outputs.push(buffer);
                buffer_of.insert((id, index), buffer);
            }
            let node_state = entry.node.has_state().then(|| {
                state_inputs.push((id, state.input_streams.clone()));
                state_inputs.len() - 1
            });
            let display = entry.node.display().is_some().then(|| {
                displays.push(id);
                displays.len() - 1
            });
            steps.push(Step { id, inputs, outputs, state: node_state, display });
        }
        Plan { steps, buffers, state_inputs, displays }
    }

    /// Whether runs of the two plans would work the same way (the same steps on the same buffers), so a run of
    /// one can carry on with the other's graph.
    fn same_shape(&self, other: &Self) -> bool {
        self.steps == other.steps && self.buffers == other.buffers
    }

    /// Each stateful node's state for the first pass.
    fn initial_states(&self, graph: &Graph<N>) -> Vec<StateSlot> {
        let state = |id: NodeId, inputs: &[Option<DataStream<N>>]| graph.get(id).map(|e| e.node.initial_state(inputs));
        self.state_inputs
            .iter()
            .map(|(id, inputs)| StateSlot::new(state(*id, inputs).unwrap_or_else(|| Box::new(()))))
            .collect()
    }

    /// The buffers a worker runs passes on.
    fn workspace(&self, graph: &Graph<N>) -> Workspace<N> {
        Workspace {
            buffers: self.buffers.iter().map(|s| Some(s.buffer(PASS_DURATION))).collect(),
            displays: self.empty_displays(graph).into_iter().map(Some).collect(),
        }
    }

    /// An empty display buffer for each node that shows live data.
    fn empty_displays(&self, graph: &Graph<N>) -> Vec<Box<dyn Any + Send>> {
        let empty = |id: &NodeId| graph.get(*id).and_then(|e| e.node.display());
        self.displays.iter().map(|id| empty(id).unwrap_or_else(|| Box::new(()))).collect()
    }

    /// Runs pass `sequence` through every step of `graph`, on `workspace`.
    fn run(&self, graph: &Graph<N>, sequence: u64, workspace: &mut Workspace<N>, run: &RunSlots) {
        let Workspace { buffers: workspace, displays } = workspace;
        for step in &self.steps {
            let Some(node) = graph.get(step.id).map(|e| &e.node) else { continue };
            // Take the outputs out of the pool, so the rest of it can be read as inputs.
            let mut outputs: Vec<Buffer<N::Edges>> =
                step.outputs.iter().map(|&b| workspace[b].take().expect("buffer in pool")).collect();
            {
                let inputs: Vec<Option<&Buffer<N::Edges>>> =
                    step.inputs.iter().map(|b| b.and_then(|b| workspace[b].as_ref())).collect();
                let display = step.display.and_then(|d| displays[d].as_deref_mut());
                let mut pass = Pass {
                    sequence,
                    duration: PASS_DURATION,
                    inputs: &inputs,
                    outputs: &mut outputs,
                    display,
                    shown: false,
                };
                match step.state {
                    Some(slot) => run.states[slot].with(sequence, |state| node.process(&mut pass, Some(state))),
                    None => node.process(&mut pass, None),
                }
                if let (true, Some(d)) = (pass.shown, step.display)
                    && let Some(display) = displays[d].as_mut()
                {
                    run.displays[d].offer(sequence, display);
                }
            }
            for (&b, buffer) in step.outputs.iter().zip(outputs) {
                workspace[b] = Some(buffer);
            }
        }
    }
}

/// Whether `to` must run after `from` (or is `from`), given which nodes must run before which.
fn reaches(before: &HashMap<NodeId, Vec<NodeId>>, from: NodeId, to: NodeId) -> bool {
    let mut stack = vec![from];
    let mut seen = Vec::new();
    while let Some(id) = stack.pop() {
        if id == to {
            return true;
        }
        if !seen.contains(&id) {
            seen.push(id);
            stack.extend(before.get(&id).into_iter().flatten());
        }
    }
    false
}

/// `nodes` ordered so each comes after those it must run after, otherwise keeping their order.
fn order(nodes: &[NodeId], before: &HashMap<NodeId, Vec<NodeId>>) -> Vec<NodeId> {
    let mut waiting: HashMap<NodeId, usize> = nodes.iter().map(|id| (*id, 0)).collect();
    for later in before.values().flatten() {
        if let Some(count) = waiting.get_mut(later) {
            *count += 1;
        }
    }
    let mut ordered = Vec::with_capacity(nodes.len());
    while ordered.len() < nodes.len() {
        // The first node (in the original order) with nothing left to wait for.
        let Some(&next) = nodes.iter().find(|id| waiting.get(id) == Some(&0)) else { break };
        waiting.remove(&next);
        ordered.push(next);
        for later in before.get(&next).into_iter().flatten() {
            if let Some(count) = waiting.get_mut(later) {
                *count -= 1;
            }
        }
    }
    ordered
}

/// A worker's buffers: data buffers for every step, and display buffers for the steps that show live data.
/// Made once per worker per plan, before the worker runs any of the plan's passes.
struct Workspace<N: NodeSet> {
    buffers: Vec<Option<Buffer<N::Edges>>>,
    displays: Vec<Option<Box<dyn Any + Send>>>,
}

impl<N: NodeSet> Workspace<N> {
    /// The memory of its data buffers.
    fn bytes(&self) -> usize {
        self.buffers.iter().flatten().map(<<N::Edges as Edges>::DataStream as DataStreamSet>::bytes).sum()
    }
}

/// What the workers share during a run, besides the graph: node state and the latest displays.
struct RunSlots {
    states: Vec<StateSlot>,
    displays: Vec<DisplaySlot>,
}

/// The latest display of a node that shows live data, for its window.
struct DisplaySlot {
    /// The pass it's from (`None` until the first), and the display.
    latest: Mutex<(Option<u64>, Box<dyn Any + Send>)>,
}

impl DisplaySlot {
    /// Starts with an empty display buffer, swapped out for the first pass's display.
    fn new(empty: Box<dyn Any + Send>) -> Self {
        DisplaySlot { latest: Mutex::new((None, empty)) }
    }

    /// Swaps `display` (from pass `sequence`) in as the latest, if it's newer and the window isn't reading it
    /// just now. `display` is left holding a buffer to reuse.
    fn offer(&self, sequence: u64, display: &mut Box<dyn Any + Send>) {
        let Ok(mut latest) = self.latest.try_lock() else { return };
        let (shown, previous) = &mut *latest;
        if shown.is_none_or(|shown| shown < sequence) {
            std::mem::swap(previous, display);
            *shown = Some(sequence);
        }
    }
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

struct Record {
    finished: Instant,
    time: Duration,
    lag: Duration,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    memory: AtomicUsize,
    busy: AtomicUsize,
    completed: AtomicU64,
    dropped: AtomicU64,
    recent: Mutex<VecDeque<Record>>,
}

struct Job {
    sequence: u64,
    /// When the pass's data was all due: the end of the time it covers.
    due: Instant,
}

/// A run of one plan: the graph its passes run against (swapped for newer snapshots of the same shape), the
/// plan, what the workers share running it, and the queue its passes are dispatched on. A change to the graph
/// that changes the plan starts a new epoch, which the dispatcher moves on to once a worker is ready for it.
struct Epoch<N: NodeSet> {
    graph: Mutex<Arc<Graph<N>>>,
    plan: Arc<Plan<N>>,
    slots: RunSlots,
    /// How many workers have a workspace for this epoch, ready to run its passes.
    ready: AtomicUsize,
    /// Taken by the dispatcher when it starts sending this epoch's passes; dropping it lets the workers on this
    /// epoch know there will be no more.
    sender: Mutex<Option<Sender<Job>>>,
    receiver: Receiver<Job>,
    shared: Arc<Shared>,
}

impl<N: NodeSet> Epoch<N> {
    fn new(snapshot: &Snapshot<N>, workers: usize, shared: Arc<Shared>) -> Self {
        let (graph, plan) = (snapshot.graph.clone(), snapshot.plan.clone());
        let slots = RunSlots {
            states: plan.initial_states(&graph),
            displays: plan.empty_displays(&graph).into_iter().map(DisplaySlot::new).collect(),
        };
        // A few passes may queue, beyond which they're dropped.
        let (sender, receiver) = crossbeam_channel::bounded(2 * workers);
        Epoch {
            graph: Mutex::new(graph),
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

impl<N: NodeSet> Drop for Epoch<N> {
    /// Passes still queued when no worker is left to run them were never run.
    fn drop(&mut self) {
        self.shared.dropped.fetch_add(self.receiver.len() as u64, Ordering::Relaxed);
    }
}

/// The epochs a run has: the newest (which workers prepare for), and those whose displays are shown.
struct Epochs<N: NodeSet> {
    wanted: Mutex<Arc<Epoch<N>>>,
    /// The epoch being dispatched and the one before, whose displays are shown until the new one has some.
    shown: Mutex<[Option<Arc<Epoch<N>>>; 2]>,
}

/// The graph running: a dispatcher and a pool of workers, until dropped.
pub struct Runner<N: NodeSet> {
    epochs: Arc<Epochs<N>>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    workers: usize,
    /// The version of the latest snapshot given to the run.
    version: AtomicU64,
}

impl<N: NodeSet> Runner<N> {
    pub fn start(snapshot: &Snapshot<N>, workers: usize) -> Self {
        let workers = workers.max(1);
        let shared = Arc::new(Shared::default());
        let epoch = Arc::new(Epoch::new(snapshot, workers, shared.clone()));
        let epochs = Arc::new(Epochs { wanted: Mutex::new(epoch), shown: Mutex::new([None, None]) });

        let mut threads = Vec::new();
        for index in 0..workers {
            let (epochs, shared) = (epochs.clone(), shared.clone());
            let worker = move || work(&epochs, &shared);
            threads
                .push(std::thread::Builder::new().name(format!("pass worker {index}")).spawn(worker).expect("worker"));
        }
        let (dispatcher_epochs, dispatcher_shared) = (epochs.clone(), shared.clone());
        let dispatcher = move || dispatch(&dispatcher_epochs, &dispatcher_shared);
        threads.push(std::thread::Builder::new().name("pass dispatcher".into()).spawn(dispatcher).expect("dispatcher"));

        Runner { epochs, shared, threads, workers, version: AtomicU64::new(snapshot.version) }
    }

    /// The graph of the latest snapshot given to the run.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn graph(&self) -> Arc<Graph<N>> {
        self.epochs.wanted.lock().unwrap().graph.lock().unwrap().clone()
    }

    /// Moves the run on to a newer snapshot. If it runs the same way (as after a change of settings), passes
    /// carry on with it from the next one, keeping the nodes' state, and this returns true. Otherwise it starts
    /// a new epoch: the workers make buffers for its plan as they come free, and its passes start (with fresh
    /// state) once one is ready; until then, passes carry on with the old one.
    pub fn update(&self, snapshot: &Snapshot<N>) -> bool {
        if self.version.swap(snapshot.version, Ordering::Relaxed) == snapshot.version {
            return true;
        }
        let mut wanted = self.epochs.wanted.lock().unwrap();
        if Arc::ptr_eq(&wanted.plan, &snapshot.plan) || wanted.plan.same_shape(&snapshot.plan) {
            *wanted.graph.lock().unwrap() = snapshot.graph.clone();
            true
        } else {
            *wanted = Arc::new(Epoch::new(snapshot, self.workers, self.shared.clone()));
            false
        }
    }

    /// Runs `f` on the latest live display of node `id`, if it's shown any yet. While `f` runs, workers skip
    /// updating it rather than wait.
    pub fn with_display<R>(&self, id: NodeId, f: impl FnOnce(&(dyn Any + Send)) -> R) -> Option<R> {
        let shown = self.epochs.shown.lock().unwrap();
        for epoch in shown.iter().flatten() {
            let Some(index) = epoch.plan.displays.iter().position(|d| *d == id) else { continue };
            let latest = epoch.slots.displays[index].latest.lock().unwrap();
            if latest.0.is_some() {
                return Some(f(latest.1.as_ref()));
            }
        }
        None
    }

    /// The plan passes are being dispatched for, once they are.
    #[cfg(test)]
    fn running_plan(&self) -> Option<Arc<Plan<N>>> {
        self.epochs.shown.lock().unwrap()[0].as_ref().map(|e| e.plan.clone())
    }

    pub fn stats(&self) -> Stats {
        let now = Instant::now();
        let recent = self.shared.recent.lock().unwrap();
        let last_second: Vec<&Record> = recent.iter().filter(|r| now - r.finished <= Duration::from_secs(1)).collect();
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

impl<N: NodeSet> Drop for Runner<N> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// How long an idle worker waits for a pass before checking for a newer epoch to get ready for.
const IDLE_CHECK: Duration = Duration::from_millis(2);

/// A worker: runs passes of its epoch, and once there's a newer one and nothing of its own queued, makes
/// buffers for that and moves on to it. Ends when the run stops, once every pass dispatched to it is done.
fn work<N: NodeSet>(epochs: &Epochs<N>, shared: &Shared) {
    let mut mine: Option<(Arc<Epoch<N>>, Workspace<N>)> = None;
    loop {
        let wanted = epochs.wanted.lock().unwrap().clone();
        let behind = match &mine {
            None => true,
            Some((epoch, _)) => !Arc::ptr_eq(epoch, &wanted) && epoch.receiver.is_empty(),
        };
        if behind {
            // Free the old buffers before making the new.
            if let Some((_, workspace)) = mine.take() {
                shared.memory.fetch_sub(workspace.bytes(), Ordering::Relaxed);
            }
            let graph = wanted.graph.lock().unwrap().clone();
            let workspace = wanted.plan.workspace(&graph);
            shared.memory.fetch_add(workspace.bytes(), Ordering::Relaxed);
            wanted.ready.fetch_add(1, Ordering::Release);
            mine = Some((wanted, workspace));
        }
        let Some((epoch, workspace)) = &mut mine else { unreachable!() };
        match epoch.receiver.recv_timeout(IDLE_CHECK) {
            Ok(job) => {
                shared.busy.fetch_add(1, Ordering::Relaxed);
                let start = Instant::now();
                let graph = epoch.graph.lock().unwrap().clone();
                epoch.plan.run(&graph, job.sequence, workspace, &epoch.slots);
                let finished = Instant::now();
                shared.busy.fetch_sub(1, Ordering::Relaxed);
                shared.completed.fetch_add(1, Ordering::Relaxed);
                let record =
                    Record { finished, time: finished - start, lag: finished.saturating_duration_since(job.due) };
                let mut recent = shared.recent.lock().unwrap();
                recent.push_back(record);
                while recent.front().is_some_and(|r| finished - r.finished > Duration::from_secs(2)) {
                    recent.pop_front();
                }
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
fn dispatch<N: NodeSet>(epochs: &Epochs<N>, shared: &Shared) {
    let duration = Duration::from_secs_f64(PASS_DURATION);
    let mut current: Option<(Arc<Epoch<N>>, Sender<Job>)> = None;
    let (mut sequence, mut due) = (0, Instant::now());
    while !shared.stop.load(Ordering::Relaxed) {
        let wanted = epochs.wanted.lock().unwrap().clone();
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
            let previous = current.take().map(|(epoch, _)| epoch); // dropping its sender ends its passes
            *epochs.shown.lock().unwrap() = [Some(wanted.clone()), previous];
            current = Some((wanted, sender));
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
            Ok(()) => sequence += 1,
            Err(TrySendError::Full(_)) => {
                shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
        due += duration;
    }
    // Dropping the sender lets the workers finish what's queued, then stop.
}

/// How many workers to use by default: one fewer than the cores (leaving one for the GUI), up to 8.
pub fn default_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1)).clamp(1, 8)
}

/// Whether two graphs would process the same way: the same nodes, settings and edges, whatever their
/// positions and labels.
pub fn same_processing<N: NodeSet>(a: &Graph<N>, b: &Graph<N>) -> bool {
    a.edges() == b.edges()
        && a.iter().count() == b.iter().count()
        && a.iter().zip(b.iter()).all(|((x, xe), (y, ye))| x == y && xe.node == ye.node)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    use crate::Buffer;
    use crate::Nodes;

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
        graph.add(Nodes::create(index), [0.0, 0.0])
    }

    fn link(graph: &mut Graph<Nodes>, from: NodeId, output: usize, to: NodeId, input: usize) {
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(from, Role::Output, output), port(to, Role::Input, input)).unwrap();
    }

    fn position(plan: &Plan<Nodes>, id: NodeId) -> usize {
        plan.steps.iter().position(|s| s.id == id).unwrap()
    }

    #[test]
    fn outputs_are_made_in_place_after_other_readers() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let gain = add(&mut graph, "Gain");
        let filter = add(&mut graph, "Band-pass Filter");
        link(&mut graph, board, 0, gain, 0);
        link(&mut graph, gain, 0, filter, 0);
        // Board (continuous and events), then gain and filter working in the board's continuous buffer.
        let plan = Plan::new(&graph);
        assert_eq!(plan.buffers.len(), 2);

        // A viewer also reading the board's data runs before the gain edits it, so the gain still works in place.
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, viewer, 0);
        let plan = Plan::new(&graph);
        assert_eq!(plan.buffers.len(), 2);
        assert!(position(&plan, viewer) < position(&plan, gain));

        // Two nodes that would edit the board's data in place: one does (last), the other gets its own buffer.
        let second_gain = add(&mut graph, "Gain");
        link(&mut graph, board, 0, second_gain, 0);
        let plan = Plan::new(&graph);
        assert_eq!(plan.buffers.len(), 3);
        assert!(position(&plan, second_gain) < position(&plan, gain));
    }

    #[test]
    fn passes_filter_and_detect_spikes() {
        use crate::standard_components::ephys::board::{Source, serve_test_board};

        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        let filter = add(&mut graph, "Band-pass Filter");
        let detector = add(&mut graph, "Spike Detector");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, detector, 0);
        let plan = Plan::new(&graph);
        let displays = plan.empty_displays(&graph).into_iter().map(DisplaySlot::new).collect();
        let slots = RunSlots { states: plan.initial_states(&graph), displays };
        let mut workspace = plan.workspace(&graph);
        // A second of samples, as they come from the board in real time.
        let (mut spikes, mut samples) = (0, 0);
        let filter_output = plan.steps.iter().find(|s| s.id == filter).unwrap().outputs[0];
        let detector_output = plan.steps.iter().find(|s| s.id == detector).unwrap().outputs[0];
        let deadline = Instant::now() + Duration::from_secs(5);
        for sequence in 0.. {
            plan.run(&graph, sequence, &mut workspace, &slots);
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
        let snapshot = Snapshot::new(0, crate::example_graph(), None);
        let runner = Runner::start(&snapshot, 2);
        std::thread::sleep(Duration::from_millis(300));
        let stats = runner.stats();
        drop(runner);
        assert!(stats.completed >= 5, "expected passes to run: {stats:?}");
        assert!(stats.memory > 0);
    }

    #[test]
    fn snapshots_share_a_plan_until_processing_changes() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let first = Snapshot::new(0, graph.clone(), None);
        graph.get_mut(board).unwrap().pos = [100.0, 50.0];
        let moved = Snapshot::new(1, graph.clone(), Some(&first));
        assert!(Arc::ptr_eq(&first.plan, &moved.plan), "moving a node doesn't change how it runs");
        add(&mut graph, "Gain");
        let added = Snapshot::new(2, graph, Some(&moved));
        assert!(!Arc::ptr_eq(&moved.plan, &added.plan));
    }

    #[test]
    fn settings_changes_carry_on_and_others_start_a_new_epoch() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let filter = add(&mut graph, "Band-pass Filter");
        link(&mut graph, board, 0, filter, 0);
        let mut snapshot = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start(&snapshot, 2);
        let mut next = |graph: &Graph<Nodes>| {
            snapshot = Snapshot::new(snapshot.version + 1, graph.clone(), Some(&snapshot));
            snapshot.clone()
        };

        // A new cut-off is a change of settings: the run carries on with it.
        if let Some(Nodes::BandPassFilter(f)) = graph.get_mut(filter).map(|e| &mut e.node) {
            f.high_hz = 3000.0;
        }
        assert!(runner.update(&next(&graph)));

        // A node that isn't connected doesn't run, so doesn't change anything; connected, it does.
        let gain = add(&mut graph, "Gain");
        assert!(runner.update(&next(&graph)));
        link(&mut graph, board, 0, gain, 0);
        let latest = next(&graph);
        assert!(!runner.update(&latest));

        // Passes move on to the new plan once a worker has buffers for it, without a pass going undone.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !runner.running_plan().is_some_and(|p| Arc::ptr_eq(&p, &latest.plan)) {
            assert!(Instant::now() < deadline, "passes should move on to the new plan");
            std::thread::sleep(Duration::from_millis(5));
        }
        let completed = runner.stats().completed;
        std::thread::sleep(Duration::from_millis(100));
        let stats = runner.stats();
        assert!(stats.completed > completed, "{stats:?}");
        assert_eq!(stats.dropped, 0, "{stats:?}");
    }
}
