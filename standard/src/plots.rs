//! Plot areas for the viewers, and (for tests) made-up signals.

use eframe::egui::{self, Align2, FontId, Rect, Sense, Stroke};

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

/// Fills the space left in `ui` with a plot area, returning its rect.
pub fn plot_area(ui: &mut egui::Ui, theme: &Colours) -> Rect {
    let size = ui.available_size().max(egui::vec2(120.0, 80.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 2.0, theme.surface_sunken);
    rect
}

/// Asks for the view to be drawn again when there's likely new data: after a pass (not every frame the display
/// could show, which on a 120 Hz display is more than twice as often). There's none unless `running`.
pub fn repaint_with_data(ui: &egui::Ui, running: bool) {
    if running {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(ephys_rs::engine::PASS_DURATION));
    }
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

/// A plot's y axis (the LFP Viewer's rows', the Spike Viewer's), at the left of `plot`, and a faint line along it at zero. The row's `height` high, from `top`.
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
