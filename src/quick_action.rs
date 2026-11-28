//! The basic view offers quick actions to people who'd rather not build the graph node by node: ready-made changes to
//! it ("the standard LTX graph", say). A quick action is data, not code, so people can make, change and share their
//! own: a build ships some (`Components::QUICK_ACTIONS`), and people's own are files in a folder beside the app's
//! settings (see `crate::gui::quick_actions`).
//!
//! A quick action is a small graph, its nodes each a *role* with a rule (`Rule`) for whether it's used, how it's found
//! in the graph it's done on, and what's done in each case. The rule mirrors the quick action editor: each choice is
//! saved under the question it answers (`if_found`, `if_not_found`, `if_window_open`, `if_window_closed`), and only
//! what isn't the default is written.
//!
//! - A role's found by its node's type, and a regular expression its label matches (`Match`): one node, each node one
//!   role's at most, paired first where there's only one candidate, then left to right (see `QuickAction::find`).
//! - A role found can be enabled or disabled, and have some of its settings set as the quick action has them
//!   (`IfFound`). One not found (`IfNotFound`) is left out, created as it is in the quick action, or stops the quick
//!   action being offered. One not used (`Rule::used`) plays no part, and nor do its edges: it shows what surrounds the
//!   rest, in a quick action made from a whole graph.
//! - A role's window can be closed if it's open (`IfWindowOpen`), and opened if it isn't (`IfWindowClosed`: a window
//!   opened for the first time goes as its `Placement` says).
//!
//! Each of the quick action's edges, between the nodes found for its ends, is enabled or disabled if they're connected
//! already, connected if its input's free, and swapped in if its input's occupied, or not, in each case (`EdgeRule`).
//! Nothing's ever disconnected, and nodes are never removed: a quick action disables them, which keeps their settings
//! for when they're wanted again.
//!
//! A quick action can also apply a base directory mode (`ApplyBaseDirectory`).
//!
//! The basic view offers a quick action if its required roles are found and doing it would change the graph (or, for
//! one that only opens and closes windows, which are open: see `QuickAction::changes_graph`). So one that's done is no
//! longer offered until something changes.
//!
//! A quick action's file is shaped as a project (`{"graph": {"nodes": ..., "edges": ...}}`), with each node's rule
//! beside it (`"quick"`) and each edge's rule on it, so the app's config patches (see `crate::config_patching`) bring
//! an old one up to date as they would a project. It says the versions that wrote it, as copied settings do.

use std::collections::{BTreeMap, HashSet};

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config_patching::{PatchSets, Versions};
use super::engine::{Edge, Graph, NodeId, NodesEnum, PortAddress, PortRole, SavedPort};
pub use super::layout::{Layout, Spot};
use super::layout::{Pane, WindowState};

/// A role's label pattern, by default: any label.
pub const ANY_LABEL: &str = ".*";

/// A quick action's file says it's one, and the versions that wrote it, under this key.
pub const MARKER: &str = "ephys-rs quick action";

/// A choice that, by default, does nothing.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn is_true(value: &bool) -> bool {
    *value
}

fn yes() -> bool {
    true
}

/// A node, or an edge, found is enabled or disabled, or left as it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnableOrDisable {
    #[default]
    DoNothing,
    Enable,
    Disable,
}

impl EnableOrDisable {
    /// Whether it's to be enabled (or disabled), if it's to be either.
    pub fn wanted(self) -> Option<bool> {
        match self {
            EnableOrDisable::DoNothing => None,
            EnableOrDisable::Enable => Some(true),
            EnableOrDisable::Disable => Some(false),
        }
    }
}

/// A node found has some of its settings (by name) set as the quick action has them, or none.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settings {
    #[default]
    DoNothing,
    Apply(Vec<String>),
}

impl Settings {
    /// The settings applied, by name: none, doing nothing.
    pub fn applied(&self) -> &[String] {
        match self {
            Settings::DoNothing => &[],
            Settings::Apply(names) => names,
        }
    }
}

/// What's done to the node found for a role.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IfFound {
    #[serde(skip_serializing_if = "is_default")]
    pub enable_or_disable: EnableOrDisable,
    #[serde(skip_serializing_if = "is_default")]
    pub settings: Settings,
}

/// What's done when a role isn't found: nothing (the quick action is still offered, leaving the role out), the node
/// created (as the quick action has it, enabled), or the quick action not offered.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfNotFound {
    DoNothing,
    #[default]
    Create,
    DontOffer,
}

impl IfNotFound {
    pub const ALL: [IfNotFound; 3] = [IfNotFound::DoNothing, IfNotFound::Create, IfNotFound::DontOffer];

    /// The choice, as the editor offers it.
    pub fn choice(self) -> &'static str {
        match self {
            IfNotFound::DoNothing => "Do nothing",
            IfNotFound::Create => "Create the node and apply all settings",
            IfNotFound::DontOffer => "Don't offer the quick action",
        }
    }
}

/// How a role's found: of the nodes of its type, those whose labels match this regular expression (`^Band-pass`,
/// say), or, without one, any (`ANY_LABEL`). One that's empty, or can't be read, matches nothing.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Match {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// What's done with the window of the node found (or created), if it's open.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfWindowOpen {
    #[default]
    DoNothing,
    Close,
}

/// What's done with the window of the node found (or created), if it isn't open: opened, placed (the first time it's
/// opened) as `Placement` says, or left closed.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfWindowClosed {
    #[default]
    DoNothing,
    Open(Placement),
}

/// A role's rule: whether it's used, how it's found, and what's done in each case (see the module docs). The file says
/// only what isn't the default.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Not used, the role plays no part: never looked for, and its edges and windows left alone. A quick action made
    /// from a whole graph keeps the parts that don't matter to it like this, to show where the parts that do are.
    #[serde(rename = "use", default = "yes", skip_serializing_if = "is_true")]
    pub used: bool,
    #[serde(rename = "match", skip_serializing_if = "is_default")]
    pub matching: Match,
    #[serde(skip_serializing_if = "is_default")]
    pub if_found: IfFound,
    #[serde(skip_serializing_if = "is_default")]
    pub if_not_found: IfNotFound,
    #[serde(skip_serializing_if = "is_default")]
    pub if_window_open: IfWindowOpen,
    #[serde(skip_serializing_if = "is_default")]
    pub if_window_closed: IfWindowClosed,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            used: true,
            matching: Match::default(),
            if_found: IfFound::default(),
            if_not_found: IfNotFound::default(),
            if_window_open: IfWindowOpen::default(),
            if_window_closed: IfWindowClosed::default(),
        }
    }
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

/// An edge that takes the end that takes just the one edge (a data stream's input, or the reading end of a value),
/// free, is connected, or not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfFree {
    DoNothing,
    #[default]
    Connect,
}

/// An edge whose end that takes just the one edge is occupied (connected to something else) is swapped in for that,
/// or not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfOccupied {
    #[default]
    DoNothing,
    Swap,
}

/// What a quick action does to an edge between the nodes found for its ends, in each case: if they're connected
/// already, enabled or disabled; else, if the end that takes just the one edge is free, connected; else (that end
/// occupied), swapped in. A quick action never disconnects an edge, and an edge it connects is enabled.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeRule {
    #[serde(skip_serializing_if = "is_default")]
    pub if_connected: EnableOrDisable,
    #[serde(skip_serializing_if = "is_default")]
    pub if_free: IfFree,
    #[serde(skip_serializing_if = "is_default")]
    pub if_occupied: IfOccupied,
}

impl EdgeRule {
    /// The rule for an edge that's only there to show what surrounds the rest: nothing's done to it.
    pub const NOTHING: EdgeRule = EdgeRule {
        if_connected: EnableOrDisable::DoNothing,
        if_free: IfFree::DoNothing,
        if_occupied: IfOccupied::DoNothing,
    };

    pub fn does_nothing(self) -> bool {
        self == Self::NOTHING
    }
}

/// One of the quick action's edges, between roles, by port id, with what's done to it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RoleEdge {
    pub from: SavedPort,
    pub to: SavedPort,
    #[serde(flatten)]
    pub rule: EdgeRule,
}

/// A tab goes at the front of its group, or the back.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Front,
    Back,
}

/// A group of tabs chosen by its shape, as laid out: the largest, the widest or the tallest.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Largest,
    Widest,
    Tallest,
}

/// A window placed in a group chosen by its shape, as a tab at the front or the back.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ByShape {
    pub shape: Shape,
    pub at: Position,
}

/// A window placed exactly: where a window was when the quick action was made, as its group's rectangle (fractions of
/// the docked panes' area: see `Layout::spot`), as a tab at the front or the back.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Exact {
    pub rect: [f32; 4],
    pub at: Position,
}

impl Exact {
    pub fn of(spot: Spot) -> Self {
        Exact { rect: spot.rect, at: if spot.front { Position::Front } else { Position::Back } }
    }

    pub fn spot(self) -> Spot {
        Spot { rect: self.rect, front: self.at == Position::Front }
    }
}

/// A window that's been open before opens where it was. Opened for the first time, it goes here: exactly where a
/// window was (see `Layout::place_exactly`); where the app puts a new window (see `Layout::place`); or as a tab in a
/// group chosen by its shape (`Layout::largest_group`, say). Windows are placed in that order, in passes (see `run`).
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    #[default]
    Default,
    Exact(Exact),
    ByGroupShape(ByShape),
}

/// A quick action (see the module docs).
#[derive(Clone, PartialEq, Debug)]
pub struct QuickAction<N> {
    pub name: String,
    pub description: String,
    /// The roles, in the order they're found and added.
    pub roles: Vec<Role<N>>,
    pub edges: Vec<RoleEdge>,
    /// The base directory mode applied, if any (see `crate::base_directory`).
    pub base_directory: Option<ApplyBaseDirectory>,
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
    /// The log says what the action did, in a line: "added Spike Detector, Spike Viewer; connected 3", say.
    pub summary: String,
    /// The base directory mode to apply, if any (see `crate::base_directory`).
    pub base_directory: Option<ApplyBaseDirectory>,
}

/// A quick action can apply a base directory mode (see `crate::base_directory`): a base directory type, by its name,
/// used with what it has (its directory, say), which the quick action leaves alone.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ApplyBaseDirectory {
    pub mode: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_directory: Option<ApplyBaseDirectory>,
}

/// A quick action's graph, as a project's is saved, with each node's rule and each edge's.
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
                show_in_basic_override: entry.show_in_basic_override,
                rule: entry.quick,
            });
        }
        Ok(QuickAction {
            name: file.name,
            description: file.description,
            roles,
            edges: file.graph.edges,
            base_directory: file.base_directory,
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
            base_directory: self.base_directory.clone(),
        };
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// A quick action made from some of a graph's nodes (`ids`, or all of them, if none): each a role, created if not
    /// found, with nothing done to what's found; the edges between them, connected if free; and the windows of theirs
    /// that are open, opened: those docked exactly where they are (see `Layout::spot`), and those floating where the
    /// app puts a new window.
    pub fn from_graph(name: &str, graph: &Graph<N>, layout: &Layout, ids: &[NodeId]) -> Self {
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
                    show_in_basic_override: entry.show_in_basic_override,
                    rule: Rule {
                        if_window_closed: match layout.spot(Pane::Node(*id)) {
                            Some(spot) => IfWindowClosed::Open(Placement::Exact(Exact::of(spot))),
                            None if open_windows.contains(id) => IfWindowClosed::Open(Placement::Default),
                            None => IfWindowClosed::DoNothing,
                        },
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
            .map(|e| RoleEdge { from: e.from, to: e.to, rule: EdgeRule::default() })
            .collect();
        QuickAction { name: name.to_string(), description: String::new(), roles, edges, base_directory: None }
    }

    /// The quick action's roles as a graph, each node with the id of its role (so the quick action's edges and windows
    /// name them), and the mode of each of the graph's edges. The quick action editor shows and
    /// changes it on a canvas (see `crate::gui::quick_actions`), and puts it back with `set_graph`.
    pub fn graph(&self) -> (Graph<N>, Vec<(Edge, EdgeRule)>) {
        let nodes: serde_json::Map<String, Value> = self
            .roles
            .iter()
            .map(|role| {
                let entry = serde_json::json!({
                    "label": role.label,
                    "pos": role.pos,
                    "node": serde_json::to_value(&role.node).unwrap_or_default(),
                    "show_in_basic_override": role.show_in_basic_override,
                    // Drawn faded only if ignored (see `Rule::used`): a node created is always enabled.
                    "disabled": false,
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
                Some((graph.edge_of(&saved)?, e.rule))
            })
            .collect();
        (graph, modes)
    }

    /// The quick action with the nodes and edges `graph` has (see `graph`): each node a role, with its rule from
    /// `rules` (or, without one, the default), and each edge with its rule from `rules_of_edges` (or the default).
    pub fn set_graph(&mut self, graph: &Graph<N>, rules: &BTreeMap<NodeId, Rule>, rules_of_edges: &[(Edge, EdgeRule)]) {
        self.roles = graph
            .iter()
            .map(|(id, entry)| Role {
                id,
                label: entry.label.clone(),
                pos: entry.pos,
                node: entry.node.clone(),
                show_in_basic_override: entry.show_in_basic_override,
                rule: rules.get(&id).cloned().unwrap_or_default(),
            })
            .collect();
        self.edges = (graph.edges().iter())
            .filter_map(|edge| {
                let saved = graph.saved_edge(edge)?;
                let rule =
                    (rules_of_edges.iter()).find(|(e, _)| e == edge).map_or(EdgeRule::default(), |(_, rule)| *rule);
                Some(RoleEdge { from: saved.from, to: saved.to, rule })
            })
            .collect();
    }

    /// The role with id `id`.
    pub fn role(&self, id: NodeId) -> Option<&Role<N>> {
        self.roles.iter().find(|r| r.id == id)
    }

    /// The nodes in `graph` that `role` could be found as (see `Rule`): those of its type whose labels match its
    /// label pattern, left-most first.
    fn candidates(&self, role: &Role<N>, graph: &Graph<N>) -> Vec<NodeId> {
        let kind = role.node.node_type();
        let pattern = match role.rule.matching.label.as_deref() {
            Some("") => return Vec::new(),
            Some(pattern) => match regex::Regex::new(pattern) {
                Ok(pattern) => Some(pattern),
                Err(_) => return Vec::new(),
            },
            None => None,
        };
        let mut fits: Vec<(NodeId, [f32; 2])> = graph
            .iter()
            .filter(|(_, e)| e.node.node_type() == kind)
            .filter(|(_, e)| pattern.as_ref().is_none_or(|p| p.is_match(&e.label)))
            .map(|(id, e)| (id, e.pos))
            .collect();
        fits.sort_by(|(a_id, a), (b_id, b)| left_most(*a, *b).then(a_id.cmp(b_id)));
        fits.into_iter().map(|(id, _)| id).collect()
    }

    /// The node in `graph` found for each role, by the role's id (roles not found, or ignored, left out); or, if a
    /// required role isn't found, its label.
    ///
    /// Each role is paired with one of its candidates (see `candidates`), each node with one role at most. A role with
    /// just one candidate left is paired first, and that node struck off the others' candidates; when none has just
    /// one, the left-most role (as the quick action has them) is paired with its left-most candidate; and so on,
    /// until no role has a candidate left. So a node only one role could be is that role's, and the rest pair up left
    /// to right, as they'd be matched by eye.
    pub fn find(&self, graph: &Graph<N>) -> Result<BTreeMap<NodeId, NodeId>, String> {
        let mut roles: Vec<&Role<N>> = self.roles.iter().filter(|r| r.rule.used).collect();
        roles.sort_by(|a, b| left_most(a.pos, b.pos).then(a.id.cmp(&b.id)));
        let mut open: Vec<(NodeId, Vec<NodeId>)> =
            roles.iter().map(|role| (role.id, self.candidates(role, graph))).collect();
        let mut found = BTreeMap::new();
        loop {
            open.retain(|(_, candidates)| !candidates.is_empty());
            if open.is_empty() {
                break;
            }
            let next = open.iter().position(|(_, candidates)| candidates.len() == 1).unwrap_or(0);
            let (role, candidates) = open.remove(next);
            let node = candidates[0];
            found.insert(role, node);
            for (_, candidates) in &mut open {
                candidates.retain(|n| *n != node);
            }
        }
        let required =
            |r: &&Role<N>| r.rule.used && r.rule.if_not_found == IfNotFound::DontOffer && !found.contains_key(&r.id);
        match self.roles.iter().find(required) {
            Some(role) => Err(role.label.clone()),
            None => Ok(found),
        }
    }

    /// The quick action done on `state`: the graph and layout changed, and what changed; or nothing if a required role
    /// isn't found, or nothing would change (when it isn't offered).
    pub fn run(&self, state: &QuickState<N>) -> Option<QuickChange<N>> {
        let mut graph = state.graph.clone();
        let mut summary = Summary::default();

        // Finding each role.
        let mut found = self.find(&graph).ok()?;
        let mut missing: Vec<NodeId> = (self.roles.iter())
            .filter(|role| role.rule.used && role.rule.if_not_found == IfNotFound::Create)
            .filter(|role| !found.contains_key(&role.id))
            .map(|role| role.id)
            .collect();

        // Adding those missing, each placed by one in the graph already (see `QuickAction::place`), enabled. Next is
        // one connected to a node in the graph (from it, preferably; then to it), so as many as can be are placed by
        // what they're connected to; otherwise the left-most.
        let empty = graph.iter().next().is_none();
        let origin = self.origin();
        let mut added_ids = HashSet::new();
        while !missing.is_empty() {
            let placed = |others: Vec<NodeId>| others.iter().any(|other| found.contains_key(other));
            let next = (missing.iter().position(|id| placed(self.upstream(*id))))
                .or_else(|| missing.iter().position(|id| placed(self.downstream(*id))))
                .unwrap_or(0);
            let id = missing.remove(next);
            let Some(role) = self.role(id) else { continue };
            let (pos, anchor) = self.place(role, &graph, &found, empty, origin);
            let added = graph.add(role.node.clone(), pos);
            clear(&mut graph, added, anchor);
            if let Some(entry) = graph.get_mut(added) {
                let label_free = state.graph.iter().all(|(_, e)| e.label != role.label);
                if label_free {
                    entry.label = role.label.clone();
                }
                entry.show_in_basic_override = role.show_in_basic_override;
                summary.added.push(entry.label.clone());
            }
            found.insert(id, added);
            added_ids.insert(added);
        }

        // Setting what the rules say on what's found.
        for role in &self.roles {
            let applied = role.rule.if_found.settings.applied();
            let settings = (!applied.is_empty()).then(|| role.settings());
            let Some(&id) = found.get(&role.id) else { continue };
            if let Some(entry) = graph.get_mut(id) {
                if let Some(settings) = &settings
                    && let Some(node) = with_settings(&entry.node, settings, applied)
                    && node != entry.node
                {
                    entry.node = node;
                    summary.set.push(entry.label.clone());
                }
                if let Some(enabled) = role.rule.if_found.enable_or_disable.wanted()
                    && !added_ids.contains(&id)
                    && entry.disabled == enabled
                {
                    entry.disabled = !enabled;
                    let list = if enabled { &mut summary.enabled } else { &mut summary.disabled };
                    list.push(entry.label.clone());
                }
            }
        }

        // The edges.
        for edge in self.edges.iter().filter(|e| !e.rule.does_nothing()) {
            let (Some(&from), Some(&to)) = (found.get(&edge.from.node), found.get(&edge.to.node)) else { continue };
            let (Some(from), Some(to)) = (address(&graph, from, &edge.from), address(&graph, to, &edge.to)) else {
                continue;
            };
            let existing = Edge { from, to };
            if graph.edges().contains(&existing) {
                if let Some(enabled) = edge.rule.if_connected.wanted()
                    && graph.is_edge_disabled(&existing) == enabled
                {
                    graph.set_edge_disabled(existing, !enabled);
                    let count = if enabled { &mut summary.edges_enabled } else { &mut summary.edges_disabled };
                    *count += 1;
                }
                continue;
            }
            let single = if to.role == PortRole::DataStreamIn { to } else { from };
            let free = !graph.edges().iter().any(|e| e.from == single || e.to == single);
            let wanted =
                if free { edge.rule.if_free == IfFree::Connect } else { edge.rule.if_occupied == IfOccupied::Swap };
            if wanted && graph.connect(from, to).is_ok() {
                summary.connected += 1;
            }
        }

        // The panes.
        let mut layout = state.layout.clone();
        let open_before = layout.open_windows();
        // Whether each window's open is as it was before the quick action, so a window that's both closed if open
        // and opened if not is toggled, not closed and opened again.
        //
        // Windows are opened in three passes: those placed where they were (cutting the layout to fit), then those in
        // the default location (with those that couldn't be placed where they were), then those by group shape; each
        // pass in the order the roles are in the quick action, left to right (and of two as far left, top to bottom).
        let mut roles: Vec<&Role<N>> = self.roles.iter().collect();
        roles.sort_by(|a, b| left_most(a.pos, b.pos));
        let mut to_open = Vec::new();
        let mut exactly = Vec::new();
        for role in roles {
            let Some(&id) = found.get(&role.id) else { continue };
            let open = open_before.contains(&id);
            match (role.rule.if_window_open, role.rule.if_window_closed) {
                (IfWindowOpen::Close, _) if open => layout.close(id),
                (_, IfWindowClosed::Open(Placement::Exact(exact)))
                    if !open && layout.window(id) == WindowState::Closed =>
                {
                    exactly.push((Pane::Node(id), exact.spot()))
                }
                (_, IfWindowClosed::Open(placement)) if !open => to_open.push((id, placement)),
                _ => {}
            }
        }
        let order: Vec<NodeId> = exactly
            .iter()
            .filter_map(|(pane, _)| if let Pane::Node(id) = pane { Some(*id) } else { None })
            .chain(to_open.iter().map(|(id, _)| *id))
            .collect();
        for pane in layout.place_exactly(&exactly) {
            if let Pane::Node(id) = pane {
                to_open.push((id, Placement::Default));
            }
        }
        let pass = |placement: &Placement| matches!(placement, Placement::ByGroupShape(_));
        let place = |id: &NodeId| order.iter().position(|o| o == id);
        to_open.sort_by_key(|(id, placement)| (pass(placement), place(id)));
        for (id, placement) in to_open {
            open_window(&mut layout, id, placement);
        }
        layout.close_empty();
        let windows_changed = layout.open_windows() != open_before;

        let changed = if self.changes_graph() { graph != *state.graph } else { windows_changed };
        if !changed {
            return None;
        }
        Some(QuickChange { graph, layout, summary: summary.line(), base_directory: self.base_directory.clone() })
    }

    /// Whether the quick action can change the graph: whether it adds nodes, sets settings, enables or disables nodes,
    /// or connects or disconnects edges. One that can is offered only while it would change the graph, as the windows
    /// it opens are for what it adds, and closing one shouldn't bring it back. One that can't (that only opens and
    /// closes windows) is offered while it would change which are open.
    pub fn changes_graph(&self) -> bool {
        let acts = |r: &Role<N>| {
            let found = &r.rule.if_found;
            let changes = found.enable_or_disable != EnableOrDisable::DoNothing || !found.settings.applied().is_empty();
            r.rule.if_not_found == IfNotFound::Create || changes
        };
        let role = |r: &Role<N>| r.rule.used && acts(r);
        let ignored = |id: NodeId| self.role(id).is_none_or(|r| !r.rule.used);
        let edge = |e: &RoleEdge| !e.rule.does_nothing() && !ignored(e.from.node) && !ignored(e.to.node);
        self.edges.iter().any(edge) || self.roles.iter().any(role)
    }

    /// The top-left of the quick action's nodes.
    fn origin(&self) -> [f32; 2] {
        let min = |i: usize| self.roles.iter().map(|r| r.pos[i]).fold(f32::INFINITY, f32::min);
        if self.roles.is_empty() { [0.0, 0.0] } else { [min(0), min(1)] }
    }

    /// The roles `id` takes data from, by the quick action's edges.
    fn upstream(&self, id: NodeId) -> Vec<NodeId> {
        self.edges.iter().filter(|e| e.to.node == id).map(|e| e.from.node).collect()
    }

    /// The roles `id` gives data to, by the quick action's edges.
    fn downstream(&self, id: NodeId) -> Vec<NodeId> {
        self.edges.iter().filter(|e| e.from.node == id).map(|e| e.to.node).collect()
    }

    /// Where a role added goes: where it is in the quick action relative to another role, in the graph already (found
    /// or added), at the same distance from it in the graph. That's one it takes data from, if any is in the graph;
    /// otherwise one it gives data to; otherwise the nearest in the quick action. Of several it's connected to, it's
    /// the nearest across in the quick action, then the nearest up or down. With none of the roles in the graph, it's where it is in the quick action,
    /// into an empty graph, or otherwise below the graph, where it is relative to the quick action's top-left.
    ///
    /// This also gives the top of the node it's placed relative to, if any, which says which way it's moved if where
    /// it goes is taken (see `clear`).
    fn place(
        &self,
        role: &Role<N>,
        graph: &Graph<N>,
        found: &BTreeMap<NodeId, NodeId>,
        empty: bool,
        origin: [f32; 2],
    ) -> ([f32; 2], Option<f32>) {
        let in_graph = |others: Vec<NodeId>| -> Vec<(NodeId, [f32; 2])> {
            (others.into_iter()).filter_map(|other| Some((other, graph.get(*found.get(&other)?)?.pos))).collect()
        };
        // Of several, the nearest across in the quick action, then the nearest up or down.
        let apart = |other: NodeId| {
            let at = self.role(other).map_or([f32::INFINITY; 2], |r| r.pos);
            ((at[0] - role.pos[0]).abs(), (at[1] - role.pos[1]).abs())
        };
        let first = |others: Vec<(NodeId, [f32; 2])>| {
            others.into_iter().min_by(|(a, _), (b, _)| {
                let (a, b) = (apart(*a), apart(*b));
                a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
            })
        };
        let distance = |other: &Role<N>| (other.pos[0] - role.pos[0]).hypot(other.pos[1] - role.pos[1]);
        let nearest = || {
            let placed = self.roles.iter().filter(|r| r.id != role.id && found.contains_key(&r.id));
            let other = placed.min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
            in_graph(vec![other.id]).pop()
        };
        let anchor = (first(in_graph(self.upstream(role.id))))
            .or_else(|| first(in_graph(self.downstream(role.id))))
            .or_else(nearest)
            .and_then(|(other, at)| Some((self.role(other)?.pos, at)));
        match anchor {
            Some((in_action, at)) => {
                let offset = [role.pos[0] - in_action[0], role.pos[1] - in_action[1]];
                ([at[0] + offset[0], at[1] + offset[1]], Some(at[1]))
            }
            None if empty => (role.pos, None),
            None => {
                let rects = rects(graph);
                let left = rects.iter().map(|(_, r)| r.left()).fold(f32::INFINITY, f32::min);
                let bottom = rects.iter().map(|(_, r)| r.bottom() + 60.0).fold(f32::NEG_INFINITY, f32::max);
                ([left + role.pos[0] - origin[0], bottom + role.pos[1] - origin[1]], None)
            }
        }
    }
}

/// The window of `id` (a node found for a role), opened as `placement` says, the first time; otherwise where it was.
fn open_window(layout: &mut Layout, id: NodeId, placement: Placement) {
    if layout.window(id) != WindowState::Closed {
        return layout.open(id, None);
    }
    let pane = Pane::Node(id);
    match placement {
        Placement::ByGroupShape(by) if let Some(group) = group_by_shape(layout, by.shape) => {
            layout.add_tab(group, pane);
            if by.at == Position::Front {
                layout.show(pane);
            }
        }
        _ => layout.open(id, None),
    }
}

/// The group of tabs with `shape` (the largest, the widest or the tallest), if any are showing.
fn group_by_shape(layout: &Layout, shape: Shape) -> Option<egui_tiles::TileId> {
    match shape {
        Shape::Largest => layout.largest_group(&[]),
        Shape::Widest => layout.widest_group(&[]),
        Shape::Tallest => layout.tallest_group(&[]),
    }
}

/// Which of two places on the canvas is further left (or, as far left, higher).
fn left_most(a: [f32; 2], b: [f32; 2]) -> std::cmp::Ordering {
    a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]))
}

/// The room each node of `graph` takes on the canvas (see `canvas::node_size`).
fn rects<N: NodesEnum>(graph: &Graph<N>) -> Vec<(NodeId, egui::Rect)> {
    let resolved = graph.resolve();
    (graph.iter())
        .filter_map(|(id, entry)| {
            let size = crate::gui::canvas::node_size(resolved.get(&id)?);
            Some((id, egui::Rect::from_min_size(egui::Pos2::from(entry.pos), size)))
        })
        .collect()
}

/// A node added where another is, overlapping it, is moved up or down to a gap among those it's level with across,
/// with this much room around each of them.
const MARGIN: f32 = 40.0;

/// A node added is moved clear of the rest with this, if it overlaps any, keeping where it is across: to the gap
/// nearest where it is, going back towards the node it's placed relative to (whose top is `anchor`) without passing
/// it, or failing that, further away (see `slot`).
fn clear<N: NodesEnum>(graph: &mut Graph<N>, id: NodeId, anchor: Option<f32>) {
    let rects = rects(graph);
    let Some(mine) = rects.iter().find(|(other, _)| *other == id).map(|(_, r)| *r) else { return };
    let others: Vec<egui::Rect> = rects.iter().filter(|(other, _)| *other != id).map(|(_, r)| *r).collect();
    // Touching isn't overlapping.
    if !others.iter().any(|other| other.intersects(mine.shrink(0.5))) {
        return;
    }
    // The tops it can't have: within the margin of a node it's level with across (within the margin too).
    let blocked: Vec<(f32, f32)> = (others.iter())
        .filter(|other| other.right() + MARGIN > mine.left() && other.left() - MARGIN < mine.right())
        .map(|other| (other.top() - MARGIN - mine.height(), other.bottom() + MARGIN))
        .collect();
    let top = slot(mine.top(), anchor, blocked);
    if let Some(entry) = graph.get_mut(id) {
        entry.pos = [mine.left(), top];
    }
}

/// The top a node added goes at: `want`, if that's free of `blocked` (each the tops between two it can't have), or
/// otherwise the nearest free from `want` back towards `anchor` (the top of the node it's placed relative to) on the
/// same side of it (below it, if level), or failing that the nearest away from it. Without an anchor, it goes down.
fn slot(want: f32, anchor: Option<f32>, mut blocked: Vec<(f32, f32)>) -> f32 {
    blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (from, to) in blocked {
        match merged.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    let Some(&(above, below)) = merged.iter().find(|(from, to)| want > *from && want < *to) else { return want };
    let under = anchor.is_none_or(|anchor| want >= anchor);
    match anchor {
        Some(anchor) if under && above >= anchor => above,
        Some(anchor) if !under && below <= anchor => below,
        _ if under => below,
        _ => above,
    }
}

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
    edges_enabled: usize,
    edges_disabled: usize,
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
        if self.edges_enabled > 0 {
            done.push(format!("enabled {} connections", self.edges_enabled));
        }
        if self.edges_disabled > 0 {
            done.push(format!("disabled {} connections", self.edges_disabled));
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
        let action = QuickAction::from_graph("Spikes", &graph, &layout, &[]);
        assert_eq!(action.roles.len(), 2);
        assert_eq!(action.edges.len(), 1);
        let spot = layout.spot(Pane::Node(viewer)).unwrap();
        assert_eq!(
            action.role(viewer).unwrap().rule.if_window_closed,
            IfWindowClosed::Open(Placement::Exact(Exact::of(spot))),
            "where it is"
        );
        assert!(action.run(&QuickState { graph: &graph, layout: &layout }).is_none(), "nothing to do");

        let empty = Graph::default();
        let change = action.run(&QuickState { graph: &empty, layout: &Layout::default() }).unwrap();
        assert_eq!(change.summary, "added Spike Detector, Spike Viewer; connected 1");
        assert_eq!(change.graph.edges().len(), 1);
        let new_viewer = change.graph.iter().find(|(_, e)| e.label == "Spike Viewer").unwrap().0;
        assert!(change.layout.open_windows().contains(&new_viewer));
        assert_eq!(change.layout.spot(Pane::Node(new_viewer)), Some(spot), "where it was");
        let positions: Vec<[f32; 2]> = change.graph.iter().map(|(_, e)| e.pos).collect();
        assert_eq!(positions, vec![[0.0, 0.0], [300.0, 0.0]], "where they were, into an empty graph");
    }

    #[test]
    fn windows_open_left_to_right_whatever_order_the_roles_are_in() {
        let mut graph = Graph::<Nodes>::default();
        let right = graph.add(Nodes::create(crate::NodeType::SpikeViewer), [500.0, 0.0]);
        let left = graph.add(Nodes::create(crate::NodeType::SpikeViewer), [0.0, 0.0]);
        let mut action = QuickAction::from_graph("Viewers", &graph, &Layout::default(), &[]);
        for role in &mut action.roles {
            role.rule.if_window_closed = IfWindowClosed::Open(Placement::Default);
        }
        let change = action.run(&QuickState { graph: &Graph::default(), layout: &Layout::default() }).unwrap();
        let label = |role| &action.role(role).unwrap().label;
        let window = |role| {
            let id = change.graph.iter().find(|(_, e)| e.label == *label(role)).unwrap().0;
            change.layout.spot(Pane::Node(id)).unwrap().rect
        };
        // The left one first: a column beside home; then the right one, below it.
        assert!(window(left)[1] < 0.01 && window(right)[1] > 0.5, "{:?} {:?}", window(left), window(right));
    }

    #[test]
    fn one_that_only_opens_windows_is_offered_while_they_are_closed() {
        let (graph, _, _, viewer) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Show the spikes", &graph, &Layout::default(), &[viewer]);
        action.roles[0].rule.if_not_found = IfNotFound::DontOffer;
        action.roles[0].rule.if_window_closed = IfWindowClosed::Open(Placement::Default);
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
        let mut action = QuickAction::from_graph("Spikes", &graph, &layout, &[]);
        action.roles[0].rule.if_not_found = IfNotFound::DontOffer;
        action.edges[0].rule = EdgeRule { if_occupied: IfOccupied::Swap, ..EdgeRule::default() };
        let (as_graph, modes) = action.graph();
        assert!(as_graph.get(detector).is_some_and(|e| e.label == "Spike Detector"), "with the roles' ids");
        assert_eq!(as_graph.edges().len(), 1);
        assert_eq!(
            modes,
            vec![(as_graph.edges()[0], EdgeRule { if_occupied: IfOccupied::Swap, ..EdgeRule::default() })]
        );
        let rules: BTreeMap<NodeId, Rule> = action.roles.iter().map(|r| (r.id, r.rule.clone())).collect();
        let mut again = action.clone();
        again.set_graph(&as_graph, &rules, &modes);
        let patches = PatchSets::default();
        assert_eq!(again.to_text(&patches), action.to_text(&patches));
        // Without the viewer, its window goes too.
        let mut fewer = as_graph.clone();
        fewer.remove(viewer);
        again.set_graph(&fewer, &rules, &modes);
        assert!(again.edges.is_empty() && again.roles.len() == 1);
    }

    #[test]
    fn edges_are_connected_if_free_or_replacing_what_is_there() {
        let (graph, layout, detector, viewer) = detector_and_viewer();
        let mut other = graph.clone();
        let second = other.add(Nodes::create(crate::NodeType::SpikeDetector), [0.0, 200.0]);
        // A quick action of the second detector feeding the viewer.
        let mut action = QuickAction::from_graph("Second", &other, &layout, &[second, viewer]);
        let edge = RoleEdge {
            from: SavedPort { node: second, role: PortRole::DataStreamOut, port: "spikes".into() },
            to: SavedPort { node: viewer, role: PortRole::DataStreamIn, port: "spikes".into() },
            rule: EdgeRule::default(),
        };
        action.edges = vec![edge.clone()];
        action.roles[0].rule.matching.label = Some(" 2$".into());
        let state = QuickState { graph: &other, layout: &layout };
        assert!(action.run(&state).is_none(), "the viewer's input isn't free");
        action.edges[0].rule = EdgeRule { if_occupied: IfOccupied::Swap, ..EdgeRule::default() };
        let replaced = action.run(&state).unwrap().graph;
        let feeding = |g: &Graph<Nodes>| g.edges().iter().find(|e| e.to.node == viewer).map(|e| e.from.node);
        assert_eq!(feeding(&replaced), Some(second));
        assert_eq!(feeding(&graph), Some(detector));
    }

    #[test]
    fn what_is_ignored_plays_no_part() {
        let (graph, layout, _, _) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Spikes", &graph, &layout, &[]);
        let empty = QuickState { graph: &Graph::default(), layout: &Layout::default() };
        action.roles[0].rule.used = false;
        let change = action.run(&empty).unwrap();
        assert_eq!(change.summary, "added Spike Viewer", "no detector, and nothing to connect it to");
        action.roles[0].rule.used = true;
        action.edges[0].rule = EdgeRule::NOTHING;
        assert_eq!(action.run(&empty).unwrap().summary, "added Spike Detector, Spike Viewer");
        // With every role ignored, the edge between them is too, even when the edge itself isn't.
        action.roles.iter_mut().for_each(|r| r.rule.used = false);
        action.edges[0].rule = EdgeRule::default();
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
        let mut action = QuickAction::from_graph("Spikes", &graph, &Layout::default(), &[right]);
        let found =
            |action: &QuickAction<Nodes>, graph: &Graph<Nodes>| action.find(graph).unwrap().get(&right).copied();
        assert_eq!(found(&action, &graph), Some(left));
        graph.get_mut(right).unwrap().label = "Spikes (tetrode 2)".into();
        action.roles[0].rule.matching.label = Some("tetrode \\d".into());
        assert_eq!(found(&action, &graph), Some(right), "only the one whose label matches");
        action.roles[0].rule.matching.label = Some("(".into());
        assert_eq!(found(&action, &graph), None, "a pattern that can't be read matches nothing");
        action.roles[0].rule.matching.label = Some(String::new());
        assert_eq!(found(&action, &graph), None, "nor does an empty one");
    }

    #[test]
    fn roles_with_one_candidate_are_paired_first_and_the_rest_left_to_right() {
        let mut graph = Graph::<Nodes>::default();
        let filter = || Nodes::create(crate::NodeType::BandPassFilter);
        let lfp = graph.add(filter(), [0.0, 0.0]);
        let spikes = graph.add(filter(), [300.0, 0.0]);
        let other = graph.add(filter(), [600.0, 0.0]);
        graph.get_mut(lfp).unwrap().label = "LFP".into();
        let mut action = QuickAction::from_graph("Filters", &graph, &Layout::default(), &[]);
        // In the quick action, the roles are listed the other way round from where they're placed.
        action.roles.reverse();
        for (role, x) in action.roles.iter_mut().zip([0.0, 300.0, 600.0]) {
            role.pos = [x, 0.0];
        }
        let (first, second, third) = (action.roles[0].id, action.roles[1].id, action.roles[2].id);
        let found = action.find(&graph).unwrap();
        assert_eq!((found[&first], found[&second], found[&third]), (lfp, spikes, other), "left to right");
        // The left-most role would take the LFP filter, but only the right-most role can.
        action.roles[2].rule.matching.label = Some("^LFP$".into());
        let found = action.find(&graph).unwrap();
        assert_eq!((found[&first], found[&second], found[&third]), (spikes, other, lfp));
    }

    #[test]
    fn edges_connected_already_can_be_enabled_or_disabled() {
        let (graph, layout, _, _) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Spikes", &graph, &layout, &[]);
        action.edges[0].rule.if_connected = EnableOrDisable::Disable;
        let change = action.run(&QuickState { graph: &graph, layout: &layout }).unwrap();
        assert!(change.graph.is_edge_disabled(&change.graph.edges()[0]));
        assert_eq!(change.summary, "disabled 1 connections");
        assert!(action.run(&QuickState { graph: &change.graph, layout: &layout }).is_none(), "disabled already");
        action.edges[0].rule.if_connected = EnableOrDisable::Enable;
        assert!(action.run(&QuickState { graph: &change.graph, layout: &layout }).is_some());
    }

    #[test]
    fn one_added_goes_as_it_is_from_what_it_is_connected_to_further_up_or_down_if_that_is_taken() {
        let (graph, layout, _, _) = detector_and_viewer();
        let mut action = QuickAction::from_graph("Spikes", &graph, &layout, &[]);
        // Where the viewer's added, with a filter at `blocked`, if any, and the room the filter takes.
        let added_at = |action: &QuickAction<Nodes>, blocked: Option<[f32; 2]>| {
            let mut graph = Graph::<Nodes>::default();
            graph.add(Nodes::create(crate::NodeType::SpikeDetector), [1000.0, 500.0]);
            let filter = blocked.map(|at| graph.add(Nodes::create(crate::NodeType::BandPassFilter), at));
            let change = action.run(&QuickState { graph: &graph, layout: &Layout::default() }).unwrap();
            let viewer = change.graph.iter().find(|(_, e)| e.label == "Spike Viewer").unwrap().0;
            let rects = rects(&change.graph);
            let rect = |id| rects.iter().find(|(other, _)| *other == id).map(|(_, r)| *r).unwrap();
            (rect(viewer), filter.map(rect))
        };
        assert_eq!(added_at(&action, None).0.min, egui::pos2(1300.0, 500.0), "as it is from the detector");
        let (viewer, filter) = added_at(&action, Some([1300.0, 500.0]));
        assert_eq!(viewer.min, egui::pos2(1300.0, filter.unwrap().bottom() + MARGIN), "level with it: down, clear");
        // Above the detector (at 500), with a filter (80 tall) where it'd go: back down towards the detector, below the
        // filter, where there's room for it (110 tall) above the detector's top...
        action.roles[1].pos = [300.0, -200.0];
        let (viewer, filter) = added_at(&action, Some([1300.0, 300.0]));
        assert_eq!(viewer.left(), 1300.0);
        assert_eq!(viewer.top(), filter.unwrap().bottom() + MARGIN, "above it: back towards it, clear");
        // ...and, where there isn't, further up.
        let (viewer, filter) = added_at(&action, Some([1300.0, 400.0]));
        assert_eq!(viewer.bottom(), filter.unwrap().top() - MARGIN, "above it: further up, clear");
        // Placed by what it gives data to, when nothing it takes data from is there.
        let mut viewer_only = Graph::<Nodes>::default();
        viewer_only.add(Nodes::create(crate::NodeType::SpikeViewer), [0.0, 0.0]);
        let change = action.run(&QuickState { graph: &viewer_only, layout: &Layout::default() }).unwrap();
        let detector = change.graph.iter().find(|(_, e)| e.label == "Spike Detector").map(|(_, e)| e.pos);
        assert_eq!(detector, Some([-300.0, 200.0]));
    }

    #[test]
    fn a_node_where_another_is_goes_to_the_nearest_gap_back_towards_what_it_is_placed_by() {
        // Tops between 100 and 200 are taken, and between 250 and 400.
        let blocked = || vec![(250.0, 400.0), (100.0, 200.0)];
        assert_eq!(slot(50.0, Some(0.0), blocked()), 50.0, "free");
        assert_eq!(slot(300.0, Some(0.0), blocked()), 250.0, "back towards it, below it");
        assert_eq!(slot(300.0, Some(260.0), blocked()), 400.0, "not passing it: further down");
        assert_eq!(slot(150.0, Some(500.0), blocked()), 200.0, "back towards it, above it");
        assert_eq!(slot(150.0, Some(180.0), blocked()), 100.0, "not passing it: further up");
        assert_eq!(slot(150.0, None, blocked()), 200.0, "with nothing it's placed by, down");
        // Gaps that overlap are one.
        assert_eq!(slot(300.0, Some(0.0), vec![(150.0, 260.0), (250.0, 400.0)]), 150.0);
    }

    #[test]
    fn a_window_opened_by_group_shape_goes_in_that_group() {
        let (graph, _, detector, viewer) = detector_and_viewer();
        // Home on the left, a third of the width, and the viewer's window on its right: as tall as home, but larger.
        let mut layout = Layout::default();
        layout.open(viewer, None);
        let mut action = QuickAction::from_graph("Detector", &graph, &Layout::default(), &[detector]);
        action.roles[0].rule.if_not_found = IfNotFound::DoNothing;
        assert_ne!(layout.largest_group(&[]), layout.tallest_group(&[]), "the tallest, of two as tall, is home's");
        let by = |shape| ByShape { shape, at: Position::Front };
        let group = |layout: &Layout| layout.group_of(Pane::Node(detector));
        for (shape, expected) in
            [(Shape::Largest, layout.largest_group(&[])), (Shape::Tallest, layout.tallest_group(&[]))]
        {
            action.roles[0].rule.if_window_closed = IfWindowClosed::Open(Placement::ByGroupShape(by(shape)));
            let change = action.run(&QuickState { graph: &graph, layout: &layout }).unwrap();
            assert_eq!(group(&change.layout), expected, "{shape:?}");
            assert!(change.layout.is_shown(Pane::Node(detector)), "at the front");
        }
    }
}
