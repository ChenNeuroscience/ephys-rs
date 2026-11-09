//! Widgets and helpers the node types share.

use eframe::egui;

use crate::engine::{EdgeTypesEnum, UiContext};
use crate::gui::widgets::{Icon, small_icon_button, small_toggle_button};
use crate::theme::Colours;
use crate::{DataStream, Edges};

pub use crate::gui::widgets::eye;

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

/// A round tick spacing (1, 2 or 5 times a power of ten) of at least `raw`.
pub fn nice_step(raw: f64) -> f64 {
    let power = 10f64.powf(raw.max(1e-9).log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * power).find(|&s| s >= raw).unwrap()
}

/// How high a time axis is.
pub const TIME_AXIS_HEIGHT: f32 = 20.0;
/// The least room between a time axis's ticks, for their labels.
const TIME_TICK_GAP: f32 = 70.0;

/// The ticks of a time axis `width` wide from `left` to `right` seconds: where each is, and its label.
pub fn time_ticks(left: f64, right: f64, width: f32) -> Vec<(f64, String)> {
    if right <= left || width <= 0.0 {
        return Vec::new();
    }
    let step = nice_step((right - left) * (TIME_TICK_GAP / width) as f64);
    let first = (left / step).ceil() as i64;
    let last = (right / step).floor() as i64;
    (first..=last).map(|i| i as f64 * step).map(|at| (at, crate::time_axis::label(at))).collect()
}

/// A time axis along the top of `rect`, which is the width of the plot above it, from `left` to `right` seconds, as
/// `crate::time_axis` says: how long ago, so what's up to now.
pub fn time_axis_ui(painter: &egui::Painter, rect: egui::Rect, (left, right): (f64, f64), theme: &Colours) {
    let stroke = egui::Stroke::new(1.0, theme.divider);
    painter.hline(rect.x_range(), rect.top(), stroke);
    for (at, text) in time_ticks(left, right, rect.width()) {
        let x = rect.left() + ((at - left) / (right - left)) as f32 * rect.width();
        painter.vline(x, rect.top()..=rect.top() + 3.0, stroke);
        let position = egui::pos2(x, rect.top() + 5.0);
        let font = egui::FontId::monospace(9.0);
        painter.text(position, egui::Align2::CENTER_TOP, text, font, theme.text_secondary);
    }
}

/// A checkbox for all of a list of `count` checkboxes, `selected` of which are ticked: a dash if it's only some. Returns
/// whether to tick them all, or none, if it's clicked: some ticked, that's all.
pub fn all_checkbox(ui: &mut egui::Ui, selected: usize, count: usize, text: &str) -> Option<bool> {
    let mut all = selected == count && count > 0;
    let some = selected > 0 && !all;
    ui.add(egui::Checkbox::new(&mut all, text).indeterminate(some))
        .on_hover_text("All or none")
        .changed()
        .then_some(all)
}

/// A small icon button for `on`.
pub fn small_toggle(ui: &mut egui::Ui, icon: Icon, on: &mut bool, label: &str, tip: &str) -> egui::Response {
    let mut button = small_toggle_button(ui, icon, label, tip, *on);
    if button.clicked() {
        *on = !*on;
        button.mark_changed();
    }
    button
}

/// The button that switches something on and off, keeping its settings, with `label` as its accessible name.
pub fn power(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let tip = if *on { "On: click to switch off, keeping the settings" } else { "Off: click to switch on" };
    small_toggle(ui, Icon::Power, on, label, tip)
}

/// Shows a toggle for all of a list as on for only some of it: a dash, as on a checkbox, for icons and other buttons.
pub fn mark_mixed(ui: &egui::Ui, button: &egui::Response) {
    let (rect, stroke) = (button.rect, egui::Stroke::new(2.0, ui.visuals().strong_text_color()));
    let y = rect.bottom() - 1.5;
    ui.painter().line_segment([egui::pos2(rect.center().x - 6.0, y), egui::pos2(rect.center().x + 6.0, y)], stroke);
}

/// A settings button, in the part of a view it's about, that opens the window of the node control `control` connects to:
/// `what` is that node. It's greyed out while the control's not connected.
pub fn open_settings_button(ui: &mut egui::Ui, context: &mut UiContext<Edges>, control: usize, what: &str) {
    let tip = format!("Open the {what}'s settings");
    let connected = context.control_connected(control);
    let button = ui.add_enabled_ui(connected, |ui| small_icon_button(ui, Icon::Settings, what, &tip)).inner;
    if button.clicked() {
        context.open_control_target(control);
    }
}

/// Widgets kept together in a wrapping row, such as a toolbar's: if they won't fit on what's left of the row, they all go
/// on the next. `separated` puts a line before them, unless they're starting a row.
pub fn inline_block(ui: &mut egui::Ui, salt: &str, separated: bool, add: impl FnOnce(&mut egui::Ui)) {
    let width_id = ui.scope_id().with(("inline block", salt));
    let width: f32 = ui.data(|d| d.get_temp(width_id)).unwrap_or(0.0);
    // Room for the line too.
    let line = if separated { 2.0 * ui.spacing().item_spacing.x + 6.0 } else { 0.0 };
    if ui.available_size_before_wrap().x < width + line {
        ui.end_row();
    } else if separated {
        ui.separator();
    }
    let width = ui.horizontal(add).response.rect.width();
    ui.data_mut(|d| d.insert_temp(width_id, width));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_ticks_are_round_and_have_room_for_their_labels() {
        let at = |left, right, width| time_ticks(left, right, width).into_iter().map(|t| t.1).collect::<Vec<_>>();
        assert_eq!(at(-2.0, 0.06, 300.0), ["−2 s", "−1.5 s", "−1 s", "−0.5 s", "now"]);
        assert_eq!(at(-60.0, 1.8, 400.0), ["−60 s", "−40 s", "−20 s", "now"]);
        assert_eq!(at(1.032, 1.052, 200.0), ["1.04 s", "1.05 s"]);
        assert!(time_ticks(0.0, 0.0, 300.0).is_empty(), "no span yet");
        assert!(time_ticks(0.0, 2.0, 0.0).is_empty());
    }
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn a_inline_block_wraps_whole() {
        let mut harness = egui_kittest::Harness::builder().with_size([220.0, 80.0]).wgpu().build_ui(|ui| {
            ui.horizontal_wrapped(|ui| {
                inline_block(ui, "a", false, |ui| drop(ui.label("A long first section")));
                inline_block(ui, "b", true, |ui| {
                    ui.label("Second");
                    ui.label("section");
                });
            });
        });
        harness.run_steps(3);
        let top = |label: &str| harness.get_by_label(label).accesskit_node().bounding_box().unwrap().y0;
        assert_eq!(top("Second"), top("section"), "kept together");
        assert!(top("Second") > top("A long first section"), "on the next row");
    }
}
