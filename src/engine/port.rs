//! Edges connect nodes at their ports. A node declares its ports (`Port`), each in one of five roles (`PortRole`), and
//! the graph finds a port by its node, role and index (`PortAddress`).

use serde::{Deserialize, Serialize};

use super::{CommandKind, DataStreamKind, EdgeTypesEnum, Edges, NodeId, ValueKind};

/// Edges connect nodes at their ports, each named for what flows through it, which way (see `PortRole`, which names a
/// port's role without what's in it). A node declares its ports on its type (see `crate::engine::ports`), and the graph
/// lists them as `Port`s (see `NodesEnum::list_ports_data_stream_in`, `list_ports_data_stream_out` and
/// `list_ports_val_cmd`). The graph numbers each role's ports separately, in declaration order (see `PortAddress`).
/// The node itself numbers its `ValInCmdOut` and `ValIn` ports together, in declaration order, because it reads
/// values through both (see `ReadsValIn::index`).
pub enum Port<E: Edges> {
    /// Data reaches the node here, each pass, from another node's `DataStreamOut`.
    DataStreamIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The port's field name in its node's `ports!`, which saved graphs use to identify the port. Unlike `name`,
        /// the id never changes when the display name does.
        id: &'static str,
        /// The kind of data stream the port takes.
        data_stream_kind: DataStreamKind<E>,
    },
    /// The node sends data on from here, each pass.
    DataStreamOut {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The port's field name in its node's `ports!`, which saved graphs use to identify the port. Unlike `name`,
        /// the id never changes when the display name does.
        id: &'static str,
        /// The kind of data stream the port sends.
        data_stream_kind: DataStreamKind<E>,
        /// The stream itself, which the graph can't always know (while the node's input isn't connected, say), and
        /// then this is `None`.
        stream: Option<E::DataStream>,
    },
    /// The node publishes a value here (usually a copy of one of its settings: see `Node::val_out`), and accepts
    /// commands through the edges into it, from other nodes' `ValInCmdOut` ports.
    ValOutCmdIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The port's field name in its node's `ports!`, which saved graphs use to identify the port. Unlike `name`,
        /// the id never changes when the display name does.
        id: &'static str,
        /// The kind of value the port has.
        value_kind: ValueKind<E>,
        /// The commands the port accepts.
        commands: Vec<CommandKind<E>>,
        /// Which side of the node the graph draws the port on (see `Side`).
        side: Side,
    },
    /// The node reads the value at the `ValOutCmdIn` this connects to, and sends that node commands (usually the one
    /// setting the value, `SetGains`, say; sometimes others, such as auto detect's `Detect`).
    ValInCmdOut {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The port's field name in its node's `ports!`, which saved graphs use to identify the port. Unlike `name`,
        /// the id never changes when the display name does.
        id: &'static str,
        /// The kind of value the port reads.
        value_kind: ValueKind<E>,
        /// The commands the port sends.
        commands: Vec<CommandKind<E>>,
        /// Which side of the node the graph draws the port on (see `Side`).
        side: Side,
    },
    /// Val-only: the node only reads the value at the `ValOutCmdIn` this connects to (a writer recording the gains).
    /// The GUI calls it read-only.
    ValIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The port's field name in its node's `ports!`, which saved graphs use to identify the port. Unlike `name`,
        /// the id never changes when the display name does.
        id: &'static str,
        /// The kind of value the port reads.
        value_kind: ValueKind<E>,
        /// Which side of the node the graph draws the port on (see `Side`).
        side: Side,
    },
}

/// The side of a node that the graph draws a port on, so that the port's wires leave the node that way.
///
/// A `ports!` declaration can give a ValCmd port's side with `side: Left` or `side: Right`. Without one, a
/// `ValOutCmdIn` port is on the right and a `ValInCmdOut` or `ValIn` port is on the left. A writer's "after recording"
/// port is declared `side: Right`, for example, because the files it sends go on to later steps, the way data does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

impl<E: Edges> Port<E> {
    /// A `DataStreamIn` port taking `data_stream_kind`.
    pub fn data_stream_in(id: &'static str, name: &'static str, data_stream_kind: DataStreamKind<E>) -> Self {
        Port::DataStreamIn { name, id, data_stream_kind }
    }

    /// A `DataStreamOut` port sending `data_stream_kind`: `stream`, once that's known.
    pub fn data_stream_out(
        id: &'static str,
        name: &'static str,
        data_stream_kind: DataStreamKind<E>,
        stream: Option<E::DataStream>,
    ) -> Self {
        Port::DataStreamOut { name, id, data_stream_kind, stream }
    }

    pub fn role(&self) -> PortRole {
        match self {
            Port::DataStreamIn { .. } => PortRole::DataStreamIn,
            Port::DataStreamOut { .. } => PortRole::DataStreamOut,
            Port::ValOutCmdIn { .. } => PortRole::ValOutCmdIn,
            Port::ValInCmdOut { .. } => PortRole::ValInCmdOut,
            Port::ValIn { .. } => PortRole::ValIn,
        }
    }

    /// The port's id, which saved graphs use to identify the port (see `Port::DataStreamIn::id`).
    pub fn id(&self) -> &'static str {
        match self {
            Port::DataStreamIn { id, .. }
            | Port::DataStreamOut { id, .. }
            | Port::ValOutCmdIn { id, .. }
            | Port::ValInCmdOut { id, .. }
            | Port::ValIn { id, .. } => id,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Port::DataStreamIn { name, .. }
            | Port::DataStreamOut { name, .. }
            | Port::ValOutCmdIn { name, .. }
            | Port::ValInCmdOut { name, .. }
            | Port::ValIn { name, .. } => name,
        }
    }

    /// Which side of the node the graph draws the port on. Data ports have fixed sides: a `DataStreamIn` port is on
    /// the left and a `DataStreamOut` port is on the right. A ValCmd port is on the side its `ports!` declaration
    /// gives (see `Side`).
    pub fn side(&self) -> Side {
        match self {
            Port::DataStreamIn { .. } => Side::Left,
            Port::DataStreamOut { .. } => Side::Right,
            Port::ValOutCmdIn { side, .. } | Port::ValInCmdOut { side, .. } | Port::ValIn { side, .. } => *side,
        }
    }

    /// The port's kind of data stream or value, by which the graph matches ports (see `PortKind`).
    pub fn kind(&self) -> PortKind {
        match self {
            Port::DataStreamIn { data_stream_kind, .. } | Port::DataStreamOut { data_stream_kind, .. } => {
                PortKind::DataStream(E::DataStream::kind_index(*data_stream_kind))
            }
            Port::ValOutCmdIn { value_kind, .. }
            | Port::ValInCmdOut { value_kind, .. }
            | Port::ValIn { value_kind, .. } => PortKind::Value(E::PublishedValue::kind_index(*value_kind)),
        }
    }

    /// A data port's kind of data stream.
    pub fn data_stream_kind(&self) -> Option<DataStreamKind<E>> {
        match self {
            Port::DataStreamIn { data_stream_kind, .. } | Port::DataStreamOut { data_stream_kind, .. } => {
                Some(*data_stream_kind)
            }
            _ => None,
        }
    }

    /// A `DataStreamOut`'s stream, once it's known.
    pub fn stream(&self) -> Option<E::DataStream> {
        match self {
            Port::DataStreamOut { stream, .. } => *stream,
            _ => None,
        }
    }

    /// A ValCmd port's kind of value.
    pub fn value_kind(&self) -> Option<ValueKind<E>> {
        match self {
            Port::ValOutCmdIn { value_kind, .. }
            | Port::ValInCmdOut { value_kind, .. }
            | Port::ValIn { value_kind, .. } => Some(*value_kind),
            _ => None,
        }
    }

    /// The commands a `ValOutCmdIn` accepts, or a `ValInCmdOut` sends; none for the others.
    pub fn commands(&self) -> &[CommandKind<E>] {
        match self {
            Port::ValOutCmdIn { commands, .. } | Port::ValInCmdOut { commands, .. } => commands,
            _ => &[],
        }
    }
}

/// A node has ports of five roles, each named for what flows through it, which way. Data edges run from a
/// `DataStreamOut` to a `DataStreamIn`, carrying a pass's data. ValCmd edges run from a `ValInCmdOut` to a
/// `ValOutCmdIn`: the node at the `ValOutCmdIn` end publishes a value, which the node at the other end reads, and which
/// that node can send commands back to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum PortRole {
    DataStreamIn,
    DataStreamOut,
    /// The node publishes a value here, and takes commands through the edges into it.
    ValOutCmdIn,
    /// The node reads another node's published value here, and sends it commands.
    ValInCmdOut,
    /// Val-only: the node only reads another node's published value here (the GUI calls it read-only).
    ValIn,
}

impl PortRole {
    /// `DataStreamIn` and `DataStreamOut` ports carry data streams; the others, values and commands.
    pub fn carries_data_stream(self) -> bool {
        matches!(self, PortRole::DataStreamIn | PortRole::DataStreamOut)
    }

    /// An edge from a port of this role ends at a port of one of these.
    pub fn partners(self) -> &'static [PortRole] {
        match self {
            PortRole::DataStreamIn => &[PortRole::DataStreamOut],
            PortRole::DataStreamOut => &[PortRole::DataStreamIn],
            PortRole::ValOutCmdIn => &[PortRole::ValInCmdOut, PortRole::ValIn],
            PortRole::ValInCmdOut | PortRole::ValIn => &[PortRole::ValOutCmdIn],
        }
    }

    /// Whether an edge can join a port of this role and one of `other`.
    pub fn pairs_with(self, other: PortRole) -> bool {
        self.partners().contains(&other)
    }

    /// Edges start at a `DataStreamOut`, `ValInCmdOut` or `ValIn`, and end at a `DataStreamIn` or `ValOutCmdIn`. This
    /// says whether they start here.
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

/// The graph finds a port by its node, role and index among the node's ports of that role. Each end of an edge is one,
/// as it's saved, too.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct PortAddress {
    pub node: NodeId,
    pub role: PortRole,
    pub port_idx: usize,
}

/// The graph names what a port carries by which kind of data stream or value it is, as its index in the program's
/// list of them (`EdgeTypesEnum::KINDS`), for the port's name, colour and description (see `edge_info`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PortKind {
    DataStream(usize),
    Value(usize),
}

impl PortKind {
    pub fn carries_data_stream(self) -> bool {
        matches!(self, PortKind::DataStream(_))
    }
}
