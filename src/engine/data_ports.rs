//! A node declares its ports on its type with `ports!` (see `crate::engine::ports`), which makes a struct of its
//! `DataStreamIn` ports and a struct of its `DataStreamOut` ports, each field a port, typed by the data stream it
//! carries. Everything else about them follows from that: the ports the graph lists, in the order they're declared;
//! which inputs the node takes and which outputs it makes in place (see `BufferUsage`); and the typed views the node
//! works with. This has what those structs are built on.
//!
//! Each struct is generic over a form (`PortForm`), which says what each field holds. With `Streams` (the default),
//! each field holds the data stream on the port, if it's known: `Option<Continuous>`, say. The node works out its
//! outputs' streams from its inputs' with these (see `Node::output_streams`), so a port's kind and its stream can't
//! disagree. With `Buffers`, each field holds the port's buffer in a pass: `Option<&ContinuousBuffer>` for an input,
//! `&mut ContinuousBuffer` for an output. The node's `process` is given these, already the right type.
//!
//! The graph and the plan still number ports, in the order they're declared, and the engine still holds streams and
//! buffers as the program's enums of them; `DataStreamInPorts` and `DataStreamOutPorts` go between the two, through `StreamOf`.

use std::fmt::Debug;
use std::marker::PhantomData;

use super::{Buffer, DataStreamType, Edges, Port, StreamOf};

/// A node's `DataStreamIn` and `DataStreamOut` structs (see `ports!`) are generic over a form, which says what each field
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

/// `ports!` implements this for a node's struct of `DataStreamIn` ports, in the `Streams` form, which is what its
/// `PortSet::DataStreamIn` names.
pub trait DataStreamInPorts<E: Edges>: Copy + PartialEq + Debug + Default + Send + Sync + 'static {
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

/// `ports!` implements this for a node's struct of `DataStreamOut` ports, in the `Streams` form, which is what its
/// `PortSet::DataStreamOut` names.
pub trait DataStreamOutPorts<E: Edges>: Copy + PartialEq + Debug + Default + Send + Sync + 'static {
    /// The struct in the `Buffers` form, which the node's `process` is given.
    type View<'a>;

    /// For each output, the input the node makes it in place of, if any (see `BufferUsage::in_place`).
    const IN_PLACE: &'static [Option<usize>];

    /// The graph lists the node's outputs, with their streams, from this, in the order they're declared.
    fn ports(&self) -> Vec<Port<E>>;

    /// A pass has a buffer for each output, made for its stream. This is the struct of them.
    fn view<'a>(outputs: &'a mut [Buffer<E>]) -> Self::View<'a>;
}

/// A node without ports of a kind has this for them (see `PortSet`): for its `DataStreamIn` ports, say. A node with no
/// ports at all names it as its `Node::Ports`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct NoPorts;

impl<E: Edges> DataStreamInPorts<E> for NoPorts {
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

impl<E: Edges> DataStreamOutPorts<E> for NoPorts {
    type View<'a> = NoPorts;
    const IN_PLACE: &'static [Option<usize>] = &[];

    fn ports(&self) -> Vec<Port<E>> {
        Vec::new()
    }

    fn view(_outputs: &mut [Buffer<E>]) -> NoPorts {
        NoPorts
    }
}

/// `ports!` finds an input's buffer to read with this: the one the node was given to take, if it was given one,
/// or else the one to read.
#[doc(hidden)]
pub fn read_input<'a, E: Edges, T: StreamOf<E>>(
    shared: Option<&'a Option<&'a Buffer<E>>>,
    taken: Option<&'a mut Option<Buffer<E>>>,
) -> Option<&'a T::Buffer> {
    let taken = taken.map(|taken| -> &'a Option<Buffer<E>> { taken });
    taken.and_then(Option::as_ref).or(shared.copied().flatten()).and_then(T::from_buffer)
}

/// `ports!` finds an input's buffer to take with this.
#[doc(hidden)]
pub fn take_input<'a, E: Edges, T: StreamOf<E>>(
    _shared: Option<&'a Option<&'a Buffer<E>>>,
    taken: Option<&'a mut Option<Buffer<E>>>,
) -> Option<&'a mut T::Buffer> {
    taken.and_then(Option::as_mut).and_then(T::from_buffer_mut)
}

/// `ports!` finds an output's buffer with this. The plan makes each output's buffer from the stream the node gave
/// for it, which is of the output's own type, so the buffer always is too.
#[doc(hidden)]
pub fn fill_output<E: Edges, T: StreamOf<E>>(output: Option<&mut Buffer<E>>) -> &mut T::Buffer {
    output
        .and_then(T::from_buffer_mut)
        .expect("the plan makes each output's buffer for the stream the node gave for it")
}
