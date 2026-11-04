//! Widgets and helpers the node types share.

use eframe::egui;

use crate::DataStream;
use crate::core::engine::EdgeSet;
pub use crate::core::gui::widgets::eye;

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
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
    let header = ui.horizontal(|ui| {
        // The triangle and the title both open and close it.
        let triangle = state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
        triangle.on_hover_cursor(egui::CursorIcon::PointingHand);
        let title = ui.add(egui::Label::new(egui::RichText::new(title).strong()).sense(egui::Sense::click()));
        let clicked = title.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            icon_toggle(ui, locked, padlock, lock_tip);
            if let Some((visible, eye_tip)) = visible {
                icon_toggle(ui, visible, eye, eye_tip);
            }
        });
        clicked
    });
    if header.inner {
        state.toggle(ui);
    }
    state.show_body_indented(&header.response, ui, |ui| body(ui, *locked));
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

/// A toggle with its label first and the triangle at the right (for right-aligned placement); label and
/// triangle are one click target.
pub fn toggle(ui: &mut egui::Ui, label: &str, open: bool) -> egui::Response {
    // Pointing left (towards the label) when closed, and down when open.
    toggle_with(ui, label, open, |c, h| {
        if open {
            vec![c + egui::vec2(-h, -h / 2.0), c + egui::vec2(h, -h / 2.0), c + egui::vec2(0.0, h * 0.75)]
        } else {
            vec![c + egui::vec2(h / 2.0, -h), c + egui::vec2(h / 2.0, h), c + egui::vec2(-h * 0.75, 0.0)]
        }
    })
}

/// A label, then a triangle made by `triangle` (from its centre and half-size).
fn toggle_with(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    triangle: impl Fn(egui::Pos2, f32) -> Vec<egui::Pos2>,
) -> egui::Response {
    let colour = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::TextStyle::Body.resolve(ui.style()), colour);
    let icon = 8.0;
    let size = egui::vec2(galley.size().x + ui.spacing().icon_spacing + icon, galley.size().y.max(icon));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, ui.is_enabled(), open, label));
    if ui.is_rect_visible(rect) {
        let colour = if response.hovered() { ui.visuals().strong_text_color() } else { colour };
        ui.painter().galley(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, colour);
        let (c, h) = (egui::pos2(rect.right() - icon / 2.0, rect.center().y), icon / 2.0);
        let points = triangle(c, h);
        ui.painter().add(egui::Shape::convex_polygon(points, colour, egui::Stroke::NONE));
    }
    response
}

/// A toggle for a chart at the side: open, its label then a triangle pointing right (to put it away); closed, a
/// narrow strip with a triangle pointing left (to bring it out) and the label running down.
pub fn side_toggle(ui: &mut egui::Ui, label: &str, open: bool) -> egui::Response {
    if open {
        let response = toggle_with(ui, label, open, |c, h| {
            vec![c + egui::vec2(-h / 2.0, -h), c + egui::vec2(-h / 2.0, h), c + egui::vec2(h * 0.75, 0.0)]
        });
        return response;
    }
    let colour = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::TextStyle::Body.resolve(ui.style()), colour);
    let icon = 8.0;
    let size = egui::vec2(galley.size().y.max(icon), icon + 8.0 + galley.size().x);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, ui.is_enabled(), open, label));
    if ui.is_rect_visible(rect) {
        let colour = if response.hovered() { ui.visuals().strong_text_color() } else { colour };
        let (c, h) = (egui::pos2(rect.center().x, rect.top() + icon / 2.0), icon / 2.0);
        let points = vec![c + egui::vec2(h / 2.0, -h), c + egui::vec2(h / 2.0, h), c + egui::vec2(-h * 0.75, 0.0)];
        ui.painter().add(egui::Shape::convex_polygon(points, colour, egui::Stroke::NONE));
        // Turned a quarter clockwise, to read downwards: it then lies to the left of where it's put.
        let at = egui::pos2(rect.center().x + galley.size().y / 2.0, rect.top() + icon + 8.0);
        let text = egui::epaint::TextShape::new(at, galley, colour).with_angle(std::f32::consts::FRAC_PI_2);
        ui.painter().add(text);
    }
    response
}
