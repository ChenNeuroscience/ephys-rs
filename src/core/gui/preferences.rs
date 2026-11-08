//! The app's preferences (Settings…), kept between sessions with the recent files: stopping recording and acquisition
//! when no one's been using the app for a while; which node, annotation and base directory types are offered; and how
//! many recent files are kept.
//!
//! Auto stop is the GUI's: it notes when there was last input (`App::last_input`), warns in the last minutes before a
//! stop (any input carries on), stops, and says it did. Recording stops no later than acquisition, as stopping
//! acquisition stops it too.
//!
//! What's offered applies from the next start (`App::hidden`): it's only what can be added, or chosen, so nothing
//! already in a graph, or in use, changes under anyone.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::dialog;
use crate::core::annotation::AnnotationInfo;
use crate::core::base_directory::BaseDirectoryType;
use crate::core::engine::{Category, NodeInfo};
use crate::core::theme::Colours;

/// How long before a stop it's warned of: five minutes, or half the limit, if that's shorter.
const WARNING: Duration = Duration::from_secs(5 * 60);
/// The longest a limit can be, in hours: a day.
const LONGEST_HOURS: u32 = 24;
/// The most recent files that can be kept.
pub const MOST_RECENT_FILES: usize = 50;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Stop recording after this long without input.
    pub stop_recording: IdleLimit,
    /// Stop acquisition (the graph running) after this long without input.
    pub stop_acquisition: IdleLimit,
    /// What isn't offered, from the next start.
    pub hidden: Hidden,
    /// How many recent files are kept (up to `MOST_RECENT_FILES`).
    pub recent_files: usize,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            stop_recording: IdleLimit { on: true, minutes: 60 },
            stop_acquisition: IdleLimit { on: true, minutes: 120 },
            hidden: Hidden::default(),
            recent_files: 10,
        }
    }
}

/// The types that aren't offered, by name: node types aren't in the add-node menu, annotation types aren't among the
/// drawing tools, and base directory types can't be chosen (unless one's in use).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Hidden {
    pub nodes: BTreeSet<String>,
    pub annotations: BTreeSet<String>,
    pub base_directories: BTreeSet<String>,
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

    /// How many recent files are kept, within what can be.
    pub fn recent_files(&self) -> usize {
        self.recent_files.min(MOST_RECENT_FILES)
    }
}

/// `minutes`, as it's shown: "45 min", "2 h", "1 h 30 min".
pub fn minutes_text(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// `left`, as a countdown: "4:59".
pub fn countdown(left: Duration) -> String {
    let seconds = left.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
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

/// The types the program has, to offer or not.
pub struct Types<'a> {
    pub nodes: &'a [NodeInfo],
    pub annotations: &'a [AnnotationInfo],
    pub base_directories: &'a [BaseDirectoryType],
}

/// A part of the Settings window, chosen on its left.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Section {
    #[default]
    AutoStop,
    Nodes,
    Annotations,
    BaseDirectories,
    RecentFiles,
}

impl Section {
    const ALL: [Section; 5] =
        [Section::AutoStop, Section::Nodes, Section::Annotations, Section::BaseDirectories, Section::RecentFiles];

    fn name(self) -> &'static str {
        match self {
            Section::AutoStop => "Auto stop",
            Section::Nodes => "Nodes",
            Section::Annotations => "Annotations",
            Section::BaseDirectories => "Base directories",
            Section::RecentFiles => "Recent files",
        }
    }
}

/// The Settings window, while it's open: which section's shown.
#[derive(Default)]
pub struct SettingsWindow {
    pub section: Section,
}

/// How tall the sections are, so the window doesn't change size between them: what's longer scrolls.
const SECTION_HEIGHT: f32 = 340.0;

impl SettingsWindow {
    /// Shows it, editing `preferences` (and `recent`, the recent files) as it goes. `hidden_at_start` is what's been
    /// hidden since the app started (of `types`), to say a restart's needed when that's changed. Returns whether it's
    /// still open.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        preferences: &mut Preferences,
        recent: &mut Vec<PathBuf>,
        types: &Types,
        hidden_at_start: &Hidden,
        colours: &Colours,
    ) -> bool {
        let mut close = false;
        let restart = preferences.hidden != *hidden_at_start;
        let has = |section: Section| match section {
            Section::Annotations => !types.annotations.is_empty(),
            Section::BaseDirectories => !types.base_directories.is_empty(),
            _ => true,
        };
        let body = |ui: &mut egui::Ui| {
            let size = egui::vec2(ui.available_width(), SECTION_HEIGHT);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.set_height(SECTION_HEIGHT);
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    for section in Section::ALL.into_iter().filter(|s| has(*s)) {
                        if ui.selectable_label(self.section == section, section.name()).clicked() {
                            self.section = section;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().max_height(SECTION_HEIGHT).auto_shrink([false, false]).show(
                        ui,
                        |ui| {
                            self.section_ui(ui, preferences, recent, types, colours);
                        },
                    );
                });
            });
        };
        let footer = |ui: &mut egui::Ui| {
            if restart {
                ui.label(
                    RichText::new("Restart ephys-rs to apply the change to what's offered").color(colours.warning),
                );
            }
            close = dialog::at_right(ui, |ui| ui.button("OK").clicked());
        };
        let dismissed = dialog::show(ctx, "Settings", 640.0, colours, body, footer);
        !close && !dismissed
    }

    fn section_ui(
        &self,
        ui: &mut egui::Ui,
        preferences: &mut Preferences,
        recent: &mut Vec<PathBuf>,
        types: &Types,
        colours: &Colours,
    ) {
        let note = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).small().color(colours.text_secondary));
            ui.add_space(4.0);
        };
        const OFFERED: &str = "Applies when ephys-rs next starts.";
        match self.section {
            Section::AutoStop => {
                note(
                    ui,
                    "Stop when no one's used ephys-rs (the mouse or keyboard, in its window) for a while. A warning \
                     shows a few minutes before, and any input carries on. Recording stops no later than acquisition.",
                );
                egui::Grid::new("auto stop").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    let most = match preferences.stop_acquisition.on {
                        true => preferences.stop_acquisition.minutes,
                        false => LONGEST_HOURS * 60,
                    };
                    limit_row(ui, "Stop recording", &mut preferences.stop_recording, most);
                    ui.end_row();
                    limit_row(ui, "Stop acquisition", &mut preferences.stop_acquisition, LONGEST_HOURS * 60);
                    ui.end_row();
                });
                // Acquisition's limit may have come down past recording's.
                preferences.stop_recording.minutes = preferences.recording_minutes();
            }
            Section::Nodes => {
                note(ui, &format!("Which node types can be added to a graph. Nodes already in one stay. {OFFERED}"));
                for category in Category::ALL {
                    let in_category = types.nodes.iter().filter(|t| t.category == category);
                    let items: Vec<(&str, &str)> = in_category.map(|t| (t.name, t.description)).collect();
                    if items.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(category.name()).small().weak());
                    checklist(ui, &items, &mut preferences.hidden.nodes);
                    ui.add_space(4.0);
                }
            }
            Section::Annotations => {
                note(ui, &format!("Which annotations can be drawn on the graph. Those already on it stay. {OFFERED}"));
                let items: Vec<(&str, &str)> = types.annotations.iter().map(|t| (t.name, t.description)).collect();
                checklist(ui, &items, &mut preferences.hidden.annotations);
            }
            Section::BaseDirectories => {
                note(
                    ui,
                    &format!("Which base directories can be chosen for recordings. The one in use stays. {OFFERED}"),
                );
                let items: Vec<(&str, &str)> = types.base_directories.iter().map(|t| (t.name, t.description)).collect();
                checklist(ui, &items, &mut preferences.hidden.base_directories);
            }
            Section::RecentFiles => {
                note(ui, "The files last opened or saved, in File › Open Recent, and the basic view's start.");
                ui.horizontal(|ui| {
                    let label = ui.label("Keep the last");
                    let count = egui::DragValue::new(&mut preferences.recent_files).range(0..=MOST_RECENT_FILES);
                    ui.add(count).labelled_by(label.id);
                    ui.label("files");
                });
                recent.truncate(preferences.recent_files());
                ui.add_space(4.0);
                let clear = egui::Button::new(format!("Clear the list ({})", recent.len()));
                if ui.add_enabled(!recent.is_empty(), clear).clicked() {
                    recent.clear();
                }
            }
        }
    }
}

/// A checkbox a line for each of `items` (its name, and what it is), on unless it's `hidden`.
fn checklist(ui: &mut egui::Ui, items: &[(&str, &str)], hidden: &mut BTreeSet<String>) {
    for &(name, description) in items {
        let mut on = !hidden.contains(name);
        if ui.checkbox(&mut on, name).on_hover_text(description).changed() {
            match on {
                true => hidden.remove(name),
                false => hidden.insert(name.to_string()),
            };
        }
    }
}

/// A limit's row: whether it's on, and after how long, in hours and minutes (at most `most` minutes, in all). Minutes
/// past an hour become hours.
fn limit_row(ui: &mut egui::Ui, name: &str, limit: &mut IdleLimit, most: u32) {
    ui.checkbox(&mut limit.on, name);
    ui.add_enabled_ui(limit.on, |ui| {
        ui.horizontal(|ui| {
            ui.label("after");
            let (mut hours, mut minutes) = (limit.minutes / 60, limit.minutes % 60);
            let h = ui.add(egui::DragValue::new(&mut hours).range(0..=LONGEST_HOURS).suffix(" h"));
            let m = ui.add(egui::DragValue::new(&mut minutes).range(0..=LONGEST_HOURS * 60).suffix(" min"));
            if h.changed() || m.changed() {
                limit.minutes = (hours * 60 + minutes).clamp(1, most.max(1));
            }
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
    fn times_are_shown_in_hours_and_minutes() {
        assert_eq!([45, 60, 90, 120].map(minutes_text), ["45 min", "1 h", "1 h 30 min", "2 h"]);
        assert_eq!(countdown(Duration::from_secs(299)), "4:59");
    }

    /// Typed in, minutes past an hour become hours; and recording's limit stays within `most`.
    #[test]
    fn minutes_past_an_hour_become_hours() {
        use egui_kittest::kittest::Queryable as _;
        let mut harness = egui_kittest::Harness::builder().with_size([500.0, 120.0]).build_ui_state(
            |ui, p: &mut Preferences| {
                egui::Grid::new("limits").show(ui, |ui| {
                    limit_row(ui, "Stop recording", &mut p.stop_recording, 150);
                    ui.end_row();
                });
            },
            Preferences::default(),
        );
        let type_minutes = |harness: &mut egui_kittest::Harness<Preferences>, from: &str, typed: &str| {
            harness.run();
            harness.get_by_value(from).click();
            harness.run();
            harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
            harness.event(egui::Event::Text(typed.into()));
            harness.key_press(egui::Key::Enter);
            harness.run();
        };
        type_minutes(&mut harness, "0 min", "75");
        assert_eq!(harness.state().stop_recording.minutes, 135, "1 h and 75 min");
        assert!(harness.query_by_value("2 h").is_some() && harness.query_by_value("15 min").is_some());
        type_minutes(&mut harness, "15 min", "59");
        assert_eq!(harness.state().stop_recording.minutes, 150, "at most what it can be");
    }

    /// See tests/snapshots/.
    #[test]
    fn the_settings_window() {
        use crate::core::annotation::AnnotationsEnum;
        use crate::core::engine::NodesEnum;
        use crate::standard_components::themes::THEMES;
        use crate::{AnnotationTypes, Nodes};
        let mut results = egui_kittest::SnapshotResults::new();
        for section in [Section::AutoStop, Section::Nodes, Section::RecentFiles] {
            let mut preferences = Preferences::default();
            preferences.hidden.nodes.insert("Spectrum".into());
            let mut window = SettingsWindow { section };
            let types = Types {
                nodes: Nodes::TYPES,
                annotations: AnnotationTypes::TYPES,
                base_directories: crate::components::BASE_DIRECTORIES,
            };
            let mut recent = vec![PathBuf::from("/a.json"), PathBuf::from("/b.json")];
            let mut harness = egui_kittest::Harness::builder().with_size([760.0, 600.0]).wgpu().build_ui_state(
                move |ui, preferences: &mut Preferences| {
                    let colours = &THEMES[0].colours;
                    window.show(ui.ctx(), preferences, &mut recent, &types, &Hidden::default(), colours);
                },
                preferences,
            );
            THEMES[0].apply(&harness.ctx);
            harness.run_steps(3);
            results.add(harness.try_snapshot(format!("settings_{}", section.name().to_lowercase().replace(' ', "_"))));
        }
    }

    #[test]
    fn preferences_from_an_older_version_keep_what_they_have() {
        let p: Preferences = serde_json::from_str(r#"{"hidden": {"nodes": ["Spectrum"]}}"#).unwrap();
        assert_eq!(p.stop_recording, Preferences::default().stop_recording);
        assert_eq!(p.recent_files, 10);
        assert!(p.hidden.nodes.contains("Spectrum") && p.hidden.annotations.is_empty());
    }
}
