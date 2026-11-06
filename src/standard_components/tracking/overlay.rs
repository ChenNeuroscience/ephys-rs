//! What's drawn over the video, and dragged: the crop (moved, resized by its sides and corners, and rotated),
//! the calibration rulers and the timing LED's spot; and the LEDs found, drawn as circles.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use eframe::egui::{self, Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, Response, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::pipeline::Blob;
use super::{Crop, Detection, Ruler, TimingLed, from_local, to_local};

pub const CROP_COLOUR: Color32 = Color32::YELLOW;
pub const RULER_COLOUR: Color32 = Color32::from_rgb(0, 200, 255);
pub const TIMING_COLOUR: Color32 = Color32::from_rgb(120, 180, 255);
/// The biggest LED, then the second biggest.
const LED_COLOURS: [Color32; 2] = [Color32::from_rgb(80, 220, 120), Color32::from_rgb(255, 110, 200)];

// Screen-space sizes, in points.
const HANDLE_RADIUS: f32 = 5.0;
const HIT_RADIUS: f32 = 9.0;
const RULER_HIT_DISTANCE: f32 = 6.0;
const ROTATE_STALK: f32 = 28.0;
/// How far the handle resizing the timing LED's spot is from its edge.
const TIMING_STALK: f32 = 12.0;
/// Smallest crop dimension, in image pixels.
const MIN_SIZE: f32 = 4.0;
const ROTATE_SNAP: f32 = 15.0 * PI / 180.0;

/// Whether something drawn over the video is shown, and can be dragged.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub visible: bool,
    /// Locked overlays are drawn but can't be dragged.
    pub locked: bool,
}

impl Default for Overlay {
    fn default() -> Self {
        Overlay { visible: true, locked: false }
    }
}

impl Overlay {
    pub fn editable(&self) -> bool {
        self.visible && !self.locked
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlays {
    /// Hidden: the outside of the crop is masked out. Visible: the whole frame is shown with the crop outline.
    pub crop: Overlay,
    /// Above-threshold pixels highlighted and the LEDs circled.
    pub detection: Overlay,
    pub rulers: Overlay,
    /// The timing LED's spot, and its pixels above the threshold.
    pub timing: Overlay,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    CropMove,
    CropRadius,
    /// A side or corner of a square or rectangle: for x then y, which side moves (+1 or -1), or 0 if neither.
    /// The opposite side(s) stay put.
    CropResize {
        sides: [i8; 2],
    },
    CropRotate,
    RulerEnd {
        ruler: usize,
        end: usize,
    },
    RulerMove(usize),
    TimingMove,
    TimingRadius,
}

/// A drag in progress.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    target: Target,
    /// Pointer position at the previous step, in image coordinates.
    last: [f32; 2],
}

/// Maps between image pixels and screen points for the displayed frame.
#[derive(Clone, Copy)]
pub struct Xform {
    pub origin: Pos2,
    pub scale: f32,
}

impl Xform {
    pub fn to_screen(self, p: [f32; 2]) -> Pos2 {
        self.origin + egui::vec2(p[0], p[1]) * self.scale
    }

    pub fn to_image(self, p: Pos2) -> [f32; 2] {
        let v = (p - self.origin) / self.scale;
        [v.x, v.y]
    }
}

/// Handles dragging the crop and rulers on the video `response`. Returns whether `detection` changed.
pub fn interact(
    ui: &egui::Ui,
    response: &Response,
    xf: Xform,
    detection: &mut Detection,
    overlays: &Overlays,
    drag: &mut Option<Drag>,
) -> bool {
    let mut changed = false;
    if response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        *drag = hit(origin, xf, detection, overlays).map(|target| Drag { target, last: xf.to_image(origin) });
    }
    if response.dragged()
        && let (Some(d), Some(pos)) = (drag.as_mut(), response.interact_pointer_pos())
    {
        let p = xf.to_image(pos);
        let snap = ui.input(|i| i.modifiers.shift);
        apply_drag(d.target, p, [p[0] - d.last[0], p[1] - d.last[1]], snap, detection);
        d.last = p;
        changed = true;
    }
    if response.drag_stopped() {
        *drag = None;
    }
    if drag.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if let Some(target) = response.hover_pos().and_then(|p| hit(p, xf, detection, overlays)) {
        ui.ctx().set_cursor_icon(if target == Target::CropMove { CursorIcon::Move } else { CursorIcon::Grab });
    }
    changed
}

/// Draws the crop, edge band, rulers, timing LED's spot (filled in if `timing_on`) and LEDs, as the overlays say.
pub fn draw(painter: &Painter, xf: Xform, detection: &Detection, overlays: &Overlays, leds: &[Blob], timing_on: bool) {
    draw_crop(painter, xf, &detection.crop, overlays.crop);
    if overlays.crop.visible
        && detection.edge_width() > 0.0
        && let Some(inset) = detection.crop.inset(detection.edge_width())
    {
        draw_outline(painter, xf, &inset, Stroke::new(1.0, CROP_COLOUR), true);
    }
    if overlays.rulers.visible {
        for (i, ruler) in detection.rulers.iter().enumerate() {
            draw_ruler(painter, xf, i, ruler, overlays.rulers.locked);
        }
    }
    if overlays.timing.visible && detection.timing_led.enabled {
        draw_timing(painter, xf, &detection.timing_led, overlays.timing.locked, timing_on);
    }
    if overlays.detection.visible {
        for (led, colour) in leds.iter().zip(LED_COLOURS) {
            let r = (led.area as f32 / PI).sqrt() * xf.scale + 4.0;
            // Centroids are in pixel indices; +0.5 puts them at pixel centres on screen.
            painter.circle_stroke(xf.to_screen([led.x + 0.5, led.y + 0.5]), r, Stroke::new(2.0, colour));
        }
    }
    if let Some(ppm) = detection.pixels_per_metre() {
        let corner = xf.to_screen([0.0, 0.0]) + egui::vec2(8.0, 8.0);
        label(painter, corner, Align2::LEFT_TOP, format!("{ppm:.1} px/m"), Color32::WHITE);
    }
}

/// Point handles in priority order, in image coordinates.
fn handles(xf: Xform, detection: &Detection, overlays: &Overlays) -> Vec<(Target, [f32; 2])> {
    let mut out = Vec::new();
    if overlays.timing.editable() && detection.timing_led.enabled {
        out.push((Target::TimingRadius, timing_handle(xf, &detection.timing_led)));
    }
    if overlays.rulers.editable() {
        for (i, r) in detection.rulers.iter().enumerate() {
            out.push((Target::RulerEnd { ruler: i, end: 0 }, r.a));
            out.push((Target::RulerEnd { ruler: i, end: 1 }, r.b));
        }
    }
    if overlays.crop.editable() {
        out.extend(crop_handles(&detection.crop, ROTATE_STALK / xf.scale));
    }
    out
}

/// Every corner and edge midpoint of a square or rectangle, every compass point of a circle, plus the
/// rotation handle.
fn crop_handles(crop: &Crop, stalk: f32) -> Vec<(Target, [f32; 2])> {
    let (Some(center), Some([hw, hh])) = (crop.center(), crop.half_extents()) else { return vec![] };
    if let Crop::Circle { .. } = crop {
        return [[1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0]]
            .map(|[dx, dy]| (Target::CropRadius, [center[0] + dx * hw, center[1] + dy * hh]))
            .to_vec();
    }
    let angle = crop.angle();
    let mut handles: Vec<_> = [-1i8, 0, 1]
        .into_iter()
        .flat_map(|sx| [-1i8, 0, 1].map(|sy| [sx, sy]))
        .filter(|&sides| sides != [0, 0])
        .map(|sides| {
            let local = [sides[0] as f32 * hw, sides[1] as f32 * hh];
            (Target::CropResize { sides }, from_local(center, angle, local))
        })
        .collect();
    handles.push((Target::CropRotate, from_local(center, angle, [0.0, -hh - stalk])));
    handles
}

/// New centre and half extents of a box after dragging the given `sides` to `p`, keeping the opposite sides
/// where they are. A square keeps equal sides, centred on any axis that isn't being dragged.
fn resize_box(
    center: [f32; 2],
    half: [f32; 2],
    angle: f32,
    sides: [i8; 2],
    p: [f32; 2],
    square: bool,
) -> ([f32; 2], [f32; 2]) {
    let l = to_local(center, angle, p);
    let mut fixed = [0.0; 2];
    let mut size = [2.0 * half[0], 2.0 * half[1]];
    for axis in 0..2 {
        let sign = sides[axis] as f32;
        if sign != 0.0 {
            fixed[axis] = -sign * half[axis];
            size[axis] = (sign * (l[axis] - fixed[axis])).max(MIN_SIZE);
        }
    }
    if square {
        let moved = (0..2).filter(|&a| sides[a] != 0).map(|a| size[a]);
        let s = moved.fold(0.0, f32::max);
        size = [s, s];
    }
    let mut mid = [0.0; 2];
    for axis in 0..2 {
        if sides[axis] != 0 {
            mid[axis] = fixed[axis] + sides[axis] as f32 * size[axis] / 2.0;
        }
    }
    (from_local(center, angle, mid), [size[0] / 2.0, size[1] / 2.0])
}

fn hit(pos: Pos2, xf: Xform, detection: &Detection, overlays: &Overlays) -> Option<Target> {
    let nearest = handles(xf, detection, overlays)
        .into_iter()
        .map(|(t, p)| (t, xf.to_screen(p).distance(pos)))
        .filter(|&(_, d)| d <= HIT_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((target, _)) = nearest {
        return Some(target);
    }
    let timing = &detection.timing_led;
    if overlays.timing.editable()
        && timing.enabled
        && xf.to_screen(timing.center).distance(pos) <= (timing.radius * xf.scale).max(HIT_RADIUS)
    {
        return Some(Target::TimingMove);
    }
    if overlays.rulers.editable() {
        for (i, r) in detection.rulers.iter().enumerate() {
            if distance_to_segment(pos, xf.to_screen(r.a), xf.to_screen(r.b)) <= RULER_HIT_DISTANCE {
                return Some(Target::RulerMove(i));
            }
        }
    }
    let crop = &detection.crop;
    (overlays.crop.editable() && *crop != Crop::None && crop.contains(xf.to_image(pos))).then_some(Target::CropMove)
}

fn apply_drag(target: Target, p: [f32; 2], delta: [f32; 2], snap: bool, detection: &mut Detection) {
    let rulers = &mut detection.rulers;
    let timing = &mut detection.timing_led;
    match (target, &mut detection.crop) {
        (Target::TimingMove, _) => {
            timing.center = [timing.center[0] + delta[0], timing.center[1] + delta[1]];
        }
        (Target::TimingRadius, _) => {
            timing.radius = (timing.radius + delta[0]).clamp(*TimingLed::RADII.start(), *TimingLed::RADII.end());
        }
        (Target::CropMove, crop) => {
            if let Some(c) = crop.center() {
                crop.set_center([c[0] + delta[0], c[1] + delta[1]]);
            }
        }
        (Target::CropRadius, Crop::Circle { center, radius }) => {
            *radius = (p[0] - center[0]).hypot(p[1] - center[1]).max(MIN_SIZE);
        }
        (Target::CropResize { sides }, Crop::Square { center, size, angle }) => {
            let (c, half) = resize_box(*center, [*size / 2.0; 2], *angle, sides, p, true);
            (*center, *size) = (c, 2.0 * half[0]);
        }
        (Target::CropResize { sides }, Crop::Rectangle { center, width, height, angle }) => {
            let (c, half) = resize_box(*center, [*width / 2.0, *height / 2.0], *angle, sides, p, false);
            (*center, *width, *height) = (c, 2.0 * half[0], 2.0 * half[1]);
        }
        (Target::CropRotate, Crop::Square { center, angle, .. } | Crop::Rectangle { center, angle, .. }) => {
            // The handle sits "above" the shape, i.e. at -90 degrees in its own frame.
            let mut a = (p[1] - center[1]).atan2(p[0] - center[0]) + FRAC_PI_2;
            if snap {
                a = (a / ROTATE_SNAP).round() * ROTATE_SNAP;
            }
            *angle = (a + PI).rem_euclid(TAU) - PI;
        }
        (Target::RulerEnd { ruler, end }, _) => {
            if let Some(r) = rulers.get_mut(ruler) {
                *if end == 0 { &mut r.a } else { &mut r.b } = p;
            }
        }
        (Target::RulerMove(i), _) => {
            if let Some(r) = rulers.get_mut(i) {
                for q in [&mut r.a, &mut r.b] {
                    q[0] += delta[0];
                    q[1] += delta[1];
                }
            }
        }
        _ => {} // the crop changed shape mid-drag
    }
}

/// Where the handle resizing the timing LED's spot is, to the right of it.
fn timing_handle(xf: Xform, led: &TimingLed) -> [f32; 2] {
    [led.center[0] + led.radius + TIMING_STALK / xf.scale, led.center[1]]
}

fn draw_timing(painter: &Painter, xf: Xform, led: &TimingLed, locked: bool, on: bool) {
    let (center, radius) = (xf.to_screen(led.center), led.radius * xf.scale);
    let stroke = Stroke::new(1.5, TIMING_COLOUR);
    if on {
        painter.circle_filled(center, radius, TIMING_COLOUR.gamma_multiply(0.5));
    }
    painter.circle_stroke(center, radius, stroke);
    let name = center + egui::vec2(0.0, -radius - 4.0);
    label(painter, name, Align2::CENTER_BOTTOM, "Timing LED".into(), TIMING_COLOUR);
    if !locked {
        let handle = xf.to_screen(timing_handle(xf, led));
        painter.line_segment([center + egui::vec2(radius, 0.0), handle], stroke);
        painter.circle(handle, HANDLE_RADIUS - 1.5, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
    }
}

fn draw_crop(painter: &Painter, xf: Xform, crop: &Crop, overlay: Overlay) {
    if !overlay.visible {
        return;
    }
    let stroke = Stroke::new(1.5, CROP_COLOUR);
    let (Some(center), Some([_, hh])) = (crop.center(), crop.half_extents()) else { return };
    let angle = crop.angle();
    draw_outline(painter, xf, crop, stroke, false);
    if overlay.locked {
        return;
    }
    let c = xf.to_screen(center);
    painter.line_segment([c - egui::vec2(6.0, 0.0), c + egui::vec2(6.0, 0.0)], stroke);
    painter.line_segment([c - egui::vec2(0.0, 6.0), c + egui::vec2(0.0, 6.0)], stroke);
    for (target, p) in crop_handles(crop, ROTATE_STALK / xf.scale) {
        let p = xf.to_screen(p);
        if target == Target::CropRotate {
            let top = xf.to_screen(from_local(center, angle, [0.0, -hh]));
            painter.line_segment([top, p], stroke);
            painter.circle(p, HANDLE_RADIUS, CROP_COLOUR, Stroke::new(1.0, Color32::BLACK));
        } else {
            painter.rect(
                Rect::from_center_size(p, egui::vec2(2.0, 2.0) * HANDLE_RADIUS),
                0.0,
                Color32::WHITE,
                Stroke::new(1.0, Color32::BLACK),
                egui::StrokeKind::Middle,
            );
        }
    }
}

/// The crop's boundary, solid or dashed.
fn draw_outline(painter: &Painter, xf: Xform, crop: &Crop, stroke: Stroke, dashed: bool) {
    let (Some(center), Some([hw, hh])) = (crop.center(), crop.half_extents()) else { return };
    let mut points: Vec<Pos2> = match crop {
        Crop::Circle { .. } => (0..=96)
            .map(|i| {
                let a = TAU * i as f32 / 96.0;
                xf.to_screen([center[0] + hw * a.cos(), center[1] + hw * a.sin()])
            })
            .collect(),
        _ => [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh], [-hw, -hh]]
            .iter()
            .map(|&l| xf.to_screen(from_local(center, crop.angle(), l)))
            .collect(),
    };
    if dashed {
        painter.extend(Shape::dashed_line(&points, stroke, 6.0, 4.0));
    } else {
        points.pop(); // closed_line joins the ends itself
        painter.add(Shape::closed_line(points, stroke));
    }
}

fn draw_ruler(painter: &Painter, xf: Xform, index: usize, ruler: &Ruler, locked: bool) {
    let (a, b) = (xf.to_screen(ruler.a), xf.to_screen(ruler.b));
    let stroke = Stroke::new(2.0, RULER_COLOUR);
    painter.line_segment([a, b], stroke);
    let dir = (b - a).normalized();
    let normal = if dir.is_finite() { egui::vec2(-dir.y, dir.x) } else { egui::vec2(0.0, -1.0) };
    for p in [a, b] {
        painter.line_segment([p - normal * 7.0, p + normal * 7.0], stroke);
        if !locked {
            painter.circle(p, HANDLE_RADIUS - 1.5, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
        }
    }
    let mut text = format!("R{}  {:.1} px", index + 1, ruler.pixels());
    if ruler.length_cm > 0.0 {
        text += &format!(" = {} cm", ruler.length_cm);
    }
    // Put the label on whichever side of the line is "up" on screen.
    let up = if normal.y > 0.0 { -normal } else { normal };
    label(painter, a + (b - a) / 2.0 + up * 14.0, Align2::CENTER_CENTER, text, RULER_COLOUR);
}

/// Text on a translucent dark background, so it's readable over any part of the frame.
fn label(painter: &Painter, pos: Pos2, anchor: Align2, text: String, colour: Color32) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), colour);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(rect.expand(3.0), 3.0, Color32::from_black_alpha(170));
    painter.galley(rect.min, galley, colour);
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_corners(crop: &Crop) -> Vec<[f32; 2]> {
        let (c, [hw, hh], a) = (crop.center().unwrap(), crop.half_extents().unwrap(), crop.angle());
        [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].iter().map(|&l| from_local(c, a, l)).collect()
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    fn with_crop(crop: Crop) -> Detection {
        Detection { crop, ..Default::default() }
    }

    #[test]
    fn dragging_a_rotated_rectangle_edge_keeps_the_opposite_edge() {
        let mut detection =
            with_crop(Crop::Rectangle { center: [100.0, 100.0], width: 60.0, height: 40.0, angle: 0.5 });
        let before = rect_corners(&detection.crop);
        // Drag the +x side outwards by 20px along the rectangle's own x axis.
        let target = from_local([100.0, 100.0], 0.5, [50.0, 0.0]);
        apply_drag(Target::CropResize { sides: [1, 0] }, target, [0.0, 0.0], false, &mut detection);
        let Crop::Rectangle { width, height, .. } = detection.crop else { unreachable!() };
        assert!((width - 80.0).abs() < 1e-3 && (height - 40.0).abs() < 1e-3);
        let after = rect_corners(&detection.crop);
        for i in [0, 3] {
            assert!(close(after[i], before[i])); // the -x side's corners
        }
    }

    #[test]
    fn dragging_a_rotated_rectangle_corner_keeps_the_opposite_corner() {
        let mut detection =
            with_crop(Crop::Rectangle { center: [100.0, 100.0], width: 60.0, height: 40.0, angle: 0.5 });
        let before = rect_corners(&detection.crop);
        // Drag the (+x, +y) corner out by 10 and 20 px in the rectangle's own frame.
        let target = from_local([100.0, 100.0], 0.5, [40.0, 40.0]);
        apply_drag(Target::CropResize { sides: [1, 1] }, target, [0.0, 0.0], false, &mut detection);
        let Crop::Rectangle { width, height, .. } = detection.crop else { unreachable!() };
        assert!((width - 70.0).abs() < 1e-3 && (height - 60.0).abs() < 1e-3);
        assert!(close(rect_corners(&detection.crop)[0], before[0])); // the (-x, -y) corner
    }

    #[test]
    fn square_edge_keeps_opposite_edge_and_stays_centred_across() {
        let mut detection = with_crop(Crop::Square { center: [0.0, 0.0], size: 10.0, angle: 0.0 });
        apply_drag(Target::CropResize { sides: [0, -1] }, [3.0, -15.0], [0.0, 0.0], false, &mut detection);
        // Top edge moved from y=-5 to y=-15 with the bottom edge (y=5) fixed: size 20, centre (0, -5).
        assert_eq!(detection.crop, Crop::Square { center: [0.0, -5.0], size: 20.0, angle: 0.0 });
    }

    #[test]
    fn the_timing_led_is_dragged_and_resized_ahead_of_the_crop_around_it() {
        let crop = Crop::Square { center: [100.0, 100.0], size: 100.0, angle: 0.0 };
        let timing_led = TimingLed { enabled: true, center: [100.0, 100.0], radius: 10.0, ..Default::default() };
        let mut detection = Detection { crop, timing_led, ..Default::default() };
        let (xf, mut overlays) = (Xform { origin: Pos2::ZERO, scale: 1.0 }, Overlays::default());
        let at = |x: f32| Pos2::new(x, 100.0);
        assert_eq!(hit(at(100.0), xf, &detection, &overlays), Some(Target::TimingMove));
        assert_eq!(hit(at(122.0), xf, &detection, &overlays), Some(Target::TimingRadius));
        assert_eq!(hit(at(135.0), xf, &detection, &overlays), Some(Target::CropMove), "beyond it");

        apply_drag(Target::TimingMove, [0.0; 2], [5.0, -3.0], false, &mut detection);
        assert_eq!(detection.timing_led.center, [105.0, 97.0]);
        apply_drag(Target::TimingRadius, [0.0; 2], [1000.0, 0.0], false, &mut detection);
        assert_eq!(detection.timing_led.radius, *TimingLed::RADII.end(), "only so big");

        for hidden_or_locked in [Overlay { visible: false, locked: false }, Overlay { visible: true, locked: true }] {
            overlays.timing = hidden_or_locked;
            assert_eq!(hit(at(105.0), xf, &detection, &overlays), Some(Target::CropMove));
        }
    }

    #[test]
    fn rotate_snaps_with_shift() {
        let mut detection = with_crop(Crop::Square { center: [0.0, 0.0], size: 10.0, angle: 0.0 });
        // Pointer slightly clockwise of straight right, i.e. ~90 degrees from the handle's rest position.
        apply_drag(Target::CropRotate, [10.0, 0.4], [0.0, 0.0], true, &mut detection);
        assert!((detection.crop.angle() - FRAC_PI_2).abs() < 1e-5);
    }
}
