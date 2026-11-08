//! `NodesEnum`, the closed list of node types a program is built with, and `nodes_enum!`, which declares it.

use std::any::{Any, TypeId};

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    BufferUsage, Control, Edges, Input, NodeInfo, NodeThread, Output, Pass, PoolStatus, SettingPort, ThreadContext,
    TreeShake, UiContext,
};

/// The closed set of node types a program is built with, as an enum with one variant per type. Implemented
/// by `nodes_enum!`, and forwards each `Node` method to the variant's type.
#[allow(clippy::type_complexity)]
pub trait NodesEnum: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The edge types its node types use.
    type Edges: Edges;

    /// Indexed by type index.
    const TYPES: &'static [NodeInfo];

    /// A node of the given type, with default settings.
    fn create(type_index: usize) -> Self;
    fn type_index(&self) -> usize;
    fn has_custom_ui(&self) -> bool;
    fn show_in_basic_default(&self) -> bool;
    fn inputs(&self) -> Vec<Input<Self::Edges>>;
    fn outputs(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<Output<Self::Edges>>;
    fn warnings(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<String>;
    fn settings(&self) -> Vec<SettingPort<Self::Edges>>;
    fn set_setting(&mut self, index: usize, value: <Self::Edges as Edges>::Setting);
    fn controls(&self) -> Vec<Control<Self::Edges>>;
    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Self::Edges>);
    fn custom_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Self::Edges>);
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake;
    fn evaluate_buffer_usage(&self, inputs: usize, outputs: usize) -> BufferUsage;
    fn triggers(&self, setting: usize) -> &'static [super::TriggerKind<Self::Edges>];
    fn on_trigger(&mut self, trigger: <Self::Edges as Edges>::Trigger);
    /// Whether the node carries state from one pass to the next.
    fn has_state(&self) -> bool;
    /// Why it does (see `PassState::WHY`).
    fn state_reason(&self) -> &'static str;
    /// Whether it carries its state over, or it's only working memory (see `PassState::CARRIED_OVER`).
    fn state_carried_over(&self) -> bool;
    fn initial_state(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Box<dyn Any + Send>;
    /// `state` is the node's state, if it has any (as made by `initial_state`).
    fn process(&self, pass: &mut Pass<Self::Edges>, state: Option<&mut (dyn Any + Send)>);
    fn prepare_to_record(
        &self,
        inputs: &[Option<<Self::Edges as Edges>::DataStream>],
        controls: &[Option<<Self::Edges as Edges>::Setting>],
        base_directory: Option<&crate::core::base_directory::Place>,
        triggers: &crate::core::triggers::ControlTriggers<<Self::Edges as Edges>::Trigger>,
    ) -> Option<Box<dyn Any + Send>>;
    fn threads(&self) -> Vec<NodeThread>;
    fn thread_ui(&self, thread: usize, ui: &mut egui::Ui, context: &ThreadContext);
    fn thread_pool(&self, thread: usize, context: &ThreadContext) -> Option<PoolStatus>;

    fn info(&self) -> &'static NodeInfo {
        &Self::TYPES[self.type_index()]
    }
}

/// Whether `S` is real state, rather than `()` for none.
pub fn is_state<S: 'static>() -> bool {
    TypeId::of::<S>() != TypeId::of::<()>()
}

/// Declares the enum of node types a program is built with, and implements `NodesEnum` for it:
///
/// ```ignore
/// nodes_enum! {
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
///
/// That makes the enum, a variant holding a node of each type, and `NodesEnum` for it, each of whose methods is a
/// `match` with an arm for each variant, handing on to that type's `Node`:
///
/// ```ignore
/// #[derive(Clone, PartialEq, Serialize, Deserialize)]
/// #[serde(tag = "type", content = "settings")]
/// pub enum Nodes {
///     Gain(gain::Gain),
///     Writer(writer::Writer),
/// }
///
/// impl NodesEnum for Nodes {
///     type Edges = Edges;
///     const TYPES: &'static [NodeInfo] = &[<gain::Gain as Node<Edges>>::INFO, <writer::Writer as Node<Edges>>::INFO];
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
    (edges: $edges:ty; $(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident($ty:ty)),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "settings")]
        $vis enum $name {
            $($variant($ty)),+
        }

        impl $crate::core::engine::NodesEnum for $name {
            type Edges = $edges;

            const TYPES: &'static [$crate::core::engine::NodeInfo] =
                &[$(<$ty as $crate::core::engine::Node<$edges>>::INFO),+];

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

            fn has_custom_ui(&self) -> bool {
                match self { $($name::$variant(_) => <$ty as $crate::core::engine::Node<$edges>>::HAS_CUSTOM_UI),+ }
            }

            fn show_in_basic_default(&self) -> bool {
                match self { $($name::$variant(_) => <$ty as $crate::core::engine::Node<$edges>>::SHOW_IN_BASIC_DEFAULT),+ }
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

            fn settings_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::core::engine::UiContext<$edges>) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::settings_ui(n, ui, context)),+ }
            }

            fn custom_ui(&mut self, ui: &mut eframe::egui::Ui, context: &mut $crate::core::engine::UiContext<$edges>) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::custom_ui(n, ui, context)),+ }
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


            fn on_trigger(&mut self, trigger: <$edges as $crate::core::engine::Edges>::Trigger) {
                match self { $($name::$variant(n) => $crate::core::engine::Node::on_trigger(n, trigger)),+ }
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
                triggers: &$crate::core::triggers::ControlTriggers<<$edges as $crate::core::engine::Edges>::Trigger>,
            ) -> Option<Box<dyn std::any::Any + Send>> {
                match self {
                    $($name::$variant(n) => $crate::core::engine::Node::prepare_to_record(n, inputs, controls, base_directory, triggers)),+
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
