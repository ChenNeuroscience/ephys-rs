use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::Buffer;
use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass};
use crate::standard_components::widgets::format_rate;
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
use crate::{DataStream, Edges, Kind};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandPassFilter {
    pub low_hz: f32,
    pub high_hz: f32,
}

impl Default for BandPassFilter {
    fn default() -> Self {
        BandPassFilter { low_hz: 300.0, high_hz: 6000.0 }
    }
}

impl Node<Edges> for BandPassFilter {
    const INFO: NodeInfo = NodeInfo {
        name: "Band-pass Filter",
        category: Category::Processing,
        description: "Keeps frequencies between the low and high cut-offs, on every channel.",
    };

    type State = Filters;

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Filters {
        let Some(DataStream::Continuous(stream)) = inputs[0] else { return Filters::default() };
        let mut filters =
            Filters { channels: vec![[Biquad::default(); 2]; stream.channels as usize], ..Default::default() };
        filters.tune(self.low_hz, self.high_hz, stream.sample_rate);
        filters
    }

    fn in_place(&self, _output: usize) -> Option<usize> {
        Some(0)
    }

    fn process(&self, pass: &mut Pass<Edges>, filters: &mut Filters) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        if filters.channels.len() < output.channels {
            return;
        }
        // The cut-offs can change while running: keep the history, with new coefficients.
        if (filters.low_hz, filters.high_hz) != (self.low_hz, self.high_hz) {
            let rate = filters.rate;
            filters.tune(self.low_hz, self.high_hz, rate);
        }
        output.transform(input, |channel, x| {
            let [high_pass, low_pass] = &mut filters.channels[channel];
            low_pass.run(high_pass.run(x))
        });
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        vec![Output { name: "filtered", kind: Kind::Continuous, stream: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if self.low_hz >= self.high_hz {
            warnings.push("The low cut-off must be below the high cut-off".into());
        }
        if let Some(DataStream::Continuous(c)) = inputs[0]
            && self.high_hz >= c.sample_rate / 2.0
        {
            warnings.push(format!(
                "The high cut-off must be below {} (half the sample rate)",
                format_rate(c.sample_rate / 2.0)
            ));
        }
        warnings
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Low cut-off", |ui| {
                ui.add(egui::DragValue::new(&mut self.low_hz).range(0.1..=20000.0).speed(1.0).suffix(" Hz"))
            });
            setting(ui, "High cut-off", |ui| {
                ui.add(egui::DragValue::new(&mut self.high_hz).range(1.0..=20000.0).speed(10.0).suffix(" Hz"))
            });
        });
        input_summary(ui, context.inputs);
    }
}

/// Each channel's filter: a high-pass then a low-pass section, with their history carried between passes.
#[derive(Default)]
pub struct Filters {
    channels: Vec<[Biquad; 2]>,
    /// What the coefficients are for.
    low_hz: f32,
    high_hz: f32,
    rate: f32,
}

impl Filters {
    /// Sets every channel's coefficients for these cut-offs, keeping their history.
    fn tune(&mut self, low_hz: f32, high_hz: f32, rate: f32) {
        let (high_pass, low_pass) = (Biquad::new(true, low_hz, rate), Biquad::new(false, high_hz, rate));
        for [hp, lp] in &mut self.channels {
            hp.set_coefficients(&high_pass);
            lp.set_coefficients(&low_pass);
        }
        (self.low_hz, self.high_hz, self.rate) = (low_hz, high_hz, rate);
    }
}

/// A second-order Butterworth section (coefficients from the RBJ audio EQ cookbook).
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(high_pass: bool, cutoff: f32, rate: f32) -> Self {
        let w = std::f32::consts::TAU * (cutoff / rate).clamp(1e-5, 0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = if high_pass {
            ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0)
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0)
        };
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: -2.0 * cos / a0, a2: (1.0 - alpha) / a0, z1: 0.0, z2: 0.0 }
    }

    fn set_coefficients(&mut self, from: &Biquad) {
        (self.b0, self.b1, self.b2, self.a1, self.a2) = (from.b0, from.b1, from.b2, from.a1, from.a2);
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}
