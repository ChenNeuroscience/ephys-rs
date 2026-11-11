//! Toolbar icons, painted rather than taken from a font so they're crisp and consistent at any scale.
//! Each is drawn on a 16×16 grid.

use std::f32::consts::PI;

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};

use super::style;
use crate::engine::Category;

/// An icon, drawn with egui's painter rather than from an image, so it's crisp at any size and in any theme's
/// colours.
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
    /// Arrange the nodes: boxes in columns, joined.
    Arrange,
    FrameAll,
    ActualSize,
    /// Take a docked window out, to float.
    Undock,
    /// Put a floating window back in the layout.
    Dock,
    /// Choose the light or dark look.
    Theme,
    /// Go home: the home pane, basic or graph. A house.
    Home,
    /// On or off: the power symbol.
    Power,
    /// Sound on or off: a speaker.
    Sound,
    /// A spectrum: bars of power across frequencies.
    Spectrum,
    /// Copy settings: two sheets.
    Copy,
    /// Paste settings: a clipboard.
    Paste,
    /// Start running the graph.
    Play,
    /// Stop running it.
    Stop,
    /// Show the developer tools: a terminal window.
    DevTools,
    /// Start or stop recording: a red dot.
    Record,
    /// Advanced mode, with the graph: three nodes, joined.
    Graph,
    /// A pane's settings: a gear.
    Settings,
    /// Turn a signal upside down: a spike, and under the line, its reflection.
    Invert,
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
                // A trash can (a cross would read as closing): its lid and handle, its body, and its ribs.
                let stroke = Stroke::new(1.5 * s, ink);
                painter.line_segment([p(2.5, 4.0), p(13.5, 4.0)], stroke);
                painter.add(Shape::line(vec![p(6.0, 4.0), p(6.5, 2.0), p(9.5, 2.0), p(10.0, 4.0)], stroke));
                painter.add(Shape::line(vec![p(3.75, 4.0), p(4.75, 14.0), p(11.25, 14.0), p(12.25, 4.0)], stroke));
                let rib = Stroke::new(1.2 * s, ink);
                painter.line_segment([p(6.75, 6.5), p(7.0, 11.5)], rib);
                painter.line_segment([p(9.25, 6.5), p(9.0, 11.5)], rib);
            }
            Icon::Arrange => {
                let link = Stroke::new(1.2 * s, ink);
                painter.line_segment([p(5.5, 8.0), p(10.5, 4.5)], link);
                painter.line_segment([p(5.5, 8.0), p(10.5, 11.5)], link);
                let colour = |c| fade(style::category_colour(c));
                for (x, y, c) in [
                    (1.5, 6.0, Category::Source),
                    (10.5, 2.5, Category::Processing),
                    (10.5, 9.5, Category::Visualisation),
                ] {
                    painter.rect(r(x, y, x + 4.5, y + 4.0), 0.8 * s, colour(c), Stroke::NONE, StrokeKind::Middle);
                }
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
            Icon::Home => {
                let roof = vec![p(1.5, 8.0), p(8.0, 2.0), p(14.5, 8.0)];
                painter.add(Shape::line(roof, Stroke::new(1.6 * s, ink)));
                let walls = vec![p(3.5, 7.0), p(3.5, 14.0), p(12.5, 14.0), p(12.5, 7.0)];
                painter.add(Shape::line(walls, Stroke::new(1.6 * s, ink)));
                painter.rect_filled(r(6.8, 9.5, 9.2, 14.0), 0.0, fade(BLUE));
            }
            Icon::Copy => {
                page(painter, &p, 1.5, 0.5, fade(PAPER), line);
                page(painter, &p, 5.0, 2.0, fade(PAPER), line);
            }
            Icon::Paste => {
                let board = Color32::from_rgb(178, 132, 82);
                painter.rect(r(2.5, 2.0, 13.5, 15.0), 1.5 * s, fade(board), line, StrokeKind::Middle);
                painter.rect(r(4.5, 4.5, 11.5, 13.5), 0.5 * s, fade(PAPER), Stroke::NONE, StrokeKind::Middle);
                painter.rect(r(5.5, 0.8, 10.5, 3.6), 1.0 * s, fade(Color32::from_gray(140)), line, StrokeKind::Middle);
                for y in [7.0, 9.0, 11.0] {
                    painter.line_segment([p(6.0, y), p(10.0, y)], Stroke::new(0.8 * s, fade(Color32::from_gray(120))));
                }
            }
            Icon::Graph => {
                let link = Stroke::new(1.3 * s, ink);
                painter.line_segment([p(4.0, 11.5), p(8.0, 4.0)], link);
                painter.line_segment([p(8.0, 4.0), p(12.5, 11.0)], link);
                painter.line_segment([p(4.0, 11.5), p(12.5, 11.0)], link);
                for (x, y, c) in [
                    (4.0, 11.5, Category::Source),
                    (8.0, 4.0, Category::Processing),
                    (12.5, 11.0, Category::Visualisation),
                ] {
                    painter.circle(p(x, y), 2.6 * s, fade(style::category_colour(c)), Stroke::new(0.8 * s, ink));
                }
            }
            Icon::Settings => {
                // A gear: teeth round a ring, with a hole.
                let centre = p(8.0, 8.0);
                for i in 0..8 {
                    let a = PI / 4.0 * i as f32;
                    let d = vec2(a.cos(), a.sin());
                    painter.line_segment([centre + d * 4.0 * s, centre + d * 6.6 * s], Stroke::new(2.4 * s, ink));
                }
                painter.circle_stroke(centre, 4.4 * s, Stroke::new(1.8 * s, ink));
                painter.circle_stroke(centre, 1.6 * s, Stroke::new(1.2 * s, ink));
            }
            Icon::Power => {
                let stroke = Stroke::new(1.7 * s, ink);
                let centre = p(8.0, 8.8);
                let radius = 5.2 * s;
                // An arc open at the top, and a line down into it.
                let arc: Vec<Pos2> = (0..=24)
                    .map(|i| {
                        let a = -PI / 2.0 + 0.55 + (2.0 * PI - 1.1) * i as f32 / 24.0;
                        centre + vec2(a.cos(), a.sin()) * radius
                    })
                    .collect();
                painter.add(Shape::line(arc, stroke));
                painter.line_segment([p(8.0, 2.0), p(8.0, 8.5)], stroke);
            }
            Icon::Spectrum => {
                for (x, top) in [(2.5, 10.0), (5.25, 4.0), (8.0, 2.0), (10.75, 7.0), (13.5, 11.0)] {
                    painter.line_segment([p(x, 14.0), p(x, top)], Stroke::new(1.8 * s, ink));
                }
            }
            Icon::Sound => {
                painter.rect_filled(r(1.5, 6.0, 5.0, 10.0), 0.0, ink);
                let cone = vec![p(4.5, 6.0), p(8.5, 2.5), p(8.5, 13.5), p(4.5, 10.0)];
                painter.add(Shape::convex_polygon(cone, ink, Stroke::NONE));
                // Two waves coming off it.
                for radius in [2.6, 5.2] {
                    let wave: Vec<Pos2> = (0..=12)
                        .map(|i| {
                            let a = -0.8 + 1.6 * i as f32 / 12.0;
                            p(9.0, 8.0) + vec2(a.cos(), a.sin()) * radius * s
                        })
                        .collect();
                    painter.add(Shape::line(wave, Stroke::new(1.4 * s, ink)));
                }
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
            Icon::Invert => {
                let middle = Stroke::new(s, ink.gamma_multiply(0.5));
                painter.line_segment([p(1.5, 8.0), p(14.5, 8.0)], middle);
                let spike = Stroke::new(1.6 * s, ink);
                painter
                    .add(Shape::line(vec![p(2.0, 8.0), p(5.0, 8.0), p(8.0, 1.5), p(11.0, 8.0), p(14.0, 8.0)], spike));
                let reflection = Stroke::new(1.2 * s, ink.gamma_multiply(0.45));
                painter.add(Shape::line(vec![p(5.0, 8.0), p(8.0, 14.5), p(11.0, 8.0)], reflection));
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

/// An open eye when `visible`, otherwise the same eye struck through.
pub fn eye(
    painter: &egui::Painter,
    rect: egui::Rect,
    colour: egui::Color32,
    _background: egui::Color32,
    visible: bool,
) {
    let c = rect.center();
    let (w, h) = (rect.width() / 2.0, rect.height() * 0.3);
    let stroke = egui::Stroke::new(1.5, colour);
    let lid = |sign: f32| -> Vec<egui::Pos2> {
        (0..=16)
            .map(|i| {
                let x = -1.0 + 2.0 * i as f32 / 16.0;
                c + egui::vec2(x * w, sign * h * (1.0 - x * x))
            })
            .collect()
    };
    painter.add(egui::Shape::line(lid(-1.0), stroke));
    painter.add(egui::Shape::line(lid(1.0), stroke));
    if visible {
        painter.circle_filled(c, h * 0.6, colour);
    } else {
        painter.circle_stroke(c, h * 0.6, stroke);
        let inset = egui::vec2(2.0, 1.0);
        painter.line_segment([rect.left_top() + inset, rect.right_bottom() - inset], stroke);
    }
}
