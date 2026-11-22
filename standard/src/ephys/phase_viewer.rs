//! The Phase Viewer shows how each channel's oscillation relates to one chosen channel, the anchor, over recent data.
//! It reads complex continuous data, usually the analytic signal from a Hilbert Transform of band-passed data. The
//! window has a cell for each channel, in a grid with as many columns and rows as fit the pane best. Each cell shows:
//!
//! - the **phase** relative to the anchor, as a marker on a ring: the average difference between the channel's phase
//!   and the anchor's, with 0° to the right and leading anticlockwise, so the anchor's own marker is always at 0°;
//!   The marker fades when the difference isn't steady (its **consistency** is low): a channel at another frequency
//!   than the anchor's has a phase difference that keeps turning, so its average angle means nothing.
//! - the **frequency**, as a bar: the average rate the channel's phase turns at, weighted by its envelope. Every
//!   cell's bar has the same scale, which spans the channels' frequencies.
//!
//! The averages are exponential, over roughly the last few seconds (the **Average over** setting).

use eframe::egui::{self, Align2, FontId, Stroke, Vec2};
use serde::{Deserialize, Serialize};

use crate::standard::ephys::ComplexContinuous;
use crate::standard::plots;
use crate::standard::pool::Pool;
use crate::standard::widgets::require_input;
use ephys_rs::engine::{
    Buffers, Category, CommandContext, CommandSource, EdgeType, NoPorts, Node, NodeInfo, PassContext, PassState,
    Prepare, PrepareContext, Prepared, UiContext,
};
use ephys_rs::theme::Colours;

ephys_rs::ports! {
    analytic: DataStreamIn<ComplexContinuous>,
}

/// The longest the averages can cover, in seconds.
const LONGEST: f32 = 60.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhaseViewer {
    /// The channel the other channels' phases are relative to, counting from 0.
    pub anchor: u32,
    /// Roughly how many seconds of recent data the averages cover (the exponential averages' time constant).
    pub seconds: f32,
}

impl Default for PhaseViewer {
    fn default() -> Self {
        PhaseViewer { anchor: 0, seconds: 2.0 }
    }
}

/// A complex number, as (real, imaginary).
type Complex = (f64, f64);

/// The running averages, which each pass decays and then adds to.
#[derive(Default)]
pub struct Phases {
    stream: Option<ComplexContinuous>,
    /// The anchor that `relative` is relative to. The sums start again when the anchor changes.
    anchor: usize,
    /// Each channel's last sample, so the phase's turn from one pass to the next counts towards the frequency.
    last: Vec<(f32, f32)>,
    /// Whether `last` holds a sample yet.
    started: bool,
    /// For each channel, the decayed sum of its unit phasor times the conjugate of the anchor's: the sum's argument is
    /// the average phase difference, and its magnitude over `weight` is the consistency.
    relative: Vec<Complex>,
    /// For each channel, the decayed sum of each sample times the conjugate of the sample before: the sum's argument is
    /// the average turn per sample, weighted by the envelope.
    turning: Vec<Complex>,
    /// The decayed count of samples, which the sums are averages over.
    weight: f64,
    /// The anchor's unit phasor at each of the pass's samples. This is room the passes reuse.
    anchor_units: Vec<(f32, f32)>,
}

impl PassState for Phases {
    const WHY: &'static str = "Each channel's running averages of its phase relative to the anchor and its frequency, \
                               which each pass decays and adds to; and each channel's last sample, so \
                               the frequency counts the turn from one pass to the next.";
}

/// One channel's averages, as shown.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct ChannelPhase {
    pub frequency_hz: f32,
    /// The channel's phase minus the anchor's, in radians, from -π to π: positive means the channel leads.
    pub phase: f32,
    /// How steady the phase difference is, from 0 (it keeps turning, or there's no relationship) to 1 (always the
    /// same): the magnitude of the average of the difference's unit phasors.
    pub consistency: f32,
}

/// What the viewer shows: each channel's averages, which channel they're relative to, and how much data they cover.
#[derive(Clone, Default)]
pub struct PhaseDisplay {
    pub channels: Vec<ChannelPhase>,
    pub anchor: usize,
    /// Seconds of data the averages cover so far, up to about the **Average over** setting.
    pub covered: f32,
}

impl<E: PortEdges> Node<E> for PhaseViewer {
    type Ports = Ports;
    type State = Phases;
    type WorkerState = ();
    /// What it shows, for its window.
    type Shared = Pool<PhaseDisplay>;
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Phase Viewer",
        category: Category::Visualisation,
        description: "Each channel's frequency, and its phase relative to an anchor channel, over recent data, from \
                      complex continuous data (a Hilbert Transform's).",
    };
    const WINDOW_SIZE: [f32; 2] = [760.0, 480.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn val_out(&self) -> NoPorts {
        NoPorts
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {}
    }

    fn output_streams(&self, _inputs: &DataStreamIn) -> NoPorts {
        NoPorts
    }

    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Phases, (), Self> {
        if context.same_streams() {
            return Prepared::nothing().into();
        }
        let Some(stream) = context.data_stream_in::<DataStreamIn>().analytic else {
            return Prepared::shared(Phases::default()).into();
        };
        Prepared::shared(Phases::new(stream)).into()
    }

    fn process(
        &self,
        pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        _: NoPorts,
        phases: &mut Phases,
        _: &mut (),
    ) {
        let Some(input) = inputs.analytic else { return };
        if phases.stream.is_none() || input.channels == 0 {
            return;
        }
        phases.add(input, self.anchor_of(input.channels), self.seconds);
        let display = phases.display();
        pass.shared::<Pool<PhaseDisplay>>().send_newest(pass.sequence, |shown| {
            shown.channels.clone_from(&display.channels);
            (shown.anchor, shown.covered) = (display.anchor, display.covered);
        });
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        let stream = context.data_stream_in::<DataStreamIn>().analytic;
        let channels = stream.map_or(1, |s| s.channels.max(1));
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!context.locked, |ui| {
                let label = ui.label("Anchor");
                let mut shown = self.anchor.min(channels - 1) + 1;
                let field = egui::DragValue::new(&mut shown).range(1..=channels).prefix("ch ");
                let tip = "The channel the others' phases are relative to";
                if ui.add(field).labelled_by(label.id).on_hover_text(tip).changed() {
                    self.anchor = shown - 1;
                }
                let label = ui.label("Average over");
                let field = egui::DragValue::new(&mut self.seconds).range(0.2..=LONGEST).speed(0.05).suffix(" s");
                ui.add(field).labelled_by(label.id).on_hover_text("Roughly how much recent data the averages cover");
            });
            context.node_toolbar(ui);
        });
        let Some(stream) = stream else {
            plots::unconnected(ui, context.theme, "Connect complex continuous data (a Hilbert Transform's) to view it");
            return;
        };
        let pool = context.shared::<Pool<PhaseDisplay>>();
        let Some(display) = pool.and_then(Pool::newest).filter(|d| !d.channels.is_empty()) else {
            let message = format!("{} · press Play to see their phases", stream.summary());
            plots::unconnected(ui, context.theme, &message);
            return;
        };
        let theme = context.theme;
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 2.0, theme.surface_sunken);
        grid_ui(ui, &display, theme);
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        let mut warnings = require_input(inputs.any_connected(), "complex continuous data");
        if let Some(stream) = inputs.analytic
            && self.anchor >= stream.channels
            && stream.channels > 0
        {
            warnings.push(format!("There's no channel {}: the anchor is the last channel", self.anchor + 1));
        }
        warnings
    }
}

impl PhaseViewer {
    /// The anchor channel, among `channels`: the setting, or the last channel if there's no such channel.
    fn anchor_of(&self, channels: usize) -> usize {
        (self.anchor as usize).min(channels.saturating_sub(1))
    }
}

/// `z`'s unit phasor, or zero if `z` is zero.
fn unit(re: f32, im: f32) -> (f32, f32) {
    let magnitude = re.hypot(im);
    if magnitude > f32::MIN_POSITIVE { (re / magnitude, im / magnitude) } else { (0.0, 0.0) }
}

impl Phases {
    fn new(stream: ComplexContinuous) -> Self {
        let channels = stream.channels as usize;
        let pass = (stream.sample_rate as f64 * ephys_rs::engine::PASS_DURATION * 1.5).ceil() as usize;
        Phases {
            stream: Some(stream),
            last: vec![(0.0, 0.0); channels],
            relative: vec![(0.0, 0.0); channels],
            turning: vec![(0.0, 0.0); channels],
            anchor_units: Vec::with_capacity(pass),
            ..Default::default()
        }
    }

    /// Decays the averages by a pass's worth of time, for averages over `seconds`, then adds the pass's samples, with
    /// phases relative to channel `anchor`.
    fn add(&mut self, input: &crate::standard::ephys::ComplexContinuousBuffer, anchor: usize, seconds: f32) {
        let Some(stream) = self.stream else { return };
        let len = input.len;
        if len == 0 {
            return;
        }
        if anchor != self.anchor {
            self.relative.fill((0.0, 0.0));
            self.anchor = anchor;
        }
        let rate = stream.sample_rate.max(1.0) as f64;
        let decay = (-(len as f64) / (rate * seconds.clamp(0.2, LONGEST) as f64)).exp();
        self.weight = self.weight * decay + len as f64;

        let (anchor_re, anchor_im) = input.channel(anchor);
        self.anchor_units.clear();
        self.anchor_units.extend(anchor_re.iter().zip(anchor_im).map(|(&re, &im)| unit(re, im)));

        let channels = input.channels.min(self.relative.len());
        for channel in 0..channels {
            let (re, im) = input.channel(channel);
            let (mut relative, mut turning) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
            let mut previous = if self.started { self.last[channel] } else { (re[0], im[0]) };
            for ((&zr, &zi), &(ar, ai)) in re.iter().zip(im).zip(&self.anchor_units) {
                let (ur, ui) = unit(zr, zi);
                // The unit phasor times the anchor's conjugate.
                relative.0 += ur * ar + ui * ai;
                relative.1 += ui * ar - ur * ai;
                // The sample times the previous sample's conjugate.
                let (pr, pi) = previous;
                turning.0 += zr * pr + zi * pi;
                turning.1 += zi * pr - zr * pi;
                previous = (zr, zi);
            }
            self.last[channel] = previous;
            let decayed = |sum: &mut Complex, add: (f32, f32)| {
                *sum = (sum.0 * decay + add.0 as f64, sum.1 * decay + add.1 as f64);
            };
            decayed(&mut self.relative[channel], relative);
            decayed(&mut self.turning[channel], turning);
        }
        self.started = true;
    }

    /// The averages, as shown.
    fn display(&self) -> PhaseDisplay {
        let rate = self.stream.map_or(1.0, |s| s.sample_rate.max(1.0)) as f64;
        let weight = self.weight.max(f64::MIN_POSITIVE);
        let channels = (0..self.relative.len())
            .map(|channel| {
                let (rr, ri) = self.relative[channel];
                let (tr, ti) = self.turning[channel];
                ChannelPhase {
                    frequency_hz: (ti.atan2(tr) * rate / std::f64::consts::TAU) as f32,
                    phase: ri.atan2(rr) as f32,
                    consistency: (rr.hypot(ri) / weight).min(1.0) as f32,
                }
            })
            .collect();
        PhaseDisplay { channels, anchor: self.anchor, covered: (self.weight / rate) as f32 }
    }
}

/// The phase as people read it: in degrees, signed, from -180° to 180°.
fn degrees(phase: f32) -> String {
    format!("{:+.0}°", phase.to_degrees())
}

/// Below this consistency, a channel's phase marker starts to fade; at `UNSTEADY` and below, it's faint.
const STEADY: f32 = 0.7;
const UNSTEADY: f32 = 0.3;

/// How opaque a phase marker is, for a channel whose phase difference has `consistency`: fully at `STEADY` and above,
/// fading to faint at `UNSTEADY`.
fn marker_opacity(consistency: f32) -> f32 {
    let faint = 0.12;
    faint + (1.0 - faint) * ((consistency - UNSTEADY) / (STEADY - UNSTEADY)).clamp(0.0, 1.0)
}

/// The smallest a cell can be, in points: with more channels than fit at this size, the grid scrolls.
const SMALLEST_CELL: f32 = 90.0;
/// The widest a cell's frequency bar is, as a fraction of the cell.
const BAR_WIDTH: f32 = 0.12;

/// How many columns to lay `cells` out in, in an area `width` by `height`: whichever makes the cells biggest, if
/// they're all to fit. With cells of at least `SMALLEST_CELL`, more columns than fit across never help.
fn columns(cells: usize, width: f32, height: f32) -> usize {
    let cells = cells.max(1);
    let most = ((width / SMALLEST_CELL).floor() as usize).clamp(1, cells);
    let side = |columns: usize| (width / columns as f32).min(height / cells.div_ceil(columns) as f32);
    (1..=most).max_by(|&a, &b| side(a).total_cmp(&side(b))).unwrap_or(1)
}

/// The frequencies the bars span: the channels' lowest to highest, padded, at least 2 Hz wide, in whole hertz.
fn frequency_range(display: &PhaseDisplay) -> (f32, f32) {
    let frequencies = display.channels.iter().map(|c| c.frequency_hz).filter(|f| f.is_finite());
    let (low, high) = frequencies.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), f| (lo.min(f), hi.max(f)));
    if low > high {
        return (0.0, 1.0);
    }
    let (mut low, mut high) = ((low - 0.5).floor().max(0.0), (high + 0.5).ceil());
    if high - low < 2.0 {
        let middle = (low + high) / 2.0;
        (low, high) = ((middle - 1.0).max(0.0), (middle - 1.0).max(0.0) + 2.0);
    }
    (low, high)
}

/// A cell for each channel, in as many columns as make the cells biggest (see `columns`), scrolling if there are too
/// many to fit at `SMALLEST_CELL`. Above them, how much data the averages cover and the frequency bars' range.
fn grid_ui(ui: &mut egui::Ui, display: &PhaseDisplay, theme: &Colours) {
    let (low_hz, high_hz) = frequency_range(display);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let note = format!(
            "Averaged over the last {:.1} s · frequency bars from {low_hz:.0} to {high_hz:.0} Hz",
            display.covered
        );
        ui.label(egui::RichText::new(note).weak());
    });
    let area = ui.available_rect_before_wrap();
    let cells = display.channels.len();
    let columns = columns(cells, area.width(), area.height());
    let rows = cells.div_ceil(columns);
    let width = area.width() / columns as f32;
    let side = width.min(area.height() / rows as f32).max(SMALLEST_CELL.min(width));
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(area.width(), side * rows as f32), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        for (channel, phase) in display.channels.iter().enumerate() {
            let (row, column) = (channel / columns, channel % columns);
            let min = rect.min + Vec2::new(column as f32 * width, row as f32 * side);
            let cell = egui::Rect::from_min_size(min, Vec2::new(width, side)).shrink(4.0);
            let anchor = channel == display.anchor;
            cell_ui(&painter, cell, channel, phase, anchor, (low_hz, high_hz), theme);
        }
        response.on_hover_text(
            "Each channel's phase relative to the anchor, as a marker on its ring (0° to the right, leading \
             anticlockwise), and its frequency, as a bar",
        );
    });
}

/// One channel's cell, in `cell`: its number, a ring with a marker at its phase relative to the anchor, and beside
/// the ring a bar for its frequency, on the scale `range` (in Hz) every cell shares.
fn cell_ui(
    painter: &egui::Painter,
    cell: egui::Rect,
    channel: usize,
    phase: &ChannelPhase,
    anchor: bool,
    range: (f32, f32),
    theme: &Colours,
) {
    painter.rect_filled(cell, 4.0, theme.surface);
    let accent = if anchor { theme.accent } else { ComplexContinuous::INFO.colour };
    let opacity = marker_opacity(phase.consistency);
    let number = if anchor { format!("{} · anchor", channel + 1) } else { format!("{}", channel + 1) };
    let text = if anchor { theme.accent } else { theme.text_secondary };
    painter.text(cell.left_top() + Vec2::new(6.0, 4.0), Align2::LEFT_TOP, number, FontId::proportional(11.0), text);

    // The ring, left of the bar, below the number and above the readings.
    let bar_width = (cell.width() * BAR_WIDTH).max(6.0);
    let body = egui::Rect::from_min_max(cell.min + Vec2::new(6.0, 18.0), cell.max - Vec2::new(6.0, 18.0));
    let ring_area = egui::Rect::from_min_max(body.min, egui::pos2(body.right() - bar_width - 8.0, body.bottom()));
    let radius = (ring_area.width().min(ring_area.height()) / 2.0 - 4.0).max(4.0);
    let centre = ring_area.center();
    painter.circle_stroke(centre, radius, Stroke::new(1.5, theme.text_disabled));
    // 0°, where the anchor is.
    painter.line_segment(
        [centre, centre + Vec2::new(radius, 0.0)],
        Stroke::new(1.0, theme.text_disabled.gamma_multiply(0.5)),
    );
    let at = centre + Vec2::new(phase.phase.cos(), -phase.phase.sin()) * radius;
    painter.line_segment([centre, at], Stroke::new(1.5, accent.gamma_multiply(0.6 * opacity)));
    painter.circle_filled(at, (radius * 0.12).clamp(3.0, 7.0), accent.gamma_multiply(opacity));

    // The bar, from the bottom up.
    let (low, high) = range;
    let fraction = ((phase.frequency_hz - low) / (high - low)).clamp(0.0, 1.0);
    let track = egui::Rect::from_min_max(egui::pos2(body.right() - bar_width, body.top()), body.max);
    painter.rect_filled(track, 2.0, theme.surface_sunken);
    let filled =
        egui::Rect::from_min_max(egui::pos2(track.left(), track.bottom() - fraction * track.height()), track.max);
    painter.rect_filled(filled, 2.0, ComplexContinuous::INFO.colour.gamma_multiply(0.8));

    // The angle, faded with the marker, then the frequency.
    let font = FontId::monospace(11.0);
    let frequency = format!("{:.1} Hz", phase.frequency_hz);
    let mut job = egui::text::LayoutJob::default();
    if !anchor {
        let colour = theme.text_primary.gamma_multiply(opacity.max(0.35));
        job.append(&format!("{} · ", degrees(phase.phase)), 0.0, egui::TextFormat::simple(font.clone(), colour));
    }
    job.append(&frequency, 0.0, egui::TextFormat::simple(font, theme.text_primary));
    let galley = painter.layout_job(job);
    let at = egui::pos2(cell.center().x - galley.size().x / 2.0, cell.bottom() - 4.0 - galley.size().y);
    painter.galley(at, galley, theme.text_primary);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edges;
    use crate::standard::ephys::ComplexContinuousBuffer;
    use ephys_rs::engine::{DataStreamInPorts, DataStreamType};

    const RATE: f32 = 1000.0;

    /// A pass of `len` samples from `start`, on channels each `(amplitude, hz, phase)`.
    fn pass(channels: &[(f32, f32, f32)], start: usize, len: usize) -> ComplexContinuousBuffer {
        let stream = ComplexContinuous { channels: channels.len() as u32, sample_rate: RATE };
        let mut buffer = stream.buffer(0.1);
        buffer.len = len;
        for (c, &(amplitude, hz, phase)) in channels.iter().enumerate() {
            let (re, im) = buffer.channel_mut(c);
            for i in 0..len {
                let angle = std::f32::consts::TAU * hz * (start + i) as f32 / RATE + phase;
                (re[i], im[i]) = (amplitude * angle.cos(), amplitude * angle.sin());
            }
        }
        buffer
    }

    fn run(phases: &mut Phases, channels: &[(f32, f32, f32)], anchor: usize, seconds: usize) {
        for p in 0..seconds * 50 {
            phases.add(&pass(channels, p * 20, 20), anchor, 2.0);
        }
    }

    fn near(value: f32, expected: f32, within: f32) {
        assert!((value - expected).abs() <= within, "{value}, not {expected}");
    }

    /// Two channels at 8 Hz, the second leading by 45° at twice the amplitude, and a third at 9 Hz: the second's phase
    /// relative to the first is +45°, steadily; the third's keeps turning, so it isn't consistent, and its marker fades.
    #[test]
    fn phases_are_relative_to_the_anchor() {
        let channels = [(1.0, 8.0, 0.0), (2.0, 8.0, std::f32::consts::FRAC_PI_4), (1.0, 9.0, 0.0)];
        let mut phases = Phases::new(ComplexContinuous { channels: 3, sample_rate: RATE });
        run(&mut phases, &channels, 0, 10);
        let display = phases.display();
        near(display.channels[0].frequency_hz, 8.0, 0.01);
        near(display.channels[2].frequency_hz, 9.0, 0.01);
        near(display.channels[0].phase, 0.0, 1e-3);
        near(display.channels[1].phase.to_degrees(), 45.0, 0.1);
        near(display.channels[1].consistency, 1.0, 1e-3);
        assert!(display.channels[2].consistency < 0.3, "{}", display.channels[2].consistency);
        assert!(marker_opacity(display.channels[2].consistency) < 0.2);
        assert_eq!(marker_opacity(display.channels[1].consistency), 1.0);
        near(display.covered, 2.0, 0.01);

        // Relative to the second channel, the first lags by 45°.
        run(&mut phases, &channels, 1, 10);
        let display = phases.display();
        assert_eq!(display.anchor, 1);
        near(display.channels[0].phase.to_degrees(), -45.0, 0.1);
    }

    /// The anchor setting is kept to the channels there are.
    #[test]
    fn an_anchor_past_the_last_channel_is_the_last() {
        let viewer = PhaseViewer { anchor: 9, ..Default::default() };
        assert_eq!(viewer.anchor_of(4), 3);
        let inputs = DataStreamIn { analytic: Some(ComplexContinuous { channels: 4, sample_rate: RATE }) };
        assert_eq!(Node::<Edges>::warnings(&viewer, &inputs).len(), 1);
    }

    /// The grid's columns make the cells as big as they can be while all fit, and the grid scrolls once cells would be
    /// smaller than `SMALLEST_CELL`.
    #[test]
    fn the_grid_fits_the_pane() {
        assert_eq!(columns(16, 800.0, 400.0), 6, "three rows of six: cells 133 square");
        assert_eq!(columns(16, 400.0, 800.0), 3, "six rows of three: cells 133 square");
        assert_eq!(columns(4, 800.0, 200.0), 4);
        assert_eq!(columns(64, 400.0, 300.0), 4, "as many as fit across, then scrolling");
        assert_eq!(columns(1, 300.0, 300.0), 1);
    }

    #[test]
    fn the_bars_span_the_frequencies() {
        let at = |hz: &[f32]| PhaseDisplay {
            channels: hz
                .iter()
                .map(|&frequency_hz| ChannelPhase { frequency_hz, phase: 0.0, consistency: 1.0 })
                .collect(),
            ..Default::default()
        };
        assert_eq!(frequency_range(&at(&[7.6, 8.3])), (7.0, 9.0));
        assert_eq!(frequency_range(&at(&[8.0, 8.0])), (7.0, 9.0), "at least 2 Hz");
        assert_eq!(frequency_range(&at(&[4.2, 11.9])), (3.0, 13.0));
        assert_eq!(frequency_range(&at(&[])), (0.0, 1.0));
    }

    /// Sixteen channels, in a grid, each with its phase relative to the anchor (faded for two at other frequencies)
    /// and its frequency. See
    /// tests/snapshots/.
    #[test]
    fn the_window_shows_each_channel_around_the_anchor() {
        use crate::Nodes;
        use crate::standard::themes::THEMES;
        use ephys_rs::engine::{Graph, NodesEnum};
        let mut graph = Graph::<Nodes>::default();
        let viewer = graph.add(Nodes::create(crate::NodeType::PhaseViewer), [0.0, 0.0]);
        // The anchor is channel 1, at 0°. The others share its frequency and spread either side of it, steadily,
        // apart from channels 6 and 12, at other frequencies, whose phase differences keep turning.
        let channels = (0..16)
            .map(|c| {
                let off = c == 5 || c == 11;
                ChannelPhase {
                    frequency_hz: match c {
                        5 => 7.4,
                        11 => 8.7,
                        _ => 8.0 + (c % 3) as f32 * 0.02,
                    },
                    phase: if c == 0 { 0.0 } else { (c as f32 * 22.5 - 170.0).to_radians() },
                    consistency: if off { 0.1 } else { 0.95 - (c % 4) as f32 * 0.05 },
                }
            })
            .collect();
        let display = PhaseDisplay { channels, anchor: 0, covered: 2.0 };
        let streams = DataStreamInPorts::<Edges>::streams(&DataStreamIn {
            analytic: Some(ComplexContinuous { channels: 8, sample_rate: RATE }),
        });
        let shared = ephys_rs::engine::SharedByNode::default().with(viewer, Pool::holding(display));
        let mut harness = egui_kittest::Harness::builder().with_size([760.0, 380.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    viewer,
                    &streams,
                    &THEMES[0].colours,
                    Some(&shared),
                    None,
                    None,
                    true,
                    false,
                    None,
                    |node, context| node.on_gui_tick(Some(ui), context),
                );
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("phase_viewer");
    }
}
