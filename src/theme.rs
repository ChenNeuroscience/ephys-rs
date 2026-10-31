//! Themes: a set of colours named by what they're for (surfaces, text, controls...), which everything draws
//! with. egui's own widgets get them as its `Visuals`; the graph and node windows use them directly (nodes
//! get them in `engine::Context`), and can derive further colours of their own from them.

use eframe::egui::{self, Color32, Shadow, Stroke};
use serde::{Deserialize, Serialize};

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

    pub fn colours(self) -> &'static Colours {
        match self {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }

    /// Makes this the theme for all of egui (panels, widgets, menus, windows).
    pub fn apply(self, ctx: &egui::Context) {
        let colours = self.colours();
        let base = if colours.dark { egui::Theme::Dark } else { egui::Theme::Light };
        ctx.set_visuals_of(base, colours.visuals());
        ctx.set_theme(base);
    }
}

/// A theme's colours, by role.
pub struct Colours {
    /// Whether this is a dark theme (light text on dark surfaces).
    pub dark: bool,

    // Surfaces, from the furthest back to the closest.
    /// Behind everything: the graph canvas.
    pub background: Color32,
    /// Set into the surface: text fields, plots and other views of data, the tab bar.
    pub surface_sunken: Color32,
    /// Panels, windows and menus.
    pub surface: Color32,
    /// On top of a surface: nodes, cards.
    pub surface_raised: Color32,
    /// Lines over the background, e.g. the graph's grid.
    pub grid: Color32,

    pub border: Color32,
    /// Lines within a surface, between its parts.
    pub divider: Color32,

    pub text_primary: Color32,
    /// Less important text: hints, details, labels on data.
    pub text_secondary: Color32,
    pub text_disabled: Color32,

    // Buttons and other things to click or drag.
    pub control: Color32,
    pub control_hovered: Color32,
    pub control_pressed: Color32,

    /// Selection, focus, and anything that should stand out.
    pub accent: Color32,
    /// Text and marks on top of the accent colour.
    pub on_accent: Color32,

    pub warning: Color32,
    pub error: Color32,
    pub success: Color32,
    pub shadow: Color32,
}

impl Colours {
    /// egui's visuals in these colours.
    pub fn visuals(&self) -> egui::Visuals {
        let mut visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        visuals.panel_fill = self.surface;
        visuals.window_fill = self.surface;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        visuals.faint_bg_color = self.surface_raised;
        visuals.extreme_bg_color = self.surface_sunken;
        visuals.code_bg_color = self.surface_sunken;
        visuals.hyperlink_color = self.accent;
        visuals.warn_fg_color = self.warning;
        visuals.error_fg_color = self.error;
        visuals.weak_text_color = Some(self.text_secondary);
        visuals.selection.bg_fill = self.accent;
        visuals.selection.stroke = Stroke::new(1.0, self.on_accent);
        visuals.window_shadow = Shadow { color: self.shadow, ..visuals.window_shadow };
        visuals.popup_shadow = Shadow { color: self.shadow, ..visuals.popup_shadow };

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.surface;
        widgets.noninteractive.weak_bg_fill = self.surface;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.divider);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text_primary);
        for (widget, fill) in [
            (&mut widgets.inactive, self.control),
            (&mut widgets.hovered, self.control_hovered),
            (&mut widgets.active, self.control_pressed),
            (&mut widgets.open, self.control_pressed),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.fg_stroke = Stroke::new(widget.fg_stroke.width, self.text_primary);
        }
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.hovered.bg_stroke = Stroke::new(1.0, self.border);
        widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        visuals
    }
}

const DARK: Colours = Colours {
    dark: true,
    background: Color32::from_gray(22),
    surface_sunken: Color32::from_gray(12),
    surface: Color32::from_gray(28),
    surface_raised: Color32::from_gray(40),
    grid: Color32::from_gray(30),
    border: Color32::from_gray(64),
    divider: Color32::from_gray(52),
    text_primary: Color32::from_gray(212),
    text_secondary: Color32::from_gray(135),
    text_disabled: Color32::from_gray(85),
    control: Color32::from_gray(55),
    control_hovered: Color32::from_gray(70),
    control_pressed: Color32::from_gray(48),
    accent: Color32::from_rgb(56, 124, 210),
    on_accent: Color32::WHITE,
    warning: Color32::from_rgb(240, 180, 60),
    error: Color32::from_rgb(232, 86, 72),
    success: Color32::from_rgb(90, 200, 120),
    shadow: Color32::from_black_alpha(90),
};

const LIGHT: Colours = Colours {
    dark: false,
    background: Color32::from_gray(236),
    surface_sunken: Color32::from_gray(255),
    surface: Color32::from_gray(246),
    surface_raised: Color32::from_gray(252),
    grid: Color32::from_gray(222),
    border: Color32::from_gray(190),
    divider: Color32::from_gray(218),
    text_primary: Color32::from_gray(32),
    text_secondary: Color32::from_gray(110),
    text_disabled: Color32::from_gray(165),
    control: Color32::from_gray(228),
    control_hovered: Color32::from_gray(216),
    control_pressed: Color32::from_gray(202),
    accent: Color32::from_rgb(40, 110, 220),
    on_accent: Color32::WHITE,
    warning: Color32::from_rgb(196, 128, 0),
    error: Color32::from_rgb(200, 52, 40),
    success: Color32::from_rgb(36, 148, 70),
    shadow: Color32::from_black_alpha(40),
};
