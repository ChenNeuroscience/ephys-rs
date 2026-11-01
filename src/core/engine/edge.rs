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

pub trait EdgeType: Clone + PartialEq + Debug + Send + Sync + 'static {
    const INFO: EdgeInfo;

    /// Short description, e.g. "32 ch · 30 kHz".
    fn summary(&self) -> String;
}

/// The closed set of edge types for one dimension: an enum with one variant per type. Implemented by
/// `edge_types!`.
pub trait EdgeSet: Clone + PartialEq + Debug + Send + Sync + 'static {
    type Kind: Copy + Eq + Hash + Debug + 'static;
    /// Indexed by `kind_index`.
    const KINDS: &'static [EdgeInfo];

    fn kind(&self) -> Self::Kind;
    fn kind_index(kind: Self::Kind) -> usize;
    fn summary(&self) -> String;
}

pub type Kind<S> = <S as EdgeSet>::Kind;

/// A data edge type, which besides describing a data stream says how a pass's worth of it is held. Data streams are
/// small descriptions, so `Copy`; settings may hold more (a list, say), so needn't be.
pub trait DataStreamType: EdgeType + Copy {
    type Buffer: Send + Sync + 'static;

    /// An empty buffer with room for `duration` seconds of this data stream. Buffers are made before running, and
    /// reused for every pass.
    fn buffer(&self, duration: f64) -> Self::Buffer;

    /// The memory a buffer has allocated, in bytes.
    fn bytes(buffer: &Self::Buffer) -> usize;
}

/// The data edge types, with their buffers. Implemented by `edge_types!`.
pub trait DataStreamSet: EdgeSet + Copy {
    type Buffer: Send + Sync + 'static;
    fn buffer(&self, duration: f64) -> Self::Buffer;
    fn bytes(buffer: &Self::Buffer) -> usize;
}

/// The edge types a program is built with, in both dimensions.
pub trait Edges: 'static {
    type DataStream: DataStreamSet;
    type Setting: EdgeSet;
}

/// A pass's worth of data on a data edge.
pub type Buffer<E> = <<E as Edges>::DataStream as DataStreamSet>::Buffer;

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
/// }
/// ```
///
/// This makes `Edges` (implementing `engine::Edges`), the `DataStream` and `Setting` enums of values, the `Kind`
/// and `SettingKind` enums naming their variants, and the `Buffer` enum of the data streams' buffers.
#[macro_export]
macro_rules! edge_types {
    (
        $vis:vis struct $edges:ident;
        data_streams: $svis:vis enum $data_stream:ident, $kind:ident, $buffer:ident { $($sv:ident($st:ty)),+ $(,)? }
        settings: $cvis:vis enum $setting:ident, $ckind:ident { $($cv:ident($ct:ty)),+ $(,)? }
    ) => {
        $vis struct $edges;

        impl $crate::core::engine::Edges for $edges {
            type DataStream = $data_stream;
            type Setting = $setting;
        }

        $crate::edge_types!(@set $svis $data_stream $kind $($sv($st)),+);
        $crate::edge_types!(@set $cvis $setting $ckind $($cv($ct)),+);

        // Data streams are all `Copy` (see `DataStreamType`), so their enum can be too.
        impl Copy for $data_stream {}

        $svis enum $buffer {
            $($sv(<$st as $crate::core::engine::DataStreamType>::Buffer)),+
        }

        impl $crate::core::engine::DataStreamSet for $data_stream {
            type Buffer = $buffer;

            fn buffer(&self, duration: f64) -> $buffer {
                match self { $($data_stream::$sv(s) => $buffer::$sv($crate::core::engine::DataStreamType::buffer(s, duration))),+ }
            }

            fn bytes(buffer: &$buffer) -> usize {
                match buffer { $($buffer::$sv(b) => <$st as $crate::core::engine::DataStreamType>::bytes(b)),+ }
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

        impl $crate::core::engine::EdgeSet for $value {
            type Kind = $kind;
            const KINDS: &'static [$crate::core::engine::EdgeInfo] = &[$(<$ty as $crate::core::engine::EdgeType>::INFO),+];

            fn kind(&self) -> $kind {
                match self { $($value::$variant(_) => $kind::$variant),+ }
            }

            fn kind_index(kind: $kind) -> usize {
                kind as usize
            }

            fn summary(&self) -> String {
                match self { $($value::$variant(v) => $crate::core::engine::EdgeType::summary(v)),+ }
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
