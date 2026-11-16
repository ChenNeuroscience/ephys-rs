//! Edges connect nodes on the graph, and each carries one type of thing. A program declares its edge types as it does
//! its node types.
//!
//! There are two dimensions of edge. Data edges carry data streams from outputs to inputs, and must form a DAG.
//! Settings edges let one node edit a setting of another (a viewer adjusting a detector's threshold, say); they carry
//! no data, so they may point anywhere, including upstream.
//!
//! Each edge type is a struct implementing `EdgeType`: for data, a small `Copy` description of the data stream (its
//! channel count and sample rate, say); for settings, the setting's value. `edge_types!` gathers them into an enum per
//! dimension, with a matching fieldless `Kind` enum that ports are declared with.

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
/// a setting (a spike detector's thresholds), and how the graph describes it.
pub trait EdgeType: Clone + PartialEq + Debug + Send + Sync + 'static {
    const INFO: EdgeInfo;

    /// The graph shows a short description of each value: "32 ch · 30 kHz", say.
    fn summary(&self) -> String;

    /// Part of a setting can be transient: it has the node do something over a while, with the data going by, rather
    /// than say how the node's set up (when auto detect was started, say: `DetectGains::started`, in the standard
    /// nodes). A transient setting is a setting like any other: a change is a new snapshot, which the node prepares for
    /// (room to sample into, say), it's undoable, and it's locked while recording. While it's set, the node runs (see
    /// `Node::evaluate_tree_shake`), and once the node's passes are done, they clear it themselves, along with whatever
    /// they set (`PassContext::trigger_settings_update`). Being transient, it isn't kept: it isn't saved
    /// (`#[serde(skip)]`), and the app clears it here, for each of a node's settings that other nodes can edit (see
    /// `Node::settings`), whenever it restores a graph from its history (undo or redo), duplicates a node, starts
    /// recording, or stops running (see `Graph::clear_transient`). By default, a setting has none.
    fn clear_transient(&mut self) {}
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
    /// See `EdgeType::clear_transient`.
    fn clear_transient(&mut self);
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

/// A program is built with edge types in both dimensions, and triggers sent along settings edges; `edge_types!` makes a
/// type implementing this, naming them.
pub trait Edges: 'static {
    /// Data streams flow from outputs to inputs, a pass at a time.
    type DataStream: DataStreamsEnum;
    /// Settings are values nodes share, through settings edges from a control to a setting.
    type Setting: EdgeTypesEnum;
    /// Triggers can be sent to a node, through a settings edge or by the node itself (see `crate::triggers`).
    type Trigger: crate::triggers::TriggersEnum;
}

/// Each of the program's triggers is of one kind (see `TriggersEnum::Kind`).
pub type TriggerKind<E> = <<E as Edges>::Trigger as crate::triggers::TriggersEnum>::Kind;

/// A data edge holds a pass's worth of data in one of these.
pub type Buffer<E> = <<E as Edges>::DataStream as DataStreamsEnum>::Buffer;

/// An edge is of one of two dimensions: data, flowing from an output to an input each pass; or settings, linking a
/// control to another node's setting, so either can see and change it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Dimension {
    #[default]
    Data,
    Settings,
}

/// A program declares the edge types it's built with like this:
///
/// ```ignore
/// edge_types! {
///     pub struct Edges;
///     data_streams: pub enum DataStream, Kind, Buffer {
///         Continuous(ephys::Continuous),
///     }
///     settings: pub enum Setting, SettingKind {
///         Thresholds(spikes::Thresholds),
///     }
///     triggers: pub enum Trigger, TriggerKind {
///         RunOnFiles(run_command::RunOnFiles),
///     }
/// }
/// ```
///
/// This makes `Edges` (implementing `engine::Edges`), the `DataStream`, `Setting` and `Trigger` enums of values, the
/// `Kind`, `SettingKind` and `TriggerKind` enums naming their variants, and the `Buffer` enum of the data streams'
/// buffers.
#[macro_export]
macro_rules! edge_types {
    (
        $vis:vis struct $edges:ident;
        data_streams: $svis:vis enum $data_stream:ident, $kind:ident, $buffer:ident { $($sv:ident($st:ty)),+ $(,)? }
        settings: $cvis:vis enum $setting:ident, $ckind:ident { $($cv:ident($ct:ty)),+ $(,)? }
        triggers: $tvis:vis enum $trigger:ident, $tkind:ident { $($tv:ident($tt:ty)),+ $(,)? }
    ) => {
        $vis struct $edges;

        impl $crate::engine::Edges for $edges {
            type DataStream = $data_stream;
            type Setting = $setting;
            type Trigger = $trigger;
        }

        #[derive(Clone, Debug)]
        $tvis enum $trigger {
            $($tv($tt)),+
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $tvis enum $tkind {
            $($tv),+
        }

        impl $crate::triggers::TriggersEnum for $trigger {
            type Kind = $tkind;

            fn kind(&self) -> $tkind {
                match self { $($trigger::$tv(_) => $tkind::$tv),+ }
            }
        }

        impl From<$tkind> for $crate::triggers::TriggerKey {
            fn from(kind: $tkind) -> Self {
                const NAMES: &[&str] = &[$(<$tt as $crate::triggers::TriggerType>::NAME),+];
                $crate::triggers::TriggerKey { index: kind as usize, name: NAMES[kind as usize] }
            }
        }

        $(
            impl From<$tt> for $trigger {
                fn from(t: $tt) -> Self {
                    $trigger::$tv(t)
                }
            }
        )+

        $crate::edge_types!(@set $svis $data_stream $kind $($sv($st)),+);
        $crate::edge_types!(@set $cvis $setting $ckind $($cv($ct)),+);

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

            fn clear_transient(&mut self) {
                match self { $($value::$variant(v) => $crate::engine::EdgeType::clear_transient(v)),+ }
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
