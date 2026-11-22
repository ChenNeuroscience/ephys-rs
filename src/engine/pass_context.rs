//! The engine runs the graph in passes, each covering `PASS_DURATION` of time, which a worker runs through every node
//! in the plan's order (see `run`). As a pass reaches a node, the node's `process` is given its inputs' and outputs'
//! buffers (as the node declared them: see `crate::engine::data_ports`), and its context in the pass (`PassContext`):
//! the values its ports read, the snapshot the pass runs against (as nodes see it: whether it's recording, say), what
//! it shares with the GUI, and where it sends commands.

use std::any::Any;
use std::sync::Arc;

use super::{Buffer, Edges, NodeId, SnapshotLite, ValueType};
use crate::commands::{Address, Commands};

/// Each pass covers this many seconds of data.
pub const PASS_DURATION: f64 = 0.02;

/// As a pass reaches a node, the engine takes the node's buffers out of the worker's, as the plan says, in the order the
/// node's ports are declared. The engine hands these to the program's `NodesEnum::process`, which gives them to the
/// node's `process` as the node's own types (see `InputPorts::view` and `OutputPorts::view`).
pub struct PassBuffers<'a, E: Edges> {
    /// For each input, the pass's data on it, if it's connected (and the node isn't taking it).
    pub inputs: &'a [Option<&'a Buffer<E>>],
    /// For each input the node takes (see `BufferUsage::takes`), the input's buffer.
    pub taken: &'a mut [Option<Buffer<E>>],
    /// For each output, a buffer for the node to fill.
    pub outputs: &'a mut [Buffer<E>],
}

/// As a pass reaches a node, the engine makes this for it, and gives it to the node's `process`, besides the node's
/// buffers.
pub struct PassContext<'a, E: Edges> {
    /// The engine numbers passes from 0, each covering the next `duration` seconds. (No node needs the number just
    /// now.)
    pub sequence: u64,
    pub duration: f64,
    /// The node the pass is running.
    pub node: NodeId,
    /// For each of the node's `ValInCmdOut` and `ValIn` ports, what the node at the other end published as the snapshot
    /// was made, if it's connected (which a writer records, say: see `val_in`).
    pub val_ins: &'a [Option<E::PublishedValue>],
    /// The pass runs against a snapshot, which nodes can see some of (see `snapshot`).
    pub(super) snapshot: &'a Arc<dyn SnapshotLite>,
    /// The node shares this with the GUI (its `Node::Shared`: see `shared`).
    pub shared: &'a dyn Any,
    /// The app takes commands from passes here (see `cmd_self`).
    pub commands: &'a Commands,
}

impl<'a, E: Edges> PassContext<'a, E> {
    /// Tests try nodes out on a pass like this: not recording, with no `ValInCmdOut` or `ValIn` ports connected.
    #[cfg(test)]
    pub fn new(sequence: u64, duration: f64) -> Self {
        let commands = Box::leak(Box::default());
        let (node, val_ins, shared) = (NodeId(0), &[], &());
        let snapshot = Self::trying(false);
        PassContext { sequence, duration, node, val_ins, snapshot, shared, commands }
    }

    /// Tests try nodes out against a snapshot like this: running as of now, and recording or not.
    #[cfg(test)]
    fn trying(recording: bool) -> &'a Arc<dyn SnapshotLite> {
        let recording = recording.then(|| {
            let recording = super::Recording::default();
            recording.take_effect(std::time::Instant::now());
            Arc::new(recording)
        });
        let trying: Arc<dyn SnapshotLite> =
            Arc::new(super::snapshot_lite::Trying { started: std::time::Instant::now(), recording });
        Box::leak(Box::new(trying))
    }

    /// Tests trying a node out can have the pass record, or not, with this.
    #[cfg(test)]
    pub fn set_recording(&mut self, recording: bool) {
        self.snapshot = Self::trying(recording);
    }

    /// The node can ask whether the graph is recording.
    pub fn recording(&self) -> bool {
        self.snapshot.recording().is_some()
    }

    /// Nodes can see some of the snapshot the pass runs against (`SnapshotLite`). A node's own thread, which carries on
    /// between passes, can keep it (the newest it's been given, say), and ask how long the graph's been acquiring and
    /// recording, as of whenever it asks.
    pub fn snapshot(&self) -> &'a Arc<dyn SnapshotLite> {
        self.snapshot
    }

    /// The node finds what it shares with the GUI here: its `Node::Shared`, which is `T`.
    pub fn shared<T: 'static>(&self) -> &'a T {
        self.shared.downcast_ref().expect("the node's own Node::Shared")
    }

    /// The node reads what its `ValInCmdOut` (or `ValIn`) port `val_in_idx` saw as the snapshot was made with this, as
    /// the type it expects, if the port's connected.
    pub fn val_in<T: ValueType<E::PublishedValue> + 'a>(&self, val_in_idx: usize) -> Option<&'a T> {
        T::from_value(self.val_ins.get(val_in_idx)?.as_ref()?)
    }

    /// Passes can't change the graph themselves, so a node sends itself `command` here (to set one of its settings from
    /// what its passes found, say), which the app delivers to the node's `Node::on_command` on the GUI's thread (see
    /// `crate::commands`).
    pub fn cmd_self(&self, command: E::Command) {
        self.commands.send(Address::Itself(self.node), command);
    }

    /// A node can have the app start (`on`) or stop recording, as the toolbar's Record does, if it isn't already.
    /// Whatever any node asks last is what happens. See `crate::commands`.
    pub fn request_recording(&self, on: bool) {
        self.commands.request_recording(self.node, on);
    }

    /// While recording, a writer finds here the base directory the recording goes in (see `crate::base_directory`), if
    /// there is one.
    pub fn base_directory(&self) -> Option<&crate::base_directory::Place> {
        self.snapshot.recording()?.base_directory()
    }
}
