//! The dev tools' picture of the threads: each thread as a container, with links between them where they hand
//! things over. It reads left to right, as the graph does, the way data goes: what starts passes and brings data in, then
//! the passes, then what the data goes to.
//!
//! - The GUI (the main thread), which makes snapshots of the graph, and shows what nodes' passes share with their
//!   windows: each pool of buffers they share (see `Shared::pool`), linked from the node's step.
//! - The pass dispatcher, sending a pass to the workers' queue every `PASS_DURATION`, with the queue in it.
//! - The pass workers: one container, stacked to show there are several, holding the plan each pass runs
//!   through, step by step: each node with the buffers it reads and fills, where it waits for the pass before
//!   (to hand over its state: a sync point), which buffers it takes (to swap away) and where the plan copies data for
//!   a node taking it that can't be the last to read it (a copy point). Then the memory each worker has for the plan.
//! - Threads nodes run of their own (acquiring from a board, say), each drawn by its node (see
//!   `Node::thread_ui`), linked to the node's step: the link points the way data goes. In it is the pool of buffers
//!   the thread swaps with the passes, if it has one (see `Node::thread_pool`).
//!
//! - Short-lived helpers, out of the way below the rest, which do what the threads above mustn't wait for.
//!
//! A pool's always shown the same way (`pool_row`), in the thread at the other end from the passes. Each thread's info
//! icon says why it's a thread.
//!
//! While running, it shows the plan being run, and what each worker is up to; otherwise, the plan of the latest
//! snapshot, as it would run.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2, vec2};

use crate::core::engine::{
    EdgeTypesEnum, Edges, Flow, Graph, NodeId, NodesEnum, PASS_DURATION, Plan, PoolStatus, Resolved, RunInfo, Runner,
    Snapshot, Step, StepTime, ThreadContext,
};
use crate::core::gui::style;
use crate::core::theme::Colours;

type DataStream<N> = <<N as NodesEnum>::Edges as Edges>::DataStream;

/// Room between the columns of threads, for the links between them.
const GAP: f32 = 80.0;
/// Room to the left of the threads, one above another, for the links between them.
const GUTTER: f32 = 60.0;
/// The width it takes to show the threads side by side.
const WIDE: f32 = 250.0 + 450.0 + 250.0 + 2.0 * GAP + 20.0;
const ROW: f32 = 20.0;

const WHY_GUI: &str = "The main thread, as windows have to be drawn from one thread. It never waits on the passes, nor \
                       they on it, so a slow frame can't lose data, and slow passes can't freeze the window.";
const WHY_DISPATCHER: &str = "On a thread of its own so passes start on time, whatever the workers are doing: all it \
                              does is wait until each is due, and queue it.";
const WHY_WORKERS: &str = "Several, so a pass can start while the one before is still running: one slow pass doesn't \
                           hold up the next, and the work's spread over the cores. Each runs whole passes.";

/// Helpers that come and go, and why each is a thread.
const HELPERS: [(&str, &str); 4] = [
    (
        "Measuring the disk",
        "Finds how much room's left, for the toolbar, now and then. On a thread of its own as a disk can take a while \
         to answer, and the GUI mustn't wait for it.",
    ),
    (
        "Dropping node state",
        "Lets go of what an old epoch's nodes held, closing their files and stopping their capture threads. On a \
         thread of its own as that can take a while, and passes and the GUI mustn't wait for it.",
    ),
    (
        "Preparing to record",
        "Opens a recording's files and so on before it starts. On a thread of its own so the GUI carries on meanwhile, \
         and the first pass recording doesn't have to wait for the disk.",
    ),
    (
        "Saving settings with a recording",
        "Writes the graph beside a recording's files. On a thread of its own so the disk can't hold up the GUI.",
    ),
];

/// A node's thread, as drawn: its node, where, how and which way it hands data to the node's passes, and whether it
/// swaps buffers with them through a pool.
struct NodeThreadAnchor {
    node: NodeId,
    rect: Rect,
    link: &'static str,
    flow: Flow,
    pooled: bool,
}

/// Where things were drawn, to link them up once everything is.
#[derive(Default)]
struct Anchors {
    dispatcher: Option<Rect>,
    /// The workers' first step, taking a pass off the queue.
    queue: Option<Rect>,
    steps: HashMap<NodeId, Rect>,
    /// Pools nodes share with their windows, in the GUI's container: the node, and where.
    displays: Vec<(NodeId, Rect)>,
    /// Nodes' threads, and how each hands data to the node's passes.
    node_threads: Vec<NodeThreadAnchor>,
}

pub fn show<N: NodesEnum>(
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
        // Where data comes from, and what starts passes.
        let before_passes = |ui: &mut egui::Ui, anchors: &mut Anchors| {
            dispatcher(ui, colours, info.as_ref(), anchors);
            ui.add_space(16.0);
            node_threads(ui, graph, colours, runner, &plan, Flow::ToPasses, anchors);
        };
        // Where it goes.
        let after_passes = |ui: &mut egui::Ui, anchors: &mut Anchors| {
            gui_thread(ui, graph, colours, runner, &plan, anchors);
            ui.add_space(16.0);
            node_threads(ui, graph, colours, runner, &plan, Flow::FromPasses, anchors);
            ui.add_space(4.0);
            helpers(ui, colours);
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                column(ui, 250.0, &mut |ui| before_passes(ui, &mut anchors));
                column(ui, 450.0, &mut |ui| {
                    workers_ui(ui, graph, resolved, colours, info.as_ref(), snapshot, &plan, &mut anchors, workers)
                });
                column(ui, 250.0, &mut |ui| after_passes(ui, &mut anchors));
            });
        } else {
            ui.horizontal_top(|ui| {
                ui.add_space(GUTTER);
                let width = (ui.available_width() - 12.0).clamp(320.0, 480.0);
                column(ui, width, &mut |ui| {
                    before_passes(ui, &mut anchors);
                    ui.add_space(4.0);
                    workers_ui(ui, graph, resolved, colours, info.as_ref(), snapshot, &plan, &mut anchors, workers);
                    ui.add_space(16.0);
                    after_passes(ui, &mut anchors);
                });
            });
        }
        links(ui, colours, &anchors, wide);
    });
}

/// An icon saying `why`, as help does (see `widgets::with_help`).
fn info_icon(ui: &mut egui::Ui, colours: &Colours, why: &str) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, ui.is_enabled(), "Why"));
    let colour = if response.hovered() { colours.text_primary } else { colours.text_disabled };
    ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(1.0, colour));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "i", FontId::proportional(9.0), colour);
    crate::core::gui::widgets::with_help(response, |ui| {
        ui.label(why);
    });
}

/// A thread: a framed box with a title, saying `why` it's a thread, and `stacked` > 1 drawn as a stack of that many.
/// Returns its rect.
#[allow(clippy::too_many_arguments)]
fn container(
    ui: &mut egui::Ui,
    colours: &Colours,
    title: &str,
    why: &str,
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
                    info_icon(ui, colours, why);
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
fn node_chip<N: NodesEnum>(ui: &mut egui::Ui, graph: &Graph<N>, id: NodeId, width: Option<f32>) -> egui::Response {
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

fn stream_info<N: NodesEnum>(stream: &DataStream<N>) -> &'static crate::core::engine::EdgeInfo {
    &<DataStream<N> as EdgeTypesEnum>::KINDS[<DataStream<N> as EdgeTypesEnum>::kind_index(stream.kind())]
}

/// A buffer: a dot in its data stream's colour, and its number.
fn buffer_tag<N: NodesEnum>(ui: &mut egui::Ui, plan: &Plan<N>, buffer: usize, colours: &Colours) -> egui::Response {
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

/// A small picture beside a step, for what happens there between threads or buffers.
#[derive(Clone, Copy)]
enum Glyph {
    /// A sync point: waiting for another thread.
    Sync,
    /// A copy point: data the plan copies, for a node taking it that can't be the last to read it.
    Copy,
    /// Made in place, in its input's buffer.
    InPlace,
    /// A queue between threads.
    Queue,
    /// Buffers swapped with another thread's, rather than copied.
    Swap,
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
        Glyph::Swap => {
            // Two arrows, each way.
            painter.line_segment([p(1.0, 4.0), p(10.0, 4.0)], line);
            painter.add(Shape::convex_polygon(vec![p(11.5, 4.0), p(8.5, 1.8), p(8.5, 6.2)], colour, Stroke::NONE));
            painter.line_segment([p(11.0, 8.5), p(2.0, 8.5)], line);
            painter.add(Shape::convex_polygon(vec![p(0.5, 8.5), p(3.5, 6.3), p(3.5, 10.7)], colour, Stroke::NONE));
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
fn plan_id<N: NodesEnum>(plan: &Arc<Plan<N>>) -> String {
    // Where it is in memory changes from run to run, which snapshots can't have.
    let address = if cfg!(test) { 0 } else { Arc::as_ptr(plan) as usize & 0xff_ffff };
    format!("@{address:06x}")
}

fn megabytes(bytes: usize) -> String {
    if bytes >= 100_000 { format!("{:.1} MB", bytes as f64 / 1e6) } else { format!("{:.1} kB", bytes as f64 / 1e3) }
}

fn gui_thread<N: NodesEnum>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    plan: &Plan<N>,
    anchors: &mut Anchors,
) {
    // The nodes sharing pools of buffers with their windows (the latest to show, say), while running: each with
    // where its buffers are.
    container(ui, colours, "GUI", WHY_GUI, "main thread", 1, true, |ui| {
        let Some(exchange) = runner.map(|r| r.exchange()) else {
            ui.label(small("Shows what nodes' passes share with their windows, while running.", colours.text_disabled));
            return;
        };
        for step in plan.steps() {
            for (name, pool) in exchange.pools(step.id) {
                let row = pool_row(ui, colours, window_pool(pool), |ui| {
                    node_chip(ui, graph, step.id, None);
                    ui.label(small(name, colours.text_secondary));
                });
                anchors.displays.push((step.id, row.with_max_x(ui.max_rect().right())));
            }
        }
    });
}

fn dispatcher<N: NodesEnum>(ui: &mut egui::Ui, colours: &Colours, info: Option<&RunInfo<N>>, anchors: &mut Anchors) {
    let period = format!("every {:.0} ms", PASS_DURATION * 1000.0);
    let rect = container(ui, colours, "Pass dispatcher", WHY_DISPATCHER, &period, 1, info.is_some(), |ui| {
        let Some(info) = info else {
            ui.label(small("Stopped. Press Play to run.", colours.text_secondary));
            return;
        };
        ui.label(small(format!("Next pass p{}", info.next_pass), colours.text_primary));
        let dropped = info.stats.dropped;
        let colour = if dropped > 0 { colours.warning } else { colours.text_secondary };
        ui.label(small(format!("{dropped} dropped (queue full)"), colour));
        let queued = info.running.as_ref().map_or(0, |e| e.queued);
        pool_row(ui, colours, pass_queue(queued, info.queue_capacity), |ui| {
            ui.label(small("Queue", colours.text_secondary));
        });
    });
    anchors.dispatcher = Some(rect);
}

/// The helpers: only a quiet list, as they aren't part of how passes run, and come and go.
fn helpers(ui: &mut egui::Ui, colours: &Colours) {
    ui.label(small("Helper threads, as they're needed", colours.text_disabled));
    for (name, why) in HELPERS {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(small(name, colours.text_disabled));
            info_icon(ui, colours, why);
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn workers_ui<N: NodesEnum>(
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
    container(ui, colours, &title, WHY_WORKERS, &detail, count, info.is_some(), |ui| {
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
fn worker_chips<N: NodesEnum>(ui: &mut egui::Ui, colours: &Colours, info: &RunInfo<N>) {
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
fn steps<N: NodesEnum>(
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
fn step_row<N: NodesEnum>(
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
        // Copied for it, if it takes a buffer it can't be the last to read.
        let copied = plan.steps().iter().flat_map(|s| &s.copies).find(|c| c.node == step.id && c.input == i);
        match buffer {
            Some(b) if step.takes.get(i) == Some(&true) && copied.is_some() => {
                let from = copied.map_or(0, |c| c.from);
                let text = format!(
                    "Takes buffer {b}, a copy of {name} (from buffer {from}) the plan makes for it, as it can't be \
                     the last to read it: then does as it likes with it (swaps it for one of its own, say)"
                );
                buffer_tag(ui, plan, *b, colours).on_hover_text(&text);
                badge(ui, Glyph::Copy, colours.accent, &text);
            }
            Some(b) if step.takes.get(i) == Some(&true) => {
                let text = format!(
                    "Swap point: takes buffer {b}, with {name} in it, being the last to read it, and swaps it for one \
                     of its own (one of its thread's, say, to write), so nothing's copied"
                );
                buffer_tag(ui, plan, *b, colours).on_hover_text(&text);
                badge(ui, Glyph::Swap, colours.success, &text);
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
    // The outputs its thread fills, which the passes swap for their own.
    let threads = graph.get(step.id).map(|e| e.node.threads()).unwrap_or_default();
    let swapped = |o: usize| threads.iter().any(|t| t.flow == Flow::ToPasses && t.swapped_outputs.contains(&o));
    for (o, &b) in step.outputs.iter().enumerate() {
        let name = output_names.get(o).copied().unwrap_or("output");
        // What's in it, if there's more to say than its kind.
        let stream = plan.buffers().get(b).map(|s| s.summary()).filter(|s| !s.is_empty());
        let stream = stream.map(|s| format!(" ({s})")).unwrap_or_default();
        match step.in_place[o] {
            Some(input) => {
                let input = input_names.get(input).copied().unwrap_or("input");
                let text =
                    format!("Makes {name}{stream} in place: edits buffer {b}, where {input} was, so nothing's copied");
                buffer_tag(ui, plan, b, colours).on_hover_text(&text);
                badge(ui, Glyph::InPlace, colours.success, &text);
            }
            None if swapped(o) => {
                let text = format!(
                    "Swap point: makes {name}{stream} by swapping buffer {b} for one its thread filled, from their \
                     pool (and giving the pass's back, to be filled again), so nothing's copied"
                );
                buffer_tag(ui, plan, b, colours).on_hover_text(&text);
                badge(ui, Glyph::Swap, colours.success, &text);
            }
            None => {
                buffer_tag(ui, plan, b, colours).on_hover_text(format!("Fills {name}{stream} in buffer {b}"));
            }
        }
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !step.copies.is_empty() {
            let copies: Vec<String> = step
                .copies
                .iter()
                .map(|c| {
                    let name = graph.get(c.node).map_or("a node", |e| e.node.info().name);
                    format!("buffer {} into buffer {}, for {name} to take", c.from, c.to)
                })
                .collect();
            let text = format!(
                "Copy point: once it's run, the plan copies {}. A node taking a buffer gets a copy when it can't be \
                 the last to read it (another reader depends on it, or the data's edited in place). Counted in its \
                 time",
                copies.join("; ")
            );
            badge(ui, Glyph::Copy, colours.accent, &text);
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
fn not_running<N: NodesEnum>(ui: &mut egui::Ui, graph: &Graph<N>, colours: &Colours, plan: &Plan<N>) {
    let idle: Vec<NodeId> =
        graph.iter().map(|(id, _)| id).filter(|id| !plan.steps().iter().any(|s| s.id == *id)).collect();
    if idle.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.label(small(
        "Not in the plan: nothing's running on its inputs, some output can't be known (an input not connected?), or \
         nothing needs its outputs now",
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
fn memory<N: NodesEnum>(
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
                        match stream.summary() {
                            summary if summary.is_empty() => stream_info::<N>(stream).name.to_string(),
                            summary => format!("{} · {summary}", stream_info::<N>(stream).name),
                        },
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
            let copies = plan.steps().iter().map(|s| s.copies.len()).sum::<usize>();
            ui.label(small(
                format!(
                    "Also: {states} node states (one each, handed from pass to pass), {working} nodes' working \
                     memory (each worker its own, so no waiting), and the pools nodes share with their threads and \
                     windows. {copies} of the buffers are copies the plan makes, for nodes taking them."
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

/// The curve of a link across from one column to another, from `from` to `to`: leaving and arriving level.
fn across_points(from: Pos2, to: Pos2) -> [Pos2; 4] {
    let bend = ((to.x - from.x).abs() * 0.5).max(30.0);
    let dx = if to.x >= from.x { bend } else { -bend };
    [from, from + vec2(dx, 0.0), to - vec2(dx, 0.0), to]
}

/// The points along a curve, `by` points below it (on its lower side, whichever way it goes).
fn parallel(curve: [Pos2; 4], by: f32) -> Vec<Pos2> {
    let shape = egui::epaint::CubicBezierShape::from_points_stroke(curve, false, Color32::TRANSPARENT, Stroke::NONE);
    let points = shape.flatten(Some(0.25));
    let last = points.len().saturating_sub(1);
    (0..points.len())
        .map(|i| {
            // The tangent here, pointing rightwards, so the offset is always below.
            let tangent = (points[(i + 1).min(last)] - points[i.saturating_sub(1)]).normalized();
            let tangent = if tangent.x < 0.0 { -tangent } else { tangent };
            points[i] - tangent.rot90() * by
        })
        .collect()
}

/// What a row of squares shows: a pool of buffers, or a queue.
struct Pooled<'a> {
    /// How many places there are, how many are full, whether one's being filled (or written), and how many are empty.
    /// Any others are elsewhere for now, with the passes or a window.
    size: usize,
    full: usize,
    filling: bool,
    empty: usize,
    /// How many, as words (how big's in the tip, as a window's pool doesn't know).
    text: String,
    /// Its buffers' streams' colours, if they're known.
    dots: &'a [Color32],
    tip: String,
    /// Whether it's run out, and lost something.
    lost: bool,
}

/// How many squares go across, before a pool takes another line.
const SQUARES_ACROSS: usize = 10;
const SQUARE_STEP: f32 = 6.0;

/// A square for each place, full ones first, then the one being filled (or written), then the empty ones (the rest
/// being elsewhere).
fn squares(ui: &mut egui::Ui, colours: &Colours, pooled: &Pooled) -> egui::Response {
    let across = pooled.size.clamp(1, SQUARES_ACROSS);
    let lines = pooled.size.div_ceil(SQUARES_ACROSS).max(1);
    let size = vec2(across as f32 * SQUARE_STEP - 1.0, lines as f32 * SQUARE_STEP - 1.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let filling = pooled.filling as usize;
    for i in 0..pooled.size {
        let at = vec2((i % SQUARES_ACROSS) as f32, (i / SQUARES_ACROSS) as f32) * SQUARE_STEP;
        let square = Rect::from_min_size(rect.min + at, Vec2::splat(SQUARE_STEP - 1.0));
        if i < pooled.full {
            painter.rect_filled(square, 1.0, colours.accent);
        } else if i < pooled.full + filling {
            painter.rect_filled(square, 1.0, colours.warning);
        } else if i < pooled.full + filling + pooled.empty {
            painter.rect_stroke(square, 1.0, Stroke::new(1.0, colours.text_disabled), egui::StrokeKind::Inside);
        }
    }
    response
}

/// A pool or queue, however it's used, in a thread: `lead` says whose, then its streams, how many, and a square for each
/// place. Outlined in red if it's lost anything. Hover for the counts. Returns where it is.
fn pool_row(ui: &mut egui::Ui, colours: &Colours, pooled: Pooled, lead: impl FnOnce(&mut egui::Ui)) -> Rect {
    let lines = pooled.size.div_ceil(SQUARES_ACROSS).max(1) as f32;
    let row = ui
        .horizontal(|ui| {
            ui.set_height(ROW.max(lines * SQUARE_STEP + 2.0));
            lead(ui);
            for colour in pooled.dots {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.5, *colour);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                squares(ui, colours, &pooled).on_hover_text(&pooled.tip);
                ui.label(small(&pooled.text, colours.text_secondary)).on_hover_text(&pooled.tip);
            });
        })
        .response
        .rect;
    if pooled.lost {
        ui.painter().rect_stroke(row.expand(2.0), 3.0, Stroke::new(1.5, colours.error), egui::StrokeKind::Inside);
    }
    row
}

/// A pool of buffers a node's thread swaps with the passes (see `PoolStatus`).
fn thread_pool(pool: PoolStatus, dots: &[Color32]) -> Pooled<'_> {
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
    }
    let (size, full, filling, empty) = (pool.size, pool.full, pool.filling, pool.empty);
    Pooled { size, full, filling, empty, text: format!("× {size}"), dots, tip, lost: pool.overflows > 0 }
}

/// A pool a node shares with its window. Passes skipping sending doesn't lose anything that's recorded.
fn window_pool(pool: PoolStatus) -> Pooled<'static> {
    let mut tip = format!(
        "A pool of {} buffers the node's passes fill (or swap their own into) for its window, which takes them (the \
         newest, or every one, as the node needs) and gives them back: {} queued, {} empty; the rest being filled, or \
         read. Neither waits on the other. {} sent so far.",
        pool.size, pool.full, pool.empty, pool.swaps
    );
    if pool.overflows > 0 {
        tip += &format!("\n\nRan out {} times: passes skipped sending.", pool.overflows);
    }
    let (size, full, filling, empty) = (pool.size, pool.full, pool.filling, pool.empty);
    Pooled { size, full, filling, empty, text: format!("× {size}"), dots: &[], tip, lost: false }
}

/// The pass queue, between the dispatcher and the workers.
fn pass_queue(queued: usize, capacity: usize) -> Pooled<'static> {
    let tip = format!(
        "The pass queue: the dispatcher puts each pass on it when its time's up, and a free worker takes it off. {queued} \
         waiting, of the {capacity} that can: beyond that, passes are dropped. Each epoch has its own."
    );
    let empty = capacity.saturating_sub(queued);
    let text = format!("{queued} of {capacity}");
    Pooled { size: capacity, full: queued, filling: false, empty, text, dots: &[], tip, lost: false }
}

fn legend(ui: &mut egui::Ui, colours: &Colours) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (which, colour, text) in [
            (Glyph::Sync, colours.warning, "sync point"),
            (Glyph::Copy, colours.accent, "copy point"),
            (Glyph::InPlace, colours.success, "made in place"),
            (Glyph::Swap, colours.success, "swapped, not copied"),
            (Glyph::Queue, colours.text_secondary, "queue"),
        ] {
            badge(ui, which, colour, text);
            ui.add(egui::Label::new(small(text, colours.text_secondary)).extend());
            ui.add_space(8.0);
        }
    });
}

/// The nodes' threads that data goes `flow` from.
fn node_threads<N: NodesEnum>(
    ui: &mut egui::Ui,
    graph: &Graph<N>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    plan: &Plan<N>,
    flow: Flow,
    anchors: &mut Anchors,
) {
    for step in plan.steps() {
        let Some(entry) = graph.get(step.id) else { continue };
        for (index, thread) in entry.node.threads().iter().enumerate().filter(|(_, t)| t.flow == flow) {
            let exchange = runner.map(|r| r.exchange());
            let context = ThreadContext { theme: colours, node: step.id, exchange };
            {
                // The pool's buffers are like the step's: its outputs' for a source, its inputs' for a writer.
                let ports: Vec<Option<usize>> = match thread.flow {
                    Flow::ToPasses => step.outputs.iter().map(|&b| Some(b)).collect(),
                    Flow::FromPasses => step.inputs.clone(),
                };
                let pool = entry.node.thread_pool(index, &context);
                let pool = pool.filter(|p| p.size > 0).map(|p| {
                    let pooled = ports.iter().enumerate().filter(|(i, _)| *i < 64 && p.ports & (1 << i) != 0);
                    let colour =
                        |b: usize| plan.buffers().get(b).map_or(colours.text_disabled, |s| stream_info::<N>(s).colour);
                    (p, pooled.filter_map(|(_, b)| b.map(colour)).collect::<Vec<Color32>>())
                });
                let rect = container(ui, colours, thread.name, thread.why, "", 1, runner.is_some(), |ui| {
                    match &pool {
                        Some((pool, dots)) => {
                            pool_row(ui, colours, thread_pool(*pool, dots), |ui| {
                                node_chip(ui, graph, step.id, None);
                            });
                        }
                        None => {
                            node_chip(ui, graph, step.id, None);
                        }
                    }
                    ui.add_space(2.0);
                    entry.node.thread_ui(index, ui, &context);
                });
                anchors.node_threads.push(NodeThreadAnchor {
                    node: step.id,
                    rect,
                    link: thread.link,
                    flow: thread.flow,
                    pooled: pool.is_some(),
                });
                ui.add_space(12.0);
            }
        }
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
        curve(across_points(from, to));
        arrow(to, to.x >= from.x);
    };
    // From one left edge round to another, `depth` out into the gutter.
    let round = |from: Pos2, to: Pos2, depth: f32| {
        curve([from, from - vec2(depth * 1.3, 0.0), to - vec2(depth * 1.3, 0.0), to]);
        arrow(to, true);
    };

    // A line along `path`, with an arrow at its end.
    let along = |path: Vec<Pos2>, dashed: bool| {
        let [.., before, end] = path[..] else { return };
        let stroke = Stroke::new(if dashed { 1.0 } else { 1.2 }, colour);
        if dashed {
            painter.extend(Shape::dashed_line(&path, stroke, 3.0, 3.0));
        } else {
            painter.add(Shape::line(path, stroke));
        }
        let (forward, side) = ((end - before).normalized(), (end - before).normalized().rot90());
        let tip = vec![end, end - forward * 6.0 + side * 3.5, end - forward * 6.0 - side * 3.5];
        painter.add(Shape::convex_polygon(tip, colour, Stroke::NONE));
    };
    // Through a pool: the data from `from` to `to`, and the empty buffers going back the other way (the passes give back
    // their own old ones as they swap in a source's; a writer's thread and a window, those they're done with, for passes
    // to use), dashed. The two are either side of where a link would be, so there's room for both.
    let through_pool = |from: Pos2, to: Pos2| {
        along(parallel(across_points(from, to), -3.0), false);
        let mut empties = parallel(across_points(from, to), 3.0);
        empties.reverse();
        along(empties, true);
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
    // To the windows, through the pools the passes share with them (the GUI reading the latest, say).
    for (index, (id, display)) in anchors.displays.iter().enumerate() {
        let Some(step) = anchors.steps.get(id) else { continue };
        if wide {
            let to = ends(*display).0;
            through_pool(ends(*step).1, to);
            if index == 0 {
                label(to + vec2(-8.0, -6.0), Align2::RIGHT_BOTTOM, "buffer pool");
            }
        } else {
            round(ends(*step).0, ends(*display).0, deeper());
        }
    }
    // Pointing the way the data goes: to the passes from a board, say, or from them to a writer.
    for NodeThreadAnchor { node: id, rect: thread, link: how, flow, pooled } in &anchors.node_threads {
        let Some(step) = anchors.steps.get(id) else { continue };
        let to_passes = *flow == Flow::ToPasses;
        if wide {
            let (from, to) =
                if to_passes { (thread.right_center(), ends(*step).0) } else { (ends(*step).1, thread.left_center()) };
            if *pooled {
                through_pool(from, to);
            } else {
                across(from, to);
            }
            if to_passes {
                label(from + vec2(6.0, -4.0), Align2::LEFT_BOTTOM, how);
            } else {
                label(to + vec2(-6.0, -4.0), Align2::RIGHT_BOTTOM, how);
            }
        } else {
            let (at_thread, at_step, depth) = (thread.left_center(), ends(*step).0, deeper());
            if to_passes {
                round(at_thread, at_step, depth)
            } else {
                round(at_step, at_thread, depth)
            }
            label(thread.left_top() + vec2(10.0, -2.0), Align2::LEFT_BOTTOM, how);
        }
    }
}
