//! The app edits a graph of nodes and the edges between their ports, saves it, and undoes changes to it; a run works
//! from snapshots of it. This module has the graph, the snapshots, and the recordings they carry.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{
    CommandContext, CommandSource, CommandsEnum, Dimension, EdgeInfo, EdgeTypesEnum, Edges, Input, Node, NodesEnum,
    Output, Plan, UiContext, ValPort,
};
use crate::commands::{Address, Commands};
use crate::theme::Colours;

/// The graph names each node with an id, unique within the graph and never reused, so it's the same node through undo,
/// redo and saving.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// A node has ports of four roles, each named for what flows through it, which way. Data edges run from a
/// `DataStreamOut` to a `DataStreamIn`, carrying a pass's data. Value edges run from a `ValInCmdOut` to a
/// `ValOutCmdIn`: the node at the `ValOutCmdIn` end publishes a value, which the node at the other end reads, and which
/// that node can send commands back to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PortRole {
    DataStreamIn,
    DataStreamOut,
    /// The node publishes a value here, and takes commands through the edges into it.
    ValOutCmdIn,
    /// The node reads another node's published value here, and sends it commands.
    ValInCmdOut,
    /// Val-only: the node only reads another node's published value here (the GUI calls it read-only). A node declares
    /// such ports (see `ValPort`), but the graph numbers them with its `ValInCmdOut` ports, and finds them as those: an
    /// edge from either starts at a `ValInCmdOut` (see `Edge::source`).
    ValIn,
}

impl PortRole {
    /// `DataStreamIn` and `DataStreamOut` ports are data ports; the others, value ports.
    pub fn dimension(self) -> Dimension {
        match self {
            PortRole::DataStreamIn | PortRole::DataStreamOut => Dimension::Data,
            PortRole::ValOutCmdIn | PortRole::ValInCmdOut | PortRole::ValIn => Dimension::Values,
        }
    }

    /// An edge from a port of this role ends at a port of this one.
    pub fn partner(self) -> PortRole {
        match self {
            PortRole::DataStreamIn => PortRole::DataStreamOut,
            PortRole::DataStreamOut => PortRole::DataStreamIn,
            PortRole::ValOutCmdIn => PortRole::ValInCmdOut,
            PortRole::ValInCmdOut | PortRole::ValIn => PortRole::ValOutCmdIn,
        }
    }

    /// Edges start at a `DataStreamOut` or `ValInCmdOut`, and end at a `DataStreamIn` or `ValOutCmdIn`. This says
    /// whether they start here.
    pub fn is_source(self) -> bool {
        matches!(self, PortRole::DataStreamOut | PortRole::ValInCmdOut | PortRole::ValIn)
    }

    /// The role's name, for people.
    pub fn name(self) -> &'static str {
        match self {
            PortRole::DataStreamIn => "input",
            PortRole::DataStreamOut => "output",
            PortRole::ValOutCmdIn => "setting",
            PortRole::ValInCmdOut => "control",
            PortRole::ValIn => "read-only control",
        }
    }
}

/// The graph finds a port by its node, role and index among the node's ports of that role.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Port {
    pub node: NodeId,
    pub role: PortRole,
    pub index: usize,
}

/// The graph stores each end of an edge as a node and a port index (the role following from the edge's dimension).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct PortRef {
    pub node: NodeId,
    pub port: usize,
}

/// An edge connects a `DataStreamOut` to a `DataStreamIn`, or a `ValInCmdOut` to a `ValOutCmdIn`. A `DataStreamIn`
/// has at most one edge into it, and a `DataStreamOut` any number out of it; a `ValInCmdOut` has at most one edge out
/// of it, and a `ValOutCmdIn` any number into it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Edge {
    #[serde(default)]
    pub dimension: Dimension,
    pub from: PortRef,
    pub to: PortRef,
}

impl Edge {
    /// The edge starts from a `DataStreamOut` or `ValInCmdOut` (or a `ValIn`, numbered and found as a `ValInCmdOut`):
    /// this one.
    pub fn source(&self) -> Port {
        let role = if self.dimension == Dimension::Data { PortRole::DataStreamOut } else { PortRole::ValInCmdOut };
        Port { node: self.from.node, role, index: self.from.port }
    }

    /// The edge goes into a `DataStreamIn` or `ValOutCmdIn`: this one.
    pub fn target(&self) -> Port {
        let role = if self.dimension == Dimension::Data { PortRole::DataStreamIn } else { PortRole::ValOutCmdIn };
        Port { node: self.to.node, role, index: self.to.port }
    }
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
    /// Only a `DataStreamOut` and a `DataStreamIn`, or a `ValInCmdOut` and a `ValOutCmdIn`, can be connected.
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
            ConnectError::KindMismatch { dimension: Dimension::Values, from, to } => {
                write!(f, "a {from} control can't edit a {to} setting")
            }
            ConnectError::Cycle => write!(f, "that would make a loop"),
        }
    }
}

/// The graph works out each node's ports and data streams from the node's settings and what's upstream of it (see
/// `Graph::resolve`). This is one node's.
pub struct NodeState<E: Edges> {
    pub inputs: Vec<Input<E>>,
    /// For each input, the data stream connected to it, if any.
    pub input_streams: Vec<Option<E::DataStream>>,
    pub outputs: Vec<Output<E>>,
    /// The node's `ValOutCmdIn` ports, in order, and what it publishes at each (see `Node::val_out`).
    pub val_outs: Vec<ValPort<E>>,
    pub values: Vec<E::PublishedValue>,
    /// The node's `ValInCmdOut` and `ValIn` ports, in order, numbered together.
    pub val_ins: Vec<ValPort<E>>,
    pub warnings: Vec<String>,
}

/// The graph names an edge type by its dimension and its index in that dimension's `EdgeTypesEnum::KINDS`.
pub type KindKey = (Dimension, usize);

impl<E: Edges> NodeState<E> {
    /// The state of `node`, given the data streams on its inputs.
    pub fn of<N: Node<E>>(node: &N, input_streams: Vec<Option<E::DataStream>>) -> Self {
        let (val_outs, val_ins): (Vec<_>, Vec<_>) = node.ports().into_iter().partition(ValPort::publishes);
        NodeState {
            inputs: node.inputs(),
            outputs: node.outputs(&input_streams),
            warnings: node.warnings(&input_streams),
            input_streams,
            values: (0..val_outs.len()).map(|port| node.val_out(port)).collect(),
            val_outs,
            val_ins,
        }
    }

    pub fn new<N: NodesEnum<Edges = E>>(node: &N, input_streams: Vec<Option<E::DataStream>>) -> Self {
        let (val_outs, val_ins): (Vec<_>, Vec<_>) = node.ports().into_iter().partition(ValPort::publishes);
        NodeState {
            inputs: node.inputs(),
            outputs: node.outputs(&input_streams),
            warnings: node.warnings(&input_streams),
            input_streams,
            values: (0..val_outs.len()).map(|port| node.val_out(port)).collect(),
            val_outs,
            val_ins,
        }
    }

    /// The graph draws each port on one side of the node, so wires leave it that way. This says whether it's the
    /// right.
    pub fn on_right(&self, role: PortRole, index: usize) -> bool {
        match role {
            PortRole::DataStreamOut => true,
            PortRole::DataStreamIn => false,
            PortRole::ValOutCmdIn => self.val_outs.get(index).is_some_and(|p| !p.flipped),
            PortRole::ValInCmdOut | PortRole::ValIn => self.val_ins.get(index).is_some_and(|p| p.flipped),
        }
    }

    /// The state of `node` with nothing connected.
    pub fn unconnected<N: NodesEnum<Edges = E>>(node: &N) -> Self {
        Self::new(node, vec![None; node.inputs().len()])
    }

    /// How many ports of `role` the node has (`ValIn` ports being counted as `ValInCmdOut`).
    pub fn count(&self, role: PortRole) -> usize {
        match role {
            PortRole::DataStreamIn => self.inputs.len(),
            PortRole::DataStreamOut => self.outputs.len(),
            PortRole::ValOutCmdIn => self.val_outs.len(),
            PortRole::ValInCmdOut => self.val_ins.len(),
            PortRole::ValIn => 0,
        }
    }

    /// The value port at `(role, index)`, if it's one.
    pub fn val_port(&self, role: PortRole, index: usize) -> Option<&ValPort<E>> {
        match role {
            PortRole::ValOutCmdIn => self.val_outs.get(index),
            PortRole::ValInCmdOut | PortRole::ValIn => self.val_ins.get(index),
            PortRole::DataStreamIn | PortRole::DataStreamOut => None,
        }
    }

    /// A `ValIn` port is val-only (read-only, to people): this says whether the `ValInCmdOut` port `index` is one.
    pub fn val_only(&self, index: usize) -> bool {
        self.val_ins.get(index).is_some_and(|p| p.role == PortRole::ValIn)
    }

    pub fn name(&self, role: PortRole, index: usize) -> Option<&'static str> {
        match role {
            PortRole::DataStreamIn => self.inputs.get(index).map(|p| p.name),
            PortRole::DataStreamOut => self.outputs.get(index).map(|p| p.name),
            _ => self.val_port(role, index).map(|p| p.name),
        }
    }

    pub fn kind(&self, role: PortRole, index: usize) -> Option<KindKey> {
        let stream = |kind| (Dimension::Data, E::DataStream::kind_index(kind));
        let value = |kind| (Dimension::Values, E::PublishedValue::kind_index(kind));
        match role {
            PortRole::DataStreamIn => self.inputs.get(index).map(|p| stream(p.kind)),
            PortRole::DataStreamOut => self.outputs.get(index).map(|p| stream(p.kind)),
            _ => self.val_port(role, index).map(|p| value(p.value_kind)),
        }
    }

    pub fn info(&self, role: PortRole, index: usize) -> Option<&'static EdgeInfo> {
        self.kind(role, index).map(edge_info::<E>)
    }

    /// The graph and the node's window show what's on a port: the data stream on an input or output, or the value the
    /// node publishes.
    pub fn summary(&self, role: PortRole, index: usize) -> Option<String> {
        match role {
            PortRole::DataStreamIn => self.input_streams.get(index).copied().flatten().map(|s| s.summary()),
            PortRole::DataStreamOut => self.outputs.get(index)?.stream.map(|s| s.summary()),
            PortRole::ValOutCmdIn => self.values.get(index).map(|v| v.summary()),
            PortRole::ValInCmdOut | PortRole::ValIn => None,
        }
    }

    pub fn ports(&self, node: NodeId) -> impl Iterator<Item = Port> + '_ {
        [PortRole::DataStreamOut, PortRole::DataStreamIn, PortRole::ValOutCmdIn, PortRole::ValInCmdOut]
            .into_iter()
            .flat_map(move |role| (0..self.count(role)).map(move |index| Port { node, role, index }))
    }

    /// Two ports fit if they're partners of the same kind. This says whether this node's port `(role, index)` fits
    /// `other`'s port `to`.
    pub fn fits(&self, (role, index): (PortRole, usize), other: &Self, to: (PortRole, usize)) -> bool {
        let kind = self.kind(role, index);
        role.partner() == to.0 && kind.is_some() && kind == other.kind(to.0, to.1)
    }

    /// When a wire's dropped on a node, it connects to the first of the node's ports that fits (see `fits`). This finds
    /// it, for `from`'s port `port`.
    pub fn find(&self, node: NodeId, from: &Self, port: (PortRole, usize)) -> Option<Port> {
        let role = port.0.partner();
        (0..self.count(role)).find(|&i| from.fits(port, self, (role, i))).map(|index| Port { node, role, index })
    }
}

pub fn edge_info<E: Edges>((dimension, index): KindKey) -> &'static EdgeInfo {
    match dimension {
        Dimension::Data => &E::DataStream::KINDS[index],
        Dimension::Values => &E::PublishedValue::KINDS[index],
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

    /// The app has every node drop what it was asked to do only for now (see `CommandsEnum::clear_transient_state`)
    /// with this.
    pub fn clear_transient_state(&mut self) {
        self.nodes.iter_mut().for_each(|(id, entry)| clear_transient_state(*id, entry));
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

    /// A `DataStreamIn` has at most one edge into it. This finds it, enabled or not (or, for a `ValOutCmdIn`, the
    /// first).
    pub fn edge_into(&self, port: Port) -> Option<Edge> {
        self.edges.iter().find(|e| e.target() == port).copied()
    }

    /// A `DataStreamOut` can have any number of edges out of it, and a `ValInCmdOut` one. This finds them, enabled or
    /// not.
    pub fn edges_from(&self, port: Port) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.source() == port).copied()
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
    pub fn live_edge_into(&self, port: Port) -> Option<Edge> {
        self.edges.iter().find(|e| e.target() == port && self.is_live(e)).copied()
    }

    /// The edges out of a `DataStreamOut` or `ValInCmdOut` that count (see `is_live`).
    pub fn live_edges_from(&self, port: Port) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().filter(move |e| e.source() == port && self.is_live(e)).copied()
    }

    /// A node's window says which nodes read and command what it publishes: for each of its `ValOutCmdIn` ports, the
    /// labels of the nodes with a live edge to it.
    fn controlled_by(&self, id: NodeId) -> Vec<Vec<String>> {
        let count = self.nodes.get(&id).map_or(0, |entry| entry.node.ports().iter().filter(|p| p.publishes()).count());
        let labels = |index| {
            let port = Port { node: id, role: PortRole::ValOutCmdIn, index };
            let edges = self.edges.iter().filter(|e| e.target() == port && self.is_live(e));
            edges.filter_map(|e| Some(self.nodes.get(&e.from.node)?.label.clone())).collect()
        };
        (0..count).map(labels).collect()
    }

    fn kind(&self, port: Port) -> Option<KindKey> {
        NodeState::unconnected(&self.nodes.get(&port.node)?.node).kind(port.role, port.index)
    }

    /// Before connecting two ports, the graph checks they can be. This returns the edge that would connect `a` and `b`
    /// (in either order), or why they can't be; what it would replace is up to `connect`.
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
        let state = |port: Port| NodeState::unconnected(&self.nodes[&port.node].node);
        if !state(from).fits((from.role, from.index), &state(to), (to.role, to.index)) {
            let name = |kind| edge_info::<N::Edges>(kind).name;
            return Err(ConnectError::KindMismatch { dimension, from: name(from_kind), to: name(to_kind) });
        }
        // Value edges carry no data, so may point anywhere; only data edges must not loop.
        if dimension == Dimension::Data && self.reaches(to.node, from.node) {
            return Err(ConnectError::Cycle);
        }
        Ok(Edge {
            dimension,
            from: PortRef { node: from.node, port: from.index },
            to: PortRef { node: to.node, port: to.index },
        })
    }

    /// The canvas connects `a` and `b` (in either order) with this, replacing anything already going into the
    /// `DataStreamIn`, or what the `ValInCmdOut` was connected to. (A `DataStreamIn` has one source, but a
    /// `ValOutCmdIn` can have many nodes reading it: one commanding it, say, and others only reading. A `ValInCmdOut`
    /// reads just the one.)
    pub fn connect(&mut self, a: Port, b: Port) -> Result<Edge, ConnectError> {
        let edge = self.check(a, b)?;
        self.edges.retain(|e| match edge.dimension {
            Dimension::Data => e.target() != edge.target(),
            Dimension::Values => e.source() != edge.source(),
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

    /// While a wire's dragged from `port`, the canvas shows the ports, on every node, that it could connect to.
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
            let (from, to) = (e.source(), e.target());
            let (Some(a), Some(b)) = (self.nodes.get(&from.node), self.nodes.get(&to.node)) else { return false };
            let (a, b) = (NodeState::unconnected(&a.node), NodeState::unconnected(&b.node));
            a.fits((from.role, from.index), &b, (to.role, to.index))
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
            let inputs = node.inputs();
            let input_streams: Vec<Option<DataStream<N>>> = (0..inputs.len())
                .map(|index| {
                    let edge = self.live_edge_into(Port { node: id, role: PortRole::DataStreamIn, index })?;
                    let stream = resolved.get(&edge.from.node)?.outputs.get(edge.from.port)?.stream?;
                    (stream.kind() == inputs[index].kind).then_some(stream)
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
                    output.stream = None;
                }
                state.warnings.clear();
            }
            resolved.insert(id, state);
        }
        resolved
    }

    /// Live value edges carrying commands (a command the near end sends that the far end accepts) can make a loop, so
    /// what a node sets off can, in the end, set it off again (Run Commands running each other, say). The canvas warns
    /// of the nodes in such a loop, which this finds.
    pub fn command_loops(&self) -> std::collections::HashSet<NodeId> {
        let mut next: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in self.edges.iter().filter(|e| e.dimension == Dimension::Values && self.is_live(e)) {
            let ports = |id: NodeId| self.nodes.get(&id).map(|n| n.node.ports()).unwrap_or_default();
            let (from, to) = (ports(edge.from.node), ports(edge.to.node));
            let sends = from.iter().filter(|p| !p.publishes()).nth(edge.from.port).map_or(&[][..], |p| p.commands);
            let accepts = to.iter().filter(|p| p.publishes()).nth(edge.to.port).map_or(&[][..], |p| p.commands);
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
        let count = entry.node.ports().iter().filter(|p| !p.publishes()).count();
        (0..count)
            .map(|port| {
                let to = self.val_in_target(id, port)?;
                let target = &self.nodes.get(&to.node)?.node;
                let publishes = target.ports().iter().filter(|p| p.publishes()).count();
                (to.port < publishes).then(|| target.val_out(to.port))
            })
            .collect()
    }

    /// Node `from`'s `ValInCmdOut` port `port` (by index among those and its `ValIn` ports) connects to at most one
    /// `ValOutCmdIn`: this finds it, if the edge is live.
    pub fn val_in_target(&self, from: NodeId, port: usize) -> Option<PortRef> {
        self.live_edges_from(Port { node: from, role: PortRole::ValInCmdOut, index: port }).next().map(|e| e.to)
    }

    /// A command's delivered to a node, from where it came (see `CommandSource`): this finds them, for one sent to
    /// `address`, following the edge now. A command sent through a port that's no longer connected goes nowhere.
    pub fn command_target(&self, address: Address) -> Option<(NodeId, CommandSource)> {
        match address {
            Address::Port { from, port } => {
                let to = self.val_in_target(from, port)?;
                Some((to.node, CommandSource::Port(to.port)))
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
        if entry.disabled && !command.is_clear_transient_state() {
            log::info!("{} is disabled: {:?} not delivered", entry.label, command.kind());
            return false;
        }
        if let CommandSource::Port(port) = from {
            let ports = entry.node.ports();
            let accepts =
                ports.iter().filter(|p| p.publishes()).nth(port).is_some_and(|p| p.commands.contains(&command.kind()));
            if !accepts {
                log::warn!("{}'s port {port} doesn't accept {:?}: not delivered", entry.label, command.kind());
                return false;
            }
        }
        let context = &mut CommandContext::new(to, &entry.label, &val_ins, ui_state, commands, running);
        entry.node.on_command(from, command, context);
        true
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
        for (port, command) in sent {
            if let Some(commands) = commands {
                commands.send(Address::Port { from: id, port }, command);
                continue;
            }
            match self.command_target(Address::Port { from: id, port }) {
                Some((to, from)) => {
                    self.command(to, from, command, None, None, running);
                }
                None => log::info!("Port {port} isn't connected: {:?} not sent", command.kind()),
            }
        }
        Some(result)
    }
}

/// A snapshot keeps, for each node with ports reading values, what each reads, if it's connected.
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

/// The app has node `id` drop what it was asked to do only for now (see `CommandsEnum::clear_transient_state`) with
/// this: when it duplicates the node, say. The node isn't edited otherwise.
pub fn clear_transient_state<N: NodesEnum>(id: NodeId, entry: &mut NodeEntry<N>) {
    let context = &mut CommandContext::new(id, &entry.label, &[], None, None, false);
    let command = <<N::Edges as Edges>::Command as CommandsEnum>::clear_transient_state();
    entry.node.on_command(CommandSource::App, command, context);
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
            .filter(|(_, entry)| entry.node.ports().iter().any(|p| !p.publishes()))
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

    fn port(node: NodeId, role: PortRole, index: usize) -> Port {
        Port { node, role, index }
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
        assert_eq!(resolved[&filter].outputs[0].stream, Some(DataStream::Continuous(continuous)));
        assert!(matches!(resolved[&detector].outputs[0].stream, Some(DataStream::Spikes(s)) if s.channels == 64));

        graph.disconnect(graph.edge_into(port(filter, PortRole::DataStreamIn, 0)).unwrap());
        let resolved = graph.resolve();
        assert_eq!(resolved[&detector].outputs[0].stream, None);
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
            Some(ConnectError::KindMismatch { dimension: Dimension::Data, .. })
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
            Some(ConnectError::KindMismatch { dimension: Dimension::Values, .. })
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
        graph.connect(port(writer, PortRole::ValInCmdOut, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
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
            context.cmd_out(0, crate::Command::SetThresholds(thresholds))
        });
        assert_eq!(graph.get(detector).unwrap().node.val_out(0), thresholds(vec![55.0]));

        // A value can be read by several nodes (an LTX writer reading the gains the viewer sets); a port reads one.
        let writer = graph.add(create("Write LTX"), [0.0, 0.0]);
        let gain = graph.add(create("Spike Gain"), [0.0, 0.0]);
        graph.connect(port(viewer, PortRole::ValInCmdOut, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
        graph.connect(port(writer, PortRole::ValInCmdOut, 1), port(gain, PortRole::ValOutCmdIn, 0)).unwrap();
        let into_gains = |graph: &Graph<Nodes>| {
            graph.edges().iter().filter(|e| e.target() == port(gain, PortRole::ValOutCmdIn, 0)).count()
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
        let address = Address::Port { from: viewer, port: 0 };
        assert_eq!(graph.command_target(address), None, "not connected: nowhere");
        graph.connect(port(viewer, PortRole::ValInCmdOut, 0), port(first, PortRole::ValOutCmdIn, 0)).unwrap();
        assert_eq!(graph.command_target(address), Some((first, CommandSource::Port(0))));
        graph.connect(port(viewer, PortRole::ValInCmdOut, 0), port(second, PortRole::ValOutCmdIn, 0)).unwrap();
        assert_eq!(graph.command_target(address), Some((second, CommandSource::Port(0))), "reconnected: the new one");

        let thresholds = |uv: f32| spikes::Thresholds { values: vec![uv], ..Default::default() };
        let set = |uv| crate::Command::SetThresholds(thresholds(uv));
        assert!(graph.command(second, CommandSource::Port(0), set(55.0), None, None, false));
        assert_eq!(graph.get(second).unwrap().node.val_out(0), thresholds(55.0).into());
        let files = crate::Command::ProcessFiles(crate::standard::other::run_command::Files(Vec::new()));
        assert!(!graph.command(second, CommandSource::Port(0), files, None, None, false), "not what its port accepts");
        graph.get_mut(second).unwrap().disabled = true;
        assert!(!graph.command(second, CommandSource::Itself, set(60.0), None, None, false), "disabled");
        assert_eq!(graph.get(second).unwrap().node.val_out(0), thresholds(55.0).into());
    }

    /// An auto detect a viewer started carries on through its edits; it's done when the node's passes send what they
    /// found, and forgotten on `ClearTransientState`.
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
        assert!(send(CommandSource::Port(0), crate::Command::Detect(DetectRequest::Start)));
        let edited = spikes::Thresholds { values: vec![70.0], ..Default::default() };
        send(CommandSource::Port(0), crate::Command::SetThresholds(edited.clone()));
        assert!(started(&graph), "an edit leaves it going");
        graph.command(detector, CommandSource::Itself, crate::Command::SetThresholds(edited), None, None, false);
        assert!(!started(&graph), "what its passes found ends it");
        graph.command(
            detector,
            CommandSource::Port(0),
            crate::Command::Detect(DetectRequest::Start),
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
