//! The LFP Viewer: scrolling traces of continuous data, a row per channel, each at a fixed height (scrolled
//! through), each with a y axis of the scale chosen, and beside each, its spectrum over a band of interest (from a
//! Spectrum node, which only runs while the spectrum's on: see `Node::evaluate_tree_shake`). The time's along the bottom, after the last
//! channel.
//!
//! Each pass adds its samples to a few seconds' worth for each channel shown, kept as the lowest and highest
//! sample in each of the view's columns (so spikes show however far it's zoomed out).
//!
//! It sets what's upstream of it and beside it, through its controls (as the Spike Viewer does the spikes'):
//! - an Continuous Gain's gains (what the continuous data's amplified by, as it's recorded): one for all can be set
//!   here, and their auto-detect run;
//! - a Channel Selector's selection, upstream: click the numbers of the channels to keep, while all are shown, then apply
//!   it, from above the first channel;
//! - a Reference's references (the spikes' re-referencing, from the same channels): a column of Rs for each, left of the
//!   channels' numbers, an R putting its channel in that reference and having it use it, as a channel's in one at most
//!   here. Settings can say otherwise (see `Reference::agrees`), when its Rs can't be changed. The traces themselves aren't
//!   referenced. Connected to the Reference's reference output too, it shows the references themselves, in rows of their
//!   own at the top, with what's particular to each: otherwise that's in the toolbar. Each reference has a colour, for
//!   telling which row's which column: the channels are neutral, to leave the colours to them.

use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Shape, Stroke};
use ephys_rs::engine::TreeShake;
use serde::{Deserialize, Serialize};

use crate::standard::auto_detect::auto_detect_ui;
use crate::standard::ephys::reference::{
    Computation, Method, Reference, format_channels, parse_channels, reference_colour,
};
use crate::standard::ephys::spectrum::{Spectra, SpectraBuffer};
use crate::standard::ephys::{ChannelSelection, Continuous, ContinuousBuffer};
use crate::standard::plots;
use crate::standard::plots::axis_ui;
use crate::standard::pool::Pool;
use crate::standard::spikes::{Channels, GainScope, Gains, gain_field};
use crate::standard::widgets::{TIME_AXIS_HEIGHT, inline_block, open_settings_button, require_input, time_axis_ui};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};
use ephys_rs::engine::{Category, Control, EdgeType, Input, Node, NodeInfo, Output, Pass, UiContext};
use ephys_rs::gui::widgets::{Icon, small_toggle_button};

/// Columns across the view.
const COLUMNS: usize = 1200;
/// The spectrum's column's width.
const SPECTRUM_WIDTH: f32 = 160.0;
/// The width of a row's y axis's labels and ticks.
const AXIS_WIDTH: f32 = 44.0;

/// Its controls: an Continuous Gain's gains, a Channel Selector's selection and a Reference's reference.
const GAINS: usize = 0;
const SELECTION: usize = 1;
const REFERENCE: usize = 2;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LfpViewer {
    /// Seconds across the view.
    pub span: f32,
    /// Microvolts from a trace's centre line to the edge of its row.
    pub scale_uv: f32,
    /// Each row's height, in points.
    pub row_height: f32,
    /// Whether to show each channel's spectrum (from a Spectrum node).
    pub spectrum: bool,
}

impl Default for LfpViewer {
    fn default() -> Self {
        LfpViewer { span: 2.0, scale_uv: 200.0, row_height: 40.0, spectrum: true }
    }
}

/// The last `span` seconds of each channel shown: each column's lowest and highest sample, round a ring.
#[derive(Default)]
pub struct Traces {
    /// The input's sample rate.
    rate: f32,
    /// What it's for: the first channel, how many, how many references, and samples per column.
    shape: (usize, usize, usize, usize),
    /// For each channel shown, then each reference, `COLUMNS` lows then highs.
    lows: Vec<f32>,
    highs: Vec<f32>,
    /// The column being filled, and how many samples it has so far.
    head: usize,
    filled: usize,
    /// Columns filled since starting (up to `COLUMNS`).
    columns: usize,
}

impl ephys_rs::engine::PassState for Traces {
    const WHY: &'static str = "The last few seconds of each channel shown (each column's lowest and highest \
                               sample, round a ring), which each pass adds its samples to.";
}

impl Traces {
    /// Adds a pass's samples of the channels shown (from `first`) to their columns: a run of samples at a time, up to
    /// where a column's full, channel by channel (as they're laid out); and the references', if there are any.
    fn add(&mut self, input: &ContinuousBuffer, first: usize, references: Option<&ContinuousBuffer>) {
        let (_, shown, count, per_column) = self.shape;
        let mut t = 0;
        while t < input.len {
            let run = (per_column - self.filled).min(input.len - t);
            for i in 0..shown + count {
                let samples = match references {
                    None if i >= shown => break,
                    Some(references) if i >= shown => {
                        // As many as there are of the pass's (each the same length, from the same board).
                        let row = references.channel(i - shown);
                        &row[t.min(row.len())..(t + run).min(row.len())]
                    }
                    _ => &input.channel(first + i)[t..t + run],
                };
                let (low, high) = extremes(samples.iter().copied());
                let at = i * COLUMNS + self.head;
                if self.filled == 0 {
                    (self.lows[at], self.highs[at]) = (low, high);
                } else {
                    self.lows[at] = self.lows[at].min(low);
                    self.highs[at] = self.highs[at].max(high);
                }
            }
            (t, self.filled) = (t + run, self.filled + run);
            if self.filled == per_column {
                (self.head, self.filled) = ((self.head + 1) % COLUMNS, 0);
                self.columns = (self.columns + 1).min(COLUMNS);
            }
        }
    }

    fn reshape(&mut self, shape: (usize, usize, usize, usize)) {
        if self.shape != shape {
            let (_, shown, references, _) = shape;
            *self = Traces {
                rate: self.rate,
                shape,
                lows: vec![0.0; (shown + references) * COLUMNS],
                highs: vec![0.0; (shown + references) * COLUMNS],
                ..Default::default()
            };
        }
    }
}

/// The lowest and highest of `values`.
fn extremes(values: impl Iterator<Item = f32>) -> (f32, f32) {
    values.fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), x| (low.min(x), high.max(x)))
}

/// The traces as of the last pass, oldest column first, and the spectra.
#[derive(Default)]
pub struct TracesDisplay {
    first: usize,
    shown: usize,
    /// How many references there are, in rows after the channels'.
    references: usize,
    columns: usize,
    /// The seconds in a column.
    column_seconds: f64,
    lows: Vec<f32>,
    highs: Vec<f32>,
    /// Each channel's spectrum, if it's being worked out.
    spectra: Option<SpectraBuffer>,
}

/// What the rows are drawn from: the traces, and the controls' reference, if it's connected.
struct Rows<'a> {
    display: &'a TracesDisplay,
    stream: Continuous,
    spectra: Option<(&'a SpectraBuffer, Spectra)>,
    reference: Option<&'a Reference>,
    /// Whether the app's in advanced mode, where the Reference's settings can be got to.
    advanced: bool,
    /// What goes beside the button applying the selection: a button for its settings, in advanced mode.
    selection_settings: Option<&'a mut dyn FnMut(&mut egui::Ui)>,
}

impl Node<Edges> for LfpViewer {
    type State = Traces;
    /// What it shows, for its window.
    type Shared = Pool<TracesDisplay>;
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "LFP Viewer",
        category: Category::Visualisation,
        description: "Scrolling traces of continuous data, one row per channel, each with its spectrum over a band \
                      (theta, say). Sets an Continuous Gain's gains, a Channel Selector's channels and a Reference's \
                      reference channels.",
    };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![
            Input { name: "continuous", kind: Kind::Continuous },
            Input { name: "spectra", kind: Kind::Spectra },
            Input { name: "reference", kind: Kind::Continuous },
        ]
    }

    /// The spectra only while they're shown: otherwise, the Spectrum node needn't run at all. It runs while it's
    /// given something it needs.
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool]) -> TreeShake {
        let needs: Vec<bool> = (0..inputs.len()).map(|input| input != 1 || self.spectrum).collect();
        let given = inputs.iter().zip(&needs).any(|(&used, &needed)| used && needed);
        TreeShake { runs: given, needs }
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "a continuous data stream")
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Traces {
        let rate = match inputs[0] {
            Some(DataStream::Continuous(c)) => c.sample_rate,
            _ => 30000.0,
        };
        Traces { rate, ..Default::default() }
    }

    fn process(&self, pass: &mut Pass<Edges>, traces: &mut Traces) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let rate = traces.rate;
        // Every channel: the view scrolls through them.
        let (first, shown) = (0, input.channels);
        let per_column = ((self.span as f64 * rate as f64) / COLUMNS as f64).round().max(1.0) as usize;
        let references = match pass.inputs.get(2) {
            Some(Some(Buffer::Continuous(references))) => Some(references),
            _ => None,
        };
        let count = references.map_or(0, |r| r.channels);
        traces.reshape((first, shown, count, per_column));
        traces.add(input, first, references);
        let column_seconds = per_column as f64 / rate as f64;
        let spectra = match pass.inputs.get(1) {
            Some(Some(Buffer::Spectra(spectra))) if self.spectrum => Some(spectra),
            _ => None,
        };
        let Traces { lows, highs, head, columns, .. } = traces;
        pass.shared::<Pool<TracesDisplay>>().send_newest(pass.sequence, |display| {
            (display.first, display.shown, display.references, display.columns) = (first, shown, count, *columns);
            display.column_seconds = column_seconds;
            display.lows.clear();
            display.highs.clear();
            for i in 0..shown + count {
                // Oldest first: from the column after the one being filled, round to it.
                let row = i * COLUMNS..(i + 1) * COLUMNS;
                let (older, newer) = lows[row.clone()].split_at(*head);
                display.lows.extend_from_slice(newer);
                display.lows.extend_from_slice(older);
                let (older, newer) = highs[row].split_at(*head);
                display.highs.extend_from_slice(newer);
                display.highs.extend_from_slice(older);
            }
            match (spectra, &mut display.spectra) {
                (Some(spectra), Some(shown)) => {
                    shown.power.clone_from(&spectra.power);
                    shown.done.clone_from(&spectra.done);
                    shown.bins = spectra.bins;
                }
                (Some(spectra), shown) => {
                    *shown = Some(SpectraBuffer {
                        power: spectra.power.clone(),
                        done: spectra.done.clone(),
                        bins: spectra.bins,
                    })
                }
                (None, shown) => *shown = None,
            }
        });
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "gains", kind: SettingKind::Gains, read_only: false, on_right: false },
            Control { name: "selection", kind: SettingKind::ChannelSelection, read_only: false, on_right: false },
            Control { name: "reference", kind: SettingKind::Reference, read_only: false, on_right: false },
        ]
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let locked = context.locked;
        let Some(DataStream::Continuous(stream)) = context.inputs[0] else {
            return plots::unconnected(ui, context.theme, "Connect a continuous data stream to view it");
        };
        let mut selection = match &context.controls[SELECTION] {
            Some(Setting::ChannelSelection(s)) => Some(s.clone()),
            _ => None,
        };
        let mut reference = match &context.controls[REFERENCE] {
            Some(Setting::Reference(r)) => Some(r.clone()),
            _ => None,
        };
        let mut gains = match &context.controls[GAINS] {
            Some(Setting::Gains(g)) => Some(g.clone()),
            _ => None,
        };
        let pool = context.shared::<Pool<TracesDisplay>>();
        let display = pool.and_then(Pool::newest);
        let reference_rows = display.is_some() && context.inputs[2].is_some();
        let applied = selection.as_ref().is_some_and(|s| s.apply);
        self.toolbar(ui, context, stream, applied, reference.as_mut(), gains.as_mut(), reference_rows);

        let Some(display) = display else {
            let message = format!("{} · press Play to see the traces", stream.summary());
            return plots::unconnected(ui, context.theme, &message);
        };
        let spectra = match (&display.spectra, context.inputs[1]) {
            (Some(buffer), Some(DataStream::Spectra(stream))) => Some((buffer, stream)),
            _ => None,
        };
        let advanced = context.advanced();
        let theme = context.theme;
        let mut settings = |ui: &mut egui::Ui| open_settings_button(ui, context, SELECTION, "Channel Selector");
        let rows = Rows {
            display: &display,
            stream,
            spectra,
            reference: reference.as_ref(),
            advanced,
            selection_settings: advanced.then_some(&mut settings),
        };
        let edit = self.rows_ui(ui, theme, rows, selection.as_mut(), locked);
        if let (Some(edit), Some(reference)) = (edit, reference.as_mut()) {
            edit.apply(reference, stream.channels);
        }
        if let Some(selection) = selection {
            context.controls[SELECTION] = Some(Setting::ChannelSelection(selection));
        }
        if let Some(reference) = reference {
            context.controls[REFERENCE] = Some(Setting::Reference(reference));
        }
        if let Some(gains) = gains {
            context.controls[GAINS] = Some(Setting::Gains(gains));
        }
        plots::repaint_with_data(ui, context.running);
    }
}

impl LfpViewer {
    fn span_field(&mut self) -> egui::DragValue<'_> {
        egui::DragValue::new(&mut self.span).range(0.1..=30.0).speed(0.05).suffix(" s")
    }

    fn scale_field(&mut self) -> egui::DragValue<'_> {
        egui::DragValue::new(&mut self.scale_uv).range(10.0..=5000.0).speed(2.0).prefix("±").suffix(" µV")
    }

    /// The span, scale and rows' height, the spectrum, and (if they're connected) the gains and the reference, which
    /// can't be changed while a selection's `applied`: it can't be told from what's shown.
    #[allow(clippy::too_many_arguments)]
    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        context: &mut UiContext<Edges>,
        stream: Continuous,
        applied: bool,
        reference: Option<&mut Reference>,
        gains: Option<&mut Gains>,
        reference_rows: bool,
    ) {
        ui.add_enabled_ui(!context.locked, |ui| {
            ui.horizontal_wrapped(|ui| {
                inline_block(ui, "view", false, |ui| {
                    let label = ui.label("Span");
                    ui.add(self.span_field()).labelled_by(label.id).on_hover_text("Seconds across the view");
                    let label = ui.label("Scale");
                    ui.add(self.scale_field())
                        .labelled_by(label.id)
                        .on_hover_text("From a trace's middle to its row's edge");
                    let label = ui.label("Row height");
                    ui.add(egui::DragValue::new(&mut self.row_height).range(12.0..=400.0).speed(0.5).suffix(" px"))
                        .labelled_by(label.id);
                });
                if let Some(gains) = gains {
                    inline_block(ui, "gains", true, |ui| {
                        gains_ui(ui, context, gains, stream.channels);
                        if context.advanced() {
                            open_settings_button(ui, context, GAINS, "Continuous Gain");
                        }
                    });
                }
                if let Some(reference) = reference {
                    inline_block(ui, "reference", true, |ui| {
                        let shown = ui.add_enabled_ui(!applied, |ui| {
                            reference_ui(ui, reference, stream.channels, reference_rows);
                        });
                        shown.response.on_disabled_hover_text("Disable the selection to change it");
                        if context.advanced() {
                            open_settings_button(ui, context, REFERENCE, "Reference");
                        }
                    });
                }
                let tip = match context.inputs[1] {
                    Some(DataStream::Spectra(s)) => format!("Show each channel's spectrum, {}", s.summary()),
                    _ => "Show each channel's spectrum: connect a Spectrum node's".into(),
                };
                let spectrum = &mut self.spectrum;
                context.node_toolbar_buttons(ui, &mut |ui| {
                    if small_toggle_button(ui, Icon::Spectrum, "Spectrum", &tip, *spectrum).clicked() {
                        *spectrum = !*spectrum;
                    }
                });
            });
        });
    }

    /// A row for each channel shown, at the row height: its trace, and its spectrum; with its number to click to select it if there's a selection to make, and left of
    /// that an R for each reference if there are any. Above the first channel, scrolling with them, are what heads those
    /// columns, and the selection, if there's one. Returns any change to the references.
    fn rows_ui(
        &self,
        ui: &mut egui::Ui,
        theme: &ephys_rs::theme::Colours,
        rows_of: Rows,
        mut selection: Option<&mut ChannelSelection>,
        locked: bool,
    ) -> Option<ReferenceEdit> {
        let Rows { display, stream, spectra, reference, advanced, mut selection_settings } = rows_of;
        let references = display.references;
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 2.0, theme.surface_sunken);
        // Which channel upstream each row is, once a selection's applied.
        let numbers = selection.as_deref().map(|s| upstream(s, stream.channels));
        // While a selection's applied, there's no selecting, or changing the reference: that takes every channel upstream
        // being shown (see `toggle_reference`).
        let applied = selection.as_deref().is_some_and(|s| s.apply);
        let selected: Option<Vec<u32>> =
            selection.as_deref().filter(|_| !applied).map(|s| selected(s, stream.channels));
        let mut toggled = None;
        let mut reference_toggled = None;
        // The references first, at the top, then the channels.
        let rows_shown = references + display.shown;
        // Its rows' colours, whatever can be changed.
        let computations = reference.map(|reference| &reference.list[..]).filter(|list| list.len() == references);
        let reference = reference.filter(|_| !applied);
        let columns = reference.map_or(0, |reference| reference.list.len());
        let height = self.row_height.clamp(12.0, 400.0);
        // A reference's row has room for what's particular to it, however short the channels' are.
        let reference_height = if columns > 0 { height.max(REFERENCE_ROW_HEIGHT) } else { height };
        // Then, scrolling with them, a little space, the line heading the channels if there's anything for it, and the
        // channels.
        let heading = selection.is_some() || columns > 0;
        let toggles_height = if heading { TOGGLES_HEIGHT } else { 0.0 };
        let gap = if references > 0 { REFERENCES_GAP } else { 0.0 };
        let references_height = reference_height * references as f32 + gap;
        let channels_top = references_height + toggles_height;
        let row_top = |row: usize| match row.checked_sub(references) {
            None => reference_height * row as f32,
            Some(channel) => channels_top + height * channel as f32,
        };
        let numbers_at = numbers_x(columns);
        let label = numbers_at + NUMBERS_WIDTH + AXIS_WIDTH;
        egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
            let channels_bottom = channels_top + height * display.shown as f32;
            ui.set_height(channels_bottom + TIME_AXIS_HEIGHT);
            let body = ui.max_rect();
            if heading && viewport.min.y < channels_top && viewport.max.y > references_height {
                let place = Rect::from_min_size(
                    body.min + egui::vec2(0.0, references_height),
                    egui::vec2(body.width(), toggles_height),
                );
                if !applied {
                    let set = column_toggles(ui, place, selection.as_deref_mut(), reference, stream.channels, locked);
                    reference_toggled = reference_toggled.or(set);
                }
                // Over the channels' traces, the selection, if there is one.
                if let Some(selection) = selection.as_deref_mut().filter(|s| has_selection(s, stream.channels)) {
                    let line = place.with_min_x(place.left() + label);
                    let layout = egui::Layout::left_to_right(egui::Align::Center);
                    ui.scope_builder(egui::UiBuilder::new().max_rect(line).layout(layout), |ui| {
                        ui.add_enabled_ui(!locked, |ui| {
                            ui.horizontal_centered(|ui| {
                                selection_ui(ui, selection, stream.channels, selection_settings.take())
                            });
                        });
                    });
                }
            }
            // Only the rows in view.
            let channel_at = |y: f32| ((y - channels_top) / height).max(0.0);
            let last = (references + channel_at(viewport.max.y).ceil() as usize + 1).min(rows_shown);
            let first = match viewport.min.y < channels_top {
                true => 0,
                false => (references + channel_at(viewport.min.y).floor() as usize).min(last),
            };
            let rows = first..last;
            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                ui.skip_ahead_auto_ids(first);
                let (rect, width) = (body, body.width());
                let spectra = spectra.filter(|_| self.spectrum && width > SPECTRUM_WIDTH * 2.0);
                let spectrum = if spectra.is_some() { SPECTRUM_WIDTH } else { 0.0 };
                let painter = ui.painter().clone();
                // Up to now, with room for the future at the right (see `time_axis`).
                let future = 1.0 + ephys_rs::time_axis::FUTURE as f32;
                let column = (width - label - spectrum) / (COLUMNS as f32 * future);
                // The columns filled so far are the last of each row's.
                let start = COLUMNS - display.columns;
                if viewport.max.y > channels_bottom {
                    let span = COLUMNS as f64 * display.column_seconds;
                    let top = rect.top() + channels_bottom;
                    let axis = Rect::from_min_max(
                        egui::pos2(rect.left() + label, top),
                        egui::pos2(rect.right() - spectrum, top + TIME_AXIS_HEIGHT),
                    );
                    time_axis_ui(&painter, axis, ephys_rs::time_axis::window(0.0, span), theme);
                }
                for shown_row in rows {
                    let top = rect.top() + row_top(shown_row);
                    let height = if shown_row < references { reference_height } else { height };
                    let row = Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(width, height));
                    if shown_row > 0 {
                        painter.hline(row.x_range(), top, Stroke::new(1.0, theme.divider.gamma_multiply(0.3)));
                    }
                    let centre = top + height * 0.5;
                    let y = |uv: f32| centre - (uv / self.scale_uv).clamp(-1.0, 1.0) * height * 0.5;
                    let plot = egui::Rangef::new(rect.left() + label, row.right() - spectrum);
                    axis_ui(&painter, plot, top, height, self.scale_uv, theme);
                    // Row `i`'s trace: down and up each column, so its whole range is drawn.
                    let trace = |i: usize, colour: egui::Color32| {
                        let lows = &display.lows[i * COLUMNS..(i + 1) * COLUMNS];
                        let highs = &display.highs[i * COLUMNS..(i + 1) * COLUMNS];
                        let points: Vec<Pos2> = (start..COLUMNS)
                            .flat_map(|c| {
                                let x = rect.left() + label + (c as f32 + 0.5) * column;
                                let (a, b) = if c % 2 == 0 { (lows[c], highs[c]) } else { (highs[c], lows[c]) };
                                [egui::pos2(x, y(a)), egui::pos2(x, y(b))]
                            })
                            .collect();
                        painter.add(Shape::line(points, Stroke::new(1.0, colour)));
                    };
                    let text_y = top + (height * 0.5).min(9.0);
                    if shown_row < references {
                        // Its colour, if it's sure to be the reference in that column.
                        let computation = computations.and_then(|list| list.get(shown_row));
                        let colour = computation.map(|_| reference_colour(shown_row));
                        painter.text(
                            egui::pos2(rect.left() + numbers_at, text_y),
                            Align2::LEFT_CENTER,
                            format!("ref {}", shown_row + 1),
                            FontId::monospace(10.0),
                            colour.unwrap_or(theme.text_primary),
                        );
                        let hover = Rect::from_min_size(row.min, egui::vec2(label, height));
                        let tip = match computation {
                            Some(computation) => format!("The reference: the {}", computation.summary()),
                            None => "The reference".into(),
                        };
                        let whole =
                            ui.interact(hover, ui.scope_id().with(("reference row", shown_row)), Sense::hover());
                        // Only with the pointer on it and not on a button on it: egui would keep its tooltip open all over
                        // its rect, and the buttons' never get a turn.
                        if whole.hovered() {
                            whole.on_hover_text(tip);
                        }
                        // What's particular to it: its method below its name, and removing it, just left of that.
                        if let Some(computation) = computation.filter(|_| columns > 0) {
                            let method = Rect::from_min_size(
                                egui::pos2(rect.left() + numbers_at, top + 16.0),
                                egui::vec2(NUMBERS_WIDTH, 14.0),
                            );
                            let remove = Rect::from_center_size(
                                egui::pos2(rect.left() + column_x(columns - 1), text_y),
                                DELETE_SIZE,
                            );
                            let edit = ui
                                .add_enabled_ui(!locked, |ui| {
                                    reference_buttons(ui, method, remove, computation, shown_row)
                                })
                                .inner;
                            reference_toggled = reference_toggled.take().or(edit);
                        }
                        trace(display.shown + shown_row, colour.unwrap_or(theme.text_secondary));
                        continue;
                    }
                    let i = shown_row - references;
                    let channel = display.first + i;
                    let number = numbers.as_ref().and_then(|n| n.get(channel).copied()).unwrap_or(channel as u32);
                    // Its number, which selects it (to pass on, from a Channel Selector upstream) if that can be done.
                    let place = number_rect(rect.left() + numbers_at, text_y);
                    if let Some(selected) = &selected {
                        let on = selected.contains(&number);
                        let tip = format!("Select channel {}, to pass on (once the selection's applied)", number + 1);
                        let label = format!("Select channel {}", number + 1);
                        let response = neutral_toggle(ui, place, (on, false), (number + 1).to_string(), &label);
                        if response.on_hover_text(tip).clicked() && !locked {
                            toggled = Some(number);
                        }
                    } else {
                        painter.text(
                            place.center(),
                            Align2::CENTER_CENTER,
                            (number + 1).to_string(),
                            NUMBER_FONT,
                            theme.text_secondary,
                        );
                    }
                    // Which reference it's in, and uses, if any (a Reference's).
                    if let Some(reference) = reference {
                        let (agrees, using) = (reference.agrees(number), reference.used_by(number));
                        for k in 0..reference.list.len() {
                            let x = rect.left() + column_x(k);
                            let (name, channel, on) = (k + 1, number + 1, using == Some(k));
                            let place = Rect::from_center_size(egui::pos2(x, text_y), TOGGLE_SIZE);
                            let label = format!("Channel {channel} in reference {name}");
                            let colour = reference_colour(k);
                            let response = ui
                                .add_enabled_ui(agrees, |ui| {
                                    r_toggle(ui, place, (on, false), "R".into(), colour, &label)
                                })
                                .inner;
                            let tip = if on {
                                format!("Channel {channel} is in reference group {name}: click to take it out")
                            } else {
                                format!("Put channel {channel} in reference group {name}")
                            };
                            let response =
                                response.on_hover_text(tip).on_disabled_hover_text(mismatch(channel, advanced));
                            if response.clicked() && !locked {
                                reference_toggled = Some(ReferenceEdit::Toggle(k, number));
                            }
                        }
                    }

                    // The trace: not selected (with a selection to apply), dimmed, as it won't be passed on.
                    let unselected = selected.as_ref().is_some_and(|t| !t.contains(&number));
                    let colour = if unselected { theme.text_primary.gamma_multiply(0.3) } else { theme.text_primary };
                    trace(i, colour);

                    if let Some((buffer, stream)) = spectra {
                        let place = Rect::from_min_max(egui::pos2(row.right() - spectrum, top), row.max).shrink(2.0);
                        spectrum_ui(&painter, place, buffer, stream, i, theme);
                    }
                }
            });
        });
        if let (Some(channel), Some(selection), Some(mut selected)) = (toggled, selection, selected) {
            match selected.iter().position(|&c| c == channel) {
                Some(at) => {
                    selected.remove(at);
                }
                None => selected.push(channel),
            }
            set_selected(selection, &selected, stream.channels);
        }
        reference_toggled
    }
}

/// A channel's spectrum over the band, scaled to its biggest bin, with the peak's frequency.
fn spectrum_ui(
    painter: &egui::Painter,
    place: Rect,
    spectra: &SpectraBuffer,
    stream: Spectra,
    i: usize,
    theme: &ephys_rs::theme::Colours,
) {
    painter.rect_filled(place, 2.0, theme.surface_raised.gamma_multiply(0.5));
    let bins = spectra.bins;
    if !spectra.done.get(i).copied().unwrap_or(false) || bins == 0 || spectra.power.len() < (i + 1) * bins {
        if place.height() >= 12.0 {
            painter.text(place.center(), Align2::CENTER_CENTER, "…", FontId::proportional(10.0), theme.text_disabled);
        }
        return;
    }
    let power = &spectra.power[i * bins..(i + 1) * bins];
    let (peak, &most) = power.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
    let (first_hz, step) = (stream.first_hz, stream.step_hz);
    let span = (stream.last_hz() - first_hz).max(0.1);
    let x = |hz: f32| place.left() + ((hz - first_hz) / span).clamp(0.0, 1.0) * place.width();
    let y = |p: f32| place.bottom() - if most > 0.0 { p / most } else { 0.0 } * place.height();
    let colour = theme.text_primary;
    let points: Vec<Pos2> =
        power.iter().enumerate().map(|(b, &p)| egui::pos2(x(first_hz + b as f32 * step), y(p))).collect();
    // Filled beneath (as quads: the outline needn't be convex).
    for pair in points.windows(2) {
        let quad = vec![pair[0], pair[1], egui::pos2(pair[1].x, place.bottom()), egui::pos2(pair[0].x, place.bottom())];
        painter.add(Shape::convex_polygon(quad, colour.gamma_multiply(0.25), Stroke::NONE));
    }
    painter.add(Shape::line(points, Stroke::new(1.0, colour)));
    let peak_hz = first_hz + peak as f32 * step;
    painter.vline(x(peak_hz), place.y_range(), Stroke::new(1.0, theme.text_secondary.gamma_multiply(0.5)));
    if place.height() >= 10.0 {
        painter.text(
            place.right_top() + egui::vec2(-3.0, 1.0),
            Align2::RIGHT_TOP,
            format!("{peak_hz:.1} Hz"),
            FontId::monospace(9.0),
            theme.text_primary,
        );
    }
}

/// What's particular to reference `index`, as words in `method` and `remove` that light up: whether it's a mean or a median,
/// which clicking swaps, and removing it.
fn reference_buttons(
    ui: &mut egui::Ui,
    method: Rect,
    remove: Rect,
    computation: &Computation,
    index: usize,
) -> Option<ReferenceEdit> {
    let (tip, other) = match computation.method {
        Method::Mean => ("A common average reference: click for the median", Method::Median),
        Method::Median => {
            ("A common median reference, which a spike on a channel hardly moves: click for the mean", Method::Mean)
        }
    };
    let name = computation.method.name();
    let at = (method.left_center(), Align2::LEFT_CENTER, FontId::monospace(10.0));
    if word_button(ui, method, name, at, ui.visuals().text_color(), name, index).on_hover_text(tip).clicked() {
        return Some(ReferenceEdit::Method(index, other));
    }
    let label = format!("Delete reference {}", index + 1);
    let deleted = ui.scope_builder(egui::UiBuilder::new().max_rect(remove), |ui| {
        small_toggle_button(ui, Icon::Delete, &label, "Delete this reference", false).clicked()
    });
    deleted.inner.then_some(ReferenceEdit::Remove(index))
}

/// `text`, in `rect`, as a button with no frame: quiet until it's hovered, when it's `hovered`.
fn word_button(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &str,
    (at, align, font): (Pos2, Align2, FontId),
    hovered: egui::Color32,
    label: &str,
    index: usize,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with((label, index)), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let ink = if response.hovered() { hovered } else { ui.visuals().weak_text_color() };
    ui.painter().text(at, align, text, font, ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The references (a Reference's, for the spikes), compactly: adding one, and, if they've no rows of their own to have it
/// on, what's particular to each.
fn reference_ui(ui: &mut egui::Ui, reference: &mut Reference, channels: u32, rows: bool) {
    ui.strong("Reference").on_hover_text(
        "What each channel has subtracted: its reference group's mean or median. R puts a channel in a group, and a \
         channel's in one at most. The traces here aren't referenced",
    );
    if reference.list.is_empty() {
        ui.weak("none");
    }
    if !rows {
        let mut edit = None;
        for (index, computation) in reference.list.iter().enumerate() {
            ui.label(egui::RichText::new((index + 1).to_string()).color(reference_colour(index)));
            let (method, _) = ui.allocate_exact_size(egui::vec2(NUMBERS_WIDTH, 14.0), Sense::hover());
            let (remove, _) = ui.allocate_exact_size(DELETE_SIZE, Sense::hover());
            let buttons = reference_buttons(ui, method, remove, computation, index);
            edit = edit.or(buttons);
        }
        if let Some(edit) = edit {
            edit.apply(reference, channels);
        }
    }
    if ui.small_button("+").on_hover_text("Add a reference group, of the channels in none").clicked() {
        reference.add(channels);
    }
}

/// The gains (a Continuous Gain's: what the continuous data's amplified by, as it's recorded), compactly: one for all,
/// or that they're each channel's; and detecting them from the data.
fn gains_ui(ui: &mut egui::Ui, context: &mut UiContext<Edges>, gains: &mut Gains, channels: u32) {
    let label = ui.strong("Gain");
    let label = label.on_hover_text(
        "The gains the continuous data's amplified by, as it's recorded (×2 doubles it). The traces here are as it \
         comes in, in µV, before them",
    );
    let layout = Channels { count: channels as usize, group_size: 1 };
    let detecting = context.trigger_progress(Some(GAINS), crate::TriggerKind::AutoDetect).is_some();
    if gains.scope == GainScope::All {
        let mut gain = gains.get(0);
        let field = ui.add_enabled_ui(!detecting, |ui| gain_field(ui, &mut gain, false)).inner;
        if field.labelled_by(label.id).changed() {
            gains.set(0, gain, layout);
        }
    } else {
        ui.label("each channel's");
    }
    auto_detect_ui(ui, context, Some(GAINS), "Sample each channel, then set its gain");
}

/// Whether a selection's channels are "all".
fn is_all(selection: &ChannelSelection) -> bool {
    let text = selection.channels.trim();
    text.is_empty() || text.eq_ignore_ascii_case("all")
}

/// The channels a selection has selected (upstream), given `channels` coming in.
fn selected(selection: &ChannelSelection, channels: u32) -> Vec<u32> {
    if is_all(selection) {
        return (0..channels).collect();
    }
    if selection.apply {
        // Those coming in are the selection; upstream there may be more.
        return parse_channels(&selection.channels, u32::MAX / 2).unwrap_or_default();
    }
    parse_channels(&selection.channels, channels).unwrap_or_default()
}

/// Which channel upstream each one coming in is.
fn upstream(selection: &ChannelSelection, channels: u32) -> Vec<u32> {
    let selected = selected(selection, channels);
    if selection.apply && selected.len() == channels as usize { selected } else { (0..channels).collect() }
}

/// Selects `selected` (upstream) channels.
fn set_selected(selection: &mut ChannelSelection, selected: &[u32], channels: u32) {
    // Applied, those coming in aren't all there are upstream, so "all" can't be told.
    let count = if selection.apply { u32::MAX } else { channels };
    selection.channels = format_channels(selected, count);
}

/// Whether there's a selection to speak of: one applied, or some channels left out of one that could be.
fn has_selection(selection: &ChannelSelection, channels: u32) -> bool {
    selection.apply || selected(selection, channels).len() < channels as usize
}

/// The selection, if there is one (see `has_selection`): the button to apply or disable it, then `settings`, then whether
/// it's applied, or waiting to be.
fn selection_ui(
    ui: &mut egui::Ui,
    selection: &mut ChannelSelection,
    channels: u32,
    settings: Option<&mut dyn FnMut(&mut egui::Ui)>,
) {
    let selected = selected(selection, channels);
    let tip = "Pass on only the selected channels, here and to everything after the Channel Selector";
    let mut apply = selection.apply;
    let text = if apply { "Disable selection" } else { "Apply selection" };
    if ui.add(egui::Button::new(text).small()).on_hover_text(tip).clicked() {
        apply = !apply;
    }
    if apply != selection.apply {
        // The same channels, written for what's coming in after the change.
        selection.apply = apply;
        if !apply && !is_all(selection) {
            selection.channels = format_channels(&selected, u32::MAX);
        }
    }
    if let Some(settings) = settings {
        settings(ui);
    }
    if selection.apply {
        ui.label("Selection applied");
    } else {
        let text = if selected.is_empty() { "Nothing selected" } else { "Selection pending" };
        ui.colored_label(ui.visuals().error_fg_color, text);
    }
}

/// How far across a row the first of its Rs starts.
const ROW_MARGIN: f32 = 4.0;
/// The room for a channel's number, or a reference's name or method, right of the Rs.
const NUMBERS_WIDTH: f32 = 40.0;
/// A channel's number, as a toggle, and as text.
const NUMBER_SIZE: egui::Vec2 = egui::vec2(28.0, 14.0);
const NUMBER_FONT: FontId = FontId::monospace(10.0);

/// How high the line heading the channels is.
const TOGGLES_HEIGHT: f32 = 22.0;

/// How high a reference's row has to be, for what's particular to it.
const REFERENCE_ROW_HEIGHT: f32 = 32.0;
/// The space between the references' rows and the channels'.
const REFERENCES_GAP: f32 = 8.0;
/// From one reference's column to the next: an R's width, and little between them, so a good few fit.
const COLUMN_PITCH: f32 = 18.0;
/// The space between the Rs and the channels' numbers.
const COLUMNS_GAP: f32 = 10.0;
const TOGGLE_SIZE: egui::Vec2 = egui::vec2(16.0, 14.0);
/// The room a small icon button takes.
const DELETE_SIZE: egui::Vec2 = egui::vec2(20.0, 20.0);

/// The middle of reference `k`'s column, from a row's left.
fn column_x(k: usize) -> f32 {
    ROW_MARGIN + TOGGLE_SIZE.x / 2.0 + k as f32 * COLUMN_PITCH
}

/// Where the numbers start, from a row's left, right of `columns` columns of Rs.
fn numbers_x(columns: usize) -> f32 {
    let gap = if columns > 0 { COLUMNS_GAP } else { 0.0 };
    ROW_MARGIN + columns as f32 * COLUMN_PITCH + gap
}

/// Where a number goes, for numbers starting at `left`, with its middle at `y`.
fn number_rect(left: f32, y: f32) -> Rect {
    Rect::from_min_size(egui::pos2(left, y - NUMBER_SIZE.y / 2.0), NUMBER_SIZE)
}

/// Why channel `channel` (from 1) can't be changed here.
fn mismatch(channel: u32, advanced: bool) -> String {
    let there = if advanced { "the Reference's settings" } else { "the Reference's settings, in advanced mode" };
    format!("Channel {channel} can't be changed here: it isn't simply in one reference group. Fix that in {there}")
}

/// A change to the references from among the rows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ReferenceEdit {
    /// Put this channel (upstream, from 0) in this reference, or take it out.
    Toggle(usize, u32),
    /// Put all the channels in this reference, or, if not, none of them.
    All(usize, bool),
    Method(usize, Method),
    Remove(usize),
}

impl ReferenceEdit {
    fn apply(self, reference: &mut Reference, channels: u32) {
        match self {
            ReferenceEdit::Toggle(index, channel) => reference.toggle_channel(index, channel, channels),
            ReferenceEdit::All(index, all) => reference.set_all(index, all, channels),
            ReferenceEdit::Method(index, method) => {
                if let Some(computation) = reference.list.get_mut(index) {
                    computation.method = method;
                }
            }
            ReferenceEdit::Remove(index) => {
                if index < reference.list.len() {
                    reference.list.remove(index);
                }
            }
        }
    }
}

/// A toggle in `rect`, exactly that size however little room there is, with a dash if some of what it's for is on, and
/// its fill and ink from `look`, given whether it's hovered.
fn toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    (on, mixed): (bool, bool),
    (text, font): (String, FontId),
    label: &str,
    look: impl FnOnce(&egui::Visuals, bool) -> (egui::Color32, egui::Color32),
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(label), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, label));
    let fade = if ui.is_enabled() { 1.0 } else { 0.4 };
    let (fill, ink) = look(ui.visuals(), response.hovered());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, fill.gamma_multiply(fade));
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, ink.gamma_multiply(fade));
    if mixed {
        painter.hline(rect.shrink(4.0).x_range(), rect.bottom() - 1.5, Stroke::new(1.0, ink));
    }
    response
}

/// An R, filled with its reference's `colour` if it's on.
fn r_toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    (on, mixed): (bool, bool),
    text: String,
    colour: egui::Color32,
    label: &str,
) -> egui::Response {
    toggle(ui, rect, (on, mixed), (text, FontId::proportional(9.5)), label, |visuals, hovered| match (on, hovered) {
        (true, _) => (colour, egui::Color32::BLACK),
        (false, true) => (visuals.widgets.hovered.weak_bg_fill, colour),
        (false, false) => (visuals.widgets.inactive.weak_bg_fill, colour),
    })
}

/// A toggle without a colour, leaving those to the references: on, it stands out from the background, off, it recedes.
fn neutral_toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    (on, mixed): (bool, bool),
    text: String,
    label: &str,
) -> egui::Response {
    toggle(ui, rect, (on, mixed), (text, NUMBER_FONT), label, |visuals, hovered| {
        let fill = match (on, hovered) {
            (_, true) => visuals.widgets.hovered.weak_bg_fill,
            (true, false) => visuals.widgets.inactive.weak_bg_fill,
            (false, false) => egui::Color32::TRANSPARENT,
        };
        (fill, if on { visuals.text_color() } else { visuals.weak_text_color() })
    })
}

/// The toggle for all the channels, or none: a check mark on a number's background, as it's over them, a dash if it's only
/// some, as on a checkbox, and nothing if it's none.
fn all_toggle(ui: &mut egui::Ui, rect: Rect, (all, some): (bool, bool)) -> egui::Response {
    let response = toggle(ui, rect, (all, false), (String::new(), NUMBER_FONT), "All", |visuals, hovered| {
        let widgets = if hovered { &visuals.widgets.hovered } else { &visuals.widgets.inactive };
        (widgets.weak_bg_fill, visuals.text_color())
    });
    let ink = ui.visuals().text_color().gamma_multiply(if ui.is_enabled() { 1.0 } else { 0.4 });
    let (centre, stroke) = (rect.center(), Stroke::new(1.5, ink));
    if all {
        let mark = [(-4.0, 0.0), (-1.0, 3.0), (4.0, -3.0)].map(|(x, y)| centre + egui::vec2(x, y));
        ui.painter().add(Shape::line(mark.to_vec(), stroke));
    } else if some {
        ui.painter().hline(centre.x - 4.0..=centre.x + 4.0, centre.y, stroke);
    }
    response
}

/// What heads the channels' columns, in `rect`: a toggle for all of them, or none, over their numbers, and an R likewise
/// for each reference (going by the channels that can be changed: see `Reference::agrees`). Returns any change to the
/// references.
fn column_toggles(
    ui: &mut egui::Ui,
    rect: Rect,
    selection: Option<&mut ChannelSelection>,
    reference: Option<&Reference>,
    channels: u32,
    locked: bool,
) -> Option<ReferenceEdit> {
    if let Some(selection) = selection {
        let count = selected(selection, channels).len();
        let all = count == channels as usize;
        let columns = reference.map_or(0, |reference| reference.list.len());
        let place = number_rect(rect.left() + numbers_x(columns), rect.center().y);
        let state = (all, count > 0 && !all);
        let response = ui.add_enabled_ui(!locked, |ui| all_toggle(ui, place, state)).inner;
        if response.on_hover_text("Select all the channels, or none").clicked() {
            selection.channels = if all { format_channels(&[], channels) } else { "all".into() };
        }
    }
    let reference = reference?;
    let able: Vec<u32> = (0..channels).filter(|&channel| reference.agrees(channel)).collect();
    let mut edit = None;
    for k in 0..reference.list.len() {
        let name = k + 1;
        let count = able.iter().filter(|&&channel| reference.used_by(channel) == Some(k)).count();
        let (all, mixed) = (!able.is_empty() && count == able.len(), count > 0 && count < able.len());
        let place = Rect::from_center_size(egui::pos2(rect.left() + column_x(k), rect.center().y), TOGGLE_SIZE);
        let text = format!("R{name}");
        let colour = reference_colour(k);
        let response =
            ui.add_enabled_ui(!locked, |ui| r_toggle(ui, place, (all, mixed), text.clone(), colour, &text)).inner;
        let tip = if mixed {
            format!("Some in reference group {name}: click for all")
        } else {
            format!("Put all the channels in reference group {name}, or none")
        };
        if response.on_hover_text(tip).clicked() {
            edit = Some(ReferenceEdit::All(k, !all));
        }
    }
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_y_axis_has_as_many_ticks_as_there_is_room_for() {
        let labels = |height, scale| {
            crate::standard::plots::axis_ticks(height, scale).into_iter().map(|t| t.1).collect::<Vec<_>>()
        };
        assert_eq!(labels(12.0, 200.0), ["±200"]);
        assert_eq!(labels(24.0, 200.0), ["200 µV", "−200"]);
        assert_eq!(labels(40.0, 200.0), ["200 µV", "0", "−200"]);
        assert_eq!(labels(120.0, 25.0), ["25 µV", "12.5", "0", "−12.5", "−25"]);
    }

    /// Rows of a reference and channels, with their y axes. See tests/snapshots/.
    #[test]
    fn rows() {
        use crate::standard::themes::THEMES;
        let (shown, references) = (3, 1);
        let wave = |i: usize, c: usize| 150.0 * ((c as f32 / 40.0) + i as f32).sin() / (i + 1) as f32;
        let columns = |offset: f32| -> Vec<f32> {
            (0..shown + references).flat_map(|i| (0..COLUMNS).map(move |c| wave(i, c) + offset)).collect()
        };
        let display = TracesDisplay {
            shown,
            references,
            columns: COLUMNS,
            column_seconds: 2.0 / COLUMNS as f64,
            lows: columns(-5.0),
            highs: columns(5.0),
            ..Default::default()
        };
        let stream = Continuous { channels: 3, sample_rate: 30000.0 };
        let mut harness = egui_kittest::Harness::builder().with_size([700.0, 280.0]).wgpu().build_ui(|ui| {
            let viewer = LfpViewer { row_height: 60.0, ..Default::default() };
            let rows = Rows {
                display: &display,
                stream,
                spectra: None,
                reference: None,
                advanced: true,
                selection_settings: None,
            };
            viewer.rows_ui(ui, &THEMES[0].colours, rows, None, false);
        });
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("lfp_viewer_rows");
    }

    /// The toggles for the columns, in the body of the pane, above the rows' Rs and numbers, and the selection beside
    /// them. See tests/snapshots/.
    #[test]
    fn toggles_head_their_columns() {
        use crate::standard::themes::THEMES;
        let (shown, columns) = (3, vec![0.0; 3 * COLUMNS]);
        let display =
            TracesDisplay { shown, columns: COLUMNS, lows: columns.clone(), highs: columns, ..Default::default() };
        let stream = Continuous { channels: 3, sample_rate: 30000.0 };
        let reference = reference_of("1");
        let mut selection = ChannelSelection { apply: false, channels: "1-2".into() };
        let mut harness = egui_kittest::Harness::builder().with_size([400.0, 160.0]).wgpu().build_ui(|ui| {
            let viewer = LfpViewer { row_height: 40.0, ..Default::default() };
            let rows = Rows {
                display: &display,
                stream,
                spectra: None,
                reference: Some(&reference),
                advanced: true,
                selection_settings: None,
            };
            viewer.rows_ui(ui, &THEMES[0].colours, rows, Some(&mut selection), false);
        });
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("lfp_viewer_toggles");
    }

    #[test]
    fn with_a_selection_applied_there_is_nothing_to_select_or_put_in_the_reference() {
        use egui_kittest::kittest::Queryable;
        let (shown, columns) = (2, vec![0.0; 2 * COLUMNS]);
        let display =
            TracesDisplay { shown, columns: COLUMNS, lows: columns.clone(), highs: columns, ..Default::default() };
        let stream = Continuous { channels: 2, sample_rate: 30000.0 };
        let reference = reference_of("1");
        let mut selection = ChannelSelection { apply: true, channels: "1, 3".into() };
        let mut harness = egui_kittest::Harness::builder().with_size([400.0, 160.0]).wgpu().build_ui(|ui| {
            let viewer = LfpViewer { row_height: 40.0, ..Default::default() };
            let rows = Rows {
                display: &display,
                stream,
                spectra: None,
                reference: Some(&reference),
                advanced: true,
                selection_settings: None,
            };
            let colours = &crate::standard::themes::THEMES[0].colours;
            viewer.rows_ui(ui, colours, rows, Some(&mut selection), false);
        });
        harness.run_steps(3);
        assert!(harness.query_by_label("All").is_none());
        assert!(harness.query_by_label("Select channel 1").is_none());
        assert!(harness.query_by_label("R").is_none());
        harness.get_by_label("Selection applied");
        harness.get_by_label("Disable selection");
    }

    /// With nothing passed on, there are no rows, but the selection can still be switched off.
    #[test]
    fn with_nothing_selected_and_applied_it_can_still_be_disabled() {
        use egui_kittest::kittest::Queryable as _;
        let stream = Continuous { channels: 0, sample_rate: 30000.0 };
        let mut traces = Traces::default();
        traces.reshape((0, 0, 0, 5));
        let input = ContinuousBuffer { channels: 0, capacity: 32, len: 32, data: Vec::new() };
        traces.add(&input, 0, None);
        let display = TracesDisplay::default();
        let selection = ChannelSelection { apply: true, channels: "none".into() };
        let mut harness = egui_kittest::Harness::builder().with_size([400.0, 100.0]).wgpu().build_ui_state(
            move |ui, selection: &mut ChannelSelection| {
                let viewer = LfpViewer::default();
                let rows = Rows {
                    display: &display,
                    stream,
                    spectra: None,
                    reference: None,
                    advanced: true,
                    selection_settings: None,
                };
                let colours = &crate::standard::themes::THEMES[0].colours;
                viewer.rows_ui(ui, colours, rows, Some(selection), false);
            },
            selection,
        );
        harness.run_steps(3);
        harness.get_by_label("Selection applied");
        harness.get_by_label("Disable selection").click();
        harness.run_steps(3);
        assert!(!harness.state().apply);
        assert_eq!(harness.state().channels, "none");
    }

    /// The Rs left of the numbers, each number selecting its channel, and the selection on the line above the first.
    #[test]
    fn a_channels_number_selects_it_and_the_selection_is_applied_above_them() {
        use egui_kittest::kittest::NodeT as _;
        use egui_kittest::kittest::Queryable as _;
        let (shown, columns) = (3, vec![0.0; 3 * COLUMNS]);
        let display =
            TracesDisplay { shown, columns: COLUMNS, lows: columns.clone(), highs: columns, ..Default::default() };
        let stream = Continuous { channels: 3, sample_rate: 30000.0 };
        let reference = reference_of("1");
        let selection = ChannelSelection { apply: false, channels: "all".into() };
        let mut harness = egui_kittest::Harness::builder().with_size([500.0, 200.0]).wgpu().build_ui_state(
            move |ui, selection: &mut ChannelSelection| {
                let viewer = LfpViewer { row_height: 40.0, ..Default::default() };
                let mut settings = |ui: &mut egui::Ui| {
                    let _ = ui.button("Settings");
                };
                let rows = Rows {
                    display: &display,
                    stream,
                    spectra: None,
                    reference: Some(&reference),
                    advanced: true,
                    selection_settings: Some(&mut settings),
                };
                let colours = &crate::standard::themes::THEMES[0].colours;
                viewer.rows_ui(ui, colours, rows, Some(selection), false);
            },
            selection,
        );
        harness.run_steps(3);
        assert!(harness.query_by_label("Apply selection").is_none(), "nothing left out");
        let area = |node: egui_kittest::Node| node.accesskit_node().bounding_box().unwrap();
        let (r, number) =
            (area(harness.get_by_label("Channel 2 in reference 1")), area(harness.get_by_label("Select channel 2")));
        assert!(r.x1 <= number.x0, "R to {}, number from {}", r.x1, number.x0);
        assert!(((r.y0 + r.y1) - (number.y0 + number.y1)).abs() < 0.6, "level");
        harness.get_by_label("Select channel 2").click();
        harness.run_steps(3);
        assert_eq!(harness.state().channels, "1, 3");
        harness.get_by_label("Selection pending");
        let apply = area(harness.get_by_label("Apply selection"));
        assert!(apply.y1 <= number.y0, "above the first channel");
        assert!(apply.x0 >= number.x1, "over the traces");
        let (settings, said) =
            (area(harness.get_by_label("Settings")), area(harness.get_by_label("Selection pending")));
        assert!(apply.x1 <= settings.x0 && settings.x1 <= said.x0, "settings right of the button, before the words");
        harness.get_by_label("Apply selection").click();
        harness.run_steps(3);
        assert!(harness.state().apply);
    }

    #[test]
    fn columns_hold_each_runs_lowest_and_highest() {
        // Passes of uneven lengths, so columns fill across them.
        let (channels, per_column) = (3, 7);
        let mut traces = Traces::default();
        traces.reshape((0, channels, 0, per_column));
        let value = |c: usize, t: usize| ((t * 37 + c * 11) % 23) as f32 - (c * t % 5) as f32;
        let mut t0 = 0;
        for len in [5, 9, 1, 20, 3, 13] {
            let mut data = vec![0.0; channels * 32];
            for c in 0..channels {
                for t in 0..len {
                    data[c * 32 + t] = value(c, t0 + t);
                }
            }
            traces.add(&ContinuousBuffer { channels, capacity: 32, len, data }, 0, None);
            t0 += len;
        }
        assert_eq!(traces.columns, t0 / per_column);
        for c in 0..channels {
            for column in 0..t0 / per_column {
                let run: Vec<f32> = (column * per_column..(column + 1) * per_column).map(|t| value(c, t)).collect();
                let low = run.iter().copied().fold(f32::INFINITY, f32::min);
                let high = run.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                assert_eq!((traces.lows[c * COLUMNS + column], traces.highs[c * COLUMNS + column]), (low, high));
            }
        }
    }

    /// Several references, their rows matching their columns, with channels in each of them, or none, and one that's been
    /// set otherwise.
    #[test]
    fn references_have_a_column_each_matching_their_rows() {
        use crate::standard::ephys::reference::has_channel;
        use crate::standard::themes::THEMES;
        use egui_kittest::kittest::NodeT as _;
        use egui_kittest::kittest::Queryable as _;
        let (shown, references) = (5, 3);
        let wave = |i: usize, c: usize| 120.0 * ((c as f32 / 40.0) + i as f32).sin() / (i % 3 + 1) as f32;
        let columns = |offset: f32| -> Vec<f32> {
            (0..shown + references).flat_map(|i| (0..COLUMNS).map(move |c| wave(i, c) + offset)).collect()
        };
        let display = TracesDisplay {
            shown,
            references,
            columns: COLUMNS,
            lows: columns(-5.0),
            highs: columns(5.0),
            ..Default::default()
        };
        let stream = Continuous { channels: 5, sample_rate: 30000.0 };
        let made = |id, method, channels: &str, used_by: &str| Computation {
            id,
            method,
            channels: channels.into(),
            used_by: used_by.into(),
        };
        let reference = Reference {
            list: vec![
                made(0, Method::Mean, "1-2", "1-2"),
                made(1, Method::Median, "3-4", "3"),
                made(2, Method::Mean, "none", "none"),
            ],
            next_id: 3,
        };
        let mut harness = egui_kittest::Harness::builder().with_size([700.0, 330.0]).wgpu().build_ui_state(
            move |ui, reference: &mut Reference| {
                let viewer = LfpViewer { row_height: 24.0, ..Default::default() };
                let reference_shown = Some(&*reference);
                let rows = Rows {
                    display: &display,
                    stream,
                    spectra: None,
                    reference: reference_shown,
                    advanced: false,
                    selection_settings: None,
                };
                if let Some(edit) = viewer.rows_ui(ui, &THEMES[0].colours, rows, None, false) {
                    edit.apply(reference, 5);
                }
            },
            reference,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("lfp_viewer_references");
        harness.get_by_label("Channel 5 in reference 3").click();
        harness.run_steps(3);
        assert_eq!(harness.state().used_by(4), Some(2));
        assert!(has_channel(&harness.state().list[2].channels, 4), "in it, and using it");
        // One at most, and clicked off.
        harness.get_by_label("Channel 1 in reference 3").click();
        harness.run_steps(3);
        assert_eq!(harness.state().used_by(0), Some(2));
        assert!(!has_channel(&harness.state().list[0].channels, 0), "out of the other");
        harness.get_by_label("Channel 1 in reference 3").click();
        harness.run_steps(3);
        assert_eq!(harness.state().used_by(0), None);
        // Channel 4's in the second, but uses none: that's for the settings.
        assert!(harness.get_by_label("Channel 4 in reference 2").accesskit_node().is_disabled());
        // Resting on a reference's row, then moving on to a button on it: the button's tooltip takes over.
        let name = harness.get_by_label("median").rect().center() - egui::vec2(0.0, 14.0);
        harness.event(egui::Event::PointerMoved(name));
        harness.run_steps(8);
        assert!(harness.query_by_label_contains("The reference: the").is_some(), "the row's");
        harness.get_by_label("Delete reference 2").hover();
        harness.run_steps(8);
        assert!(harness.query_by_label("Delete this reference").is_some(), "the button's");
        assert!(harness.query_by_label_contains("The reference: the").is_none(), "and not the row's");
        // On a reference's own row: its method, and removing it.
        harness.get_by_label("median").click();
        harness.run_steps(3);
        assert_eq!(harness.state().list[1].method, Method::Mean);
        harness.get_by_label("Delete reference 3").click();
        harness.run_steps(3);
        assert_eq!(harness.state().list.len(), 2);
    }

    /// With a selection applied, nothing can be changed, but what's what can still be told.
    #[test]
    fn references_keep_their_colours_with_a_selection_applied() {
        use crate::standard::themes::THEMES;
        let (shown, references) = (2, 2);
        let columns = |offset: f32| -> Vec<f32> {
            (0..shown + references)
                .flat_map(|i| (0..COLUMNS).map(move |c| 100.0 * ((c as f32 / 40.0) + i as f32).sin() + offset))
                .collect()
        };
        let display = TracesDisplay {
            shown,
            references,
            columns: COLUMNS,
            lows: columns(-5.0),
            highs: columns(5.0),
            ..Default::default()
        };
        let stream = Continuous { channels: 2, sample_rate: 30000.0 };
        let mut reference = reference_of("1");
        reference.add(2);
        let mut selection = ChannelSelection { apply: true, channels: "1-2".into() };
        let mut harness = egui_kittest::Harness::builder().with_size([500.0, 200.0]).wgpu().build_ui(|ui| {
            let viewer = LfpViewer { row_height: 40.0, ..Default::default() };
            let rows = Rows {
                display: &display,
                stream,
                spectra: None,
                reference: Some(&reference),
                advanced: true,
                selection_settings: None,
            };
            viewer.rows_ui(ui, &THEMES[0].colours, rows, Some(&mut selection), false);
        });
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("lfp_viewer_references_applied");
    }

    #[test]
    fn what_cannot_be_changed_says_where_it_can() {
        assert!(mismatch(4, true).starts_with("Channel 4 can't"));
        assert!(mismatch(4, true).ends_with("in the Reference's settings"));
        assert!(mismatch(4, false).ends_with("in advanced mode"), "which it takes to get to them");
    }

    #[test]
    fn edits_from_the_rows_change_the_references() {
        let mut reference = reference_of("1");
        ReferenceEdit::Toggle(0, 1).apply(&mut reference, 4);
        assert_eq!((reference.list[0].channels.as_str(), reference.list[0].used_by.as_str()), ("1-2", "1-2"));
        ReferenceEdit::All(0, true).apply(&mut reference, 4);
        assert_eq!(reference.list[0].channels, "all");
        ReferenceEdit::All(0, false).apply(&mut reference, 4);
        assert_eq!((reference.list[0].channels.as_str(), reference.list[0].used_by.as_str()), ("none", "none"));
        ReferenceEdit::Method(0, Method::Median).apply(&mut reference, 4);
        assert_eq!(reference.list[0].method, Method::Median);
        ReferenceEdit::Remove(0).apply(&mut reference, 4);
        assert!(reference.list.is_empty());
        ReferenceEdit::Remove(0).apply(&mut reference, 4);
    }

    #[test]
    fn each_reference_has_a_colour_of_its_own_and_none_is_blue() {
        use crate::standard::ephys::reference::REFERENCE_COLOURS;
        let colours: Vec<_> = (0..REFERENCE_COLOURS.len()).map(reference_colour).collect();
        assert!(colours.iter().enumerate().all(|(i, c)| !colours[..i].contains(c)));
        assert_eq!(reference_colour(REFERENCE_COLOURS.len()), colours[0], "they go round");
        assert!(!colours.contains(&Continuous::INFO.colour));
    }

    /// The toggles at the top of the columns, and the selection.
    fn toggles(
        selection: ChannelSelection,
        reference: Reference,
    ) -> egui_kittest::Harness<'static, (ChannelSelection, Reference)> {
        type State = (ChannelSelection, Reference);
        let mut harness = egui_kittest::Harness::builder().with_size([500.0, 80.0]).wgpu().build_ui_state(
            |ui, (selection, reference): &mut State| {
                if has_selection(selection, 8) {
                    ui.horizontal(|ui| selection_ui(ui, selection, 8, None));
                }
                if !selection.apply {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(400.0, TOGGLES_HEIGHT), Sense::hover());
                    if let Some(edit) = column_toggles(ui, rect, Some(selection), Some(&*reference), 8, false) {
                        edit.apply(reference, 8);
                    }
                }
            },
            (selection, reference),
        );
        harness.run_steps(3);
        harness
    }

    fn reference_of(channels: &str) -> Reference {
        let reference = Computation { channels: channels.into(), used_by: channels.into(), ..Default::default() };
        Reference { list: vec![reference], next_id: 1 }
    }

    #[test]
    fn the_columns_top_toggles_select_all_or_none() {
        use egui_kittest::kittest::Queryable;
        let selection = ChannelSelection { apply: false, channels: "1-3".into() };
        let mut harness = toggles(selection, reference_of("1-2"));
        harness.get_by_label("Selection pending");
        harness.snapshot("lfp_viewer_column_toggles");
        // All and the R are level.
        use egui_kittest::kittest::NodeT as _;
        let middle = |node: egui_kittest::Node| {
            let area = node.accesskit_node().bounding_box().unwrap();
            (area.y0 + area.y1) / 2.0
        };
        let (all, r) = (middle(harness.get_by_label("All")), middle(harness.get_by_label("R1")));
        assert!((all - r).abs() < 0.6, "All at {all}, R at {r}");
        harness.get_by_label("All").click();
        harness.run_steps(3);
        assert_eq!(harness.state().0.channels, "all", "some selected, so it's all");
        assert!(harness.query_by_label("Selection pending").is_none(), "nothing left out");
        assert!(harness.query_by_label("Apply selection").is_none(), "so nothing to apply");
        harness.snapshot("lfp_viewer_all_selected");
        harness.get_by_label("All").click();
        harness.run_steps(3);
        assert_eq!(selected(&harness.state().0, 8), Vec::<u32>::new());
        harness.get_by_label("Nothing selected");

        harness.get_by_label("R1").click();
        harness.run_steps(3);
        assert_eq!(harness.state().1.list[0].channels, "all");
        harness.get_by_label("R1").click();
        harness.run_steps(3);
        assert_eq!(harness.state().1.list[0].used_by, "none");

        // Applied, the selection can only be disabled.
        let selection = ChannelSelection { apply: true, channels: "1-3".into() };
        let mut harness = toggles(selection, reference_of("1-2"));
        harness.get_by_label("Selection applied");
        assert!(harness.query_by_label("All").is_none());
        harness.get_by_label("Disable selection").click();
        harness.run_steps(3);
        assert!(!harness.state().0.apply);
        harness.get_by_label("Apply selection");
    }

    #[test]
    fn selecting_and_applying_a_selection() {
        let mut selection = ChannelSelection { apply: false, channels: "all".into() };
        set_selected(&mut selection, &[0, 1, 2, 5], 8);
        assert_eq!(selection.channels, "1-3, 6");
        assert_eq!(selected(&selection, 8), vec![0, 1, 2, 5]);
        selection.apply = true;
        // Now four come in: rows are channels 1, 2, 3 and 6 upstream.
        assert_eq!(upstream(&selection, 4), vec![0, 1, 2, 5]);
        set_selected(&mut selection, &[0, 5], 4);
        assert_eq!(selection.channels, "1, 6");
    }
}
