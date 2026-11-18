//! Edges connect nodes at their ports. A node declares its ports (`Port`), each in one of five roles (`PortRole`), and
//! the graph finds a port by its node, role and index (`PortAddress`).

use serde::{Deserialize, Serialize};

use super::{CommandKind, Edges, Kind, NodeId};

/// Edges connect nodes at their ports, each named for what flows through it, which way (see `PortRole`, which names a
/// port's role without what's in it). A node declares its ports as these: its data ports
/// (`Node::list_ports_data_stream_in` and `Node::list_ports_data_stream_out`) and its ValCmd ports
/// (`Node::list_ports_val_cmd`). The graph numbers the ports of each role separately, in the order they're declared
/// (see `PortAddress`). The node itself numbers its `ValInCmdOut` and `ValIn` ports together, in the order they're
/// declared, as the ports it reads values through: `UiContext::val_in` and the like take that number.
pub enum Port<E: Edges> {
    /// Data reaches the node here, each pass, from another node's `DataStreamOut`.
    DataStreamIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The kind of data stream the port takes.
        data_stream_kind: Kind<E::DataStream>,
    },
    /// The node sends data on from here, each pass.
    DataStreamOut {
        /// What the graph and the node's window call the port.
        name: &'static str,
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
        /// The kind of value the port has.
        value_kind: Kind<E::PublishedValue>,
        /// The commands the port accepts.
        commands: &'static [CommandKind<E>],
        /// Whether it's drawn on the left of the node, rather than the right (see `Port::flipped`).
        flipped: bool,
    },
    /// The node reads the value at the `ValOutCmdIn` this connects to, and sends that node commands (usually the one
    /// setting the value, `SetGains`, say; sometimes others, such as auto detect's `Detect`).
    ValInCmdOut {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The kind of value the port reads.
        value_kind: Kind<E::PublishedValue>,
        /// The commands the port sends.
        commands: &'static [CommandKind<E>],
        /// Whether it's drawn on the right of the node, rather than the left (see `Port::flipped`).
        flipped: bool,
    },
    /// Val-only: the node only reads the value at the `ValOutCmdIn` this connects to (a writer recording the gains).
    /// The GUI calls it read-only.
    ValIn {
        /// What the graph and the node's window call the port.
        name: &'static str,
        /// The kind of value the port reads.
        value_kind: Kind<E::PublishedValue>,
        /// Whether it's drawn on the right of the node, rather than the left (see `Port::flipped`).
        flipped: bool,
    },
}

impl<E: Edges> Port<E> {
    /// A `DataStreamIn` port taking `data_stream_kind`.
    pub fn data_stream_in(name: &'static str, data_stream_kind: Kind<E::DataStream>) -> Self {
        Port::DataStreamIn { name, data_stream_kind }
    }

    /// A `DataStreamOut` port sending `data_stream_kind`: `stream`, once that's known.
    pub fn data_stream_out(
        name: &'static str,
        data_stream_kind: Kind<E::DataStream>,
        stream: Option<E::DataStream>,
    ) -> Self {
        Port::DataStreamOut { name, data_stream_kind, stream }
    }

    /// A `ValOutCmdIn` port, with a value of kind `value_kind`, accepting `commands`.
    pub fn val_out_cmd_in(
        name: &'static str,
        value_kind: Kind<E::PublishedValue>,
        commands: &'static [CommandKind<E>],
    ) -> Self {
        Port::ValOutCmdIn { name, value_kind, commands, flipped: false }
    }

    /// A `ValInCmdOut` port, reading a value of kind `value_kind`, sending `commands`.
    pub fn val_in_cmd_out(
        name: &'static str,
        value_kind: Kind<E::PublishedValue>,
        commands: &'static [CommandKind<E>],
    ) -> Self {
        Port::ValInCmdOut { name, value_kind, commands, flipped: false }
    }

    /// A `ValIn` port (val-only), reading a value of kind `value_kind`.
    pub fn val_in(name: &'static str, value_kind: Kind<E::PublishedValue>) -> Self {
        Port::ValIn { name, value_kind, flipped: false }
    }

    /// The graph draws `DataStreamOut` and `ValOutCmdIn` ports on the right of the node, and the others on the left,
    /// unless a ValCmd port's flipped (a writer's "after recording" is on the right, as what it sets off comes after
    /// it). This is the same port, flipped (data ports aren't).
    pub fn flipped(self) -> Self {
        match self {
            Port::ValOutCmdIn { name, value_kind, commands, .. } => {
                Port::ValOutCmdIn { name, value_kind, commands, flipped: true }
            }
            Port::ValInCmdOut { name, value_kind, commands, .. } => {
                Port::ValInCmdOut { name, value_kind, commands, flipped: true }
            }
            Port::ValIn { name, value_kind, .. } => Port::ValIn { name, value_kind, flipped: true },
            data => data,
        }
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

    pub fn name(&self) -> &'static str {
        match self {
            Port::DataStreamIn { name, .. }
            | Port::DataStreamOut { name, .. }
            | Port::ValOutCmdIn { name, .. }
            | Port::ValInCmdOut { name, .. }
            | Port::ValIn { name, .. } => name,
        }
    }

    /// Whether a ValCmd port's flipped (see `flipped`); data ports never are.
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
    pub fn commands(&self) -> &'static [CommandKind<E>] {
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
