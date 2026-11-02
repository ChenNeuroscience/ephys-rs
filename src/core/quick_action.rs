//! Quick actions: ready-made changes to the graph, offered in the basic view to people who'd rather not build it
//! node by node ("add spike tracking", say). Each is a pure function: given the graph (with all its settings) and
//! which node windows are open, it gives the graph to change to and the windows to open; or nothing, if it doesn't
//! apply to this graph (it's already doing it, say), when it isn't offered. Windows it opens are put somewhere
//! sensible by the layout, beside the main pane. The program supplies its quick actions (see main.rs), through
//! `Components`.

use std::collections::HashSet;

use super::engine::{Graph, NodeId, NodeSet};

pub struct QuickAction<N: NodeSet> {
    pub name: &'static str,
    pub description: &'static str,
    pub run: fn(&QuickState<N>) -> Option<QuickChange<N>>,
}

/// What a quick action works from.
pub struct QuickState<'a, N: NodeSet> {
    pub graph: &'a Graph<N>,
    /// The nodes whose windows are open.
    pub open_windows: &'a HashSet<NodeId>,
}

/// What a quick action does.
pub struct QuickChange<N: NodeSet> {
    pub graph: Graph<N>,
    /// Node windows to open, in order.
    pub open: Vec<NodeId>,
}
