//! Dialogs: a modal window with a title bar (the program's icon and a title), a body, and a footer for its buttons.

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, TextureHandle, TextureOptions, vec2};

use crate::theme::Colours;

const ROUNDING: u8 = 8;
/// Between a dialog's edge and what's in it.
const PAD: i8 = 16;

/// `png` as a texture, called `name`, made the first time it's asked for.
pub fn texture(ctx: &egui::Context, name: &str, png: &[u8]) -> Option<TextureHandle> {
    let id = egui::Id::unique(("dialog texture", name));
    if let Some(texture) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(texture);
    }
    let image = image::load_from_memory(png).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    let texture = ctx.load_texture(name, pixels, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

/// Shows a dialog `width` wide. Returns whether it should close, by clicking outside it or pressing Esc.
pub fn show(
    ctx: &egui::Context,
    title: &str,
    width: f32,
    colours: &Colours,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui),
) -> bool {
    let frame = Frame::popup(&ctx.global_style()).inner_margin(0).corner_radius(ROUNDING);
    let inner = width - 2.0 * f32::from(PAD);
    let modal = egui::Modal::new(egui::Id::unique(title)).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.y = 0.0;
        title_bar(ui, title, inner, colours);
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), (1.0, colours.divider));
        Frame::new().inner_margin(Margin::symmetric(PAD, 14)).show(ui, |ui| {
            ui.set_width(inner);
            ui.spacing_mut().item_spacing = spacing;
            body(ui);
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), (1.0, colours.divider));
        let bottom = CornerRadius { nw: 0, ne: 0, sw: ROUNDING, se: ROUNDING };
        let frame = Frame::new().fill(colours.surface_sunken).corner_radius(bottom);
        frame.inner_margin(Margin::symmetric(PAD, 8)).show(ui, |ui| {
            ui.set_width(inner);
            ui.spacing_mut().item_spacing = spacing;
            ui.horizontal(footer);
        });
    });
    modal.should_close()
}

/// The program's icon and `title`.
fn title_bar(ui: &mut egui::Ui, title: &str, width: f32, colours: &Colours) {
    let top = CornerRadius { nw: ROUNDING, ne: ROUNDING, sw: 0, se: 0 };
    let frame = Frame::new().fill(colours.surface_raised).corner_radius(top);
    frame.inner_margin(Margin::symmetric(PAD, 10)).show(ui, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(icon) = texture(ui.ctx(), "icon", include_bytes!("../../assets/icon.png")) {
                ui.add(egui::Image::new(&icon).fit_to_exact_size(vec2(22.0, 22.0)));
            }
            ui.label(RichText::new(title).size(15.0).color(colours.text_primary));
        });
    });
}

/// `add`, at the right of a footer.
pub fn at_right<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner
}

/// `parts` on a line of their own, centred, `size` points high: a headline. Those marked bold are drawn again a hair
/// to the right, as there's no bold font.
pub fn headline(ui: &mut egui::Ui, parts: &[(&str, bool)], size: f32, colour: Color32) {
    let job = |bold_only: bool| {
        let mut job = egui::text::LayoutJob::default();
        for &(text, bold) in parts {
            let color = if bold_only && !bold { Color32::TRANSPARENT } else { colour };
            job.append(
                text,
                0.0,
                egui::TextFormat { font_id: egui::FontId::proportional(size), color, ..Default::default() },
            );
        }
        job
    };
    let galley = ui.painter().layout_job(job(false));
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y), egui::Sense::hover());
    let text: String = parts.iter().map(|(text, _)| *text).collect();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    let at = egui::pos2(rect.center().x - galley.size().x / 2.0, rect.top());
    ui.painter().galley(at, galley, colour);
    ui.painter().galley(at + vec2(0.7, 0.0), ui.painter().layout_job(job(true)), colour);
}
