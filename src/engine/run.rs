//! Running the graph: a small pool of worker threads, each running whole passes over a snapshot of it.
//!
//! Time is cut into passes of `PASS_DURATION`, numbered from 0. A dispatcher thread releases each pass when
//! its time is up, to whichever worker is free. A worker runs the pass through every node in order, on
//! buffers of its own made before the run starts (so nothing is allocated per pass), with each node reading
//! its inputs from the outputs of the nodes before it.
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
//! the next pass and adjust (keeping their state). Otherwise the run restarts.
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

use crossbeam_channel::TrySendError;
use crossbeam_utils::CachePadded;

use super::{Buffer, Edges, Graph, NodeId, NodeSet, Port, Role, SignalSet, Snapshot};

type Signal<N> = <<N as NodeSet>::Edges as Edges>::Signal;

/// The length of time each pass covers, in seconds.
pub const PASS_DURATION: f64 = 0.02;

/// What a node works on for one pass.
pub struct Pass<'a, E: Edges> {
    /// Passes are numbered from 0, each covering the next `duration` seconds.
    pub sequence: u64,
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

/// Which nodes run, in what order, on which buffers: worked out once per snapshot, before running.
///
/// The nodes that run are those whose outputs are all known (so can have buffers). Each output gets a buffer
/// of its own, except where a node can make an output in place from an input (see `Node::in_place`): then
/// the output takes over the input's buffer, as long as every other node reading that data can run before
/// it. Within a pass everything runs on one thread, in an order of our choosing, so the plan orders the
/// nodes to make that so: the other readers first (they only read, e.g. viewers copying what they show),
/// then the one editing in place. Only if another reader depends on the editor's output can't it be.
struct Plan<N: NodeSet> {
    steps: Vec<Step>,
    /// The signal each buffer in the pool holds.
    buffers: Vec<Signal<N>>,
    /// For each node with state, the signals on its inputs (for its initial state).
    state_inputs: Vec<(NodeId, Vec<Option<Signal<N>>>)>,
    /// The nodes that show live data in their windows.
    displays: Vec<NodeId>,
}

impl<N: NodeSet> Plan<N> {
    fn new(graph: &Graph<N>) -> Self {
        let resolved = graph.resolve();
        let runs: Vec<NodeId> = graph
            .topological_order()
            .into_iter()
            .filter(|id| resolved.get(id).is_some_and(|s| s.outputs.iter().all(|o| o.signal.is_some())))
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
                        buffers.push(output.signal.expect("only nodes with known outputs run"));
                        buffers.len() - 1
                    }
                };
                outputs.push(buffer);
                buffer_of.insert((id, index), buffer);
            }
            let node_state = entry.node.has_state().then(|| {
                state_inputs.push((id, state.input_signals.clone()));
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
        let state = |id: NodeId, inputs: &[Option<Signal<N>>]| graph.get(id).map(|e| e.node.initial_state(inputs));
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
struct Workspace<N: NodeSet> {
    buffers: Vec<Option<Buffer<N::Edges>>>,
    displays: Vec<Option<Box<dyn Any + Send>>>,
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
    /// Memory allocated for the workers' buffers, in bytes.
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

/// The graph running: a dispatcher and a pool of workers, until dropped.
pub struct Runner<N: NodeSet> {
    /// The graph passes run against, swapped for newer snapshots of the same shape.
    graph: Arc<Mutex<Arc<Graph<N>>>>,
    plan: Arc<Plan<N>>,
    slots: Arc<RunSlots>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    workers: usize,
    memory: usize,
}

impl<N: NodeSet> Runner<N> {
    pub fn start(snapshot: &Snapshot<N>, workers: usize) -> Self {
        let workers = workers.max(1);
        let graph = snapshot.graph.clone();
        let plan = Arc::new(Plan::new(&graph));
        let slots = Arc::new(RunSlots {
            states: plan.initial_states(&graph),
            displays: plan.empty_displays(&graph).into_iter().map(DisplaySlot::new).collect(),
        });
        let current = Arc::new(Mutex::new(graph.clone()));
        let shared = Arc::new(Shared::default());
        // A few passes may queue, beyond which they're dropped.
        let (sender, receiver) = crossbeam_channel::bounded::<Job>(2 * workers);

        let workspaces: Vec<_> = (0..workers).map(|_| plan.workspace(&graph)).collect();
        let memory = workspaces
            .iter()
            .flat_map(|w| w.buffers.iter().flatten())
            .map(<<N::Edges as Edges>::Signal as SignalSet>::bytes)
            .sum();

        let mut threads = Vec::new();
        for (index, mut workspace) in workspaces.into_iter().enumerate() {
            let (plan, slots, shared, receiver) = (plan.clone(), slots.clone(), shared.clone(), receiver.clone());
            let current = current.clone();
            let worker = move || {
                // Ends once the dispatcher has gone and every queued pass is done.
                while let Ok(job) = receiver.recv() {
                    shared.busy.fetch_add(1, Ordering::Relaxed);
                    let start = Instant::now();
                    let graph = current.lock().unwrap().clone();
                    plan.run(&graph, job.sequence, &mut workspace, &slots);
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
            };
            threads
                .push(std::thread::Builder::new().name(format!("pass worker {index}")).spawn(worker).expect("worker"));
        }

        let dispatcher_shared = shared.clone();
        let dispatcher = move || {
            let shared = dispatcher_shared;
            let duration = Duration::from_secs_f64(PASS_DURATION);
            let start = Instant::now();
            let (mut sequence, mut due) = (0, start + duration);
            while !shared.stop.load(Ordering::Relaxed) {
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
        };
        threads.push(std::thread::Builder::new().name("pass dispatcher".into()).spawn(dispatcher).expect("dispatcher"));

        Runner { graph: current, plan, slots, shared, threads, workers, memory }
    }

    /// The graph being run.
    pub fn graph(&self) -> Arc<Graph<N>> {
        self.graph.lock().unwrap().clone()
    }

    /// Carries on with a newer snapshot from the next pass, if it runs the same way (as after a change of
    /// settings), returning whether it could. If not, the run needs restarting.
    pub fn update(&self, snapshot: &Snapshot<N>) -> bool {
        if !Plan::new(&snapshot.graph).same_shape(&self.plan) {
            return false;
        }
        *self.graph.lock().unwrap() = snapshot.graph.clone();
        true
    }

    /// Runs `f` on the latest live display of node `id`, if it's shown any yet. While `f` runs, workers skip
    /// updating it rather than wait.
    pub fn with_display<R>(&self, id: NodeId, f: impl FnOnce(&(dyn Any + Send)) -> R) -> Option<R> {
        let index = self.plan.displays.iter().position(|d| *d == id)?;
        let latest = self.slots.displays[index].latest.lock().unwrap();
        latest.0.map(|_| f(latest.1.as_ref()))
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
            memory: self.memory,
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

    use crate::edges::Buffer;
    use crate::nodes::Nodes;

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
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let filter = add(&mut graph, "Band-pass Filter");
        let detector = add(&mut graph, "Spike Detector");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, detector, 0);
        let plan = Plan::new(&graph);
        let slots = RunSlots { states: plan.initial_states(&graph), displays: Vec::new() };
        let mut workspace = plan.workspace(&graph);
        let mut spikes = 0;
        for sequence in 0..50 {
            plan.run(&graph, sequence, &mut workspace, &slots);
            let detector_output = plan.steps.iter().find(|s| s.id == detector).unwrap().outputs[0];
            let Some(Buffer::Spikes(output)) = &workspace.buffers[detector_output] else { panic!("spikes buffer") };
            spikes += output.spikes.len();
        }
        let filter_output = plan.steps.iter().find(|s| s.id == filter).unwrap().outputs[0];
        let Some(Buffer::Continuous(filtered)) = &workspace.buffers[filter_output] else { panic!("continuous buffer") };
        assert_eq!(filtered.len, 600, "20 ms at 30 kHz");
        assert!(filtered.channel(0).iter().all(|x| x.is_finite() && x.abs() < 1000.0));
        // The simulation puts a spike in about 1% of milliseconds on each of 64 channels: about 640 in a second.
        assert!((100..3000).contains(&spikes), "found {spikes} spikes in a second");
    }

    #[test]
    fn runs_the_example_graph() {
        let graph = crate::nodes::example();
        let snapshot = Snapshot { version: 0, graph: Arc::new(graph) };
        let runner = Runner::start(&snapshot, 2);
        std::thread::sleep(Duration::from_millis(300));
        let stats = runner.stats();
        drop(runner);
        assert!(stats.completed >= 5, "expected passes to run: {stats:?}");
        assert!(stats.memory > 0);
    }

    #[test]
    fn settings_changes_carry_on_and_others_restart() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let filter = add(&mut graph, "Band-pass Filter");
        link(&mut graph, board, 0, filter, 0);
        let runner = Runner::start(&Snapshot { version: 0, graph: Arc::new(graph.clone()) }, 1);

        // A new cut-off is a change of settings: the run carries on with it.
        if let Some(Nodes::BandPassFilter(f)) = graph.get_mut(filter).map(|e| &mut e.node) {
            f.high_hz = 3000.0;
        }
        assert!(runner.update(&Snapshot { version: 1, graph: Arc::new(graph.clone()) }));

        // A node that isn't connected doesn't run, so doesn't change anything; connected, it does.
        let gain = add(&mut graph, "Gain");
        assert!(runner.update(&Snapshot { version: 2, graph: Arc::new(graph.clone()) }));
        link(&mut graph, board, 0, gain, 0);
        assert!(!runner.update(&Snapshot { version: 3, graph: Arc::new(graph) }));
    }
}
