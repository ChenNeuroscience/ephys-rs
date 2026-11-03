//! Quick actions: ready-made changes to the graph, offered in the basic view to people who'd rather not build it
//! node by node ("add spike tracking", say). Each is a pure function: given the graph (with all its settings) and
//! which node windows are open, it gives the graph to change to and the windows to open and close; or nothing, if it doesn't
//! apply to this graph (it's already doing it, say), when it isn't offered. Windows it opens are put somewhere
//! sensible by the layout, beside the home pane; or it can arrange the panes itself (see `Arrangement`). It says
//! what it did (`QuickChange::summary`), which is logged. The program supplies its quick actions (see main.rs), through
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
    /// Node windows to close, if they're open.
    pub close: Vec<NodeId>,
    /// Whether to lay the graph out afresh (see the toolbar's Arrange), as after building it from nothing.
    pub arrange: bool,
    /// Where to put the panes, if it says: replacing the layout, the windows in it opened (and docked).
    pub panes: Option<Arrangement>,
    /// What it did, in a line, for the log: "added Spike Detector, Spike Viewer; connected 3", say.
    pub summary: String,
    /// The base directory to use (see `crate::core::base_directory`), by its type's name, if it says.
    pub base_directory: Option<&'static str>,
}

/// An arrangement of the panes: rows and columns of groups of tabs.
pub enum Arrangement {
    /// Side by side, left to right, each with its share of the width.
    Row(Vec<(f32, Arrangement)>),
    /// One above another, top to bottom, each with its share of the height.
    Column(Vec<(f32, Arrangement)>),
    /// Tabs, the one at `active` in front.
    Tabs { tabs: Vec<Tab>, active: usize },
}

/// A tab in an arrangement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Home,
    /// A node's window.
    Window(NodeId),
    /// The panes docked before that aren't put elsewhere, each as a tab (those hidden staying hidden). Without
    /// this, they're closed.
    Others,
}

impl Arrangement {
    /// Whether it has `tab`.
    pub fn places(&self, tab: Tab) -> bool {
        match self {
            Arrangement::Row(parts) | Arrangement::Column(parts) => parts.iter().any(|(_, part)| part.places(tab)),
            Arrangement::Tabs { tabs, .. } => tabs.contains(&tab),
        }
    }
}
