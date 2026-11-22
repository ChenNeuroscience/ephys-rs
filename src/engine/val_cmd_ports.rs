//! A node declares its ValCmd ports on its type with `ports!` (see `crate::engine::ports`), as it does its data ports.
//! This has what they're built on.
//!
//! - **`ValOutCmdIn`** ports: the node publishes a value at each, and takes commands through it. `ports!` makes a
//!   struct of the values (`ValOutCmdIn`), which the node gives with `Node::val_out`, and an enum of every command they
//!   take (`CmdIn`), which the app hands the node each command as (see `Node::on_command`), so the node matches every
//!   command it takes, and none it doesn't.
//! - **`ValInCmdOut`** and **`ValIn`** ports: the node reads another node's value at each, and through a `ValInCmdOut`
//!   sends it commands. `ports!` makes a handle to each (`ValInCmdOut::gains`, say, a `ValInCmdOutPort`; or a
//!   `ValInPort`), which the contexts read and command through (`UiContext::val_in`, `UiContext::cmd_out`). A
//!   `ValInCmdOut` port can send only the commands it's declared with.
//!
//! Each port lists its commands as a tuple of their types: `(SetGains, Detect)`, or `()` for none.

use std::fmt::Debug;
use std::marker::PhantomData;

use super::{CommandKind, CommandOf, Edges, NoPorts, Port, ValueOf};

/// A port lists the commands it takes or sends as a tuple of their types. This lists their kinds, for the graph (see
/// `Port::commands`).
pub trait CommandList<E: Edges> {
    fn kinds() -> Vec<CommandKind<E>>;
}

impl<E: Edges> CommandList<E> for () {
    fn kinds() -> Vec<CommandKind<E>> {
        Vec::new()
    }
}

impl<E: Edges, A: CommandOf<E>> CommandList<E> for (A,) {
    fn kinds() -> Vec<CommandKind<E>> {
        vec![A::KIND]
    }
}

impl<E: Edges, A: CommandOf<E>, B: CommandOf<E>> CommandList<E> for (A, B) {
    fn kinds() -> Vec<CommandKind<E>> {
        vec![A::KIND, B::KIND]
    }
}

impl<E: Edges, A: CommandOf<E>, B: CommandOf<E>, C: CommandOf<E>> CommandList<E> for (A, B, C) {
    fn kinds() -> Vec<CommandKind<E>> {
        vec![A::KIND, B::KIND, C::KIND]
    }
}

impl<E: Edges, A: CommandOf<E>, B: CommandOf<E>, C: CommandOf<E>, D: CommandOf<E>> CommandList<E> for (A, B, C, D) {
    fn kinds() -> Vec<CommandKind<E>> {
        vec![A::KIND, B::KIND, C::KIND, D::KIND]
    }
}

/// Where a command is in a tuple of them (see `Includes`): the first.
pub struct Here;
/// Where a command is in a tuple of them (see `Includes`): after the first, at `I` in the rest.
pub struct There<I>(PhantomData<I>);

/// A `ValInCmdOut` port (or a sender kept for one) can only send the commands its tuple lists. A tuple of commands
/// implements this for each it lists (`I` saying where, which the compiler works out).
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

/// `ports!` implements this for a node's enum of the commands it takes (`CmdIn`), which its `PortSet::CmdIn` names.
pub trait CommandSet<E: Edges>: Debug + Send + Sized + 'static {
    /// `command` as one of the node's, if it is, or else `command` back.
    fn from_command(command: E::Command) -> Result<Self, E::Command>;
    /// The command, as the program's enum of them.
    fn wrap(self) -> E::Command;
}

/// A node that takes no commands (it has no `ValOutCmdIn` ports) has this for them: it can't be made, so its
/// `on_command` is `match command {}`.
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

/// A node reads another node's value, and sends it commands, through a `ValInCmdOut` port: this is the handle to one,
/// reading a value of type `V` and sending the commands in the tuple `S`. `ports!` makes one for each such port, as a
/// constant named as the port: `ValInCmdOut::gains`, say.
pub struct ValInCmdOutPort<V, S> {
    index: usize,
    types: PhantomData<fn() -> (V, S)>,
}

/// A node only reads another node's value through a `ValIn` port: this is the handle to one, reading a value of type
/// `V`. `ports!` makes one for each such port, as a constant named as the port: `ValIn::gains`, say.
pub struct ValInPort<V> {
    index: usize,
    types: PhantomData<fn() -> V>,
}

impl<V, S> ValInCmdOutPort<V, S> {
    /// `ports!` makes a handle for the node's port `index`, as the node numbers them (its `ValInCmdOut` and `ValIn`
    /// ports together, in the order declared).
    #[doc(hidden)]
    pub const fn new(index: usize) -> Self {
        ValInCmdOutPort { index, types: PhantomData }
    }
}

impl<V> ValInPort<V> {
    /// See `ValInCmdOutPort::new`.
    #[doc(hidden)]
    pub const fn new(index: usize) -> Self {
        ValInPort { index, types: PhantomData }
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

/// The contexts read a value through a handle to either kind of port reading one (`ValInCmdOutPort` or `ValInPort`)
/// with this.
pub trait ReadsValIn: Copy {
    /// The type of value the port reads.
    type Value;
    /// The port's number, as the node numbers them (see `ValInCmdOutPort::new`).
    fn index(self) -> usize;
}

impl<V, S> ReadsValIn for ValInCmdOutPort<V, S> {
    type Value = V;

    fn index(self) -> usize {
        self.index
    }
}

impl<V> ReadsValIn for ValInPort<V> {
    type Value = V;

    fn index(self) -> usize {
        self.index
    }
}

/// `ports!` makes each `ValInCmdOut` and `ValIn` port, for the graph, through this.
#[doc(hidden)]
pub trait ValInPortOf<E: Edges> {
    fn port(name: &'static str, flipped: bool) -> Port<E>;
}

impl<E: Edges, V: ValueOf<E>, S: CommandList<E>> ValInPortOf<E> for ValInCmdOutPort<V, S> {
    fn port(name: &'static str, flipped: bool) -> Port<E> {
        Port::ValInCmdOut { name, value_kind: V::KIND, commands: S::kinds(), flipped }
    }
}

impl<E: Edges, V: ValueOf<E>> ValInPortOf<E> for ValInPort<V> {
    fn port(name: &'static str, flipped: bool) -> Port<E> {
        Port::ValIn { name, value_kind: V::KIND, flipped }
    }
}

/// `ports!` implements this for a node's struct of the values at its `ValOutCmdIn` ports (`ValOutCmdIn`), which its
/// `PortSet::ValOutCmdIn` names.
pub trait ValOutCmdInPorts<E: Edges>: Sized {
    /// The graph lists the node's `ValOutCmdIn` ports from this, in the order they're declared.
    fn ports() -> Vec<Port<E>>;
    /// The values, as the program's enum of them, in the order they're declared.
    fn values(self) -> Vec<E::PublishedValue>;
}

impl<E: Edges> ValOutCmdInPorts<E> for NoPorts {
    fn ports() -> Vec<Port<E>> {
        Vec::new()
    }

    fn values(self) -> Vec<E::PublishedValue> {
        Vec::new()
    }
}
