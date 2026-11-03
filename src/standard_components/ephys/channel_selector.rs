//! The Channel Selector: passes on some of a continuous stream's channels (such as "1-4, 9"), or, until the
//! selection's applied, all of them. The selection's a setting, which an LFP Viewer downstream can choose: look
//! through all the channels, tick the ones to keep, then apply it.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, PassState, SettingPort};
use crate::standard_components::ephys::reference::parse_channels;
use crate::standard_components::ephys::{ChannelSelection, Continuous};
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelSelector {
    /// Whether only the selected channels are passed on (otherwise all are).
    pub apply: bool,
    /// The channels selected, such as "all" or "1-4, 9" (counting from 1).
    pub channels: String,
}

impl Default for ChannelSelector {
    fn default() -> Self {
        ChannelSelector { apply: false, channels: "all".into() }
    }
}

impl ChannelSelector {
    /// The channels passed on, of `count`: the selection, if it's applied and makes sense, else all.
    pub fn passed(&self, count: u32) -> Vec<u32> {
        match parse_channels(&self.channels, count) {
            Ok(channels) if self.apply && !channels.is_empty() => channels,
            _ => (0..count).collect(),
        }
    }
}

/// The channels passed on, as of the settings it was worked out from.
#[derive(Default)]
pub struct Passed {
    from: Option<(bool, String)>,
    channels: Vec<u32>,
}

impl PassState for Passed {
    const WHY: &'static str = "The channels passed on, worked out from the settings.";
    const CARRIED_OVER: bool = false;
}

impl Node<Edges> for ChannelSelector {
    const INFO: NodeInfo = NodeInfo {
        name: "Channel Selector",
        category: Category::Processing,
        description: "Passes on some of the channels: all of them until the selection's applied.",
    };

    type State = Passed;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Continuous(c)) => {
                let channels = self.passed(c.channels).len() as u32;
                Some(DataStream::Continuous(Continuous { channels, ..c }))
            }
            _ => None,
        };
        vec![Output { name: "selected", kind: Kind::Continuous, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if let Some(DataStream::Continuous(c)) = inputs[0] {
            match parse_channels(&self.channels, c.channels) {
                Err(e) => warnings.push(format!("{e}: passing on all the channels")),
                Ok(channels) if channels.is_empty() && self.apply => {
                    warnings.push("No channels selected: passing on all of them".into())
                }
                _ => {}
            }
        }
        warnings
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Passed {
        Passed::default()
    }

    fn process(&self, pass: &mut Pass<Edges>, passed: &mut Passed) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        // Worked out again only when the settings change.
        if passed.from.as_ref().is_none_or(|(apply, channels)| *apply != self.apply || *channels != self.channels) {
            passed.channels = self.passed(input.channels as u32);
            passed.from = Some((self.apply, self.channels.clone()));
        }
        output.len = input.len.min(output.capacity);
        for (to, &from) in passed.channels.iter().take(output.channels).enumerate() {
            let len = output.len;
            output.channel_mut(to).copy_from_slice(&input.channel(from as usize)[..len]);
        }
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        let selection = ChannelSelection { apply: self.apply, channels: self.channels.clone() };
        vec![SettingPort { name: "selection", value: selection.into() }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::ChannelSelection(selection) = value {
            (self.apply, self.channels) = (selection.apply, selection.channels);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Channels", |ui| {
                ui.text_edit_singleline(&mut self.channels).on_hover_text("Such as \"all\" or \"1-4, 9\"")
            });
            setting(ui, "Apply", |ui| ui.checkbox(&mut self.apply, "pass on only these"));
        });
        input_summary(ui, context.inputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DataStreamType, PASS_DURATION};

    #[test]
    fn passes_on_the_selected_channels_once_applied() {
        let stream = Continuous { channels: 4, sample_rate: 1000.0 };
        let mut selector = ChannelSelector { apply: false, channels: "2, 4".into() };
        let run = |selector: &ChannelSelector| {
            let Some(DataStream::Continuous(out)) = selector.outputs(&[Some(DataStream::Continuous(stream))])[0].stream
            else {
                unreachable!()
            };
            let mut input = stream.buffer(PASS_DURATION);
            input.len = 3;
            for c in 0..4 {
                input.channel_mut(c).fill(c as f32);
            }
            let input = Buffer::Continuous(input);
            let mut outputs = [Buffer::Continuous(out.buffer(PASS_DURATION))];
            let inputs = [Some(&input)];
            let mut pass = Pass::new(0, PASS_DURATION, &inputs, &mut outputs);
            selector.process(&mut pass, &mut Passed::default());
            let Buffer::Continuous(output) = &outputs[0] else { unreachable!() };
            (0..output.channels).map(|c| output.channel(c)[2]).collect::<Vec<_>>()
        };
        assert_eq!(run(&selector), vec![0.0, 1.0, 2.0, 3.0], "all, until applied");
        selector.apply = true;
        assert_eq!(run(&selector), vec![1.0, 3.0]);
    }
}
