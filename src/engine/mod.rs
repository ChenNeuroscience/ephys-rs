//! The graph engine runs a graph of nodes: each node type implements the `Node` trait, each edge type the `EdgeType`
//! trait, and the `Graph` holds the node instances and the edges between them.
//!
//! The engine is generic over the closed lists of node and edge types a program is compiled with, which the program
//! declares with `nodes_enum!` and `edge_types!`. The engine reaches a type through a plain `match` (enum dispatch),
//! with no dynamic loading.

mod data_ports;
mod edge;
mod graph;
mod node;
mod node_threads;
mod nodes_enum;
mod pass_context;
mod plan;
mod port;
mod ports;
mod prepare_context;
mod run;
mod shared;
mod snapshot_lite;
mod ui_context;
mod val_cmd_ports;

pub use data_ports::*;
pub use edge::*;
pub use graph::*;
pub use node::*;
pub use node_threads::*;
pub use nodes_enum::*;
pub use pass_context::*;
pub use plan::*;
pub use port::*;
pub use ports::*;
pub use prepare_context::*;
pub use run::*;
pub use shared::*;
pub use snapshot_lite::*;
pub use ui_context::*;
pub use val_cmd_ports::*;
