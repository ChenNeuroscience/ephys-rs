//! The themes to choose from: dark and light, and three for fun.

use eframe::egui::Color32;

use ephys_rs::theme::{Colours, Theme};

/// Every standard theme. The first is the default.
pub const THEMES: &[Theme] = &[DARK, LIGHT, RED_ENVELOPE, SPANGLED, SONG_CONTEST];

pub const DARK: Theme = Theme { name: "Dark", colours: DARK_COLOURS };
pub const LIGHT: Theme = Theme { name: "Light", colours: LIGHT_COLOURS };
/// The Red Envelope theme is the red and gold of a hongbao.
pub const RED_ENVELOPE: Theme = Theme { name: "Red Envelope", colours: RED_ENVELOPE_COLOURS };
/// The Spangled theme is the red, white and blue of the stars and stripes.
pub const SPANGLED: Theme = Theme { name: "Spangled", colours: SPANGLED_COLOURS };
/// The Song Contest theme is the blue and gold of the European flag.
pub const SONG_CONTEST: Theme = Theme { name: "Song Contest", colours: SONG_CONTEST_COLOURS };

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
    chrome: None,
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
    chrome: None,
};

/// Red Envelope's panels are lacquer red, what's raised on them a brighter red, and gold what stands out. Views of data stay
/// nearly black, so traces and spikes can be read, with jade for success.
const RED_ENVELOPE_COLOURS: Colours = Colours {
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
    chrome: None,
};

/// Spangled's panes are white and pale blue, edged and written in the flag's Old Glory Blue, and its chrome (the
/// menu bar, toolbar and status bar) is Old Glory Red, with white text.
const SPANGLED_COLOURS: Colours = Colours {
    dark: false,
    background: Color32::from_rgb(208, 220, 238),
    surface_sunken: Color32::WHITE,
    surface: Color32::from_rgb(246, 248, 252),
    surface_raised: Color32::WHITE,
    grid: Color32::from_rgb(186, 202, 228),
    border: Color32::from_rgb(10, 49, 97),
    divider: Color32::from_rgb(204, 214, 232),
    text_primary: Color32::from_rgb(10, 49, 97),
    text_secondary: Color32::from_rgb(84, 106, 142),
    text_disabled: Color32::from_rgb(156, 170, 196),
    control: Color32::from_rgb(226, 233, 245),
    control_hovered: Color32::from_rgb(206, 219, 240),
    control_pressed: Color32::from_rgb(186, 204, 232),
    accent: Color32::from_rgb(10, 49, 97),
    on_accent: Color32::WHITE,
    warning: Color32::from_rgb(196, 128, 0),
    error: Color32::from_rgb(179, 25, 66),
    success: Color32::from_rgb(36, 148, 70),
    shadow: Color32::from_black_alpha(40),
    chrome: Some(&SPANGLED_CHROME),
};

/// Spangled's chrome is Old Glory Red, with white text, and white for what stands out on it.
const SPANGLED_CHROME: Colours = Colours {
    dark: true,
    background: Color32::from_rgb(179, 25, 66),
    surface_sunken: Color32::from_rgb(132, 14, 46),
    surface: Color32::from_rgb(179, 25, 66),
    surface_raised: Color32::from_rgb(200, 52, 88),
    grid: Color32::from_rgb(200, 52, 88),
    border: Color32::from_rgb(250, 236, 240),
    divider: Color32::from_rgb(214, 112, 138),
    text_primary: Color32::WHITE,
    text_secondary: Color32::from_rgb(255, 212, 222),
    text_disabled: Color32::from_rgb(226, 140, 162),
    control: Color32::from_rgb(150, 18, 52),
    control_hovered: Color32::from_rgb(204, 62, 98),
    control_pressed: Color32::from_rgb(120, 12, 40),
    accent: Color32::WHITE,
    on_accent: Color32::from_rgb(179, 25, 66),
    warning: Color32::from_rgb(255, 206, 90),
    error: Color32::WHITE,
    success: Color32::from_rgb(160, 240, 180),
    shadow: Color32::from_black_alpha(90),
    chrome: None,
};

/// Song Contest's panels are the European flag's blue, edged in its stars' gold, with gold for what stands out. Views
/// of data stay nearly black, so traces and spikes can be read.
const SONG_CONTEST_COLOURS: Colours = Colours {
    dark: true,
    background: Color32::from_rgb(0, 40, 120),
    surface_sunken: Color32::from_rgb(4, 12, 40),
    surface: Color32::from_rgb(0, 51, 153),
    surface_raised: Color32::from_rgb(20, 72, 180),
    grid: Color32::from_rgb(20, 60, 140),
    border: Color32::from_rgb(255, 204, 0),
    divider: Color32::from_rgb(200, 160, 40),
    text_primary: Color32::WHITE,
    text_secondary: Color32::from_rgb(255, 214, 60),
    text_disabled: Color32::from_rgb(130, 150, 200),
    control: Color32::from_rgb(0, 36, 110),
    control_hovered: Color32::from_rgb(30, 90, 210),
    control_pressed: Color32::from_rgb(0, 28, 90),
    accent: Color32::from_rgb(255, 204, 0),
    on_accent: Color32::from_rgb(0, 40, 120),
    warning: Color32::from_rgb(255, 150, 60),
    error: Color32::from_rgb(255, 130, 150),
    success: Color32::from_rgb(110, 230, 170),
    shadow: Color32::from_black_alpha(110),
    chrome: None,
};
