//! The basic view offers quick actions to people who'd rather not build the graph node by node: ready-made changes to
//! it ("add spike tracking", say). Each is a pure function: given the graph (with all its settings) and
//! the layout of panes, it gives the graph and the layout to change to (a changed copy of the one it was given, made
//! with `Layout`'s helpers: opening windows where they'd go, say, or arranging them all); or nothing, if it doesn't
//! apply (it's already been done, say), when it isn't offered. It says what it did (`QuickChange::summary`), which
//! is logged. The program supplies its quick actions (see `Components`), through `Components`.

use super::engine::{Graph, NodesEnum};
pub use super::layout::{Arrangement, Layout, Tab};

/// A quick action has a name and description, as offered, and what it does.
pub struct QuickAction<N: NodesEnum> {
    pub name: &'static str,
    pub description: &'static str,
    pub run: fn(&QuickState<N>) -> Option<QuickChange<N>>,
}

/// A quick action works from this.
pub struct QuickState<'a, N: NodesEnum> {
    pub graph: &'a Graph<N>,
    /// The panes, and where they are.
    pub layout: &'a Layout,
}

/// A quick action gives this, saying what it does.
pub struct QuickChange<N: NodesEnum> {
    pub graph: Graph<N>,
    /// The panes, where they're to be: the layout the action was given, changed (windows opened and closed, say, with
    /// `Layout`'s helpers, or arranged afresh: see `Layout::arrange`).
    pub layout: Layout,
    /// The action can have the graph laid out afresh (see the toolbar's Arrange), as after building it from nothing.
    pub arrange: bool,
    /// The log says what the action did, in a line: "added Spike Detector, Spike Viewer; connected 3", say.
    pub summary: String,
    /// The action can choose the base directory to use (see `crate::base_directory`), by its type's name.
    pub base_directory: Option<&'static str>,
}
