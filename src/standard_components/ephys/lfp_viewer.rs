//! The LFP Viewer: scrolling traces of continuous data, a row per channel, each at a fixed height (scrolled
//! through), and beside each, its spectrum over a band of interest (theta, 4–12 Hz, to begin with).
//!
//! Each pass adds its samples to a few seconds' worth for each channel shown, kept as the lowest and highest
//! sample in each of the view's columns (so spikes show however far it's zoomed out). For the spectra, it keeps
//! about four seconds of each channel, averaged down to about 1 kHz, and works out a few channels' spectra each
//! pass (by FFT, Hann windowed), so each is fresh every few passes, smoothed over the last few.
//!
//! Connected to a Channel Selector upstream, it chooses the channels it passes on: tick the ones to keep, while
//! all are shown, then apply the selection.

use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Shape, Stroke};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Control, CopyTo, EdgeType, Input, Node, NodeInfo, Output, Pass};
use crate::core::exchange::Latest;
use crate::standard_components::ephys::reference::{format_channels, parse_channels};
use crate::standard_components::ephys::{ChannelSelection, Continuous, ContinuousBuffer};
use crate::standard_components::preview;
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Columns across the view.
const COLUMNS: usize = 1200;
/// About how many samples a second the spectra are worked out from.
const SPECTRUM_RATE: f32 = 1000.0;
/// Samples each spectrum's worked out from: about four seconds.
const SPECTRUM_SAMPLES: usize = 4096;
/// Each channel's spectrum is worked out again every so many passes (a few channels a pass).
const SPECTRUM_PASSES: usize = 10;
/// How much of each new spectrum goes into the smoothed one.
const SMOOTHING: f32 = 0.3;
/// The spectrum's column's width.
const SPECTRUM_WIDTH: f32 = 160.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LfpViewer {
    /// Seconds across the view.
    pub span: f32,
    /// Microvolts from a trace's centre line to the edge of its row.
    pub scale_uv: f32,
    /// Each row's height, in points.
    pub row_height: f32,
    /// Whether to show each channel's spectrum, and over which band (Hz).
    pub spectrum: bool,
    pub band: [f32; 2],
}

impl Default for LfpViewer {
    fn default() -> Self {
        LfpViewer { span: 2.0, scale_uv: 200.0, row_height: 40.0, spectrum: true, band: [4.0, 12.0] }
    }
}

/// The last `span` seconds of each channel shown: each column's lowest and highest sample, round a ring; and the
/// spectra.
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
    spectra: Spectra,
    shown: Latest<TracesDisplay>,
}

impl crate::core::engine::PassState for Traces {
    const WHY: &'static str = "The last few seconds of each channel shown (each column's lowest and highest \
                               sample, round a ring, and samples for the spectra), which each pass adds its \
                               samples to; and the spectra, smoothed over passes.";
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
                let samples = &input.channel(first + i)[t..t + run];
                let low = samples.iter().copied().fold(f32::INFINITY, f32::min);
                let high = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
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

/// The channels' spectra over the band: from the last `SPECTRUM_SAMPLES` of each, averaged down from the input.
#[derive(Default)]
struct Spectra {
    /// What they're for: channels, input samples to each kept, and the band's bins.
    shape: (usize, usize, Range<usize>),
    /// For each channel, the sum of the input samples so far toward the next kept, and how many.
    sums: Vec<f32>,
    summed: usize,
    /// For each channel, `SPECTRUM_SAMPLES` kept, round a ring; where the next goes, and how many there are.
    kept: Vec<f32>,
    head: usize,
    count: usize,
    /// For each channel, the power in each of the band's bins, smoothed; and whether it's been worked out yet.
    power: Vec<f32>,
    done: Vec<bool>,
    /// The next channel to work out.
    next: usize,
    fft: Option<Arc<dyn Fft<f32>>>,
    window: Vec<f32>,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Spectra {
    /// Input samples to each kept, for a rate.
    fn factor(rate: f32) -> usize {
        (rate / SPECTRUM_RATE).round().max(1.0) as usize
    }

    /// The bins of a band, at a rate.
    fn bins(rate: f32, band: [f32; 2]) -> Range<usize> {
        let step = rate / Self::factor(rate) as f32 / SPECTRUM_SAMPLES as f32;
        let low = (band[0].max(0.0) / step).ceil() as usize;
        let high = ((band[1] / step).floor() as usize + 1).min(SPECTRUM_SAMPLES / 2);
        low.min(high)..high
    }

    fn reshape(&mut self, channels: usize, rate: f32, band: [f32; 2]) {
        let shape = (channels, Self::factor(rate), Self::bins(rate, band));
        if self.shape == shape {
            return;
        }
        let bins = shape.2.len();
        let n = SPECTRUM_SAMPLES;
        let fft = self.fft.take().unwrap_or_else(|| FftPlanner::new().plan_fft_forward(n));
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        // Keep the samples so far, if they're for the same channels.
        let (kept, head, count) = if self.shape.0 == channels && self.shape.1 == shape.1 {
            (std::mem::take(&mut self.kept), self.head, self.count)
        } else {
            (vec![0.0; channels * n], 0, 0)
        };
        *self = Spectra {
            shape,
            sums: vec![0.0; channels],
            summed: 0,
            kept,
            head,
            count,
            power: vec![0.0; channels * bins],
            done: vec![false; channels],
            next: 0,
            fft: Some(fft),
            window: (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect(),
            buffer: vec![Complex::default(); n],
            scratch,
        };
    }

    /// Keeps a pass's samples of the channels shown (from `first`).
    /// A run of samples at a time, up to where the next is kept, channel by channel (as they're laid out).
    fn add(&mut self, input: &ContinuousBuffer, first: usize) {
        let (channels, factor, _) = self.shape.clone();
        let n = SPECTRUM_SAMPLES;
        let mut t = 0;
        while t < input.len {
            let run = (factor - self.summed).min(input.len - t);
            for (c, sum) in self.sums.iter_mut().enumerate().take(channels) {
                *sum += input.channel(first + c)[t..t + run].iter().sum::<f32>();
            }
            (t, self.summed) = (t + run, self.summed + run);
            if self.summed == factor {
                for (c, sum) in self.sums.iter_mut().enumerate() {
                    self.kept[c * n + self.head] = *sum / factor as f32;
                    *sum = 0.0;
                }
                (self.head, self.summed, self.count) = ((self.head + 1) % n, 0, (self.count + 1).min(n));
            }
        }
    }

    /// Works out the next few channels' spectra, once there are enough samples.
    fn work_out(&mut self) {
        let (channels, _, bins) = self.shape.clone();
        let n = SPECTRUM_SAMPLES;
        let Some(fft) = self.fft.clone() else { return };
        if self.count < n || channels == 0 {
            return;
        }
        for _ in 0..channels.div_ceil(SPECTRUM_PASSES) {
            let c = self.next;
            self.next = (self.next + 1) % channels;
            // Oldest first, less the mean, windowed.
            let kept = &self.kept[c * n..(c + 1) * n];
            let mean = kept.iter().sum::<f32>() / n as f32;
            for (i, value) in self.buffer.iter_mut().enumerate() {
                let x = kept[(self.head + i) % n] - mean;
                *value = Complex::new(x * self.window[i], 0.0);
            }
            fft.process_with_scratch(&mut self.buffer, &mut self.scratch);
            let power = &mut self.power[c * bins.len()..(c + 1) * bins.len()];
            let first = !self.done[c];
            for (p, bin) in power.iter_mut().zip(bins.clone()) {
                let new = self.buffer[bin].norm_sqr() / n as f32;
                *p = if first { new } else { *p + SMOOTHING * (new - *p) };
            }
            self.done[c] = true;
        }
    }
}

/// The traces as of the last pass, oldest column first, and the spectra.
#[derive(Default)]
pub struct TracesDisplay {
    first: usize,
    shown: usize,
    columns: usize,
    lows: Vec<f32>,
    highs: Vec<f32>,
    /// The spectra's frequencies: the first bin's, and the step from each to the next.
    spectrum_hz: (f32, f32),
    /// For each channel shown, its power in each bin, if it's been worked out.
    power: Vec<f32>,
    bins: usize,
    done: Vec<bool>,
}

impl Node<Edges> for LfpViewer {
    type State = Traces;

    const INFO: NodeInfo = NodeInfo {
        name: "LFP Viewer",
        category: Category::Visualisation,
        description: "Scrolling traces of continuous data, one row per channel, each with its spectrum over a band \
                      (theta, say). Chooses an upstream Channel Selector's channels.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "a continuous data stream")
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Window)
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
        let spectra = &mut traces.spectra;
        if self.spectrum {
            spectra.reshape(shown, rate, self.band);
            spectra.add(input, first);
            spectra.work_out();
        }
        let step = rate / Spectra::factor(rate) as f32 / SPECTRUM_SAMPLES as f32;
        let spectrum = self.spectrum;
        let Traces { lows, highs, head, columns, spectra, shown: latest, .. } = traces;
        latest.send(pass, |display| {
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
            display.power.clear();
            display.done.clear();
            if spectrum {
                display.spectrum_hz = (spectra.shape.2.start as f32 * step, step);
                display.bins = spectra.shape.2.len();
                display.power.extend_from_slice(&spectra.power);
                display.done.extend_from_slice(&spectra.done);
            }
        });
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "gain", kind: SettingKind::Gain, read_only: false },
            Control { name: "selection", kind: SettingKind::ChannelSelection, read_only: false },
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
        let mut selection = match &context.controls[1] {
            Some(Setting::ChannelSelection(s)) => Some(s.clone()),
            _ => None,
        };
        self.toolbar(ui, context, stream, selection.as_mut());

        let Some(display) = context.display::<TracesDisplay>() else {
            let message = format!("{} · press Play to see the traces", stream.summary());
            return preview::unconnected(ui, context.theme, &message);
        };
        self.rows_ui(ui, context.theme, display, stream, selection.as_mut(), locked);
        if let Some(selection) = selection {
            context.controls[1] = Some(Setting::ChannelSelection(selection));
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

    /// The span, scale and rows' height, the spectrum, the upstream gain and the channel selection (if they're
    /// connected).
    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        context: &mut Context<Edges>,
        stream: Continuous,
        selection: Option<&mut ChannelSelection>,
    ) {
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
                ui.checkbox(&mut self.spectrum, "Spectrum")
                    .on_hover_text("Each channel's spectrum over the band, from its last four seconds");
                ui.add_enabled_ui(self.spectrum, |ui| {
                    let nyquist = stream.sample_rate / Spectra::factor(stream.sample_rate) as f32 / 2.0;
                    let [low, high] = &mut self.band;
                    ui.add(egui::DragValue::new(low).range(0.0..=nyquist).speed(0.1)).on_hover_text("From (Hz)");
                    ui.label("–");
                    ui.add(egui::DragValue::new(high).range(0.5..=nyquist).speed(0.1).suffix(" Hz"))
                        .on_hover_text("To (Hz)");
                    if *high <= *low {
                        *high = (*low + 0.5).min(nyquist);
                    }
                });
                // With the gain control connected, the upstream gain can be adjusted from here.
                if let Some(Setting::Gain(mut gain)) = context.controls[0] {
                    ui.separator();
                    let label = ui.label("Upstream gain");
                    ui.add(egui::DragValue::new(&mut gain.factor).range(0.0..=1000.0).speed(0.01).prefix("×"))
                        .labelled_by(label.id);
                    context.controls[0] = Some(Setting::Gain(gain));
                }
                if let Some(selection) = selection {
                    ui.separator();
                    selection_ui(ui, selection, stream.channels);
                }
                context.node_toolbar(ui);
            });
        });
    }

    /// A row for each channel shown, at the row height: its trace, and its spectrum, with a tick to select it if
    /// there's a selection to make.
    fn rows_ui(
        &self,
        ui: &mut egui::Ui,
        theme: &crate::core::theme::Colours,
        display: &TracesDisplay,
        stream: Continuous,
        selection: Option<&mut ChannelSelection>,
        locked: bool,
    ) {
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 2.0, theme.surface_sunken);
        let height = self.row_height.clamp(12.0, 400.0);
        // Which channel upstream each row is, once a selection's applied.
        let numbers = selection.as_deref().map(|s| upstream(s, stream.channels));
        let ticked: Option<Vec<u32>> = selection.as_deref().map(|s| ticked(s, stream.channels));
        let mut toggled = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, height, display.shown, |ui, rows| {
            let width = ui.available_width();
            let label = 44.0;
            let spectrum = if self.spectrum && width > SPECTRUM_WIDTH * 2.0 { SPECTRUM_WIDTH } else { 0.0 };
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
                    format!("{}", number + 1),
                    FontId::monospace(10.0),
                    theme.text_secondary,
                );
                if let Some(ticked) = &ticked {
                    let mut on = ticked.contains(&number);
                    let tick = Rect::from_center_size(egui::pos2(rect.left() + 32.0, text_y), egui::vec2(16.0, 16.0));
                    let response = ui.put(tick, egui::Checkbox::without_text(&mut on));
                    if response.on_hover_text(format!("Select channel {}", number + 1)).changed() && !locked {
                        toggled = Some(number);
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
                painter.add(Shape::line(points, Stroke::new(1.0, Continuous::INFO.colour)));

                if spectrum > 0.0 {
                    let place = Rect::from_min_max(egui::pos2(row.right() - spectrum, top), row.max).shrink(2.0);
                    spectrum_ui(&painter, place, display, i, self.band, theme);
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
    }
}

/// A channel's spectrum over the band, scaled to its biggest bin, with the peak's frequency.
fn spectrum_ui(
    painter: &egui::Painter,
    place: Rect,
    display: &TracesDisplay,
    i: usize,
    band: [f32; 2],
    theme: &crate::core::theme::Colours,
) {
    painter.rect_filled(place, 2.0, theme.surface_raised.gamma_multiply(0.5));
    let bins = display.bins;
    if !display.done.get(i).copied().unwrap_or(false) || bins == 0 {
        if place.height() >= 12.0 {
            painter.text(place.center(), Align2::CENTER_CENTER, "…", FontId::proportional(10.0), theme.text_disabled);
        }
        return;
    }
    let power = &display.power[i * bins..(i + 1) * bins];
    let (peak, &most) = power.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
    let (first_hz, step) = display.spectrum_hz;
    let span = (band[1] - band[0]).max(0.1);
    let x = |hz: f32| place.left() + ((hz - band[0]) / span).clamp(0.0, 1.0) * place.width();
    let y = |p: f32| place.bottom() - if most > 0.0 { p / most } else { 0.0 } * place.height();
    let colour = Continuous::INFO.colour;
    let mut points: Vec<Pos2> =
        power.iter().enumerate().map(|(b, &p)| egui::pos2(x(first_hz + b as f32 * step), y(p))).collect();
    let line = Shape::line(points.clone(), Stroke::new(1.0, colour));
    // Filled beneath (as quads: the outline needn't be convex).
    for pair in points.windows(2) {
        let quad = vec![pair[0], pair[1], egui::pos2(pair[1].x, place.bottom()), egui::pos2(pair[0].x, place.bottom())];
        painter.add(Shape::convex_polygon(quad, colour.gamma_multiply(0.25), Stroke::NONE));
    }
    painter.add(line);
    points.clear();
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
    use crate::core::engine::{DataStreamType, PASS_DURATION};

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
    fn the_spectrum_peaks_at_the_rhythm() {
        let viewer = LfpViewer::default();
        let stream = Continuous { channels: 2, sample_rate: 30000.0 };
        let mut traces = viewer.initial_state(&[Some(DataStream::Continuous(stream))]);
        let mut input = stream.buffer(PASS_DURATION);
        let per_pass = 600;
        input.len = per_pass;
        // 8 Hz on the first channel, 10 Hz on the second, for five seconds.
        for sequence in 0..250u64 {
            for (c, hz) in [(0, 8.0), (1, 10.0)] {
                for (s, x) in input.channel_mut(c).iter_mut().enumerate() {
                    let t = (sequence as usize * per_pass + s) as f32 / 30000.0;
                    *x = 100.0 * (std::f32::consts::TAU * hz * t).sin();
                }
            }
            let buffer = Buffer::Continuous(input);
            let inputs = [Some(&buffer)];
            let mut pass = Pass::new(sequence, PASS_DURATION, &inputs, &mut []);
            viewer.process(&mut pass, &mut traces);
            let Buffer::Continuous(b) = buffer else { unreachable!() };
            input = b;
        }
        let spectra = &traces.spectra;
        assert!(spectra.done.iter().all(|d| *d));
        let bins = spectra.shape.2.clone();
        let step = 30000.0 / 30.0 / SPECTRUM_SAMPLES as f32;
        for (c, hz) in [(0, 8.0), (1, 10.0)] {
            let power = &spectra.power[c * bins.len()..(c + 1) * bins.len()];
            let peak = power.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
            let peak_hz = (bins.start + peak) as f32 * step;
            assert!((peak_hz - hz).abs() < step, "channel {c}: {peak_hz} Hz");
        }
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
