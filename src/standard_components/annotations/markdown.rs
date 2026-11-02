//! A markdown note on the canvas, on a plain background: headings, emphasis, lists, code, links (which open in the
//! browser), and images. An image pasted in while editing (or added from a file) becomes an attachment, kept with the
//! note as base64, and referenced from the text as `![name](attachment:name)`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Painter, Rect, RichText, Shape, Stroke};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};

use super::{encode, encode_pasted, image_size, image_uri};
use crate::core::annotation::{AnnotationInfo, AnnotationType};
use crate::core::theme::Colours;

/// How the text refers to an attachment.
const SCHEME: &str = "attachment:";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Markdown {
    pub text: String,
    /// Images, by name: each a file (PNG, JPEG...) in base64.
    pub attachments: BTreeMap<String, String>,
}

impl Default for Markdown {
    fn default() -> Self {
        Markdown {
            text: "### Markdown\nDouble-click to edit: **bold**, *italic*, `code`, lists, \
                   [links](https://open-ephys.org), and images pasted in."
                .into(),
            attachments: BTreeMap::new(),
        }
    }
}

impl Markdown {
    /// Attaches an image (a file, in base64), returning the markdown that shows it.
    pub(super) fn attach(&mut self, base64: String, extension: &str) -> String {
        let name = (1..).map(|n| format!("image{n}.{extension}")).find(|n| !self.attachments.contains_key(n)).unwrap();
        self.attachments.insert(name.clone(), base64);
        format!("![{name}]({SCHEME}{name})")
    }

    /// The text, with its attachments' references pointing to where egui can load them from.
    pub(super) fn shown(&self, ctx: &egui::Context) -> String {
        let mut text = self.text.clone();
        for (name, base64) in &self.attachments {
            text = text.replace(&format!("{SCHEME}{name}"), &image_uri(ctx, base64));
        }
        text
    }

    /// Asks for an image file, and attaches it, adding its markdown to the end.
    fn add_image_file(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg"]).pick_file() else {
            return;
        };
        match std::fs::read(&path) {
            Ok(bytes) if image_size(&bytes).is_some() => {
                let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
                let markdown = self.attach(encode(&bytes), &extension);
                self.text = format!("{}\n\n{markdown}\n", self.text.trim_end());
            }
            Ok(_) => log::warn!("{} isn't an image this can show", path.display()),
            Err(e) => log::warn!("Couldn't read {}: {e}", path.display()),
        }
    }
}

/// The cheat sheet's name, and what it says: markdown, and what it gives.
const CHEAT_SHEET: &str = "Markdown cheat sheet";
const CHEATS: &[(&str, &str)] = &[
    ("# Heading", "a heading (## and ### smaller)"),
    ("**bold**", "bold"),
    ("*italic*", "italic"),
    ("`code`", "code"),
    ("- item", "a list"),
    ("1. item", "a numbered list"),
    ("[text](https://…)", "a link"),
    ("![name](address)", "an image"),
    ("> quote", "a quote"),
    ("```", "a block of code, between two"),
    ("---", "a line across"),
];

/// Markdown, briefly, under a header to open it.
fn cheat_sheet(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(CHEAT_SHEET).id_salt(CHEAT_SHEET).show(ui, |ui| {
        egui::Grid::new("markdown cheats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            for (markdown, gives) in CHEATS {
                ui.label(RichText::new(*markdown).monospace());
                ui.label(RichText::new(*gives).small().weak());
                ui.end_row();
            }
        });
    });
}

impl Markdown {
    /// The text, to edit, in `size`.
    fn editor(&mut self, ui: &mut egui::Ui, size: egui::Vec2) {
        let label = ui.label(RichText::new("Markdown").small().weak());
        egui::ScrollArea::vertical().id_salt("markdown text").max_height(size.y - 20.0).show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut self.text)
                .code_editor()
                .desired_width(size.x)
                .min_size(egui::vec2(size.x, size.y - 24.0));
            ui.add(edit).labelled_by(label.id);
        });
    }

    /// Its images: each, to remove, and a button to add one from a file.
    fn images_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Images").small().weak());
            let tip = "Add an image from a file (or paste one in while editing the note)";
            if super::icon_button(ui, "Add an image", tip, |p, r| super::image::Image::paint_icon(p, r)).clicked() {
                self.add_image_file();
            }
        });
        if self.attachments.is_empty() {
            ui.label(RichText::new("None yet: paste one in while editing the note, or add one").small().weak());
        }
        let mut remove = None;
        for (name, base64) in &self.attachments {
            ui.horizontal(|ui| {
                let tip = format!("Remove {name}");
                if super::icon_button(ui, &tip, &tip, super::paint_cross).clicked() {
                    remove = Some(name.clone());
                }
                ui.label(RichText::new(format!("{name} ({:.0} kB)", base64.len() as f64 * 0.75 / 1e3)).small());
            });
        }
        if let Some(name) = remove {
            self.attachments.remove(&name);
        }
    }
}

impl AnnotationType for Markdown {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Markdown",
        description: "A note in markdown, with links and images, on a plain background. Not part of the graph.",
        default_size: [260.0, 160.0],
    };

    /// Drawn out, it starts empty, to type straight into.
    fn drawn(_flipped: [bool; 2]) -> Self {
        Markdown { text: String::new(), ..Default::default() }
    }

    fn fill(&self, theme: &Colours) -> Color32 {
        if theme.dark { theme.surface_raised } else { Color32::WHITE }
    }

    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool {
        if editing {
            let output = egui::TextEdit::multiline(&mut self.text)
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .code_editor()
                .show(ui);
            let editor = &output.response.response;
            // An image pasted in is attached, and shown where the cursor is.
            let pasted = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::PasteImage(image) => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some((base64, _)) = pasted.as_deref().and_then(encode_pasted) {
                let markdown = self.attach(base64, "png");
                let at = output.cursor_range.map_or(self.text.chars().count(), |r| r.primary.index.0);
                let byte = self.text.char_indices().nth(at).map_or(self.text.len(), |(b, _)| b);
                self.text.insert_str(byte, &markdown);
            }
            if !editor.has_focus() && !editor.lost_focus() {
                editor.request_focus();
            }
            !editor.lost_focus()
        } else {
            // The markdown viewer's cache, shared by every note.
            let id = egui::Id::unique("markdown annotation");
            let cache = ui.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<CommonMarkCache>>>(id).clone());
            let text = self.shown(ui.ctx());
            CommonMarkViewer::new().max_image_width(Some(ui.available_width() as usize)).show(
                ui,
                &mut cache.lock().unwrap(),
                &text,
            );
            false
        }
    }

    /// The text, filling most of the pane; its images (to add from a file, or remove); and a cheat sheet. Side by
    /// side if the pane is wide, otherwise one above another.
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size();
        let wide = size.x > 1.3 * size.y && size.x > 560.0;
        if wide {
            let side = 260.0;
            ui.horizontal_top(|ui| {
                let editor = egui::vec2(ui.available_width() - side - 12.0, ui.available_height());
                ui.vertical(|ui| self.editor(ui, editor));
                ui.vertical(|ui| {
                    ui.set_width(side);
                    self.images_ui(ui);
                    ui.add_space(6.0);
                    cheat_sheet(ui);
                });
            });
        } else {
            // Room below the text for the images (and the cheat sheet, if it's open).
            let cheat_open =
                egui::collapsing_header::CollapsingState::load(ui.ctx(), ui.make_persistent_id(CHEAT_SHEET))
                    .is_some_and(|s| s.is_open());
            let below = 70.0 + 22.0 * self.attachments.len() as f32 + if cheat_open { 250.0 } else { 0.0 };
            let editor = egui::vec2(ui.available_width(), (size.y - below).max(120.0));
            self.editor(ui, editor);
            ui.add_space(6.0);
            self.images_ui(ui);
            ui.add_space(4.0);
            cheat_sheet(ui);
        }
    }

    /// A white page with a heading and lines of text, one a link.
    fn paint_icon(painter: &Painter, rect: Rect) {
        let s = rect.width() / 16.0;
        let p = |x: f32, y: f32| rect.min + egui::vec2(x, y) * s;
        let outline = Stroke::new(s, Color32::from_gray(90));
        painter.add(Shape::convex_polygon(
            vec![p(2.0, 1.5), p(14.0, 1.5), p(14.0, 14.5), p(2.0, 14.5)],
            Color32::WHITE,
            outline,
        ));
        painter.line_segment([p(4.0, 4.5), p(10.0, 4.5)], Stroke::new(2.0 * s, Color32::from_gray(60)));
        painter.line_segment([p(4.0, 8.0), p(12.0, 8.0)], Stroke::new(s, Color32::from_gray(130)));
        painter.line_segment([p(4.0, 11.0), p(9.0, 11.0)], Stroke::new(s, Color32::from_rgb(40, 110, 220)));
    }
}
