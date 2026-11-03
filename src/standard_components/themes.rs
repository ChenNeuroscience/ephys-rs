//! The themes to choose from.

use eframe::egui::Color32;

use crate::core::theme::{Colours, Theme};

pub const THEMES: &[Theme] = &[DARK, LIGHT];

pub const DARK: Theme = Theme { name: "Dark", colours: DARK_COLOURS };
pub const LIGHT: Theme = Theme { name: "Light", colours: LIGHT_COLOURS };

const DARK_COLOURS: Colours = Colours {
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

const LIGHT_COLOURS: Colours = Colours {
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
