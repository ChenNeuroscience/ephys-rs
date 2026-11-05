//! The Alarm Processor: measures spikes, LFP, positions and TTLs over each alarm's period, and says whether each is
//! within its range. An alarm that's off doesn't need its input (see `Node::needs`). Each measure's counted in bins, a
//! small fraction of the period each, so however long the period, a window is the same small size, and what's selected
//! can change without waiting for a new period. An alarm doesn't go off until it's seen a whole period.

use std::collections::VecDeque;

use eframe::egui;
use serde::{Deserialize, Serialize};

use super::settings::alarms_ui;
use super::{Alarm, AlarmState, AlarmStates, Alarms, Phase, Source};
use crate::core::engine::{Category, Context, Input, Node, NodeInfo, Output, Pass, PassState, SettingPort};
use crate::standard_components::widgets::require_input;
use crate::{Buffer, DataStream, Edges, Kind, Setting};

/// How many bins a period's cut into. At the shortest period, that's a bin a pass.
const BINS: usize = 50;

#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlarmProcessor {
    pub alarms: Alarms,
}

/// What came in during part of a period.
struct Bin {
    seconds: f64,
    /// For each group, channel or line.
    counts: Vec<u32>,
}

/// How much of something's been counted over the last period, for each group, channel or line.
struct Window {
    period: f64,
    bins: VecDeque<Bin>,
    /// Bins to use, so that passes don't allocate.
    spare: Vec<Bin>,
    /// The seconds and counts of all the bins.
    seconds: f64,
    totals: Vec<u64>,
}

impl Window {
    fn new(items: usize) -> Self {
        Window {
            period: 0.0,
            bins: VecDeque::with_capacity(BINS + 2),
            spare: (0..BINS + 2).map(|_| Bin { seconds: 0.0, counts: vec![0; items] }).collect(),
            seconds: 0.0,
            totals: vec![0; items],
        }
    }

    fn clear(&mut self) {
        self.spare.extend(self.bins.drain(..));
        self.seconds = 0.0;
        self.totals.fill(0);
    }

    /// Moves on `seconds`, dropping what's now older than `period` (and everything, if the period's changed).
    fn advance(&mut self, period: f64, seconds: f64) {
        if self.period != period {
            self.clear();
            self.period = period;
        }
        if self.bins.back().is_none_or(|bin| bin.seconds >= period / BINS as f64) {
            let mut bin = self.spare.pop().unwrap_or(Bin { seconds: 0.0, counts: vec![0; self.totals.len()] });
            bin.seconds = 0.0;
            bin.counts.fill(0);
            self.bins.push_back(bin);
        }
        if let Some(bin) = self.bins.back_mut() {
            bin.seconds += seconds;
        }
        self.seconds += seconds;
        while self.bins.front().is_some_and(|bin| self.seconds - bin.seconds >= period) {
            let Some(bin) = self.bins.pop_front() else { break };
            self.seconds -= bin.seconds;
            for (total, count) in self.totals.iter_mut().zip(&bin.counts) {
                *total -= *count as u64;
            }
            self.spare.push(bin);
        }
    }

    /// Adds `count` to an item, in the bin of now.
    fn count(&mut self, item: usize, count: u32) {
        if let (Some(total), Some(bin)) = (self.totals.get_mut(item), self.bins.back_mut()) {
            *total += count as u64;
            bin.counts[item] += count;
        }
    }

    /// Whether it's seen a whole period.
    fn full(&self) -> bool {
        self.seconds + 1e-9 >= self.period
    }
}

/// Each alarm's window, and the rates its counts are out of.
pub struct Watching {
    windows: [Window; 4],
    lfp_rate: f64,
    fps: f64,
}

impl Default for Watching {
    fn default() -> Self {
        Watching { windows: std::array::from_fn(|_| Window::new(0)), lfp_rate: 1.0, fps: 1.0 }
    }
}

impl PassState for Watching {
    const WHY: &'static str = "What's been counted over each alarm's period, which each pass adds to.";
}

/// What `source` measures of an item's `total`, over `seconds`.
fn measure(source: Source, total: u64, seconds: f64, lfp_rate: f64, fps: f64) -> f32 {
    let total = total as f64;
    let seconds = seconds.max(f64::EPSILON);
    let value = match source {
        Source::Spikes => total / seconds,
        Source::Lfp => (100.0 * total / (lfp_rate * seconds)).min(100.0),
        // Frames with an LED, of those there should have been: a camera that's stopped has no data either.
        Source::Positions => (100.0 * total / (fps * seconds)).min(100.0),
        Source::Ttl => total,
    };
    value as f32
}

/// How `alarm` stands, given what's counted.
fn judge(source: Source, alarm: &Alarm, window: &Window, lfp_rate: f64, fps: f64) -> AlarmState {
    if !window.full() {
        let waited = (window.seconds / window.period.max(f64::EPSILON)) as f32;
        return AlarmState { phase: Phase::Waiting(waited), ..Default::default() };
    }
    let selected = || (0..window.totals.len() as u32).filter(|item| alarm.selected(*item));
    let value = |total: u64| measure(source, total, window.seconds, lfp_rate, fps);
    let (value, item) = if alarm.together {
        let total: u64 = selected().map(|item| window.totals[item as usize]).sum();
        let count = selected().count();
        if count == 0 {
            return AlarmState::default();
        }
        // Rates and counts add up; fractions are of the time of each.
        let shared = if matches!(source, Source::Lfp) { total / count as u64 } else { total };
        (value(shared), None)
    } else {
        // How far an item's outside the range, or if it's inside, how near its edge.
        let outside = |value: f32| (alarm.low - value).max(value - alarm.high);
        let worst = selected()
            .map(|item| (value(window.totals[item as usize]), item))
            .max_by(|a, b| outside(a.0).total_cmp(&outside(b.0)));
        let Some((value, item)) = worst else { return AlarmState::default() };
        (value, Some(item))
    };
    let phase = if value < alarm.low || value > alarm.high { Phase::Alarm } else { Phase::Clear };
    AlarmState { phase, value, item }
}

/// Adds a pass's data to `window`.
fn count(alarms: &Alarms, input: &Buffer, window: &mut Window) {
    match input {
        Buffer::Spikes(spikes) => {
            for spike in &spikes.spikes {
                window.count(spike.group as usize, 1);
            }
        }
        Buffer::Continuous(continuous) => {
            let level = alarms.saturation_uv;
            for channel in 0..continuous.channels {
                let saturated = continuous.channel(channel).iter().filter(|x| x.abs() >= level).count();
                window.count(channel, saturated as u32);
            }
        }
        Buffer::Positions(positions) => {
            let found = positions.samples.iter().filter(|s| s.leds[0].is_some()).count();
            window.count(0, found as u32);
        }
        Buffer::Events(events) => {
            for event in events.events.iter().filter(|e| e.high) {
                window.count(event.line as usize, 1);
            }
        }
        _ => {}
    }
}

/// What the alarms have to choose from, given the processor's inputs.
pub fn streams(inputs: &[Option<DataStream>]) -> AlarmStates {
    let mut states = AlarmStates::default();
    for input in inputs {
        match input {
            Some(DataStream::Spikes(spikes)) => {
                (states.groups, states.group_size) = (spikes.groups(), spikes.group_size)
            }
            Some(DataStream::Continuous(continuous)) => states.channels = continuous.channels,
            Some(DataStream::Positions(_)) => states.positions = true,
            Some(DataStream::Events(events)) => states.lines = events.lines,
            _ => {}
        }
    }
    states
}

impl Node<Edges> for AlarmProcessor {
    type State = Watching;

    const INFO: NodeInfo = NodeInfo {
        name: "Alarm Processor",
        category: Category::Processing,
        description: "Watches spikes, LFP, positions and TTLs for rates and fractions outside their ranges, over a \
                      period chosen for each: for the Alarm Viewer, which shows them.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![
            Input { name: "spikes", kind: Kind::Spikes },
            Input { name: "lfp", kind: Kind::Continuous },
            Input { name: "positions", kind: Kind::Positions },
            Input { name: "ttl", kind: Kind::Events },
        ]
    }

    fn needs(&self, input: usize) -> bool {
        Source::ALL.get(input).is_some_and(|source| self.alarms.watching(*source))
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = Some(DataStream::AlarmStates(streams(inputs)));
        vec![Output { name: "alarms", kind: Kind::AlarmStates, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "something to watch")
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "alarms", value: Setting::Alarms(self.alarms.clone()) }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Alarms(alarms) = value {
            self.alarms = alarms;
        }
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Watching {
        let stream = streams(inputs);
        let lfp_rate = inputs.iter().find_map(|input| match input {
            Some(DataStream::Continuous(continuous)) => Some(continuous.sample_rate as f64),
            _ => None,
        });
        let fps = inputs.iter().find_map(|input| match input {
            Some(DataStream::Positions(positions)) => Some(positions.fps as f64),
            _ => None,
        });
        Watching {
            windows: Source::ALL.map(|source| Window::new(stream.items(source) as usize)),
            lfp_rate: lfp_rate.unwrap_or(1.0).max(1.0),
            fps: fps.unwrap_or(1.0).max(1.0),
        }
    }

    fn process(&self, pass: &mut Pass<Edges>, watching: &mut Watching) {
        let mut states = [AlarmState::default(); 4];
        let Watching { windows, lfp_rate, fps } = watching;
        for (index, source) in Source::ALL.into_iter().enumerate() {
            let alarm = self.alarms.alarm(source);
            let input = pass.inputs.get(index).copied().flatten().filter(|_| self.alarms.watching(source));
            let window = &mut windows[index];
            let Some(input) = input else {
                window.clear();
                continue;
            };
            window.advance(alarm.period(), pass.duration);
            count(&self.alarms, input, window);
            states[index] = judge(source, alarm, window, *lfp_rate, *fps);
        }
        if let Some(Buffer::AlarmStates(output)) = pass.outputs.first_mut() {
            output.states = states;
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        alarms_ui(ui, &mut self.alarms, streams(context.inputs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DataStreamType, PASS_DURATION};
    use crate::standard_components::ephys::{Continuous, Event, Events, EventsBuffer};
    use crate::standard_components::spikes::{Polarity, Spike, Spikes};
    use crate::standard_components::tracking::pipeline::Blob;
    use crate::standard_components::tracking::{PositionSample, Positions, PositionsBuffer};

    const SPIKES: Spikes =
        Spikes { channels: 8, sample_rate: 1000.0, group_size: 4, pre: 2, post: 2, polarity: Polarity::Negative };

    /// A processor of `alarms`, watching only what `inputs` has.
    struct Run {
        processor: AlarmProcessor,
        watching: Watching,
    }

    impl Run {
        fn new(alarms: Alarms, inputs: [Option<DataStream>; 4]) -> Self {
            let processor = AlarmProcessor { alarms };
            let watching = processor.initial_state(&inputs);
            Run { processor, watching }
        }

        /// Passes for `seconds`, each with `input` as input `index`; how the alarm stands after.
        fn run(&mut self, index: usize, input: &Buffer, seconds: f64) -> AlarmState {
            let mut outputs = [Buffer::AlarmStates(AlarmStates::default().buffer(PASS_DURATION))];
            for sequence in 0..(seconds / PASS_DURATION).round() as u64 {
                let mut inputs = [None; 4];
                inputs[index] = Some(input);
                let mut pass = Pass::new(sequence, PASS_DURATION, &inputs, &mut outputs);
                self.processor.process(&mut pass, &mut self.watching);
            }
            let Buffer::AlarmStates(output) = &outputs[0] else { unreachable!() };
            output.states[index]
        }
    }

    /// `per_pass` spikes on each group of `groups`.
    fn spikes(groups: &[u32], per_pass: usize) -> Buffer {
        let mut buffer = SPIKES.buffer(PASS_DURATION);
        for &group in groups {
            for _ in 0..per_pass {
                buffer.push(Spike { sample: 0, group, channel: group * 4 });
            }
        }
        Buffer::Spikes(buffer)
    }

    fn spike_input() -> [Option<DataStream>; 4] {
        [Some(DataStream::Spikes(SPIKES)), None, None, None]
    }

    #[test]
    fn nothing_goes_off_until_a_whole_period_is_seen() {
        let alarms = Alarms { spikes: Alarm { seconds: 2.0, ..Alarms::default().spikes }, ..Default::default() };
        let mut run = Run::new(alarms, spike_input());
        let silent = spikes(&[], 0);
        let state = run.run(0, &silent, 1.0);
        assert!(matches!(state.phase, Phase::Waiting(w) if (w - 0.5).abs() < 0.01), "{state:?}");
        assert_eq!(run.run(0, &silent, 1.0).phase, Phase::Alarm, "too few, once there's a period of it");
    }

    #[test]
    fn total_rate_is_of_the_groups_selected() {
        let spikes_alarm = Alarm { seconds: 1.0, low: 5.0, high: 60.0, together: true, ..Default::default() };
        let alarms = Alarms { spikes: spikes_alarm, ..Default::default() };
        let mut run = Run::new(alarms.clone(), spike_input());
        // A spike a pass on each of two groups: 50 Hz each.
        let state = run.run(0, &spikes(&[0, 1], 1), 2.0);
        assert_eq!((state.phase, state.value.round()), (Phase::Alarm, 100.0), "together they're too many");
        run.processor.alarms.spikes.left_out = vec![1];
        let state = run.run(0, &spikes(&[0, 1], 1), 0.02);
        assert_eq!((state.phase, state.value.round()), (Phase::Clear, 50.0), "at once, without a new period");
    }

    #[test]
    fn any_one_group_outside_the_range_sets_it_off() {
        let spikes_alarm = Alarm { seconds: 1.0, low: 10.0, high: 400.0, together: false, ..Default::default() };
        let alarms = Alarms { spikes: spikes_alarm, ..Default::default() };
        let mut run = Run::new(alarms, spike_input());
        let state = run.run(0, &spikes(&[0], 1), 2.0);
        assert_eq!((state.phase, state.item, state.value), (Phase::Alarm, Some(1), 0.0), "the second has none");
        assert_eq!(run.run(0, &spikes(&[0, 1], 1), 2.0).phase, Phase::Clear);
    }

    #[test]
    fn lfp_is_the_fraction_of_time_saturated() {
        let stream = Continuous { channels: 2, sample_rate: 1000.0 };
        let lfp = Alarm { seconds: 1.0, high: 10.0, together: false, ..Default::default() };
        let alarms = Alarms { lfp, saturation_uv: 100.0, ..Default::default() };
        let mut run = Run::new(alarms, [None, Some(DataStream::Continuous(stream)), None, None]);
        let mut buffer = stream.buffer(PASS_DURATION);
        buffer.len = 20;
        // A quarter of each pass on the second channel, either side of zero.
        for (i, x) in buffer.channel_mut(1).iter_mut().take(5).enumerate() {
            *x = if i % 2 == 0 { 100.0 } else { -150.0 };
        }
        let state = run.run(1, &Buffer::Continuous(buffer), 2.0);
        assert_eq!((state.phase, state.item, state.value.round()), (Phase::Alarm, Some(1), 25.0));
    }

    #[test]
    fn positions_are_the_fraction_of_samples_with_leds() {
        let stream = Positions { fps: 50.0, width: 100, height: 100 };
        let positions = Alarm { seconds: 1.0, low: 80.0, ..Default::default() };
        let alarms = Alarms { positions, ..Default::default() };
        let mut run = Run::new(alarms, [None, None, Some(DataStream::Positions(stream)), None]);
        let sample = |leds| PositionSample { frame: 0, time: 0.0, leds, timing_led: None, pixels_per_metre: f32::NAN };
        let found = Buffer::Positions(PositionsBuffer {
            samples: vec![sample([Some(Blob { x: 1.0, y: 1.0, area: 1 }), None])],
        });
        let state = run.run(2, &found, 2.0);
        assert_eq!((state.phase, state.value.round()), (Phase::Clear, 100.0));
        let lost = Buffer::Positions(PositionsBuffer { samples: vec![sample([None, None])] });
        let state = run.run(2, &lost, 2.0);
        assert_eq!((state.phase, state.value), (Phase::Alarm, 0.0), "frames, but no LED");
        let stopped = Buffer::Positions(PositionsBuffer { samples: Vec::new() });
        assert_eq!(run.run(2, &stopped, 2.0).value, 0.0, "no frames at all");
    }

    #[test]
    fn ttls_are_counted_by_their_ups_over_the_period() {
        let stream = Events { lines: 2, sample_rate: 1000.0 };
        let ttl = Alarm { seconds: 1.0, low: 40.0, high: 60.0, together: true, ..Default::default() };
        let alarms = Alarms { ttl, ..Default::default() };
        let mut run = Run::new(alarms, [None, None, None, Some(DataStream::Events(stream))]);
        let change = |high| Event { sample: 0, line: 0, high, time: None };
        let events = EventsBuffer { events: vec![change(true), change(false)], pass_samples: 20 };
        let state = run.run(3, &Buffer::Events(events), 2.0);
        assert_eq!((state.phase, state.value), (Phase::Clear, 50.0), "a pass's up, and not its down");
        run.processor.alarms.ttl.left_out = vec![0];
        let events = EventsBuffer { events: Vec::new(), pass_samples: 20 };
        assert_eq!(run.run(3, &Buffer::Events(events), 0.02).phase, Phase::Alarm, "the other line has none");
    }

    #[test]
    fn an_alarm_off_needs_no_input_and_starts_again_when_on() {
        let mut processor = AlarmProcessor::default();
        assert!((0..4).all(|input| processor.needs(input)));
        processor.alarms.spikes.on = false;
        assert!(!processor.needs(0) && processor.needs(1));
        processor.alarms.on = false;
        assert!((0..4).all(|input| !processor.needs(input)), "off altogether");
    }

    #[test]
    fn a_changed_period_starts_again() {
        let alarms = Alarms { spikes: Alarm { seconds: 1.0, ..Alarms::default().spikes }, ..Default::default() };
        let mut run = Run::new(alarms, spike_input());
        assert_eq!(run.run(0, &spikes(&[], 0), 1.0).phase, Phase::Alarm);
        run.processor.alarms.spikes.seconds = 2.0;
        assert!(matches!(run.run(0, &spikes(&[], 0), 1.0).phase, Phase::Waiting(_)));
    }

    /// All four connected, as in its window. See tests/snapshots/.
    #[test]
    fn settings_read_as_when_to_alarm() {
        use crate::standard_components::themes::THEMES;
        let stream = AlarmStates { groups: 8, group_size: 4, channels: 4, positions: true, lines: 1 };
        let mut harness = egui_kittest::Harness::builder()
            .with_size([460.0, 560.0])
            .wgpu()
            .build_ui_state(move |ui, alarms: &mut Alarms| alarms_ui(ui, alarms, stream), Alarms::default());
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("alarm_processor_settings");
    }

    #[test]
    fn a_long_run_keeps_to_the_bins_it_was_given() {
        let alarms = Alarms { spikes: Alarm { seconds: 1.0, ..Alarms::default().spikes }, ..Default::default() };
        let mut run = Run::new(alarms, spike_input());
        run.run(0, &spikes(&[0], 1), 30.0);
        let window = &run.watching.windows[0];
        assert!(window.bins.len() + window.spare.len() == BINS + 2, "none made, none lost");
        assert!((window.seconds - 1.0).abs() < 0.05, "{}", window.seconds);
    }
}
