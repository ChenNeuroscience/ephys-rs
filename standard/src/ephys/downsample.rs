//! Downsample: continuous data at a lower rate, keeping every so many samples (every 6th, from 30 kHz to 5 kHz,
//! say), on every channel. It doesn't filter: what's above half the new rate should be filtered out first (with a
//! Band-pass Filter), or it aliases.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::Buffer;
use crate::standard::ephys::Continuous;
use crate::standard::widgets::{format_rate, input_summary, require_input, setting, settings_grid};
use crate::{DataStream, Edges, Kind};
use ephys_rs::engine::{Category, Input, Node, NodeInfo, Output, Pass, TreeShake, UiContext};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Downsample {
    /// The rate it makes, which the input's must be a multiple of.
    pub rate_hz: f32,
}

impl Default for Downsample {
    fn default() -> Self {
        Downsample { rate_hz: 1000.0 }
    }
}

impl Downsample {
    /// Every how many of `input`'s samples it keeps, if its rate's a multiple of the one it makes.
    fn step(&self, input: Continuous) -> Option<usize> {
        let step = (input.sample_rate / self.rate_hz).round();
        let multiple = step >= 1.0 && (input.sample_rate - step * self.rate_hz).abs() < 1e-3 * input.sample_rate;
        multiple.then_some(step as usize)
    }
}

impl Node<Edges> for Downsample {
    const INFO: NodeInfo = NodeInfo {
        name: "Downsample",
        category: Category::Processing,
        description: "Continuous data at a lower rate, keeping every so many samples, on every channel. Filter out \
                      what's above half the new rate first.",
    };

    type State = Downsampling;
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Downsampling {
        let step = match inputs[0] {
            Some(DataStream::Continuous(input)) => self.step(input).unwrap_or(0),
            _ => 0,
        };
        Downsampling { step, so_far: 0 }
    }

    fn process(&self, pass: &mut Pass<Edges>, downsampling: &mut Downsampling) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        output.len = 0;
        let (Some(input), step) = (input, downsampling.step) else { return };
        if step == 0 {
            return;
        }
        let first = keeping_from(downsampling.so_far, step);
        output.len = input.len.saturating_sub(first).div_ceil(step).min(output.capacity);
        for channel in 0..output.channels.min(input.channels) {
            let kept = input.channel(channel).iter().skip(first).step_by(step);
            for (to, &from) in output.channel_mut(channel).iter_mut().zip(kept) {
                *to = from;
            }
        }
        downsampling.so_far += input.len as u64;
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Continuous(input)) if self.step(input).is_some() => {
                Some(DataStream::Continuous(Continuous { sample_rate: self.rate_hz, ..input }))
            }
            _ => None,
        };
        vec![Output { name: "downsampled", kind: Kind::Continuous, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if let Some(DataStream::Continuous(input)) = inputs[0]
            && self.step(input).is_none()
        {
            warnings.push(format!(
                "The input's rate ({}) must be a multiple of {}",
                format_rate(input.sample_rate),
                format_rate(self.rate_hz)
            ));
        }
        warnings
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            settings_grid(ui, |ui| {
                setting(ui, "Rate", |ui| {
                    ui.add(egui::DragValue::new(&mut node.rate_hz).range(1.0..=100_000.0).speed(10.0).suffix(" Hz"))
                });
            });
            input_summary(ui, context.inputs);
        });
    }
}

/// Which samples it keeps, carried from pass to pass.
#[derive(Default)]
pub struct Downsampling {
    /// Every how many it keeps (0 if it can't).
    step: usize,
    /// Samples so far.
    so_far: u64,
}

impl ephys_rs::engine::PassState for Downsampling {
    const WHY: &'static str = "How many samples have come so far, so each pass keeps every so many carrying on from \
                               the last, whatever each pass's length.";
}

/// The first of the next samples to keep, keeping every `step`th, with `so_far` before them.
fn keeping_from(so_far: u64, step: usize) -> usize {
    (step - (so_far % step as u64) as usize) % step
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephys_rs::engine::DataStreamType;

    /// Keeping every 6th of passes 4, 9 and 5 samples long keeps samples 0, 6 and 12 (of 18), on each channel.
    #[test]
    fn it_keeps_every_step_th_sample_across_passes() {
        let node = Downsample { rate_hz: 5000.0 };
        let input = Continuous { channels: 2, sample_rate: 30000.0 };
        let mut downsampling = node.initial_state(&[Some(DataStream::Continuous(input))]);
        let Some(DataStream::Continuous(output)) = node.outputs(&[Some(DataStream::Continuous(input))])[0].stream
        else {
            panic!("a 5 kHz stream");
        };
        assert_eq!(output.sample_rate, 5000.0);
        let (mut so_far, mut kept) = (0, vec![Vec::new(); 2]);
        for len in [4usize, 9, 5] {
            let mut data = input.buffer(0.02);
            data.len = len;
            for channel in 0..2 {
                for (t, x) in data.channel_mut(channel).iter_mut().enumerate() {
                    *x = (channel * 1000 + so_far + t) as f32;
                }
            }
            so_far += len;
            let data = Buffer::Continuous(data);
            let inputs = [Some(&data)];
            let mut outputs = [Buffer::Continuous(output.buffer(0.02))];
            node.process(&mut Pass::new(0, 0.02, &inputs, &mut outputs), &mut downsampling);
            let [Buffer::Continuous(out)] = &outputs else { unreachable!() };
            for (channel, kept) in kept.iter_mut().enumerate() {
                kept.extend_from_slice(out.channel(channel));
            }
        }
        assert_eq!(kept, vec![vec![0.0, 6.0, 12.0], vec![1000.0, 1006.0, 1012.0]]);
    }

    #[test]
    fn the_input_must_be_a_multiple_of_its_rate() {
        let node = Downsample { rate_hz: 5000.0 };
        let input = |hz| [Some(DataStream::Continuous(Continuous { channels: 1, sample_rate: hz }))];
        assert!(node.warnings(&input(30000.0)).is_empty());
        assert!(node.warnings(&input(5000.0)).is_empty(), "as it is");
        assert_eq!(node.warnings(&input(25600.0)).len(), 1);
        assert!(node.outputs(&input(25600.0))[0].stream.is_none());
    }
}
