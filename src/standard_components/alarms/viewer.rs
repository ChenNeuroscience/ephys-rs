//! The Alarm Viewer: how the alarms stand, prominently, with switches for them and for sound, in basic mode too. The
//! switches edit the Alarm Processor's settings through a control; the rest are in its own window, which it has a button
//! for, in basic mode too. The processor's data stays with it: this only gets the alarms' states.
//!
//! Switching an alarm off changes what the processor needs, and so the plan. While recording, the graph can't change, so
//! an alarm switched off is only ignored here (`Overrides`); that's saved in the settings once recording's stopped, if
//! the window's open by then. And an alarm off when recording started has no data coming, so can't come on until then.
//!
//! It can also pip as an alarm goes off, each at its own pitch (see `tone`), from the passes, so with its window closed
//! too.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};
use serde::{Deserialize, Serialize};

use super::tone::Tone;
use super::{Alarm, AlarmState, AlarmStates, Alarms, Phase, Source};
use crate::core::engine::{
    Category, Context, Control, Flow, Input, Node, NodeInfo, NodeThread, Output, Pass, PassState,
};
use crate::core::exchange::Shared;
use crate::core::theme::Colours;
use crate::standard_components::pool::{Pool, SharedPool};
use crate::standard_components::preview;
use crate::standard_components::widgets::require_input;
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Its one control: the processor's alarms.
const ALARMS: usize = 0;

#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlarmViewer {
    /// Whether an alarm going off pips. Off to begin with, as it'd be a surprise in a quiet room.
    pub sound: bool,
}

/// What's been switched off while recording, for the passes and the window to ignore.
#[derive(Default)]
pub struct Overrides {
    /// By `Source`.
    ignored: [AtomicBool; 4],
    all: AtomicBool,
    /// The sound, if it's been switched: 0 if it hasn't, 1 for off, 2 for on. It's the viewer's own setting, so can go
    /// either way.
    sound: AtomicU8,
}

impl Shared for Overrides {
    fn name(&self) -> &'static str {
        "alarms ignored"
    }
}

impl Overrides {
    fn ignores(&self, index: usize) -> bool {
        self.all.load(Ordering::Relaxed) || self.ignored[index].load(Ordering::Relaxed)
    }

    /// Whether there's sound, given the setting.
    fn sound(&self, setting: bool) -> bool {
        match self.sound.load(Ordering::Relaxed) {
            0 => setting,
            switched => switched == 2,
        }
    }

    /// Moves what's been switched into the settings, as recording's over.
    fn save(&self, alarms: &mut Alarms, sound: &mut bool) {
        *sound = self.sound(*sound);
        self.sound.store(0, Ordering::Relaxed);
        alarms.on &= !self.all.swap(false, Ordering::Relaxed);
        for (index, source) in Source::ALL.into_iter().enumerate() {
            alarms.alarm_mut(source).on &= !self.ignored[index].swap(false, Ordering::Relaxed);
        }
    }
}

/// What the passes keep to hand.
#[derive(Default)]
pub struct Showing {
    shown: SharedPool<AlarmsDisplay>,
    overrides: Option<Arc<Overrides>>,
    tone: Tone,
    /// Which alarms were going off as of the last pass.
    going_off: [bool; 4],
}

impl PassState for Showing {
    const WHY: &'static str = "The pool of states for its window, and what's being ignored.";
}

/// What's shown: how each alarm stands.
#[derive(Clone, Default)]
pub struct AlarmsDisplay {
    pub states: [AlarmState; 4],
}

impl Node<Edges> for AlarmViewer {
    type State = Showing;

    const INFO: NodeInfo = NodeInfo {
        name: "Alarm Viewer",
        category: Category::Visualisation,
        description: "How an Alarm Processor's alarms stand, with switches for them and for sound.",
    };
    const HAS_VIEW: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "alarms", kind: Kind::AlarmStates }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "alarms", kind: SettingKind::Alarms, read_only: false }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "an Alarm Processor")
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "Alarm tone",
            link: "flag",
            flow: Flow::FromPasses,
            why: "On a thread of its own so that opening the sound device, the first time an alarm sounds, can't hold \
                  up the passes. Only started if it's needed.",
            swaps: false,
        }]
    }

    fn process(&self, pass: &mut Pass<Edges>, showing: &mut Showing) {
        let Some(Some(Buffer::AlarmStates(input))) = pass.inputs.first() else { return };
        let overrides = showing.overrides.get_or_insert_with(|| pass.shared(Overrides::default));
        let mut states = input.states;
        for (index, state) in states.iter_mut().enumerate() {
            if overrides.ignores(index) {
                *state = AlarmState::default();
            }
        }
        let going_off = states.map(|state| state.phase == Phase::Alarm);
        for index in newly_off(showing.going_off, going_off) {
            if overrides.sound(self.sound) {
                showing.tone.pips(index);
            }
        }
        showing.going_off = going_off;
        showing.shown.send_newest(pass, |display| display.states = states);
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _context: &mut Context<Edges>) {
        ui.weak("The Alarm Processor has the alarms' settings.");
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let (Some(DataStream::AlarmStates(stream)), Some(Setting::Alarms(alarms))) =
            (context.inputs[0], context.controls[ALARMS].as_ref())
        else {
            ui.horizontal(|ui| context.node_toolbar(ui));
            return preview::unconnected(ui, context.theme, "Connect an Alarm Processor's alarms and its setting");
        };
        let mut alarms = alarms.clone();
        let overrides = context.shared::<Overrides>();
        let overrides = overrides.as_deref();
        let (locked, theme) = (context.locked, context.theme);
        if let (false, Some(overrides)) = (locked, overrides) {
            overrides.save(&mut alarms, &mut self.sound);
        }
        let pool = context.shared::<Pool<AlarmsDisplay>>();
        let display = pool.as_deref().and_then(Pool::newest).filter(|_| context.running);
        let states = display.as_ref().map(|d| &d.states);

        ui.horizontal(|ui| {
            switch(ui, &mut alarms.on, overrides.map(|o| &o.all), locked, "Alarms on");
            let mut sound = overrides.map_or(self.sound, |o| o.sound(self.sound));
            let checkbox = ui
                .checkbox(&mut sound, "Sound")
                .on_hover_text("A couple of pips as an alarm goes off, each at its own pitch");
            match (checkbox.changed(), overrides) {
                (true, Some(overrides)) if locked => overrides.sound.store(1 + sound as u8, Ordering::Relaxed),
                (true, _) => self.sound = sound,
                _ => {}
            }
            let button = ui.button("Alarm Processor").on_hover_text("Open it, for the alarms' settings");
            if button.clicked() {
                context.open_control_target(ALARMS);
            }
            context.node_toolbar(ui);
        });
        banner(ui, theme, &alarms, stream, states);
        ui.add_space(6.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (index, source) in Source::ALL.into_iter().enumerate() {
                let ignored = overrides.map(|o| &o.ignored[index]);
                let state = states.map(|states| states[index]);
                row(ui, theme, source, &mut alarms, stream, state, ignored, locked);
                ui.add_space(4.0);
            }
        });
        context.controls[ALARMS] = Some(Setting::Alarms(alarms));
        preview::repaint_with_data(ui);
    }
}

/// The alarms that have just gone off.
fn newly_off(before: [bool; 4], now: [bool; 4]) -> impl Iterator<Item = usize> {
    (0..4).filter(move |index| now[*index] && !before[*index])
}

/// A switch for a setting. While recording the setting's locked, so it only ignores what's coming in (`ignored`), and
/// if the setting's off, there's nothing coming in to go back to.
fn switch(ui: &mut egui::Ui, on: &mut bool, ignored: Option<&AtomicBool>, locked: bool, label: &str) -> egui::Response {
    if !locked {
        return ui.checkbox(on, label);
    }
    let mut shown = *on && !ignored.is_some_and(|i| i.load(Ordering::Relaxed));
    let checkbox = egui::Checkbox::new(&mut shown, label);
    let response = ui.add_enabled(*on && ignored.is_some(), checkbox).on_disabled_hover_text(
        "It was off when recording started, so its data isn't coming: it can come on once recording stops",
    );
    if let (true, Some(ignored)) = (response.changed(), ignored) {
        ignored.store(!shown, Ordering::Relaxed);
    }
    response
}

/// "tetrode 3", or nothing if it's of them all.
fn item_text(source: Source, stream: AlarmStates, item: Option<u32>) -> String {
    match item {
        Some(item) if source != Source::Positions => format!(" on {} {}", stream.item_name(source), item + 1),
        _ => String::new(),
    }
}

/// "5–400 Hz over 20 s".
fn range_text(source: Source, alarm: &Alarm) -> String {
    if source == Source::Lfp {
        return format!("Samples outside range above {}{} over {} s", alarm.high, source.unit(), alarm.period());
    }
    format!("{}–{}{} over {} s", alarm.low, alarm.high, source.unit(), alarm.period())
}

/// What's in the banner: its text, and its colours.
fn banner_text(
    theme: &Colours,
    alarms: &Alarms,
    stream: AlarmStates,
    states: Option<&[AlarmState; 4]>,
) -> (String, Color32, Color32) {
    let quiet = (theme.surface_raised, theme.text_secondary);
    if !alarms.on {
        return ("Alarms off".into(), quiet.0, quiet.1);
    }
    let Some(states) = states else {
        return ("Not running".into(), quiet.0, quiet.1);
    };
    let sounding: Vec<String> = Source::ALL
        .into_iter()
        .zip(states)
        .filter(|(_, state)| state.phase == Phase::Alarm)
        .map(|(source, state)| {
            format!("{}: {}{}", source.name(), source.value(state.value), item_text(source, stream, state.item))
        })
        .collect();
    if !sounding.is_empty() {
        return (format!("ALARM  {}", sounding.join(" · ")), theme.error, Color32::WHITE);
    }
    if states.iter().any(|state| state.phase == Phase::Clear) {
        ("All clear".into(), theme.success, Color32::WHITE)
    } else if states.iter().any(|state| matches!(state.phase, Phase::Waiting(_))) {
        ("Waiting for a whole period".into(), quiet.0, quiet.1)
    } else {
        ("Nothing watched".into(), quiet.0, quiet.1)
    }
}

/// The alarms at a glance, across the window: red, while any's going off.
fn banner(ui: &mut egui::Ui, theme: &Colours, alarms: &Alarms, stream: AlarmStates, states: Option<&[AlarmState; 4]>) {
    let (text, fill, ink) = banner_text(theme, alarms, stream, states);
    egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(12).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).size(22.0).strong().color(ink)).wrap());
    });
}

/// An alarm: its switch and how it stands, and what it's looking for.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    theme: &Colours,
    source: Source,
    alarms: &mut Alarms,
    stream: AlarmStates,
    state: Option<AlarmState>,
    ignored: Option<&AtomicBool>,
    locked: bool,
) {
    let connected = stream.items(source) > 0;
    let period = alarms.alarm(source).period();
    let (text, colour) = match state.map(|state| (state.phase, state)) {
        _ if !connected => ("Not connected".to_owned(), theme.text_disabled),
        None => (String::new(), theme.text_secondary),
        Some((Phase::Off, _)) => ("Off".to_owned(), theme.text_disabled),
        Some((Phase::Waiting(waited), _)) => {
            (format!("Waiting: {:.0} of {period} s", waited as f64 * period), theme.text_secondary)
        }
        Some((Phase::Clear, state)) => {
            (format!("{}{}", source.value(state.value), item_text(source, stream, state.item)), theme.success)
        }
        Some((Phase::Alarm, state)) => {
            (format!("{}{}", source.value(state.value), item_text(source, stream, state.item)), theme.error)
        }
    };
    let alarming = state.is_some_and(|state| state.phase == Phase::Alarm);
    let border = if alarming { Stroke::new(2.0, theme.error) } else { Stroke::new(1.0, theme.border) };
    egui::Frame::new()
        .fill(theme.surface_raised)
        .stroke(border)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let on = &mut alarms.alarm_mut(source).on;
                ui.add_enabled_ui(connected, |ui| switch(ui, on, ignored, locked, source.name()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(text).color(colour).strong());
                });
            });
            ui.weak(detail(source, alarms.alarm(source), stream));
        });
}

/// What an alarm's looking for, in a line.
fn detail(source: Source, alarm: &Alarm, stream: AlarmStates) -> String {
    let items = stream.items(source);
    let selected = (0..items).filter(|item| alarm.selected(*item)).count();
    let name = stream.item_name(source);
    let plural = if selected == 1 { "" } else { "s" };
    let what = match source {
        Source::Positions => {
            return format!("Samples with data below {}{} over {} s", alarm.low, source.unit(), alarm.period());
        }
        _ if alarm.together => format!("{selected} {name}{plural} together"),
        _ => format!("each of {selected} {name}{plural}"),
    };
    format!("{} · {what}", range_text(source, alarm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nodes;
    use crate::core::engine::{Graph, NodeSet, Port, Role};
    use crate::core::exchange::Exchange;
    use crate::standard_components::alarms::processor::AlarmProcessor;
    use crate::standard_components::themes::THEMES;
    use egui_kittest::kittest::Queryable;

    const CHECK: egui::accesskit::Role = egui::accesskit::Role::CheckBox;
    const STREAM: AlarmStates = AlarmStates { groups: 8, group_size: 4, channels: 4, positions: true, lines: 1 };

    /// The viewer's window, with `states` from a run, and connected to a processor of `alarms`.
    fn window(
        alarms: Alarms,
        states: [AlarmState; 4],
        overrides: Overrides,
        locked: bool,
        size: [f32; 2],
    ) -> (egui_kittest::Harness<'static, Graph<Nodes>>, Arc<Exchange>, crate::core::engine::NodeId) {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), [0.0, 0.0])
        };
        let (processor, viewer) = (add(&mut graph, "Alarm Processor"), add(&mut graph, "Alarm Viewer"));
        if let Some(Nodes::AlarmProcessor(node)) = graph.get_mut(processor).map(|e| &mut e.node) {
            *node = AlarmProcessor { alarms };
        }
        let (from, to) = (
            Port { node: viewer, role: Role::Control, index: 0 },
            Port { node: processor, role: Role::Setting, index: 0 },
        );
        graph.connect(from, to).unwrap();
        let exchange = Arc::new(Exchange::default());
        exchange.shared(viewer, || Pool::holding(AlarmsDisplay { states }));
        exchange.shared(viewer, || overrides);
        let shared = exchange.clone();
        let stream = Some(DataStream::AlarmStates(STREAM));
        let harness = egui_kittest::Harness::builder().with_size(size).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    viewer,
                    &[stream],
                    &THEMES[0].colours,
                    Some(&shared),
                    None,
                    true,
                    locked,
                    None,
                    |node, context| node.view_ui(ui, context),
                );
            },
            graph,
        );
        (harness, exchange, processor)
    }

    fn processor_alarms(graph: &Graph<Nodes>, id: crate::core::engine::NodeId) -> Alarms {
        match graph.get(id).map(|e| &e.node) {
            Some(Nodes::AlarmProcessor(node)) => node.alarms.clone(),
            _ => panic!("not a processor"),
        }
    }

    fn clear(value: f32) -> AlarmState {
        AlarmState { phase: Phase::Clear, value, item: None }
    }

    /// One going off, one fine, one waiting and one off. See tests/snapshots/.
    #[test]
    fn shows_alarms_prominently() {
        let alarm = AlarmState { phase: Phase::Alarm, value: 2.5, item: Some(2) };
        let states =
            [alarm, clear(0.2), AlarmState { phase: Phase::Waiting(0.5), ..Default::default() }, Default::default()];
        let (mut harness, ..) = window(Alarms::default(), states, Overrides::default(), false, [460.0, 420.0]);
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("alarms");
    }

    #[test]
    fn pips_are_only_for_going_off() {
        let newly = |before, now| newly_off(before, now).collect::<Vec<_>>();
        assert_eq!(newly([false; 4], [true, false, true, false]), vec![0, 2]);
        assert_eq!(newly([true, false, false, false], [true, false, false, false]), Vec::<usize>::new(), "still off");
        assert_eq!(newly([true, false, false, false], [false; 4]), Vec::<usize>::new(), "clear again");
    }

    #[test]
    fn the_switches_edit_the_processor() {
        let (mut harness, _, processor) =
            window(Alarms::default(), [clear(10.0); 4], Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(CHECK, "Spikes").click();
        harness.run_steps(2);
        let alarms = processor_alarms(harness.state(), processor);
        assert!(!alarms.spikes.on && alarms.lfp.on);
        harness.get_by_role_and_label(CHECK, "Alarms on").click();
        harness.run_steps(2);
        assert!(!processor_alarms(harness.state(), processor).on);
    }

    #[test]
    fn while_recording_a_switch_only_ignores_until_recording_stops() {
        let mut off = Alarms::default();
        off.ttl.on = false;
        let (mut harness, exchange, processor) =
            window(off, [clear(10.0); 4], Overrides::default(), true, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(CHECK, "Spikes").click();
        harness.get_by_role_and_label(CHECK, "TTLs").click();
        harness.run_steps(2);
        let alarms = processor_alarms(harness.state(), processor);
        assert!(alarms.spikes.on, "the graph's left alone");
        let viewer = crate::core::engine::NodeId(2);
        let overrides = exchange.shared(viewer, Overrides::default);
        assert!(overrides.ignores(0), "but it's ignored");
        assert!(!overrides.ignores(3) && !alarms.ttl.on, "one that's off has no data to come back to");
        harness.get_by_role_and_label(CHECK, "Sound").click();
        harness.run_steps(2);
        assert!(overrides.sound(false), "the sound can go either way");
        let (mut saved, mut sound) = (alarms.clone(), false);
        overrides.save(&mut saved, &mut sound);
        assert!(sound && !overrides.sound(false));
        assert!(!saved.spikes.on && saved.lfp.on && !overrides.ignores(0), "saved once recording's over");
    }

    /// A real run: the fake board's first line goes up twice a second, into a processor and on to the window's pool.
    #[test]
    fn a_run_gets_from_the_board_to_the_window() {
        use crate::core::engine::{Runner, Snapshot};
        use crate::standard_components::ephys::board::{Source as Board, serve_test_board};

        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), [0.0, 0.0])
        };
        let (board, processor, viewer) =
            (add(&mut graph, "Acquisition Board"), add(&mut graph, "Alarm Processor"), add(&mut graph, "Alarm Viewer"));
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Board::Fake { address: serve_test_board() };
        }
        let ttl = Alarm { seconds: 1.0, low: 1.0, high: 100.0, together: true, ..Default::default() };
        let alarms = Alarms { ttl, ..Default::default() };
        if let Some(Nodes::AlarmProcessor(node)) = graph.get_mut(processor).map(|e| &mut e.node) {
            node.alarms = alarms;
        }
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(board, Role::Output, 1), port(processor, Role::Input, 3)).unwrap();
        graph.connect(port(processor, Role::Output, 0), port(viewer, Role::Input, 0)).unwrap();

        let snapshot = Snapshot::new(0, graph, None);
        let runner = Runner::start(&snapshot, 2);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let pool = loop {
            if let Some(pool) = runner.exchange().get::<Pool<AlarmsDisplay>>(viewer) {
                break pool;
            }
            assert!(std::time::Instant::now() < deadline, "no pool");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        loop {
            let state = pool.newest().map(|display| display.states[3]);
            if let Some(AlarmState { phase: Phase::Clear, value, .. }) = state {
                assert!((1.0..=4.0).contains(&value), "two a second, more or less: {value}");
                break;
            }
            assert_ne!(state.map(|s| s.phase), Some(Phase::Alarm), "goes off though the ups are coming");
            assert!(std::time::Instant::now() < deadline, "never got there: {state:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(pool.newest().unwrap().states[0].phase, Phase::Off, "the spikes aren't connected");
    }

    #[test]
    fn ignored_alarms_are_not_passed_on() {
        let viewer = AlarmViewer::default();
        let mut showing = Showing::default();
        let input = Buffer::AlarmStates(super::super::AlarmStatesBuffer {
            states: [AlarmState { phase: Phase::Alarm, value: 1.0, item: None }; 4],
        });
        let inputs = [Some(&input)];
        let mut pass = Pass::new(0, 0.02, &inputs, &mut []);
        pass.shared(Overrides::default).ignored[1].store(true, Ordering::Relaxed);
        viewer.process(&mut pass, &mut showing);
        let pool = pass.shared(|| Pool::holding(AlarmsDisplay::default()));
        let display = pool.newest().unwrap();
        assert_eq!(display.states[0].phase, Phase::Alarm);
        assert_eq!(display.states[1].phase, Phase::Off);
    }
}
