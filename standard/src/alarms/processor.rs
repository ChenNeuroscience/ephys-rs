//! The Alarm Processor: measures spikes, LFP, positions and TTLs over each alarm's period, and says whether each is
//! within its range. An input's only needed while an alarm that's on watches it (see `Node::evaluate_tree_shake`). Each
//! measure's counted in bins, a small fraction of the period each, so however long the period, a window is the same
//! small size, and what's selected can change without waiting for a new period. An alarm doesn't go off until it's seen
//! a whole period.

use ephys_rs::engine::{Buffers, DataStreamInPorts, TreeShake};
use std::collections::VecDeque;

use eframe::egui;
use serde::{Deserialize, Serialize};

use super::settings::alarms_ui;
use super::{Alarm, AlarmKind, AlarmState, AlarmStates, Alarms, Phase, SetAlarms, Source};
use crate::Edges;
use crate::standard::ephys::{Continuous, Events};
use crate::standard::spikes::Spikes;
use crate::standard::tracking::Positions;
use crate::standard::widgets::require_input;
use ephys_rs::engine::{
    Category, CommandContext, CommandSource, Node, NodeInfo, PassContext, PassState, Prepare, PrepareContext, Prepared,
    UiContext,
};

ephys_rs::ports! {
    // An input for each of the alarms' sources, in the order of `Source::ALL`.
    spikes: DataStreamIn<Spikes>,
    lfp: DataStreamIn<Continuous>,
    positions: DataStreamIn<Positions>,
    ttl: DataStreamIn<Events>,
    alarms: DataStreamOut<AlarmStates>,
    alarms: ValOutCmdIn<Alarms, (SetAlarms,)>,
}

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
    /// Whether the line's up, for an alarm that goes by that: as of its last change, whatever the period.
    up: bool,
}

impl Window {
    fn new(items: usize) -> Self {
        Window {
            period: 0.0,
            bins: VecDeque::with_capacity(BINS + 2),
            spare: (0..BINS + 2).map(|_| Bin { seconds: 0.0, counts: vec![0; items] }).collect(),
            seconds: 0.0,
            totals: vec![0; items],
            up: false,
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

/// Each alarm's window (by id, in the alarms' order), and what its counts are out of.
pub struct Watching {
    windows: Vec<(u32, Window)>,
    stream: AlarmStates,
    lfp_rate: f64,
    fps: f64,
}

impl Default for Watching {
    fn default() -> Self {
        Watching { windows: Vec::new(), stream: AlarmStates::default(), lfp_rate: 1.0, fps: 1.0 }
    }
}

impl Watching {
    /// Keeps a window for each of `alarms`, those there already carrying on. Adding one needn't change the plan, so can
    /// happen in a run; it's only then that this allocates.
    fn keep_up(&mut self, alarms: &[Alarm]) {
        if self.windows.iter().map(|(id, _)| *id).eq(alarms.iter().map(|alarm| alarm.id)) {
            return;
        }
        let mut old = std::mem::take(&mut self.windows);
        for alarm in alarms {
            let window = match old.iter().position(|(id, _)| *id == alarm.id) {
                Some(index) => old.swap_remove(index).1,
                None => Window::new(self.stream.items(alarm.kind.source()) as usize),
            };
            self.windows.push((alarm.id, window));
        }
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
        // Those saturated are what's counted.
        Source::Lfp => 100.0 - (100.0 * total / (lfp_rate * seconds)).min(100.0),
        // Frames with an LED, of those there should have been: a camera that's stopped has no data either.
        Source::Positions => (100.0 * total / (fps * seconds)).min(100.0),
        Source::Ttl => total,
    };
    value as f32
}

/// How `alarm` stands, given what's counted.
fn judge(alarm: &Alarm, window: &Window, lfp_rate: f64, fps: f64) -> AlarmState {
    let source = alarm.kind.source();
    let off = AlarmState { id: alarm.id, source, ..Default::default() };
    if !alarm.kind.has_period() {
        let phase = if window.up { Phase::Alarm } else { Phase::Clear };
        return AlarmState { phase, value: window.up as u8 as f32, ..off };
    }
    if !window.full() {
        let waited = (window.seconds / window.period.max(f64::EPSILON)) as f32;
        return AlarmState { phase: Phase::Waiting(waited), ..off };
    }
    let selection = alarm.kind.selection();
    let selected = || (0..window.totals.len() as u32).filter(|item| selection.is_none_or(|s| s.selected(*item)));
    let value = |total: u64| measure(source, total, window.seconds, lfp_rate, fps);
    let (low, high) = alarm.kind.range();
    let (value, item) = if selection.is_some_and(|s| s.together) {
        let total: u64 = selected().map(|item| window.totals[item as usize]).sum();
        let count = selected().count();
        if count == 0 {
            return off;
        }
        // Rates and counts add up; fractions are of the time of each.
        let shared = if matches!(source, Source::Lfp) { total / count as u64 } else { total };
        (value(shared), None)
    } else {
        // How far an item's outside the range, or if it's inside, how near its edge.
        let outside = |value: f32| (low - value).max(value - high);
        let worst = selected()
            .map(|item| (value(window.totals[item as usize]), item))
            .max_by(|a, b| outside(a.0).total_cmp(&outside(b.0)));
        let Some((value, item)) = worst else { return off };
        (value, Some(item))
    };
    let phase = if value < low || value > high { Phase::Alarm } else { Phase::Clear };
    AlarmState { phase, value, item, ..off }
}

/// Whether the pass has data from the source an alarm of `kind` watches.
fn given(kind: &AlarmKind, inputs: &DataStreamIn<Buffers>) -> bool {
    match kind.source() {
        Source::Spikes => inputs.spikes.is_some(),
        Source::Lfp => inputs.lfp.is_some(),
        Source::Positions => inputs.positions.is_some(),
        Source::Ttl => inputs.ttl.is_some(),
    }
}

/// Adds a pass's data, from the source an alarm of `kind` watches, to `window`.
fn count(kind: &AlarmKind, inputs: &DataStreamIn<Buffers>, window: &mut Window) {
    match (kind, inputs) {
        (AlarmKind::SpikeRate { .. }, DataStreamIn { spikes: Some(spikes), .. }) => {
            for spike in &spikes.spikes {
                window.count(spike.group as usize, 1);
            }
        }
        (AlarmKind::LfpSaturation { level_uv, .. }, DataStreamIn { lfp: Some(continuous), .. }) => {
            for channel in 0..continuous.channels {
                let saturated = continuous.channel(channel).iter().filter(|x| x.abs() >= *level_uv).count();
                window.count(channel, saturated as u32);
            }
        }
        (AlarmKind::MissingPositions { .. }, DataStreamIn { positions: Some(positions), .. }) => {
            let found = positions.samples.iter().filter(|s| s.leds[0].is_some()).count();
            window.count(0, found as u32);
        }
        (AlarmKind::TtlCount { .. }, DataStreamIn { ttl: Some(events), .. }) => {
            for event in events.events.iter().filter(|e| e.high) {
                window.count(event.line as usize, 1);
            }
        }
        _ => {}
    }
}

/// What the alarms have to choose from, given the processor's inputs.
pub fn streams(inputs: &DataStreamIn) -> AlarmStates {
    let mut states = AlarmStates::default();
    if let Some(spikes) = inputs.spikes {
        (states.groups, states.group_size) = (spikes.groups(), spikes.group_size);
    }
    if let Some(continuous) = inputs.lfp {
        states.channels = continuous.channels;
    }
    states.positions = inputs.positions.is_some();
    if let Some(events) = inputs.ttl {
        states.lines = events.lines;
    }
    states
}

impl AlarmProcessor {
    /// Whether an alarm that's on watches an input.
    fn needs(&self, input: usize) -> bool {
        Source::ALL.get(input).is_some_and(|source| self.alarms.watches(*source))
    }
}

impl Node<Edges> for AlarmProcessor {
    type Ports = Ports;
    type State = Watching;
    type WorkerState = ();
    type Shared = ();
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Alarm Processor",
        category: Category::Processing,
        description: "Alarms you add, watching spikes, LFP, positions or TTLs for what's outside a range, over a period \
                      chosen for each: for the Alarm Viewer, which shows them.",
    };

    /// Only the inputs an alarm that's on watches; and only while it's given one of those, and something needs what
    /// it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], _recording: bool) -> TreeShake {
        let needs: Vec<bool> = (0..inputs.len()).map(|input| self.needs(input)).collect();
        let given = inputs.iter().zip(&needs).any(|(&used, &needed)| used && needed);
        TreeShake { runs: given && outputs.contains(&true), needs }
    }

    fn output_streams(&self, inputs: &DataStreamIn) -> DataStreamOut {
        DataStreamOut { alarms: Some(streams(inputs)) }
    }

    fn val_out(&self) -> ValOutCmdIn {
        ValOutCmdIn { alarms: self.alarms.clone() }
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<Edges>) {
        match command {
            CmdIn::SetAlarms(SetAlarms(alarms)) => self.alarms = alarms,
        }
    }

    fn prepare_for_snapshot(&self, context: &PrepareContext<Edges, Self>) -> Prepare<Watching, (), Self> {
        if context.same_streams() {
            return Prepared::nothing().into();
        }
        let inputs = DataStreamIn::of(context.inputs);
        let stream = streams(&inputs);
        let lfp_rate = inputs.lfp.map(|continuous| continuous.sample_rate as f64);
        let fps = inputs.positions.map(|positions| positions.fps as f64);
        let mut watching = Watching {
            windows: Vec::new(),
            stream,
            lfp_rate: lfp_rate.unwrap_or(1.0).max(1.0),
            fps: fps.unwrap_or(1.0).max(1.0),
        };
        watching.keep_up(&self.alarms.list);
        Prepared::shared(watching).into()
    }

    fn process(
        &self,
        pass: &mut PassContext<Edges>,
        inputs: DataStreamIn<Buffers>,
        outputs: DataStreamOut<Buffers>,
        watching: &mut Watching,
        _: &mut (),
    ) {
        watching.keep_up(&self.alarms.list);
        let output = outputs.alarms;
        output.states.clear();
        for (alarm, (_, window)) in self.alarms.list.iter().zip(&mut watching.windows) {
            let source = alarm.kind.source();
            // Whether it's on or not: the line stays where it is, and it's only its changes that come.
            if let (AlarmKind::TtlUp { line }, Some(events)) = (&alarm.kind, inputs.ttl) {
                let mut changes = events.events.iter().filter(|event| event.line as u32 == *line);
                window.up = changes.next_back().map_or(window.up, |event| event.high);
            }
            if !(alarm.on && given(&alarm.kind, &inputs)) {
                window.clear();
                output.states.push(AlarmState { id: alarm.id, source, ..Default::default() });
                continue;
            }
            window.advance(alarm.period(), pass.duration);
            count(&alarm.kind, &inputs, window);
            output.states.push(judge(alarm, window, watching.lfp_rate, watching.fps));
        }
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<Edges>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            alarms_ui(ui, &mut node.alarms, streams(&DataStreamIn::of(context.inputs)));
        });
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        require_input(inputs, "something to watch")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::alarms::Selection;
    use crate::standard::ephys::{ContinuousBuffer, Event, EventsBuffer};
    use crate::standard::spikes::{Spike, SpikesBuffer};
    use crate::standard::tracking::pipeline::Blob;
    use crate::standard::tracking::{PositionSample, PositionsBuffer};
    use Input::{Lfp, Ttl};
    use ephys_rs::engine::{DataStreamType, PASS_DURATION};

    const SPIKES: Spikes = Spikes { channels: 8, sample_rate: 1000.0, group_size: 4, pre: 2, post: 2 };

    /// A processor of `alarms`, watching only what `inputs` has.
    struct Run {
        processor: AlarmProcessor,
        watching: Watching,
    }

    /// A pass's data on one of the processor's inputs.
    #[derive(Clone, Copy)]
    enum Input<'a> {
        Spikes(&'a SpikesBuffer),
        Lfp(&'a ContinuousBuffer),
        Positions(&'a PositionsBuffer),
        Ttl(&'a EventsBuffer),
    }

    impl<'a> Input<'a> {
        /// The processor's inputs, with this on its own.
        fn alone(self) -> DataStreamIn<Buffers<'a>> {
            let mut inputs = DataStreamIn { spikes: None, lfp: None, positions: None, ttl: None };
            match self {
                Input::Spikes(spikes) => inputs.spikes = Some(spikes),
                Input::Lfp(lfp) => inputs.lfp = Some(lfp),
                Input::Positions(positions) => inputs.positions = Some(positions),
                Input::Ttl(ttl) => inputs.ttl = Some(ttl),
            }
            inputs
        }
    }

    impl Run {
        fn new(alarms: Alarms, inputs: DataStreamIn) -> Self {
            let processor = AlarmProcessor { alarms };
            let watching = ephys_rs::engine::fresh_state(&processor, &inputs);
            Run { processor, watching }
        }

        /// Passes for `seconds`, each with `input`; how the alarms stand after.
        fn all(&mut self, input: Input, seconds: f64) -> Vec<AlarmState> {
            let mut output = AlarmStates::default().buffer(PASS_DURATION);
            for sequence in 0..(seconds / PASS_DURATION).round() as u64 {
                let mut pass = PassContext::new(sequence, PASS_DURATION);
                let outputs = DataStreamOut { alarms: &mut output };
                self.processor.process(&mut pass, input.alone(), outputs, &mut self.watching, &mut ());
            }
            output.states.clone()
        }

        /// The same, for the first alarm.
        fn run(&mut self, input: Input, seconds: f64) -> AlarmState {
            self.all(input, seconds)[0]
        }

        fn alarm(&mut self) -> &mut Alarm {
            &mut self.processor.alarms.list[0]
        }

        fn left_out(&mut self, left_out: Vec<u32>) {
            let (AlarmKind::SpikeRate { selection, .. } | AlarmKind::TtlCount { selection, .. }) =
                &mut self.alarm().kind
            else {
                panic!("nothing to select");
            };
            selection.left_out = left_out;
        }
    }

    /// Just an alarm of `kind`, over `seconds`.
    fn one(kind: AlarmKind, seconds: f32) -> Alarms {
        let mut alarms = Alarms::default();
        alarms.add(kind).seconds = seconds;
        alarms
    }

    fn spike_rate(low: f32, high: f32, together: bool) -> AlarmKind {
        AlarmKind::SpikeRate { low, high, selection: Selection { together, ..Default::default() } }
    }

    /// `per_pass` spikes on each group of `groups`.
    fn spikes(groups: &[u32], per_pass: usize) -> SpikesBuffer {
        let mut buffer = SPIKES.buffer(PASS_DURATION);
        for &group in groups {
            for _ in 0..per_pass {
                buffer.push(Spike { sample: 0, group, channel: group * 4 });
            }
        }
        buffer
    }

    fn spike_input() -> DataStreamIn {
        DataStreamIn { spikes: Some(SPIKES), ..Default::default() }
    }

    #[test]
    fn nothing_goes_off_until_a_whole_period_is_seen() {
        let mut run = Run::new(one(spike_rate(5.0, 400.0, true), 2.0), spike_input());
        let silent = spikes(&[], 0);
        let state = run.run(Input::Spikes(&silent), 1.0);
        assert!(matches!(state.phase, Phase::Waiting(w) if (w - 0.5).abs() < 0.01), "{state:?}");
        assert_eq!(run.run(Input::Spikes(&silent), 1.0).phase, Phase::Alarm, "too few, once there's a period of it");
    }

    #[test]
    fn total_rate_is_of_the_groups_selected() {
        let mut run = Run::new(one(spike_rate(5.0, 60.0, true), 1.0), spike_input());
        // A spike a pass on each of two groups: 50 Hz each.
        let state = run.run(Input::Spikes(&spikes(&[0, 1], 1)), 2.0);
        assert_eq!((state.phase, state.value.round()), (Phase::Alarm, 100.0), "together they're too many");
        run.left_out(vec![1]);
        let state = run.run(Input::Spikes(&spikes(&[0, 1], 1)), 0.02);
        assert_eq!((state.phase, state.value.round()), (Phase::Clear, 50.0), "at once, without a new period");
    }

    #[test]
    fn any_one_group_outside_the_range_sets_it_off() {
        let mut run = Run::new(one(spike_rate(10.0, 400.0, false), 1.0), spike_input());
        let state = run.run(Input::Spikes(&spikes(&[0], 1)), 2.0);
        assert_eq!((state.phase, state.item, state.value), (Phase::Alarm, Some(1), 0.0), "the second has none");
        assert_eq!(run.run(Input::Spikes(&spikes(&[0, 1], 1)), 2.0).phase, Phase::Clear);
    }

    #[test]
    fn lfp_is_the_fraction_of_time_in_range() {
        let stream = Continuous { channels: 2, sample_rate: 1000.0 };
        let kind = AlarmKind::LfpSaturation { level_uv: 100.0, low: 90.0, selection: Selection::default() };
        let mut run = Run::new(one(kind, 1.0), DataStreamIn { lfp: Some(stream), ..Default::default() });
        let mut buffer = stream.buffer(PASS_DURATION);
        buffer.len = 20;
        // A quarter of each pass on the second channel, either side of zero.
        for (i, x) in buffer.channel_mut(1).iter_mut().take(5).enumerate() {
            *x = if i % 2 == 0 { 100.0 } else { -150.0 };
        }
        let state = run.run(Lfp(&buffer), 2.0);
        assert_eq!((state.phase, state.item, state.value.round()), (Phase::Alarm, Some(1), 75.0));
    }

    #[test]
    fn positions_are_the_fraction_of_samples_with_leds() {
        let stream = Positions { fps: 50.0, width: 100, height: 100, source: "test" };
        let alarms = one(AlarmKind::MissingPositions { low: 80.0 }, 1.0);
        let mut run = Run::new(alarms, DataStreamIn { positions: Some(stream), ..Default::default() });
        let sample = |leds| PositionSample { frame: 0, time: 0.0, leds, timing_led: None, pixels_per_metre: f32::NAN };
        let found = PositionsBuffer { samples: vec![sample([Some(Blob { x: 1.0, y: 1.0, area: 1 }), None])] };
        let state = run.run(Input::Positions(&found), 2.0);
        assert_eq!((state.phase, state.value.round()), (Phase::Clear, 100.0));
        let lost = PositionsBuffer { samples: vec![sample([None, None])] };
        let state = run.run(Input::Positions(&lost), 2.0);
        assert_eq!((state.phase, state.value), (Phase::Alarm, 0.0), "frames, but no LED");
        let stopped = PositionsBuffer { samples: Vec::new() };
        assert_eq!(run.run(Input::Positions(&stopped), 2.0).value, 0.0, "no frames at all");
    }

    #[test]
    fn ttls_are_counted_by_their_ups_over_the_period() {
        let stream = Events { lines: 2, sample_rate: 1000.0 };
        let selection = Selection { together: true, ..Default::default() };
        let alarms = one(AlarmKind::TtlCount { low: 40.0, high: 60.0, selection }, 1.0);
        let mut run = Run::new(alarms, DataStreamIn { ttl: Some(stream), ..Default::default() });
        let change = |high| Event { sample: 0, line: 0, high, time: None };
        let events = EventsBuffer { events: vec![change(true), change(false)], pass_samples: 20 };
        let state = run.run(Ttl(&events), 2.0);
        assert_eq!((state.phase, state.value), (Phase::Clear, 50.0), "a pass's up, and not its down");
        run.left_out(vec![0]);
        let events = EventsBuffer { events: Vec::new(), pass_samples: 20 };
        assert_eq!(run.run(Ttl(&events), 0.02).phase, Phase::Alarm, "the other line has none");
    }

    #[test]
    fn a_ttl_up_alarm_follows_its_line_at_once() {
        let stream = Events { lines: 2, sample_rate: 1000.0 };
        let mut run =
            Run::new(one(AlarmKind::TtlUp { line: 1 }, 1.0), DataStreamIn { ttl: Some(stream), ..Default::default() });
        let pass = |events: Vec<Event>| EventsBuffer { events, pass_samples: 20 };
        let change = |line, high| Event { sample: 0, line, high, time: None };
        assert_eq!(run.run(Ttl(&pass(Vec::new())), 0.02).phase, Phase::Clear, "with no period to wait for");
        assert_eq!(run.run(Ttl(&pass(vec![change(0, true)])), 0.02).phase, Phase::Clear, "the other line");
        let state = run.run(Ttl(&pass(vec![change(1, true)])), 0.02);
        assert_eq!((state.phase, state.value), (Phase::Alarm, 1.0));
        assert_eq!(run.run(Ttl(&pass(Vec::new())), 5.0).phase, Phase::Alarm, "for as long as it's up");

        // It goes down while the alarm's off, which it still sees.
        run.alarm().on = false;
        assert_eq!(run.run(Ttl(&pass(vec![change(1, false)])), 0.02).phase, Phase::Off);
        run.alarm().on = true;
        assert_eq!(run.run(Ttl(&pass(Vec::new())), 0.02).phase, Phase::Clear);

        let pulse = pass(vec![change(1, true), change(1, false)]);
        assert_eq!(run.run(Ttl(&pulse), 0.02).phase, Phase::Clear, "the last change in a pass is where it's left");
    }

    #[test]
    fn only_inputs_with_an_alarm_on_are_needed() {
        let mut processor = AlarmProcessor::default();
        assert!((0..4).all(|input| !processor.needs(input)), "none to begin with");
        let first = processor.alarms.add(spike_rate(5.0, 400.0, true)).id;
        let second = processor.alarms.add(spike_rate(0.0, 100.0, false)).id;
        assert!(processor.needs(0) && !processor.needs(1));
        processor.alarms.list[0].on = false;
        assert!(processor.needs(0), "the other's still on");
        processor.alarms.remove(second);
        assert!(!processor.needs(0), "and now there's only one, that's off");
        processor.alarms.list[0].on = true;
        assert!(processor.needs(0));
        assert!(processor.alarms.get(first).is_some());
    }

    #[test]
    fn a_changed_period_starts_again() {
        let mut run = Run::new(one(spike_rate(5.0, 400.0, true), 1.0), spike_input());
        assert_eq!(run.run(Input::Spikes(&spikes(&[], 0)), 1.0).phase, Phase::Alarm);
        run.alarm().seconds = 2.0;
        assert!(matches!(run.run(Input::Spikes(&spikes(&[], 0)), 1.0).phase, Phase::Waiting(_)));
    }

    #[test]
    fn alarms_can_come_and_go_in_a_run_and_the_rest_carry_on() {
        let mut run = Run::new(one(spike_rate(5.0, 400.0, true), 1.0), spike_input());
        let silent = spikes(&[], 0);
        assert_eq!(run.run(Input::Spikes(&silent), 1.0).phase, Phase::Alarm);

        // Two of a kind, each with its own settings.
        let added = run.processor.alarms.add(spike_rate(0.0, 400.0, true));
        added.seconds = 1.0;
        let added = added.id;
        let states = run.all(Input::Spikes(&silent), 0.5);
        assert_eq!(states[0].phase, Phase::Alarm, "without starting again");
        assert!(matches!(states[1].phase, Phase::Waiting(_)));
        assert_eq!(states[1].id, added);
        assert_eq!(run.all(Input::Spikes(&silent), 0.5)[1].phase, Phase::Clear);

        // The first goes, the second keeps what it's counted, and its id is its own.
        let first = run.processor.alarms.list[0].id;
        run.processor.alarms.remove(first);
        let states = run.all(Input::Spikes(&silent), 0.02);
        assert_eq!((states.len(), states[0].id, states[0].phase), (1, added, Phase::Clear));
        assert_ne!(run.processor.alarms.add(spike_rate(0.0, 1.0, true)).id, first, "ids aren't reused");
    }

    #[test]
    fn an_alarm_off_stays_on_the_list_with_nothing_to_say() {
        let mut run = Run::new(one(spike_rate(5.0, 400.0, true), 1.0), spike_input());
        assert_eq!(run.run(Input::Spikes(&spikes(&[], 0)), 1.0).phase, Phase::Alarm);
        run.alarm().on = false;
        let state = run.run(Input::Spikes(&spikes(&[], 0)), 0.02);
        assert_eq!((state.id, state.source, state.phase), (0, Source::Spikes, Phase::Off));
        run.alarm().on = true;
        assert!(
            matches!(run.run(Input::Spikes(&spikes(&[], 0)), 0.02).phase, Phase::Waiting(_)),
            "from the start of a period"
        );
    }

    #[test]
    fn a_file_from_before_alarms_were_added_one_at_a_time_opens_with_none() {
        let old = serde_json::json!({
            "on": true,
            "spikes": { "on": true, "seconds": 20.0, "low": 5.0, "high": 400.0, "left_out": [], "together": true },
            "saturation_uv": 6300.0,
        });
        let alarms: Alarms = serde_json::from_value(old).unwrap();
        assert!(alarms.list.is_empty());
    }

    #[test]
    fn saved_alarms_keep_their_settings_and_their_ids_stay_unique() {
        let mut alarms = Alarms::default();
        for kind in AlarmKind::library() {
            alarms.add(kind);
        }
        alarms.list[1].on = false;
        let saved = serde_json::to_string(&alarms).unwrap();
        let mut opened: Alarms = serde_json::from_str(&saved).unwrap();
        assert_eq!(opened, alarms);
        assert_eq!(opened.add(AlarmKind::MissingPositions { low: 1.0 }).id, 5);
    }

    const CONNECTED: AlarmStates = AlarmStates { groups: 16, group_size: 4, channels: 64, positions: true, lines: 2 };

    fn settings_window(alarms: Alarms, stream: AlarmStates) -> egui_kittest::Harness<'static, Alarms> {
        use crate::standard::themes::THEMES;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([460.0, 900.0])
            .wgpu()
            .build_ui_state(move |ui, alarms: &mut Alarms| alarms_ui(ui, alarms, stream), alarms);
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness
    }

    /// One of each, all connected, as in its window. See tests/snapshots/.
    #[test]
    fn settings_read_as_when_to_alarm() {
        let mut alarms = Alarms::default();
        for kind in AlarmKind::library() {
            alarms.add(kind);
        }
        settings_window(alarms, CONNECTED).snapshot("alarm_processor_settings");
    }

    #[test]
    fn alarms_are_added_from_the_library_and_can_be_switched_off_or_removed() {
        use egui_kittest::kittest::Queryable;
        let mut harness = settings_window(Alarms::default(), CONNECTED);
        harness.get_by_label("No alarms yet");
        harness.get_by_label("Add alarm").click();
        harness.run_steps(3);
        harness.snapshot("alarm_processor_adding");
        harness.get_by_label("Position data available").click();
        harness.run_steps(3);
        harness.get_by_label("Add alarm").click();
        harness.run_steps(3);
        harness.get_by_label("TTL count in range").click();
        harness.run_steps(3);
        let names =
            |h: &egui_kittest::Harness<Alarms>| h.state().list.iter().map(|a| a.kind.name()).collect::<Vec<_>>();
        assert_eq!(names(&harness), vec!["Position data available", "TTL count in range"]);

        harness.get_by_role_and_label(egui::accesskit::Role::Button, "TTL count in range").click();
        harness.run_steps(3);
        assert!(!harness.state().list[1].on, "off, and still there");

        harness.get_all_by_label("Delete").next().unwrap().click();
        harness.run_steps(3);
        assert_eq!(names(&harness), vec!["TTL count in range"]);
    }

    #[test]
    fn the_buttons_for_all_set_each_alarm() {
        use egui_kittest::kittest::Queryable;
        let button = egui::accesskit::Role::Button;
        let mut alarms = Alarms::default();
        alarms.add(spike_rate(5.0, 400.0, true));
        alarms.add(AlarmKind::MissingPositions { low: 80.0 }).on = false;
        let mut harness = settings_window(alarms, CONNECTED);
        let on = |h: &egui_kittest::Harness<Alarms>| h.state().list.iter().map(|a| a.on).collect::<Vec<_>>();

        harness.get_by_role_and_label(button, "All alarms").click();
        harness.run_steps(3);
        assert_eq!(on(&harness), vec![true, true], "some off, so it's all on");
        harness.get_by_role_and_label(button, "All alarms").click();
        harness.run_steps(3);
        assert_eq!(on(&harness), vec![false, false], "and all off");

        // A list's own checkbox for all of it: a dash, for some.
        let mut selection = Selection { left_out: vec![1], together: false };
        let mut harness2 = egui_kittest::Harness::builder().with_size([300.0, 60.0]).wgpu().build_ui_state(
            |ui, selection: &mut Selection| {
                crate::standard::widgets::settings_grid(ui, |ui| {
                    super::super::settings::selection_ui(ui, selection, 3, "line");
                });
            },
            std::mem::take(&mut selection),
        );
        harness2.run_steps(2);
        harness2.get_all_by_role(egui::accesskit::Role::CheckBox).next().unwrap().click();
        harness2.run_steps(2);
        assert!(harness2.state().left_out.is_empty(), "some, so it's all");
        harness2.get_all_by_role(egui::accesskit::Role::CheckBox).next().unwrap().click();
        harness2.run_steps(2);
        assert_eq!(harness2.state().left_out, vec![0, 1, 2], "then none");

        harness.get_by_label("Delete all").click();
        harness.run_steps(3);
        assert!(harness.state().list.is_empty());
        harness.get_by_label("No alarms yet");
    }

    #[test]
    fn only_kinds_with_their_input_connected_are_offered() {
        use egui_kittest::kittest::Queryable;
        let ttl_only = AlarmStates { lines: 2, ..Default::default() };
        let mut harness = settings_window(Alarms::default(), ttl_only);
        harness.get_by_label("Add alarm").click();
        harness.run_steps(3);
        assert!(
            harness.query_by_label("TTL count in range").is_some() && harness.query_by_label("TTL line down").is_some()
        );
        assert!(harness.query_by_label("Spike rate in range").is_none(), "nothing's connected for it");
        assert!(harness.query_by_label("Position data available").is_none());

        let mut nothing = settings_window(Alarms::default(), AlarmStates::default());
        nothing.get_by_label("Add alarm").click();
        nothing.run_steps(3);
        assert!(nothing.query_by_label("TTL count in range").is_none(), "it can't be opened");

        // One already there stays, for when it's connected again.
        let mut alarms = Alarms::default();
        alarms.add(spike_rate(5.0, 400.0, true));
        let harness = settings_window(alarms, ttl_only);
        harness.get_by_label("Not connected");
    }

    #[test]
    fn a_long_run_keeps_to_the_bins_it_was_given() {
        let mut run = Run::new(one(spike_rate(5.0, 400.0, true), 1.0), spike_input());
        run.run(Input::Spikes(&spikes(&[0], 1)), 30.0);
        let window = &run.watching.windows[0].1;
        assert!(window.bins.len() + window.spare.len() == BINS + 2, "none made, none lost");
        assert!((window.seconds - 1.0).abs() < 0.05, "{}", window.seconds);
    }
}
