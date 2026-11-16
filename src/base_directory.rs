//! Recordings go in a base directory, chosen in a section of the main toolbar.
//!
//! A program offers base directory types (see `Components::BASE_DIRECTORIES`), of which one's in use: the first,
//! unless another's chosen (in advanced mode, from its section's settings). Each draws its own section of the main
//! toolbar, as wide as it likes, and says where recordings go (a `Place`): for the next recording, or the one going
//! on. That's a directory, and perhaps a prefix for what's written there (a subject's trial, say), which writers use
//! rather than their own.
//!
//! When recording's asked for, the GUI asks the base directory in use where, and hands that to every node preparing for
//! the recording's snapshot (`PrepareContext::recording`, see `Node::prepare_for_snapshot`, then
//! `PassContext::base_directory`); writers decide what to do with it (write in it, say, or in a subdirectory of it).
//! The base directory's told when recordings start and stop. What each type keeps (the directory chosen, say), and
//! which is in use, is saved as JSON: with the app's preferences, and in saved files (so with each recording too), and
//! given back when it's next made or the file's opened.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::theme::Colours;

/// A recording goes in a place: the base directory, and a prefix, if there's one (otherwise writers name things
/// themselves). The prefix is the whole of what's written's name but its extension, whatever a writer makes of it (its
/// files, or a folder for them), so it should be different for each recording: nothing stops one writing over the
/// last.
#[derive(Clone, PartialEq, Debug)]
pub struct Place {
    pub directory: PathBuf,
    pub prefix: Option<String>,
}

impl Place {
    /// A place that's just a directory.
    pub fn directory(directory: impl Into<PathBuf>) -> Self {
        Place { directory: directory.into(), prefix: None }
    }

    /// The place as a path: the directory and prefix (the directory, if there's no prefix).
    pub fn path(&self) -> PathBuf {
        match &self.prefix {
            Some(prefix) => self.directory.join(prefix),
            None => self.directory.clone(),
        }
    }
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", Path::new(&self.path()).display())
    }
}

/// Each base directory type implements this (see the module docs).
pub trait BaseDirectory: Send {
    /// The base directory draws its section of the main toolbar with this.
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext);

    /// The base directory says where recordings go: the recording going on, while there is one, otherwise the next,
    /// were it to start `now` (to name it by). `None` if there's nowhere (none chosen yet, say).
    fn place(&self, now: &chrono::DateTime<chrono::Local>) -> Option<Place>;

    /// The app tells the base directory a recording's started, in `place` (what `place` said when it was asked for).
    fn recording_started(&mut self, _place: &Place) {}

    /// The app tells the base directory the recording's stopped.
    fn recording_stopped(&mut self) {}

    /// The base directory keeps this between runs of the program and in saved files, as JSON (given back to
    /// `BaseDirectoryType::make`). A saved file counts as changed when this does, so it should only be what's chosen.
    fn save(&self) -> serde_json::Value;
}

/// A base directory's toolbar section gets this to work with.
pub struct BaseDirectoryContext<'a> {
    pub theme: &'a Colours,
    /// While recording (in `place()`), what decides where it goes shouldn't change; this says whether it's recording.
    pub recording: bool,
}

/// A program offers kinds of base directory, each described by this.
pub struct BaseDirectoryType {
    pub name: &'static str,
    /// The kind's description, for choosing it.
    pub description: &'static str,
    /// The app makes a base directory of the kind with this, from what one saved before (see `BaseDirectory::save`), if
    /// there's anything.
    pub make: fn(saved: Option<&serde_json::Value>) -> Box<dyn BaseDirectory>,
}

/// The GUI keeps the base directory in use here, and what each type saved.
pub struct BaseDirectories {
    types: &'static [BaseDirectoryType],
    /// The type in use, and its base directory.
    current: Option<(usize, Box<dyn BaseDirectory>)>,
    /// What each type saved, by name, including those not in use.
    saved: serde_json::Map<String, serde_json::Value>,
}

impl BaseDirectories {
    /// The app makes this from `types`, and what was saved before (see `save`): the type chosen then, or the first.
    pub fn new(types: &'static [BaseDirectoryType], saved: Option<&serde_json::Value>) -> Self {
        let saved_types = saved.and_then(|s| s.get("types")).and_then(|t| t.as_object()).cloned().unwrap_or_default();
        let chosen = saved.and_then(|s| s.get("chosen")).and_then(|c| c.as_str());
        let index = chosen.and_then(|name| types.iter().position(|t| t.name == name)).unwrap_or(0);
        let mut directories = BaseDirectories { types, current: None, saved: saved_types };
        directories.choose(index);
        directories
    }

    /// The types offered.
    pub fn types(&self) -> &'static [BaseDirectoryType] {
        self.types
    }

    /// The type in use, if any's offered.
    pub fn chosen(&self) -> Option<usize> {
        self.current.as_ref().map(|(index, _)| *index)
    }

    /// Choosing a type uses type `index`, keeping what the one in use had (for if it's chosen again).
    pub fn choose(&mut self, index: usize) {
        if self.chosen() == Some(index) {
            return;
        }
        let Some(kind) = self.types.get(index) else { return };
        if let Some((old, current)) = self.current.take() {
            self.saved.insert(self.types[old].name.into(), current.save());
        }
        self.current = Some((index, (kind.make)(self.saved.get(kind.name))));
    }

    /// The base directory in use, if any's offered.
    pub fn current(&mut self) -> Option<&mut dyn BaseDirectory> {
        Some(self.current.as_mut()?.1.as_mut())
    }

    /// Recordings go here, if one started now (see `BaseDirectory::place`).
    pub fn place(&self) -> Option<Place> {
        self.place_at(&chrono::Local::now())
    }

    /// Recordings go here, if one started at `start` (see `BaseDirectory::place`).
    pub fn place_at(&self, start: &chrono::DateTime<chrono::Local>) -> Option<Place> {
        self.current.as_ref()?.1.place(start)
    }

    /// The app keeps this between runs of the program and in saved files: which type's in use, and what each has.
    pub fn save(&self) -> serde_json::Value {
        let mut types = self.saved.clone();
        if let Some((index, current)) = &self.current {
            types.insert(self.types[*index].name.into(), current.save());
        }
        let chosen = self.chosen().map(|i| self.types[i].name);
        serde_json::json!({ "chosen": chosen, "types": types })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test's base directory saves the directory's name.
    struct Named(String);

    impl BaseDirectory for Named {
        fn toolbar_ui(&mut self, _ui: &mut egui::Ui, _context: &BaseDirectoryContext) {}

        fn place(&self, _now: &chrono::DateTime<chrono::Local>) -> Option<Place> {
            Some(Place::directory(&self.0))
        }

        fn save(&self) -> serde_json::Value {
            serde_json::json!(self.0)
        }
    }

    fn named(saved: Option<&serde_json::Value>, default: &str) -> Box<dyn BaseDirectory> {
        Box::new(Named(saved.and_then(|s| s.as_str()).unwrap_or(default).into()))
    }

    const TYPES: &[BaseDirectoryType] = &[
        BaseDirectoryType { name: "a", description: "", make: |saved| named(saved, "/a") },
        BaseDirectoryType { name: "b", description: "", make: |saved| named(saved, "/b") },
    ];

    #[test]
    fn the_one_chosen_and_what_each_had_are_kept() {
        let mut directories = BaseDirectories::new(TYPES, None);
        assert_eq!((directories.chosen(), directories.place()), (Some(0), Some(Place::directory("/a"))));
        directories.choose(1);
        let saved = directories.save();
        let again = BaseDirectories::new(TYPES, Some(&saved));
        assert_eq!((again.chosen(), again.place()), (Some(1), Some(Place::directory("/b"))));
        assert_eq!(saved["types"]["a"], "/a", "what the one not in use had");
        assert_eq!(BaseDirectories::new(&[], None).place(), None, "none offered");
    }
}
