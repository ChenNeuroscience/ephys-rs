//! Edges connect nodes on the graph, and each carries one type of thing. A program declares its edge types as it does
//! its node types.
//!
//! There are two dimensions of edge. Data edges carry data streams from outputs to inputs, and must form a DAG.
//! Value edges let one node read a value another publishes, and send that node commands (a viewer reading a
//! detector's thresholds, and setting them, say); they carry no data, so they may point anywhere, including upstream.
//!
//! Each edge type is a struct implementing `EdgeType`: for data, a small `Copy` description of the data stream (its
//! channel count and sample rate, say); for values, the value itself. `edge_types!` gathers them into an enum per
//! dimension, with a matching fieldless `Kind` enum that ports are declared with. It gathers the program's commands into
//! an enum as well (see `CommandsEnum`).

use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

/// The graph describes each edge type by these facts, which never change.
pub struct EdgeInfo {
    pub name: &'static str,
    pub colour: Color32,
    pub description: &'static str,
}

/// Each edge type implements this: what flows along an edge of one kind, a data stream (32 channels at 30 kHz, say) or
/// a published value (a spike detector's thresholds), and how the graph describes it.
pub trait EdgeType: Clone + PartialEq + Debug + Send + Sync + 'static {
    const INFO: EdgeInfo;

    /// The graph shows a short description of each value: "32 ch · 30 kHz", say.
    fn summary(&self) -> String;
}

/// A program's edge types for one dimension form a closed set: an enum with one variant per type, which `edge_types!`
/// makes and implements this for.
pub trait EdgeTypesEnum: Clone + PartialEq + Debug + Send + Sync + 'static {
    /// Ports are matched by which type a value is, without what's in it: its kind.
    type Kind: Copy + Eq + Hash + Debug + 'static;
    /// Each type's facts, by `kind_index`.
    const KINDS: &'static [EdgeInfo];

    fn kind(&self) -> Self::Kind;
    fn kind_index(kind: Self::Kind) -> usize;
    fn summary(&self) -> String;
}

/// A value of `S` is of one edge type, named by its kind (see `EdgeTypesEnum::Kind`).
pub type Kind<S> = <S as EdgeTypesEnum>::Kind;

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

/// A node reading a published value (see `Node::ports`) finds it as its own type, rather than the program's enum of
/// them, through this, which `edge_types!` implements for each type of value `V` holds.
pub trait ValueType<V>: Sized {
    /// `value` as this type, if that's the type it is.
    fn from_value(value: &V) -> Option<&Self>;
}

/// A node can command another through a value edge (or itself, from its passes or threads; see `Node::on_command`).
/// The program declares its commands with `edge_types!`, which makes an enum of them, implementing this. The core adds
/// one command of its own, `ClearTransientState`, which the app sends nodes itself.
pub trait CommandsEnum: Debug + Send + 'static {
    /// Ports say which commands they take or send by their kinds: which command, without what it carries.
    type Kind: Copy + Eq + Hash + Debug + Send + Sync + 'static;

    fn kind(&self) -> Self::Kind;

    /// Some of what a node has been asked to do is only for now (an auto detect that's started, say): the node keeps it
    /// in fields of its own, not saved (`#[serde(skip)]`), and drops it when it's sent this. The app sends it, straight
    /// away, to every node whenever it restores the graph from its history (undo or redo), and starts recording or
    /// stops running; and to a node it duplicates.
    fn clear_transient_state() -> Self;

    /// Whether this is the core's `ClearTransientState`.
    fn is_clear_transient_state(&self) -> bool;
}

/// A program is built with edge types in both dimensions, and the commands sent along value edges; `edge_types!` makes
/// a type implementing this, naming them.
pub trait Edges: 'static {
    /// Data streams flow from outputs to inputs, a pass at a time.
    type DataStream: DataStreamsEnum;
    /// Nodes publish values, which other nodes read through value edges.
    type PublishedValue: EdgeTypesEnum;
    /// Nodes command each other through value edges (see `CommandsEnum`).
    type Command: CommandsEnum;
}

/// Each of the program's commands is of one kind (see `CommandsEnum::Kind`).
pub type CommandKind<E> = <<E as Edges>::Command as CommandsEnum>::Kind;

/// A data edge holds a pass's worth of data in one of these.
pub type Buffer<E> = <<E as Edges>::DataStream as DataStreamsEnum>::Buffer;

/// An edge is of one of two dimensions: data, flowing from an output to an input each pass; or values, from a node
/// reading a value another publishes, and commanding it (see `PortRole`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Dimension {
    #[default]
    Data,
    /// Saved graphs from before it was renamed call it `Settings`.
    #[serde(alias = "Settings")]
    Values,
}

/// A program declares the edge types it's built with, and its commands, like this:
///
/// ```ignore
/// edge_types! {
///     pub struct Edges;
///     data_streams: pub enum DataStream, Kind, Buffer {
///         Continuous(ephys::Continuous),
///     }
///     values: pub enum PublishedValue, ValueKind {
///         Thresholds(spikes::Thresholds),
///     }
///     commands: pub enum Command, CommandKind {
///         SetThresholds(spikes::Thresholds),
///     }
/// }
/// ```
///
/// This makes `Edges` (implementing `engine::Edges`), the `DataStream`, `PublishedValue` and `Command` enums, the
/// `Kind`, `ValueKind` and `CommandKind` enums naming their variants, and the `Buffer` enum of the data streams'
/// buffers. `Command` and `CommandKind` have one more variant, the core's `ClearTransientState` (see `CommandsEnum`).
#[macro_export]
macro_rules! edge_types {
    (
        $vis:vis struct $edges:ident;
        data_streams: $svis:vis enum $data_stream:ident, $kind:ident, $buffer:ident { $($sv:ident($st:ty)),+ $(,)? }
        values: $cvis:vis enum $value:ident, $ckind:ident { $($cv:ident($ct:ty)),+ $(,)? }
        commands: $mvis:vis enum $command:ident, $mkind:ident { $($mv:ident($mt:ty)),+ $(,)? }
    ) => {
        $vis struct $edges;

        impl $crate::engine::Edges for $edges {
            type DataStream = $data_stream;
            type PublishedValue = $value;
            type Command = $command;
        }

        #[derive(Debug)]
        $mvis enum $command {
            $($mv($mt),)+
            /// The core's own (see `CommandsEnum::clear_transient_state`).
            ClearTransientState,
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $mvis enum $mkind {
            $($mv,)+
            ClearTransientState,
        }

        impl $crate::engine::CommandsEnum for $command {
            type Kind = $mkind;

            fn kind(&self) -> $mkind {
                match self {
                    $($command::$mv(_) => $mkind::$mv,)+
                    $command::ClearTransientState => $mkind::ClearTransientState,
                }
            }

            fn clear_transient_state() -> Self {
                $command::ClearTransientState
            }

            fn is_clear_transient_state(&self) -> bool {
                matches!(self, $command::ClearTransientState)
            }
        }

        $(
            impl $crate::engine::ValueType<$value> for $ct {
                fn from_value(value: &$value) -> Option<&Self> {
                    #[allow(unreachable_patterns)]
                    match value {
                        $value::$cv(v) => Some(v),
                        _ => None,
                    }
                }
            }
        )+

        $crate::edge_types!(@set $svis $data_stream $kind $($sv($st)),+);
        $crate::edge_types!(@set $cvis $value $ckind $($cv($ct)),+);

        // Data streams are all `Copy` (see `DataStreamType`), so their enum can be too.
        impl Copy for $data_stream {}

        $svis enum $buffer {
            $($sv(<$st as $crate::engine::DataStreamType>::Buffer)),+
        }

        impl $crate::engine::DataStreamsEnum for $data_stream {
            type Buffer = $buffer;

            fn buffer(&self, duration: f64) -> $buffer {
                match self { $($data_stream::$sv(s) => $buffer::$sv($crate::engine::DataStreamType::buffer(s, duration))),+ }
            }

            fn bytes(buffer: &$buffer) -> usize {
                match buffer { $($buffer::$sv(b) => <$st as $crate::engine::DataStreamType>::bytes(b)),+ }
            }

            fn empty(buffer: &mut $buffer) {
                match buffer { $($buffer::$sv(b) => <$st as $crate::engine::DataStreamType>::empty(b)),+ }
            }

            #[allow(unreachable_patterns)]
            fn copy(from: &$buffer, to: &mut $buffer) {
                match (from, to) {
                    $(($buffer::$sv(from), $buffer::$sv(to)) => <$st as $crate::engine::DataStreamType>::copy(from, to),)+
                    _ => {}
                }
            }
        }
    };

    (@set $vis:vis $value:ident $kind:ident $($variant:ident($ty:ty)),+) => {
        #[derive(Clone, PartialEq, Debug)]
        $vis enum $value {
            $($variant($ty)),+
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $vis enum $kind {
            $($variant),+
        }

        impl $crate::engine::EdgeTypesEnum for $value {
            type Kind = $kind;
            const KINDS: &'static [$crate::engine::EdgeInfo] = &[$(<$ty as $crate::engine::EdgeType>::INFO),+];

            fn kind(&self) -> $kind {
                match self { $($value::$variant(_) => $kind::$variant),+ }
            }

            fn kind_index(kind: $kind) -> usize {
                kind as usize
            }

            fn summary(&self) -> String {
                match self { $($value::$variant(v) => $crate::engine::EdgeType::summary(v)),+ }
            }
        }

        $(
            impl From<$ty> for $value {
                fn from(v: $ty) -> Self {
                    $value::$variant(v)
                }
            }
        )+
    };
}
