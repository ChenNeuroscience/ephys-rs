//! The graph engine: the `Node` trait each node type implements, the `EdgeType` trait each edge type
//! implements, and the `Graph` of node instances.
//!
//! The engine is generic over the closed lists of node and edge types the program is compiled with,
//! declared with `node_set!` and `edge_types!`. Dispatch to a type is a plain `match`, with no dynamic
//! loading.

mod edge;
mod graph;
mod node;

pub use edge::*;
pub use graph::*;
pub use node::*;
