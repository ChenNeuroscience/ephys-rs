//! An image on the canvas: pasted onto it (Ctrl/⌘ V), or from a file. Kept with the graph, as base64.

use eframe::egui::{self, Color32, ColorImage, Painter, Rect, RichText, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{canvas_size, encode, encode_pasted, image_size, image_uri};
use crate::core::annotation::{AnnotationInfo, AnnotationType};
use crate::core::theme::Colours;

/// The most an image pasted onto the canvas takes across, in canvas units, to start with.
const PASTED_SIZE: f32 = 360.0;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Image {
    /// The image's file (PNG, JPEG...), in base64; empty for none yet.
    pub data: String,
    /// Its size in pixels.
    pub pixels: [u32; 2],
}

impl Image {
    /// Asks for an image file, and shows it.
    fn choose_file(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg"]).pick_file() else {
            return;
        };
        match std::fs::read(&path) {
            Ok(bytes) => match image_size(&bytes) {
                Some(pixels) => *self = Image { data: encode(&bytes), pixels },
                None => log::warn!("{} isn't an image this can show", path.display()),
            },
            Err(e) => log::warn!("Couldn't read {}: {e}", path.display()),
        }
    }
}

impl AnnotationType for Image {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Image",
        description: "An image on the canvas: paste one (Ctrl/⌘ V), or choose a file. Not part of the graph.",
        default_size: [240.0, 180.0],
    };

    fn fill(&self, theme: &Colours) -> Color32 {
        theme.surface_raised
    }

    fn pasted_image(image: &ColorImage) -> Option<(Self, [f32; 2])> {
        let (data, pixels) = encode_pasted(image)?;
        Some((Image { data, pixels }, canvas_size(pixels, PASTED_SIZE)))
    }

    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool {
        // While it's selected for editing, a pasted image replaces it.
        if editing
            && let Some(pasted) = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::PasteImage(image) => Some(image.clone()),
                    _ => None,
                })
            })
            && let Some((data, pixels)) = encode_pasted(&pasted)
        {
            *self = Image { data, pixels };
        }
        if self.data.is_empty() {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Paste an image (Ctrl/⌘ V) while editing, or").weak());
                if ui.button("Choose a file…").clicked() {
                    self.choose_file();
                }
            });
            return editing;
        }
        let uri = image_uri(ui.ctx(), &self.data);
        let size = ui.available_size();
        ui.centered_and_justified(|ui| {
            ui.add(egui::Image::new(uri).max_size(size).maintain_aspect_ratio(true));
        });
        false
    }

    fn context_menu_ui(&mut self, ui: &mut egui::Ui) {
        if ui.button("Choose a file…").clicked() {
            self.choose_file();
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        if self.data.is_empty() {
            ui.weak("No image yet.");
        } else {
            let [w, h] = self.pixels;
            ui.label(format!("{w} × {h} pixels, {:.0} kB", self.data.len() as f64 * 0.75 / 1e3));
        }
        if ui.button("Choose a file…").clicked() {
            self.choose_file();
        }
        ui.weak("Or paste an image onto the canvas to add another.");
    }

    /// A framed picture: a sun over hills.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
        let frame = Rect::from_min_max(p(1.5, 2.5), p(14.5, 13.5));
        painter.rect(
            frame,
            s,
            Color32::from_rgb(170, 210, 245),
            Stroke::new(s, Color32::from_gray(90)),
            egui::StrokeKind::Middle,
        );
        painter.circle_filled(p(10.5, 5.5), 1.6 * s, Color32::from_rgb(245, 200, 70));
        painter.add(Shape::convex_polygon(
            vec![p(2.0, 13.0), p(6.5, 7.5), p(11.0, 13.0)],
            Color32::from_rgb(70, 150, 80),
            Stroke::NONE,
        ));
        painter.add(Shape::convex_polygon(
            vec![p(8.0, 13.0), p(11.0, 9.5), p(14.0, 13.0)],
            Color32::from_rgb(50, 120, 65),
            Stroke::NONE,
        ));
    }
}
