//! Toolbar icons, painted rather than taken from a font so they're crisp and consistent at any scale.
//! Each is drawn on a 16×16 grid.

use std::f32::consts::PI;

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};

use super::style;
use crate::core::engine::Category;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    New,
    Open,
    Save,
    Undo,
    Redo,
    AddNode,
    Duplicate,
    Delete,
    FrameAll,
    ActualSize,
    /// Take a docked window out, to float.
    Undock,
    /// Put a floating window back in the layout.
    Dock,
    /// Choose the light or dark look.
    Theme,
    /// Start running the graph.
    Play,
    /// Stop running it.
    Stop,
    /// Show the developer tools: a terminal window.
    DevTools,
    /// Start or stop recording: a red dot.
    Record,
}

const PAPER: Color32 = Color32::from_gray(246);
const FOLDER: Color32 = Color32::from_rgb(232, 182, 72);
const FOLDER_BACK: Color32 = Color32::from_rgb(200, 145, 45);
const BLUE: Color32 = Color32::from_rgb(64, 122, 214);
const GREEN: Color32 = Color32::from_rgb(56, 168, 82);
const RED: Color32 = Color32::from_rgb(214, 64, 56);

impl Icon {
    /// Paints the icon to fill `rect`. `ink` is for outlines; `enabled` false greys it out.
    pub fn paint(self, painter: &Painter, rect: Rect, ink: Color32, enabled: bool) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + vec2(x, y) * s;
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.35) };
        let ink = fade(ink);
        let line = Stroke::new(s, ink);
        let poly = |points: Vec<Pos2>, fill: Color32| {
            painter.add(Shape::convex_polygon(points, fade(fill), line));
        };
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));
        match self {
            Icon::New => page(painter, &p, 3.0, 1.0, fade(PAPER), line),
            Icon::Open => {
                poly(
                    vec![p(1.0, 3.0), p(6.0, 3.0), p(7.5, 4.5), p(14.0, 4.5), p(14.0, 13.5), p(1.0, 13.5)],
                    FOLDER_BACK,
                );
                poly(vec![p(3.5, 7.5), p(15.5, 7.5), p(13.0, 13.5), p(1.0, 13.5)], FOLDER);
            }
            Icon::Save => {
                painter.rect(r(1.5, 1.5, 14.5, 14.5), 1.5 * s, fade(BLUE), line, StrokeKind::Middle);
                painter.rect_filled(r(4.0, 1.5, 12.0, 6.5), 0.0, fade(Color32::from_gray(225)));
                painter.rect_filled(r(9.5, 2.5, 11.0, 5.5), 0.0, fade(BLUE));
                painter.rect_filled(r(3.5, 9.0, 12.5, 14.5), 0.0, fade(PAPER));
            }
            Icon::Undo | Icon::Redo => {
                // A curved arrow, round the top and down to the left (undo) or right (redo).
                let flip = |x: f32| if self == Icon::Redo { 16.0 - x } else { x };
                let (cx, cy, radius) = (8.0, 9.5, 5.0);
                let arc: Vec<Pos2> = (0..=16)
                    .map(|i| {
                        let a = -PI * i as f32 / 16.0;
                        p(flip(cx + radius * a.cos()), cy + radius * a.sin())
                    })
                    .collect();
                painter.add(Shape::line(arc, Stroke::new(2.0 * s, fade(BLUE))));
                let tip = vec![
                    p(flip(cx - radius - 3.0), cy - 1.0),
                    p(flip(cx - radius + 3.0), cy - 1.0),
                    p(flip(cx - radius), cy + 3.5),
                ];
                painter.add(Shape::convex_polygon(tip, fade(BLUE), Stroke::NONE));
            }
            Icon::AddNode => {
                let body = r(1.0, 4.0, 12.0, 14.5);
                painter.rect(body, 2.0 * s, fade(PAPER), line, StrokeKind::Middle);
                painter.rect_filled(
                    r(1.5, 4.5, 11.5, 7.5),
                    1.0 * s,
                    fade(style::category_colour(Category::Processing)),
                );
                plus(painter, &p, s, 12.5, 3.5, fade(GREEN));
            }
            Icon::Duplicate => {
                page(painter, &p, 1.5, 0.5, fade(PAPER), line);
                page(painter, &p, 5.0, 3.5, fade(PAPER), line);
            }
            Icon::Delete => {
                let stroke = Stroke::new(2.2 * s, fade(RED));
                painter.line_segment([p(3.5, 3.5), p(12.5, 12.5)], stroke);
                painter.line_segment([p(12.5, 3.5), p(3.5, 12.5)], stroke);
            }
            Icon::FrameAll => {
                for (x, y, dx, dy) in
                    [(1.5, 1.5, 1.0, 1.0), (14.5, 1.5, -1.0, 1.0), (1.5, 14.5, 1.0, -1.0), (14.5, 14.5, -1.0, -1.0)]
                {
                    let corner = vec![p(x + 4.0 * dx, y), p(x, y), p(x, y + 4.0 * dy)];
                    painter.add(Shape::line(corner, Stroke::new(1.6 * s, ink)));
                }
                painter.rect(
                    r(5.5, 5.5, 10.5, 10.5),
                    1.0 * s,
                    fade(style::category_colour(Category::Source)),
                    Stroke::NONE,
                    StrokeKind::Middle,
                );
            }
            Icon::Undock => {
                painter.rect_stroke(r(1.5, 4.5, 11.5, 14.5), 1.0 * s, line, StrokeKind::Middle);
                let arrow = Stroke::new(1.6 * s, ink);
                painter.line_segment([p(6.5, 9.5), p(14.0, 2.0)], arrow);
                painter.add(Shape::line(vec![p(9.0, 2.0), p(14.0, 2.0), p(14.0, 7.0)], arrow));
            }
            Icon::Dock => {
                painter.rect_stroke(r(1.5, 1.5, 14.5, 14.5), 1.0 * s, line, StrokeKind::Middle);
                painter.rect_filled(r(2.0, 9.0, 14.0, 14.0), 0.0, ink.gamma_multiply(0.6));
                let arrow = Stroke::new(1.6 * s, ink);
                painter.line_segment([p(8.0, 3.5), p(8.0, 7.5)], arrow);
                painter.add(Shape::line(vec![p(5.5, 5.5), p(8.0, 8.0), p(10.5, 5.5)], arrow));
            }
            Icon::Theme => {
                // A circle, half light and half dark.
                let centre = p(8.0, 8.0);
                let radius = 6.0 * s;
                painter.circle_filled(centre, radius, fade(PAPER));
                let dark: Vec<Pos2> = (0..=16)
                    .map(|i| {
                        let a = -PI / 2.0 + PI * i as f32 / 16.0;
                        centre + vec2(a.cos(), a.sin()) * radius
                    })
                    .collect();
                painter.add(Shape::convex_polygon(dark, fade(Color32::from_gray(40)), Stroke::NONE));
                painter.circle_stroke(centre, radius, line);
            }
            Icon::Play => {
                poly(vec![p(4.0, 2.0), p(14.0, 8.0), p(4.0, 14.0)], GREEN);
            }
            Icon::Record => {
                painter.circle(p(8.0, 8.0), 5.0 * s, fade(RED), line);
            }
            Icon::DevTools => {
                painter.rect(r(1.0, 2.0, 15.0, 14.0), 1.5 * s, fade(Color32::from_gray(40)), line, StrokeKind::Middle);
                let prompt = Stroke::new(1.5 * s, fade(GREEN));
                painter.add(Shape::line(vec![p(3.5, 5.5), p(6.5, 8.0), p(3.5, 10.5)], prompt));
                painter.line_segment([p(8.0, 11.0), p(12.0, 11.0)], Stroke::new(1.5 * s, fade(PAPER)));
            }
            Icon::Stop => {
                painter.rect(r(3.0, 3.0, 13.0, 13.0), 1.5 * s, fade(RED), line, StrokeKind::Middle);
            }
            Icon::ActualSize => {
                painter.circle(p(6.5, 6.5), 4.5 * s, fade(PAPER), line);
                painter.line_segment([p(9.8, 9.8), p(14.5, 14.5)], Stroke::new(2.4 * s, ink));
                painter.text(
                    p(6.5, 6.7),
                    eframe::egui::Align2::CENTER_CENTER,
                    "1",
                    eframe::egui::FontId::proportional(7.5 * s),
                    ink,
                );
            }
        }
    }
}

/// A sheet of paper with a folded corner, 10×14 with its top-left at (x, y).
fn page(painter: &Painter, p: &impl Fn(f32, f32) -> Pos2, x: f32, y: f32, fill: Color32, line: Stroke) {
    let outline = vec![p(x, y), p(x + 7.0, y), p(x + 10.0, y + 3.0), p(x + 10.0, y + 14.0), p(x, y + 14.0)];
    painter.add(Shape::convex_polygon(outline, fill, line));
    painter.add(Shape::line(vec![p(x + 7.0, y), p(x + 7.0, y + 3.0), p(x + 10.0, y + 3.0)], line));
}

fn plus(painter: &Painter, p: &impl Fn(f32, f32) -> Pos2, s: f32, x: f32, y: f32, colour: Color32) {
    let stroke = Stroke::new(2.2 * s, colour);
    painter.line_segment([p(x - 3.0, y), p(x + 3.0, y)], stroke);
    painter.line_segment([p(x, y - 3.0), p(x, y + 3.0)], stroke);
}

/// A small downward triangle, for dropdowns.
pub fn dropdown_arrow(painter: &Painter, centre: Pos2, colour: Color32) {
    let points = vec![centre + vec2(-3.5, -1.5), centre + vec2(3.5, -1.5), centre + vec2(0.0, 2.5)];
    painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
}
