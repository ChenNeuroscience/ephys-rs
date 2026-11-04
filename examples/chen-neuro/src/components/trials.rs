//! Where chen-neuro's recordings go: a directory for each subject, within one chosen, and in it, one for each day, with
//! a file prefix mirroring them: the subject, day and trial ("r2405 2026-11-03 a"). See
//! `ephys_rs::core::base_directory`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use ephys_rs::core::base_directory::{BaseDirectory, BaseDirectoryContext, BaseDirectoryType, Place};
use ephys_rs::standard_components::base_directories::{
    PathEdit, directory_ui, name_piece, open_button, path_chip, path_separator,
};
use ephys_rs::standard_components::recording::{NO_BASE, expand};
use serde::{Deserialize, Serialize};

pub const TRIALS: BaseDirectoryType = BaseDirectoryType {
    name: "Subject, date and trial",
    description: "Recordings go in a directory for the day, in one for the subject (an animal's id, say), within a \
                  directory you choose, with a file prefix of the subject, day and trial (\"r2405 2026-11-03 a\"): \
                  the next trial is the letter after the last one there.",
    make: |saved| Box::new(Trials::from_saved(saved)),
};

/// A directory for each subject, within one, and in it, one for each day, with a prefix mirroring them: in
/// `<directory>/<subject>/<date>/`, with the prefix `<subject> <date> <trial>` ("r2405 2026-11-03 a"). Trials are lettered
/// a to z, then 1a to 1z, 2a...; the next is the one after the last on disk, so an empty directory, or a new day, starts
/// at a. It can be chosen by hand, for the next recording.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Trials {
    pub directory: String,
    pub subject: String,
    /// The day and trial chosen by hand for the next recording, if one was.
    #[serde(skip)]
    chosen: Option<(String, u32)>,
    /// Where the recording going on is, while there is one.
    #[serde(skip)]
    recording: Option<Place>,
    /// The piece of the path being changed, if one is.
    #[serde(skip)]
    editing: Option<Editing>,
    /// Which of the subject's two pieces was hovered, for the other to show it's the same.
    #[serde(skip)]
    subject_hovered: Option<Piece>,
    /// What was last seen on disk (`place` is asked for every frame, so it isn't looked at every time).
    #[serde(skip)]
    looked: RefCell<Option<Looked>>,
}

/// A piece of the path that can be changed.
#[derive(Clone, Copy, PartialEq)]
enum Piece {
    /// The subject's directory.
    Subject,
    /// The subject in the prefix: the same subject, so changing either changes both.
    SubjectInPrefix,
    Trial,
}

impl Piece {
    fn is_subject(self) -> bool {
        self != Piece::Trial
    }
}

/// A piece being changed, in place.
struct Editing {
    piece: Piece,
    edit: PathEdit,
}

/// The trials on disk for `day`, as of `at`.
struct Looked {
    at: Instant,
    day: String,
    trials: Vec<u32>,
}

/// How long what's on disk is taken to stay as it was.
const LOOK_EVERY: Duration = Duration::from_secs(1);

impl Default for Trials {
    fn default() -> Self {
        Trials {
            directory: NO_BASE.into(),
            subject: String::new(),
            chosen: None,
            recording: None,
            editing: None,
            subject_hovered: None,
            looked: RefCell::new(None),
        }
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

/// The day and trial in a prefix, which might have a subject before them (with spaces in, even).
fn day_and_trial(prefix: &str) -> Option<(&str, &str)> {
    let (rest, letters) = prefix.rsplit_once(' ')?;
    Some((rest.rsplit(' ').next()?, letters))
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
        saved.and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default()
    }

    /// The subject's directory.
    fn subject_directory(&self) -> PathBuf {
        let directory = expand(&self.directory);
        if self.subject.trim().is_empty() { directory } else { directory.join(self.subject.trim()) }
    }

    /// What every prefix of `day` starts with: the subject, if there is one, and the day.
    fn prefix_of_day(&self, day: &str) -> String {
        match self.subject.trim() {
            "" => format!("{day} "),
            subject => format!("{subject} {day} "),
        }
    }

    /// Where trial `trial` of `day` goes.
    fn place_of(&self, day: &str, trial: u32) -> Place {
        let prefix = format!("{}{}", self.prefix_of_day(day), trial_letters(trial));
        Place { directory: self.subject_directory().join(day), prefix: Some(prefix) }
    }

    /// The trials of `day` in its directory: the names starting with its prefix, then a trial's letters (then an
    /// extension, perhaps).
    fn read_trials(&self, day: &str) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir(self.subject_directory().join(day)) else { return Vec::new() };
        let prefix = self.prefix_of_day(day);
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_owned();
                let rest = name.strip_prefix(&prefix)?;
                trial_number(rest.split('.').next()?)
            })
            .collect()
    }

    /// Today's trials already on disk.
    fn on_disk(&self) -> Vec<u32> {
        let day = today();
        let mut looked = self.looked.borrow_mut();
        match &*looked {
            Some(seen) if seen.day == day && seen.at.elapsed() < LOOK_EVERY => seen.trials.clone(),
            _ => {
                let trials = self.read_trials(&day);
                *looked = Some(Looked { at: Instant::now(), day, trials: trials.clone() });
                trials
            }
        }
    }

    /// The trial after the last of today's on disk (so gaps aren't filled, and trials stay in the order they were made).
    fn after_last(&self) -> u32 {
        self.on_disk().into_iter().max().map_or(1, |last| last + 1)
    }

    /// The next trial today: the one chosen, or the one after the last.
    fn next(&self) -> u32 {
        match &self.chosen {
            Some((day, trial)) if *day == today() => *trial,
            _ => self.after_last(),
        }
    }

    /// What's next is worked out afresh.
    fn start_again(&mut self) {
        self.chosen = None;
        self.looked.take();
    }
}

impl Trials {
    /// The subject, or the trial, as a piece of the path, unless it's being changed: clicked to change it, in place.
    fn piece_ui(
        &mut self,
        ui: &mut egui::Ui,
        piece: Piece,
        text: RichText,
        info: impl Into<egui::WidgetText>,
        context: &BaseDirectoryContext,
    ) -> Option<egui::Response> {
        if self.editing.as_ref().is_some_and(|editing| editing.piece == piece) {
            self.editing_ui(ui);
            return None;
        }
        let response = path_chip(ui, text, info, !context.recording, context);
        if response.clicked() {
            let text = if piece.is_subject() { self.subject.clone() } else { trial_letters(self.next()) };
            self.editing = Some(Editing { piece, edit: PathEdit::new(text) });
        }
        Some(response)
    }

    /// One of the subject's two pieces: it follows what's typed into the other, and lights up with it.
    fn subject_ui(&mut self, ui: &mut egui::Ui, piece: Piece, context: &BaseDirectoryContext) {
        let theme = context.theme;
        let typed = self.editing.as_ref().filter(|editing| editing.piece.is_subject());
        let subject = typed.map_or(self.subject.trim(), |editing| editing.edit.text.trim());
        let text = if subject.is_empty() {
            RichText::new("subject").color(theme.text_disabled)
        } else {
            RichText::new(subject).color(theme.text_primary)
        };
        let info = "The subject (an animal's id, say): its directory, and in the file prefix";
        let info = if context.recording { info.to_owned() } else { format!("{info}\nClick to change both") };
        let response = self.piece_ui(ui, piece, text, info, context);
        let hovered = response.as_ref().is_some_and(egui::Response::hovered);
        let other_hovered = self.subject_hovered.is_some_and(|other| other != piece);
        if let (Some(response), true) = (&response, other_hovered) {
            ui.painter().rect_filled(response.rect, 3.0, theme.text_primary.gamma_multiply(0.16));
        }
        if !other_hovered && hovered != self.subject_hovered.is_some() {
            self.subject_hovered = hovered.then_some(piece);
            ui.ctx().request_repaint();
        }
    }

    /// The piece being changed: as a `PathEdit`, and up and down step a trial.
    fn editing_ui(&mut self, ui: &mut egui::Ui) {
        let next = self.next();
        let Some(editing) = &mut self.editing else { return };
        let id = PathEdit::id(ui);
        if editing.piece == Piece::Trial && ui.memory(|m| m.has_focus(id)) {
            for (key, step) in [(egui::Key::ArrowUp, 1), (egui::Key::ArrowDown, -1)] {
                while ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                    let trial = trial_number(&editing.edit.text).unwrap_or(next);
                    editing.edit.text = trial_letters(trial.saturating_add_signed(step));
                }
            }
        }
        let Some(apply) = editing.edit.ui(ui) else { return };
        if let (Some(editing), true) = (self.editing.take(), apply) {
            if editing.piece.is_subject() {
                self.subject = name_piece(&editing.edit.text);
                self.start_again();
            } else if let Some(trial) = trial_number(&editing.edit.text) {
                self.chosen = (trial != self.after_last()).then_some((today(), trial));
            }
        }
    }
}

impl BaseDirectory for Trials {
    /// As a flat path, of pale pieces: the directory, the subject, the day, then the subject, day and trial:
    /// "~/recordings/r2405/2026-11-03/r2405 2026-11-03 a".
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext) {
        if context.recording {
            self.editing = None;
        }
        let theme = context.theme;
        let now = self.recording.as_ref().and_then(|place| day_and_trial(place.prefix.as_deref()?));
        let now = now.map(|(day, letters)| (day.to_owned(), letters.to_owned()));
        let day = now.as_ref().map_or_else(today, |(day, _)| day.clone());
        let place = self.recording.clone().unwrap_or_else(|| self.place_of(&day, self.next()));
        open_button(ui, &place.directory);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if directory_ui(ui, &mut self.directory, context, "The directory the subjects are in") {
                self.start_again();
            }
            path_separator(ui, context);

            self.subject_ui(ui, Piece::Subject, context);
            path_separator(ui, context);

            let text = RichText::new(&day).color(theme.text_secondary);
            path_chip(ui, text.clone(), "The day: a directory of its own", false, context);
            path_separator(ui, context);
            self.subject_ui(ui, Piece::SubjectInPrefix, context);
            ui.add_space(3.0);
            let info = "The day, in the file prefix: the writers start their files', or folders', names with it";
            path_chip(ui, text, info, false, context);
            ui.add_space(3.0);

            match now {
                Some((_, letters)) => {
                    let text = RichText::new(letters).strong().color(theme.error);
                    path_chip(ui, text, format!("The trial recording now: {place}"), false, context);
                }
                None => {
                    let trial = self.next();
                    let letters = trial_letters(trial);
                    if self.on_disk().contains(&trial) {
                        let tip = format!("The next trial: {place} is there already, and would be added to");
                        let text = RichText::new(format!("⚠ {letters}")).color(theme.warning);
                        let info = RichText::new(tip).color(theme.warning);
                        self.piece_ui(ui, Piece::Trial, text, info, context);
                    } else {
                        let text = RichText::new(letters).color(theme.text_primary);
                        let info = format!("The next trial: {place}\nClick to change it, for this recording");
                        self.piece_ui(ui, Piece::Trial, text, info, context);
                    }
                }
            }
        });
    }

    fn place(&self) -> Option<Place> {
        Some(self.recording.clone().unwrap_or_else(|| self.place_of(&today(), self.next())))
    }

    fn recording_started(&mut self, place: &Place) {
        self.recording = Some(place.clone());
    }

    fn recording_stopped(&mut self) {
        self.recording = None;
        self.start_again();
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
    fn a_prefix_gives_its_day_and_trial() {
        assert_eq!(day_and_trial("r2405 2026-11-03 a"), Some(("2026-11-03", "a")));
        assert_eq!(day_and_trial("rat 7 2026-11-03 1b"), Some(("2026-11-03", "1b")), "a subject with a space");
        assert_eq!(day_and_trial("2026-11-03 a"), Some(("2026-11-03", "a")), "no subject");
    }

    #[test]
    fn the_next_trial_is_after_the_last_on_disk() {
        let base = std::env::temp_dir().join(format!("ephys-rs-trials-{}", std::process::id()));
        let mut trials = Trials { directory: base.display().to_string(), subject: "r123".into(), ..Default::default() };
        let day = base.join("r123").join(today());
        let name = |trial: &str| format!("r123 {} {trial}", today());
        let named = |trial: &str| Some(Place { directory: day.clone(), prefix: Some(name(trial)) });
        assert_eq!(trials.place(), named("a"), "nothing's there yet");

        // A recording: its settings are there (and a writer's files, say) by when it stops.
        std::fs::create_dir_all(&day).unwrap();
        let place = trials.place().unwrap();
        trials.recording_started(&place);
        std::fs::write(day.join(format!("{}.ephys-rs.json", name("a"))), "").unwrap();
        trials.looked.take();
        assert_eq!(trials.place(), named("a"), "where the recording is, whatever's on disk by now");
        trials.recording_stopped();
        assert_eq!(trials.place(), named("b"));

        // After the last, leaving any gap; other days', and other things, don't count.
        std::fs::create_dir(day.join(name("b"))).unwrap();
        std::fs::write(day.join(format!("{}.set", name("d"))), "").unwrap();
        std::fs::write(day.join(format!("{}.txt", name("notes"))), "").unwrap();
        std::fs::write(day.join("r123 2000-01-01 x.set"), "").unwrap();
        std::fs::write(day.join(format!("r999 {} x.set", today())), "").unwrap();
        trials.looked.take();
        assert_eq!(trials.place(), named("e"));

        // Chosen by hand: for the next recording, then carrying on from it.
        trials.chosen = Some((today(), 6));
        assert_eq!(trials.place(), named("f"));
        trials.recording_started(&trials.place().unwrap());
        std::fs::write(day.join(format!("{}.set", name("f"))), "").unwrap();
        trials.recording_stopped();
        assert_eq!(trials.place(), named("g"));

        // What's kept is what was chosen: what's next is on disk.
        let saved = trials.save();
        assert_eq!(saved, serde_json::json!({ "directory": base.display().to_string(), "subject": "r123" }));
        assert_eq!(Trials::from_saved(Some(&saved)).place(), named("g"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
