//! The Spike Viewer: each group's (tetrode's, say) recent spikes overlaid, a plot per channel of the group, each
//! with the channel's threshold. Connected to a spike detector's thresholds and gains, it sets them: drag a
//! threshold's line, use a channel's gain buttons (shown over it), or type either (G or T over the channel). Or
//! choose them from the data: the passes sample it for a while (a few seconds), then the thresholds are so many
//! standard deviations of each channel's noise, and the gains have each channel's bigger spikes fill its plot.
//!
//! A plot shows a channel's ±(full scale / gain): just what the LTX format's 8-bit samples hold, so a spike that
//! reaches a plot's edge is clipped in the recording. Spikes go up the plot, whichever way they go (a negative-going
//! spike is shown upside down), so the threshold is always above the middle (and below too, if they go both ways).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, FontId, Key, Pos2, Rect, Sense, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, Control, CopyTo, EdgeType, Input, Node, NodeInfo, Output, Pass};
use crate::core::exchange::Latest;
use crate::standard_components::preview;
use crate::standard_components::spikes::{
    DEFAULT_GAIN, DEFAULT_THRESHOLD_UV, FULL_SCALE_UV, GAINS, Gains, Polarity, Spikes, Thresholds, gain_step, range_uv,
};
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// A channel's plot's width, for its height.
const ASPECT: f32 = 0.75;
/// The most spikes sampled on each channel, to choose its gain from.
const SAMPLED_SPIKES: usize = 4000;
/// How long the passes can sample for, in seconds.
const SAMPLING: std::ops::RangeInclusive<f32> = 4.0..=60.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeViewer {
    pub first_group: u32,
    pub groups_shown: u32,
    /// Most recent waveforms overlaid on each group.
    pub waveforms: u32,
    /// Each channel's plot's height, in points (its width follows).
    pub plot_height: f32,
    /// Seconds to sample the data for, to choose gains or thresholds from.
    pub sampling: f32,
    /// Thresholds chosen from the data are this many standard deviations of the noise.
    pub threshold_sd: f32,
    /// What to choose from the data.
    pub choosing: Choosing,
}

/// What to choose from the data.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub enum Choosing {
    #[default]
    Both,
    Gains,
    Thresholds,
}

impl Choosing {
    fn name(self) -> &'static str {
        match self {
            Choosing::Both => "Gains and thresholds",
            Choosing::Gains => "Gains",
            Choosing::Thresholds => "Thresholds",
        }
    }
}

impl Default for SpikeViewer {
    fn default() -> Self {
        SpikeViewer {
            first_group: 0,
            groups_shown: 16,
            waveforms: 30,
            plot_height: 120.0,
            sampling: 10.0,
            threshold_sd: 4.0,
            choosing: Choosing::Both,
        }
    }
}

/// Each group's most recent spikes, round a ring, and each channel's latest threshold, gain and noise.
#[derive(Default)]
pub struct Recent {
    stream: Option<Spikes>,
    keep: usize,
    /// For each group, `keep` waveforms (each `group_size × samples`).
    waveforms: Vec<f32>,
    /// For each group: its next slot, and how many it has (up to `keep`).
    head: Vec<usize>,
    count: Vec<usize>,
    /// Spikes found, for each group.
    total: Vec<u64>,
    thresholds: Vec<f32>,
    gains: Vec<f32>,
    /// Sampling the data to choose gains or thresholds from, when the window's asked to.
    sampling: Option<Sampling>,
    mailbox: Arc<Mutex<Mailbox>>,
    shown: Latest<RecentDisplay>,
}

impl crate::core::engine::PassState for Recent {
    const WHY: &'static str = "Each group's most recent spikes (round a ring), and how many it's had, which \
                               each pass adds to; and, when choosing gains or thresholds, what's been sampled so far.";
}

/// Between the window and the passes. (The window gets the latest the passes send it, but has no way to ask
/// anything of them: so it asks here, which it's sent.) The passes only look if it's free, so they never wait.
#[derive(Default)]
pub struct Mailbox {
    /// What the window's asked for.
    request: Option<Request>,
    /// What the passes chose, for the window to take.
    chosen: Option<Chosen>,
}

/// To sample the data for so long, then choose gains, or thresholds, or both.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Request {
    id: u64,
    seconds: f32,
    gains: bool,
    thresholds: bool,
}

/// What's been sampled so far, in answer to a request.
struct Sampling {
    request: Request,
    elapsed: f64,
    /// For each channel: its noise (a standard deviation) summed over the passes so far.
    noise: Vec<f64>,
    passes: u32,
    /// For each channel, each spike's biggest excursion (either way) on it.
    peaks: Vec<Vec<f32>>,
}

/// Gains or thresholds chosen from the data, for each channel.
#[derive(Clone, PartialEq, Debug)]
struct Chosen {
    gains: Option<Vec<f32>>,
    thresholds: Option<Vec<f32>>,
}

/// The groups shown, as of the last pass.
#[derive(Default)]
pub struct RecentDisplay {
    stream: Option<Spikes>,
    first: usize,
    shown: usize,
    keep: usize,
    /// For each group shown: its waveforms, newest first, and how many.
    waveforms: Vec<f32>,
    count: Vec<usize>,
    total: Vec<u64>,
    thresholds: Vec<f32>,
    gains: Vec<f32>,
    /// Seconds sampled so far, and to sample, while choosing gains or thresholds.
    sampling: Option<(f32, f32)>,
    mailbox: Option<Arc<Mutex<Mailbox>>>,
}

/// Which setting's being typed.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Typed {
    Gain,
    Threshold,
}

/// A channel's gain or threshold being typed, in a box over its plot.
#[derive(Clone, Debug)]
struct Typing {
    channel: usize,
    what: Typed,
    text: String,
    /// Whether the box has been focused yet.
    focused: bool,
}

impl Node<Edges> for SpikeViewer {
    type State = Recent;

    const INFO: NodeInfo = NodeInfo {
        name: "Spike Viewer",
        category: Category::Visualisation,
        description: "Each group's (tetrode's, say) recent spike waveforms overlaid, a plot per channel, with the \
                      thresholds: set them, and the gains, here.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "spikes", kind: Kind::Spikes }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "spikes")
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "thresholds", kind: SettingKind::Thresholds, read_only: false },
            Control { name: "gains", kind: SettingKind::Gains, read_only: false },
        ]
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Window)
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Recent {
        let Some(DataStream::Spikes(stream)) = inputs[0] else { return Recent::default() };
        let groups = stream.groups() as usize;
        let keep = self.waveforms.clamp(1, 200) as usize;
        let size = (stream.group_size * stream.waveform_samples()) as usize;
        Recent {
            stream: Some(stream),
            keep,
            waveforms: vec![0.0; groups * keep * size],
            head: vec![0; groups],
            count: vec![0; groups],
            total: vec![0; groups],
            thresholds: vec![DEFAULT_THRESHOLD_UV; stream.channels as usize],
            gains: vec![DEFAULT_GAIN; stream.channels as usize],
            ..Default::default()
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, recent: &mut Recent) {
        let Some(Some(Buffer::Spikes(input))) = pass.inputs.first() else { return };
        let Some(stream) = recent.stream else { return };
        let size = input.group_size * input.samples;
        let keep = recent.keep;
        for (i, spike) in input.spikes.iter().enumerate() {
            let group = spike.group as usize;
            let slot = (group * keep + recent.head[group]) * size;
            recent.waveforms[slot..slot + size].copy_from_slice(input.waveform(i));
            recent.head[group] = (recent.head[group] + 1) % keep;
            recent.count[group] = (recent.count[group] + 1).min(keep);
            recent.total[group] += 1;
        }
        let channels = recent.thresholds.len();
        recent.thresholds.copy_from_slice(&input.thresholds[..channels]);
        recent.gains.copy_from_slice(&input.gains[..channels]);
        self.sample(pass, recent);

        let groups = stream.groups() as usize;
        let (first, shown) = self.groups(groups);
        let progress = recent.sampling.as_ref().map(|s| (s.elapsed as f32, s.request.seconds));
        let Recent { waveforms, head, count, total, thresholds, gains, mailbox, shown: latest, .. } = recent;
        latest.send(pass, |display| {
            (display.stream, display.first, display.shown, display.keep) = (Some(stream), first, shown, keep);
            display.waveforms.clear();
            display.count.clear();
            display.total.clear();
            for group in first..first + shown {
                // Newest first.
                for age in 0..count[group] {
                    let slot = (head[group] + keep - 1 - age) % keep;
                    let at = (group * keep + slot) * size;
                    display.waveforms.extend_from_slice(&waveforms[at..at + size]);
                }
                display.count.push(count[group]);
                display.total.push(total[group]);
            }
            display.thresholds.clone_from(thresholds);
            display.gains.clone_from(gains);
            display.sampling = progress;
            display.mailbox.get_or_insert_with(|| mailbox.clone());
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let groups = match context.inputs[0] {
            Some(DataStream::Spikes(s)) => s.groups(),
            _ => 16,
        };
        settings_grid(ui, |ui| {
            setting(ui, "First group", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.first_group)
                        .range(0..=groups.saturating_sub(1))
                        .custom_formatter(|v, _| format!("{}", v + 1.0))
                        .custom_parser(|s| s.trim().parse::<f64>().ok().map(|v| v - 1.0)),
                )
            });
            setting(ui, "Groups shown", |ui| ui.add(egui::DragValue::new(&mut self.groups_shown).range(1..=64)));
            setting(ui, "Waveforms", |ui| ui.add(egui::DragValue::new(&mut self.waveforms).range(1..=200)));
            setting(ui, "Chosen thresholds", |ui| {
                ui.add(egui::DragValue::new(&mut self.threshold_sd).range(1.0..=20.0).speed(0.05).suffix(" SD"))
                    .on_hover_text("Thresholds chosen from the data are this many standard deviations of the noise")
            });
        });
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let locked = context.locked;
        crate::standard_components::widgets::view_settings(ui, |ui| {
            ui.add_enabled_ui(!locked, |ui| self.settings_ui(ui, context));
        });
        let Some(DataStream::Spikes(stream)) = context.inputs[0] else {
            return preview::unconnected(ui, context.theme, "Connect spikes to view them");
        };
        let display = context.display::<RecentDisplay>().filter(|d| d.stream.is_some());
        let mut thresholds = match &context.controls[0] {
            Some(Setting::Thresholds(t)) => Some(t.clone()),
            _ => None,
        };
        let mut gains = match &context.controls[1] {
            Some(Setting::Gains(g)) => Some(g.clone()),
            _ => None,
        };
        // What the passes chose from the data, once they've sampled it.
        let mailbox = display.and_then(|d| d.mailbox.clone());
        let chosen = mailbox.as_ref().and_then(|m| m.lock().unwrap().chosen.take());
        if let Some(chosen) = chosen.filter(|_| !locked) {
            if let (Some(chosen), Some(thresholds)) = (chosen.thresholds, &mut thresholds) {
                thresholds.0 = chosen;
            }
            if let (Some(chosen), Some(gains)) = (chosen.gains, &mut gains) {
                gains.0 = chosen;
            }
        }

        let sampling = display.and_then(|d| d.sampling);
        let request = self.toolbar(ui, context, sampling, thresholds.is_some(), gains.is_some());
        if let (Some(request), Some(mailbox)) = (request, &mailbox) {
            mailbox.lock().unwrap().request = Some(request);
        }

        let plots = Plots {
            stream,
            display,
            thresholds: thresholds.as_mut(),
            gains: gains.as_mut(),
            theme: context.theme,
            locked,
        };
        self.plots_ui(ui, plots);
        if let Some(thresholds) = thresholds {
            context.controls[0] = Some(Setting::Thresholds(thresholds));
        }
        if let Some(gains) = gains {
            context.controls[1] = Some(Setting::Gains(gains));
        }
        if context.running {
            ui.ctx().request_repaint();
        }
    }
}

/// What the plots are drawn from, and set.
struct Plots<'a> {
    stream: Spikes,
    display: Option<&'a RecentDisplay>,
    /// The controls' thresholds and gains, if connected, to set.
    thresholds: Option<&'a mut Thresholds>,
    gains: Option<&'a mut Gains>,
    theme: &'a crate::core::theme::Colours,
    locked: bool,
}

impl SpikeViewer {
    /// The first group shown, and how many, of `groups`.
    fn groups(&self, groups: usize) -> (usize, usize) {
        let first = (self.first_group as usize).min(groups.saturating_sub(1));
        (first, (self.groups_shown as usize).min(groups - first))
    }

    /// The plots' height, and choosing gains or thresholds from the data, with a hint of what else can be done.
    /// Returns a request to choose them, if one's been made.
    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        context: &Context<Edges>,
        sampling: Option<(f32, f32)>,
        thresholds: bool,
        gains: bool,
    ) -> Option<Request> {
        let mut request = None;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!context.locked, |ui| {
                let label = ui.label("Plot height");
                ui.add(egui::DragValue::new(&mut self.plot_height).range(40.0..=600.0).speed(1.0).suffix(" px"))
                    .labelled_by(label.id);
            });
            ui.separator();
            let can = context.running && !context.locked && sampling.is_none();
            let why = if context.locked {
                "Not while recording"
            } else if !context.running {
                "Only while acquiring: it needs data"
            } else {
                "Sampling…"
            };
            // What to choose: picked from a menu, then chosen with the button.
            ui.add_enabled_ui(!context.locked, |ui| {
                let options = [Choosing::Both, Choosing::Gains, Choosing::Thresholds];
                egui::ComboBox::from_id_salt("choosing")
                    .selected_text(self.choosing.name())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for option in options {
                            ui.selectable_value(&mut self.choosing, option, option.name());
                        }
                    })
                    .response
                    .on_hover_text("What to choose from the data");
            });
            let (g, t) = match self.choosing {
                Choosing::Both => (true, true),
                Choosing::Gains => (true, false),
                Choosing::Thresholds => (false, true),
            };
            let connected = (!g || gains) && (!t || thresholds);
            let tip = format!(
                "Sample the data, then set each channel's {}",
                match self.choosing {
                    Choosing::Both => format!(
                        "threshold to {} standard deviations of its noise, and its gain so its bigger spikes fill its \
                         plot (and the recording's range)",
                        self.threshold_sd
                    ),
                    Choosing::Gains => "gain so its bigger spikes fill its plot (and the recording's range)".into(),
                    Choosing::Thresholds =>
                        format!("threshold to {} standard deviations of its noise", self.threshold_sd),
                }
            );
            let missing = "Connect the thresholds and gains to a spike detector to set them";
            let button = ui.add_enabled(can && connected, egui::Button::new("Choose").small());
            let chosen =
                button.on_hover_text(tip).on_disabled_hover_text(if connected { why } else { missing }).clicked();
            let label = ui.label("from");
            ui.add_enabled_ui(!context.locked, |ui| {
                let field = egui::DragValue::new(&mut self.sampling).range(SAMPLING).speed(0.2).suffix(" s");
                ui.add(field).labelled_by(label.id).on_hover_text("How long to sample the data for");
            });
            if let Some((elapsed, total)) = sampling {
                ui.add(egui::Spinner::new().size(12.0));
                ui.weak(format!("sampling, {:.0} s of {total:.0}", elapsed.min(total)));
            }
            if chosen {
                static REQUESTS: AtomicU64 = AtomicU64::new(1);
                let id = REQUESTS.fetch_add(1, Ordering::Relaxed);
                let seconds = self.sampling.clamp(*SAMPLING.start(), *SAMPLING.end());
                request = Some(Request { id, seconds, gains: g, thresholds: t });
            }
            ui.separator();
            ui.weak("Over a channel: G to type its gain, T its threshold. Drag a threshold to move it.");
        });
        request
    }

    /// Each group shown: a plot for each of its channels, at the plot height, as many to a row as fit.
    fn plots_ui(&self, ui: &mut egui::Ui, mut plots: Plots) {
        let stream = plots.stream;
        let theme = plots.theme;
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 2.0, theme.surface_sunken);
        let (first, shown) = self.groups(stream.groups() as usize);
        // The display, if it's of these groups.
        let display = plots.display.filter(|d| d.first == first && d.shown == shown);
        let (k, samples) = (stream.group_size.max(1) as usize, stream.waveform_samples() as usize);
        let height = self.plot_height.clamp(40.0, 600.0);
        let plot = egui::vec2(height * ASPECT, height);
        let (header, gap) = (16.0, 8.0);
        let tile = egui::vec2(plot.x * k as f32, header + plot.y);
        let typing_id = ui.scope_id().with("typing");
        let mut typing: Option<Typing> = ui.data(|d| d.get_temp(typing_id));

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let width = ui.available_width() - gap;
            let columns = (((width + gap) / (tile.x + gap)).floor() as usize).max(1);
            let rows = shown.div_ceil(columns);
            let size =
                egui::vec2(gap + columns.min(shown.max(1)) as f32 * (tile.x + gap), gap + rows as f32 * (tile.y + gap));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let painter = ui.painter_at(ui.clip_rect());
            let mut offset = 0;
            for i in 0..shown {
                let group = first + i;
                let min = rect.min
                    + egui::vec2(gap, gap)
                    + egui::vec2((i % columns) as f32, (i / columns) as f32) * (tile + egui::vec2(gap, gap));
                let count = display.map_or(0, |d| d.count[i]);
                let total = display.map(|d| d.total[i]);
                let label = match total {
                    Some(total) => format!("{} · {total} spikes", group + 1),
                    None => format!("{}", group + 1),
                };
                painter.text(min, Align2::LEFT_TOP, label, FontId::monospace(10.0), theme.text_secondary);
                for (c, channel) in stream.group_channels(group as u32).enumerate() {
                    let channel = channel as usize;
                    let rect = Rect::from_min_size(min + egui::vec2(c as f32 * plot.x, header), plot);
                    let waveforms = display.map(|d| {
                        let size = k * samples;
                        (0..count).map(move |age| &d.waveforms[offset + age * size + c * samples..][..samples])
                    });
                    let waveforms: Vec<&[f32]> = waveforms.into_iter().flatten().collect();
                    let keep = display.map_or(1, |d| d.keep.max(1));
                    self.plot_ui(ui, &painter, rect, channel, &waveforms, keep, &mut plots, &mut typing);
                }
                offset += count * k * samples;
            }
        });
        ui.data_mut(|d| match typing {
            Some(typing) => {
                d.insert_temp(typing_id, typing);
            }
            None => d.remove::<Typing>(typing_id),
        });
    }

    /// A channel's plot: its spikes, its threshold (to drag), and, while the pointer's over it, its gain buttons and
    /// values (and G or T to type them).
    #[allow(clippy::too_many_arguments)]
    fn plot_ui(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        channel: usize,
        waveforms: &[&[f32]],
        keep: usize,
        plots: &mut Plots,
        typing: &mut Option<Typing>,
    ) {
        let theme = plots.theme;
        let polarity = plots.stream.polarity;
        let shown = |values: &[f32], default: f32| values.get(channel).copied().unwrap_or(default);
        let display_thresholds = plots.display.map_or(&[][..], |d| &d.thresholds[..]);
        let display_gains = plots.display.map_or(&[][..], |d| &d.gains[..]);
        let threshold = match &plots.thresholds {
            Some(t) => t.get(channel),
            None => shown(display_thresholds, DEFAULT_THRESHOLD_UV),
        };
        let gain = match &plots.gains {
            Some(g) => g.get(channel),
            None => shown(display_gains, DEFAULT_GAIN),
        };
        let range = range_uv(gain);
        // Spikes go up: negative-going ones are shown upside down.
        let flip = if polarity == Polarity::Negative { -1.0 } else { 1.0 };
        let half = rect.height() * 0.5;
        let y = |uv: f32| rect.center().y - (flip * uv / range).clamp(-1.0, 1.0) * half;
        let x = |s: usize, samples: usize| rect.left() + (s as f32 + 0.5) / samples.max(1) as f32 * rect.width();

        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, theme.divider), egui::StrokeKind::Inside);
        painter.hline(rect.x_range(), rect.center().y, Stroke::new(1.0, theme.divider.gamma_multiply(0.4)));
        // Oldest first, so the newest is on top.
        for (age, waveform) in waveforms.iter().enumerate().rev() {
            let points: Vec<Pos2> =
                waveform.iter().enumerate().map(|(s, v)| egui::pos2(x(s, waveform.len()), y(*v))).collect();
            let alpha = 1.0 - 0.8 * age as f32 / keep as f32;
            painter.add(Shape::line(points, Stroke::new(1.0, Spikes::INFO.colour.gamma_multiply(alpha))));
        }

        let interactive = !plots.locked;
        let hovered = ui.rect_contains_pointer(rect);
        // The threshold: above the middle, and below too if spikes go both ways. Dragged, if it can be set.
        let mut lines = vec![y(flip * threshold)];
        if polarity == Polarity::Both {
            lines.push(y(-threshold));
        }
        for (i, line) in lines.into_iter().enumerate() {
            let mut active = false;
            if let (true, Some(thresholds)) = (interactive, plots.thresholds.as_deref_mut()) {
                let grab = Rect::from_x_y_ranges(rect.x_range(), line - 4.0..=line + 4.0);
                let response = ui.interact(grab, ui.scope_id().with(("threshold", channel, i)), Sense::drag());
                if let Some(pointer) = response.interact_pointer_pos().filter(|_| response.dragged()) {
                    let uv = ((rect.center().y - pointer.y) / half).abs() * range;
                    thresholds.set(channel, uv.clamp(0.5, range));
                }
                active = response.hovered() || response.dragged();
                response
                    .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                    .on_hover_text(format!("Channel {}'s threshold: {threshold:.1} µV. Drag to move it", channel + 1));
            }
            let colour = crate::standard_components::spikes::Thresholds::INFO.colour;
            let stroke = if active { Stroke::new(2.0, colour) } else { Stroke::new(1.0, colour.gamma_multiply(0.8)) };
            painter.hline(rect.x_range(), line, stroke);
        }

        let typing_here = typing.as_ref().is_some_and(|t| t.channel == channel);
        if hovered || typing_here {
            let values = format!("×{gain:.0}\n{threshold:.0} µV");
            let corner = rect.left_top() + egui::vec2(3.0, 2.0);
            painter.text(corner, Align2::LEFT_TOP, values, FontId::monospace(9.0), theme.text_secondary);
        }
        if hovered
            && interactive
            && let Some(gains) = plots.gains.as_deref_mut()
        {
            // Gain buttons: up at the top right, down at the bottom right.
            let button = egui::vec2(18.0, 18.0);
            let up = Rect::from_min_size(rect.right_top() + egui::vec2(-button.x - 2.0, 2.0), button);
            let down = Rect::from_min_size(rect.right_bottom() - button - egui::vec2(2.0, 2.0), button);
            for (place, up, text) in [(up, true, "+"), (down, false, "−")] {
                let tip = if up { "Raise the gain: spikes look bigger" } else { "Lower the gain: spikes look smaller" };
                if ui.put(place, egui::Button::new(text).small()).on_hover_text(tip).clicked() {
                    gains.set(channel, gain_step(gain, up));
                }
            }
        }

        // G or T over a channel types its gain or threshold.
        if hovered && interactive && typing.is_none() && ui.ctx().memory(|m| m.focused().is_none()) {
            let key = |key: Key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
            let what = if plots.gains.is_some() && key(Key::G) {
                Some(Typed::Gain)
            } else if plots.thresholds.is_some() && key(Key::T) {
                Some(Typed::Threshold)
            } else {
                None
            };
            if let Some(what) = what {
                let text = match what {
                    Typed::Gain => format!("{gain:.0}"),
                    Typed::Threshold => format!("{threshold:.1}"),
                };
                *typing = Some(Typing { channel, what, text, focused: false });
            }
        }
        if let Some(t) = typing.as_mut().filter(|t| t.channel == channel) {
            let (label, unit) = match t.what {
                Typed::Gain => ("Gain", "×"),
                Typed::Threshold => ("Threshold", "µV"),
            };
            let size = egui::vec2((rect.width() - 8.0).clamp(30.0, 80.0), 20.0);
            let field = Rect::from_center_size(rect.center(), size);
            painter.text(
                field.center_top() - egui::vec2(0.0, 2.0),
                Align2::CENTER_BOTTOM,
                format!("{label} ({unit})"),
                FontId::proportional(10.0),
                theme.text_primary,
            );
            let edit = egui::TextEdit::singleline(&mut t.text).desired_width(size.x).align(Align2::CENTER_CENTER);
            let output = ui.put(field, edit);
            if !t.focused {
                output.request_focus();
                t.focused = true;
            }
            if output.lost_focus() {
                let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                let value = t.text.trim().trim_start_matches('×').trim_end_matches("µV").trim().parse::<f32>().ok();
                match (cancelled, value.filter(|v| v.is_finite() && *v > 0.0), t.what) {
                    (false, Some(v), Typed::Gain) => {
                        if let Some(gains) = plots.gains.as_deref_mut() {
                            gains.set(channel, v)
                        }
                    }
                    (false, Some(v), Typed::Threshold) => {
                        if let Some(thresholds) = plots.thresholds.as_deref_mut() {
                            thresholds.set(channel, v)
                        }
                    }
                    _ => {}
                }
                *typing = None;
            }
        }
    }

    /// While the window's asked for gains or thresholds to be chosen: samples the pass's spikes and noise, and once
    /// it's sampled for long enough, chooses them, for the window.
    fn sample(&self, pass: &Pass<Edges>, recent: &mut Recent) {
        let Some(Some(Buffer::Spikes(input))) = pass.inputs.first() else { return };
        let channels = recent.thresholds.len();
        // A new request starts sampling afresh; none (taken back) stops it.
        if let Ok(mailbox) = recent.mailbox.try_lock() {
            match (mailbox.request, &recent.sampling) {
                (Some(request), Some(s)) if s.request == request => {}
                (Some(request), _) => {
                    recent.sampling = Some(Sampling {
                        request,
                        elapsed: 0.0,
                        noise: vec![0.0; channels],
                        passes: 0,
                        peaks: (0..channels).map(|_| Vec::with_capacity(SAMPLED_SPIKES)).collect(),
                    })
                }
                (None, _) => recent.sampling = None,
            }
        }
        let Some(sampling) = &mut recent.sampling else { return };
        for (sum, noise) in sampling.noise.iter_mut().zip(&input.noise) {
            *sum += *noise as f64;
        }
        sampling.passes += 1;
        let Some(stream) = recent.stream else { return };
        let samples = input.samples;
        for (i, spike) in input.spikes.iter().enumerate() {
            let waveform = input.waveform(i);
            for (c, channel) in stream.group_channels(spike.group).enumerate() {
                let peak = waveform[c * samples..(c + 1) * samples].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                let peaks = &mut sampling.peaks[channel as usize];
                if peaks.len() < SAMPLED_SPIKES {
                    peaks.push(peak);
                }
            }
        }
        sampling.elapsed += pass.duration;
        if sampling.elapsed < sampling.request.seconds as f64 {
            return;
        }
        let chosen = choose(sampling, &recent.thresholds, self.threshold_sd);
        let Ok(mut mailbox) = recent.mailbox.try_lock() else { return }; // done next pass, then
        if mailbox.request == Some(sampling.request) {
            mailbox.request = None;
            mailbox.chosen = Some(chosen);
        }
        recent.sampling = None;
    }
}

/// Gains or thresholds, as asked, chosen from what's been sampled, given the thresholds now. A channel's
/// threshold is `threshold_sd` standard deviations of its noise. Its gain has the bigger of its spikes (the 98th
/// percentile's biggest excursion) just fill its plot, with room for its threshold (at most two-thirds of the way
/// up); with too few spikes to tell, it has its plot show eight standard deviations of its noise.
fn choose(sampling: &mut Sampling, thresholds: &[f32], threshold_sd: f32) -> Chosen {
    let passes = sampling.passes.max(1) as f64;
    let noise: Vec<f32> = sampling.noise.iter().map(|n| (n / passes) as f32).collect();
    let chosen_thresholds: Vec<f32> = noise
        .iter()
        .zip(thresholds)
        .map(|(&noise, &now)| if noise > 0.0 { (threshold_sd * noise).max(0.5) } else { now })
        .collect();
    let thresholds = if sampling.request.thresholds { &chosen_thresholds } else { thresholds };
    let gains = sampling
        .peaks
        .iter_mut()
        .zip(&noise)
        .zip(thresholds)
        .map(|((peaks, &noise), &threshold)| {
            let biggest = if peaks.len() >= 10 {
                peaks.sort_unstable_by(f32::total_cmp);
                peaks[(peaks.len() - 1) * 98 / 100] * 1.05
            } else {
                8.0 * noise
            };
            let range = biggest.max(1.5 * threshold);
            if range > 0.0 { (FULL_SCALE_UV / range).round().clamp(*GAINS.start(), *GAINS.end()) } else { DEFAULT_GAIN }
        })
        .collect();
    Chosen {
        gains: sampling.request.gains.then_some(gains),
        thresholds: sampling.request.thresholds.then_some(chosen_thresholds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DataStreamType, PASS_DURATION};
    use crate::standard_components::spikes::Spike;

    #[test]
    fn chooses_thresholds_from_the_noise_and_gains_from_the_spikes() {
        let viewer = SpikeViewer { sampling: 4.0, ..Default::default() };
        let stream =
            Spikes { channels: 4, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32, polarity: Polarity::Negative };
        let mut recent = viewer.initial_state(&[Some(DataStream::Spikes(stream))]);
        let request = Request { id: 1, seconds: 4.0, gains: true, thresholds: true };
        recent.mailbox.lock().unwrap().request = Some(request);
        let mut buffer = stream.buffer(PASS_DURATION);
        buffer.noise = vec![10.0, 10.0, 20.0, 0.0];
        buffer.thresholds = vec![DEFAULT_THRESHOLD_UV; 4];
        let passes = (4.0 / PASS_DURATION).round() as u64;
        for sequence in 0..passes {
            buffer.clear();
            // A spike each pass, peaking at 100 µV on the first channel, 500 µV on the second (now and then).
            if let Some(waveform) = buffer.push(Spike { sample: 0, group: 0, channel: 0 }) {
                waveform[8] = -100.0;
                waveform[40 + 8] = if sequence % 4 == 0 { -500.0 } else { -50.0 };
            }
            let input = Buffer::Spikes(buffer);
            let inputs = [Some(&input)];
            let mut pass = Pass::new(sequence, PASS_DURATION, &inputs, &mut []);
            viewer.process(&mut pass, &mut recent);
            let Buffer::Spikes(b) = input else { unreachable!() };
            buffer = b;
            assert_eq!(recent.sampling.is_some(), sequence + 1 < passes, "samples for 4 s, at pass {sequence}");
        }
        let mailbox = recent.mailbox.lock().unwrap();
        assert_eq!(mailbox.request, None, "answered");
        let chosen = mailbox.chosen.clone().expect("chosen");
        assert_eq!(chosen.thresholds, Some(vec![40.0, 40.0, 80.0, DEFAULT_THRESHOLD_UV]));
        let gains = chosen.gains.unwrap();
        assert_eq!(gains[0], (FULL_SCALE_UV / 105.0).round(), "the first channel's spikes fill its plot");
        assert_eq!(gains[1], (FULL_SCALE_UV / 525.0).round(), "the second's biggest");
        assert_eq!(gains[2], (FULL_SCALE_UV / 120.0).round(), "no spikes: room for the threshold (80 µV)");
    }
}
