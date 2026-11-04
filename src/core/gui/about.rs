//! The About window: the program's version, what its build says about itself, and where its source is (see
//! `Components::ABOUT` and `SOURCE`).

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, TextureHandle, TextureOptions, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::core::theme::Colours;

const WIDTH: f32 = 460.0;
const ROUNDING: u8 = 8;

/// `png` as a texture, called `name`.
fn texture(ctx: &egui::Context, name: &str, png: &[u8]) -> Option<TextureHandle> {
    let image = image::load_from_memory(png).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, pixels, TextureOptions::LINEAR))
}

/// The window, while it's open.
pub struct About {
    markdown: CommonMarkCache,
    icon: Option<TextureHandle>,
    github: Option<TextureHandle>,
}

impl About {
    pub fn new(ctx: &egui::Context) -> Self {
        About {
            markdown: CommonMarkCache::default(),
            icon: texture(ctx, "about: icon", include_bytes!("../../../assets/icon.png")),
            github: texture(ctx, "about: github", include_bytes!("../../../assets/github.png")),
        }
    }

    /// Returns whether it's still open.
    pub fn show(&mut self, ctx: &egui::Context, text: &str, source: &str, colours: &Colours) -> bool {
        let frame = Frame::popup(&ctx.global_style()).inner_margin(0).corner_radius(ROUNDING);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("about")).frame(frame).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 0.0;
            self.title_bar(ui, colours);
            ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), (1.0, colours.divider));
            if !text.is_empty() {
                Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                    ui.set_width(WIDTH - 32.0);
                    // As tall as the text, so what's beside it in a line (bullets, and links) lines up with it.
                    ui.spacing_mut().interact_size.y = ui.text_style_height(&egui::TextStyle::Body);
                    CommonMarkViewer::new().show(ui, &mut self.markdown, text);
                });
            }
            ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), (1.0, colours.divider));
            close = self.footer(ui, source, colours);
        });
        !close && !modal.should_close()
    }

    /// The icon and version.
    fn title_bar(&self, ui: &mut egui::Ui, colours: &Colours) {
        let top = CornerRadius { nw: ROUNDING, ne: ROUNDING, sw: 0, se: 0 };
        Frame::new().fill(colours.surface_raised).corner_radius(top).inner_margin(Margin::symmetric(16, 10)).show(
            ui,
            |ui| {
                ui.set_width(WIDTH - 32.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if let Some(icon) = &self.icon {
                        ui.add(egui::Image::new(icon).fit_to_exact_size(vec2(22.0, 22.0)));
                    }
                    let version = RichText::new(crate::core::full_version()).size(15.0).color(colours.text_primary);
                    ui.label(version);
                });
            },
        );
    }

    /// The link to the source, and OK. Returns whether OK was clicked.
    fn footer(&self, ui: &mut egui::Ui, source: &str, colours: &Colours) -> bool {
        let bottom = CornerRadius { nw: 0, ne: 0, sw: ROUNDING, se: ROUNDING };
        let frame = Frame::new().fill(colours.surface_sunken).corner_radius(bottom);
        frame
            .inner_margin(Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.set_width(WIDTH - 32.0);
                ui.horizontal(|ui| {
                    // The mark only says what's true of the link: a fork's source might be anywhere.
                    let mark = self.github.as_ref().filter(|_| source.contains("github.com"));
                    let button = match mark {
                        Some(mark) => {
                            let image =
                                egui::Image::new(mark).fit_to_exact_size(vec2(16.0, 16.0)).tint(colours.text_primary);
                            egui::Button::image_and_text(image, "Source code")
                        }
                        None => egui::Button::new("Source code"),
                    };
                    if ui.add(button).on_hover_text(source).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(source));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.button("OK").clicked())
                        .inner
                })
                .inner
            })
            .inner
    }
}
