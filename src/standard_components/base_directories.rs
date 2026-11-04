//! The standard base directory (see `crate::core::base_directory`): a directory chosen.

use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

use eframe::egui::{self, Order, RichText, Sense, TextStyle, TextWrapMode, WidgetText};
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

/// `directory` as it's shown, with this system's separators: the home directory as `~`.
fn shown(directory: &str) -> String {
    let full = expand(directory);
    let home = expand("~");
    let text = match full.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
        Err(_) => full.display().to_string(),
    };
    text.replace(['/', '\\'], MAIN_SEPARATOR_STR)
}

/// A separator between the pieces of a path in the toolbar.
pub fn path_separator(ui: &mut egui::Ui, context: &BaseDirectoryContext) {
    ui.label(RichText::new(MAIN_SEPARATOR_STR).color(context.theme.text_secondary));
}

/// A piece of a path in the toolbar: pale, just enough to hint that it's a piece, and clicked if `clickable`. Hovering
/// it says what it is, right below it. See the toolbar's other pieces in `Trials` (in the example), say.
pub fn path_chip(
    ui: &mut egui::Ui,
    text: RichText,
    info: impl Into<WidgetText>,
    clickable: bool,
    context: &BaseDirectoryContext,
) -> egui::Response {
    let galley = WidgetText::from(text).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
    let pad = egui::vec2(3.0, 1.0);
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(galley.size() + 2.0 * pad, sense);
    let label = galley.text().to_owned();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, clickable, &label));
    let theme = context.theme;
    let fill = match (response.hovered(), clickable) {
        (true, true) => theme.control_hovered,
        (true, false) => theme.text_primary.gamma_multiply(0.16),
        (false, _) => theme.text_primary.gamma_multiply(0.08),
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    ui.painter().galley(rect.min + pad, galley, theme.text_secondary);
    if response.hovered() {
        let at = rect.left_bottom() + egui::vec2(0.0, 5.0);
        egui::Area::new(response.id.with("info")).order(Order::Tooltip).interactable(false).fixed_pos(at).show(
            ui.ctx(),
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(460.0);
                    ui.label(info);
                });
            },
        );
    }
    if clickable { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

/// A button to open `directory`, or as much of it as there is yet, in the system's file manager.
pub fn open_button(ui: &mut egui::Ui, directory: &Path) {
    let tip = format!("Open where recordings go: {}", directory.display());
    if toolbar_button(ui, Icon::Open, &tip, true).clicked() {
        open_directory(directory.to_owned());
    }
}

/// Opens `directory`, or as much of it as there is, on a thread of its own, as a disk can take a while to answer.
fn open_directory(directory: PathBuf) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let open = move || {
        let there = directory.ancestors().find(|path| path.exists()).unwrap_or(&directory);
        if let Err(e) = std::process::Command::new(opener).arg(there).status() {
            log::error!("Couldn't open {}: {e}", there.display());
        }
    };
    if let Err(e) = std::thread::Builder::new().name("opening a directory".into()).spawn(open) {
        log::error!("Couldn't open the directory: {e}");
    }
}

/// `directory`, as a piece of a path, compactly: clicked to choose another. Whether it was changed.
pub fn directory_ui(ui: &mut egui::Ui, directory: &mut String, context: &BaseDirectoryContext, what: &str) -> bool {
    let text = shown(directory);
    let short = if text.chars().count() > 28 {
        format!("…{}", text.chars().rev().take(27).collect::<Vec<_>>().into_iter().rev().collect::<String>())
    } else {
        text
    };
    let whole = expand(directory);
    let info = if context.recording {
        format!("{what}: {}", whole.display())
    } else {
        format!("{what}: {}\nClick to choose another", whole.display())
    };
    let text = RichText::new(short).color(context.theme.text_primary);
    if path_chip(ui, text, info, !context.recording, context).clicked()
        && let Some(chosen) = rfd::FileDialog::new().set_directory(&whole).pick_folder()
    {
        *directory = chosen.display().to_string();
        return true;
    }
    false
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
        open_button(ui, &expand(&self.directory));
        directory_ui(ui, &mut self.directory, context, "Where recordings go");
    }

    fn place(&self) -> Option<Place> {
        Some(Place::directory(expand(&self.directory)))
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_shown_with_this_systems_separators() {
        let sep = MAIN_SEPARATOR;
        assert_eq!(shown("~"), "~");
        assert_eq!(shown("~/recordings/r1"), format!("~{sep}recordings{sep}r1"));
        assert_eq!(shown(&expand("~/recordings").display().to_string()), format!("~{sep}recordings"), "the home");
        assert_eq!(shown("/data/recordings"), format!("{sep}data{sep}recordings"));
    }
}
