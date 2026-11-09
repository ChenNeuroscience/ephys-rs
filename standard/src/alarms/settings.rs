//! The alarms' settings, in the Alarm Processor's window: a row for all of them, then each with what's healthy, over how
//! long and what it watches, and what can be added.

use eframe::egui;

use super::{Alarm, AlarmKind, AlarmStates, Alarms, PERIODS, Selection};
use crate::standard::widgets::{all_checkbox, mark_mixed, power, settings_grid, small_toggle};
use ephys_rs::gui::widgets::{Icon, small_toggle_button};

/// Between the buttons at the right of an alarm.
pub const BUTTON_GAP: f32 = 2.0;

/// The power button for all the alarms, with `some` of them on, of `count`. Returns whether to switch them all on, or off,
/// if it's clicked.
pub fn all_power(ui: &mut egui::Ui, some: usize, count: usize) -> Option<bool> {
    let mut all = some == count && count > 0;
    let mixed = some > 0 && !all;
    let tip = if mixed {
        "Some on: click to switch them all on"
    } else if all {
        "All on: click to switch them off"
    } else {
        "All off: click to switch them on"
    };
    let button = small_toggle(ui, Icon::Power, &mut all, "All alarms", tip);
    if mixed {
        mark_mixed(ui, &button);
    }
    button.changed().then_some(all)
}

/// An alarm's title, faded while it's off.
pub fn title(ui: &mut egui::Ui, on: bool, name: &str) {
    let text = egui::RichText::new(name).size(15.0).strong();
    ui.label(if on { text } else { text.weak() });
}

/// The alarms, to change or remove, and a menu to add one.
pub fn alarms_ui(ui: &mut egui::Ui, alarms: &mut Alarms, stream: AlarmStates) {
    ui.weak("An alarm goes off when its healthy condition isn't met over a complete period.");
    ui.add_space(6.0);
    if alarms.list.is_empty() {
        ui.weak("No alarms yet");
        ui.add_space(6.0);
    } else {
        all_row(ui, alarms);
        ui.add_space(2.0);
    }
    let mut removed = None;
    for alarm in &mut alarms.list {
        ui.push_id(alarm.id, |ui| {
            if card(ui, alarm, stream) {
                removed = Some(alarm.id);
            }
        });
        ui.add_space(6.0);
    }
    if let Some(id) = removed {
        alarms.remove(id);
    }
    // Only what there's something connected for.
    let library = AlarmKind::library();
    let offered: Vec<&AlarmKind> = library.iter().filter(|kind| stream.items(kind.source()) > 0).collect();
    ui.add_enabled_ui(!offered.is_empty(), |ui| {
        let menu = ui.menu_button("Add alarm", |ui| {
            for kind in offered {
                if ui.button(kind.name()).on_hover_text(kind.description()).clicked() {
                    alarms.add(kind.clone());
                    ui.close();
                }
            }
        });
        menu.response.on_disabled_hover_text("Connect something to this node's inputs to watch");
    });
}

/// Buttons for all the alarms, lined up with each alarm's.
fn all_row(ui: &mut egui::Ui, alarms: &mut Alarms) {
    // A stroke there's nothing to see of, so it's as wide as the cards and the buttons line up.
    let stroke =
        egui::Stroke::new(ui.style().visuals.widgets.noninteractive.bg_stroke.width, egui::Color32::TRANSPARENT);
    egui::Frame::group(ui.style()).stroke(stroke).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                if small_toggle_button(ui, Icon::Delete, "Delete all", "Delete all the alarms", false).clicked() {
                    alarms.list.clear();
                }
                let on = alarms.list.iter().filter(|alarm| alarm.on).count();
                if let Some(all) = all_power(ui, on, alarms.list.len()) {
                    alarms.list.iter_mut().for_each(|alarm| alarm.on = all);
                }
            });
        });
    });
}

/// An alarm: its title and buttons, and its settings. Returns whether its delete button was clicked.
fn card(ui: &mut egui::Ui, alarm: &mut Alarm, stream: AlarmStates) -> bool {
    let mut remove = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            title(ui, alarm.on, alarm.kind.name());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                remove = small_toggle_button(ui, Icon::Delete, "Delete", "Delete this alarm", false).clicked();
                power(ui, &mut alarm.on, alarm.kind.name());
            });
        });
        settings(ui, alarm, stream);
    });
    remove
}

/// A field for one end of a range.
fn bound<'a>(value: &'a mut f32, most: f32, suffix: &str) -> egui::DragValue<'a> {
    egui::DragValue::new(value).range(0.0..=most).speed(0.2).suffix(suffix.to_owned())
}

/// A row of the settings: a label, and what's added after it, which is given the label to name its fields with.
fn line(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui, egui::Id)) {
    let label = ui.label(label);
    ui.horizontal(|ui| add(ui, label.id));
    ui.end_row();
}

/// A kind's settings: what's healthy, and over how long; then what it watches.
fn settings(ui: &mut egui::Ui, alarm: &mut Alarm, stream: AlarmStates) {
    let Alarm { seconds, kind, .. } = alarm;
    let source = kind.source();
    let items = stream.items(source);
    let has_period = kind.has_period();
    let together = if matches!(kind, AlarmKind::LfpSaturation { .. }) { "Their mean" } else { "Their total" };
    let name = stream.item_name(source);
    settings_grid(ui, |ui| {
        match kind {
            AlarmKind::SpikeRate { low, high, .. } => line(ui, "Healthy", |ui, id| {
                ui.add(bound(low, f32::MAX, " Hz")).labelled_by(id);
                ui.label("to");
                ui.add(bound(high, f32::MAX, " Hz")).labelled_by(id);
            }),
            AlarmKind::LfpSaturation { level_uv, low, .. } => line(ui, "Healthy", |ui, id| {
                ui.label("at least");
                ui.add(bound(low, 100.0, " %")).labelled_by(id);
                ui.label("of samples within");
                let field = egui::DragValue::new(level_uv).range(1.0..=10_000.0).speed(5.0).prefix("±").suffix(" µV");
                ui.add(field).labelled_by(id);
            }),
            AlarmKind::MissingPositions { low } => line(ui, "Healthy", |ui, id| {
                ui.label("at least");
                ui.add(bound(low, 100.0, " %")).labelled_by(id);
                ui.label("of samples contain data");
            }),
            AlarmKind::TtlCount { low, high, .. } => line(ui, "Healthy", |ui, id| {
                ui.add(bound(low, f32::MAX, "")).labelled_by(id);
                ui.label("to");
                ui.add(bound(high, f32::MAX, "")).labelled_by(id);
                ui.label("TTLs");
            }),
            AlarmKind::TtlUp { line: chosen } => line(ui, "Healthy", |ui, _| {
                ui.label("line");
                for item in 0..items {
                    ui.selectable_value(chosen, item, (item + 1).to_string());
                }
                ui.label("is down");
            }),
        }
        if has_period {
            line(ui, "Over", |ui, id| {
                let field = egui::DragValue::new(seconds).range(PERIODS).speed(0.2).suffix(" s");
                ui.add(field).labelled_by(id);
            });
        }
        if let (true, Some(selection)) = (items > 0, kind.selection_mut()) {
            line(ui, "Measure", |ui, _| {
                ui.selectable_value(&mut selection.together, false, format!("Any one {name}"));
                ui.selectable_value(&mut selection.together, true, together);
            });
            selection_ui(ui, selection, items, name);
        }
    });
    if items == 0 {
        ui.weak("Not connected");
        return;
    }
    if !has_period {
        let note = "⚠ Taken as down until it first changes. Pulses under 20 ms can be missed.";
        ui.colored_label(ui.visuals().warn_fg_color, egui::RichText::new(note).small());
    }
}

/// A row of the settings: which groups, channels or lines it watches, with a checkbox for all of them in the labels' column.
pub fn selection_ui(ui: &mut egui::Ui, selection: &mut Selection, items: u32, name: &str) {
    // Grid cells are centred, so a tall one wants pinning to the top.
    let top = egui::Layout::top_down(egui::Align::Min);
    let selected = (0..items).filter(|item| selection.selected(*item)).count();
    let right = egui::Layout::right_to_left(egui::Align::Min);
    let all = ui.with_layout(right, |ui| all_checkbox(ui, selected, items as usize, "")).inner;
    match all {
        Some(true) => selection.left_out.clear(),
        Some(false) => selection.left_out = (0..items).collect(),
        None => {}
    }
    ui.with_layout(top, |ui| {
        ui.horizontal_wrapped(|ui| {
            for item in 0..items {
                let mut selected = selection.selected(item);
                let checkbox = ui.checkbox(&mut selected, (item + 1).to_string());
                if checkbox.on_hover_text(format!("{name} {}", item + 1)).changed() {
                    selection.left_out.retain(|left_out| *left_out != item);
                    if !selected {
                        selection.left_out.push(item);
                    }
                }
            }
        });
    });
    ui.end_row();
}
