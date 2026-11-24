//! The building blocks for ValCmd ports. A node declares its ValCmd ports with `ports!`, as it does its data ports
//! (see `crate::engine::ports`).
//!
//! - **`ValOutCmdIn`** ports: the node offers a value out at each port and accepts commands through the port. `ports!`
//!   generates a struct of the values (`ValOutCmdIn`), which the node returns from `Node::val_out`. It also generates
//!   an enum of every command those ports accept (`CmdIn`), which the app passes to `Node::on_command`. The node must
//!   match every command it accepts, and can't receive any other.
//! - **`ValInCmdOut`** and **`ValIn`** ports: the node reads another node's value through each port, and can send
//!   that node commands through a `ValInCmdOut` port. `ports!` generates a handle for each port (such as
//!   `ValInCmdOut::gains`, a `ValInCmdOutPort`, or a `ValInPort`). The contexts take the handle to read the value and
//!   send commands (`UiContext::val_in`, `UiContext::cmd_out`). A `ValInCmdOut` port can send only the commands it's
//!   declared with.
//!
//! Each port lists its commands as a tuple of their types, such as `(SetGains, Detect)`, or `()` for none.

use std::fmt::Debug;
use std::marker::PhantomData;

use super::{CommandKind, EdgeType, Edges, HasCommand, HasValue, NoPorts, Port, Side};

/// Each command is a type of its own, holding the data the command carries (see `HasCommand`). Any `Debug + Send`
/// type can be a command.
pub trait Command: std::fmt::Debug + Send + 'static {}

impl<T: std::fmt::Debug + Send + 'static> Command for T {}

/// A tuple of command types, as a port declares the commands it accepts or sends. `kinds` converts the tuple to the
/// program's command kinds, for the graph (see `Port::commands`).
pub trait CommandList<E: Edges> {
    fn kinds() -> Vec<CommandKind<E>>;
}

impl<E: Edges> CommandList<E> for () {
    fn kinds() -> Vec<CommandKind<E>> {
        Vec::new()
    }
}

impl<E: HasCommand<A>, A: Command> CommandList<E> for (A,) {
    fn kinds() -> Vec<CommandKind<E>> {
        vec![<E as HasCommand<A>>::KIND]
    }
}

impl<E: HasCommand<A> + HasCommand<B>, A: Command, B: Command> CommandList<E> for (A, B) {
    fn kinds() -> Vec<CommandKind<E>> {
        vec![<E as HasCommand<A>>::KIND, <E as HasCommand<B>>::KIND]
    }
}

impl<E, A: Command, B: Command, C: Command> CommandList<E> for (A, B, C)
where
    E: HasCommand<A> + HasCommand<B> + HasCommand<C>,
{
    fn kinds() -> Vec<CommandKind<E>> {
        vec![<E as HasCommand<A>>::KIND, <E as HasCommand<B>>::KIND, <E as HasCommand<C>>::KIND]
    }
}

impl<E, A: Command, B: Command, C: Command, D: Command> CommandList<E> for (A, B, C, D)
where
    E: HasCommand<A> + HasCommand<B> + HasCommand<C> + HasCommand<D>,
{
    fn kinds() -> Vec<CommandKind<E>> {
        vec![
            <E as HasCommand<A>>::KIND,
            <E as HasCommand<B>>::KIND,
            <E as HasCommand<C>>::KIND,
            <E as HasCommand<D>>::KIND,
        ]
    }
}

/// Marks a command's position in a tuple of commands (see `Includes`): the first element.
pub struct Here;
/// Marks a command's position in a tuple of commands (see `Includes`): position `I` among the elements after the
/// first.
pub struct There<I>(PhantomData<I>);

/// Restricts a `ValInCmdOut` port, and any sender made from the port, to the commands in the port's tuple. A tuple of
/// commands implements `Includes<C, I>` for each command `C` it contains. `I` is the command's position (`Here` or
/// `There<...>`), which the compiler infers, so callers never write it.
#[diagnostic::on_unimplemented(
    message = "`{C}` isn't one of the commands `{Self}` lists",
    label = "this port doesn't send `{C}`",
    note = "add it to the commands the port's declared with, in its node's `ports!`"
)]
pub trait Includes<C, I> {}

impl<A> Includes<A, Here> for (A,) {}
impl<A, B> Includes<A, Here> for (A, B) {}
impl<A, B> Includes<B, There<Here>> for (A, B) {}
impl<A, B, C> Includes<A, Here> for (A, B, C) {}
impl<A, B, C> Includes<B, There<Here>> for (A, B, C) {}
impl<A, B, C> Includes<C, There<There<Here>>> for (A, B, C) {}
impl<A, B, C, D> Includes<A, Here> for (A, B, C, D) {}
impl<A, B, C, D> Includes<B, There<Here>> for (A, B, C, D) {}
impl<A, B, C, D> Includes<C, There<There<Here>>> for (A, B, C, D) {}
impl<A, B, C, D> Includes<D, There<There<There<Here>>>> for (A, B, C, D) {}

/// Converts between a node's own enum of the commands it accepts (`CmdIn`, which `PortSet::CmdIn` names) and the
/// program's enum of all commands. `ports!` implements this trait for each node's `CmdIn`.
pub trait CommandSet<E: Edges>: Debug + Send + Sized + 'static {
    /// Converts `command` to the node's enum if the node accepts it, or returns `command` unchanged if not.
    fn from_command(command: E::Command) -> Result<Self, E::Command>;
    /// Converts the command to the program's enum of all commands.
    fn wrap(self) -> E::Command;
}

/// The command enum of a node with no `ValOutCmdIn` ports, which accepts no commands. `NoCommands` has no variants,
/// so no value of it can exist, and the node's `on_command` is just `match command {}`.
#[derive(Debug)]
pub enum NoCommands {}

impl<E: Edges> CommandSet<E> for NoCommands {
    fn from_command(command: E::Command) -> Result<Self, E::Command> {
        Err(command)
    }

    fn wrap(self) -> E::Command {
        match self {}
    }
}

/// A handle to a `ValInCmdOut` port, through which a node reads another node's value of type `V` and sends it the
/// commands in the tuple `S`. `ports!` generates one handle for each such port, as a constant named after the port
/// (for example, `ValInCmdOut::gains`).
pub struct ValInCmdOutPort<V, S> {
    index: usize,
    types: PhantomData<fn() -> (V, S)>,
}

/// A handle to a `ValIn` port, through which a node reads another node's value of type `V` but sends no commands.
/// `ports!` generates one handle for each such port, as a constant named after the port (for example,
/// `ValIn::gains`).
pub struct ValInPort<V> {
    index: usize,
    types: PhantomData<fn() -> V>,
}

impl<V, S> ValInCmdOutPort<V, S> {
    /// Makes the handle for the node's port number `index`. The node numbers its `ValInCmdOut` and `ValIn` ports
    /// together, in declaration order. Only `ports!` calls this.
    #[doc(hidden)]
    pub const fn new(index: usize) -> Self {
        ValInCmdOutPort { index, types: PhantomData }
    }

    /// The port's number among the node's `ValInCmdOut` and `ValIn` ports (see `new`).
    pub const fn index(self) -> usize {
        self.index
    }
}

impl<V> ValInPort<V> {
    /// Makes the handle for the node's port number `index` (see `ValInCmdOutPort::new`). Only `ports!` calls this.
    #[doc(hidden)]
    pub const fn new(index: usize) -> Self {
        ValInPort { index, types: PhantomData }
    }

    /// The port's number among the node's `ValInCmdOut` and `ValIn` ports (see `ValInCmdOutPort::new`).
    pub const fn index(self) -> usize {
        self.index
    }
}

impl<V, S> Clone for ValInCmdOutPort<V, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V, S> Copy for ValInCmdOutPort<V, S> {}

impl<V> Clone for ValInPort<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for ValInPort<V> {}

/// A handle to a port that reads a value: either a `ValInCmdOutPort` or a `ValInPort`. The contexts read values
/// through any handle implementing this trait.
pub trait ReadsValIn: Copy {
    /// The type of value the port reads.
    type Value: EdgeType;
    /// The port's number among the node's `ValInCmdOut` and `ValIn` ports (see `ValInCmdOutPort::new`).
    fn index(self) -> usize;
}

impl<V: EdgeType, S> ReadsValIn for ValInCmdOutPort<V, S> {
    type Value = V;

    fn index(self) -> usize {
        self.index
    }
}

impl<V: EdgeType> ReadsValIn for ValInPort<V> {
    type Value = V;

    fn index(self) -> usize {
        self.index
    }
}

/// Builds the graph's description (`Port`) of a `ValInCmdOut` or `ValIn` port from the port's handle type. Only
/// `ports!` uses this.
#[doc(hidden)]
pub trait ValInPortOf<E: Edges> {
    fn port(id: &'static str, name: &'static str, side: Side) -> Port<E>;
}

impl<E: HasValue<V>, V: EdgeType, S: CommandList<E>> ValInPortOf<E> for ValInCmdOutPort<V, S> {
    fn port(id: &'static str, name: &'static str, side: Side) -> Port<E> {
        Port::ValInCmdOut { name, id, value_kind: <E as HasValue<V>>::KIND, commands: S::kinds(), side }
    }
}

impl<E: HasValue<V>, V: EdgeType> ValInPortOf<E> for ValInPort<V> {
    fn port(id: &'static str, name: &'static str, side: Side) -> Port<E> {
        Port::ValIn { name, id, value_kind: <E as HasValue<V>>::KIND, side }
    }
}

/// A node's struct of its values out, at its `ValOutCmdIn` ports (`ValOutCmdIn`, which `PortSet::ValOutCmdIn`
/// names). `ports!` implements this trait for each node's struct.
pub trait ValOutCmdInPorts<E: Edges>: Sized {
    /// The node's `ValOutCmdIn` ports, in declaration order, for the graph.
    fn ports() -> Vec<Port<E>>;
    /// The values, each converted to the program's enum of values, in declaration order.
    fn values(self) -> Vec<E::ValOut>;
}

impl<E: Edges> ValOutCmdInPorts<E> for NoPorts {
    fn ports() -> Vec<Port<E>> {
        Vec::new()
    }

    fn values(self) -> Vec<E::ValOut> {
        Vec::new()
    }
}
