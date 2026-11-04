//! The TTL Viewer: each digital input line's level over the last few seconds, a row per line, and how many times
//! each has gone up since acquisition started, and since recording last started.

use std::collections::VecDeque;

use eframe::egui::{self, Align2, FontId, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, EdgeType, Input, Node, NodeInfo, Output, Pass, PassState};
use crate::standard_components::ephys::Events;
use crate::standard_components::pool::{Pool, SharedPool};
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
    /// Each line's height, in points: more lines than fit scroll.
    pub line_height: f32,
}

impl Default for TtlViewer {
    fn default() -> Self {
        TtlViewer { span: 5.0, line_height: 32.0 }
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
    shown: SharedPool<LinesDisplay>,
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
        shown.send_newest(pass, |display| {
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
        let locked = context.locked;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!locked, |ui| {
                let label = ui.label("Span");
                let field = egui::DragValue::new(&mut self.span).range(0.1..=LONGEST).speed(0.05).suffix(" s");
                ui.add(field).labelled_by(label.id).on_hover_text("Seconds across the view");
                let label = ui.label("Line height");
                let field = egui::DragValue::new(&mut self.line_height).range(12.0..=200.0).speed(0.5).suffix(" px");
                ui.add(field).labelled_by(label.id);
            });
            context.node_toolbar(ui);
        });
        let Some(DataStream::Events(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect events to view them");
        };
        let pool = context.shared::<Pool<LinesDisplay>>();
        let Some(display) = pool.as_deref().and_then(Pool::newest).filter(|d| d.lines > 0) else {
            let message = format!("{} · press Play to see them", stream.summary());
            return preview::unconnected(ui, context.theme, &message);
        };
        let theme = context.theme;
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 2.0, theme.surface_sunken);
        let row = self.line_height.clamp(12.0, 200.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| Self::lines_ui(ui, &display, row, theme));
        preview::repaint_with_data(ui);
    }
}

impl TtlViewer {
    /// The lines, each `row` high.
    fn lines_ui(ui: &mut egui::Ui, display: &LinesDisplay, row: f32, theme: &crate::core::theme::Colours) {
        let size = egui::vec2(ui.available_width(), row * display.lines as f32);
        let (rect, hover) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        // Line numbers on the left, counts on the right.
        let (label, counts) = (34.0, 170.0);
        let trace = egui::Rect::from_x_y_ranges(rect.left() + label..=rect.right() - counts, rect.y_range());
        // Filling from the left, then scrolling, with room for the future at the right (see `time_axis`).
        let (left, right) = crate::core::time_axis::window(0.0, display.now, display.span);
        let x = |time: f64| trace.left() + ((time - left) / (right - left)) as f32 * trace.width();
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
            points.push(Pos2::new(x(display.now), if level { high } else { low }));
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
        hover.on_hover_text(
            "Each line's level; and how many times it's gone up since acquisition started, and since recording last \
             started",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard_components::ephys::{Event, EventsBuffer};

    fn events(list: &[(u32, u8, bool)], pass_samples: usize) -> crate::Buffer {
        let events = list.iter().map(|&(sample, line, high)| Event { sample, line, high, time: None }).collect();
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

    /// Eight lines at the line height set, scrolling past those that don't fit. See tests/snapshots/.
    #[test]
    fn lines_are_as_high_as_set() {
        use crate::Nodes;
        use crate::core::engine::{Graph, NodeSet};
        use crate::standard_components::themes::THEMES;
        let mut graph = Graph::<Nodes>::default();
        let index = Nodes::TYPES.iter().position(|t| t.name == "TTL Viewer").unwrap();
        let viewer = graph.add(Nodes::create(index), [0.0, 0.0]);
        let changes = (0..8)
            .flat_map(|line| {
                (0..4).map(move |i| Change { line, time: 0.5 + i as f64 + line as f64 * 0.1, high: i % 2 == 0 })
            })
            .collect();
        let display = LinesDisplay {
            lines: 8,
            now: 5.0,
            span: 5.0,
            changes,
            ups: vec![2; 8],
            ups_recording: vec![0; 8],
            ..Default::default()
        };
        let stream = Some(DataStream::Events(Events { lines: 8, sample_rate: 30000.0 }));
        let exchange = crate::core::exchange::Exchange::default();
        exchange.shared(viewer, || Pool::holding(display));
        let mut harness = egui_kittest::Harness::builder().with_size([600.0, 220.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    viewer,
                    &[stream],
                    &THEMES[0].colours,
                    Some(&exchange),
                    None,
                    true,
                    false,
                    None,
                    |node, context| node.view_ui(ui, context),
                );
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("ttl_viewer");
    }
}
