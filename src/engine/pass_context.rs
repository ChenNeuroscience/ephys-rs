//! The engine runs the graph in passes, each covering `PASS_DURATION` of time, which a worker runs through every node
//! in the plan's order (see `run`). As a pass reaches a node, the node's `process` is given its context in the pass
//! (`PassContext`): its inputs, buffers for its outputs, the values its ports read, the snapshot the pass runs against
//! (as nodes see it: whether it's recording, say), what it shares with the GUI, and where it sends commands.

use std::any::Any;
use std::sync::Arc;

use super::{Buffer, Edges, NodeId, SnapshotLite, ValueType};
use crate::commands::{Address, Commands};

/// Each pass covers this many seconds of data.
pub const PASS_DURATION: f64 = 0.02;

/// As a pass reaches a node, the engine makes this for it, and gives it to the node's `process`.
pub struct PassContext<'a, E: Edges> {
    /// The engine numbers passes from 0, each covering the next `duration` seconds. (No node needs the number just
    /// now.)
    pub sequence: u64,
    pub duration: f64,
    /// The node the pass is running.
    pub node: NodeId,
    /// For each input, the pass's data on it, if it's connected (and the node isn't taking it: see `take_input`).
    pub inputs: &'a [Option<&'a Buffer<E>>],
    /// For each input the node takes (see `BufferUsage::takes`), the input's buffer.
    pub(super) taken: &'a mut [Option<Buffer<E>>],
    /// For each output, a buffer for the node to fill.
    pub outputs: &'a mut [Buffer<E>],
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
    pub fn new(
        sequence: u64,
        duration: f64,
        inputs: &'a [Option<&'a Buffer<E>>],
        outputs: &'a mut [Buffer<E>],
    ) -> Self {
        let taken: &'a mut [Option<Buffer<E>>] = Box::leak(Box::new([]));
        let commands = Box::leak(Box::default());
        let (node, val_ins, shared) = (NodeId(0), &[], &());
        let snapshot = Self::trying(false);
        PassContext { sequence, duration, node, inputs, taken, outputs, val_ins, snapshot, shared, commands }
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

    /// The node reads an input's data with this, whether the node takes the input (see `take_input`) or not.
    pub fn input(&self, input_idx: usize) -> Option<&Buffer<E>> {
        match self.taken.get(input_idx) {
            Some(Some(buffer)) => Some(buffer),
            _ => self.inputs.get(input_idx).copied().flatten(),
        }
    }

    /// A node that takes an input (see `BufferUsage::takes`) gets the input's buffer here, if it's connected, to swap
    /// for another just as big (of the same data stream), which the pass carries on with. The data's the node's to do
    /// as it likes with.
    pub fn take_input(&mut self, input_idx: usize) -> Option<&mut Buffer<E>> {
        self.taken.get_mut(input_idx)?.as_mut()
    }

    /// While recording, a writer finds here the base directory the recording goes in (see `crate::base_directory`), if
    /// there is one.
    pub fn base_directory(&self) -> Option<&crate::base_directory::Place> {
        self.snapshot.recording()?.base_directory()
    }
}
