//! A node's threads of its own (acquiring from a board, writing files), as the dev tools show them.

use std::sync::Arc;

use super::NodeId;
use crate::core::exchange::{Exchange, Shared};
use crate::core::theme::Colours;

/// A thread a node runs of its own while the graph runs (acquiring from a board, say), shown in the dev tools.
pub struct NodeThread {
    pub name: &'static str,
    /// How the thread and the node's passes hand data to each other, e.g. "block queue".
    pub link: &'static str,
    /// Which way the data goes: to the passes (from a board, say) or from them (to disk).
    pub flow: Flow,
    /// Why it's a thread of its own, rather than done in the passes.
    pub why: &'static str,
    /// Whether the data's buffers are swapped with the passes' own (through a pool: see `Node::thread_pool`), rather
    /// than copied.
    pub swaps: bool,
}

/// Where the buffers are that a node's thread swaps with its passes, rather than copying (see `Node::thread_pool`):
/// so many in the pool, of which some are empty (ready to fill), one's maybe being filled (or, by a writer,
/// written), and some are full (waiting to be taken by a pass, or written). Swaps move buffers between the pool
/// and the workers', which aren't counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStatus {
    pub size: usize,
    /// Each buffer's memory (for a source's, all its outputs for a pass, together).
    pub bytes: usize,
    /// Which of the node's ports the buffers are like, a bit each: its outputs (for a source) or inputs (for a
    /// writer).
    pub ports: u64,
    pub empty: usize,
    pub filling: bool,
    pub full: usize,
    /// Swaps with the passes so far.
    pub swaps: u64,
    /// Data lost because the pool ran out of buffers: the passes, or the disk, not keeping up.
    pub overflows: u64,
}

/// Which way data goes between a node's thread and its passes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    ToPasses,
    FromPasses,
}

/// What a node's drawing of one of its threads (in the dev tools) gets to work with.
pub struct ThreadContext<'a> {
    pub theme: &'a Colours,
    pub node: NodeId,
    /// What the node's passes and window share (see `crate::core::exchange`), while running.
    pub exchange: Option<&'a Exchange>,
}

impl ThreadContext<'_> {
    /// The node's shared `T` (see `crate::core::exchange`), if it's running and its passes have made one.
    pub fn shared<T: Shared>(&self) -> Option<Arc<T>> {
        self.exchange?.get(self.node)
    }
}
