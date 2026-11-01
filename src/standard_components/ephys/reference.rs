//! The Reference node: re-references channels, subtracting from each a reference made from other channels.
//!
//! As in Open Ephys: its common average reference (the average of some channels, times a gain, from others),
//! and its channel map's reference channels (each channel referenced to one other). Besides those: the median
//! rather than the average (a common median reference: a spike on one reference channel hardly moves it), and
//! averages or medians taken separately within groups of channels (each shank of a probe, say).

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass};
use crate::standard_components::widgets::{input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Method {
    /// Every channel referenced to one channel (counting from 1).
    Channel { channel: u32 },
    /// The average of the reference channels.
    Average,
    /// The median of the reference channels.
    Median,
    /// Each channel referenced to a channel of its own (counting from 1), or not at all (0).
    PerChannel { references: Vec<u32> },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reference {
    pub method: Method,
    /// For an average or median: the channels it's of, such as "all" or "1-4, 9".
    pub reference_channels: String,
    /// For an average or median: whether it's taken separately in each group of this many channels (from each
    /// group's reference channels, for that group's channels), or over them all as one (0).
    pub group_size: u32,
    /// The channels re-referenced; the others pass through as they are.
    pub channels: String,
    /// How much of the reference is subtracted, in percent.
    pub gain: f32,
}

impl Default for Reference {
    fn default() -> Self {
        Reference {
            method: Method::Median,
            reference_channels: "all".into(),
            group_size: 0,
            channels: "all".into(),
            gain: 100.0,
        }
    }
}

/// The channels a set such as "all", "none", "1-4, 9" names (counting from 1), out of `count`, as indices from 0; or why
/// it doesn't make sense.
pub fn parse_channels(text: &str, count: u32) -> Result<Vec<u32>, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("all") || text.is_empty() {
        return Ok((0..count).collect());
    }
    if text.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let mut channels = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let number = |s: &str| s.trim().parse::<u32>().map_err(|_| format!("\"{s}\" isn't a channel number"));
        let (first, last) = match part.split_once('-') {
            Some((a, b)) => (number(a)?, number(b)?),
            None => (number(part)?, number(part)?),
        };
        if first == 0 || last < first || last > count {
            return Err(format!("{part}: channels go from 1 to {count}"));
        }
        channels.extend(first - 1..last);
    }
    channels.sort_unstable();
    channels.dedup();
    Ok(channels)
}

/// Channels (indices from 0) as a set `parse_channels` reads: "all" if they're all `count`, else runs like "1-4, 9".
pub fn format_channels(channels: &[u32], count: u32) -> String {
    let mut sorted = channels.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() == count as usize && sorted.iter().enumerate().all(|(i, &c)| i as u32 == c) {
        return "all".into();
    }
    if sorted.is_empty() {
        return "none".into();
    }
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for c in sorted {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == c => *last = c,
            _ => runs.push((c, c)),
        }
    }
    let run = |&(a, b): &(u32, u32)| if a == b { format!("{}", a + 1) } else { format!("{}-{}", a + 1, b + 1) };
    runs.iter().map(run).collect::<Vec<_>>().join(", ")
}

/// What a pass needs, worked out from the settings (again whenever they change).
#[derive(Default)]
pub struct Referencing {
    /// What it's worked out for.
    settings: Option<Reference>,
    count: usize,
    /// For each channel: the reference subtracted from it, if any (an index into `references`).
    reference_of: Vec<Option<usize>>,
    /// Each reference: the channels it's made from.
    references: Vec<Vec<usize>>,
    median: bool,
    /// This pass's samples as they came, and each reference's value at a sample.
    original: Vec<f32>,
    values: Vec<f32>,
    scratch: Vec<f32>,
}

impl crate::core::engine::PassState for Referencing {
    const WHY: &'static str = "Only working memory (what the settings work out to, and room for a copy of the \
                               samples), kept so it isn't remade every pass: each worker has its own.";
    const CARRIED_OVER: bool = false;
}

impl Referencing {
    fn plan(&mut self, settings: &Reference, count: usize) {
        if self.settings.as_ref() == Some(settings) && self.count == count {
            return;
        }
        let all = count as u32;
        let applies: Vec<u32> = parse_channels(&settings.channels, all).unwrap_or_default();
        let (mut reference_of, mut references) = (vec![None; count], Vec::new());
        let mut refer = |channel: u32, sources: Vec<usize>, references: &mut Vec<Vec<usize>>| {
            if applies.contains(&channel) && !sources.is_empty() {
                let index = references.iter().position(|r| *r == sources).unwrap_or_else(|| {
                    references.push(sources);
                    references.len() - 1
                });
                reference_of[channel as usize] = Some(index);
            }
        };
        match &settings.method {
            Method::Channel { channel } => {
                for c in 0..all {
                    if (1..=all).contains(channel) {
                        refer(c, vec![*channel as usize - 1], &mut references);
                    }
                }
            }
            Method::PerChannel { references: chosen } => {
                for c in 0..all {
                    if let Some(&r) = chosen.get(c as usize).filter(|r| (1..=all).contains(r)) {
                        refer(c, vec![r as usize - 1], &mut references);
                    }
                }
            }
            Method::Average | Method::Median => {
                let sources = parse_channels(&settings.reference_channels, all).unwrap_or_default();
                let group = if settings.group_size == 0 { all } else { settings.group_size };
                for c in 0..all {
                    let in_group = |s: &&u32| **s / group == c / group;
                    refer(c, sources.iter().filter(in_group).map(|&s| s as usize).collect(), &mut references);
                }
            }
        }
        self.median = settings.method == Method::Median;
        (self.reference_of, self.references, self.count) = (reference_of, references, count);
        self.settings = Some(settings.clone());
    }
}

impl Node<Edges> for Reference {
    const INFO: NodeInfo = NodeInfo {
        name: "Reference",
        category: Category::Processing,
        description: "Re-references channels: to one channel, to the average or median of some (in groups, or \
                      all together), or each to a channel of its own.",
    };

    type State = Referencing;

    fn in_place(&self, _output: usize) -> Option<usize> {
        Some(0)
    }

    fn process(&self, pass: &mut Pass<Edges>, state: &mut Referencing) {
        let input = match pass.inputs[0] {
            Some(Buffer::Continuous(input)) => Some(input),
            _ => None,
        };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        output.transform(input, |_, x| x); // the samples as they came, to change below
        state.plan(self, output.channels);
        let (n, channels) = (output.len, output.channels);
        state.original.resize(channels * n, 0.0);
        for channel in 0..channels {
            state.original[channel * n..(channel + 1) * n].copy_from_slice(output.channel(channel));
        }
        // Each reference's value at each sample.
        state.values.resize(state.references.len() * n, 0.0);
        for (r, sources) in state.references.iter().enumerate() {
            let values = &mut state.values[r * n..(r + 1) * n];
            for (t, value) in values.iter_mut().enumerate() {
                *value = if state.median {
                    state.scratch.clear();
                    state.scratch.extend(sources.iter().map(|&s| state.original[s * n + t]));
                    let middle = state.scratch.len() / 2;
                    *state.scratch.select_nth_unstable_by(middle, |a, b| a.total_cmp(b)).1
                } else {
                    sources.iter().map(|&s| state.original[s * n + t]).sum::<f32>() / sources.len() as f32
                };
            }
        }
        let gain = self.gain / 100.0;
        for channel in 0..channels {
            let Some(r) = state.reference_of.get(channel).copied().flatten() else { continue };
            let reference = &state.values[r * n..(r + 1) * n];
            for (y, reference) in output.channel_mut(channel).iter_mut().zip(reference) {
                *y -= gain * reference;
            }
        }
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        vec![Output { name: "referenced", kind: Kind::Continuous, stream: inputs[0] }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if let Some(DataStream::Continuous(c)) = inputs[0] {
            let mut check = |what: &str, text: &str| {
                if let Err(e) = parse_channels(text, c.channels) {
                    warnings.push(format!("{what}: {e}"));
                }
            };
            check("Channels", &self.channels);
            match &self.method {
                Method::Average | Method::Median => check("Reference channels", &self.reference_channels),
                Method::Channel { channel } if *channel == 0 || *channel > c.channels => {
                    warnings.push(format!("The reference channel must be from 1 to {}", c.channels));
                }
                _ => {}
            }
        }
        warnings
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let count = match context.inputs[0] {
            Some(DataStream::Continuous(c)) => c.channels,
            _ => 64,
        };
        let names = ["One channel", "Average (CAR)", "Median (CMR)", "Each its own"];
        let current = match self.method {
            Method::Channel { .. } => 0,
            Method::Average => 1,
            Method::Median => 2,
            Method::PerChannel { .. } => 3,
        };
        let channel_set = |ui: &mut egui::Ui, text: &mut String| {
            ui.add(egui::TextEdit::singleline(text).hint_text("all, or e.g. 1-4, 9").desired_width(140.0))
        };
        settings_grid(ui, |ui| {
            setting(ui, "Reference", |ui| {
                egui::ComboBox::from_id_salt("method")
                    .selected_text(names[current])
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            if ui.selectable_label(i == current, *name).clicked() && i != current {
                                self.method = match i {
                                    0 => Method::Channel { channel: 1 },
                                    1 => Method::Average,
                                    2 => Method::Median,
                                    _ => Method::PerChannel { references: vec![0; count as usize] },
                                };
                            }
                        }
                    })
                    .response
            });
            match &mut self.method {
                Method::Channel { channel } => {
                    setting(ui, "Channel", |ui| ui.add(egui::DragValue::new(channel).range(1..=count)));
                }
                Method::Average | Method::Median => {
                    setting(ui, "Of channels", |ui| channel_set(ui, &mut self.reference_channels));
                    setting(ui, "Groups of", |ui| {
                        ui.add(egui::DragValue::new(&mut self.group_size).range(0..=count).custom_formatter(|v, _| {
                            if v == 0.0 { "all as one".into() } else { format!("{v} channels") }
                        }))
                        .on_hover_text("Take the reference separately in each group of this many channels")
                    });
                }
                Method::PerChannel { .. } => {}
            }
            setting(ui, "Re-reference", |ui| channel_set(ui, &mut self.channels));
            setting(ui, "Gain", |ui| ui.add(egui::Slider::new(&mut self.gain, 0.0..=100.0).suffix(" %")));
        });
        if let Method::PerChannel { references } = &mut self.method {
            references.resize(count as usize, 0);
            ui.label(RichText::new("Each channel's reference (0 for none)").small().weak());
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                egui::Grid::new("references").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                    for (channel, reference) in references.iter_mut().enumerate() {
                        let label = ui.label(format!("{}", channel + 1));
                        ui.add(egui::DragValue::new(reference).range(0..=count)).labelled_by(label.id);
                        if channel % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            });
        }
        input_summary(ui, context.inputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DataStreamType, PASS_DURATION};
    use crate::standard_components::ephys::Continuous;

    /// Runs one pass of `channels` (each a few samples) through `reference`.
    fn run(reference: &Reference, channels: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let stream = Continuous { channels: channels.len() as u32, sample_rate: 30000.0 };
        let mut input = stream.buffer(PASS_DURATION);
        input.len = channels[0].len();
        for (c, samples) in channels.iter().enumerate() {
            input.channel_mut(c).copy_from_slice(samples);
        }
        let input = Buffer::Continuous(input);
        let inputs = [Some(&input)];
        let mut outputs = [Buffer::Continuous(stream.buffer(PASS_DURATION))];
        let mut pass = Pass::new(0, PASS_DURATION, &inputs, &mut outputs);
        reference.process(&mut pass, &mut Referencing::default());
        let Buffer::Continuous(output) = &outputs[0] else { unreachable!() };
        (0..channels.len()).map(|c| output.channel(c).to_vec()).collect()
    }

    #[test]
    fn channel_sets_parse_as_ranges() {
        assert_eq!(parse_channels("all", 4).unwrap(), vec![0, 1, 2, 3]);
        assert_eq!(parse_channels("1-2, 4", 4).unwrap(), vec![0, 1, 3]);
        assert!(parse_channels("5", 4).is_err());
        assert!(parse_channels("x", 4).is_err());
    }

    #[test]
    fn references_as_asked() {
        let data = vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 9.0], vec![10.0, 0.0]];
        let channel = Reference { method: Method::Channel { channel: 2 }, ..Default::default() };
        assert_eq!(run(&channel, &data)[0], vec![-2.0, -2.0]);
        assert_eq!(run(&channel, &data)[1], vec![0.0, 0.0], "the reference itself, referenced, is flat");

        let average = Reference { method: Method::Average, ..Default::default() };
        assert_eq!(run(&average, &data)[3], vec![10.0 - 4.75, 0.0 - 3.75]);

        let median = Reference { method: Method::Median, ..Default::default() };
        assert_eq!(run(&median, &data)[0], vec![1.0 - 5.0, 2.0 - 4.0], "the upper middle of an even count");

        let grouped = Reference { method: Method::Average, group_size: 2, ..Default::default() };
        assert_eq!(run(&grouped, &data)[2], vec![5.0 - 7.5, 9.0 - 4.5], "from its own group only");

        let some = Reference { method: Method::Average, channels: "4".into(), gain: 50.0, ..Default::default() };
        let out = run(&some, &data);
        assert_eq!(out[0], data[0], "channels not re-referenced pass through");
        assert_eq!(out[3], vec![10.0 - 0.5 * 4.75, 0.0 - 0.5 * 3.75]);

        let own = Reference { method: Method::PerChannel { references: vec![0, 1, 1, 3] }, ..Default::default() };
        let out = run(&own, &data);
        assert_eq!(
            (out[0].clone(), out[2].clone(), out[3].clone()),
            (data[0].clone(), vec![4.0, 7.0], vec![5.0, -9.0])
        );
    }
}
