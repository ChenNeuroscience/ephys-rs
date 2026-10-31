use eframe::egui::{self, Align2, Color32, FontId, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{preview, require_input, setting, settings_grid};
use crate::edges::{Edges, Kind, Setting, SettingKind, Signal};
use crate::engine::{Category, Context, Control, Input, Node, NodeInfo, Output};

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

impl Node<Edges> for LfpViewer {
    const INFO: NodeInfo = NodeInfo {
        name: "LFP Viewer",
        category: Category::Visualisation,
        description: "Scrolling traces of continuous signals, one row per channel.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        require_input(inputs, "a continuous signal")
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "gain", kind: SettingKind::Gain }]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let channels = match context.inputs[0] {
            Some(Signal::Continuous(c)) => c.channels,
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
                ui.add(egui::DragValue::new(&mut self.first_channel).range(0..=channels.saturating_sub(1)))
            });
            setting(ui, "Channels shown", |ui| ui.add(egui::DragValue::new(&mut self.channels_shown).range(1..=64)));
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let Some(Signal::Continuous(signal)) = context.inputs[0] else {
            return preview::unconnected(ui, "Connect a continuous signal to view it");
        };
        // With the gain control connected, the upstream gain can be adjusted from here.
        if let Some(Setting::Gain(mut gain)) = context.controls[0] {
            ui.horizontal(|ui| {
                ui.label("Upstream gain");
                ui.add(egui::DragValue::new(&mut gain.factor).range(0.0..=1000.0).speed(0.01).prefix("×"));
            });
            context.controls[0] = Some(Setting::Gain(gain));
        }
        let rect = preview::plot_area(ui);
        let channels = signal.channels;
        let first = self.first_channel.min(channels.saturating_sub(1));
        let shown = self.channels_shown.min(channels - first).max(1);
        let row = rect.height() / shown as f32;
        let painter = ui.painter_at(rect);

        // Simulated: slow oscillation plus noise and the odd spike, on a time grid so it scrolls smoothly.
        let columns = (rect.width() / 2.0) as i64;
        let per_second = columns as f64 / self.span as f64;
        let now = (ui.input(|i| i.time) * per_second) as i64;
        let amplitude = 200.0 / self.scale_uv;
        for i in 0..shown {
            let channel = (first + i) as u64;
            let centre = rect.top() + row * (i as f32 + 0.5);
            let points: Vec<Pos2> = (0..columns)
                .map(|c| {
                    let sample = now - columns + c;
                    let t = sample as f64 / per_second;
                    let wave = 0.3 * ((t * 7.0 + channel as f64 * 0.7).sin() as f32);
                    let spike = if preview::noise(channel, sample / 6) > 0.985 {
                        -2.0 * preview::spike_shape(0.3)
                    } else {
                        0.0
                    };
                    let value = (wave + 0.25 * preview::noise(channel, sample) + spike) * amplitude;
                    egui::pos2(rect.left() + c as f32 * 2.0, centre - value.clamp(-1.0, 1.0) * row * 0.5)
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(1.0, Color32::from_rgb(110, 170, 255))));
            painter.text(
                egui::pos2(rect.left() + 4.0, centre - row * 0.5 + 2.0),
                Align2::LEFT_TOP,
                format!("{channel}"),
                FontId::monospace(10.0),
                Color32::from_gray(120),
            );
        }
        ui.ctx().request_repaint();
    }
}
