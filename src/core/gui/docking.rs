//! The dockable layout, like an IDE's: the home pane, the dev tools and node windows are panes that can be
//! dragged by their tabs to sit beside, above or below each other, or together as tabs. The home pane is in one of
//! two modes: the basic view (the viewers, and quick actions), or the graph.
//!
//! A node's window opens docked. In the graph mode, it goes alongside the largest group of node windows, or failing
//! that in a new panel below the graph. In the basic mode, it goes beside the basic view, the windows sharing the
//! space beside it (a few, one above another, then as tabs). It can be undocked to float, and docked again. Closing a docked window hides it but keeps
//! its place in the layout, so it comes back where it was when reopened.

use std::collections::{BTreeSet, HashSet};

use eframe::egui;
use egui_tiles::{Container, Linear, LinearDir, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};
use serde::{Deserialize, Serialize};

use super::canvas::{Canvas, Selection};
use super::dev_tools::{self, DevTools};
use super::frame_times::FrameTimes;
use super::icons::Icon;
use super::node_tools::NodeTools;
use super::style;
use super::{Event, basic, node_window, toolbar};
use crate::core::annotation::{AnnotationSet, Annotations};
use crate::core::engine::{Graph, NodeId, NodeSet, Resolved, Runner, Snapshot};
use crate::core::quick_action::QuickAction;
use crate::core::theme::Colours;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Pane {
    /// Home: the basic view, or the graph (see `Mode`).
    #[serde(alias = "Main", alias = "Canvas")]
    Home,
    DevTools,
    /// The selected annotation's settings: there while an annotation's selected.
    Annotation,
    /// A node's window: its view, or its settings.
    Node(NodeId),
}

/// What the home pane shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Mode {
    /// The viewers, large, and quick actions: for most people, most of the time.
    #[default]
    Basic,
    /// The graph, to build and change it node by node.
    Graph,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Basic => "Basic",
            Mode::Graph => "Graph",
        }
    }
}

/// The narrowest and shortest a pane can be dragged to.
const MIN_PANE: f32 = 300.0;

/// How wide the basic view is left, beside the first window opened from it.
const BASIC_WIDTH: f32 = 360.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowState {
    Closed,
    Floating,
    Docked,
    /// Docked, but closed: it keeps its place for when it's next opened.
    Hidden,
}

#[derive(Serialize, Deserialize)]
pub struct Layout {
    tree: Tree<Pane>,
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

    /// Opens a node's window (where it was last, if it's been open before), or closes it.
    pub fn toggle(&mut self, id: NodeId) {
        match self.window(id) {
            WindowState::Closed => self.dock(id),
            WindowState::Floating => {
                self.floating.remove(&id);
            }
            WindowState::Docked => self.hide(Pane::Node(id)),
            WindowState::Hidden => self.show(Pane::Node(id)),
        }
    }

    /// Opens a node's window (where it was last, if it's been open before), or brings it to the front.
    pub fn open(&mut self, id: NodeId) {
        match self.window(id) {
            WindowState::Floating => {}
            WindowState::Docked | WindowState::Hidden => self.show(Pane::Node(id)),
            WindowState::Closed => self.dock(id),
        }
    }

    pub fn is_shown(&self, pane: Pane) -> bool {
        self.tile(pane).is_some_and(|t| self.tree.tiles.is_visible(t))
    }

    /// Shows or hides a pane: the first time, a pane not in the default layout (the dev tools) goes below the
    /// home pane, where browsers put theirs.
    pub fn toggle_pane(&mut self, pane: Pane) {
        if self.tile(pane).is_none() {
            let tile = self.tree.tiles.insert_pane(pane);
            self.below_home(tile, 0.5);
            return self.show(pane);
        }
        if self.is_shown(pane) { self.hide(pane) } else { self.show(pane) }
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

    /// Puts a node's window in the layout: back where it was if it's been docked before; otherwise, in the graph
    /// mode, as a tab in the largest group of node windows showing, or failing that in a new panel below the graph;
    /// in the basic mode, beside the basic view (see `beside_home`).
    pub fn dock(&mut self, id: NodeId) {
        self.floating.remove(&id);
        let pane = Pane::Node(id);
        if self.tile(pane).is_none() {
            let target = self.largest_window_tabs();
            let new = self.tree.tiles.insert_pane(pane);
            if self.mode == Mode::Basic {
                self.beside_home(new);
            } else {
                match target.and_then(|tabs| self.tree.tiles.get_mut(tabs)) {
                    Some(Tile::Container(container)) => container.add_child(new),
                    _ => self.below_home(new, 0.65),
                }
            }
        }
        self.show(pane);
    }

    /// Puts a new window beside the basic view, so the windows share the space at its right: the first in a
    /// panel there (the basic view narrowing to `BASIC_WIDTH`); the second below it; the third in a column of its own
    /// beside them; and after that, as a tab in the biggest group.
    fn beside_home(&mut self, pane: TileId) {
        let mut groups = self.window_tab_groups();
        // The same one every time, when two are as big.
        groups.sort_by_key(|tile| std::cmp::Reverse(tile.0));
        let biggest = groups.iter().copied().max_by(|a, b| self.area(*a).total_cmp(&self.area(*b)));
        let tiles = &mut self.tree.tiles;
        let Some(main) = tiles.find_pane(&Pane::Home) else { return };
        let left = tiles.parent_of(main).unwrap_or(main);
        let new = tiles.insert_tab_tile(vec![pane]);
        // Puts `container` where `old` was, in `parent`: looked up before `container` took `old` in (which then
        // becomes `old`'s parent).
        let replace = |tree: &mut Tree<Pane>, parent: Option<TileId>, old: TileId, container: TileId| match parent
            .and_then(|p| tree.tiles.get_mut(p))
        {
            Some(Tile::Container(parent)) => {
                let _ = parent.replace_child(old, container);
            }
            _ => tree.root = Some(container),
        };
        match (groups.len(), biggest) {
            (0, _) | (_, None) => {
                let width = tiles.rect(left).map_or(1400.0, |r| r.width());
                let share = (BASIC_WIDTH / width).clamp(0.15, 0.5);
                let parent = tiles.parent_of(left);
                let row = tiles.insert_container(Linear::new_binary(LinearDir::Horizontal, [left, new], share));
                replace(&mut self.tree, parent, left, row);
            }
            (1, Some(group)) => {
                let parent = tiles.parent_of(group);
                let column = tiles.insert_container(Linear::new_binary(LinearDir::Vertical, [group, new], 0.5));
                replace(&mut self.tree, parent, group, column);
            }
            (2, Some(group)) => {
                // A column of its own, beside the basic view and the others, sharing their width.
                let row = tiles.parent_of(left);
                match row.and_then(|r| tiles.get_mut(r)) {
                    Some(Tile::Container(Container::Linear(row))) if row.dir == LinearDir::Horizontal => {
                        // Halving the other windows' share, so the basic view keeps its width.
                        if let Some(others) = row.children.iter().copied().find(|c| *c != left) {
                            let share = row.shares[others] / 2.0;
                            row.shares.set_share(others, share);
                            row.shares.set_share(new, share);
                        }
                        row.children.push(new);
                    }
                    _ => {
                        let parent = tiles.parent_of(group);
                        let column = tiles.insert_container(Linear::new_binary(LinearDir::Vertical, [group, new], 0.5));
                        replace(&mut self.tree, parent, group, column);
                    }
                }
            }
            (_, Some(group)) => {
                tiles.remove(new);
                if let Some(Tile::Container(container)) = tiles.get_mut(group) {
                    container.add_child(pane);
                }
            }
        }
    }

    /// The groups of tabs showing that hold node windows (and not the home pane).
    fn window_tab_groups(&self) -> Vec<TileId> {
        let tiles = &self.tree.tiles;
        tiles
            .iter()
            .filter(|(tile, t)| match t {
                Tile::Container(Container::Tabs(tabs)) => {
                    let panes = || tabs.children.iter().filter_map(|c| tiles.get_pane(c));
                    tiles.is_visible(**tile)
                        && panes().any(|p| matches!(p, Pane::Node(_)))
                        && !panes().any(|p| *p == Pane::Home)
                }
                _ => false,
            })
            .map(|(tile, _)| *tile)
            .collect()
    }

    /// A tile's area, as last laid out.
    fn area(&self, tile: TileId) -> f32 {
        self.tree.tiles.rect(tile).map_or(0.0, |r| r.area())
    }

    /// The biggest (as last laid out) group of tabs that holds node windows and is showing.
    fn largest_window_tabs(&self) -> Option<TileId> {
        let tiles = &self.tree.tiles;
        let holds_windows =
            |tabs: &egui_tiles::Tabs| tabs.children.iter().any(|c| matches!(tiles.get_pane(c), Some(Pane::Node(_))));
        let area = |tile: TileId| tiles.rect(tile).map_or(0.0, |r| r.area());
        tiles
            .iter()
            .filter(|(tile, t)| {
                matches!(t, Tile::Container(Container::Tabs(tabs)) if holds_windows(tabs))
                    && tiles.is_visible_in_layout(**tile)
            })
            .map(|(tile, _)| *tile)
            .max_by(|a, b| area(*a).total_cmp(&area(*b)))
    }

    /// Puts `pane` below the home pane, which keeps `share` of the height.
    fn below_home(&mut self, pane: TileId, share: f32) {
        let tiles = &mut self.tree.tiles;
        let Some(main) = tiles.find_pane(&Pane::Home) else { return };
        let above = tiles.parent_of(main).unwrap_or(main);
        let parent = tiles.parent_of(above);
        let below = tiles.insert_tab_tile(vec![pane]);
        let column = tiles.insert_container(Linear::new_binary(LinearDir::Vertical, [above, below], share));
        match parent.and_then(|p| tiles.get_mut(p)) {
            Some(Tile::Container(container)) => {
                let _ = container.replace_child(above, column);
            }
            _ => self.tree.root = Some(column),
        }
    }

    /// Shows or hides the annotation pane: the first time, beside home (where the graph is), where it then stays.
    pub fn show_annotation_pane(&mut self, show: bool) {
        if self.is_shown(Pane::Annotation) == show {
            return;
        }
        if !show {
            return self.hide(Pane::Annotation);
        }
        if self.tile(Pane::Annotation).is_none() {
            // Beside home (where the graph is), in a row with it.
            let tiles = &mut self.tree.tiles;
            let pane = tiles.insert_pane(Pane::Annotation);
            let right = tiles.insert_tab_tile(vec![pane]);
            match tiles.find_pane(&Pane::Home) {
                Some(home) => {
                    let left = tiles.parent_of(home).unwrap_or(home);
                    let parent = tiles.parent_of(left);
                    let row = tiles.insert_container(Linear::new_binary(LinearDir::Horizontal, [left, right], 0.75));
                    match parent.and_then(|p| tiles.get_mut(p)) {
                        Some(Tile::Container(container)) => {
                            let _ = container.replace_child(left, row);
                        }
                        _ => self.tree.root = Some(row),
                    }
                }
                None => match self.tree.root {
                    Some(root) => {
                        let row = Linear::new_binary(LinearDir::Horizontal, [root, right], 0.78);
                        self.tree.root = Some(self.tree.tiles.insert_container(row));
                    }
                    None => self.tree.root = Some(right),
                },
            }
        }
        self.show(Pane::Annotation);
    }

    /// Takes a docked window out of the layout, to float.
    pub fn float(&mut self, id: NodeId) {
        if let Some(tile) = self.tile(Pane::Node(id)) {
            self.tree.remove_recursively(tile);
        }
        self.floating.insert(id);
    }

    /// Forgets the windows of nodes that no longer exist.
    pub fn retain_nodes<N: NodeSet>(&mut self, graph: &Graph<N>) {
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

    pub fn ui<N: NodeSet, A: AnnotationSet>(&mut self, ui: &mut egui::Ui, panes: &mut Panes<N, A>) {
        self.tree.ui(panes, ui);
    }
}

fn hide_tile(tiles: &mut Tiles<Pane>, tile: TileId) {
    tiles.set_visible(tile, false);
    // Bring another tab to the front in its place.
    let Some(parent) = tiles.parent_of(tile) else { return };
    let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get(parent) else { return };
    let next = tabs.children.iter().copied().find(|c| *c != tile && tiles.is_visible(*c));
    if let (Some(next), Some(Tile::Container(Container::Tabs(tabs)))) = (next, tiles.get_mut(parent)) {
        tabs.set_active(next);
    }
}

/// What the panes need to draw themselves, for one frame.
pub struct Panes<'a, N: NodeSet, A: AnnotationSet> {
    pub graph: &'a mut Graph<N>,
    pub annotations: &'a mut Annotations<A>,
    pub resolved: &'a Resolved<N::Edges>,
    pub colours: &'static Colours,
    pub runner: Option<&'a Runner<N>>,
    /// The latest snapshot of the graph.
    pub snapshot: &'a Snapshot<N>,
    /// What the home pane shows, and the quick actions the basic view offers.
    pub mode: Mode,
    /// What the graph's toolbar can do.
    pub graph_tools: toolbar::GraphTools,
    pub quick_actions: &'a [QuickAction<N>],
    /// How many workers runs start with.
    pub workers: usize,
    /// Whether settings are locked (while recording).
    pub locked: bool,
    pub dev_tools: &'a mut DevTools,
    pub canvas: &'a mut Canvas,
    pub selection: &'a mut Selection,
    pub open_windows: &'a HashSet<NodeId>,
    pub events: &'a mut Vec<Event>,
    /// For the dev tools' chart; and each pane's time this frame, as it's drawn.
    pub frame_times: &'a FrameTimes,
    pub timings: &'a mut Vec<(String, f32)>,
}

/// A node's window content: its own view if it has one (which shows its settings as it likes), otherwise the
/// default window (`node_window`). While running, it gets the latest its passes have left for its window.
#[allow(clippy::too_many_arguments)]
pub fn node_ui<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &mut Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    locked: bool,
    id: NodeId,
    events: &mut Vec<Event>,
) {
    let Some(state) = resolved.get(&id) else { return };
    let has_view = graph.get(id).is_some_and(|e| e.node.has_view());
    let running = runner.is_some();
    // A view stays live while recording (changes it makes to settings are undone: see `App::lock`).
    let mut show = |graph: &mut Graph<N>, display: Option<&(dyn std::any::Any + Send)>| {
        if has_view {
            let Some(entry) = graph.get(id) else { return };
            let mut tools =
                NodeTools { id, in_basic: entry.in_basic(), enabled: !entry.disabled, events: &mut *events };
            let tools = Some(&mut tools as &mut dyn crate::core::engine::NodeToolbar);
            graph.edit(id, &state.input_streams, colours, display, running, locked, tools, |node: &mut N, context| {
                node.view_ui(ui, context)
            });
        } else {
            node_window::show(ui, graph, resolved, colours, display, running, locked, id, events);
        }
    };
    let shown = runner.and_then(|runner| runner.exchange().with_latest(id, |display| show(graph, Some(display))));
    if shown.is_none() {
        show(graph, None);
    }
}

/// What was clicked in a floating window's header.
#[derive(Default)]
pub struct HeaderClicks {
    pub close: bool,
    pub dock: bool,
}

/// The top of a floating window, drawn as egui_tiles draws a docked window's tab bar, holding just the one
/// (active) tab, so floating and docked windows look alike. Dragging it moves the window.
pub fn floating_header(ui: &mut egui::Ui, title: &str, colour: egui::Color32) -> HeaderClicks {
    const HEIGHT: f32 = 24.0;
    const MARGIN: f32 = 8.0;
    const CLOSE: f32 = 12.0;
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), HEIGHT), egui::Sense::hover());
    let bar_colour = if visuals.dark_mode {
        visuals.extreme_bg_color
    } else {
        (egui::Rgba::from(visuals.panel_fill) * egui::Rgba::from_gray(0.8)).into()
    };
    painter.rect_filled(bar, 0.0, bar_colour);
    painter.hline(bar.x_range(), bar.bottom(), egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color));

    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = tab_title(title, colour).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, font);
    let width = galley.size().x + 2.0 * MARGIN + 4.0 + CLOSE;
    let tab = egui::Rect::from_min_size(bar.min, egui::vec2(width.min(bar.width() - 30.0), HEIGHT));
    painter.rect(
        tab.shrink(0.5),
        0.0,
        visuals.panel_fill,
        egui::Stroke::new(1.0, visuals.widgets.active.bg_fill),
        egui::StrokeKind::Inside,
    );
    // Joins the tab to the content below, as a docked active tab does.
    painter.hline(tab.x_range(), tab.bottom(), egui::Stroke::new(2.0, visuals.panel_fill));
    let text = egui::Align2::LEFT_CENTER.align_size_within_rect(galley.size(), tab.shrink(MARGIN)).min;
    painter.with_clip_rect(tab).galley(text, galley, visuals.widgets.active.text_color());

    let mut clicks = HeaderClicks::default();
    let close_rect = egui::Align2::RIGHT_CENTER.align_size_within_rect(egui::Vec2::splat(CLOSE), tab.shrink(MARGIN));
    let close = ui.interact(close_rect, ui.auto_id_with("close"), egui::Sense::click());
    close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close"));
    let stroke = ui.style().interact(&close).fg_stroke;
    let cross = close_rect.shrink(2.0).expand(ui.style().interact(&close).expansion);
    painter.line_segment([cross.left_top(), cross.right_bottom()], stroke);
    painter.line_segment([cross.right_top(), cross.left_bottom()], stroke);
    clicks.close = close.clicked();

    // Where a docked tab bar has its undock button.
    let buttons = egui::Rect::from_min_max(
        egui::pos2(bar.right() - 72.0, bar.top()),
        egui::pos2(bar.right() - 4.0, bar.bottom()),
    );
    let layout = egui::Layout::right_to_left(egui::Align::Center);
    ui.scope_builder(egui::UiBuilder::new().max_rect(buttons).layout(layout), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        clicks.dock = toolbar::small_icon_button(ui, Icon::Dock, "Dock", "Dock into the layout").clicked();
    });
    clicks
}

/// A node window's tab title: a block of the node's colour on the graph, then its label.
pub fn tab_title(label: &str, colour: egui::Color32) -> egui::WidgetText {
    let font = egui::FontId::proportional(14.0);
    let mut job = egui::text::LayoutJob::default();
    // A space with the colour behind it, in the title's font so it's as tall as the title and level with it.
    let block =
        egui::TextFormat { font_id: font.clone(), background: colour, extra_letter_spacing: 1.5, ..Default::default() };
    job.append(" ", 0.0, block);
    // The placeholder colour takes the tab's text colour (lighter when it's not the active tab).
    job.append(label, 6.0, egui::TextFormat { font_id: font, color: egui::Color32::PLACEHOLDER, ..Default::default() });
    egui::WidgetText::LayoutJob(std::sync::Arc::new(job))
}

/// The selected annotation's settings; for several, how many, to delete together.
fn annotation_ui<A: AnnotationSet>(
    ui: &mut egui::Ui,
    annotations: &mut Annotations<A>,
    selection: &Selection,
    events: &mut Vec<Event>,
) {
    let count = selection.annotations.len();
    match selection.annotations.first() {
        None => {}
        Some(id) if count == 1 => {
            let Some(annotation) = annotations.get_mut(*id) else { return };
            ui.horizontal(|ui| {
                ui.heading(annotation.kind.info().name);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A mini toolbar: delete, and the annotation's own buttons.
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let delete = toolbar::small_icon_button(ui, Icon::Delete, "Delete", "Delete this annotation");
                    if delete.clicked() {
                        events.push(Event::DeleteAnnotation(*id));
                    }
                    annotation.kind.toolbar_ui(ui);
                });
            });
            ui.add_space(4.0);
            // Its settings have all the room there is (a markdown note's text fills it).
            annotation.kind.settings_ui(ui);
        }
        Some(_) => {
            ui.heading(format!("{count} annotations"));
            ui.weak("Select one to change its settings.");
        }
    }
}

fn padded(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().inner_margin(8).show(ui, add);
}

impl<N: NodeSet, A: AnnotationSet> Panes<'_, N, A> {
    fn pane_contents(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Home if self.mode == Mode::Basic => basic::show(
                ui,
                self.graph,
                self.resolved,
                self.colours,
                self.quick_actions,
                self.open_windows,
                self.locked,
                self.events,
            ),
            Pane::Home => {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| toolbar::graph_toolbar::<A>(ui, self.graph_tools, self.canvas.tool, self.events));
                ui.separator();
                self.canvas.show(
                    ui,
                    self.graph,
                    self.annotations,
                    self.resolved,
                    self.selection,
                    self.open_windows,
                    self.events,
                )
            }
            Pane::Annotation => padded(ui, |ui| annotation_ui(ui, self.annotations, self.selection, self.events)),
            Pane::DevTools => dev_tools::show(
                ui,
                self.dev_tools,
                self.graph,
                self.resolved,
                self.colours,
                self.runner,
                self.snapshot,
                self.workers,
                self.frame_times,
            ),
            Pane::Node(id) => {
                let (graph, resolved, colours, runner, locked) =
                    (&mut *self.graph, self.resolved, self.colours, self.runner, self.locked);
                padded(ui, |ui| node_ui(ui, graph, resolved, colours, runner, locked, id, self.events))
            }
        }
    }
}

impl<N: NodeSet, A: AnnotationSet> egui_tiles::Behavior<Pane> for Panes<'_, N, A> {
    /// Where a dragged window would go: in the accent (egui_tiles uses the selection's stroke, which is the colour
    /// on the accent, so hard to see on a light theme).
    fn drag_preview_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(2.0, self.colours.accent)
    }

    fn drag_preview_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        self.colours.accent.gamma_multiply(0.35)
    }

    /// No pane (or group of them) is dragged narrower or shorter than this.
    fn min_size(&self) -> f32 {
        MIN_PANE
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile: TileId, pane: &mut Pane) -> UiResponse {
        let started = std::time::Instant::now();
        self.pane_contents(ui, *pane);
        let name = match *pane {
            Pane::Home => format!("Home ({})", self.mode.name()),
            Pane::DevTools => "Developer Tools".into(),
            Pane::Annotation => "Annotation".into(),
            Pane::Node(id) => self.graph.get(id).map_or("?".into(), |e| e.label.clone()),
        };
        self.timings.push((name, started.elapsed().as_secs_f32() * 1000.0));
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Home => "Home".into(),
            Pane::DevTools => "Developer Tools".into(),
            Pane::Annotation => "Annotation".into(),
            Pane::Node(id) => match self.graph.get(*id) {
                Some(entry) => tab_title(&entry.label, style::category_colour(entry.node.info().category)),
                None => "?".into(),
            },
        }
    }

    /// The annotation pane comes and goes with the selection, so isn't closed by hand. (The home pane can be: the
    /// toolbar's Basic | Graph brings it back.)
    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile: TileId) -> bool {
        !matches!(tiles.get_pane(&tile), Some(Pane::Annotation))
    }

    /// Hides rather than removes, so the pane comes back in the same place.
    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile: TileId) -> bool {
        hide_tile(tiles, tile);
        false
    }

    fn top_bar_right_ui(
        &mut self,
        tiles: &Tiles<Pane>,
        ui: &mut egui::Ui,
        _tile: TileId,
        tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        let active = tabs.active.or(tabs.children.first().copied());
        if let Some(Pane::Node(id)) = active.and_then(|t| tiles.get_pane(&t)) {
            ui.add_space(2.0);
            if toolbar::small_icon_button(ui, Icon::Undock, "Undock", "Undock into a floating window").clicked() {
                self.events.push(Event::FloatWindow(*id));
            }
        }
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions { all_panes_must_have_tabs: true, ..Default::default() }
    }
}
