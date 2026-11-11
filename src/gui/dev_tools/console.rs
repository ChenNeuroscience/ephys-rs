//! The dev tools' console: the log (see `crate::console`), as a terminal shows output, a line per row with
//! its time, level, thread and pass (and node), the messages styled by their escape codes. Filtered to a level
//! and above, and by text.

use eframe::egui::{self, Color32, FontId, RichText, text::LayoutJob};
use log::{Level, LevelFilter};

use super::ansi;
use crate::console::{self, Line};
use crate::engine::{Graph, NodesEnum};
use crate::theme::Colours;

/// What the console shows.
pub struct ConsoleView {
    /// The lowest level shown.
    pub level: LevelFilter,
    pub filter: String,
}

impl Default for ConsoleView {
    fn default() -> Self {
        ConsoleView { level: LevelFilter::Debug, filter: String::new() }
    }
}

const LEVELS: [(LevelFilter, &str); 5] = [
    (LevelFilter::Error, "Errors"),
    (LevelFilter::Warn, "Warnings and up"),
    (LevelFilter::Info, "Info and up"),
    (LevelFilter::Debug, "Debug and up"),
    (LevelFilter::Trace, "Everything"),
];

fn level_colour(level: Level, colours: &Colours) -> Color32 {
    match level {
        Level::Error => colours.error,
        Level::Warn => colours.warning,
        Level::Info => colours.success,
        Level::Debug => colours.accent,
        Level::Trace => colours.text_disabled,
    }
}

pub fn show<N: NodesEnum>(ui: &mut egui::Ui, view: &mut ConsoleView, graph: &Graph<N>, colours: &Colours) {
    ui.horizontal(|ui| {
        let current = LEVELS.iter().find(|(l, _)| *l == view.level).map_or("", |(_, name)| name);
        egui::ComboBox::new("console level", "shown").selected_text(current).show_ui(ui, |ui| {
            for (level, name) in LEVELS {
                ui.selectable_value(&mut view.level, level, name);
            }
        });
        ui.add(egui::TextEdit::singleline(&mut view.filter).hint_text("Filter").desired_width(160.0));
        if ui.button("Clear").clicked() {
            console::clear();
        }
        let dropped = console::dropped();
        if dropped > 0 {
            ui.label(RichText::new(format!("{dropped} dropped")).color(colours.warning))
                .on_hover_text("Messages logged faster than the console took them");
        }
    });
    ui.add_space(2.0);

    let font = FontId::monospace(11.5);
    let row_height = ui.fonts_mut(|f| f.row_height(&font));
    let filter = view.filter.to_lowercase();
    egui::Frame::new().fill(colours.surface_sunken).inner_margin(4).corner_radius(3).show(ui, |ui| {
        console::with_lines(|lines, _| {
            // A continuation is shown with the line it continues.
            let mut shown: Vec<&Line> = Vec::new();
            let mut showing = false;
            for line in lines {
                if !line.continues {
                    showing = line.level <= view.level && (filter.is_empty() || matches(line, &filter));
                }
                if showing {
                    shown.push(line);
                }
            }
            // Set before show_rows, which places the rows by its spacing: the rows' own would put them nearer
            // together than it thinks, leaving the bottom of the view empty.
            ui.spacing_mut().item_spacing.y = 1.0;
            egui::ScrollArea::both().auto_shrink(false).stick_to_bottom(true).show_rows(
                ui,
                row_height,
                shown.len(),
                |ui, rows| {
                    for line in &shown[rows] {
                        let job = layout(line, graph, colours, &font);
                        ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend));
                    }
                },
            );
        });
    });
}

fn matches(line: &Line, filter: &str) -> bool {
    line.text.to_lowercase().contains(filter)
        || line.target.contains(filter)
        || line.thread.name().is_some_and(|n| n.contains(filter))
}

/// The line as a row: its time, level, thread and pass (or blanks, for a continuation), then the text.
fn layout<N: NodesEnum>(line: &Line, graph: &Graph<N>, colours: &Colours, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |colour| egui::TextFormat { font_id: font.clone(), color: colour, ..Default::default() };
    let thread = line.thread.name().unwrap_or("?");
    let pass = line.pass.map(|p| {
        let node = p.node.and_then(|id| graph.get(id)).map(|e| format!(" {}", e.label)).unwrap_or_default();
        format!("e{}·p{}{node}", p.epoch, p.sequence)
    });
    let prefix = format!("{:9.3} {:5} {thread:<16} ", line.time, line.level);
    if line.continues {
        job.append(&" ".repeat(prefix.chars().count()), 0.0, format(colours.text_secondary));
    } else {
        job.append(&format!("{:9.3} ", line.time), 0.0, format(colours.text_disabled));
        job.append(&format!("{:5} ", line.level), 0.0, format(level_colour(line.level, colours)));
        job.append(&format!("{thread:<16} "), 0.0, format(colours.text_secondary));
        if let Some(pass) = &pass {
            job.append(pass, 0.0, format(colours.accent));
            job.append(" ", 0.0, format(colours.text_secondary));
        }
    }
    let text = match line.level {
        Level::Error => colours.error,
        Level::Warn => colours.warning,
        Level::Trace => colours.text_secondary,
        _ => colours.text_primary,
    };
    ansi::append(&mut job, &line.text, font, text, colours.dark);
    job
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::Nodes;
    use crate::standard::themes::THEMES;

    /// With more lines than fit, the view's filled to its bottom: the rows are where the scroll area thinks they are,
    /// rather than nearer together, which left the bottom empty, and lines vanishing whole as it scrolled.
    #[test]
    fn more_lines_than_fit_fill_the_view() {
        crate::console::init(true);
        for i in 0..200 {
            log::info!("console fills the view {i:03}");
        }
        // Other tests log at the same time, so only these are shown.
        let mut view = ConsoleView { filter: "console fills the view".into(), ..Default::default() };
        let graph = Graph::<Nodes>::default();
        let height = 300.0;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([700.0, height])
            .build_ui(move |ui| show(ui, &mut view, &graph, &THEMES[0].colours));
        harness.run_steps(3);
        let last = harness.get_by_label_contains("console fills the view 199").rect();
        // Below it, just the frame's margin and the window's.
        assert!(last.max.y > height - 20.0, "the last line ends at {}, of {height}", last.max.y);
    }
}
