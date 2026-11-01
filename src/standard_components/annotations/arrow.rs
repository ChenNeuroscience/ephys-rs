//! An arrow on the canvas, to point things out: straight, thick and with a large head, in a colour of its own, so
//! it isn't taken for one of the graph's edges (thin curves, with small arrowheads halfway along). It runs between
//! two points, the way it was drawn, and each end can be dragged.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use serde::{Deserialize, Serialize};

use super::Hue;
use crate::core::annotation::{AnnotationInfo, AnnotationType};
use crate::core::theme::Colours;

/// How near its line, on screen, an arrow can be grabbed.
const GRAB: f32 = 10.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arrow {
    pub hue: Hue,
    /// Whether it points leftwards, and upwards.
    pub flipped: [bool; 2],
}

impl Default for Arrow {
    fn default() -> Self {
        Arrow { hue: Hue::Grey, flipped: [false; 2] }
    }
}

impl Arrow {
    /// Where it starts and ends, within `rect` (on screen).
    fn ends(&self, rect: Rect) -> (Pos2, Pos2) {
        let [left, up] = self.flipped;
        let x = |towards: bool| if towards { rect.left() } else { rect.right() };
        let y = |towards: bool| if towards { rect.top() } else { rect.bottom() };
        (Pos2::new(x(!left), y(!up)), Pos2::new(x(left), y(up)))
    }
}

impl AnnotationType for Arrow {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Arrow",
        description: "An arrow to point something out, drawn from where it starts to where it points. Not part of \
                      the graph.",
        default_size: [140.0, 60.0],
    };
    const MIN_SIZE: [f32; 2] = [4.0, 4.0];

    fn drawn(flipped: [bool; 2]) -> Self {
        Arrow { flipped, ..Default::default() }
    }

    fn fill(&self, _theme: &Colours) -> Color32 {
        Color32::TRANSPARENT
    }

    fn paint(&self, painter: &Painter, rect: Rect, zoom: f32, _theme: &Colours) {
        let (start, end) = self.ends(rect);
        let direction = (end - start).normalized();
        if !direction.is_finite() {
            return;
        }
        let colour = self.hue.colour();
        let (width, head) = (4.0 * zoom, 18.0 * zoom);
        let base = end - direction * head;
        painter.circle_filled(start, width * 0.9, colour);
        painter.line_segment([start, base + direction * 1.0], Stroke::new(width, colour));
        let side = direction.rot90() * head * 0.55;
        painter.add(Shape::convex_polygon(vec![end, base + side, base - side], colour, Stroke::NONE));
    }

    fn points(&self, rect: Rect) -> Option<[Pos2; 2]> {
        let (start, end) = self.ends(rect);
        Some([start, end])
    }

    fn set_points(&mut self, [start, end]: [Pos2; 2]) -> Rect {
        self.flipped = [end.x < start.x, end.y < start.y];
        Rect::from_two_pos(start, end)
    }

    fn hit(&self, rect: Rect, at: Pos2, _zoom: f32) -> bool {
        let (start, end) = self.ends(rect);
        let along: Vec2 = end - start;
        let t = ((at - start).dot(along) / along.length_sq().max(1e-6)).clamp(0.0, 1.0);
        (start + along * t).distance(at) <= GRAB
    }

    fn content_ui(&mut self, _ui: &mut egui::Ui, _editing: bool) -> bool {
        false
    }

    fn context_menu_ui(&mut self, ui: &mut egui::Ui) {
        Hue::menu(ui, &mut self.hue);
        if ui.button("Reverse").clicked() {
            self.flipped = self.flipped.map(|f| !f);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        Hue::picker(ui, &mut self.hue);
        if ui.button("Reverse").clicked() {
            self.flipped = self.flipped.map(|f| !f);
        }
        ui.weak("Drag either end to move it.");
    }

    /// A thick grey arrow, up and to the right.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
        let grey = Hue::Grey.colour();
        painter.line_segment([p(2.5, 13.5), p(10.0, 6.0)], Stroke::new(2.2 * s, grey));
        painter.add(Shape::convex_polygon(vec![p(14.0, 2.0), p(13.0, 9.5), p(6.5, 3.0)], grey, Stroke::NONE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_the_way_it_was_drawn_and_is_grabbed_near_its_line() {
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0));
        let up_left = Arrow::drawn([true, true]);
        assert_eq!(up_left.ends(rect), (egui::pos2(100.0, 50.0), egui::pos2(0.0, 0.0)));
        assert!(up_left.hit(rect, egui::pos2(52.0, 24.0), 1.0), "on the line");
        assert!(!up_left.hit(rect, egui::pos2(90.0, 5.0), 1.0), "in its box, but away from the line");
        // Dragging its start past its end turns it round.
        let mut arrow = up_left;
        let moved = arrow.set_points([egui::pos2(0.0, 0.0), egui::pos2(60.0, 40.0)]);
        assert_eq!(arrow.flipped, [false, false]);
        assert_eq!(arrow.points(moved), Some([egui::pos2(0.0, 0.0), egui::pos2(60.0, 40.0)]));
    }
}
