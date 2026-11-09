//! The Alarm Viewer: how the alarms stand, prominently, with switches for them and for sound, in basic mode too. The
//! switches edit the Alarm Processor's settings through a control; adding, removing and the rest are in its own window,
//! which it has a button for, in basic mode too. The processor's data stays with it: this only gets the alarms' states.
//!
//! Its passes send every pass's alarm states to the GUI's side (`Update`), where its work on the GUI's thread (see
//! `Node::gui_background`) keeps track of them, whether its window's open or not, for the window to show.
//!
//! Switching an alarm off changes what the processor needs, and so the plan. While recording, the graph can't change, so
//! an alarm switched off is only ignored here (`Overrides`); that's saved in the settings once recording's stopped. And an
//! alarm off when recording started has no data coming, so can't come on until then.
//!
//! It can also pip as an alarm goes off, at a pitch for what it watches (see `tone`), on the GUI's side, so with its
//! window closed (or the app minimised) too, but only while recording, as what's wrong before then needn't fill the room. An alarm still going off pips
//! again, more each time (`REMINDERS`), in case the first were missed, and one going off as recording starts pips then. Which
//! alarms pip is its own setting, not the processor's. That's locked while recording too, but as nothing depends on it, it
//! can go either way (`Overrides` again).
//!
//! The buttons for all the alarms are only shortcuts: they set each one.

use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};
use serde::{Deserialize, Serialize};

use super::settings::{BUTTON_GAP, all_power, title};
use super::tone::Tone;
use super::{Alarm, AlarmKind, AlarmState, AlarmStates, Alarms, Phase, Source};
use crate::core::engine::{
    Category, Control, Flow, Input, Node, NodeInfo, NodeThread, Output, Pass, PassState, PoolStatus, Shared, UiContext,
};
use crate::core::gui::widgets::Icon;
use crate::core::theme::Colours;
use crate::standard_components::plots;
use crate::standard_components::pool::Pool;
use crate::standard_components::widgets::{mark_mixed, open_settings_button, require_input};
use crate::standard_components::widgets::{power, small_toggle};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Its one control: the processor's alarms.
const ALARMS: usize = 0;

#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlarmViewer {
    /// Which alarms pip as they go off, by id. None to begin with, as it'd be a surprise in a quiet room.
    pub sounding: Vec<u32>,
}

/// What's been switched while recording, on the GUI's side.
#[derive(Default)]
pub struct Overrides {
    /// The alarms switched off, by id: while recording there's no adding or removing them.
    ignored: Vec<u32>,
    /// The alarms whose sound has been switched, by id.
    sounds: Vec<u32>,
}

impl Overrides {
    /// Whether alarm `id` has been switched off.
    fn ignored(&self, id: u32) -> bool {
        self.ignored.contains(&id)
    }

    fn set_ignored(&mut self, id: u32, ignore: bool) {
        self.ignored.retain(|ignored| *ignored != id);
        if ignore {
            self.ignored.push(id);
        }
    }

    /// `state`, unless it's ignored.
    fn apply(&self, state: AlarmState) -> AlarmState {
        match self.ignored(state.id) {
            true => AlarmState { id: state.id, source: state.source, ..Default::default() },
            false => state,
        }
    }

    /// Whether alarm `id` pips, given the setting.
    fn sounds(&self, id: u32, setting: &[u32]) -> bool {
        setting.contains(&id) != self.sounds.contains(&id)
    }

    fn switch_sound(&mut self, id: u32) {
        flip(&mut self.sounds, id);
    }

    /// Moves what's been switched into the settings, as recording's over.
    fn save(&mut self, alarms: Option<&mut Alarms>, sounding: &mut Vec<u32>) {
        self.sounds.drain(..).for_each(|id| flip(sounding, id));
        for alarm in alarms.into_iter().flat_map(|alarms| &mut alarms.list) {
            if self.ignored.contains(&alarm.id) {
                alarm.on = false;
            }
        }
        self.ignored.clear();
    }
}

/// What its passes send the GUI's side, every pass: that a pass has come (so long, and whether it's recording), then
/// how each alarm stood in it.
pub enum Update {
    Pass { seconds: f64, recording: bool },
    State(AlarmState),
}

/// What its passes share with the GUI: every pass's alarm states, in order.
#[derive(Default)]
pub struct SentStates(Pool<Vec<Update>>);

impl Shared for SentStates {
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        vec![("alarm states", self.0.status())]
    }
}

/// Adds `id` to `ids`, or takes it out if it's there.
fn flip(ids: &mut Vec<u32>, id: u32) {
    match ids.iter().position(|other| *other == id) {
        Some(index) => drop(ids.remove(index)),
        None => ids.push(id),
    }
}

/// An alarm that's going off.
struct GoingOff {
    id: u32,
    /// For how long, so far.
    seconds: f64,
}

/// What's said while there's sound to come.
const SOUND_NOTE: &str = "Alarms will only sound when recording";

/// How many pips an alarm gets as it goes off.
const FIRST_PIPS: u8 = 2;
/// How long an alarm's to have been going off for more pips, and how many. After the last it says no more.
const REMINDERS: [(f64, u8); 2] = [(30.0, 3), (60.0, 4)];

/// While running: what's not been sent to the GUI yet.
#[derive(Default)]
pub struct Sending {
    pending: Vec<Update>,
}

impl PassState for Sending {
    const WHY: &'static str = "What the GUI hasn't been sent yet, which each pass adds to, in order.";
}

/// How many passes' updates wait, at most, for the GUI to take them (if it's held up): a minute's.
const PENDING_PASSES: usize = 3000;

/// The Alarm Viewer on the GUI's side: how the alarms stand (as of the last pass sent), how long those going off have
/// been, what's been switched while recording, and the sound.
#[derive(Default)]
pub struct AlarmsUi {
    shown: AlarmsDisplay,
    going_off: Vec<GoingOff>,
    /// Whether the last pass was recording.
    recording: bool,
    overrides: Overrides,
    tone: Tone,
}

/// What's shown: how each alarm stands.
#[derive(Clone, Default)]
pub struct AlarmsDisplay {
    pub states: Vec<AlarmState>,
    /// How long those going off have been, by id, in seconds.
    pub going_off: Vec<(u32, f64)>,
}

impl Node<Edges> for AlarmViewer {
    type State = Sending;
    type Shared = SentStates;
    type UiState = AlarmsUi;
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Alarm Viewer",
        category: Category::Visualisation,
        description: "How an Alarm Processor's alarms stand, with switches for them and for sound.",
    };
    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "alarms", kind: Kind::AlarmStates }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "alarms", kind: SettingKind::Alarms, read_only: false, on_right: false }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "an Alarm Processor")
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "Alarm tone",
            link: "flag",
            flow: Flow::FromGui,
            why: "On a thread of its own so that opening the sound device, the first time an alarm sounds, can't hold \
                  up the GUI. Only started if it's needed.",
            swapped_outputs: Vec::new(),
        }]
    }

    /// Sends the pass's alarm states to the GUI's side.
    fn process(&self, pass: &mut Pass<Edges>, sending: &mut Sending) {
        let Some(Some(Buffer::AlarmStates(input))) = pass.inputs.first().copied() else { return };
        let pending = &mut sending.pending;
        pending.push(Update::Pass { seconds: pass.duration, recording: pass.recording });
        pending.extend(input.states.iter().map(|state| Update::State(*state)));
        // If the GUI's held up a long while, the oldest passes go.
        if pending.len() > 2 * PENDING_PASSES * (1 + input.states.len()) {
            let keep = PENDING_PASSES * (1 + input.states.len());
            pending.drain(..pending.len() - keep);
            let first = pending.iter().position(|update| matches!(update, Update::Pass { .. }));
            pending.drain(..first.unwrap_or(pending.len()));
        }
        pass.shared::<SentStates>().0.send_pending(pending);
    }

    /// Goes through the passes sent, keeping track of the alarms going off, and pipping as they do; and once recording's
    /// over, saves what was switched while it was.
    fn gui_background(&mut self, context: &mut UiContext<Edges>) -> Option<Instant> {
        let mut ui = context.ui_state::<AlarmsUi>();
        let AlarmsUi { shown, going_off, recording, overrides, tone } = &mut *ui;
        if !context.locked && (!overrides.ignored.is_empty() || !overrides.sounds.is_empty()) {
            let alarms = match &mut context.controls[ALARMS] {
                Some(Setting::Alarms(alarms)) => Some(alarms),
                _ => None,
            };
            overrides.save(alarms, &mut self.sounding);
        }
        let sent = context.shared::<SentStates>()?;
        // A pass's states, once they've all come.
        let mut pass = |seconds: f64, now_recording: bool, states: &mut Vec<AlarmState>| {
            for state in states.iter_mut() {
                *state = overrides.apply(*state);
            }
            let was_recording = std::mem::replace(recording, now_recording);
            let sounding = &self.sounding;
            pips_due(going_off, states.iter().copied(), seconds, (was_recording, now_recording), |state, pips| {
                if overrides.sounds(state.id, sounding) {
                    tone.pips(state.source.index(), pips);
                }
            });
            std::mem::swap(&mut shown.states, states);
            states.clear();
        };
        let mut states = Vec::new();
        sent.0.receive_all(|updates| {
            let mut current = None;
            for update in updates {
                match update {
                    Update::Pass { seconds, recording } => {
                        if let Some((seconds, recording)) = current.replace((seconds, recording)) {
                            pass(seconds, recording, &mut states);
                        }
                    }
                    Update::State(state) => states.push(state),
                }
            }
            if let Some((seconds, recording)) = current {
                pass(seconds, recording, &mut states);
            }
        });
        shown.going_off.clear();
        shown.going_off.extend(going_off.iter().map(|going| (going.id, going.seconds)));
        None
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let (Some(DataStream::AlarmStates(stream)), Some(Setting::Alarms(alarms))) =
            (context.inputs[0], context.controls[ALARMS].as_ref())
        else {
            ui.horizontal(|ui| context.node_toolbar(ui));
            return plots::unconnected(ui, context.theme, "Connect an Alarm Processor's alarms and its setting");
        };
        let mut alarms = alarms.clone();
        let (locked, theme) = (context.locked, context.theme);
        let mut gui = context.ui_state::<AlarmsUi>();
        let AlarmsUi { shown, overrides, .. } = &mut *gui;
        // Nothing to show until it's run.
        let states = (!shown.states.is_empty()).then_some(shown.states.as_slice());
        let going_off = shown.going_off.as_slice();

        ui.horizontal(|ui| context.node_toolbar(ui));
        banner(ui, theme, &alarms, stream, states);
        ui.add_space(6.0);
        let waiting_to_sound = context.running && !locked && any_sound(&alarms, overrides, &self.sounding);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            all_row(ui, &mut alarms, overrides, locked, &mut self.sounding, waiting_to_sound, |ui| {
                open_settings_button(ui, context, ALARMS, "Alarm Processor");
            });
            ui.add_space(2.0);
            for alarm in &mut alarms.list {
                let state = states.and_then(|states| states.iter().find(|state| state.id == alarm.id).copied());
                let off_for = going_off.iter().find(|(id, _)| *id == alarm.id).map(|(_, seconds)| *seconds);
                let sounding = &mut self.sounding;
                ui.push_id(alarm.id, |ui| row(ui, theme, alarm, stream, state, off_for, overrides, locked, sounding));
                ui.add_space(4.0);
            }
        });
        context.controls[ALARMS] = Some(Setting::Alarms(alarms));
        plots::repaint_with_data(ui, context.running);
    }
}

/// A button for whether there's sound.
fn sound_button(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let tip = if *on {
        "While recording, pips as it goes off, and more at 30 and 60 s if it still is: click for quiet"
    } else {
        "Quiet: click for pips as it goes off, while recording"
    };
    small_toggle(ui, Icon::Sound, on, label, tip)
}

/// Moves on `seconds`, keeping `going_off` up to date with `states`, and calling `pip` with an alarm and how many pips it's
/// due, as it goes off or reaches a reminder.
fn keep_time(
    going_off: &mut Vec<GoingOff>,
    states: impl Iterator<Item = AlarmState> + Clone,
    seconds: f64,
    mut pip: impl FnMut(AlarmState, u8),
) {
    let alarming = states.filter(|state| state.phase == Phase::Alarm);
    going_off.retain(|going| alarming.clone().any(|state| state.id == going.id));
    for state in alarming {
        match going_off.iter_mut().find(|going| going.id == state.id) {
            Some(going) => {
                let (before, now) = (going.seconds, going.seconds + seconds);
                going.seconds = now;
                if let Some((_, pips)) = REMINDERS.iter().find(|(at, _)| before < *at && now >= *at) {
                    pip(state, *pips);
                }
            }
            None => {
                going_off.push(GoingOff { id: state.id, seconds: 0.0 });
                pip(state, FIRST_PIPS);
            }
        }
    }
}

/// As `keep_time`, calling `pip` only while recording, and for those going off as it starts. `recording` is whether it was
/// on the last pass, and whether it is on this one.
fn pips_due(
    going_off: &mut Vec<GoingOff>,
    states: impl Iterator<Item = AlarmState> + Clone,
    seconds: f64,
    recording: (bool, bool),
    mut pip: impl FnMut(AlarmState, u8),
) {
    if recording == (false, true) {
        let still_off = |state: &AlarmState| state.phase == Phase::Alarm && going_off.iter().any(|g| g.id == state.id);
        states.clone().filter(still_off).for_each(|state| pip(state, FIRST_PIPS));
    }
    keep_time(going_off, states, seconds, |state, pips| {
        if recording.1 {
            pip(state, pips);
        }
    });
}

/// A power button for an alarm. While recording its setting's locked, so it only ignores what's coming in (`ignored`, if
/// it can), and if the setting's off, there's nothing coming in to go back to. Returns whether to ignore it now, if that's
/// changed.
fn switch(ui: &mut egui::Ui, on: &mut bool, ignored: Option<bool>, locked: bool, label: &str) -> Option<bool> {
    if !locked {
        power(ui, on, label);
        return None;
    }
    let mut shown = *on && ignored != Some(true);
    let response = ui.add_enabled_ui(*on && ignored.is_some(), |ui| power(ui, &mut shown, label)).inner;
    let response = response.on_disabled_hover_text(
        "It was off when recording started, so its data isn't coming: it can come on once recording stops",
    );
    response.changed().then_some(!shown)
}

/// Whether `alarm` is on, with what's been switched while recording.
fn active(alarm: &Alarm, overrides: &Overrides) -> bool {
    alarm.on && !overrides.ignored(alarm.id)
}

/// Whether alarm `id` pips, with what's been switched while recording.
fn sounds(overrides: &Overrides, id: u32, sounding: &[u32]) -> bool {
    overrides.sounds(id, sounding)
}

/// Whether any alarm that's on has sound.
fn any_sound(alarms: &Alarms, overrides: &Overrides, sounding: &[u32]) -> bool {
    alarms.list.iter().any(|alarm| active(alarm, overrides) && sounds(overrides, alarm.id, sounding))
}

/// Switches whether alarm `id` pips: only for now, while recording.
fn switch_sound(overrides: &mut Overrides, id: u32, locked: bool, sounding: &mut Vec<u32>) {
    match locked {
        true => overrides.switch_sound(id),
        false => flip(sounding, id),
    }
}

/// Buttons for all the alarms, lined up with each alarm's, if there are any: they set each one. While recording, one that
/// was off when it started can't come on, so doesn't count. To their left, `settings`, there with or without alarms; and at
/// the far left, if alarms with sound are `waiting_to_sound`, a note saying what for.
fn all_row(
    ui: &mut egui::Ui,
    alarms: &mut Alarms,
    overrides: &mut Overrides,
    locked: bool,
    sounding: &mut Vec<u32>,
    waiting_to_sound: bool,
    settings: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new().inner_margin(8).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            if waiting_to_sound {
                ui.weak(SOUND_NOTE);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                if !alarms.list.is_empty() {
                    // While recording, those off since it started can't come on, so don't count.
                    let can_switch = |alarm: &&mut Alarm| !locked || alarm.on;
                    let count = alarms.list.iter().filter(|a| !locked || a.on).count();
                    let on = alarms.list.iter().filter(|a| active(a, overrides)).count();
                    if let Some(all) = all_power(ui, on, count) {
                        for alarm in alarms.list.iter_mut().filter(can_switch) {
                            match locked {
                                true => overrides.set_ignored(alarm.id, !all),
                                false => alarm.on = all,
                            }
                        }
                    }
                    let active_alarms = || alarms.list.iter().filter(|alarm| active(alarm, overrides));
                    let any = active_alarms().next().is_some();
                    let quiet = active_alarms().filter(|a| !sounds(overrides, a.id, sounding)).count();
                    let mut all = any && quiet == 0;
                    let mixed = any && !all && quiet < active_alarms().count();
                    let tip = mixed.then_some("Some pip: click for them all to");
                    let button = ui
                        .add_enabled_ui(any, |ui| {
                            let button = match tip {
                                Some(tip) => small_toggle(ui, Icon::Sound, &mut all, "All sound", tip),
                                None => sound_button(ui, &mut all, "All sound"),
                            };
                            if mixed {
                                mark_mixed(ui, &button);
                            }
                            button
                        })
                        .inner;
                    if button.changed() {
                        for alarm in &alarms.list {
                            if sounds(overrides, alarm.id, sounding) != all {
                                switch_sound(overrides, alarm.id, locked, sounding);
                            }
                        }
                    }
                }
                settings(ui);
            });
        });
    });
}

/// "tetrode 3", or nothing if it's of them all.
fn item_text(source: Source, stream: AlarmStates, item: Option<u32>) -> String {
    match item {
        Some(item) if source != Source::Positions => format!(" on {} {}", stream.item_name(source), item + 1),
        _ => String::new(),
    }
}

/// What's in the banner: its text, and its colours.
fn banner_text(
    theme: &Colours,
    alarms: &Alarms,
    stream: AlarmStates,
    states: Option<&[AlarmState]>,
) -> (String, Color32, Color32) {
    let quiet = (theme.surface_raised, theme.text_secondary);
    if alarms.list.is_empty() {
        return ("No alarms".into(), quiet.0, quiet.1);
    }
    if alarms.list.iter().all(|alarm| !alarm.on) {
        return ("Alarms off".into(), quiet.0, quiet.1);
    }
    let Some(states) = states else {
        return ("Not running".into(), quiet.0, quiet.1);
    };
    let sounding: Vec<String> = states
        .iter()
        .filter(|state| state.phase == Phase::Alarm)
        .filter_map(|state| {
            let kind = &alarms.get(state.id)?.kind;
            let item = item_text(state.source, stream, state.item);
            Some(format!("{}: {}{item}", state.source.name(), kind.value(state.value)))
        })
        .collect();
    if !sounding.is_empty() {
        return (format!("ALARM  {}", sounding.join(" · ")), theme.error, Color32::WHITE);
    }
    let watching = |state: &AlarmState| matches!(state.phase, Phase::Clear | Phase::Waiting(_));
    if states.iter().any(watching) {
        ("All clear".into(), theme.success, Color32::WHITE)
    } else {
        ("Nothing watched".into(), quiet.0, quiet.1)
    }
}

/// The alarms at a glance, across the window: red, while any's going off.
fn banner(ui: &mut egui::Ui, theme: &Colours, alarms: &Alarms, stream: AlarmStates, states: Option<&[AlarmState]>) {
    let (text, fill, ink) = banner_text(theme, alarms, stream, states);
    // All's clear so far, with some yet to see a whole period.
    let waiting = |state: &AlarmState| matches!(state.phase, Phase::Waiting(_));
    let pending = fill == theme.success && states.is_some_and(|states| states.iter().any(waiting));
    egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(12).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let label = egui::Label::new(RichText::new(text).size(22.0).strong().color(ink)).wrap();
        if pending {
            ui.horizontal(|ui| {
                ui.add(label);
                ui.add(egui::Spinner::new().size(20.0).color(ink))
                    .on_hover_text("Some alarms have yet to see a whole period");
            });
        } else {
            ui.add(label);
        }
    });
}

/// How long something's gone on: "0:07", "12:34", "1:02:03".
fn timer_text(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

/// An alarm: how it stands, its sound and power buttons, and what it's looking for.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    theme: &Colours,
    alarm: &mut Alarm,
    stream: AlarmStates,
    state: Option<AlarmState>,
    off_for: Option<f64>,
    overrides: &mut Overrides,
    locked: bool,
    sounding: &mut Vec<u32>,
) {
    let source = alarm.kind.source();
    let connected = stream.items(source) > 0;
    let period = alarm.period();
    let value =
        |state: AlarmState| format!("{}{}", alarm.kind.value(state.value), item_text(source, stream, state.item));
    let (text, colour) = match state.map(|state| (state.phase, state)) {
        _ if !connected => ("Not connected".to_owned(), theme.text_disabled),
        None => (String::new(), theme.text_secondary),
        Some((Phase::Off, _)) => ("Off".to_owned(), theme.text_disabled),
        Some((Phase::Waiting(_), _)) => (String::new(), theme.text_secondary),
        Some((Phase::Clear, state)) => (value(state), theme.success),
        Some((Phase::Alarm, state)) => {
            let timer = off_for.map_or(String::new(), |seconds| format!(" · {}", timer_text(seconds)));
            (format!("{}{timer}", value(state)), theme.error)
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
                let (id, name) = (alarm.id, alarm.kind.name());
                let ignored = Some(overrides.ignored(id));
                title(ui, alarm.on && ignored != Some(true), name);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                    let switched = ui.add_enabled_ui(connected, |ui| switch(ui, &mut alarm.on, ignored, locked, name));
                    if let Some(ignore) = switched.inner {
                        overrides.set_ignored(id, ignore);
                    }
                    // An alarm that's off has no sound to speak of, whatever it'll have when it's on.
                    let active = active(alarm, overrides);
                    let mut pips = active && sounds(overrides, id, sounding);
                    let label = format!("{name} sound");
                    let enabled = connected && active;
                    let button = ui.add_enabled_ui(enabled, |ui| sound_button(ui, &mut pips, &label)).inner;
                    if button.changed() {
                        switch_sound(overrides, id, locked, sounding);
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(text).color(colour).strong());
                });
            });
            ui.horizontal_top(|ui| {
                // The detail wraps, leaving room for the period at the right.
                let has_period = alarm.kind.has_period();
                let width = ui.available_width() - if has_period { PERIOD_WIDTH } else { 0.0 };
                ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
                    ui.add(egui::Label::new(RichText::new(detail(alarm, stream)).weak()).wrap());
                });
                if has_period {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        ui.weak(period_text(period, state.map(|state| state.phase)));
                    });
                }
            });
        });
}

/// Room for the period, at the right of a row.
const PERIOD_WIDTH: f32 = 80.0;

/// What an alarm's evaluated over, or while gathering its first period, how far it's got: "over 20s", "10 of 20s".
fn period_text(period: f64, phase: Option<Phase>) -> String {
    match phase {
        Some(Phase::Waiting(waited)) => format!("{:.0} of {period}s", waited as f64 * period),
        _ => format!("over {period}s"),
    }
}

/// What's healthy for an alarm, and what it measures: "Healthy: 5–400 Hz · total of 8 tetrodes".
fn detail(alarm: &Alarm, stream: AlarmStates) -> String {
    let healthy = match &alarm.kind {
        AlarmKind::SpikeRate { low, high, .. } => format!("{low}–{high} Hz"),
        AlarmKind::LfpSaturation { level_uv, low, .. } => format!(">= {low}% within ±{level_uv} µV"),
        AlarmKind::MissingPositions { low } => format!(">= {low}% contain data"),
        AlarmKind::TtlCount { low, high, .. } => format!("{low}–{high} TTLs"),
        AlarmKind::TtlUp { line } => format!("line {} down", line + 1),
    };
    let healthy = format!("Healthy: {healthy}");
    let Some(selection) = alarm.kind.selection() else { return healthy };
    let source = alarm.kind.source();
    let selected = (0..stream.items(source)).filter(|item| selection.selected(*item)).count();
    let name = stream.item_name(source);
    let together = if source == Source::Lfp { "mean" } else { "total" };
    match (selected, selection.together) {
        (1, _) => format!("{healthy} · 1 {name}"),
        (_, true) => format!("{healthy} · {together} of {selected} {name}s"),
        (_, false) => format!("{healthy} · each of {selected} {name}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeType;
    use crate::Nodes;
    use crate::core::engine::NodeTypeEnum;
    use crate::core::engine::{Graph, NodesEnum, Port, PortRole};
    use crate::core::engine::{NodeId, SharedByNode};
    use crate::standard_components::alarms::Selection;
    use crate::standard_components::alarms::processor::AlarmProcessor;
    use crate::standard_components::themes::THEMES;
    use egui_kittest::kittest::Queryable;
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The viewer's state on the GUI's side, shared with a test.
    type Gui = Rc<RefCell<Box<dyn Any>>>;

    /// What's been switched, in `gui`.
    fn overrides<R>(gui: &Gui, f: impl FnOnce(&mut Overrides) -> R) -> R {
        f(&mut gui.borrow_mut().downcast_mut::<AlarmsUi>().unwrap().overrides)
    }

    /// The viewer's work on the GUI's side, as the app does it, sharing `by_node` with its passes.
    fn background(
        graph: &mut Graph<Nodes>,
        viewer: NodeId,
        by_node: &SharedByNode,
        gui: &RefCell<Box<dyn Any>>,
        locked: bool,
    ) {
        let inputs = [Some(DataStream::AlarmStates(STREAM))];
        graph.edit(viewer, &inputs, &THEMES[0].colours, Some(by_node), Some(gui), None, true, locked, None, |n, c| {
            _ = n.gui_background(c);
        });
    }

    /// `passes` passes of `states` through a viewer, each taken on the GUI's side, recording or not, with `overrides`:
    /// what it then has there.
    fn through(states: Vec<AlarmState>, passes: usize, overrides: Overrides, recording: bool) -> AlarmsUi {
        let mut graph = Graph::<Nodes>::default();
        let viewer = graph.add(Nodes::AlarmViewer(AlarmViewer::default()), [0.0, 0.0]);
        let by_node = SharedByNode::default().with(viewer, SentStates::default());
        let input = Buffer::AlarmStates(super::super::AlarmStatesBuffer { states });
        let inputs = [Some(&input)];
        let mut sending = Sending::default();
        let gui = RefCell::new(Box::new(AlarmsUi { overrides, ..Default::default() }) as Box<dyn Any>);
        for _ in 0..passes {
            let mut pass = Pass::new(0, 0.02, &inputs, &mut []);
            (pass.shared, pass.recording) = (by_node.get::<SentStates>(viewer).unwrap(), recording);
            AlarmViewer::default().process(&mut pass, &mut sending);
            background(&mut graph, viewer, &by_node, &gui, recording);
        }
        *gui.into_inner().downcast().unwrap()
    }

    const BUTTON: egui::accesskit::Role = egui::accesskit::Role::Button;
    const STREAM: AlarmStates = AlarmStates { groups: 8, group_size: 4, channels: 4, positions: true, lines: 1 };
    const SPIKES: &str = "Spike rate in range";
    const TTLS: &str = "TTL count in range";

    /// One of each kind, with ids 0 to 3.
    fn one_of_each() -> Alarms {
        let mut alarms = Alarms::default();
        for kind in AlarmKind::library().into_iter().take(4) {
            alarms.add(kind);
        }
        alarms
    }

    /// The viewer's window, with `states` from a run, and connected to a processor of `alarms`.
    fn window(
        alarms: Alarms,
        display: impl Into<AlarmsDisplay>,
        overrides: Overrides,
        locked: bool,
        size: [f32; 2],
    ) -> (egui_kittest::Harness<'static, Graph<Nodes>>, Gui, crate::core::engine::NodeId) {
        window_of(alarms, display, overrides, locked, true, size)
    }

    /// The same, running or stopped.
    fn window_of(
        alarms: Alarms,
        display: impl Into<AlarmsDisplay>,
        overrides: Overrides,
        locked: bool,
        running: bool,
        size: [f32; 2],
    ) -> (egui_kittest::Harness<'static, Graph<Nodes>>, Gui, crate::core::engine::NodeId) {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let node_type = NodeType::named(name).unwrap();
            graph.add(Nodes::create(node_type), [0.0, 0.0])
        };
        let (processor, viewer) = (add(&mut graph, "Alarm Processor"), add(&mut graph, "Alarm Viewer"));
        if let Some(Nodes::AlarmProcessor(node)) = graph.get_mut(processor).map(|e| &mut e.node) {
            *node = AlarmProcessor { alarms };
        }
        let (from, to) = (
            Port { node: viewer, role: PortRole::Control, index: 0 },
            Port { node: processor, role: PortRole::Setting, index: 0 },
        );
        graph.connect(from, to).unwrap();
        let gui: Gui =
            Rc::new(RefCell::new(Box::new(AlarmsUi { shown: display.into(), overrides, ..Default::default() })));
        let state = gui.clone();
        let stream = Some(DataStream::AlarmStates(STREAM));
        let harness = egui_kittest::Harness::builder().with_size(size).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    viewer,
                    &[stream],
                    &THEMES[0].colours,
                    None,
                    Some(&*state),
                    None,
                    running,
                    locked,
                    None,
                    |node, context| node.gui_ui(ui, context),
                );
            },
            graph,
        );
        (harness, gui, processor)
    }

    fn processor_alarms(graph: &Graph<Nodes>, id: crate::core::engine::NodeId) -> Alarms {
        match graph.get(id).map(|e| &e.node) {
            Some(Nodes::AlarmProcessor(node)) => node.alarms.clone(),
            _ => panic!("not a processor"),
        }
    }

    fn viewer_node(graph: &Graph<Nodes>) -> &AlarmViewer {
        match graph.get(crate::core::engine::NodeId(2)).map(|e| &e.node) {
            Some(Nodes::AlarmViewer(node)) => node,
            _ => panic!("not a viewer"),
        }
    }

    impl From<Vec<AlarmState>> for AlarmsDisplay {
        fn from(states: Vec<AlarmState>) -> Self {
            AlarmsDisplay { states, going_off: Vec::new() }
        }
    }

    /// Alarm `id` of `one_of_each`, with `phase`.
    fn state(id: u32, phase: Phase, value: f32, item: Option<u32>) -> AlarmState {
        AlarmState { id, source: Source::ALL[id as usize], phase, value, item }
    }

    fn all_clear() -> Vec<AlarmState> {
        (0..4).map(|id| state(id, Phase::Clear, 10.0, None)).collect()
    }

    /// One going off, one fine, one waiting and one off. See tests/snapshots/.
    #[test]
    fn shows_alarms_prominently() {
        let states = vec![
            state(0, Phase::Alarm, 2.5, Some(2)),
            state(1, Phase::Clear, 99.8, None),
            state(2, Phase::Waiting(0.5), 0.0, None),
            state(3, Phase::Off, 0.0, None),
        ];
        let display = AlarmsDisplay { states, going_off: vec![(0, 83.4)] };
        let mut alarms = one_of_each();
        alarms.list[3].on = false;
        let (mut harness, ..) = window(alarms, display, Overrides::default(), false, [460.0, 420.0]);
        if let Some(Nodes::AlarmViewer(node)) =
            harness.state_mut().get_mut(crate::core::engine::NodeId(2)).map(|e| &mut e.node)
        {
            node.sounding = vec![0, 2, 3];
        }
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.get_by_label("2.5 Hz on tetrode 3 · 1:23");
        harness.snapshot("alarms");
    }

    #[test]
    fn a_ttl_up_alarm_says_which_line() {
        let mut alarms = Alarms::default();
        alarms.add(AlarmKind::TtlUp { line: 2 });
        let states = vec![AlarmState { id: 0, source: Source::Ttl, phase: Phase::Alarm, value: 1.0, item: None }];
        let (mut harness, ..) = window(alarms, states, Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_label("Healthy: line 3 down");
        harness.get_by_label("ALARM  TTLs: Line up");
    }

    #[test]
    fn the_banner_is_green_while_some_are_still_waiting() {
        let states = vec![state(0, Phase::Clear, 10.0, None), state(1, Phase::Waiting(0.5), 0.0, None)];
        let (mut harness, ..) = window(one_of_each(), states, Overrides::default(), false, [460.0, 420.0]);
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.get_by_label("All clear");
        harness.snapshot("alarms_pending");

        let waiting = vec![state(0, Phase::Waiting(0.1), 0.0, None)];
        let (mut harness, ..) = window(one_of_each(), waiting, Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(3);
        harness.get_by_label("All clear");
    }

    #[test]
    fn the_period_says_how_far_it_has_got_while_waiting() {
        assert_eq!(period_text(20.0, None), "over 20s");
        assert_eq!(period_text(20.0, Some(Phase::Clear)), "over 20s");
        assert_eq!(period_text(20.0, Some(Phase::Waiting(0.5))), "10 of 20s");
    }

    #[test]
    fn summaries_follow_the_settings() {
        let text = |index: usize| {
            let alarms = one_of_each();
            detail(&alarms.list[index], STREAM)
        };
        assert_eq!(text(0), "Healthy: 5–400 Hz · total of 8 tetrodes");
        assert_eq!(text(1), "Healthy: >= 95% within ±6300 µV · each of 4 channels");
        assert_eq!(text(2), "Healthy: >= 80% contain data");
        assert_eq!(text(3), "Healthy: 1–3 TTLs · 1 line");
    }

    #[test]
    fn says_when_there_are_no_alarms() {
        let (mut harness, ..) = window(Alarms::default(), Vec::new(), Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_label("No alarms");
    }

    #[test]
    fn a_state_is_shown_on_its_own_alarm_whatever_else_is_there() {
        let mut alarms = one_of_each();
        alarms.remove(0);
        let states = vec![state(2, Phase::Clear, 90.0, None), state(0, Phase::Alarm, 1.0, None)];
        let (mut harness, ..) = window(alarms, states, Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_label("90.0% with data");
        assert!(harness.query_by_label_contains("Spike").is_none(), "one that's gone isn't shown, or in the banner");
    }

    /// The pips due over `seconds` of passes, by when, with `states` throughout.
    fn pips_over(going_off: &mut Vec<GoingOff>, states: &[AlarmState], seconds: f64) -> Vec<(u32, u8)> {
        let mut pips = Vec::new();
        for _ in 0..(seconds / 0.02).round() as u32 {
            keep_time(going_off, states.iter().copied(), 0.02, |state, count| pips.push((state.id, count)));
        }
        pips
    }

    #[test]
    fn timers_read_as_clocks() {
        assert_eq!(timer_text(7.9), "0:07");
        assert_eq!(timer_text(754.0), "12:34");
        assert_eq!(timer_text(3723.0), "1:02:03");
    }

    #[test]
    fn the_time_going_off_gets_to_the_window() {
        let mut overrides = Overrides::default();
        overrides.set_ignored(0, true);
        let display = through(vec![state(1, Phase::Alarm, 1.0, None)], 50, overrides, false).shown;
        let [(id, seconds)] = display.going_off[..] else { panic!("{:?}", display.going_off) };
        assert_eq!(id, 1);
        assert!((seconds - 0.98).abs() < 1e-6, "{seconds}");
    }

    #[test]
    fn alarms_only_sound_while_recording() {
        let off = |id| state(id, Phase::Alarm, 0.0, None);
        let fine = |id| state(id, Phase::Clear, 0.0, None);
        let mut going = Vec::new();
        let mut pass = |states: &[AlarmState], recording| {
            let mut pips = Vec::new();
            pips_due(&mut going, states.iter().copied(), 0.02, recording, |state, count| pips.push((state.id, count)));
            pips
        };
        assert_eq!(pass(&[off(0), fine(1)], (false, false)), vec![], "going off, but not recording");
        // Recording starts: what's going off says so then, and only that.
        assert_eq!(pass(&[off(0), fine(1)], (false, true)), vec![(0, 2)]);
        assert_eq!(pass(&[off(0), fine(1)], (true, true)), vec![], "once");
        assert_eq!(pass(&[off(0), off(1)], (true, true)), vec![(1, 2)], "and what goes off while it is");
        assert_eq!(pass(&[fine(0), fine(1)], (true, false)), vec![], "over");
        assert_eq!(pass(&[off(2)], (false, false)), vec![], "not recording again");
    }

    #[test]
    fn a_reminder_is_only_heard_while_recording() {
        let off = state(0, Phase::Alarm, 0.0, None);
        let mut going = vec![GoingOff { id: 0, seconds: 29.99 }];
        let mut pips = Vec::new();
        pips_due(&mut going, [off].into_iter(), 0.02, (false, false), |state, count| pips.push((state.id, count)));
        assert_eq!(pips, vec![]);
        let mut going = vec![GoingOff { id: 0, seconds: 29.99 }];
        pips_due(&mut going, [off].into_iter(), 0.02, (true, true), |state, count| pips.push((state.id, count)));
        assert_eq!(pips, vec![(0, 3)]);
    }

    /// Stopped, the states stay as they were, to look at, and it's only quiet that it won't sound.
    #[test]
    fn stopped_it_shows_the_states_it_last_had() {
        let shown = |running| {
            let overrides = Overrides::default();
            let (mut harness, _, _) = window_of(one_of_each(), all_clear(), overrides, false, running, [700.0, 400.0]);
            harness.run_steps(2);
            (harness.query_by_label("Not running").is_none(), harness.query_by_label(SOUND_NOTE).is_some())
        };
        assert_eq!(shown(false).0, shown(true).0);
        assert!(shown(false).0);
        assert!(!shown(false).1, "nothing's waiting to sound");
    }

    #[test]
    fn it_says_alarms_will_only_sound_when_recording() {
        let shown = |locked, sounding: Vec<u32>| {
            let overrides = Overrides::default();
            let (mut harness, _, _) = window(one_of_each(), all_clear(), overrides, locked, [700.0, 400.0]);
            let viewer = harness.state().iter().find(|(_, e)| matches!(e.node, Nodes::AlarmViewer(_))).unwrap().0;
            if let Some(Nodes::AlarmViewer(node)) = harness.state_mut().get_mut(viewer).map(|e| &mut e.node) {
                node.sounding = sounding;
            }
            harness.run_steps(2);
            harness.query_by_label(SOUND_NOTE).is_some()
        };
        assert!(shown(false, vec![0]), "running, with sound");
        assert!(!shown(false, vec![]), "nothing to sound");
        assert!(!shown(true, vec![0]), "recording: it will");

        // Only those on count.
        let mut alarms = one_of_each();
        alarms.list.iter_mut().for_each(|alarm| alarm.on = false);
        assert!(!any_sound(&alarms, &Overrides::default(), &[0]));
    }

    #[test]
    fn pips_are_only_for_going_off() {
        let off = |id| state(id, Phase::Alarm, 0.0, None);
        let fine = |id| state(id, Phase::Clear, 0.0, None);
        let mut going = Vec::new();
        assert_eq!(pips_over(&mut going, &[off(0), fine(1), off(2)], 0.02), vec![(0, 2), (2, 2)]);
        assert_eq!(pips_over(&mut going, &[off(0), fine(1), off(2)], 1.0), vec![], "still off");
        assert_eq!(pips_over(&mut going, &[fine(0), fine(1), off(2)], 1.0), vec![], "clear again");
        assert_eq!(pips_over(&mut going, &[off(0), fine(1), off(2)], 0.02), vec![(0, 2)], "and off again");
    }

    #[test]
    fn an_alarm_still_going_off_pips_more_at_30_and_60_s_and_then_stops() {
        let off = [state(0, Phase::Alarm, 0.0, None)];
        let mut going = Vec::new();
        assert_eq!(pips_over(&mut going, &off, 29.0), vec![(0, 2)]);
        assert_eq!(pips_over(&mut going, &off, 2.0), vec![(0, 3)]);
        assert_eq!(pips_over(&mut going, &off, 28.0), vec![]);
        assert_eq!(pips_over(&mut going, &off, 2.0), vec![(0, 4)]);
        assert_eq!(pips_over(&mut going, &off, 600.0), vec![], "enough");

        // Clearing starts it all again.
        assert_eq!(pips_over(&mut going, &[state(0, Phase::Clear, 0.0, None)], 1.0), vec![]);
        assert_eq!(pips_over(&mut going, &off, 31.0), vec![(0, 2), (0, 3)]);
    }

    #[test]
    fn the_switches_edit_the_processor() {
        let (mut harness, _, processor) =
            window(one_of_each(), all_clear(), Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(BUTTON, SPIKES).click();
        harness.run_steps(2);
        let alarms = processor_alarms(harness.state(), processor);
        assert!(!alarms.list[0].on && alarms.list[1].on);
        harness.get_by_role_and_label(BUTTON, "All alarms").click();
        harness.run_steps(2);
        let alarms = processor_alarms(harness.state(), processor);
        assert!(alarms.list.iter().all(|alarm| alarm.on), "one off, so it's all on");
        harness.get_by_role_and_label(BUTTON, "All alarms").click();
        harness.run_steps(2);
        let alarms = processor_alarms(harness.state(), processor);
        assert!(alarms.list.iter().all(|alarm| !alarm.on), "then all off");
    }

    #[test]
    fn sound_is_for_each_and_all_set_them_all() {
        let (mut harness, ..) = window(one_of_each(), all_clear(), Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        assert!(viewer_node(harness.state()).sounding.is_empty(), "quiet to begin with");
        harness.get_by_role_and_label(BUTTON, "Spike rate in range sound").click();
        harness.run_steps(2);
        assert_eq!(viewer_node(harness.state()).sounding, vec![0]);
        harness.get_by_role_and_label(BUTTON, "All sound").click();
        harness.run_steps(2);
        assert_eq!(viewer_node(harness.state()).sounding.len(), 4);
        harness.get_by_role_and_label(BUTTON, "All sound").click();
        harness.run_steps(2);
        assert!(viewer_node(harness.state()).sounding.is_empty());
    }

    #[test]
    fn an_alarm_off_has_no_sound_to_switch_and_all_sound_goes_by_those_on() {
        let mut alarms = one_of_each();
        alarms.list[3].on = false;
        let (mut harness, ..) = window(alarms, all_clear(), Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(BUTTON, "TTL count in range sound").click();
        harness.run_steps(2);
        assert!(viewer_node(harness.state()).sounding.is_empty(), "it can't be switched");

        // All sound's on once those that are on are, and it's kept for one coming on.
        harness.get_by_role_and_label(BUTTON, "All sound").click();
        harness.run_steps(2);
        assert_eq!(viewer_node(harness.state()).sounding.len(), 4);
        harness.get_by_role_and_label(BUTTON, "All sound").click();
        harness.run_steps(2);
        assert!(viewer_node(harness.state()).sounding.is_empty(), "all on, so all off");
    }

    #[test]
    fn with_every_alarm_off_all_sound_can_not_be_switched() {
        let mut alarms = one_of_each();
        alarms.list.iter_mut().for_each(|alarm| alarm.on = false);
        let (mut harness, ..) = window(alarms, all_clear(), Overrides::default(), false, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(BUTTON, "All sound").click();
        harness.run_steps(2);
        assert!(viewer_node(harness.state()).sounding.is_empty());
    }

    #[test]
    fn while_recording_a_switch_only_ignores_until_recording_stops() {
        let mut off = one_of_each();
        off.list[3].on = false;
        let (mut harness, gui, processor) = window(off, all_clear(), Overrides::default(), true, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(BUTTON, SPIKES).click();
        harness.get_by_role_and_label(BUTTON, TTLS).click();
        harness.run_steps(2);
        let alarms = processor_alarms(harness.state(), processor);
        assert!(alarms.list[0].on, "the graph's left alone");
        assert!(overrides(&gui, |o| o.ignored(0)), "but it's ignored");
        assert!(!overrides(&gui, |o| o.ignored(3)) && !alarms.list[3].on, "one that's off has no data to come back to");
        harness.get_by_role_and_label(BUTTON, "LFP within range sound").click();
        harness.run_steps(2);
        assert!(viewer_node(harness.state()).sounding.is_empty(), "its setting's left alone too");
        assert!(overrides(&gui, |o| o.sounds(1, &[]) && !o.sounds(1, &[1])), "but it's switched, either way");

        // Once recording's over, its work on the GUI's side saves what was switched, window open or not.
        background(harness.state_mut(), NodeId(2), &SharedByNode::default(), &gui, false);
        assert_eq!(viewer_node(harness.state()).sounding, vec![1]);
        let saved = processor_alarms(harness.state(), processor);
        assert!(!saved.list[0].on && saved.list[1].on, "the ignored ones switched off");
        assert!(overrides(&gui, |o| o.sounds(1, &[1]) && !o.ignored(0)), "and nothing's left switched");
    }

    #[test]
    fn while_recording_all_ignores_what_can_be_ignored() {
        let mut off = one_of_each();
        off.list[3].on = false;
        let (mut harness, gui, processor) = window(off, all_clear(), Overrides::default(), true, [460.0, 420.0]);
        harness.run_steps(2);
        harness.get_by_role_and_label(BUTTON, "All alarms").click();
        harness.run_steps(2);
        assert!(overrides(&gui, |o| (0..3).all(|id| o.ignored(id)) && !o.ignored(3)));
        assert!(processor_alarms(harness.state(), processor).list[0].on, "the graph's left alone");
        harness.get_by_role_and_label(BUTTON, "All alarms").click();
        harness.run_steps(2);
        assert!(overrides(&gui, |o| (0..3).all(|id| !o.ignored(id))), "and back");
        harness.get_by_role_and_label(BUTTON, "All sound").click();
        harness.run_steps(2);
        assert!(overrides(&gui, |o| (0..4).all(|id| o.sounds(id, &[]))));
        assert!(viewer_node(harness.state()).sounding.is_empty());
    }

    /// A real run: the fake board's first line goes up twice a second, into a processor and on to the window's pool.
    #[test]
    fn a_run_gets_from_the_board_to_the_window() {
        use crate::core::engine::{Runner, Snapshot};
        use crate::standard_components::ephys::board::{Source as Board, serve_test_board};

        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let node_type = NodeType::named(name).unwrap();
            graph.add(Nodes::create(node_type), [0.0, 0.0])
        };
        let (board, processor, viewer) =
            (add(&mut graph, "Acquisition Board"), add(&mut graph, "Alarm Processor"), add(&mut graph, "Alarm Viewer"));
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Board::Fake { address: serve_test_board() };
        }
        let mut alarms = Alarms::default();
        let selection = Selection { together: true, ..Default::default() };
        alarms.add(AlarmKind::TtlCount { low: 1.0, high: 100.0, selection }).seconds = 1.0;
        alarms.add(AlarmKind::library()[0].clone());
        if let Some(Nodes::AlarmProcessor(node)) = graph.get_mut(processor).map(|e| &mut e.node) {
            node.alarms = alarms;
        }
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(board, PortRole::Output, 1), port(processor, PortRole::Input, 3)).unwrap();
        graph.connect(port(processor, PortRole::Output, 0), port(viewer, PortRole::Input, 0)).unwrap();

        let snapshot = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start(&snapshot, 2);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let shared = runner.shared();
        let gui = RefCell::new(Box::new(AlarmsUi::default()) as Box<dyn Any>);
        let shown = |gui: &RefCell<Box<dyn Any>>| gui.borrow().downcast_ref::<AlarmsUi>().unwrap().shown.states.clone();
        loop {
            background(&mut graph, viewer, &shared, &gui, false);
            let state = shown(&gui).first().copied();
            if let Some(AlarmState { phase: Phase::Clear, value, .. }) = state {
                assert!((1.0..=4.0).contains(&value), "two a second, more or less: {value}");
                break;
            }
            assert_ne!(state.map(|s| s.phase), Some(Phase::Alarm), "goes off though the ups are coming");
            assert!(std::time::Instant::now() < deadline, "never got there: {state:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(shown(&gui)[1].phase, Phase::Off, "the spikes aren't connected");
    }

    #[test]
    fn ignored_alarms_are_not_passed_on() {
        let states = (0..2).map(|id| state(id, Phase::Alarm, 1.0, None)).collect();
        let mut overrides = Overrides::default();
        overrides.set_ignored(1, true);
        let display = through(states, 1, overrides, true).shown;
        assert_eq!(display.states[0].phase, Phase::Alarm);
        assert_eq!(display.states[1].phase, Phase::Off);
    }
}
