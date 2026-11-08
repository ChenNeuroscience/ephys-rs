//! The Spike Viewer: each group's (tetrode's, say) recent spikes overlaid, a plot per channel of the group, each
//! with the channel's threshold. Connected to a Spike Detector's thresholds and a Spike Gain's gains, it sets them:
//! drag a threshold's line, use a channel's gain buttons (shown over it), or type either (G or T over the channel:
//! up and down step it, Enter or clicking elsewhere applies it, Esc cancels).
//! Or have them detected from the data, by triggering the detector's and the gain's auto-detect (see
//! `Node::triggers`): how they do it, they say, in their own settings (a click away, in advanced mode).
//!
//! A plot shows a channel's range (see `Gains`): just what a recording's values hold, so a spike that reaches a
//! plot's edge is clipped in the recording. Spikes go up the plot, whichever way they go (a negative-going
//! spike is shown upside down), so the threshold is always above the middle (and below too, if they go both ways).

use eframe::egui::{self, Align2, FontId, Key, Pos2, Rect, Sense, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Control, EdgeType, Input, Node, NodeInfo, Output, Pass, UiContext};
use crate::standard_components::auto_detect::AutoDetect;
use crate::standard_components::plots;
use crate::standard_components::pool::Pool;
use crate::standard_components::spikes::{
    Channels, DEFAULT_RANGE_UV, DEFAULT_THRESHOLD_UV, Gains, Polarity, Spikes, Thresholds, range_step, threshold_step,
};
use crate::standard_components::widgets::{open_settings_button, require_input};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// A channel's plot's width, for its height.
const ASPECT: f32 = 0.75;
/// The lowest a threshold's dragged or stepped to.
const MIN_THRESHOLD_UV: f32 = 0.5;
/// The most recent waveforms overlaid on each group.
const WAVEFORMS: usize = 30;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeViewer {
    /// Each channel's plot's height, in points (its width follows).
    pub plot_height: f32,
    /// What to detect from the data, when asked.
    pub detect: Detect,
}

impl Default for SpikeViewer {
    fn default() -> Self {
        SpikeViewer { plot_height: 120.0, detect: Detect::Both }
    }
}

/// What to detect from the data.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub enum Detect {
    #[default]
    Both,
    Gains,
    Thresholds,
}

impl Detect {
    fn name(self) -> &'static str {
        match self {
            Detect::Both => "Gains and thresholds",
            Detect::Gains => "Gains",
            Detect::Thresholds => "Thresholds",
        }
    }

    /// The controls whose settings' auto-detect it triggers, with what each detects and what does it.
    fn controls(self) -> &'static [(usize, &'static str, &'static str)] {
        const GAIN: (usize, &str, &str) = (GAINS_CONTROL, "gains", "Spike Gain");
        const THRESHOLD: (usize, &str, &str) = (THRESHOLDS, "thresholds", "Spike Detector");
        match self {
            Detect::Both => &[GAIN, THRESHOLD],
            Detect::Gains => &[GAIN],
            Detect::Thresholds => &[THRESHOLD],
        }
    }
}

/// Its controls: the thresholds (a Spike Detector's) and the gains (a Spike Gain's) it sets.
const THRESHOLDS: usize = 0;
const GAINS_CONTROL: usize = 1;

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
    /// Each channel's range (µV either side of zero), from its gain.
    ranges: Vec<f32>,
}

impl crate::core::engine::PassState for Recent {
    const WHY: &'static str = "Each group's most recent spikes (round a ring), and how many it's had, which \
                               each pass adds to.";
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
    /// Each channel's range (µV either side of zero), from its gain.
    ranges: Vec<f32>,
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

impl Typing {
    /// What's typed, as a value: the box's own decorations are allowed, if they're typed or pasted.
    fn value(&self) -> Option<f32> {
        let text = self.text.trim().trim_start_matches('±').trim_end_matches("µV").trim();
        text.parse().ok().filter(|v: &f32| v.is_finite() && *v > 0.0)
    }

    /// Steps what's typed (or, if it isn't a value, `range` or `threshold`, as they are) up or down.
    fn step(&mut self, up: bool, range: f32, threshold: f32) {
        self.text = match self.what {
            Typed::Gain => format!("{:.0}", range_step(self.value().unwrap_or(range), !up)),
            Typed::Threshold => {
                let next = threshold_step(self.value().unwrap_or(threshold), up);
                format!("{:.1}", next.clamp(MIN_THRESHOLD_UV, range))
            }
        };
    }
}

/// A group's title, saying which channels it is, counting from 1. A channel on its own is its own title.
fn group_title(kind: &str, group: usize, channels: std::ops::Range<u32>) -> String {
    let (first, last) = (channels.start + 1, channels.end);
    match (kind, last - first) {
        ("Channel", _) => format!("Channel {first}"),
        (_, 0) => format!("{kind} {} (channel {first})", group + 1),
        _ => format!("{kind} {} (channels {first}-{last})", group + 1),
    }
}

impl Node<Edges> for SpikeViewer {
    type State = Recent;
    /// What it shows, for its window.
    type Shared = Pool<RecentDisplay>;
    type UiState = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Spike Viewer",
        category: Category::Visualisation,
        description: "Each group's (tetrode's, say) recent spike waveforms overlaid, a plot per channel, with the \
                      thresholds: set them, and the gains, here.",
    };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

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
            Control { name: "thresholds", kind: SettingKind::Thresholds, read_only: false, on_right: false },
            Control { name: "gains", kind: SettingKind::Gains, read_only: false, on_right: false },
        ]
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Recent {
        let Some(DataStream::Spikes(stream)) = inputs[0] else { return Recent::default() };
        let groups = stream.groups() as usize;
        let keep = WAVEFORMS;
        let size = (stream.group_size * stream.waveform_samples()) as usize;
        Recent {
            stream: Some(stream),
            keep,
            waveforms: vec![0.0; groups * keep * size],
            head: vec![0; groups],
            count: vec![0; groups],
            total: vec![0; groups],
            thresholds: vec![DEFAULT_THRESHOLD_UV; stream.channels as usize],
            ranges: vec![DEFAULT_RANGE_UV; stream.channels as usize],
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
            // In µV, whatever gains they've been multiplied by (see `SpikesBuffer::gains`).
            let stored = &mut recent.waveforms[slot..slot + size];
            stored.copy_from_slice(input.waveform(i));
            for (c, values) in stored.chunks_mut(input.samples.max(1)).enumerate() {
                let gain = input.gains.get(group * input.group_size + c).copied().unwrap_or(1.0);
                values.iter_mut().for_each(|v| *v /= gain.max(f32::MIN_POSITIVE));
            }
            recent.head[group] = (recent.head[group] + 1) % keep;
            recent.count[group] = (recent.count[group] + 1).min(keep);
            recent.total[group] += 1;
        }
        let channels = recent.thresholds.len();
        recent.thresholds.copy_from_slice(&input.thresholds[..channels]);
        for (range, gain) in recent.ranges.iter_mut().zip(&input.gains) {
            *range = input.output_range / gain.max(f32::MIN_POSITIVE);
        }

        let groups = stream.groups() as usize;
        let (first, shown) = self.groups(groups);
        let Recent { waveforms, head, count, total, thresholds, ranges, .. } = recent;
        pass.shared::<Pool<RecentDisplay>>().send_newest(pass.sequence, |display| {
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
            display.ranges.clone_from(ranges);
        });
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let locked = context.locked;
        let Some(DataStream::Spikes(stream)) = context.inputs[0] else {
            return plots::unconnected(ui, context.theme, "Connect spikes to view them");
        };
        let pool = context.shared::<Pool<RecentDisplay>>();
        let reading = pool.and_then(Pool::newest).filter(|d| d.stream.is_some());
        let display = reading.as_deref();
        self.toolbar(ui, context);
        let mut thresholds = match &context.controls[THRESHOLDS] {
            Some(Setting::Thresholds(t)) => Some(t.clone()),
            _ => None,
        };
        let mut gains = match &context.controls[GAINS_CONTROL] {
            Some(Setting::Gains(g)) => Some(g.clone()),
            _ => None,
        };

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
            context.controls[THRESHOLDS] = Some(Setting::Thresholds(thresholds));
        }
        if let Some(gains) = gains {
            context.controls[GAINS_CONTROL] = Some(Setting::Gains(gains));
        }
        plots::repaint_with_data(ui, context.running);
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
    /// The first group shown, and how many, of `groups`: all of them, scrolled through.
    fn groups(&self, groups: usize) -> (usize, usize) {
        (0, groups)
    }

    /// The plots' height; detecting the gains and thresholds from the data (triggering the Spike Gain's and Spike
    /// Detector's auto-detect), and in advanced mode, opening them, and a hint of what else can be done.
    fn toolbar(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!context.locked, |ui| {
                let label = ui.label("Plot height");
                ui.add(egui::DragValue::new(&mut self.plot_height).range(40.0..=600.0).speed(1.0).suffix(" px"))
                    .labelled_by(label.id);
            });
            ui.separator();
            // Auto detect: what, then run (each by its node, as it says: see their settings).
            let label = ui.strong("Auto detect");
            // What it's detecting, while it is.
            let running: Vec<(usize, f32)> = [GAINS_CONTROL, THRESHOLDS]
                .into_iter()
                .filter_map(|c| context.trigger_progress(Some(c), crate::TriggerKind::AutoDetect).map(|p| (c, p)))
                .collect();
            ui.add_enabled_ui(!context.locked && running.is_empty(), |ui| {
                egui::ComboBox::from_id_salt("detect")
                    .selected_text(self.detect.name())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for option in [Detect::Both, Detect::Gains, Detect::Thresholds] {
                            ui.selectable_value(&mut self.detect, option, option.name());
                        }
                    })
                    .response
                    .labelled_by(label.id)
                    .on_hover_text("What to detect from the data");
            });
            if !running.is_empty() {
                let stop = ui.add(egui::Button::new("Stop").small());
                if stop.on_hover_text("Stop detecting, leaving things as they were").clicked() {
                    for &(control, _) in &running {
                        context.cancel_trigger(Some(control), crate::TriggerKind::AutoDetect);
                    }
                }
                let least = running.iter().map(|r| r.1).fold(1.0, f32::min);
                ui.add(egui::ProgressBar::new(least).desired_width(60.0).show_percentage());
            } else {
                let controls = self.detect.controls();
                let missing: Vec<String> = controls
                    .iter()
                    .filter(|&&(c, _, _)| !context.control_connected(c))
                    .map(|&(_, what, by)| format!("connect its {what} to a {by}"))
                    .collect();
                let can = missing.is_empty()
                    && !context.locked
                    && controls.iter().all(|&(c, _, _)| context.can_trigger(Some(c), crate::TriggerKind::AutoDetect));
                let why = if !missing.is_empty() {
                    format!("To detect them, {}", missing.join(", and "))
                } else if context.locked {
                    "Not while recording".into()
                } else {
                    "Only while acquiring: it needs data".into()
                };
                let tip = format!(
                    "Sample each channel, then set its {}",
                    controls.iter().map(|c| c.1).collect::<Vec<_>>().join(" and ")
                );
                let run = ui.add_enabled(can, egui::Button::new("Run").small());
                if run.on_hover_text(tip).on_disabled_hover_text(why).clicked() {
                    for &(control, _, _) in controls {
                        context.trigger(Some(control), AutoDetect);
                    }
                }
            }
            if context.advanced() {
                ui.separator();
                ui.label("Gains");
                open_settings_button(ui, context, GAINS_CONTROL, "Spike Gain");
                ui.label("Thresholds");
                open_settings_button(ui, context, THRESHOLDS, "Spike Detector");
                ui.separator();
                ui.weak("Over a channel: G to type its gain, T its threshold. Drag a threshold to move it.");
            }
            context.node_toolbar(ui);
        });
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
        // Each group on a card of its own: a title, then its plots, with room around them.
        let (header, pad, gap) = (22.0, 6.0, 16.0);
        let tile = egui::vec2(plot.x * k as f32 + 2.0 * pad, header + plot.y + pad);
        let kind = match k {
            1 => "Channel",
            2 => "Stereotrode",
            4 => "Tetrode",
            8 => "Octrode",
            _ => "Group",
        };
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
                let card = Rect::from_min_size(min, tile);
                painter.rect(card, 6.0, theme.surface_raised, Stroke::new(1.0, theme.border), egui::StrokeKind::Inside);
                let title_y = min.y + header / 2.0;
                let title = group_title(kind, group, stream.group_channels(group as u32));
                let left = egui::pos2(min.x + pad + 2.0, title_y);
                painter.text(left, Align2::LEFT_CENTER, title, FontId::proportional(13.0), theme.text_primary);
                if let Some(total) = total {
                    let right = egui::pos2(card.right() - pad - 2.0, title_y);
                    let spikes = format!("{total} spikes");
                    painter.text(right, Align2::RIGHT_CENTER, spikes, FontId::monospace(10.0), theme.text_secondary);
                }
                for (c, channel) in stream.group_channels(group as u32).enumerate() {
                    let channel = channel as usize;
                    let rect = Rect::from_min_size(min + egui::vec2(pad + c as f32 * plot.x, header), plot);
                    painter.rect_filled(rect, 0.0, theme.surface_sunken);
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
        let layout = Channels { count: plots.stream.channels as usize, group_size: plots.stream.group_size as usize };
        let shown = |values: &[f32], default: f32| values.get(channel).copied().unwrap_or(default);
        let display_thresholds = plots.display.map_or(&[][..], |d| &d.thresholds[..]);
        let display_ranges = plots.display.map_or(&[][..], |d| &d.ranges[..]);
        let threshold = match &plots.thresholds {
            Some(t) => t.get(channel),
            None => shown(display_thresholds, DEFAULT_THRESHOLD_UV),
        };
        let range = match &plots.gains {
            Some(g) => g.range_uv(channel),
            None => shown(display_ranges, DEFAULT_RANGE_UV),
        };
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
                    thresholds.set(channel, uv.clamp(MIN_THRESHOLD_UV, range));
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
            let values = format!("±{range:.0}\n{threshold:.0} µV");
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
                    gains.set_range(channel, range_step(range, up), layout);
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
                    Typed::Gain => format!("{range:.0}"),
                    Typed::Threshold => format!("{threshold:.1}"),
                };
                *typing = Some(Typing { channel, what, text, focused: false });
            }
        }
        if let Some(t) = typing.as_mut().filter(|t| t.channel == channel) {
            let (label, unit) = match t.what {
                Typed::Gain => ("Range", "±µV"),
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
            // Its own id, as the widgets around it come and go with the pointer: it keeps the focus till it's done.
            let id = ui.scope_id().with(("typing", channel));
            if ui.memory(|m| m.has_focus(id)) {
                for (key, up) in [(Key::ArrowUp, true), (Key::ArrowDown, false)] {
                    while ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                        t.step(up, range, threshold);
                    }
                }
            }
            let edit = egui::TextEdit::singleline(&mut t.text)
                .id(id)
                .lock_focus(true)
                .desired_width(size.x)
                .align(Align2::CENTER_CENTER);
            let output = ui.put(field, edit);
            if !t.focused {
                output.request_focus();
                t.focused = true;
            }
            // Enter, or a click elsewhere, applies it.
            if output.lost_focus() {
                let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                match (cancelled, t.value(), t.what) {
                    (false, Some(v), Typed::Gain) => {
                        if let Some(gains) = plots.gains.as_deref_mut() {
                            gains.set_range(channel, v, layout)
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
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::standard_components::themes::THEMES;

    #[test]
    fn a_groups_title_says_its_channels() {
        assert_eq!(group_title("Tetrode", 0, 0..4), "Tetrode 1 (channels 1-4)");
        assert_eq!(group_title("Tetrode", 2, 8..12), "Tetrode 3 (channels 9-12)");
        assert_eq!(group_title("Tetrode", 2, 8..10), "Tetrode 3 (channels 9-10)", "a smaller last group");
        assert_eq!(group_title("Tetrode", 2, 8..9), "Tetrode 3 (channel 9)");
        assert_eq!(group_title("Channel", 4, 4..5), "Channel 5", "it says it already");
    }

    const STREAM: Spikes =
        Spikes { channels: 8, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32, polarity: Polarity::Negative };

    fn harness<'a>() -> Harness<'a, (Thresholds, Gains)> {
        Harness::builder().with_size([700.0, 400.0]).build_ui_state(
            |ui, (thresholds, gains): &mut (Thresholds, Gains)| {
                let plots = Plots {
                    stream: STREAM,
                    display: None,
                    thresholds: Some(thresholds),
                    gains: Some(gains),
                    theme: &THEMES[0].colours,
                    locked: false,
                };
                SpikeViewer::default().plots_ui(ui, plots);
            },
            Default::default(),
        )
    }

    fn pointer_at(harness: &mut Harness<'_, (Thresholds, Gains)>, x: f32, y: f32) {
        harness.event(egui::Event::PointerMoved(egui::pos2(x, y)));
        harness.run_steps(2);
    }

    /// Over the first channel's plot.
    fn type_threshold(harness: &mut Harness<'_, (Thresholds, Gains)>, key: Key) {
        pointer_at(harness, 70.0, 80.0);
        harness.key_press(key);
        harness.run_steps(2);
        assert!(typing(harness), "the box is up");
    }

    fn typing(harness: &Harness<'_, (Thresholds, Gains)>) -> bool {
        harness.query_all_by_role(egui::accesskit::Role::TextInput).next().is_some()
    }

    fn click_elsewhere(harness: &mut Harness<'_, (Thresholds, Gains)>) {
        pointer_at(harness, 690.0, 390.0);
        let click = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(690.0, 390.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        harness.event(click(true));
        harness.event(click(false));
        harness.run_steps(2);
    }

    #[test]
    fn the_box_keeps_the_focus_until_its_done() {
        let mut harness = harness();
        type_threshold(&mut harness, Key::T);
        // The pointer leaving the plot takes its gain buttons with it.
        pointer_at(&mut harness, 690.0, 390.0);
        pointer_at(&mut harness, 70.0, 80.0);
        pointer_at(&mut harness, 690.0, 390.0);
        harness.key_press(Key::ArrowUp);
        harness.key_press(Key::ArrowUp);
        harness.run_steps(2);
        assert!(typing(&harness), "still there");
        assert_eq!(harness.state().0.get(0), DEFAULT_THRESHOLD_UV, "nothing's applied yet");

        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(!typing(&harness));
        assert_eq!(harness.state().0.get(0), DEFAULT_THRESHOLD_UV, "cancelled");

        type_threshold(&mut harness, Key::T);
        harness.key_press(Key::ArrowUp);
        harness.key_press(Key::ArrowUp);
        harness.key_press(Key::ArrowDown);
        harness.run_steps(2);
        click_elsewhere(&mut harness);
        assert!(!typing(&harness));
        assert_eq!(harness.state().0.get(0), 51.0, "applied by clicking elsewhere");

        type_threshold(&mut harness, Key::G);
        harness.key_press(Key::ArrowDown);
        harness.key_press(Key::Enter);
        harness.run_steps(2);
        assert!(!typing(&harness));
        assert!((harness.state().1.range_uv(0) - 200.0).abs() < 1e-3, "a step down the ranges, applied by Enter");
    }
}
