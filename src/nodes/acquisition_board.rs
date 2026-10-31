use eframe::egui;
use serde::{Deserialize, Serialize};

use super::preview::{noise, spike_shape};
use super::{combo, setting, settings_grid};
use crate::edges::signals::{Continuous, Event, Events};
use crate::edges::{Buffer, Edges, Kind, Signal};
use crate::engine::{Category, Context, Node, NodeInfo, Output, Pass};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AcquisitionBoard {
    pub channels: u32,
    pub sample_rate: f32,
    pub ttl_lines: u32,
}

impl Default for AcquisitionBoard {
    fn default() -> Self {
        AcquisitionBoard { channels: 64, sample_rate: 30000.0, ttl_lines: 8 }
    }
}

impl Node<Edges> for AcquisitionBoard {
    const INFO: NodeInfo = NodeInfo {
        name: "Acquisition Board",
        category: Category::Source,
        description: "Headstage channels and digital inputs from an acquisition board (simulated for now).",
    };

    type State = ();

    /// Simulated: on each channel, in microvolts, a slow oscillation, noise, and now and then a spike; and a
    /// pulse on the first TTL line each second. Made from the sample number, so continuous across passes.
    fn process(&self, pass: &mut Pass<Edges>, _state: &mut ()) {
        let rate = self.sample_rate as f64;
        let samples = (rate * pass.duration).round() as usize;
        let first = pass.sequence * samples as u64;
        if let Some(Buffer::Continuous(out)) = pass.outputs.get_mut(0) {
            out.len = samples.min(out.capacity);
            // Spikes last a millisecond; whether one starts in each is down to chance.
            let ms = ((rate / 1000.0) as u64).max(1);
            for channel in 0..out.channels {
                let phase = channel as f64 * 0.7;
                for (i, value) in out.channel_mut(channel).iter_mut().enumerate() {
                    let n = first + i as u64;
                    let wave = 60.0 * (std::f64::consts::TAU * 8.0 * n as f64 / rate + phase).sin() as f32;
                    let spike = if noise(channel as u64, (n / ms) as i64) > 0.99 {
                        -150.0 * spike_shape((n % ms) as f32 / ms as f32)
                    } else {
                        0.0
                    };
                    *value = wave + 20.0 * noise(1000 + channel as u64, n as i64) + spike;
                }
            }
        }
        if let Some(Buffer::Events(out)) = pass.outputs.get_mut(1) {
            out.events.clear();
            let period = rate as u64;
            let next = first.div_ceil(period) * period;
            if self.ttl_lines > 0 && next < first + samples as u64 {
                out.events.push(Event { sample: (next - first) as u32, line: 0, high: true });
            }
        }
    }

    fn outputs(&self, _inputs: &[Option<Signal>]) -> Vec<Output<Edges>> {
        vec![
            Output {
                name: "continuous",
                kind: Kind::Continuous,
                signal: Some(Signal::Continuous(Continuous { channels: self.channels, sample_rate: self.sample_rate })),
            },
            Output {
                name: "events",
                kind: Kind::Events,
                signal: Some(Signal::Events(Events { lines: self.ttl_lines })),
            },
        ]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Channels", |ui| {
                combo(
                    ui,
                    "channels",
                    &mut self.channels,
                    &[(16, "16"), (32, "32"), (64, "64"), (128, "128"), (256, "256")],
                )
            });
            setting(ui, "Sample rate", |ui| {
                combo(
                    ui,
                    "rate",
                    &mut self.sample_rate,
                    &[(20000.0, "20 kHz"), (25000.0, "25 kHz"), (30000.0, "30 kHz")],
                )
            });
            setting(ui, "TTL lines", |ui| ui.add(egui::DragValue::new(&mut self.ttl_lines).range(0..=16)));
        });
    }
}
