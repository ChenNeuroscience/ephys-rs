//! Annotations: things on the canvas that aren't part of the graph, like sticky notes. They're saved and undone
//! along with the graph, but take no part in processing.
//!
//! Like node types, annotation types are declared by the program (with `annotation_set!`): each is a struct
//! implementing `AnnotationType`, which draws its own content, toolbar icon and settings. An image pasted onto
//! the canvas becomes an annotation of the first type that takes one (see `AnnotationType::pasted_image`).

use std::collections::BTreeMap;

use eframe::egui::{self, Color32, ColorImage, Painter, Pos2, Rect, UiBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::theme::Colours;

/// The smallest an annotation can be made, in canvas units, unless its type says otherwise.
pub const MIN_SIZE: [f32; 2] = [80.0, 50.0];

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

    /// Its background on the canvas, in the theme's colours if it wants. An opaque one has a shadow.
    fn fill(&self, theme: &Colours) -> Color32;

    /// Draws what it is besides its content, over its background (a border, say), filling `rect` (on screen).
    fn paint(&self, _painter: &Painter, _rect: Rect, _zoom: f32, _theme: &Colours) {}

    /// Whether it can be grabbed (to move, select or edit) at `at`, on screen, within `rect`: by default anywhere
    /// in it, but a box, say, only by its edge, so what's inside it can still be grabbed.
    fn hit(&self, _rect: Rect, _at: Pos2, _zoom: f32) -> bool {
        true
    }

    /// Whether it goes behind other annotations (a box grouping them, say).
    fn behind(&self) -> bool {
        false
    }

    /// Whether it goes above the graph (as most do), rather than beneath it (a box grouping nodes, say).
    fn above_graph(&self) -> bool {
        true
    }

    /// For one that's a line between two points (an arrow, say): the points, within `rect`. The canvas then lets
    /// each be dragged, rather than the rect resized.
    fn points(&self, _rect: Rect) -> Option<[Pos2; 2]> {
        None
    }

    /// Makes it run between two points, returning the rect that now holds it.
    fn set_points(&mut self, points: [Pos2; 2]) -> Rect {
        Rect::from_two_pos(points[0], points[1])
    }

    /// One showing an image pasted onto the canvas, if this type shows images; with its natural size (which the
    /// canvas scales down to fit the view).
    fn pasted_image(_image: &ColorImage) -> Option<(Self, [f32; 2])> {
        None
    }

    /// Whether it's drawn out on the canvas (with its toolbar tool). If not, it's made with `choose` instead.
    const DRAWN: bool = true;

    /// One made by choosing something (an image file, say), and its natural size; `None` if nothing was chosen.
    fn choose() -> Option<(Self, [f32; 2])> {
        None
    }

    /// The width over the height it keeps, if it keeps one (an image, say).
    fn aspect(&self) -> Option<f32> {
        None
    }

    /// Shows its content in `ui` (sized and scaled to fit it on the canvas): as it looks, or for editing while
    /// `editing`. Returns whether editing should carry on.
    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool;

    /// Its settings, for the annotation pane (shown while it's selected).
    fn settings_ui(&mut self, ui: &mut egui::Ui);

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
    fn aspect(&self) -> Option<f32>;
    fn paint(&self, painter: &Painter, rect: Rect, zoom: f32, theme: &Colours);
    fn hit(&self, rect: Rect, at: Pos2, zoom: f32) -> bool;
    fn behind(&self) -> bool;
    fn above_graph(&self) -> bool;
    fn points(&self, rect: Rect) -> Option<[Pos2; 2]>;
    fn set_points(&mut self, points: [Pos2; 2]) -> Rect;
    fn type_index(&self) -> usize;
    fn fill(&self, theme: &Colours) -> Color32;
    /// An annotation of the first type that shows pasted images, and its size.
    fn pasted_image(image: &ColorImage) -> Option<(Self, [f32; 2])>;
    fn content_ui(&mut self, ui: &mut egui::Ui, editing: bool) -> bool;
    fn settings_ui(&mut self, ui: &mut egui::Ui);
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

            fn aspect(&self) -> Option<f32> {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::aspect(a)),+ }
            }

            fn paint(
                &self,
                painter: &eframe::egui::Painter,
                rect: eframe::egui::Rect,
                zoom: f32,
                theme: &$crate::core::theme::Colours,
            ) {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::paint(a, painter, rect, zoom, theme)),+ }
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

            fn points(&self, rect: eframe::egui::Rect) -> Option<[eframe::egui::Pos2; 2]> {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::points(a, rect)),+ }
            }

            fn set_points(&mut self, points: [eframe::egui::Pos2; 2]) -> eframe::egui::Rect {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::set_points(a, points)),+ }
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

            fn pasted_image(image: &eframe::egui::ColorImage) -> Option<(Self, [f32; 2])> {
                $(
                    if let Some((a, size)) = <$ty as $crate::core::annotation::AnnotationType>::pasted_image(image) {
                        return Some(($name::$variant(a), size));
                    }
                )+
                None
            }

            fn content_ui(&mut self, ui: &mut eframe::egui::Ui, editing: bool) -> bool {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::content_ui(a, ui, editing)),+ }
            }


            fn settings_ui(&mut self, ui: &mut eframe::egui::Ui) {
                match self { $($name::$variant(a) => $crate::core::annotation::AnnotationType::settings_ui(a, ui)),+ }
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

    /// Shows the content in `rect` (on screen) at `zoom`, scaled so its text is laid out at the size it's
    /// shown (so stays sharp). Returns whether editing should carry on.
    pub fn content_ui(&mut self, ui: &mut egui::Ui, rect: Rect, zoom: f32, editing: bool) -> bool {
        let inner = rect.shrink(8.0 * zoom);
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        let style = child.style_mut();
        for font in style.text_styles.values_mut() {
            font.size *= zoom;
        }
        style.spacing.item_spacing *= zoom;
        style.spacing.indent *= zoom;
        style.interaction.selectable_labels = false;
        // Below this, text would be too small to read anyway.
        if zoom <= 0.35 && !editing {
            return false;
        }
        self.kind.content_ui(&mut child, editing)
    }
}

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
