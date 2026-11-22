//! Downsample: continuous data at a lower rate, keeping every so many samples (every 6th, from 30 kHz to 5 kHz,
//! say), on every channel. It doesn't filter: what's above half the new rate should be filtered out first (with a
//! Band-pass Filter), or it aliases.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::standard::ephys::Continuous;
use crate::standard::widgets::{format_rate, input_summary, require_input, setting, settings_grid};
use ephys_rs::engine::{
    Buffers, Category, CommandContext, CommandSource, NoPorts, Node, NodeInfo, PassContext, Prepare, PrepareContext,
    Prepared, TreeShake, UiContext,
};

ephys_rs::ports! {
    continuous: DataStreamIn<Continuous>,
    downsampled: DataStreamOut<Continuous>,
}

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

impl<E: PortEdges> Node<E> for Downsample {
    type Ports = Ports;
    const INFO: NodeInfo = NodeInfo {
        name: "Downsample",
        category: Category::Processing,
        description: "Continuous data at a lower rate, keeping every so many samples, on every channel. Filter out \
                      what's above half the new rate first.",
    };

    type State = Downsampling;
    type WorkerState = ();
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], _recording: bool) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Downsampling, (), Self> {
        if context.same_streams() {
            return Prepared::nothing().into();
        }
        let step = context.data_stream_in::<DataStreamIn>().continuous.and_then(|input| self.step(input)).unwrap_or(0);
        Prepared::shared(Downsampling { step, so_far: 0 }).into()
    }

    fn process(
        &self,
        _pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        outputs: DataStreamOut<Buffers>,
        downsampling: &mut Downsampling,
        _: &mut (),
    ) {
        let output = outputs.downsampled;
        output.len = 0;
        let (Some(input), step) = (inputs.continuous, downsampling.step) else { return };
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

    fn val_out(&self) -> NoPorts {
        NoPorts
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {}
    }

    fn output_streams(&self, inputs: &DataStreamIn) -> DataStreamOut {
        let input = inputs.continuous.filter(|&input| self.step(input).is_some());
        DataStreamOut { downsampled: input.map(|input| Continuous { sample_rate: self.rate_hz, ..input }) }
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            settings_grid(ui, |ui| {
                setting(ui, "Rate", |ui| {
                    ui.add(egui::DragValue::new(&mut node.rate_hz).range(1.0..=100_000.0).speed(10.0).suffix(" Hz"))
                });
            });
            input_summary(ui, context);
        });
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        let mut warnings = require_input(inputs.any_connected(), "a continuous data stream");
        if let Some(input) = inputs.continuous
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
    use crate::Edges;
    use ephys_rs::engine::DataStreamType;

    /// Keeping every 6th of passes 4, 9 and 5 samples long keeps samples 0, 6 and 12 (of 18), on each channel.
    #[test]
    fn it_keeps_every_step_th_sample_across_passes() {
        let node = Downsample { rate_hz: 5000.0 };
        let input = Continuous { channels: 2, sample_rate: 30000.0 };
        let inputs = DataStreamIn { continuous: Some(input) };
        let mut downsampling = ephys_rs::engine::fresh_state::<Edges, _>(&node, &inputs);
        let output = Node::<Edges>::output_streams(&node, &inputs).downsampled.expect("a 5 kHz stream");
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
            let mut out = output.buffer(0.02);
            let (inputs, outputs) = (DataStreamIn { continuous: Some(&data) }, DataStreamOut { downsampled: &mut out });
            node.process(&mut PassContext::<Edges>::new(0, 0.02), inputs, outputs, &mut downsampling, &mut ());
            for (channel, kept) in kept.iter_mut().enumerate() {
                kept.extend_from_slice(out.channel(channel));
            }
        }
        assert_eq!(kept, vec![vec![0.0, 6.0, 12.0], vec![1000.0, 1006.0, 1012.0]]);
    }

    #[test]
    fn the_input_must_be_a_multiple_of_its_rate() {
        let node = Downsample { rate_hz: 5000.0 };
        let input = |hz| DataStreamIn { continuous: Some(Continuous { channels: 1, sample_rate: hz }) };
        assert!(Node::<Edges>::warnings(&node, &input(30000.0)).is_empty());
        assert!(Node::<Edges>::warnings(&node, &input(5000.0)).is_empty(), "as it is");
        assert_eq!(Node::<Edges>::warnings(&node, &input(25600.0)).len(), 1);
        assert!(Node::<Edges>::output_streams(&node, &input(25600.0)).downsampled.is_none());
    }
}
