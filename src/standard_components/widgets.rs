//! Widgets and helpers the node types share.

use eframe::egui;

use crate::DataStream;
use crate::core::engine::EdgeSet;

/// Two-column grid of labelled settings.
pub fn settings_grid(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    named_settings_grid(ui, "settings", add);
}

/// A settings grid with its own id, for when there's more than one in the same place.
pub fn named_settings_grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 6.0]).show(ui, add);
}

/// A row of `settings_grid`: a label and the widget it names.
pub fn setting(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> egui::Response) {
    let label = ui.label(label);
    add(ui).labelled_by(label.id);
    ui.end_row();
}

pub fn combo<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    salt: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> egui::Response {
    let selected = options.iter().find(|(v, _)| v == value).map_or("?", |(_, name)| name);
    egui::ComboBox::from_id_salt(salt)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (option, name) in options {
                ui.selectable_value(value, *option, *name);
            }
        })
        .response
}

pub fn require_input(inputs: &[Option<DataStream>], what: &str) -> Vec<String> {
    if inputs.iter().all(|i| i.is_none()) { vec![format!("Connect {what}")] } else { Vec::new() }
}

/// Shows the incoming data streams, for settings panels of nodes that pass them on.
pub fn input_summary(ui: &mut egui::Ui, inputs: &[Option<DataStream>]) {
    for stream in inputs {
        match stream {
            Some(s) => ui.weak(format!("Input: {}", s.summary())),
            None => ui.weak("Input: not connected"),
        };
    }
}

/// "30 kHz", "2.5 kHz", "500 Hz".
pub fn format_rate(hz: f32) -> String {
    if hz >= 1000.0 { format!("{} kHz", hz / 1000.0) } else { format!("{hz} Hz") }
}
