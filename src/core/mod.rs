//! The core: the graph engine (`engine`), annotations (`annotation`), themes (`theme`), the log (`console`),
//! handing things from passes to windows (`exchange`), threads finishing in the background (`background`), how plots
//! over time lay out time (`time_axis`), quick actions (`quick_action`), and the GUI (`gui`).
//! None of it knows about particular node, edge or annotation types, or themes: the program supplies those
//! (see `example/`), declared with `edge_types!`, `node_set!` and `annotation_set!`, through `Components`.

pub mod annotation;
pub mod background;
pub mod base_directory;
pub mod console;
pub mod crash;
pub mod engine;
pub mod exchange;
pub mod gui;
pub mod layout;
pub mod quick_action;
pub mod theme;
pub mod time_axis;

/// What a program is built from: its node types (which bring their edge types), annotation types, themes, quick
/// actions and base directories.
pub trait Components: 'static {
    type Nodes: engine::NodeSet;
    type Annotations: annotation::AnnotationSet;
    /// The first is the default.
    const THEMES: &'static [theme::Theme];
    /// In the order they're offered.
    const QUICK_ACTIONS: &'static [quick_action::QuickAction<Self::Nodes>] = &[];
    /// Where recordings go (see `base_directory`): the first is the default.
    const BASE_DIRECTORIES: &'static [base_directory::BaseDirectoryType] = &[];
}
