//! Annotations: things on the canvas that aren't part of the graph, like sticky notes. They're saved and undone
//! along with the graph, but take no part in processing.
//!
//! Like node types, annotation types are declared by the program (with `annotation_set!`): each is a struct
//! implementing `AnnotationType`. The canvas does what's the same for them all: selecting them, moving them,
//! editing and deleting them, which goes in front of which, and making them (drawn out, chosen, or pasted). Each draws
//! itself, and takes what interaction of its own it likes (resizing, dragging an arrow's ends), in `canvas_ui`, with
//! the helpers in `AnnotationUi` for what they share: a card's background, a resizing corner, ends to drag, content
//! scaled to the zoom. What's pasted onto the canvas (an image, or text) becomes an annotation of whichever type
//! suits it best (see `AnnotationType::pasted`).

use std::collections::BTreeMap;

use eframe::egui::emath::TSTransform;
use eframe::egui::{self, Color32, ColorImage, Id, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::theme::Colours;

/// The smallest an annotation can be made, in canvas units, unless its type says otherwise.
pub const MIN_SIZE: [f32; 2] = [80.0, 50.0];

/// Something pasted onto the canvas.
#[derive(Clone, Copy, Debug)]
pub enum PastedContent<'a> {
    Image(&'a ColorImage),
    /// Text, trimmed (and not empty).
    Text(&'a str),
}

/// How well an annotation type suits something pasted (see `AnnotationType::pasted`), worst first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Fit {
    /// It can show it, if nothing's better.
    Possible,
    /// It shows it well.
    Good,
    /// It's what it's for.
    Best,
}

/// Static facts about an annotation type.
pub struct AnnotationInfo {
    pub name: &'static str,
    pub description: &'static str,
    /// Its size when added without being drawn out.
    pub default_size: [f32; 2],
}

/// An annotation type.
pub trait AnnotationType: Default + Clone + PartialEq + Serialize + DeserializeOwned + 'static {
    const INFO: AnnotationInfo;

    /// The smallest it can be made, in canvas units.
    const MIN_SIZE: [f32; 2] = MIN_SIZE;

    /// One just drawn out on the canvas, which goes straight into editing. `flipped` says whether it was drawn
    /// leftwards and upwards (an arrow drawn that way points that way, say). By default the same as `default`.
    fn drawn(_flipped: [bool; 2]) -> Self {
        Self::default()
    }

    /// Draws it on the canvas, and takes what interaction of its own it likes, with `cx`'s helpers (see
    /// `AnnotationUi`). Returns whether editing (if it is: `cx.editing`) should carry on. By default, a card: resized
    /// by its corner, its background (`fill`), what's drawn over it (`paint`), and its content (`content_ui`).
    fn canvas_ui(&mut self, cx: &mut AnnotationUi) -> bool {
        cx.resize_corner(None);
        cx.card(self.fill(cx.theme));
        self.paint(cx.painter, cx.rect(), cx.zoom(), cx.theme);
        let editing = cx.editing;
        cx.content(|ui| self.content_ui(ui, editing))
    }

    /// Its background on the canvas, in the theme's colours if it wants (for the default `canvas_ui`, and the outline
    /// while one's drawn out). An opaque one has a shadow.
    fn fill(&self, theme: &Colours) -> Color32;

    /// For the default `canvas_ui`: what it draws besides its content, over its background (a border, say), filling
    /// `rect` (on screen).
    fn paint(&self, _painter: &Painter, _rect: Rect, _zoom: f32, _theme: &Colours) {}

    /// Whether it can be grabbed (to move, select or edit) at `at`, on screen, within `rect` (grown by
    /// `AnnotationUi::REACH` all round, for one that's thin, like a line): by default anywhere in `rect` itself; a box,
    /// say, only by its edge, so what's inside it can still be grabbed.
    fn hit(&self, rect: Rect, at: Pos2, _zoom: f32) -> bool {
        rect.contains(at)
    }

    /// Whether it goes behind other annotations (a box grouping them, say).
    fn behind(&self) -> bool {
        false
    }

    /// Whether it goes above the graph (as most do), rather than beneath it (a box grouping nodes, say).
    fn above_graph(&self) -> bool {
        true
    }

    /// One showing what's been pasted onto the canvas, if this type can: how well it suits it, the annotation, and
    /// its natural size (which the canvas scales down to fit the view). Of the types that can, the one that suits it
    /// best makes it (of two as good, the first in `annotation_set!`).
    fn pasted(_pasted: &PastedContent) -> Option<(Fit, Self, [f32; 2])> {
        None
    }

    /// Whether it's drawn out on the canvas (with its toolbar tool). If not, it's made with `choose` instead.
    const DRAWN: bool = true;

    /// One made by choosing something (an image file, say), and its natural size; `None` if nothing was chosen.
    fn choose() -> Option<(Self, [f32; 2])> {
        None
    }

    /// For the default `canvas_ui`: shows its content in `ui` (sized and scaled to fit it on the canvas: see
    /// `AnnotationUi::content`), as it looks, or for editing while `editing`. Returns whether editing should carry on.
    fn content_ui(&mut self, _ui: &mut egui::Ui, _editing: bool) -> bool {
        false
    }

    /// Its settings, for the annotation pane (shown while it's selected).
    fn settings_ui(&mut self, ui: &mut egui::Ui);

    /// Buttons of its own in the annotation pane's toolbar, beside its delete button (laid out right to left).
    fn toolbar_ui(&mut self, _ui: &mut egui::Ui) {}

    /// Paints its icon (for the toolbar's tool to draw one) to fill `rect`.
    fn paint_icon(painter: &Painter, rect: Rect);
}

/// The closed set of annotation types a program is built with, as an enum with one variant per type.
/// Implemented by `annotation_set!`, and forwards each `AnnotationType` method to the variant's type.
pub trait AnnotationSet: Clone + PartialEq + Serialize + DeserializeOwned + 'static {
    /// Indexed by type index.
    const TYPES: &'static [AnnotationInfo];

    fn create(type_index: usize) -> Self;
    fn drawn(type_index: usize, flipped: [bool; 2]) -> Self;
    fn min_size(type_index: usize) -> [f32; 2];
    /// Whether annotations of a type are drawn out, or made with `choose`.
    fn is_drawn(type_index: usize) -> bool;
    fn choose(type_index: usize) -> Option<(Self, [f32; 2])>;
    fn canvas_ui(&mut self, cx: &mut AnnotationUi) -> bool;
    fn hit(&self, rect: Rect, at: Pos2, zoom: f32) -> bool;
    fn behind(&self) -> bool;
    fn above_graph(&self) -> bool;
    fn type_index(&self) -> usize;
    fn fill(&self, theme: &Colours) -> Color32;
    /// An annotation of the type that best suits what's pasted (see `AnnotationType::pasted`), and its size.
    fn pasted(pasted: &PastedContent) -> Option<(Self, [f32; 2])>;
    fn settings_ui(&mut self, ui: &mut egui::Ui);
    fn toolbar_ui(&mut self, ui: &mut egui::Ui);
    fn paint_icon(type_index: usize, painter: &Painter, rect: Rect);

    fn info(&self) -> &'static AnnotationInfo {
        &Self::TYPES[self.type_index()]
    }
}

/// Declares the enum of annotation types a program is built with, and implements `AnnotationSet` for it:
///
/// ```ignore
/// annotation_set! {
///     pub enum AnnotationTypes {
///         StickyNote(sticky_note::StickyNote),
///     }
/// }
/// ```
///
/// Saved files name each annotation by its variant, so renaming a variant breaks existing files.
#[macro_export]
macro_rules! annotation_set {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident($ty:ty)),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "content")]
        $vis enum $name {
            $($variant($ty)),+
        }

        impl $crate::core::annotation::AnnotationSet for $name {
            const TYPES: &'static [$crate::core::annotation::AnnotationInfo] =
                &[$(<$ty as $crate::core::annotation::AnnotationType>::INFO),+];

            #[allow(unused_assignments)]
            fn create(type_index: usize) -> Self {
                let mut i = 0;
                $(
                    if i == type_index {
                        return $name::$variant(<$ty>::default());
                    }
                    i += 1;
                )+
                panic!("no annotation type {type_index}")
            }

            #[allow(unused_assignments)]
            fn drawn(type_index: usize, flipped: [bool; 2]) -> Self {
                let mut i = 0;
                $(
                    if i == type_index {
                        return $name::$variant(<$ty as $crate::core::annotation::AnnotationType>::drawn(flipped));
                    }
                    i += 1;
                )+
                panic!("no annotation type {type_index}")
            }

            #[allow(unused_assignments)]
            fn min_size(type_index: usize) -> [f32; 2] {
                let mut i = 0;
                $(
                    if i == type_index {
                        return <$ty as $crate::core::annotation::AnnotationType>::MIN_SIZE;
                    }
                    i += 1;
                )+
                $crate::core::annotation::MIN_SIZE
            }

            #[allow(unused_assignments)]
            fn is_drawn(type_index: usize) -> bool {
                let mut i = 0;
                $(
                    if i == type_index {
                        return <$ty as $crate::core::annotation::AnnotationType>::DRAWN;
                    }
                    i += 1;
                )+
                true
            }

            #[allow(unused_assignments)]
            fn choose(type_index: usize) -> Option<(Self, [f32; 2])> {
                let mut i = 0;
                $(
                    if i == type_index {
                        return <$ty as $crate::core::annotation::AnnotationType>::choose().map(|(a, size)| ($name::$variant(a), size));
                    }
                    i += 1;
                )+
                None
            }

            fn canvas_ui(&mut self, cx: &mut $crate::core::annotation::AnnotationUi) -> bool {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::canvas_ui(a, cx)),+ }
            }

            fn hit(&self, rect: eframe::egui::Rect, at: eframe::egui::Pos2, zoom: f32) -> bool {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::hit(a, rect, at, zoom)),+ }
            }

            fn behind(&self) -> bool {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::behind(a)),+ }
            }

            fn above_graph(&self) -> bool {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::above_graph(a)),+ }
            }

            #[allow(unused_assignments, irrefutable_let_patterns)]
            fn type_index(&self) -> usize {
                let mut i = 0;
                $(
                    if let $name::$variant(_) = self {
                        return i;
                    }
                    i += 1;
                )+
                unreachable!()
            }

            fn fill(&self, theme: &$crate::core::theme::Colours) -> eframe::egui::Color32 {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::fill(a, theme)),+ }
            }

            fn pasted(pasted: &$crate::core::annotation::PastedContent) -> Option<(Self, [f32; 2])> {
                let mut best: Option<($crate::core::annotation::Fit, Self, [f32; 2])> = None;
                $(
                    if let Some((fit, a, size)) = <$ty as $crate::core::annotation::AnnotationType>::pasted(pasted)
                        && best.as_ref().is_none_or(|(b, _, _)| fit > *b)
                    {
                        best = Some((fit, $name::$variant(a), size));
                    }
                )+
                best.map(|(_, a, size)| (a, size))
            }

            fn settings_ui(&mut self, ui: &mut eframe::egui::Ui) {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::settings_ui(a, ui)),+ }
            }

            fn toolbar_ui(&mut self, ui: &mut eframe::egui::Ui) {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::toolbar_ui(a, ui)),+ }
            }

            #[allow(unused_assignments)]
            fn paint_icon(type_index: usize, painter: &eframe::egui::Painter, rect: eframe::egui::Rect) {
                let mut i = 0;
                $(
                    if i == type_index {
                        return <$ty as $crate::core::annotation::AnnotationType>::paint_icon(painter, rect);
                    }
                    i += 1;
                )+
            }
        }
    };
}

/// An annotation on the canvas, unique within it and never reused, as `NodeId` is for nodes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationId(pub u64);

/// An annotation on the canvas.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "A: AnnotationSet")]
pub struct Annotation<A> {
    /// Top-left corner on the canvas.
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub kind: A,
}

impl<A: AnnotationSet> Annotation<A> {
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos.into(), self.size.into())
    }
}

/// The annotations on the canvas, by id: saved and undone with the graph, but no part of how it runs.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "A: AnnotationSet")]
pub struct Annotations<A> {
    next_id: u64,
    items: BTreeMap<AnnotationId, Annotation<A>>,
}

impl<A> Default for Annotations<A> {
    fn default() -> Self {
        Annotations { next_id: 0, items: BTreeMap::new() }
    }
}

impl<A: AnnotationSet> Annotations<A> {
    pub fn add(&mut self, annotation: Annotation<A>) -> AnnotationId {
        self.next_id += 1;
        let id = AnnotationId(self.next_id);
        self.items.insert(id, annotation);
        id
    }

    pub fn remove(&mut self, id: AnnotationId) {
        self.items.remove(&id);
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Annotation<A>> {
        self.items.get(&id)
    }

    pub fn get_mut(&mut self, id: AnnotationId) -> Option<&mut Annotation<A>> {
        self.items.get_mut(&id)
    }

    pub fn contains(&self, id: AnnotationId) -> bool {
        self.items.contains_key(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = AnnotationId> + '_ {
        self.items.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (AnnotationId, &Annotation<A>)> {
        self.items.iter().map(|(id, a)| (*id, a))
    }
}

/// What an annotation draws itself with on the canvas (see `AnnotationType::canvas_ui`): where it is, how it's being
/// shown, and helpers for what annotations share.
pub struct AnnotationUi<'a> {
    pub ui: &'a mut egui::Ui,
    pub painter: &'a Painter,
    pub theme: &'a Colours,
    pub id: AnnotationId,
    /// Where it is on the canvas (its top-left corner), and its size, in canvas units: to change, to resize it, say.
    pub pos: &'a mut [f32; 2],
    pub size: &'a mut [f32; 2],
    /// The smallest it can be made (its type's `MIN_SIZE`).
    pub min_size: [f32; 2],
    pub selected: bool,
    /// Whether the pointer's over where it's grabbed.
    pub hovered: bool,
    /// Whether it's being edited (after a double-click, say).
    pub editing: bool,
    /// From the canvas to the screen.
    pub to_screen: TSTransform,
}

impl AnnotationUi<'_> {
    /// How far round its rect an annotation can be grabbed, on screen: so a thin one (a line) can be, and the handles
    /// at its ends (see `drag_ends`).
    pub const REACH: f32 = 8.0;

    /// Where it is, on screen.
    pub fn rect(&self) -> Rect {
        self.to_screen * self.canvas_rect()
    }

    /// Where it is, on the canvas.
    pub fn canvas_rect(&self) -> Rect {
        Rect::from_min_size((*self.pos).into(), (*self.size).into())
    }

    /// Moves or resizes it, to `rect` on the canvas.
    pub fn set_canvas_rect(&mut self, rect: Rect) {
        (*self.pos, *self.size) = (rect.min.into(), rect.size().into());
    }

    pub fn zoom(&self) -> f32 {
        self.to_screen.scaling
    }

    /// A handle in its bottom-right corner to resize it by, no smaller than `min_size`; keeping `aspect` (its width
    /// over its height), if it's given, by whichever side's dragged further.
    pub fn resize_corner(&mut self, aspect: Option<f32>) {
        let rect = self.rect();
        let handle = Rect::from_min_max(rect.max - Vec2::splat(12.0 * self.zoom()), rect.max);
        let resize = self.ui.interact(handle, Id::unique(("annotation-resize", self.id)), Sense::drag());
        if resize.dragged() {
            let old = Vec2::from(*self.size);
            let mut size = (old + resize.drag_delta() / self.zoom()).max(self.min_size.into());
            if let Some(aspect) = aspect.filter(|a| a.is_finite() && *a > 0.0) {
                if (size.x - old.x).abs() / aspect >= (size.y - old.y).abs() {
                    size.y = size.x / aspect;
                } else {
                    size.x = size.y * aspect;
                }
            }
            *self.size = size.into();
        }
        resize.on_hover_cursor(egui::CursorIcon::ResizeNwSe);
    }

    /// A card: its background in `fill` (with a shadow, if it's opaque), outlined while it's selected, with grip lines
    /// in the resizing corner (for one that's see-through, only while it's selected).
    pub fn card(&self, fill: Color32) {
        let (rect, zoom, painter) = (self.rect(), self.zoom(), self.painter);
        let opaque = fill.a() == 255;
        if opaque {
            painter.rect_filled(rect.translate(egui::vec2(0.0, 3.0 * zoom)), 4.0 * zoom, self.theme.shadow);
        }
        painter.rect_filled(rect, 4.0 * zoom, fill);
        if self.selected {
            painter.rect_stroke(rect, 4.0 * zoom, Stroke::new(2.0, self.theme.accent), StrokeKind::Outside);
        }
        let grip = if opaque { Color32::from_black_alpha(70) } else { self.theme.text_secondary };
        for i in (1..=3).filter(|_| opaque || self.selected) {
            let d = i as f32 * 3.0 * zoom;
            let corner = rect.max - Vec2::splat(2.0 * zoom);
            painter.line_segment([corner - egui::vec2(d, 0.0), corner - egui::vec2(0.0, d)], Stroke::new(1.0, grip));
        }
    }

    /// Handles at `ends` (points on the canvas: an arrow's, say), each of which can be dragged. Returns where they
    /// are now. (Draw them with `mark_ends`, once it's drawn.)
    pub fn drag_ends<const N: usize>(&mut self, mut ends: [Pos2; N]) -> [Pos2; N] {
        for (index, end) in ends.iter_mut().enumerate() {
            let handle = Rect::from_center_size(self.to_screen * *end, Vec2::splat(2.0 * Self::REACH));
            let response = self.ui.interact(handle, Id::unique(("annotation-end", self.id, index)), Sense::drag());
            if response.dragged() {
                *end += response.drag_delta() / self.zoom();
            }
            response.on_hover_cursor(egui::CursorIcon::Crosshair);
        }
        ends
    }

    /// Marks `ends` (points on the canvas) as handles to drag, while it's selected or hovered.
    pub fn mark_ends(&self, ends: &[Pos2]) {
        if self.selected || self.hovered {
            for end in ends {
                let stroke = Stroke::new(2.0, self.theme.accent);
                self.painter.circle(self.to_screen * *end, 4.5, self.theme.surface, stroke);
            }
        }
    }

    /// Its content, in `add`'s `ui`: within it (inset a little), scaled to the zoom so its text is laid out at the
    /// size it's shown (so stays sharp). Not shown when it's zoomed so far out it couldn't be read, unless it's being
    /// edited. Returns what `add` does (whether editing should carry on), or false.
    pub fn content(&mut self, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
        let zoom = self.zoom();
        if zoom <= 0.35 && !self.editing {
            return false;
        }
        let inner = self.rect().shrink(8.0 * zoom);
        let builder = UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min));
        let mut child = self.ui.new_child(builder);
        child.set_clip_rect(inner.intersect(self.ui.clip_rect()));
        let style = child.style_mut();
        for font in style.text_styles.values_mut() {
            font.size *= zoom;
        }
        style.spacing.item_spacing *= zoom;
        style.spacing.indent *= zoom;
        style.interaction.selectable_labels = false;
        add(&mut child)
    }
}
