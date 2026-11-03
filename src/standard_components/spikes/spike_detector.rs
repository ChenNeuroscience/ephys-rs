//! The Spike Detector: finds spikes in groups of channels (tetrodes, say) of continuous data: a crossing of a
//! channel's threshold (µV past zero, the way the spikes go), on any of a group's channels, then the waveform on all
//! of them around the peak that follows. Each channel's threshold is a setting, set by hand, from a Spike Viewer,
//! or chosen from the data when it's triggered ("auto detect"): so many standard deviations of the channel's noise
//! over a while (its median absolute value / 0.6745, as the Open Ephys spike detector has it, which spikes
//! themselves hardly move).
//!
//! A group can't spike again until its last spike's waveform has ended, so spikes don't overlap, and a pass's
//! spikes always fit the buffer (see `Spikes`). A spike is only found once its whole waveform has come, so
//! those near the end of a pass come with the next, which carries on from the end of this one.

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::EdgeType;
use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, PASS_DURATION, Pass, SettingPort};
use crate::core::exchange::Latest;
use crate::standard_components::auto_detect::{AUTO_DETECT, SAMPLING, auto_detect_ui, detecting};
use crate::standard_components::spikes::{Polarity, Spike, Spikes, Thresholds};
use crate::standard_components::widgets::{
    combo, input_summary, named_settings_grid, require_input, setting, settings_grid,
};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpikeDetector {
    pub polarity: Polarity,
    /// Channels in each group: 4 for tetrodes, 1 for single electrodes.
    pub group_size: u32,
    /// Waveform samples kept before and after each spike's peak.
    pub pre: u32,
    pub post: u32,
    pub thresholds: Thresholds,
    /// Thresholds chosen from the data are this many standard deviations of each channel's noise, sampled for so
    /// many seconds.
    pub threshold_sd: f32,
    pub sampling: f32,
}

impl Default for SpikeDetector {
    fn default() -> Self {
        SpikeDetector {
            polarity: Polarity::Negative,
            group_size: 4,
            pre: 8,
            post: 32,
            thresholds: Thresholds::default(),
            threshold_sd: 4.0,
            sampling: 10.0,
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
            polarity: self.polarity,
        }
    }
}

/// Each channel's recent samples and noise, and when each group can next spike.
#[derive(Default)]
pub struct Detector {
    spikes: Option<Spikes>,
    /// Samples a peak is looked for in, after a threshold crossing.
    lookahead: usize,
    /// For each channel, the end of the samples so far (`tail` long), then this pass's: `stride` apiece.
    samples: Vec<f32>,
    tail: usize,
    stride: usize,
    /// Each channel's noise (a standard deviation, µV), from the samples' median absolute value.
    noise: Vec<f32>,
    /// Room to find the median in.
    scratch: Vec<f32>,
    /// For each group, the first position (in `samples`) its next spike's peak can be at.
    next: Vec<i64>,
    /// While choosing thresholds from the data: seconds sampled, of how many; and each channel's noise summed over
    /// the passes so far, and how many.
    sampling: Option<(f64, f64, Vec<f64>, u32)>,
    shown: Latest<Option<(f32, f32)>>,
}

impl crate::core::engine::PassState for Detector {
    const WHY: &'static str = "Each channel's last samples (so a spike across two passes is found whole), its \
                               noise level (smoothed over passes), when each group can next spike (so a group's \
                               spikes don't overlap), and while choosing thresholds, the noise sampled so far.";
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
            samples: vec![0.0; channels * (tail + pass)],
            tail,
            stride: tail + pass,
            noise: Vec::new(),
            scratch: Vec::with_capacity(pass),
            next: vec![0; spikes.groups() as usize],
            ..Default::default()
        }
    }

    /// Makes room for passes of `n` samples (should one be longer than expected).
    fn fit(&mut self, n: usize, channels: usize) {
        if self.tail + n <= self.stride {
            return;
        }
        let stride = self.tail + n;
        let mut samples = vec![0.0; channels * stride];
        for channel in 0..channels {
            let tail = &self.samples[channel * self.stride..channel * self.stride + self.tail];
            samples[channel * stride..channel * stride + self.tail].copy_from_slice(tail);
        }
        (self.samples, self.stride) = (samples, stride);
    }
}

impl Node<Edges> for SpikeDetector {
    const INFO: NodeInfo = NodeInfo {
        name: "Spike Detector",
        category: Category::Processing,
        description: "Finds spikes on groups of channels (tetrodes, say): threshold crossings, with the waveform \
                      on each of the group's channels around the peak.",
    };

    type State = Detector;

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Detector {
        match inputs[0] {
            Some(DataStream::Continuous(c)) => Detector::new(self.stream(c.channels, c.sample_rate)),
            _ => Detector::default(),
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, detector: &mut Detector) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let Some(Buffer::Spikes(output)) = pass.outputs.get_mut(0) else { return };
        output.clear();
        output.pass_samples = input.len;
        let Some(spikes) = detector.spikes else { return };
        let channels = (spikes.channels as usize).min(input.channels);
        let n = input.len;
        detector.fit(n, channels);
        let (tail, stride) = (detector.tail, detector.stride);

        // This pass's samples after the tail, and each channel's noise (from the first pass with samples on).
        let first = detector.noise.is_empty();
        if first && n > 0 {
            detector.noise.resize(channels, 0.0);
        }
        for channel in 0..channels {
            output.thresholds[channel] = self.thresholds.get(channel);
        }
        if detector.noise.len() < channels {
            output.noise.fill(0.0);
            return;
        }
        for channel in 0..channels {
            let row = &mut detector.samples[channel * stride..(channel + 1) * stride];
            row[tail..tail + n].copy_from_slice(input.channel(channel));
            if n > 0 {
                detector.scratch.clear();
                detector.scratch.extend(input.channel(channel).iter().map(|x| x.abs()));
                let middle = n / 2;
                let median = *detector.scratch.select_nth_unstable_by(middle, |a, b| a.total_cmp(b)).1;
                let sd = median / 0.6745;
                let noise = &mut detector.noise[channel];
                *noise = if first { sd } else { *noise + 0.05 * (sd - *noise) };
            }
            output.noise[channel] = detector.noise[channel];
        }

        // Spikes peaking at positions from `pre` (the first not looked at last pass) on, whose waveform and
        // lookahead have all come.
        let (pre, post, lookahead) = (spikes.pre as usize, spikes.post as usize, detector.lookahead);
        let end = (pre + n) as i64;
        let extremity = |x: f32| self.polarity.extremity(x);
        let beyond = |x: f32, threshold: f32| extremity(x) > threshold;
        for group in 0..spikes.groups() {
            let group_channels = spikes.group_channels(group);
            let mut position = detector.next[group as usize].max(pre as i64);
            while position < end {
                let p = position as usize;
                let crossed = group_channels.clone().find(|&channel| {
                    let row = &detector.samples[channel as usize * stride..];
                    let threshold = output.thresholds[channel as usize];
                    beyond(row[p], threshold) && !beyond(row[p - 1], threshold)
                });
                let Some(channel) = crossed else {
                    position += 1;
                    continue;
                };
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
                position = (peak + pre + post) as i64;
            }
            detector.next[group as usize] = position - n as i64;
        }

        // Keep the end of the samples, for the next pass.
        for channel in 0..channels {
            let row = &mut detector.samples[channel * stride..(channel + 1) * stride];
            row.copy_within(n..n + tail, 0);
        }
        self.choose_thresholds(pass, detector);
    }

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Continuous(c)) => Some(DataStream::Spikes(self.stream(c.channels, c.sample_rate))),
            _ => None,
        };
        vec![Output { name: "spikes", kind: Kind::Spikes, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a (filtered) continuous data stream");
        if let Some(DataStream::Continuous(c)) = inputs[0]
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

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "thresholds", value: self.thresholds.clone().into() }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Thresholds(t) = value {
            self.thresholds = t;
        }
    }

    fn triggers(&self, _setting: usize) -> &'static [&'static str] {
        &[AUTO_DETECT]
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let (rate, channels) = match context.inputs[0] {
            Some(DataStream::Continuous(c)) => (c.sample_rate, c.channels as usize),
            _ => (30000.0, 0),
        };
        let ms = |samples: u32| format!(" samples ({:.2} ms)", samples as f32 * 1000.0 / rate);
        settings_grid(ui, |ui| {
            setting(ui, "Polarity", |ui| {
                combo(
                    ui,
                    "polarity",
                    &mut self.polarity,
                    &[(Polarity::Negative, "Negative"), (Polarity::Positive, "Positive"), (Polarity::Both, "Both")],
                )
            });
            setting(ui, "Group", |ui| {
                combo(
                    ui,
                    "group",
                    &mut self.group_size,
                    &[(1, "Single channels"), (2, "Stereotrodes (2)"), (4, "Tetrodes (4)"), (8, "Octrodes (8)")],
                )
            });
            let (pre, post) = (ms(self.pre), ms(self.post));
            setting(ui, "Before peak", |ui| ui.add(egui::DragValue::new(&mut self.pre).range(1..=64).suffix(pre)));
            setting(ui, "After peak", |ui| ui.add(egui::DragValue::new(&mut self.post).range(1..=128).suffix(post)));
            setting(ui, "Thresholds", |ui| ui.label(self.thresholds.summary()));
        });
        // Choosing the thresholds from the data (not while it's going).
        ui.add_space(8.0);
        ui.strong("Auto detect");
        ui.add_enabled_ui(!detecting(context), |ui| {
            named_settings_grid(ui, "auto detect", |ui| {
                setting(ui, "At", |ui| {
                    ui.add(egui::DragValue::new(&mut self.threshold_sd).range(1.0..=20.0).speed(0.05).suffix(" SD"))
                        .on_hover_text("Thresholds chosen from the data are this many standard deviations of the noise")
                });
                setting(ui, "Sampling for", |ui| {
                    ui.add(egui::DragValue::new(&mut self.sampling).range(SAMPLING).speed(0.2).suffix(" s"))
                        .on_hover_text("How long to sample the noise for")
                });
            });
        });
        let tip = format!(
            "Sample the noise, then set each channel's threshold to {} standard deviations of it",
            self.threshold_sd
        );
        auto_detect_ui(ui, context, None, &tip);
        if channels > 0 {
            self.channels_ui(ui, channels);
        }
        input_summary(ui, context.inputs);
    }
}

impl SpikeDetector {
    /// When triggered, samples each channel's noise for a while, then proposes thresholds from it.
    fn choose_thresholds(&self, pass: &Pass<Edges>, detector: &mut Detector) {
        let channels = detector.noise.len();
        if pass.triggered(AUTO_DETECT) {
            let seconds = self.sampling.clamp(*SAMPLING.start(), *SAMPLING.end()) as f64;
            detector.sampling = Some((0.0, seconds, vec![0.0; channels], 0));
        }
        if detector.sampling.is_some() && pass.trigger_cancelled(AUTO_DETECT) {
            detector.sampling = None;
        }
        if let Some((elapsed, seconds, sums, passes)) = &mut detector.sampling {
            for (sum, noise) in sums.iter_mut().zip(&detector.noise) {
                *sum += *noise as f64;
            }
            *passes += 1;
            *elapsed += pass.duration;
            pass.trigger_progress(AUTO_DETECT, (*elapsed / *seconds) as f32);
            if *elapsed >= *seconds {
                let mut thresholds = self.thresholds.clone();
                for (channel, sum) in sums.iter().enumerate() {
                    let noise = (sum / (*passes).max(1) as f64) as f32;
                    if noise > 0.0 {
                        thresholds.set(channel, (self.threshold_sd * noise).max(0.5));
                    }
                }
                pass.propose(0, Setting::Thresholds(thresholds));
                pass.trigger_done(AUTO_DETECT);
                detector.sampling = None;
            }
        }
        let progress = detector.sampling.as_ref().map(|s| (s.0 as f32, s.1 as f32));
        detector.shown.send(pass, |shown| *shown = progress);
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
    use crate::core::engine::{DataStreamType, Pass};
    use crate::standard_components::ephys::Continuous;

    /// Runs passes of `samples` through a detector, returning the spikes found (with their pass).
    fn detect(detector: &SpikeDetector, channels: u32, passes: &[Vec<Vec<f32>>]) -> Vec<(usize, Spike, Vec<f32>)> {
        let stream = Continuous { channels, sample_rate: 30000.0 };
        let mut state = detector.initial_state(&[Some(DataStream::Continuous(stream))]);
        let spikes = detector.stream(channels, 30000.0);
        let mut found = Vec::new();
        for (index, pass) in passes.iter().enumerate() {
            let mut input = stream.buffer(PASS_DURATION);
            input.len = pass[0].len();
            for (channel, samples) in pass.iter().enumerate() {
                input.channel_mut(channel).copy_from_slice(samples);
            }
            let input = Buffer::Continuous(input);
            let mut outputs = [Buffer::Spikes(spikes.buffer(PASS_DURATION))];
            let inputs = [Some(&input)];
            let mut pass = Pass::new(index as u64, PASS_DURATION, &inputs, &mut outputs);
            detector.process(&mut pass, &mut state);
            let Buffer::Spikes(output) = &outputs[0] else { unreachable!() };
            for (i, spike) in output.spikes.iter().enumerate() {
                found.push((index, *spike, output.waveform(i).to_vec()));
            }
        }
        found
    }

    /// Noise of about 10 µV on each channel.
    fn noise(channels: u32, samples: usize, seed: u64) -> Vec<Vec<f32>> {
        (0..channels as u64)
            .map(|c| {
                (0..samples).map(|s| 10.0 * crate::standard_components::preview::noise(seed + c, s as i64)).collect()
            })
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
    fn chooses_thresholds_from_the_noise_when_triggered() {
        let detector = SpikeDetector { sampling: 4.0, threshold_sd: 4.0, ..Default::default() };
        let stream = Continuous { channels: 4, sample_rate: 30000.0 };
        let mut state = detector.initial_state(&[Some(DataStream::Continuous(stream))]);
        let spikes = detector.stream(4, 30000.0);
        let exchange = Pass::<Edges>::new(0, PASS_DURATION, &[], &mut []).exchange;
        exchange.fire(crate::core::engine::NodeId(0), AUTO_DETECT);
        let passes = (4.0 / PASS_DURATION).round() as u64;
        for sequence in 0..passes {
            let mut input = stream.buffer(PASS_DURATION);
            input.len = 600;
            for (channel, samples) in noise(4, 600, sequence * 10).iter().enumerate() {
                input.channel_mut(channel).copy_from_slice(samples);
            }
            let input = Buffer::Continuous(input);
            let mut outputs = [Buffer::Spikes(spikes.buffer(PASS_DURATION))];
            let inputs = [Some(&input)];
            let mut pass = Pass::new(sequence, PASS_DURATION, &inputs, &mut outputs);
            pass.exchange = exchange;
            detector.process(&mut pass, &mut state);
        }
        let proposals = exchange.take_proposals();
        let Some(Setting::Thresholds(thresholds)) = proposals[0].2.downcast_ref::<Setting>() else { panic!() };
        for channel in 0..4 {
            let threshold = thresholds.get(channel);
            assert!((20.0..50.0).contains(&threshold), "4 SD of the noise (10 µV at most): {threshold}");
        }
    }
}
