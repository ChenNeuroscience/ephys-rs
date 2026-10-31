use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{preview, require_input, setting, settings_grid};
use crate::edges::settings::Threshold;
use crate::edges::{Edges, Kind, Setting, SettingKind, Signal};
use crate::engine::{Category, Context, Control, EdgeType, Input, Node, NodeInfo, Output};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeViewer {
    pub columns: u32,
    pub channels_shown: u32,
    /// Most recent waveforms overlaid on each channel.
    pub waveforms: u32,
}

impl Default for SpikeViewer {
    fn default() -> Self {
        SpikeViewer { columns: 4, channels_shown: 16, waveforms: 20 }
    }
}

impl Node<Edges> for SpikeViewer {
    const INFO: NodeInfo = NodeInfo {
        name: "Spike Viewer",
        category: Category::Visualisation,
        description: "Recent spike waveforms overlaid, one panel per channel.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "spikes", kind: Kind::Spikes }]
    }

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<Signal>]) -> Vec<String> {
        require_input(inputs, "spikes")
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "threshold", kind: SettingKind::Threshold }]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Columns", |ui| ui.add(egui::DragValue::new(&mut self.columns).range(1..=16)));
            setting(ui, "Channels shown", |ui| ui.add(egui::DragValue::new(&mut self.channels_shown).range(1..=64)));
            setting(ui, "Waveforms", |ui| ui.add(egui::DragValue::new(&mut self.waveforms).range(1..=200)));
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let Some(Signal::Spikes(signal)) = context.inputs[0] else {
            return preview::unconnected(ui, "Connect spikes to view them");
        };
        let rect = preview::plot_area(ui);
        let painter = ui.painter_at(rect);
        let shown = self.channels_shown.min(signal.channels).max(1);
        let columns = self.columns.clamp(1, shown);
        let rows = shown.div_ceil(columns);
        let cell = egui::vec2(rect.width() / columns as f32, rect.height() / rows as f32);
        // Simulated: each channel has its own amplitude, with a new spike every fifth of a second.
        let latest = (ui.input(|i| i.time) * 5.0) as i64;
        // With the threshold control connected, show the threshold on every channel and drag to change it.
        let mut threshold = match context.controls[0] {
            Some(Setting::Threshold(t)) => Some(t),
            _ => None,
        };
        if let Some(t) = &mut threshold {
            let response = ui.interact(rect, ui.scope_id().with("threshold"), Sense::drag());
            t.sd = (t.sd + response.drag_delta().y / (NOISE_SD * cell.y)).clamp(1.0, 20.0);
            response.on_hover_cursor(egui::CursorIcon::ResizeVertical).on_hover_text("Drag to set the threshold");
            context.controls[0] = Some(Setting::Threshold(*t));
        }
        for channel in 0..shown {
            let min = rect.min + egui::vec2((channel % columns) as f32 * cell.x, (channel / columns) as f32 * cell.y);
            let cell_rect = Rect::from_min_size(min, cell).shrink(3.0);
            painter.rect_stroke(cell_rect, 2.0, Stroke::new(1.0, Color32::from_gray(40)), egui::StrokeKind::Inside);
            let amplitude = 0.4 + 0.3 * preview::noise(channel as u64, -1);
            for age in 0..self.waveforms {
                let spike = latest - age as i64;
                let alpha = 1.0 - age as f32 / self.waveforms as f32;
                let scale = amplitude * (1.0 + 0.15 * preview::noise(channel as u64, spike));
                let points: Vec<Pos2> = (0..=32)
                    .map(|s| {
                        let t = s as f32 / 32.0;
                        let v = scale * preview::spike_shape(t) + 0.04 * preview::noise(spike as u64, s);
                        egui::pos2(
                            cell_rect.left() + t * cell_rect.width(),
                            cell_rect.center().y - v * cell_rect.height(),
                        )
                    })
                    .collect();
                painter
                    .add(Shape::line(points, Stroke::new(1.0, Color32::from_rgb(255, 170, 70).gamma_multiply(alpha))));
            }
            if let Some(t) = threshold {
                let y = cell_rect.center().y + t.sd * NOISE_SD * cell_rect.height();
                painter.hline(cell_rect.x_range(), y, Stroke::new(1.0, Threshold::INFO.colour.gamma_multiply(0.8)));
            }
            painter.text(
                cell_rect.left_top() + egui::vec2(3.0, 2.0),
                Align2::LEFT_TOP,
                format!("{channel}"),
                FontId::monospace(10.0),
                Color32::from_gray(120),
            );
        }
        let note = match threshold {
            Some(t) => format!("threshold {}", t.summary()),
            None => "connect the threshold control to a detector to set it here".into(),
        };
        let colour = if threshold.is_some() { Threshold::INFO.colour } else { Color32::from_gray(110) };
        painter.text(
            rect.right_top() + egui::vec2(-6.0, 4.0),
            Align2::RIGHT_TOP,
            note,
            FontId::proportional(11.0),
            colour,
        );
        ui.ctx().request_repaint();
    }
}

/// The simulated noise's standard deviation, as a fraction of a channel's panel height.
const NOISE_SD: f32 = 0.06;
