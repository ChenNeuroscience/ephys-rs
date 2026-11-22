//! The app edits a graph of nodes and the edges between their ports, saves it, and undoes changes to it; a run works
//! from snapshots of it. This module has the graph, the snapshots, and the recordings they carry.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{
    CommandContext, CommandSource, CommandsEnum, DataStreamInOf, DataStreamInPorts, DataStreamOutPorts, EdgeInfo,
    EdgeTypesEnum, Edges, Node, NodesEnum, Plan, Port, PortAddress, PortKind, PortRole, PortSet, UiContext,
    ValOutCmdInOf, ValOutCmdInPorts,
};
use crate::commands::{Address, Commands};
use crate::theme::Colours;

/// The graph names each node with an id, unique within the graph and never reused, so it's the same node through undo,
/// redo and saving.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// An edge connects a `DataStreamOut` to a `DataStreamIn`, or a `ValInCmdOut` or `ValIn` to a `ValOutCmdIn`, each end
/// saved as the port's address (its node, role and index among the node's ports of that role). A `DataStreamIn` has
/// at most one edge into it, and a `DataStreamOut` any number out of it; a `ValInCmdOut` or `ValIn` has at most one
/// edge out of it, and a `ValOutCmdIn` any number into it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub from: PortAddress,
    pub to: PortAddress,
}

impl Edge {
    /// Whether the edge carries a data stream (rather than a value and commands), by the roles of its ports.
    pub fn carries_data_stream(&self) -> bool {
        self.from.role.carries_data_stream()
    }
}

/// A node numbers its `ValInCmdOut` and `ValIn` ports together, in the order it declares them (`ports`, of all it
/// declares), as the ports it reads values through. The graph numbers each role's apart. This is the graph's role and
/// index for the node's port `val_in_idx`.
pub fn val_in_address<E: Edges>(ports: &[Port<E>], val_in_idx: usize) -> Option<(PortRole, usize)> {
    let reading = ports.iter().filter(|p| p.role() != PortRole::ValOutCmdIn);
    let role = reading.clone().nth(val_in_idx)?.role();
    Some((role, reading.take(val_in_idx).filter(|p| p.role() == role).count()))
}

/// The port of `ports` (of all a node declares) at `role`'s index `port_idx`.
pub fn port_at<E: Edges>(ports: &[Port<E>], role: PortRole, port_idx: usize) -> Option<&Port<E>> {
    ports.iter().filter(|p| p.role() == role).nth(port_idx)
}

/// Besides a node's settings, the graph keeps its label, where it is, and whether it's disabled or shown in the basic
/// view. This is a node, with all that.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeEntry<N> {
    pub node: N,
    /// The graph shows the node's label on it. The label's unique when the node is added, though people can rename it
    /// to anything.
    pub label: String,
    /// The node's top-left corner on the canvas.
    pub pos: [f32; 2],
    /// Someone can set whether the node's listed in the basic view; otherwise its type decides (see
    /// `Node::SHOW_IN_BASIC_DEFAULT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_in_basic_override: Option<bool>,
    /// A disabled node is kept, but runs as if it weren't there (see `Graph::is_live`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl<N: NodesEnum> NodeEntry<N> {
    /// The basic view lists the node if someone set that, or otherwise if its type says to.
    pub fn shown_in_basic(&self) -> bool {
        self.show_in_basic_override.unwrap_or(self.node.show_in_basic_default())
    }
}

/// Some ports can't be connected; this says why.
#[derive(Debug, PartialEq)]
pub enum ConnectError {
    NoSuchPort,
    SameNode,
    /// Only a `DataStreamOut` and a `DataStreamIn`, or a `ValInCmdOut` or `ValIn` and a `ValOutCmdIn`, can be
    /// connected.
    RoleMismatch,
    /// A `DataStreamOut` of one kind of data stream can't feed a `DataStreamIn` of another.
    DataStreamKindMismatch {
        from: &'static str,
        to: &'static str,
    },
    /// A `ValInCmdOut` or `ValIn` can't read a `ValOutCmdIn` of another kind of value.
    ValueKindMismatch {
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
            ConnectError::DataStreamKindMismatch { from, to } => write!(f, "a {from} output can't feed a {to} input"),
            ConnectError::ValueKindMismatch { from, to } => write!(f, "a {from} control can't edit a {to} setting"),
            ConnectError::Cycle => write!(f, "that would make a loop"),
        }
    }
}

/// The graph works out each node's ports and data streams from the node's settings and what's upstream of it (see
/// `Graph::resolve`). This is one node's.
pub struct NodeState<E: Edges> {
    pub inputs: Vec<Port<E>>,
    /// For each input, the data stream connected to it, if any.
    pub input_streams: Vec<Option<E::DataStream>>,
    pub outputs: Vec<Port<E>>,
    /// The node's `ValOutCmdIn` ports, in order, and what it publishes at each (see `Node::val_out`).
    pub val_outs: Vec<Port<E>>,
    pub values: Vec<E::PublishedValue>,
    /// The node's `ValInCmdOut` and `ValIn` ports, in the order it declares them (as it numbers them: see
    /// `val_in_address`).
    pub val_ins: Vec<Port<E>>,
    pub warnings: Vec<String>,
}

impl<E: Edges> NodeState<E> {
    /// The state of `node`, given the data streams on its inputs.
    pub fn of<N: Node<E>>(node: &N, input_streams: Vec<Option<E::DataStream>>) -> Self {
        let inputs = DataStreamInOf::<N, E>::of(&input_streams);
        NodeState {
            inputs: DataStreamInOf::<N, E>::ports(),
            outputs: node.output_streams(&inputs).ports(),
            warnings: node.warnings(&inputs),
            input_streams,
            values: node.val_out().values(),
            val_outs: ValOutCmdInOf::<N, E>::ports(),
            val_ins: N::Ports::val_in_ports(),
        }
    }

    pub fn new<N: NodesEnum<Edges = E>>(node: &N, input_streams: Vec<Option<E::DataStream>>) -> Self {
        let (val_outs, val_ins): (Vec<_>, Vec<_>) =
            node.list_ports_val_cmd().into_iter().partition(|p| p.role() == PortRole::ValOutCmdIn);
        NodeState {
            inputs: node.list_ports_data_stream_in(),
            outputs: node.list_ports_data_stream_out(&input_streams),
            warnings: node.warnings(&input_streams),
            input_streams,
            values: (0..val_outs.len()).map(|port| node.val_out(port)).collect(),
            val_outs,
            val_ins,
        }
    }

    /// The graph draws each port on one side of the node, so wires leave it that way. This says whether it's the
    /// right.
    pub fn on_right(&self, role: PortRole, port_idx: usize) -> bool {
        match role {
            PortRole::DataStreamOut => true,
            PortRole::DataStreamIn => false,
            PortRole::ValOutCmdIn => self.val_outs.get(port_idx).is_some_and(|p| !p.is_flipped()),
            PortRole::ValInCmdOut | PortRole::ValIn => self.val_port(role, port_idx).is_some_and(|p| p.is_flipped()),
        }
    }

    /// The state of `node` with nothing connected.
    pub fn unconnected<N: NodesEnum<Edges = E>>(node: &N) -> Self {
        Self::new(node, vec![None; node.list_ports_data_stream_in().len()])
    }

    /// How many ports of `role` the node has.
    pub fn count(&self, role: PortRole) -> usize {
        match role {
            PortRole::DataStreamIn => self.inputs.len(),
            PortRole::DataStreamOut => self.outputs.len(),
            PortRole::ValOutCmdIn => self.val_outs.len(),
            PortRole::ValInCmdOut | PortRole::ValIn => self.val_ins.iter().filter(|p| p.role() == role).count(),
        }
    }

    /// The ValCmd port at `(role, port_idx)`, if it's one.
    pub fn val_port(&self, role: PortRole, port_idx: usize) -> Option<&Port<E>> {
        match role {
            PortRole::ValOutCmdIn => self.val_outs.get(port_idx),
            PortRole::ValInCmdOut | PortRole::ValIn => port_at(&self.val_ins, role, port_idx),
            PortRole::DataStreamIn | PortRole::DataStreamOut => None,
        }
    }

    /// The graph's role and index for the node's `ValInCmdOut` or `ValIn` port `val_in_idx` (see `val_in_address`).
    pub fn val_in_address(&self, val_in_idx: usize) -> Option<(PortRole, usize)> {
        val_in_address(&self.val_ins, val_in_idx)
    }

    pub fn name(&self, role: PortRole, port_idx: usize) -> Option<&'static str> {
        match role {
            PortRole::DataStreamIn => self.inputs.get(port_idx).map(|p| p.name()),
            PortRole::DataStreamOut => self.outputs.get(port_idx).map(|p| p.name()),
            _ => self.val_port(role, port_idx).map(|p| p.name()),
        }
    }

    pub fn kind(&self, role: PortRole, port_idx: usize) -> Option<PortKind> {
        let stream = |kind| PortKind::DataStream(E::DataStream::kind_index(kind));
        let value = |kind| PortKind::Value(E::PublishedValue::kind_index(kind));
        match role {
            PortRole::DataStreamIn => self.inputs.get(port_idx).and_then(Port::data_stream_kind).map(stream),
            PortRole::DataStreamOut => self.outputs.get(port_idx).and_then(Port::data_stream_kind).map(stream),
            _ => self.val_port(role, port_idx).and_then(Port::value_kind).map(value),
        }
    }

    pub fn info(&self, role: PortRole, port_idx: usize) -> Option<&'static EdgeInfo> {
        self.kind(role, port_idx).map(edge_info::<E>)
    }

    /// The graph and the node's window show what's on a port: the data stream on an input or output, or the value the
    /// node publishes.
    pub fn summary(&self, role: PortRole, port_idx: usize) -> Option<String> {
        match role {
            PortRole::DataStreamIn => self.input_streams.get(port_idx).copied().flatten().map(|s| s.summary()),
            PortRole::DataStreamOut => self.outputs.get(port_idx)?.stream().map(|s| s.summary()),
            PortRole::ValOutCmdIn => self.values.get(port_idx).map(|v| v.summary()),
            PortRole::ValInCmdOut | PortRole::ValIn => None,
        }
    }

    /// The node's ports, as the graph draws them: its outputs, inputs, `ValOutCmdIn` ports, then its `ValInCmdOut` and
    /// `ValIn` ports in the order it declares them.
    pub fn ports(&self, node: NodeId) -> impl Iterator<Item = PortAddress> + '_ {
        let of = move |role| (0..self.count(role)).map(move |port_idx| PortAddress { node, role, port_idx });
        let reading = (0..self.val_ins.len()).filter_map(move |val_in_idx| {
            let (role, port_idx) = self.val_in_address(val_in_idx)?;
            Some(PortAddress { node, role, port_idx })
        });
        of(PortRole::DataStreamOut).chain(of(PortRole::DataStreamIn)).chain(of(PortRole::ValOutCmdIn)).chain(reading)
    }

    /// Two ports fit if their roles pair up and they're of the same kind. This says whether this node's port
    /// `(role, port_idx)` fits `other`'s port `to`.
    pub fn fits(&self, (role, port_idx): (PortRole, usize), other: &Self, to: (PortRole, usize)) -> bool {
        let kind = self.kind(role, port_idx);
        role.pairs_with(to.0) && kind.is_some() && kind == other.kind(to.0, to.1)
    }

    /// When a wire's dropped on a node, it connects to the first of the node's ports that fits (see `fits`). This finds
    /// it, for `from`'s port `port`.
    pub fn find(&self, node: NodeId, from: &Self, port: (PortRole, usize)) -> Option<PortAddress> {
        let fitting =
            |&role: &PortRole| (0..self.count(role)).find(|&i| from.fits(port, self, (role, i))).map(|i| (role, i));
        let (role, port_idx) = port.0.partners().iter().find_map(fitting)?;
        Some(PortAddress { node, role, port_idx })
    }
}

pub fn edge_info<E: Edges>(kind: PortKind) -> &'static EdgeInfo {
    match kind {
        PortKind::DataStream(index) => &E::DataStream::KINDS[index],
        PortKind::Value(index) => &E::PublishedValue::KINDS[index],
    }
}

/// The graph works out every node's state (see `NodeState`): its ports, and the data streams on them, from its settings
/// and what's upstream.
pub type Resolved<E> = HashMap<NodeId, NodeState<E>>;

/// The app edits, saves and undoes the nodes and the edges between them, held in this. A run works from snapshots of it
/// (see `Snapshot`), so the graph's never read from more than one thread.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "N: NodesEnum")]
pub struct Graph<N> {
    next_id: u64,
    nodes: BTreeMap<NodeId, NodeEntry<N>>,
    edges: Vec<Edge>,
    /// A disabled edge is kept, but runs as if it weren't there; these are the disabled edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disabled_edges: Vec<Edge>,
}

impl<N> Default for Graph<N> {
    fn default() -> Self {
        Graph { next_id: 1, nodes: BTreeMap::new(), edges: Vec::new(), disabled_edges: Vec::new() }
    }
}

type PublishedValue<N> = <<N as NodesEnum>::Edges as Edges>::PublishedValue;
type DataStream<N> = <<N as NodesEnum>::Edges as Edges>::DataStream;

impl<N: NodesEnum> Graph<N> {
    pub fn add(&mut self, node: N, pos: [f32; 2]) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let label = self.unique_label(node.info().name);
        self.nodes.insert(id, NodeEntry { node, label, pos, show_in_basic_override: None, disabled: false });
        id
    }

    /// A node added is labelled `base`, or "`base` 2", "`base` 3"... whichever is first not already taken.
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

    /// The app has every node drop what it was asked to do only for now (see `Node::clear_transient_state`)
    /// with this.
    pub fn clear_transient_state(&mut self) {
        self.nodes.values_mut().for_each(clear_transient_state);
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
        self.edges.iter().filter(|e| e.carries_data_stream())
    }

    /// A `DataStreamIn` has at most one edge into it. This finds it, enabled or not (or, for a `ValOutCmdIn`, the
    /// first).
    pub fn edge_into(&self, port: PortAddress) -> Option<Edge> {
        self.edges.iter().find(|e| e.to == port).copied()
    }

    /// A `DataStreamOut` can have any number of edges out of it, and a `ValInCmdOut` one. This finds them, enabled or
    /// not.
    pub fn edges_from(&self, port: PortAddress) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.from == port).copied()
    }

    /// An edge counts, as the graph runs, if neither it nor the nodes at its ends are disabled. This says whether it
    /// does.
    pub fn is_live(&self, edge: &Edge) -> bool {
        let enabled = |id: NodeId| self.nodes.get(&id).is_some_and(|e| !e.disabled);
        !self.disabled_edges.contains(edge) && enabled(edge.from.node) && enabled(edge.to.node)
    }

    /// This says whether the edge itself is disabled.
    pub fn is_edge_disabled(&self, edge: &Edge) -> bool {
        self.disabled_edges.contains(edge)
    }

    /// The canvas disables an edge, or enables it, with this.
    pub fn set_edge_disabled(&mut self, edge: Edge, disabled: bool) {
        self.disabled_edges.retain(|e| *e != edge);
        if disabled && self.edges.contains(&edge) {
            self.disabled_edges.push(edge);
        }
    }

    /// The edge into a `DataStreamIn`, if there's one and it counts (see `is_live`).
    pub fn live_edge_into(&self, port: PortAddress) -> Option<Edge> {
        self.edges.iter().find(|e| e.to == port && self.is_live(e)).copied()
    }

    /// The edges out of a `DataStreamOut` or `ValInCmdOut` that count (see `is_live`).
    pub fn live_edges_from(&self, port: PortAddress) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.from == port && self.is_live(e)).copied()
    }

    /// A node's window says which nodes read and command what it publishes: for each of its `ValOutCmdIn` ports, the
    /// labels of the nodes with a live edge to it.
    fn controlled_by(&self, id: NodeId) -> Vec<Vec<String>> {
        let count = self.nodes.get(&id).map_or(0, |entry| {
            entry.node.list_ports_val_cmd().iter().filter(|p| p.role() == PortRole::ValOutCmdIn).count()
        });
        let labels = |index| {
            let port = PortAddress { node: id, role: PortRole::ValOutCmdIn, port_idx: index };
            let edges = self.edges.iter().filter(|e| e.to == port && self.is_live(e));
            edges.filter_map(|e| Some(self.nodes.get(&e.from.node)?.label.clone())).collect()
        };
        (0..count).map(labels).collect()
    }

    fn kind(&self, port: PortAddress) -> Option<PortKind> {
        NodeState::unconnected(&self.nodes.get(&port.node)?.node).kind(port.role, port.port_idx)
    }

    /// Before connecting two ports, the graph checks they can be. This returns the edge that would connect `a` and `b`
    /// (in either order), or why they can't be; what it would replace is up to `connect`.
    pub fn check(&self, a: PortAddress, b: PortAddress) -> Result<Edge, ConnectError> {
        if !a.role.pairs_with(b.role) {
            return Err(ConnectError::RoleMismatch);
        }
        let (from, to) = if a.role.is_source() { (a, b) } else { (b, a) };
        let (Some(from_kind), Some(to_kind)) = (self.kind(from), self.kind(to)) else {
            return Err(ConnectError::NoSuchPort);
        };
        if from.node == to.node {
            return Err(ConnectError::SameNode);
        }
        let state = |port: PortAddress| NodeState::unconnected(&self.nodes[&port.node].node);
        if !state(from).fits((from.role, from.port_idx), &state(to), (to.role, to.port_idx)) {
            let name = |kind| edge_info::<N::Edges>(kind).name;
            let (from, to) = (name(from_kind), name(to_kind));
            let data_stream = from_kind.carries_data_stream();
            return Err(if data_stream {
                ConnectError::DataStreamKindMismatch { from, to }
            } else {
                ConnectError::ValueKindMismatch { from, to }
            });
        }
        // ValCmd edges carry no data, so may point anywhere; only data edges must not loop.
        if from.role.carries_data_stream() && self.reaches(to.node, from.node) {
            return Err(ConnectError::Cycle);
        }
        Ok(Edge { from, to })
    }

    /// The canvas connects `a` and `b` (in either order) with this, replacing anything already going into the
    /// `DataStreamIn`, or what the `ValInCmdOut` or `ValIn` was connected to. (A `DataStreamIn` has one source, but a
    /// `ValOutCmdIn` can have many edges into it: one from a `ValInCmdOut`, say, and others from `ValIn`s. A
    /// `ValInCmdOut` or `ValIn` reads just the one.)
    pub fn connect(&mut self, a: PortAddress, b: PortAddress) -> Result<Edge, ConnectError> {
        let edge = self.check(a, b)?;
        self.edges.retain(|e| if edge.carries_data_stream() { e.to != edge.to } else { e.from != edge.from });
        let edges = &self.edges;
        self.disabled_edges.retain(|e| edges.contains(e));
        self.edges.push(edge);
        Ok(edge)
    }

    pub fn disconnect(&mut self, edge: Edge) {
        self.edges.retain(|e| *e != edge);
        self.disabled_edges.retain(|e| *e != edge);
    }

    /// While a wire's dragged from `port`, the canvas shows the ports, on every node, that it could connect to.
    pub fn targets(&self, port: PortAddress) -> Vec<PortAddress> {
        let mut targets = Vec::new();
        for (&id, entry) in &self.nodes {
            let state = NodeState::unconnected(&entry.node);
            for &role in port.role.partners() {
                let ports = (0..state.count(role)).map(|port_idx| PortAddress { node: id, role, port_idx });
                targets.extend(ports.filter(|t| self.check(port, *t).is_ok()));
            }
        }
        targets
    }

    /// Data edges can't make a loop, so the graph checks whether `to` is downstream of `from` (or is `from`).
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

    /// A change of settings can remove a port, or change what it carries. The graph drops edges whose ports no longer
    /// exist or no longer match, with this.
    pub fn prune_edges(&mut self) {
        let fits = |e: &Edge| {
            let (from, to) = (e.from, e.to);
            let (Some(a), Some(b)) = (self.nodes.get(&from.node), self.nodes.get(&to.node)) else { return false };
            let (a, b) = (NodeState::unconnected(&a.node), NodeState::unconnected(&b.node));
            a.fits((from.role, from.port_idx), &b, (to.role, to.port_idx))
        };
        let valid: Vec<bool> = self.edges.iter().map(fits).collect();
        let mut valid = valid.into_iter();
        self.edges.retain(|_| valid.next().unwrap());
        let edges = &self.edges;
        self.disabled_edges.retain(|e| edges.contains(e));
    }

    /// The graph orders its nodes so that each comes after everything feeding it data. Nodes in a loop (only possible
    /// in a hand-edited file) come last, in id order.
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

    /// A node's ports can depend on its settings, and its outputs' data streams on what's connected to its inputs. This
    /// works out every node's, from the sources down.
    pub fn resolve(&self) -> Resolved<N::Edges> {
        let mut resolved: Resolved<N::Edges> = Resolved::new();
        let in_loops = self.command_loops();
        for id in self.topological_order() {
            let entry = &self.nodes[&id];
            let node = &entry.node;
            let inputs = node.list_ports_data_stream_in();
            let input_streams: Vec<Option<DataStream<N>>> = (0..inputs.len())
                .map(|index| {
                    let edge =
                        self.live_edge_into(PortAddress { node: id, role: PortRole::DataStreamIn, port_idx: index })?;
                    let stream = resolved.get(&edge.from.node)?.outputs.get(edge.from.port_idx)?.stream()?;
                    (inputs[index].data_stream_kind() == Some(stream.kind())).then_some(stream)
                })
                .collect();
            let mut state = NodeState::new(node, input_streams);
            if in_loops.contains(&id) {
                state.warnings.push(
                    "In a loop of commands: what it sets off can set it off again, over and over (Stop ends it)".into(),
                );
            }
            // A disabled node makes nothing, and has nothing to warn about.
            if entry.disabled {
                for output in &mut state.outputs {
                    if let Port::DataStreamOut { stream, .. } = output {
                        *stream = None;
                    }
                }
                state.warnings.clear();
            }
            resolved.insert(id, state);
        }
        resolved
    }

    /// Live ValCmd edges carrying commands (a command the near end sends that the far end accepts) can make a loop, so
    /// what a node sets off can, in the end, set it off again (Run Commands running each other, say). The canvas warns
    /// of the nodes in such a loop, which this finds.
    pub fn command_loops(&self) -> std::collections::HashSet<NodeId> {
        let mut next: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in self.edges.iter().filter(|e| !e.carries_data_stream() && self.is_live(e)) {
            let ports = |id: NodeId| self.nodes.get(&id).map(|n| n.node.list_ports_val_cmd()).unwrap_or_default();
            let (from, to) = (ports(edge.from.node), ports(edge.to.node));
            let commands =
                |ports, end: PortAddress| port_at(ports, end.role, end.port_idx).map_or(&[][..], Port::commands);
            let (sends, accepts) = (commands(&from, edge.from), commands(&to, edge.to));
            if sends.iter().any(|kind| accepts.contains(kind)) {
                next.entry(edge.from.node).or_default().push(edge.to.node);
            }
        }
        // A node's in a loop if it can be reached from what it leads to.
        let reaches_itself = |start: NodeId| {
            let mut seen = std::collections::HashSet::new();
            let mut stack: Vec<NodeId> = next.get(&start).cloned().unwrap_or_default();
            while let Some(id) = stack.pop() {
                if id == start {
                    return true;
                }
                if seen.insert(id) {
                    stack.extend(next.get(&id).into_iter().flatten().copied());
                }
            }
            false
        };
        next.keys().copied().filter(|&id| reaches_itself(id)).collect()
    }

    /// A node reads values through its `ValInCmdOut` and `ValIn` ports. This gives, for each of node `id`'s, what the
    /// node at the other end publishes there now, if the port's connected (by a live edge).
    pub fn val_ins(&self, id: NodeId) -> Vec<Option<PublishedValue<N>>> {
        let Some(entry) = self.nodes.get(&id) else { return Vec::new() };
        let count = entry.node.list_ports_val_cmd().iter().filter(|p| p.role() != PortRole::ValOutCmdIn).count();
        (0..count)
            .map(|val_in_idx| {
                let to = self.val_in_target(id, val_in_idx)?;
                let target = &self.nodes.get(&to.node)?.node;
                let val_outs = target.list_ports_val_cmd().iter().filter(|p| p.role() == PortRole::ValOutCmdIn).count();
                (to.port_idx < val_outs).then(|| target.val_out(to.port_idx))
            })
            .collect()
    }

    /// Node `from`'s `ValInCmdOut` or `ValIn` port `val_in_idx` (as the node numbers them: see `val_in_address`)
    /// connects to at most one `ValOutCmdIn`: this finds it, if the edge is live.
    pub fn val_in_target(&self, from: NodeId, val_in_idx: usize) -> Option<PortAddress> {
        let (role, port_idx) = val_in_address(&self.nodes.get(&from)?.node.list_ports_val_cmd(), val_in_idx)?;
        self.live_edges_from(PortAddress { node: from, role, port_idx }).next().map(|e| e.to)
    }

    /// A command's delivered to a node, from where it came (see `CommandSource`): this finds them, for one sent to
    /// `address`, following the edge now. A command sent through a port that's no longer connected goes nowhere.
    pub fn command_target(&self, address: Address) -> Option<(NodeId, CommandSource)> {
        match address {
            Address::Port { from, val_in_idx } => {
                let to = self.val_in_target(from, val_in_idx)?;
                Some((to.node, CommandSource::Port(to.port_idx)))
            }
            Address::Itself(node) => self.nodes.contains_key(&node).then_some((node, CommandSource::Itself)),
        }
    }

    /// The app delivers `command` to node `to`, from `from`, with this. The node's `ui_state` is where it keeps work it
    /// starts, `commands` where it sends commands in turn, and `running` whether the graph's running. A command
    /// through a port that doesn't accept it, or to a disabled node, isn't delivered (the app logs it), and this
    /// returns false.
    #[allow(clippy::too_many_arguments)]
    pub fn command(
        &mut self,
        to: NodeId,
        from: CommandSource,
        command: <N::Edges as Edges>::Command,
        ui_state: Option<&crate::engine::UiStateCell>,
        commands: Option<&Arc<Commands>>,
        running: bool,
    ) -> bool {
        let val_ins = self.val_ins(to);
        let Some(entry) = self.nodes.get_mut(&to) else { return false };
        if entry.disabled {
            log::info!("{} is disabled: {:?} not delivered", entry.label, command.kind());
            return false;
        }
        if let CommandSource::Port(port_idx) = from {
            let ports = entry.node.list_ports_val_cmd();
            let accepts = ports
                .iter()
                .filter(|p| p.role() == PortRole::ValOutCmdIn)
                .nth(port_idx)
                .is_some_and(|p| p.commands().contains(&command.kind()));
            if !accepts {
                log::warn!("{}'s port_idx {port_idx} doesn't accept {:?}: not delivered", entry.label, command.kind());
                return false;
            }
        }
        let context = &mut CommandContext::new(to, &entry.label, &val_ins, ui_state, commands, running);
        match entry.node.on_command(from, command, context) {
            Ok(()) => true,
            Err(command) => {
                log::warn!("{} doesn't take {:?}: not delivered", entry.label, command.kind());
                false
            }
        }
    }

    /// The app runs `f` (a node's window, say) on a node with this. The commands the window sends through its ports
    /// go to `commands`, for the app to deliver, or, where there's no `commands` (a test, say), straight to the nodes
    /// the ports connect to, as `f` returns.
    #[allow(clippy::too_many_arguments)]
    pub fn edit<R>(
        &mut self,
        id: NodeId,
        inputs: &[Option<DataStream<N>>],
        theme: &Colours,
        shared_by_node: Option<&super::SharedByNode>,
        ui_state: Option<&crate::engine::UiStateCell>,
        base_directory: Option<&crate::base_directory::Place>,
        running: bool,
        locked: bool,
        tools: Option<&mut dyn super::NodeToolbar>,
        f: impl FnOnce(&mut N, &mut UiContext<N::Edges>) -> R,
    ) -> Option<R> {
        self.edit_sending(id, inputs, theme, shared_by_node, ui_state, base_directory, running, locked, tools, None, f)
    }

    /// The same as `edit`, sending the window's commands to `commands`.
    #[allow(clippy::too_many_arguments)]
    pub fn edit_sending<R>(
        &mut self,
        id: NodeId,
        inputs: &[Option<DataStream<N>>],
        theme: &Colours,
        shared_by_node: Option<&super::SharedByNode>,
        ui_state: Option<&crate::engine::UiStateCell>,
        base_directory: Option<&crate::base_directory::Place>,
        running: bool,
        locked: bool,
        tools: Option<&mut dyn super::NodeToolbar>,
        commands: Option<&Arc<Commands>>,
        f: impl FnOnce(&mut N, &mut UiContext<N::Edges>) -> R,
    ) -> Option<R> {
        let val_ins = self.val_ins(id);
        let controlled_by = self.controlled_by(id);
        let entry = self.nodes.get_mut(&id)?;
        // A fresh one, if the caller doesn't keep it (a test, say).
        let fresh;
        let ui_state = match ui_state {
            Some(ui_state) => ui_state,
            None => {
                fresh = std::cell::RefCell::new(entry.node.new_ui_state());
                &fresh
            }
        };
        let mut sent = Vec::new();
        let context = &mut UiContext {
            inputs,
            val_ins: &val_ins,
            controlled_by: &controlled_by,
            theme,
            node: id,
            label: &mut entry.label,
            shared_by_node,
            ui_state,
            base_directory,
            running,
            locked,
            commands,
            sent: &mut sent,
            // Reborrowed, to live as long as the context.
            tools: match tools {
                Some(tools) => Some(&mut *tools),
                None => None,
            },
        };
        let result = f(&mut entry.node, context);
        for (val_in_idx, command) in sent {
            if let Some(commands) = commands {
                commands.send(Address::Port { from: id, val_in_idx }, command);
                continue;
            }
            match self.command_target(Address::Port { from: id, val_in_idx }) {
                Some((to, from)) => {
                    self.command(to, from, command, None, None, running);
                }
                None => log::info!("Port {val_in_idx} isn't connected: {:?} not sent", command.kind()),
            }
        }
        Some(result)
    }
}

/// A snapshot keeps, for each node with `ValInCmdOut` or `ValIn` ports, the value at the other end of each, if it's
/// connected.
pub type ValIns<N> = HashMap<NodeId, Vec<Option<PublishedValue<N>>>>;

/// When Record's pressed (or a node asks), the app makes a recording: its base directory (see
/// `crate::base_directory`), when it was asked for, and once it has, when it took effect. Its nodes prepare for it as
/// they do for any snapshot (see `Node::prepare_for_snapshot`): a snapshot recording it doesn't run until they have,
/// the one before carrying on meanwhile.
pub struct Recording {
    /// The recording's written in a base directory, if there is one.
    base_directory: Option<crate::base_directory::Place>,
    /// Record was pressed (or a node asked) at this time, by the calendar, which names and dates the files.
    start_requested_date: chrono::DateTime<chrono::Local>,
    /// The recording takes effect when the first pass recording it starts (see `take_effect`).
    effective: std::sync::OnceLock<Effective>,
}

/// A recording takes effect at the start of the time covered by its first pass (as opposed to when it was asked for).
/// This is that time, by the monotonic clock (for durations) and by the calendar (for files' headers).
#[derive(Clone, Copy, Debug)]
pub struct Effective {
    pub at: Instant,
    pub date: chrono::DateTime<chrono::Local>,
}

impl Default for Recording {
    /// Tests try nodes out with a recording asked for now, with no base directory.
    fn default() -> Self {
        Recording::new(None, chrono::Local::now())
    }
}

impl Recording {
    /// The app makes a recording asked for at `start_requested_date`, in `base_directory` (if there is one), with this.
    /// Everything that names or dates the recording goes by that one time.
    pub fn new(
        base_directory: Option<crate::base_directory::Place>,
        start_requested_date: chrono::DateTime<chrono::Local>,
    ) -> Self {
        Recording { base_directory, start_requested_date, effective: Default::default() }
    }

    /// The recording's base directory, if there is one.
    pub fn base_directory(&self) -> Option<&crate::base_directory::Place> {
        self.base_directory.as_ref()
    }

    /// The recording was asked for at this time, by the calendar (see `new`).
    pub fn start_requested_date(&self) -> &chrono::DateTime<chrono::Local> {
        &self.start_requested_date
    }

    /// Once the recording's taken effect (a pass recording it has been sent), the time it did.
    pub fn effective(&self) -> Option<&Effective> {
        self.effective.get()
    }

    /// As the dispatcher (or a test) sends the first pass of a snapshot recording it, the recording takes effect at
    /// `at` (the start of the time that pass covers), if it hasn't already.
    pub fn take_effect(&self, at: Instant) {
        self.effective.get_or_init(|| {
            let ago = chrono::Duration::from_std(Instant::now().saturating_duration_since(at)).unwrap_or_default();
            Effective { at, date: chrono::Local::now() - ago }
        });
    }
}

/// The app has a node drop what it was asked to do only for now (see `Node::clear_transient_state`) with this: when it
/// duplicates the node, say. The node isn't edited otherwise.
pub fn clear_transient_state<N: NodesEnum>(entry: &mut NodeEntry<N>) {
    entry.node.clear_transient_state();
}

/// The passes need to know the graph, with all its settings, the values its nodes read, the plan for running it, and
/// the run's times and recording. The app bakes all that into a snapshot (see `bake`), and bakes a new one whenever any
/// of it changes. Each pass runs against one, and gives node code what it can see of it (`SnapshotLite`, see
/// `PassContext::snapshot`). A snapshot's frozen, so it's cheap to clone and to share between threads.
pub struct Snapshot<N: NodesEnum> {
    /// The app numbers the snapshots it bakes, in order.
    pub version: u64,
    pub graph: Arc<Graph<N>>,
    pub plan: Arc<Plan<N>>,
    /// The values each node reads, for the passes (see `PassContext::val_in`).
    pub val_ins: Arc<ValIns<N>>,
    /// While running, the run started (Play pressed) at this time, by the monotonic clock.
    pub started: Option<Instant>,
    /// While recording, the recording.
    pub recording: Option<Arc<Recording>>,
}

impl<N: NodesEnum> Snapshot<N> {
    /// The app bakes what the passes need of `graph` with this, for a run that `started` (while running), with
    /// `recording` (while recording), planning how to run it from scratch.
    pub fn bake(
        version: u64,
        graph: Arc<Graph<N>>,
        started: Option<Instant>,
        recording: Option<Arc<Recording>>,
    ) -> Self {
        let val_ins = graph
            .iter()
            .filter(|(_, entry)| entry.node.list_ports_val_cmd().iter().any(|p| p.role() != PortRole::ValOutCmdIn))
            .map(|(id, _)| (id, graph.val_ins(id)))
            .collect();
        let plan = Arc::new(Plan::new(&graph, recording.is_some()));
        Snapshot { version, graph, plan, val_ins: Arc::new(val_ins), started, recording }
    }

    /// Tests try graphs out with a snapshot of `graph` as a run started just now would have it, not recording. (The
    /// app bakes its own.)
    pub fn new(version: u64, graph: Graph<N>) -> Self {
        Self::bake(version, Arc::new(graph), Some(Instant::now()), None)
    }
}

impl<N: NodesEnum> Clone for Snapshot<N> {
    fn clone(&self) -> Self {
        Snapshot {
            version: self.version,
            graph: self.graph.clone(),
            plan: self.plan.clone(),
            val_ins: self.val_ins.clone(),
            started: self.started,
            recording: self.recording.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeType;
    use crate::Nodes;
    use crate::engine::NodeTypeEnum;
    use crate::standard::{ephys, spikes};
    use crate::{DataStream, PublishedValue};

    fn create(name: &str) -> Nodes {
        Nodes::create(NodeType::named(name).unwrap())
    }

    fn port(node: NodeId, role: PortRole, index: usize) -> PortAddress {
        PortAddress { node, role, port_idx: index }
    }

    #[test]
    fn data_streams_propagate() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let filter = graph.add(create("Band-pass Filter"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        graph.connect(port(board, PortRole::DataStreamOut, 0), port(filter, PortRole::DataStreamIn, 0)).unwrap();
        graph.connect(port(detector, PortRole::DataStreamIn, 0), port(filter, PortRole::DataStreamOut, 0)).unwrap();

        let resolved = graph.resolve();
        let continuous = ephys::Continuous { channels: 64, sample_rate: 30000.0 };
        assert_eq!(resolved[&filter].outputs[0].stream(), Some(DataStream::Continuous(continuous)));
        assert!(matches!(resolved[&detector].outputs[0].stream(), Some(DataStream::Spikes(s)) if s.channels == 64));

        graph.disconnect(graph.edge_into(port(filter, PortRole::DataStreamIn, 0)).unwrap());
        let resolved = graph.resolve();
        assert_eq!(resolved[&detector].outputs[0].stream(), None);
        assert!(!resolved[&detector].warnings.is_empty(), "unconnected input should warn");
    }

    #[test]
    fn rejects_bad_connections() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let gain = graph.add(create("Continuous Gain"), [0.0, 0.0]);
        let gain2 = graph.add(create("Continuous Gain"), [0.0, 0.0]);
        let video = graph.add(create("Video Viewer"), [0.0, 0.0]);
        let spikes = graph.add(create("Spike Viewer"), [0.0, 0.0]);

        let check = |a, b| graph.check(a, b).err();
        assert_eq!(
            check(port(gain, PortRole::DataStreamOut, 0), port(gain, PortRole::DataStreamIn, 0)),
            Some(ConnectError::SameNode)
        );
        assert!(matches!(
            check(port(board, PortRole::DataStreamOut, 0), port(video, PortRole::DataStreamIn, 0)),
            Some(ConnectError::DataStreamKindMismatch { .. })
        ));
        assert_eq!(
            check(port(board, PortRole::DataStreamOut, 5), port(gain, PortRole::DataStreamIn, 0)),
            Some(ConnectError::NoSuchPort)
        );
        assert_eq!(
            check(port(board, PortRole::DataStreamOut, 0), port(gain, PortRole::DataStreamOut, 0)),
            Some(ConnectError::RoleMismatch)
        );
        assert!(matches!(
            check(port(spikes, PortRole::ValInCmdOut, 0), port(gain, PortRole::ValOutCmdIn, 0)),
            Some(ConnectError::ValueKindMismatch { .. })
        ));

        graph.connect(port(gain, PortRole::DataStreamOut, 0), port(gain2, PortRole::DataStreamIn, 0)).unwrap();
        assert_eq!(
            graph.check(port(gain2, PortRole::DataStreamOut, 0), port(gain, PortRole::DataStreamIn, 0)).err(),
            Some(ConnectError::Cycle)
        );
    }

    #[test]
    fn connecting_replaces_the_existing_input() {
        let mut graph = Graph::default();
        let a = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let b = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let gain = graph.add(create("Continuous Gain"), [0.0, 0.0]);
        graph.connect(port(a, PortRole::DataStreamOut, 0), port(gain, PortRole::DataStreamIn, 0)).unwrap();
        let edge = graph.connect(port(b, PortRole::DataStreamOut, 0), port(gain, PortRole::DataStreamIn, 0)).unwrap();
        assert_eq!(graph.edges(), &[edge]);
    }

    #[test]
    fn a_node_knows_what_controls_its_settings_while_the_edge_is_live() {
        let mut graph = Graph::default();
        let gain = graph.add(create("Spike Gain"), [0.0, 0.0]);
        let (viewer, writer) =
            (graph.add(create("Spike Viewer"), [0.0, 0.0]), graph.add(create("Write LTX"), [0.0, 0.0]));
        assert_eq!(graph.controlled_by(gain), [Vec::<String>::new()]);
        let edge = graph.connect(port(viewer, PortRole::ValInCmdOut, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
        graph.connect(port(writer, PortRole::ValIn, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
        assert_eq!(graph.controlled_by(gain), [["Spike Viewer", "Write LTX"]]);
        graph.set_edge_disabled(edge, true);
        assert_eq!(graph.controlled_by(gain), [["Write LTX"]]);
        graph.get_mut(writer).unwrap().disabled = true;
        assert_eq!(graph.controlled_by(gain), [Vec::<String>::new()]);
    }

    #[test]
    fn settings_edges_may_point_upstream_and_edit_settings() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        let viewer = graph.add(create("Spike Viewer"), [0.0, 0.0]);
        graph.connect(port(board, PortRole::DataStreamOut, 0), port(detector, PortRole::DataStreamIn, 0)).unwrap();
        graph.connect(port(detector, PortRole::DataStreamOut, 0), port(viewer, PortRole::DataStreamIn, 0)).unwrap();
        // The viewer is downstream of the detector, but can still edit its thresholds.
        graph.connect(port(viewer, PortRole::ValInCmdOut, 0), port(detector, PortRole::ValOutCmdIn, 0)).unwrap();
        assert_eq!(graph.topological_order(), vec![board, detector, viewer]);

        let thresholds =
            |uv: Vec<f32>| PublishedValue::Thresholds(spikes::Thresholds { values: uv, ..Default::default() });
        assert_eq!(graph.val_ins(viewer), vec![Some(thresholds(vec![])), None]);
        let theme = &crate::standard::themes::THEMES[0].colours;
        graph.edit(viewer, &[], theme, None, None, None, false, false, None, |_, context| {
            let thresholds = spikes::Thresholds { values: vec![55.0], ..Default::default() };
            let thresholds_port = crate::standard::spikes::spike_viewer::ValInCmdOut::thresholds;
            context.cmd_out(thresholds_port, spikes::SetThresholds(thresholds))
        });
        assert_eq!(graph.get(detector).unwrap().node.val_out(0), thresholds(vec![55.0]));

        // A value can be read by several nodes (an LTX writer reading the gains the viewer sets); a port reads one.
        let writer = graph.add(create("Write LTX"), [0.0, 0.0]);
        let gain = graph.add(create("Spike Gain"), [0.0, 0.0]);
        graph.connect(port(viewer, PortRole::ValInCmdOut, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
        graph.connect(port(writer, PortRole::ValIn, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
        let into_gains = |graph: &Graph<Nodes>| {
            graph.edges().iter().filter(|e| e.to == port(gain, PortRole::ValOutCmdIn, 0)).count()
        };
        assert_eq!(into_gains(&graph), 2);
    }

    /// A command through a port goes where the edge points as it's delivered, if the port there accepts it; one a
    /// node sends itself needn't be accepted by any port.
    #[test]
    fn a_command_goes_where_the_edge_points_when_its_delivered() {
        use crate::commands::Address;
        let mut graph = Graph::default();
        let (viewer, first, second) = (
            graph.add(create("Spike Viewer"), [0.0, 0.0]),
            graph.add(create("Spike Detector"), [0.0, 0.0]),
            graph.add(create("Spike Detector"), [0.0, 0.0]),
        );
        let address = Address::Port { from: viewer, val_in_idx: 0 };
        assert_eq!(graph.command_target(address), None, "not connected: nowhere");
        graph.connect(port(viewer, PortRole::ValInCmdOut, 0), port(first, PortRole::ValOutCmdIn, 0)).unwrap();
        assert_eq!(graph.command_target(address), Some((first, CommandSource::Port(0))));
        graph.connect(port(viewer, PortRole::ValInCmdOut, 0), port(second, PortRole::ValOutCmdIn, 0)).unwrap();
        assert_eq!(graph.command_target(address), Some((second, CommandSource::Port(0))), "reconnected: the new one");

        let thresholds = |uv: f32| spikes::Thresholds { values: vec![uv], ..Default::default() };
        let set = |uv| crate::Command::SetThresholds(spikes::SetThresholds(thresholds(uv)));
        assert!(graph.command(second, CommandSource::Port(0), set(55.0), None, None, false));
        assert_eq!(graph.get(second).unwrap().node.val_out(0), thresholds(55.0).into());
        let files = crate::Command::ProcessFiles(crate::standard::other::run_command::ProcessFiles(Vec::new()));
        assert!(!graph.command(second, CommandSource::Port(0), files, None, None, false), "not what its port accepts");
        graph.get_mut(second).unwrap().disabled = true;
        assert!(!graph.command(second, CommandSource::Itself, set(60.0), None, None, false), "disabled");
        assert_eq!(graph.get(second).unwrap().node.val_out(0), thresholds(55.0).into());
    }

    /// An auto detect a viewer started carries on through its edits; it's done when the node's passes send what they
    /// found, and forgotten when the node's told to clear its transient state.
    #[test]
    fn auto_detect_is_started_by_a_command_and_ended_by_the_nodes_own() {
        use crate::standard::auto_detect::DetectRequest;
        let mut graph = Graph::default();
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        let started = |graph: &Graph<Nodes>| match graph.get(detector).unwrap().node.val_out(0) {
            PublishedValue::Thresholds(thresholds) => thresholds.detect.started.is_some(),
            _ => unreachable!(),
        };
        let mut send = |from, command| graph.command(detector, from, command, None, None, false);
        assert!(send(
            CommandSource::Port(0),
            crate::Command::from(crate::standard::auto_detect::Detect(DetectRequest::Start))
        ));
        let edited = spikes::Thresholds { values: vec![70.0], ..Default::default() };
        send(CommandSource::Port(0), crate::Command::SetThresholds(spikes::SetThresholds(edited.clone())));
        assert!(started(&graph), "an edit leaves it going");
        graph.command(
            detector,
            CommandSource::Itself,
            crate::Command::SetThresholds(spikes::SetThresholds(edited)),
            None,
            None,
            false,
        );
        assert!(!started(&graph), "what its passes found ends it");
        graph.command(
            detector,
            CommandSource::Port(0),
            crate::Command::from(crate::standard::auto_detect::Detect(DetectRequest::Start)),
            None,
            None,
            false,
        );
        graph.clear_transient_state();
        assert!(!started(&graph), "forgotten");
    }

    #[test]
    fn disabled_nodes_and_edges_run_as_if_they_were_not_there() {
        let mut graph = Graph::default();
        let board = graph.add(create("Acquisition Board"), [0.0, 0.0]);
        let detector = graph.add(create("Spike Detector"), [0.0, 0.0]);
        let viewer = graph.add(create("Spike Viewer"), [0.0, 0.0]);
        let feed =
            graph.connect(port(board, PortRole::DataStreamOut, 0), port(detector, PortRole::DataStreamIn, 0)).unwrap();
        graph.connect(port(detector, PortRole::DataStreamOut, 0), port(viewer, PortRole::DataStreamIn, 0)).unwrap();
        let control =
            graph.connect(port(viewer, PortRole::ValInCmdOut, 0), port(detector, PortRole::ValOutCmdIn, 0)).unwrap();
        let streams = |graph: &Graph<Nodes>, id| graph.resolve()[&id].input_streams[0].is_some();
        let planned =
            |graph: &Graph<Nodes>, id| super::super::Plan::new(graph, false).steps().iter().any(|s| s.id == id);
        assert!(streams(&graph, viewer) && planned(&graph, detector));

        // A disabled node makes nothing, and doesn't run.
        graph.get_mut(detector).unwrap().disabled = true;
        assert!(!streams(&graph, viewer), "nothing downstream of it");
        assert!(!planned(&graph, detector));
        assert_eq!(graph.val_ins(viewer)[0], None, "its settings can't be edited through it");
        graph.get_mut(detector).unwrap().disabled = false;

        // A disabled edge is as if it weren't connected, but is kept.
        graph.set_edge_disabled(feed, true);
        assert!(!streams(&graph, detector) && !streams(&graph, viewer));
        assert!(graph.edges().contains(&feed));
        graph.set_edge_disabled(feed, false);
        assert!(streams(&graph, viewer));
        graph.set_edge_disabled(control, true);
        assert_eq!(graph.val_ins(viewer)[0], None);
        // And it changes how the graph runs.
        let before = graph.clone();
        graph.set_edge_disabled(control, false);
        assert!(!crate::engine::same_processing(&before, &graph));
    }

    #[test]
    fn labels_are_unique() {
        let mut graph = Graph::<Nodes>::default();
        let a = graph.add(create("Continuous Gain"), [0.0, 0.0]);
        let b = graph.add(create("Continuous Gain"), [0.0, 0.0]);
        assert_eq!(graph.get(a).unwrap().label, "Continuous Gain");
        assert_eq!(graph.get(b).unwrap().label, "Continuous Gain 2");
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
