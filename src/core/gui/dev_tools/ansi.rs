//! Text styled with ANSI escape codes, as a terminal shows it: `cansi` parses them, and this maps its 16 colours,
//! bold, faint, italic, underline and reverse onto egui's text formats, in colours toned for the theme.

use cansi::v3::{Color, Intensity, categorise_text};
use eframe::egui::{Color32, FontId, Stroke, TextFormat, text::LayoutJob};

/// The 16 terminal colours (black, red, green, yellow, blue, magenta, cyan, white, then their bright forms), toned
/// to read on a dark background or a light one.
fn palette(dark: bool) -> [Color32; 16] {
    let rgb = Color32::from_rgb;
    if dark {
        [
            rgb(90, 94, 102),
            rgb(232, 106, 112),
            rgb(152, 195, 121),
            rgb(229, 192, 123),
            rgb(97, 175, 239),
            rgb(198, 120, 221),
            rgb(86, 182, 194),
            rgb(200, 204, 212),
            rgb(128, 132, 140),
            rgb(255, 135, 140),
            rgb(180, 225, 150),
            rgb(245, 215, 150),
            rgb(140, 200, 255),
            rgb(220, 160, 240),
            rgb(120, 215, 225),
            rgb(240, 242, 246),
        ]
    } else {
        [
            rgb(30, 30, 30),
            rgb(190, 34, 42),
            rgb(46, 128, 40),
            rgb(146, 100, 0),
            rgb(34, 90, 196),
            rgb(150, 50, 170),
            rgb(10, 120, 132),
            rgb(110, 110, 110),
            rgb(90, 90, 90),
            rgb(220, 50, 60),
            rgb(60, 150, 50),
            rgb(170, 120, 0),
            rgb(50, 110, 220),
            rgb(175, 70, 195),
            rgb(20, 145, 160),
            rgb(150, 150, 150),
        ]
    }
}

/// Adds `text` to `job`, styled by its escape codes. `colour` is the text's colour where it doesn't set one.
pub fn append(job: &mut LayoutJob, text: &str, font: &FontId, colour: Color32, dark: bool) {
    let palette = palette(dark);
    let lookup = |c: Color| palette[c as usize];
    for slice in categorise_text(text) {
        let mut foreground = slice.fg.map_or(colour, lookup);
        let mut background = slice.bg.map_or(Color32::TRANSPARENT, lookup);
        match slice.intensity {
            Some(Intensity::Bold) if dark => foreground = foreground.lerp_to_gamma(Color32::WHITE, 0.2),
            Some(Intensity::Faint) => foreground = foreground.gamma_multiply(0.6),
            _ => {}
        }
        if slice.reversed == Some(true) {
            let behind = slice.bg.map_or(if dark { Color32::BLACK } else { Color32::WHITE }, lookup);
            (foreground, background) = (behind, foreground);
        }
        let underline = slice.underline == Some(true);
        let format = TextFormat {
            font_id: font.clone(),
            color: foreground,
            background,
            italics: slice.italic == Some(true),
            underline: if underline { Stroke::new(1.0, foreground) } else { Stroke::NONE },
            strikethrough: if slice.strikethrough == Some(true) { Stroke::new(1.0, foreground) } else { Stroke::NONE },
            ..Default::default()
        };
        job.append(slice.text, 0.0, format);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(text: &str) -> Vec<(String, Color32, bool)> {
        let mut job = LayoutJob::default();
        append(&mut job, text, &FontId::monospace(10.0), Color32::WHITE, true);
        job.sections
            .iter()
            .map(|s| (job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.color, s.format.italics))
            .collect()
    }

    #[test]
    fn styles_by_escape_codes() {
        let red = palette(true)[1];
        let shown: Vec<_> = sections("plain \x1b[31mred\x1b[0m \x1b[3mitalic\x1b[0m")
            .into_iter()
            .filter(|(text, _, _)| !text.is_empty())
            .collect();
        assert_eq!(
            shown,
            vec![
                ("plain ".into(), Color32::WHITE, false),
                ("red".into(), red, false),
                (" ".into(), Color32::WHITE, false),
                ("italic".into(), Color32::WHITE, true),
            ]
        );
    }
}
