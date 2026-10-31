//! Annotations: things on the canvas that aren't part of the graph, like sticky notes. They're saved and
//! undone along with the graph, but take no part in processing.

use std::collections::BTreeMap;

use eframe::egui::{self, Color32, Rect, UiBuilder};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationId(pub u64);

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// Top-left corner on the canvas.
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub kind: AnnotationKind,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnnotationKind {
    StickyNote(StickyNote),
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StickyNote {
    /// Markdown.
    pub text: String,
    pub colour: NoteColour,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum NoteColour {
    Yellow,
    Blue,
    Green,
    Pink,
}

impl NoteColour {
    pub const ALL: [NoteColour; 4] = [NoteColour::Yellow, NoteColour::Blue, NoteColour::Green, NoteColour::Pink];

    pub fn name(self) -> &'static str {
        match self {
            NoteColour::Yellow => "Yellow",
            NoteColour::Blue => "Blue",
            NoteColour::Green => "Green",
            NoteColour::Pink => "Pink",
        }
    }

    pub fn fill(self) -> Color32 {
        match self {
            NoteColour::Yellow => Color32::from_rgb(242, 222, 130),
            NoteColour::Blue => Color32::from_rgb(160, 200, 245),
            NoteColour::Green => Color32::from_rgb(170, 225, 170),
            NoteColour::Pink => Color32::from_rgb(242, 175, 200),
        }
    }
}

/// Text on a note.
const INK: Color32 = Color32::from_gray(30);

impl Annotation {
    pub const MIN_SIZE: [f32; 2] = [80.0, 50.0];

    pub fn sticky_note(pos: [f32; 2]) -> Self {
        Annotation {
            pos,
            size: [220.0, 140.0],
            kind: AnnotationKind::StickyNote(StickyNote {
                text: "### Note\nDouble-click to edit. Supports **markdown**:\n\n- lists\n- `code`".into(),
                colour: NoteColour::Yellow,
            }),
        }
    }

    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos.into(), self.size.into())
    }

    pub fn fill(&self) -> Color32 {
        match &self.kind {
            AnnotationKind::StickyNote(note) => note.colour.fill(),
        }
    }

    /// Shows the content in `rect` (on screen) at `zoom`: rendered, or as an editor while `editing`. Returns
    /// whether editing should carry on.
    pub fn content_ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        zoom: f32,
        editing: bool,
        cache: &mut CommonMarkCache,
    ) -> bool {
        let AnnotationKind::StickyNote(note) = &mut self.kind;
        let inner = rect.shrink(8.0 * zoom);
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        // Scaled so the text is laid out at the size it's shown, so stays sharp.
        let style = child.style_mut();
        for font in style.text_styles.values_mut() {
            font.size *= zoom;
        }
        style.spacing.item_spacing *= zoom;
        style.spacing.indent *= zoom;
        style.interaction.selectable_labels = false;
        style.visuals.override_text_color = Some(INK);
        // Strong text (bold, headings) takes its colour from the widget visuals.
        for widget in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.fg_stroke.color = INK;
        }
        if editing {
            let editor = child.add(
                egui::TextEdit::multiline(&mut note.text)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(4)
                    .text_color(INK)
                    .code_editor(),
            );
            if !editor.has_focus() && !editor.lost_focus() {
                editor.request_focus();
            }
            !editor.lost_focus()
        } else {
            // Below this the text would be too small to read anyway.
            if zoom > 0.35 {
                CommonMarkViewer::new().show(&mut child, cache, &note.text);
            }
            false
        }
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Annotations {
    next_id: u64,
    items: BTreeMap<AnnotationId, Annotation>,
}

impl Annotations {
    pub fn add(&mut self, annotation: Annotation) -> AnnotationId {
        self.next_id += 1;
        let id = AnnotationId(self.next_id);
        self.items.insert(id, annotation);
        id
    }

    pub fn remove(&mut self, id: AnnotationId) {
        self.items.remove(&id);
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.items.get(&id)
    }

    pub fn get_mut(&mut self, id: AnnotationId) -> Option<&mut Annotation> {
        self.items.get_mut(&id)
    }

    pub fn contains(&self, id: AnnotationId) -> bool {
        self.items.contains_key(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = AnnotationId> + '_ {
        self.items.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (AnnotationId, &Annotation)> {
        self.items.iter().map(|(id, a)| (*id, a))
    }
}
