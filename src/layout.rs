//! The app lays out its panes like an IDE: the home pane, the dev tools and node windows, each a tab in a group, the
//! groups side by side or one above another. Panes can be dragged by their tabs to rearrange them (see
//! `crate::gui::docking`, which draws it). The app is in basic or advanced mode (see `Mode`): the home pane
//! shows the basic view (the viewers, and quick actions) or the graph.
//!
//! Where a pane goes when it's first opened is up to the code opening it, using the helpers here (which quick actions
//! use too, on a copy of the layout: see `crate::quick_action`):
//!
//! - A node's window takes a step towards the default layout: with one group showing, it's a new column on the right;
//!   with two, the second is split, it below; after that, it's a tab in the largest group other than the one it was
//!   opened from (see `place`).
//! - A quick action can put windows where they were (`Spot`), cutting the layout to fit (see `place_exactly`).
//! - The dev tools are a tab in the largest group.
//! - The annotation pane is a tab in the group beside home (on its right, or failing that its left), or failing that
//!   a new column on its right.
//!
//! A window can be undocked to float, and docked again. Closing a docked window hides it but keeps its place, so it
//! comes back where it was when reopened.
//!
//! The layout works out sizes itself (see `rects`): the window's size when it was last drawn, shared out as
//! the groups share it, so panes placed one after another (before they've been drawn) are placed as they'll be.

use std::collections::{BTreeSet, HashMap, HashSet};

use eframe::egui::{self, Pos2, Rect, pos2, vec2};
use egui_tiles::{Container, Linear, LinearDir, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use crate::engine::{Graph, NodeId, NodesEnum};

/// A pane in the layout shows one of these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Pane {
    /// The home pane shows the basic view: the components, and the quick actions.
    Home,
    /// The graph pane shows the graph, to build and change node by node. It's there only in advanced mode (see
    /// `Layout::set_mode`), and can't be closed by hand.
    Graph,
    DevTools,
    /// The annotation pane shows the selected annotation's settings, while an annotation's selected.
    Annotation,
    /// A node's window shows its view, or its settings.
    Node(NodeId),
    /// A group of tabs with nothing in it has this, while a quick action places windows exactly (see
    /// `Layout::place_exactly`), as a placeholder for what goes there. It's never drawn: those left are closed first
    /// (`Layout::close_empty`).
    Empty,
}

/// The app is in basic or advanced mode, which decides whether there's a graph pane, and what panes offer (each pane
/// decides: see `UiContext::advanced`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Basic mode shows the viewers, large, and quick actions: for most people, most of the time.
    #[default]
    Basic,
    /// Advanced mode adds the graph pane, to build and change the graph node by node, and shows each pane's settings.
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

/// A node's window is in one of these states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowState {
    Closed,
    Floating,
    Docked,
    /// A hidden window's docked, but closed: it keeps its place for when it's next opened.
    Hidden,
}

/// A new group goes on one side of another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// The layout keeps where the panes are (docked in a tree of tabs and splits, or floating), and whether the home pane's
/// basic or advanced. It's saved with the file, but for the mode.
#[derive(Clone, Serialize, Deserialize)]
pub struct Layout {
    pub(crate) tree: Tree<Pane>,
    floating: BTreeSet<NodeId>,
    /// The mode isn't kept with the file: the home pane opens at home (see `App::home`).
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

/// Before the layout's been drawn, the layout takes the window to be this size.
const TYPICAL: egui::Vec2 = vec2(1400.0, 800.0);

/// No pane (or group of them) can be dragged narrower or shorter than this (see `crate::gui::docking`).
pub const MIN_PANE: f32 = 300.0;

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

    /// The nodes whose windows are showing, floating or docked.
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

    /// The app opens a node's window with this (where it was last, if it's been open before; otherwise see `place`,
    /// `from` being the pane it's opened from, if any), or closes it.
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

    /// The app opens a node's window with this (where it was last, if it's been open before; otherwise see `place`),
    /// or brings it to the front.
    pub fn open(&mut self, id: NodeId, from: Option<Pane>) {
        match self.window(id) {
            WindowState::Floating => {}
            WindowState::Docked | WindowState::Hidden => self.show(Pane::Node(id)),
            WindowState::Closed => self.dock(id, from),
        }
    }

    /// The app closes a node's window with this, if it's open (keeping its place, if it's docked).
    pub fn close(&mut self, id: NodeId) {
        match self.window(id) {
            WindowState::Floating => {
                self.floating.remove(&id);
            }
            WindowState::Docked => self.hide(Pane::Node(id)),
            WindowState::Closed | WindowState::Hidden => {}
        }
    }

    /// The app shows or hides the dev tools with this: the first time, as a tab in the largest group.
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

    /// The app shows or hides the annotation pane with this: the first time, as a tab in the group beside home (on its
    /// right, or failing that its left), or failing that in a new column on its right.
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

    /// The app changes mode with this. Into advanced mode, the graph pane's brought to the front (the first time, as a
    /// tab in the largest group); out of it, the graph pane's taken out of the layout, and home brought to the front.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        let pane = Pane::Graph;
        match mode {
            Mode::Advanced => {
                if self.tile(pane).is_none() {
                    match self.largest_group(&[]) {
                        Some(group) => self.add_tab(group, pane),
                        None => self.as_root(pane),
                    }
                }
                self.show(pane);
            }
            Mode::Basic => {
                if let Some(tile) = self.tile(pane) {
                    self.tree.remove_recursively(tile);
                }
                self.show(Pane::Home);
            }
        }
    }

    /// The app shows a docked pane with this, bringing its tab to the front.
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

    /// The app hides a docked pane with this, keeping its place.
    pub fn hide(&mut self, pane: Pane) {
        if let Some(tile) = self.tile(pane) {
            hide_tile(&mut self.tree.tiles, tile);
        }
    }

    /// Docking puts a node's window in the layout (see `place`), unless it's there already (hidden), and shows it.
    pub fn dock(&mut self, id: NodeId, from: Option<Pane>) {
        self.floating.remove(&id);
        let pane = Pane::Node(id);
        if self.tile(pane).is_none() {
            self.place(pane, from);
        }
        self.show(pane);
    }

    /// Floating takes a docked window out of the layout.
    pub fn float(&mut self, id: NodeId) {
        if let Some(tile) = self.tile(Pane::Node(id)) {
            self.tree.remove_recursively(tile);
        }
        self.floating.insert(id);
    }

    /// The layout forgets the windows of nodes that no longer exist, with this.
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

    /// A pane opened for the first time goes in the layout here. With a group left empty for it (see
    /// `place_exactly`), there, the largest; otherwise, a step towards the default layout (a column a third of the
    /// width on the left, the full height; on the right, one group above another, two thirds and a third of the
    /// height): with one group showing, in a new column on its right, taking two thirds of the width; with two, below
    /// the second (the rightmost), taking a third of its height; after that, as a tab in the largest group, other than
    /// the one it's opened from (`from`'s), if it's opened from a pane.
    pub fn place(&mut self, pane: Pane, from: Option<Pane>) {
        if let Some(group) = self.empty_group() {
            return self.add_tab(group, pane);
        }
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

    /// The groups of tabs showing (with a tab showing), as laid out (`rects`).
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
        self.group_with_most(except, Rect::area)
    }

    /// The widest group of tabs showing, as `largest_group` finds the largest.
    pub fn widest_group(&self, except: &[TileId]) -> Option<TileId> {
        self.group_with_most(except, Rect::width)
    }

    /// The tallest group of tabs showing, as `largest_group` finds the largest.
    pub fn tallest_group(&self, except: &[TileId]) -> Option<TileId> {
        self.group_with_most(except, Rect::height)
    }

    /// The group of tabs showing (as laid out: see `rects`), other than `except`, with the most of `measure` (its area,
    /// say): of two with as much, the first made.
    fn group_with_most(&self, except: &[TileId], measure: fn(&Rect) -> f32) -> Option<TileId> {
        let rects = self.rects();
        let groups = self.groups_in(&rects);
        let size = |tile: &TileId| rects.get(tile).map_or(0.0, measure);
        groups.into_iter().filter(|g| !except.contains(g)).rev().max_by(|a, b| size(a).total_cmp(&size(b)))
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

    /// This adds `pane` as a tab of `group`, in place of its placeholder, if it has one (see `place_exactly`).
    pub fn add_tab(&mut self, group: TileId, pane: Pane) {
        let tile = self.tree.tiles.insert_pane(pane);
        let placeholders: Vec<TileId> = match self.tree.tiles.get(group) {
            Some(Tile::Container(container)) => (container.children().copied())
                .filter(|c| matches!(self.tree.tiles.get(*c), Some(Tile::Pane(Pane::Empty))))
                .collect(),
            _ => Vec::new(),
        };
        match self.tree.tiles.get_mut(group) {
            Some(Tile::Container(container)) => container.add_child(tile),
            _ => self.tree.root = Some(self.tree.tiles.insert_tab_tile(vec![tile])),
        }
        for placeholder in &placeholders {
            self.tree.remove_recursively(*placeholder);
        }
        if !placeholders.is_empty()
            && let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(group)
        {
            tabs.set_active(tile);
        }
    }

    /// This puts `pane` in a new group on `side` of `target` (a tile: a group, or a row or column of them), taking
    /// `share` of the space they then share, and returns the new group.
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

    /// With nothing showing, this makes `pane` the whole layout (otherwise it goes alongside what's there, on the
    /// right).
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

    /// The layout works out each tile's rectangle with this: the whole layout's, as last drawn (or a typical window's,
    /// before it's been drawn), shared out as the rows and columns share it. Only those showing have one.
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

    // Placing panes exactly.

    /// A quick action keeps where a window is with this (see `Spot`): only a pane docked and showing has a spot.
    pub fn spot(&self, pane: Pane) -> Option<Spot> {
        let tile = self.tile(pane)?;
        let group = self.group_of(pane)?;
        let rects = self.rects();
        let (whole, rect) = (*rects.get(&self.tree.root?)?, *rects.get(&group)?);
        if !self.tree.tiles.is_visible(tile) {
            return None;
        }
        let x = |at: f32| (at - whole.left()) / whole.width();
        let y = |at: f32| (at - whole.top()) / whole.height();
        let front = matches!(self.tree.tiles.get(group), Some(Tile::Container(Container::Tabs(tabs))) if tabs.active == Some(tile));
        Some(Spot { rect: [x(rect.left()), y(rect.top()), x(rect.right()), y(rect.bottom())], front })
    }

    /// A quick action puts windows opened for the first time where they were (each `Spot`) with this, before it opens
    /// any others. The layout is cut into groups with each spot one of them, along lines across the whole of what's
    /// cut, as rows and columns are; the rest of the room left is in groups of its own (empty, at first). Each pane
    /// docked before goes in the group of those left over it overlaps most, or, overlapping none, the group of a spot
    /// it overlaps most, as it was (in front, or behind, or hidden): so the windows placed keep their spots while
    /// there's room for what was there.
    ///
    /// Spots are as wide and as tall as the layout is now: lines closer than `MIN_PANE` (or a sixth of the layout,
    /// whichever is less) are taken to be one line, so a spot from a larger window leaves no slivers. A window whose
    /// spot is one already placed (another window's, before it, of the same group) is a tab of its group; one whose
    /// spot overlaps another's, or can't be cut out with those before it, isn't placed. This gives the panes not
    /// placed.
    ///
    /// The groups left empty are filled first by windows opened after (see `place`), and those still empty are closed
    /// (see `close_empty`), so none is ever drawn.
    pub fn place_exactly(&mut self, wanted: &[(Pane, Spot)]) -> Vec<Pane> {
        if wanted.is_empty() {
            return Vec::new();
        }
        let before = self.rects();
        let whole = (self.tree.root.and_then(|root| before.get(&root).copied()))
            .unwrap_or(Rect::from_min_size(Pos2::ZERO, TYPICAL));
        let lines = |across: bool| {
            let (from, to) = span(&whole, across);
            let at: Vec<f32> = (wanted.iter())
                .flat_map(|(_, spot)| {
                    let [left, top, right, bottom] = spot.rect;
                    if across { [left, right] } else { [top, bottom] }
                })
                .map(|f| from + f.clamp(0.0, 1.0) * (to - from))
                .collect();
            snap(&at, from, to, MIN_PANE.min((to - from) / 6.0))
        };
        let (xs, ys) = (lines(true), lines(false));

        // The spots, each one group, in order, with the windows placed there.
        let mut spots: Vec<(Rect, Vec<(Pane, bool)>)> = Vec::new();
        let mut unplaced = Vec::new();
        for (i, (pane, spot)) in wanted.iter().enumerate() {
            let rect = Rect::from_min_max(pos2(xs[2 * i], ys[2 * i]), pos2(xs[2 * i + 1], ys[2 * i + 1]));
            if let Some((_, panes)) = spots.iter_mut().find(|(other, _)| *other == rect) {
                panes.push((*pane, spot.front));
                continue;
            }
            let clashes = spots.iter().any(|(other, _)| other.intersect(rect).is_positive());
            let rects: Vec<Rect> = spots.iter().map(|(r, _)| *r).chain([rect]).collect();
            if !rect.is_positive() || clashes || cut(whole, &rects, (0..rects.len()).collect()).is_none() {
                unplaced.push(*pane);
                continue;
            }
            spots.push((rect, vec![(*pane, spot.front)]));
        }
        let rects: Vec<Rect> = spots.iter().map(|(r, _)| *r).collect();
        let Some(cuts) = cut(whole, &rects, (0..rects.len()).collect()) else { return unplaced };

        // The panes there before, each in the group it overlaps most, with whether it's showing and in front.
        let groups = cuts.groups();
        let left_over = cuts.left_over();
        let placing: Vec<Pane> = spots.iter().flat_map(|(_, panes)| panes.iter().map(|(pane, _)| *pane)).collect();
        let mut adopted: Vec<Vec<(Pane, bool)>> = vec![Vec::new(); groups.len()];
        let mut fronts = Vec::new();
        for (pane, shown) in self.panes_in_order() {
            if placing.contains(&pane) || pane == Pane::Empty {
                continue;
            }
            let group = self.group_of(pane);
            let overlap = |rect: &Rect| match group.and_then(|g| before.get(&g)) {
                Some(old) => old.intersect(*rect).area().max(0.0),
                // Not showing (in a group hidden), it goes in the largest.
                None => rect.area(),
            };
            let best_of = |among: &dyn Fn(usize) -> bool| {
                (groups.iter().enumerate().rev())
                    .filter(|(i, rect)| among(*i) && overlap(rect) > 0.0)
                    .max_by(|a, b| overlap(a.1).total_cmp(&overlap(b.1)))
                    .map(|(i, _)| i)
            };
            let best = best_of(&|i| left_over[i]).or_else(|| best_of(&|_| true)).unwrap_or(0);
            if shown && group.is_some_and(|g| self.tab_in_front(g) == self.tile(pane)) {
                fronts.push(pane);
            }
            adopted[best].push((pane, shown));
        }

        let mut tiles = Tiles::default();
        let root = cuts.build(&mut tiles, &spots, &adopted, &fronts, &mut 0);
        self.tree = Tree::new(self.tree.id(), root, tiles);
        unplaced
    }

    /// The tab in front of `group`, if it's a group of tabs.
    fn tab_in_front(&self, group: TileId) -> Option<TileId> {
        match self.tree.tiles.get(group) {
            Some(Tile::Container(Container::Tabs(tabs))) => tabs.active,
            _ => None,
        }
    }

    /// The groups of tabs left with nothing in them, of those `place_exactly` makes, are closed with this, as though
    /// by hand: those with panes hidden in them are hidden, the rest taken out. Their room goes to the groups beside
    /// them.
    pub fn close_empty(&mut self) {
        let empty: Vec<TileId> = (self.tree.tiles.iter())
            .filter_map(|(tile, t)| matches!(t, Tile::Pane(Pane::Empty)).then_some(*tile))
            .collect();
        for tile in empty {
            let group = self.tree.tiles.parent_of(tile);
            self.tree.remove_recursively(tile);
            let Some(group) = group else { continue };
            let tiles = &self.tree.tiles;
            let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get(group) else { continue };
            let showing = tabs.children.iter().copied().find(|c| tiles.is_visible(*c));
            let lost_front = !tabs.active.is_some_and(|front| tabs.children.contains(&front));
            if tabs.children.is_empty() {
                self.tree.remove_recursively(group);
            } else if let Some(next) = showing {
                if let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(group)
                    && lost_front
                {
                    tabs.set_active(next);
                }
            } else {
                self.tree.tiles.set_visible(group, false);
            }
        }
    }

    /// The largest group showing with nothing in it but a placeholder (see `place_exactly`), if any.
    fn empty_group(&self) -> Option<TileId> {
        let rects = self.rects();
        let tiles = &self.tree.tiles;
        let empty = |group: &TileId| match tiles.get(*group) {
            Some(Tile::Container(Container::Tabs(tabs))) => (tabs.children.iter())
                .filter(|c| tiles.is_visible(**c))
                .all(|c| matches!(tiles.get(*c), Some(Tile::Pane(Pane::Empty)))),
            _ => false,
        };
        let area = |group: &TileId| rects.get(group).map_or(0.0, Rect::area);
        (self.groups_in(&rects).into_iter().filter(empty).rev()).max_by(|a, b| area(a).total_cmp(&area(b)))
    }

    /// The docked panes, in the order they're laid out (left to right, top to bottom, tab by tab), and whether each is
    /// shown.
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

    /// Tests find the tab group a docked pane's in, and whether it's the tab in front, with this.
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

/// A quick action keeps where a window is as this (see `Layout::spot`): the rectangle of its group, as fractions of
/// the width and height of the docked panes' area (left, top, right, bottom), so it's the same in a window of another
/// size; and whether its tab's in front.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Spot {
    pub rect: [f32; 4],
    pub front: bool,
}

/// A rectangle's extent across (left to right) or down (top to bottom).
fn span(rect: &Rect, across: bool) -> (f32, f32) {
    if across { (rect.left(), rect.right()) } else { (rect.top(), rect.bottom()) }
}

/// Placing panes exactly takes lines closer than `tolerance` to be one with this: each of `values` goes to the first
/// of those near it, or to `from` or `to` (the edges) if they're among them.
fn snap(values: &[f32], from: f32, to: f32, tolerance: f32) -> Vec<f32> {
    let values: Vec<f32> = values.iter().map(|v| v.clamp(from, to)).collect();
    let mut sorted: Vec<f32> = values.iter().copied().chain([from, to]).collect();
    sorted.sort_by(f32::total_cmp);
    // Each run of lines near the first of it, as (first, last).
    let mut runs: Vec<(f32, f32)> = Vec::new();
    for value in sorted {
        match runs.last_mut() {
            Some(run) if value - run.0 < tolerance => run.1 = value,
            _ => runs.push((value, value)),
        }
    }
    let line = |(first, last): (f32, f32)| {
        if first <= from {
            from
        } else if last >= to {
            to
        } else {
            first
        }
    };
    values.iter().map(|v| runs.iter().find(|r| *v >= r.0 && *v <= r.1).map_or(*v, |r| line(*r))).collect()
}

/// Placing panes exactly cuts the layout into this: groups (each the spot of one of the rectangles cut out, by index,
/// or room left over), in rows and columns.
enum Cut {
    Group(Option<usize>, Rect),
    Row(Vec<(f32, Cut)>),
    Column(Vec<(f32, Cut)>),
}

/// The cut of `region` with each of `rects` (those of them `inside` it) a group, along lines across the whole of
/// `region` that cross none of them (first left to right, failing that top to bottom, and so on within each part), if
/// there's one.
fn cut(region: Rect, rects: &[Rect], inside: Vec<usize>) -> Option<Cut> {
    match inside.as_slice() {
        [] => return Some(Cut::Group(None, region)),
        [only] if rects[*only] == region => return Some(Cut::Group(Some(*only), region)),
        _ => {}
    }
    for across in [true, false] {
        let (from, to) = span(&region, across);
        let mut lines: Vec<f32> = (inside.iter())
            .flat_map(|i| <[f32; 2]>::from(span(&rects[*i], across)))
            .filter(|line| *line > from && *line < to)
            .filter(|line| {
                inside.iter().all(|i| !(span(&rects[*i], across).0 < *line && span(&rects[*i], across).1 > *line))
            })
            .collect();
        lines.sort_by(f32::total_cmp);
        lines.dedup();
        if lines.is_empty() {
            continue;
        }
        let edges: Vec<f32> = [from].into_iter().chain(lines).chain([to]).collect();
        let mut parts = Vec::new();
        for pair in edges.windows(2) {
            let part = if across {
                Rect::from_min_max(pos2(pair[0], region.top()), pos2(pair[1], region.bottom()))
            } else {
                Rect::from_min_max(pos2(region.left(), pair[0]), pos2(region.right(), pair[1]))
            };
            let within: Vec<usize> = (inside.iter().copied())
                .filter(|i| span(&rects[*i], across).0 >= pair[0] && span(&rects[*i], across).1 <= pair[1])
                .collect();
            parts.push(((pair[1] - pair[0]) / (to - from), cut(part, rects, within)?));
        }
        return Some(if across { Cut::Row(parts) } else { Cut::Column(parts) });
    }
    None
}

impl Cut {
    /// Whether each group (in order) is room left over, not a spot.
    fn left_over(&self) -> Vec<bool> {
        match self {
            Cut::Group(spot, _) => vec![spot.is_none()],
            Cut::Row(parts) | Cut::Column(parts) => parts.iter().flat_map(|(_, part)| part.left_over()).collect(),
        }
    }

    /// The groups' rectangles, in order.
    fn groups(&self) -> Vec<Rect> {
        match self {
            Cut::Group(_, rect) => vec![*rect],
            Cut::Row(parts) | Cut::Column(parts) => parts.iter().flat_map(|(_, part)| part.groups()).collect(),
        }
    }

    /// The cut, put in `tiles`, returning its top tile: each group with the windows placed in its spot (of `spots`),
    /// and the panes `adopted` into it (by group, in order, from `next`, each with whether it's showing), the first in
    /// front of the windows placed there that should be, or of the panes in `fronts`, or failing that the first
    /// showing. A group with nothing showing has a placeholder (`Pane::Empty`).
    fn build(
        &self,
        tiles: &mut Tiles<Pane>,
        spots: &[(Rect, Vec<(Pane, bool)>)],
        adopted: &[Vec<(Pane, bool)>],
        fronts: &[Pane],
        next: &mut usize,
    ) -> TileId {
        match self {
            Cut::Group(spot, _) => {
                let placed = spot.map_or(&[][..], |i| &spots[i].1[..]);
                let others = &adopted[*next];
                *next += 1;
                let mut children = Vec::new();
                let mut front = None;
                for (pane, in_front) in placed {
                    let tile = tiles.insert_pane(*pane);
                    if *in_front && front.is_none() {
                        front = Some(tile);
                    }
                    children.push(tile);
                }
                let mut showing = !placed.is_empty();
                let mut first_other = None;
                for (pane, shown) in others {
                    let tile = tiles.insert_pane(*pane);
                    if *shown {
                        showing = true;
                        first_other = first_other.or(Some(tile));
                    } else {
                        tiles.set_visible(tile, false);
                    }
                    if fronts.contains(pane) && front.is_none() {
                        front = Some(tile);
                    }
                    children.push(tile);
                }
                if !showing {
                    children.push(tiles.insert_pane(Pane::Empty));
                }
                let front = front.or(children.first().copied().filter(|_| !placed.is_empty())).or(first_other);
                let mut group = egui_tiles::Tabs::new(children);
                if let Some(front) = front {
                    group.set_active(front);
                }
                tiles.insert_container(group)
            }
            Cut::Row(parts) | Cut::Column(parts) => {
                let dir = if matches!(self, Cut::Row(_)) { LinearDir::Horizontal } else { LinearDir::Vertical };
                let children: Vec<(TileId, f32)> = parts
                    .iter()
                    .map(|(share, part)| (part.build(tiles, spots, adopted, fronts, next), *share))
                    .collect();
                let mut linear = Linear::new(dir, children.iter().map(|c| c.0).collect());
                for (child, share) in children {
                    linear.shares.set_share(child, share);
                }
                tiles.insert_container(linear)
            }
        }
    }
}

/// Hiding a tile brings another tab to the front in its place.
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

    fn spot(rect: [f32; 4], front: bool) -> Spot {
        Spot { rect, front }
    }

    fn at(layout: &Layout, n: u64) -> [f32; 4] {
        let rect = layout.spot(node(n)).unwrap().rect;
        rect.map(|f| (f * 1000.0).round() / 1000.0)
    }

    #[test]
    fn windows_placed_exactly_go_where_they_were_and_what_was_there_where_it_overlaps_most() {
        let third = 1.0 / 3.0;
        let (left, top, bottom) =
            ([0.0, 0.0, third, 1.0], [third, 0.0, 1.0, 2.0 * third], [third, 2.0 * third, 1.0, 1.0]);
        let mut layout = Layout::default();
        let wanted = [
            (node(1), spot(top, true)),
            (node(2), spot(bottom, true)),
            (node(3), spot(bottom, false)),
            (node(4), spot(left, true)),
        ];
        assert!(layout.place_exactly(&wanted).is_empty());
        layout.close_empty();
        let round = |rect: [f32; 4]| rect.map(|f| (f * 1000.0).round() / 1000.0);
        assert_eq!([at(&layout, 1), at(&layout, 2), at(&layout, 4)], [top, bottom, left].map(round));
        assert_eq!(layout.group_of(node(3)), layout.group_of(node(2)), "the same spot, a tab of its group");
        assert_eq!(layout.tab_group(node(3)).map(|g| g.1), Some(false), "behind");
        // Home, which filled the layout, goes behind the window in the group it overlaps most: the top right.
        assert_eq!(layout.tab_group(Pane::Home), Some((layout.group_of(node(1)).unwrap(), false)));
    }

    #[test]
    fn room_left_over_is_filled_by_windows_opened_after_and_what_is_still_empty_is_closed() {
        let mut layout = Layout::default();
        // A window in the middle, and one overlapping it, which isn't placed.
        let wanted = [(node(1), spot([0.25, 0.25, 0.75, 0.75], true)), (node(2), spot([0.5, 0.5, 1.0, 1.0], true))];
        assert_eq!(layout.place_exactly(&wanted), vec![node(2)]);
        assert_eq!(at(&layout, 1), [0.25, 0.25, 0.75, 0.75]);
        // Home goes in the first of the groups it overlaps as much (the room on the left), so the largest left empty
        // is on the right, and the next window opened goes there.
        layout.open(NodeId(3), None);
        assert_eq!(at(&layout, 3), [0.75, 0.0, 1.0, 1.0]);
        assert!(layout.spot(Pane::Home).is_some_and(|s| s.rect[2] <= 0.25 + 1e-3));
        // The room above and below the first is closed, so it has the whole height.
        layout.close_empty();
        assert!(layout.panes_in_order().iter().all(|(pane, _)| *pane != Pane::Empty));
        assert_eq!(at(&layout, 1), [0.25, 0.0, 0.75, 1.0]);
    }

    #[test]
    fn lines_close_together_are_one_so_there_are_no_slivers() {
        let mut layout = Layout::default();
        let wanted = [(node(1), spot([0.0, 0.0, 0.34, 1.0], true)), (node(2), spot([0.33, 0.02, 1.0, 1.0], true))];
        assert!(layout.place_exactly(&wanted).is_empty(), "overlapping only a little, so not at all");
        layout.close_empty();
        assert_eq!(at(&layout, 1)[2], at(&layout, 2)[0]);
        assert_eq!(at(&layout, 2)[1], 0.0, "to the top");
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
