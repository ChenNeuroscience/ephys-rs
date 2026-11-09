//! A heading on the canvas: plain text, large, on nothing, in the theme's text colour.

use eframe::egui::{self, Color32, FontId, Painter, Rect, RichText, Stroke};
use serde::{Deserialize, Serialize};

use crate::annotation::{AnnotationInfo, AnnotationType, Fit, PastedContent};
use crate::theme::Colours;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Heading {
    pub text: String,
    pub size: Size,
}

impl Default for Heading {
    fn default() -> Self {
        Heading { text: "Heading".into(), size: Size::Medium }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    const ALL: [Size; 3] = [Size::Small, Size::Medium, Size::Large];

    fn name(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
        }
    }

    /// How big, relative to the theme's headings.
    fn scale(self) -> f32 {
        match self {
            Size::Small => 1.0,
            Size::Medium => 1.5,
            Size::Large => 2.2,
        }
    }
}

impl AnnotationType for Heading {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Heading",
        description: "A heading on the canvas: large plain text. Not part of the graph.",
        default_size: [320.0, 60.0],
    };
    const MIN_SIZE: [f32; 2] = [60.0, 30.0];

    /// Drawn out, it starts empty, to type straight into.
    fn drawn(_flipped: [bool; 2]) -> Self {
        Heading { text: String::new(), ..Default::default() }
    }

    /// Text pasted, if it's a single short line: what it's for.
    fn pasted(pasted: &PastedContent) -> Option<(Fit, Self, [f32; 2])> {
        let PastedContent::Text(text) = pasted else { return None };
        (!text.contains('\n') && text.chars().count() <= 40)
            .then(|| (Fit::Best, Heading { text: (*text).into(), ..Default::default() }, Self::INFO.default_size))
    }

    fn fill(&self, _theme: &Colours) -> Color32 {
        Color32::TRANSPARENT
    }

    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool {
        // The theme's heading size, already scaled to the canvas's zoom.
        let size = egui::TextStyle::Heading.resolve(ui.style()).size * self.size.scale();
        let colour = ui.visuals().strong_text_color();
        if editing {
            let editor = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(size))
                    .text_color(colour),
            );
            if !editor.has_focus() && !editor.lost_focus() {
                editor.request_focus();
            }
            !editor.lost_focus()
        } else {
            ui.add(egui::Label::new(RichText::new(&self.text).size(size).strong().color(colour)).wrap());
            false
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let label = ui.label(RichText::new("Text").small().weak());
        ui.text_edit_singleline(&mut self.text).labelled_by(label.id);
        ui.horizontal(|ui| {
            for size in Size::ALL {
                ui.selectable_value(&mut self.size, size, size.name());
            }
        });
    }

    /// A large T over a line.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
        let ink = Stroke::new(2.4 * s, Color32::from_gray(110));
        painter.line_segment([p(3.0, 3.0), p(13.0, 3.0)], ink);
        painter.line_segment([p(8.0, 3.0), p(8.0, 11.5)], ink);
        painter.line_segment([p(2.0, 14.0), p(14.0, 14.0)], Stroke::new(s, Color32::from_gray(140)));
    }
}
