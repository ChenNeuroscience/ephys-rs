//! The app offers help the same way everywhere: text that's dimmed with a dotted underline (or an icon), saying more in
//! a popup. The popup shows while the pointer's over either, or, clicked, until something else is; only one shows at a
//! time.

use eframe::egui::{self, Stroke, pos2};

/// A window offers help with this: `text`, dimmed with a dotted underline, showing `help` when hovered over, or
/// clicked.
pub fn help_link(ui: &mut egui::Ui, text: &str, help: impl Fn(&mut egui::Ui)) -> egui::Response {
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        let open = ui.ctx().data(|d| d.get_temp::<bool>(pinned_id(&response))).unwrap_or(false);
        let visuals = ui.visuals();
        let colour = if response.hovered() || open { visuals.text_color() } else { visuals.weak_text_color() };
        let painter = ui.painter();
        painter.galley(rect.min, galley, colour);
        let (y, stroke) = (rect.bottom() - 0.5, Stroke::new(1.0, colour));
        let mut x = rect.left();
        while x < rect.right() {
            painter.line_segment([pos2(x, y), pos2((x + 1.5).min(rect.right()), y)], stroke);
            x += 3.5;
        }
    }
    with_help(response, help)
}

/// A window can offer help as an ⓘ icon, saying `text` when hovered over, or clicked: beside a setting, say, to say
/// what it's for.
pub fn info_icon(ui: &mut egui::Ui, text: &str) -> egui::Response {
    info_icon_with(ui, |ui| {
        ui.set_max_width(320.0);
        ui.label(text);
    })
}

/// An ⓘ icon showing `help` when hovered over, or clicked: more than a line of text (see `info_icon`).
pub fn info_icon_with(ui: &mut egui::Ui, help: impl Fn(&mut egui::Ui)) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, ui.is_enabled(), "What's this?"));
    let visuals = ui.visuals();
    let colour = if response.hovered() { visuals.text_color() } else { visuals.weak_text_color() };
    ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(1.0, colour));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "i", egui::FontId::proportional(9.0), colour);
    with_help(response, help)
}

fn popup_id(response: &egui::Response) -> egui::Id {
    response.id.with("help")
}

/// Whether a popup's been clicked open is kept under this id.
fn pinned_id(response: &egui::Response) -> egui::Id {
    popup_id(response).with("pinned")
}

/// Anything can show `help` in a popup with this, given its `response`: while the pointer's over it, or the popup, or,
/// clicked, until something else is. Where it's disabled, it can't be clicked.
pub fn with_help(response: egui::Response, help: impl Fn(&mut egui::Ui)) -> egui::Response {
    let ctx = response.ctx.clone();
    let id = popup_id(&response);
    let (pinned_id, rect_id) = (pinned_id(&response), id.with("rect"));
    let mut pinned = ctx.data(|d| d.get_temp::<bool>(pinned_id)).unwrap_or(false);
    if response.clicked() {
        pinned = !pinned;
    }
    let pointer = ctx.pointer_hover_pos();
    let over = |rect: egui::Rect| pointer.is_some_and(|p| rect.contains(p));
    let last = ctx.data(|d| d.get_temp::<egui::Rect>(rect_id));
    let open = pinned || response.contains_pointer() || last.is_some_and(over);
    // As a tooltip, the popup's above everything, a modal included: a popup (in front of the app, as a modal is) went
    // behind the modal it was opened from once a click inside the modal brought the modal to the front.
    let popup = egui::Popup::from_response(&response).id(id).kind(egui::PopupKind::Tooltip);
    let shown = popup.open(open).show(|ui| help(ui));
    match shown {
        Some(shown) => {
            let rect = shown.response.rect;
            let clicked_elsewhere = ctx.input(|i| i.pointer.any_click()) && !response.clicked() && !over(rect);
            pinned &= !clicked_elsewhere;
            ctx.data_mut(|d| d.insert_temp(rect_id, rect));
        }
        None => ctx.data_mut(|d| d.remove::<egui::Rect>(rect_id)),
    }
    ctx.data_mut(|d| d.insert_temp(pinned_id, pinned));
    if response.enabled() { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

/// The bits of regular expressions most likely wanted, for a help popup (see `help_link`) beside a pattern: Run
/// Command's files, say, or a quick action's label pattern.
pub fn regex_cheat_sheet(ui: &mut egui::Ui) {
    const BITS: [(&str, &str); 10] = [
        (r"\d", "digit"),
        ("[a-z]", "letter"),
        (r"\.", "literal dot"),
        (".", "any character"),
        ("?", "optional"),
        ("+", "one or more"),
        ("*", "zero or more"),
        ("a|b", "a or b"),
        ("^", "start"),
        ("$", "end"),
    ];
    // Which kind, so it can be looked up: the regex crate's, as `ScriptConfig::matching` uses.
    ui.set_max_width(260.0);
    ui.label("Rust regex syntax: like Perl's or Python's, without look-around or backreferences.");
    ui.add_space(4.0);
    egui::Grid::new("regular expressions").spacing([10.0, 2.0]).show(ui, |ui| {
        for (bit, meaning) in BITS {
            ui.label(egui::RichText::new(bit).monospace());
            ui.label(meaning);
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn help_shows_when_hovered_over_and_stays_when_clicked() {
        let mut harness = Harness::builder().with_size([300.0, 200.0]).build_ui(|ui| {
            help_link(ui, "What's this?", |ui| {
                ui.label("It's help");
            });
            ui.add_space(100.0);
            ui.label("Elsewhere");
        });
        harness.run_steps(2);
        assert!(harness.query_by_label("It's help").is_none());
        harness.get_by_label("What's this?").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label("It's help").is_some(), "hovered over");
        assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "it can be clicked");
        harness.get_by_label("It's help").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label("It's help").is_some(), "the pointer moved onto it");
        harness.get_by_label("Elsewhere").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label("It's help").is_none(), "not hovered over");
        harness.get_by_label("What's this?").click();
        harness.run_steps(2);
        harness.get_by_label("Elsewhere").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label("It's help").is_some(), "clicked, it stays");
        harness.get_by_label("Elsewhere").click();
        harness.run_steps(4);
        assert!(harness.query_by_label("It's help").is_none(), "till something else is");
    }

    #[test]
    fn disabled_help_still_shows_when_hovered_over() {
        let mut harness = Harness::builder().with_size([300.0, 200.0]).build_ui(|ui| {
            ui.add_enabled_ui(false, |ui| {
                help_link(ui, "What's this?", |ui| {
                    ui.label("It's help");
                });
            });
        });
        harness.run_steps(2);
        harness.get_by_label("What's this?").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label("It's help").is_some());
    }
}
