//! Widgets and helpers the node types share.

use eframe::egui;

use crate::DataStream;
use crate::core::engine::EdgeSet;
use crate::core::theme::Colours;

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

/// A collapsible section, open to begin with, whose header carries toggles at the right: a padlock (`locked`,
/// with its tooltip) and, for a section with something drawn over a view, an eye (`visible`). The body is given
/// whether it's locked, to disable what locking should.
pub fn section(
    ui: &mut egui::Ui,
    title: &str,
    visible: Option<(&mut bool, &str)>,
    (locked, lock_tip): (&mut bool, &str),
    body: impl FnOnce(&mut egui::Ui, bool),
) {
    let id = ui.make_persistent_id(title);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new(title).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icon_toggle(ui, locked, padlock, lock_tip);
                if let Some((visible, eye_tip)) = visible {
                    icon_toggle(ui, visible, eye, eye_tip);
                }
            });
        })
        .body(|ui| body(ui, *locked));
}

/// Paints an icon in a rect: given the ink and background colours, and whether it's on.
type PaintToggle = fn(&egui::Painter, egui::Rect, egui::Color32, egui::Color32, bool);

/// A small square icon button that flips `value` when clicked. `tooltip` is also its accessible name.
pub fn icon_toggle(ui: &mut egui::Ui, value: &mut bool, paint: PaintToggle, tooltip: &str) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let on = *value;
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), on, tooltip));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let background = if response.hovered() {
            ui.painter().rect_filled(rect, 3.0, visuals.weak_bg_fill);
            visuals.weak_bg_fill
        } else {
            ui.visuals().panel_fill
        };
        paint(ui.painter(), rect.shrink(3.0), visuals.fg_stroke.color, background, on);
    }
    response.on_hover_text(tooltip)
}

/// An open eye when `visible`, otherwise the same eye struck through.
pub fn eye(
    painter: &egui::Painter,
    rect: egui::Rect,
    colour: egui::Color32,
    _background: egui::Color32,
    visible: bool,
) {
    let c = rect.center();
    let (w, h) = (rect.width() / 2.0, rect.height() * 0.3);
    let stroke = egui::Stroke::new(1.5, colour);
    let lid = |sign: f32| -> Vec<egui::Pos2> {
        (0..=16)
            .map(|i| {
                let x = -1.0 + 2.0 * i as f32 / 16.0;
                c + egui::vec2(x * w, sign * h * (1.0 - x * x))
            })
            .collect()
    };
    painter.add(egui::Shape::line(lid(-1.0), stroke));
    painter.add(egui::Shape::line(lid(1.0), stroke));
    if visible {
        painter.circle_filled(c, h * 0.6, colour);
    } else {
        painter.circle_stroke(c, h * 0.6, stroke);
        let inset = egui::vec2(2.0, 1.0);
        painter.line_segment([rect.left_top() + inset, rect.right_bottom() - inset], stroke);
    }
}

/// A closed padlock when `locked`, otherwise with the shackle raised and swung open.
pub fn padlock(
    painter: &egui::Painter,
    rect: egui::Rect,
    colour: egui::Color32,
    background: egui::Color32,
    locked: bool,
) {
    let stroke = egui::Stroke::new(1.5, colour);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left() + rect.width() * 0.15, rect.top() + rect.height() * 0.45),
        egui::pos2(rect.right() - rect.width() * 0.15, rect.bottom()),
    );
    painter.rect_filled(body, 2.0, colour);
    let r = body.width() * 0.3;
    let lift = if locked { 0.0 } else { rect.height() * 0.18 };
    let centre = egui::pos2(body.center().x, body.top() - lift - r * 0.4);
    // Down the left leg's top, over the arc, then down the right leg, which stops short when unlocked.
    let mut points = vec![centre + egui::vec2(-r, r * 0.4 + lift)];
    points.extend((0..=12).map(|i| {
        let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
        centre + egui::vec2(r * a.cos(), r * a.sin())
    }));
    points.push(centre + egui::vec2(r, if locked { r * 0.4 } else { 0.0 }));
    painter.add(egui::Shape::line(points, stroke));
    painter.circle_filled(body.center() - egui::vec2(0.0, 0.5), body.height() * 0.14, background); // keyhole
}

/// How full a queue between threads is: "12 of 512 blocks", and a bar. For nodes' threads in the dev tools.
pub fn queue_ui(ui: &mut egui::Ui, theme: &Colours, what: &str, queued: usize, capacity: usize) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{queued} of {capacity} {what} queued")).size(11.0));
        let (bar, _) = ui.allocate_exact_size(egui::vec2(50.0, 6.0), egui::Sense::hover());
        ui.painter().rect_filled(bar, 2.0, theme.surface_sunken);
        let filled = (queued as f32 / capacity.max(1) as f32).min(1.0);
        let fill = if filled > 0.75 { theme.warning } else { theme.accent };
        ui.painter().rect_filled(bar.with_max_x(bar.left() + bar.width() * filled), 2.0, fill);
    });
}
