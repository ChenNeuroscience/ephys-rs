//! Each node declares its ports once, on its type, with the `ports!` macro. Each port has a name, one of the five
//! roles (see `PortRole`), and the type of what it carries. The macro generates everything else about the ports from
//! that declaration:
//!
//! - `DataStreamIn` and `DataStreamOut`: structs of the node's data ports, generic over a form (see
//!   `crate::engine::data_ports`). In one form the fields are the ports' streams, which `Node::data_stream_out` uses;
//!   in another they are the ports' buffers, which `Node::process` uses.
//! - `ValOutCmdIn`: a struct of the node's values out, which `Node::val_out` returns. `CmdIn` is an enum of
//!   every command those ports accept, which `Node::on_command` receives (see `crate::engine::val_cmd_ports`).
//! - `ValInCmdOut` and `ValIn`: a handle for each port that reads another node's value (such as
//!   `ValInCmdOut::gains`). The contexts take these handles to read values and send commands.
//! - `Ports`: the node's `Node::Ports`, which gathers the generated types for the engine (see `PortSet`).
//! - `PortEdges`: the edge types that a program's edges must include for the node. The node implements
//!   `impl<E: PortEdges> Node<E> for MyNode`, so the node works in any program that lists the types its ports carry,
//!   and each program lists only the types its nodes need.
//!
//! The graph numbers each role's ports in declaration order. The node itself numbers its `ValInCmdOut` and `ValIn`
//! ports together, because it reads values through both. Saved graphs don't use numbers: they name each port by its
//! field name (see `Port::id`).

use super::{
    CommandSet, DataStreamInPorts, DataStreamOutPorts, Edges, NoCommands, NoPorts, Node, Port, ValOutCmdInPorts,
};

/// The types that `ports!` generates for one node, gathered for the engine. `ports!` implements this trait for the
/// node's `Ports` struct, which the node names as its `Node::Ports`. A role with no ports uses `NoPorts`, and a node
/// with no `ValOutCmdIn` ports uses `NoCommands` for its commands.
pub trait PortSet<E: Edges>: 'static {
    type DataStreamIn: DataStreamInPorts<E>;
    type DataStreamOut: DataStreamOutPorts<E>;
    type ValOutCmdIn: ValOutCmdInPorts<E>;
    type CmdIn: CommandSet<E>;

    /// The node's `ValInCmdOut` and `ValIn` ports, in declaration order, for the graph.
    fn val_in_ports() -> Vec<Port<E>>;
}

/// A node with no ports at all names `NoPorts` as its `Node::Ports`.
impl<E: Edges> PortSet<E> for NoPorts {
    type DataStreamIn = NoPorts;
    type DataStreamOut = NoPorts;
    type ValOutCmdIn = NoPorts;
    type CmdIn = NoCommands;

    fn val_in_ports() -> Vec<Port<E>> {
        Vec::new()
    }
}

/// Declares a node's ports. Each port is a field named after the port, in one of the five roles (see `PortRole`):
///
/// ```ignore
/// ports! {
///     continuous: DataStreamIn<Continuous>,
///     video: DataStreamIn<Video> take,
///     filtered: DataStreamOut<Continuous> in_place_of continuous,
///     thresholds: ValOutCmdIn<Thresholds, (SetThresholds, Detect)>,
///     gains: ValInCmdOut<Gains, (SetGains, Detect)>,
///     recorded: ValIn<Gains> = "recorded gains",
///     after_recording: ValInCmdOut<FileInput, (ProcessFiles,)> = "after recording" side: Right,
/// }
/// ```
///
/// Each port's type parameter is the data stream or value the port carries. A `ValOutCmdIn` or `ValInCmdOut` port
/// also lists the commands it accepts or sends, as a tuple of command types that are in scope (see `HasCommand`). The
/// graph shows a port under its field name, unless a string follows (`= "recorded gains"`). These options can follow
/// a port:
///
/// - `take`: the node takes ownership of a `DataStreamIn` port's buffer (see `BufferUsage::takes`).
/// - `in_place_of`: the node writes a `DataStreamOut` port into the buffer of a `DataStreamIn` port of the same type
///   (see `BufferUsage::in_place`).
/// - `side: Left` or `side: Right`: the side of the node that the graph draws a ValCmd port on (see `Side`).
///
/// The macro generates these items in the node's module:
///
/// - `DataStreamIn` and `DataStreamOut`, if the node has such ports: structs of the ports, generic over a `PortForm`
///   (`Streams` by default), implementing `DataStreamInPorts` and `DataStreamOutPorts`. Each `DataStreamIn` port also
///   gets an associated constant holding the port's number (`DataStreamIn::continuous`).
/// - `ValOutCmdIn`, if the node has such ports: a struct of the ports' values out, implementing
///   `ValOutCmdInPorts`. Each port also gets an associated constant holding the port's number (`ValOutCmdIn::files`),
///   which `CommandSource::Port` compares against.
/// - `CmdIn`: an enum with one variant for each command the `ValOutCmdIn` ports accept (`CmdIn::Detect(Detect)`),
///   implementing `CommandSet`. Only one `ValOutCmdIn` port can accept a given command, so each command appears once.
/// - `ValInCmdOut` and `ValIn`: unit structs holding a handle for each such port, as a constant named after the port
///   (`ValInCmdOut::gains: ValInCmdOutPort<Gains, (SetGains, Detect)>`).
/// - `Ports`, implementing `PortSet`.
/// - `PortEdges`: a trait requiring `HasDataStream<T>`, `HasValue<V>` and `HasCommand<C>` for each type the ports
///   carry, implemented for every `Edges` type that has them. The node implements
///   `impl<E: PortEdges> Node<E> for MyNode`, so the node works in any program whose `edge_types!` lists those types.
///
/// Saved graphs name each port by its role and field name (see `Port::id`), never by the display name. Reordering
/// ports therefore doesn't break existing files, but renaming a field does, unless a config patch renames the edges'
/// ends too.
#[macro_export]
macro_rules! ports {
    // Each port is added to one of four lists: `DataStreamIn`, `DataStreamOut`, `ValOutCmdIn`, and `ValInCmdOut`
    // together with `ValIn` (because the node numbers those two roles together).
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]
        $(#[$m:meta])* $field:ident : DataStreamIn < $ty:ty > $(= $name:literal)? take, $($rest:tt)*
    ) => {
        $crate::ports!(@munch [$($i)* { ($(#[$m])*) $field ($ty) ($($name)?) Take }] [$($o)*] [$($vo)*] [$($vi)*] $($rest)*);
    };
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]
        $(#[$m:meta])* $field:ident : DataStreamIn < $ty:ty > $(= $name:literal)?, $($rest:tt)*
    ) => {
        $crate::ports!(@munch [$($i)* { ($(#[$m])*) $field ($ty) ($($name)?) In }] [$($o)*] [$($vo)*] [$($vi)*] $($rest)*);
    };
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]
        $(#[$m:meta])* $field:ident : DataStreamOut < $ty:ty > $(= $name:literal)? $(in_place_of $input:ident)?,
        $($rest:tt)*
    ) => {
        $crate::ports!(
            @munch [$($i)*] [$($o)* { ($(#[$m])*) $field ($ty) ($($name)?) ($($input)?) }] [$($vo)*] [$($vi)*] $($rest)*
        );
    };
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]
        $(#[$m:meta])* $field:ident : ValOutCmdIn < $value:ty , ($($command:ident),* $(,)?) >
        $(= $name:literal)? $(side: $side:ident)?, $($rest:tt)*
    ) => {
        $crate::ports!(@munch [$($i)*] [$($o)*]
            [$($vo)* { ($(#[$m])*) $field ($value) ($($command),*) ($($name)?) ($($side)?) }] [$($vi)*] $($rest)*
        );
    };
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]
        $(#[$m:meta])* $field:ident : ValInCmdOut < $value:ty , ($($command:ident),* $(,)?) >
        $(= $name:literal)? $(side: $side:ident)?, $($rest:tt)*
    ) => {
        $crate::ports!(@munch [$($i)*] [$($o)*] [$($vo)*]
            [$($vi)* { ($(#[$m])*) $field ValInCmdOut ($value) ($($command),*) ($($name)?) ($($side)?) }] $($rest)*
        );
    };
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]
        $(#[$m:meta])* $field:ident : ValIn < $value:ty > $(= $name:literal)? $(side: $side:ident)?, $($rest:tt)*
    ) => {
        $crate::ports!(@munch [$($i)*] [$($o)*] [$($vo)*]
            [$($vi)* { ($(#[$m])*) $field ValIn ($value) () ($($name)?) ($($side)?) }]
            $($rest)*
        );
    };
    // Skips a leftover comma. The rules above consume one comma after each port, so a trailing comma in the
    // declaration is left over.
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*] , $($rest:tt)*) => {
        $crate::ports!(@munch [$($i)*] [$($o)*] [$($vo)*] [$($vi)*] $($rest)*);
    };
    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*]) => {
        $crate::ports!(@data_stream_in $($i)*);
        $crate::ports!(@data_stream_out $($o)*);
        $crate::ports!(@val_out_cmd_in $($vo)*);
        $crate::ports!(@cmd_in $($vo)*);
        $crate::ports!(@val_in $($vi)*);
        $crate::ports!(@port_edges [$($i)*] [$($o)*] [$($vo)*] [$($vi)*]);

        /// All of the node's ports (see `ephys_rs::engine::ports`). The node names this type as its `Node::Ports`.
        pub struct Ports;

        impl<E: $crate::engine::Edges> $crate::engine::PortSet<E> for Ports
        where
            $crate::ports!(@type DataStreamIn [$($i)*]): $crate::engine::DataStreamInPorts<E>,
            $crate::ports!(@type DataStreamOut [$($o)*]): $crate::engine::DataStreamOutPorts<E>,
            $crate::ports!(@type ValOutCmdIn [$($vo)*]): $crate::engine::ValOutCmdInPorts<E>,
            CmdIn: $crate::engine::CommandSet<E>,
            ValIn: $crate::engine::ValInPortList<E>,
        {
            type DataStreamIn = $crate::ports!(@type DataStreamIn [$($i)*]);
            type DataStreamOut = $crate::ports!(@type DataStreamOut [$($o)*]);
            type ValOutCmdIn = $crate::ports!(@type ValOutCmdIn [$($vo)*]);
            type CmdIn = CmdIn;

            fn val_in_ports() -> Vec<$crate::engine::Port<E>> {
                <ValIn as $crate::engine::ValInPortList<E>>::ports()
            }
        }
    };

    (@munch [$($i:tt)*] [$($o:tt)*] [$($vo:tt)*] [$($vi:tt)*] $($rest:tt)+) => {
        compile_error!(concat!(
            "ports!: each port is `name: DataStreamIn<T>`, `DataStreamOut<T>`, `ValOutCmdIn<V, (Commands, ...)>`, ",
            "`ValInCmdOut<V, (Commands, ...)>` or `ValIn<V>`; this isn't: ",
            stringify!($($rest)+)
        ));
    };

    (@type DataStreamIn []) => { $crate::engine::NoPorts };
    (@type DataStreamIn [$($any:tt)+]) => { DataStreamIn };
    (@type DataStreamOut []) => { $crate::engine::NoPorts };
    (@type DataStreamOut [$($any:tt)+]) => { DataStreamOut };
    (@type ValOutCmdIn []) => { $crate::engine::NoPorts };
    (@type ValOutCmdIn [$($any:tt)+]) => { ValOutCmdIn };

    (@data_stream_in) => {};
    (@data_stream_in $({ ($($m:tt)*) $field:ident ($ty:ty) ($($name:literal)?) $role:ident })+) => {
        /// The node's `DataStreamIn` ports (see `ephys_rs::engine::ports`).
        pub struct DataStreamIn<F: $crate::engine::PortForm = $crate::engine::Streams> {
            $($($m)* pub $field: F::$role<$ty>,)+
        }

        const _: () = {
            #[allow(non_camel_case_types)]
            enum Index { $($field),+ }

            #[allow(non_upper_case_globals, dead_code)]
            impl DataStreamIn {
                $(
                    #[doc(hidden)]
                    pub const $field: usize = Index::$field as usize;
                )+
            }

            impl Clone for DataStreamIn {
                fn clone(&self) -> Self {
                    *self
                }
            }

            impl Copy for DataStreamIn {}

            impl PartialEq for DataStreamIn {
                fn eq(&self, other: &Self) -> bool {
                    true $(&& self.$field == other.$field)+
                }
            }

            impl std::fmt::Debug for DataStreamIn {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.debug_struct("DataStreamIn") $(.field(stringify!($field), &self.$field))+ .finish()
                }
            }

            impl Default for DataStreamIn {
                fn default() -> Self {
                    DataStreamIn { $($field: None),+ }
                }
            }

            impl DataStreamIn {
                /// Whether any of the node's `DataStreamIn` ports is connected.
                pub fn any_connected(&self) -> bool {
                    false $(|| self.$field.is_some())+
                }
            }

            impl<E: $crate::engine::Edges> $crate::engine::DataStreamInPorts<E> for DataStreamIn
            where
                $(E: $crate::engine::HasDataStream<$ty>,)+
            {
                type View<'a> = DataStreamIn<$crate::engine::Buffers<'a>>;
                const TAKES: &'static [bool] = &[$($crate::ports!(@takes $role)),+];

                fn ports() -> Vec<$crate::engine::Port<E>> {
                    vec![$(
                        $crate::engine::Port::data_stream_in(
                            stringify!($field),
                            $crate::ports!(@name $field $($name)?),
                            <E as $crate::engine::HasDataStream<$ty>>::KIND,
                        )
                    ),+]
                }

                fn of(streams: &[Option<E::DataStream>]) -> Self {
                    let mut streams = streams.iter();
                    DataStreamIn {
                        $($field: streams
                            .next()
                            .and_then(Option::as_ref)
                            .and_then(<E as $crate::engine::HasDataStream<$ty>>::from_stream),)+
                    }
                }

                fn streams(&self) -> Vec<Option<E::DataStream>> {
                    vec![$(self.$field.map(<E as $crate::engine::HasDataStream<$ty>>::wrap)),+]
                }

                fn view<'a>(
                    shared: &'a [Option<&'a $crate::engine::Buffer<E>>],
                    taken: &'a mut [Option<$crate::engine::Buffer<E>>],
                ) -> Self::View<'a> {
                    let (mut shared, mut taken) = (shared.iter(), taken.iter_mut());
                    DataStreamIn {
                        $($field: $crate::ports!(@view $role E $ty, shared.next(), taken.next()),)+
                    }
                }
            }
        };
    };

    (@data_stream_out) => {};
    (@data_stream_out $({ ($($m:tt)*) $field:ident ($ty:ty) ($($name:literal)?) ($($input:ident)?) })+) => {
        /// The node's `DataStreamOut` ports (see `ephys_rs::engine::ports`).
        pub struct DataStreamOut<F: $crate::engine::PortForm = $crate::engine::Streams> {
            $($($m)* pub $field: F::Out<$ty>,)+
        }

        const _: () = {
            // An output written in place of an input reuses that input's buffer, so the two must have the same type.
            // This line fails to compile if they don't.
            $($(
                const _: fn(DataStreamIn) -> Option<$ty> = |inputs| inputs.$input;
            )?)+

            impl Clone for DataStreamOut {
                fn clone(&self) -> Self {
                    *self
                }
            }

            impl Copy for DataStreamOut {}

            impl PartialEq for DataStreamOut {
                fn eq(&self, other: &Self) -> bool {
                    true $(&& self.$field == other.$field)+
                }
            }

            impl std::fmt::Debug for DataStreamOut {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.debug_struct("DataStreamOut") $(.field(stringify!($field), &self.$field))+ .finish()
                }
            }

            impl Default for DataStreamOut {
                fn default() -> Self {
                    DataStreamOut { $($field: None),+ }
                }
            }

            impl<E: $crate::engine::Edges> $crate::engine::DataStreamOutPorts<E> for DataStreamOut
            where
                $(E: $crate::engine::HasDataStream<$ty>,)+
            {
                type View<'a> = DataStreamOut<$crate::engine::Buffers<'a>>;
                const IN_PLACE: &'static [Option<usize>] = &[$($crate::ports!(@in_place $($input)?)),+];

                fn ports(&self) -> Vec<$crate::engine::Port<E>> {
                    vec![$(
                        $crate::engine::Port::data_stream_out(
                            stringify!($field),
                            $crate::ports!(@name $field $($name)?),
                            <E as $crate::engine::HasDataStream<$ty>>::KIND,
                            self.$field.map(<E as $crate::engine::HasDataStream<$ty>>::wrap),
                        )
                    ),+]
                }

                fn view<'a>(outputs: &'a mut [$crate::engine::Buffer<E>]) -> Self::View<'a> {
                    let mut outputs = outputs.iter_mut();
                    DataStreamOut { $($field: $crate::engine::fill_output::<E, $ty>(outputs.next()),)+ }
                }
            }
        };
    };

    (@val_out_cmd_in) => {};
    (@val_out_cmd_in $(
        { ($($m:tt)*) $field:ident ($value:ty) ($($command:ident),*) ($($name:literal)?) ($($side:ident)?) }
    )+) => {
        /// The node's values out, at its `ValOutCmdIn` ports (see `ephys_rs::engine::ports`).
        pub struct ValOutCmdIn {
            $($($m)* pub $field: $value,)+
        }

        const _: () = {
            #[allow(non_camel_case_types)]
            enum Index { $($field),+ }

            #[allow(non_upper_case_globals, dead_code)]
            impl ValOutCmdIn {
                $(
                    #[doc(hidden)]
                    pub const $field: usize = Index::$field as usize;
                )+
            }
        };

        impl<E: $crate::engine::Edges> $crate::engine::ValOutCmdInPorts<E> for ValOutCmdIn
        where
            $(E: $crate::engine::HasValue<$value>, ($($command,)*): $crate::engine::CommandList<E>,)+
        {
            fn ports() -> Vec<$crate::engine::Port<E>> {
                vec![$(
                    $crate::engine::Port::ValOutCmdIn {
                        name: $crate::ports!(@name $field $($name)?),
                        id: stringify!($field),
                        value_kind: <E as $crate::engine::HasValue<$value>>::KIND,
                        commands: <($($command,)*) as $crate::engine::CommandList<E>>::kinds(),
                        side: $crate::ports!(@side Right $($side)?),
                    }
                ),+]
            }

            fn values(self) -> Vec<E::ValOut> {
                vec![$(<E as $crate::engine::HasValue<$value>>::wrap(self.$field)),+]
            }
        }
    };

    (@cmd_in $({ ($($m:tt)*) $field:ident ($value:ty) ($($command:ident),*) ($($name:literal)?) ($($side:ident)?) })*) => {
        /// The commands the node accepts through its `ValOutCmdIn` ports (see `ephys_rs::engine::ports`).
        #[derive(Debug)]
        pub enum CmdIn {
            $($($command($command),)*)*
        }

        $($(
            impl From<$command> for CmdIn {
                fn from(command: $command) -> Self {
                    CmdIn::$command(command)
                }
            }
        )*)*

        impl<E: $crate::engine::Edges> $crate::engine::CommandSet<E> for CmdIn
        where
            $($(E: $crate::engine::HasCommand<$command>,)*)*
        {
            fn from_command(command: E::Command) -> Result<Self, E::Command> {
                $($(
                    let command = match <E as $crate::engine::HasCommand<$command>>::from_command(command) {
                        Ok(command) => return Ok(CmdIn::$command(command)),
                        Err(command) => command,
                    };
                )*)*
                Err(command)
            }

            fn wrap(self) -> E::Command {
                match self {
                    $($(CmdIn::$command(command) => <E as $crate::engine::HasCommand<$command>>::wrap(command),)*)*
                }
            }
        }
    };

    (@val_in $(
        { ($($m:tt)*) $field:ident $role:ident ($value:ty) ($($command:ident),*) ($($name:literal)?) ($($side:ident)?) }
    )*) => {
        /// Handles to the node's `ValInCmdOut` ports (see `ephys_rs::engine::ports`).
        #[allow(dead_code, reason = "a node without ValInCmdOut ports never uses this struct")]
        pub struct ValInCmdOut;
        /// Handles to the node's `ValIn` ports (see `ephys_rs::engine::ports`).
        #[allow(dead_code, reason = "a node without ValIn ports never uses this struct")]
        pub struct ValIn;

        const _: () = {
            #[allow(non_camel_case_types, dead_code)]
            enum Index { $($field,)* }

            $(
                #[allow(non_upper_case_globals, dead_code)]
                impl $role {
                    $($m)*
                    pub const $field: $crate::ports!(@handle $role $value, $($command),*) =
                        <$crate::ports!(@handle $role $value, $($command),*)>::new(Index::$field as usize);
                }
            )*

            impl<E: $crate::engine::Edges> $crate::engine::ValInPortList<E> for ValIn
            where
                $($crate::ports!(@handle $role $value, $($command),*): $crate::engine::ValInPortOf<E>,)*
            {
                fn ports() -> Vec<$crate::engine::Port<E>> {
                    vec![$(
                        <$crate::ports!(@handle $role $value, $($command),*) as $crate::engine::ValInPortOf<E>>::port(
                            stringify!($field),
                            $crate::ports!(@name $field $($name)?),
                            $crate::ports!(@side Left $($side)?),
                        )
                    ),*]
                }
            }
        };
    };

    (@handle ValInCmdOut $value:ty, $($command:ident),*) => { $crate::engine::ValInCmdOutPort<$value, ($($command,)*)> };
    (@handle ValIn $value:ty,) => { $crate::engine::ValInPort<$value> };
    (@port_edges
        [$({ ($($im:tt)*) $ifield:ident ($ity:ty) ($($iname:literal)?) $irole:ident })*]
        [$({ ($($om:tt)*) $ofield:ident ($oty:ty) ($($oname:literal)?) ($($oinput:ident)?) })*]
        [$({ ($($vm:tt)*) $vfield:ident ($vty:ty) ($($vc:ident),*) ($($vname:literal)?) ($($vside:ident)?) })*]
        [$({ ($($wm:tt)*) $wfield:ident $wrole:ident ($wty:ty) ($($wc:ident),*) ($($wname:literal)?) ($($wside:ident)?) })*]
    ) => {
        /// The edge types that the node's ports carry (see `ephys_rs::engine::ports`). The node's `Node` impl applies
        /// to any program's edges that implement this trait, which means the program lists all of those types.
        pub trait PortEdges:
            $crate::engine::Edges
            $(+ $crate::engine::HasDataStream<$ity>)*
            $(+ $crate::engine::HasDataStream<$oty>)*
            $(+ $crate::engine::HasValue<$vty> $(+ $crate::engine::HasCommand<$vc>)*)*
            $(+ $crate::engine::HasValue<$wty> $(+ $crate::engine::HasCommand<$wc>)*)*
        {
        }

        impl<E> PortEdges for E where
            E: $crate::engine::Edges
                $(+ $crate::engine::HasDataStream<$ity>)*
                $(+ $crate::engine::HasDataStream<$oty>)*
                $(+ $crate::engine::HasValue<$vty> $(+ $crate::engine::HasCommand<$vc>)*)*
                $(+ $crate::engine::HasValue<$wty> $(+ $crate::engine::HasCommand<$wc>)*)*
        {
        }
    };
    (@name $field:ident $name:literal) => { $name };
    (@name $field:ident) => { stringify!($field) };
    (@side $default:ident $side:ident) => { $crate::engine::Side::$side };
    (@side $default:ident) => { $crate::engine::Side::$default };
    (@takes In) => { false };
    (@takes Take) => { true };
    (@view In $e:ident $ty:ty, $shared:expr, $taken:expr) => { $crate::engine::read_input::<$e, $ty>($shared, $taken) };
    (@view Take $e:ident $ty:ty, $shared:expr, $taken:expr) => { $crate::engine::take_input::<$e, $ty>($shared, $taken) };
    (@in_place $input:ident) => { Some(<DataStreamIn>::$input) };
    (@in_place) => { None };

    ($($port:tt)*) => {
        $crate::ports!(@munch [] [] [] [] $($port)* ,);
    };
}

/// A node's `DataStreamIn` ports, in the `Streams` form (see `PortSet`).
pub type DataStreamInOf<N, E> = <<N as Node<E>>::Ports as PortSet<E>>::DataStreamIn;
/// A node's `DataStreamOut` ports, in the `Streams` form.
pub type DataStreamOutOf<N, E> = <<N as Node<E>>::Ports as PortSet<E>>::DataStreamOut;
/// The values at a node's `ValOutCmdIn` ports.
pub type ValOutCmdInOf<N, E> = <<N as Node<E>>::Ports as PortSet<E>>::ValOutCmdIn;
/// The commands a node takes.
pub type CmdInOf<N, E> = <<N as Node<E>>::Ports as PortSet<E>>::CmdIn;

/// `ports!` lists a node's `ValInCmdOut` and `ValIn` ports through this.
#[doc(hidden)]
pub trait ValInPortList<E: Edges> {
    fn ports() -> Vec<Port<E>>;
}
