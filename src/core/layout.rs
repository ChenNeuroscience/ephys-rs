//! The layout of panes, like an IDE's: the home pane, the dev tools and node windows, each a tab in a group, the
//! groups side by side or one above another. Panes can be dragged by their tabs to rearrange them (see
//! `crate::core::gui::docking`, which draws it). The app is in basic or advanced mode (see `Mode`): the home pane
//! shows the basic view (the viewers, and quick actions) or the graph.
//!
//! Where a pane goes when it's first opened is up to the code opening it, using the helpers here (which quick actions
//! use too, on a copy of the layout: see `crate::core::quick_action`):
//!
//! - A node's window takes a step towards the default layout (see `Arrangement::default_layout`): with one group
//!   showing, it's a new column on the right; with two, the second is split, it below; after that, it's a tab in the
//!   largest group other than the one it was opened from (see `place`).
//! - The dev tools are a tab in the largest group.
//! - The annotation pane is a tab in the group beside home (on its right, or failing that its left), or failing that
//!   a new column on its right.
//!
//! A window can be undocked to float, and docked again. Closing a docked window hides it but keeps its place, so it
//! comes back where it was when reopened.
//!
//! Sizes are worked out from the layout itself (see `rects`): the window's size when it was last drawn, shared out as
//! the groups share it, so panes placed one after another (before they've been drawn) are placed as they'll be.

use std::collections::{BTreeSet, HashMap, HashSet};

use eframe::egui::{self, Pos2, Rect, vec2};
use egui_tiles::{Container, Linear, LinearDir, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Graph, NodeId, NodesEnum};

/// What a pane in the layout shows.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Pane {
    /// Home: the basic view, or the graph (see `Mode`).
    Home,
    DevTools,
    /// The selected annotation's settings: there while an annotation's selected.
    Annotation,
    /// A node's window: its view, or its settings.
    Node(NodeId),
}

/// Basic or advanced: what the home pane shows, and what panes offer (each pane decides: see `UiContext::advanced`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Mode {
    /// The viewers, large, and quick actions: for most people, most of the time.
    #[default]
    Basic,
    /// The graph, to build and change it node by node, and each pane's settings.
    Advanced,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Basic => "Basic",
            Mode::Advanced => "Advanced",
        }
    }
}

/// Where a node's window is, if anywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowState {
    Closed,
    Floating,
    Docked,
    /// Docked, but closed: it keeps its place for when it's next opened.
    Hidden,
}

/// Which side of a group a new one goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// Where the panes are: docked in a tree of tabs and splits, or floating; and whether the home pane's basic or
/// advanced. Saved with the file, but for the mode.
#[derive(Clone, Serialize, Deserialize)]
pub struct Layout {
    pub(crate) tree: Tree<Pane>,
    floating: BTreeSet<NodeId>,
    /// Not kept with the file: the home pane opens at home (see `App::home`).
    #[serde(skip)]
    pub mode: Mode,
}

impl Default for Layout {
    fn default() -> Self {
        let mut tiles = Tiles::default();
        let main = tiles.insert_pane(Pane::Home);
        let root = tiles.insert_tab_tile(vec![main]);
        Layout { tree: Tree::new(egui::Id::unique("dock"), root, tiles), floating: BTreeSet::new(), mode: Mode::Basic }
    }
}

/// The window's size, before the layout's been drawn.
const TYPICAL: egui::Vec2 = vec2(1400.0, 800.0);

impl Layout {
    fn tile(&self, pane: Pane) -> Option<TileId> {
        self.tree.tiles.find_pane(&pane)
    }

    pub fn window(&self, id: NodeId) -> WindowState {
        if self.floating.contains(&id) {
            return WindowState::Floating;
        }
        match self.tile(Pane::Node(id)) {
            Some(tile) if self.tree.tiles.is_visible(tile) => WindowState::Docked,
            Some(_) => WindowState::Hidden,
            None => WindowState::Closed,
        }
    }

    /// Nodes whose windows are showing, floating or docked.
    pub fn open_windows(&self) -> HashSet<NodeId> {
        let docked = self.tree.tiles.iter().filter_map(|(tile, t)| match t {
            Tile::Pane(Pane::Node(id)) if self.tree.tiles.is_visible(*tile) => Some(*id),
            _ => None,
        });
        self.floating.iter().copied().chain(docked).collect()
    }

    pub fn floating(&self) -> Vec<NodeId> {
        self.floating.iter().copied().collect()
    }

    pub fn is_shown(&self, pane: Pane) -> bool {
        self.tile(pane).is_some_and(|t| self.tree.tiles.is_visible(t))
    }

    // Opening and closing.

    /// Opens a node's window (where it was last, if it's been open before; otherwise see `place`, `from` being the
    /// pane it's opened from, if any), or closes it.
    pub fn toggle(&mut self, id: NodeId, from: Option<Pane>) {
        match self.window(id) {
            WindowState::Closed => self.dock(id, from),
            WindowState::Floating => {
                self.floating.remove(&id);
            }
            WindowState::Docked => self.hide(Pane::Node(id)),
            WindowState::Hidden => self.show(Pane::Node(id)),
        }
    }

    /// Opens a node's window (where it was last, if it's been open before; otherwise see `place`), or brings it to the
    /// front.
    pub fn open(&mut self, id: NodeId, from: Option<Pane>) {
        match self.window(id) {
            WindowState::Floating => {}
            WindowState::Docked | WindowState::Hidden => self.show(Pane::Node(id)),
            WindowState::Closed => self.dock(id, from),
        }
    }

    /// Closes a node's window, if it's open (keeping its place, if it's docked).
    pub fn close(&mut self, id: NodeId) {
        match self.window(id) {
            WindowState::Floating => {
                self.floating.remove(&id);
            }
            WindowState::Docked => self.hide(Pane::Node(id)),
            WindowState::Closed | WindowState::Hidden => {}
        }
    }

    /// Shows or hides the dev tools: the first time, as a tab in the largest group.
    pub fn toggle_dev_tools(&mut self) {
        let pane = Pane::DevTools;
        if self.tile(pane).is_none() {
            match self.largest_group(&[]) {
                Some(group) => self.add_tab(group, pane),
                None => self.as_root(pane),
            }
            return self.show(pane);
        }
        if self.is_shown(pane) { self.hide(pane) } else { self.show(pane) }
    }

    /// Shows or hides the annotation pane: the first time, as a tab in the group beside home (on its right, or
    /// failing that its left), or failing that in a new column on its right.
    pub fn show_annotation_pane(&mut self, show: bool) {
        let pane = Pane::Annotation;
        if self.is_shown(pane) == show {
            return;
        }
        if !show {
            return self.hide(pane);
        }
        if self.tile(pane).is_none() {
            match self.group_of(Pane::Home) {
                Some(home) => match self.beside(home) {
                    Some(group) => self.add_tab(group, pane),
                    None => {
                        self.split(home, pane, Side::Right, 0.25);
                    }
                },
                None => match self.largest_group(&[]) {
                    Some(group) => self.add_tab(group, pane),
                    None => self.as_root(pane),
                },
            }
        }
        self.show(pane);
    }

    /// Shows a docked pane, bringing its tab to the front.
    pub fn show(&mut self, pane: Pane) {
        let Some(tile) = self.tile(pane) else { return };
        let mut child = tile;
        self.tree.tiles.set_visible(child, true);
        while let Some(parent) = self.tree.tiles.parent_of(child) {
            self.tree.tiles.set_visible(parent, true);
            if let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(parent) {
                tabs.set_active(child);
            }
            child = parent;
        }
    }

    /// Hides a docked pane, keeping its place.
    pub fn hide(&mut self, pane: Pane) {
        if let Some(tile) = self.tile(pane) {
            hide_tile(&mut self.tree.tiles, tile);
        }
    }

    /// Puts a node's window in the layout (see `place`), unless it's there already (hidden), and shows it.
    pub fn dock(&mut self, id: NodeId, from: Option<Pane>) {
        self.floating.remove(&id);
        let pane = Pane::Node(id);
        if self.tile(pane).is_none() {
            self.place(pane, from);
        }
        self.show(pane);
    }

    /// Takes a docked window out of the layout, to float.
    pub fn float(&mut self, id: NodeId) {
        if let Some(tile) = self.tile(Pane::Node(id)) {
            self.tree.remove_recursively(tile);
        }
        self.floating.insert(id);
    }

    /// Forgets the windows of nodes that no longer exist.
    pub fn retain_nodes<N: NodesEnum>(&mut self, graph: &Graph<N>) {
        self.floating.retain(|id| graph.contains(*id));
        let gone: Vec<TileId> = self
            .tree
            .tiles
            .iter()
            .filter_map(|(tile, t)| match t {
                Tile::Pane(Pane::Node(id)) if !graph.contains(*id) => Some(*tile),
                _ => None,
            })
            .collect();
        for tile in gone {
            self.tree.remove_recursively(tile);
        }
    }

    // Placing panes.

    /// Puts a pane in the layout, a step towards the default layout (see `Arrangement::default_layout`): with one
    /// group showing, in a new column on its right, taking two thirds of the width; with two, below the second (the
    /// rightmost), taking a third of its height; after that, as a tab in the largest group, other than the one it's
    /// opened from (`from`'s), if it's opened from a pane.
    pub fn place(&mut self, pane: Pane, from: Option<Pane>) {
        let rects = self.rects();
        let groups = self.groups_in(&rects);
        match groups.as_slice() {
            [] => self.as_root(pane),
            [only] => {
                self.split(*only, pane, Side::Right, 2.0 / 3.0);
            }
            [_, _] => {
                let second = groups.iter().copied().max_by(|a, b| rects[a].left().total_cmp(&rects[b].left()));
                self.split(second.unwrap_or(groups[1]), pane, Side::Below, 1.0 / 3.0);
            }
            _ => {
                let except: Vec<TileId> = from.and_then(|from| self.group_of(from)).into_iter().collect();
                match self.largest_group(&except) {
                    Some(group) => self.add_tab(group, pane),
                    None => self.as_root(pane),
                }
            }
        }
    }

    /// The group of tabs `pane` is in, if it's docked.
    pub fn group_of(&self, pane: Pane) -> Option<TileId> {
        let parent = self.tree.tiles.parent_of(self.tile(pane)?)?;
        matches!(self.tree.tiles.get(parent), Some(Tile::Container(Container::Tabs(_)))).then_some(parent)
    }

    /// The groups of tabs showing (with a tab showing), as laid out (see `rects`).
    pub fn groups(&self) -> Vec<TileId> {
        self.groups_in(&self.rects())
    }

    fn groups_in(&self, rects: &HashMap<TileId, Rect>) -> Vec<TileId> {
        let tiles = &self.tree.tiles;
        let mut groups: Vec<TileId> = rects
            .keys()
            .copied()
            .filter(|tile| match tiles.get(*tile) {
                Some(Tile::Container(Container::Tabs(tabs))) => tabs.children.iter().any(|c| tiles.is_visible(*c)),
                _ => false,
            })
            .collect();
        groups.sort_by_key(|tile| tile.0);
        groups
    }

    /// The largest group of tabs showing (as laid out: see `rects`), other than `except`: of two as large, the first
    /// made.
    pub fn largest_group(&self, except: &[TileId]) -> Option<TileId> {
        let rects = self.rects();
        let groups = self.groups_in(&rects);
        let area = |tile: &TileId| rects.get(tile).map_or(0.0, |r| r.area());
        groups.into_iter().filter(|g| !except.contains(g)).rev().max_by(|a, b| area(a).total_cmp(&area(b)))
    }

    /// The group of tabs beside `group` (as laid out: see `rects`): on its right, or failing that its left, the one
    /// alongside the most of it.
    pub fn beside(&self, group: TileId) -> Option<TileId> {
        let rects = self.rects();
        let of = *rects.get(&group)?;
        let alongside = |rect: &Rect| (rect.bottom().min(of.bottom()) - rect.top().max(of.top())).max(0.0);
        let side = |right: bool| {
            self.groups_in(&rects)
                .into_iter()
                .filter(|g| *g != group)
                .filter(|g| {
                    let rect = rects[g];
                    let touching =
                        if right { rect.left() >= of.right() - 1.0 } else { rect.right() <= of.left() + 1.0 };
                    touching && alongside(&rect) > 0.0
                })
                .max_by(|a, b| alongside(&rects[a]).total_cmp(&alongside(&rects[b])))
        };
        side(true).or_else(|| side(false))
    }

    /// Adds `pane` as a tab of `group`.
    pub fn add_tab(&mut self, group: TileId, pane: Pane) {
        let tile = self.tree.tiles.insert_pane(pane);
        match self.tree.tiles.get_mut(group) {
            Some(Tile::Container(container)) => container.add_child(tile),
            _ => self.tree.root = Some(self.tree.tiles.insert_tab_tile(vec![tile])),
        }
    }

    /// Puts `pane` in a new group on `side` of `target` (a tile: a group, or a row or column of them), taking `share`
    /// of the space they then share. Returns the new group.
    pub fn split(&mut self, target: TileId, pane: Pane, side: Side, share: f32) -> TileId {
        let tiles = &mut self.tree.tiles;
        let tile = tiles.insert_pane(pane);
        let new = tiles.insert_tab_tile(vec![tile]);
        let parent = tiles.parent_of(target);
        let dir = if matches!(side, Side::Left | Side::Right) { LinearDir::Horizontal } else { LinearDir::Vertical };
        let linear = match side {
            Side::Right | Side::Below => Linear::new_binary(dir, [target, new], 1.0 - share),
            Side::Left | Side::Above => Linear::new_binary(dir, [new, target], share),
        };
        let container = tiles.insert_container(linear);
        match parent.and_then(|p| tiles.get_mut(p)) {
            Some(Tile::Container(parent)) => {
                let _ = parent.replace_child(target, container);
            }
            _ => self.tree.root = Some(container),
        }
        new
    }

    /// Makes `pane` the whole layout, if there's nothing showing (else alongside what's there, on the right).
    fn as_root(&mut self, pane: Pane) {
        match self.tree.root {
            Some(root) => {
                self.split(root, pane, Side::Right, 0.5);
            }
            None => {
                let tile = self.tree.tiles.insert_pane(pane);
                self.tree.root = Some(self.tree.tiles.insert_tab_tile(vec![tile]));
            }
        }
    }

    /// Each tile's rectangle, as it's laid out: the whole layout's, as last drawn (or a typical window's, before it's
    /// been drawn), shared out as the rows and columns share it. Only those showing.
    pub fn rects(&self) -> HashMap<TileId, Rect> {
        let mut rects = HashMap::new();
        if let Some(root) = self.tree.root {
            let whole = self.tree.tiles.rect(root).unwrap_or(Rect::from_min_size(Pos2::ZERO, TYPICAL));
            self.lay_out(root, whole, &mut rects);
        }
        rects
    }

    fn lay_out(&self, tile: TileId, rect: Rect, rects: &mut HashMap<TileId, Rect>) {
        let tiles = &self.tree.tiles;
        if !tiles.is_visible(tile) {
            return;
        }
        rects.insert(tile, rect);
        match tiles.get(tile) {
            Some(Tile::Container(Container::Linear(linear))) => {
                let showing: Vec<TileId> = linear.children.iter().copied().filter(|c| tiles.is_visible(*c)).collect();
                let total: f32 = showing.iter().map(|c| linear.shares[*c]).sum::<f32>().max(f32::MIN_POSITIVE);
                let mut at = 0.0;
                for child in showing {
                    let part = linear.shares[child] / total;
                    let child_rect = match linear.dir {
                        LinearDir::Horizontal => Rect::from_min_size(
                            rect.min + vec2(rect.width() * at, 0.0),
                            vec2(rect.width() * part, rect.height()),
                        ),
                        LinearDir::Vertical => Rect::from_min_size(
                            rect.min + vec2(0.0, rect.height() * at),
                            vec2(rect.width(), rect.height() * part),
                        ),
                    };
                    self.lay_out(child, child_rect, rects);
                    at += part;
                }
            }
            Some(Tile::Container(container)) => {
                for child in container.children().copied().collect::<Vec<_>>() {
                    self.lay_out(child, rect, rects);
                }
            }
            _ => {}
        }
    }

    // Arranging it all.

    /// Replaces the layout with `arrangement`, its windows docked and shown (those floating docked), and the panes
    /// docked before that it doesn't place where it says (see `Tab::Others`).
    pub fn arrange(&mut self, arrangement: &Arrangement) {
        let placed = |pane: Pane| match pane {
            Pane::Home => arrangement.places(Tab::Home),
            Pane::Node(id) => arrangement.places(Tab::Window(id)),
            _ => false,
        };
        let others: Vec<(Pane, bool)> = self.panes_in_order().into_iter().filter(|(pane, _)| !placed(*pane)).collect();
        let mut tiles = Tiles::default();
        let root = build(&mut tiles, arrangement, &others);
        self.tree = Tree::new(self.tree.id(), root, tiles);
        for (pane, shown) in others {
            if !shown && let Some(tile) = self.tile(pane) {
                self.tree.tiles.set_visible(tile, false);
            }
        }
        self.floating.retain(|id| !arrangement.places(Tab::Window(*id)));
    }

    /// The docked panes, in the order they're laid out (left to right, top to bottom, tab by tab), and whether each
    /// is shown.
    fn panes_in_order(&self) -> Vec<(Pane, bool)> {
        let tiles = &self.tree.tiles;
        let mut panes = Vec::new();
        let mut stack: Vec<(TileId, bool)> = self.tree.root.map(|r| (r, true)).into_iter().collect();
        while let Some((tile, shown)) = stack.pop() {
            let shown = shown && tiles.is_visible(tile);
            match tiles.get(tile) {
                Some(Tile::Pane(pane)) => panes.push((*pane, shown)),
                Some(Tile::Container(container)) => {
                    let children: Vec<TileId> = container.children().copied().collect();
                    stack.extend(children.into_iter().rev().map(|c| (c, shown)));
                }
                None => {}
            }
        }
        panes
    }

    /// The tab group a docked pane's in, and whether it's the tab in front.
    #[cfg(test)]
    pub fn tab_group(&self, pane: Pane) -> Option<(TileId, bool)> {
        let tile = self.tile(pane)?;
        let group = self.tree.tiles.parent_of(tile)?;
        match self.tree.tiles.get(group) {
            Some(Tile::Container(Container::Tabs(tabs))) => Some((group, tabs.active == Some(tile))),
            _ => None,
        }
    }
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
    /// The default layout: a column the full height on the left, a third of the width; and on the right, one above
    /// another, `top` (two thirds of the height) and `bottom`. Windows opened take steps towards it (see
    /// `Layout::place`).
    pub fn default_layout(left: Arrangement, top: Arrangement, bottom: Arrangement) -> Arrangement {
        Arrangement::Row(vec![
            (1.0 / 3.0, left),
            (2.0 / 3.0, Arrangement::Column(vec![(2.0 / 3.0, top), (1.0 / 3.0, bottom)])),
        ])
    }

    /// Tabs, the first in front.
    pub fn tabs(tabs: Vec<Tab>) -> Arrangement {
        Arrangement::Tabs { tabs, active: 0 }
    }

    /// Whether it has `tab`.
    pub fn places(&self, tab: Tab) -> bool {
        match self {
            Arrangement::Row(parts) | Arrangement::Column(parts) => parts.iter().any(|(_, part)| part.places(tab)),
            Arrangement::Tabs { tabs, .. } => tabs.contains(&tab),
        }
    }
}

/// Puts `arrangement` in `tiles`, returning its top tile; `others` go where it says.
fn build(tiles: &mut Tiles<Pane>, arrangement: &Arrangement, others: &[(Pane, bool)]) -> TileId {
    match arrangement {
        Arrangement::Row(parts) | Arrangement::Column(parts) => {
            let dir =
                if matches!(arrangement, Arrangement::Row(_)) { LinearDir::Horizontal } else { LinearDir::Vertical };
            let children: Vec<(TileId, f32)> =
                parts.iter().map(|(share, part)| (build(tiles, part, others), *share)).collect();
            let mut linear = Linear::new(dir, children.iter().map(|c| c.0).collect());
            for (child, share) in children {
                linear.shares.set_share(child, share);
            }
            tiles.insert_container(linear)
        }
        Arrangement::Tabs { tabs, active } => {
            let mut children = Vec::new();
            let mut front = None;
            for (index, tab) in tabs.iter().enumerate() {
                let panes: Vec<Pane> = match tab {
                    Tab::Home => vec![Pane::Home],
                    Tab::Window(id) => vec![Pane::Node(*id)],
                    Tab::Others => others.iter().map(|o| o.0).collect(),
                };
                for pane in panes {
                    let tile = tiles.insert_pane(pane);
                    if index == *active && front.is_none() {
                        front = Some(tile);
                    }
                    children.push(tile);
                }
            }
            let mut container = egui_tiles::Tabs::new(children);
            if let Some(front) = front {
                container.set_active(front);
            }
            tiles.insert_container(container)
        }
    }
}

/// Hides a tile, bringing another tab to the front in its place.
pub(crate) fn hide_tile(tiles: &mut Tiles<Pane>, tile: TileId) {
    tiles.set_visible(tile, false);
    let Some(parent) = tiles.parent_of(tile) else { return };
    let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get(parent) else { return };
    let next = tabs.children.iter().copied().find(|c| *c != tile && tiles.is_visible(*c));
    if let (Some(next), Some(Tile::Container(Container::Tabs(tabs)))) = (next, tiles.get_mut(parent)) {
        tabs.set_active(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(n: u64) -> Pane {
        Pane::Node(NodeId(n))
    }

    #[test]
    fn windows_take_steps_towards_the_default_layout() {
        let mut layout = Layout::default();
        // One group (home's): a column on its right, two thirds of the width.
        layout.open(NodeId(1), Some(Pane::Home));
        let rects = layout.rects();
        let (home, first) = (layout.group_of(Pane::Home).unwrap(), layout.group_of(node(1)).unwrap());
        assert!((rects[&first].width() - rects[&home].width() * 2.0).abs() < 1.0);
        // Two: the second split, two thirds above, a third below.
        layout.open(NodeId(2), Some(Pane::Home));
        let rects = layout.rects();
        let second = layout.group_of(node(2)).unwrap();
        assert!(rects[&second].top() > rects[&first].top());
        assert!((rects[&first].height() - rects[&second].height() * 2.0).abs() < 1.0);
        // Three: a tab in the largest group, not the one it's opened from.
        layout.open(NodeId(3), Some(node(1)));
        assert_eq!(layout.group_of(node(3)), Some(home), "home's, as the first's is the one it's opened from");
        layout.open(NodeId(4), None);
        assert_eq!(layout.group_of(node(4)), Some(first), "the largest");
    }

    #[test]
    fn the_dev_tools_and_annotation_pane_go_beside_what_is_there() {
        // With only home, the dev tools are a tab of it, and the annotation pane a column on its right.
        let mut layout = Layout::default();
        layout.toggle_dev_tools();
        let home = layout.group_of(Pane::Home).unwrap();
        assert_eq!(layout.group_of(Pane::DevTools), Some(home));
        layout.show_annotation_pane(true);
        let annotation = layout.group_of(Pane::Annotation).unwrap();
        assert!(layout.rects()[&annotation].left() >= layout.rects()[&home].right() - 1.0);
        // With windows on the right, a tab beside them.
        let mut layout = Layout::default();
        layout.open(NodeId(1), Some(Pane::Home));
        layout.show_annotation_pane(true);
        assert_eq!(layout.group_of(Pane::Annotation), layout.group_of(node(1)));
        // And with them on the left, there.
        let mut layout = Layout::default();
        layout.open(NodeId(1), Some(Pane::Home));
        let windows = layout.group_of(node(1)).unwrap();
        let root = layout.tree.root.unwrap();
        if let Some(Tile::Container(Container::Linear(row))) = layout.tree.tiles.get_mut(root) {
            row.children.reverse();
        }
        layout.show_annotation_pane(true);
        assert_eq!(layout.group_of(Pane::Annotation), Some(windows));
    }
}
