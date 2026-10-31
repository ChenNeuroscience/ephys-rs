//! Colours that are the same in every theme. The rest come from the theme (see `crate::theme`).

use eframe::egui::Color32;

use crate::engine::Category;

/// Node header colour.
pub fn category_colour(category: Category) -> Color32 {
    match category {
        Category::Source => Color32::from_rgb(40, 96, 78),
        Category::Processing => Color32::from_rgb(48, 72, 120),
        Category::Visualisation => Color32::from_rgb(98, 66, 112),
        Category::Recording => Color32::from_rgb(124, 52, 52),
    }
}
