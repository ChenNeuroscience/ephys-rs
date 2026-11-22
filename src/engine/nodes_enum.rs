//! A program is built with a closed list of node types, which it declares with `nodes_enum!`. The engine reaches each
//! node through `NodesEnum`, which that macro implements.

use std::any::{Any, TypeId};

use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    BufferUsage, CommandContext, CommandSource, Edges, NodeInfo, NodeThread, PassBuffers, PassContext, PoolStatus,
    Port, PortRole, Prepare, PrepareContext, Shared, ThreadContext, TreeShake, UiContext,
};

/// The app sometimes needs a node's type without the node: what the add-node menu offers, say. `nodes_enum!` makes a
/// fieldless enum of the program's node types for that, one variant each, in the same order (`NodesEnum::Type`), and
/// implements this for it.
pub trait NodeTypeEnum: Copy + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static {
    /// Every type, in the order the program declares them (which the add-node menu keeps).
    const ALL: &'static [Self];

    /// The type's name, category and description (its `Node::INFO`).
    fn info(self) -> &'static NodeInfo;

    /// The keys the type's windows answer to (its `Node::SHORTCUTS`).
    fn shortcuts(self) -> &'static [super::Shortcut];

    /// The type called `name` (by its `NodeInfo::name`), if there is one.
    fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.info().name == name)
    }
}

/// The engine holds a program's nodes as an enum with one variant per node type, which `nodes_enum!` makes and
/// implements this for. Each method hands on to the variant's type's `Node` method: enum dispatch, as the
/// `enum_dispatch` crate does it, by hand, as `Node` (with its associated types and consts) can't be a trait object.
/// Where a method has no comment of its own, see `Node`'s.
#[allow(clippy::type_complexity)]
pub trait NodesEnum: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The program's node types all use these edge types.
    type Edges: Edges;

    /// The program's node types, without their settings (see `NodeTypeEnum`).
    type Type: NodeTypeEnum;

    /// The add-node menu makes a node of type `node_type`, with default settings, with this.
    fn create(node_type: Self::Type) -> Self;
    fn node_type(&self) -> Self::Type;
    fn window_size(&self) -> [f32; 2];
    fn show_in_basic_default(&self) -> bool;
    /// Settings… has a tab for each type of global settings the program's node types read: each type once, in the order
    /// of the node types that first name them.
    fn global_settings_types() -> Vec<crate::global_settings::GlobalSettingsType>;
    /// The node's `DataStreamIn` ports, in declaration order, for the graph (see `Node::Ports`).
    fn list_ports_data_stream_in(&self) -> Vec<Port<Self::Edges>>;
    /// The node's `DataStreamOut` ports, in declaration order, for the graph. Each port includes the stream on it,
    /// computed from the streams on the node's inputs (see `Node::output_streams`).
    fn list_ports_data_stream_out(
        &self,
        inputs: &[Option<<Self::Edges as Edges>::DataStream>],
    ) -> Vec<Port<Self::Edges>>;
    fn warnings(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Vec<String>;
    /// The node's ports of `role`, in declaration order (which is how the graph numbers them). A node's ports are
    /// fixed by its type, so this needs no input streams, and the `DataStreamOut` ports have no stream filled in. Use
    /// this for what a port is (its kind and id); `NodeState` has what a port currently carries.
    fn ports_of(&self, role: PortRole) -> Vec<Port<Self::Edges>>;
    /// The ids of the node's ports of `role`, in declaration order. Saved graphs identify ports by these ids (see
    /// `Port::id`).
    fn port_ids(&self, role: PortRole) -> Vec<&'static str> {
        self.ports_of(role).iter().map(Port::id).collect()
    }
    /// The node's ValCmd ports, for the graph: its `ValOutCmdIn` ports, then its `ValInCmdOut` and `ValIn` ports, each
    /// group in declaration order (see `Node::Ports`).
    fn list_ports_val_cmd(&self) -> Vec<Port<Self::Edges>>;
    /// The current value at the node's `ValOutCmdIn` port number `port_idx` (see `Node::val_out`).
    fn val_out(&self, port_idx: usize) -> <Self::Edges as Edges>::PublishedValue;
    /// Delivers `command` to the node, converted to the node's own `CmdIn` (see `Node::on_command`). If the node doesn't
    /// accept the command, nothing is delivered and the command is returned as the error.
    fn on_command(
        &mut self,
        from: CommandSource,
        command: <Self::Edges as Edges>::Command,
        context: &mut CommandContext<Self::Edges>,
    ) -> Result<(), <Self::Edges as Edges>::Command>;
    fn clear_transient_state(&mut self);
    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<Self::Edges>);
    /// The app keeps the node's `UiState`, starting from this.
    fn new_ui_state(&self) -> Box<dyn Any>;
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], recording: bool) -> TreeShake;
    /// How the node uses its buffers, for the plan. This follows from the node's data port declarations (see
    /// `BufferUsage`).
    fn buffer_usage(&self) -> BufferUsage;
    /// A node with state (other than `()`) carries it from one pass to the next, and passes wait for each other there.
    /// This says whether the node has state.
    fn has_state(&self) -> bool;
    /// The dev tools say why the node has state (see `PassState::WHY`).
    fn state_reason(&self) -> &'static str;
    /// Each worker starts with this as its state for the node (the `WorkerState`'s `Default`), unless the node made one
    /// (see `Node::prepare_for_snapshot`).
    fn new_worker_state(&self) -> Box<dyn Any + Send>;
    fn prepare_for_snapshot(
        &self,
        context: &PrepareContext<Self::Edges, Self>,
    ) -> Prepare<Box<dyn Any + Send>, Box<dyn Any + Send>, Self>;
    /// `state` and `proto_state` are the node's state, as its own type (see `Node::adopt_snapshot`).
    fn adopt_snapshot(&self, state: &mut (dyn Any + Send), proto_state: &mut (dyn Any + Send));
    fn shared(&self, inputs: &[Option<<Self::Edges as Edges>::DataStream>]) -> Box<dyn Shared>;
    /// Runs the node for one pass on `buffers`, with the node's state and the worker's state as
    /// `prepare_for_snapshot` made them (`None` if there is none). If the node lacks the state it needs, because it
    /// failed to prepare, the node does nothing and this returns false.
    fn process(
        &self,
        pass: &mut PassContext<Self::Edges>,
        buffers: PassBuffers<'_, Self::Edges>,
        state: Option<&mut (dyn Any + Send)>,
        worker_state: Option<&mut (dyn Any + Send)>,
    ) -> bool;
    fn threads(&self) -> Vec<NodeThread>;
    fn thread_ui(&self, thread_idx: usize, ui: &mut egui::Ui, context: &ThreadContext);
    fn thread_pool(&self, thread_idx: usize, context: &ThreadContext) -> Option<PoolStatus>;

    fn info(&self) -> &'static NodeInfo {
        self.node_type().info()
    }
}

/// A node type with no state of a kind says so with `()`. This says whether `S` is real state.
pub fn is_state<S: 'static>() -> bool {
    TypeId::of::<S>() != TypeId::of::<()>()
}

/// A program declares the node types it's built with like this, which makes the enum of them, implementing
/// `NodesEnum`, and the enum of just their types, named as the program says, implementing `NodeTypeEnum`:
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
/// Here `Edges` was declared with `edge_types!`. Saved graphs name each node by its variant, so renaming a variant
/// breaks existing files (unless a config patch renames it: see `crate::config_patching`).
///
/// The macro makes the enum, with a variant holding a node of each type, and `NodesEnum` for it, each of whose methods
/// is a `match` with an arm for each variant, handing on to that type's `Node`; and the enum of types, with the same
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
///     fn process(&self, pass: &mut PassContext<Edges>, buffers: PassBuffers<Edges>, state: Option<&mut (dyn Any + Send)>) {
///         match self {
///             Nodes::Gain(n) => Node::process(n, pass, /* the buffers and state, converted to the node's types */),
///             Nodes::Writer(n) => Node::process(n, pass, /* … */),
///         }
///     }
///     // … and so on, for each method
/// }
/// ```
///
/// So there's no dynamic dispatch: each call is a branch on the variant, then a call to the type's own method (see
/// `NodesEnum`).
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

        impl $crate::engine::NodeTypeEnum for $type_name {
            const ALL: &'static [Self] = &[$($type_name::$variant),+];

            fn info(self) -> &'static $crate::engine::NodeInfo {
                match self { $($type_name::$variant => &<$ty as $crate::engine::Node<$edges>>::INFO),+ }
            }

            fn shortcuts(self) -> &'static [$crate::engine::Shortcut] {
                match self { $($type_name::$variant => <$ty as $crate::engine::Node<$edges>>::SHORTCUTS),+ }
            }
        }

        impl $crate::engine::NodesEnum for $name {
            type Edges = $edges;
            type Type = $type_name;

            fn create(node_type: $type_name) -> Self {
                match node_type { $($type_name::$variant => $name::$variant(<$ty>::default())),+ }
            }

            fn node_type(&self) -> $type_name {
                match self { $($name::$variant(_) => $type_name::$variant),+ }
            }

            fn window_size(&self) -> [f32; 2] {
                match self { $($name::$variant(_) => <$ty as $crate::engine::Node<$edges>>::WINDOW_SIZE),+ }
            }

            fn show_in_basic_default(&self) -> bool {
                match self { $($name::$variant(_) => <$ty as $crate::engine::Node<$edges>>::SHOW_IN_BASIC_DEFAULT),+ }
            }

            fn global_settings_types() -> Vec<$crate::global_settings::GlobalSettingsType> {
                let mut types = Vec::new();
                $(
                    <<$ty as $crate::engine::Node<$edges>>::GlobalSettings
                        as $crate::global_settings::GlobalSettingsSet>::types(&mut types);
                )+
                types
            }

            fn list_ports_data_stream_in(&self) -> Vec<$crate::engine::Port<$edges>> {
                match self {
                    $($name::$variant(_) => {
                        <$crate::engine::DataStreamInOf<$ty, $edges> as $crate::engine::DataStreamInPorts<$edges>>::ports()
                    }),+
                }
            }

            fn list_ports_data_stream_out(
                &self,
                inputs: &[Option<<$edges as $crate::engine::Edges>::DataStream>],
            ) -> Vec<$crate::engine::Port<$edges>> {
                match self {
                    $($name::$variant(n) => {
                        let inputs = <$crate::engine::DataStreamInOf<$ty, $edges> as $crate::engine::DataStreamInPorts<$edges>>::of(inputs);
                        let outputs: $crate::engine::DataStreamOutOf<$ty, $edges> =
                            $crate::engine::Node::<$edges>::output_streams(n, &inputs);
                        $crate::engine::DataStreamOutPorts::<$edges>::ports(&outputs)
                    }),+
                }
            }

            fn warnings(&self, inputs: &[Option<<$edges as $crate::engine::Edges>::DataStream>]) -> Vec<String> {
                match self {
                    $($name::$variant(n) => {
                        $crate::engine::Node::<$edges>::warnings(n, &<$crate::engine::DataStreamInOf<$ty, $edges> as $crate::engine::DataStreamInPorts<$edges>>::of(inputs))
                    }),+
                }
            }

            fn ports_of(&self, role: $crate::engine::PortRole) -> Vec<$crate::engine::Port<$edges>> {
                use $crate::engine::PortRole;
                match self {
                    $($name::$variant(_) => {
                        let ports = match role {
                            PortRole::DataStreamIn => <$crate::engine::DataStreamInOf<$ty, $edges>
                                as $crate::engine::DataStreamInPorts<$edges>>::ports(),
                            PortRole::DataStreamOut => $crate::engine::DataStreamOutPorts::<$edges>::ports(
                                &<$crate::engine::DataStreamOutOf<$ty, $edges> as Default>::default(),
                            ),
                            PortRole::ValOutCmdIn => <$crate::engine::ValOutCmdInOf<$ty, $edges>
                                as $crate::engine::ValOutCmdInPorts<$edges>>::ports(),
                            PortRole::ValInCmdOut | PortRole::ValIn => <<$ty as $crate::engine::Node<$edges>>::Ports
                                as $crate::engine::PortSet<$edges>>::val_in_ports(),
                        };
                        ports.into_iter().filter(|port| port.role() == role).collect()
                    }),+
                }
            }

            fn list_ports_val_cmd(&self) -> Vec<$crate::engine::Port<$edges>> {
                match self {
                    $($name::$variant(_) => {
                        let mut ports = <$crate::engine::ValOutCmdInOf<$ty, $edges>
                            as $crate::engine::ValOutCmdInPorts<$edges>>::ports();
                        ports.extend(<<$ty as $crate::engine::Node<$edges>>::Ports
                            as $crate::engine::PortSet<$edges>>::val_in_ports());
                        ports
                    }),+
                }
            }

            fn val_out(&self, port_idx: usize) -> <$edges as $crate::engine::Edges>::PublishedValue {
                match self {
                    $($name::$variant(n) => {
                        let values = $crate::engine::Node::<$edges>::val_out(n);
                        let mut values = $crate::engine::ValOutCmdInPorts::<$edges>::values(values).into_iter();
                        values.nth(port_idx).expect("the graph asks only for the settings a node declares")
                    }),+
                }
            }

            #[allow(unreachable_code, unused_variables, reason = "a program without commands has none to deliver")]
            fn on_command(
                &mut self,
                from: $crate::engine::CommandSource,
                command: <$edges as $crate::engine::Edges>::Command,
                context: &mut $crate::engine::CommandContext<$edges>,
            ) -> Result<(), <$edges as $crate::engine::Edges>::Command> {
                match self {
                    $($name::$variant(n) => {
                        type CmdIn = $crate::engine::CmdInOf<$ty, $edges>;
                        let command = <CmdIn as $crate::engine::CommandSet<$edges>>::from_command(command)?;
                        $crate::engine::Node::<$edges>::on_command(n, from, command, context);
                        Ok(())
                    }),+
                }
            }

            fn clear_transient_state(&mut self) {
                match self { $($name::$variant(n) => $crate::engine::Node::<$edges>::clear_transient_state(n)),+ }
            }

            fn on_gui_tick(
                &mut self,
                ui: Option<&mut eframe::egui::Ui>,
                context: &mut $crate::engine::UiContext<$edges>,
            ) {
                match self { $($name::$variant(n) => $crate::engine::Node::<$edges>::on_gui_tick(n, ui, context)),+ }
            }

            fn new_ui_state(&self) -> Box<dyn std::any::Any> {
                match self {
                    $($name::$variant(_) => {
                        Box::new(<<$ty as $crate::engine::Node<$edges>>::UiState as Default>::default())
                    }),+
                }
            }

            fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool], recording: bool) -> $crate::engine::TreeShake {
                match self {
                    $($name::$variant(n) => $crate::engine::Node::<$edges>::evaluate_tree_shake(n, inputs, outputs, recording)),+
                }
            }

            fn buffer_usage(&self) -> $crate::engine::BufferUsage {
                match self {
                    $($name::$variant(_) => $crate::engine::BufferUsage {
                        in_place: <$crate::engine::DataStreamOutOf<$ty, $edges>
                            as $crate::engine::DataStreamOutPorts<$edges>>::IN_PLACE.to_vec(),
                        takes: <$crate::engine::DataStreamInOf<$ty, $edges>
                            as $crate::engine::DataStreamInPorts<$edges>>::TAKES.to_vec(),
                    }),+
                }
            }

            fn has_state(&self) -> bool {
                match self {
                    $($name::$variant(_) => {
                        $crate::engine::is_state::<<$ty as $crate::engine::Node<$edges>>::State>()
                    }),+
                }
            }

            fn new_worker_state(&self) -> Box<dyn std::any::Any + Send> {
                match self {
                    $($name::$variant(_) => {
                        Box::new(<<$ty as $crate::engine::Node<$edges>>::WorkerState as Default>::default())
                    }),+
                }
            }

            fn state_reason(&self) -> &'static str {
                match self {
                    $($name::$variant(_) => {
                        <<$ty as $crate::engine::Node<$edges>>::State as $crate::engine::PassState>::WHY
                    }),+
                }
            }

            fn prepare_for_snapshot(
                &self,
                context: &$crate::engine::PrepareContext<$edges, Self>,
            ) -> $crate::engine::Prepare<Box<dyn std::any::Any + Send>, Box<dyn std::any::Any + Send>, Self> {
                match self {
                    $($name::$variant(n) => {
                        #[allow(unreachable_patterns)]
                        let context = context.of(|previous| match previous {
                            $name::$variant(previous) => Some(previous),
                            _ => None,
                        });
                        $crate::engine::Node::<$edges>::prepare_for_snapshot(n, &context).map(
                            |state| Box::new(state) as Box<dyn std::any::Any + Send>,
                            |state| Box::new(state) as Box<dyn std::any::Any + Send>,
                            $name::$variant,
                        )
                    }),+
                }
            }

            fn adopt_snapshot(
                &self,
                state: &mut (dyn std::any::Any + Send),
                proto_state: &mut (dyn std::any::Any + Send),
            ) {
                match self {
                    $($name::$variant(n) => {
                        type State = <$ty as $crate::engine::Node<$edges>>::State;
                        if let (Some(state), Some(proto_state)) =
                            (state.downcast_mut::<State>(), proto_state.downcast_mut::<State>())
                        {
                            $crate::engine::Node::<$edges>::adopt_snapshot(n, state, proto_state);
                        }
                    }),+
                }
            }

            fn shared(
                &self,
                inputs: &[Option<<$edges as $crate::engine::Edges>::DataStream>],
            ) -> Box<dyn $crate::engine::Shared> {
                match self {
                    $($name::$variant(n) => {
                        let inputs = <$crate::engine::DataStreamInOf<$ty, $edges> as $crate::engine::DataStreamInPorts<$edges>>::of(inputs);
                        Box::new($crate::engine::Node::<$edges>::shared(n, &inputs))
                    }),+
                }
            }

            fn process(
                &self,
                pass: &mut $crate::engine::PassContext<$edges>,
                buffers: $crate::engine::PassBuffers<'_, $edges>,
                state: Option<&mut (dyn std::any::Any + Send)>,
                worker_state: Option<&mut (dyn std::any::Any + Send)>,
            ) -> bool {
                match self {
                    $($name::$variant(n) => {
                        type State = <$ty as $crate::engine::Node<$edges>>::State;
                        type WorkerState = <$ty as $crate::engine::Node<$edges>>::WorkerState;
                        // A node without state of a kind has `()` for it.
                        let (mut no_state, mut no_worker_state) = ((), ());
                        let state = state.unwrap_or(&mut no_state).downcast_mut::<State>();
                        let worker_state = worker_state.unwrap_or(&mut no_worker_state).downcast_mut::<WorkerState>();
                        match (state, worker_state) {
                            (Some(state), Some(worker_state)) => {
                                let $crate::engine::PassBuffers { inputs, taken, outputs } = buffers;
                                let inputs = <$crate::engine::DataStreamInOf<$ty, $edges>
                                    as $crate::engine::DataStreamInPorts<$edges>>::view(inputs, taken);
                                let outputs = <$crate::engine::DataStreamOutOf<$ty, $edges>
                                    as $crate::engine::DataStreamOutPorts<$edges>>::view(outputs);
                                $crate::engine::Node::<$edges>::process(n, pass, inputs, outputs, state, worker_state);
                                true
                            }
                            _ => false,
                        }
                    }),+
                }
            }

            fn threads(&self) -> Vec<$crate::engine::NodeThread> {
                match self { $($name::$variant(n) => $crate::engine::Node::<$edges>::threads(n)),+ }
            }

            fn thread_ui(
                &self,
                thread_idx: usize,
                ui: &mut eframe::egui::Ui,
                context: &$crate::engine::ThreadContext,
            ) {
                match self { $($name::$variant(n) => $crate::engine::Node::<$edges>::thread_ui(n, thread_idx, ui, context)),+ }
            }

            fn thread_pool(
                &self,
                thread_idx: usize,
                context: &$crate::engine::ThreadContext,
            ) -> Option<$crate::engine::PoolStatus> {
                match self { $($name::$variant(n) => $crate::engine::Node::<$edges>::thread_pool(n, thread_idx, context)),+ }
            }
        }
    };
}
