//! The dev tools' performance section: how long the workers' passes take, and how long the GUI takes over each
//! frame, part by part (see `frame_times`), on one time axis. Each frame is a bar, split into its parts in shades of
//! one colour; the key below says which is which, and hovering (or clicking, to keep it) one of its parts picks it
//! out in every frame.

use eframe::egui::{self, RichText, Sense, Stroke};
use egui_plot::{Bar, BarChart, HLine, Line, Plot, PlotBounds, PlotPoints};

use crate::core::engine::PASS_DURATION;
use crate::core::gui::frame_times::{FrameTimes, PAINTING};
use crate::core::theme::Colours;
use crate::core::time_axis;

/// Seconds shown.
const SPAN: f64 = 10.0;
/// A frame's time to keep up 60 frames a second.
const FRAME_BUDGET: f64 = 1000.0 / 60.0;
/// Frames are shown in bins this long (seconds), each as its slowest frame, so there aren't more bars than can be
/// seen (or drawn quickly).
const BIN: f64 = SPAN / 200.0;
/// Passes averaged over: half a second's.
const SMOOTHING: usize = 25;

/// What the section remembers between frames: the part picked out, if it's been clicked.
#[derive(Default)]
pub struct Performance {
    pinned: Option<String>,
}

/// The height the section takes.
pub const HEIGHT: f32 = 330.0;

pub fn show(
    ui: &mut egui::Ui,
    view: &mut Performance,
    colours: &Colours,
    frames: &FrameTimes,
    passes: Option<&[[f64; 2]]>,
) {
    let now = frames.now();
    let start = frames.frames.front().map_or(now, |f| f.at);
    let (left, right) = time_axis::window(start, now, SPAN);
    let shown: Vec<_> = frames.frames.iter().filter(|f| f.at >= left).collect();
    // Each part, in the order they first come, for the key and the bars.
    let mut parts: Vec<&str> = Vec::new();
    for frame in &shown {
        for (part, _) in &frame.parts {
            if !parts.contains(&part.as_str()) {
                parts.push(part);
            }
        }
    }
    let total = |f: &&crate::core::gui::frame_times::Frame| f.parts.iter().map(|p| p.1).sum::<f32>();
    let count = shown.len().max(1) as f32;
    let average = shown.iter().map(total).sum::<f32>() / count;
    let slowest = shown.iter().map(total).fold(0.0, f32::max);
    let seconds = shown.last().map_or(0.0, |f| f.at) - shown.first().map_or(0.0, |f| f.at);
    let fps = if seconds > 0.0 { (shown.len() - 1) as f64 / seconds } else { 0.0 };

    ui.label(RichText::new("Performance").strong());
    let mut summary =
        format!("GUI: {fps:.0} frames a second, {average:.1} ms a frame on average, {slowest:.0} ms at most");
    if let Some(passes) = passes.filter(|p| !p.is_empty()) {
        let average = passes.iter().map(|p| p[1]).sum::<f64>() / passes.len() as f64 * 1000.0;
        summary += &format!(" · passes: {average:.2} ms on average");
    }
    ui.label(RichText::new(summary).small().color(colours.text_secondary));
    // Seconds ago (the future, at the right, has none).
    let axis = |mark: egui_plot::GridMark, _: &std::ops::RangeInclusive<f64>| {
        let ago = now - mark.value;
        if ago < -0.01 {
            String::new()
        } else if ago < 0.5 {
            "now".into()
        } else {
            format!("{:.0} s", -ago)
        }
    };
    let plot = |id: &str, height: f32| {
        Plot::new(id)
            .height(height)
            .y_axis_formatter(|mark, _| format!("{} ms", mark.value))
            .x_axis_formatter(axis)
            .allow_zoom(false)
            .allow_drag(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .y_axis_min_width(44.0)
    };

    // The workers' passes, while running: each, and their average, against the time each covers.
    if let (Some(passes), Some(run_started)) = (passes, frames.run_started) {
        let ms: Vec<[f64; 2]> = passes.iter().map(|&[t, d]| [run_started + t, d * 1000.0]).collect();
        let average: Vec<[f64; 2]> = ms
            .iter()
            .enumerate()
            .map(|(i, &[t, _])| {
                let window = &ms[i.saturating_sub(SMOOTHING - 1)..=i];
                [t, window.iter().map(|p| p[1]).sum::<f64>() / window.len() as f64]
            })
            .collect();
        let budget = PASS_DURATION * 1000.0;
        let top = ms.iter().map(|p| p[1]).fold(budget * 1.25, f64::max);
        ui.label(RichText::new("Passes").small().color(colours.text_secondary));
        plot("pass times", 80.0)
            .show(ui, |plot| {
                plot.set_plot_bounds(PlotBounds::from_min_max([left, 0.0], [right, top]));
                plot.hline(HLine::new("each pass's time", budget).color(colours.warning.gamma_multiply(0.7)));
                let each = Line::new("pass", PlotPoints::from(ms)).color(colours.accent.gamma_multiply(0.45));
                plot.line(each.width(1.0));
                plot.line(Line::new("average", PlotPoints::from(average)).color(colours.accent).width(2.0));
            })
            .response
            .on_hover_text(format!(
                "How long each pass took to run (thin), and the average over {SMOOTHING} passes (thick), against the \
                 {budget:.0} ms each covers (the line across): to keep up, passes must take less on average"
            ));
    }

    // The GUI's frames: a bar each, split into its parts.
    let key_id = ui.scope_id().with("hovered part");
    let hovered: Option<String> = ui.data(|d| d.get_temp(key_id));
    let picked = hovered.or_else(|| view.pinned.clone());
    let colour = |index: usize, part: &str| {
        let base = if part == PAINTING { colours.text_secondary } else { colours.accent };
        let shade = if index.is_multiple_of(2) { base } else { base.gamma_multiply(0.6) };
        match &picked {
            Some(p) if p == part => colours.warning,
            Some(_) => shade.gamma_multiply(0.3),
            None => shade,
        }
    };
    let top = shown.iter().map(total).fold(FRAME_BUDGET as f32 * 1.25, f32::max) as f64;
    ui.label(RichText::new("GUI frames").small().color(colours.text_secondary));
    plot("frame times", 110.0)
        .show(ui, |plot| {
            plot.set_plot_bounds(PlotBounds::from_min_max([left, 0.0], [right, top]));
            plot.hline(HLine::new("60 frames a second", FRAME_BUDGET).color(colours.warning.gamma_multiply(0.7)));
            // The slowest frame in each bin.
            let mut slowest: Vec<&crate::core::gui::frame_times::Frame> = Vec::new();
            for frame in &shown {
                let bin = |f: &crate::core::gui::frame_times::Frame| (f.at / BIN).floor() as i64;
                match slowest.last_mut() {
                    Some(last) if bin(last) == bin(frame) => {
                        if total(frame) > total(last) {
                            *last = frame;
                        }
                    }
                    _ => slowest.push(frame),
                }
            }
            let mut bases = vec![0.0f64; slowest.len()];
            for (index, part) in parts.iter().enumerate() {
                let bars: Vec<Bar> = slowest
                    .iter()
                    .zip(&mut bases)
                    .filter_map(|(frame, base)| {
                        let ms = frame.parts.iter().find(|p| p.0 == *part)?.1 as f64;
                        let x = ((frame.at / BIN).floor() + 0.5) * BIN;
                        let bar = Bar::new(x, ms).base_offset(*base).width(BIN * 0.8).fill(colour(index, part));
                        *base += ms;
                        Some(bar.stroke(Stroke::NONE))
                    })
                    .collect();
                plot.bar_chart(BarChart::new(*part, bars).allow_hover(false));
            }
        })
        .response
        .on_hover_text(
            "How long the GUI took over each frame, part by part: those it builds, then laying it out and \
                        painting it; the slowest in each twentieth of a second. Against the 1/60 s a frame can take, \
                        at 60 frames a second (the line across)",
        );

    // The key: each part's colour, name and average time; hover to pick it out, click to keep it picked out.
    let mut hovered_now = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (index, part) in parts.iter().enumerate() {
            let average =
                shown.iter().filter_map(|f| f.parts.iter().find(|p| p.0 == *part)).map(|p| p.1).sum::<f32>() / count;
            let response = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (swatch, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(swatch, 2.0, colour(index, part));
                    let pinned = view.pinned.as_deref() == Some(*part);
                    let text = RichText::new(format!("{part} {average:.1} ms")).small();
                    ui.label(if pinned { text.strong() } else { text.color(colours.text_secondary) });
                })
                .response
                .interact(Sense::click());
            if response.hovered() {
                hovered_now = Some(part.to_string());
            }
            if response.clicked() {
                view.pinned = if view.pinned.as_deref() == Some(*part) { None } else { Some(part.to_string()) };
            }
            response.on_hover_text("Pick it out in every frame (click to keep it picked out)");
        }
    });
    ui.data_mut(|d| match hovered_now {
        Some(part) => {
            d.insert_temp(key_id, part);
        }
        None => d.remove::<String>(key_id),
    });
}
