//! The TTL Viewer: each digital input line's level over the last few seconds, a row per line, and how many times
//! each has gone up since acquisition started, and since recording last started.

use std::collections::VecDeque;

use eframe::egui::{self, Align2, FontId, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, CopyTo, EdgeType, Input, Node, NodeInfo, Output, Pass, PassState};
use crate::core::exchange::Latest;
use crate::standard_components::ephys::Events;
use crate::standard_components::preview;
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};

/// The most changes kept, however many come.
const KEPT: usize = 20_000;
/// The longest span that can be shown, in seconds.
const LONGEST: f32 = 60.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtlViewer {
    /// Seconds across the view.
    pub span: f32,
}

impl Default for TtlViewer {
    fn default() -> Self {
        TtlViewer { span: 5.0 }
    }
}

/// A line going up or down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    /// Seconds since acquisition started.
    pub time: f64,
    pub line: u8,
    pub high: bool,
}

/// The lines' levels and changes, and how many times each has gone up.
#[derive(Default)]
pub struct Lines {
    stream: Option<Events>,
    /// Samples so far, which the events are timed by.
    samples: u64,
    /// The lines' levels now, a bit each.
    levels: u32,
    /// Changes over the longest span that can be shown.
    changes: VecDeque<Change>,
    /// For each line, times it's gone up since acquisition started, and since recording last started.
    ups: Vec<u64>,
    ups_recording: Vec<u64>,
    /// Whether it's recording, and whether it has since acquisition started.
    recording: bool,
    recorded: bool,
    shown: Latest<LinesDisplay>,
}

impl PassState for Lines {
    const WHY: &'static str = "The lines' levels, their changes over the last minute (to plot), and how many times \
                               each has gone up, which each pass carries on from; and samples so far, to time the \
                               changes.";
}

/// What's shown: the changes over the span, the levels as it began, and the counts.
#[derive(Clone, Default)]
pub struct LinesDisplay {
    pub lines: usize,
    /// Seconds since acquisition started, at the end of the last pass.
    pub now: f64,
    pub span: f64,
    /// The lines' levels at the start of the span, a bit each.
    pub levels_before: u32,
    pub changes: Vec<Change>,
    pub ups: Vec<u64>,
    pub ups_recording: Vec<u64>,
    /// Whether it's recording, or has recorded (so there's a count since recording started).
    pub recording: bool,
    pub recorded: bool,
}

impl Node<Edges> for TtlViewer {
    type State = Lines;

    const INFO: NodeInfo = NodeInfo {
        name: "TTL Viewer",
        category: Category::Visualisation,
        description: "Digital inputs' levels over time, a row per line, with how many times each has gone up since \
                      acquisition, and recording, started.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "events", kind: Kind::Events }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "events")
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Window)
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Lines {
        let Some(DataStream::Events(stream)) = inputs[0] else { return Lines::default() };
        let lines = stream.lines as usize;
        Lines {
            stream: Some(stream),
            changes: VecDeque::with_capacity(KEPT),
            ups: vec![0; lines],
            ups_recording: vec![0; lines],
            ..Default::default()
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, lines: &mut Lines) {
        let Some(Some(Buffer::Events(events))) = pass.inputs.first() else { return };
        let Some(stream) = lines.stream else { return };
        let rate = stream.sample_rate.max(1.0) as f64;
        if pass.recording && !lines.recording {
            lines.ups_recording.fill(0);
        }
        lines.recording = pass.recording;
        lines.recorded |= pass.recording;
        for event in &events.events {
            let line = event.line as usize;
            let bit = 1 << event.line;
            if event.high && lines.levels & bit == 0 {
                if let Some(up) = lines.ups.get_mut(line) {
                    *up += 1;
                }
                if let (true, Some(up)) = (pass.recording, lines.ups_recording.get_mut(line)) {
                    *up += 1;
                }
            }
            lines.levels = if event.high { lines.levels | bit } else { lines.levels & !bit };
            let time = (lines.samples + event.sample as u64) as f64 / rate;
            if lines.changes.len() == KEPT {
                lines.changes.pop_front();
            }
            lines.changes.push_back(Change { time, line: event.line, high: event.high });
        }
        lines.samples += events.pass_samples as u64;
        let now = lines.samples as f64 / rate;
        while lines.changes.front().is_some_and(|c| c.time < now - LONGEST as f64) {
            lines.changes.pop_front();
        }

        let span = self.span.clamp(0.1, LONGEST) as f64;
        let Lines { levels, changes, ups, ups_recording, recording, recorded, shown, .. } = lines;
        shown.send(pass, |display| {
            let first = changes.partition_point(|c| c.time < now - span);
            // The levels before the span: now's, with the changes since undone.
            let mut before = *levels;
            for change in changes.range(first..).rev() {
                let bit = 1 << change.line;
                before = if change.high { before & !bit } else { before | bit };
            }
            display.lines = stream.lines as usize;
            (display.now, display.span, display.levels_before) = (now, span, before);
            display.changes.clear();
            display.changes.extend(changes.range(first..));
            display.ups.clone_from(ups);
            display.ups_recording.clone_from(ups_recording);
            (display.recording, display.recorded) = (*recording, *recorded);
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Span", |ui| {
                ui.add(egui::DragValue::new(&mut self.span).range(0.1..=LONGEST).speed(0.05).suffix(" s"))
            });
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        crate::standard_components::widgets::view_settings(ui, |ui| self.settings_ui(ui, context));
        let Some(DataStream::Events(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect events to view them");
        };
        let Some(display) = context.display::<LinesDisplay>().filter(|d| d.lines > 0) else {
            let message = format!("{} · press Play to see them", stream.summary());
            return preview::unconnected(ui, context.theme, &message);
        };
        let theme = context.theme;
        let rect = preview::plot_area(ui, theme);
        let painter = ui.painter_at(rect);
        // Line numbers on the left, counts on the right.
        let (label, counts) = (34.0, 170.0);
        let trace = egui::Rect::from_x_y_ranges(rect.left() + label..=rect.right() - counts, rect.y_range());
        let row = rect.height() / display.lines as f32;
        let x =
            |time: f64| trace.left() + ((time - (display.now - display.span)) / display.span) as f32 * trace.width();
        let font = FontId::monospace(11.0);
        for line in 0..display.lines {
            let top = rect.top() + row * line as f32;
            let (high, low) = (top + row * 0.25, top + row * 0.75);
            let bit = 1 << line;
            // A step trace: along at the level, and up or down at each change.
            let mut level = display.levels_before & bit != 0;
            let mut points = vec![Pos2::new(trace.left(), if level { high } else { low })];
            for change in display.changes.iter().filter(|c| c.line as usize == line) {
                let at = x(change.time);
                points.push(Pos2::new(at, if level { high } else { low }));
                level = change.high;
                points.push(Pos2::new(at, if level { high } else { low }));
            }
            points.push(Pos2::new(trace.right(), if level { high } else { low }));
            painter.add(Shape::line(points, Stroke::new(1.5, Events::INFO.colour)));
            if line > 0 {
                painter.hline(rect.x_range(), top, Stroke::new(1.0, theme.divider.gamma_multiply(0.5)));
            }
            let middle = top + row / 2.0;
            painter.text(
                Pos2::new(rect.left() + 6.0, middle),
                Align2::LEFT_CENTER,
                line + 1,
                font.clone(),
                theme.text_secondary,
            );
            let ups = display.ups.get(line).copied().unwrap_or(0);
            let mut text = format!("{ups} up");
            if display.recorded {
                text += &format!(" · {} recording", display.ups_recording.get(line).copied().unwrap_or(0));
            }
            let colour = if ups > 0 { theme.text_primary } else { theme.text_disabled };
            painter.text(Pos2::new(rect.right() - 6.0, middle), Align2::RIGHT_CENTER, text, font.clone(), colour);
        }
        let hover = ui.interact(rect, ui.scope_id().with("ttl rows"), egui::Sense::hover());
        hover.on_hover_text("Each line's level; and how many times it's gone up since acquisition started, and since recording last started");
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard_components::ephys::{Event, EventsBuffer};

    fn events(list: &[(u32, u8, bool)], pass_samples: usize) -> crate::Buffer {
        let events = list.iter().map(|&(sample, line, high)| Event { sample, line, high }).collect();
        Buffer::Events(EventsBuffer { events, pass_samples })
    }

    #[test]
    fn counts_ups_since_acquisition_and_recording() {
        let viewer = TtlViewer::default();
        let stream = Events { lines: 2, sample_rate: 1000.0 };
        let mut lines = viewer.initial_state(&[Some(DataStream::Events(stream))]);
        let mut run = |list: &[(u32, u8, bool)], recording: bool| {
            let input = events(list, 100);
            let inputs = [Some(&input)];
            let mut pass = Pass::new(0, 0.1, &inputs, &mut []);
            pass.recording = recording;
            viewer.process(&mut pass, &mut lines);
        };
        run(&[(10, 0, true), (20, 0, false), (30, 0, true)], false);
        run(&[(5, 0, false), (50, 0, true), (60, 1, true)], true);
        assert_eq!(lines.ups, vec![3, 1]);
        assert_eq!(lines.ups_recording, vec![1, 1]);
        assert_eq!(lines.changes[3].time, 0.105, "a change in the second pass, 5 samples in");
        assert_eq!(lines.levels, 0b11);
    }
}
