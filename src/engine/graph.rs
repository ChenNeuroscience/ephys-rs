//! The graph of node instances and the edges between their ports.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{Context, Control, Dimension, EdgeInfo, EdgeSet, Edges, Input, NodeSet, Output, SettingPort};
use crate::theme::Colours;

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

/// A node's ports and signals, worked out from its settings and what's upstream of it.
pub struct NodeState<E: Edges> {
    pub inputs: Vec<Input<E>>,
    /// For each input, the signal connected to it, if any.
    pub input_signals: Vec<Option<E::Signal>>,
    pub outputs: Vec<Output<E>>,
    pub settings: Vec<SettingPort<E>>,
    pub controls: Vec<Control<E>>,
    pub warnings: Vec<String>,
}

/// Identifies an edge type: its dimension, and its index in that dimension's `EdgeSet::KINDS`.
pub type KindKey = (Dimension, usize);

impl<E: Edges> NodeState<E> {
    pub fn new<N: NodeSet<Edges = E>>(node: &N, input_signals: Vec<Option<E::Signal>>) -> Self {
        NodeState {
            inputs: node.inputs(),
            outputs: node.outputs(&input_signals),
            warnings: node.warnings(&input_signals),
            input_signals,
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
        let signal = |kind| (Dimension::Data, E::Signal::kind_index(kind));
        let setting = |kind| (Dimension::Settings, E::Setting::kind_index(kind));
        match role {
            Role::Input => self.inputs.get(index).map(|p| signal(p.kind)),
            Role::Output => self.outputs.get(index).map(|p| signal(p.kind)),
            Role::Setting => self.settings.get(index).map(|p| setting(p.value.kind())),
            Role::Control => self.controls.get(index).map(|p| setting(p.kind)),
        }
    }

    pub fn info(&self, role: Role, index: usize) -> Option<&'static EdgeInfo> {
        self.kind(role, index).map(edge_info::<E>)
    }

    /// The signal on an input or output, or the value of a setting.
    pub fn summary(&self, role: Role, index: usize) -> Option<String> {
        match role {
            Role::Input => self.input_signals.get(index).copied().flatten().map(|s| s.summary()),
            Role::Output => self.outputs.get(index)?.signal.map(|s| s.summary()),
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
        Dimension::Data => &E::Signal::KINDS[index],
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
}

impl<N> Default for Graph<N> {
    fn default() -> Self {
        Graph { next_id: 1, nodes: BTreeMap::new(), edges: Vec::new() }
    }
}

type Setting<N> = <<N as NodeSet>::Edges as Edges>::Setting;
type Signal<N> = <<N as NodeSet>::Edges as Edges>::Signal;

impl<N: NodeSet> Graph<N> {
    pub fn add(&mut self, node: N, pos: [f32; 2]) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let label = self.unique_label(node.info().name);
        self.nodes.insert(id, NodeEntry { node, label, pos });
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

    /// The edge into an input or setting, if any.
    pub fn edge_into(&self, port: Port) -> Option<Edge> {
        self.edges.iter().find(|e| e.target() == port).copied()
    }

    /// The edges out of an output or control.
    pub fn edges_from(&self, port: Port) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.source() == port).copied()
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

    /// Connects `a` and `b` (in either order), replacing anything already going into the input or setting.
    pub fn connect(&mut self, a: Port, b: Port) -> Result<Edge, ConnectError> {
        let edge = self.check(a, b)?;
        self.edges.retain(|e| e.target() != edge.target());
        self.edges.push(edge);
        Ok(edge)
    }

    pub fn disconnect(&mut self, edge: Edge) {
        self.edges.retain(|e| *e != edge);
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

    /// Works out every node's ports and signals, from the sources down.
    pub fn resolve(&self) -> Resolved<N::Edges> {
        let mut resolved: Resolved<N::Edges> = Resolved::new();
        for id in self.topological_order() {
            let node = &self.nodes[&id].node;
            let inputs = node.inputs();
            let input_signals: Vec<Option<Signal<N>>> = (0..inputs.len())
                .map(|index| {
                    let edge = self.edge_into(Port { node: id, role: Role::Input, index })?;
                    let signal = resolved.get(&edge.from.node)?.outputs.get(edge.from.port)?.signal?;
                    (signal.kind() == inputs[index].kind).then_some(signal)
                })
                .collect();
            resolved.insert(id, NodeState::new(node, input_signals));
        }
        resolved
    }

    /// For each of a node's controls, the current value of the setting it's connected to, if any.
    pub fn control_values(&self, id: NodeId) -> Vec<Option<Setting<N>>> {
        let Some(entry) = self.nodes.get(&id) else { return Vec::new() };
        (0..entry.node.controls().len())
            .map(|index| {
                let edge = self.edges_from(Port { node: id, role: Role::Control, index }).next()?;
                Some(self.nodes.get(&edge.to.node)?.node.settings().get(edge.to.port)?.value)
            })
            .collect()
    }

    /// Runs `f` (a node's UI, say) on a node, then applies any changes it made through its controls to the
    /// settings they're connected to.
    pub fn edit<R>(
        &mut self,
        id: NodeId,
        inputs: &[Option<Signal<N>>],
        theme: &Colours,
        f: impl FnOnce(&mut N, &mut Context<N::Edges>) -> R,
    ) -> Option<R> {
        let before = self.control_values(id);
        let mut controls = before.clone();
        let result = f(&mut self.nodes.get_mut(&id)?.node, &mut Context { inputs, controls: &mut controls, theme });
        for (index, (old, new)) in before.into_iter().zip(controls).enumerate() {
            let Some(new) = new.filter(|new| Some(*new) != old) else { continue };
            let targets: Vec<PortRef> =
                self.edges_from(Port { node: id, role: Role::Control, index }).map(|e| e.to).collect();
            for target in targets {
                let Some(entry) = self.nodes.get_mut(&target.node) else { continue };
                if entry.node.settings().get(target.port).is_some_and(|s| s.value.kind() == new.kind()) {
                    entry.node.set_setting(target.port, new);
                }
            }
        }
        Some(result)
    }
}

/// A frozen version of the graph, with all its configuration. Undo steps back through snapshots, and (in time)
/// each processing pass will run against one. Cheap to clone and to share between threads.
pub struct Snapshot<N> {
    /// Increases with every snapshot taken.
    pub version: u64,
    pub graph: Arc<Graph<N>>,
}

impl<N> Clone for Snapshot<N> {
    fn clone(&self) -> Self {
        Snapshot { version: self.version, graph: self.graph.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::{Setting, Signal, settings, signals};
    use crate::nodes::Nodes;

    fn create(name: &str) -> Nodes {
        Nodes::create(Nodes::TYPES.iter().position(|t| t.name == name).unwrap())
    }

    fn port(node: NodeId, role: Role, index: usize) -> Port {
        Port { node, role, index }
    }

    #[test]
    fn signals_propagate() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let filter = graph.add(create("Band-pass Filter"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        graph.connect(port(board, Role::Output, 0), port(filter, Role::Input, 0)).unwrap();
        graph.connect(port(detector, Role::Input, 0), port(filter, Role::Output, 0)).unwrap();

        let resolved = graph.resolve();
        let continuous = signals::Continuous { channels: 64, sample_rate: 30000.0 };
        assert_eq!(resolved[&filter].outputs[0].signal, Some(Signal::Continuous(continuous)));
        assert!(matches!(resolved[&detector].outputs[0].signal, Some(Signal::Spikes(s)) if s.channels == 64));

        graph.disconnect(graph.edge_into(port(filter, Role::Input, 0)).unwrap());
        let resolved = graph.resolve();
        assert_eq!(resolved[&detector].outputs[0].signal, None);
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
        // The viewer is downstream of the detector, but can still edit its threshold.
        graph.connect(port(viewer, Role::Control, 0), port(detector, Role::Setting, 0)).unwrap();
        assert_eq!(graph.topological_order(), vec![board, detector, viewer]);

        let threshold = |sd| Setting::Threshold(settings::Threshold { sd });
        assert_eq!(graph.control_values(viewer), vec![Some(threshold(4.0))]);
        graph.edit(viewer, &[], crate::theme::Theme::Dark.colours(), |_, context| {
            context.controls[0] = Some(threshold(5.5))
        });
        assert_eq!(graph.get(detector).unwrap().node.settings()[0].value, threshold(5.5));
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
        let graph = crate::nodes::example();
        let json = serde_json::to_string(&graph).unwrap();
        let back: Graph<Nodes> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.edges(), graph.edges());
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
}
