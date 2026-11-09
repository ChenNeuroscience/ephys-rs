//! `NodesEnum`, the closed list of node types a program is built with, and `nodes_enum!`, which declares it.

use std::any::{Any, TypeId};

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    BufferUsage, Control, Edges, Input, NodeInfo, NodeThread, Output, Pass, PoolStatus, SettingPort, Shared,
    ThreadContext, TreeShake, UiContext,
};

/// The node types a program is built with, as a fieldless enum, one variant each (`NodesEnum::Type`): what the add-node
/// menu offers, say, or a node's type. Made by `nodes_enum!`, in the same order.
pub trait NodeTypeEnum: Copy + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static {
    /// Every type, in the order they're declared (the add-node menu's).
    const ALL: &'static [Self];

    fn info(self) -> &'static NodeInfo;

    /// The type called `name` (its `NodeInfo::name`), if there is one.
    fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.info().name == name)
    }
}

/// The closed set of node types a program is built with, as an enum with one variant per type. Implemented
/// by `nodes_enum!`, and forwards each `Node` method to the variant's type: enum dispatch, as the `enum_dispatch` crate
/// does it, by hand, as `Node` (with its associated types and consts) can't be a trait object.
#[allow(clippy::type_complexity)]
pub trait NodesEnum: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The edge types its node types use.
    type Edges: Edges;

    /// Its node types, without their settings.
    type Type: NodeTypeEnum;

    /// A node of type `node_type`, with default settings.
    fn create(node_type: Self::Type) -> Self;
    fn node_type(&self) -> Self::Type;
    fn window_size(&self) -> [f32; 2];
    fn show_in_basic_default(&self) -> bool;
    /// The global settings its node types read, each type once, in the order of the node types that first name them.
    fn global_settings_types() -> Vec<crate::core::global_settings::GlobalSettingsType>;
    fn inputs(&self) -> Vec<Input<Self::Edges>>;
    fn outputs(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<Output<Self::Edges>>;
    fn warnings(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<String>;
    fn settings(&self) -> Vec<SettingPort<Self::Edges>>;
    fn set_setting(&mut self, index: usize, value: <Self::Edges as Edges>::Setting);
    fn controls(&self) -> Vec<Control<Self::Edges>>;
    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Self::Edges>);
    fn gui_background(&mut self, context: &mut UiContext<Self::Edges>) -> Option<std::time::Instant>;
    /// Its `UiState` as it starts.
    fn new_ui_state(&self) -> Box<dyn Any>;
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake;
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage;
    fn triggers(&self, setting: usize) -> &'static [super::TriggerKind<Self::Edges>];
    fn on_trigger(
        &mut self,
        trigger: <Self::Edges as Edges>::Trigger,
        context: &mut UiContext<Self::Edges>,
        triggers: &crate::core::triggers::ControlTriggers<<Self::Edges as Edges>::Trigger>,
    );
    /// Whether the node carries state from one pass to the next.
    fn has_state(&self) -> bool;
    /// Why it does (see `PassState::WHY`).
    fn state_reason(&self) -> &'static str;
    /// Whether it carries its state over, or it's only working memory (see `PassState::CARRIED_OVER`).
    fn state_carried_over(&self) -> bool;
    fn initial_state(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Box<dyn Any + Send>;
    fn shared(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Box<dyn Shared>;
    /// `state` is the node's state, if it has any (as made by `initial_state`).
    fn process(&self, pass: &mut Pass<Self::Edges>, state: Option<&mut (dyn Any + Send)>);
    fn prepare_to_record(
        &self,
        inputs: &[Option<<Self::Edges as Edges>::DataStream>],
        controls: &[Option<<Self::Edges as Edges>::Setting>],
        base_directory: Option<&crate::core::base_directory::Place>,
        start: &chrono::DateTime<chrono::Local>,
        triggers: &crate::core::triggers::ControlTriggers<<Self::Edges as Edges>::Trigger>,
    ) -> Option<Box<dyn Any + Send>>;
    fn threads(&self) -> Vec<NodeThread>;
    fn thread_ui(&self, thread: usize, ui: &mut egui::Ui, context: &ThreadContext);
    fn thread_pool(&self, thread: usize, context: &ThreadContext) -> Option<PoolStatus>;

    fn info(&self) -> &'static NodeInfo {
        self.node_type().info()
    }
}

/// Whether `S` is real state, rather than `()` for none.
pub fn is_state<S: 'static>() -> bool {
    TypeId::of::<S>() != TypeId::of::<()>()
}

/// Declares the enum of node types a program is built with, and implements `NodesEnum` for it, and the enum of just
/// their types, named after it, implementing `NodeTypeEnum`:
///
/// ```ignore
/// nodes_enum! {
///     edges: Edges;
///     pub enum Nodes {
///         Gain(gain::Gain),
///         Writer(writer::Writer),
///     }
///     pub enum NodeType;
/// }
/// ```
///
/// where `Edges` was declared with `edge_types!`. Saved graphs name each node by its variant, so renaming a
/// variant breaks existing files.
///
/// That makes the enum, a variant holding a node of each type, and `NodesEnum` for it, each of whose methods is a
/// `match` with an arm for each variant, handing on to that type's `Node`; and the enum of types, with the same
/// variants, holding nothing:
///
/// ```ignore
/// #[derive(Clone, PartialEq, Serialize, Deserialize)]
/// #[serde(tag = "type", content = "settings")]
/// pub enum Nodes {
///     Gain(gain::Gain),
///     Writer(writer::Writer),
/// }
///
/// #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
/// pub enum NodeType {
///     Gain,
///     Writer,
/// }
///
/// impl NodesEnum for Nodes {
///     type Edges = Edges;
///     type Type = NodeType;
///
///     fn process(&self, pass: &mut Pass<Edges>, state: Option<&mut (dyn Any + Send)>) {
///         match self {
///             Nodes::Gain(n) => Node::process(n, pass, /* its state, as its own type */),
///             Nodes::Writer(n) => Node::process(n, pass, /* … */),
///         }
///     }
///     // … and so on, for each method
/// }
/// ```
///
/// So there's no dynamic dispatch: each call is a branch on which variant it is, then a call to the type's own method
/// (see `NodesEnum`).
#[macro_export]
macro_rules! nodes_enum {
    (
        edges: $edges:ty;
        $(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident($ty:ty)),+ $(,)? }
        $(#[$type_meta:meta])* $type_vis:vis enum $type_name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "settings")]
        $vis enum $name {
            $($variant($ty)),+
        }

        $(#[$type_meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        $type_vis enum $type_name {
            $($variant),+
        }

        impl $crate::core::engine::NodeTypeEnum for $type_name {
            const ALL: &'static [Self] = &[$($type_name::$variant),+];

            fn info(self) -> &'static $crate::core::engine::NodeInfo {
                match self { $($type_name::$variant => &<$ty as $crate::core::engine::Node<$edges>>::INFO),+ }
            }
        }

        impl $crate::core::engine::NodesEnum for $name {
            type Edges = $edges;
            type Type = $type_name;

            fn create(node_type: $type_name) -> Self {
                match node_type { $($type_name::$variant => $name::$variant(<$ty>::default())),+ }
            }

            fn node_type(&self) -> $type_name {
                match self { $($name::$variant(_) => $type_name::$variant),+ }
            }

            fn window_size(&self) -> [f32; 2] {
                match self { $($name::$variant(_) => <$ty as $crate::core::engine::Node<$edges>>::WINDOW_SIZE),+ }
            }

            fn show_in_basic_default(&self) -> bool {
                match self { $($name::$variant(_) => <$ty as $crate::core::engine::Node<$edges>>::SHOW_IN_BASIC_DEFAULT),+ }
            }

            fn global_settings_types() -> Vec<$crate::core::global_settings::GlobalSettingsType> {
                let mut types = Vec::new();
                $(
                    <<$ty as $crate::core::engine::Node<$edges>>::GlobalSettings
                        as $crate::core::global_settings::GlobalSettingsSet>::types(&mut types);
                )+
                types
            }

            fn inputs(&self) -> Vec<$crate::core::engine::Input<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::inputs(n)),+ }
            }

            fn outputs(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
            ) -> Vec<$crate::core::engine::Output<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::outputs(n, inputs)),+ }
            }

            fn warnings(&self, inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>]) -> Vec<String> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::warnings(n, inputs)),+ }
            }

            fn settings(&self) -> Vec<$crate::core::engine::SettingPort<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::settings(n)),+ }
            }

            fn set_setting(&mut self, index: usize, value: <$edges as $crate::core::engine::Edges>::Setting) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::set_setting(n, index, value)),+ }
            }

            fn controls(&self) -> Vec<$crate::core::engine::Control<$edges>> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::controls(n)),+ }
            }

            fn gui_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::core::engine::UiContext<$edges>) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::gui_ui(n, ui, context)),+ }
            }

            fn gui_background(
                &mut self,
                context: &mut $crate::core::engine::UiContext<$edges>,
            ) -> Option<std::time::Instant> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::gui_background(n, context)),+ }
            }

            fn new_ui_state(&self) -> Box<dyn std::any::Any> {
                match self {
                    $($name::$variant(_) => {
                        Box::new(<<$ty as $crate::core::engine::Node<$edges>>::UiState as Default>::default())
                    }),+
                }
            }

            fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> $crate::core::engine::TreeShake {
                match self {
                    $($name::$variant(n) => $crate::core::engine::Node::evaluate_tree_shake(n, inputs, outputs)),+
                }
            }

            fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> $crate::core::engine::BufferUsage {
                match self {
                    $($name::$variant(n) => $crate::core::engine::Node::evaluate_buffer_usage(n, inputs, outputs)),+
                }
            }

            fn triggers(&self, setting: usize) -> &'static [$crate::core::engine::TriggerKind<$edges>] {
                match self { $($name::$variant(n) => $crate::core::engine::Node::triggers(n, setting)),+ }
            }


            fn on_trigger(
                &mut self,
                trigger: <$edges as $crate::core::engine::Edges>::Trigger,
                context: &mut $crate::core::engine::UiContext<$edges>,
                triggers: &$crate::core::triggers::ControlTriggers<<$edges as $crate::core::engine::Edges>::Trigger>,
            ) {
                match self {
                    $($name::$variant(n) => $crate::core::engine::Node::on_trigger(n, trigger, context, triggers)),+
                }
            }

            fn has_state(&self) -> bool {
                match self {
                    $($name::$variant(_) => {
                        $crate::core::engine::is_state::<<$ty as $crate::core::engine::Node<$edges>>::State>()
                    }),+
                }
            }

            fn state_carried_over(&self) -> bool {
                match self {
                    $($name::$variant(_) => {
                        <<$ty as $crate::core::engine::Node<$edges>>::State as $crate::core::engine::PassState>::CARRIED_OVER
                    }),+
                }
            }

            fn state_reason(&self) -> &'static str {
                match self {
                    $($name::$variant(_) => {
                        <<$ty as $crate::core::engine::Node<$edges>>::State as $crate::core::engine::PassState>::WHY
                    }),+
                }
            }

            fn initial_state(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
            ) -> Box<dyn std::any::Any + Send> {
                match self { $($name::$variant(n) => Box::new($crate::core::engine::Node::initial_state(n, inputs))),+ }
            }

            fn shared(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
            ) -> Box<dyn $crate::core::engine::Shared> {
                match self { $($name::$variant(n) => Box::new($crate::core::engine::Node::shared(n, inputs))),+ }
            }

            fn process(
                &self,
                pass: &mut $crate::core::engine::Pass<$edges>,
                state: Option<&mut (dyn std::any::Any + Send)>,
            ) {
                match self {
                    $($name::$variant(n) => {
                        let state = state.and_then(|s| s.downcast_mut::<<$ty as $crate::core::engine::Node<$edges>>::State>());
                        match state {
                            Some(state) => $crate::core::engine::Node::process(n, pass, state),
                            None => $crate::core::engine::Node::process(n, pass, &mut Default::default()),
                        }
                    }),+
                }
            }

            fn prepare_to_record(
                &self,
                inputs: &[Option<<$edges as $crate::core::engine::Edges>::DataStream>],
                controls: &[Option<<$edges as $crate::core::engine::Edges>::Setting>],
                base_directory: Option<&$crate::core::base_directory::Place>,
                start: &chrono::DateTime<chrono::Local>,
                triggers: &$crate::core::triggers::ControlTriggers<<$edges as $crate::core::engine::Edges>::Trigger>,
            ) -> Option<Box<dyn std::any::Any + Send>> {
                match self {
                    $($name::$variant(n) => $crate::core::engine::Node::prepare_to_record(n, inputs, controls, base_directory, start, triggers)),+
                }
            }

            fn threads(&self) -> Vec<$crate::core::engine::NodeThread> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::<$edges>::threads(n)),+ }
            }

            fn thread_ui(
                &self,
                thread: usize,
                ui: &mut eframe::egui::Ui,
                context: &$crate::core::engine::ThreadContext,
            ) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::<$edges>::thread_ui(n, thread, ui, context)),+ }
            }

            fn thread_pool(
                &self,
                thread: usize,
                context: &$crate::core::engine::ThreadContext,
            ) -> Option<$crate::core::engine::PoolStatus> {
                match self { $($name::$variant(n) => $crate::core::engine::Node::<$edges>::thread_pool(n, thread, context)),+ }
            }
        }
    };
}
