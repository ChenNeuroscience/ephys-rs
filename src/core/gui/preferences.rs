//! The app's preferences (Settings…), kept between sessions as the recent files are: stopping recording and acquisition
//! when no one's been using the app for a while, and which node types can be added to a graph.
//!
//! Unattended stopping is the GUI's: it notes when there was last input (`App::last_input`), warns in the last minutes
//! before a stop (any input carries on), stops, and says it did. Recording stops no later than acquisition, as stopping
//! acquisition stops it too.

use std::collections::BTreeSet;
use std::time::Duration;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::dialog;
use crate::core::engine::{Category, NodeInfo};
use crate::core::theme::Colours;

/// How long before a stop it's warned of: five minutes, or half the limit, if that's shorter.
const WARNING: Duration = Duration::from_secs(5 * 60);
/// The longest a limit can be, in minutes: a day.
const LONGEST: u32 = 24 * 60;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Stop recording after this long without input.
    pub stop_recording: IdleLimit,
    /// Stop acquisition (the graph running) after this long without input.
    pub stop_acquisition: IdleLimit,
    /// Node types that can't be added to a graph, by name (see `NodeInfo::name`). From the next start.
    pub hidden_types: BTreeSet<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            stop_recording: IdleLimit { on: true, minutes: 60 },
            stop_acquisition: IdleLimit { on: true, minutes: 120 },
            hidden_types: BTreeSet::new(),
        }
    }
}

/// A limit on how long something carries on without input. Its time's kept while it's off, for when it's on again.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct IdleLimit {
    pub on: bool,
    pub minutes: u32,
}

/// What's stopped when no one's been using the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stopping {
    Recording,
    /// Acquisition, and recording with it.
    Acquisition,
}

/// Where things stand, after so long without input.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Unattended {
    Fine,
    /// The next stop's soon: in `left`.
    Warning {
        stopping: Stopping,
        left: Duration,
    },
    /// Time to stop.
    Stop(Stopping),
}

impl Preferences {
    /// After `idle` without input, running or not, and recording or not: whether something's to stop, or soon will.
    pub fn unattended(&self, idle: Duration, running: bool, recording: bool) -> Unattended {
        let minutes = |m: u32| Duration::from_secs(u64::from(m) * 60);
        let acquisition = (running && self.stop_acquisition.on)
            .then(|| (Stopping::Acquisition, minutes(self.stop_acquisition.minutes)));
        let recording =
            (recording && self.stop_recording.on).then(|| (Stopping::Recording, minutes(self.recording_minutes())));
        // The sooner: acquisition, if they're the same, as stopping it stops recording too.
        let next = [acquisition, recording]
            .into_iter()
            .flatten()
            .min_by_key(|&(s, limit)| (limit, s != Stopping::Acquisition));
        let Some((stopping, limit)) = next else { return Unattended::Fine };
        if idle >= limit {
            Unattended::Stop(stopping)
        } else if limit - idle <= WARNING.min(limit / 2) {
            Unattended::Warning { stopping, left: limit - idle }
        } else {
            Unattended::Fine
        }
    }

    /// Recording's limit: no more than acquisition's, if that's on.
    fn recording_minutes(&self) -> u32 {
        match self.stop_acquisition.on {
            true => self.stop_recording.minutes.min(self.stop_acquisition.minutes),
            false => self.stop_recording.minutes,
        }
    }
}

/// Whether `event` is someone using the app: the mouse or keyboard.
pub fn is_input(event: &egui::Event) -> bool {
    use egui::Event::*;
    matches!(
        event,
        Key { .. }
            | Text(_)
            | PointerMoved(_)
            | PointerButton { .. }
            | MouseWheel { .. }
            | Touch { .. }
            | Zoom(_)
            | Copy
            | Cut
            | Paste(_)
    )
}

/// `minutes`, as it's shown: "45 min", "2 h", "1 h 30 min".
pub fn minutes_text(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Minutes, from what's typed: "90", "90 min", "2h", "1 h 30 min", "1.5h", "1:30".
pub fn parse_minutes(text: &str) -> Option<f64> {
    let text = text.trim().to_lowercase();
    if let Some((h, m)) = text.split_once(':') {
        return Some(h.trim().parse::<f64>().ok()? * 60.0 + m.trim().parse::<f64>().ok()?);
    }
    let mut total = 0.0;
    let mut rest = text.as_str();
    let mut any = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        let number_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        let number: f64 = rest[..number_end].parse().ok()?;
        rest = rest[number_end..].trim_start();
        let unit_end = rest.find(|c: char| !c.is_alphabetic()).unwrap_or(rest.len());
        total += match &rest[..unit_end] {
            "h" | "hr" | "hrs" | "hour" | "hours" => number * 60.0,
            "" | "m" | "min" | "mins" | "minute" | "minutes" => number,
            _ => return None,
        };
        rest = &rest[unit_end..];
        any = true;
    }
    any.then_some(total)
}

/// `left`, as a countdown: "4:59".
pub fn countdown(left: Duration) -> String {
    let seconds = left.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The Settings window, editing `preferences` as it goes. `hidden_at_start` are the node types hidden since the app
/// started (from `types`), to say a restart's needed when they're changed. Returns whether it's still open.
pub fn show(
    ctx: &egui::Context,
    preferences: &mut Preferences,
    types: &[NodeInfo],
    hidden_at_start: &BTreeSet<String>,
    colours: &Colours,
) -> bool {
    let mut close = false;
    let body = |ui: &mut egui::Ui| {
        ui.label(RichText::new("When no one's using ephys-rs").strong());
        egui::Grid::new("unattended").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            let acquisition_on = preferences.stop_acquisition.on;
            let acquisition = preferences.stop_acquisition.minutes;
            let most = if acquisition_on { acquisition } else { LONGEST };
            limit_row(ui, "Stop recording", &mut preferences.stop_recording, most);
            ui.end_row();
            limit_row(ui, "Stop acquisition", &mut preferences.stop_acquisition, LONGEST);
            ui.end_row();
        });
        // Acquisition's limit may have come down past recording's.
        if preferences.stop_acquisition.on {
            preferences.stop_recording.minutes = preferences.recording_minutes();
        }
        ui.label(
            RichText::new(
                "With no mouse or keyboard input in ephys-rs for that long. A warning shows a few minutes before, and \
                 any input carries on. Recording stops no later than acquisition.",
            )
            .small()
            .color(colours.text_secondary),
        );

        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Node types").strong());
            let all = preferences.hidden_types.is_empty();
            if ui.add_enabled(!all, egui::Button::new("Allow all").small()).clicked() {
                preferences.hidden_types.clear();
            }
        });
        ui.label(
            RichText::new(
                "Which can be added to a graph. Nodes already in a graph stay as they are. Changes apply when \
                 ephys-rs next starts.",
            )
            .small()
            .color(colours.text_secondary),
        );
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            for category in Category::ALL {
                let in_category: Vec<&NodeInfo> = types.iter().filter(|t| t.category == category).collect();
                if in_category.is_empty() {
                    continue;
                }
                ui.label(RichText::new(category.name()).small().weak());
                ui.horizontal_wrapped(|ui| {
                    for info in in_category {
                        let mut on = !preferences.hidden_types.contains(info.name);
                        if ui.checkbox(&mut on, info.name).on_hover_text(info.description).changed() {
                            match on {
                                true => preferences.hidden_types.remove(info.name),
                                false => preferences.hidden_types.insert(info.name.to_string()),
                            };
                        }
                    }
                });
            }
        });
        if preferences.hidden_types != *hidden_at_start {
            ui.add_space(4.0);
            ui.label(RichText::new("Restart ephys-rs to apply the change to node types.").color(colours.warning));
        }
    };
    let footer = |ui: &mut egui::Ui| {
        close = dialog::at_right(ui, |ui| ui.button("OK").clicked());
    };
    let dismissed = dialog::show(ctx, "Settings", 520.0, colours, body, footer);
    !close && !dismissed
}

/// A limit's row: whether it's on, and after how long (at most `most` minutes).
fn limit_row(ui: &mut egui::Ui, name: &str, limit: &mut IdleLimit, most: u32) {
    ui.checkbox(&mut limit.on, name);
    ui.add_enabled_ui(limit.on, |ui| {
        ui.horizontal(|ui| {
            ui.label("after");
            let time = egui::DragValue::new(&mut limit.minutes)
                .range(1..=most.max(1))
                .speed(1.0)
                .custom_formatter(|m, _| minutes_text(m as u32))
                .custom_parser(parse_minutes);
            ui.add(time).on_hover_text("Drag, or type a time: 90, 2h, 1h 30m, 1:30");
            ui.label("without input");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minutes(m: u64) -> Duration {
        Duration::from_secs(m * 60)
    }

    #[test]
    fn by_default_recording_stops_after_an_hour_and_acquisition_after_two() {
        let p = Preferences::default();
        assert_eq!(
            p.unattended(minutes(59), true, true),
            Unattended::Warning { stopping: Stopping::Recording, left: minutes(1) }
        );
        assert_eq!(p.unattended(minutes(60), true, true), Unattended::Stop(Stopping::Recording));
        assert_eq!(p.unattended(minutes(60), true, false), Unattended::Fine, "not recording: acquisition's later");
        assert_eq!(
            p.unattended(minutes(115), true, false),
            Unattended::Warning { stopping: Stopping::Acquisition, left: minutes(5) }
        );
        assert_eq!(p.unattended(minutes(114), true, false), Unattended::Fine, "five minutes' warning");
        assert_eq!(p.unattended(minutes(120), true, false), Unattended::Stop(Stopping::Acquisition));
        assert_eq!(p.unattended(minutes(500), false, false), Unattended::Fine, "nothing running");
    }

    #[test]
    fn recording_stops_no_later_than_acquisition() {
        let mut p = Preferences::default();
        p.stop_recording.minutes = 180;
        assert_eq!(p.unattended(minutes(120), true, true), Unattended::Stop(Stopping::Acquisition), "both, as one");
        p.stop_acquisition.on = false;
        assert_eq!(p.unattended(minutes(120), true, true), Unattended::Fine);
        assert_eq!(p.unattended(minutes(180), true, true), Unattended::Stop(Stopping::Recording));
        p.stop_recording.on = false;
        assert_eq!(p.unattended(minutes(10_000), true, true), Unattended::Fine, "neither on");
    }

    #[test]
    fn a_short_limit_warns_for_half_of_it() {
        let mut p = Preferences::default();
        p.stop_acquisition.minutes = 4;
        assert_eq!(p.unattended(minutes(1), true, false), Unattended::Fine);
        assert!(matches!(p.unattended(minutes(2), true, false), Unattended::Warning { .. }));
    }

    #[test]
    fn times_are_shown_and_typed_in_hours_and_minutes() {
        assert_eq!([45, 60, 90, 120].map(minutes_text), ["45 min", "1 h", "1 h 30 min", "2 h"]);
        for (typed, m) in [("90", 90.0), ("90 min", 90.0), ("2h", 120.0), ("1 h 30 min", 90.0), ("1.5h", 90.0)] {
            assert_eq!(parse_minutes(typed), Some(m), "{typed}");
        }
        assert_eq!(parse_minutes("1:30"), Some(90.0));
        assert_eq!(parse_minutes(&minutes_text(90)), Some(90.0), "what's shown can be typed back");
        assert_eq!(parse_minutes("soon"), None);
        assert_eq!(parse_minutes(""), None);
        assert_eq!(countdown(Duration::from_secs(299)), "4:59");
    }

    /// See tests/snapshots/.
    #[test]
    fn the_settings_window() {
        use crate::Nodes;
        use crate::core::engine::NodeSet;
        use crate::standard_components::themes::THEMES;
        let mut preferences = Preferences::default();
        preferences.hidden_types.insert("Spectrum".into());
        let mut harness = egui_kittest::Harness::builder().with_size([700.0, 760.0]).wgpu().build_ui_state(
            |ui, preferences: &mut Preferences| {
                show(ui.ctx(), preferences, Nodes::TYPES, &BTreeSet::new(), &THEMES[0].colours);
            },
            preferences,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("settings");
    }

    #[test]
    fn preferences_from_an_older_version_keep_what_they_have() {
        let p: Preferences = serde_json::from_str(r#"{"hidden_types": ["Spectrum"]}"#).unwrap();
        assert_eq!(p.stop_recording, Preferences::default().stop_recording);
        assert!(p.hidden_types.contains("Spectrum"));
    }
}
