//! A sticky note on the canvas: plain text, on coloured paper. (For formatting, links and images, see `markdown`.)

use eframe::egui::{self, Color32, Painter, Rect, RichText, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::annotation::{AnnotationInfo, AnnotationType, Fit, PastedContent};
use crate::theme::Colours;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickyNote {
    pub text: String,
    pub colour: NoteColour,
}

impl Default for StickyNote {
    fn default() -> Self {
        StickyNote { text: "Note\n\nDouble-click to edit.".into(), colour: NoteColour::Yellow }
    }
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum NoteColour {
    Yellow,
    Blue,
    Green,
    Pink,
}

impl NoteColour {
    pub const ALL: [NoteColour; 4] = [NoteColour::Yellow, NoteColour::Blue, NoteColour::Green, NoteColour::Pink];

    pub fn name(self) -> &'static str {
        match self {
            NoteColour::Yellow => "Yellow",
            NoteColour::Blue => "Blue",
            NoteColour::Green => "Green",
            NoteColour::Pink => "Pink",
        }
    }

    pub fn fill(self) -> Color32 {
        match self {
            NoteColour::Yellow => Color32::from_rgb(242, 222, 130),
            NoteColour::Blue => Color32::from_rgb(160, 200, 245),
            NoteColour::Green => Color32::from_rgb(170, 225, 170),
            NoteColour::Pink => Color32::from_rgb(242, 175, 200),
        }
    }
}

/// Text on a note.
const INK: Color32 = Color32::from_gray(30);

impl AnnotationType for StickyNote {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Sticky Note",
        description: "A sticky note on the canvas, in plain text. Not part of the graph.",
        default_size: [220.0, 140.0],
    };

    /// Drawn out, it starts empty, to type straight into.
    fn drawn(_flipped: [bool; 2]) -> Self {
        StickyNote { text: String::new(), ..Default::default() }
    }

    /// Text pasted: a short note shows it well; a longer one can (markdown, perhaps, better).
    fn pasted(pasted: &PastedContent) -> Option<(Fit, Self, [f32; 2])> {
        let PastedContent::Text(text) = pasted else { return None };
        let fit = if text.chars().count() <= 300 { Fit::Good } else { Fit::Possible };
        Some((fit, StickyNote { text: (*text).into(), ..Default::default() }, Self::INFO.default_size))
    }

    fn fill(&self, _theme: &Colours) -> Color32 {
        self.colour.fill()
    }

    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool {
        if editing {
            let editor = ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(4)
                    .text_color(INK),
            );
            if !editor.has_focus() && !editor.lost_focus() {
                editor.request_focus();
            }
            !editor.lost_focus()
        } else {
            ui.add(egui::Label::new(RichText::new(&self.text).color(INK)).wrap());
            false
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for colour in NoteColour::ALL {
                ui.selectable_value(&mut self.colour, colour, colour.name());
            }
        });
        ui.add_space(4.0);
        let label = ui.label(RichText::new("Text").small().weak());
        ui.add(egui::TextEdit::multiline(&mut self.text).desired_rows(12).desired_width(f32::INFINITY))
            .labelled_by(label.id);
    }

    /// A yellow note with a folded corner and some lines of writing.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
        let outline = Stroke::new(s, Color32::from_gray(90));
        let fill = NoteColour::Yellow.fill();
        painter.add(Shape::convex_polygon(
            vec![p(2.0, 2.0), p(14.0, 2.0), p(14.0, 10.5), p(10.5, 14.0), p(2.0, 14.0)],
            fill,
            outline,
        ));
        painter.add(Shape::convex_polygon(
            vec![p(14.0, 10.5), p(10.5, 10.5), p(10.5, 14.0)],
            Color32::from_rgb(214, 182, 70),
            outline,
        ));
        for y in [5.0, 7.5, 10.0] {
            let end = if y > 9.0 { 9.0 } else { 11.5 };
            painter.line_segment([p(4.5, y), p(end, y)], Stroke::new(s, Color32::from_gray(120)));
        }
    }
}
