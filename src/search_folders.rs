//! Folders to look in for files a component needs from elsewhere (a vendor's library, an FPGA's bitfile), as global
//! settings keep them (see `crate::global_settings`): the folders chosen first, then where the files usually are.

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::theme::Colours;

/// Folders to look in first, in order.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SearchFolders(pub Vec<PathBuf>);

impl SearchFolders {
    /// Where to look for a file of any of `names`: in each of these folders, then in each of `then`.
    pub fn candidates(&self, names: &[&str], then: &[PathBuf]) -> Vec<PathBuf> {
        let folders = self.0.iter().chain(then);
        folders.flat_map(|folder| names.iter().map(move |name| folder.join(name))).collect()
    }

    /// The first of `candidates` that's there.
    pub fn find(&self, names: &[&str], then: &[PathBuf]) -> Option<PathBuf> {
        self.candidates(names, then).into_iter().find(|p| p.is_file())
    }

    /// The folders, each to remove, and a button to add one.
    pub fn ui(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let mut remove = None;
        for (i, folder) in self.0.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                let missing = !folder.is_dir();
                let text = RichText::new(folder.display().to_string());
                ui.label(if missing { text.color(colours.warning) } else { text }).on_hover_text(if missing {
                    "Not found"
                } else {
                    ""
                });
            });
        }
        if let Some(i) = remove {
            self.0.remove(i);
        }
        if self.0.is_empty() {
            ui.label(RichText::new("None").color(colours.text_secondary));
        }
        if ui.button("Add a folder…").clicked()
            && let Some(folder) = rfd::FileDialog::new().pick_folder()
        {
            self.0.push(folder);
        }
    }
}

/// Where to get the files, if they're missing: a link to each place, with what's there.
pub fn get_them_ui(ui: &mut egui::Ui, colours: &Colours, links: &[(&str, &str)]) {
    ui.label(RichText::new("Where to get them").small().color(colours.text_secondary));
    for (what, url) in links {
        ui.hyperlink_to(*what, *url);
    }
}

/// Said under the files found, by a library's settings.
pub const LOADED_ONCE: &str =
    "A library's kept once it's been loaded: a change to where it's found applies when ephys-rs next starts.";

/// Explains a setting, small and secondary.
pub fn note(ui: &mut egui::Ui, colours: &Colours, text: &str) {
    ui.label(RichText::new(text).small().color(colours.text_secondary));
    ui.add_space(4.0);
}

/// A line for each file (what it is, and where it'd be found), as a grid.
pub fn found_ui(ui: &mut egui::Ui, colours: &Colours, files: &[(&str, Option<PathBuf>)]) {
    egui::Grid::new("found").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        for (what, found) in files {
            ui.label(*what);
            match found {
                Some(path) => ui.label(RichText::new(path.display().to_string()).small()),
                None => ui.label(RichText::new("not found").small().color(colours.warning)),
            };
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_chosen_come_first_and_each_name_is_tried_in_each() {
        let folders = SearchFolders(vec!["/a".into()]);
        let candidates = folders.candidates(&["x.1.dylib", "x.dylib"], &["/b".into()]);
        let expected: Vec<PathBuf> =
            ["/a/x.1.dylib", "/a/x.dylib", "/b/x.1.dylib", "/b/x.dylib"].iter().map(PathBuf::from).collect();
        assert_eq!(candidates, expected);
    }

    #[test]
    fn found_is_the_first_there() {
        let dir = std::env::temp_dir().join(format!("search-folders-{}", std::process::id()));
        let (first, second) = (dir.join("first"), dir.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("lib"), b"").unwrap();
        let folders = SearchFolders(vec![first.clone(), second.clone()]);
        assert_eq!(folders.find(&["lib"], &[]), Some(second.join("lib")));
        std::fs::write(first.join("lib"), b"").unwrap();
        assert_eq!(folders.find(&["lib"], &[]), Some(first.join("lib")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
