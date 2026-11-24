//! Edges connect nodes on the graph, and each carries one type of thing. A program declares its edge types as it does
//! its node types.
//!
//! There are two kinds of edge, by the roles of the ports they join (see `PortRole`). Data edges carry data streams
//! from outputs to inputs, and must form a DAG. ValCmd edges let one node read another's value out, and send that
//! node commands (a viewer reading a detector's thresholds, and setting them, say); they carry no data, so they may
//! point anywhere, including upstream.
//!
//! Each edge type is a struct implementing `EdgeType`: for data, a small `Copy` description of the data stream (its
//! channel count and sample rate, say); for values, the value itself. Each program lists the edge types it
//! uses with `edge_types!`, which gathers them into one enum of data streams and one of values, each with a matching
//! fieldless enum of kinds that the graph matches ports by. `edge_types!` also gathers the program's commands into an
//! enum (see `CommandsEnum`).

use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::Color32;

/// The graph describes each edge type by these facts, which never change.
pub struct EdgeInfo {
    pub name: &'static str,
    pub colour: Color32,
    pub description: &'static str,
}

/// Each edge type implements this: what flows along an edge of one kind, a data stream (32 channels at 30 kHz, say) or
/// a value out (a spike detector's thresholds), and how the graph describes it.
pub trait EdgeType: Clone + PartialEq + Debug + Send + Sync + 'static {
    const INFO: EdgeInfo;

    /// The graph shows a short description of each value: "32 ch · 30 kHz", say.
    fn summary(&self) -> String;
}

/// A program's data streams form a closed set, and so do its values: each an enum with one variant per type, which
/// `edge_types!` makes and implements this for.
pub trait EdgeTypesEnum: Clone + PartialEq + Debug + Send + Sync + 'static {
    /// Ports are matched by which type a value is, without what's in it: its kind.
    type Kind: Copy + Eq + Hash + Debug + 'static;
    /// Each type's facts, by `kind_index`.
    const KINDS: &'static [EdgeInfo];

    fn kind(&self) -> Self::Kind;
    fn kind_index(kind: Self::Kind) -> usize;
    fn summary(&self) -> String;
}

/// Each data edge type implements this as well: besides describing a data stream, a data edge type says how a pass's
/// worth of the stream is held. Data streams are small descriptions, so `Copy`; settings may hold more (a list, say),
/// so needn't be.
pub trait DataStreamType: EdgeType + Copy {
    /// A pass's worth of the data stream is held in one of these, made once and reused for every pass.
    type Buffer: Send + Sync + 'static;

    /// The plan makes buffers before running, each with room for `duration` seconds of the data stream, and reuses
    /// them for every pass. This makes one, empty.
    fn buffer(&self, duration: f64) -> Self::Buffer;

    /// The dev tools show how much memory a buffer has allocated, in bytes, from this.
    fn bytes(buffer: &Self::Buffer) -> usize;

    /// The plan copies a buffer for a node that takes it but can't be the last to read it (see `BufferUsage::takes`).
    /// This copies `from` into `to`, a buffer of the same data stream, without allocating.
    fn copy(from: &Self::Buffer, to: &mut Self::Buffer);

    /// A node that failed to prepare doesn't run, and its outputs are emptied, as they'd be from a pass with nothing to
    /// say. This empties a buffer, without allocating.
    fn empty(buffer: &mut Self::Buffer);
}

/// A program's data edge types also have buffers, which `edge_types!` gathers into an enum, implementing this.
pub trait DataStreamsEnum: EdgeTypesEnum + Copy {
    /// A buffer of any of the data edge types: an enum, with a variant for each type's.
    type Buffer: Send + Sync + 'static;
    fn buffer(&self, duration: f64) -> Self::Buffer;
    fn bytes(buffer: &Self::Buffer) -> usize;
    /// See `DataStreamType::copy`. Buffers of different kinds aren't copied.
    fn copy(from: &Self::Buffer, to: &mut Self::Buffer);
    /// See `DataStreamType::empty`.
    fn empty(buffer: &mut Self::Buffer);
}

/// Converts between a data stream type `T` and the program's enums of data streams and buffers. A node declares its
/// data ports by their data stream types (see `ports!`), while the engine stores streams and buffers as the program's
/// enums. `edge_types!` implements this trait on the program's `Edges` for each data stream type it lists. A node
/// written for any program states the types it needs as bounds on the program's edges, such as
/// `E: HasDataStream<Continuous>` (or the `PortEdges` trait that `ports!` generates).
#[diagnostic::on_unimplemented(
    message = "the program's edges have no `{T}` data stream",
    note = "add it to the program's `edge_types!`"
)]
pub trait HasDataStream<T: DataStreamType>: Edges {
    /// The kind of `T`, by which the graph matches ports.
    const KIND: DataStreamKind<Self>;
    /// Converts `stream` to the program's enum of data streams.
    fn wrap(stream: T) -> Self::DataStream;
    /// Converts `stream` back to a `T`, or returns `None` if `stream` holds another type.
    fn from_stream(stream: &Self::DataStream) -> Option<T>;
    /// Gives `buffer` as a `T`'s buffer, or `None` if `buffer` belongs to another type.
    fn from_buffer(buffer: &Buffer<Self>) -> Option<&T::Buffer>;
    /// Gives `buffer` as a `T`'s buffer to fill, or `None` if `buffer` belongs to another type.
    fn from_buffer_mut(buffer: &mut Buffer<Self>) -> Option<&mut T::Buffer>;
}

/// Converts between a value type `T` and the program's enum of values. A node declares its ValCmd ports by their value
/// types (see `ports!`), while the engine stores values as the program's enum. `edge_types!` implements this trait on
/// the program's `Edges` for each value type it lists (see `HasDataStream`).
#[diagnostic::on_unimplemented(
    message = "the program's edges have no `{T}` value",
    note = "add it to the program's `edge_types!`"
)]
pub trait HasValue<T: EdgeType>: Edges {
    /// The kind of `T`, by which the graph matches ports.
    const KIND: ValueKind<Self>;
    /// Converts `value` to the program's enum of values.
    fn wrap(value: T) -> Self::ValOut;
    /// Converts `value` back to a `T`, or returns `None` if `value` holds another type.
    fn from_value(value: &Self::ValOut) -> Option<&T>;
}

/// A node can command another through a ValCmd edge (or itself, from its passes or threads; see `Node::on_command`).
/// The program's enum of all commands. Each command is a type of its own (such as `SetGains`, which holds the gains),
/// and `edge_types!` generates this enum from the commands the program lists.
pub trait CommandsEnum: Debug + Send + 'static {
    /// Ports say which commands they take or send by their kinds: which command, without what it carries.
    type Kind: Copy + Eq + Hash + Debug + Send + Sync + 'static;

    fn kind(&self) -> Self::Kind;
}

/// Converts between a command type `T` and the program's enum of commands. Nodes send and receive commands as their
/// own types, while the engine carries them as the program's enum. `edge_types!` implements this trait on the
/// program's `Edges` for each command it lists (see `HasDataStream`).
#[diagnostic::on_unimplemented(
    message = "the program's edges have no `{T}` command",
    note = "add it to the program's `edge_types!`"
)]
pub trait HasCommand<T: Debug + Send + 'static>: Edges {
    /// The kind of `T`, by which ports list the commands they accept or send.
    const KIND: CommandKind<Self>;
    /// Converts `command` to the program's enum of commands.
    fn wrap(command: T) -> Self::Command;
    /// Converts `command` back to a `T`, or returns `command` unchanged if it holds another type.
    fn from_command(command: Self::Command) -> Result<T, Self::Command>;
}

/// A program is built with edge types of both kinds, data streams and values, and the commands sent along ValCmd edges;
/// `edge_types!` makes a type implementing this, naming them.
pub trait Edges: 'static {
    /// Data streams flow from outputs to inputs, a pass at a time.
    type DataStream: DataStreamsEnum;
    /// Nodes offer values out, which other nodes read through ValCmd edges.
    type ValOut: EdgeTypesEnum;
    /// Nodes command each other through ValCmd edges (see `CommandsEnum`).
    type Command: CommandsEnum;
}

/// The program's enum of data stream kinds (see `EdgeTypesEnum::Kind`).
pub type DataStreamKind<E> = <<E as Edges>::DataStream as EdgeTypesEnum>::Kind;

/// The program's enum of value kinds (see `EdgeTypesEnum::Kind`).
pub type ValueKind<E> = <<E as Edges>::ValOut as EdgeTypesEnum>::Kind;

/// The program's enum of command kinds (see `CommandsEnum::Kind`).
pub type CommandKind<E> = <<E as Edges>::Command as CommandsEnum>::Kind;

/// A data edge holds a pass's worth of data in one of these.
pub type Buffer<E> = <<E as Edges>::DataStream as DataStreamsEnum>::Buffer;

/// A program declares the edge types it's built with, and its commands, like this:
///
/// ```ignore
/// edge_types! {
///     pub struct Edges;
///     data_streams: pub enum DataStream, DataStreamKind, Buffer {
///         Continuous(ephys::Continuous),
///     }
///     values: pub enum ValOut, ValueKind {
///         Thresholds(spikes::Thresholds),
///     }
///     commands: pub enum Command, CommandKind {
///         SetThresholds(spikes::SetThresholds),
///     }
/// }
/// ```
///
/// This generates:
///
/// - `Edges`, implementing `engine::Edges`;
/// - the `DataStream`, `ValOut` and `Command` enums;
/// - the `DataStreamKind`, `ValueKind` and `CommandKind` enums, naming the variants of those three;
/// - the `Buffer` enum of the data streams' buffers;
/// - `HasDataStream`, `HasValue` and `HasCommand` on `Edges` for each type listed, which nodes use to work with the
///   types directly.
#[macro_export]
macro_rules! edge_types {
    (
        $vis:vis struct $edges:ident;
        data_streams: $svis:vis enum $data_stream:ident, $kind:ident, $buffer:ident { $($sv:ident($st:ty)),* $(,)? }
        values: $cvis:vis enum $value:ident, $ckind:ident { $($cv:ident($ct:ty)),* $(,)? }
        commands: $mvis:vis enum $command:ident, $mkind:ident { $($mv:ident($mt:ty)),* $(,)? }
    ) => {
        $vis struct $edges;

        impl $crate::engine::Edges for $edges {
            type DataStream = $data_stream;
            type ValOut = $value;
            type Command = $command;
        }

        #[derive(Debug)]
        $mvis enum $command {
            $($mv($mt),)*
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $mvis enum $mkind {
            $($mv,)*
        }

        impl $crate::engine::CommandsEnum for $command {
            type Kind = $mkind;

            fn kind(&self) -> $mkind {
                match *self {
                    $($command::$mv(_) => $mkind::$mv,)*
                }
            }
        }

        $(
            #[allow(unreachable_patterns)]
            impl $crate::engine::HasCommand<$mt> for $edges {
                const KIND: $mkind = $mkind::$mv;

                fn wrap(command: $mt) -> $command {
                    $command::$mv(command)
                }

                fn from_command(command: $command) -> Result<$mt, $command> {
                    match command {
                        $command::$mv(c) => Ok(c),
                        command => Err(command),
                    }
                }
            }

            impl From<$mt> for $command {
                fn from(c: $mt) -> Self {
                    $command::$mv(c)
                }
            }
        )*

        $(
            #[allow(unreachable_patterns)]
            impl $crate::engine::HasValue<$ct> for $edges {
                const KIND: $ckind = $ckind::$cv;

                fn wrap(value: $ct) -> $value {
                    $value::$cv(value)
                }

                fn from_value(value: &$value) -> Option<&$ct> {
                    match value {
                        $value::$cv(v) => Some(v),
                        _ => None,
                    }
                }
            }
        )*

        $crate::edge_types!(@set $svis $data_stream $kind $($sv($st)),*);
        $crate::edge_types!(@set $cvis $value $ckind $($cv($ct)),*);

        // Data streams are all `Copy` (see `DataStreamType`), so their enum can be too.
        impl Copy for $data_stream {}

        $svis enum $buffer {
            $($sv(<$st as $crate::engine::DataStreamType>::Buffer)),*
        }

        $(
            #[allow(unreachable_patterns)]
            impl $crate::engine::HasDataStream<$st> for $edges {
                const KIND: $kind = $kind::$sv;

                fn wrap(stream: $st) -> $data_stream {
                    $data_stream::$sv(stream)
                }

                fn from_stream(stream: &$data_stream) -> Option<$st> {
                    match stream {
                        $data_stream::$sv(s) => Some(*s),
                        _ => None,
                    }
                }

                fn from_buffer(buffer: &$buffer) -> Option<&<$st as $crate::engine::DataStreamType>::Buffer> {
                    match buffer {
                        $buffer::$sv(b) => Some(b),
                        _ => None,
                    }
                }

                fn from_buffer_mut(
                    buffer: &mut $buffer,
                ) -> Option<&mut <$st as $crate::engine::DataStreamType>::Buffer> {
                    match buffer {
                        $buffer::$sv(b) => Some(b),
                        _ => None,
                    }
                }
            }
        )*

        impl $crate::engine::DataStreamsEnum for $data_stream {
            type Buffer = $buffer;

            fn buffer(&self, duration: f64) -> $buffer {
                match self { $($data_stream::$sv(s) => $buffer::$sv($crate::engine::DataStreamType::buffer(s, duration))),* }
            }

            fn bytes(buffer: &$buffer) -> usize {
                match buffer { $($buffer::$sv(b) => <$st as $crate::engine::DataStreamType>::bytes(b)),* }
            }

            fn empty(buffer: &mut $buffer) {
                match buffer { $($buffer::$sv(b) => <$st as $crate::engine::DataStreamType>::empty(b)),* }
            }

            #[allow(unreachable_patterns)]
            fn copy(from: &$buffer, to: &mut $buffer) {
                match (from, to) {
                    $(($buffer::$sv(from), $buffer::$sv(to)) => <$st as $crate::engine::DataStreamType>::copy(from, to),)*
                    _ => {}
                }
            }
        }
    };

    (@set $vis:vis $value:ident $kind:ident $($variant:ident($ty:ty)),*) => {
        #[derive(Clone, PartialEq, Debug)]
        $vis enum $value {
            $($variant($ty)),*
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $vis enum $kind {
            $($variant),*
        }

        impl $crate::engine::EdgeTypesEnum for $value {
            type Kind = $kind;
            const KINDS: &'static [$crate::engine::EdgeInfo] = &[$(<$ty as $crate::engine::EdgeType>::INFO),*];

            fn kind(&self) -> $kind {
                match *self { $($value::$variant(_) => $kind::$variant),* }
            }

            fn kind_index(kind: $kind) -> usize {
                let kinds: &[$kind] = &[$($kind::$variant),*];
                kinds.iter().position(|k| *k == kind).expect("each kind is listed")
            }

            fn summary(&self) -> String {
                match *self { $($value::$variant(ref v) => $crate::engine::EdgeType::summary(v)),* }
            }
        }

        $(
            impl From<$ty> for $value {
                fn from(v: $ty) -> Self {
                    $value::$variant(v)
                }
            }
        )*
    };
}
