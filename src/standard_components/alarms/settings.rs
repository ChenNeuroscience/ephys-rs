//! The alarms' settings, in the Alarm Processor's window: when each goes off, and what it watches.

use eframe::egui;

use super::{Alarm, AlarmStates, Alarms, PERIODS, Source};

/// Every alarm's settings. Those with nothing connected have only a note.
pub fn alarms_ui(ui: &mut egui::Ui, alarms: &mut Alarms, stream: AlarmStates) {
    ui.checkbox(&mut alarms.on, "Alarms on");
    for source in Source::ALL {
        ui.add_space(6.0);
        ui.separator();
        if stream.items(source) == 0 {
            ui.strong(source.name());
            ui.weak("Not connected");
            continue;
        }
        ui.checkbox(&mut alarms.alarm_mut(source).on, egui::RichText::new(source.name()).strong());
        settings(ui, source, alarms, stream);
    }
}

/// An alarm's settings: "Alarm when the average rate is outside 5 to 400 Hz, evaluated over 20 s".
fn settings(ui: &mut egui::Ui, source: Source, alarms: &mut Alarms, stream: AlarmStates) {
    let saturation = &mut alarms.saturation_uv;
    let alarm = match source {
        Source::Spikes => &mut alarms.spikes,
        Source::Lfp => &mut alarms.lfp,
        Source::Positions => &mut alarms.positions,
        Source::Ttl => &mut alarms.ttl,
    };
    ui.horizontal_wrapped(|ui| {
        let most = if matches!(source, Source::Lfp | Source::Positions) { 100.0 } else { f32::MAX };
        let suffix = source.unit();
        let bound = |value| egui::DragValue::new(value).range(0.0..=most).speed(0.2).suffix(suffix);
        let label = ui.label(format!("Alarm when {}", source.measured()));
        match source {
            // The range is of the samples: it's how much of the time they're outside it that has a limit.
            Source::Lfp => {
                let field = egui::DragValue::new(saturation).range(1.0..=10_000.0).speed(5.0).prefix("±").suffix(" µV");
                ui.add(field).labelled_by(label.id);
                let label = ui.label("is greater than");
                ui.add(bound(&mut alarm.high)).labelled_by(label.id);
            }
            Source::Positions => {
                ui.add(bound(&mut alarm.low)).labelled_by(label.id);
            }
            Source::Spikes | Source::Ttl => {
                ui.add(bound(&mut alarm.low)).labelled_by(label.id);
                let label = ui.label("to");
                ui.add(bound(&mut alarm.high)).labelled_by(label.id);
            }
        }
        let label = ui.label("evaluated over");
        let field = egui::DragValue::new(&mut alarm.seconds).range(PERIODS).speed(0.2).suffix(" s");
        ui.add(field).labelled_by(label.id);
    });
    if source != Source::Positions {
        selection(ui, source, alarm, stream);
    }
}

/// Which groups, channels or lines it watches, and whether they're taken together.
fn selection(ui: &mut egui::Ui, source: Source, alarm: &mut Alarm, stream: AlarmStates) {
    let items = stream.items(source);
    let name = stream.item_name(source);
    let together = if source == Source::Lfp { "Their mean" } else { "Their total" };
    ui.horizontal(|ui| {
        ui.label("Measure");
        ui.selectable_value(&mut alarm.together, false, format!("Any one {name}"));
        ui.selectable_value(&mut alarm.together, true, together);
    });
    ui.horizontal_wrapped(|ui| {
        if ui.small_button("All").clicked() {
            alarm.left_out.clear();
        }
        if ui.small_button("None").clicked() {
            alarm.left_out = (0..items).collect();
        }
        for item in 0..items {
            let mut selected = alarm.selected(item);
            let checkbox = ui.checkbox(&mut selected, (item + 1).to_string());
            if checkbox.on_hover_text(format!("{name} {}", item + 1)).changed() {
                alarm.left_out.retain(|left_out| *left_out != item);
                if !selected {
                    alarm.left_out.push(item);
                }
            }
        }
    });
}
