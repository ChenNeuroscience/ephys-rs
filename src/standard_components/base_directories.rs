//! The standard base directory (see `crate::core::base_directory`): a directory chosen.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::base_directory::{BaseDirectory, BaseDirectoryContext, BaseDirectoryType, Place};
use crate::core::gui::widgets::{Icon, toolbar_button};
use crate::standard_components::recording::{NO_BASE, expand};

/// A directory you choose.
pub const A_DIRECTORY: BaseDirectoryType = BaseDirectoryType {
    name: "A directory",
    description: "Recordings go in a directory you choose.",
    make: |saved| Box::new(Chosen::from_saved(saved)),
};

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
pub fn directory_ui(ui: &mut egui::Ui, directory: &mut String, context: &BaseDirectoryContext, what: &str) -> bool {
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
