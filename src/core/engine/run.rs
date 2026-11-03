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
//! When the graph changes while running, the new snapshot is swapped in between passes if its plan has the
//! same shape (the same nodes and buffers, as after a change of settings): nodes see their new settings on
//! the next pass and adjust (keeping their state). Otherwise it starts a new epoch, with its own node state
//! and pass queue. Workers make buffers for the new plan as they come free (once they've nothing of the old
//! one queued), and the dispatcher only sends passes for it once a worker is ready, to the workers that are;
//! until then, passes carry on with the old one. So a worker is never handed a pass it isn't ready to run.
//!
//! Nodes don't copy their inputs: a node that would hand an input's data on (to a thread of its own, say, or its
//! window) takes its buffer (see `Node::takes`), swapping it for one of its own. The plan decides when that needs a
//! copy: if the node can be the last to read the data, it takes the buffer itself; otherwise the plan copies the data
//! into a buffer of the node's own, as soon as it's made, which the node takes instead (a copy point). Each run has
//! an `Exchange` where nodes keep what their passes and windows share (see `crate::core::exchange`).

use std::any::Any;
use std::cell::UnsafeCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use crossbeam_utils::CachePadded;

use crate::core::console::{self, PassId};
use crate::core::exchange::Exchange;

use super::{Buffer, DataStreamSet, Edges, Graph, NodeId, NodeSet, Port, Recording, Role, Snapshot};

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
    /// The node being run.
    pub node: NodeId,
    /// For each input, this pass's data on it, if it's connected (and not taken: see `take_input`).
    pub inputs: &'a [Option<&'a Buffer<E>>],
    /// For each input, its buffer, if the node's taking it (see `Node::takes`).
    taken: &'a mut [Option<Buffer<E>>],
    /// For each output, a buffer to fill.
    pub outputs: &'a mut [Buffer<E>],
    /// For each of the node's controls, the value of the setting it's connected to, if it is (for a writer to
    /// record, say).
    pub controls: &'a [Option<E::Setting>],
    /// Whether the graph is recording.
    pub recording: bool,
    /// The recording, with what nodes prepared for it.
    prepared: Option<&'a Recording>,
    /// What the node's passes and window share (see `crate::core::exchange`).
    pub exchange: &'a Exchange,
}

impl<'a, E: Edges> Pass<'a, E> {
    /// A pass for trying a node out on, not recording, with no controls connected.
    #[cfg(test)]
    pub fn new(
        sequence: u64,
        duration: f64,
        inputs: &'a [Option<&'a Buffer<E>>],
        outputs: &'a mut [Buffer<E>],
    ) -> Self {
        Self::taking(sequence, duration, inputs, Box::leak(Box::new([])), outputs)
    }

    /// The same, with the node given `taken` to take (see `Node::takes`), for each input (`None` for those it reads).
    #[cfg(test)]
    pub fn taking(
        sequence: u64,
        duration: f64,
        inputs: &'a [Option<&'a Buffer<E>>],
        taken: &'a mut [Option<Buffer<E>>],
        outputs: &'a mut [Buffer<E>],
    ) -> Self {
        let exchange: &'static Exchange = Box::leak(Box::default());
        let (node, controls, recording, prepared) = (NodeId(0), &[], false, None);
        Pass { sequence, duration, node, inputs, taken, outputs, controls, recording, prepared, exchange }
    }

    /// The node's shared `T` in the run's exchange, made with `make` the first time (see `crate::core::exchange`):
    /// keep it in the node's state, rather than finding it every pass.
    pub fn shared<T: crate::core::exchange::Shared>(&self, make: impl FnOnce() -> T) -> Arc<T> {
        self.exchange.shared(self.node, make)
    }

    /// Whether the node's trigger `name` (see `Node::triggers`) has been fired, taking it if so: the node then says
    /// how it's getting on (`trigger_progress`) until it's done (`trigger_done`).
    pub fn triggered(&self, name: &'static str) -> bool {
        self.exchange.take_trigger(self.node, name)
    }

    /// Whether the node's trigger `name`, taken, has been stopped since: the node should abandon what it's doing
    /// for it (it's then done).
    pub fn trigger_cancelled(&self, name: &'static str) -> bool {
        self.exchange.take_cancel(self.node, name)
    }

    /// How far along the node's trigger `name` is, from 0 to 1, for the GUI to show.
    pub fn trigger_progress(&self, name: &'static str, progress: f32) {
        self.exchange.set_trigger_progress(self.node, name, Some(progress.clamp(0.0, 1.0)));
    }

    /// The node's trigger `name` is done.
    pub fn trigger_done(&self, name: &'static str) {
        self.exchange.set_trigger_progress(self.node, name, None);
    }

    /// Proposes a new value for the node's setting `setting` (by index), which the GUI applies as an edit (unless
    /// it's recording): passes can't change the graph themselves.
    pub fn propose(&self, setting: usize, value: E::Setting) {
        self.exchange.propose(self.node, setting, Box::new(value));
    }

    /// An input's data, taken (see `take_input`) or not.
    pub fn input(&self, index: usize) -> Option<&Buffer<E>> {
        match self.taken.get(index) {
            Some(Some(buffer)) => Some(buffer),
            _ => self.inputs.get(index).copied().flatten(),
        }
    }

    /// The buffer of an input the node takes (see `Node::takes`), if it's connected: to swap for another just as big
    /// (of the same data stream), which the pass carries on with. The data's the node's to do as it likes with.
    pub fn take_input(&mut self, index: usize) -> Option<&mut Buffer<E>> {
        self.taken.get_mut(index)?.as_mut()
    }

    /// What the node prepared to record (see `Node::prepare_to_record`), if it did and it's of type `T`: given once,
    /// to the first pass that asks while recording.
    pub fn take_prepared<T: 'static>(&self) -> Option<T> {
        let prepared = self.prepared?.take(self.node)?;
        prepared.downcast().ok().map(|b| *b)
    }
}

/// A node to run in each pass, and which buffers (in the workspace's pool) it works with.
#[derive(PartialEq)]
pub struct Step {
    pub id: NodeId,
    /// For each input, the buffer feeding it. `None` if it's unconnected, or an output is made from it in place.
    pub inputs: Vec<Option<usize>>,
    /// For each output, its buffer.
    pub outputs: Vec<usize>,
    /// For each output, the input it's made from in place (taking over its buffer), if it is.
    pub in_place: Vec<Option<usize>>,
    /// For each input, whether the node takes its buffer (see `Node::takes`): being the last to read it, or a copy.
    pub takes: Vec<bool>,
    /// Copies of outputs the plan makes once the node's run, for nodes taking them that can't be the last to read
    /// them: from the output's buffer, into the taker's own.
    pub copies: Vec<CopyPoint>,
    /// The node's state, if it carries any over: an index into the run's state slots. Each pass waits here for
    /// the pass before to hand the state over.
    pub state: Option<usize>,
    /// The node's working memory, if its state is only that: an index into each worker's own.
    pub memory: Option<usize>,
}

/// A copy of a buffer the plan makes, for a node taking it that can't be the last to read it.
#[derive(Clone, PartialEq, Debug)]
pub struct CopyPoint {
    pub from: usize,
    pub to: usize,
    /// The node it's for, and which of its inputs.
    pub node: NodeId,
    pub input: usize,
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
///
/// Likewise a node taking an input's buffer (see `Node::takes`) takes it if every other reader of the data can run
/// first, and no one's editing it in place; otherwise the plan copies it for the node into a buffer of its own, once
/// it's made. Otherwise nodes run in the graph's order.
pub struct Plan<N: NodeSet> {
    steps: Vec<Step>,
    /// The data stream each buffer in the pool holds.
    buffers: Vec<DataStream<N>>,
    /// For each node with state, the data streams on its inputs (for its initial state).
    state_inputs: Vec<(NodeId, Vec<Option<DataStream<N>>>)>,
    /// For each node with working memory, the same.
    memory_inputs: Vec<(NodeId, Vec<Option<DataStream<N>>>)>,
    /// The memory each buffer takes, once asked (see `buffer_bytes`).
    bytes: OnceLock<Vec<usize>>,
}

impl<N: NodeSet> Plan<N> {
    pub fn new(graph: &Graph<N>) -> Self {
        let resolved = graph.resolve();
        let topological = graph.topological_order();
        let mut runs: Vec<NodeId> = topological
            .iter()
            .copied()
            .filter(|id| graph.get(*id).is_some_and(|e| !e.disabled))
            .filter(|id| resolved.get(id).is_some_and(|s| s.outputs.iter().all(|o| o.stream.is_some())))
            .collect();
        // Data edges to inputs their nodes need now.
        let needed_edge = |e: &&super::Edge, runs: &[NodeId]| {
            e.dimension == super::Dimension::Data
                && graph.is_live(e)
                && runs.contains(&e.from.node)
                && runs.contains(&e.to.node)
                && graph.get(e.to.node).is_some_and(|n| n.node.needs(e.to.port))
        };
        // Only nodes something needs the outputs of run, besides sources and those without outputs (viewers,
        // writers): from the last back, as needing passes back along the edges.
        for &id in topological.iter().rev() {
            let Some(entry) = graph.get(id) else { continue };
            let always = entry.node.info().category == super::Category::Source
                || resolved.get(&id).is_some_and(|s| s.outputs.is_empty());
            let needed = always || graph.edges().iter().any(|e| e.from.node == id && needed_edge(&e, &runs));
            if !needed {
                runs.retain(|r| *r != id);
            }
        }
        let data_edges: Vec<_> = graph.edges().iter().filter(|e| needed_edge(e, &runs)).collect();
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

        // Pick the inputs taken: those whose data no one edits in place, whose other readers can run first.
        let mut taken: Vec<(NodeId, usize)> = Vec::new();
        for &id in &runs {
            let (Some(entry), Some(state)) = (graph.get(id), resolved.get(&id)) else { continue };
            for input in (0..state.inputs.len()).filter(|&i| entry.node.takes(i)) {
                let Some(edge) = data_edges.iter().find(|e| e.to.node == id && e.to.port == input) else { continue };
                let source = (edge.from.node, edge.from.port);
                if claimed.contains(&source) {
                    continue;
                }
                let others: Vec<NodeId> = data_edges
                    .iter()
                    .filter(|e| (e.from.node, e.from.port) == source && e.to.node != id)
                    .map(|e| e.to.node)
                    .collect();
                if others.iter().any(|&other| reaches(&before, id, other)) {
                    continue;
                }
                for other in others {
                    before.entry(other).or_default().push(id);
                }
                claimed.push(source);
                taken.push((id, input));
            }
        }

        let (mut steps, mut buffers, mut state_inputs, mut memory_inputs) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // The buffer each output (of the nodes so far) is in.
        let mut buffer_of: HashMap<(NodeId, usize), usize> = HashMap::new();
        for id in order(&runs, &before) {
            let (Some(entry), Some(state)) = (graph.get(id), resolved.get(&id)) else { continue };
            let mut inputs: Vec<Option<usize>> = (0..state.inputs.len())
                .map(|index| {
                    let edge = graph.live_edge_into(Port { node: id, role: Role::Input, index })?;
                    buffer_of.get(&(edge.from.node, edge.from.port)).copied()
                })
                .collect();
            let (mut outputs, mut made_in_place) = (Vec::new(), Vec::new());
            for (index, output) in state.outputs.iter().enumerate() {
                let from = in_place.get(&(id, index)).and_then(|&input| Some((input, inputs[input]?)));
                let buffer = match from {
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
                made_in_place.push(from.map(|(input, _)| input));
                buffer_of.insert((id, index), buffer);
            }
            let carried = entry.node.has_state() && entry.node.state_carried_over();
            let node_state = carried.then(|| {
                state_inputs.push((id, state.input_streams.clone()));
                state_inputs.len() - 1
            });
            let memory = (entry.node.has_state() && !carried).then(|| {
                memory_inputs.push((id, state.input_streams.clone()));
                memory_inputs.len() - 1
            });
            // The inputs it takes but can't be the last to read: copied for it, once they're made.
            let mut takes = Vec::with_capacity(inputs.len());
            for (input, buffer) in inputs.iter_mut().enumerate() {
                let Some(from) = *buffer else {
                    takes.push(false);
                    continue;
                };
                if !(entry.node.takes(input) && entry.node.needs(input)) {
                    takes.push(false);
                } else if taken.contains(&(id, input)) {
                    takes.push(true);
                } else {
                    // Copied once the node making it has run, before anything edits it in place.
                    let edge = graph.live_edge_into(Port { node: id, role: Role::Input, index: input });
                    let maker = edge.and_then(|e| steps.iter_mut().find(|s: &&mut Step| s.id == e.from.node));
                    let Some(maker) = maker else {
                        takes.push(false);
                        continue;
                    };
                    buffers.push(buffers[from]);
                    let to = buffers.len() - 1;
                    maker.copies.push(CopyPoint { from, to, node: id, input });
                    *buffer = Some(to);
                    takes.push(true);
                }
            }
            let copies = Vec::new();
            steps.push(Step { id, inputs, outputs, in_place: made_in_place, takes, state: node_state, memory, copies });
        }
        Plan { steps, buffers, state_inputs, memory_inputs, bytes: OnceLock::new() }
    }

    /// The nodes that run, in the order they run in each pass.
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The data stream each buffer holds (each worker has one of each).
    pub fn buffers(&self) -> &[DataStream<N>] {
        &self.buffers
    }

    /// The memory each buffer takes, in bytes. Worked out (by making one of each) the first time it's asked.
    pub fn buffer_bytes(&self) -> &[usize] {
        self.bytes.get_or_init(|| {
            self.buffers.iter().map(|s| <DataStream<N> as DataStreamSet>::bytes(&s.buffer(PASS_DURATION))).collect()
        })
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
                    recording: snapshot.recording.is_some(),
                    prepared: snapshot.recording.as_deref(),
                    exchange: &run.exchange,
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
                    <DataStream<N> as DataStreamSet>::copy(from, &mut to);
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

/// `nodes` ordered so each comes after those it must run after, otherwise keeping their order, but with those
/// in `early` as soon as they can be.
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
        let ready = |id: &&NodeId| waiting.get(id) == Some(&0);
        let Some(&next) = nodes.iter().find(ready) else { break };
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

/// A worker's buffers, for every step. Made once per worker per plan, before the worker runs any of the plan's
/// passes.
struct Workspace<N: NodeSet> {
    buffers: Vec<Option<Buffer<N::Edges>>>,
    memory: Vec<Box<dyn Any + Send>>,
}

impl<N: NodeSet> Workspace<N> {
    /// The memory of its data buffers.
    fn bytes(&self) -> usize {
        self.buffers.iter().flatten().map(<<N::Edges as Edges>::DataStream as DataStreamSet>::bytes).sum()
    }
}

/// What the workers share during a run of one plan, besides the graph: the nodes' state, the run's exchange, and how
/// long each step's taking.
struct RunSlots {
    states: Vec<StateSlot>,
    exchange: Arc<Exchange>,
    times: Vec<StepTimes>,
}

impl RunSlots {
    fn new<N: NodeSet>(plan: &Plan<N>, graph: &Graph<N>, exchange: Arc<Exchange>) -> Self {
        let times = plan.steps.iter().map(|_| StepTimes::default()).collect();
        RunSlots { states: plan.initial_states(graph), exchange, times }
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
    sequence: u64,
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
    /// Where nodes leave things for their windows.
    exchange: Arc<Exchange>,
    /// What each worker is up to.
    workers: Vec<WorkerSlot>,
    /// The next pass the dispatcher will send.
    next_pass: AtomicU64,
    /// Epochs started, which numbers them (from 1).
    epochs: AtomicU64,
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
pub struct EpochInfo<N: NodeSet> {
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
pub struct RunInfo<N: NodeSet> {
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

struct Job {
    sequence: u64,
    /// When the pass's data was all due: the end of the time it covers.
    due: Instant,
}

/// A run of one plan: the snapshot its passes run against (swapped for newer snapshots of the same shape), the
/// plan, what the workers share running it, and the queue its passes are dispatched on. A change to the graph
/// that changes the plan starts a new epoch, which the dispatcher moves on to once a worker is ready for it.
struct Epoch<N: NodeSet> {
    number: u64,
    snapshot: Mutex<Snapshot<N>>,
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
        let plan = snapshot.plan.clone();
        let slots = RunSlots::new(&plan, &snapshot.graph, shared.exchange.clone());
        // A few passes may queue, beyond which they're dropped.
        let (sender, receiver) = crossbeam_channel::bounded(queue_capacity(workers));
        let number = shared.epochs.fetch_add(1, Ordering::Relaxed) + 1;
        Epoch {
            number,
            snapshot: Mutex::new(snapshot.clone()),
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
    /// Passes still queued when no worker is left to run them were never run. The nodes' state is let go of in
    /// the background: that can take a while (a capture thread stopping, say), and this may be on a worker, whose
    /// passes mustn't wait, or the GUI.
    fn drop(&mut self) {
        self.shared.dropped.fetch_add(self.receiver.len() as u64, Ordering::Relaxed);
        let states = std::mem::take(&mut self.slots.states);
        if states.is_empty() {
            return;
        }
        let dropping = std::thread::Builder::new().name("dropping node state".into()).spawn(move || drop(states));
        if let Ok(thread) = dropping {
            crate::core::background::finish_later(thread);
        }
    }
}

/// The epochs a run has: the newest (which workers prepare for), and the one being dispatched.
struct Epochs<N: NodeSet> {
    wanted: Mutex<Arc<Epoch<N>>>,
    running: Mutex<Option<Arc<Epoch<N>>>>,
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
        let shared =
            Arc::new(Shared { workers: (0..workers).map(|_| WorkerSlot::default()).collect(), ..Default::default() });
        let epoch = Arc::new(Epoch::new(snapshot, workers, shared.clone()));
        log::info!("Running on {workers} workers, {} ms passes", PASS_DURATION * 1000.0);
        log::debug!("Epoch {} for version {}: {}", epoch.number, snapshot.version, describe(&epoch.plan));
        let epochs = Arc::new(Epochs { wanted: Mutex::new(epoch), running: Mutex::new(None) });

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

        Runner { epochs, shared, threads, workers, version: AtomicU64::new(snapshot.version) }
    }

    /// The graph of the latest snapshot given to the run.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn graph(&self) -> Arc<Graph<N>> {
        self.epochs.wanted.lock().unwrap().snapshot.lock().unwrap().graph.clone()
    }

    /// Where the run's nodes leave things for their windows.
    pub fn exchange(&self) -> &Exchange {
        &self.shared.exchange
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
        let mut wanted = self.epochs.wanted.lock().unwrap();
        if Arc::ptr_eq(&wanted.plan, &snapshot.plan) || wanted.plan.same_shape(&snapshot.plan) {
            *wanted.snapshot.lock().unwrap() = snapshot.clone();
            log::trace!("Version {} carries on epoch {}", snapshot.version, wanted.number);
            true
        } else {
            *wanted = Arc::new(Epoch::new(snapshot, self.workers, self.shared.clone()));
            log::info!(
                "Version {} changes the plan: epoch {}, {}",
                snapshot.version,
                wanted.number,
                describe(&wanted.plan)
            );
            false
        }
    }

    /// The plan passes are being dispatched for, once they are.
    #[cfg(test)]
    fn running_plan(&self) -> Option<Arc<Plan<N>>> {
        self.epochs.running.lock().unwrap().as_ref().map(|e| e.plan.clone())
    }

    /// Everything about the run as of now: its epochs, workers and dispatcher, and the stats.
    pub fn inspect(&self) -> RunInfo<N> {
        let info = |epoch: &Epoch<N>| EpochInfo {
            number: epoch.number,
            version: epoch.snapshot.lock().unwrap().version,
            plan: epoch.plan.clone(),
            ready: epoch.ready.load(Ordering::Relaxed),
            queued: epoch.receiver.len(),
            step_times: epoch.slots.times.iter().map(StepTimes::get).collect(),
        };
        let running = self.epochs.running.lock().unwrap().as_deref().map(info);
        let wanted = self.epochs.wanted.lock().unwrap().clone();
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
        let recent = self.shared.recent.lock().unwrap();
        let mut times: Vec<[f64; 2]> =
            recent.iter().map(|r| [(r.sequence + 1) as f64 * PASS_DURATION, r.time.as_secs_f64()]).collect();
        times.sort_by(|a, b| a[0].total_cmp(&b[0]));
        times
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
fn work<N: NodeSet>(epochs: &Epochs<N>, shared: &Shared, index: usize) {
    let slot = &shared.workers[index];
    slot.pass.store(IDLE, Ordering::Relaxed);
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
            let graph = wanted.snapshot.lock().unwrap().graph.clone();
            let workspace = wanted.plan.workspace(&graph);
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
            mine = Some((wanted, workspace));
        }
        let Some((epoch, workspace)) = &mut mine else { unreachable!() };
        match epoch.receiver.recv_timeout(IDLE_CHECK) {
            Ok(job) => {
                shared.busy.fetch_add(1, Ordering::Relaxed);
                slot.pass.store(job.sequence, Ordering::Relaxed);
                let start = Instant::now();
                let snapshot = epoch.snapshot.lock().unwrap().clone();
                let pass = PassId { epoch: epoch.number, sequence: job.sequence, node: None, node_type: None };
                epoch.plan.run(&snapshot, pass, workspace, &epoch.slots);
                let finished = Instant::now();
                slot.pass.store(IDLE, Ordering::Relaxed);
                slot.passes.fetch_add(1, Ordering::Relaxed);
                shared.busy.fetch_sub(1, Ordering::Relaxed);
                shared.completed.fetch_add(1, Ordering::Relaxed);
                let (time, lag) = (finished - start, finished.saturating_duration_since(job.due));
                let record = Record { sequence: job.sequence, finished, time, lag };
                let mut recent = shared.recent.lock().unwrap();
                recent.push_back(record);
                while recent.front().is_some_and(|r| finished - r.finished > PASS_TIMES) {
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
    // Passes dropped in a row, so a run of them is reported once.
    let mut dropping = 0u64;
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
            log::debug!("Epoch {} running from pass {sequence}", wanted.number);
            *epochs.running.lock().unwrap() = Some(wanted.clone());
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

/// "4 steps, 6 buffers (1.2 MB a worker), 3 with state, 1 copy".
fn describe<N: NodeSet>(plan: &Plan<N>) -> String {
    let bytes: usize = plan.buffer_bytes().iter().sum();
    let copies: usize = plan.steps.iter().map(|s| s.copies.len()).sum();
    format!(
        "{} steps, {} buffers ({:.2} MB a worker), {} with state, {copies} {}",
        plan.steps.len(),
        plan.buffers.len(),
        bytes as f64 / 1e6,
        plan.state_inputs.len(),
        if copies == 1 { "copy" } else { "copies" }
    )
}

/// How many workers to use by default: half the cores (leaving the rest for the GUI, nodes' own threads and
/// everything else), up to 4.
pub fn default_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4)
}

/// Whether two graphs would process the same way: the same nodes, settings and edges, whatever their
/// positions and labels.
pub fn same_processing<N: NodeSet>(a: &Graph<N>, b: &Graph<N>) -> bool {
    a.edges() == b.edges()
        && a.edges().iter().all(|e| a.is_edge_disabled(e) == b.is_edge_disabled(e))
        && a.iter().count() == b.iter().count()
        && a.iter().zip(b.iter()).all(|((x, xe), (y, ye))| x == y && xe.node == ye.node && xe.disabled == ye.disabled)
}

#[cfg(test)]
pub mod tests {
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
        // Something needing the filter's output, so it runs (see `Node::needs`).
        let end = add(&mut graph, "LFP Viewer");
        link(&mut graph, filter, 0, end, 0);
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
        let second_end = add(&mut graph, "LFP Viewer");
        link(&mut graph, second_gain, 0, second_end, 0);
        let plan = Plan::new(&graph);
        assert_eq!(plan.buffers.len(), 3);
        assert!(position(&plan, second_gain) < position(&plan, gain));
    }

    #[test]
    fn writers_take_buffers_no_one_else_needs_after_them() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let writer = add(&mut graph, "Write Raw");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, writer, 0);
        link(&mut graph, board, 1, writer, 1);
        link(&mut graph, board, 0, viewer, 0);
        // The writer takes both the board's buffers, after the viewer's read the continuous one.
        let plan = Plan::new(&graph);
        let step = |plan: &Plan<Nodes>, id| plan.steps.iter().find(|s| s.id == id).unwrap().takes.clone();
        assert_eq!(step(&plan, writer), vec![true, true]);
        assert!(position(&plan, viewer) < position(&plan, writer));

        // A filter editing the board's data in place: the plan copies it for the writer, once the board's run.
        let filter = add(&mut graph, "Band-pass Filter");
        link(&mut graph, board, 0, filter, 0);
        let end = add(&mut graph, "LFP Viewer");
        link(&mut graph, filter, 0, end, 0);
        let plan = Plan::new(&graph);
        assert_eq!(step(&plan, writer), vec![true, true], "it's always given buffers to take");
        let board_step = plan.steps.iter().find(|s| s.id == board).unwrap();
        let writer_step = plan.steps.iter().find(|s| s.id == writer).unwrap();
        let copy =
            CopyPoint { from: board_step.outputs[0], to: writer_step.inputs[0].unwrap(), node: writer, input: 0 };
        assert_eq!(board_step.copies, vec![copy]);
        assert_eq!(plan.buffers[writer_step.inputs[0].unwrap()], plan.buffers[board_step.outputs[0]]);
    }

    #[test]
    fn nodes_run_only_while_something_needs_their_outputs() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let spectrum = add(&mut graph, "Spectrum");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, spectrum, 0);
        link(&mut graph, board, 0, viewer, 0);
        link(&mut graph, spectrum, 0, viewer, 1);
        let runs = |graph: &Graph<Nodes>, id| Plan::new(graph).steps.iter().any(|s| s.id == id);
        assert!(runs(&graph, spectrum), "the viewer shows the spectra");
        if let Some(Nodes::LfpViewer(v)) = graph.get_mut(viewer).map(|e| &mut e.node) {
            v.spectrum = false;
        }
        assert!(!runs(&graph, spectrum), "nothing needs them");
        assert!(runs(&graph, board) && runs(&graph, viewer), "sources and viewers run regardless");
        let plan = Plan::new(&graph);
        assert_eq!(plan.steps.iter().find(|s| s.id == viewer).unwrap().inputs[1], None, "nor are they passed");
    }

    #[test]
    fn copies_for_nodes_taking_are_made_before_the_data_is_edited() {
        use crate::standard_components::ephys::board::{Source, serve_test_board};

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
        let slots = RunSlots::new(plan, &snapshot.graph, Arc::default());
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
        use crate::standard_components::ephys::board::{Source, serve_test_board};

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
        let slots = RunSlots::new(plan, &snapshot.graph, Arc::default());
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
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, filter, 0);
        link(&mut graph, filter, 0, viewer, 0);
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

        // A node that isn't connected doesn't run, so doesn't change anything; connected (with something needing
        // its output), it does.
        let gain = add(&mut graph, "Gain");
        assert!(runner.update(&next(&graph)));
        link(&mut graph, board, 0, gain, 0);
        assert!(runner.update(&next(&graph)), "nothing needs its output");
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
        let completed = runner.stats().completed;
        let stats = wait_for_passes(&runner, completed + 1);
        assert!(stats.completed > completed, "{stats:?}");
        assert_eq!(stats.dropped, 0, "{stats:?}");
    }
}
