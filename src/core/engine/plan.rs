//! The plan of a snapshot: which nodes run (each decides, given what it's connected to), in what order, on which
//! buffers, and where data's copied. Worked out once, when the snapshot's taken, for every pass of it (see `run`).
//!
//! Nodes don't copy their inputs: a node that would hand an input's data on (to a thread of its own, say, or its
//! window) takes its buffer (see `BufferUsage::takes`), swapping it for one of its own. The plan decides when that needs a
//! copy: if the node can be the last to read the data, it takes the buffer itself; otherwise the plan copies the data
//! into a buffer of the node's own, as soon as it's made, which the node takes instead (a copy point).

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use super::pass::*;
use super::{BufferUsage, DataStreamsEnum, Edges, Graph, NodeId, NodesEnum, Port, PortRole, TreeShake};

type DataStream<N> = <<N as NodesEnum>::Edges as Edges>::DataStream;

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
    /// For each input, whether the node takes its buffer (see `BufferUsage::takes`): being the last to read it, or a copy.
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
/// of its own, except where a node can make an output in place from an input (see `BufferUsage::in_place`): then
/// the output takes over the input's buffer, as long as every other node reading that data can run before
/// it. Within a pass everything runs on one thread, in an order of our choosing, so the plan orders the
/// nodes to make that so: the other readers first (they only read, e.g. viewers copying what they show),
/// then the one editing in place. Only if another reader depends on the editor's output can't it be.
///
/// Likewise a node taking an input's buffer (see `BufferUsage::takes`) takes it if every other reader of the data can run
/// first, and no one's editing it in place; otherwise the plan copies it for the node into a buffer of its own, once
/// it's made. Otherwise nodes run in the graph's order.
pub struct Plan<N: NodesEnum> {
    pub(super) steps: Vec<Step>,
    /// The data stream each buffer in the pool holds.
    pub(super) buffers: Vec<DataStream<N>>,
    /// For each node with state, the data streams on its inputs (for its initial state).
    pub(super) state_inputs: Vec<(NodeId, Vec<Option<DataStream<N>>>)>,
    /// For each node with working memory, the same.
    pub(super) memory_inputs: Vec<(NodeId, Vec<Option<DataStream<N>>>)>,
    /// For each step, the same (for what it shares with the GUI).
    pub(super) step_inputs: Vec<Vec<Option<DataStream<N>>>>,
    /// The memory each buffer takes, once asked (see `buffer_bytes`).
    bytes: OnceLock<Vec<usize>>,
}

/// A data edge, as `settle` sees it: from an output to an input.
struct Link {
    from: super::PortRef,
    to: super::PortRef,
}

/// How many times a node's asked for its tree shake before its answer's taken as it stands (see `settle`).
const SETTLE_ASKS: usize = 8;

/// Which of `nodes` (each with how many inputs and outputs it has, upstream first) run, and which inputs each needs,
/// as each `answer`s (see `Node::evaluate_tree_shake`), given which of its inputs have something running into them,
/// and which of its outputs have something running that needs them. Everything starts out running and needing
/// everything, and each is asked in turn; when one's answer changes, its neighbours are asked again, if what they'd be
/// told has changed, until nothing does.
///
/// A node's answer can keep changing, if more in use can make it run or need less: so after `SETTLE_ASKS`, it's asked
/// once more, and runs, and needs each input, if either of its last two answers did, for good. Running when it needn't
/// is better than not when it should. That's warned of, by its `label`.
fn settle(
    nodes: &[(NodeId, usize, usize)],
    links: &[Link],
    answer: impl Fn(NodeId, &[bool], &[bool]) -> TreeShake,
    label: impl Fn(NodeId) -> String,
) -> Vec<(NodeId, Vec<bool>)> {
    let mut shakes: HashMap<NodeId, TreeShake> =
        nodes.iter().map(|&(id, inputs, _)| (id, TreeShake::needing_all(true, inputs))).collect();
    let mut told: HashMap<NodeId, (Vec<bool>, Vec<bool>)> = HashMap::new();
    let mut asks: HashMap<NodeId, usize> = HashMap::new();
    let mut queue: VecDeque<(NodeId, usize, usize)> = nodes.iter().copied().collect();
    while let Some((id, input_count, output_count)) = queue.pop_front() {
        let asked = asks.get(&id).copied().unwrap_or(0);
        if asked > SETTLE_ASKS {
            continue;
        }
        let port = |port| super::PortRef { node: id, port };
        let runs = |node: NodeId| shakes.get(&node).is_some_and(|s| s.runs);
        let needs = |to: super::PortRef| shakes.get(&to.node).is_some_and(|s| s.runs && s.needs[to.port]);
        let inputs: Vec<bool> =
            (0..input_count).map(|i| links.iter().any(|l| l.to == port(i) && runs(l.from.node))).collect();
        let outputs: Vec<bool> =
            (0..output_count).map(|o| links.iter().any(|l| l.from == port(o) && needs(l.to))).collect();
        if told.get(&id) == Some(&(inputs.clone(), outputs.clone())) {
            continue;
        }
        let was = shakes[&id].clone();
        let mut now = answer(id, &inputs, &outputs);
        assert_eq!(now.needs.len(), input_count, "{}: a need for each input", label(id));
        if asked == SETTLE_ASKS {
            now.runs |= was.runs;
            now.needs.iter_mut().zip(&was.needs).for_each(|(now, was)| *now |= was);
            log::warn!(
                "{} kept changing its mind about whether to run, or what it needs, given what it's connected to: it {}",
                label(id),
                if now.runs { "runs" } else { "doesn't run" }
            );
        }
        asks.insert(id, asked + 1);
        told.insert(id, (inputs, outputs));
        if now == was {
            continue;
        }
        shakes.insert(id, now);
        // What it's connected to is told something different now.
        for link in links.iter().filter(|l| l.from.node == id || l.to.node == id) {
            let other = if link.from.node == id { link.to.node } else { link.from.node };
            if let Some(node) = nodes.iter().find(|(n, _, _)| *n == other)
                && !queue.iter().any(|(n, _, _)| *n == other)
            {
                queue.push_back(*node);
            }
        }
    }
    // In the order they were given.
    nodes.iter().filter_map(|(id, _, _)| shakes.remove(id).filter(|s| s.runs).map(|s| (*id, s.needs))).collect()
}

impl<N: NodesEnum> Plan<N> {
    pub fn new(graph: &Graph<N>) -> Self {
        let resolved = graph.resolve();
        let topological = graph.topological_order();
        // Which nodes run is up to them (see `settle`): of those that can, not disabled, with every output known.
        let data: Vec<&super::Edge> =
            graph.edges().iter().filter(|e| e.dimension == super::Dimension::Data && graph.is_live(e)).collect();
        let can_run: Vec<(NodeId, usize, usize)> = topological
            .iter()
            .filter(|id| graph.get(**id).is_some_and(|e| !e.disabled))
            .filter_map(|id| resolved.get(id).map(|s| (*id, s)))
            .filter(|(_, s)| s.outputs.iter().all(|o| o.stream.is_some()))
            .map(|(id, s)| (id, s.inputs.len(), s.outputs.len()))
            .collect();
        let links: Vec<Link> = data.iter().map(|e| Link { from: e.from, to: e.to }).collect();
        let shaken = settle(
            &can_run,
            &links,
            |id, inputs, outputs| match graph.get(id) {
                Some(e) => e.node.evaluate_tree_shake(inputs, outputs),
                None => TreeShake::needing_all(false, inputs.len()),
            },
            |id| graph.get(id).map_or_else(|| format!("{id:?}"), |e| e.label.clone()),
        );
        let runs: Vec<NodeId> = shaken.iter().map(|(id, _)| *id).collect();
        let needs: HashMap<NodeId, Vec<bool>> = shaken.into_iter().collect();
        let needed = |node: NodeId, input: usize| needs.get(&node).is_some_and(|n| n[input]);
        // How each node that runs uses its buffers.
        let usage: HashMap<NodeId, BufferUsage> = runs
            .iter()
            .filter_map(|&id| {
                let (entry, state) = (graph.get(id)?, resolved.get(&id)?);
                Some((id, entry.node.evaluate_buffer_usage(state.inputs.len(), state.outputs.len())))
            })
            .collect();
        let data_edges: Vec<&super::Edge> =
            data.iter().copied().filter(|e| runs.contains(&e.from.node) && needed(e.to.node, e.to.port)).collect();
        // Which nodes must run before which: those feeding them, and those reading data another edits in place.
        let mut before: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in &data_edges {
            before.entry(edge.from.node).or_default().push(edge.to.node);
        }

        // Pick the outputs made in place: (node, output) -> its input, and the data each takes over.
        let mut in_place: HashMap<(NodeId, usize), usize> = HashMap::new();
        let mut claimed: Vec<(NodeId, usize)> = Vec::new();
        for &id in &runs {
            let Some(usage) = usage.get(&id) else { continue };
            for (output, &input) in usage.in_place.iter().enumerate() {
                let Some(input) = input else { continue };
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
            let Some(usage) = usage.get(&id) else { continue };
            for input in (0..usage.takes.len()).filter(|&i| usage.takes[i]) {
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

        let (mut steps, mut buffers, mut state_inputs, mut memory_inputs, mut step_inputs) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // The buffer each output (of the nodes so far) is in.
        let mut buffer_of: HashMap<(NodeId, usize), usize> = HashMap::new();
        for id in order(&runs, &before) {
            let (Some(entry), Some(state)) = (graph.get(id), resolved.get(&id)) else { continue };
            let mut inputs: Vec<Option<usize>> = (0..state.inputs.len())
                .map(|index| {
                    let edge = graph.live_edge_into(Port { node: id, role: PortRole::Input, index })?;
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
                if !(usage[&id].takes[input] && needed(id, input)) {
                    takes.push(false);
                } else if taken.contains(&(id, input)) {
                    takes.push(true);
                } else {
                    // Copied once the node making it has run, before anything edits it in place.
                    let edge = graph.live_edge_into(Port { node: id, role: PortRole::Input, index: input });
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
            step_inputs.push(state.input_streams.clone());
            steps.push(Step { id, inputs, outputs, in_place: made_in_place, takes, state: node_state, memory, copies });
        }
        Plan { steps, buffers, state_inputs, memory_inputs, step_inputs, bytes: OnceLock::new() }
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
            self.buffers.iter().map(|s| <DataStream<N> as DataStreamsEnum>::bytes(&s.buffer(PASS_DURATION))).collect()
        })
    }

    /// Whether runs of the two plans would work the same way (the same steps on the same buffers), so a run of
    /// one can carry on with the other's graph.
    pub fn same_shape(&self, other: &Self) -> bool {
        self.steps == other.steps && self.buffers == other.buffers
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

/// "4 steps, 6 buffers (1.2 MB a worker), 3 with state, 1 copy".
pub(super) fn describe<N: NodesEnum>(plan: &Plan<N>) -> String {
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

/// Whether two graphs would process the same way: the same nodes, settings and edges, whatever their
/// positions and labels.
pub fn same_processing<N: NodesEnum>(a: &Graph<N>, b: &Graph<N>) -> bool {
    a.edges() == b.edges()
        && a.edges().iter().all(|e| a.is_edge_disabled(e) == b.is_edge_disabled(e))
        && a.iter().count() == b.iter().count()
        && a.iter().zip(b.iter()).all(|((x, xe), (y, ye))| x == y && xe.node == ye.node && xe.disabled == ye.disabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeType;
    use crate::core::engine::NodeTypeEnum;
    use std::sync::Arc;

    use super::super::Snapshot;

    use crate::Nodes;

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let node_type = NodeType::named(name).unwrap();
        graph.add(Nodes::create(node_type), [0.0, 0.0])
    }

    fn link(graph: &mut Graph<Nodes>, from: NodeId, output: usize, to: NodeId, input: usize) {
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(from, PortRole::Output, output), port(to, PortRole::Input, input)).unwrap();
    }

    fn position(plan: &Plan<Nodes>, id: NodeId) -> usize {
        plan.steps.iter().position(|s| s.id == id).unwrap()
    }

    #[test]
    fn outputs_are_made_in_place_after_other_readers() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let gain = add(&mut graph, "Continuous Gain");
        let filter = add(&mut graph, "Band-pass Filter");
        link(&mut graph, board, 0, gain, 0);
        link(&mut graph, gain, 0, filter, 0);
        // Something needing the filter's output, so it runs (see `Node::evaluate_tree_shake`).
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
        let second_gain = add(&mut graph, "Continuous Gain");
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
    fn a_node_runs_if_it_would_given_what_runs_upstream() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let viewer = add(&mut graph, "LFP Viewer");
        let alone = add(&mut graph, "TTL Viewer");
        let writer = add(&mut graph, "Write Raw");
        link(&mut graph, board, 0, viewer, 0);
        let runs = |graph: &Graph<Nodes>, id| Plan::new(graph).steps.iter().any(|s| s.id == id);
        assert!(runs(&graph, board), "a source runs, as nodes do unless they say otherwise");
        assert!(runs(&graph, viewer), "something's running on its input");
        assert!(runs(&graph, alone), "nor need it say otherwise: there's just nothing to show");
        assert!(!runs(&graph, writer), "a writer only with something to record");
        let lone = add(&mut graph, "Camera");
        assert!(runs(&graph, lone), "with no inputs, it runs whether or not anything needs it");
        graph.get_mut(board).unwrap().disabled = true;
        assert!(!runs(&graph, viewer), "what feeds it isn't running");
        let filter = add(&mut graph, "Band-pass Filter");
        let fed = add(&mut graph, "LFP Viewer");
        link(&mut graph, filter, 0, fed, 0);
        assert!(!runs(&graph, filter) && !runs(&graph, fed), "connected, but to nothing running");
    }

    #[test]
    fn a_node_that_keeps_changing_its_mind_ends_up_running() {
        use std::cell::Cell;
        let (a, b) = (NodeId(1), NodeId(2));
        let links =
            [Link { from: super::super::PortRef { node: a, port: 0 }, to: super::super::PortRef { node: b, port: 0 } }];
        let asked = Cell::new(0);
        // a runs only while nothing uses what it makes, and b only while it's given something: round they'd go.
        let answer = |id, inputs: &[bool], outputs: &[bool]| {
            asked.set(asked.get() + 1);
            let runs = if id == a { !outputs[0] } else { inputs[0] };
            TreeShake::needing_all(runs, inputs.len())
        };
        let runs = settle(&[(a, 0, 1), (b, 1, 0)], &links, answer, |id| format!("{id:?}"));
        assert_eq!(runs, [(a, vec![]), (b, vec![true])], "running, when in doubt, and what it feeds with it");
        assert!(asked.get() <= 2 * (SETTLE_ASKS + 1), "{} asks", asked.get());

        let steady =
            |_, inputs: &[bool], _: &[bool]| TreeShake::needing_all(inputs.is_empty() || inputs[0], inputs.len());
        assert_eq!(settle(&[(a, 0, 1), (b, 1, 0)], &links, steady, |_| String::new()), runs);

        // b needs what a makes only while it's not given it: round they'd go, so it needs it, and a runs.
        asked.set(0);
        let needy = |id, inputs: &[bool], outputs: &[bool]| {
            asked.set(asked.get() + 1);
            if id == a {
                TreeShake::needing_all(outputs[0], 0)
            } else {
                TreeShake { runs: true, needs: vec![!inputs[0]] }
            }
        };
        let runs = settle(&[(a, 0, 1), (b, 1, 0)], &links, needy, |id| format!("{id:?}"));
        assert_eq!(runs, [(a, vec![]), (b, vec![true])], "needing it, when in doubt");
        assert!(asked.get() <= 2 * (SETTLE_ASKS + 1), "{} asks", asked.get());
    }

    #[test]
    fn what_a_node_does_not_need_counts_for_neither_it_nor_what_feeds_it() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let spectrum = add(&mut graph, "Spectrum");
        let viewer = add(&mut graph, "LFP Viewer");
        link(&mut graph, board, 0, spectrum, 0);
        link(&mut graph, spectrum, 0, viewer, 1);
        let runs = |graph: &Graph<Nodes>, id| Plan::new(graph).steps.iter().any(|s| s.id == id);
        assert!(runs(&graph, spectrum) && runs(&graph, viewer), "it shows the spectra");
        if let Some(Nodes::LfpViewer(v)) = graph.get_mut(viewer).map(|e| &mut e.node) {
            v.spectrum = false;
        }
        assert!(!runs(&graph, viewer), "the spectra are all it's given, and it doesn't need them");
        assert!(!runs(&graph, spectrum), "so nothing needs them");
        assert!(runs(&graph, board), "which needn't stop a source");
    }

    #[test]
    fn snapshots_share_a_plan_until_processing_changes() {
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        let first = Snapshot::new(0, graph.clone(), None);
        graph.get_mut(board).unwrap().pos = [100.0, 50.0];
        let moved = Snapshot::new(1, graph.clone(), Some(&first));
        assert!(Arc::ptr_eq(&first.plan, &moved.plan), "moving a node doesn't change how it runs");
        add(&mut graph, "Continuous Gain");
        let added = Snapshot::new(2, graph, Some(&moved));
        assert!(!Arc::ptr_eq(&moved.plan, &added.plan));
    }
}
