//! A chart of the two LEDs' sizes over the last few seconds, below the Tracker View's video: steady lines
//! mean steady tracking; gaps are frames where an LED wasn't found.

use std::collections::VecDeque;

use eframe::egui::{self, Color32};
use egui_plot::{Line, Plot, PlotBounds, uniform_grid_spacer};

/// Seconds of LED sizes kept and shown.
pub const SECONDS: f64 = 10.0;

/// Big (green) and small (red) LED, the colours tracking setups have long used for the two head LEDs.
pub const COLOURS: [Color32; 2] = [Color32::GREEN, Color32::RED];

/// (camera timestamp, [big, small] LED areas in pixels) per frame, oldest first; an area of 0 is an LED not
/// found in that frame.
pub type History = VecDeque<(f64, [u32; 2])>;

/// Adds a frame's LED areas to `history`, dropping those more than `SECONDS` older.
pub fn record(history: &mut History, time: f64, areas: [u32; 2]) {
    history.push_back((time, areas));
    while history.front().is_some_and(|&(t, _)| t < time - SECONDS) {
        history.pop_front();
    }
}

/// The chart, filling the space left: 0 s is the oldest frame kept, so the lines fill from the left and then
/// scroll (see `time_axis`). Frames where an LED wasn't found leave a gap in its line.
pub fn chart(ui: &mut egui::Ui, history: &History) {
    let start = history.front().map_or(0.0, |&(t, _)| t);
    let top = history.iter().map(|&(_, a)| a[0].max(a[1])).max().unwrap_or(0).max(10) as f64 * 1.15;
    let step = nice_step(top / 5.0);
    Plot::new(ui.scope_id().with("led sizes"))
        .width(ui.available_width())
        .height(ui.available_height())
        .x_grid_spacer(uniform_grid_spacer(|_| [1.0, 2.0, 10.0]))
        .x_axis_formatter(|mark, _| format!("{}s", mark.value.round()))
        .y_grid_spacer(uniform_grid_spacer(move |_| [step / 2.0, step, step * 10.0]))
        .y_axis_formatter(|mark, _| format!("{}px", mark.value.round()))
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show(ui, |plot| {
            let now = history.back().map_or(0.0, |&(t, _)| t - start);
            let (left, right) = crate::core::time_axis::window(0.0, now, SECONDS);
            plot.set_plot_bounds(PlotBounds::from_min_max([left, 0.0], [right, top]));
            for (i, name) in ["Big", "Small"].into_iter().enumerate() {
                for run in seen_runs(history, i, start) {
                    plot.line(Line::new(name, run).color(COLOURS[i]).width(2.0));
                }
            }
        });
}

/// A round tick spacing (1, 2 or 5 times a power of ten) of at least `raw`.
fn nice_step(raw: f64) -> f64 {
    let power = 10f64.powf(raw.max(1e-9).log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * power).find(|&s| s >= raw).unwrap()
}

/// Runs of consecutive frames in which LED `i` was found, as (seconds after `start`, area) points.
fn seen_runs(history: &History, i: usize, start: f64) -> Vec<Vec<[f64; 2]>> {
    let mut runs = vec![Vec::new()];
    for &(t, areas) in history {
        if areas[i] > 0 {
            runs.last_mut().unwrap().push([t - start, areas[i] as f64]);
        } else if !runs.last().unwrap().is_empty() {
            runs.push(Vec::new());
        }
    }
    runs.retain(|r| !r.is_empty());
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_break_where_the_led_was_not_found() {
        let history: History = [(1.0, [30, 10]), (1.1, [32, 0]), (1.2, [0, 14]), (1.3, [31, 12])].into();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let big = seen_runs(&history, 0, 1.0);
        assert_eq!(big.len(), 2);
        assert!(close(big[0][0][0], 0.0) && close(big[1][0][0], 0.3) && big[0].len() == 2 && big[1].len() == 1);
        assert_eq!(seen_runs(&history, 1, 1.0).iter().map(Vec::len).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn history_keeps_the_last_seconds() {
        let mut history = History::new();
        for frame in 0..=600 {
            record(&mut history, frame as f64 / 50.0, [30, 10]);
        }
        assert_eq!(history.len(), 501, "10 s at 50 fps, both ends included");
        assert_eq!(history.front().unwrap().0, 2.0);
    }

    #[test]
    fn tick_steps_are_round() {
        assert_eq!(nice_step(9.6), 10.0);
        assert_eq!(nice_step(13.0), 20.0);
        assert_eq!(nice_step(41.0), 50.0);
        assert_eq!(nice_step(0.3), 0.5);
    }
}
