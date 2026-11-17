//! Nodes command each other through value edges (see `crate::engine::PortRole`), and themselves, from their passes and
//! threads. A command goes on its way through the app's queue (`Commands`), which delivers it to the commanded node's
//! `Node::on_command` on the GUI's thread, as soon as it can, whether anything's running or not: from a thread that's
//! outlived the run that started it, say.
//!
//! A command sent through a value edge is addressed to the edge, not the node at its far end: to whatever node `from`'s
//! `ValInCmdOut` port `port` connects to (`Address::Port`). The app follows the edge as it delivers the command, so a
//! sender kept for a while (a writer's, for when its files are finished: see `CmdSender`) sends to where the edge points
//! then, or nowhere, if it's been disconnected.
//!
//! A pass can also ask the app to start or stop recording (`PassContext::request_recording`), as the toolbar's Record
//! does. That goes through the queue too, though it isn't a command to a node.
//!
//! On their way, commands are carried as `Box<dyn Any>`, so the queue (and the run, which holds it) needn't know the
//! program's enum of them; what's sent and what's taken is always the enum.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;

use crate::engine::NodeId;

/// A command goes to a node through one of the node's value edges, or from the node to itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Address {
    /// Whatever node `from`'s `ValInCmdOut` port `port` (by index among them) connects to, when the command's delivered.
    Port { from: NodeId, port: usize },
    /// The node itself, from its passes or a thread of its own.
    Itself(NodeId),
}

/// What waits in the queue to be delivered.
pub enum Sent {
    /// A command (of the program's enum), to the node at `Address`.
    Command(Address, Box<dyn Any + Send>),
    /// The node asks for recording to start (true) or stop, if it isn't already.
    Recording(NodeId, bool),
}

/// Commands (and requests to record) wait here to be delivered. This is the app's, so they outlive any run.
#[derive(Default)]
pub struct Commands {
    waiting: Mutex<Vec<Sent>>,
    /// When a command comes, this is told, as a stopped app might not be drawing anything.
    wake: OnceLock<egui::Context>,
}

impl Commands {
    /// The app has `ctx` draw a frame when a command comes, for it to be delivered, with this.
    pub fn wake(&self, ctx: &egui::Context) {
        self.wake.get_or_init(|| ctx.clone());
    }

    fn push(&self, sent: Sent) {
        self.waiting.lock().unwrap().push(sent);
        if let Some(ctx) = self.wake.get() {
            ctx.request_repaint();
        }
    }

    /// This sends `command` (of the program's enum of commands) to the node at `address`.
    pub fn send<C: Any + Send>(&self, address: Address, command: C) {
        self.push(Sent::Command(address, Box::new(command)));
    }

    /// This asks the app, for node `node`, to start (`on`) or stop recording.
    pub fn request_recording(&self, node: NodeId, on: bool) {
        self.push(Sent::Recording(node, on));
    }

    /// The app takes what's waiting, to deliver, with this.
    pub fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.waiting.lock().unwrap())
    }

    /// Tests take the commands waiting with this: where each is going, and the command, as the program's `C`.
    pub fn take_commands<C: Any>(&self) -> Vec<(Address, C)> {
        let commands = self.take().into_iter().filter_map(|sent| match sent {
            Sent::Command(address, command) => Some((address, *command.downcast::<C>().ok()?)),
            Sent::Recording(..) => None,
        });
        commands.collect()
    }
}

impl Drop for Commands {
    /// Whatever's left when this is dropped never got to its node: the app closed first, while a thread finished up.
    fn drop(&mut self) {
        for sent in self.take() {
            if let Sent::Command(address, _) = sent {
                log::warn!("A command for {address:?} was sent as the app closed, and wasn't delivered");
            }
        }
    }
}

/// A node keeps one of these to send commands of the program's `C` later, from any thread: through one of its value
/// edges (a writer's "after recording", say), or to itself.
pub struct CmdSender<C> {
    commands: Arc<Commands>,
    address: Address,
    command: PhantomData<fn(C)>,
}

impl<C> Clone for CmdSender<C> {
    fn clone(&self) -> Self {
        CmdSender { commands: self.commands.clone(), address: self.address, command: PhantomData }
    }
}

impl<C: Any + Send> CmdSender<C> {
    /// A sender of commands to the node at `address`, through `commands`.
    pub fn new(commands: Arc<Commands>, address: Address) -> Self {
        CmdSender { commands, address, command: PhantomData }
    }

    /// This sends `command`. The app delivers it as soon as it can, following the edge then (see `Address`).
    pub fn send(&self, command: C) {
        self.commands.send(self.address, command);
    }

    /// Where the sender sends.
    pub fn address(&self) -> Address {
        self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_sends_through_the_queue_to_its_address() {
        let commands = Arc::new(Commands::default());
        let address = Address::Port { from: NodeId(1), port: 2 };
        let sender = CmdSender::<u32>::new(commands.clone(), address);
        sender.send(7);
        sender.clone().send(8);
        CmdSender::<u32>::new(commands.clone(), Address::Itself(NodeId(3))).send(9);
        commands.request_recording(NodeId(4), true);
        let sent = commands.take_commands::<u32>();
        assert_eq!(sent, [(address, 7), (address, 8), (Address::Itself(NodeId(3)), 9)]);
        assert!(commands.take().is_empty(), "taken, the request to record with them");
    }
}
