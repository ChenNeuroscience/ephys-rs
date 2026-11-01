//! Developer tools, as browsers have: a picture of the threads running the graph, with the plan each pass runs
//! and the memory it takes (`threads`), above the program's log (`console`).

mod ansi;
mod console;
mod threads;

use eframe::egui;

use crate::core::engine::{Graph, NodeSet, Resolved, Runner, Snapshot};
use crate::core::theme::Colours;

/// What the dev tools remember between frames.
pub struct DevTools {
    console: console::ConsoleView,
    /// The share of the height the console takes.
    console_share: f32,
}

impl Default for DevTools {
    fn default() -> Self {
        DevTools { console: console::ConsoleView::default(), console_share: 0.35 }
    }
}

impl DevTools {
    /// Filters the console to show nothing.
    #[cfg(test)]
    pub fn hide_console_lines(&mut self) {
        self.console.filter = "(nothing matches this)".into();
    }
}

/// The threads and the console, with a bar between them to drag: the console below, or beside the threads if
/// the pane is much wider than it's tall.
pub fn show<N: NodeSet>(
    ui: &mut egui::Ui,
    tools: &mut DevTools,
    graph: &Graph<N>,
    resolved: &Resolved<N::Edges>,
    colours: &Colours,
    runner: Option<&Runner<N>>,
    snapshot: &Snapshot<N>,
) {
    let area = ui.available_rect_before_wrap();
    let beside = area.width() > 2.5 * area.height() && area.width() > 900.0;
    // Along the axis the two share.
    let (start, length) = if beside { (area.left(), area.width()) } else { (area.top(), area.height()) };
    let split = start + length * (1.0 - tools.console_share);
    let cross = |range: egui::Rangef| {
        if beside {
            egui::Rect::from_x_y_ranges(range, area.y_range())
        } else {
            egui::Rect::from_x_y_ranges(area.x_range(), range)
        }
    };
    let bar = cross(egui::Rangef::new(split - 3.0, split + 3.0));
    let drag = ui.interact(bar, ui.scope_id().with("dev tools split"), egui::Sense::drag());
    if let Some(pointer) = drag.interact_pointer_pos().filter(|_| drag.dragged()) {
        let at = if beside { pointer.x } else { pointer.y };
        tools.console_share = ((start + length - at) / length).clamp(0.1, 0.9);
    }
    let bar_colour = if drag.hovered() || drag.dragged() { colours.accent } else { colours.divider };
    let stroke = egui::Stroke::new(1.0, bar_colour);
    if beside {
        ui.painter().vline(split, area.y_range(), stroke);
    } else {
        ui.painter().hline(area.x_range(), split, stroke);
    }
    let cursor = if beside { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical };
    drag.on_hover_cursor(cursor);

    let section = |ui: &mut egui::Ui, rect: egui::Rect, add: &mut dyn FnMut(&mut egui::Ui)| {
        let layout = egui::Layout::top_down(egui::Align::Min);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 6.0))).layout(layout), |ui| {
            ui.set_clip_rect(rect);
            add(ui)
        });
    };
    section(ui, cross(egui::Rangef::new(start, split - 3.0)), &mut |ui| {
        ui.label(egui::RichText::new("Threads").strong());
        ui.add_space(4.0);
        threads::show(ui, graph, resolved, colours, runner, snapshot);
    });
    section(ui, cross(egui::Rangef::new(split + 3.0, start + length)), &mut |ui| {
        console::show(ui, &mut tools.console, graph, colours)
    });
    ui.advance_cursor_after_rect(area);
    // New log lines and the workers' progress show without waiting for input.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(if runner.is_some() { 100 } else { 500 }));
}
