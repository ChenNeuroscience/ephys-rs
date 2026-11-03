//! The base directories offered (see `crate::core::base_directory`): a directory chosen; or a directory for each
//! subject within one, each recording named after the day and trial ("2026-11-03 a").

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::base_directory::{BaseDirectory, BaseDirectoryContext, BaseDirectoryType, Place};
use crate::core::gui::widgets::{Icon, toolbar_button};
use crate::standard_components::recording::{NO_BASE, expand};

/// In the order they're offered: the first is the default.
pub const BASE_DIRECTORIES: &[BaseDirectoryType] = &[
    BaseDirectoryType {
        name: "A directory",
        description: "Recordings go in a directory you choose.",
        make: |saved| Box::new(Chosen::from_saved(saved)),
    },
    BaseDirectoryType {
        name: "Subject, date and trial",
        description: "Recordings go in a directory for the subject (an animal's id, say), within a directory you \
                      choose, each named after the day and the trial (\"2026-11-03 a\"): the trials lettered from a \
                      each day.",
        make: |saved| Box::new(Trials::from_saved(saved)),
    },
];

/// `directory` as it's shown: the home directory as `~`.
fn shown(directory: &str) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    match home.and_then(|home| Path::new(directory).strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => directory.into(),
    }
}

/// A button to choose `directory`, then it, compactly (whole when hovered). Whether it was changed.
fn directory_ui(ui: &mut egui::Ui, directory: &mut String, context: &BaseDirectoryContext, what: &str) -> bool {
    let tip = if context.recording { format!("{what}: not while recording") } else { format!("Choose {what}") };
    let mut changed = false;
    if toolbar_button(ui, Icon::Open, &tip, !context.recording).clicked()
        && let Some(chosen) = rfd::FileDialog::new().set_directory(expand(directory)).pick_folder()
    {
        *directory = chosen.display().to_string();
        changed = true;
    }
    let text = shown(directory);
    let short = if text.chars().count() > 28 {
        format!("…{}", text.chars().rev().take(27).collect::<Vec<_>>().into_iter().rev().collect::<String>())
    } else {
        text
    };
    let label = ui.label(RichText::new(short).color(context.theme.text_secondary));
    label.on_hover_text(format!("{what}: {}", expand(directory).display()));
    changed
}

/// A directory chosen.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Chosen {
    pub directory: String,
}

impl Default for Chosen {
    fn default() -> Self {
        Chosen { directory: NO_BASE.into() }
    }
}

impl Chosen {
    fn from_saved(saved: Option<&serde_json::Value>) -> Self {
        saved.and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default()
    }
}

impl BaseDirectory for Chosen {
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext) {
        directory_ui(ui, &mut self.directory, context, "Where recordings go");
    }

    fn place(&self) -> Option<Place> {
        Some(Place::directory(expand(&self.directory)))
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// A directory for each subject, within one, each recording named after the day and trial: in
/// `<directory>/<subject>/`, named `<date> <trial>` ("2026-11-03 a"; writers decide what that names: files, or a
/// folder). Trials are lettered a to z, then 1a to 1z, 2a...; the trial's counted up after each recording, and
/// starts again each day from the first not yet on disk (so after a restart, it carries on); it can be changed by hand.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Trials {
    pub directory: String,
    pub subject: String,
    /// The next trial (or, while recording, this one), counting from 1 (a).
    pub trial: u32,
    /// The day the trials are counted for, as in the name ("2026-11-03").
    pub day: String,
    /// Where the recording going on is, while there is one.
    #[serde(skip)]
    recording: Option<Place>,
}

impl Default for Trials {
    fn default() -> Self {
        Trials { directory: NO_BASE.into(), subject: String::new(), trial: 1, day: today(), recording: None }
    }
}

/// Today, as in the name. In tests, always the same day, so the toolbar looks the same every run.
fn today() -> String {
    if cfg!(test) { "2026-01-01".into() } else { chrono::Local::now().format("%Y-%m-%d").to_string() }
}

/// A trial's letters: 1 is a, 26 z, 27 1a, 52 1z, 53 2a...
pub fn trial_letters(trial: u32) -> String {
    let (round, letter) = ((trial.max(1) - 1) / 26, (trial.max(1) - 1) % 26);
    let letter = char::from(b'a' + letter as u8);
    if round == 0 { letter.to_string() } else { format!("{round}{letter}") }
}

/// The trial letters name ("2", "1a"...), if they do.
pub fn trial_number(letters: &str) -> Option<u32> {
    let letters = letters.trim().to_ascii_lowercase();
    let letter = letters.chars().last().filter(char::is_ascii_lowercase)?;
    let round = &letters[..letters.len() - 1];
    let round: u32 = if round.is_empty() { 0 } else { round.parse().ok()? };
    Some(round * 26 + (letter as u32 - 'a' as u32) + 1)
}

impl Trials {
    fn from_saved(saved: Option<&serde_json::Value>) -> Self {
        let mut trials: Trials = saved.and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default();
        trials.new_day();
        trials
    }

    /// The subject's directory.
    fn subject_directory(&self) -> PathBuf {
        let directory = expand(&self.directory);
        if self.subject.trim().is_empty() { directory } else { directory.join(self.subject.trim()) }
    }

    /// Trial `trial`'s name.
    fn name(&self, trial: u32) -> String {
        format!("{} {}", self.day, trial_letters(trial))
    }

    /// The trials of the day already on disk: the names in the subject's directory starting with the day's, then
    /// a trial's letters (then an extension, perhaps).
    fn on_disk(&self) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir(self.subject_directory()) else { return Vec::new() };
        let prefix = format!("{} ", self.day);
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_owned();
                let rest = name.strip_prefix(&prefix)?;
                trial_number(rest.split('.').next()?)
            })
            .collect()
    }

    /// The first trial after those already on disk for the subject and day.
    fn first_free(&self) -> u32 {
        self.on_disk().into_iter().max().map_or(1, |n| n + 1)
    }

    /// If it's a new day, letters trials from the first free.
    fn new_day(&mut self) {
        if self.recording.is_none() && self.day != today() {
            self.day = today();
            self.trial = self.first_free();
        }
    }
}

impl BaseDirectory for Trials {
    /// As a path: the directory, the subject, then the day and trial: "~/recordings / r2405 / 2026-11-03 a".
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext) {
        self.new_day();
        let separator = |ui: &mut egui::Ui| {
            ui.label(RichText::new(std::path::MAIN_SEPARATOR_STR).color(context.theme.text_disabled));
        };
        let mut changed = directory_ui(ui, &mut self.directory, context, "The directory the subjects' are in");
        separator(ui);
        let subject = egui::TextEdit::singleline(&mut self.subject).hint_text("subject").desired_width(60.0);
        let response = ui.add_enabled(!context.recording, subject);
        changed |= response.changed();
        response.on_hover_text("The subject (an animal's id, say): a directory of its own");
        if changed {
            self.trial = self.first_free();
        }
        separator(ui);
        ui.spacing_mut().item_spacing.x = 4.0;
        let day = ui.label(RichText::new(&self.day).color(context.theme.text_secondary));
        day.on_hover_text("Each recording's named after the day and trial (the writers' files, or folders, are)");
        match &self.recording {
            Some(place) => {
                let text = RichText::new(trial_letters(self.trial)).strong().color(context.theme.error);
                ui.label(text).on_hover_text(format!("The trial recording now: {place}"));
            }
            None => {
                let next = self.name(self.trial);
                let taken = self.on_disk().contains(&self.trial);
                let field = egui::DragValue::new(&mut self.trial)
                    .range(1..=9999)
                    .custom_formatter(|n, _| trial_letters(n as u32))
                    .custom_parser(|text| trial_number(text).map(f64::from));
                let field = if taken { field.prefix("⚠ ") } else { field };
                ui.spacing_mut().interact_size.x = 20.0; // as narrow as its letters
                let response = ui.add(field);
                let place = self.subject_directory().join(&next);
                if taken {
                    let tip = format!("The next trial: {} is there already, and would be added to", place.display());
                    response.on_hover_text(RichText::new(tip).color(context.theme.warning));
                } else {
                    response.on_hover_text(format!("The next trial: {}", place.display()));
                }
            }
        }
    }

    fn place(&self) -> Option<Place> {
        let next = || Place { directory: self.subject_directory(), name: Some(self.name(self.trial)) };
        Some(self.recording.clone().unwrap_or_else(next))
    }

    fn recording_started(&mut self) {
        self.recording = self.place();
    }

    fn recording_stopped(&mut self) {
        if self.recording.take().is_some() {
            self.trial += 1;
        }
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trials_are_lettered() {
        let letters: Vec<String> = [1, 2, 26, 27, 52, 53].into_iter().map(trial_letters).collect();
        assert_eq!(letters, ["a", "b", "z", "1a", "1z", "2a"]);
        assert!([1, 2, 26, 27, 52, 53, 1000].into_iter().all(|n| trial_number(&trial_letters(n)) == Some(n)));
        assert_eq!((trial_number("B"), trial_number("x1"), trial_number("")), (Some(2), None, None));
    }

    #[test]
    fn trials_count_up_and_start_from_the_first_free() {
        let base = std::env::temp_dir().join(format!("ephys-rs-trials-{}", std::process::id()));
        let mut trials = Trials { directory: base.display().to_string(), subject: "r123".into(), ..Default::default() };
        let subject = base.join("r123");
        let named =
            |trial: &str| Some(Place { directory: subject.clone(), name: Some(format!("{} {trial}", today())) });
        assert_eq!(trials.place(), named("a"));
        trials.recording_started();
        trials.recording_stopped();
        assert_eq!(trials.place(), named("b"), "counted up after a recording");
        // Made again (the program restarted, say) on another day: from the first not on disk (an LTX recording's
        // files, say).
        std::fs::create_dir_all(&subject).unwrap();
        std::fs::write(subject.join(format!("{} c.set", today())), "").unwrap();
        let saved = Trials { day: "2000-01-01".into(), ..Trials::from_saved(Some(&trials.save())) };
        let again = Trials::from_saved(Some(&saved.save()));
        assert_eq!(again.place(), named("d"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
