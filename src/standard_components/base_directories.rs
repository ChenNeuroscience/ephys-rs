//! The base directories offered (see `crate::core::base_directory`): a directory chosen, or a directory for each
//! subject, day and trial within one.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::base_directory::{BaseDirectory, BaseDirectoryContext, BaseDirectoryType};
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
        description: "Recordings go in a directory for the subject (an animal's id, say), the day and the trial, \
                      within a directory you choose: the trials counted up from 1 each day.",
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

    fn directory(&self) -> Option<PathBuf> {
        Some(expand(&self.directory))
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// A directory for each subject, day and trial, within one: `<directory>/<subject>/<date>/trial_<n>`. The trial's
/// counted up after each recording, and starts again each day, from the first not yet on disk (so after a restart,
/// it carries on); it can be changed by hand.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Trials {
    pub directory: String,
    pub subject: String,
    /// The next trial (or, while recording, this one).
    pub trial: u32,
    /// The day the trials are counted for, as in the path ("2026-11-03").
    pub day: String,
    /// The recording going on's directory, while there is one.
    #[serde(skip)]
    recording: Option<PathBuf>,
}

impl Default for Trials {
    fn default() -> Self {
        Trials { directory: NO_BASE.into(), subject: String::new(), trial: 1, day: today(), recording: None }
    }
}

/// Today, as in the path.
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// A trial's directory's name.
fn trial_name(trial: u32) -> String {
    format!("trial_{trial:02}")
}

impl Trials {
    fn from_saved(saved: Option<&serde_json::Value>) -> Self {
        let mut trials: Trials = saved.and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default();
        trials.new_day();
        trials
    }

    /// The subject's directory for the day.
    fn day_directory(&self) -> PathBuf {
        let mut directory = expand(&self.directory);
        if !self.subject.trim().is_empty() {
            directory.push(self.subject.trim());
        }
        directory.join(&self.day)
    }

    /// Trial `trial`'s directory.
    fn trial_directory(&self, trial: u32) -> PathBuf {
        self.day_directory().join(trial_name(trial))
    }

    /// The first trial after those already on disk for the subject and day.
    fn first_free(&self) -> u32 {
        let Ok(entries) = std::fs::read_dir(self.day_directory()) else { return 1 };
        let numbers = entries.flatten().filter_map(|e| e.file_name().to_str()?.strip_prefix("trial_")?.parse().ok());
        numbers.max().map_or(1, |n: u32| n + 1)
    }

    /// If it's a new day, counts trials from the first free.
    fn new_day(&mut self) {
        if self.recording.is_none() && self.day != today() {
            self.day = today();
            self.trial = self.first_free();
        }
    }
}

impl BaseDirectory for Trials {
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext) {
        self.new_day();
        let mut changed = directory_ui(ui, &mut self.directory, context, "The directory the subjects' are in");
        let subject = egui::TextEdit::singleline(&mut self.subject).hint_text("subject").desired_width(70.0);
        let response = ui.add_enabled(!context.recording, subject);
        changed |= response.changed();
        response.on_hover_text("The subject: an animal's id, say. Each has a directory, with one for each day");
        if changed {
            self.trial = self.first_free();
        }
        match &self.recording {
            Some(directory) => {
                let text = RichText::new(format!("trial {}", self.trial)).color(context.theme.error);
                ui.label(text).on_hover_text(format!("Recording in {}", directory.display()));
            }
            None => {
                let next = self.trial_directory(self.trial);
                let exists = next.exists();
                let field = egui::DragValue::new(&mut self.trial).range(1..=9999).prefix("next trial ");
                let response = ui.add(field);
                if exists {
                    let tip = format!("{} is there already: its files would be added to", next.display());
                    response.on_hover_text(RichText::new(tip).color(context.theme.warning));
                    ui.label(RichText::new("!").strong().color(context.theme.warning));
                } else {
                    response.on_hover_text(format!("The next recording goes in {}", next.display()));
                }
            }
        }
    }

    fn directory(&self) -> Option<PathBuf> {
        Some(self.recording.clone().unwrap_or_else(|| self.trial_directory(self.trial)))
    }

    fn recording_started(&mut self) {
        self.recording = Some(self.trial_directory(self.trial));
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
    fn trials_count_up_and_start_from_the_first_free() {
        let base = std::env::temp_dir().join(format!("ephys-rs-trials-{}", std::process::id()));
        let mut trials = Trials { directory: base.display().to_string(), subject: "r123".into(), ..Default::default() };
        let day = base.join("r123").join(today());
        assert_eq!(trials.directory(), Some(day.join("trial_01")));
        trials.recording_started();
        trials.recording_stopped();
        assert_eq!(trials.directory(), Some(day.join("trial_02")), "counted up after a recording");
        // Made again (the program restarted, say) on another day: from the first not on disk.
        std::fs::create_dir_all(day.join("trial_03")).unwrap();
        let saved = Trials { day: "2000-01-01".into(), ..Trials::from_saved(Some(&trials.save())) };
        let again = Trials::from_saved(Some(&saved.save()));
        assert_eq!(again.directory(), Some(day.join("trial_04")));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
