//! The graph of node instances and the edges between their ports.

use std::any::Any;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::{
    Context, Control, Dimension, EdgeInfo, EdgeSet, Edges, Input, NodeSet, Output, Plan, SettingPort, same_processing,
};
use crate::core::theme::Colours;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// What a port is for. Data edges run from outputs to inputs; settings edges from controls to settings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Input,
    Output,
    /// A setting of this node that another node may edit.
    Setting,
    /// Edits a setting of another node.
    Control,
}

impl Role {
    pub fn dimension(self) -> Dimension {
        match self {
            Role::Input | Role::Output => Dimension::Data,
            Role::Setting | Role::Control => Dimension::Settings,
        }
    }

    /// The role at the other end of an edge.
    pub fn partner(self) -> Role {
        match self {
            Role::Input => Role::Output,
            Role::Output => Role::Input,
            Role::Setting => Role::Control,
            Role::Control => Role::Setting,
        }
    }

    /// Whether edges start here (rather than end here).
    pub fn is_source(self) -> bool {
        matches!(self, Role::Output | Role::Control)
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Input => "input",
            Role::Output => "output",
            Role::Setting => "setting",
            Role::Control => "control",
        }
    }
}

/// A port of a node: one of its inputs, outputs, settings or controls, by index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Port {
    pub node: NodeId,
    pub role: Role,
    pub index: usize,
}

/// One end of an edge, as stored.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct PortRef {
    pub node: NodeId,
    pub port: usize,
}

/// An output feeding an input, or a control editing a setting. Each input or setting has at most one edge
/// into it; an output or control can have any number out of it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Edge {
    #[serde(default)]
    pub dimension: Dimension,
    pub from: PortRef,
    pub to: PortRef,
}

impl Edge {
    /// The output or control the edge starts from.
    pub fn source(&self) -> Port {
        let role = if self.dimension == Dimension::Data { Role::Output } else { Role::Control };
        Port { node: self.from.node, role, index: self.from.port }
    }

    /// The input or setting the edge goes into.
    pub fn target(&self) -> Port {
        let role = if self.dimension == Dimension::Data { Role::Input } else { Role::Setting };
        Port { node: self.to.node, role, index: self.to.port }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeEntry<N> {
    pub node: N,
    /// Shown on the node; unique when the node is added, though the user can rename it to anything.
    pub label: String,
    /// Top-left corner on the canvas.
    pub pos: [f32; 2],
    /// Whether it's listed in the basic view, if set; otherwise, whether it's a visualisation (see `in_basic`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<bool>,
    /// Whether it's disabled: kept, but run as if it weren't there (see `Graph::is_live`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl<N: NodeSet> NodeEntry<N> {
    /// Whether it's listed in the basic view: as set, or by default if it's a visualisation.
    pub fn in_basic(&self) -> bool {
        self.basic.unwrap_or(self.node.info().category == super::Category::Visualisation)
    }
}

#[derive(Debug, PartialEq)]
pub enum ConnectError {
    NoSuchPort,
    SameNode,
    /// Neither an output and an input, nor a control and a setting.
    RoleMismatch,
    KindMismatch {
        dimension: Dimension,
        from: &'static str,
        to: &'static str,
    },
    Cycle,
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConnectError::NoSuchPort => write!(f, "no such port"),
            ConnectError::SameNode => write!(f, "a node can't connect to itself"),
            ConnectError::RoleMismatch => write!(f, "outputs connect to inputs, and controls to settings"),
            ConnectError::KindMismatch { dimension: Dimension::Data, from, to } => {
                write!(f, "a {from} output can't feed a {to} input")
            }
            ConnectError::KindMismatch { dimension: Dimension::Settings, from, to } => {
                write!(f, "a {from} control can't edit a {to} setting")
            }
            ConnectError::Cycle => write!(f, "that would make a loop"),
        }
    }
}

/// A node's ports and data streams, worked out from its settings and what's upstream of it.
pub struct NodeState<E: Edges> {
    pub inputs: Vec<Input<E>>,
    /// For each input, the data stream connected to it, if any.
    pub input_streams: Vec<Option<E::DataStream>>,
    pub outputs: Vec<Output<E>>,
    pub settings: Vec<SettingPort<E>>,
    pub controls: Vec<Control<E>>,
    pub warnings: Vec<String>,
}

/// Identifies an edge type: its dimension, and its index in that dimension's `EdgeSet::KINDS`.
pub type KindKey = (Dimension, usize);

impl<E: Edges> NodeState<E> {
    pub fn new<N: NodeSet<Edges = E>>(node: &N, input_streams: Vec<Option<E::DataStream>>) -> Self {
        NodeState {
            inputs: node.inputs(),
            outputs: node.outputs(&input_streams),
            warnings: node.warnings(&input_streams),
            input_streams,
            settings: node.settings(),
            controls: node.controls(),
        }
    }

    /// The node's state with nothing connected.
    pub fn unconnected<N: NodeSet<Edges = E>>(node: &N) -> Self {
        Self::new(node, vec![None; node.inputs().len()])
    }

    pub fn count(&self, role: Role) -> usize {
        match role {
            Role::Input => self.inputs.len(),
            Role::Output => self.outputs.len(),
            Role::Setting => self.settings.len(),
            Role::Control => self.controls.len(),
        }
    }

    pub fn name(&self, role: Role, index: usize) -> Option<&'static str> {
        match role {
            Role::Input => self.inputs.get(index).map(|p| p.name),
            Role::Output => self.outputs.get(index).map(|p| p.name),
            Role::Setting => self.settings.get(index).map(|p| p.name),
            Role::Control => self.controls.get(index).map(|p| p.name),
        }
    }

    pub fn kind(&self, role: Role, index: usize) -> Option<KindKey> {
        let stream = |kind| (Dimension::Data, E::DataStream::kind_index(kind));
        let setting = |kind| (Dimension::Settings, E::Setting::kind_index(kind));
        match role {
            Role::Input => self.inputs.get(index).map(|p| stream(p.kind)),
            Role::Output => self.outputs.get(index).map(|p| stream(p.kind)),
            Role::Setting => self.settings.get(index).map(|p| setting(p.value.kind())),
            Role::Control => self.controls.get(index).map(|p| setting(p.kind)),
        }
    }

    pub fn info(&self, role: Role, index: usize) -> Option<&'static EdgeInfo> {
        self.kind(role, index).map(edge_info::<E>)
    }

    /// The data stream on an input or output, or the value of a setting.
    pub fn summary(&self, role: Role, index: usize) -> Option<String> {
        match role {
            Role::Input => self.input_streams.get(index).copied().flatten().map(|s| s.summary()),
            Role::Output => self.outputs.get(index)?.stream.map(|s| s.summary()),
            Role::Setting => self.settings.get(index).map(|s| s.value.summary()),
            Role::Control => None,
        }
    }

    pub fn ports(&self, node: NodeId) -> impl Iterator<Item = Port> + '_ {
        [Role::Output, Role::Input, Role::Setting, Role::Control]
            .into_iter()
            .flat_map(move |role| (0..self.count(role)).map(move |index| Port { node, role, index }))
    }

    /// The first port with `role` and `kind`.
    pub fn find(&self, node: NodeId, role: Role, kind: KindKey) -> Option<Port> {
        (0..self.count(role)).find(|&i| self.kind(role, i) == Some(kind)).map(|index| Port { node, role, index })
    }
}

pub fn edge_info<E: Edges>((dimension, index): KindKey) -> &'static EdgeInfo {
    match dimension {
        Dimension::Data => &E::DataStream::KINDS[index],
        Dimension::Settings => &E::Setting::KINDS[index],
    }
}

pub type Resolved<E> = HashMap<NodeId, NodeState<E>>;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "N: NodeSet")]
pub struct Graph<N> {
    next_id: u64,
    nodes: BTreeMap<NodeId, NodeEntry<N>>,
    edges: Vec<Edge>,
    /// Edges that are disabled: kept, but run as if they weren't there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disabled_edges: Vec<Edge>,
}

impl<N> Default for Graph<N> {
    fn default() -> Self {
        Graph { next_id: 1, nodes: BTreeMap::new(), edges: Vec::new(), disabled_edges: Vec::new() }
    }
}

type Setting<N> = <<N as NodeSet>::Edges as Edges>::Setting;
type DataStream<N> = <<N as NodeSet>::Edges as Edges>::DataStream;

impl<N: NodeSet> Graph<N> {
    pub fn add(&mut self, node: N, pos: [f32; 2]) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let label = self.unique_label(node.info().name);
        self.nodes.insert(id, NodeEntry { node, label, pos, basic: None, disabled: false });
        id
    }

    /// `base`, or "`base` 2", "`base` 3"... whichever is first not already taken.
    fn unique_label(&self, base: &str) -> String {
        let taken = |label: &str| self.nodes.values().any(|n| n.label == label);
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|label| !taken(label)).unwrap()
    }

    pub fn remove(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.edges.retain(|e| e.from.node != id && e.to.node != id);
        self.disabled_edges.retain(|e| self.edges.contains(e));
    }

    pub fn get(&self, id: NodeId) -> Option<&NodeEntry<N>> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut NodeEntry<N>> {
        self.nodes.get_mut(&id)
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &NodeEntry<N>)> {
        self.nodes.iter().map(|(id, n)| (*id, n))
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    fn data_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(|e| e.dimension == Dimension::Data)
    }

    /// The edge into an input or setting, if any (enabled or not).
    pub fn edge_into(&self, port: Port) -> Option<Edge> {
        self.edges.iter().find(|e| e.target() == port).copied()
    }

    /// The edges out of an output or control (enabled or not).
    pub fn edges_from(&self, port: Port) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.source() == port).copied()
    }

    /// Whether an edge counts, as the graph runs: neither it nor the nodes at its ends disabled.
    pub fn is_live(&self, edge: &Edge) -> bool {
        let enabled = |id: NodeId| self.nodes.get(&id).is_some_and(|e| !e.disabled);
        !self.disabled_edges.contains(edge) && enabled(edge.from.node) && enabled(edge.to.node)
    }

    /// Whether an edge itself is disabled.
    pub fn is_edge_disabled(&self, edge: &Edge) -> bool {
        self.disabled_edges.contains(edge)
    }

    /// Disables an edge, or enables it.
    pub fn set_edge_disabled(&mut self, edge: Edge, disabled: bool) {
        self.disabled_edges.retain(|e| *e != edge);
        if disabled && self.edges.contains(&edge) {
            self.disabled_edges.push(edge);
        }
    }

    /// The live edge into an input or setting, if any (see `is_live`).
    pub fn live_edge_into(&self, port: Port) -> Option<Edge> {
        self.edges.iter().find(|e| e.target() == port && self.is_live(e)).copied()
    }

    /// The live edges out of an output or control.
    pub fn live_edges_from(&self, port: Port) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.source() == port && self.is_live(e)).copied()
    }

    fn kind(&self, port: Port) -> Option<KindKey> {
        NodeState::unconnected(&self.nodes.get(&port.node)?.node).kind(port.role, port.index)
    }

    /// The edge that would connect `a` and `b` (in either order), replacing anything already going into
    /// the input or setting.
    pub fn check(&self, a: Port, b: Port) -> Result<Edge, ConnectError> {
        if a.role.partner() != b.role {
            return Err(ConnectError::RoleMismatch);
        }
        let (from, to) = if a.role.is_source() { (a, b) } else { (b, a) };
        let (Some(from_kind), Some(to_kind)) = (self.kind(from), self.kind(to)) else {
            return Err(ConnectError::NoSuchPort);
        };
        if from.node == to.node {
            return Err(ConnectError::SameNode);
        }
        let dimension = from.role.dimension();
        if from_kind != to_kind {
            let name = |kind| edge_info::<N::Edges>(kind).name;
            return Err(ConnectError::KindMismatch { dimension, from: name(from_kind), to: name(to_kind) });
        }
        // Settings edges carry no data, so may point anywhere; only data edges must not loop.
        if dimension == Dimension::Data && self.reaches(to.node, from.node) {
            return Err(ConnectError::Cycle);
        }
        Ok(Edge {
            dimension,
            from: PortRef { node: from.node, port: from.index },
            to: PortRef { node: to.node, port: to.index },
        })
    }

    /// Connects `a` and `b` (in either order), replacing anything already going into the input, or the setting the
    /// control was linked to. (An input has one source, but a setting can have many controls: one editing it, say,
    /// and others reading it. A control has just the one setting.)
    pub fn connect(&mut self, a: Port, b: Port) -> Result<Edge, ConnectError> {
        let edge = self.check(a, b)?;
        self.edges.retain(|e| match edge.dimension {
            Dimension::Data => e.target() != edge.target(),
            Dimension::Settings => e.source() != edge.source(),
        });
        let edges = &self.edges;
        self.disabled_edges.retain(|e| edges.contains(e));
        self.edges.push(edge);
        Ok(edge)
    }

    pub fn disconnect(&mut self, edge: Edge) {
        self.edges.retain(|e| *e != edge);
        self.disabled_edges.retain(|e| *e != edge);
    }

    /// The ports, on every node, that `port` could connect to.
    pub fn targets(&self, port: Port) -> Vec<Port> {
        let mut targets = Vec::new();
        for (&id, entry) in &self.nodes {
            let state = NodeState::unconnected(&entry.node);
            let role = port.role.partner();
            targets.extend(
                (0..state.count(role))
                    .map(|index| Port { node: id, role, index })
                    .filter(|t| self.check(port, *t).is_ok()),
            );
        }
        targets
    }

    /// Whether `to` is downstream of `from` (or is `from`), along data edges.
    fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        let mut stack = vec![from];
        let mut seen = vec![];
        while let Some(id) = stack.pop() {
            if id == to {
                return true;
            }
            if !seen.contains(&id) {
                seen.push(id);
                stack.extend(self.data_edges().filter(|e| e.from.node == id).map(|e| e.to.node));
            }
        }
        false
    }

    /// Drops edges whose ports no longer exist or no longer match, e.g. after a settings change removed a port.
    pub fn prune_edges(&mut self) {
        let valid: Vec<bool> = self
            .edges
            .iter()
            .map(|e| matches!((self.kind(e.source()), self.kind(e.target())), (Some(a), Some(b)) if a == b))
            .collect();
        let mut valid = valid.into_iter();
        self.edges.retain(|_| valid.next().unwrap());
        let edges = &self.edges;
        self.disabled_edges.retain(|e| edges.contains(e));
    }

    /// Nodes ordered so that each comes after everything feeding it data. Nodes in a loop (only possible in
    /// a hand-edited file) come last, in id order.
    pub fn topological_order(&self) -> Vec<NodeId> {
        let mut pending: HashMap<NodeId, usize> = self.nodes.keys().map(|id| (*id, 0)).collect();
        for e in self.data_edges() {
            *pending.get_mut(&e.to.node).unwrap() += 1;
        }
        let mut ready: VecDeque<NodeId> = self.nodes.keys().filter(|id| pending[id] == 0).copied().collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(id) = ready.pop_front() {
            order.push(id);
            for e in self.data_edges().filter(|e| e.from.node == id) {
                let count = pending.get_mut(&e.to.node).unwrap();
                *count -= 1;
                if *count == 0 {
                    ready.push_back(e.to.node);
                }
            }
        }
        order.extend(self.nodes.keys().filter(|id| !order.contains(id)).collect::<Vec<_>>());
        order
    }

    /// Works out every node's ports and data streams, from the sources down.
    pub fn resolve(&self) -> Resolved<N::Edges> {
        let mut resolved: Resolved<N::Edges> = Resolved::new();
        for id in self.topological_order() {
            let entry = &self.nodes[&id];
            let node = &entry.node;
            let inputs = node.inputs();
            let input_streams: Vec<Option<DataStream<N>>> = (0..inputs.len())
                .map(|index| {
                    let edge = self.live_edge_into(Port { node: id, role: Role::Input, index })?;
                    let stream = resolved.get(&edge.from.node)?.outputs.get(edge.from.port)?.stream?;
                    (stream.kind() == inputs[index].kind).then_some(stream)
                })
                .collect();
            let mut state = NodeState::new(node, input_streams);
            // A disabled node makes nothing, and has nothing to warn about.
            if entry.disabled {
                for output in &mut state.outputs {
                    output.stream = None;
                }
                state.warnings.clear();
            }
            resolved.insert(id, state);
        }
        resolved
    }

    /// For each of a node's controls, the current value of the setting it's connected to, if any.
    pub fn control_values(&self, id: NodeId) -> Vec<Option<Setting<N>>> {
        let Some(entry) = self.nodes.get(&id) else { return Vec::new() };
        (0..entry.node.controls().len())
            .map(|index| {
                let edge = self.live_edges_from(Port { node: id, role: Role::Control, index }).next()?;
                self.nodes.get(&edge.to.node)?.node.settings().into_iter().nth(edge.to.port).map(|s| s.value)
            })
            .collect()
    }

    /// Runs `f` (a node's UI, say) on a node, then applies any changes it made through its controls to the
    /// settings they're connected to.
    #[allow(clippy::too_many_arguments)]
    pub fn edit<R>(
        &mut self,
        id: NodeId,
        inputs: &[Option<DataStream<N>>],
        theme: &Colours,
        display: Option<&(dyn Any + Send)>,
        running: bool,
        locked: bool,
        f: impl FnOnce(&mut N, &mut Context<N::Edges>) -> R,
    ) -> Option<R> {
        let before = self.control_values(id);
        let mut controls = before.clone();
        let entry = self.nodes.get_mut(&id)?;
        let read_only: Vec<bool> = entry.node.controls().iter().map(|c| c.read_only).collect();
        let context = &mut Context { inputs, controls: &mut controls, theme, display, running, locked };
        let result = f(&mut entry.node, context);
        for (index, (old, new)) in before.into_iter().zip(controls).enumerate() {
            if read_only.get(index) == Some(&true) {
                continue;
            }
            let Some(new) = new.filter(|new| Some(new) != old.as_ref()) else { continue };
            let targets: Vec<PortRef> =
                self.live_edges_from(Port { node: id, role: Role::Control, index }).map(|e| e.to).collect();
            for target in targets {
                let Some(entry) = self.nodes.get_mut(&target.node) else { continue };
                if entry.node.settings().get(target.port).is_some_and(|s| s.value.kind() == new.kind()) {
                    entry.node.set_setting(target.port, new.clone());
                }
            }
        }
        Some(result)
    }
}

/// For each node with controls, the value of the setting each is connected to, if it is.
pub type ControlValues<N> = HashMap<NodeId, Vec<Option<Setting<N>>>>;

/// A recording, once the nodes have prepared for it (see `Node::prepare_to_record`): what each prepared, until its
/// first recording pass takes it.
#[derive(Default)]
pub struct Recording {
    prepared: Mutex<HashMap<NodeId, Box<dyn Any + Send>>>,
}

impl Recording {
    /// Has every node that runs in `snapshot` prepare to record. Can take a while (opening files, say), so isn't
    /// for the GUI's thread.
    pub fn prepare<N: NodeSet>(snapshot: &Snapshot<N>) -> Self {
        let resolved = snapshot.graph.resolve();
        let mut prepared = HashMap::new();
        for step in snapshot.plan.steps() {
            let (Some(entry), Some(state)) = (snapshot.graph.get(step.id), resolved.get(&step.id)) else { continue };
            let controls = snapshot.controls.get(&step.id).map_or(&[][..], Vec::as_slice);
            if let Some(ready) = entry.node.prepare_to_record(&state.input_streams, controls) {
                prepared.insert(step.id, ready);
            }
        }
        Recording { prepared: Mutex::new(prepared) }
    }

    /// What node `id` prepared, if it did and it hasn't been taken.
    pub fn take(&self, id: NodeId) -> Option<Box<dyn Any + Send>> {
        self.prepared.lock().unwrap().remove(&id)
    }
}

/// A frozen version of the graph, with all its configuration (whether it's recording, too), and the plan for
/// running it. Undo steps back through snapshots, and each processing pass runs against one. Cheap to clone and to
/// share between threads.
pub struct Snapshot<N: NodeSet> {
    /// Increases with every snapshot taken.
    pub version: u64,
    pub graph: Arc<Graph<N>>,
    pub plan: Arc<Plan<N>>,
    /// What each node's controls see (for the passes; see `Pass::controls`).
    pub controls: Arc<ControlValues<N>>,
    /// The recording, while recording.
    pub recording: Option<Arc<Recording>>,
}

impl<N: NodeSet> Snapshot<N> {
    /// A snapshot of `graph`, planning how to run it, unless it processes the same as `previous` (when only
    /// positions or labels have changed, say), whose plan it shares. It records if `previous` does.
    pub fn new(version: u64, graph: Graph<N>, previous: Option<&Snapshot<N>>) -> Self {
        let (plan, controls) = match previous {
            Some(previous) if same_processing(&previous.graph, &graph) => {
                (previous.plan.clone(), previous.controls.clone())
            }
            _ => {
                let controls = graph
                    .iter()
                    .filter(|(_, entry)| !entry.node.controls().is_empty())
                    .map(|(id, _)| (id, graph.control_values(id)))
                    .collect();
                (Arc::new(Plan::new(&graph)), Arc::new(controls))
            }
        };
        let recording = previous.and_then(|p| p.recording.clone());
        Snapshot { version, graph: Arc::new(graph), plan, controls, recording }
    }

    /// This snapshot as version `version`, recording or not. Recording doesn't change how the graph runs, so the
    /// plan's the same, and a run carries on with it.
    pub fn with_recording(&self, version: u64, recording: Option<Arc<Recording>>) -> Self {
        Snapshot { version, recording, ..self.clone() }
    }
}

impl<N: NodeSet> Clone for Snapshot<N> {
    fn clone(&self) -> Self {
        Snapshot {
            version: self.version,
            graph: self.graph.clone(),
            plan: self.plan.clone(),
            controls: self.controls.clone(),
            recording: self.recording.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nodes;
    use crate::standard_components::{ephys, spikes};
    use crate::{DataStream, Setting};

    fn create(name: &str) -> Nodes {
        Nodes::create(Nodes::TYPES.iter().position(|t| t.name == name).unwrap())
    }

    fn port(node: NodeId, role: Role, index: usize) -> Port {
        Port { node, role, index }
    }

    #[test]
    fn data_streams_propagate() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let filter = graph.add(create("Band-pass Filter"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        graph.connect(port(board, Role::Output, 0), port(filter, Role::Input, 0)).unwrap();
        graph.connect(port(detector, Role::Input, 0), port(filter, Role::Output, 0)).unwrap();

        let resolved = graph.resolve();
        let continuous = ephys::Continuous { channels: 64, sample_rate: 30000.0 };
        assert_eq!(resolved[&filter].outputs[0].stream, Some(DataStream::Continuous(continuous)));
        assert!(matches!(resolved[&detector].outputs[0].stream, Some(DataStream::Spikes(s)) if s.channels == 64));

        graph.disconnect(graph.edge_into(port(filter, Role::Input, 0)).unwrap());
        let resolved = graph.resolve();
        assert_eq!(resolved[&detector].outputs[0].stream, None);
        assert!(!resolved[&detector].warnings.is_empty(), "unconnected input should warn");
    }

    #[test]
    fn rejects_bad_connections() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let gain = graph.add(create("Gain"), [0.0, 0.0]);
        let gain2 = graph.add(create("Gain"), [0.0, 0.0]);
        let video = graph.add(create("Video Viewer"), [0.0, 0.0]);
        let spikes = graph.add(create("Spike Viewer"), [0.0, 0.0]);

        let check = |a, b| graph.check(a, b).err();
        assert_eq!(check(port(gain, Role::Output, 0), port(gain, Role::Input, 0)), Some(ConnectError::SameNode));
        assert!(matches!(
            check(port(board, Role::Output, 0), port(video, Role::Input, 0)),
            Some(ConnectError::KindMismatch { dimension: Dimension::Data, .. })
        ));
        assert_eq!(check(port(board, Role::Output, 5), port(gain, Role::Input, 0)), Some(ConnectError::NoSuchPort));
        assert_eq!(check(port(board, Role::Output, 0), port(gain, Role::Output, 0)), Some(ConnectError::RoleMismatch));
        assert!(matches!(
            check(port(spikes, Role::Control, 0), port(gain, Role::Setting, 0)),
            Some(ConnectError::KindMismatch { dimension: Dimension::Settings, .. })
        ));

        graph.connect(port(gain, Role::Output, 0), port(gain2, Role::Input, 0)).unwrap();
        assert_eq!(
            graph.check(port(gain2, Role::Output, 0), port(gain, Role::Input, 0)).err(),
            Some(ConnectError::Cycle)
        );
    }

    #[test]
    fn connecting_replaces_the_existing_input() {
        let mut graph = Graph::default();
        let a = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let b = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let gain = graph.add(create("Gain"), [0.0, 0.0]);
        graph.connect(port(a, Role::Output, 0), port(gain, Role::Input, 0)).unwrap();
        let edge = graph.connect(port(b, Role::Output, 0), port(gain, Role::Input, 0)).unwrap();
        assert_eq!(graph.edges(), &[edge]);
    }

    #[test]
    fn settings_edges_may_point_upstream_and_edit_settings() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        let viewer = graph.add(create("Spike Viewer"), [0.0, 0.0]);
        graph.connect(port(board, Role::Output, 0), port(detector, Role::Input, 0)).unwrap();
        graph.connect(port(detector, Role::Output, 0), port(viewer, Role::Input, 0)).unwrap();
        // The viewer is downstream of the detector, but can still edit its thresholds.
        graph.connect(port(viewer, Role::Control, 0), port(detector, Role::Setting, 0)).unwrap();
        assert_eq!(graph.topological_order(), vec![board, detector, viewer]);

        let thresholds = |uv: Vec<f32>| Setting::Thresholds(spikes::Thresholds(uv));
        assert_eq!(graph.control_values(viewer), vec![Some(thresholds(vec![])), None]);
        let theme = &crate::standard_components::themes::THEMES[0].colours;
        graph.edit(viewer, &[], theme, None, false, false, |_, context| {
            context.controls[0] = Some(thresholds(vec![55.0]))
        });
        assert_eq!(graph.get(detector).unwrap().node.settings()[0].value, thresholds(vec![55.0]));

        // A setting can have several controls (an LTX writer reading the gains the viewer sets); a control, one
        // setting.
        let writer = graph.add(create("Write LTX"), [0.0, 0.0]);
        graph.connect(port(viewer, Role::Control, 1), port(detector, Role::Setting, 1)).unwrap();
        graph.connect(port(writer, Role::Control, 1), port(detector, Role::Setting, 1)).unwrap();
        let into_gains = |graph: &Graph<Nodes>| {
            graph.edges().iter().filter(|e| e.target() == port(detector, Role::Setting, 1)).count()
        };
        assert_eq!(into_gains(&graph), 2);
    }

    #[test]
    fn disabled_nodes_and_edges_run_as_if_they_were_not_there() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        let viewer = graph.add(create("Spike Viewer"), [0.0, 0.0]);
        let feed = graph.connect(port(board, Role::Output, 0), port(detector, Role::Input, 0)).unwrap();
        graph.connect(port(detector, Role::Output, 0), port(viewer, Role::Input, 0)).unwrap();
        let control = graph.connect(port(viewer, Role::Control, 0), port(detector, Role::Setting, 0)).unwrap();
        let streams = |graph: &Graph<Nodes>, id| graph.resolve()[&id].input_streams[0].is_some();
        let planned = |graph: &Graph<Nodes>, id| super::super::Plan::new(graph).steps().iter().any(|s| s.id == id);
        assert!(streams(&graph, viewer) && planned(&graph, detector));

        // A disabled node makes nothing, and doesn't run.
        graph.get_mut(detector).unwrap().disabled = true;
        assert!(!streams(&graph, viewer), "nothing downstream of it");
        assert!(!planned(&graph, detector));
        assert_eq!(graph.control_values(viewer)[0], None, "its settings can't be edited through it");
        graph.get_mut(detector).unwrap().disabled = false;

        // A disabled edge is as if it weren't connected, but is kept.
        graph.set_edge_disabled(feed, true);
        assert!(!streams(&graph, detector) && !streams(&graph, viewer));
        assert!(graph.edges().contains(&feed));
        graph.set_edge_disabled(feed, false);
        assert!(streams(&graph, viewer));
        graph.set_edge_disabled(control, true);
        assert_eq!(graph.control_values(viewer)[0], None);
        // And it changes how the graph runs.
        let before = graph.clone();
        graph.set_edge_disabled(control, false);
        assert!(!crate::core::engine::same_processing(&before, &graph));
    }

    #[test]
    fn labels_are_unique() {
        let mut graph = Graph::<Nodes>::default();
        let a = graph.add(create("Gain"), [0.0, 0.0]);
        let b = graph.add(create("Gain"), [0.0, 0.0]);
        assert_eq!(graph.get(a).unwrap().label, "Gain");
        assert_eq!(graph.get(b).unwrap().label, "Gain 2");
    }

    #[test]
    fn round_trips_through_json() {
        let graph = crate::example_graph();
        let json = serde_json::to_string(&graph).unwrap();
        let back: Graph<Nodes> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.edges(), graph.edges());
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
}
