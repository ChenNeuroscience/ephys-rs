//! A node declares its data ports on its type, once, with `data_ports!`: a struct of its inputs and a struct of its
//! outputs, each field a port, typed by the data stream it carries. Everything else about the ports follows from
//! that: the ports the graph lists, in the order they're declared; which inputs the node takes and which outputs it
//! makes in place (see `BufferUsage`); and the typed views the node works with.
//!
//! Each struct is generic over a form (`PortForm`), which says what each field holds. With `Streams` (the default),
//! each field holds the data stream on the port, if it's known: `Option<Continuous>`, say. The node works out its
//! outputs' streams from its inputs' with these (see `Node::output_streams`), so a port's kind and its stream can't
//! disagree. With `Buffers`, each field holds the port's buffer in a pass: `Option<&ContinuousBuffer>` for an input,
//! `&mut ContinuousBuffer` for an output. The node's `process` is given these, already the right type.
//!
//! The graph and the plan still number ports, in the order they're declared, and the engine still holds streams and
//! buffers as the program's enums of them; `InputPorts` and `OutputPorts` go between the two, through `StreamOf`.

use std::fmt::Debug;
use std::marker::PhantomData;

use super::{Buffer, DataStreamType, Edges, Port, StreamOf};

/// A node's `Inputs` and `Outputs` structs (see `data_ports!`) are generic over a form, which says what each field
/// holds, by the field's role: an input the node reads (`In`), an input the node takes (`Take`), or an output (`Out`).
pub trait PortForm {
    type In<T: DataStreamType>;
    type Take<T: DataStreamType>;
    type Out<T: DataStreamType>;
}

/// The graph works out which data streams are on a node's ports. In this form, each field holds the stream on its port,
/// if that's known (an input with nothing connected has none, and so, usually, do the outputs made from it).
pub struct Streams;

impl PortForm for Streams {
    type In<T: DataStreamType> = Option<T>;
    type Take<T: DataStreamType> = Option<T>;
    type Out<T: DataStreamType> = Option<T>;
}

/// In a pass, the node reads its inputs' buffers and fills its outputs'. In this form, each field holds its port's
/// buffer: an input's to read (`None` if the input isn't connected, or its data's in an output the node makes in place);
/// an input's to take (`None` if it isn't connected); and an output's to fill.
pub struct Buffers<'a>(PhantomData<&'a mut ()>);

impl<'a> PortForm for Buffers<'a> {
    type In<T: DataStreamType> = Option<&'a T::Buffer>;
    type Take<T: DataStreamType> = Option<&'a mut T::Buffer>;
    type Out<T: DataStreamType> = &'a mut T::Buffer;
}

/// `data_ports!` implements this for a node's struct of inputs, in the `Streams` form, which is what the node's
/// `Node::Inputs` names.
pub trait InputPorts<E: Edges>: Copy + PartialEq + Debug + Default + Send + Sync + 'static {
    /// The struct in the `Buffers` form, which the node's `process` is given.
    type View<'a>;

    /// For each input, whether the node takes it (see `BufferUsage::takes`): those declared `Take`.
    const TAKES: &'static [bool];

    /// The graph lists the node's inputs from this, in the order they're declared.
    fn ports() -> Vec<Port<E>>;

    /// The graph has the stream on each input (if any), in the order the inputs are declared. This is the struct of them.
    fn of(streams: &[Option<E::DataStream>]) -> Self;

    /// The streams, as the graph has them (see `of`).
    fn streams(&self) -> Vec<Option<E::DataStream>>;

    /// A pass has, for each input, its buffer to read (`shared`) or, if the node takes it, to take (`taken`). This is
    /// the struct of them.
    fn view<'a>(shared: &'a [Option<&'a Buffer<E>>], taken: &'a mut [Option<Buffer<E>>]) -> Self::View<'a>;
}

/// `data_ports!` implements this for a node's struct of outputs, in the `Streams` form, which is what the node's
/// `Node::Outputs` names.
pub trait OutputPorts<E: Edges>: Copy + PartialEq + Debug + Default + Send + Sync + 'static {
    /// The struct in the `Buffers` form, which the node's `process` is given.
    type View<'a>;

    /// For each output, the input the node makes it in place of, if any (see `BufferUsage::in_place`).
    const IN_PLACE: &'static [Option<usize>];

    /// The graph lists the node's outputs, with their streams, from this, in the order they're declared.
    fn ports(&self) -> Vec<Port<E>>;

    /// A pass has a buffer for each output, made for its stream. This is the struct of them.
    fn view<'a>(outputs: &'a mut [Buffer<E>]) -> Self::View<'a>;
}

/// A node with no inputs, or no outputs, names this as its `Node::Inputs` or `Node::Outputs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct NoPorts;

impl<E: Edges> InputPorts<E> for NoPorts {
    type View<'a> = NoPorts;
    const TAKES: &'static [bool] = &[];

    fn ports() -> Vec<Port<E>> {
        Vec::new()
    }

    fn of(_streams: &[Option<E::DataStream>]) -> Self {
        NoPorts
    }

    fn streams(&self) -> Vec<Option<E::DataStream>> {
        Vec::new()
    }

    fn view<'a>(_shared: &'a [Option<&'a Buffer<E>>], _taken: &'a mut [Option<Buffer<E>>]) -> NoPorts {
        NoPorts
    }
}

impl<E: Edges> OutputPorts<E> for NoPorts {
    type View<'a> = NoPorts;
    const IN_PLACE: &'static [Option<usize>] = &[];

    fn ports(&self) -> Vec<Port<E>> {
        Vec::new()
    }

    fn view(_outputs: &mut [Buffer<E>]) -> NoPorts {
        NoPorts
    }
}

/// `data_ports!` finds an input's buffer to read with this: the one the node was given to take, if it was given one,
/// or else the one to read.
#[doc(hidden)]
pub fn read_input<'a, E: Edges, T: StreamOf<E>>(
    shared: Option<&'a Option<&'a Buffer<E>>>,
    taken: Option<&'a mut Option<Buffer<E>>>,
) -> Option<&'a T::Buffer> {
    let taken = taken.map(|taken| -> &'a Option<Buffer<E>> { taken });
    taken.and_then(Option::as_ref).or(shared.copied().flatten()).and_then(T::from_buffer)
}

/// `data_ports!` finds an input's buffer to take with this.
#[doc(hidden)]
pub fn take_input<'a, E: Edges, T: StreamOf<E>>(
    _shared: Option<&'a Option<&'a Buffer<E>>>,
    taken: Option<&'a mut Option<Buffer<E>>>,
) -> Option<&'a mut T::Buffer> {
    taken.and_then(Option::as_mut).and_then(T::from_buffer_mut)
}

/// `data_ports!` finds an output's buffer with this. The plan makes each output's buffer from the stream the node gave
/// for it, which is of the output's own type, so the buffer always is too.
#[doc(hidden)]
pub fn fill_output<E: Edges, T: StreamOf<E>>(output: Option<&mut Buffer<E>>) -> &mut T::Buffer {
    output
        .and_then(T::from_buffer_mut)
        .expect("the plan makes each output's buffer for the stream the node gave for it")
}

/// A node declares its data ports with this, as a struct of its inputs and a struct of its outputs (either can be left
/// out, for a node with none: it names `NoPorts` instead):
///
/// ```ignore
/// data_ports! {
///     /// Doc comments on the structs, and on each field, go with them.
///     inputs: pub struct Inputs {
///         continuous: In<Continuous>,
///         ttl: Take<Events> = "pos ttl",
///     }
///     outputs: pub struct Outputs {
///         filtered: Out<Continuous> in_place_of continuous,
///     }
/// }
/// ```
///
/// Each field is a port, named for people by the field's name, or by the text after it. An input is `In` (the node
/// reads it) or `Take` (the node takes it: see `BufferUsage::takes`). An output can be made `in_place_of` an input of
/// the same type (see `BufferUsage::in_place`). Saved graphs name ports by their index, in the order they're declared
/// here, so reordering them breaks existing files (unless a config patch moves the edges).
///
/// The structs are generic over a `PortForm`, `Streams` by default (see `crate::engine::data_ports`). The macro makes
/// them, `InputPorts` and `OutputPorts` for them, and, for each input, an associated constant with the input's name,
/// holding its index (which `in_place_of` uses).
#[macro_export]
macro_rules! data_ports {
    (
        $(#[$im:meta])* inputs: $ivis:vis struct $inputs:ident { $($ibody:tt)* }
        $(#[$om:meta])* outputs: $ovis:vis struct $outputs:ident { $($obody:tt)* }
    ) => {
        $crate::data_ports!(@inputs $(#[$im])* $ivis $inputs { $($ibody)* });
        $crate::data_ports!(@outputs $inputs; $(#[$om])* $ovis $outputs { $($obody)* });
    };
    ($(#[$im:meta])* inputs: $ivis:vis struct $inputs:ident { $($ibody:tt)* }) => {
        $crate::data_ports!(@inputs $(#[$im])* $ivis $inputs { $($ibody)* });
    };
    ($(#[$om:meta])* outputs: $ovis:vis struct $outputs:ident { $($obody:tt)* }) => {
        $crate::data_ports!(@outputs $crate::engine::NoPorts; $(#[$om])* $ovis $outputs { $($obody)* });
    };

    (@inputs
        $(#[$im:meta])* $ivis:vis $inputs:ident {
            $($(#[$fm:meta])* $field:ident : $role:ident < $ty:ty > $(= $name:literal)?),+ $(,)?
        }
    ) => {
        $(#[$im])*
        $ivis struct $inputs<F: $crate::engine::PortForm = $crate::engine::Streams> {
            $($(#[$fm])* pub $field: F::$role<$ty>,)+
        }

        const _: () = {
            #[allow(non_camel_case_types)]
            enum Index { $($field),+ }

            #[allow(non_upper_case_globals, dead_code)]
            impl $inputs {
                $(
                    #[doc(hidden)]
                    pub const $field: usize = Index::$field as usize;
                )+
            }

            impl Clone for $inputs {
                fn clone(&self) -> Self {
                    *self
                }
            }

            impl Copy for $inputs {}

            impl PartialEq for $inputs {
                fn eq(&self, other: &Self) -> bool {
                    true $(&& self.$field == other.$field)+
                }
            }

            impl std::fmt::Debug for $inputs {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.debug_struct(stringify!($inputs)) $(.field(stringify!($field), &self.$field))+ .finish()
                }
            }

            impl Default for $inputs {
                fn default() -> Self {
                    $inputs { $($field: None),+ }
                }
            }

            impl<E: $crate::engine::Edges> $crate::engine::InputPorts<E> for $inputs
            where
                $($ty: $crate::engine::StreamOf<E>,)+
            {
                type View<'a> = $inputs<$crate::engine::Buffers<'a>>;
                const TAKES: &'static [bool] = &[$($crate::data_ports!(@takes $role)),+];

                fn ports() -> Vec<$crate::engine::Port<E>> {
                    vec![$(
                        $crate::engine::Port::data_stream_in(
                            $crate::data_ports!(@name $field $($name)?),
                            <$ty as $crate::engine::StreamOf<E>>::KIND,
                        )
                    ),+]
                }

                fn of(streams: &[Option<E::DataStream>]) -> Self {
                    let mut streams = streams.iter();
                    $inputs {
                        $($field: streams
                            .next()
                            .and_then(Option::as_ref)
                            .and_then(<$ty as $crate::engine::StreamOf<E>>::from_stream),)+
                    }
                }

                fn streams(&self) -> Vec<Option<E::DataStream>> {
                    vec![$(self.$field.map($crate::engine::StreamOf::<E>::wrap)),+]
                }

                fn view<'a>(
                    shared: &'a [Option<&'a $crate::engine::Buffer<E>>],
                    taken: &'a mut [Option<$crate::engine::Buffer<E>>],
                ) -> Self::View<'a> {
                    let (mut shared, mut taken) = (shared.iter(), taken.iter_mut());
                    $inputs {
                        $($field: $crate::data_ports!(@view $role E $ty, shared.next(), taken.next()),)+
                    }
                }
            }
        };
    };

    (@outputs
        $inputs:ty; $(#[$om:meta])* $ovis:vis $outputs:ident {
            $(
                $(#[$fm:meta])* $field:ident : Out < $ty:ty > $(= $name:literal)? $(in_place_of $input:ident)?
            ),+ $(,)?
        }
    ) => {
        $(#[$om])*
        $ovis struct $outputs<F: $crate::engine::PortForm = $crate::engine::Streams> {
            $($(#[$fm])* pub $field: F::Out<$ty>,)+
        }

        const _: () = {
            // An output made in place of an input holds that input's data, so must be the same type.
            $($(
                const _: fn($inputs) -> Option<$ty> = |inputs| inputs.$input;
            )?)+

            impl Clone for $outputs {
                fn clone(&self) -> Self {
                    *self
                }
            }

            impl Copy for $outputs {}

            impl PartialEq for $outputs {
                fn eq(&self, other: &Self) -> bool {
                    true $(&& self.$field == other.$field)+
                }
            }

            impl std::fmt::Debug for $outputs {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.debug_struct(stringify!($outputs)) $(.field(stringify!($field), &self.$field))+ .finish()
                }
            }

            impl Default for $outputs {
                fn default() -> Self {
                    $outputs { $($field: None),+ }
                }
            }

            impl<E: $crate::engine::Edges> $crate::engine::OutputPorts<E> for $outputs
            where
                $($ty: $crate::engine::StreamOf<E>,)+
            {
                type View<'a> = $outputs<$crate::engine::Buffers<'a>>;
                const IN_PLACE: &'static [Option<usize>] = &[$($crate::data_ports!(@in_place $inputs; $($input)?)),+];

                fn ports(&self) -> Vec<$crate::engine::Port<E>> {
                    vec![$(
                        $crate::engine::Port::data_stream_out(
                            $crate::data_ports!(@name $field $($name)?),
                            <$ty as $crate::engine::StreamOf<E>>::KIND,
                            self.$field.map($crate::engine::StreamOf::<E>::wrap),
                        )
                    ),+]
                }

                fn view<'a>(outputs: &'a mut [$crate::engine::Buffer<E>]) -> Self::View<'a> {
                    let mut outputs = outputs.iter_mut();
                    $outputs { $($field: $crate::engine::fill_output::<E, $ty>(outputs.next()),)+ }
                }
            }
        };
    };

    (@name $field:ident $name:literal) => { $name };
    (@name $field:ident) => { stringify!($field) };
    (@takes In) => { false };
    (@takes Take) => { true };
    (@view In $e:ident $ty:ty, $shared:expr, $taken:expr) => { $crate::engine::read_input::<$e, $ty>($shared, $taken) };
    (@view Take $e:ident $ty:ty, $shared:expr, $taken:expr) => { $crate::engine::take_input::<$e, $ty>($shared, $taken) };
    (@in_place $inputs:ty; $input:ident) => { Some(<$inputs>::$input) };
    (@in_place $inputs:ty;) => { None };
}
