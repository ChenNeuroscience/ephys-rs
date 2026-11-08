//! Drawing the layout of panes (see `crate::core::layout`, which says where they go): each pane's contents, its tab,
//! and what dragging and closing tabs does.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;
use egui_tiles::{SimplificationOptions, TileId, Tiles, UiResponse};

use super::canvas::{Canvas, Selection};
use super::dev_tools::{self, DevTools};
use super::frame_times::FrameTimes;
use super::icons::Icon;
use super::node_tools::NodeTools;
use super::style;
use super::{Event, basic, node_window, toolbar};
use crate::core::annotation::{Annotations, AnnotationsEnum};
use crate::core::engine::{Graph, NodeId, NodesEnum, Resolved, Runner, Snapshot};
use crate::core::layout::{Layout, Mode, Pane, hide_tile};
use crate::core::quick_action::QuickAction;
use crate::core::theme::Colours;

/// The narrowest and shortest a pane can be dragged to.
const MIN_PANE: f32 = 300.0;

impl Layout {
    pub fn ui<N: NodesEnum, A: AnnotationsEnum>(&mut self, ui: &mut egui::Ui, panes: &mut Panes<N, A>) {
        self.tree.ui(panes, ui);
    }
}

/// What the panes need to draw themselves, for one frame.
pub struct Panes<'a, N: NodesEnum, A: AnnotationsEnum> {
    /// The annotation types hidden in Settings: not among the drawing tools.
    pub hidden_annotations: &'a std::collections::BTreeSet<String>,
    pub graph: &'a mut Graph<N>,
    pub annotations: &'a mut Annotations<A>,
    pub resolved: &'a Resolved<N::Edges>,
    pub colours: &'static Colours,
    pub runner: Option<&'a Runner<N>>,
    /// What the running nodes' passes share with the GUI, or what the last run's shared, once it's stopped.
    pub shared: Option<&'a crate::core::engine::SharedByNode>,
    /// Each node's state on the GUI's side (see `Node::UiState`).
    pub(crate) ui_states: &'a super::UiStates,
    /// The base directory recordings go in (see `crate::core::base_directory`).
    pub base_directory: Option<&'a crate::core::base_directory::Place>,
    /// The latest snapshot of the graph.
    pub snapshot: &'a Snapshot<N>,
    /// What the home pane shows, and the quick actions the basic view offers.
    pub mode: Mode,
    /// What the graph's toolbar can do.
    pub graph_tools: toolbar::GraphTools,
    pub quick_actions: &'a [QuickAction<N>],
    /// Which of them are offered, by index (they apply to the graph and layout as they are).
    pub offered: &'a [usize],
    /// How many workers runs start with.
    pub workers: usize,
    /// Whether settings are locked (while recording).
    pub locked: bool,
    pub dev_tools: &'a mut DevTools,
    pub canvas: &'a mut Canvas,
    pub selection: &'a mut Selection,
    pub open_windows: &'a HashSet<NodeId>,
    /// The files last opened or saved, latest first.
    pub recent: &'a [PathBuf],
    pub events: &'a mut Vec<Event>,
    /// For the dev tools' chart; and each pane's time this frame, as it's drawn.
    pub frame_times: &'a FrameTimes,
    pub timings: &'a mut Vec<(String, f32)>,
}

/// A node's window content: its own view if it has one (which shows its settings as it likes), otherwise the
/// default window (`node_window`). It finds what its passes share with it in `shared`: the run's, or the last run's.
#[allow(clippy::too_many_arguments)]
pub fn node_ui<N: NodesEnum>(
    ui: &mut egui::Ui,
    graph: &mut Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    shared: Option<&crate::core::engine::SharedByNode>,
    ui_state: Option<&crate::core::engine::UiStateCell>,
    base_directory: Option<&crate::core::base_directory::Place>,
    locked: bool,
    advanced: bool,
    id: NodeId,
    events: &mut Vec<Event>,
) {
    let Some(state) = resolved.get(&id) else { return };
    let has_custom_ui = graph.get(id).is_some_and(|e| e.node.has_custom_ui());
    let running = runner.is_some();
    // A view stays live while recording (changes it makes to settings are undone: see `App::lock`). It finds what
    // its passes share with it in `shared`; triggers need a run, to go to.
    let live = runner.map(|r| r.pass_triggers());
    if has_custom_ui {
        let Some(mut tools) = NodeTools::new(graph, id, advanced, live, &mut *events) else { return };
        let tools = Some(&mut tools as &mut dyn crate::core::engine::NodeToolbar);
        let streams = &state.input_streams;
        graph.edit(
            id,
            streams,
            colours,
            shared,
            ui_state,
            base_directory,
            running,
            locked,
            tools,
            |node: &mut N, context| node.custom_ui(ui, context),
        );
    } else {
        let gui = (shared, ui_state);
        node_window::show(ui, graph, resolved, colours, live, gui, base_directory, locked, advanced, id, events);
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
fn annotation_ui<A: AnnotationsEnum>(
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

/// The margin inside a pane.
pub const PANE_MARGIN: i8 = 8;

fn padded(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().inner_margin(PANE_MARGIN).show(ui, add);
}

impl<N: NodesEnum, A: AnnotationsEnum> Panes<'_, N, A> {
    fn pane_contents(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Home if self.mode == Mode::Basic => basic::show(
                ui,
                self.graph,
                self.resolved,
                self.colours,
                self.quick_actions,
                self.offered,
                self.open_windows,
                self.recent,
                self.locked,
                self.events,
            ),
            Pane::Home => {
                // The line under the toolbar right under it, so its margins above and below are the same.
                ui.spacing_mut().item_spacing.y = 0.0;
                toolbar::graph_toolbar::<A>(
                    ui,
                    self.graph_tools,
                    self.canvas.tool,
                    self.hidden_annotations,
                    self.events,
                );
                ui.add(egui::Separator::default().spacing(1.0));
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
                let (advanced, base, shared) = (self.mode == Mode::Advanced, self.base_directory, self.shared);
                let ui_state = self.ui_states.get(graph, id);
                padded(ui, |ui| {
                    let events = &mut *self.events;
                    node_ui(ui, graph, resolved, colours, runner, shared, ui_state, base, locked, advanced, id, events)
                })
            }
        }
    }
}

impl<N: NodesEnum, A: AnnotationsEnum> egui_tiles::Behavior<Pane> for Panes<'_, N, A> {
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
