//! Annotation types: sticky notes (plain text), markdown notes (with links, and images pasted into them), and
//! images. Images are kept in the graph's file, as base64.

pub mod image;
pub mod markdown;
pub mod sticky_note;

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use eframe::egui::{self, ColorImage};

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

/// The size for an image `pixels` big on the canvas: as it is, unless that's more than `largest` across.
pub fn canvas_size(pixels: [u32; 2], largest: f32) -> [f32; 2] {
    let [w, h] = pixels.map(|n| n.max(1) as f32);
    let scale = (largest / w.max(h)).min(1.0);
    [w * scale, h * scale]
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
        assert_eq!(size, [4.0, 3.0], "small images keep their size");
        let png = STANDARD.decode(&image.data).unwrap();
        assert_eq!(image_size(&png), Some([4, 3]));
        assert_eq!(canvas_size([2000, 1000], 360.0), [360.0, 180.0]);
    }

    #[test]
    fn markdown_shows_attachments_from_where_egui_loads_them() {
        let ctx = egui::Context::default();
        let mut note = markdown::Markdown::drawn();
        let (base64, _) = encode_pasted(&red_square()).unwrap();
        note.text = note.attach(base64, "png");
        assert_eq!(note.text, "![image1.png](attachment:image1.png)");
        let shown = note.shown(&ctx);
        assert!(shown.starts_with("![image1.png](bytes://ephys-rs-image-"), "{shown}");
    }
}
