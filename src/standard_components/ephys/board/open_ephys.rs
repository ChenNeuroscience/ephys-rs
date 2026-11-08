//! Where the real boards' libraries and bitfiles come from: the Open Ephys GUI's acquisition board plugin, which
//! keeps them in a `shared` folder of the GUI's. Which folder is a global setting (`OpenEphysSettings`, a tab in
//! Settings…): the GUI's own, found as it is, or one chosen. Nothing's looked for anywhere else.

use std::io;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::core::global_settings::GlobalSettings;
use crate::core::search_folders;
use crate::core::theme::Colours;

/// Opal Kelly's FrontPanel library, for the boards on Opal Kelly FPGAs.
#[cfg(windows)]
pub const FRONTPANEL: &[&str] = &["okFrontPanel.dll"];
#[cfg(target_os = "macos")]
pub const FRONTPANEL: &[&str] = &["libokFrontPanel.1.dylib", "libokFrontPanel.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
pub const FRONTPANEL: &[&str] = &["libokFrontPanel.so"];

/// liboni, for the generation 3 board: the plugin has it as a library of its own up to version 1.1.
#[cfg(windows)]
pub const LIBONI: &[&str] = &["liboni.dll"];
#[cfg(target_os = "macos")]
pub const LIBONI: &[&str] = &["liboni.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
pub const LIBONI: &[&str] = &["liboni.so"];

/// liboni's driver for the generation 3 board's USB chip.
#[cfg(windows)]
pub const FT600_DRIVER: &[&str] = &["libonidriver_ft600.dll"];
#[cfg(target_os = "macos")]
pub const FT600_DRIVER: &[&str] = &["libonidriver_ft600.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
pub const FT600_DRIVER: &[&str] = &["libonidriver_ft600.so"];

/// Open Ephys's Rhythm plugins' files: FrontPanel for each platform, and every bitfile.
const RHYTHM_PLUGINS: &str =
    "https://github.com/open-ephys-plugins/rhythm-plugins/tree/a83689808331a447a5855d2c7658459ec7019c80/Resources";

/// The acquisition board plugin's last release with liboni as a library of its own, for this platform: everything
/// but Intan's bitfile is in its `shared` folder.
#[cfg(windows)]
const PLUGIN_1_1: &str =
    "https://openephys.jfrog.io/artifactory/AcquisitionBoard-plugin/windows/AcquisitionBoard-windows_1.1.6-API10.zip";
#[cfg(target_os = "macos")]
const PLUGIN_1_1: &str =
    "https://openephys.jfrog.io/artifactory/AcquisitionBoard-plugin/mac/AcquisitionBoard-mac_1.1.6-API10.zip";
#[cfg(all(unix, not(target_os = "macos")))]
const PLUGIN_1_1: &str =
    "https://openephys.jfrog.io/artifactory/AcquisitionBoard-plugin/linux/AcquisitionBoard-linux_1.1.6-API10.zip";

/// The boards' USB drivers, for Windows: FTDI's for generation 3, Opal Kelly's for the rest.
const USB_DRIVERS: &str =
    "https://github.com/open-ephys/plugin-GUI/tree/560c995500f7f1438c6c3646311ff11776da49bb/Resources/DLLs";

/// The files the boards use: what each is, its names, which boards use it, and where to get it on its own.
const FILES: &[(&str, &[&str], &str, &str)] = &[
    ("FrontPanel library", FRONTPANEL, "Gen 1/2, Intan USB", RHYTHM_PLUGINS),
    ("rhd2000.bit", &["rhd2000.bit"], "Gen 1", RHYTHM_PLUGINS),
    ("rhd2000_usb3.bit", &["rhd2000_usb3.bit"], "Gen 2", RHYTHM_PLUGINS),
    ("intan_rhd_usb.bit", &["intan_rhd_usb.bit"], "Intan USB", RHYTHM_PLUGINS),
    ("liboni", LIBONI, "Gen 3", PLUGIN_1_1),
    ("FT600 driver", FT600_DRIVER, "Gen 3", PLUGIN_1_1),
];

/// Which folder of the Open Ephys GUI's the boards' files come from.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenEphysSettings {
    /// Chosen, or `None` for the GUI's own (`usual_folder`).
    pub folder: Option<PathBuf>,
}

impl OpenEphysSettings {
    /// The folder the files come from, if there is one.
    pub fn folder(&self) -> Option<PathBuf> {
        self.folder.clone().or_else(usual_folder)
    }

    /// The file of the first of `names` that's in the folder.
    pub fn file(&self, what: &str, names: &[&str]) -> io::Result<PathBuf> {
        let Some(folder) = self.folder() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{what} not found: nothing's in Open Ephys's default location. Install the Open Ephys GUI and \
                     its Acquisition Board plugin, or choose the folder the files are in, in Settings… › {}",
                    Self::NAME
                ),
            ));
        };
        names
            .iter()
            .map(|name| folder.join(name))
            .find(|p| p.is_file())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{what} not found in {}", folder.display())))
    }
}

impl GlobalSettings for OpenEphysSettings {
    const NAME: &'static str = "Open Ephys";

    fn ui(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let small = |text: &str| RichText::new(text).small().color(colours.text_secondary);
        ui.label(small("The acquisition boards need the files below. Either:"));
        ui.add_space(2.0);
        // Open Ephys's, for its newest plugin API: "shared-api10", say.
        let shared = usual_folder()
            .and_then(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "shared-api10".into());
        ui.indent("options", |ui| {
            let option = |ui: &mut egui::Ui, letter: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
                ui.horizontal_top(|ui| {
                    // As the text is laid out, so they line up.
                    ui.horizontal_wrapped(|ui| {
                        ui.set_width(14.0);
                        ui.label(small(letter));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        body(ui);
                    });
                });
            };
            option(ui, "A.", &mut |ui| {
                ui.label(small("Download each file from its \"get\" link into a folder of your choosing"));
                if cfg!(windows) {
                    ui.label(small(", install "));
                    ui.hyperlink_to(RichText::new("the boards' USB drivers").small(), USB_DRIVERS);
                }
                ui.label(small(", and choose that folder here."));
            });
            option(ui, "B.", &mut |ui| {
                ui.label(small("Install "));
                ui.hyperlink_to(RichText::new("the Open Ephys GUI").small(), "https://open-ephys.org/gui");
                ui.label(small(&format!(
                    " and its Acquisition Board plugin, then choose the \"{shared}\" folder in its installation \
                     (ephys-rs tries to find it automatically)."
                )));
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Folder");
            match self.folder() {
                Some(folder) => ui.label(RichText::new(folder.display().to_string()).small()),
                None => ui.label(RichText::new("not found").small().color(colours.warning)),
            };
            if self.folder.is_none() {
                ui.label(small("(Open Ephys's default location)"));
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked()
                && let Some(folder) = rfd::FileDialog::new().pick_folder()
            {
                self.folder = Some(folder);
            }
            if self.folder.is_some() && ui.button("Use Open Ephys's default location").clicked() {
                self.folder = None;
            }
        });
        ui.add_space(8.0);
        egui::Grid::new("open ephys files").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
            for (what, names, boards, url) in FILES {
                ui.label(*what);
                ui.label(RichText::new(*boards).small().color(colours.text_secondary));
                ui.hyperlink_to(RichText::new("get").small(), *url).on_hover_text(*url);
                match self.file(what, names) {
                    // In the folder above: its name says which it is.
                    Ok(path) => {
                        let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                        ui.label(RichText::new(name).small()).on_hover_text(path.display().to_string())
                    }
                    Err(_) => ui.label(RichText::new("not found").small().color(colours.warning)),
                };
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        search_folders::note(ui, colours, search_folders::LOADED_ONCE);
    }
}

/// Where the Open Ephys GUI keeps what its plugins come with: a `shared` folder for each plugin API it's had, the
/// newest first.
fn gui_folders() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let saved_state: Vec<PathBuf> = if cfg!(windows) {
        ["ProgramData", "APPDATA", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|d| PathBuf::from(d).join("Open Ephys"))
            .collect()
    } else if cfg!(target_os = "macos") {
        home.map(|h| vec![h.join("Library/Application Support/open-ephys")]).unwrap_or_default()
    } else {
        home.map(|h| vec![h.join(".config/open-ephys")]).unwrap_or_default()
    };
    let mut folders = Vec::new();
    for dir in saved_state {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut shared: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("shared")))
            .collect();
        shared.sort();
        shared.reverse();
        folders.extend(shared);
    }
    folders
}

/// The Open Ephys GUI's own folder: the newest plugin API's.
pub fn usual_folder() -> Option<PathBuf> {
    gui_folders().into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_come_only_from_the_folder() {
        let dir = std::env::temp_dir().join(format!("open-ephys-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = OpenEphysSettings { folder: Some(dir.clone()) };
        let missing = settings.file("rhd2000.bit", &["rhd2000.bit"]).unwrap_err();
        assert!(missing.to_string().contains(&dir.display().to_string()), "{missing}");
        std::fs::write(dir.join("libokFrontPanel.dylib"), b"").unwrap();
        std::fs::write(dir.join("rhd2000.bit"), b"").unwrap();
        assert_eq!(settings.file("rhd2000.bit", &["rhd2000.bit"]).unwrap(), dir.join("rhd2000.bit"));
        // The first name there.
        let names = ["libokFrontPanel.1.dylib", "libokFrontPanel.dylib"];
        assert_eq!(settings.file("FrontPanel", &names).unwrap(), dir.join("libokFrontPanel.dylib"));
        std::fs::write(dir.join("libokFrontPanel.1.dylib"), b"").unwrap();
        assert_eq!(settings.file("FrontPanel", &names).unwrap(), dir.join("libokFrontPanel.1.dylib"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_chosen_is_used_rather_than_the_guis() {
        let chosen = OpenEphysSettings { folder: Some("/chosen".into()) };
        assert_eq!(chosen.folder(), Some(PathBuf::from("/chosen")));
        assert_eq!(OpenEphysSettings::default().folder(), usual_folder());
    }
}
