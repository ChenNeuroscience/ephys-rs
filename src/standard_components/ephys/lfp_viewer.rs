//! The LFP Viewer: scrolling traces of continuous data, a row per channel. Each pass adds its samples to a few
//! seconds' worth for each channel shown, kept as the lowest and highest sample in each of the view's columns
//! (so spikes show however far it's zoomed out).

use eframe::egui::{self, Align2, FontId, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Control, EdgeType, Input, Node, NodeInfo, Output, Pass};
use crate::standard_components::ephys::Continuous;
use crate::standard_components::preview;
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Columns across the view.
const COLUMNS: usize = 1200;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LfpViewer {
    /// Seconds across the view.
    pub span: f32,
    /// Microvolts from a trace's centre line to the edge of its row.
    pub scale_uv: f32,
    pub first_channel: u32,
    pub channels_shown: u32,
}

impl Default for LfpViewer {
    fn default() -> Self {
        LfpViewer { span: 2.0, scale_uv: 200.0, first_channel: 0, channels_shown: 16 }
    }
}

/// The last `span` seconds of each channel shown: each column's lowest and highest sample, round a ring.
#[derive(Default)]
pub struct Traces {
    /// The input's sample rate.
    rate: f32,
    /// What it's for: the first channel, how many, and samples per column.
    shape: (usize, usize, usize),
    /// For each channel shown, `COLUMNS` lows then highs.
    lows: Vec<f32>,
    highs: Vec<f32>,
    /// The column being filled, and how many samples it has so far.
    head: usize,
    filled: usize,
    /// Columns filled since starting (up to `COLUMNS`).
    columns: usize,
}

impl Traces {
    fn reshape(&mut self, shape: (usize, usize, usize)) {
        if self.shape != shape {
            let (_, shown, _) = shape;
            *self = Traces {
                rate: self.rate,
                shape,
                lows: vec![0.0; shown * COLUMNS],
                highs: vec![0.0; shown * COLUMNS],
                ..Default::default()
            };
        }
    }
}

/// The traces as of the last pass, oldest column first.
#[derive(Default)]
pub struct TracesDisplay {
    first: usize,
    shown: usize,
    columns: usize,
    lows: Vec<f32>,
    highs: Vec<f32>,
}

impl Node<Edges> for LfpViewer {
    type State = Traces;

    const INFO: NodeInfo = NodeInfo {
        name: "LFP Viewer",
        category: Category::Visualisation,
        description: "Scrolling traces of continuous data, one row per channel.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "a continuous data stream")
    }

    fn display(&self) -> Option<Box<dyn std::any::Any + Send>> {
        Some(Box::new(TracesDisplay::default()))
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Traces {
        let rate = match inputs[0] {
            Some(DataStream::Continuous(c)) => c.sample_rate,
            _ => 30000.0,
        };
        Traces { rate, ..Default::default() }
    }

    fn process(&self, pass: &mut Pass<Edges>, traces: &mut Traces) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let rate = traces.rate as f64;
        let first = (self.first_channel as usize).min(input.channels.saturating_sub(1));
        let shown = (self.channels_shown as usize).min(input.channels - first).max(1);
        let per_column = ((self.span as f64 * rate) / COLUMNS as f64).round().max(1.0) as usize;
        traces.reshape((first, shown, per_column));
        for t in 0..input.len {
            for i in 0..shown {
                let x = input.channel(first + i)[t];
                let at = i * COLUMNS + traces.head;
                if traces.filled == 0 {
                    (traces.lows[at], traces.highs[at]) = (x, x);
                } else {
                    traces.lows[at] = traces.lows[at].min(x);
                    traces.highs[at] = traces.highs[at].max(x);
                }
            }
            traces.filled += 1;
            if traces.filled == per_column {
                (traces.head, traces.filled) = ((traces.head + 1) % COLUMNS, 0);
                traces.columns = (traces.columns + 1).min(COLUMNS);
            }
        }
        let Some(display) = pass.display::<TracesDisplay>() else { return };
        (display.first, display.shown, display.columns) = (first, shown, traces.columns);
        display.lows.clear();
        display.highs.clear();
        for i in 0..shown {
            // Oldest first: from the column after the one being filled, round to it.
            let row = i * COLUMNS..(i + 1) * COLUMNS;
            let (older, newer) = traces.lows[row.clone()].split_at(traces.head);
            display.lows.extend_from_slice(newer);
            display.lows.extend_from_slice(older);
            let (older, newer) = traces.highs[row].split_at(traces.head);
            display.highs.extend_from_slice(newer);
            display.highs.extend_from_slice(older);
        }
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "gain", kind: SettingKind::Gain }]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let channels = match context.inputs[0] {
            Some(DataStream::Continuous(c)) => c.channels,
            _ => 64,
        };
        settings_grid(ui, |ui| {
            setting(ui, "Span", |ui| {
                ui.add(egui::DragValue::new(&mut self.span).range(0.1..=30.0).speed(0.05).suffix(" s"))
            });
            setting(ui, "Scale", |ui| {
                ui.add(egui::DragValue::new(&mut self.scale_uv).range(10.0..=5000.0).speed(2.0).suffix(" µV"))
            });
            setting(ui, "First channel", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.first_channel)
                        .range(0..=channels.saturating_sub(1))
                        .custom_formatter(|v, _| format!("{}", v + 1.0))
                        .custom_parser(|s| s.trim().parse::<f64>().ok().map(|v| v - 1.0)),
                )
            });
            setting(ui, "Channels shown", |ui| ui.add(egui::DragValue::new(&mut self.channels_shown).range(1..=64)));
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let Some(DataStream::Continuous(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect a continuous data stream to view it");
        };
        // With the gain control connected, the upstream gain can be adjusted from here.
        if let Some(Setting::Gain(mut gain)) = context.controls[0] {
            ui.horizontal(|ui| {
                ui.label("Upstream gain");
                ui.add(egui::DragValue::new(&mut gain.factor).range(0.0..=1000.0).speed(0.01).prefix("×"));
            });
            context.controls[0] = Some(Setting::Gain(gain));
        }
        let rect = preview::plot_area(ui, context.theme);
        let Some(display) = context.display.and_then(|d| d.downcast_ref::<TracesDisplay>()) else {
            let message = format!("{} · press Play to see the traces", stream.summary());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                message,
                FontId::proportional(14.0),
                context.theme.text_secondary,
            );
            return;
        };
        let row = rect.height() / display.shown.max(1) as f32;
        let painter = ui.painter_at(rect);
        let column = rect.width() / COLUMNS as f32;
        // The newest column at the right; columns not yet filled are left empty.
        let start = COLUMNS - display.columns;
        for i in 0..display.shown {
            let centre = rect.top() + row * (i as f32 + 0.5);
            let y = |uv: f32| centre - (uv / self.scale_uv).clamp(-1.0, 1.0) * row * 0.5;
            let lows = &display.lows[i * COLUMNS..(i + 1) * COLUMNS];
            let highs = &display.highs[i * COLUMNS..(i + 1) * COLUMNS];
            let points: Vec<Pos2> = (start..COLUMNS)
                .flat_map(|c| {
                    let x = rect.left() + (c as f32 + 0.5) * column;
                    // Down and up each column, so its whole range is drawn.
                    let (a, b) = if c % 2 == 0 { (lows[c], highs[c]) } else { (highs[c], lows[c]) };
                    [egui::pos2(x, y(a)), egui::pos2(x, y(b))]
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(1.0, Continuous::INFO.colour)));
            painter.text(
                egui::pos2(rect.left() + 4.0, centre - row * 0.5 + 2.0),
                Align2::LEFT_TOP,
                format!("{}", display.first + i + 1),
                FontId::monospace(10.0),
                context.theme.text_secondary,
            );
        }
        ui.ctx().request_repaint();
    }
}
