//! The basic view offers quick actions to people who'd rather not build the graph node by node: ready-made changes to
//! it ("the standard LTX graph", say). A quick action is data, not code, so people can make, change and share their
//! own: a build ships some (`Components::QUICK_ACTIONS`), and people's own are files in a folder beside the app's
//! settings (see `crate::gui::quick_actions`).
//!
//! A quick action is a small graph, its nodes each a *role* with a rule (`Rule`) for how it's found in the graph it's
//! done on, and what's done to it:
//!
//! - A role's found by its node's type, and optionally a regular expression its label matches: one node, not another
//!   role's already, the left-most on the canvas of those that fit.
//! - A role that's *required* (`Mode::Require`) has to be found, or the quick action isn't offered. One that's
//!   *ensured* (`Mode::Ensure`) is added if it isn't found, as it is in the quick action; one that's only *if present*
//!   (`Mode::IfPresent`) is just left out. One that's *ignored* (`Mode::Ignore`) plays no part, and nor do its edges:
//!   it shows where the rest are, in a quick action made from a whole graph.
//! - A role found can have some of its settings (`Rule::settings`) set as they are in the quick action, and be enabled
//!   or disabled (`Rule::enabled`).
//!
//! Each of the quick action's edges is ensured (connected if the port that takes just the one edge is free), replaced
//! (connected, replacing what was there), removed or ignored (`EdgeMode`), between every node found for each of its ends. Nodes
//! are never removed: a quick action disables them, which keeps their settings for when they're wanted again.
//!
//! A quick action also closes the roles' windows if they're open, and opens them if they aren't (`Window`: a window
//! opened for the first time goes as its `Placement` says), and can arrange the panes (`Plan`), choose the base directory, and
//! have a graph it builds from nothing laid out afresh.
//!
//! The basic view offers a quick action if its required roles are found and doing it would change the graph (or, for
//! one that only opens and closes windows, which are open: see `QuickAction::changes_graph`). So one that's done is no
//! longer offered until something changes.
//!
//! A quick action's file is shaped as a project (`{"graph": {"nodes": ..., "edges": ...}}`), with each node's rule
//! beside it (`"quick"`) and each edge's mode on it, so the app's config patches (see `crate::config_patching`) bring
//! an old one up to date as they would a project. It says the versions that wrote it, as copied settings do.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config_patching::{PatchSets, Versions};
use super::engine::{Edge, Graph, NodeId, NodesEnum, PortAddress, PortRole, SavedPort};
pub use super::layout::{Arrangement, Layout, Tab};
use super::layout::{Pane, Side, WindowState};

/// A quick action's file says it's one, and the versions that wrote it, under this key.
pub const MARKER: &str = "ephys-rs quick action";

/// A role is found, or not, as its rule's mode says.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The quick action is only offered if the role's found.
    Require,
    /// The role's added if it isn't found.
    #[default]
    Ensure,
    /// The role's only acted on if it's found; without it, the quick action is still offered.
    IfPresent,
    /// The role plays no part: never looked for, and its edges and windows left alone. A quick action made from a
    /// whole graph keeps the parts that don't matter to it like this, to show where the parts that do are.
    Ignore,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Ensure, Mode::IfPresent, Mode::Require, Mode::Ignore];

    /// A short name, for the badge under the role's node.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Require => "Required",
            Mode::Ensure => "Added if missing",
            Mode::IfPresent => "Skipped if missing",
            Mode::Ignore => "Ignored",
        }
    }

    /// The choice, as offered in the editor: what's done when the role isn't found (or that it's ignored).
    pub fn choice(self) -> &'static str {
        match self {
            Mode::Require => "Don't offer the quick action",
            Mode::Ensure => "Create the node and apply all settings",
            Mode::IfPresent => "Don't create the node",
            Mode::Ignore => "Ignore this node",
        }
    }
}

/// A role's rule: how it's found, and what's done to it (see the module docs).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub mode: Mode,
    /// Only a node whose label matches this regular expression fits (`^Band-pass`, say). One that can't be read
    /// fits nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_pattern: Option<String>,
    /// The settings, by name, set as they are in the quick action on the nodes found. A node added has all of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<String>,
    /// The nodes found (and added) are enabled, or disabled; or, without this, left as they are (added as the quick
    /// action has them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// What's done with the window of the node found (or added).
    #[serde(skip_serializing_if = "is_default")]
    pub window: Window,
}

/// A role: one of the quick action's nodes, with its rule.
#[derive(Clone, PartialEq, Debug)]
pub struct Role<N> {
    /// The role's id among the quick action's nodes, which its edges and windows name it by.
    pub id: NodeId,
    pub label: String,
    /// Where the role's node is in the quick action, which places it, when added, beside the nodes it's connected to.
    pub pos: [f32; 2],
    pub node: N,
    /// When added, the node's disabled (unless the rule says to enable it).
    pub disabled: bool,
    /// When added, whether the node's listed in the basic view, if the quick action says.
    pub show_in_basic_override: Option<bool>,
    pub rule: Rule,
}

impl<N: NodesEnum> Role<N> {
    /// The role's node's settings, as saved.
    pub fn settings(&self) -> Value {
        settings_of(&self.node)
    }
}

fn settings_of<N: NodesEnum>(node: &N) -> Value {
    serde_json::to_value(node).ok().and_then(|mut v| v.get_mut("settings").map(Value::take)).unwrap_or_default()
}

/// What a quick action does to an edge between the nodes found for its ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeMode {
    /// Connected, if the end that takes just the one edge (a data stream's input, or the reading end of a value) is
    /// free.
    #[default]
    Ensure,
    /// Connected, replacing whatever the end that takes just the one edge was connected to.
    Replace,
    /// Disconnected, if it's there.
    Remove,
    /// Left as it is: kept only to show where the parts that matter are (see `Mode::Ignore`).
    Ignore,
}

impl EdgeMode {
    pub const ALL: [EdgeMode; 4] = [EdgeMode::Ensure, EdgeMode::Replace, EdgeMode::Remove, EdgeMode::Ignore];

    pub fn name(self) -> &'static str {
        match self {
            EdgeMode::Ensure => "Connect if free",
            EdgeMode::Replace => "Connect, replacing",
            EdgeMode::Remove => "Disconnect",
            EdgeMode::Ignore => "Ignore",
        }
    }
}

/// One of the quick action's edges, between roles, by port id, with what's done to it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RoleEdge {
    pub from: SavedPort,
    pub to: SavedPort,
    #[serde(default, skip_serializing_if = "is_default")]
    pub mode: EdgeMode,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// What a quick action does with a role's window: if it's open, closes it, or not; if it isn't, opens it, or not. By
/// default, the window's left as it is.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    /// If the window's open, it's closed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
    /// If the window isn't open, it's opened, placed (the first time) like this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<Placement>,
}

/// A window that's been open before opens where it was. Opened for the first time, it goes here.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Where the app puts a new window (see `Layout::place`).
    #[default]
    Default,
    /// In front, as a tab in the largest group (see `Layout::largest_group`).
    Largest,
    /// Behind another role's window, as a tab in its group.
    Behind(NodeId),
    /// In a group of its own, on one side of another role's window's group, taking a share of the space.
    Beside { of: NodeId, side: Side, share: f32 },
}

impl Placement {
    /// The role whose window the placement is relative to, if any.
    pub fn anchor(self) -> Option<NodeId> {
        match self {
            Placement::Behind(of) | Placement::Beside { of, .. } => Some(of),
            Placement::Default | Placement::Largest => None,
        }
    }
}

/// How the panes are arranged, if the quick action says.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    /// The panes are arranged like this (each window by role), after the windows are opened. Windows of roles not
    /// found are left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<Arrangement>,
}

/// A quick action (see the module docs).
#[derive(Clone, PartialEq, Debug)]
pub struct QuickAction<N> {
    pub name: String,
    pub description: String,
    /// The roles, in the order they're found and added.
    pub roles: Vec<Role<N>>,
    pub edges: Vec<RoleEdge>,
    pub panes: Plan,
    /// The base directory chosen, by its type's name (see `crate::base_directory`).
    pub base_directory: Option<String>,
    /// A graph built from nothing is laid out afresh (see the toolbar's Arrange).
    pub arrange_if_empty: bool,
}

/// A quick action works from this.
pub struct QuickState<'a, N: NodesEnum> {
    pub graph: &'a Graph<N>,
    /// The panes, and where they are.
    pub layout: &'a Layout,
}

/// Doing a quick action gives this.
pub struct QuickChange<N: NodesEnum> {
    pub graph: Graph<N>,
    /// The panes, where they're to be.
    pub layout: Layout,
    /// The graph's laid out afresh (see the toolbar's Arrange), as after building it from nothing.
    pub arrange: bool,
    /// The log says what the action did, in a line: "added Spike Detector, Spike Viewer; connected 3", say.
    pub summary: String,
    /// The base directory to use (see `crate::base_directory`), by its type's name.
    pub base_directory: Option<String>,
}

// The file.

/// A quick action's file, as JSON (see the module docs).
#[derive(Serialize, Deserialize)]
struct File {
    #[serde(rename = "ephys-rs quick action")]
    versions: Versions,
    name: String,
    #[serde(default)]
    description: String,
    graph: FileGraph,
    #[serde(default)]
    panes: Plan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_directory_type: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    arrange_if_empty: bool,
}

/// A quick action's graph, as a project's is saved, with each node's rule and each edge's mode.
#[derive(Serialize, Deserialize)]
struct FileGraph {
    #[serde(default)]
    next_id: u64,
    nodes: BTreeMap<NodeId, FileNode>,
    #[serde(default)]
    edges: Vec<RoleEdge>,
}

#[derive(Serialize, Deserialize)]
struct FileNode {
    label: String,
    #[serde(default)]
    pos: [f32; 2],
    node: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    show_in_basic_override: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    disabled: bool,
    #[serde(default)]
    quick: Rule,
}

/// Whether `text` is a quick action's file (rather than any other text, or JSON).
pub fn is_quick_action(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|v| v.get(MARKER).is_some())
}

impl<N: NodesEnum> QuickAction<N> {
    /// Reads a quick action from its file's text, bringing it up to date with `patches`. A node of a type this
    /// program doesn't have, or settings it can't read, make it unreadable, saying why.
    pub fn parse(text: &str, patches: &PatchSets) -> Result<Self, String> {
        let mut json: Value = serde_json::from_str(text).map_err(|e| format!("Isn't a quick action: {e}"))?;
        let versions: Versions = json
            .get(MARKER)
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| "Isn't a quick action".to_string())?;
        patches.apply_to_project(&versions, &mut json);
        let file: File = serde_json::from_value(json).map_err(|e| format!("Can't be read: {e}"))?;
        let mut roles = Vec::new();
        for (id, entry) in file.graph.nodes {
            let kind = entry.node.get("type").and_then(Value::as_str).unwrap_or("?").to_string();
            let node = serde_json::from_value::<N>(entry.node)
                .map_err(|e| format!("{} ({kind}) can't be read here: {e}", entry.label))?;
            roles.push(Role {
                id,
                label: entry.label,
                pos: entry.pos,
                node,
                disabled: entry.disabled,
                show_in_basic_override: entry.show_in_basic_override,
                rule: entry.quick,
            });
        }
        Ok(QuickAction {
            name: file.name,
            description: file.description,
            roles,
            edges: file.graph.edges,
            panes: file.panes,
            base_directory: file.base_directory_type,
            arrange_if_empty: file.arrange_if_empty,
        })
    }

    /// The quick action's file, saying the versions of `patches`.
    pub fn to_text(&self, patches: &PatchSets) -> String {
        let nodes = self
            .roles
            .iter()
            .map(|role| {
                let entry = FileNode {
                    label: role.label.clone(),
                    pos: role.pos,
                    node: serde_json::to_value(&role.node).unwrap_or_default(),
                    show_in_basic_override: role.show_in_basic_override,
                    disabled: role.disabled,
                    quick: role.rule.clone(),
                };
                (role.id, entry)
            })
            .collect();
        let next_id = self.roles.iter().map(|r| r.id.0 + 1).max().unwrap_or(1);
        let file = File {
            versions: patches.versions(),
            name: self.name.clone(),
            description: self.description.clone(),
            graph: FileGraph { next_id, nodes, edges: self.edges.clone() },
            panes: self.panes.clone(),
            base_directory_type: self.base_directory.clone(),
            arrange_if_empty: self.arrange_if_empty,
        };
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// A quick action made from some of a graph's nodes (`ids`, or all of them, if none): each a role, ensured, with
    /// no settings set on what's found; the edges between them, ensured; and the windows of theirs that are open,
    /// opened, arranged as they are (if `arrange`).
    pub fn from_graph(name: &str, graph: &Graph<N>, layout: &Layout, ids: &[NodeId], arrange: bool) -> Self {
        let chosen: Vec<NodeId> = if ids.is_empty() { graph.ids().collect() } else { ids.to_vec() };
        let open_windows = layout.open_windows();
        let roles: Vec<Role<N>> = chosen
            .iter()
            .filter_map(|id| {
                let entry = graph.get(*id)?;
                Some(Role {
                    id: *id,
                    label: entry.label.clone(),
                    pos: entry.pos,
                    node: entry.node.clone(),
                    disabled: entry.disabled,
                    show_in_basic_override: entry.show_in_basic_override,
                    rule: Rule {
                        window: Window { open: open_windows.contains(id).then_some(Placement::Default), close: false },
                        ..Rule::default()
                    },
                })
            })
            .collect();
        let edges = graph
            .edges()
            .iter()
            .filter(|e| chosen.contains(&e.from.node) && chosen.contains(&e.to.node))
            .filter_map(|e| graph.saved_edge(e))
            .map(|e| RoleEdge { from: e.from, to: e.to, mode: EdgeMode::Ensure })
            .collect();
        let arrangement = arrange
            .then(|| layout.arrangement())
            .flatten()
            .and_then(|a| a.map_windows(&|id| chosen.contains(&id).then_some(id)));
        QuickAction {
            name: name.to_string(),
            description: String::new(),
            roles,
            edges,
            panes: Plan { arrangement },
            base_directory: None,
            arrange_if_empty: false,
        }
    }

    /// The quick action's roles as a graph, each node with the id of its role (so the quick action's edges, windows
    /// and arrangement name them), and the mode of each of the graph's edges. The quick action editor shows and
    /// changes it on a canvas (see `crate::gui::quick_actions`), and puts it back with `set_graph`.
    pub fn graph(&self) -> (Graph<N>, Vec<(Edge, EdgeMode)>) {
        let nodes: serde_json::Map<String, Value> = self
            .roles
            .iter()
            .map(|role| {
                let entry = serde_json::json!({
                    "label": role.label,
                    "pos": role.pos,
                    "node": serde_json::to_value(&role.node).unwrap_or_default(),
                    "show_in_basic_override": role.show_in_basic_override,
                    "disabled": role.disabled,
                });
                (role.id.0.to_string(), entry)
            })
            .collect();
        let saved: Vec<Value> =
            (self.edges.iter()).map(|e| serde_json::json!({ "from": e.from, "to": e.to })).collect();
        let next_id = self.roles.iter().map(|r| r.id.0 + 1).max().unwrap_or(1);
        let file = serde_json::json!({ "next_id": next_id, "nodes": nodes, "edges": saved });
        let graph: Graph<N> = serde_json::from_value(file).unwrap_or_default();
        let modes = (self.edges.iter())
            .filter_map(|e| {
                let saved = crate::engine::SavedEdge { from: e.from.clone(), to: e.to.clone() };
                Some((graph.edge_of(&saved)?, e.mode))
            })
            .collect();
        (graph, modes)
    }

    /// The quick action with the nodes and edges `graph` has (see `graph`): each node a role, with its rule from
    /// `rules` (or, without one, the default), and each edge with its mode from `modes` (or ensured). Windows and the
    /// arrangement leave out the roles no longer there.
    pub fn set_graph(&mut self, graph: &Graph<N>, rules: &BTreeMap<NodeId, Rule>, modes: &[(Edge, EdgeMode)]) {
        self.roles = graph
            .iter()
            .map(|(id, entry)| Role {
                id,
                label: entry.label.clone(),
                pos: entry.pos,
                node: entry.node.clone(),
                disabled: entry.disabled,
                show_in_basic_override: entry.show_in_basic_override,
                rule: rules.get(&id).cloned().unwrap_or_default(),
            })
            .collect();
        self.edges = (graph.edges().iter())
            .filter_map(|edge| {
                let saved = graph.saved_edge(edge)?;
                let mode = modes.iter().find(|(e, _)| e == edge).map_or(EdgeMode::Ensure, |(_, mode)| *mode);
                Some(RoleEdge { from: saved.from, to: saved.to, mode })
            })
            .collect();
        // A window placed relative to a role no longer there goes where the app puts a new one.
        for role in &mut self.roles {
            if let Some(placement) = &mut role.rule.window.open
                && placement.anchor().is_some_and(|of| !graph.contains(of))
            {
                *placement = Placement::Default;
            }
        }
        self.panes.arrangement =
            self.panes.arrangement.as_ref().and_then(|a| a.map_windows(&|id| graph.contains(id).then_some(id)));
    }

    /// The role with id `id`.
    pub fn role(&self, id: NodeId) -> Option<&Role<N>> {
        self.roles.iter().find(|r| r.id == id)
    }

    /// The node in `graph` found for `role`, leaving out those in `taken` (see `Rule`): of the nodes of its type
    /// (whose labels match its `label_pattern`, if it has one), the left-most.
    fn fitting(&self, role: &Role<N>, graph: &Graph<N>, taken: &HashSet<NodeId>) -> Option<NodeId> {
        let kind = role.node.node_type();
        let pattern = match &role.rule.label_pattern {
            Some(pattern) => Some(regex::Regex::new(pattern).ok()?),
            None => None,
        };
        graph
            .iter()
            .filter(|(id, e)| !taken.contains(id) && e.node.node_type() == kind)
            .filter(|(_, e)| pattern.as_ref().is_none_or(|p| p.is_match(&e.label)))
            .min_by(|(a_id, a), (b_id, b)| {
                (a.pos[0].total_cmp(&b.pos[0])).then(a.pos[1].total_cmp(&b.pos[1])).then(a_id.cmp(b_id))
            })
            .map(|(id, _)| id)
    }

    /// The node in `graph` found for each role, by the role's id (roles not found, or ignored, left out); or, if a
    /// required role isn't found, its label.
    pub fn find(&self, graph: &Graph<N>) -> Result<BTreeMap<NodeId, NodeId>, String> {
        let mut found = BTreeMap::new();
        let mut taken = HashSet::new();
        for role in self.roles.iter().filter(|r| r.rule.mode != Mode::Ignore) {
            match self.fitting(role, graph, &taken) {
                Some(id) => {
                    taken.insert(id);
                    found.insert(role.id, id);
                }
                None if role.rule.mode == Mode::Require => return Err(role.label.clone()),
                None => {}
            }
        }
        Ok(found)
    }

    /// The quick action done on `state`: the graph and layout changed, and what changed; or nothing if a required role
    /// isn't found, or nothing would change (when it isn't offered).
    pub fn run(&self, state: &QuickState<N>) -> Option<QuickChange<N>> {
        let mut graph = state.graph.clone();
        let empty = graph.iter().next().is_none();
        let mut summary = Summary::default();

        // Finding each role.
        let mut found = self.find(&graph).ok()?;
        let missing: Vec<NodeId> = (self.roles.iter())
            .filter(|role| role.rule.mode == Mode::Ensure && !found.contains_key(&role.id))
            .map(|role| role.id)
            .collect();

        // Adding those missing, beside what they're connected to.
        let origin = self.origin();
        for id in missing {
            let Some(role) = self.role(id) else { continue };
            let pos = self.place(role, &graph, &found, empty, origin);
            let added = graph.add(role.node.clone(), pos);
            if let Some(entry) = graph.get_mut(added) {
                let label_free = state.graph.iter().all(|(_, e)| e.label != role.label);
                if label_free {
                    entry.label = role.label.clone();
                }
                entry.disabled = role.rule.enabled.map_or(role.disabled, |enabled| !enabled);
                entry.show_in_basic_override = role.show_in_basic_override;
                summary.added.push(entry.label.clone());
            }
            found.insert(id, added);
        }

        // Setting what the rules say on what's found.
        for role in &self.roles {
            let settings = (!role.rule.settings.is_empty()).then(|| role.settings());
            if let Some(entry) = found.get(&role.id).and_then(|id| graph.get_mut(*id)) {
                if let Some(settings) = &settings
                    && let Some(node) = with_settings(&entry.node, settings, &role.rule.settings)
                    && node != entry.node
                {
                    entry.node = node;
                    summary.set.push(entry.label.clone());
                }
                if let Some(enabled) = role.rule.enabled
                    && entry.disabled == enabled
                {
                    entry.disabled = !enabled;
                    let list = if enabled { &mut summary.enabled } else { &mut summary.disabled };
                    list.push(entry.label.clone());
                }
            }
        }

        // The edges.
        for edge in self.edges.iter().filter(|e| e.mode != EdgeMode::Ignore) {
            let (Some(&from), Some(&to)) = (found.get(&edge.from.node), found.get(&edge.to.node)) else { continue };
            {
                let (Some(from), Some(to)) = (address(&graph, from, &edge.from), address(&graph, to, &edge.to)) else {
                    continue;
                };
                let exists = graph.edges().contains(&Edge { from, to });
                match edge.mode {
                    EdgeMode::Remove if exists => {
                        graph.disconnect(Edge { from, to });
                        summary.disconnected += 1;
                    }
                    EdgeMode::Remove | EdgeMode::Ignore => {}
                    _ if exists => {}
                    EdgeMode::Ensure | EdgeMode::Replace => {
                        let single = if to.role == PortRole::DataStreamIn { to } else { from };
                        let free = !graph.edges().iter().any(|e| e.from == single || e.to == single);
                        if (free || edge.mode == EdgeMode::Replace) && graph.connect(from, to).is_ok() {
                            summary.connected += 1;
                        }
                    }
                }
            }
        }

        // The panes.
        let mut layout = state.layout.clone();
        let open_before = layout.open_windows();
        // Whether each window's open is as it was before the quick action, so a window that's both closed if open
        // and opened if not is toggled, not closed and opened again.
        let mut to_open = Vec::new();
        for role in &self.roles {
            let Some(&id) = found.get(&role.id) else { continue };
            match role.rule.window {
                Window { close: true, .. } if open_before.contains(&id) => layout.close(id),
                Window { open: Some(placement), .. } if !open_before.contains(&id) => to_open.push((id, placement)),
                _ => {}
            }
        }
        // Windows placed relative to another's are opened after the rest, so that one's open already.
        to_open.sort_by_key(|(_, placement)| placement.anchor().is_some());
        for (id, placement) in to_open {
            open_window(&mut layout, id, placement, &found);
        }
        let windows_changed = layout.open_windows() != open_before;

        let changed = if self.changes_graph() { graph != *state.graph } else { windows_changed };
        if !changed {
            return None;
        }
        if let Some(arrangement) = &self.panes.arrangement {
            let map = |role: NodeId| found.get(&role).copied();
            if let Some(arrangement) = arrangement.map_windows(&map) {
                layout.arrange(&arrangement);
            }
        }
        Some(QuickChange {
            graph,
            layout,
            arrange: empty && self.arrange_if_empty,
            summary: summary.line(),
            base_directory: self.base_directory.clone(),
        })
    }

    /// Whether the quick action can change the graph: whether it adds nodes, sets settings, enables or disables nodes,
    /// or connects or disconnects edges. One that can is offered only while it would change the graph, as the windows
    /// it opens are for what it adds, and closing one shouldn't bring it back. One that can't (that only opens and
    /// closes windows) is offered while it would change which are open.
    pub fn changes_graph(&self) -> bool {
        let acts = |r: &Role<N>| r.rule.mode == Mode::Ensure || !r.rule.settings.is_empty() || r.rule.enabled.is_some();
        let role = |r: &Role<N>| r.rule.mode != Mode::Ignore && acts(r);
        let ignored = |id: NodeId| self.role(id).is_none_or(|r| r.rule.mode == Mode::Ignore);
        let edge = |e: &RoleEdge| e.mode != EdgeMode::Ignore && !ignored(e.from.node) && !ignored(e.to.node);
        self.edges.iter().any(edge) || self.roles.iter().any(role)
    }

    /// The top-left of the quick action's nodes.
    fn origin(&self) -> [f32; 2] {
        let min = |i: usize| self.roles.iter().map(|r| r.pos[i]).fold(f32::INFINITY, f32::min);
        if self.roles.is_empty() { [0.0, 0.0] } else { [min(0), min(1)] }
    }

    /// Where a role added goes: where it is in the quick action relative to a node it's connected to that's in the
    /// graph (one it takes data from, preferably); otherwise, into an empty graph, where it is, or below the graph,
    /// where it is relative to the quick action's top-left. Moved down, if need be, clear of the nodes there.
    fn place(
        &self,
        role: &Role<N>,
        graph: &Graph<N>,
        found: &BTreeMap<NodeId, NodeId>,
        empty: bool,
        origin: [f32; 2],
    ) -> [f32; 2] {
        let upstream = self.edges.iter().filter(|e| e.to.node == role.id).map(|e| e.from.node);
        let downstream = self.edges.iter().filter(|e| e.from.node == role.id).map(|e| e.to.node);
        let anchor = upstream.chain(downstream).find_map(|other| {
            let at = graph.get(*found.get(&other)?)?.pos;
            let in_action = self.role(other)?.pos;
            Some([at[0] + role.pos[0] - in_action[0], at[1] + role.pos[1] - in_action[1]])
        });
        let positions: Vec<[f32; 2]> = graph.iter().map(|(_, e)| e.pos).collect();
        let mut pos = anchor.unwrap_or_else(|| {
            if empty {
                return role.pos;
            }
            let left = positions.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
            let bottom = positions.iter().map(|p| p[1] + NODE_HEIGHT + 60.0).fold(f32::NEG_INFINITY, f32::max);
            [left + role.pos[0] - origin[0], bottom + role.pos[1] - origin[1]]
        });
        let taken = |pos: [f32; 2]| {
            positions.iter().any(|p| (p[0] - pos[0]).abs() < NODE_WIDTH && (p[1] - pos[1]).abs() < NODE_HEIGHT)
        };
        while taken(pos) {
            pos[1] += 40.0;
        }
        pos
    }
}

/// The window of `id` (a node found for a role), opened as `placement` says, the first time; otherwise where it was.
/// A placement relative to a role whose window isn't open falls back to where the app puts a new window.
fn open_window(layout: &mut Layout, id: NodeId, placement: Placement, found: &BTreeMap<NodeId, NodeId>) {
    if layout.window(id) != WindowState::Closed {
        return layout.open(id, None);
    }
    let pane = Pane::Node(id);
    let group_of = |layout: &Layout, role: NodeId| {
        let other = *found.get(&role)?;
        (other != id && layout.open_windows().contains(&other)).then(|| layout.group_of(Pane::Node(other)))?
    };
    match placement {
        Placement::Largest if let Some(group) = layout.largest_group(&[]) => {
            layout.add_tab(group, pane);
            layout.show(pane);
        }
        Placement::Behind(of) if let Some(group) = group_of(layout, of) => layout.add_tab(group, pane),
        Placement::Beside { of, side, share } if let Some(group) = group_of(layout, of) => {
            layout.split(group, pane, side, share.clamp(0.1, 0.9));
        }
        _ => layout.open(id, None),
    }
}

/// Room for a node on the canvas, about, for placing those added clear of the rest.
const NODE_WIDTH: f32 = 250.0;
const NODE_HEIGHT: f32 = 150.0;

/// The port on `node` that `port` names by id.
fn address<N: NodesEnum>(graph: &Graph<N>, node: NodeId, port: &SavedPort) -> Option<PortAddress> {
    let ids = graph.get(node)?.node.port_ids(port.role);
    let port_idx = ids.iter().position(|id| *id == port.port)?;
    Some(PortAddress { node, role: port.role, port_idx })
}

/// `node` with the settings named in `names` as `settings` has them, or nothing if that can't be read.
fn with_settings<N: NodesEnum>(node: &N, settings: &Value, names: &[String]) -> Option<N> {
    let mut json = serde_json::to_value(node).ok()?;
    let target = json.get_mut("settings")?.as_object_mut()?;
    for name in names {
        if let Some(value) = settings.get(name) {
            target.insert(name.clone(), value.clone());
        }
    }
    serde_json::from_value(json).ok()
}

/// What a quick action did, for the log.
#[derive(Default)]
struct Summary {
    added: Vec<String>,
    set: Vec<String>,
    enabled: Vec<String>,
    disabled: Vec<String>,
    connected: usize,
    disconnected: usize,
}

impl Summary {
    fn line(&self) -> String {
        let mut done = Vec::new();
        let list = |what: &str, names: &[String]| format!("{what} {}", names.join(", "));
        if !self.added.is_empty() {
            done.push(list("added", &self.added));
        }
        if !self.set.is_empty() {
            done.push(list("set the settings of", &self.set));
        }
        if !self.enabled.is_empty() {
            done.push(list("enabled", &self.enabled));
        }
        if !self.disabled.is_empty() {
            done.push(list("disabled", &self.disabled));
        }
        if self.connected > 0 {
            done.push(format!("connected {}", self.connected));
        }
        if self.disconnected > 0 {
            done.push(format!("disconnected {}", self.disconnected));
        }
        if done.is_empty() { "opened windows".into() } else { done.join("; ") }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::Nodes;

    /// A graph of a Spike Detector feeding a Spike Viewer, the viewer's window open.
    fn detector_and_viewer() -> (Graph<Nodes>, Layout, NodeId, NodeId) {
        let mut graph = Graph::<Nodes>::default();
        let detector = graph.add(Nodes::create(crate::NodeType::SpikeDetector), [0.0, 0.0]);
        let viewer = graph.add(Nodes::create(crate::NodeType::SpikeViewer), [300.0, 0.0]);
        let port = |node, role| PortAddress { node, role, port_idx: 0 };
        graph.connect(port(detector, PortRole::DataStreamOut), port(viewer, PortRole::DataStreamIn)).unwrap();
        let mut layout = Layout::default();
        layout.open(viewer, None);
        (graph, layout, detector, viewer)
    }

    #[test]
    fn one_made_from_a_graph_builds_it_again_and_then_is_done() {
        let (graph, layout, _, viewer) = detector_and_viewer();
        let action = QuickAction::from_graph("Spikes", &graph, &layout, &[], true);
        assert_eq!(action.roles.len(), 2);
        assert_eq!(action.edges.len(), 1);
        assert_eq!(action.role(viewer).unwrap().rule.window.open, Some(Placement::Default));
        assert!(action.panes.arrangement.as_ref().is_some_and(|a| a.windows() == vec![viewer]));
        assert!(action.run(&QuickState { graph: &graph, layout: &layout }).is_none(), "nothing to do");

        let empty = Graph::default();
        let change = action.run(&QuickState { graph: &empty, layout: &Layout::default() }).unwrap();
        assert_eq!(change.summary, "added Spike Detector, Spike Viewer; connected 1");
        assert_eq!(change.graph.edges().len(), 1);
        let new_viewer = change.graph.iter().find(|(_, e)| e.label == "Spike Viewer").unwrap().0;
        assert!(change.layout.open_windows().contains(&new_viewer));
        let positions: Vec<[f32; 2]> = change.graph.iter().map(|(_, e)| e.pos).collect();
        assert_eq!(positions, vec![[0.0, 0.0], [300.0, 0.0]], "where they were, into an empty graph");
    }

    #[test]
    fn one_that_only_opens_windows_is_offered_while_they_are_closed() {
        let (graph, _, _, viewer) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Show the spikes", &graph, &Layout::default(), &[viewer], false);
        action.roles[0].rule.mode = Mode::Require;
        action.roles[0].rule.window.open = Some(Placement::Default);
        assert!(!action.changes_graph());
        let closed = Layout::default();
        let change = action.run(&QuickState { graph: &graph, layout: &closed }).expect("offered");
        assert_eq!(change.summary, "opened windows");
        assert!(action.run(&QuickState { graph: &graph, layout: &change.layout }).is_none(), "open already");
        let without = Graph::default();
        assert!(action.run(&QuickState { graph: &without, layout: &closed }).is_none(), "no viewer to show");
    }

    #[test]
    fn as_a_graph_and_back_it_is_the_same() {
        let (graph, layout, detector, viewer) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Spikes", &graph, &layout, &[], true);
        action.roles[0].rule.mode = Mode::Require;
        action.edges[0].mode = EdgeMode::Replace;
        let (as_graph, modes) = action.graph();
        assert!(as_graph.get(detector).is_some_and(|e| e.label == "Spike Detector"), "with the roles' ids");
        assert_eq!(as_graph.edges().len(), 1);
        assert_eq!(modes, vec![(as_graph.edges()[0], EdgeMode::Replace)]);
        let rules: BTreeMap<NodeId, Rule> = action.roles.iter().map(|r| (r.id, r.rule.clone())).collect();
        let mut again = action.clone();
        again.set_graph(&as_graph, &rules, &modes);
        let patches = PatchSets::default();
        assert_eq!(again.to_text(&patches), action.to_text(&patches));
        // Without the viewer, its window and arrangement go too.
        let mut fewer = as_graph.clone();
        fewer.remove(viewer);
        again.set_graph(&fewer, &rules, &modes);
        assert!(again.edges.is_empty() && again.roles.len() == 1);
        assert!(again.panes.arrangement.is_some_and(|a| a.windows().is_empty()), "home's still placed");
    }

    #[test]
    fn edges_are_connected_if_free_replaced_or_removed() {
        let (graph, layout, detector, viewer) = detector_and_viewer();
        let mut other = graph.clone();
        let second = other.add(Nodes::create(crate::NodeType::SpikeDetector), [0.0, 200.0]);
        // A quick action of the second detector feeding the viewer.
        let mut action = QuickAction::from_graph("Second", &other, &layout, &[second, viewer], false);
        let edge = RoleEdge {
            from: SavedPort { node: second, role: PortRole::DataStreamOut, port: "spikes".into() },
            to: SavedPort { node: viewer, role: PortRole::DataStreamIn, port: "spikes".into() },
            mode: EdgeMode::Ensure,
        };
        action.edges = vec![edge.clone()];
        action.roles[0].rule.label_pattern = Some(" 2$".into());
        let state = QuickState { graph: &other, layout: &layout };
        assert!(action.run(&state).is_none(), "the viewer's input isn't free");
        action.edges[0].mode = EdgeMode::Replace;
        let replaced = action.run(&state).unwrap().graph;
        let feeding = |g: &Graph<Nodes>| g.edges().iter().find(|e| e.to.node == viewer).map(|e| e.from.node);
        assert_eq!(feeding(&replaced), Some(second));
        action.edges[0].mode = EdgeMode::Remove;
        let removed = action.run(&QuickState { graph: &replaced, layout: &layout }).unwrap();
        assert_eq!(feeding(&removed.graph), None);
        assert_eq!(removed.summary, "disconnected 1");
        assert_eq!(feeding(&graph), Some(detector));
    }

    #[test]
    fn what_is_ignored_plays_no_part() {
        let (graph, layout, _, _) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Spikes", &graph, &layout, &[], false);
        let empty = QuickState { graph: &Graph::default(), layout: &Layout::default() };
        action.roles[0].rule.mode = Mode::Ignore;
        let change = action.run(&empty).unwrap();
        assert_eq!(change.summary, "added Spike Viewer", "no detector, and nothing to connect it to");
        action.roles[0].rule.mode = Mode::Ensure;
        action.edges[0].mode = EdgeMode::Ignore;
        assert_eq!(action.run(&empty).unwrap().summary, "added Spike Detector, Spike Viewer");
        // With every role ignored, the edge between them is too, even when the edge itself isn't.
        action.roles.iter_mut().for_each(|r| r.rule.mode = Mode::Ignore);
        action.edges[0].mode = EdgeMode::Ensure;
        assert!(!action.changes_graph());
        let mut detached = graph.clone();
        detached.disconnect(detached.edges()[0]);
        assert!(action.run(&QuickState { graph: &detached, layout: &layout }).is_none());
    }

    #[test]
    fn of_several_that_fit_the_left_most_is_found() {
        let mut graph = Graph::<Nodes>::default();
        let right = graph.add(Nodes::create(crate::NodeType::SpikeDetector), [400.0, 0.0]);
        let left = graph.add(Nodes::create(crate::NodeType::SpikeDetector), [0.0, 300.0]);
        let mut action = QuickAction::from_graph("Spikes", &graph, &Layout::default(), &[right], false);
        let found =
            |action: &QuickAction<Nodes>, graph: &Graph<Nodes>| action.find(graph).unwrap().get(&right).copied();
        assert_eq!(found(&action, &graph), Some(left));
        graph.get_mut(right).unwrap().label = "Spikes (tetrode 2)".into();
        action.roles[0].rule.label_pattern = Some("tetrode \\d".into());
        assert_eq!(found(&action, &graph), Some(right), "only the one whose label matches");
        action.roles[0].rule.label_pattern = Some("(".into());
        assert_eq!(found(&action, &graph), None, "a pattern that can't be read matches nothing");
    }
}
