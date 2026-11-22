//! The Phase Viewer shows how each channel's oscillation relates to one chosen channel, the anchor, over recent data.
//! It reads complex continuous data, usually the analytic signal from a Hilbert Transform of band-passed data, and for
//! each channel shows:
//!
//! - the **frequency**: the average rate the channel's phase turns at, weighted by its envelope;
//! - the **phase** relative to the anchor: the average difference between the channel's phase and the anchor's, so a
//!   positive phase means the channel leads the anchor;
//! - the **consistency** of that difference, from 0 (no steady relationship) to 1 (always the same difference);
//! - the **envelope**: the channel's average amplitude.
//!
//! The averages are exponential, over roughly the last few seconds (the **Average over** setting). A polar plot shows
//! every channel at once: each channel's angle is its phase relative to the anchor, and its distance from the centre is
//! its consistency.

use eframe::egui::{self, Align2, FontId, Pos2, Stroke, Vec2};
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
    /// For each channel, the decayed sum of its samples' magnitudes.
    envelope: Vec<f64>,
    /// The decayed count of samples, which the sums are averages over.
    weight: f64,
    /// The anchor's unit phasor at each of the pass's samples. This is room the passes reuse.
    anchor_units: Vec<(f32, f32)>,
}

impl PassState for Phases {
    const WHY: &'static str = "Each channel's running averages of its phase relative to the anchor, its frequency and \
                               its envelope, which each pass decays and adds to; and each channel's last sample, so \
                               the frequency counts the turn from one pass to the next.";
}

/// One channel's averages, as shown.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct ChannelPhase {
    pub frequency_hz: f32,
    /// The channel's phase minus the anchor's, in radians, from -π to π: positive means the channel leads.
    pub phase: f32,
    /// How steady the phase difference is, from 0 to 1.
    pub consistency: f32,
    pub envelope: f32,
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
        ui.horizontal_top(|ui| {
            let side = area.height().min(area.width() * 0.45).max(120.0);
            polar_ui(ui, &display, side, theme);
            ui.add_space(12.0);
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| ui.vertical(|ui| table_ui(ui, &display, theme)));
        });
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
            envelope: vec![0.0; channels],
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
            let (mut relative, mut turning, mut envelope) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32), 0.0f32);
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
                envelope += zr.hypot(zi);
                previous = (zr, zi);
            }
            self.last[channel] = previous;
            let decayed = |sum: &mut Complex, add: (f32, f32)| {
                *sum = (sum.0 * decay + add.0 as f64, sum.1 * decay + add.1 as f64);
            };
            decayed(&mut self.relative[channel], relative);
            decayed(&mut self.turning[channel], turning);
            self.envelope[channel] = self.envelope[channel] * decay + envelope as f64;
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
                    envelope: (self.envelope[channel] / weight) as f32,
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

/// A polar plot, `side` points square: each channel at the angle of its phase relative to the anchor (0° to the right,
/// leading anticlockwise), as far out as it's consistent. The anchor is always at 0°, on the edge.
fn polar_ui(ui: &mut egui::Ui, display: &PhaseDisplay, side: f32, theme: &Colours) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let centre = rect.center();
    let radius = side / 2.0 - 40.0;
    let grid = Stroke::new(1.0, theme.grid);
    for fraction in [0.5, 1.0] {
        painter.circle_stroke(centre, radius * fraction, grid);
    }
    painter.hline(centre.x - radius..=centre.x + radius, centre.y, grid);
    painter.vline(centre.x, centre.y - radius..=centre.y + radius, grid);
    let font = FontId::proportional(11.0);
    let gap = radius + 6.0;
    for (text, at, align) in [
        ("0°", Vec2::new(gap, 0.0), Align2::LEFT_CENTER),
        ("+90°", Vec2::new(0.0, -gap), Align2::CENTER_BOTTOM),
        ("±180°", Vec2::new(-gap, 0.0), Align2::RIGHT_CENTER),
        ("−90°", Vec2::new(0.0, gap), Align2::CENTER_TOP),
    ] {
        painter.text(centre + at, align, text, font.clone(), theme.text_secondary);
    }
    let colour = ComplexContinuous::INFO.colour;
    let point = |channel: &ChannelPhase| {
        let distance = radius * channel.consistency.clamp(0.0, 1.0);
        centre + Vec2::new(channel.phase.cos(), -channel.phase.sin()) * distance
    };
    // The anchor last, so it's on top.
    let order = (0..display.channels.len()).filter(|&c| c != display.anchor).chain([display.anchor]);
    for channel in order {
        let Some(phase) = display.channels.get(channel) else { continue };
        let at = point(phase);
        let anchor = channel == display.anchor;
        let fill = if anchor { theme.accent } else { colour };
        painter.circle_filled(at, if anchor { 5.0 } else { 4.0 }, fill);
        let label = Pos2::new(at.x + 6.0, at.y - 6.0);
        painter.text(label, Align2::LEFT_BOTTOM, channel + 1, FontId::monospace(10.0), theme.text_secondary);
    }
    response.on_hover_text(
        "Each channel's phase relative to the anchor (0°, highlighted): anticlockwise leads, clockwise lags. The \
         further out, the steadier the difference.",
    );
}

/// A row for each channel: its frequency, phase relative to the anchor, consistency and envelope.
fn table_ui(ui: &mut egui::Ui, display: &PhaseDisplay, theme: &Colours) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(format!("Averaged over the last {:.1} s", display.covered)).weak());
    ui.add_space(4.0);
    egui::Grid::new("phases").num_columns(5).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
        for heading in ["Ch", "Frequency", "Phase", "Consistency", "Envelope"] {
            ui.label(egui::RichText::new(heading).strong());
        }
        ui.end_row();
        for (channel, phase) in display.channels.iter().enumerate() {
            let anchor = channel == display.anchor;
            let colour = if anchor { theme.accent } else { theme.text_primary };
            ui.label(egui::RichText::new(format!("{}", channel + 1)).monospace().color(colour));
            ui.label(egui::RichText::new(format!("{:.1} Hz", phase.frequency_hz)).monospace());
            let relative = if anchor { "anchor".to_string() } else { degrees(phase.phase) };
            ui.label(egui::RichText::new(relative).monospace().color(colour));
            ui.label(egui::RichText::new(format!("{:.0}%", phase.consistency * 100.0)).monospace());
            ui.label(egui::RichText::new(format!("{:.1} µV", phase.envelope)).monospace());
            ui.end_row();
        }
    });
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
    /// relative to the first is +45°, steadily; the third's drifts, so it isn't consistent.
    #[test]
    fn phases_are_relative_to_the_anchor() {
        let channels = [(1.0, 8.0, 0.0), (2.0, 8.0, std::f32::consts::FRAC_PI_4), (1.0, 9.0, 0.0)];
        let mut phases = Phases::new(ComplexContinuous { channels: 3, sample_rate: RATE });
        run(&mut phases, &channels, 0, 10);
        let display = phases.display();
        near(display.channels[0].frequency_hz, 8.0, 0.01);
        near(display.channels[2].frequency_hz, 9.0, 0.01);
        near(display.channels[0].phase, 0.0, 1e-3);
        near(display.channels[0].consistency, 1.0, 1e-3);
        near(display.channels[1].phase.to_degrees(), 45.0, 0.1);
        near(display.channels[1].consistency, 1.0, 1e-3);
        near(display.channels[1].envelope, 2.0, 1e-3);
        assert!(display.channels[2].consistency < 0.3, "{}", display.channels[2].consistency);
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

    /// Eight channels around the anchor, some steadier than others. See tests/snapshots/.
    #[test]
    fn the_window_shows_each_channel_around_the_anchor() {
        use crate::Nodes;
        use crate::standard::themes::THEMES;
        use ephys_rs::engine::{Graph, NodesEnum};
        let mut graph = Graph::<Nodes>::default();
        let viewer = graph.add(Nodes::create(crate::NodeType::PhaseViewer), [0.0, 0.0]);
        // The anchor is channel 1, at 0° and always steady; the others spread either side of it.
        let channels = (0..8)
            .map(|c| ChannelPhase {
                frequency_hz: 7.8 + c as f32 * 0.05,
                phase: if c == 0 { 0.0 } else { (c as f32 * 30.0 - 120.0).to_radians() },
                consistency: if c == 0 { 1.0 } else { 0.95 - c as f32 * 0.1 },
                envelope: 40.0 + c as f32 * 5.0,
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
