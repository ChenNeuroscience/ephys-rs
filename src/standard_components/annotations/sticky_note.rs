//! A note on the canvas, written in markdown.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Painter, Rect, RichText, Shape, Stroke};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};

use crate::core::annotation::{AnnotationInfo, AnnotationType};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickyNote {
    /// Markdown.
    pub text: String,
    pub colour: NoteColour,
}

impl Default for StickyNote {
    fn default() -> Self {
        StickyNote {
            text: "### Note\nDouble-click to edit. Supports **markdown**:\n\n- lists\n- `code`".into(),
            colour: NoteColour::Yellow,
        }
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
        description: "A note on the canvas, written in markdown. Not part of the graph.",
        default_size: [220.0, 140.0],
    };

    /// Drawn out, it starts empty, to type straight into.
    fn drawn() -> Self {
        StickyNote { text: String::new(), ..Default::default() }
    }

    fn fill(&self) -> Color32 {
        self.colour.fill()
    }

    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool {
        let style = ui.style_mut();
        style.visuals.override_text_color = Some(INK);
        // Strong text (bold, headings) takes its colour from the widget visuals.
        for widget in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.fg_stroke.color = INK;
        }
        if editing {
            let editor = ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(4)
                    .text_color(INK)
                    .code_editor(),
            );
            if !editor.has_focus() && !editor.lost_focus() {
                editor.request_focus();
            }
            !editor.lost_focus()
        } else {
            // The markdown viewer's cache, shared by every note.
            let id = egui::Id::unique("sticky note markdown");
            let cache = ui.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<CommonMarkCache>>>(id).clone());
            CommonMarkViewer::new().show(ui, &mut cache.lock().unwrap(), &self.text);
            false
        }
    }

    fn context_menu_ui(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Colour", |ui| {
            for colour in NoteColour::ALL {
                if ui.selectable_label(colour == self.colour, colour.name()).clicked() {
                    self.colour = colour;
                }
            }
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for colour in NoteColour::ALL {
                ui.selectable_value(&mut self.colour, colour, colour.name());
            }
        });
        ui.add_space(4.0);
        let label = ui.label(RichText::new("Markdown").small().weak());
        ui.add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_rows(12).desired_width(f32::INFINITY))
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
