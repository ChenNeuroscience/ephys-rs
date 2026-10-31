//! Edge types: what can flow along an edge, declared by the program like node types are.
//!
//! There are two dimensions of edge. Data edges carry signals from outputs to inputs, and must form a DAG.
//! Settings edges let one node edit a setting of another (a viewer adjusting a detector's threshold, say);
//! they carry no data, so may point anywhere, including upstream.
//!
//! Each edge type is a small `Copy` struct implementing `EdgeType`: for data, a description of the signal
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

pub trait EdgeType: Copy + PartialEq + Debug + 'static {
    const INFO: EdgeInfo;

    /// Short description, e.g. "32 ch · 30 kHz".
    fn summary(&self) -> String;
}

/// The closed set of edge types for one dimension: an enum with one variant per type. Implemented by
/// `edge_types!`.
pub trait EdgeSet: Copy + PartialEq + Debug + 'static {
    type Kind: Copy + Eq + Hash + Debug + 'static;
    /// Indexed by `kind_index`.
    const KINDS: &'static [EdgeInfo];

    fn kind(&self) -> Self::Kind;
    fn kind_index(kind: Self::Kind) -> usize;
    fn summary(&self) -> String;
}

pub type Kind<S> = <S as EdgeSet>::Kind;

/// The edge types a program is built with, in both dimensions.
pub trait Edges: 'static {
    type Signal: EdgeSet;
    type Setting: EdgeSet;
}

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
///     signals: pub enum Signal, Kind {
///         Continuous(signals::Continuous),
///     }
///     settings: pub enum Setting, SettingKind {
///         Threshold(settings::Threshold),
///     }
/// }
/// ```
///
/// This makes `Edges` (implementing `engine::Edges`), the `Signal` and `Setting` enums of values, and the
/// `Kind` and `SettingKind` enums naming their variants.
#[macro_export]
macro_rules! edge_types {
    (
        $vis:vis struct $edges:ident;
        signals: $svis:vis enum $signal:ident, $kind:ident { $($sv:ident($st:ty)),+ $(,)? }
        settings: $cvis:vis enum $setting:ident, $ckind:ident { $($cv:ident($ct:ty)),+ $(,)? }
    ) => {
        $vis struct $edges;

        impl $crate::engine::Edges for $edges {
            type Signal = $signal;
            type Setting = $setting;
        }

        $crate::edge_types!(@set $svis $signal $kind $($sv($st)),+);
        $crate::edge_types!(@set $cvis $setting $ckind $($cv($ct)),+);
    };

    (@set $vis:vis $value:ident $kind:ident $($variant:ident($ty:ty)),+) => {
        #[derive(Clone, Copy, PartialEq, Debug)]
        $vis enum $value {
            $($variant($ty)),+
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $vis enum $kind {
            $($variant),+
        }

        impl $crate::engine::EdgeSet for $value {
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
