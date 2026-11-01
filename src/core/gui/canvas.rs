//! The graph editor: nodes, their ports and the edges between them, plus annotations, on a canvas that pans
//! and zooms.
//!
//! Everything is laid out and painted in screen coordinates, with sizes scaled by the zoom, so text is
//! rasterised at the size it's shown and stays sharp. (Scaling already-drawn shapes, as `egui::Scene` does,
//! blurs text when zoomed in.) Positions are stored in scene coordinates.
//!
//! Each frame registers interactions in stacking order (annotations, then nodes, then ports), applying any
//! moves straight away, then paints the edges, any wire being dragged out, and the nodes.
//!
//! Data ports sit with outputs on the right of a node and inputs on the left, so data flows left to right.
//! Settings ports are the other way round: settings on the right and controls on the left, since settings
//! edges usually run back upstream. Their edges are dashed.

use std::collections::{BTreeSet, HashMap, HashSet};

use eframe::egui::emath::TSTransform;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Painter, PointerButton, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};

use super::Event;
use super::style;
use crate::core::annotation::{Annotation, AnnotationId, AnnotationSet, Annotations};
use crate::core::engine::{
    Dimension, Edge, EdgeInfo, EdgeSet, Edges, Graph, KindKey, NodeEntry, NodeId, NodeSet, NodeState, Port, Resolved,
    Role,
};
use crate::core::theme::Colours;

pub const NODE_WIDTH: f32 = 190.0;
pub const HEADER: f32 = 26.0;
const ROW: f32 = 22.0;
const PAD: f32 = 5.0;
/// Extra space between the data ports and the settings ports.
const DIVIDER: f32 = 8.0;
const PORT_RADIUS: f32 = 4.5;
/// Screen points within which the pointer counts as on a port or edge.
const HIT_DISTANCE: f32 = 8.0;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 2.5;

#[derive(Default)]
pub struct Selection {
    pub nodes: BTreeSet<NodeId>,
    pub annotations: BTreeSet<AnnotationId>,
    pub edge: Option<Edge>,
}

impl Selection {
    pub fn only(&mut self, id: NodeId) {
        self.clear();
        self.nodes.insert(id);
    }

    pub fn only_annotation(&mut self, id: AnnotationId) {
        self.clear();
        self.annotations.insert(id);
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.annotations.clear();
        self.edge = None;
    }

    /// The selected node, if it's the only thing selected.
    pub fn single(&self) -> Option<NodeId> {
        if self.nodes.len() == 1 && self.annotations.is_empty() { self.nodes.first().copied() } else { None }
    }

    /// Drops anything no longer there (after an undo, say).
    pub fn retain_existing<N: NodeSet, A: AnnotationSet>(&mut self, graph: &Graph<N>, annotations: &Annotations<A>) {
        self.nodes.retain(|id| graph.contains(*id));
        self.annotations.retain(|id| annotations.contains(*id));
        self.edge = self.edge.filter(|e| graph.edges().contains(e));
    }
}

/// What dragging on the canvas does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tool {
    /// Select and move things, and connect ports.
    #[default]
    Select,
    /// Draw out an annotation of a type (by index), then go back to selecting.
    Draw(usize),
}

/// Whether a port sits on the right of its node, so wires leave it rightwards.
pub fn on_right(role: Role) -> bool {
    matches!(role, Role::Output | Role::Setting)
}

/// Where a node and its parts are, on screen.
struct Layout {
    rect: Rect,
    header: Rect,
    zoom: f32,
    ports: HashMap<(Role, usize), Pos2>,
    /// Between the data and settings ports, if it has both.
    divider: Option<f32>,
    /// Where a warning's shown, in the header's right corner.
    badge: Rect,
}

/// A node's size on the graph (unzoomed): its header and a row per port, with a divider between the data and
/// settings ports, if it has both.
pub fn node_size<E: Edges>(state: &NodeState<E>) -> egui::Vec2 {
    let count = |role| state.count(role);
    let data_rows = count(Role::Output) + count(Role::Input);
    let settings_rows = count(Role::Setting) + count(Role::Control);
    let divider = if settings_rows > 0 && data_rows > 0 { DIVIDER } else { 0.0 };
    egui::vec2(NODE_WIDTH, HEADER + 2.0 * PAD + ROW * (data_rows + settings_rows) as f32 + divider)
}

impl Layout {
    /// One row per port: outputs, inputs, then settings and controls.
    fn new<E: Edges>(pos: [f32; 2], state: &NodeState<E>, to_screen: &TSTransform) -> Self {
        let zoom = to_screen.scaling;
        let count = |role| state.count(role);
        let data_rows = count(Role::Output) + count(Role::Input);
        let settings_rows = count(Role::Setting) + count(Role::Control);
        let divider = if settings_rows > 0 && data_rows > 0 { DIVIDER } else { 0.0 };
        let height = node_size(state).y;
        let min = *to_screen * Pos2::from(pos);
        let rect = Rect::from_min_size(min, egui::vec2(NODE_WIDTH, height) * zoom);
        let header = Rect::from_min_size(min, egui::vec2(NODE_WIDTH, HEADER) * zoom);
        let row_y = |row: usize, extra: f32| header.bottom() + (PAD + ROW * (row as f32 + 0.5) + extra) * zoom;

        let mut ports = HashMap::new();
        let mut row = 0;
        for role in [Role::Output, Role::Input, Role::Setting, Role::Control] {
            let extra = if role.dimension() == Dimension::Settings { divider } else { 0.0 };
            let x = if on_right(role) { rect.right() } else { rect.left() };
            for index in 0..count(role) {
                ports.insert((role, index), egui::pos2(x, row_y(row, extra)));
                row += 1;
            }
        }
        let badge = Rect::from_center_size(
            egui::pos2(header.right() - 15.0 * zoom, header.center().y),
            egui::vec2(16.0, 13.0) * zoom,
        );
        Layout {
            rect,
            header,
            zoom,
            ports,
            divider: (divider > 0.0).then(|| header.bottom() + (PAD + ROW * data_rows as f32 + divider / 2.0) * zoom),
            badge,
        }
    }

    fn port(&self, port: Port) -> Option<Pos2> {
        self.ports.get(&(port.role, port.index)).copied()
    }
}

/// A wire from `a` to `b`, leaving each in the direction its port faces.
fn bezier(a: Pos2, a_right: bool, b: Pos2, b_right: bool, zoom: f32) -> [Pos2; 4] {
    let dx = ((b.x - a.x).abs() * 0.5).max(40.0 * zoom);
    let out = |right: bool| egui::vec2(if right { dx } else { -dx }, 0.0);
    [a, a + out(a_right), b + out(b_right), b]
}

fn bezier_points([a, b, c, d]: [Pos2; 4]) -> Vec<Pos2> {
    (0..=32)
        .map(|i| {
            let t = i as f32 / 32.0;
            let u = 1.0 - t;
            let p = a.to_vec2() * u * u * u
                + b.to_vec2() * 3.0 * u * u * t
                + c.to_vec2() * 3.0 * u * t * t
                + d.to_vec2() * t * t * t;
            p.to_pos2()
        })
        .collect()
}

/// Data edges are solid, in their type's colour, with an arrowhead halfway along showing which way the data
/// flows. Settings edges are grey (`wire_colour`) and dashed, with arrowheads both ways (the setting's node and
/// the control's can both change it): their ports show their type. A read-only one (to a control that only reads
/// the setting) is dotted, with an arrowhead pointing to the reader.
fn paint_wire(painter: &Painter, points: [Pos2; 4], dimension: Dimension, read_only: bool, stroke: Stroke, zoom: f32) {
    let path = bezier_points(points);
    let [a, b, c, d] = points.map(|p| p.to_vec2());
    // The curve's midpoint and its direction there.
    let middle = ((a + 3.0 * b + 3.0 * c + d) / 8.0).to_pos2();
    let direction = 0.75 * ((b - a) + 2.0 * (c - b) + (d - c));
    let size = (4.0 + 1.5 * stroke.width) * zoom.sqrt();
    match dimension {
        Dimension::Data => {
            painter.add(Shape::line(path, stroke));
            arrowhead(painter, middle, direction, size, stroke.color);
        }
        Dimension::Settings if read_only => {
            painter.extend(Shape::dotted_line(&path, stroke.color, 5.0 * zoom, stroke.width * 0.6));
            // From the setting (where the wire ends) back to the control reading it.
            arrowhead(painter, middle, -direction, size, stroke.color);
        }
        Dimension::Settings => {
            painter.extend(Shape::dashed_line(&path, stroke, 7.0 * zoom, 4.0 * zoom));
            both_ways(painter, middle, direction, size, stroke.color);
        }
    }
}

/// A pair of arrowheads back to back at `at`, pointing either way along `direction`.
fn both_ways(painter: &Painter, at: Pos2, direction: Vec2, size: f32, colour: Color32) {
    let offset = direction.normalized() * size * 0.55;
    if offset.is_finite() {
        arrowhead(painter, at + offset, direction, size, colour);
        arrowhead(painter, at - offset, -direction, size, colour);
    }
}

/// Whether a control only reads the setting it's linked to.
fn read_only<E: Edges>(resolved: &Resolved<E>, port: Port) -> bool {
    port.role == Role::Control
        && resolved.get(&port.node).and_then(|s| s.controls.get(port.index)).is_some_and(|c| c.read_only)
}

/// The colour to draw an edge of a type with colour `colour`: settings edges are all the same grey, so they
/// stay in the background of the data flowing through the graph.
fn wire_colour(dimension: Dimension, colour: Color32, colours: &Colours) -> Color32 {
    match dimension {
        Dimension::Data => colour,
        Dimension::Settings => colours.text_secondary,
    }
}

/// A diamond centred on `at`, `d` from its centre to each corner.
fn diamond(at: Pos2, d: f32) -> Vec<Pos2> {
    vec![at - egui::vec2(0.0, d), at + egui::vec2(d, 0.0), at + egui::vec2(0.0, d), at - egui::vec2(d, 0.0)]
}

/// A filled arrowhead centred on `at`, pointing along `direction`.
fn arrowhead(painter: &Painter, at: Pos2, direction: Vec2, size: f32, colour: Color32) {
    let forward = direction.normalized();
    if !forward.is_finite() {
        return;
    }
    let side = forward.rot90() * size * 0.6;
    let tip = at + forward * size * 0.6;
    let back = at - forward * size * 0.4;
    painter.add(Shape::convex_polygon(vec![tip, back + side, back - side], colour, Stroke::NONE));
}

/// How far either side of a line annotation, and around its ends' handles, it can be grabbed, on screen.
const HANDLE: f32 = 8.0;

pub struct Canvas {
    /// Scene to canvas coordinates, relative to the canvas's top-left corner.
    view: TSTransform,
    /// Scene to screen, as of the latest frame.
    to_screen: TSTransform,
    /// Where the canvas is on screen.
    screen_rect: Rect,
    frame_requested: bool,
    /// The port a wire is being dragged from.
    wire: Option<Port>,
    /// Where a selection rectangle started, in scene coordinates.
    marquee: Option<Pos2>,
    hovered_edge: Option<Edge>,
    /// The annotation whose text is being edited.
    editing: Option<AnnotationId>,
    pub colours: &'static Colours,
    pub tool: Tool,
    /// Where an annotation being drawn out started, in scene coordinates.
    drawing: Option<Pos2>,
    /// Whether the legend is expanded (rather than just its title).
    pub legend_open: bool,
}

impl Canvas {
    pub fn new(colours: &'static Colours) -> Self {
        Canvas {
            view: TSTransform::IDENTITY,
            to_screen: TSTransform::IDENTITY,
            screen_rect: Rect::NOTHING,
            frame_requested: true,
            wire: None,
            marquee: None,
            hovered_edge: None,
            editing: None,
            colours,
            tool: Tool::default(),
            drawing: None,
            legend_open: true,
        }
    }
}

impl Canvas {
    /// Fits everything in view on the next frame.
    pub fn frame_all(&mut self) {
        self.frame_requested = true;
    }

    pub fn reset_zoom(&mut self) {
        let centre = self.screen_rect.size() / 2.0;
        self.view = TSTransform::from_translation(centre)
            * TSTransform::from_scaling(1.0 / self.view.scaling)
            * TSTransform::from_translation(-centre)
            * self.view;
    }

    pub fn view_centre(&self) -> Pos2 {
        self.to_screen.inverse() * self.screen_rect.center()
    }

    pub fn to_screen(&self, scene: Pos2) -> Pos2 {
        self.to_screen * scene
    }

    /// The pointer's position on screen and in the scene, if it's over the canvas.
    pub fn pointer(&self, ctx: &egui::Context) -> Option<(Pos2, Pos2)> {
        let screen = ctx.input(|i| i.pointer.latest_pos()).filter(|p| self.screen_rect.contains(*p))?;
        Some((screen, self.to_screen.inverse() * screen))
    }

    pub fn edit_annotation(&mut self, id: AnnotationId) {
        self.editing = Some(id);
    }

    /// What the user can do in the middle of a gesture, for the status bar.
    pub fn hint(&self) -> Option<&'static str> {
        if let Tool::Draw(_) = self.tool {
            Some("Drag out the annotation, or click for the default size. Escape to cancel")
        } else if let Some(port) = self.wire {
            Some(match port.role.dimension() {
                Dimension::Data => "Release on a highlighted port to connect, or on empty space to add a node there",
                Dimension::Settings => {
                    "Release on a highlighted setting or control to link them, or on empty space to add a node"
                }
            })
        } else if self.marquee.is_some() {
            Some("Hold shift to add to the selection")
        } else if self.editing.is_some() {
            Some("Editing an annotation. Click outside it or press Escape to finish")
        } else {
            None
        }
    }

    fn framing<N: NodeSet, A: AnnotationSet>(
        &self,
        graph: &Graph<N>,
        annotations: &Annotations<A>,
        resolved: &Resolved<N::Edges>,
    ) -> TSTransform {
        let nodes = layouts(graph, resolved, &TSTransform::IDENTITY).into_values().map(|l| l.rect);
        let notes = annotations.iter().map(|(_, a)| a.rect());
        let bounds = nodes
            .chain(notes)
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::from_center_size(Pos2::ZERO, egui::vec2(400.0, 300.0)))
            .expand(60.0);
        let size = self.screen_rect.size();
        // Never zoom in past 100% just to fill the view with a small graph.
        let zoom = (size.x / bounds.width()).min(size.y / bounds.height()).clamp(MIN_ZOOM, 1.0);
        TSTransform::new(size / 2.0 - bounds.center().to_vec2() * zoom, zoom)
    }

    /// Middle- or right-drag, or scroll, to pan; pinch or ctrl-scroll to zoom about the pointer.
    fn pan_and_zoom(&mut self, ui: &egui::Ui, background: &Response) {
        if background.dragged_by(PointerButton::Middle) || background.dragged_by(PointerButton::Secondary) {
            self.view.translation += background.drag_delta();
        }
        if !ui.rect_contains_pointer(self.screen_rect) {
            return;
        }
        let (scroll, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if zoom != 1.0
            && let Some(pointer) = pointer
        {
            let zoom = (self.view.scaling * zoom).clamp(MIN_ZOOM, MAX_ZOOM) / self.view.scaling;
            let about = pointer - self.screen_rect.min;
            self.view = TSTransform::from_translation(about)
                * TSTransform::from_scaling(zoom)
                * TSTransform::from_translation(-about)
                * self.view;
        }
        self.view.translation += scroll;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show<N: NodeSet, A: AnnotationSet>(
        &mut self,
        ui: &mut egui::Ui,
        graph: &mut Graph<N>,
        annotations: &mut Annotations<A>,
        resolved: &Resolved<N::Edges>,
        selection: &mut Selection,
        open_windows: &HashSet<NodeId>,
        events: &mut Vec<Event>,
    ) {
        let (rect, background) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.screen_rect = rect;
        if self.frame_requested && rect.is_positive() {
            self.view = self.framing(graph, annotations, resolved);
            self.frame_requested = false;
        }
        self.pan_and_zoom(ui, &background);
        self.to_screen = TSTransform::from_translation(rect.min.to_vec2()) * self.view;
        let zoom = self.to_screen.scaling;
        let painter = ui.painter_at(rect);
        self.paint_grid(&painter);

        let pointer = ui.input(|i| i.pointer.latest_pos());
        let shift = ui.input(|i| i.modifiers.shift);

        self.annotations(ui, &painter, graph, annotations, selection, events, false);
        self.paste_image(ui, annotations, selection, pointer.filter(|p| rect.contains(*p)));

        // Selected nodes are drawn last, so on top.
        let mut order: Vec<NodeId> = graph.ids().filter(|id| !selection.nodes.contains(id)).collect();
        order.extend(selection.nodes.iter().filter(|id| graph.contains(**id)));

        let mut layouts = layouts(graph, resolved, &self.to_screen);
        let mut moved = false;
        for &id in &order {
            if let (Some(layout), Some(state)) = (layouts.get(&id), resolved.get(&id)) {
                let open = open_windows.contains(&id);
                moved |= self.interact_node(ui, id, layout, state, open, graph, annotations, selection, events);
            }
        }
        if moved {
            layouts = self::layouts(graph, resolved, &self.to_screen);
        }

        let drawn = match self.tool {
            Tool::Select => None,
            Tool::Draw(index) => self.draw_annotation(ui, rect, index, annotations, selection),
        };
        self.background(ui, &background, selection, events);
        let marquee = self.marquee.zip(pointer).map(|(start, p)| Rect::from_two_pos(self.to_screen * start, p));
        if let Some(m) = marquee
            && !ui.input(|i| i.pointer.primary_down())
        {
            selection.nodes.extend(layouts.iter().filter(|(_, l)| l.rect.intersects(m)).map(|(id, _)| *id));
            let notes = annotations.iter().filter(|(_, a)| (self.to_screen * a.rect()).intersects(m));
            selection.annotations.extend(notes.map(|(id, _)| id));
            self.marquee = None;
        }

        let targets = self.wire.map(|port| graph.targets(port)).unwrap_or_default();
        let target = pointer.and_then(|p| target_at(p, &layouts, &targets));
        if let Some(port) = self.wire
            && !ui.input(|i| i.pointer.primary_down())
        {
            self.wire = None;
            if let Some(p) = pointer {
                self.drop_wire(port, target, p, graph, resolved, &layouts, selection, events);
            }
        }

        let over_node = pointer.is_some_and(|p| layouts.values().any(|l| l.rect.contains(p)));
        self.hovered_edge = None;
        if self.wire.is_none() && self.marquee.is_none() && !over_node && background.hovered() {
            self.hovered_edge = pointer.and_then(|p| {
                graph
                    .edges()
                    .iter()
                    .filter_map(|e| {
                        let points = bezier_points(edge_points(e, &layouts, zoom)?);
                        Some((*e, points.iter().map(|q| q.distance(p)).fold(f32::INFINITY, f32::min)))
                    })
                    .filter(|(_, d)| *d < HIT_DISTANCE)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(e, _)| e)
            });
        }
        if background.clicked() {
            if let Some(edge) = self.hovered_edge {
                selection.clear();
                selection.edge = Some(edge);
            } else if !shift {
                selection.clear();
            }
        }

        self.paint_edges(&painter, graph, resolved, &layouts, selection);
        if let (Some(port), Some(p)) = (self.wire, pointer)
            && let Some(start) = port_pos(&layouts, port)
            && let Some(info) = resolved.get(&port.node).and_then(|s| s.info(port.role, port.index))
        {
            let tip = target.and_then(|t| port_pos(&layouts, t)).unwrap_or(p);
            let points = if port.role.is_source() {
                bezier(start, on_right(port.role), tip, !on_right(port.role), zoom)
            } else {
                bezier(tip, !on_right(port.role), start, on_right(port.role), zoom)
            };
            let dimension = port.role.dimension();
            let colour = wire_colour(dimension, info.colour, self.colours);
            let read_only = read_only(resolved, port) || target.is_some_and(|t| read_only(resolved, t));
            paint_wire(&painter, points, dimension, read_only, Stroke::new(2.0 * zoom.sqrt(), colour), zoom);
        }

        let connected: HashSet<Port> = graph.edges().iter().flat_map(|e| [e.source(), e.target()]).collect();
        for &id in &order {
            let (Some(layout), Some(entry), Some(state)) = (layouts.get(&id), graph.get(id), resolved.get(&id)) else {
                continue;
            };
            let selected = selection.nodes.contains(&id) || marquee.is_some_and(|m| m.intersects(layout.rect));
            let has_target = targets.iter().any(|t| t.node == id);
            let dim = self.wire.is_some_and(|w| w.node != id) && !has_target;
            let ports = PortPaint { connected: &connected, targets: &targets, target, pointer, colours: self.colours };
            paint_node(&painter, id, layout, entry, state, selected, dim, &ports);
        }
        // Those above the graph (images), over the nodes, and grabbed before them.
        self.annotations(ui, &painter, graph, annotations, selection, events, true);

        if let (Some(r), Tool::Draw(index)) = (drawn, self.tool) {
            painter.rect(
                r,
                4.0 * zoom,
                A::create(index).fill(self.colours).gamma_multiply(0.6),
                Stroke::new(1.0, self.colours.text_secondary),
                StrokeKind::Inside,
            );
        }
        self.legend::<N>(ui, &painter, resolved);
        if let Some(m) = marquee {
            let stroke = Stroke::new(1.0, self.colours.text_secondary);
            painter.rect(m, 0.0, self.colours.text_primary.gamma_multiply(0.12), stroke, StrokeKind::Inside);
        }
    }

    /// With a drawing tool picked, the whole canvas is for drawing: drag out the annotation (or click for the
    /// default size). Returns the outline so far, on screen, while dragging.
    fn draw_annotation<A: AnnotationSet>(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        type_index: usize,
        annotations: &mut Annotations<A>,
        selection: &mut Selection,
    ) -> Option<Rect> {
        let response = ui
            .interact(rect, Id::unique("draw-annotation"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Crosshair);
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let to_scene = self.to_screen.inverse();
        if response.drag_started() {
            // From where the button went down, not where the movement was first noticed.
            self.drawing = ui.input(|i| i.pointer.press_origin()).or(pointer).map(|p| to_scene * p);
        }
        let outline = self.drawing.zip(pointer).map(|(start, p)| Rect::from_two_pos(self.to_screen * start, p));
        // Which way it was drawn: leftwards, upwards.
        let flipped = self.drawing.zip(pointer).map_or([false; 2], |(start, p)| {
            let end = to_scene * p;
            [end.x < start.x, end.y < start.y]
        });
        let default_size = Vec2::from(A::TYPES[type_index].default_size);
        let min_size = A::min_size(type_index);
        let drawn = if response.drag_stopped() {
            self.drawing = None;
            outline.map(|r| to_scene * r)
        } else if response.clicked() {
            pointer.map(|p| Rect::from_min_size(to_scene * p, default_size))
        } else {
            None
        };
        if let Some(drawn) = drawn {
            // A slip of a drag gets the default size, rather than something too small to use.
            let size = if drawn.width() < min_size[0] && drawn.height() < min_size[1] {
                default_size
            } else {
                drawn.size().max(min_size.into())
            };
            let kind = A::drawn(type_index, flipped);
            let annotation = Annotation { pos: drawn.min.into(), size: size.into(), kind };
            let id = annotations.add(annotation);
            selection.only_annotation(id);
            self.editing = Some(id);
            self.tool = Tool::Select;
        }
        outline.filter(|_| self.drawing.is_some())
    }

    /// A key to the edge types on the graph (on an edge or a port), in the bottom-right corner of the canvas
    /// (not of the scene, so it stays put as the graph pans and zooms). Click its title to collapse or expand it.
    fn legend<N: NodeSet>(&mut self, ui: &mut egui::Ui, painter: &Painter, resolved: &Resolved<N::Edges>) {
        const WIDTH: f32 = 160.0;
        const ROW: f32 = 18.0;
        const PAD: f32 = 8.0;
        const MARGIN: f32 = 10.0;
        let sections = legend_entries::<N>(resolved);
        if sections.is_empty() {
            return; // nothing on the graph to explain
        }
        // Whether to explain read-only settings edges too.
        let read_only = resolved.values().any(|s| s.controls.iter().any(|c| c.read_only));
        let rows: usize = if self.legend_open {
            sections.iter().map(|(_, _, kinds)| 1 + kinds.len()).sum::<usize>() + read_only as usize
        } else {
            0
        };
        let height = 2.0 * PAD + ROW * (1 + rows) as f32;
        let rect = Rect::from_min_size(
            self.screen_rect.max - egui::vec2(WIDTH + MARGIN, height + MARGIN),
            egui::vec2(WIDTH, height),
        );
        if !self.screen_rect.contains_rect(rect) {
            return; // no room
        }
        // Over the whole legend, so clicks on it don't reach the graph beneath.
        ui.interact(rect, Id::unique("legend"), Sense::click_and_drag());
        let title = Rect::from_min_size(rect.min + egui::vec2(PAD, PAD), egui::vec2(WIDTH - 2.0 * PAD, ROW));
        let toggle = ui.interact(title, Id::unique("legend-toggle"), Sense::click());
        toggle.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Legend"));
        if toggle.clicked() {
            self.legend_open = !self.legend_open;
        }

        let colours = self.colours;
        painter.rect_filled(rect.translate(egui::vec2(0.0, 2.0)), 6.0, colours.shadow);
        painter.rect(rect, 6.0, colours.surface, Stroke::new(1.0, colours.border), StrokeKind::Inside);
        let font = FontId::proportional(12.0);
        let text = if toggle.hovered() { colours.text_primary } else { colours.text_secondary };
        painter.text(title.left_center(), Align2::LEFT_CENTER, "Legend", font.clone(), text);
        // A chevron: down when open, left when closed (it opens upwards).
        let c = egui::pos2(title.right() - 6.0, title.center().y);
        let chevron = if self.legend_open {
            vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(0.0, 2.0), c + egui::vec2(4.0, -2.0)]
        } else {
            vec![c + egui::vec2(2.0, -4.0), c + egui::vec2(-2.0, 0.0), c + egui::vec2(2.0, 4.0)]
        };
        painter.add(Shape::line(chevron, Stroke::new(1.5, text)));
        if !self.legend_open {
            return;
        }

        let mut y = title.bottom();
        for (heading, dimension, kinds) in sections {
            painter.text(
                egui::pos2(title.left(), y + ROW / 2.0),
                Align2::LEFT_CENTER,
                heading,
                FontId::proportional(10.5),
                colours.text_disabled,
            );
            y += ROW;
            for info in kinds {
                let mid = y + ROW / 2.0;
                let (start, end) = (egui::pos2(title.left(), mid), egui::pos2(title.left() + 26.0, mid));
                let stroke = Stroke::new(2.0, wire_colour(dimension, info.colour, colours));
                match dimension {
                    Dimension::Data => {
                        painter.line_segment([start, end], stroke);
                        arrowhead(painter, start.lerp(end, 0.5), egui::vec2(1.0, 0.0), 7.0, info.colour);
                    }
                    // A grey edge into a port of the type's colour, as on the graph.
                    Dimension::Settings => {
                        painter.extend(Shape::dashed_line(&[start, end], stroke, 5.0, 3.0));
                        both_ways(painter, start.lerp(end, 0.45), egui::vec2(1.0, 0.0), 6.0, stroke.color);
                        painter.add(Shape::convex_polygon(diamond(end, 5.0), info.colour, Stroke::NONE));
                        painter.add(Shape::convex_polygon(diamond(end, 2.0), stroke.color, Stroke::NONE));
                    }
                }
                let label = egui::pos2(title.left() + 36.0, mid);
                painter.text(label, Align2::LEFT_CENTER, info.name, font.clone(), colours.text_primary);
                y += ROW;
            }
        }
        if read_only {
            // A dotted edge into a hollow port, as a control that only reads a setting has.
            let mid = y + ROW / 2.0;
            let (start, end) = (egui::pos2(title.left(), mid), egui::pos2(title.left() + 26.0, mid));
            painter.extend(Shape::dotted_line(&[start, end], colours.text_secondary, 4.0, 1.2));
            let outline = Stroke::new(1.5, colours.text_secondary);
            painter.add(Shape::convex_polygon(diamond(start, 4.5), colours.surface, outline));
            arrowhead(painter, start.lerp(end, 0.5), egui::vec2(-1.0, 0.0), 6.0, colours.text_secondary);
            let label = egui::pos2(title.left() + 36.0, mid);
            painter.text(label, Align2::LEFT_CENTER, "read only", font.clone(), colours.text_primary);
        }
    }

    /// Clicks and drags on empty canvas (middle and right drags pan, in `pan_and_zoom`).
    fn background(&mut self, ui: &egui::Ui, response: &Response, selection: &mut Selection, events: &mut Vec<Event>) {
        let pointer = ui.input(|i| i.pointer.latest_pos());
        if (response.double_clicked() || response.secondary_clicked())
            && let Some(screen) = pointer
        {
            events.push(Event::OpenMenu { screen, scene: self.to_screen.inverse() * screen, from: None });
        }
        if response.drag_started_by(PointerButton::Primary) {
            self.marquee = pointer.map(|p| self.to_screen.inverse() * p);
            if !ui.input(|i| i.modifiers.shift) {
                selection.clear();
            }
        }
    }

    /// An image pasted over the canvas (while nothing's being typed in) becomes an annotation there, if there's
    /// an annotation type that shows images.
    fn paste_image<A: AnnotationSet>(
        &mut self,
        ui: &egui::Ui,
        annotations: &mut Annotations<A>,
        selection: &mut Selection,
        pointer: Option<Pos2>,
    ) {
        if self.editing.is_some() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pasted = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::PasteImage(image) => Some(image.clone()),
                _ => None,
            })
        });
        let Some((kind, size)) = pasted.and_then(|image| A::pasted_image(&image)) else { return };
        // Centred on the pointer, or the middle of the view.
        let centre = pointer.map_or_else(|| self.view_centre(), |p| self.to_screen.inverse() * p);
        self.place(annotations, selection, kind, size, centre);
    }

    /// Makes an annotation of a type that's chosen rather than drawn (an image from a file, say), in the middle of
    /// the view.
    pub fn insert_chosen<A: AnnotationSet>(
        &mut self,
        type_index: usize,
        annotations: &mut Annotations<A>,
        selection: &mut Selection,
    ) {
        if let Some((kind, size)) = A::choose(type_index) {
            let centre = self.view_centre();
            self.place(annotations, selection, kind, size, centre);
        }
    }

    /// Adds an annotation centred on `centre`, scaled down (keeping its shape) to take no more than a quarter of
    /// the view across or down, and selects it.
    fn place<A: AnnotationSet>(
        &mut self,
        annotations: &mut Annotations<A>,
        selection: &mut Selection,
        kind: A,
        size: [f32; 2],
        centre: Pos2,
    ) {
        let view = self.screen_rect.size() / self.to_screen.scaling * 0.25;
        let scale = (view.x / size[0].max(1.0)).min(view.y / size[1].max(1.0)).min(1.0);
        let size = [size[0] * scale, size[1] * scale];
        let pos = [centre.x - size[0] / 2.0, centre.y - size[1] / 2.0];
        let id = annotations.add(Annotation { pos, size, kind });
        selection.only_annotation(id);
    }

    /// Annotations sit above the graph, or beneath it (those that go there: boxes, see `above_graph`): drag to
    /// move (along with anything else selected), drag the corner to resize, double-click to edit.
    #[allow(clippy::too_many_arguments)]
    fn annotations<N: NodeSet, A: AnnotationSet>(
        &mut self,
        ui: &mut egui::Ui,
        painter: &Painter,
        graph: &mut Graph<N>,
        annotations: &mut Annotations<A>,
        selection: &mut Selection,
        events: &mut Vec<Event>,
        above_graph: bool,
    ) {
        let zoom = self.to_screen.scaling;
        let shift = ui.input(|i| i.modifiers.shift);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        // Those that go behind the others first, so beneath them.
        let mut ids: Vec<AnnotationId> =
            annotations.iter().filter(|(_, a)| a.kind.above_graph() == above_graph).map(|(id, _)| id).collect();
        ids.sort_by_key(|id| !annotations.get(*id).is_some_and(|a| a.kind.behind()));
        for id in ids {
            let Some(annotation) = annotations.get(id) else { continue };
            let rect = self.to_screen * annotation.rect();
            // A line between two points can be grabbed a little either side of it, even where its rect is thin.
            let line = annotation.kind.points(annotation.rect()).is_some();
            let reach = if line { rect.expand(HANDLE) } else { rect };
            // Only where it can be grabbed (or while it's being dragged), so clicks elsewhere in it reach what's
            // beneath: a box's inside, say.
            let body_id = Id::unique(("annotation", id));
            let grabbable = ui.ctx().dragged_id() == Some(body_id)
                || pointer.is_some_and(|p| reach.contains(p) && annotation.kind.hit(rect, p, zoom));
            let sense = if grabbable { Sense::click_and_drag() } else { Sense::hover() };
            let body = ui.interact(reach, body_id, sense);
            if body.drag_started() && !selection.annotations.contains(&id) {
                if !shift {
                    selection.clear();
                }
                selection.annotations.insert(id);
            }
            let delta = body.drag_delta() / zoom;
            if delta != Vec2::ZERO {
                move_selection(graph, annotations, selection, delta);
            }
            if body.clicked() {
                if !shift {
                    selection.only_annotation(id);
                } else if !selection.annotations.remove(&id) {
                    selection.annotations.insert(id);
                }
            }
            if body.double_clicked() {
                self.editing = Some(id);
            }
            body.context_menu(|ui| {
                if ui.button("Edit").clicked() {
                    self.editing = Some(id);
                }
                if ui.button("Delete").clicked() {
                    events.push(Event::DeleteAnnotation(id));
                }
            });

            let Some(annotation) = annotations.get_mut(id) else { continue };
            let selected = selection.annotations.contains(&id);
            if let Some(points) = annotation.kind.points(annotation.rect()) {
                self.line_annotation(ui, painter, id, annotation, points, selected || body.hovered());
                continue;
            }
            let handle = Rect::from_min_max(rect.max - Vec2::splat(12.0 * zoom), rect.max);
            let resize = ui.interact(handle, Id::unique(("annotation-resize", id)), Sense::drag());
            if resize.dragged() {
                let mut size = (Vec2::from(annotation.size) + resize.drag_delta() / zoom)
                    .max(A::min_size(annotation.kind.type_index()).into());
                // Keeping its shape, if it keeps one: by whichever side was dragged further.
                if let Some(aspect) = annotation.kind.aspect().filter(|a| a.is_finite() && *a > 0.0) {
                    let old = Vec2::from(annotation.size);
                    if (size.x - old.x).abs() / aspect >= (size.y - old.y).abs() {
                        size.y = size.x / aspect;
                    } else {
                        size.x = size.y * aspect;
                    }
                }
                annotation.size = size.into();
            }
            resize.on_hover_cursor(egui::CursorIcon::ResizeNwSe);

            let rect = self.to_screen * annotation.rect();
            let fill = annotation.kind.fill(self.colours);
            let opaque = fill.a() == 255;
            if opaque {
                painter.rect_filled(rect.translate(egui::vec2(0.0, 3.0 * zoom)), 4.0 * zoom, self.colours.shadow);
            }
            painter.rect_filled(rect, 4.0 * zoom, fill);
            annotation.kind.paint(painter, rect, zoom, self.colours);
            if selected {
                let stroke = Stroke::new(2.0, self.colours.accent);
                painter.rect_stroke(rect, 4.0 * zoom, stroke, StrokeKind::Outside);
            }
            // Grip lines in the resize corner (only once selected, for one that's see-through).
            let grip = if opaque { Color32::from_black_alpha(70) } else { self.colours.text_secondary };
            for i in (1..=3).filter(|_| opaque || selected) {
                let d = i as f32 * 3.0 * zoom;
                let corner = rect.max - Vec2::splat(2.0 * zoom);
                let stroke = Stroke::new(1.0, grip);
                painter.line_segment([corner - egui::vec2(d, 0.0), corner - egui::vec2(0.0, d)], stroke);
            }
            let editing = self.editing == Some(id);
            let still_editing = annotation.content_ui(ui, rect, zoom, editing);
            if editing && !still_editing {
                self.editing = None;
            }
        }
    }

    /// An annotation that's a line between two points (an arrow, say): drawn, with a handle at each end (shown
    /// once it's selected or hovered) to drag that end.
    fn line_annotation<A: AnnotationSet>(
        &self,
        ui: &mut egui::Ui,
        painter: &Painter,
        id: AnnotationId,
        annotation: &mut Annotation<A>,
        mut points: [Pos2; 2],
        show_handles: bool,
    ) {
        let zoom = self.to_screen.scaling;
        for (end, point) in points.into_iter().enumerate() {
            let at = self.to_screen * point;
            let handle = Rect::from_center_size(at, Vec2::splat(2.0 * HANDLE));
            let response = ui.interact(handle, Id::unique(("annotation-end", id, end)), Sense::drag());
            if response.dragged() {
                points[end] = point + response.drag_delta() / zoom;
                let rect = annotation.kind.set_points(points);
                (annotation.pos, annotation.size) = (rect.min.into(), rect.size().into());
            }
            response.on_hover_cursor(egui::CursorIcon::Crosshair);
        }
        let rect = self.to_screen * annotation.rect();
        annotation.kind.paint(painter, rect, zoom, self.colours);
        if show_handles {
            for point in annotation.kind.points(annotation.rect()).into_iter().flatten() {
                let at = self.to_screen * point;
                painter.circle(at, 4.5, self.colours.surface, Stroke::new(2.0, self.colours.accent));
            }
        }
    }

    /// Registers the interactions for a node and its ports. Returns whether any nodes moved.
    #[allow(clippy::too_many_arguments)]
    fn interact_node<N: NodeSet, A: AnnotationSet>(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        layout: &Layout,
        state: &NodeState<N::Edges>,
        window_open: bool,
        graph: &mut Graph<N>,
        annotations: &mut Annotations<A>,
        selection: &mut Selection,
        events: &mut Vec<Event>,
    ) -> bool {
        let shift = ui.input(|i| i.modifiers.shift);
        let body = ui.interact(layout.rect, Id::unique(("node", id)), Sense::click_and_drag());
        if body.drag_started() && !selection.nodes.contains(&id) {
            if !shift {
                selection.clear();
            }
            selection.nodes.insert(id);
        }
        let delta = body.drag_delta() / layout.zoom;
        let moved = delta != Vec2::ZERO;
        if moved {
            move_selection(graph, annotations, selection, delta);
        }
        // A click selects it; once it's selected, a click (the second of a double-click, say) opens its window, or
        // brings it to the front.
        if body.clicked() {
            if !shift {
                if selection.nodes.contains(&id) {
                    events.push(Event::OpenWindow(id));
                }
                selection.only(id);
            } else if !selection.nodes.remove(&id) {
                selection.nodes.insert(id);
            }
        }
        body.context_menu(|ui| {
            if let Some(entry) = graph.get_mut(id) {
                let name = ui.label(egui::RichText::new("Name").small().weak());
                ui.text_edit_singleline(&mut entry.label).labelled_by(name.id);
                ui.separator();
            }
            if ui.button(if window_open { "Close window" } else { "Open window" }).clicked() {
                events.push(Event::ToggleWindow(id));
            }
            if ui.button("Duplicate").clicked() {
                events.push(Event::Duplicate(id));
            }
            if ui.button("Disconnect").clicked() {
                events.push(Event::Disconnect(id));
            }
            ui.separator();
            if ui.button("Delete").clicked() {
                events.push(Event::Delete(id));
            }
        });

        if !state.warnings.is_empty() {
            ui.interact(layout.badge, Id::unique(("badge", id)), Sense::hover())
                .on_hover_text(state.warnings.join("\n"));
        }

        for port in state.ports(id) {
            let Some(pos) = layout.port(port) else { continue };
            let response = ui.interact(
                Rect::from_center_size(pos, Vec2::splat(2.0 * HIT_DISTANCE)),
                Id::unique(("port", port.node, port.role.name(), port.index)),
                Sense::click_and_drag(),
            );
            if response.drag_started() {
                // Dragging from a connected input or setting picks up its edge, to move it elsewhere or drop it.
                self.wire = Some(match graph.edge_into(port) {
                    Some(edge) => {
                        graph.disconnect(edge);
                        edge.source()
                    }
                    None => port,
                });
            }
            if response.clicked() {
                events.push(Event::OpenMenu { screen: pos, scene: self.to_screen.inverse() * pos, from: Some(port) });
            }
            let name = state.name(port.role, port.index).unwrap_or("?");
            let (kind, description) = state.info(port.role, port.index).map_or(("?", ""), |i| (i.name, i.description));
            let summary = state.summary(port.role, port.index).unwrap_or_else(|| "not connected".into());
            let help = match port.role {
                Role::Input | Role::Output => "Drag to connect; click to add or pick a node to connect",
                Role::Setting => "Other nodes can edit this. Drag to a control to link them",
                Role::Control if state.controls.get(port.index).is_some_and(|c| c.read_only) => {
                    "Reads a setting of another node, without changing it. Drag to a setting to link them"
                }
                Role::Control => "Edits a setting of another node. Drag to a setting to link them",
            };
            response.on_hover_text(format!("{name} ({kind} {}): {summary}\n{description}\n\n{help}", port.role.name()));
        }
        moved
    }

    #[allow(clippy::too_many_arguments)]
    fn drop_wire<N: NodeSet>(
        &self,
        port: Port,
        target: Option<Port>,
        pointer: Pos2,
        graph: &mut Graph<N>,
        resolved: &Resolved<N::Edges>,
        layouts: &HashMap<NodeId, Layout>,
        selection: &mut Selection,
        events: &mut Vec<Event>,
    ) {
        if let Some(target) = target {
            if let Err(e) = graph.connect(port, target) {
                events.push(Event::Status(format!("Can't connect: {e}")));
            }
            selection.edge = None;
            return;
        }
        let Some((&node, _)) = layouts.iter().find(|(_, l)| l.rect.contains(pointer)) else {
            let scene = self.to_screen.inverse() * pointer;
            events.push(Event::OpenMenu { screen: pointer, scene, from: Some(port) });
            return;
        };
        if node == port.node {
            return; // let go where it started
        }
        // Dropped on a node with nowhere to connect: say why.
        let Some(state) = resolved.get(&node) else { return };
        let Some(kind) = resolved.get(&port.node).and_then(|s| s.kind(port.role, port.index)) else { return };
        let reason = state.find(node, port.role.partner(), kind).and_then(|p| graph.check(port, p).err());
        let reason = reason.map(|e| e.to_string()).unwrap_or_else(|| {
            let label = graph.get(node).map_or("", |e| e.label.as_str());
            let kind = crate::core::engine::edge_info::<N::Edges>(kind).name;
            format!("{label} has no {kind} {}", port.role.partner().name())
        });
        events.push(Event::Status(format!("Can't connect: {reason}")));
    }

    fn paint_grid(&self, painter: &Painter) {
        const SPACING: f32 = 40.0;
        let clip = self.screen_rect;
        painter.rect_filled(clip, 0.0, self.colours.background);
        let step = SPACING * self.to_screen.scaling;
        if step < 10.0 {
            return;
        }
        let stroke = Stroke::new(1.0, self.colours.grid);
        let origin = self.to_screen * Pos2::ZERO;
        let mut x = origin.x - ((origin.x - clip.left()) / step).floor() * step;
        while x < clip.right() {
            painter.vline(x, clip.y_range(), stroke);
            x += step;
        }
        let mut y = origin.y - ((origin.y - clip.top()) / step).floor() * step;
        while y < clip.bottom() {
            painter.hline(clip.x_range(), y, stroke);
            y += step;
        }
    }

    fn paint_edges<N: NodeSet>(
        &self,
        painter: &Painter,
        graph: &Graph<N>,
        resolved: &Resolved<N::Edges>,
        layouts: &HashMap<NodeId, Layout>,
        selection: &Selection,
    ) {
        let zoom = self.to_screen.scaling;
        let width = 2.0 * zoom.sqrt();
        for edge in graph.edges() {
            let Some(points) = edge_points(edge, layouts, zoom) else { continue };
            let source = edge.source();
            let Some(state) = resolved.get(&source.node) else { continue };
            let Some(info) = state.info(source.role, source.index) else { continue };
            let mut colour = wire_colour(edge.dimension, info.colour, self.colours);
            let unknown = edge.dimension == Dimension::Data && state.summary(source.role, source.index).is_none();
            if unknown || self.wire.is_some() {
                colour = colour.gamma_multiply(0.4);
            }
            let selected = selection.edge == Some(*edge);
            let w = if selected {
                width * 2.0
            } else if self.hovered_edge == Some(*edge) {
                width * 1.5
            } else {
                width
            };
            if selected {
                let halo = Stroke::new(w + 3.0, self.colours.text_primary.gamma_multiply(0.12));
                paint_wire(painter, points, Dimension::Data, false, halo, zoom);
            }
            let read_only = read_only(resolved, source);
            paint_wire(painter, points, edge.dimension, read_only, Stroke::new(w, colour), zoom);
        }
    }
}

/// The legend's sections (data, then settings) with the edge types to list in each: those on some port of
/// the graph (which covers the edges, as each connects two ports). Sections with none are left out.
pub fn legend_entries<N: NodeSet>(
    resolved: &Resolved<N::Edges>,
) -> Vec<(&'static str, Dimension, Vec<&'static EdgeInfo>)> {
    let present: HashSet<KindKey> =
        resolved.iter().flat_map(|(id, state)| state.ports(*id).filter_map(|p| state.kind(p.role, p.index))).collect();
    let shown = |dimension: Dimension, kinds: &'static [EdgeInfo]| -> Vec<&'static EdgeInfo> {
        kinds.iter().enumerate().filter(|(i, _)| present.contains(&(dimension, *i))).map(|(_, k)| k).collect()
    };
    [
        ("Data", Dimension::Data, shown(Dimension::Data, <<N::Edges as Edges>::DataStream as EdgeSet>::KINDS)),
        ("Settings", Dimension::Settings, shown(Dimension::Settings, <<N::Edges as Edges>::Setting as EdgeSet>::KINDS)),
    ]
    .into_iter()
    .filter(|(_, _, kinds)| !kinds.is_empty())
    .collect()
}

/// Moves every selected node and annotation by `delta`, in scene coordinates.
fn move_selection<N: NodeSet, A: AnnotationSet>(
    graph: &mut Graph<N>,
    annotations: &mut Annotations<A>,
    selection: &Selection,
    delta: Vec2,
) {
    for id in &selection.nodes {
        if let Some(entry) = graph.get_mut(*id) {
            entry.pos = (Pos2::from(entry.pos) + delta).into();
        }
    }
    for id in &selection.annotations {
        if let Some(annotation) = annotations.get_mut(*id) {
            annotation.pos = (Pos2::from(annotation.pos) + delta).into();
        }
    }
}

fn layouts<N: NodeSet>(
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    to_screen: &TSTransform,
) -> HashMap<NodeId, Layout> {
    graph.iter().filter_map(|(id, entry)| Some((id, Layout::new(entry.pos, resolved.get(&id)?, to_screen)))).collect()
}

fn port_pos(layouts: &HashMap<NodeId, Layout>, port: Port) -> Option<Pos2> {
    layouts.get(&port.node)?.port(port)
}

fn edge_points(edge: &Edge, layouts: &HashMap<NodeId, Layout>, zoom: f32) -> Option<[Pos2; 4]> {
    let (source, target) = (edge.source(), edge.target());
    let (a, b) = (port_pos(layouts, source)?, port_pos(layouts, target)?);
    Some(bezier(a, on_right(source.role), b, on_right(target.role), zoom))
}

/// The target port under the pointer: the nearest within reach, or failing that the first on a node it's over.
fn target_at(pointer: Pos2, layouts: &HashMap<NodeId, Layout>, targets: &[Port]) -> Option<Port> {
    let nearest = targets
        .iter()
        .filter_map(|t| Some((*t, port_pos(layouts, *t)?.distance(pointer))))
        .filter(|(_, d)| *d < 1.5 * HIT_DISTANCE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t);
    nearest.or_else(|| {
        let (node, _) = layouts.iter().find(|(_, l)| l.rect.contains(pointer))?;
        targets.iter().find(|t| t.node == *node).copied()
    })
}

struct PortPaint<'a> {
    connected: &'a HashSet<Port>,
    targets: &'a [Port],
    target: Option<Port>,
    pointer: Option<Pos2>,
    colours: &'a Colours,
}

#[allow(clippy::too_many_arguments)]
fn paint_node<N: NodeSet>(
    painter: &Painter,
    id: NodeId,
    layout: &Layout,
    entry: &NodeEntry<N>,
    state: &NodeState<N::Edges>,
    selected: bool,
    dim: bool,
    ports: &PortPaint,
) {
    let z = layout.zoom;
    let fade = |c: Color32| if dim { c.gamma_multiply(0.35) } else { c };
    let rect = layout.rect;
    let info = entry.node.info();
    let radius = 6.0 * z;

    let colours = ports.colours;
    let shadow = if dim { colours.shadow.gamma_multiply(0.35) } else { colours.shadow };
    painter.rect_filled(rect.translate(egui::vec2(0.0, 3.0 * z)), radius, shadow);
    painter.rect_filled(rect, radius, fade(colours.surface_raised));
    let r = radius.round().clamp(0.0, 255.0) as u8;
    let header_fill = fade(style::category_colour(info.category));
    painter.rect_filled(layout.header, CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, header_fill);
    let border = if selected { Stroke::new(2.0, colours.accent) } else { Stroke::new(1.0, fade(colours.border)) };
    painter.rect_stroke(rect, radius, border, StrokeKind::Outside);
    if let Some(y) = layout.divider {
        painter.hline(rect.x_range().shrink(8.0 * z), y, Stroke::new(1.0, fade(colours.divider)));
    }

    let font = |size: f32| FontId::proportional(size * z);
    let title_right = if state.warnings.is_empty() { layout.header.right() } else { layout.badge.left() };
    let title_clip = Rect::from_min_max(layout.header.min, egui::pos2(title_right - 4.0 * z, layout.header.bottom()));
    painter.with_clip_rect(title_clip.intersect(painter.clip_rect())).text(
        egui::pos2(layout.header.left() + 9.0 * z, layout.header.center().y),
        Align2::LEFT_CENTER,
        &entry.label,
        font(13.5),
        fade(Color32::from_gray(240)),
    );

    if !state.warnings.is_empty() {
        let b = layout.badge;
        let triangle = vec![
            egui::pos2(b.center().x, b.top() - z),
            b.right_bottom() + egui::vec2(z, z),
            b.left_bottom() + egui::vec2(-z, z),
        ];
        painter.add(Shape::convex_polygon(triangle, fade(colours.warning), Stroke::NONE));
        painter.text(b.center() + egui::vec2(0.0, 1.5 * z), Align2::CENTER_CENTER, "!", font(10.0), Color32::BLACK);
    }

    for port in state.ports(id) {
        let Some(pos) = layout.port(port) else { continue };
        let name = state.name(port.role, port.index).unwrap_or("?");
        let connected = ports.connected.contains(&port);
        let colour = state.info(port.role, port.index).map_or(Color32::GRAY, |i| i.colour);
        let (name_pos, align) = if on_right(port.role) {
            (pos - egui::vec2(12.0 * z, 0.0), Align2::RIGHT_CENTER)
        } else {
            (pos + egui::vec2(12.0 * z, 0.0), Align2::LEFT_CENTER)
        };
        let name_colour =
            if connected || port.role.is_source() { colours.text_primary } else { colours.text_secondary };
        painter.text(name_pos, align, name, font(12.5), fade(name_colour));
        // Outputs and settings also show what they carry, on the far side.
        if on_right(port.role) {
            let (summary, colour) = match state.summary(port.role, port.index) {
                Some(summary) => (summary, colours.text_secondary),
                None => ("—".into(), colours.text_disabled),
            };
            let at = egui::pos2(rect.left() + 10.0 * z, pos.y);
            painter.text(at, Align2::LEFT_CENTER, summary, font(11.0), fade(colour));
        }
        let read_only = port.role == Role::Control && state.controls.get(port.index).is_some_and(|c| c.read_only);
        paint_port(painter, port, pos, colour, connected, read_only, ports, z, fade);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_port(
    painter: &Painter,
    port: Port,
    pos: Pos2,
    colour: Color32,
    connected: bool,
    read_only: bool,
    ports: &PortPaint,
    z: f32,
    fade: impl Fn(Color32) -> Color32,
) {
    let colour = fade(colour);
    let r = PORT_RADIUS * z;
    let hovered = ports.pointer.is_some_and(|p| p.distance(pos) < HIT_DISTANCE);
    if ports.target == Some(port) {
        painter.circle_filled(pos, r + 7.0 * z, colour.gamma_multiply(0.5));
    } else if ports.targets.contains(&port) {
        painter.circle_filled(pos, r + 5.0 * z, colour.gamma_multiply(0.2));
        painter.circle_stroke(pos, r + 5.0 * z, Stroke::new(1.5, colour));
    } else if hovered {
        painter.circle_filled(pos, r + 4.0 * z, colour.gamma_multiply(0.25));
    }
    let fill = if connected { colour } else { fade(ports.colours.surface_raised) };
    match port.role.dimension() {
        Dimension::Data => {
            painter.circle_filled(pos, r, fill);
            if !connected {
                painter.circle_stroke(pos, r - 0.75 * z, Stroke::new(1.5 * z, colour));
            }
        }
        // Settings ports are diamonds, of the type's colour. A connected one has a grey centre, the colour of
        // its edge, to tie the two together. A read-only control is only an outline: it can't fill in the setting.
        Dimension::Settings => {
            let d = r * 1.25;
            let fill = if read_only { fade(ports.colours.surface_raised) } else { fill };
            painter.add(Shape::convex_polygon(diamond(pos, d), fill, Stroke::new(1.5 * z, colour)));
            if connected {
                let centre = fade(wire_colour(Dimension::Settings, colour, ports.colours));
                painter.add(Shape::convex_polygon(diamond(pos, d * 0.45), centre, Stroke::NONE));
            }
        }
    }
}
