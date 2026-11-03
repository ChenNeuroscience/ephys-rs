# Annotations

Annotations are things on the canvas that aren't part of the graph: sticky notes, markdown, images, headings, boxes
and arrows. They're saved with the graph and undone with it, but take no part in processing.

![Drawing a note](../tests/snapshots/graph_drawing_note.png)

An annotation type is a struct implementing [`AnnotationType`](../src/core/annotation.rs), listed in
`annotation_set!` in [`main.rs`](../src/main.rs); the standard ones are in
[`annotations`](../src/standard_components/annotations/mod.rs). The simplest is the
[sticky note](../src/standard_components/annotations/sticky_note.rs).

- `INFO`: its name, description, and size when added without being drawn out; `MIN_SIZE`.
- `paint_icon`: its tool's icon in the graph's toolbar. Drawing with the tool makes one (`drawn`); `DRAWN = false`
  instead asks for one when the tool's chosen (`choose`: an image from a file, say).
- `fill`, `paint`: its background and anything drawn over it (a border), in the theme's colours if it likes.
- `content_ui(ui, editing)`: what it shows, and editing it in place (double-click); `settings_ui`: its settings in the
  annotation pane, while it's selected; `toolbar_ui`: buttons over it while editing.
- `hit`, `behind`, `above_graph`: where it can be grabbed, and what it's drawn in front of (a box goes behind other
  annotations and beneath the graph, and is grabbed by its edge, so what's inside it still can be).
- `points` and `set_points`: for one defined by two points rather than a rectangle (an arrow).
- `aspect`: a fixed aspect ratio, kept as it's resized (an image).
- `pasted_image`: whether an image pasted onto the canvas becomes one of these (the first type that takes it wins).

Images pasted into a markdown note are kept with it in the file, and shown from where egui loads them.
