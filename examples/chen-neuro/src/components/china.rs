//! A theme for fun: the red and gold of the flag. Panels are lacquer red, what's raised on them a brighter red, and
//! gold what stands out. Views of data stay nearly black, so traces and spikes can be read, with jade for success.

use eframe::egui::Color32;
use ephys_rs::core::theme::{Colours, Theme};

pub const CHINA: Theme = Theme {
    name: "China",
    colours: Colours {
        dark: true,
        background: Color32::from_rgb(92, 12, 16),
        surface_sunken: Color32::from_rgb(36, 8, 9),
        surface: Color32::from_rgb(142, 20, 24),
        surface_raised: Color32::from_rgb(172, 30, 30),
        grid: Color32::from_rgb(132, 52, 20),
        border: Color32::from_rgb(255, 208, 40),
        divider: Color32::from_rgb(184, 112, 36),
        text_primary: Color32::from_rgb(255, 242, 214),
        text_secondary: Color32::from_rgb(255, 216, 80),
        text_disabled: Color32::from_rgb(206, 136, 70),
        control: Color32::from_rgb(112, 14, 20),
        control_hovered: Color32::from_rgb(196, 96, 24),
        control_pressed: Color32::from_rgb(84, 10, 14),
        accent: Color32::from_rgb(255, 208, 40),
        on_accent: Color32::from_rgb(92, 12, 16),
        warning: Color32::from_rgb(255, 160, 50),
        error: Color32::from_rgb(255, 150, 130),
        success: Color32::from_rgb(110, 230, 170),
        shadow: Color32::from_black_alpha(110),
    },
};
