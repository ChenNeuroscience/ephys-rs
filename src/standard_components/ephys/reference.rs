//! The Reference node: re-references every channel, subtracting from each a reference made from other channels.
//!
//! As in Open Ephys: its common average reference (the average of some channels), and its channel map's reference
//! channels (each channel referenced to one other). Besides those: the median rather than the average (a common
//! median reference: a spike on one reference channel hardly moves it).
//!
//! Its settings are one setting (`reference`), which another node can edit through a settings edge. Besides the
//! referenced data, it outputs the references themselves: a channel each (one, for an average, a median or a single
//! channel; for each its own, each channel referenced to).

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, EdgeInfo, EdgeType, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::standard_components::ephys::Continuous;
use crate::standard_components::widgets::{all_checkbox, input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
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

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Reference {
    pub method: Method,
    /// For an average or median: the channels it's of, such as "all" or "1-4, 9".
    pub reference_channels: String,
}

impl Default for Reference {
    fn default() -> Self {
        Reference { method: Method::Average, reference_channels: "all".into() }
    }
}

/// The Reference's settings, as a setting another node can edit.
impl EdgeType for Reference {
    const INFO: EdgeInfo = EdgeInfo {
        name: "reference",
        colour: Color32::from_rgb(150, 200, 120),
        description: "How channels are re-referenced: the method, and which channels the reference is made of.",
    };

    fn summary(&self) -> String {
        match &self.method {
            Method::Channel { channel } => format!("channel {channel}"),
            Method::Average => format!("average of {}", channels_summary(&self.reference_channels)),
            Method::Median => format!("median of {}", channels_summary(&self.reference_channels)),
            Method::PerChannel { .. } => "each its own".into(),
        }
    }
}

impl Reference {
    /// How many references it makes, of `channels` (at least one, for its output: of zeros, if there are none).
    pub fn references(&self, channels: u32) -> usize {
        let mut referencing = Referencing::default();
        referencing.plan(self, channels as usize);
        referencing.references.len().max(1)
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

/// A set of channels (as `parse_channels` reads it) in a few words, however many there are: "all channels", "no
/// channels", "channel 5", "8 channels".
pub fn channels_summary(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("all") {
        return "all channels".into();
    }
    match parse_channels(text, u32::MAX / 2).as_deref() {
        Ok([]) => "no channels".into(),
        Ok([channel]) => format!("channel {}", channel + 1),
        Ok(channels) => format!("{} channels", channels.len()),
        Err(_) => text.into(),
    }
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
        let (mut reference_of, mut references) = (vec![None; count], Vec::new());
        let mut refer = |channel: u32, sources: Vec<usize>, references: &mut Vec<Vec<usize>>| {
            if !sources.is_empty() {
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
                let sources: Vec<usize> = parse_channels(&settings.reference_channels, all)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s as usize)
                    .collect();
                for c in 0..all {
                    refer(c, sources.clone(), &mut references);
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
        description: "Re-references every channel: to one channel, to the average or median of some, or each to a \
                      channel of its own.",
    };

    type State = Referencing;

    fn in_place(&self, _output: usize) -> Option<usize> {
        Some(0)
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "reference", value: self.clone().into() }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Reference(reference) = value {
            *self = reference;
        }
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
        for channel in 0..channels {
            let Some(r) = state.reference_of.get(channel).copied().flatten() else { continue };
            let reference = &state.values[r * n..(r + 1) * n];
            for (y, reference) in output.channel_mut(channel).iter_mut().zip(reference) {
                *y -= reference;
            }
        }
        // The references themselves (none: a channel of zeros).
        if let Some(Buffer::Continuous(references)) = pass.outputs.get_mut(1) {
            references.len = n.min(references.capacity);
            for r in 0..references.channels {
                let row = references.channel_mut(r);
                match state.values.get(r * n..(r + 1) * n) {
                    Some(values) if r < state.references.len() => row.copy_from_slice(&values[..row.len()]),
                    _ => row.fill(0.0),
                }
            }
        }
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let references = match inputs[0] {
            Some(DataStream::Continuous(c)) => {
                Some(DataStream::Continuous(Continuous { channels: self.references(c.channels) as u32, ..c }))
            }
            _ => None,
        };
        vec![
            Output { name: "referenced", kind: Kind::Continuous, stream: inputs[0] },
            Output { name: "reference", kind: Kind::Continuous, stream: references },
        ]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if let Some(DataStream::Continuous(c)) = inputs[0] {
            let mut check = |what: &str, text: &str| {
                if let Err(e) = parse_channels(text, c.channels) {
                    warnings.push(format!("{what}: {e}"));
                }
            };
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
                Method::Average | Method::Median | Method::PerChannel { .. } => {}
            }
        });
        if matches!(self.method, Method::Average | Method::Median) {
            ui.add_space(4.0);
            reference_channels_ui(ui, &mut self.reference_channels, count);
        }
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

/// The channels the reference is of, a checkbox each, in rows as wide as fit; and one for all of them.
fn reference_channels_ui(ui: &mut egui::Ui, text: &mut String, count: u32) {
    let mut chosen: Vec<u32> = parse_channels(text, count).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Of {} of the {count} channels", chosen.len())).strong());
        match all_checkbox(ui, chosen.len(), count as usize) {
            Some(true) => *text = "all".into(),
            Some(false) => *text = "none".into(),
            None => {}
        }
    });
    let across = ((ui.available_width() / 48.0) as u32).clamp(4, 16);
    let mut changed = false;
    egui::ScrollArea::vertical().max_height(240.0).id_salt("reference channels").show(ui, |ui| {
        egui::Grid::new("reference channels").spacing([6.0, 2.0]).show(ui, |ui| {
            for channel in 0..count {
                let mut on = chosen.contains(&channel);
                if ui.checkbox(&mut on, format!("{:>2}", channel + 1)).changed() {
                    if on {
                        chosen.push(channel)
                    } else {
                        chosen.retain(|&c| c != channel)
                    }
                    changed = true;
                }
                if (channel + 1) % across == 0 {
                    ui.end_row();
                }
            }
        });
    });
    if changed {
        *text = format_channels(&chosen, count);
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
    fn channel_sets_are_summed_up_in_a_few_words() {
        let summaries: Vec<String> = ["all", "", "none", "5", "1-4, 9, 12-15"].map(channels_summary).into();
        assert_eq!(summaries, ["all channels", "all channels", "no channels", "channel 5", "9 channels"]);
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

        let some = Reference { method: Method::Average, reference_channels: "1-2".into() };
        assert_eq!(run(&some, &data)[3], vec![10.0 - 2.0, 0.0 - 3.0], "of some channels, every channel referenced");

        // And the references themselves: a channel each (each channel referenced to, here).
        let stream = Continuous { channels: 4, sample_rate: 30000.0 };
        let own = Reference { method: Method::PerChannel { references: vec![0, 1, 1, 3] }, ..Default::default() };
        assert_eq!((own.references(4), average.references(4)), (2, 1));
        let outputs = own.outputs(&[Some(DataStream::Continuous(stream))]);
        assert_eq!(outputs[1].stream, Some(DataStream::Continuous(Continuous { channels: 2, ..stream })));
        let mut input = stream.buffer(PASS_DURATION);
        input.len = 2;
        for (c, samples) in data.iter().enumerate() {
            input.channel_mut(c).copy_from_slice(samples);
        }
        let input = Buffer::Continuous(input);
        let inputs = [Some(&input)];
        let references = Continuous { channels: 2, ..stream };
        let mut outputs =
            [Buffer::Continuous(stream.buffer(PASS_DURATION)), Buffer::Continuous(references.buffer(PASS_DURATION))];
        own.process(&mut Pass::new(0, PASS_DURATION, &inputs, &mut outputs), &mut Referencing::default());
        let Buffer::Continuous(references) = &outputs[1] else { unreachable!() };
        assert_eq!((references.channel(0), references.channel(1)), (&[1.0, 2.0][..], &[5.0, 9.0][..]));

        let out = run(&own, &data);
        assert_eq!(
            (out[0].clone(), out[2].clone(), out[3].clone()),
            (data[0].clone(), vec![4.0, 7.0], vec![5.0, -9.0])
        );
    }

    #[test]
    fn settings_window() {
        use crate::Nodes;
        use crate::core::engine::{Graph, NodeSet};
        use crate::standard_components::themes::THEMES;
        let mut graph = Graph::<Nodes>::default();
        use egui_kittest::kittest::Queryable as _;
        let index = Nodes::TYPES.iter().position(|t| t.name == "Reference").unwrap();
        let id = graph.add(Nodes::create(index), [0.0, 0.0]);
        if let Some(Nodes::Reference(reference)) = graph.get_mut(id).map(|e| &mut e.node) {
            reference.reference_channels = "1-3, 5-8, 13".into();
        }
        let inputs = [Some(DataStream::Continuous(Continuous { channels: 16, sample_rate: 30000.0 }))];
        let mut harness = egui_kittest::Harness::builder().with_size([360.0, 300.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, false, false, None, |node, context| {
                    node.settings_ui(ui, context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("reference_settings");
        // Ticking a channel puts it in the reference.
        harness.get_by_label(" 4").click();
        harness.run_steps(2);
        let reference = match harness.state().get(id).map(|e| &e.node) {
            Some(Nodes::Reference(reference)) => reference.reference_channels.clone(),
            _ => unreachable!(),
        };
        assert_eq!(reference, "1-8, 13");
    }
}
