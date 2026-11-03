//! Base directories: where recordings go, chosen in a section of the main toolbar.
//!
//! A program offers base directory types (see `Components::BASE_DIRECTORIES`), of which one's in use: the first,
//! unless another's chosen (in advanced mode, from its section's settings). Each draws its own section of the main
//! toolbar, as wide as it likes, and says what the base directory is: for the next recording, or the one going on.
//! When recording's asked for, the GUI asks it, and hands that to every node preparing to record
//! (`Node::prepare_to_record`, then `Pass::base_directory`); writers decide what to do with it (write in it, say,
//! or in a subdirectory of it). It's told when recordings start and stop, to count them, say. What it keeps (the
//! directory chosen, say) is saved with the app's preferences, as JSON, and given back when it's next made.

use std::path::PathBuf;

use eframe::egui;

use crate::core::theme::Colours;

/// A base directory type in use (see the module docs).
pub trait BaseDirectory: Send {
    /// Its section of the main toolbar.
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext);

    /// The base directory: for the recording going on, while there is one, otherwise for the next. `None` if there
    /// isn't one (none chosen yet, say).
    fn directory(&self) -> Option<PathBuf>;

    /// A recording's started, in `directory()`.
    fn recording_started(&mut self) {}

    /// The recording's stopped.
    fn recording_stopped(&mut self) {}

    /// What it keeps between runs of the program, as JSON (given back to `BaseDirectoryType::make`).
    fn save(&self) -> serde_json::Value;
}

/// What a base directory's toolbar section gets to work with.
pub struct BaseDirectoryContext<'a> {
    pub theme: &'a Colours,
    /// Whether it's recording (in `directory()`): what decides where it goes shouldn't change meanwhile.
    pub recording: bool,
}

/// A kind of base directory a program offers.
pub struct BaseDirectoryType {
    pub name: &'static str,
    /// What it is, for choosing it.
    pub description: &'static str,
    /// Makes one, from what one saved before (see `BaseDirectory::save`), if there's anything.
    pub make: fn(saved: Option<&serde_json::Value>) -> Box<dyn BaseDirectory>,
}

/// The base directory in use, and what each type saved: kept by the GUI.
pub struct BaseDirectories {
    types: &'static [BaseDirectoryType],
    /// Which type's in use, and it.
    current: Option<(usize, Box<dyn BaseDirectory>)>,
    /// What each type saved, by name, including those not in use.
    saved: serde_json::Map<String, serde_json::Value>,
}

impl BaseDirectories {
    /// Of `types`, from what was saved before (see `save`): the type chosen then, or the first.
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

    /// Which type's in use, if any's offered.
    pub fn chosen(&self) -> Option<usize> {
        self.current.as_ref().map(|(index, _)| *index)
    }

    /// Uses type `index`, keeping what the one in use had (for if it's chosen again).
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

    /// Its directory (see `BaseDirectory::directory`).
    pub fn directory(&self) -> Option<PathBuf> {
        self.current.as_ref()?.1.directory()
    }

    /// What to keep between runs of the program: which is in use, and what each has.
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

    /// A directory named in what it saved.
    struct Named(String);

    impl BaseDirectory for Named {
        fn toolbar_ui(&mut self, _ui: &mut egui::Ui, _context: &BaseDirectoryContext) {}

        fn directory(&self) -> Option<PathBuf> {
            Some(PathBuf::from(&self.0))
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
        assert_eq!((directories.chosen(), directories.directory()), (Some(0), Some("/a".into())));
        directories.choose(1);
        let saved = directories.save();
        let again = BaseDirectories::new(TYPES, Some(&saved));
        assert_eq!((again.chosen(), again.directory()), (Some(1), Some("/b".into())));
        assert_eq!(saved["types"]["a"], "/a", "what the one not in use had");
        assert_eq!(BaseDirectories::new(&[], None).directory(), None, "none offered");
    }
}
