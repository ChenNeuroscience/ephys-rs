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

impl AnnotationType for Markdown {
    const INFO: AnnotationInfo = AnnotationInfo {
        name: "Markdown",
        description: "A note in markdown, with links and images, on a plain background. Not part of the graph.",
        default_size: [260.0, 160.0],
    };

    /// Drawn out, it starts empty, to type straight into.
    fn drawn() -> Self {
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

    fn context_menu_ui(&mut self, ui: &mut egui::Ui) {
        if ui.button("Add image from file…").clicked() {
            self.add_image_file();
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let label = ui.label(RichText::new("Markdown").small().weak());
        ui.add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_rows(12).desired_width(f32::INFINITY))
            .labelled_by(label.id);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Images").small().weak());
            if ui.small_button("Add from file…").clicked() {
                self.add_image_file();
            }
        });
        if self.attachments.is_empty() {
            ui.weak("Paste an image while editing the note to attach it.");
        }
        let mut remove = None;
        for (name, base64) in &self.attachments {
            ui.horizontal(|ui| {
                ui.label(format!("{name} ({:.0} kB)", base64.len() as f64 * 0.75 / 1e3));
                if ui.small_button("Remove").clicked() {
                    remove = Some(name.clone());
                }
            });
        }
        if let Some(name) = remove {
            self.attachments.remove(&name);
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
