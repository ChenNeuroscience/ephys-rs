//! The dockable layout, like an IDE's: the canvas, inspector and node windows are panes that can be
//! dragged by their tabs to sit beside, above or below each other, or together as tabs.
//!
//! A node's window opens docked: alongside the largest group of node windows, or failing that in a new panel
//! below the canvas. It can be undocked to float, and docked again. Closing a docked window hides it but keeps
//! its place in the layout, so it comes back where it was when reopened.

use std::collections::{BTreeSet, HashSet};

use eframe::egui;
use egui_tiles::{Container, Linear, LinearDir, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};
use serde::{Deserialize, Serialize};

use super::canvas::{Canvas, Selection};
use super::dev_tools::{self, DevTools};
use super::icons::Icon;
use super::style;
use super::{Event, inspector, toolbar};
use crate::core::annotation::{AnnotationSet, Annotations};
use crate::core::engine::{Graph, NodeId, NodeSet, Resolved, Runner, Snapshot};
use crate::core::theme::Colours;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Pane {
    Canvas,
    Inspector,
    DevTools,
    /// A node's window: its view, or its settings.
    Node(NodeId),
}

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
}

impl Default for Layout {
    fn default() -> Self {
        let mut tiles = Tiles::default();
        let columns: Vec<TileId> = [Pane::Canvas, Pane::Inspector]
            .into_iter()
            .map(|pane| {
                let pane = tiles.insert_pane(pane);
                tiles.insert_tab_tile(vec![pane])
            })
            .collect();
        let mut row = Linear::new(LinearDir::Horizontal, columns.clone());
        for (column, share) in columns.iter().zip([4.0, 1.0]) {
            row.shares.set_share(*column, share);
        }
        let root = tiles.insert_container(row);
        Layout { tree: Tree::new(egui::Id::unique("dock"), root, tiles), floating: BTreeSet::new() }
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

    pub fn is_shown(&self, pane: Pane) -> bool {
        self.tile(pane).is_some_and(|t| self.tree.tiles.is_visible(t))
    }

    /// Shows or hides a pane: the first time, a pane not in the default layout (the dev tools) goes below the
    /// canvas, where browsers put theirs.
    pub fn toggle_pane(&mut self, pane: Pane) {
        if self.tile(pane).is_none() {
            let tile = self.tree.tiles.insert_pane(pane);
            self.below_canvas(tile, 0.5);
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

    /// Puts a node's window in the layout: back where it was if it's been docked before, otherwise as a tab
    /// in the largest group of node windows showing, or failing that in a new panel below the canvas.
    pub fn dock(&mut self, id: NodeId) {
        self.floating.remove(&id);
        let pane = Pane::Node(id);
        if self.tile(pane).is_none() {
            let target = self.largest_window_tabs();
            let tiles = &mut self.tree.tiles;
            let new = tiles.insert_pane(pane);
            match target.and_then(|tabs| tiles.get_mut(tabs)) {
                Some(Tile::Container(container)) => container.add_child(new),
                _ => self.below_canvas(new, 0.65),
            }
        }
        self.show(pane);
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

    /// Puts `pane` below the canvas, which keeps `share` of the height.
    fn below_canvas(&mut self, pane: TileId, share: f32) {
        let tiles = &mut self.tree.tiles;
        let Some(canvas) = tiles.find_pane(&Pane::Canvas) else { return };
        let above = tiles.parent_of(canvas).unwrap_or(canvas);
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
    /// Whether settings are locked (while recording).
    pub locked: bool,
    pub dev_tools: &'a mut DevTools,
    pub canvas: &'a mut Canvas,
    pub selection: &'a mut Selection,
    pub open_windows: &'a HashSet<NodeId>,
    pub events: &'a mut Vec<Event>,
}

/// A node's window content: its view if it has one, otherwise its settings.
/// While running, it gets the latest live data the node's processing has shown.
pub fn node_ui<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &mut Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    locked: bool,
    id: NodeId,
) {
    let Some(state) = resolved.get(&id) else { return };
    let has_view = graph.get(id).is_some_and(|e| e.node.has_view());
    // A view stays live while recording (changes it makes to settings are undone: see `App::lock`).
    if locked && !has_view {
        ui.disable();
    }
    let running = runner.is_some();
    let mut show = |graph: &mut Graph<N>, display: Option<&(dyn std::any::Any + Send)>| {
        graph.edit(id, &state.input_streams, colours, display, running, |node: &mut N, context| {
            if has_view { node.view_ui(ui, context) } else { node.settings_ui(ui, context) }
        });
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
    let dock_rect =
        egui::Rect::from_center_size(egui::pos2(bar.right() - 14.0, bar.center().y), egui::vec2(20.0, 20.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(dock_rect), |ui| {
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

fn padded(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().inner_margin(8).show(ui, add);
}

impl<N: NodeSet, A: AnnotationSet> egui_tiles::Behavior<Pane> for Panes<'_, N, A> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile: TileId, pane: &mut Pane) -> UiResponse {
        match *pane {
            Pane::Canvas => self.canvas.show(
                ui,
                self.graph,
                self.annotations,
                self.resolved,
                self.selection,
                self.open_windows,
                self.events,
            ),
            Pane::Inspector => padded(ui, |ui| {
                inspector::show(
                    ui,
                    self.graph,
                    self.annotations,
                    self.resolved,
                    self.colours,
                    self.selection,
                    self.open_windows,
                    self.locked,
                    self.events,
                )
            }),
            Pane::DevTools => {
                dev_tools::show(ui, self.dev_tools, self.graph, self.resolved, self.colours, self.runner, self.snapshot)
            }
            Pane::Node(id) => {
                padded(ui, |ui| node_ui(ui, self.graph, self.resolved, self.colours, self.runner, self.locked, id))
            }
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Canvas => "Graph".into(),
            Pane::Inspector => "Inspector".into(),
            Pane::DevTools => "Developer Tools".into(),
            Pane::Node(id) => match self.graph.get(*id) {
                Some(entry) => tab_title(&entry.label, style::category_colour(entry.node.info().category)),
                None => "?".into(),
            },
        }
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile: TileId) -> bool {
        !matches!(tiles.get_pane(&tile), Some(Pane::Canvas))
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
