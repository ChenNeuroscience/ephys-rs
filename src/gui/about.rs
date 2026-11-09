//! The About window: the program's version, what its build says about itself, and where its source is (see
//! `Components::ABOUT` and `SOURCE`).

use eframe::egui::{self, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::dialog;
use crate::theme::Colours;

/// The window, while it's open.
#[derive(Default)]
pub struct About {
    markdown: CommonMarkCache,
}

impl About {
    /// Returns whether it's still open.
    pub fn show(&mut self, ctx: &egui::Context, text: &str, source: &str, colours: &Colours) -> bool {
        let mut close = false;
        let title = crate::full_version();
        let body = |ui: &mut egui::Ui| {
            // As tall as the text, so what's beside it in a line (bullets, and links) lines up with it.
            ui.spacing_mut().interact_size.y = ui.text_style_height(&egui::TextStyle::Body);
            CommonMarkViewer::new().show(ui, &mut self.markdown, text);
        };
        let footer = |ui: &mut egui::Ui| {
            // The mark only says what's true of the link: a fork's source might be anywhere.
            let png = include_bytes!("../../assets/github.png");
            let mark = dialog::texture(ui.ctx(), "github", png).filter(|_| source.contains("github.com"));
            let button = match &mark {
                Some(mark) => {
                    let image = egui::Image::new(mark).fit_to_exact_size(vec2(16.0, 16.0)).tint(colours.text_primary);
                    egui::Button::image_and_text(image, "Source code")
                }
                None => egui::Button::new("Source code"),
            };
            if ui.add(button).on_hover_text(source).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(source));
            }
            close = dialog::at_right(ui, |ui| ui.button("OK").clicked());
        };
        let dismissed = dialog::show(ctx, &title, 460.0, colours, body, footer);
        !close && !dismissed
    }
}
