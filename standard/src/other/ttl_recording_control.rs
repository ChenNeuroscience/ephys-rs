//! TTL Recording Control: starts recording when a line of its events goes up, as the toolbar's Record does, and, if
//! it's to, stops it when the line goes down (see `PassContext::request_recording`). Lines are numbered from 1, as the
//! LTX files number them.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::standard::ephys::{Event, Events};
use crate::standard::pool::Pool;
use crate::standard::widgets::{named_settings_grid, require_input, setting};
use ephys_rs::engine::{
    Buffers, Category, CommandContext, CommandSource, NoPorts, Node, NodeInfo, PassContext, PassState, Prepare,
    PrepareContext, Prepared, UiContext,
};

ephys_rs::ports! {
    events: DataStreamIn<Events>,
}

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

impl<E: PortEdges> Node<E> for TtlRecordingControl {
    type Ports = Ports;
    const INFO: NodeInfo = NodeInfo {
        name: "TTL Recording Control",
        category: Category::Other,
        description: "Starts recording when a line of its events goes up, and can stop it when it goes down.",
    };

    type State = Seen;
    type WorkerState = ();
    /// What it's seen, for its window.
    type Shared = Pool<Seen>;
    type UiState = ();
    type GlobalSettings = ();

    fn val_out(&self) -> NoPorts {
        NoPorts
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {}
    }

    fn data_stream_out(&self, _inputs: &DataStreamIn) -> NoPorts {
        NoPorts
    }

    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Seen, (), Self> {
        if context.same_streams() {
            return Prepared::nothing().into();
        }
        Prepared::shared(Seen::default()).into()
    }

    fn process(
        &self,
        pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        _: NoPorts,
        seen: &mut Seen,
        _: &mut (),
    ) {
        let Some(events) = inputs.events else { return };
        if let Some(on) = self.wanted(&events.events, seen).filter(|on| *on != pass.recording()) {
            pass.request_recording(on);
            *if on { &mut seen.starts } else { &mut seen.stops } += 1;
        }
        let seen = *seen;
        pass.shared::<Pool<Seen>>().send_newest(pass.sequence, |shown| *shown = seen);
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        let lines = context.data_stream_in::<DataStreamIn>().events.map(|stream| stream.lines);
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

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        let mut warnings = require_input(inputs.any_connected(), "events");
        if let Some(stream) = inputs.events
            && self.line > stream.lines
        {
            let lines = if stream.lines == 1 { "line".to_string() } else { format!("{} lines", stream.lines) };
            warnings.push(format!("Its events have {lines}: there's no line {}", self.line));
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edges;
    use crate::standard::ephys::EventsBuffer;
    use ephys_rs::commands::Sent;

    fn event(line: u8, high: bool) -> Event {
        Event { at: 0.0, line, high }
    }

    /// What a pass of `events` asks for, recording or not.
    fn asks(node: &TtlRecordingControl, events: Vec<Event>, recording: bool, seen: &mut Seen) -> Vec<bool> {
        let buffer = EventsBuffer { events, start: 0.0, end: 0.0 };
        let mut pass = PassContext::<Edges>::new(0, 0.02);
        let shared = Pool::<Seen>::default();
        pass.shared = &shared;
        pass.set_recording(recording);
        node.process(&mut pass, DataStreamIn { events: Some(&buffer) }, NoPorts, seen, &mut ());
        let asked = pass.commands.take().into_iter().map(|sent| match sent {
            Sent::Recording(_, on) => on,
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
        assert_eq!(Node::<Edges>::warnings(&node, &DataStreamIn { events: None }), ["Connect events"]);
        let events = |lines| DataStreamIn {
            events: Some(Events { lines, clock: crate::standard::ephys::Clock::Ticks { rate: 1000.0 } }),
        };
        assert_eq!(Node::<Edges>::warnings(&node, &events(2)), ["Its events have 2 lines: there's no line 3"]);
        assert!(Node::<Edges>::warnings(&node, &events(3)).is_empty());
    }
}
