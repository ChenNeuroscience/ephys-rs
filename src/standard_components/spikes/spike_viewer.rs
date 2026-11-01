//! The Spike Viewer: each group's (tetrode's, say) recent spikes overlaid, a panel per channel of the group,
//! with each channel's threshold; dragging adjusts the detector's threshold, through the threshold control.

use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Control, CopyTo, EdgeType, Input, Node, NodeInfo, Output, Pass};
use crate::core::exchange::Latest;
use crate::standard_components::preview;
use crate::standard_components::spikes::{Spikes, Threshold};
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeViewer {
    pub columns: u32,
    pub first_group: u32,
    pub groups_shown: u32,
    /// Most recent waveforms overlaid on each group.
    pub waveforms: u32,
    /// Microvolts from a panel's centre line to its edge.
    pub scale_uv: f32,
}

impl Default for SpikeViewer {
    fn default() -> Self {
        SpikeViewer { columns: 4, first_group: 0, groups_shown: 16, waveforms: 30, scale_uv: 250.0 }
    }
}

/// Each group's most recent spikes, round a ring, and each channel's latest threshold.
#[derive(Default)]
pub struct Recent {
    stream: Option<Spikes>,
    keep: usize,
    /// For each group, `keep` waveforms (each `group_size × samples`).
    waveforms: Vec<f32>,
    /// For each group: its next slot, and how many it has (up to `keep`).
    head: Vec<usize>,
    count: Vec<usize>,
    /// Spikes found, for each group.
    total: Vec<u64>,
    thresholds: Vec<f32>,
    shown: Latest<RecentDisplay>,
}

impl crate::core::engine::PassState for Recent {
    const WHY: &'static str = "Each group's most recent spikes (round a ring), and how many it's had, which \
                               each pass adds to.";
}

/// The groups shown, as of the last pass.
#[derive(Default)]
pub struct RecentDisplay {
    stream: Option<Spikes>,
    first: usize,
    shown: usize,
    keep: usize,
    /// For each group shown: its waveforms, newest first, and how many.
    waveforms: Vec<f32>,
    count: Vec<usize>,
    total: Vec<u64>,
    thresholds: Vec<f32>,
}

impl Node<Edges> for SpikeViewer {
    type State = Recent;

    const INFO: NodeInfo = NodeInfo {
        name: "Spike Viewer",
        category: Category::Visualisation,
        description: "Each group's (tetrode's, say) recent spike waveforms overlaid, a panel per channel, with the \
                      thresholds: drag to adjust them.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "spikes", kind: Kind::Spikes }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "spikes")
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "threshold", kind: SettingKind::Threshold, read_only: false }]
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Window)
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Recent {
        let Some(DataStream::Spikes(stream)) = inputs[0] else { return Recent::default() };
        let groups = stream.groups() as usize;
        let keep = self.waveforms.clamp(1, 200) as usize;
        let size = (stream.group_size * stream.waveform_samples()) as usize;
        Recent {
            stream: Some(stream),
            keep,
            waveforms: vec![0.0; groups * keep * size],
            head: vec![0; groups],
            count: vec![0; groups],
            total: vec![0; groups],
            thresholds: vec![0.0; stream.channels as usize],
            shown: Latest::default(),
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, recent: &mut Recent) {
        let Some(Some(Buffer::Spikes(input))) = pass.inputs.first() else { return };
        let Some(stream) = recent.stream else { return };
        let size = input.group_size * input.samples;
        let keep = recent.keep;
        for (i, spike) in input.spikes.iter().enumerate() {
            let group = spike.group as usize;
            let slot = (group * keep + recent.head[group]) * size;
            recent.waveforms[slot..slot + size].copy_from_slice(input.waveform(i));
            recent.head[group] = (recent.head[group] + 1) % keep;
            recent.count[group] = (recent.count[group] + 1).min(keep);
            recent.total[group] += 1;
        }
        let channels = recent.thresholds.len();
        recent.thresholds.copy_from_slice(&input.thresholds[..channels]);

        let groups = stream.groups() as usize;
        let first = (self.first_group as usize).min(groups.saturating_sub(1));
        let shown = (self.groups_shown as usize).min(groups - first);
        let Recent { waveforms, head, count, total, thresholds, shown: latest, .. } = recent;
        latest.send(pass, |display| {
            (display.stream, display.first, display.shown, display.keep) = (Some(stream), first, shown, keep);
            display.waveforms.clear();
            display.count.clear();
            display.total.clear();
            for group in first..first + shown {
                // Newest first.
                for age in 0..count[group] {
                    let slot = (head[group] + keep - 1 - age) % keep;
                    let at = (group * keep + slot) * size;
                    display.waveforms.extend_from_slice(&waveforms[at..at + size]);
                }
                display.count.push(count[group]);
                display.total.push(total[group]);
            }
            display.thresholds.clone_from(thresholds);
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let groups = match context.inputs[0] {
            Some(DataStream::Spikes(s)) => s.groups(),
            _ => 16,
        };
        settings_grid(ui, |ui| {
            setting(ui, "Columns", |ui| ui.add(egui::DragValue::new(&mut self.columns).range(1..=16)));
            setting(ui, "First group", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.first_group)
                        .range(0..=groups.saturating_sub(1))
                        .custom_formatter(|v, _| format!("{}", v + 1.0))
                        .custom_parser(|s| s.trim().parse::<f64>().ok().map(|v| v - 1.0)),
                )
            });
            setting(ui, "Groups shown", |ui| ui.add(egui::DragValue::new(&mut self.groups_shown).range(1..=64)));
            setting(ui, "Waveforms", |ui| ui.add(egui::DragValue::new(&mut self.waveforms).range(1..=200)));
            setting(ui, "Scale", |ui| {
                ui.add(egui::DragValue::new(&mut self.scale_uv).range(10.0..=5000.0).speed(2.0).suffix(" µV"))
            });
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        crate::standard_components::widgets::view_settings(ui, |ui| self.settings_ui(ui, context));
        let Some(DataStream::Spikes(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect spikes to view them");
        };
        let display = context.display.and_then(|d| d.downcast_ref::<RecentDisplay>()).filter(|d| d.stream.is_some());
        let Some(display) = display else {
            let message = format!("{} · press Play to see them", stream.summary());
            return preview::unconnected(ui, context.theme, &message);
        };
        let rect = preview::plot_area(ui, context.theme);
        let painter = ui.painter_at(rect);
        let shown = display.shown.max(1);
        let columns = (self.columns as usize).clamp(1, shown);
        let rows = shown.div_ceil(columns);
        let cell = egui::vec2(rect.width() / columns as f32, rect.height() / rows as f32);
        let (k, samples) = (stream.group_size as usize, stream.waveform_samples() as usize);
        let size = k * samples;

        // With the threshold control connected, drag up or down to change the threshold.
        let mut threshold = match context.controls[0] {
            Some(Setting::Threshold(t)) => Some(t),
            _ => None,
        };
        if let Some(t) = &mut threshold {
            let response = ui.interact(rect, ui.scope_id().with("threshold"), Sense::drag());
            // The thresholds' average size, per unit of threshold: so many µV a unit.
            let sizes: Vec<f32> = display.thresholds.iter().map(|t| t.abs()).filter(|t| *t > 0.0).collect();
            if !sizes.is_empty() && t.sd > 0.0 {
                let per_unit = sizes.iter().sum::<f32>() / sizes.len() as f32 / t.sd;
                let uv_per_pixel = self.scale_uv / (cell.y * 0.5);
                let downwards = display.thresholds.iter().any(|t| *t < 0.0);
                let drag = response.drag_delta().y * if downwards { 1.0 } else { -1.0 };
                t.sd = (t.sd + drag * uv_per_pixel / per_unit).clamp(1.0, 20.0);
            }
            response.on_hover_cursor(egui::CursorIcon::ResizeVertical).on_hover_text("Drag to set the threshold");
            context.controls[0] = Some(Setting::Threshold(*t));
        }

        let mut offset = 0;
        for (i, &count) in display.count.iter().enumerate() {
            let group = display.first + i;
            let min = rect.min + egui::vec2((i % columns) as f32 * cell.x, (i / columns) as f32 * cell.y);
            let tile = Rect::from_min_size(min, cell).shrink(3.0);
            painter.rect_stroke(tile, 2.0, Stroke::new(1.0, context.theme.divider), egui::StrokeKind::Inside);
            let panel_width = tile.width() / k as f32;
            let y = |uv: f32| tile.center().y - (uv / self.scale_uv).clamp(-1.0, 1.0) * tile.height() * 0.5;
            let channels = stream.group_channels(group as u32);
            for (c, channel) in channels.clone().enumerate() {
                let left = tile.left() + c as f32 * panel_width;
                if c > 0 {
                    painter.vline(left, tile.y_range(), Stroke::new(1.0, context.theme.divider.gamma_multiply(0.5)));
                }
                let x = |s: usize| left + (s as f32 + 0.5) / samples as f32 * panel_width;
                // Oldest first, so the newest is on top.
                for age in (0..count).rev() {
                    let waveform = &display.waveforms[offset + age * size + c * samples..][..samples];
                    let points: Vec<Pos2> = waveform.iter().enumerate().map(|(s, v)| egui::pos2(x(s), y(*v))).collect();
                    let alpha = 1.0 - 0.8 * age as f32 / display.keep as f32;
                    painter.add(Shape::line(points, Stroke::new(1.0, Spikes::INFO.colour.gamma_multiply(alpha))));
                }
                if let Some(&threshold_uv) = display.thresholds.get(channel as usize) {
                    let colour = Threshold::INFO.colour.gamma_multiply(0.8);
                    painter.hline(left..=left + panel_width, y(threshold_uv), Stroke::new(1.0, colour));
                }
            }
            offset += count * size;
            painter.text(
                tile.left_top() + egui::vec2(3.0, 2.0),
                Align2::LEFT_TOP,
                format!("{} · {} spikes", group + 1, display.total[i]),
                FontId::monospace(10.0),
                context.theme.text_secondary,
            );
        }
        let note = match threshold {
            Some(t) => format!("threshold {}", t.summary()),
            None => "connect the threshold control to a detector to set it here".into(),
        };
        let colour = if threshold.is_some() { Threshold::INFO.colour } else { context.theme.text_secondary };
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
