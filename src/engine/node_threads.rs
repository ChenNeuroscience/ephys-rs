//! Some nodes run threads of their own (acquiring from a board, writing files), which the dev tools show. This module
//! describes those threads, and the pools of buffers they swap with the passes.

use super::{NodeId, Shared, SharedByNode};
use crate::theme::Colours;

/// A node can run a thread of its own while the graph runs (acquiring from a board, say). The dev tools show it from
/// this description (see `Node::threads`).
pub struct NodeThread {
    pub name: &'static str,
    /// The thread and the node's passes hand data to each other somehow: "block queue", say.
    pub link: &'static str,
    /// The data goes one way: to the passes (from a board, say) or from them (to disk).
    pub flow: Flow,
    /// The reason the work is on a thread of its own, rather than done in the passes.
    pub why: &'static str,
    /// For some of the node's outputs, the passes swap buffers for ones the thread's filled (through a pool: see
    /// `Node::thread_pool`), rather than copying. These are those outputs. (Data going the other way, to the thread, is
    /// swapped from the inputs the node takes: see `BufferUsage::takes`.)
    pub swapped_outputs: Vec<usize>,
}

/// A node's thread swaps buffers with its passes through a pool, rather than copying (see `Node::thread_pool`). The dev
/// tools show where those buffers are, from this: so many in the pool, of which some are empty (ready to fill), one's
/// maybe being filled (or, by a writer, written), and some are full (waiting to be taken by a pass, or written). Swaps
/// move buffers between the pool and the workers', which aren't counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStatus {
    pub size: usize,
    /// Each buffer takes this much memory (for a source's, all its outputs for a pass, together).
    pub bytes: usize,
    /// The buffers are like some of the node's ports, a bit each: its outputs (for a source) or inputs (for a
    /// writer).
    pub ports: u64,
    pub empty: usize,
    pub filling: bool,
    pub full: usize,
    /// The thread and the passes have swapped this many buffers so far.
    pub swaps: u64,
    /// When the pool runs out of buffers (the passes, or the disk, not keeping up), data's lost; this counts how
    /// often.
    pub overflows: u64,
}

/// Data goes one way between a node's thread and its passes: to the passes, or from them. Or it goes from the GUI's
/// side instead (the node's work on the GUI's thread: see `Node::on_gui_tick`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    ToPasses,
    FromPasses,
    FromGui,
}

/// The dev tools have a node draw what each of its threads is up to (see `Node::thread_ui`), giving it this to work
/// with.
pub struct ThreadContext<'a> {
    pub theme: &'a Colours,
    pub node: NodeId,
    /// While running, what the nodes' passes share with the GUI (see `crate::engine::Shared`).
    pub shared_by_node: Option<&'a SharedByNode>,
}

impl<'a> ThreadContext<'a> {
    /// While the node's running, what its passes share (its `Node::Shared`, which is `T`).
    pub fn shared<T: Shared>(&self) -> Option<&'a T> {
        self.shared_by_node?.get(self.node)
    }
}
