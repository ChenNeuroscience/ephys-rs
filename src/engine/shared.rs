//! What a node's passes share with the GUI: its window, its work on the GUI's thread, and the dev tools. Each node type
//! says what (`Node::Shared`): pools of buffers, usually (see `crate::standard::pool`), which the passes
//! fill and the GUI takes from. One's made for each node that runs, with each epoch (`Node::shared`), sized for its
//! inputs' data streams then, so it's always the right shape. Once a run's stopped, the app keeps its last epoch's, so
//! what's shown stays, until the next run or a change to the graph that would start a new epoch.

use std::any::Any;
use std::collections::HashMap;

use super::{NodeId, PoolStatus};

/// What a node's passes share with the GUI (see `Node::Shared`).
pub trait Shared: Any + Send + Sync {
    /// Its pools of buffers, each with a name, for the dev tools to show.
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        Vec::new()
    }
}

impl Shared for () {}

/// What each node that runs in an epoch shares, by node.
#[derive(Default)]
pub struct SharedByNode(pub(super) HashMap<NodeId, Box<dyn Shared>>);

impl SharedByNode {
    /// Node `node`'s, if it runs, and it's a `T` (its `Node::Shared`).
    pub fn get<T: Shared>(&self, node: NodeId) -> Option<&T> {
        self.any(node)?.downcast_ref()
    }

    /// Node `node`'s, as whatever it is.
    pub(super) fn any(&self, node: NodeId) -> Option<&dyn Any> {
        let shared: &dyn Shared = self.0.get(&node)?.as_ref();
        Some(shared)
    }

    /// With node `node`'s as `shared` (for trying a window out, say).
    pub fn with(mut self, node: NodeId, shared: impl Shared) -> Self {
        self.0.insert(node, Box::new(shared));
        self
    }

    /// Node `node`'s pools, and where their buffers are, for the dev tools.
    pub fn pools(&self, node: NodeId) -> Vec<(&'static str, PoolStatus)> {
        self.0.get(&node).map(|shared| shared.pools()).unwrap_or_default()
    }
}
