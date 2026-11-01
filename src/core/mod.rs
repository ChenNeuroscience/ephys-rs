//! The core: the graph engine (`engine`), annotations (`annotation`), themes (`theme`), the log (`console`)
//! and the GUI (`gui`).
//! None of it knows about particular node, edge or annotation types, or themes: the program supplies those
//! (see main.rs), declared with `edge_types!`, `node_set!` and `annotation_set!`, through `Components`.

pub mod annotation;
pub mod console;
pub mod engine;
pub mod gui;
pub mod theme;

/// What a program is built from: its node types (which bring their edge types), annotation types and themes.
pub trait Components: 'static {
    type Nodes: engine::NodeSet;
    type Annotations: annotation::AnnotationSet;
    /// The first is the default.
    const THEMES: &'static [theme::Theme];
}
