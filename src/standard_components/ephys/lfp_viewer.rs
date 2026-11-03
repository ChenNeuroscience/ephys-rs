//! The LFP Viewer: scrolling traces of continuous data, a row per channel, each at a fixed height (scrolled
//! through), and beside each, its spectrum over a band of interest (from a Spectrum node, which only runs while the
//! spectrum's on: see `Node::needs`).
//!
//! Each pass adds its samples to a few seconds' worth for each channel shown, kept as the lowest and highest
//! sample in each of the view's columns (so spikes show however far it's zoomed out).
//!
//! It sets what's upstream of it and beside it, through its controls (as the Spike Viewer does the spikes'):
//! - an Continuous Gain's gains (what the continuous data's recorded at): each row shows the range they fit, and their
//!   auto-detect can be run from here;
//! - a Channel Selector's selection, upstream: tick the channels to keep, while all are shown, then apply it;
//! - a Reference's reference (the spikes' re-referencing, from the same channels): R on a row puts its channel in the
//!   reference, or takes it out. The traces themselves aren't referenced.

use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Control, EdgeType, Input, Node, NodeInfo, Output, Pass};
use crate::standard_components::auto_detect::auto_detect_ui;
use crate::standard_components::ephys::reference::{Method, Reference, format_channels, parse_channels};
use crate::standard_components::ephys::spectrum::{Spectra, SpectraBuffer};
use crate::standard_components::ephys::{ChannelSelection, Continuous, ContinuousBuffer};
use crate::standard_components::pool::{Pool, SharedPool};
use crate::standard_components::preview;
use crate::standard_components::spikes::{Channels, GainScope, Gains};
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Columns across the view.
const COLUMNS: usize = 1200;
/// The spectrum's column's width.
const SPECTRUM_WIDTH: f32 = 160.0;

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
    /// What it's for: the first channel, how many, and samples per column.
    shape: (usize, usize, usize),
    /// For each channel shown, `COLUMNS` lows then highs.
    lows: Vec<f32>,
    highs: Vec<f32>,
    /// The column being filled, and how many samples it has so far.
    head: usize,
    filled: usize,
    /// Columns filled since starting (up to `COLUMNS`).
    columns: usize,
    shown: SharedPool<TracesDisplay>,
}

impl crate::core::engine::PassState for Traces {
    const WHY: &'static str = "The last few seconds of each channel shown (each column's lowest and highest \
                               sample, round a ring), which each pass adds its samples to.";
}

impl Traces {
    /// Adds a pass's samples of the channels shown (from `first`) to their columns: a run of samples at a time, up to
    /// where a column's full, channel by channel (as they're laid out).
    fn add(&mut self, input: &ContinuousBuffer, first: usize) {
        let (_, shown, per_column) = self.shape;
        let mut t = 0;
        while t < input.len {
            let run = (per_column - self.filled).min(input.len - t);
            for i in 0..shown {
                let (low, high) = extremes(input.channel(first + i)[t..t + run].iter().copied());
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

    fn reshape(&mut self, shape: (usize, usize, usize)) {
        if self.shape != shape {
            let (_, shown, _) = shape;
            *self = Traces {
                rate: self.rate,
                shape,
                lows: vec![0.0; shown * COLUMNS],
                highs: vec![0.0; shown * COLUMNS],
                shown: std::mem::take(&mut self.shown),
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
    columns: usize,
    lows: Vec<f32>,
    highs: Vec<f32>,
    /// Each channel's spectrum, if it's being worked out.
    spectra: Option<SpectraBuffer>,
}

/// What the rows are drawn from: the traces, and the controls' gains and reference, if they're connected.
struct Rows<'a> {
    display: &'a TracesDisplay,
    stream: Continuous,
    spectra: Option<(&'a SpectraBuffer, Spectra)>,
    gains: Option<&'a Gains>,
    reference: Option<&'a Reference>,
}

impl Node<Edges> for LfpViewer {
    type State = Traces;

    const INFO: NodeInfo = NodeInfo {
        name: "LFP Viewer",
        category: Category::Visualisation,
        description: "Scrolling traces of continuous data, one row per channel, each with its spectrum over a band \
                      (theta, say). Sets an Continuous Gain's gains, a Channel Selector's channels and a Reference's \
                      reference channels.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }, Input { name: "spectra", kind: Kind::Spectra }]
    }

    /// The spectra only while they're shown: otherwise, the Spectrum node needn't run at all.
    fn needs(&self, input: usize) -> bool {
        input != 1 || self.spectrum
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
        let (first, shown) = (0, input.channels.max(1));
        let per_column = ((self.span as f64 * rate as f64) / COLUMNS as f64).round().max(1.0) as usize;
        traces.reshape((first, shown, per_column));
        traces.add(input, first);
        let spectra = match pass.inputs.get(1) {
            Some(Some(Buffer::Spectra(spectra))) if self.spectrum => Some(spectra),
            _ => None,
        };
        let Traces { lows, highs, head, columns, shown: sent, .. } = traces;
        sent.send_newest(pass, |display| {
            (display.first, display.shown, display.columns) = (first, shown, *columns);
            display.lows.clear();
            display.highs.clear();
            for i in 0..shown {
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
            Control { name: "gains", kind: SettingKind::Gains, read_only: false },
            Control { name: "selection", kind: SettingKind::ChannelSelection, read_only: false },
            Control { name: "reference", kind: SettingKind::Reference, read_only: false },
        ]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Span", |ui| ui.add(self.span_field()));
            setting(ui, "Scale", |ui| ui.add(self.scale_field()));
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let locked = context.locked;
        let Some(DataStream::Continuous(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect a continuous data stream to view it");
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
        self.toolbar(ui, context, stream, selection.as_mut(), reference.as_mut(), gains.as_mut());

        let pool = context.shared::<Pool<TracesDisplay>>();

        let Some(display) = pool.as_deref().and_then(Pool::newest) else {
            let message = format!("{} · press Play to see the traces", stream.summary());
            return preview::unconnected(ui, context.theme, &message);
        };
        let spectra = match (&display.spectra, context.inputs[1]) {
            (Some(buffer), Some(DataStream::Spectra(stream))) => Some((buffer, stream)),
            _ => None,
        };
        let rows = Rows { display: &display, stream, spectra, gains: gains.as_ref(), reference: reference.as_ref() };
        let toggled = self.rows_ui(ui, context.theme, rows, selection.as_mut(), locked);
        // R on a row: its channel (upstream) in the reference, or out.
        if let (Some(channel), Some(reference)) = (toggled, reference.as_mut()) {
            toggle_reference(reference, channel, stream.channels);
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
        preview::repaint_with_data(ui);
    }
}

impl LfpViewer {
    fn span_field(&mut self) -> egui::DragValue<'_> {
        egui::DragValue::new(&mut self.span).range(0.1..=30.0).speed(0.05).suffix(" s")
    }

    fn scale_field(&mut self) -> egui::DragValue<'_> {
        egui::DragValue::new(&mut self.scale_uv).range(10.0..=5000.0).speed(2.0).prefix("±").suffix(" µV")
    }

    /// The span, scale and rows' height, the spectrum, and (if they're connected) the gains, the reference and the
    /// channel selection.
    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        context: &mut Context<Edges>,
        stream: Continuous,
        selection: Option<&mut ChannelSelection>,
        reference: Option<&mut Reference>,
        gains: Option<&mut Gains>,
    ) {
        // The reference's channels can be told apart (to change) only while every channel upstream is shown.
        let all_shown = !selection.as_deref().is_some_and(|s| s.apply);
        ui.add_enabled_ui(!context.locked, |ui| {
            ui.horizontal_wrapped(|ui| {
                let label = ui.label("Span");
                ui.add(self.span_field()).labelled_by(label.id).on_hover_text("Seconds across the view");
                let label = ui.label("Scale");
                ui.add(self.scale_field())
                    .labelled_by(label.id)
                    .on_hover_text("From a trace's middle to its row's edge");
                let label = ui.label("Row height");
                ui.add(egui::DragValue::new(&mut self.row_height).range(12.0..=400.0).speed(0.5).suffix(" px"))
                    .labelled_by(label.id);
                ui.separator();
                let tip = match context.inputs[1] {
                    Some(DataStream::Spectra(s)) => format!("Show each channel's spectrum, {}", s.summary()),
                    _ => "Show each channel's spectrum: connect a Spectrum node's".into(),
                };
                if ui.add(egui::Button::selectable(self.spectrum, "Spectrum")).on_hover_text(tip).clicked() {
                    self.spectrum = !self.spectrum;
                }
                if let Some(gains) = gains {
                    ui.separator();
                    gains_ui(ui, context, gains, stream.channels);
                }
                if let Some(reference) = reference {
                    ui.separator();
                    reference_ui(ui, reference, stream.channels, all_shown);
                }
                if let Some(selection) = selection {
                    ui.separator();
                    ui.strong("Selection");
                    selection_ui(ui, selection, stream.channels);
                }
                if context.advanced() {
                    ui.separator();
                    for (control, what) in [(GAINS, "Continuous Gain"), (REFERENCE, "Reference")] {
                        let tip = format!("Open the {what}, for the rest of its settings");
                        let button =
                            ui.add_enabled(context.control_connected(control), egui::Button::new(what).small());
                        if button.on_hover_text(tip).clicked() {
                            context.open_control_target(control);
                        }
                    }
                }
                context.node_toolbar(ui);
            });
        });
    }

    /// A row for each channel shown, at the row height: its trace, the range its gain fits (if the gains are
    /// connected), and its spectrum; with a tick to select it if there's a selection to make, and R for the reference
    /// if there's one. Returns the channel (upstream) whose R was clicked, if one was: to put in the reference, or take
    /// out.
    fn rows_ui(
        &self,
        ui: &mut egui::Ui,
        theme: &crate::core::theme::Colours,
        rows_of: Rows,
        selection: Option<&mut ChannelSelection>,
        locked: bool,
    ) -> Option<u32> {
        let Rows { display, stream, spectra, gains, reference } = rows_of;
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 2.0, theme.surface_sunken);
        let height = self.row_height.clamp(12.0, 400.0);
        // Which channel upstream each row is, once a selection's applied.
        let numbers = selection.as_deref().map(|s| upstream(s, stream.channels));
        let ticked: Option<Vec<u32>> = selection.as_deref().map(|s| ticked(s, stream.channels));
        let selection_applied = selection.as_deref().map(|s| s.apply);
        // The reference's channels can be changed only while every channel upstream is shown (see `toggle_reference`).
        let editable = !locked && selection_applied != Some(true);
        let mut toggled = None;
        let mut reference_toggled = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, height, display.shown, |ui, rows| {
            let width = ui.available_width();
            let label = 64.0;
            let spectra = spectra.filter(|_| self.spectrum && width > SPECTRUM_WIDTH * 2.0);
            let spectrum = if spectra.is_some() { SPECTRUM_WIDTH } else { 0.0 };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height * rows.len() as f32), Sense::hover());
            let painter = ui.painter_at(rect);
            // Filling from the left, then scrolling, with room for the future at the right (see `time_axis`).
            let future = 1.0 + crate::core::time_axis::FUTURE as f32;
            let column = (width - label - spectrum) / (COLUMNS as f32 * future);
            // The columns filled so far are the last of each row's.
            let start = COLUMNS - display.columns;
            for (r, i) in rows.enumerate() {
                let top = rect.top() + r as f32 * height;
                let row = Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(width, height));
                if i > 0 {
                    painter.hline(row.x_range(), top, Stroke::new(1.0, theme.divider.gamma_multiply(0.3)));
                }
                let channel = display.first + i;
                let number = numbers.as_ref().and_then(|n| n.get(channel).copied()).unwrap_or(channel as u32);
                let text_y = top + (height * 0.5).min(9.0);
                painter.text(
                    egui::pos2(rect.left() + 4.0, text_y),
                    Align2::LEFT_CENTER,
                    (number + 1).to_string(),
                    FontId::monospace(10.0),
                    theme.text_secondary,
                );
                // Whether it's selected (to pass on, from a Channel Selector upstream).
                if let Some(ticked) = &ticked {
                    let mut on = ticked.contains(&number);
                    let tick = Rect::from_center_size(egui::pos2(rect.left() + 32.0, text_y), egui::vec2(16.0, 16.0));
                    let tip = format!("Select channel {}, to pass on (once the selection's applied)", number + 1);
                    if ui.put(tick, egui::Checkbox::without_text(&mut on)).on_hover_text(tip).changed() && !locked {
                        toggled = Some(number);
                    }
                }
                // Whether it's in the reference (a Reference's, beside this).
                if let Some(reference) = reference.filter(|r| matches!(r.method, Method::Average | Method::Median)) {
                    let on = in_reference(reference, number);
                    let place = Rect::from_center_size(egui::pos2(rect.left() + 52.0, text_y), egui::vec2(18.0, 16.0));
                    let tip = match (on, editable) {
                        (_, false) if !locked => format!(
                            "Channel {} is {}in the reference: show all the channels to change it",
                            number + 1,
                            if on { "" } else { "not " }
                        ),
                        (true, _) => format!("Channel {} is in the reference: click to take it out", number + 1),
                        (false, _) => format!("Put channel {} in the reference", number + 1),
                    };
                    let button =
                        egui::Button::selectable(on, egui::RichText::new("R").size(10.0)).min_size(place.size());
                    if ui.put(place, button).on_hover_text(tip).clicked() && editable {
                        reference_toggled = Some(number);
                    }
                }

                // The trace.
                let centre = top + height * 0.5;
                let y = |uv: f32| centre - (uv / self.scale_uv).clamp(-1.0, 1.0) * height * 0.5;
                let lows = &display.lows[i * COLUMNS..(i + 1) * COLUMNS];
                let highs = &display.highs[i * COLUMNS..(i + 1) * COLUMNS];
                let points: Vec<Pos2> = (start..COLUMNS)
                    .flat_map(|c| {
                        let x = rect.left() + label + ((c - start) as f32 + 0.5) * column;
                        // Down and up each column, so its whole range is drawn.
                        let (a, b) = if c % 2 == 0 { (lows[c], highs[c]) } else { (highs[c], lows[c]) };
                        [egui::pos2(x, y(a)), egui::pos2(x, y(b))]
                    })
                    .collect();
                // Not selected (with a selection to apply), dimmed: it won't be passed on.
                let unselected =
                    selection_applied == Some(false) && ticked.as_ref().is_some_and(|t| !t.contains(&number));
                let colour =
                    if unselected { Continuous::INFO.colour.gamma_multiply(0.3) } else { Continuous::INFO.colour };
                painter.add(Shape::line(points, Stroke::new(1.0, colour)));
                // The range its gain fits (what's recorded), if it's within the row: beyond it, it's clipped.
                if let Some(gains) = gains {
                    let range = gains.range_uv(i);
                    if range < self.scale_uv {
                        let x = egui::Rangef::new(rect.left() + label, row.right() - spectrum);
                        let stroke = Stroke::new(1.0, theme.warning.gamma_multiply(0.5));
                        for level in [range, -range] {
                            let dashes = Shape::dashed_line(
                                &[egui::pos2(x.min, y(level)), egui::pos2(x.max, y(level))],
                                stroke,
                                4.0,
                                4.0,
                            );
                            painter.extend(dashes);
                        }
                    }
                }

                if let Some((buffer, stream)) = spectra {
                    let place = Rect::from_min_max(egui::pos2(row.right() - spectrum, top), row.max).shrink(2.0);
                    spectrum_ui(&painter, place, buffer, stream, i, theme);
                }
            }
        });
        if let (Some(channel), Some(selection), Some(mut ticked)) = (toggled, selection, ticked) {
            match ticked.iter().position(|&c| c == channel) {
                Some(at) => {
                    ticked.remove(at);
                }
                None => ticked.push(channel),
            }
            set_ticked(selection, &ticked, stream.channels);
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
    theme: &crate::core::theme::Colours,
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
    let colour = Continuous::INFO.colour;
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

/// Whether channel `number` (upstream, from 0) is in the reference.
fn in_reference(reference: &Reference, number: u32) -> bool {
    let text = reference.reference_channels.trim();
    text.is_empty()
        || text.eq_ignore_ascii_case("all")
        || parse_channels(text, u32::MAX / 2).is_ok_and(|channels| channels.contains(&number))
}

/// Puts channel `number` (upstream, from 0) in the reference, or takes it out, of the `channels` there are (all of
/// them shown, so "all" can be told).
fn toggle_reference(reference: &mut Reference, number: u32, channels: u32) {
    let mut members: Vec<u32> = (0..channels).filter(|&c| in_reference(reference, c)).collect();
    match members.iter().position(|&c| c == number) {
        Some(at) => {
            members.remove(at);
        }
        None => members.push(number),
    }
    reference.reference_channels = format_channels(&members, channels);
}

/// The reference (a Reference's, for the spikes), compactly: the mean or median of which channels (each chosen on its
/// row, with R), and all or none of them. Other methods are set in the Reference itself.
fn reference_ui(ui: &mut egui::Ui, reference: &mut Reference, channels: u32, all_shown: bool) {
    let label = ui.strong("Reference").on_hover_text(
        "The spikes' reference, made by the Reference beside this from the same channels: R on a row puts its channel \
         in it, or takes it out. The traces here aren't referenced",
    );
    let median = match reference.method {
        Method::Average => false,
        Method::Median => true,
        _ => {
            ui.label(reference.summary()).on_hover_text("Set in the Reference");
            return;
        }
    };
    let mut chosen = median;
    egui::ComboBox::from_id_salt("reference method")
        .selected_text(if median { "median" } else { "mean" })
        .width(70.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut chosen, false, "mean").on_hover_text("A common average reference");
            ui.selectable_value(&mut chosen, true, "median")
                .on_hover_text("A common median reference: a spike on one of its channels hardly moves it");
        })
        .response
        .labelled_by(label.id);
    if chosen != median {
        reference.method = if chosen { Method::Median } else { Method::Average };
    }
    let members: Vec<u32> = (0..channels).filter(|&c| in_reference(reference, c)).collect();
    let text = match reference.reference_channels.trim() {
        "" | "all" => "of all".to_owned(),
        _ if members.is_empty() => "of none".to_owned(),
        text => format!("of {text}"),
    };
    ui.label(text);
    let why = "Show all the channels to change it";
    let all = ui.add_enabled(all_shown, egui::Button::new("All").small());
    if all.on_hover_text("Every channel").on_disabled_hover_text(why).clicked() {
        reference.reference_channels = "all".into();
    }
    let none = ui.add_enabled(all_shown, egui::Button::new("None").small());
    if none.on_hover_text("No reference").on_disabled_hover_text(why).clicked() {
        reference.reference_channels = "none".into();
    }
}

/// The gains (an Continuous Gain's: what the continuous data's recorded at, beside this), compactly: the range one for all
/// fits, or that they're each channel's (as each row shows); and detecting them from the data.
fn gains_ui(ui: &mut egui::Ui, context: &mut Context<Edges>, gains: &mut Gains, channels: u32) {
    let label = ui.strong("Gain");
    let label = label.on_hover_text(
        "The Continuous Gain's gains, beside this: what the continuous data's recorded at. Each row shows the range it fits \
         (dashed, if it's within the row): beyond it, it's clipped",
    );
    let layout = Channels { count: channels as usize, group_size: 1 };
    let detecting =
        context.trigger_progress(Some(GAINS), crate::standard_components::auto_detect::AUTO_DETECT).is_some();
    if gains.scope == GainScope::All {
        let mut range = gains.range_uv(0);
        let speed = range * 0.01;
        let field = egui::DragValue::new(&mut range).range(1.0..=100_000.0).speed(speed).prefix("fits ±").suffix(" µV");
        let field = ui.add_enabled(!detecting, field).labelled_by(label.id);
        if field.on_hover_text("The range one gain for all channels fits").changed() {
            gains.set_range(0, range, layout);
        }
    } else {
        ui.label("each channel's");
    }
    let tip = format!(
        "Sample the data, then set the gains so its bigger excursions reach ±{} (as the Continuous Gain says)",
        gains.output_range
    );
    auto_detect_ui(ui, context, Some(GAINS), &tip);
}

/// Whether a selection's channels are "all".
fn is_all(selection: &ChannelSelection) -> bool {
    let text = selection.channels.trim();
    text.is_empty() || text.eq_ignore_ascii_case("all")
}

/// The channels a selection has ticked (upstream), given `channels` coming in.
fn ticked(selection: &ChannelSelection, channels: u32) -> Vec<u32> {
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
    let ticked = ticked(selection, channels);
    if selection.apply && ticked.len() == channels as usize { ticked } else { (0..channels).collect() }
}

/// Ticks `ticked` (upstream) channels.
fn set_ticked(selection: &mut ChannelSelection, ticked: &[u32], channels: u32) {
    // Applied, those coming in aren't all there are upstream, so "all" can't be told.
    let count = if selection.apply { u32::MAX } else { channels };
    selection.channels = format_channels(ticked, count);
}

/// The selection: how many are ticked, ticking all or none, and applying it.
fn selection_ui(ui: &mut egui::Ui, selection: &mut ChannelSelection, channels: u32) {
    let ticked = ticked(selection, channels);
    let label = if selection.apply {
        format!("Showing {} selected", ticked.len())
    } else {
        format!("{} of {channels} ticked", ticked.len())
    };
    ui.label(label);
    if !selection.apply {
        if ui.small_button("All").clicked() {
            selection.channels = "all".into();
        }
        if ui.small_button("None").clicked() {
            selection.channels = format_channels(&[], channels);
        }
    }
    let tip = "Pass on only the ticked channels, here and to everything after the Channel Selector";
    let can = !ticked.is_empty();
    let mut apply = selection.apply;
    let text = if apply { "Show all" } else { "Apply selection" };
    if ui.add_enabled(can || apply, egui::Button::new(text).small()).on_hover_text(tip).clicked() {
        apply = !apply;
    }
    if apply != selection.apply {
        // The same channels, written for what's coming in after the change.
        selection.apply = apply;
        if !apply && !is_all(selection) {
            selection.channels = format_channels(&ticked, u32::MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_hold_each_runs_lowest_and_highest() {
        // Passes of uneven lengths, so columns fill across them.
        let (channels, per_column) = (3, 7);
        let mut traces = Traces::default();
        traces.reshape((0, channels, per_column));
        let value = |c: usize, t: usize| ((t * 37 + c * 11) % 23) as f32 - (c * t % 5) as f32;
        let mut t0 = 0;
        for len in [5, 9, 1, 20, 3, 13] {
            let mut data = vec![0.0; channels * 32];
            for c in 0..channels {
                for t in 0..len {
                    data[c * 32 + t] = value(c, t0 + t);
                }
            }
            traces.add(&ContinuousBuffer { channels, capacity: 32, len, data }, 0);
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

    #[test]
    fn rows_put_their_channels_in_the_reference_or_take_them_out() {
        let mut reference = Reference { reference_channels: "all".into(), ..Default::default() };
        assert!(in_reference(&reference, 7));
        toggle_reference(&mut reference, 2, 4);
        assert_eq!(reference.reference_channels, "1-2, 4");
        assert!(!in_reference(&reference, 2) && in_reference(&reference, 3));
        toggle_reference(&mut reference, 2, 4);
        assert_eq!(reference.reference_channels, "all");
        reference.reference_channels = "none".into();
        assert!(!in_reference(&reference, 0));
        toggle_reference(&mut reference, 0, 4);
        assert_eq!(reference.reference_channels, "1");
    }

    #[test]
    fn ticking_and_applying_a_selection() {
        let mut selection = ChannelSelection { apply: false, channels: "all".into() };
        set_ticked(&mut selection, &[0, 1, 2, 5], 8);
        assert_eq!(selection.channels, "1-3, 6");
        assert_eq!(ticked(&selection, 8), vec![0, 1, 2, 5]);
        selection.apply = true;
        // Now four come in: rows are channels 1, 2, 3 and 6 upstream.
        assert_eq!(upstream(&selection, 4), vec![0, 1, 2, 5]);
        set_ticked(&mut selection, &[0, 5], 4);
        assert_eq!(selection.channels, "1, 6");
    }
}
