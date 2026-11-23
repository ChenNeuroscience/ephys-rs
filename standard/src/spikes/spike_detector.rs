//! The Spike Detector: finds spikes in groups of channels (tetrodes, say) of continuous data: a crossing of a
//! channel's threshold (µV past zero, either way: below its negative, or above it), on any of a group's channels,
//! then the waveform on all of them around the peak that follows (its furthest from zero). Each channel's threshold
//! is a setting, set by hand, from a Spike Viewer, or chosen from the data ("auto detect": see
//! `Thresholds::detect`): a
//! percentile of the channel's samples over a while, on whichever side of zero it's further out, times a factor.
//!
//! A group can't spike again until its last spike's waveform has ended, so spikes don't overlap, and a pass's
//! spikes always fit the buffer (see `Spikes`). A spike is only found once its whole waveform has come, so
//! those near the end of a pass come with the next, which carries on from the end of this one.

use eframe::egui;
use ephys_rs::engine::{Buffers, TreeShake};
use serde::{Deserialize, Serialize};

use crate::standard::auto_detect::{Detect, Distribution, PercentMultiple, SAMPLING, auto_detect_ui, detect_changed};
use crate::standard::ephys::{Continuous, ContinuousBuffer};
use crate::standard::spikes::{SetThresholds, Spike, Spikes, Thresholds};
use crate::standard::widgets::{combo, input_summary, named_settings_grid, require_input, setting, settings_grid};
use ephys_rs::engine::EdgeType;
use ephys_rs::engine::{
    Category, CommandContext, CommandSource, Node, NodeInfo, PASS_DURATION, PassContext, Prepare, PrepareContext,
    Prepared, UiContext,
};
use ephys_rs::gui::widgets::info_icon;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeDetector {
    /// Channels in each group: 4 for tetrodes, 1 for single electrodes.
    pub group_size: u32,
    /// Waveform samples kept before and after each spike's peak.
    pub pre: u32,
    pub post: u32,
    pub thresholds: Thresholds,
    /// Thresholds chosen from the data are this, of each channel's samples either side of zero.
    pub level: PercentMultiple,
}

impl Default for SpikeDetector {
    fn default() -> Self {
        SpikeDetector {
            group_size: 4,
            pre: 8,
            post: 32,
            thresholds: Thresholds::default(),
            level: PercentMultiple { percent: 98.0, multiple: 2.1 },
        }
    }
}

impl SpikeDetector {
    fn stream(&self, channels: u32, sample_rate: f32) -> Spikes {
        Spikes {
            channels,
            sample_rate,
            group_size: self.group_size.max(1),
            pre: self.pre.max(1),
            post: self.post.max(1),
        }
    }
}

/// Positions a group's channels are looked at together for crossings: compares on a row's samples as they lie, with
/// no branches, rather than a position at a time. Rows have this much room past a pass's samples, so the last positions
/// are looked at the same way (what's past them isn't counted).
const SCAN: usize = 16;

/// Each channel's recent samples, and when each group can next spike.
#[derive(Default)]
pub struct Detector {
    spikes: Option<Spikes>,
    /// Samples a peak is looked for in, after a threshold crossing.
    lookahead: usize,
    /// For each channel, the end of the samples so far (`tail` long), then this pass's, then room for a scan past them
    /// (`SCAN`): `stride` apiece.
    samples: Vec<f32>,
    tail: usize,
    stride: usize,
    /// For each group, the first position (in `samples`) its next spike's peak can be at.
    next: Vec<i64>,
}

/// What the node carries from pass to pass: the detector, and, only while the thresholds are being detected (see
/// `crate::standard::auto_detect`), what it's choosing them from.
#[derive(Default)]
pub struct Detecting {
    /// `None` only in a proto-state made for detecting the thresholds starting or ending, the node's data streams being
    /// the same: the detector so far carries on (see `adopt_snapshot`).
    detector: Option<Detector>,
    sampling: Option<Sampling>,
}

/// While choosing thresholds from the data: seconds sampled, of how many; and each channel's samples so far.
struct Sampling {
    elapsed: f64,
    seconds: f64,
    /// Each channel's samples (none, once it's done).
    data: Vec<Distribution>,
    /// Whether the thresholds have been chosen: `detect`'s still set in the snapshot, till the update choosing them
    /// (which also clears it) takes over.
    done: bool,
}

impl Sampling {
    fn new(seconds: f64, channels: usize) -> Self {
        Sampling { elapsed: 0.0, seconds, data: vec![Distribution::default(); channels], done: false }
    }
}

impl ephys_rs::engine::PassState for Detecting {
    const WHY: &'static str = "Each channel's last samples (so a spike across two passes is found whole), when each \
                               group can next spike (so a group's spikes don't overlap), and while choosing \
                               thresholds, the samples so far.";
}

impl Detector {
    fn new(spikes: Spikes) -> Self {
        let channels = spikes.channels as usize;
        let lookahead = ((spikes.sample_rate / 2000.0) as usize).max(1); // half a millisecond
        let tail = (spikes.pre + spikes.post) as usize + lookahead;
        let pass = (spikes.sample_rate as f64 * PASS_DURATION).ceil() as usize * 5 / 4 + 1;
        Detector {
            spikes: Some(spikes),
            lookahead,
            samples: vec![0.0; channels * (tail + pass + SCAN)],
            tail,
            stride: tail + pass + SCAN,
            next: vec![0; spikes.groups() as usize],
        }
    }

    /// Makes room for passes of `n` samples (should one be longer than expected).
    fn fit(&mut self, n: usize, channels: usize) {
        if self.tail + n + SCAN <= self.stride {
            return;
        }
        let stride = self.tail + n + SCAN;
        let mut samples = vec![0.0; channels * stride];
        for channel in 0..channels {
            let tail = &self.samples[channel * self.stride..channel * self.stride + self.tail];
            samples[channel * stride..channel * stride + self.tail].copy_from_slice(tail);
        }
        (self.samples, self.stride) = (samples, stride);
    }
}

ephys_rs::ports! {
    continuous: DataStreamIn<Continuous>,
    spikes: DataStreamOut<Spikes>,
    thresholds: ValOutCmdIn<Thresholds, (SetThresholds, Detect)>,
}

impl<E: PortEdges> Node<E> for SpikeDetector {
    type Ports = Ports;
    const INFO: NodeInfo = NodeInfo {
        name: "Spike Detector",
        category: Category::Processing,
        description: "Finds spikes on groups of channels (tetrodes, say): threshold crossings, with the waveform \
                      on each of the group's channels around the peak.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], _recording: bool) -> TreeShake {
        // And while detecting its thresholds, whatever needs what it makes.
        let wanted = outputs.contains(&true) || self.thresholds.detect.started.is_some();
        TreeShake::needing_all(inputs.contains(&true) && wanted, inputs.len())
    }

    type State = Detecting;
    type WorkerState = ();
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    /// The detector, for the node's data streams, unless they're the same; and while the thresholds are being
    /// detected, each channel's room to sample into.
    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Detecting, (), Self> {
        let before = context.previous.as_ref().map(|previous| previous.node.thresholds.detect.started);
        if context.same_streams() && !detect_changed(self.thresholds.detect.started, before) {
            return Prepared::nothing().into();
        }
        let input = context.data_stream_in::<DataStreamIn>().continuous;
        let detector = (!context.same_streams()).then(|| match input {
            Some(c) => Detector::new(self.stream(c.channels, c.sample_rate)),
            None => Detector::default(),
        });
        let seconds = self.thresholds.detect.seconds.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
        let channels = input.map_or(0, |c| c.channels as usize);
        let sampling = self.thresholds.detect.started.map(|_| Sampling::new(seconds, channels));
        Prepared::shared(Detecting { detector, sampling }).into()
    }

    /// The detector so far carries on unless the node made a new one; the sampling's always what the node prepared.
    fn adopt_snapshot(&self, current_state: &mut Detecting, new_proto_state: &mut Detecting) {
        if new_proto_state.detector.is_some() {
            std::mem::swap(&mut current_state.detector, &mut new_proto_state.detector);
        }
        std::mem::swap(&mut current_state.sampling, &mut new_proto_state.sampling);
    }

    fn process(
        &self,
        pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        outputs: DataStreamOut<Buffers>,
        state: &mut Detecting,
        _: &mut (),
    ) {
        let (Some(input), output) = (inputs.continuous, outputs.spikes) else { return };
        output.clear();
        output.pass_samples = input.len;
        let Detecting { detector: Some(detector), sampling } = state else { return };
        let Some(spikes) = detector.spikes else { return };
        let channels = (spikes.channels as usize).min(input.channels);
        let n = input.len;
        detector.fit(n, channels);
        let (tail, stride) = (detector.tail, detector.stride);

        // This pass's samples after the tail.
        for (channel, threshold) in output.thresholds.iter_mut().enumerate().take(channels) {
            *threshold = self.thresholds.get(channel);
        }
        for channel in 0..channels {
            let row = &mut detector.samples[channel * stride..(channel + 1) * stride];
            row[tail..tail + n].copy_from_slice(input.channel(channel));
        }

        // Spikes peaking at positions from `pre` (the first not looked at last pass) on, whose waveform and
        // lookahead have all come.
        let (pre, post, lookahead) = (spikes.pre as usize, spikes.post as usize, detector.lookahead);
        let end = pre + n;
        // Either way: how far past zero.
        let extremity = |x: f32| x.abs();
        let beyond = |x: f32, threshold: f32| extremity(x) > threshold;
        for group in 0..spikes.groups() {
            let group_channels = spikes.group_channels(group);
            let mut position = detector.next[group as usize].max(pre as i64) as usize;
            while position < end {
                // The next `SCAN` positions' crossings, on any of the group's channels: a bit each.
                let mut crossings = [false; SCAN];
                for channel in group_channels.clone() {
                    let start = channel as usize * stride + position - 1;
                    let row: &[f32; SCAN + 1] = detector.samples[start..start + SCAN + 1].try_into().unwrap();
                    let threshold = output.thresholds[channel as usize];
                    for (i, crossed) in crossings.iter_mut().enumerate() {
                        *crossed |= beyond(row[i + 1], threshold) & !beyond(row[i], threshold);
                    }
                }
                let bits = crossings.iter().rev().fold(0u32, |bits, &crossed| bits << 1 | crossed as u32);
                let bits = bits & ((1 << SCAN.min(end - position)) - 1);
                if bits == 0 {
                    position = (position + SCAN).min(end); // what's past the end comes with the next pass
                    continue;
                }
                // The first, and the first of the group's channels crossing there.
                let p = position + bits.trailing_zeros() as usize;
                let channel = group_channels
                    .clone()
                    .find(|&channel| {
                        let row = &detector.samples[channel as usize * stride..];
                        let threshold = output.thresholds[channel as usize];
                        beyond(row[p], threshold) && !beyond(row[p - 1], threshold)
                    })
                    .expect("one crossed there");
                let row = &detector.samples[channel as usize * stride..];
                let peak =
                    (p..=p + lookahead).max_by(|&a, &b| extremity(row[a]).total_cmp(&extremity(row[b]))).unwrap();
                let spike = Spike { sample: peak as i32 - tail as i32, group, channel };
                if let Some(waveforms) = output.push(spike) {
                    let samples = pre + post;
                    for (i, c) in group_channels.clone().enumerate() {
                        let row = &detector.samples[c as usize * stride..];
                        waveforms[i * samples..(i + 1) * samples].copy_from_slice(&row[peak - pre..peak + post]);
                    }
                }
                // Spikes don't overlap: the next can't start before this one's waveform ends.
                position = peak + pre + post;
            }
            detector.next[group as usize] = position as i64 - n as i64;
        }

        // Keep the end of the samples, for the next pass.
        for channel in 0..channels {
            let row = &mut detector.samples[channel * stride..(channel + 1) * stride];
            row.copy_within(n..n + tail, 0);
        }
        self.choose_thresholds(pass, input, sampling);
    }

    fn data_stream_out(&self, inputs: &DataStreamIn) -> DataStreamOut {
        DataStreamOut { spikes: inputs.continuous.map(|c| self.stream(c.channels, c.sample_rate)) }
    }

    fn val_out(&self) -> ValOutCmdIn {
        ValOutCmdIn { thresholds: self.thresholds.clone() }
    }

    /// New thresholds: from its passes, those auto detect chose, with it done; from a viewer, an edit, leaving auto
    /// detect as it is.
    fn on_command(&mut self, from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {
            CmdIn::SetThresholds(SetThresholds(thresholds)) => {
                let started = self.thresholds.detect.started;
                self.thresholds = thresholds;
                if from != CommandSource::Itself {
                    self.thresholds.detect.started = started;
                }
            }
            CmdIn::Detect(Detect(request)) => request.apply(&mut self.thresholds.detect.started),
        }
    }

    fn clear_transient_state(&mut self) {
        self.thresholds.detect.started = None;
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let (rate, channels) = match context.data_stream_in::<DataStreamIn>().continuous {
                Some(c) => (c.sample_rate, c.channels as usize),
                _ => (30000.0, 0),
            };
            let ms = |samples: u32| format!(" samples ({:.2} ms)", samples as f32 * 1000.0 / rate);
            settings_grid(ui, |ui| {
                setting(ui, "Group", |ui| {
                    combo(
                        ui,
                        "group",
                        &mut node.group_size,
                        &[(1, "Single channels"), (2, "Stereotrodes (2)"), (4, "Tetrodes (4)"), (8, "Octrodes (8)")],
                    )
                });
                let (pre, post) = (ms(node.pre), ms(node.post));
                setting(ui, "Before peak", |ui| ui.add(egui::DragValue::new(&mut node.pre).range(1..=64).suffix(pre)));
                setting(ui, "After peak", |ui| {
                    ui.add(egui::DragValue::new(&mut node.post).range(1..=128).suffix(post))
                });
                setting(ui, "Thresholds", |ui| ui.label(node.thresholds.summary()));
            });
            // Choosing the thresholds from the data (not while it's going).
            ui.add_space(8.0);
            ui.strong("Auto detect");
            ui.add_enabled_ui(node.thresholds.detect.started.is_none(), |ui| {
                named_settings_grid(ui, "auto detect", |ui| {
                    setting(ui, "Level", |ui| {
                        ui.horizontal(|ui| {
                            node.level.ui(ui, "The percentile of the samples to use", "The percentile times this");
                            info_icon(
                                ui,
                                "For each channel, computes the given percentile of its voltage over the sampling \
                                 time. Then sets its spike threshold to that percentile multiplied by the given \
                                 factor.",
                            );
                        })
                        .response
                    });
                    setting(ui, "Sampling for", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut node.thresholds.detect.seconds)
                                .range(SAMPLING)
                                .speed(0.2)
                                .suffix(" s"),
                        )
                        .on_hover_text("How long to sample for")
                    });
                });
            });
            let detect = auto_detect_ui(
                ui,
                context,
                node.thresholds.detect.started,
                node.thresholds.detect.seconds,
                "Sample each channel, then set its threshold",
            );
            if let Some(request) = detect {
                request.apply(&mut node.thresholds.detect.started);
            }
            if channels > 0 {
                node.channels_ui(ui, channels);
            }
            input_summary(ui, context);
        });
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        let mut warnings = require_input(inputs.any_connected(), "a (filtered) continuous data stream");
        if let Some(c) = inputs.continuous
            && c.channels % self.group_size.max(1) != 0
        {
            warnings.push(format!(
                "{} channels don't make whole groups of {}: the last group has {}",
                c.channels,
                self.group_size,
                c.channels % self.group_size.max(1)
            ));
        }
        warnings
    }
}

impl SpikeDetector {
    /// While the thresholds are being detected, samples each channel for a while, then sends the node
    /// thresholds chosen from it, with `detect` cleared.
    fn choose_thresholds<E: PortEdges>(
        &self,
        pass: &PassContext<E>,
        input: &ContinuousBuffer,
        sampling: &mut Option<Sampling>,
    ) {
        if let Some(sampling) = sampling.as_mut().filter(|s| !s.done) {
            for (channel, data) in sampling.data.iter_mut().enumerate().take(input.channels) {
                input.channel(channel).iter().for_each(|x| data.add(*x));
            }
            sampling.elapsed += pass.duration;
            if sampling.elapsed >= sampling.seconds {
                let mut thresholds = self.thresholds.clone();
                // Done: no longer detecting.
                thresholds.detect.started = None;
                for (channel, data) in sampling.data.iter().enumerate() {
                    let level = self.level.either_side(data);
                    if level > 0.0 {
                        thresholds.set(channel, level.max(0.5));
                    }
                }
                pass.cmd_self(CmdIn::SetThresholds(SetThresholds(thresholds)));
                *sampling = Sampling { done: true, ..Sampling::new(sampling.seconds, 0) };
            }
        }
    }

    /// Each channel's threshold, to set by hand (usually set from a Spike Viewer, or chosen from the data).
    fn channels_ui(&mut self, ui: &mut egui::Ui, channels: usize) {
        egui::CollapsingHeader::new("Each channel")
            .id_salt("each channel")
            .show(ui, |ui| {
                egui::Grid::new("channels").num_columns(2).striped(true).show(ui, |ui| {
                    ui.strong("Channel");
                    ui.strong("Threshold");
                    ui.end_row();
                    for channel in 0..channels {
                        ui.label(format!("{}", channel + 1));
                        let mut threshold = self.thresholds.get(channel);
                        let field = egui::DragValue::new(&mut threshold).range(0.1..=10_000.0).speed(0.5).suffix(" µV");
                        if ui.add(field).changed() {
                            self.thresholds.set(channel, threshold);
                        }
                        ui.end_row();
                    }
                });
            })
            .header_response
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Command;
    use crate::Edges;
    use ephys_rs::engine::{DataStreamType, PassContext};

    /// Runs passes of `samples` through a detector, returning the spikes found (with their pass).
    fn detect(detector: &SpikeDetector, channels: u32, passes: &[Vec<Vec<f32>>]) -> Vec<(usize, Spike, Vec<f32>)> {
        let stream = Continuous { channels, sample_rate: 30000.0 };
        let mut state = ephys_rs::engine::fresh_state::<Edges, _>(detector, &DataStreamIn { continuous: Some(stream) });
        let spikes = detector.stream(channels, 30000.0);
        let mut found = Vec::new();
        for (index, pass) in passes.iter().enumerate() {
            let mut input = stream.buffer(PASS_DURATION);
            input.len = pass[0].len();
            for (channel, samples) in pass.iter().enumerate() {
                input.channel_mut(channel).copy_from_slice(samples);
            }
            let mut output = spikes.buffer(PASS_DURATION);
            let (inputs, outputs) = (DataStreamIn { continuous: Some(&input) }, DataStreamOut { spikes: &mut output });
            let mut pass = PassContext::<Edges>::new(index as u64, PASS_DURATION);
            detector.process(&mut pass, inputs, outputs, &mut state, &mut ());
            for (i, spike) in output.spikes.iter().enumerate() {
                found.push((index, *spike, output.waveform(i).to_vec()));
            }
        }
        found
    }

    /// Noise of about 10 µV on each channel.
    fn noise(channels: u32, samples: usize, seed: u64) -> Vec<Vec<f32>> {
        (0..channels as u64)
            .map(|c| (0..samples).map(|s| 10.0 * crate::standard::plots::noise(seed + c, s as i64)).collect())
            .collect()
    }

    #[test]
    fn finds_a_spike_across_a_pass_boundary_with_its_whole_waveform() {
        let detector = SpikeDetector::default();
        let mut passes = vec![noise(4, 600, 0), noise(4, 600, 100)];
        // A spike on channel 2 peaking 5 samples before the end of the first pass, smaller on the others.
        for (channel, size) in [(0, 60.0), (1, 80.0), (2, 200.0), (3, 40.0)] {
            for k in 0..6 {
                passes[0][channel][594 - 2 + k] -= size * [0.3, 0.7, 1.0, 1.0, 0.7, 0.3][k];
            }
        }
        let found = detect(&detector, 4, &passes);
        assert_eq!(found.len(), 1, "{:?}", found.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>());
        let (pass, spike, waveform) = &found[0];
        assert_eq!((*pass, spike.group, spike.channel), (1, 0, 2), "found in the next pass, once it all came");
        assert!((-7..=-5).contains(&spike.sample), "at its peak, in the previous pass: {}", spike.sample);
        // The waveform's peak is at `pre`, on every channel of the group.
        let samples = 40;
        let peak_channel = &waveform[2 * samples..3 * samples];
        assert!(peak_channel[8] < -150.0, "{:?}", &peak_channel[..12]);
    }

    #[test]
    fn finds_the_same_spikes_however_the_samples_come_in_passes() {
        let detector = SpikeDetector::default();
        let mut signal = noise(8, 1500, 3);
        // Spikes all through it, either side of zero, on each channel in turn.
        for (i, at) in (20..1450).step_by(47).enumerate() {
            signal[i % 8][at] += if i % 3 == 0 { 120.0 } else { -150.0 };
        }
        // Each spike's sample from the start, channel and waveforms, in order (a pass's come group by group).
        let found = |lengths: &[usize]| {
            let starts: Vec<usize> =
                lengths.iter().scan(0, |start, len| Some(std::mem::replace(start, *start + len))).collect();
            let passes: Vec<Vec<Vec<f32>>> = starts
                .iter()
                .zip(lengths)
                .map(|(&start, len)| signal.iter().map(|row| row[start..start + len].to_vec()).collect())
                .collect();
            let mut found: Vec<_> = detect(&detector, 8, &passes)
                .into_iter()
                .map(|(pass, spike, waveform)| (starts[pass] as i32 + spike.sample, spike.channel, waveform))
                .collect();
            found.sort_by_key(|(sample, ..)| *sample);
            found
        };
        let whole = found(&[750, 750]);
        assert_eq!(whole.len(), 31, "each of them: they're further apart than a waveform");
        assert_eq!(found(&[1, 0, 37, 15, 16, 17, 600, 64, 250, 500]), whole);
        // Passes of each length up to past a scan's, so spikes come everywhere about where passes join.
        for length in 1..=48 {
            let mut lengths = vec![length; 1500 / length];
            lengths.push(1500 % length);
            assert_eq!(found(&lengths), whole, "passes of {length}");
        }
    }

    #[test]
    fn spikes_in_a_group_never_overlap() {
        let detector = SpikeDetector::default();
        let mut pass = noise(4, 600, 7);
        // Spikes every 10 samples on one channel: closer than a waveform (40), so only some count.
        for start in (20..560).step_by(10) {
            pass[0][start] -= 300.0;
        }
        let found = detect(&detector, 4, &[pass, noise(4, 600, 9)]);
        let peaks: Vec<i64> = found.iter().map(|(p, s, _)| *p as i64 * 600 + s.sample as i64).collect();
        assert!(peaks.windows(2).all(|w| w[1] - w[0] >= 40), "{peaks:?}");
        assert!(found.len() >= 10, "{}", found.len());
    }

    #[test]
    fn chooses_thresholds_from_the_noise_when_detecting() {
        let mut detector =
            SpikeDetector { level: PercentMultiple { percent: 98.0, multiple: 2.0 }, ..Default::default() };
        detector.thresholds.detect.seconds = 4.0;
        let stream = Continuous { channels: 4, sample_rate: 30000.0 };
        let started = std::time::Instant::now();
        let inputs = DataStreamIn { continuous: Some(stream) };
        detector.thresholds.detect.started = Some(started);
        let mut state = ephys_rs::engine::fresh_state::<Edges, _>(&detector, &inputs);
        let spikes = detector.stream(4, 30000.0);
        let commands = ephys_rs::commands::Commands::default();
        let passes = (4.0 / PASS_DURATION).round() as u64;
        for sequence in 0..passes {
            let mut input = stream.buffer(PASS_DURATION);
            input.len = 600;
            // Negative-going spikes are plotted upside down, so these are at 10 and 30 µV.
            for channel in 0..4 {
                for (t, x) in input.channel_mut(channel).iter_mut().enumerate() {
                    *x = [-10.0, -30.0][t % 2];
                }
            }
            let mut output = spikes.buffer(PASS_DURATION);
            let (inputs, outputs) = (DataStreamIn { continuous: Some(&input) }, DataStreamOut { spikes: &mut output });
            let mut pass = PassContext::<Edges>::new(sequence, PASS_DURATION);
            pass.commands = &commands;
            detector.process(&mut pass, inputs, outputs, &mut state, &mut ());
        }
        let updates = commands.take_commands::<Command>();
        let [(_, Command::SetThresholds(SetThresholds(thresholds)))] = &updates[..] else { panic!("{updates:?}") };
        for channel in 0..4 {
            let threshold = thresholds.get(channel);
            assert!((threshold - 60.0).abs() < 1.0, "the 98th percentile, 30, doubled, from zero: {threshold}");
        }
    }
}
