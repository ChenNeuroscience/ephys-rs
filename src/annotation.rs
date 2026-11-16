//! The canvas can have annotations on it, things that aren't part of the graph, like sticky notes. They're saved and
//! undone along with the graph, but take no part in processing.
//!
//! Like node types, annotation types are declared by the program (with `annotations_enum!`): each is a struct
//! implementing `AnnotationType`. The canvas does what's the same for them all: selecting them, moving them, editing
//! and deleting them, which goes in front of which, and making them (drawn out, chosen, or pasted). Each type draws its
//! annotations itself, and takes what interaction of its own it likes (resizing, dragging an arrow's ends), in
//! `canvas_ui`, with the helpers in `AnnotationUi` for what they share: a card's background, a resizing corner, ends to
//! drag, content scaled to the zoom. What's pasted onto the canvas (an image, or text) becomes an annotation of
//! whichever type suits it best (see `AnnotationType::pasted`).

use std::collections::BTreeMap;

use eframe::egui::emath::TSTransform;
use eframe::egui::{self, Color32, ColorImage, Id, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::theme::Colours;

/// An annotation can be made no smaller than this, in canvas units, unless its type says otherwise.
pub const MIN_SIZE: [f32; 2] = [80.0, 50.0];

/// Someone can paste an image or text onto the canvas.
#[derive(Clone, Copy, Debug)]
pub enum PastedContent<'a> {
    Image(&'a ColorImage),
    /// Text pasted, trimmed (and not empty).
    Text(&'a str),
}

/// An annotation type suits something pasted this well (see `AnnotationType::pasted`), worst first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Fit {
    /// The type can show it, if nothing's better.
    Possible,
    /// The type shows it well.
    Good,
    /// The type's for showing it.
    Best,
}

/// The toolbar describes each annotation type by these facts, which never change.
pub struct AnnotationInfo {
    pub name: &'static str,
    pub description: &'static str,
    /// An annotation of the type added without being drawn out is this size.
    pub default_size: [f32; 2],
}

/// Each annotation type implements this.
pub trait AnnotationType: Default + Clone + PartialEq + Serialize + DeserializeOwned + 'static {
    const INFO: AnnotationInfo;

    /// An annotation of the type can be made no smaller than this, in canvas units.
    const MIN_SIZE: [f32; 2] = MIN_SIZE;

    /// An annotation just drawn out on the canvas goes straight into editing; this makes it. `flipped` says whether it
    /// was drawn leftwards and upwards (an arrow drawn that way points that way, say). By default, it's `default`'s.
    fn drawn(_flipped: [bool; 2]) -> Self {
        Self::default()
    }

    /// The canvas has the annotation draw itself with this, taking what interaction of its own it likes, with `cx`'s
    /// helpers (see `AnnotationUi`). This returns whether editing (if it is: `cx.editing`) should carry on. By default,
    /// the annotation's a card: resized by its corner, its background (`fill`), what's drawn over it (`paint`), and its
    /// content (`content_ui`).
    fn canvas_ui(&mut self, cx: &mut AnnotationUi) -> bool {
        cx.resize_corner(None);
        cx.card(self.fill(cx.theme));
        self.paint(cx.painter, cx.rect(), cx.zoom(), cx.theme);
        let editing = cx.editing;
        cx.content(|ui| self.content_ui(ui, editing))
    }

    /// The annotation's background on the canvas, in the theme's colours if it wants (for the default `canvas_ui`, and
    /// the outline while one's drawn out). An opaque one has a shadow.
    fn fill(&self, theme: &Colours) -> Color32;

    /// The default `canvas_ui` has the annotation draw what it likes besides its content, over its background (a
    /// border, say), filling `rect` (on screen), with this.
    fn paint(&self, _painter: &Painter, _rect: Rect, _zoom: f32, _theme: &Colours) {}

    /// The canvas asks whether the annotation can be grabbed (to move, select or edit) at `at`, on screen, within
    /// `rect` (grown by `AnnotationUi::REACH` all round, for one that's thin, like a line): by default anywhere in
    /// `rect` itself; a box, say, only by its edge, so what's inside it can still be grabbed.
    fn hit(&self, rect: Rect, at: Pos2, _zoom: f32) -> bool {
        rect.contains(at)
    }

    /// Some annotations go behind others (a box grouping them, say). This says whether this one does.
    fn behind(&self) -> bool {
        false
    }

    /// Most annotations go above the graph, but some go beneath it (a box grouping nodes, say). This says whether this
    /// one goes above.
    fn above_graph(&self) -> bool {
        true
    }

    /// When something's pasted onto the canvas, each type that can show it makes an annotation here: how well it suits
    /// it, the annotation, and its natural size (which the canvas scales down to fit the view). Of the types that can,
    /// the one that suits it best is used (of two as good, the first in `annotations_enum!`).
    fn pasted(_pasted: &PastedContent) -> Option<(Fit, Self, [f32; 2])> {
        None
    }

    /// Annotations of most types are drawn out on the canvas (with the type's toolbar tool); others are made with
    /// `choose` instead. This says whether this type's are drawn.
    const DRAWN: bool = true;

    /// A type that isn't drawn makes an annotation by having someone choose something (an image file, say); this gives
    /// it, and its natural size, or `None` if nothing was chosen.
    fn choose() -> Option<(Self, [f32; 2])> {
        None
    }

    /// The default `canvas_ui` has the annotation show its content in `ui` (sized and scaled to fit it on the canvas:
    /// see `AnnotationUi::content`), as it looks, or for editing while `editing`. This returns whether editing should
    /// carry on.
    fn content_ui(&mut self, _ui: &mut egui::Ui, _editing: bool) -> bool {
        false
    }

    /// The annotation pane shows the annotation's settings while it's selected, with this.
    fn settings_ui(&mut self, ui: &mut egui::Ui);

    /// The annotation can put buttons of its own in the annotation pane's toolbar, beside its delete button (laid out
    /// right to left).
    fn toolbar_ui(&mut self, _ui: &mut egui::Ui) {}

    /// The toolbar's tool for drawing one shows the type's icon, painted with this to fill `rect`.
    fn paint_icon(painter: &Painter, rect: Rect);
}

/// The app sometimes needs an annotation's type without the annotation: the toolbar's tools, say. `annotations_enum!`
/// makes a fieldless enum of the program's annotation types for that, one variant each, in the same order
/// (`AnnotationsEnum::Type`), and implements this for it.
pub trait AnnotationTypeEnum: Copy + Eq + std::hash::Hash + std::fmt::Debug + 'static {
    /// Every type, in the order the program declares them.
    const ALL: &'static [Self];

    fn info(self) -> &'static AnnotationInfo;
    /// An annotation of the type can be made no smaller than this (its `AnnotationType::MIN_SIZE`).
    fn min_size(self) -> [f32; 2];
    /// Annotations of the type are drawn out, or made with `AnnotationsEnum::choose`; this says which.
    fn is_drawn(self) -> bool;
    /// The toolbar's tool shows the type's icon, painted with this to fill `rect`.
    fn paint_icon(self, painter: &Painter, rect: Rect);

    /// The type called `name` (by its `AnnotationInfo::name`), if there is one.
    fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.info().name == name)
    }
}

/// The app holds a program's annotations as an enum with one variant per annotation type, which `annotations_enum!`
/// makes and implements this for. Each method hands on to the variant's type's `AnnotationType` method.
pub trait AnnotationsEnum: Clone + PartialEq + Serialize + DeserializeOwned + 'static {
    /// The program's annotation types, without their content (see `AnnotationTypeEnum`).
    type Type: AnnotationTypeEnum;

    fn create(annotation_type: Self::Type) -> Self;
    fn drawn(annotation_type: Self::Type, flipped: [bool; 2]) -> Self;
    fn choose(annotation_type: Self::Type) -> Option<(Self, [f32; 2])>;
    fn canvas_ui(&mut self, cx: &mut AnnotationUi) -> bool;
    fn hit(&self, rect: Rect, at: Pos2, zoom: f32) -> bool;
    fn behind(&self) -> bool;
    fn above_graph(&self) -> bool;
    fn annotation_type(&self) -> Self::Type;
    fn fill(&self, theme: &Colours) -> Color32;
    /// When something's pasted, this makes an annotation of the type that best suits it (see `AnnotationType::pasted`),
    /// and gives its size.
    fn pasted(pasted: &PastedContent) -> Option<(Self, [f32; 2])>;
    fn settings_ui(&mut self, ui: &mut egui::Ui);
    fn toolbar_ui(&mut self, ui: &mut egui::Ui);

    fn info(&self) -> &'static AnnotationInfo {
        self.annotation_type().info()
    }
}

/// A program declares the annotation types it's built with like this, which makes the enum of them, implementing
/// `AnnotationsEnum`, and the enum of just their types, named as the program says, with the same variants holding
/// nothing, implementing `AnnotationTypeEnum`:
///
/// ```ignore
/// annotations_enum! {
///     pub enum AnnotationTypes {
///         StickyNote(sticky_note::StickyNote),
///     }
///     pub enum AnnotationKind;
/// }
/// ```
///
/// Saved files name each annotation by its variant, so renaming a variant breaks existing files (unless a config patch
/// renames it: see `crate::config_patching`).
#[macro_export]
macro_rules! annotations_enum {
    (
        $(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident($ty:ty)),+ $(,)? }
        $(#[$type_meta:meta])* $type_vis:vis enum $type_name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "content")]
        $vis enum $name {
            $($variant($ty)),+
        }

        $(#[$type_meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $type_vis enum $type_name {
            $($variant),+
        }

        impl $crate::annotation::AnnotationTypeEnum for $type_name {
            const ALL: &'static [Self] = &[$($type_name::$variant),+];

            fn info(self) -> &'static $crate::annotation::AnnotationInfo {
                match self { $($type_name::$variant => &<$ty as $crate::annotation::AnnotationType>::INFO),+ }
            }

            fn min_size(self) -> [f32; 2] {
                match self { $($type_name::$variant => <$ty as $crate::annotation::AnnotationType>::MIN_SIZE),+ }
            }

            fn is_drawn(self) -> bool {
                match self { $($type_name::$variant => <$ty as $crate::annotation::AnnotationType>::DRAWN),+ }
            }

            fn paint_icon(self, painter: &eframe::egui::Painter, rect: eframe::egui::Rect) {
                match self { $($type_name::$variant => <$ty as $crate::annotation::AnnotationType>::paint_icon(painter, rect)),+ }
            }
        }

        impl $crate::annotation::AnnotationsEnum for $name {
            type Type = $type_name;

            fn create(annotation_type: $type_name) -> Self {
                match annotation_type { $($type_name::$variant => $name::$variant(<$ty>::default())),+ }
            }

            fn drawn(annotation_type: $type_name, flipped: [bool; 2]) -> Self {
                match annotation_type {
                    $($type_name::$variant => $name::$variant(<$ty as $crate::annotation::AnnotationType>::drawn(flipped))),+
                }
            }

            fn choose(annotation_type: $type_name) -> Option<(Self, [f32; 2])> {
                match annotation_type {
                    $($type_name::$variant => <$ty as $crate::annotation::AnnotationType>::choose().map(|(a, size)| ($name::$variant(a), size))),+
                }
            }

            fn canvas_ui(&mut self, cx: &mut $crate::annotation::AnnotationUi) -> bool {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::canvas_ui(a, cx)),+ }
            }

            fn hit(&self, rect: eframe::egui::Rect, at: eframe::egui::Pos2, zoom: f32) -> bool {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::hit(a, rect, at, zoom)),+ }
            }

            fn behind(&self) -> bool {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::behind(a)),+ }
            }

            fn above_graph(&self) -> bool {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::above_graph(a)),+ }
            }

            fn annotation_type(&self) -> $type_name {
                match self { $($name::$variant(_) => $type_name::$variant),+ }
            }

            fn fill(&self, theme: &$crate::theme::Colours) -> eframe::egui::Color32 {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::fill(a, theme)),+ }
            }

            fn pasted(pasted: &$crate::annotation::PastedContent) -> Option<(Self, [f32; 2])> {
                let mut best: Option<($crate::annotation::Fit, Self, [f32; 2])> = None;
                $(
                    if let Some((fit, a, size)) = <$ty as $crate::annotation::AnnotationType>::pasted(pasted)
                        && best.as_ref().is_none_or(|(b, _, _)| fit > *b)
                    {
                        best = Some((fit, $name::$variant(a), size));
                    }
                )+
                best.map(|(_, a, size)| (a, size))
            }

            fn settings_ui(&mut self, ui: &mut eframe::egui::Ui) {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::settings_ui(a, ui)),+ }
            }

            fn toolbar_ui(&mut self, ui: &mut eframe::egui::Ui) {
                match self { $($name::$variant(a) => $crate::annotation::AnnotationType::toolbar_ui(a, ui)),+ }
            }
        }
    };
}

/// The canvas names each annotation with an id, unique within it and never reused, as `NodeId` is for nodes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationId(pub u64);

/// The canvas holds each annotation with where it is.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "A: AnnotationsEnum")]
pub struct Annotation<A> {
    /// The annotation's top-left corner on the canvas.
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub kind: A,
}

impl<A: AnnotationsEnum> Annotation<A> {
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos.into(), self.size.into())
    }
}

/// The canvas keeps its annotations by id, saved and undone with the graph, but no part of how it runs.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "A: AnnotationsEnum")]
pub struct Annotations<A> {
    next_id: u64,
    items: BTreeMap<AnnotationId, Annotation<A>>,
}

impl<A> Default for Annotations<A> {
    fn default() -> Self {
        Annotations { next_id: 0, items: BTreeMap::new() }
    }
}

impl<A: AnnotationsEnum> Annotations<A> {
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

/// An annotation draws itself on the canvas with this (see `AnnotationType::canvas_ui`): where it is, how it's being
/// shown, and helpers for what annotations share.
pub struct AnnotationUi<'a> {
    pub ui: &'a mut egui::Ui,
    pub painter: &'a Painter,
    pub theme: &'a Colours,
    pub id: AnnotationId,
    /// Where the annotation is on the canvas (its top-left corner), and its size, in canvas units, which it changes to
    /// resize itself, say.
    pub pos: &'a mut [f32; 2],
    pub size: &'a mut [f32; 2],
    /// The annotation can be made no smaller than this (its type's `MIN_SIZE`).
    pub min_size: [f32; 2],
    pub selected: bool,
    /// While the pointer's over where it's grabbed, this is set.
    pub hovered: bool,
    /// While it's being edited (after a double-click, say), this is set.
    pub editing: bool,
    /// The canvas maps its coordinates to the screen's by this.
    pub to_screen: TSTransform,
}

impl AnnotationUi<'_> {
    /// An annotation can be grabbed this far round its rect, on screen, so a thin one (a line) can be, and the handles
    /// at its ends (see `drag_ends`).
    pub const REACH: f32 = 8.0;

    /// Where the annotation is, on screen.
    pub fn rect(&self) -> Rect {
        self.to_screen * self.canvas_rect()
    }

    /// Where the annotation is, on the canvas.
    pub fn canvas_rect(&self) -> Rect {
        Rect::from_min_size((*self.pos).into(), (*self.size).into())
    }

    /// The annotation moves or resizes itself to `rect` on the canvas with this.
    pub fn set_canvas_rect(&mut self, rect: Rect) {
        (*self.pos, *self.size) = (rect.min.into(), rect.size().into());
    }

    pub fn zoom(&self) -> f32 {
        self.to_screen.scaling
    }

    /// An annotation can have a handle in its bottom-right corner to resize it by, no smaller than `min_size`, keeping
    /// `aspect` (its width over its height), if it's given, by whichever side's dragged further.
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

    /// An annotation can draw itself as a card: its background in `fill` (with a shadow, if it's opaque), outlined
    /// while it's selected, with grip lines in the resizing corner (for one that's see-through, only while it's
    /// selected).
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

    /// An annotation can have handles at `ends` (points on the canvas: an arrow's, say), each of which can be dragged.
    /// This returns where they are now. (They're drawn with `mark_ends`, once the annotation's drawn.)
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

    /// While the annotation's selected or hovered, this marks `ends` (points on the canvas) as handles to drag.
    pub fn mark_ends(&self, ends: &[Pos2]) {
        if self.selected || self.hovered {
            for end in ends {
                let stroke = Stroke::new(2.0, self.theme.accent);
                self.painter.circle(self.to_screen * *end, 4.5, self.theme.surface, stroke);
            }
        }
    }

    /// An annotation shows its content in `add`'s `ui` with this: within it (inset a little), scaled to the zoom so its
    /// text is laid out at the size it's shown (so stays sharp). It isn't shown when it's zoomed so far out it couldn't
    /// be read, unless it's being edited. This returns what `add` does (whether editing should carry on), or false.
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
