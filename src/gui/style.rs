//! Colours, and the themes the user can choose between.

use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

use crate::engine::Category;

pub const AMBER: Color32 = Color32::from_rgb(240, 180, 60);

/// Node header colour, the same in every theme.
pub fn category_colour(category: Category) -> Color32 {
    match category {
        Category::Source => Color32::from_rgb(40, 96, 78),
        Category::Processing => Color32::from_rgb(48, 72, 120),
        Category::Visualisation => Color32::from_rgb(98, 66, 112),
        Category::Recording => Color32::from_rgb(124, 52, 52),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub const ALL: [Theme; 2] = [Theme::Dark, Theme::Light];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
        }
    }

    /// The egui theme this is based on.
    fn base(self) -> egui::Theme {
        match self {
            Theme::Dark => egui::Theme::Dark,
            Theme::Light => egui::Theme::Light,
        }
    }

    /// Makes this the theme for all of egui (panels, widgets, windows).
    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_visuals_of(self.base(), self.base().default_visuals());
        ctx.set_theme(self.base());
    }

    /// Colours for the graph canvas.
    pub fn palette(self) -> &'static Palette {
        match self {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }
}

/// The graph canvas's colours. (Edge colours come from the edge types, and node headers from their category.)
pub struct Palette {
    pub canvas: Color32,
    pub grid: Color32,
    pub node: Color32,
    pub node_border: Color32,
    /// Between a node's data and settings ports.
    pub divider: Color32,
    pub text: Color32,
    /// Unconnected inputs, and signal summaries.
    pub text_weak: Color32,
    /// Outline of selected nodes and notes.
    pub selection: Color32,
    pub shadow: Color32,
    /// Selection rectangle fill, and the glow round a selected edge.
    pub highlight: Color32,
    pub marquee: Color32,
}

const DARK: Palette = Palette {
    canvas: Color32::from_gray(22),
    grid: Color32::from_gray(30),
    node: Color32::from_gray(40),
    node_border: Color32::from_gray(64),
    divider: Color32::from_gray(58),
    text: Color32::from_gray(215),
    text_weak: Color32::from_gray(128),
    selection: Color32::from_gray(235),
    shadow: Color32::from_black_alpha(90),
    highlight: Color32::from_rgba_premultiplied(28, 28, 28, 28),
    marquee: Color32::from_gray(160),
};

const LIGHT: Palette = Palette {
    canvas: Color32::from_gray(236),
    grid: Color32::from_gray(222),
    node: Color32::from_gray(252),
    node_border: Color32::from_gray(190),
    divider: Color32::from_gray(218),
    text: Color32::from_gray(40),
    text_weak: Color32::from_gray(125),
    selection: Color32::from_rgb(40, 110, 220),
    shadow: Color32::from_black_alpha(40),
    highlight: Color32::from_black_alpha(18),
    marquee: Color32::from_gray(110),
};
