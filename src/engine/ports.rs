//! A node declares its ports on its type, once, with `ports!`: each port named, of one of the five roles (see
//! `PortRole`), and typed by what it carries. Everything else about the ports follows from that:
//!
//! - `DataStreamIn` and `DataStreamOut`: structs of the node's data ports, generic over a form (see
//!   `crate::engine::data_ports`): their streams, for `Node::output_streams`, or their buffers, for `Node::process`.
//! - `ValOutCmdIn`: a struct of the values the node publishes, for `Node::val_out`; and `CmdIn`, an enum of every command
//!   they take, for `Node::on_command` (see `crate::engine::val_cmd_ports`).
//! - `ValInCmdOut` and `ValIn`: a handle to each port reading another node's value (`ValInCmdOut::gains`), for the
//!   contexts to read and command through.
//! - `Ports`, which the node names as its `Node::Ports`, gathering them for the engine (`PortSet`).
//! - `PortEdges`: what a program's edges must have for the node (`impl<E: PortEdges> Node<E> for MyNode`), so a node
//!   works in any program listing the types its ports carry, and a program lists only the types its nodes need.
//!
//! The graph numbers each role's ports, in the order they're declared (and the node numbers its `ValInCmdOut` and
//! `ValIn` ports together, as it reads values through them); saved graphs name ports by their fields' names instead
//! (see `Port::id`).

use super::{
    CommandSet, DataStreamInPorts, DataStreamOutPorts, Edges, NoCommands, NoPorts, Node, Port, ValOutCmdInPorts,
};

/// `ports!` implements this for a node's `Ports`, gathering what it makes, which the node names as its `Node::Ports`.
/// A node without ports of a role has `NoPorts` for them (or `NoCommands`, for commands, without `ValOutCmdIn` ports).
pub trait PortSet<E: Edges>: 'static {
    type DataStreamIn: DataStreamInPorts<E>;
    type DataStreamOut: DataStreamOutPorts<E>;
    type ValOutCmdIn: ValOutCmdInPorts<E>;
    type CmdIn: CommandSet<E>;

    /// The graph lists the node's `ValInCmdOut` and `ValIn` ports from this, in the order they're declared.
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

/// A node declares its ports with this, each a field named as the port, of one of the five roles (see `PortRole`):
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
/// Each port's typed by the data stream or value it carries, and a `ValOutCmdIn` or `ValInCmdOut` port by the commands
/// it takes or sends, as a tuple of command types in scope (see `HasCommand`). A port's named for people by its field's
/// name, or the text after it. After that:
///
/// - `take`: the node takes a `DataStreamIn` port's buffer (see `BufferUsage::takes`);
/// - `in_place_of`: the node makes a `DataStreamOut` port in place of a `DataStreamIn` port of the same type (see
///   `BufferUsage::in_place`);
/// - `side: Left` or `side: Right`: the side of the node the graph draws a ValCmd port on (see `Side`).
///
/// The macro makes, in the node's module:
///
/// - `DataStreamIn` and `DataStreamOut` (if there are such ports): structs of them, generic over a `PortForm`
///   (`Streams` by default), implementing `DataStreamInPorts` and `DataStreamOutPorts`, with, for each `DataStreamIn`
///   port, an associated constant named as it holding its number (`DataStreamIn::continuous`);
/// - `ValOutCmdIn` (if there are such ports): a struct of their values, implementing `ValOutCmdInPorts`, with, for each,
///   an associated constant named as it holding its number (`ValOutCmdIn::files`, which `CommandSource::Port` gives);
/// - `CmdIn`: an enum of the commands the `ValOutCmdIn` ports take, a variant holding each (`CmdIn::Detect(Detect)`),
///   implementing `CommandSet` (a command can be taken through only one `ValOutCmdIn` port, so it's listed once);
/// - `ValInCmdOut` and `ValIn`: unit structs with a handle to each such port, as a constant named as it
///   (`ValInCmdOut::gains: ValInCmdOutPort<Gains, (SetGains, Detect)>`);
/// - `Ports`, implementing `PortSet`;
/// - `PortEdges`, a trait gathering what a program's edges must have for the node's ports (`HasDataStream<T>`,
///   `HasValue<V>` and `HasCommand<C>` for each type they carry), implemented for any that do: so the node's `Node` is
///   `impl<E: PortEdges> Node<E> for MyNode`, for any program whose `edge_types!` lists them.
///
/// Saved graphs name each port by its field's name, with its role (`Port::id`), never by the text it's shown with, so
/// renaming a field breaks existing files (unless a config patch renames the edges' ends), but reordering doesn't.
#[macro_export]
macro_rules! ports {
    // Each port goes on a list of its role's: `DataStreamIn`, `DataStreamOut`, `ValOutCmdIn`, and `ValInCmdOut` with
    // `ValIn` (which the node numbers together).
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
    // A comma left over (the declarations' own trailing one, as the macro adds one).
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

        /// The node's ports (see `ephys_rs::engine::ports`), which it names as its `Node::Ports`.
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
                /// Whether anything's connected to any of the node's `DataStreamIn` ports.
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
            // An output made in place of an input holds that input's data, so must be the same type.
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
        /// The values the node publishes at its `ValOutCmdIn` ports (see `ephys_rs::engine::ports`).
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

            fn values(self) -> Vec<E::PublishedValue> {
                vec![$(<E as $crate::engine::HasValue<$value>>::wrap(self.$field)),+]
            }
        }
    };

    (@cmd_in $({ ($($m:tt)*) $field:ident ($value:ty) ($($command:ident),*) ($($name:literal)?) ($($side:ident)?) })*) => {
        /// The commands the node takes, through its `ValOutCmdIn` ports (see `ephys_rs::engine::ports`).
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
        #[allow(dead_code, reason = "a node without such ports has none")]
        pub struct ValInCmdOut;
        /// Handles to the node's `ValIn` ports (see `ephys_rs::engine::ports`).
        #[allow(dead_code, reason = "a node without such ports has none")]
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
        /// The edge types the node's ports carry, which a program's edges must have for the node (see
        /// `ephys_rs::engine::ports`): the node's `Node` is implemented for any program's edges implementing this.
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
