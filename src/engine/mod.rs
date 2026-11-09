//! The graph engine: the `Node` trait each node type implements, the `EdgeType` trait each edge type
//! implements, and the `Graph` of node instances.
//!
//! The engine is generic over the closed lists of node and edge types the program is compiled with,
//! declared with `nodes_enum!` and `edge_types!`. Dispatch to a type is a plain `match` (enum dispatch), with no
//! dynamic loading.

mod edge;
mod graph;
mod node;
mod node_threads;
mod nodes_enum;
mod pass;
mod plan;
mod run;
mod shared;
mod ui_context;

pub use edge::*;
pub use graph::*;
pub use node::*;
pub use node_threads::*;
pub use nodes_enum::*;
pub use pass::*;
pub use plan::*;
pub use run::*;
pub use shared::*;
pub use ui_context::*;
