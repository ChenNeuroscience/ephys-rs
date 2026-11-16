//! Most colours come from the theme (see `crate::theme`); these are the same in every theme.

use eframe::egui::Color32;

use crate::engine::Category;

/// The graph colours each node's header by its category, as Open Ephys colours its kinds of processor (sources orange,
/// filters sky blue, sinks green, recording red), toned down to sit behind white text in either theme.
pub fn category_colour(category: Category) -> Color32 {
    match category {
        Category::Source => Color32::from_rgb(168, 82, 40),
        Category::Processing => Color32::from_rgb(30, 104, 150),
        Category::Visualisation => Color32::from_rgb(34, 116, 72),
        Category::Recording => Color32::from_rgb(158, 44, 44),
        Category::Other => Color32::from_rgb(112, 84, 150),
    }
}
