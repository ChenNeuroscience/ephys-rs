//! The `Node` trait implemented by each node type, and `NodeSet`, the list of types a program is built with.

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Edges, Kind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Source,
    Processing,
    Visualisation,
    Recording,
}

impl Category {
    pub const ALL: [Category; 4] =
        [Category::Source, Category::Processing, Category::Visualisation, Category::Recording];

    pub fn name(self) -> &'static str {
        match self {
            Category::Source => "Sources",
            Category::Processing => "Processing",
            Category::Visualisation => "Visualisation",
            Category::Recording => "Recording",
        }
    }
}

/// Static facts about a node type.
pub struct NodeInfo {
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
}

pub struct Input<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::Signal>,
}

pub struct Output<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::Signal>,
    /// What the output produces, or `None` while that can't be known (e.g. its input isn't connected).
    pub signal: Option<E::Signal>,
}

/// A setting that other nodes can edit through a settings edge, with its current value.
pub struct SettingPort<E: Edges> {
    pub name: &'static str,
    pub value: E::Setting,
}

/// A setting of some other node that this one can edit, through a settings edge.
pub struct Control<E: Edges> {
    pub name: &'static str,
    pub kind: Kind<E::Setting>,
}

/// What a node's UI gets to work with besides its own settings.
pub struct Context<'a, E: Edges> {
    /// For each input, the signal connected to it, if any.
    pub inputs: &'a [Option<E::Signal>],
    /// For each control, the current value of the setting it's connected to, if any. Changing a value
    /// edits that setting.
    pub controls: &'a mut [Option<E::Setting>],
}

/// A node type. Implementors are plain settings structs, saved with the graph.
///
/// The ports may depend on the settings, and the output signals on what is connected to the inputs; the
/// graph works these out in order from the sources down (see `Graph::resolve`).
pub trait Node<E: Edges>: Default + Clone + PartialEq + Serialize + DeserializeOwned + 'static {
    const INFO: NodeInfo;

    /// Whether the node's window is a view of its own (a viewer, say) rather than just its settings.
    const HAS_VIEW: bool = false;

    fn inputs(&self) -> Vec<Input<E>> {
        Vec::new()
    }

    /// `inputs` has an entry for each of `self.inputs()`: the signal connected to it, if any.
    fn outputs(&self, inputs: &[Option<E::Signal>]) -> Vec<Output<E>>;

    /// Problems with the configuration, shown on the node.
    fn warnings(&self, _inputs: &[Option<E::Signal>]) -> Vec<String> {
        Vec::new()
    }

    /// Settings that other nodes may edit, with their current values.
    fn settings(&self) -> Vec<SettingPort<E>> {
        Vec::new()
    }

    /// Called with a new value for one of `self.settings()`, of the same kind.
    fn set_setting(&mut self, _index: usize, _value: E::Setting) {}

    fn controls(&self) -> Vec<Control<E>> {
        Vec::new()
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<E>);

    fn view_ui(&mut self, _ui: &mut egui::Ui, _context: &mut Context<E>) {}
}

/// The closed set of node types a program is built with, as an enum with one variant per type. Implemented
/// by `node_set!`, and forwards each `Node` method to the variant's type.
#[allow(clippy::type_complexity)]
pub trait NodeSet: Clone + PartialEq + Serialize + DeserializeOwned + 'static {
    type Edges: Edges;

    /// Indexed by type index.
    const TYPES: &'static [NodeInfo];

    /// A node of the given type, with default settings.
    fn create(type_index: usize) -> Self;
    fn type_index(&self) -> usize;
    fn has_view(&self) -> bool;
    fn inputs(&self) -> Vec<Input<Self::Edges>>;
    fn outputs(&self, inputs: &[Option<<Self::Edges as Edges>::Signal>]) -> Vec<Output<Self::Edges>>;
    fn warnings(&self, inputs: &[Option<<Self::Edges as Edges>::Signal>]) -> Vec<String>;
    fn settings(&self) -> Vec<SettingPort<Self::Edges>>;
    fn set_setting(&mut self, index: usize, value: <Self::Edges as Edges>::Setting);
    fn controls(&self) -> Vec<Control<Self::Edges>>;
    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Self::Edges>);
    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Self::Edges>);

    fn info(&self) -> &'static NodeInfo {
        &Self::TYPES[self.type_index()]
    }
}

/// Declares the enum of node types a program is built with, and implements `NodeSet` for it:
///
/// ```ignore
/// node_set! {
///     edges: Edges;
///     pub enum Nodes {
///         Gain(gain::Gain),
///         Writer(writer::Writer),
///     }
/// }
/// ```
///
/// where `Edges` was declared with `edge_types!`. Saved graphs name each node by its variant, so renaming a
/// variant breaks existing files.
#[macro_export]
macro_rules! node_set {
    (edges: $edges:ty; $(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident($ty:ty)),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "settings")]
        $vis enum $name {
            $($variant($ty)),+
        }

        impl $crate::engine::NodeSet for $name {
            type Edges = $edges;

            const TYPES: &'static [$crate::engine::NodeInfo] =
                &[$(<$ty as $crate::engine::Node<$edges>>::INFO),+];

            #[allow(unused_assignments)]
            fn create(type_index: usize) -> Self {
                let mut i = 0;
                $(
                    if i == type_index {
                        return $name::$variant(<$ty>::default());
                    }
                    i += 1;
                )+
                panic!("no node type {type_index}")
            }

            #[allow(unused_assignments)]
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

            fn has_view(&self) -> bool {
                match self { $($name::$variant(_) => <$ty as $crate::engine::Node<$edges>>::HAS_VIEW),+ }
            }

            fn inputs(&self) -> Vec<$crate::engine::Input<$edges>> {
                match self { $($name::$variant(n) => $crate::engine::Node::inputs(n)),+ }
            }

            fn outputs(
                &self,
                inputs: &[Option<<$edges as $crate::engine::Edges>::Signal>],
            ) -> Vec<$crate::engine::Output<$edges>> {
                match self { $($name::$variant(n) => $crate::engine::Node::outputs(n, inputs)),+ }
            }

            fn warnings(&self, inputs: &[Option<<$edges as $crate::engine::Edges>::Signal>]) -> Vec<String> {
                match self { $($name::$variant(n) => $crate::engine::Node::warnings(n, inputs)),+ }
            }

            fn settings(&self) -> Vec<$crate::engine::SettingPort<$edges>> {
                match self { $($name::$variant(n) => $crate::engine::Node::settings(n)),+ }
            }

            fn set_setting(&mut self, index: usize, value: <$edges as $crate::engine::Edges>::Setting) {
                match self { $($name::$variant(n) => $crate::engine::Node::set_setting(n, index, value)),+ }
            }

            fn controls(&self) -> Vec<$crate::engine::Control<$edges>> {
                match self { $($name::$variant(n) => $crate::engine::Node::controls(n)),+ }
            }

            fn settings_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::engine::Context<$edges>) {
                match self { $($name::$variant(n) => $crate::engine::Node::settings_ui(n, ui, context)),+ }
            }

            fn view_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::engine::Context<$edges>) {
                match self { $($name::$variant(n) => $crate::engine::Node::view_ui(n, ui, context)),+ }
            }
        }
    };
}
