//! Plot areas for the viewers, lines through dense data, and (for tests) made-up signals.

use eframe::egui::{self, Align2, FontId, Rangef, Rect, Sense, Shape, Stroke};

use ephys_rs::theme::Colours;

/// Repeatable noise in -1..1 for any pair of integers.
#[cfg(test)]
pub fn noise(a: u64, b: i64) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// A spike waveform over `t` in 0..1: a sharp trough then a slower rebound.
#[cfg(test)]
pub fn spike_shape(t: f32) -> f32 {
    let g = |centre: f32, width: f32| (-((t - centre) / width).powi(2)).exp();
    -g(0.3, 0.05) + 0.35 * g(0.48, 0.1)
}

/// A line through dense data, as `Shape::line` draws one through points, but for data with many values to a pixel (a
/// long trace in a narrow plot): for each x across, in order, the range of what's there (screen y, either way round),
/// or NaN where there's nothing (a gap: the line stops, and starts again after). Drawn a physical pixel at a time
/// (`pixels_per_point`): each the whole range of what falls in it, joined to the next by sloping edges, as wide as
/// `stroke`, its edges fading over a pixel either side of where they'd be, as egui draws a line's, so it looks the
/// same. Built straight into a mesh, with no line to tessellate, which is what makes `Shape::line` slow for so many
/// points: four vertices and six triangles a pixel, however much data.
pub fn dense_line(ranges: impl IntoIterator<Item = (f32, Rangef)>, stroke: Stroke, pixels_per_point: f32) -> Shape {
    let pixel = 1.0 / pixels_per_point;
    let mut mesh = egui::Mesh::default();
    // Solid to half the width less half a pixel, faded out by half the width and half a pixel more.
    let (solid, faded) = ((stroke.width - pixel).max(0.0) * 0.5, (stroke.width + pixel) * 0.5);
    // A pixel's edge: faded out, the top, the bottom, faded out; joined to the one before, if it's to be.
    let edge = |at: f32, (top, bottom): (f32, f32), join: bool, mesh: &mut egui::Mesh| {
        let before = mesh.vertices.len() as u32;
        let (colour, fade) = (stroke.color, egui::Color32::TRANSPARENT);
        for (y, colour) in
            [(top - faded, fade), (top - solid, colour), (bottom + solid, colour), (bottom + faded, fade)]
        {
            mesh.colored_vertex(egui::pos2(at, y), colour);
        }
        if join && let Some(last) = before.checked_sub(4) {
            for k in last..last + 3 {
                mesh.add_triangle(k, k + 1, k + 4);
                mesh.add_triangle(k + 1, k + 5, k + 4);
            }
        }
    };
    // The stretch being drawn (between gaps): how many edges it has, and the last.
    let mut stretch: Option<(usize, f32, (f32, f32))> = None;
    let put = |at: f32, range: (f32, f32), stretch: &mut Option<(usize, f32, (f32, f32))>, mesh: &mut egui::Mesh| {
        edge(at, range, stretch.is_some(), mesh);
        *stretch = Some((stretch.map_or(0, |(edges, ..)| edges) + 1, at, range));
    };
    // A stretch of one is drawn a pixel wide.
    let end = |stretch: &mut Option<(usize, f32, (f32, f32))>, mesh: &mut egui::Mesh| {
        if let Some((1, at, range)) = stretch.take() {
            edge(at + pixel, range, true, mesh);
        }
    };
    // The pixel being gathered: its middle, and the top and bottom of what's in it (screen y: the top's the smaller).
    let mut gathering: Option<(f32, (f32, f32))> = None;
    for (x, range) in ranges {
        if range.min.is_nan() || range.max.is_nan() {
            if let Some((middle, range)) = gathering.take() {
                put(middle, range, &mut stretch, &mut mesh);
            }
            end(&mut stretch, &mut mesh);
            continue;
        }
        let at = ((x / pixel).floor() + 0.5) * pixel;
        let (top, bottom) = (range.min.min(range.max), range.min.max(range.max));
        match &mut gathering {
            Some((middle, (t, b))) if *middle == at => (*t, *b) = (t.min(top), b.max(bottom)),
            _ => {
                if let Some((middle, range)) = gathering.take() {
                    put(middle, range, &mut stretch, &mut mesh);
                }
                gathering = Some((at, (top, bottom)));
            }
        }
    }
    if let Some((middle, range)) = gathering {
        put(middle, range, &mut stretch, &mut mesh);
    }
    end(&mut stretch, &mut mesh);
    Shape::mesh(mesh)
}

/// Fills the space left in `ui` with a plot area, returning its rect.
pub fn plot_area(ui: &mut egui::Ui, theme: &Colours) -> Rect {
    let size = ui.available_size().max(egui::vec2(120.0, 80.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 2.0, theme.surface_sunken);
    rect
}

/// The plot area with a message in place of data.
pub fn unconnected(ui: &mut egui::Ui, theme: &Colours, message: &str) {
    let rect = plot_area(ui, theme);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, message, FontId::proportional(14.0), theme.text_secondary);
}

/// The ticks of a plot's y axis, top to bottom, for a row `height` high showing ±`scale_uv`: where each is (from 1, at
/// the top, to -1), and its label. As many as there's room for.
pub fn axis_ticks(height: f32, scale_uv: f32) -> Vec<(f32, String)> {
    let value = |at: f32| {
        // Plus zero, so zero isn't "−0" (as it is, times a negative scale).
        let uv = (at * scale_uv * 10.0).round() / 10.0 + 0.0;
        if uv < 0.0 { format!("−{}", -uv) } else { uv.to_string() }
    };
    let top = format!("{} µV", value(1.0));
    match height {
        ..20.0 => vec![(0.0, format!("±{}", value(1.0)))],
        ..30.0 => vec![(1.0, top), (-1.0, value(-1.0))],
        ..100.0 => vec![(1.0, top), (0.0, value(0.0)), (-1.0, value(-1.0))],
        _ => {
            let middle = [0.5, 0.0, -0.5].map(|at| (at, value(at)));
            std::iter::once((1.0, top)).chain(middle).chain([(-1.0, value(-1.0))]).collect()
        }
    }
}

/// Draws a plot's y axis at the left of `plot`, with a faint line across the plot at zero. The axis is `height` high,
/// starting at `top`. The Continuous Data Viewer's rows and the Spike Viewer's plots use this.
pub fn axis_ui(
    painter: &egui::Painter,
    plot: egui::Rangef,
    top: f32,
    height: f32,
    scale_uv: f32,
    theme: &ephys_rs::theme::Colours,
) {
    let stroke = Stroke::new(1.0, theme.divider);
    let centre = top + height * 0.5;
    painter.hline(plot, centre, Stroke::new(1.0, theme.divider.gamma_multiply(0.4)));
    painter.vline(plot.min, top + 1.0..=top + height - 1.0, stroke);
    for (at, text) in axis_ticks(height, scale_uv) {
        // The ends' labels stay within the row.
        let (y, anchor) = match at {
            1.0 => (top + 1.0, Align2::RIGHT_TOP),
            -1.0 => (top + height - 1.0, Align2::RIGHT_BOTTOM),
            _ => (centre - at * height * 0.5, Align2::RIGHT_CENTER),
        };
        painter.hline(plot.min - 3.0..=plot.min, y, stroke);
        let position = egui::pos2(plot.min - 5.0, y);
        painter.text(position, anchor, text, FontId::monospace(9.0), theme.text_secondary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dense line is the whole range of what's at each pixel, joined pixel to pixel, as wide as its stroke, its
    /// edges fading over a pixel.
    #[test]
    fn dense_lines_are_each_pixels_range_joined_up_with_smooth_edges() {
        // Four values a pixel, drawn upwards (screen y is down).
        let lows = [0.0, -1.0, 0.0, 0.0, 10.0, 10.0, 10.0, 10.0];
        let highs = [1.0, 1.0, 2.0, 1.0, 10.0, 11.0, 10.0, 10.0];
        let ranges = (0..8).map(|i| (i as f32 * 0.25, Rangef::new(-highs[i], -lows[i])));
        let Shape::Mesh(mesh) = dense_line(ranges, Stroke::new(1.0, egui::Color32::WHITE), 1.0) else {
            panic!("a mesh")
        };
        let edges: Vec<(f32, Vec<f32>, Vec<u8>)> = mesh
            .vertices
            .chunks(4)
            .map(|edge| {
                (edge[0].pos.x, edge.iter().map(|v| v.pos.y).collect(), edge.iter().map(|v| v.color.a()).collect())
            })
            .collect();
        assert_eq!(
            edges,
            [
                (0.5, vec![-3.0, -2.0, 1.0, 2.0], vec![0, 255, 255, 0]),
                (1.5, vec![-12.0, -11.0, -10.0, -9.0], vec![0, 255, 255, 0]),
            ],
            "-1 to 2, then 10 to 11: a line a pixel wide is solid to its range, fading over a pixel beyond"
        );
        assert_eq!(mesh.indices.len(), 3 * 6, "joined by three quads: the fades and the line");
        let one = dense_line([(0.0, Rangef::point(-5.0))], Stroke::new(1.0, egui::Color32::WHITE), 1.0);
        let Shape::Mesh(one) = one else { panic!("a mesh") };
        assert_eq!(one.vertices.len(), 8, "a single value's drawn a pixel wide");
    }

    /// Where there's nothing (NaN), the line stops, and starts again after, not joined across.
    #[test]
    fn dense_lines_stop_at_gaps() {
        let value = |v: f32| Rangef::point(v);
        let ranges = [
            (0.0, value(1.0)),
            (1.0, value(2.0)),
            (2.0, Rangef::point(f32::NAN)),
            (3.0, value(3.0)),
            (4.0, value(4.0)),
        ];
        let Shape::Mesh(mesh) = dense_line(ranges, Stroke::new(1.0, egui::Color32::WHITE), 1.0) else {
            panic!("a mesh")
        };
        assert_eq!(mesh.vertices.len(), 4 * 4, "two edges either side of the gap");
        assert_eq!(mesh.indices.len(), 2 * 3 * 6, "each pair joined, but not across the gap");
        let ranges = [(0.0, value(1.0)), (1.0, Rangef::point(f32::NAN)), (2.0, value(3.0)), (3.0, value(4.0))];
        let Shape::Mesh(mesh) = dense_line(ranges, Stroke::new(1.0, egui::Color32::WHITE), 1.0) else {
            panic!("a mesh")
        };
        assert_eq!(mesh.vertices.len(), 4 * 4, "the lone value before the gap drawn a pixel wide");
    }
}
