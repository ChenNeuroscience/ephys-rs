//! The building blocks for data ports. A node declares its ports on its type with `ports!` (see
//! `crate::engine::ports`), which generates a struct of the node's `DataStreamIn` ports and a struct of its
//! `DataStreamOut` ports. Each field is one port, typed by the data stream the port carries. Everything else about the
//! data ports follows from those structs: the list of ports the graph shows, in declaration order; which inputs the
//! node takes and which outputs it writes in place (see `BufferUsage`); and the typed views the node works with.
//!
//! Each struct is generic over a form (`PortForm`), which decides what each field holds. In the `Streams` form (the
//! default), each field holds the data stream on the port, if known, such as `Option<Continuous>`. The node computes
//! its output streams from its input streams in this form (see `Node::output_streams`), so a port's kind and its
//! stream can't disagree. In the `Buffers` form, each field holds the port's buffer during a pass:
//! `Option<&ContinuousBuffer>` for an input and `&mut ContinuousBuffer` for an output. The node's `process` receives
//! the buffers already converted to the right types.
//!
//! The graph and the plan still number ports in declaration order, and the engine still stores streams and buffers as
//! the program's enums. `DataStreamInPorts` and `DataStreamOutPorts` convert between the two, using `HasDataStream`.

use std::fmt::Debug;
use std::marker::PhantomData;

use super::{Buffer, DataStreamType, Edges, HasDataStream, Port};

/// The form of a node's `DataStreamIn` and `DataStreamOut` structs (see `ports!`), which decides what each field holds.
/// A field's type depends on the port's role: an input the node reads (`In`), an input the node takes (`Take`), or an
/// output (`Out`).
pub trait PortForm {
    type In<T: DataStreamType>;
    type Take<T: DataStreamType>;
    type Out<T: DataStreamType>;
}

/// The form in which each field holds the data stream on its port, as the graph works it out. The field is `None` when
/// the stream isn't known: an unconnected input has no stream, and usually neither do the outputs computed from it.
pub struct Streams;

impl PortForm for Streams {
    type In<T: DataStreamType> = Option<T>;
    type Take<T: DataStreamType> = Option<T>;
    type Out<T: DataStreamType> = Option<T>;
}

/// The form in which each field holds its port's buffer during a pass. An input that the node reads holds a shared
/// reference, which is `None` if the input isn't connected or if an output is being written in place of it. An input
/// that the node takes holds a mutable reference, which is `None` if the input isn't connected. An output holds a
/// mutable reference to the buffer the node fills.
pub struct Buffers<'a>(PhantomData<&'a mut ()>);

impl<'a> PortForm for Buffers<'a> {
    type In<T: DataStreamType> = Option<&'a T::Buffer>;
    type Take<T: DataStreamType> = Option<&'a mut T::Buffer>;
    type Out<T: DataStreamType> = &'a mut T::Buffer;
}

/// A node's struct of `DataStreamIn` ports, in the `Streams` form (which `PortSet::DataStreamIn` names). `ports!`
/// implements this trait for each node's struct.
pub trait DataStreamInPorts<E: Edges>: Copy + PartialEq + Debug + Default + Send + Sync + 'static {
    /// The same struct in the `Buffers` form, which the node's `process` receives.
    type View<'a>;

    /// Whether the node takes each input's buffer (see `BufferUsage::takes`): true for inputs declared `take`.
    const TAKES: &'static [bool];

    /// The node's inputs, in declaration order, for the graph.
    fn ports() -> Vec<Port<E>>;

    /// Builds the struct from the graph's list of the stream on each input (if any), in declaration order.
    fn of(streams: &[Option<E::DataStream>]) -> Self;

    /// Converts the struct back to the graph's list of streams (the reverse of `of`).
    fn streams(&self) -> Vec<Option<E::DataStream>>;

    /// Builds the `Buffers` form for a pass. Each input's buffer is in `shared`, for the node to read, or in `taken`, if
    /// the node takes the input.
    fn view<'a>(shared: &'a [Option<&'a Buffer<E>>], taken: &'a mut [Option<Buffer<E>>]) -> Self::View<'a>;
}

/// A node's struct of `DataStreamOut` ports, in the `Streams` form (which `PortSet::DataStreamOut` names). `ports!`
/// implements this trait for each node's struct.
pub trait DataStreamOutPorts<E: Edges>: Copy + PartialEq + Debug + Default + Send + Sync + 'static {
    /// The same struct in the `Buffers` form, which the node's `process` receives.
    type View<'a>;

    /// For each output, the number of the input whose buffer the node writes the output into, if any (see
    /// `BufferUsage::in_place`).
    const IN_PLACE: &'static [Option<usize>];

    /// The node's outputs with their streams, in declaration order, for the graph.
    fn ports(&self) -> Vec<Port<E>>;

    /// Builds the `Buffers` form for a pass from the outputs' buffers, each made for the output's stream.
    fn view<'a>(outputs: &'a mut [Buffer<E>]) -> Self::View<'a>;
}

/// The port type for a role in which a node has no ports (see `PortSet`). For example, a viewer with no outputs uses
/// `NoPorts` for its `DataStreamOut`. A node with no ports at all names `NoPorts` as its `Node::Ports`.
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

/// Finds the buffer of an input that the node reads. The plan sometimes hands the node an input's buffer to own even
/// though the node only reads it; this uses that buffer if there is one, and otherwise the shared buffer. Only
/// `ports!` calls this.
#[doc(hidden)]
pub fn read_input<'a, E: HasDataStream<T>, T: DataStreamType>(
    shared: Option<&'a Option<&'a Buffer<E>>>,
    taken: Option<&'a mut Option<Buffer<E>>>,
) -> Option<&'a T::Buffer> {
    let taken = taken.map(|taken| -> &'a Option<Buffer<E>> { taken });
    taken.and_then(Option::as_ref).or(shared.copied().flatten()).and_then(E::from_buffer)
}

/// Finds the buffer of an input that the node takes. Only `ports!` calls this.
#[doc(hidden)]
pub fn take_input<'a, E: HasDataStream<T>, T: DataStreamType>(
    _shared: Option<&'a Option<&'a Buffer<E>>>,
    taken: Option<&'a mut Option<Buffer<E>>>,
) -> Option<&'a mut T::Buffer> {
    taken.and_then(Option::as_mut).and_then(E::from_buffer_mut)
}

/// Finds an output's buffer. The plan makes each output's buffer from the stream that the node's `output_streams`
/// returned for the output, and that stream has the output's declared type, so the buffer has the matching type too.
/// Only `ports!` calls this.
#[doc(hidden)]
pub fn fill_output<E: HasDataStream<T>, T: DataStreamType>(output: Option<&mut Buffer<E>>) -> &mut T::Buffer {
    output
        .and_then(E::from_buffer_mut)
        .expect("the plan makes each output's buffer for the stream the node gave for it")
}
