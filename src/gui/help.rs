//! Help, the same everywhere: text that's dimmed with a dotted underline (or an icon), saying more in a popup. It's
//! shown while the pointer's over either, or, clicked, until something else is: the one popup, whichever.

use eframe::egui::{self, Stroke, pos2};

/// Help: `text`, dimmed with a dotted underline, showing `help` when hovered over, or clicked.
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

/// Help as an ⓘ icon, saying `text` when hovered over, or clicked: beside a setting, say, to say what it's for.
pub fn info_icon(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, ui.is_enabled(), "What's this?"));
    let visuals = ui.visuals();
    let colour = if response.hovered() { visuals.text_color() } else { visuals.weak_text_color() };
    ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(1.0, colour));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "i", egui::FontId::proportional(9.0), colour);
    with_help(response, |ui| {
        ui.set_max_width(320.0);
        ui.label(text);
    })
}

fn popup_id(response: &egui::Response) -> egui::Id {
    response.id.with("help")
}

/// Where whether it's been clicked open is kept.
fn pinned_id(response: &egui::Response) -> egui::Id {
    popup_id(response).with("pinned")
}

/// `response`, of something showing `help` in a popup: while the pointer's over it, or the popup, or, clicked, until
/// something else is. Where it's disabled, it can't be clicked.
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
    let shown = egui::Popup::from_response(&response).id(id).open(open).show(|ui| help(ui));
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
