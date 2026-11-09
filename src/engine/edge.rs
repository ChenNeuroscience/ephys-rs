//! Edge types: what can flow along an edge, declared by the program like node types are.
//!
//! There are two dimensions of edge. Data edges carry data streams from outputs to inputs, and must form a DAG.
//! Settings edges let one node edit a setting of another (a viewer adjusting a detector's threshold, say);
//! they carry no data, so may point anywhere, including upstream.
//!
//! Each edge type is a small `Copy` struct implementing `EdgeType`: for data, a description of the data stream
//! (e.g. channel count and sample rate); for settings, the setting's value. `edge_types!` gathers them into
//! an enum per dimension, with a matching fieldless `Kind` enum that ports are declared with.

use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

/// Static facts about an edge type.
pub struct EdgeInfo {
    pub name: &'static str,
    pub colour: Color32,
    pub description: &'static str,
}

/// An edge type: what flows along an edge of one kind, a data stream (32 channels at 30 kHz, say) or a setting (a
/// spike detector's thresholds), and how it's described on the graph.
pub trait EdgeType: Clone + PartialEq + Debug + Send + Sync + 'static {
    const INFO: EdgeInfo;

    /// Short description, e.g. "32 ch · 30 kHz".
    fn summary(&self) -> String;
}

/// The closed set of edge types for one dimension: an enum with one variant per type. Implemented by
/// `edge_types!`.
pub trait EdgeTypesEnum: Clone + PartialEq + Debug + Send + Sync + 'static {
    /// Which type a value is, without what's in it: what ports are matched by.
    type Kind: Copy + Eq + Hash + Debug + 'static;
    /// Indexed by `kind_index`.
    const KINDS: &'static [EdgeInfo];

    fn kind(&self) -> Self::Kind;
    fn kind_index(kind: Self::Kind) -> usize;
    fn summary(&self) -> String;
}

/// Which edge type a value of `S` is (see `EdgeTypesEnum::Kind`).
pub type Kind<S> = <S as EdgeTypesEnum>::Kind;

/// A data edge type, which besides describing a data stream says how a pass's worth of it is held. Data streams are
/// small descriptions, so `Copy`; settings may hold more (a list, say), so needn't be.
pub trait DataStreamType: EdgeType + Copy {
    /// A pass's worth of it, made once and reused for every pass.
    type Buffer: Send + Sync + 'static;

    /// An empty buffer with room for `duration` seconds of this data stream. Buffers are made before running, and
    /// reused for every pass.
    fn buffer(&self, duration: f64) -> Self::Buffer;

    /// The memory a buffer has allocated, in bytes.
    fn bytes(buffer: &Self::Buffer) -> usize;

    /// Copies `from` into `to`, a buffer of the same data stream, without allocating (for the plan, copying for a
    /// node taking a buffer it can't be the last to read: see `BufferUsage::takes`).
    fn copy(from: &Self::Buffer, to: &mut Self::Buffer);
}

/// The data edge types, with their buffers. Implemented by `edge_types!`.
pub trait DataStreamsEnum: EdgeTypesEnum + Copy {
    /// A buffer of any of them: an enum, with a variant for each type's.
    type Buffer: Send + Sync + 'static;
    fn buffer(&self, duration: f64) -> Self::Buffer;
    fn bytes(buffer: &Self::Buffer) -> usize;
    /// See `DataStreamType::copy`: nothing, if they're different kinds of buffer.
    fn copy(from: &Self::Buffer, to: &mut Self::Buffer);
}

/// The edge types a program is built with, in both dimensions, and the triggers sent along settings edges.
pub trait Edges: 'static {
    /// What flows from outputs to inputs, a pass at a time.
    type DataStream: DataStreamsEnum;
    /// Values nodes share, through settings edges from a control to a setting.
    type Setting: EdgeTypesEnum;
    /// What can be sent to a node, through a settings edge or by itself (see `crate::triggers`).
    type Trigger: crate::triggers::TriggersEnum;
}

/// Which kind of the program's triggers one is (see `TriggersEnum::Kind`).
pub type TriggerKind<E> = <<E as Edges>::Trigger as crate::triggers::TriggersEnum>::Kind;

/// A pass's worth of data on a data edge.
pub type Buffer<E> = <<E as Edges>::DataStream as DataStreamsEnum>::Buffer;

/// The two kinds of edge: data, flowing from an output to an input each pass; and settings, linking a control to
/// another node's setting, so either can see and change it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Dimension {
    #[default]
    Data,
    Settings,
}

/// Declares the edge types a program is built with:
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
///         AutoDetect(auto_detect::AutoDetect),
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
                const TYPES: &[(&str, $crate::triggers::Delivery)] =
                    &[$((<$tt as $crate::triggers::TriggerType>::NAME, <$tt as $crate::triggers::TriggerType>::DELIVERY)),+];
                let (name, delivery) = TYPES[kind as usize];
                $crate::triggers::TriggerKey { index: kind as usize, name, delivery }
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
