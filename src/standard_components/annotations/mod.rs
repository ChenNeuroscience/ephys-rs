//! Annotation types: sticky notes (plain text), markdown notes (with links, and images pasted into them), images,
//! headings, boxes (to group things), and arrows. Images are kept in the graph's file, as base64.

pub mod arrow;
pub mod group_box;
pub mod heading;
pub mod image;
pub mod markdown;
pub mod sticky_note;

use serde::{Deserialize, Serialize};

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use eframe::egui::{self, ColorImage};

/// Colours for boxes and arrows: mid tones, which show on the canvas whether it's light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hue {
    Grey,
    #[default]
    Blue,
    Green,
    Orange,
    Red,
    Purple,
}

impl Hue {
    pub const ALL: [Hue; 6] = [Hue::Grey, Hue::Blue, Hue::Green, Hue::Orange, Hue::Red, Hue::Purple];

    pub fn name(self) -> &'static str {
        match self {
            Hue::Grey => "Grey",
            Hue::Blue => "Blue",
            Hue::Green => "Green",
            Hue::Orange => "Orange",
            Hue::Red => "Red",
            Hue::Purple => "Purple",
        }
    }

    pub fn colour(self) -> egui::Color32 {
        let rgb = egui::Color32::from_rgb;
        match self {
            Hue::Grey => rgb(150, 150, 150),
            Hue::Blue => rgb(70, 140, 230),
            Hue::Green => rgb(60, 175, 95),
            Hue::Orange => rgb(240, 145, 40),
            Hue::Red => rgb(225, 75, 75),
            Hue::Purple => rgb(160, 110, 230),
        }
    }

    /// A row of swatches to pick from.
    pub fn picker(ui: &mut egui::Ui, hue: &mut Hue) {
        ui.horizontal(|ui| {
            for option in Hue::ALL {
                let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::RadioButton, true, *hue == option, option.name())
                });
                ui.painter().circle_filled(rect.center(), 7.0, option.colour());
                if *hue == option {
                    ui.painter().circle_stroke(
                        rect.center(),
                        8.5,
                        egui::Stroke::new(1.5, ui.visuals().strong_text_color()),
                    );
                }
                if response.on_hover_text(option.name()).clicked() {
                    *hue = option;
                }
            }
        });
    }

    /// A menu of them.
    pub fn menu(ui: &mut egui::Ui, hue: &mut Hue) {
        ui.menu_button("Colour", |ui| {
            for option in Hue::ALL {
                if ui.selectable_label(*hue == option, option.name()).clicked() {
                    *hue = option;
                }
            }
        });
    }
}

/// An image's file (PNG, JPEG...) as base64, to keep in the graph's file.
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// A pasted image as a PNG file, in base64, and its size in pixels.
pub fn encode_pasted(image: &ColorImage) -> Option<(String, [u32; 2])> {
    let [width, height] = image.size.map(|n| n as u32);
    let pixels: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    let rgba = ::image::RgbaImage::from_raw(width, height, pixels)?;
    let mut png = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut png), ::image::ImageFormat::Png).ok()?;
    Some((encode(&png), [width, height]))
}

/// An image file's size in pixels, if it's an image this can read.
pub fn image_size(bytes: &[u8]) -> Option<[u32; 2]> {
    let reader = ::image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.into_dimensions().ok().map(|(w, h)| [w, h])
}

/// A URI egui's image loaders can show a base64 image from (with `egui::Image::new`, or in markdown). The image
/// is decoded and handed to egui the first time it's asked for.
pub fn image_uri(ctx: &egui::Context, base64: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    base64.hash(&mut hasher);
    let hash = hasher.finish();
    let uri = format!("bytes://ephys-rs-image-{hash:016x}");
    let id = egui::Id::unique("annotation images given to egui");
    let known = ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<std::sync::Mutex<HashSet<u64>>>>(id).clone());
    if known.lock().unwrap().insert(hash) {
        let bytes = STANDARD.decode(base64).unwrap_or_default();
        ctx.include_bytes(uri.clone(), bytes);
    }
    uri
}

/// A small, flat button showing an icon (painted by `paint` into a 16-point square), named `label` for screen
/// readers, with `tooltip` on hover.
pub fn icon_button(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: &str,
    paint: impl Fn(&egui::Painter, egui::Rect),
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    if response.hovered() {
        ui.painter().rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    paint(ui.painter(), egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0)));
    response.on_hover_text(tooltip)
}

/// A red cross, for removing something.
pub fn paint_cross(painter: &egui::Painter, rect: egui::Rect) {
    let s = rect.width() / 16.0;
    let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
    let stroke = egui::Stroke::new(1.8 * s, egui::Color32::from_rgb(214, 72, 64));
    painter.line_segment([p(4.0, 4.0), p(12.0, 12.0)], stroke);
    painter.line_segment([p(12.0, 4.0), p(4.0, 12.0)], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::annotation::AnnotationType;

    fn red_square() -> ColorImage {
        ColorImage::new([4, 3], vec![egui::Color32::RED; 12])
    }

    #[test]
    fn a_pasted_image_becomes_an_annotation_of_its_shape() {
        let (image, size) = image::Image::pasted_image(&red_square()).expect("images take pasted images");
        assert_eq!(image.pixels, [4, 3]);
        assert_eq!(size, [4.0, 3.0], "its natural size, for the canvas to fit to the view");
        assert_eq!(image.aspect(), 4.0 / 3.0, "and it keeps its shape");
        let png = STANDARD.decode(&image.data).unwrap();
        assert_eq!(image_size(&png), Some([4, 3]));
    }

    #[test]
    fn markdown_shows_attachments_from_where_egui_loads_them() {
        let ctx = egui::Context::default();
        let mut note = markdown::Markdown::drawn([false; 2]);
        let (base64, _) = encode_pasted(&red_square()).unwrap();
        note.text = note.attach(base64, "png");
        assert_eq!(note.text, "![image1.png](attachment:image1.png)");
        let shown = note.shown(&ctx);
        assert!(shown.starts_with("![image1.png](bytes://ephys-rs-image-"), "{shown}");
    }
}
