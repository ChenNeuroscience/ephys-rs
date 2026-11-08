//! An image on the canvas: chosen from a file, or pasted onto the canvas (Ctrl/⌘ V). It's just the image, at its
//! own shape, which it keeps when resized. Kept with the graph, as base64.

use eframe::egui::{self, Color32, Painter, Rect, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{encode, encode_pasted, image_size, image_uri};
use crate::core::annotation::{AnnotationInfo, AnnotationType, AnnotationUi, Fit, PastedContent};
use crate::core::theme::Colours;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Image {
    /// The image's file (PNG, JPEG...), in base64.
    pub data: String,
    /// Its size in pixels.
    pub pixels: [u32; 2],
}

impl Image {
    /// Asks for an image file, and reads it.
    fn from_file() -> Option<Image> {
        let path = rfd::FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg"]).pick_file()?;
        match std::fs::read(&path) {
            Ok(bytes) => match image_size(&bytes) {
                Some(pixels) => Some(Image { data: encode(&bytes), pixels }),
                None => {
                    log::warn!("{} isn't an image this can show", path.display());
                    None
                }
            },
            Err(e) => {
                log::warn!("Couldn't read {}: {e}", path.display());
                None
            }
        }
    }

    fn natural_size(&self) -> [f32; 2] {
        self.pixels.map(|n| n.max(1) as f32)
    }

    /// Its width over its height, which it keeps.
    pub fn aspect(&self) -> f32 {
        let [w, h] = self.natural_size();
        w / h
    }
}

impl AnnotationType for Image {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Image",
        description: "An image on the canvas, from a file (or paste one onto the canvas). Not part of the graph.",
        default_size: [240.0, 180.0],
    };
    const MIN_SIZE: [f32; 2] = [16.0, 16.0];
    const DRAWN: bool = false;

    fn choose() -> Option<(Self, [f32; 2])> {
        let image = Image::from_file()?;
        let size = image.natural_size();
        Some((image, size))
    }

    /// An image pasted: what it's for.
    fn pasted(pasted: &PastedContent) -> Option<(Fit, Self, [f32; 2])> {
        let PastedContent::Image(image) = pasted else { return None };
        let (data, pixels) = encode_pasted(image)?;
        let image = Image { data, pixels };
        let size = image.natural_size();
        Some((Fit::Best, image, size))
    }

    /// Resized by its corner, keeping its shape; the image, filling it.
    fn canvas_ui(&mut self, cx: &mut AnnotationUi) -> bool {
        cx.resize_corner(Some(self.aspect()));
        cx.card(Color32::TRANSPARENT);
        self.paint(cx.painter, cx.rect(), cx.zoom(), cx.theme);
        false
    }

    fn fill(&self, _theme: &Colours) -> Color32 {
        Color32::TRANSPARENT
    }

    /// Just the image, filling its rect (whose shape is the image's).
    fn paint(&self, painter: &Painter, rect: Rect, _zoom: f32, theme: &Colours) {
        if self.data.is_empty() {
            painter.rect_stroke(rect, 2.0, Stroke::new(1.0, theme.text_disabled), egui::StrokeKind::Inside);
            return;
        }
        let uri = image_uri(painter.ctx(), &self.data);
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        match painter.ctx().try_load_texture(&uri, Default::default(), egui::SizeHint::default()) {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                painter.image(texture.id, rect, uv, Color32::WHITE);
            }
            _ => {
                painter.rect_filled(rect, 2.0, theme.surface_raised);
                painter.ctx().request_repaint();
            }
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let [w, h] = self.pixels;
        ui.label(format!("{w} × {h} pixels, {:.0} kB", self.data.len() as f64 * 0.75 / 1e3));
        if ui.button("Replace with a file…").clicked()
            && let Some(image) = Image::from_file()
        {
            *self = image;
        }
        ui.weak("Its shape stays the image's when resized.");
    }

    /// A framed picture: a sun over hills.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
        let frame = Rect::from_min_max(p(1.5, 2.5), p(14.5, 13.5));
        let outline = Stroke::new(s, Color32::from_gray(90));
        painter.rect(frame, s, Color32::from_rgb(170, 210, 245), outline, egui::StrokeKind::Middle);
        painter.circle_filled(p(10.5, 5.5), 1.6 * s, Color32::from_rgb(245, 200, 70));
        let hill = |points, colour| Shape::convex_polygon(points, colour, Stroke::NONE);
        painter.add(hill(vec![p(2.0, 13.0), p(6.5, 7.5), p(11.0, 13.0)], Color32::from_rgb(70, 150, 80)));
        painter.add(hill(vec![p(8.0, 13.0), p(11.0, 9.5), p(14.0, 13.0)], Color32::from_rgb(50, 120, 65)));
    }
}
