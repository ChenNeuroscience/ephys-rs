//! A chart of the two LEDs' sizes over the last few seconds, below the Tracker View's video: steady lines
//! mean steady tracking; gaps are frames where an LED wasn't found. Pale blue bars are when the timing LED was on.

use crate::standard::widgets::nice_step;
use std::collections::VecDeque;

use eframe::egui::{self, Color32};
use egui_plot::{Line, Plot, PlotBounds, Polygon, uniform_grid_spacer};

/// Seconds of LED sizes kept and shown.
pub const SECONDS: f64 = 10.0;

/// Big (green) and small (red) LED, the colours tracking setups have long used for the two head LEDs.
pub const COLOURS: [Color32; 2] = [Color32::GREEN, Color32::RED];

/// When the timing LED was on: pale blue, under the lines.
const TIMING_COLOUR: Color32 = Color32::from_rgba_unmultiplied_const(120, 180, 255, 70);

/// (camera timestamp, [big, small] LED areas in pixels, whether the timing LED was on) per frame, oldest first; an
/// area of 0 is an LED not found in that frame.
pub type History = VecDeque<(f64, [u32; 2], bool)>;

/// Adds a frame's LED areas, and whether the timing LED was on, to `history`, dropping those more than `SECONDS` older.
pub fn record(history: &mut History, time: f64, areas: [u32; 2], timing_on: bool) {
    history.push_back((time, areas, timing_on));
    while history.front().is_some_and(|&(t, ..)| t < time - SECONDS) {
        history.pop_front();
    }
}

/// The chart, filling the space left: the newest frame is now, and the older ones go left (see `time_axis`). Frames where
/// an LED wasn't found leave a gap in its line.
pub fn chart(ui: &mut egui::Ui, history: &History) {
    let newest = history.back().map_or(0.0, |&(t, ..)| t);
    let top = history.iter().map(|&(_, a, _)| a[0].max(a[1])).max().unwrap_or(0).max(10) as f64 * 1.15;
    let step = nice_step(top / 5.0);
    Plot::new(ui.scope_id().with("led sizes"))
        .width(ui.available_width())
        .height(ui.available_height())
        .x_grid_spacer(uniform_grid_spacer(|_| [1.0, 2.0, 10.0]))
        .x_axis_formatter(|mark, _| crate::time_axis::label(mark.value))
        .y_grid_spacer(uniform_grid_spacer(move |_| [step / 2.0, step, step * 10.0]))
        .y_axis_formatter(|mark, _| format!("{}px", mark.value.round()))
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show(ui, |plot| {
            let (left, right) = crate::time_axis::window(0.0, SECONDS);
            plot.set_plot_bounds(PlotBounds::from_min_max([left, 0.0], [right, top]));
            for [from, to] in timing_runs(history, newest) {
                let bar = vec![[from, 0.0], [to, 0.0], [to, top], [from, top]];
                plot.polygon(Polygon::new("Timing LED", bar).fill_color(TIMING_COLOUR).stroke((0.0, TIMING_COLOUR)));
            }
            for (i, name) in ["Big", "Small"].into_iter().enumerate() {
                for run in seen_runs(history, i, newest) {
                    plot.line(Line::new(name, run).color(COLOURS[i]).width(2.0));
                }
            }
        });
}

/// Runs of consecutive frames in which LED `i` was found, as (seconds after `start`, area) points.
fn seen_runs(history: &History, i: usize, start: f64) -> Vec<Vec<[f64; 2]>> {
    let mut runs = vec![Vec::new()];
    for &(t, areas, _) in history {
        if areas[i] > 0 {
            runs.last_mut().unwrap().push([t - start, areas[i] as f64]);
        } else if !runs.last().unwrap().is_empty() {
            runs.push(Vec::new());
        }
    }
    runs.retain(|r| !r.is_empty());
    runs
}

/// When the timing LED was on, as seconds after `start`: from the first frame it was on to the first it was off, or
/// the last there is.
fn timing_runs(history: &History, start: f64) -> Vec<[f64; 2]> {
    let (mut runs, mut from) = (Vec::new(), None);
    for &(t, _, on) in history {
        match (on, from) {
            (true, None) => from = Some(t - start),
            (false, Some(began)) => {
                runs.push([began, t - start]);
                from = None;
            }
            _ => {}
        }
    }
    if let (Some(began), Some(&(t, ..))) = (from, history.back()) {
        runs.push([began, t - start]);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_bars_run_from_it_coming_on_to_it_going_off() {
        let frames = [(1.0, false), (2.0, true), (3.0, true), (4.0, false), (5.0, true), (6.0, true)];
        let history: History = frames.map(|(t, on)| (t, [0, 0], on)).into();
        assert_eq!(timing_runs(&history, 1.0), [[1.0, 3.0], [4.0, 5.0]], "the last still on");
    }

    #[test]
    fn runs_break_where_the_led_was_not_found() {
        let frames = [(1.0, [30, 10]), (1.1, [32, 0]), (1.2, [0, 14]), (1.3, [31, 12])];
        let history: History = frames.map(|(t, areas)| (t, areas, false)).into();
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
            record(&mut history, frame as f64 / 50.0, [30, 10], false);
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
