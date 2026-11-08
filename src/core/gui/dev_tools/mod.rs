//! Developer tools, as browsers have: a picture of the threads running the graph, with the plan each pass runs
//! and the memory it takes (`threads`); how long passes and the GUI's frames take (`performance`); and the
//! program's log (`console`).

mod ansi;
mod console;
mod performance;
mod threads;

use eframe::egui;

use super::frame_times::FrameTimes;
use crate::core::engine::{Graph, NodesEnum, Resolved, Runner, Snapshot};
use crate::core::theme::Colours;

/// What the dev tools remember between frames.
pub struct DevTools {
    console: console::ConsoleView,
    performance: performance::Performance,
    /// The share of the space the performance and console take (from the threads).
    console_share: f32,
    /// Whether the threads, performance and console sections are open.
    open: [bool; 3],
}

impl DevTools {
    /// Opens the console's section.
    pub fn open_console(&mut self) {
        self.open[2] = true;
    }
}

impl Default for DevTools {
    fn default() -> Self {
        DevTools {
            console: console::ConsoleView::default(),
            performance: Default::default(),
            console_share: 0.5,
            open: [true; 3],
        }
    }
}

impl DevTools {
    /// Filters the console to show nothing.
    #[cfg(test)]
    pub fn hide_console_lines(&mut self) {
        self.console.filter = "(nothing matches this)".into();
    }
}

/// The threads, the performance and the console, each under a header that collapses it: one above another, or
/// (if the pane is much wider than it's tall) the threads beside the other two. A bar between the threads and the
/// others, while both are open, can be dragged to share the space between them.
#[allow(clippy::too_many_arguments)]
pub fn show<N: NodesEnum>(
    ui: &mut egui::Ui,
    tools: &mut DevTools,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    snapshot: &Snapshot<N>,
    workers: usize,
    frame_times: &FrameTimes,
) {
    const HEADER: f32 = 24.0;
    let area = ui.available_rect_before_wrap();
    let [threads_open, performance_open, console_open] = tools.open;
    let others_open = performance_open || console_open;
    let beside = threads_open && others_open && area.width() > 2.5 * area.height() && area.width() > 900.0;

    // Where each section goes: the threads' rect, and the other two's column.
    let (threads, others) = if beside {
        let split = area.left() + area.width() * (1.0 - tools.console_share);
        (
            egui::Rect::from_x_y_ranges(area.left()..=split - 3.0, area.y_range()),
            egui::Rect::from_x_y_ranges(split + 3.0..=area.right(), area.y_range()),
        )
    } else {
        // The headers, the performance (fixed), then what's left shared between the threads and the console.
        let fixed = 3.0 * HEADER + if performance_open { performance::HEIGHT } else { 0.0 };
        let flex = (area.height() - fixed).max(0.0);
        let threads_height = match (threads_open, console_open) {
            (true, true) => flex * (1.0 - tools.console_share),
            (true, false) => flex,
            _ => 0.0,
        };
        let split = area.top() + HEADER + threads_height;
        (
            egui::Rect::from_x_y_ranges(area.x_range(), area.top()..=split),
            egui::Rect::from_x_y_ranges(area.x_range(), split..=area.bottom()),
        )
    };
    // The bar to drag, between the threads and the others, while both are open.
    if threads_open && console_open {
        let (bar, along) = if beside {
            (egui::Rect::from_x_y_ranges(threads.right()..=others.left(), area.y_range()), area.width())
        } else {
            (
                egui::Rect::from_x_y_ranges(area.x_range(), threads.bottom() - 3.0..=threads.bottom() + 3.0),
                area.height(),
            )
        };
        let drag = ui.interact(bar, ui.scope_id().with("dev tools split"), egui::Sense::drag());
        if drag.dragged() {
            let delta = if beside { drag.drag_delta().x } else { drag.drag_delta().y };
            tools.console_share = (tools.console_share - delta / along).clamp(0.1, 0.9);
        }
        let colour = if drag.hovered() || drag.dragged() { colours.accent } else { colours.divider };
        let stroke = egui::Stroke::new(1.0, colour);
        if beside {
            ui.painter().vline(bar.center().x, bar.y_range(), stroke);
        } else {
            ui.painter().hline(bar.x_range(), bar.center().y, stroke);
        }
        let cursor = if beside { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical };
        drag.on_hover_cursor(cursor);
    }

    // A section: its header (clicking it opens or closes it), then, if open, its contents in what's left of `rect`.
    let mut section = |ui: &mut egui::Ui,
                       index: usize,
                       title: &str,
                       rect: egui::Rect,
                       add: &mut dyn FnMut(&mut egui::Ui, &mut DevTools)| {
        let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), HEADER));
        let response = ui.interact(header, ui.scope_id().with(("section", index)), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let open = tools.open[index];
        let ink = if response.hovered() { colours.text_primary } else { colours.text_secondary };
        let at = egui::pos2(header.left() + 14.0, header.center().y);
        let h = 4.0;
        let triangle = if open {
            vec![at + egui::vec2(-h, -h / 2.0), at + egui::vec2(h, -h / 2.0), at + egui::vec2(0.0, h * 0.75)]
        } else {
            vec![at + egui::vec2(-h / 2.0, -h), at + egui::vec2(-h / 2.0, h), at + egui::vec2(h * 0.75, 0.0)]
        };
        ui.painter().add(egui::Shape::convex_polygon(triangle, ink, egui::Stroke::NONE));
        let font = egui::FontId::proportional(14.0);
        let title_at = egui::pos2(header.left() + 24.0, header.center().y);
        ui.painter().text(title_at, egui::Align2::LEFT_CENTER, title, font, colours.text_primary);
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, true, open, title));
        if response.clicked() {
            tools.open[index] = !open;
        }
        if !open {
            return header.height();
        }
        let body = egui::Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);
        let layout = egui::Layout::top_down(egui::Align::Min);
        ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(8.0, 2.0))).layout(layout), |ui| {
            ui.set_clip_rect(body);
            add(ui, tools)
        });
        rect.height()
    };
    section(ui, 0, "Threads", threads, &mut |ui, _| {
        threads::show(ui, graph, resolved, colours, runner, snapshot, workers)
    });
    let passes = runner.map(|r| r.pass_times());
    // The performance's height: its own, but giving the console (if open) some room, if there isn't much.
    let console_room = if console_open { HEADER + 140.0 } else { 0.0 };
    let performance_body =
        (others.height() - HEADER - console_room).clamp(performance::MIN_HEIGHT, performance::HEIGHT);
    let performance_height = HEADER + if performance_open { performance_body } else { 0.0 };
    let performance = egui::Rect::from_min_size(others.min, egui::vec2(others.width(), performance_height));
    section(ui, 1, "Performance", performance, &mut |ui, tools| {
        performance::show(ui, &mut tools.performance, colours, frame_times, passes.as_deref())
    });
    let console = egui::Rect::from_min_max(egui::pos2(others.left(), performance.bottom()), others.max);
    section(ui, 2, "Console", console, &mut |ui, tools| console::show(ui, &mut tools.console, graph, colours));
    ui.advance_cursor_after_rect(area);
    // New log lines and the workers' progress show without waiting for input.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(if runner.is_some() { 100 } else { 500 }));
}
