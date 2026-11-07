//! The Reference node: makes any number of references, each the mean or median of some channels, and subtracts from each
//! channel the one it uses, if any.
//!
//! As in Open Ephys's common average reference; besides that, the median rather than the mean (a common median reference:
//! a spike on one reference channel hardly moves it). One channel on its own is a reference of one channel.
//!
//! Its settings are one setting (`reference`), which another node can edit through a settings edge. Besides the
//! referenced data, it outputs the references themselves: a channel each.

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, EdgeInfo, EdgeType, Input, Node, NodeInfo, Output, Pass, SettingPort};
use crate::core::gui::widgets::{Icon, small_toggle_button};
use crate::standard_components::ephys::{Continuous, ContinuousBuffer};
use crate::standard_components::widgets::{all_checkbox, input_summary, require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

/// How a reference is made from its channels.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    Mean,
    Median,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Mean => "mean",
            Method::Median => "median",
        }
    }
}

/// A reference: which channels it's made from, and which channels use it. Each of those is a set such as "all" or "1-4, 9".
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Computation {
    /// Which it is, whatever's added and removed around it.
    pub id: u32,
    pub method: Method,
    pub channels: String,
    /// A channel uses one reference at most: if two have it here, the first.
    pub used_by: String,
}

impl Default for Computation {
    fn default() -> Self {
        Computation { id: 0, method: Method::Mean, channels: "none".into(), used_by: "none".into() }
    }
}

impl Computation {
    /// What it is, in a few words: "mean of 8 channels".
    pub fn summary(&self) -> String {
        format!("{} of {}", self.method.name(), channels_summary(&self.channels))
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(from = "Saved")]
pub struct Reference {
    pub list: Vec<Computation>,
    pub next_id: u32,
}

impl Default for Reference {
    fn default() -> Self {
        let mean = Computation { channels: "all".into(), used_by: "all".into(), ..Default::default() };
        Reference { list: vec![mean], next_id: 1 }
    }
}

/// What graphs have saved: as it is now, or before a Reference could make several.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Saved {
    list: Option<Vec<Computation>>,
    next_id: u32,
    method: Option<OldMethod>,
    reference_channels: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum OldMethod {
    Channel { channel: u32 },
    Average,
    Median,
    PerChannel { references: Vec<u32> },
}

impl From<Saved> for Reference {
    fn from(saved: Saved) -> Self {
        if let Some(list) = saved.list {
            return Reference { list, next_id: saved.next_id };
        }
        let everyone = |method, channels: String| (method, channels, "all".to_string());
        let channels = saved.reference_channels.unwrap_or_else(|| "all".into());
        let made = match saved.method {
            None => return Reference::default(),
            Some(OldMethod::Average) => vec![everyone(Method::Mean, channels)],
            Some(OldMethod::Median) => vec![everyone(Method::Median, channels)],
            Some(OldMethod::Channel { channel }) => vec![everyone(Method::Mean, channel.to_string())],
            // A reference for each channel referenced to, used by those that were.
            Some(OldMethod::PerChannel { references }) => {
                let mut sources: Vec<u32> = references.iter().copied().filter(|r| *r > 0).collect();
                sources.sort_unstable();
                sources.dedup();
                let users = |source: u32| -> Vec<u32> {
                    (0..references.len() as u32).filter(|c| references[*c as usize] == source).collect()
                };
                let made = sources
                    .iter()
                    .map(|&source| (Method::Mean, source.to_string(), format_channels(&users(source), u32::MAX)));
                made.collect()
            }
        };
        let list: Vec<Computation> = made
            .into_iter()
            .enumerate()
            .map(|(id, (method, channels, used_by))| Computation { id: id as u32, method, channels, used_by })
            .collect();
        Reference { next_id: list.len() as u32, list }
    }
}

/// The Reference's settings, as a setting another node can edit.
impl EdgeType for Reference {
    const INFO: EdgeInfo = EdgeInfo {
        name: "reference",
        colour: Color32::from_rgb(150, 200, 120),
        description: "How channels are re-referenced: the references made, of which channels, and which channels use each.",
    };

    fn summary(&self) -> String {
        match &self.list[..] {
            [] => "no references".into(),
            [only] => only.summary(),
            list => format!("{} references", list.len()),
        }
    }
}

/// The references' colours, in turn: none of them blue, which traces and what's selected are.
pub const REFERENCE_COLOURS: [Color32; 6] = [
    Color32::from_rgb(240, 160, 60),
    Color32::from_rgb(235, 110, 190),
    Color32::from_rgb(70, 205, 185),
    Color32::from_rgb(225, 205, 80),
    Color32::from_rgb(175, 145, 255),
    Color32::from_rgb(130, 210, 110),
];

/// Reference `index`'s colour, wherever it's to be told from the others.
pub fn reference_colour(index: usize) -> Color32 {
    REFERENCE_COLOURS[index % REFERENCE_COLOURS.len()]
}

/// Whether `set` (as `parse_channels` reads it) has channel `number` (from 0).
pub fn has_channel(set: &str, number: u32) -> bool {
    let set = set.trim();
    set.is_empty()
        || set.eq_ignore_ascii_case("all")
        || parse_channels(set, u32::MAX / 2).is_ok_and(|channels| channels.contains(&number))
}

/// `set`, of `count` channels, with `number` (from 0) in it or out of it.
fn with_channel(set: &str, number: u32, count: u32, wanted: bool) -> String {
    let mut channels: Vec<u32> = (0..count).filter(|&c| c != number && has_channel(set, c)).collect();
    if wanted {
        channels.push(number);
    }
    format_channels(&channels, count)
}

/// The space below each reference in the settings, setting them apart.
const REFERENCES_GAP: f32 = 12.0;

impl Reference {
    /// How many channels its reference output has: one each.
    pub fn references(&self) -> usize {
        self.list.len()
    }

    /// Adds a reference, of the channels using none, of the `count` there are, which they use.
    pub fn add(&mut self, count: u32) {
        let free = (0..count).filter(|&c| self.used_by(c).is_none()).collect::<Vec<_>>();
        let channels = format_channels(&free, count);
        self.list.push(Computation { id: self.next_id, used_by: channels.clone(), channels, ..Default::default() });
        self.next_id += 1;
    }

    /// Which reference channel `number` (from 0) uses, if any: the first that says so.
    pub fn used_by(&self, number: u32) -> Option<usize> {
        self.list.iter().position(|reference| has_channel(&reference.used_by, number))
    }

    /// Whether channel `number` is in the reference it uses and no other, or uses none and is in none. Then it's simple
    /// enough to be one choice (see `toggle_channel`): settings can say otherwise.
    pub fn agrees(&self, number: u32) -> bool {
        let of = |set: fn(&Computation) -> &String| -> Vec<usize> {
            (0..self.list.len()).filter(|&i| has_channel(set(&self.list[i]), number)).collect()
        };
        let (made_from, using) = (of(|r| &r.channels), of(|r| &r.used_by));
        made_from == using && using.len() <= 1
    }

    /// Has channel `number` be in reference `index` and use it, and no other; or, if it is, none.
    pub fn toggle_channel(&mut self, index: usize, number: u32, count: u32) {
        let wanted = self.used_by(number) != Some(index);
        for (i, reference) in self.list.iter_mut().enumerate() {
            reference.channels = with_channel(&reference.channels, number, count, i == index && wanted);
            reference.used_by = with_channel(&reference.used_by, number, count, i == index && wanted);
        }
    }

    /// Has all the channels that agree (see `agrees`) be in reference `index`, or, if not `all`, none.
    pub fn set_all(&mut self, index: usize, all: bool, count: u32) {
        for number in (0..count).filter(|&c| self.agrees(c)).collect::<Vec<_>>() {
            if (self.used_by(number) == Some(index)) != all {
                self.toggle_channel(index, number, count);
            }
        }
    }

    /// Has no channel use any reference.
    pub fn clear_use(&mut self) {
        self.list.iter_mut().for_each(|reference| reference.used_by = "none".into());
    }

    /// Has channel `number` use reference `index`, or, if it does, none.
    pub fn toggle_use(&mut self, index: usize, number: u32, count: u32) {
        let using = self.used_by(number) == Some(index);
        for (i, reference) in self.list.iter_mut().enumerate() {
            reference.used_by = with_channel(&reference.used_by, number, count, i == index && !using);
        }
    }

    /// Has every channel use reference `index`, or, if not `all`, none of them.
    pub fn set_all_use(&mut self, index: usize, all: bool) {
        for (i, reference) in self.list.iter_mut().enumerate() {
            if i == index {
                reference.used_by = if all { "all" } else { "none" }.into();
            } else if all {
                reference.used_by = "none".into();
            }
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
    /// Each reference: the channels it's made from, and whether it's their median.
    references: Vec<(Vec<usize>, bool)>,
    /// Each reference's value at each sample.
    values: Vec<f32>,
    scratch: Vec<f32>,
}

impl crate::core::engine::PassState for Referencing {
    const WHY: &'static str = "Only working memory (what the settings work out to, and room for the references' \
                               values), kept so it isn't remade every pass: each worker has its own.";
    const CARRIED_OVER: bool = false;
}

impl Referencing {
    fn plan(&mut self, settings: &Reference, count: usize) {
        if self.settings.as_ref() == Some(settings) && self.count == count {
            return;
        }
        let mut reference_of = vec![None; count];
        let mut references = Vec::new();
        for (index, reference) in settings.list.iter().enumerate() {
            let set = |text: &str| parse_channels(text, count as u32).unwrap_or_default();
            let sources: Vec<usize> = set(&reference.channels).into_iter().map(|c| c as usize).collect();
            // A reference of nothing has nothing to subtract.
            if !sources.is_empty() {
                for channel in set(&reference.used_by) {
                    reference_of[channel as usize].get_or_insert(index);
                }
            }
            references.push((sources, reference.method == Method::Median));
        }
        (self.reference_of, self.references, self.count) = (reference_of, references, count);
        self.settings = Some(settings.clone());
    }
}

impl Node<Edges> for Reference {
    const INFO: NodeInfo = NodeInfo {
        name: "Reference",
        category: Category::Processing,
        description: "Makes references, each the mean or median of some channels, and subtracts from each channel the one \
                      it uses, if any.",
    };

    type State = Referencing;

    fn in_place(&self, _output: usize) -> Option<usize> {
        Some(0)
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "reference", value: self.clone().into(), on_right: true }]
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
        // Each reference's value at each sample, from the samples as they came: nothing's subtracted until they're all made.
        state.values.resize(state.references.len() * n, 0.0);
        for (r, (sources, median)) in state.references.iter().enumerate() {
            let values = &mut state.values[r * n..(r + 1) * n];
            if sources.is_empty() {
                values.fill(0.0);
            } else if *median {
                medians(values, sources, output, &mut state.scratch);
            } else {
                means(values, sources, output);
            }
        }
        for channel in 0..channels {
            let Some(r) = state.reference_of.get(channel).copied().flatten() else { continue };
            subtract(output.channel_mut(channel), &state.values[r * n..(r + 1) * n]);
        }
        // The references themselves.
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
                Some(DataStream::Continuous(Continuous { channels: self.references() as u32, ..c }))
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
            for (index, reference) in self.list.iter().enumerate() {
                for (what, text) in [("channels", &reference.channels), ("users", &reference.used_by)] {
                    if let Err(e) = parse_channels(text, c.channels) {
                        warnings.push(format!("Reference {}'s {what}: {e}", index + 1));
                    }
                }
            }
        }
        warnings
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let count = match context.inputs[0] {
            Some(DataStream::Continuous(c)) => c.channels,
            _ => 64,
        };
        let mut remove = None;
        for index in 0..self.list.len() {
            let reference = &mut self.list[index];
            let title = RichText::new(format!("Reference {}: {}", index + 1, reference.summary()))
                .strong()
                .color(reference_colour(index));
            ui.push_id(reference.id, |ui| {
                // The line down its left, in its colour, to hold together all that's its.
                ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::new(2.0, reference_colour(index));
                let section = egui::CollapsingHeader::new(title).default_open(true).show(ui, |ui| {
                    settings_grid(ui, |ui| {
                        setting(ui, "Method", |ui| method_ui(ui, &mut reference.method));
                    });
                    ui.add_space(4.0);
                    channels_ui(ui, "Made from", &mut reference.channels, count, |_, _| {});
                });
                // Right after its title, there whether or not it's open.
                let title = section.header_response.rect;
                let place = title.with_min_x(title.right() + 4.0).with_max_x(ui.max_rect().right());
                let layout = egui::Layout::left_to_right(egui::Align::Center);
                let mut buttons = ui.new_child(egui::UiBuilder::new().max_rect(place).layout(layout));
                let label = format!("Delete reference {}", index + 1);
                if small_toggle_button(&mut buttons, Icon::Delete, &label, "Delete this reference", false).clicked() {
                    remove = Some(index);
                }
            });
            ui.add_space(REFERENCES_GAP);
        }
        if let Some(index) = remove {
            self.list.remove(index);
        }
        if ui.button("Add reference").on_hover_text("Channels using none will use it").clicked() {
            self.add(count);
        }
        ui.add_space(6.0);
        uses_ui(ui, self, count);
        ui.add_space(4.0);
        input_summary(ui, context.inputs);
    }
}

/// Which reference each channel uses, or none, a row each, with a checkbox for all of them at the top of each column.
fn uses_ui(ui: &mut egui::Ui, reference: &mut Reference, count: u32) {
    ui.strong("Which each channel uses");
    let references = reference.list.len();
    if references == 0 {
        ui.weak("There are no references to use.");
        return;
    }
    let using = |reference: &Reference| (0..count).map(|c| reference.used_by(c)).collect::<Vec<_>>();
    let chosen = using(reference);
    // Clicked: a channel and what it's to use.
    let mut picked: Option<(u32, Option<usize>)> = None;
    // Clicked at the top of a column: what's to be used by all of them, or (if it's on) by none of them.
    let mut column: Option<(Option<usize>, bool)> = None;
    egui::Grid::new("uses").num_columns(references + 2).striped(true).spacing([14.0, 2.0]).show(ui, |ui| {
        ui.label("");
        ui.weak("None");
        for index in 0..references {
            ui.label(RichText::new((index + 1).to_string()).color(reference_colour(index)));
        }
        ui.end_row();
        let options = || std::iter::once(None).chain((0..references).map(Some));
        ui.label("");
        for option in options() {
            let using = chosen.iter().filter(|used| **used == option).count();
            if let Some(all) = all_checkbox(ui, using, count as usize, "") {
                column = Some((option, all));
            }
        }
        ui.end_row();
        for channel in 0..count {
            let label = ui.label((channel + 1).to_string());
            for option in options() {
                if ui.radio(chosen[channel as usize] == option, "").labelled_by(label.id).clicked() {
                    picked = Some((channel, option));
                }
            }
            ui.end_row();
        }
    });
    match column {
        Some((Some(index), all)) => reference.set_all_use(index, all),
        Some((None, true)) => reference.clear_use(),
        _ => {}
    }
    match picked.map(|(channel, option)| (channel, option, chosen[channel as usize])) {
        Some((channel, Some(index), used)) if used != Some(index) => reference.toggle_use(index, channel, count),
        Some((channel, None, Some(used))) => reference.toggle_use(used, channel, count),
        _ => {}
    }
}

/// Each sample's mean over the channels `sources`, a channel at a time, so it's over whole slices, and vectorises.
fn means(values: &mut [f32], sources: &[usize], data: &ContinuousBuffer) {
    values.fill(0.0);
    for &source in sources {
        add(values, data.channel(source));
    }
    let count = sources.len() as f32;
    values.iter_mut().for_each(|value| *value /= count);
}

fn add(sums: &mut [f32], samples: &[f32]) {
    sums.iter_mut().zip(samples).for_each(|(sum, sample)| *sum += *sample);
}

fn subtract(samples: &mut [f32], reference: &[f32]) {
    samples.iter_mut().zip(reference).for_each(|(sample, reference)| *sample -= *reference);
}

/// Each sample's median over the channels `sources`.
fn medians(values: &mut [f32], sources: &[usize], data: &ContinuousBuffer, scratch: &mut Vec<f32>) {
    for (t, value) in values.iter_mut().enumerate() {
        scratch.clear();
        scratch.extend(sources.iter().map(|&source| data.channel(source)[t]));
        let middle = scratch.len() / 2;
        *value = *scratch.select_nth_unstable_by(middle, |a, b| a.total_cmp(b)).1;
    }
}

/// Mean or median.
pub fn method_ui(ui: &mut egui::Ui, method: &mut Method) -> egui::Response {
    egui::ComboBox::from_id_salt("reference method")
        .selected_text(method.name())
        .width(70.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(method, Method::Mean, "mean").on_hover_text("A common average reference");
            ui.selectable_value(method, Method::Median, "median")
                .on_hover_text("A common median reference: a spike on one of its channels hardly moves it");
        })
        .response
}

/// A set of channels such as "1-4, 9" (see `parse_channels`), of `count`: a checkbox each, in rows as wide as fit, and one
/// for all of them, beside a `heading` saying how many it is, then `beside`, given how many that is.
pub fn channels_ui(
    ui: &mut egui::Ui,
    heading: &str,
    text: &mut String,
    count: u32,
    beside: impl FnOnce(&mut egui::Ui, usize),
) {
    let mut chosen: Vec<u32> = parse_channels(text, count).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{heading} {} of the {count} channels", chosen.len())).strong());
        match all_checkbox(ui, chosen.len(), count as usize, "All") {
            Some(true) => *text = "all".into(),
            Some(false) => *text = "none".into(),
            None => {}
        }
        beside(ui, chosen.len());
    });
    let across = ((ui.available_width() / 48.0) as u32).clamp(4, 16);
    let mut changed = false;
    egui::Grid::new("made from").spacing([6.0, 2.0]).show(ui, |ui| {
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
    if changed {
        *text = format_channels(&chosen, count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DataStreamType, PASS_DURATION};

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

    /// A reference of `channels`, used by `used_by`.
    fn made(id: u32, method: Method, channels: &str, used_by: &str) -> Computation {
        Computation { id, method, channels: channels.into(), used_by: used_by.into() }
    }

    fn of(list: Vec<Computation>) -> Reference {
        Reference { next_id: list.len() as u32, list }
    }

    #[test]
    fn references_as_asked() {
        let data = vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 9.0], vec![10.0, 0.0]];
        let one = of(vec![made(0, Method::Mean, "2", "all")]);
        assert_eq!(run(&one, &data)[0], vec![-2.0, -2.0], "one channel's a reference too");
        assert_eq!(run(&one, &data)[1], vec![0.0, 0.0], "the reference itself, referenced, is flat");

        let mean = Reference::default();
        assert_eq!(run(&mean, &data)[3], vec![10.0 - 4.75, 0.0 - 3.75]);

        let median = of(vec![made(0, Method::Median, "all", "all")]);
        assert_eq!(run(&median, &data)[0], vec![1.0 - 5.0, 2.0 - 4.0], "the upper middle of an even count");

        let some = of(vec![made(0, Method::Mean, "1-2", "all")]);
        assert_eq!(run(&some, &data)[3], vec![10.0 - 2.0, 0.0 - 3.0], "of some channels, every channel referenced");
    }

    #[test]
    fn each_channel_uses_the_reference_it_says_or_none() {
        let data = vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 9.0], vec![10.0, 0.0]];
        let two = of(vec![made(0, Method::Mean, "1-2", "1-2"), made(1, Method::Median, "3", "3")]);
        let out = run(&two, &data);
        assert_eq!(out[0], vec![1.0 - 2.0, 2.0 - 3.0], "the first");
        assert_eq!(out[2], vec![0.0, 0.0], "the second");
        assert_eq!(out[3], data[3], "none");
        assert_eq!(run(&of(vec![]), &data), data, "no references at all");
        assert_eq!(run(&of(vec![made(0, Method::Median, "none", "all")]), &data), data, "one of nothing: no panic");
        let both = of(vec![made(0, Method::Mean, "1", "all"), made(1, Method::Mean, "2", "all")]);
        assert_eq!(run(&both, &data)[3], vec![9.0, -2.0], "said twice: the first");
    }

    #[test]
    fn the_references_are_output_a_channel_each_in_order() {
        let data = [vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 9.0], vec![10.0, 0.0]];
        let stream = Continuous { channels: 4, sample_rate: 30000.0 };
        let reference = of(vec![
            made(0, Method::Mean, "1", "none"),
            made(1, Method::Mean, "none", "all"),
            made(2, Method::Mean, "3", "all"),
        ]);
        assert_eq!((reference.references(), of(vec![]).references()), (3, 0));
        let outputs = reference.outputs(&[Some(DataStream::Continuous(stream))]);
        assert_eq!(outputs[1].stream, Some(DataStream::Continuous(Continuous { channels: 3, ..stream })));
        let mut input = stream.buffer(PASS_DURATION);
        input.len = 2;
        for (c, samples) in data.iter().enumerate() {
            input.channel_mut(c).copy_from_slice(samples);
        }
        let input = Buffer::Continuous(input);
        let inputs = [Some(&input)];
        let references = Continuous { channels: 3, ..stream };
        let mut outputs =
            [Buffer::Continuous(stream.buffer(PASS_DURATION)), Buffer::Continuous(references.buffer(PASS_DURATION))];
        reference.process(&mut Pass::new(0, PASS_DURATION, &inputs, &mut outputs), &mut Referencing::default());
        let Buffer::Continuous(references) = &outputs[1] else { unreachable!() };
        assert_eq!(references.channel(0), &[1.0, 2.0][..], "even if nothing uses it");
        assert_eq!(references.channel(1), &[0.0, 0.0][..], "of nothing");
        assert_eq!(references.channel(2), &[5.0, 9.0][..]);
    }

    #[test]
    fn a_channel_uses_one_reference_at_most() {
        let mut reference = of(vec![made(0, Method::Mean, "all", "1-3"), made(1, Method::Mean, "all", "4")]);
        reference.toggle_use(1, 0, 4);
        assert_eq!((reference.list[0].used_by.as_str(), reference.list[1].used_by.as_str()), ("2-3", "1, 4"));
        assert_eq!(reference.used_by(0), Some(1));
        reference.toggle_use(1, 0, 4);
        assert_eq!(reference.used_by(0), None, "clicked again: none");
        reference.set_all_use(1, true);
        assert_eq!((reference.list[0].used_by.as_str(), reference.list[1].used_by.as_str()), ("none", "all"));
        reference.set_all_use(1, false);
        assert_eq!(reference.used_by(2), None);
    }

    #[test]
    fn a_new_reference_is_of_the_channels_with_none() {
        let mut reference = of(vec![made(0, Method::Mean, "1-3", "1-3")]);
        reference.add(8);
        assert_eq!((reference.list[1].channels.as_str(), reference.list[1].used_by.as_str()), ("4-8", "4-8"));
        assert_eq!(reference.list[1].id, 1);
        reference.list.remove(0);
        reference.add(8);
        assert_eq!(reference.list[1].id, 2, "an id isn't used again");
        assert_eq!(reference.list[1].used_by, "1-3");
    }

    #[test]
    fn a_channel_can_be_in_a_reference_and_use_it_in_one_go() {
        let mut reference = of(vec![made(0, Method::Mean, "1-2", "1-2"), made(1, Method::Mean, "3", "3")]);
        reference.toggle_channel(1, 0, 4);
        assert_eq!((reference.list[0].channels.as_str(), reference.list[1].channels.as_str()), ("2", "1, 3"));
        assert_eq!(reference.used_by(0), Some(1));
        reference.toggle_channel(1, 0, 4);
        assert_eq!((reference.used_by(0), has_channel(&reference.list[1].channels, 0)), (None, false));
        assert!((0..4).all(|c| reference.agrees(c)));
    }

    #[test]
    fn settings_can_say_what_a_row_of_rs_can_not() {
        // Made from channels 1-2, used by 2-3; channel 4's in the second, and uses none.
        let reference = of(vec![made(0, Method::Mean, "1-2", "2-3"), made(1, Method::Mean, "4", "none")]);
        assert_eq!((0..5).map(|c| reference.agrees(c)).collect::<Vec<_>>(), [false, true, false, false, true]);
        let twice = of(vec![made(0, Method::Mean, "1", "1"), made(1, Method::Mean, "1", "1")]);
        assert!(!twice.agrees(0), "in two");
        let mut some = of(vec![made(0, Method::Mean, "1-2", "2-3"), made(1, Method::Mean, "5", "5")]);
        some.set_all(1, true, 5);
        assert_eq!(some.list[1].channels, "2, 4-5", "only those that agree");
        assert_eq!(some.list[0].used_by, "3");
    }

    #[test]
    fn saved_graphs_keep_their_reference() {
        let opened = |json: serde_json::Value| serde_json::from_value::<Reference>(json).unwrap();
        let mean = opened(serde_json::json!({"method": {"kind": "average"}, "reference_channels": "1-4"}));
        assert_eq!(mean.list, vec![made(0, Method::Mean, "1-4", "all")]);
        let median = opened(serde_json::json!({"method": {"kind": "median"}, "reference_channels": "all"}));
        assert_eq!(median.list, vec![made(0, Method::Median, "all", "all")]);
        let one = opened(serde_json::json!({"method": {"kind": "channel", "channel": 3}}));
        assert_eq!(one.list, vec![made(0, Method::Mean, "3", "all")]);
        let own = opened(serde_json::json!({"method": {"kind": "per_channel", "references": [0, 1, 1, 3]}}));
        assert_eq!(own.list, vec![made(0, Method::Mean, "1", "2-3"), made(1, Method::Mean, "3", "4")]);
        assert_eq!(own.next_id, 2);
        assert_eq!(opened(serde_json::json!({})), Reference::default());
        let now = of(vec![made(4, Method::Median, "2", "1")]);
        assert_eq!(opened(serde_json::to_value(&now).unwrap()), now, "and as they are now");
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
            reference.list[0].channels = "1-3, 5-8, 13".into();
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
        let reference = |harness: &egui_kittest::Harness<Graph<Nodes>>| match harness.state().get(id).map(|e| &e.node) {
            Some(Nodes::Reference(reference)) => reference.clone(),
            _ => unreachable!(),
        };
        assert_eq!(reference(&harness).list[0].channels, "1-8, 13");
        harness.set_size(egui::vec2(360.0, 1100.0));
        harness.run_steps(2);
        harness.get_by_label("Add reference").click();
        harness.run_steps(2);
        assert_eq!(reference(&harness).list.len(), 2);
        harness.snapshot("reference_settings_uses");

        // A row each, of None, 1 and 2. Channel 3 to the second, of 1-16 using the first.
        let radio = |harness: &egui_kittest::Harness<Graph<Nodes>>, row: usize, column: usize| {
            let radios: Vec<_> = harness.get_all_by_role(egui::accesskit::Role::RadioButton).collect();
            radios[row * 3 + column].click();
        };
        radio(&harness, 2, 2);
        harness.run_steps(2);
        assert_eq!(reference(&harness).used_by(2), Some(1));
        assert_eq!(reference(&harness).used_by(1), Some(0), "the rest as they were");
        radio(&harness, 2, 0);
        harness.run_steps(2);
        assert_eq!(reference(&harness).used_by(2), None, "none");
        // The checkboxes at the top of those columns, the last on the page.
        let column = |harness: &egui_kittest::Harness<Graph<Nodes>>, column: usize| {
            let checkboxes: Vec<_> = harness.get_all_by_role(egui::accesskit::Role::CheckBox).collect();
            checkboxes[checkboxes.len() - 3 + column].click();
        };
        column(&harness, 2);
        harness.run_steps(2);
        assert!((0..16).all(|c| reference(&harness).used_by(c) == Some(1)), "all of them");
        column(&harness, 2);
        harness.run_steps(2);
        assert!((0..16).all(|c| reference(&harness).used_by(c).is_none()), "and none");
        column(&harness, 1);
        harness.run_steps(2);
        assert!((0..16).all(|c| reference(&harness).used_by(c) == Some(0)));
        column(&harness, 0);
        harness.run_steps(2);
        assert!((0..16).all(|c| reference(&harness).used_by(c).is_none()), "None, for all of them");
        harness.get_by_label("Delete reference 2").click();
        harness.run_steps(2);
        assert_eq!(reference(&harness).list.len(), 1);
    }
}
