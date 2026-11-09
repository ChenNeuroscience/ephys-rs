//! A box on the canvas, behind everything else, to group what's in it: a tinted fill and a border, in a colour
//! that shows whether the canvas is light or dark, and a title. It's grabbed by its border or title, so what's
//! inside it can still be grabbed.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, RichText, Shape, Stroke, StrokeKind};
use serde::{Deserialize, Serialize};

use super::Hue;
use crate::annotation::{AnnotationInfo, AnnotationType};
use crate::theme::Colours;

/// How far in from its edge, on screen, a box can be grabbed.
const GRAB: f32 = 8.0;
/// The title's strip, at the top, in canvas units.
const TITLE: f32 = 28.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupBox {
    pub title: String,
    pub hue: Hue,
    /// Whether it's tinted inside.
    pub fill: bool,
    pub border: Border,
}

impl Default for GroupBox {
    fn default() -> Self {
        GroupBox { title: "Group".into(), hue: Hue::Blue, fill: true, border: Border::Solid }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Border {
    Solid,
    Dashed,
    None,
}

impl Border {
    const ALL: [Border; 3] = [Border::Solid, Border::Dashed, Border::None];

    fn name(self) -> &'static str {
        match self {
            Border::Solid => "Solid",
            Border::Dashed => "Dashed",
            Border::None => "None",
        }
    }
}

impl AnnotationType for GroupBox {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Box",
        description: "A box behind the graph, to group what's in it, with a title. Grab it by its border or title. \
                      Not part of the graph.",
        default_size: [420.0, 260.0],
    };

    fn fill(&self, _theme: &Colours) -> Color32 {
        if self.fill { self.hue.colour().gamma_multiply(0.14) } else { Color32::TRANSPARENT }
    }

    fn paint(&self, painter: &Painter, rect: Rect, zoom: f32, _theme: &Colours) {
        let stroke = Stroke::new(1.5 * zoom.sqrt(), self.hue.colour());
        match self.border {
            Border::Solid => {
                painter.rect_stroke(rect, 4.0 * zoom, stroke, StrokeKind::Inside);
            }
            Border::Dashed => {
                let r = rect.shrink(stroke.width / 2.0);
                let corners = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                painter.extend(Shape::dashed_line(&corners, stroke, 8.0 * zoom, 5.0 * zoom));
            }
            Border::None => {}
        }
    }

    fn hit(&self, rect: Rect, at: Pos2, zoom: f32) -> bool {
        let title = at.y < rect.top() + TITLE * zoom;
        title || !rect.shrink(GRAB).contains(at)
    }

    fn behind(&self) -> bool {
        true
    }

    /// Beneath the graph, so the nodes it groups are on it.
    fn above_graph(&self) -> bool {
        false
    }

    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool {
        let size = egui::TextStyle::Body.resolve(ui.style()).size * 1.2;
        let colour = self.hue.colour();
        if editing {
            let editor = ui.add(
                egui::TextEdit::singleline(&mut self.title)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(size))
                    .text_color(colour),
            );
            if !editor.has_focus() && !editor.lost_focus() {
                editor.request_focus();
            }
            !editor.lost_focus()
        } else {
            ui.add(egui::Label::new(RichText::new(&self.title).size(size).strong().color(colour)).truncate());
            false
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let label = ui.label(RichText::new("Title").small().weak());
        ui.text_edit_singleline(&mut self.title).labelled_by(label.id);
        ui.add_space(4.0);
        Hue::picker(ui, &mut self.hue);
        ui.checkbox(&mut self.fill, "Fill");
        ui.horizontal(|ui| {
            ui.label("Border");
            for border in Border::ALL {
                ui.selectable_value(&mut self.border, border, border.name());
            }
        });
    }

    /// A tinted box with a dashed border.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let r = Rect::from_min_max(rect.min + egui::vec2(1.5, 2.5) * s, rect.min + egui::vec2(14.5, 13.5) * s);
        let blue = Hue::Blue.colour();
        painter.rect_filled(r, s, blue.gamma_multiply(0.3));
        let corners = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.extend(Shape::dashed_line(&corners, Stroke::new(1.2 * s, blue), 2.5 * s, 1.5 * s));
    }
}
