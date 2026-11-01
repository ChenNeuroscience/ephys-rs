//! Stand-in data for the viewers, until real data flows through the graph.

use eframe::egui::{self, Align2, FontId, Rect, Sense};

use crate::core::theme::Colours;

/// Repeatable noise in -1..1 for any pair of integers.
pub fn noise(a: u64, b: i64) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// A spike waveform over `t` in 0..1: a sharp trough then a slower rebound.
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

/// The plot area with a message in place of data.
pub fn unconnected(ui: &mut egui::Ui, theme: &Colours, message: &str) {
    let rect = plot_area(ui, theme);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, message, FontId::proportional(14.0), theme.text_secondary);
}
