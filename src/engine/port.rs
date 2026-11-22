//! Edges connect nodes at their ports. A node declares its ports (`Port`), each in one of five roles (`PortRole`), and
//! the graph finds a port by its node, role and index (`PortAddress`).

use serde::{Deserialize, Serialize};

use super::{CommandKind, Edges, Kind, NodeId};

/// Edges connect nodes at their ports, each named for what flows through it, which way (see `PortRole`, which names a
/// port's role without what's in it). A node declares its ports on its type (see `crate::engine::ports`), and the graph
/// lists them as these (see `NodesEnum::list_ports_data_stream_in`, `list_ports_data_stream_out` and
/// `list_ports_val_cmd`). The graph numbers the ports of each role separately, in the order they're declared (see
/// `PortAddress`). The node itself numbers its `ValInCmdOut` and `ValIn` ports together, in the order they're
/// declared, as the ports it reads values through (see `ReadsValIn::index`).
pub enum Port<E: Edges> {
    /// Data reaches the node here, each pass, from another node's `DataStreamOut`.
    DataStreamIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// Saved graphs name the port by this: its field's name in its node's `ports!`, which never changes as its
        /// `name` might.
        id: &'static str,
        /// The kind of data stream the port takes.
        data_stream_kind: Kind<E::DataStream>,
    },
    /// The node sends data on from here, each pass.
    DataStreamOut {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// Saved graphs name the port by this: its field's name in its node's `ports!`, which never changes as its
        /// `name` might.
        id: &'static str,
        /// The kind of data stream the port sends.
        data_stream_kind: Kind<E::DataStream>,
        /// The stream itself, which the graph can't always know (while the node's input isn't connected, say), and
        /// then this is `None`.
        stream: Option<E::DataStream>,
    },
    /// The node publishes a value here (usually a copy of one of its settings: see `Node::val_out`), and accepts
    /// commands through the edges into it, from other nodes' `ValInCmdOut` ports.
    ValOutCmdIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// Saved graphs name the port by this: its field's name in its node's `ports!`, which never changes as its
        /// `name` might.
        id: &'static str,
        /// The kind of value the port has.
        value_kind: Kind<E::PublishedValue>,
        /// The commands the port accepts.
        commands: Vec<CommandKind<E>>,
        /// Whether it's drawn on the left of the node, rather than the right (as `ports!` declares it: `flipped`).
        flipped: bool,
    },
    /// The node reads the value at the `ValOutCmdIn` this connects to, and sends that node commands (usually the one
    /// setting the value, `SetGains`, say; sometimes others, such as auto detect's `Detect`).
    ValInCmdOut {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// Saved graphs name the port by this: its field's name in its node's `ports!`, which never changes as its
        /// `name` might.
        id: &'static str,
        /// The kind of value the port reads.
        value_kind: Kind<E::PublishedValue>,
        /// The commands the port sends.
        commands: Vec<CommandKind<E>>,
        /// Whether it's drawn on the right of the node, rather than the left (as `ports!` declares it: `flipped`).
        flipped: bool,
    },
    /// Val-only: the node only reads the value at the `ValOutCmdIn` this connects to (a writer recording the gains).
    /// The GUI calls it read-only.
    ValIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// Saved graphs name the port by this: its field's name in its node's `ports!`, which never changes as its
        /// `name` might.
        id: &'static str,
        /// The kind of value the port reads.
        value_kind: Kind<E::PublishedValue>,
        /// Whether it's drawn on the right of the node, rather than the left (as `ports!` declares it: `flipped`).
        flipped: bool,
    },
}

impl<E: Edges> Port<E> {
    /// A `DataStreamIn` port taking `data_stream_kind`.
    pub fn data_stream_in(id: &'static str, name: &'static str, data_stream_kind: Kind<E::DataStream>) -> Self {
        Port::DataStreamIn { name, id, data_stream_kind }
    }

    /// A `DataStreamOut` port sending `data_stream_kind`: `stream`, once that's known.
    pub fn data_stream_out(
        id: &'static str,
        name: &'static str,
        data_stream_kind: Kind<E::DataStream>,
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

    /// Saved graphs name the port by this (see `Port::DataStreamIn::id`).
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

    /// The graph draws `DataStreamOut` and `ValOutCmdIn` ports on the right of the node, and the others on the left,
    /// unless a ValCmd port's flipped (a writer's "after recording" is on the right, as what it sets off comes after
    /// it). This says whether it is; data ports never are.
    pub fn is_flipped(&self) -> bool {
        match self {
            Port::ValOutCmdIn { flipped, .. } | Port::ValInCmdOut { flipped, .. } | Port::ValIn { flipped, .. } => {
                *flipped
            }
            Port::DataStreamIn { .. } | Port::DataStreamOut { .. } => false,
        }
    }

    /// A data port's kind of data stream.
    pub fn data_stream_kind(&self) -> Option<Kind<E::DataStream>> {
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
    pub fn value_kind(&self) -> Option<Kind<E::PublishedValue>> {
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
