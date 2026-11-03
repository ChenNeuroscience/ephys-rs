//! Quick actions: ready-made changes to the graph, offered in the basic view to people who'd rather not build it
//! node by node ("add spike tracking", say). Each is a pure function: given the graph (with all its settings) and
//! the layout of panes, it gives the graph and the layout to change to (a changed copy of the one it was given, made
//! with `Layout`'s helpers: opening windows where they'd go, say, or arranging them all); or nothing, if it doesn't
//! apply (it's already been done, say), when it isn't offered. It says what it did (`QuickChange::summary`), which
//! is logged. The program supplies its quick actions (see `Components`), through `Components`.

use super::engine::{Graph, NodeSet};
pub use super::layout::{Arrangement, Layout, Tab};

pub struct QuickAction<N: NodeSet> {
    pub name: &'static str,
    pub description: &'static str,
    pub run: fn(&QuickState<N>) -> Option<QuickChange<N>>,
}

/// What a quick action works from.
pub struct QuickState<'a, N: NodeSet> {
    pub graph: &'a Graph<N>,
    /// The panes, and where they are.
    pub layout: &'a Layout,
}

/// What a quick action does.
pub struct QuickChange<N: NodeSet> {
    pub graph: Graph<N>,
    /// The panes, where they're to be: the layout it was given, changed (windows opened and closed, say, with
    /// `Layout`'s helpers, or arranged afresh: see `Layout::arrange`).
    pub layout: Layout,
    /// Whether to lay the graph out afresh (see the toolbar's Arrange), as after building it from nothing.
    pub arrange: bool,
    /// What it did, in a line, for the log: "added Spike Detector, Spike Viewer; connected 3", say.
    pub summary: String,
    /// The base directory to use (see `crate::core::base_directory`), by its type's name, if it says.
    pub base_directory: Option<&'static str>,
}
