//! The standard base directory (see `crate::base_directory`): a directory chosen, and perhaps a file prefix.

use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

use eframe::egui::{self, Order, RichText, Sense, TextStyle, TextWrapMode, WidgetText};
use serde::{Deserialize, Serialize};

use crate::base_directory::{BaseDirectory, BaseDirectoryContext, BaseDirectoryType, Place};
use crate::gui::widgets::{Icon, toolbar_button};
use crate::standard::recording::{NO_BASE, expand, session_name};

/// A directory you choose.
pub const A_DIRECTORY: BaseDirectoryType = BaseDirectoryType {
    name: "A directory",
    description: "Recordings go in a directory you choose, with a file prefix if you give one.",
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
/// it says what it is, right below it. See the toolbar's other pieces in `Trials` (in chen-neuro), say.
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

/// Text being typed into a piece of a path, in place of its chip.
pub struct PathEdit {
    pub text: String,
    /// Whether it's been given the focus yet.
    focused: bool,
}

impl PathEdit {
    pub fn new(text: impl Into<String>) -> Self {
        PathEdit { text: text.into(), focused: false }
    }

    /// Its id, in `ui`.
    pub fn id(ui: &egui::Ui) -> egui::Id {
        ui.scope_id().with("changing a piece of the path")
    }

    /// The text box. When it's finished, whether to apply it: Enter or clicking elsewhere does, Esc cancels.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<bool> {
        let width = 8.0 * (self.text.chars().count() + 1) as f32;
        let edit = egui::TextEdit::singleline(&mut self.text)
            .id(Self::id(ui))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::symmetric(3, 1))
            .desired_width(width.max(24.0))
            .lock_focus(true);
        let response = ui.add(edit);
        if !self.focused {
            response.request_focus();
            self.focused = true;
        }
        response.lost_focus().then(|| !ui.input(|i| i.key_pressed(egui::Key::Escape)))
    }
}

/// `text`, to go in a file's name: a separator would make it a path.
pub fn name_piece(text: &str) -> String {
    text.trim().replace(['/', '\\'], "-")
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

/// A directory chosen, and perhaps a file prefix: recordings are named after it and when they start (so one doesn't
/// write over the last), otherwise by the writers.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Chosen {
    pub directory: String,
    pub prefix: String,
    /// Where the recording going on is, while there is one.
    #[serde(skip)]
    recording: Option<Place>,
    /// The prefix being changed, if it is.
    #[serde(skip)]
    editing: Option<PathEdit>,
}

impl Default for Chosen {
    fn default() -> Self {
        Chosen { directory: NO_BASE.into(), prefix: String::new(), recording: None, editing: None }
    }
}

impl Chosen {
    fn from_saved(saved: Option<&serde_json::Value>) -> Self {
        saved.and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default()
    }

    /// The prefix, as the last piece of the path: clicked to change it, in place.
    fn prefix_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext) {
        if let Some(editing) = &mut self.editing {
            if let Some(apply) = editing.ui(ui) {
                if apply {
                    self.prefix = name_piece(&editing.text);
                }
                self.editing = None;
            }
            return;
        }
        let theme = context.theme;
        let prefix = self.prefix.trim();
        if let Some(name) = self.recording.as_ref().and_then(|place| place.prefix.as_deref()) {
            path_chip(ui, RichText::new(name).color(theme.text_primary), "The file prefix", false, context);
        } else if !context.recording {
            let example = session_name(if prefix.is_empty() { "prefix" } else { prefix }, &chrono::Local::now());
            let info = format!(
                "The file prefix, if you want one: recordings are named after it and when they start \
                 (\"{example}\"), otherwise the writers name them\nClick to change it"
            );
            let text = match prefix {
                "" => RichText::new("file prefix").color(theme.text_disabled),
                prefix => RichText::new(prefix).color(theme.text_primary),
            };
            if path_chip(ui, text, info, true, context).clicked() {
                self.editing = Some(PathEdit::new(prefix));
            }
        }
    }
}

impl BaseDirectory for Chosen {
    /// As a flat path, of pale pieces: "~/recordings/mouse1".
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, context: &BaseDirectoryContext) {
        if context.recording {
            self.editing = None;
        }
        open_button(ui, &expand(&self.directory));
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            directory_ui(ui, &mut self.directory, context, "Where recordings go");
            if !context.recording || self.recording.as_ref().is_some_and(|place| place.prefix.is_some()) {
                path_separator(ui, context);
                self.prefix_ui(ui, context);
            }
        });
    }

    fn place(&self, now: &chrono::DateTime<chrono::Local>) -> Option<Place> {
        if self.recording.is_some() {
            return self.recording.clone();
        }
        let prefix = self.prefix.trim();
        let prefix = (!prefix.is_empty()).then(|| session_name(prefix, now));
        Some(Place { directory: expand(&self.directory), prefix })
    }

    fn recording_started(&mut self, place: &Place) {
        self.recording = Some(place.clone());
    }

    fn recording_stopped(&mut self) {
        self.recording = None;
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

    #[test]
    fn a_prefix_is_optional_and_stays_the_same_through_a_recording() {
        use chrono::TimeZone;
        let (start, later) = (chrono::Local.with_ymd_and_hms(2026, 1, 1, 14, 30, 5).unwrap(), chrono::Local::now());
        let mut chosen = Chosen { directory: "/r".into(), ..Default::default() };
        assert_eq!(chosen.place(&start), Some(Place::directory("/r")), "the writers name it");

        chosen.prefix = "mouse1".into();
        let place = chosen.place(&start).unwrap();
        let prefix = place.prefix.clone().unwrap();
        assert!(prefix.starts_with("mouse1_2026"), "with when it starts, so as not to write over the last: {prefix}");
        chosen.recording_started(&place);
        assert_eq!(chosen.place(&later).as_ref(), Some(&place), "whatever the time");
        chosen.recording_stopped();
        assert_ne!(chosen.place(&later).as_ref(), Some(&place), "the next one's its own");

        let saved = chosen.save();
        assert_eq!(saved, serde_json::json!({ "directory": "/r", "prefix": "mouse1" }));
        assert_eq!(Chosen::from_saved(Some(&serde_json::json!({ "directory": "/r" }))).prefix, "", "saved before");
    }
}
