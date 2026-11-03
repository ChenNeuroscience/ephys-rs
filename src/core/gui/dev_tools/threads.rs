//! The dev tools' picture of the threads: each thread as a container, with links between them where they hand
//! things over.
//!
//! - The GUI (the main thread), which makes snapshots of the graph and shows nodes' latest displays.
//! - The pass dispatcher, sending a pass to the workers' queue every `PASS_DURATION`.
//! - The pass workers: one container, stacked to show there are several, holding the plan each pass runs
//!   through, step by step: each node with the buffers it reads and fills, where it waits for the pass before
//!   (to hand over its state: a sync point) and where it copies out what it shows (a copy point). Then the memory
//!   each worker has for the plan.
//! - Threads nodes run of their own (acquiring from a board, say), each drawn by its node (see
//!   `Node::thread_ui`), linked to the node's step: the link points the way data goes, and on it is the pool of
//!   buffers the thread swaps with the passes, if it has one (see `Node::thread_pool`).
//!
//! While running, it shows the plan being run, and what each worker is up to; otherwise, the plan of the latest
//! snapshot, as it would run.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2, vec2};

use crate::core::engine::{
    CopyTo, EdgeSet, Edges, Flow, Graph, NodeId, NodeSet, PASS_DURATION, Plan, PoolStatus, Resolved, RunInfo, Runner,
    Snapshot, Step, StepTime, ThreadContext,
};
use crate::core::gui::style;
use crate::core::theme::Colours;

type DataStream<N> = <<N as NodeSet>::Edges as Edges>::DataStream;

/// Room between the columns of threads, for the links between them.
const GAP: f32 = 96.0;
/// Room to the left of the threads, one above another, for the links between them.
const GUTTER: f32 = 60.0;
/// The width it takes to show the threads side by side.
const WIDE: f32 = 220.0 + 450.0 + 250.0 + 2.0 * GAP + 20.0;
const ROW: f32 = 20.0;

/// A node's thread, as drawn: its node, where, how and which way it hands data to the node's passes, and the pool of
/// buffers it swaps with them, if it does (with its buffers' streams' colours).
struct NodeThreadAnchor {
    node: NodeId,
    rect: Rect,
    link: &'static str,
    flow: Flow,
    pool: Option<(PoolStatus, Vec<Color32>)>,
}

/// Where things were drawn, to link them up once everything is.
#[derive(Default)]
struct Anchors {
    dispatcher: Option<Rect>,
    /// The workers' first step, taking a pass off the queue.
    queue: Option<Rect>,
    steps: HashMap<NodeId, Rect>,
    /// Nodes' displays, in the GUI's container.
    displays: Vec<(NodeId, Rect)>,
    /// Nodes' threads, and how each hands data to the node's passes.
    node_threads: Vec<NodeThreadAnchor>,
}

pub fn show<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    snapshot: &Snapshot<N>,
    workers: usize,
) {
    let info = runner.map(|r| r.inspect());
    let plan = info.as_ref().and_then(|i| i.running.as_ref()).map_or_else(|| snapshot.plan.clone(), |e| e.plan.clone());
    let mut anchors = Anchors::default();
    // Side by side if there's room, otherwise one above another, linked down the left.
    let wide = ui.available_width() >= WIDE;
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        let spacing = ui.spacing().item_spacing;
        // A column, with the usual spacing inside it.
        let column = |ui: &mut egui::Ui, width: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = spacing;
                ui.set_width(width);
                add(ui);
            });
        };
        let gui_and_dispatcher = |ui: &mut egui::Ui, anchors: &mut Anchors| {
            gui_thread(ui, graph, colours, &plan, anchors);
            ui.add_space(16.0);
            dispatcher(ui, colours, info.as_ref(), anchors);
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                column(ui, 220.0, &mut |ui| gui_and_dispatcher(ui, &mut anchors));
                column(ui, 450.0, &mut |ui| {
                    workers_ui(ui, graph, resolved, colours, info.as_ref(), snapshot, &plan, &mut anchors, workers)
                });
                column(ui, 250.0, &mut |ui| node_threads(ui, graph, colours, runner, &plan, &mut anchors));
            });
        } else {
            ui.horizontal_top(|ui| {
                ui.add_space(GUTTER);
                let width = (ui.available_width() - 12.0).clamp(320.0, 480.0);
                column(ui, width, &mut |ui| {
                    gui_and_dispatcher(ui, &mut anchors);
                    ui.add_space(16.0);
                    workers_ui(ui, graph, resolved, colours, info.as_ref(), snapshot, &plan, &mut anchors, workers);
                    ui.add_space(16.0);
                    node_threads(ui, graph, colours, runner, &plan, &mut anchors);
                });
            });
        }
        links(ui, colours, &anchors, wide);
    });
}

/// A thread: a framed box with a title, and `stacked` > 1 drawn as a stack of that many. Returns its rect.
fn container(
    ui: &mut egui::Ui,
    colours: &Colours,
    title: &str,
    detail: &str,
    stacked: usize,
    active: bool,
    add: impl FnOnce(&mut egui::Ui),
) -> Rect {
    let behind = ui.painter().add(Shape::Noop);
    let border = if active { colours.border } else { colours.divider };
    let rect = egui::Frame::new()
        .fill(colours.surface_raised)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let row = ui
                .horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    let fill = if active { colours.success } else { colours.text_disabled };
                    ui.painter().circle_filled(dot.center(), 4.0, fill);
                    text(ui, title, FontId::proportional(13.5), colours.text_primary);
                })
                .response
                .rect;
            let right = egui::pos2(ui.max_rect().right(), row.center().y);
            ui.painter().text(right, Align2::RIGHT_CENTER, detail, FontId::proportional(10.5), colours.text_secondary);
            ui.add_space(4.0);
            add(ui);
        })
        .response
        .rect;
    let copies = stacked.saturating_sub(1).min(2);
    let shapes = (1..=copies)
        .rev()
        .map(|k| {
            let offset = rect.translate(Vec2::splat(5.0 * k as f32));
            let stroke = Stroke::new(1.0, colours.divider);
            Shape::Rect(egui::epaint::RectShape::new(
                offset,
                6.0,
                colours.surface_raised,
                stroke,
                egui::StrokeKind::Inside,
            ))
        })
        .collect();
    ui.painter().set(behind, Shape::Vec(shapes));
    if copies > 0 {
        ui.add_space(5.0 * copies as f32);
    }
    rect
}

/// A node as on the graph: its label on its category's colour, `width` wide (or as wide as the label).
fn node_chip<N: NodeSet>(ui: &mut egui::Ui, graph: &Graph<N>, id: NodeId, width: Option<f32>) -> egui::Response {
    let (label, category) = match graph.get(id) {
        Some(entry) => (entry.label.clone(), entry.node.info().category),
        None => ("(removed)".to_string(), crate::core::engine::Category::Processing),
    };
    let font = FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(label, font, Color32::WHITE);
    let size = vec2(width.unwrap_or(galley.size().x + 12.0), 16.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, style::category_colour(category));
    painter.galley(rect.left_center() + vec2(6.0, -galley.size().y / 2.0), galley, Color32::WHITE);
    response
}

fn stream_info<N: NodeSet>(stream: &DataStream<N>) -> &'static crate::core::engine::EdgeInfo {
    &<DataStream<N> as EdgeSet>::KINDS[<DataStream<N> as EdgeSet>::kind_index(stream.kind())]
}

/// A buffer: a dot in its data stream's colour, and its number.
fn buffer_tag<N: NodeSet>(ui: &mut egui::Ui, plan: &Plan<N>, buffer: usize, colours: &Colours) -> egui::Response {
    let colour = plan.buffers().get(buffer).map_or(colours.text_disabled, |s| stream_info::<N>(s).colour);
    let text = format!("b{buffer}");
    let font = FontId::monospace(10.5);
    let galley = ui.painter().layout_no_wrap(text, font, colours.text_secondary);
    let (rect, response) = ui.allocate_exact_size(vec2(12.0 + galley.size().x, ROW), Sense::hover());
    ui.painter().circle_filled(egui::pos2(rect.left() + 4.0, rect.center().y), 4.0, colour);
    ui.painter().galley(
        egui::pos2(rect.left() + 11.0, rect.center().y - galley.size().y / 2.0),
        galley,
        colours.text_secondary,
    );
    response
}

#[derive(Clone, Copy)]
enum Glyph {
    /// A sync point: waiting for another thread.
    Sync,
    /// A copy point: data copied out for another thread.
    Copy,
    /// Made in place, in its input's buffer.
    InPlace,
    /// A queue between threads.
    Queue,
}

fn glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, colour: Color32) {
    let s = rect.width() / 12.0;
    let p = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let line = Stroke::new(1.2 * s, colour);
    match glyph {
        Glyph::Sync => {
            // An hourglass.
            painter.add(Shape::convex_polygon(vec![p(2.0, 1.0), p(10.0, 1.0), p(6.0, 6.0)], colour, Stroke::NONE));
            painter.add(Shape::convex_polygon(
                vec![p(6.0, 6.0), p(10.0, 11.0), p(2.0, 11.0)],
                Color32::TRANSPARENT,
                line,
            ));
        }
        Glyph::Copy => {
            painter.rect_stroke(Rect::from_min_max(p(1.0, 1.0), p(8.0, 8.0)), 1.0, line, egui::StrokeKind::Middle);
            painter.rect_filled(Rect::from_min_max(p(4.0, 4.0), p(11.0, 11.0)), 1.0, colour);
        }
        Glyph::InPlace => {
            // Round and back to where it started.
            let centre = p(6.0, 6.0);
            let arc: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.3 + 1.5 * i as f32 / 12.0);
                    centre + vec2(a.cos(), -a.sin()) * 4.0 * s
                })
                .collect();
            let end = *arc.last().unwrap();
            painter.add(Shape::line(arc, line));
            let tip = vec![end + vec2(-2.5, -1.0) * s, end + vec2(2.5, -1.0) * s, end + vec2(0.0, 2.5) * s];
            painter.add(Shape::convex_polygon(tip, colour, Stroke::NONE));
        }
        Glyph::Queue => {
            for x in [1.0, 4.5, 8.0] {
                painter.rect_filled(Rect::from_min_max(p(x, 3.0), p(x + 2.5, 9.0)), 0.5, colour);
            }
        }
    }
}

/// A glyph with a tooltip. Returns where it is.
fn badge(ui: &mut egui::Ui, which: Glyph, colour: Color32, tooltip: &str) -> Rect {
    let (rect, response) = ui.allocate_exact_size(vec2(14.0, ROW), Sense::hover());
    glyph(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(11.0)), which, colour);
    response.on_hover_text(tooltip);
    rect
}

/// Text just as wide as it is (a label in a row here can be given the rest of the row, and centred in it).
fn text(ui: &mut egui::Ui, text: &str, font: FontId, colour: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, colour);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, colour);
    response
}

fn small(text: impl Into<String>, colour: Color32) -> RichText {
    RichText::new(text).size(11.0).color(colour)
}

/// A short name for a plan, from where it is in memory: the same plan, the same name.
fn plan_id<N: NodeSet>(plan: &Arc<Plan<N>>) -> String {
    format!("@{:06x}", Arc::as_ptr(plan) as usize & 0xff_ffff)
}

fn megabytes(bytes: usize) -> String {
    if bytes >= 100_000 { format!("{:.1} MB", bytes as f64 / 1e6) } else { format!("{:.1} kB", bytes as f64 / 1e3) }
}

fn gui_thread<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    colours: &Colours,
    plan: &Plan<N>,
    anchors: &mut Anchors,
) {
    // The windows it shows the latest copied for.
    container(ui, colours, "GUI", "main thread", 1, true, |ui| {
        let shown: Vec<NodeId> =
            plan.steps().iter().filter(|s| s.copies == Some(CopyTo::Window)).map(|s| s.id).collect();
        if shown.is_empty() {
            return;
        }
        for id in shown {
            let row = ui
                .horizontal(|ui| {
                    ui.set_height(ROW);
                    node_chip(ui, graph, id, None);
                })
                .response;
            anchors.displays.push((id, row.rect.with_max_x(ui.max_rect().right())));
        }
    });
}

fn dispatcher<N: NodeSet>(ui: &mut egui::Ui, colours: &Colours, info: Option<&RunInfo<N>>, anchors: &mut Anchors) {
    let period = format!("every {:.0} ms", PASS_DURATION * 1000.0);
    let rect = container(ui, colours, "Pass dispatcher", &period, 1, info.is_some(), |ui| {
        let Some(info) = info else {
            ui.label(small("Stopped. Press Play to run.", colours.text_secondary));
            return;
        };
        ui.label(small(format!("Next pass p{}", info.next_pass), colours.text_primary));
        let queued = info.running.as_ref().map_or(0, |e| e.queued);
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(small(format!("Queued {queued} of {}", info.queue_capacity), colours.text_secondary))
                    .extend(),
            );
            let (bar, _) = ui.allocate_exact_size(vec2(60.0, 6.0), Sense::hover());
            ui.painter().rect_filled(bar, 2.0, colours.surface_sunken);
            let filled = queued as f32 / info.queue_capacity.max(1) as f32;
            let fill = if filled >= 1.0 { colours.warning } else { colours.accent };
            ui.painter().rect_filled(bar.with_max_x(bar.left() + bar.width() * filled.min(1.0)), 2.0, fill);
        });
        let dropped = info.stats.dropped;
        let colour = if dropped > 0 { colours.warning } else { colours.text_secondary };
        ui.label(small(format!("{dropped} dropped (queue full)"), colour));
    });
    anchors.dispatcher = Some(rect);
}

#[allow(clippy::too_many_arguments)]
fn workers_ui<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    info: Option<&RunInfo<N>>,
    snapshot: &Snapshot<N>,
    plan: &Arc<Plan<N>>,
    anchors: &mut Anchors,
    workers: usize,
) {
    let count = info.map_or(workers, |i| i.workers.len());
    let title = format!("Pass workers × {count}");
    let running = info.and_then(|i| i.running.as_ref());
    let detail = match running {
        Some(epoch) => format!("epoch {} · plan {} · version {}", epoch.number, plan_id(plan), epoch.version),
        None => format!("plan {} · version {}", plan_id(plan), snapshot.version),
    };
    container(ui, colours, &title, &detail, count, info.is_some(), |ui| {
        match info {
            Some(info) => {
                worker_chips(ui, colours, info);
            }
            None => {
                ui.label(small(
                    "Not running: the plan for the latest snapshot, as it would run.",
                    colours.text_secondary,
                ));
            }
        }
        if let Some(preparing) = info.and_then(|i| i.preparing.as_ref()) {
            let text = format!(
                "Getting ready for epoch {} (plan {}, version {}): {} of {count} workers have buffers",
                preparing.number,
                plan_id(&preparing.plan),
                preparing.version,
                preparing.ready
            );
            ui.label(small(text, colours.warning));
        }
        ui.add_space(6.0);
        let times = running.map(|epoch| &epoch.step_times[..]);
        let each_pass = match times {
            Some(times) => {
                let work: f64 = times.iter().map(|t| t.work_us).sum();
                format!("Each pass: {:.2} ms of work, on average", work / 1000.0)
            }
            None => "Each pass".to_owned(),
        };
        ui.label(small(each_pass, colours.text_secondary));
        steps(ui, graph, resolved, colours, plan, times, anchors);
        not_running(ui, graph, colours, plan);
        ui.add_space(6.0);
        memory(ui, graph, colours, plan, info);
        ui.add_space(4.0);
        legend(ui, colours);
    });
}

/// What each worker is doing: the pass it's running, idle, or still on an older epoch.
fn worker_chips<N: NodeSet>(ui: &mut egui::Ui, colours: &Colours, info: &RunInfo<N>) {
    let running = info.running.as_ref().map(|e| e.number);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (index, worker) in info.workers.iter().enumerate() {
            let (text, fill, ink) = match (worker.pass, worker.epoch) {
                (_, None) => (format!("{index} making buffers"), colours.control, colours.text_secondary),
                (Some(pass), Some(epoch)) if Some(epoch) == running => {
                    (format!("{index} p{pass}"), colours.accent, colours.on_accent)
                }
                (Some(pass), Some(epoch)) => (format!("{index} e{epoch}·p{pass}"), colours.warning, colours.on_accent),
                (None, Some(epoch)) if Some(epoch) == running => {
                    (format!("{index} idle"), colours.control, colours.text_primary)
                }
                (None, Some(epoch)) => (format!("{index} on e{epoch}"), colours.control, colours.warning),
            };
            let font = FontId::monospace(10.5);
            // All as wide as the widest a chip gets, so they don't change size as workers take passes.
            let widest = ui.painter().layout_no_wrap(format!("{index} making buffers"), font.clone(), ink).size();
            let galley = ui.painter().layout_no_wrap(text, font, ink);
            let size = vec2(galley.size().x.max(widest.x), galley.size().y) + vec2(10.0, 4.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect_filled(rect, 3.0, fill);
            ui.painter().galley(rect.min + vec2(5.0, 2.0), galley, ink);
            response.on_hover_text(format!(
                "Worker {index}: {} passes run, {} of buffers",
                worker.passes,
                megabytes(worker.memory)
            ));
        }
    });
}

/// The plan, a step to a row, between taking the pass off the queue and finishing it.
fn steps<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    plan: &Plan<N>,
    times: Option<&[StepTime]>,
    anchors: &mut Anchors,
) {
    let queue = ui
        .horizontal(|ui| {
            ui.set_height(ROW);
            badge(ui, Glyph::Queue, colours.text_secondary, "The pass's turn comes: a worker takes it off the queue");
            text(ui, "take the next pass off the queue", FontId::proportional(11.0), colours.text_secondary);
        })
        .response
        .rect;
    anchors.queue = Some(queue);
    for (index, step) in plan.steps().iter().enumerate() {
        let time = times.and_then(|t| t.get(index)).copied();
        let row = ui.horizontal(|ui| step_row(ui, graph, resolved, colours, plan, index, step, time)).response;
        anchors.steps.insert(step.id, row.rect.with_max_x(ui.max_rect().right()));
    }
    if plan.steps().is_empty() {
        ui.label(small("Nothing runs: no node has all its outputs known.", colours.text_secondary));
    }
}

#[allow(clippy::too_many_arguments)]
fn step_row<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    plan: &Plan<N>,
    index: usize,
    step: &Step,
    time: Option<StepTime>,
) {
    ui.set_height(ROW);
    ui.spacing_mut().item_spacing.x = 4.0;
    ui.label(RichText::new(format!("{:>2}", index + 1)).monospace().size(10.5).color(colours.text_disabled));
    node_chip(ui, graph, step.id, Some(130.0));
    let state = resolved.get(&step.id);
    let input_names: Vec<&str> = state.map(|s| s.inputs.iter().map(|i| i.name).collect()).unwrap_or_default();
    let output_names: Vec<&str> = state.map(|s| s.outputs.iter().map(|o| o.name).collect()).unwrap_or_default();
    for (i, buffer) in step.inputs.iter().enumerate() {
        let name = input_names.get(i).copied().unwrap_or("input");
        match buffer {
            Some(b) if step.takes.get(i) == Some(&true) => {
                let text = format!(
                    "Takes buffer {b}, with {name} in it, being the last to read it: swaps it for one of its own, so \
                     nothing's copied"
                );
                buffer_tag(ui, plan, *b, colours).on_hover_text(&text);
                badge(ui, Glyph::InPlace, colours.success, &text);
            }
            Some(b) => {
                buffer_tag(ui, plan, *b, colours).on_hover_text(format!("Reads {name} from buffer {b}"));
            }
            // Made in place: shown with the output.
            None if step.in_place.contains(&Some(i)) => {}
            None => {
                ui.add(egui::Label::new(small("–", colours.text_disabled)).extend())
                    .on_hover_text(format!("{name}: not connected"));
            }
        }
    }
    let (arrow, _) = ui.allocate_exact_size(vec2(12.0, ROW), Sense::hover());
    let (y, stroke) = (arrow.center().y, Stroke::new(1.0, colours.text_secondary));
    ui.painter().line_segment([egui::pos2(arrow.left() + 1.0, y), egui::pos2(arrow.right() - 1.0, y)], stroke);
    let tip = egui::pos2(arrow.right() - 1.0, y);
    ui.painter().add(Shape::line(vec![tip + vec2(-3.5, -3.0), tip, tip + vec2(-3.5, 3.0)], stroke));
    for (o, &b) in step.outputs.iter().enumerate() {
        let name = output_names.get(o).copied().unwrap_or("output");
        let stream = plan.buffers().get(b).map(|s| s.summary()).unwrap_or_default();
        match step.in_place[o] {
            Some(input) => {
                let input = input_names.get(input).copied().unwrap_or("input");
                let text = format!(
                    "Makes {name} ({stream}) in place: edits buffer {b}, where {input} was, so nothing's copied"
                );
                buffer_tag(ui, plan, b, colours).on_hover_text(&text);
                badge(ui, Glyph::InPlace, colours.success, &text);
            }
            None => {
                buffer_tag(ui, plan, b, colours).on_hover_text(format!("Fills {name} ({stream}) in buffer {b}"));
            }
        }
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        match step.copies {
            Some(CopyTo::Window) => {
                badge(
                    ui,
                    Glyph::Copy,
                    colours.accent,
                    "Copy point: only copies its inputs out, to show in its window. It makes the copy itself, so \
                     runs as early as it can",
                );
            }
            Some(CopyTo::Thread) => {
                badge(
                    ui,
                    Glyph::Copy,
                    colours.accent,
                    "Copy point: only copies its inputs out, to a thread of its own. It makes the copy itself, so \
                     runs as early as it can",
                );
            }
            None => {}
        }
        if step.state.is_some() {
            let why = graph.get(step.id).map_or("", |e| e.node.state_reason());
            let text = format!(
                "Sync point: waits for the pass before to finish with the node's state, then hands it on to the \
                 next.\n\nWhy it has state: {why}"
            );
            badge(ui, Glyph::Sync, colours.warning, &text);
        }
        if let Some(time) = time {
            let mut text = format!("{:.0} µs", time.work_us);
            if time.wait_us >= 10.0 {
                text += &format!(" +{:.0} waiting", time.wait_us);
            }
            let tip = format!(
                "On average, over its last few dozen passes: {:.0} µs working{}",
                time.work_us,
                if step.state.is_some() {
                    format!(", and {:.0} µs waiting for the pass before to hand over the node's state", time.wait_us)
                } else {
                    String::new()
                }
            );
            ui.label(RichText::new(text).monospace().size(10.5).color(colours.text_secondary)).on_hover_text(tip);
        }
    });
}

/// Nodes that don't run, and why.
fn not_running<N: NodeSet>(ui: &mut egui::Ui, graph: &Graph<N>, colours: &Colours, plan: &Plan<N>) {
    let idle: Vec<NodeId> =
        graph.iter().map(|(id, _)| id).filter(|id| !plan.steps().iter().any(|s| s.id == *id)).collect();
    if idle.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.label(small(
        "Not in the plan: some output can't be known (an input not connected?), or nothing needs its outputs now",
        colours.text_secondary,
    ));
    ui.horizontal_wrapped(|ui| {
        ui.multiply_opacity(0.5);
        for id in idle {
            node_chip(ui, graph, id, None);
        }
    });
}

/// Each buffer a worker has for the plan, what it holds and its size, then the totals.
fn memory<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    colours: &Colours,
    plan: &Plan<N>,
    info: Option<&RunInfo<N>>,
) {
    let bytes = plan.buffer_bytes();
    let total: usize = bytes.iter().sum();
    egui::CollapsingHeader::new(small(format!("Memory: {} a worker", megabytes(total)), colours.text_primary))
        .id_salt("dev tools memory")
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("dev tools buffers").num_columns(4).spacing(vec2(12.0, 2.0)).striped(true).show(ui, |ui| {
                for heading in ["buffer", "holds", "filled by", "size"] {
                    ui.add(egui::Label::new(small(heading, colours.text_disabled)).extend());
                }
                ui.end_row();
                for (b, stream) in plan.buffers().iter().enumerate() {
                    buffer_tag(ui, plan, b, colours);
                    ui.label(small(
                        format!("{} · {}", stream_info::<N>(stream).name, stream.summary()),
                        colours.text_primary,
                    ));
                    // The first step to fill it, and any making outputs in place in it after.
                    let fillers: Vec<NodeId> =
                        plan.steps().iter().filter(|s| s.outputs.contains(&b)).map(|s| s.id).collect();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for id in fillers {
                            node_chip(ui, graph, id, None);
                        }
                    });
                    ui.add(egui::Label::new(small(megabytes(bytes[b]), colours.text_secondary)).extend());
                    ui.end_row();
                }
            });
            let states = plan.steps().iter().filter(|s| s.state.is_some()).count();
            let working = plan.steps().iter().filter(|s| s.memory.is_some()).count();
            let copying = plan.steps().iter().filter(|s| s.copies.is_some()).count();
            ui.label(small(
                format!(
                    "Also: {states} node states (one each, handed from pass to pass), {working} nodes' working \
                     memory (each worker its own, so no waiting), and whatever the {copying} nodes copying out keep \
                     to copy into."
                ),
                colours.text_secondary,
            ));
            if let Some(info) = info {
                let all: usize = info.workers.iter().map(|w| w.memory).sum();
                ui.label(small(
                    format!("All workers: {} (while moving to a new plan, old buffers and new)", megabytes(all)),
                    colours.text_secondary,
                ));
            }
        })
        .header_response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
}

/// Halfway between two points.
fn pos2_between(a: Pos2, b: Pos2) -> Pos2 {
    a + (b - a) * 0.5
}

/// A pool of buffers a node's thread swaps with the passes (see `PoolStatus`), as a box at `at`: each buffer's
/// streams (dots in their colours, as the workers' buffers are shown), its size and how many; then a square for each
/// buffer, full ones first, then the one being filled (or written), then the empty ones (the rest being the
/// workers', for now). Hover for the counts.
fn pool_ui(ui: &egui::Ui, colours: &Colours, pool: PoolStatus, dots: &[Color32], at: (Pos2, Align2), id: egui::Id) {
    const ACROSS: usize = 10;
    const SQUARE: f32 = 5.0;
    const STEP: f32 = 6.0;
    let painter = ui.painter();
    let text = format!("{} × {}", megabytes(pool.bytes), pool.size);
    let galley = painter.layout_no_wrap(text, FontId::monospace(9.5), colours.text_secondary);
    let dots_width = dots.len() as f32 * 8.0;
    let grid = vec2(ACROSS as f32 * STEP - 1.0, pool.size.div_ceil(ACROSS).max(1) as f32 * STEP - 1.0);
    let inner = vec2((dots_width + 3.0 + galley.size().x).max(grid.x), galley.size().y + 4.0 + grid.y);
    let rect = at.1.anchor_size(at.0, inner + vec2(10.0, 10.0));
    painter.rect(rect, 4.0, colours.surface_raised, Stroke::new(1.0, colours.border), egui::StrokeKind::Inside);
    let top = rect.min + vec2(5.0, 5.0);
    for (i, colour) in dots.iter().enumerate() {
        painter.circle_filled(top + vec2(3.5 + i as f32 * 8.0, galley.size().y / 2.0), 3.5, *colour);
    }
    painter.galley(top + vec2(dots_width + 3.0, 0.0), galley.clone(), colours.text_secondary);
    let grid_top = top + vec2(0.0, galley.size().y + 4.0);
    let filling = pool.filling as usize;
    for i in 0..pool.size {
        let min = grid_top + vec2((i % ACROSS) as f32 * STEP, (i / ACROSS) as f32 * STEP);
        let square = Rect::from_min_size(min, Vec2::splat(SQUARE));
        if i < pool.full {
            painter.rect_filled(square, 1.0, colours.accent);
        } else if i < pool.full + filling {
            painter.rect_filled(square, 1.0, colours.warning);
        } else if i < pool.full + filling + pool.empty {
            painter.rect_stroke(square, 1.0, Stroke::new(1.0, colours.text_disabled), egui::StrokeKind::Inside);
        }
    }
    let mut tip = format!(
        "A pool of {} buffers of {} each ({} in all), swapped with the passes rather than copied: {} full, waiting{}, \
         {} empty; the rest with the workers, for now. {} swaps so far.",
        pool.size,
        megabytes(pool.bytes),
        megabytes(pool.bytes * pool.size),
        pool.full,
        if pool.filling { ", 1 being filled (or written)" } else { "" },
        pool.empty,
        pool.swaps
    );
    if pool.overflows > 0 {
        tip += &format!("\n\nRan out {} times: data was lost.", pool.overflows);
        painter.rect_stroke(rect, 4.0, Stroke::new(1.5, colours.error), egui::StrokeKind::Inside);
    }
    let _ = ui.interact(rect, id, Sense::hover()).on_hover_text(tip);
}

fn legend(ui: &mut egui::Ui, colours: &Colours) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (which, colour, text) in [
            (Glyph::Sync, colours.warning, "sync point"),
            (Glyph::Copy, colours.accent, "copy point"),
            (Glyph::InPlace, colours.success, "made in place"),
            (Glyph::Queue, colours.text_secondary, "queue"),
        ] {
            badge(ui, which, colour, text);
            ui.add(egui::Label::new(small(text, colours.text_secondary)).extend());
            ui.add_space(8.0);
        }
    });
}

fn node_threads<N: NodeSet>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    plan: &Plan<N>,
    anchors: &mut Anchors,
) {
    let mut any = false;
    for step in plan.steps() {
        let Some(entry) = graph.get(step.id) else { continue };
        for (index, thread) in entry.node.threads().iter().enumerate() {
            any = true;
            let mut draw = |ui: &mut egui::Ui, display: Option<&(dyn std::any::Any + Send)>| {
                // The pool's buffers are like the step's: its outputs' for a source, its inputs' for a writer.
                let ports: Vec<Option<usize>> = match thread.flow {
                    Flow::ToPasses => step.outputs.iter().map(|&b| Some(b)).collect(),
                    Flow::FromPasses => step.inputs.clone(),
                };
                let pool = entry.node.thread_pool(index, &ThreadContext { theme: colours, display });
                let pool = pool.filter(|p| p.size > 0).map(|p| {
                    let pooled = ports.iter().enumerate().filter(|(i, _)| *i < 64 && p.ports & (1 << i) != 0);
                    let colour =
                        |b: usize| plan.buffers().get(b).map_or(colours.text_disabled, |s| stream_info::<N>(s).colour);
                    (p, pooled.filter_map(|(_, b)| b.map(colour)).collect())
                });
                let rect = container(ui, colours, thread.name, "", 1, runner.is_some(), |ui| {
                    node_chip(ui, graph, step.id, None);
                    ui.add_space(2.0);
                    entry.node.thread_ui(index, ui, &ThreadContext { theme: colours, display });
                    ui.add_space(2.0);
                    ui.label(small(thread.why, colours.text_disabled));
                });
                anchors.node_threads.push(NodeThreadAnchor {
                    node: step.id,
                    rect,
                    link: thread.link,
                    flow: thread.flow,
                    pool,
                });
                ui.add_space(12.0);
            };
            let shown = runner.and_then(|r| r.exchange().with_latest(step.id, |d| draw(ui, Some(d))));
            if shown.is_none() {
                draw(ui, None);
            }
        }
    }
    if !any {
        ui.label(small("No node runs threads of its own.", colours.text_secondary));
    }
}

/// Curves from one thread to another where they hand things over, each kind labelled once, by where it starts.
/// Side by side, they cross between the columns; one above another, they loop round the left, in the gutter.
fn links(ui: &egui::Ui, colours: &Colours, anchors: &Anchors, wide: bool) {
    let painter = ui.painter();
    let colour = colours.text_secondary.gamma_multiply(0.7);
    let label = |at: Pos2, align: Align2, text: &str| {
        let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(10.0), colours.text_secondary);
        let rect = align.anchor_size(at, galley.size()).expand2(vec2(3.0, 1.0));
        painter.rect_filled(rect, 3.0, colours.surface);
        painter.galley(rect.min + vec2(3.0, 1.0), galley, colours.text_secondary);
    };
    let arrow = |to: Pos2, rightwards: bool| {
        let back = if rightwards { -1.0 } else { 1.0 };
        let tip = vec![to, to + vec2(6.0 * back, -3.5), to + vec2(6.0 * back, 3.5)];
        painter.add(Shape::convex_polygon(tip, colour, Stroke::NONE));
    };
    let curve = |points: [Pos2; 4]| {
        let stroke = Stroke::new(1.2, colour);
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, stroke));
    };
    // Across from one column to another.
    let across = |from: Pos2, to: Pos2| {
        let bend = ((to.x - from.x).abs() * 0.5).max(30.0);
        let rightwards = to.x >= from.x;
        let dx = if rightwards { bend } else { -bend };
        curve([from, from + vec2(dx, 0.0), to - vec2(dx, 0.0), to]);
        arrow(to, rightwards);
    };
    // From one left edge round to another, `depth` out into the gutter.
    let round = |from: Pos2, to: Pos2, depth: f32| {
        curve([from, from - vec2(depth * 1.3, 0.0), to - vec2(depth * 1.3, 0.0), to]);
        arrow(to, true);
    };

    let ends = |rect: Rect| (rect.left_center() - vec2(4.0, 0.0), rect.right_center() + vec2(4.0, 0.0));
    let mut depth = 14.0;
    let mut deeper = || {
        depth = if depth >= GUTTER - 8.0 { 14.0 } else { depth + 9.0 };
        depth
    };
    if let (Some(dispatcher), Some(queue)) = (anchors.dispatcher, anchors.queue) {
        let (from, to) =
            if wide { (dispatcher.right_center(), ends(queue).0) } else { (dispatcher.left_center(), ends(queue).0) };
        if wide {
            across(from, to);
            label(from + vec2(6.0, -4.0), Align2::LEFT_BOTTOM, "pass queue");
        } else {
            round(from, to, deeper());
            label(from - vec2(6.0, 10.0), Align2::RIGHT_CENTER, "pass queue");
        }
    }
    for (index, (id, display)) in anchors.displays.iter().enumerate() {
        let Some(step) = anchors.steps.get(id) else { continue };
        if wide {
            let to = ends(*display).1;
            across(ends(*step).0, to);
            if index == 0 {
                label(to + vec2(8.0, -6.0), Align2::LEFT_BOTTOM, "latest copy");
            }
        } else {
            round(ends(*step).0, ends(*display).0, deeper());
        }
    }
    // Pointing the way the data goes: to the passes from a board, say, or from them to a writer.
    for (index, NodeThreadAnchor { node: id, rect: thread, link: how, flow, pool }) in
        anchors.node_threads.iter().enumerate()
    {
        let Some(step) = anchors.steps.get(id) else { continue };
        let to_passes = *flow == Flow::ToPasses;
        // The pool's buffers, between the thread and the passes: below the link, or inside the thread's top right.
        let pool_at = if wide {
            let (at_thread, at_step) = (thread.left_center(), ends(*step).1);
            if to_passes {
                across(at_thread, at_step)
            } else {
                across(at_step, at_thread)
            }
            label(at_thread + vec2(-6.0, -4.0), Align2::RIGHT_BOTTOM, how);
            (pos2_between(at_step, at_thread), Align2::CENTER_CENTER)
        } else {
            let (at_thread, at_step, depth) = (thread.left_center(), ends(*step).0, deeper());
            if to_passes {
                round(at_thread, at_step, depth)
            } else {
                round(at_step, at_thread, depth)
            }
            label(thread.left_top() + vec2(10.0, -2.0), Align2::LEFT_BOTTOM, how);
            (thread.right_top() + vec2(-10.0, 10.0), Align2::RIGHT_TOP)
        };
        if let Some((pool, dots)) = pool {
            pool_ui(ui, colours, *pool, dots, pool_at, ui.scope_id().with(("pool", index)));
        }
    }
}
