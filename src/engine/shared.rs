//! A node's passes run apart from the GUI (the node's window, its work on the GUI's thread, the dev tools), and share
//! with it only what the node type says (`Node::Shared`): pools of buffers, usually (see `crate::standard::pool`),
//! which the passes fill and the GUI takes from. The engine makes one for each node that runs (`Node::shared`), sized
//! for the node's input data streams then, so it's always the right shape, and hands it on from one snapshot to the
//! next while the node's streams stay the same (as it does the node's state: see `crate::engine::run`). Once a run's
//! stopped, the app keeps the last run's, so what's shown stays, until the next run or a change to the graph that would
//! need new ones.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use super::{NodeId, PoolStatus};

/// Each node type's `Node::Shared`, what its passes share with the GUI, is one of these.
pub trait Shared: Any + Send + Sync {
    /// The dev tools show the pools of buffers in what's shared, each with a name, which this lists.
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        Vec::new()
    }
}

impl Shared for () {}

/// The engine keeps what each node that runs shares with the GUI here, by node.
#[derive(Default)]
pub struct SharedByNode(pub(super) HashMap<NodeId, Arc<dyn Shared>>);

impl SharedByNode {
    /// What node `node` shares, if it runs, and it's a `T` (its `Node::Shared`).
    pub fn get<T: Shared>(&self, node: NodeId) -> Option<&T> {
        self.any(node)?.downcast_ref()
    }

    /// What node `node` shares, as whatever it is (for its passes).
    pub(super) fn any(&self, node: NodeId) -> Option<&dyn Any> {
        let shared: &dyn Shared = self.0.get(&node)?.as_ref();
        Some(shared)
    }

    /// Tests trying a window out can have node `node` share `shared`, with this.
    pub fn with(mut self, node: NodeId, shared: impl Shared) -> Self {
        self.0.insert(node, Arc::new(shared));
        self
    }

    /// The dev tools show node `node`'s pools, and where their buffers are, from this.
    pub fn pools(&self, node: NodeId) -> Vec<(&'static str, PoolStatus)> {
        self.0.get(&node).map(|shared| shared.pools()).unwrap_or_default()
    }
}
