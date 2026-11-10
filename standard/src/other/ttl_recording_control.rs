//! TTL Recording Control: starts recording when a line of its events goes up, as the toolbar's Record does, and, if
//! it's to, stops it when the line goes down (see `Pass::trigger_recording`). Lines are numbered from 1, as the LTX
//! files number them.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::standard::ephys::Event;
use crate::standard::pool::Pool;
use crate::standard::widgets::{named_settings_grid, require_input, setting};
use crate::{Buffer, DataStream, Edges, Kind};
use ephys_rs::engine::{Category, Input, Node, NodeInfo, Output, Pass, PassState, UiContext};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtlRecordingControl {
    /// The line it goes by, from 1.
    pub line: u32,
    /// Whether recording stops when the line goes down.
    pub stop_on_down: bool,
}

impl Default for TtlRecordingControl {
    fn default() -> Self {
        TtlRecordingControl { line: 1, stop_on_down: true }
    }
}

/// What it's seen of its line, and asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seen {
    /// Whether the line's up, once it's changed.
    pub up: Option<bool>,
    /// Times it's asked for recording to start, and to stop.
    pub starts: u64,
    pub stops: u64,
}

impl PassState for Seen {
    const WHY: &'static str = "Whether its line's up, and how many times it's started and stopped recording, which \
                               each pass carries on from, for its window.";
}

impl TtlRecordingControl {
    /// Whether recording should start (true) or stop, by `events`, if they say either: the last change on its line
    /// that does. A pulse that's up and down in a pass starts nothing if it stops on down.
    fn wanted(&self, events: &[Event], seen: &mut Seen) -> Option<bool> {
        let mut wanted = None;
        for event in events.iter().filter(|e| u32::from(e.line) + 1 == self.line) {
            seen.up = Some(event.high);
            if event.high || self.stop_on_down {
                wanted = Some(event.high);
            }
        }
        wanted
    }
}

impl Node<Edges> for TtlRecordingControl {
    const INFO: NodeInfo = NodeInfo {
        name: "TTL Recording Control",
        category: Category::Other,
        description: "Starts recording when a line of its events goes up, and can stop it when it goes down.",
    };

    type State = Seen;
    /// What it's seen, for its window.
    type Shared = Pool<Seen>;
    type UiState = ();
    type GlobalSettings = ();

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "events", kind: Kind::Events }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "events");
        if let Some(Some(DataStream::Events(stream))) = inputs.first()
            && self.line > stream.lines
        {
            let lines = if stream.lines == 1 { "line".to_string() } else { format!("{} lines", stream.lines) };
            warnings.push(format!("Its events have {lines}: there's no line {}", self.line));
        }
        warnings
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Seen {
        Seen::default()
    }

    fn process(&self, pass: &mut Pass<Edges>, seen: &mut Seen) {
        let Some(Some(Buffer::Events(events))) = pass.inputs.first() else { return };
        if let Some(on) = self.wanted(&events.events, seen).filter(|on| *on != pass.recording()) {
            pass.trigger_recording(on);
            *if on { &mut seen.starts } else { &mut seen.stops } += 1;
        }
        let seen = *seen;
        pass.shared::<Pool<Seen>>().send_newest(pass.sequence, |shown| *shown = seen);
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let lines = match context.inputs.first() {
            Some(Some(DataStream::Events(stream))) => Some(stream.lines),
            _ => None,
        };
        let seen = context.shared::<Pool<Seen>>().and_then(Pool::newest).map(|seen| *seen);
        let warning = context.theme.warning;
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, _context| {
            ui.label("Recording starts when the line goes up, as if Record were pressed.");
            ui.add_space(4.0);
            named_settings_grid(ui, "ttl recording control", |ui| {
                setting(ui, "Line", |ui| {
                    let most = lines.unwrap_or(64).max(1);
                    ui.add(egui::DragValue::new(&mut node.line).range(1..=most))
                        .on_hover_text("Numbered from 1, as the LTX files number them")
                });
                setting(ui, "Stop on down", |ui| {
                    ui.checkbox(&mut node.stop_on_down, "Recording stops when the line goes down")
                });
            });
            ui.separator();
            if lines.is_none() {
                ui.label(RichText::new("Connect events: a TTL line's changes").color(warning));
                return;
            }
            let Some(seen) = seen else {
                ui.weak("Press Play to watch the line.");
                return;
            };
            ui.label(match seen.up {
                Some(true) => format!("Line {} is up", node.line),
                Some(false) => format!("Line {} is down", node.line),
                None => format!("Line {} hasn't changed yet", node.line),
            });
            ui.weak(format!("Started recording {} times, stopped it {} times", seen.starts, seen.stops));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::ephys::{Events, EventsBuffer};
    use ephys_rs::triggers::Triggered;

    fn event(line: u8, high: bool) -> Event {
        Event { sample: 0, line, high, time: None }
    }

    /// What a pass of `events` asks for, recording or not.
    fn asks(node: &TtlRecordingControl, events: Vec<Event>, recording: bool, seen: &mut Seen) -> Vec<bool> {
        let buffer = Buffer::Events(EventsBuffer { events, pass_samples: 0 });
        let inputs = [Some(&buffer)];
        let mut pass = Pass::new(0, 0.02, &inputs, &mut []);
        let shared = Pool::<Seen>::default();
        pass.shared = &shared;
        pass.start_recording = recording.then(std::time::Instant::now);
        node.process(&mut pass, seen);
        let asked = pass.direct.take().into_iter().map(|(_, triggered)| match triggered {
            Triggered::Recording(on) => on,
            _ => panic!("recording"),
        });
        asked.collect()
    }

    #[test]
    fn its_line_going_up_starts_recording_and_going_down_stops_it() {
        let node = TtlRecordingControl { line: 2, stop_on_down: true };
        let mut seen = Seen::default();
        assert_eq!(asks(&node, vec![event(0, true), event(2, true)], false, &mut seen), [] as [bool; 0], "other lines");
        assert_eq!(seen.up, None);
        assert_eq!(asks(&node, vec![event(1, true)], false, &mut seen), [true], "line 2 is line 1 from 0");
        assert_eq!(asks(&node, vec![event(1, true)], true, &mut seen), [] as [bool; 0], "recording already");
        assert_eq!(seen.up, Some(true));
        assert_eq!(asks(&node, vec![event(1, false)], true, &mut seen), [false]);
        assert_eq!(asks(&node, vec![event(1, true), event(1, false)], false, &mut seen), [] as [bool; 0], "a pulse");
        assert_eq!((seen.up, seen.starts, seen.stops), (Some(false), 1, 1));
    }

    #[test]
    fn it_can_leave_stopping_to_whoever_started_it() {
        let node = TtlRecordingControl { line: 1, stop_on_down: false };
        let mut seen = Seen::default();
        assert_eq!(asks(&node, vec![event(0, true), event(0, false)], false, &mut seen), [true], "a pulse");
        assert_eq!(asks(&node, vec![event(0, false)], true, &mut seen), [] as [bool; 0]);
    }

    #[test]
    fn it_warns_of_a_line_its_events_dont_have() {
        let node = TtlRecordingControl { line: 3, ..Default::default() };
        assert_eq!(node.warnings(&[None]), ["Connect events"]);
        let events = |lines| [Some(DataStream::Events(Events { lines, sample_rate: 1000.0 }))];
        assert_eq!(node.warnings(&events(2)), ["Its events have 2 lines: there's no line 3"]);
        assert!(node.warnings(&events(3)).is_empty());
    }
}
