//! Passes: each a `PASS_DURATION` of time, which a worker runs through every node in the plan's order (see `run`).
//! Each node's `process` is given its context in the pass (`PassContext`): its inputs, buffers for its outputs, its
//! controls' settings, the snapshot the pass runs against (as nodes see it: whether it's recording, say), what it
//! shares with the GUI, and its triggers.

use std::any::Any;
use std::sync::Arc;

use super::{Buffer, Edges, NodeId, SnapshotLite, TriggerKind};
use crate::pass_triggers::PassTriggers;
use crate::triggers::DirectTriggers;

/// The length of time each pass covers, in seconds.
pub const PASS_DURATION: f64 = 0.02;

/// A node's context in a pass: what its `process` is given, made for it as the pass reaches it.
pub struct PassContext<'a, E: Edges> {
    /// Passes are numbered from 0, each covering the next `duration` seconds. (No node needs them just now.)
    pub sequence: u64,
    pub duration: f64,
    /// The node being run.
    pub node: NodeId,
    /// For each input, this pass's data on it, if it's connected (and not taken: see `take_input`).
    pub inputs: &'a [Option<&'a Buffer<E>>],
    /// For each input, its buffer, if the node's taking it (see `BufferUsage::takes`).
    pub(super) taken: &'a mut [Option<Buffer<E>>],
    /// For each output, a buffer to fill.
    pub outputs: &'a mut [Buffer<E>],
    /// For each of the node's controls, the value of the setting it's connected to, if it is (for a writer to
    /// record, say).
    pub controls: &'a [Option<E::Setting>],
    /// The snapshot this pass runs against, as nodes can see it: see `snapshot`.
    pub(super) snapshot: &'a Arc<dyn SnapshotLite>,
    /// What the node shares with the GUI (its `Node::Shared`): see `shared`.
    pub shared: &'a dyn Any,
    /// The run's pass triggers (see `triggered`).
    pub triggers: &'a PassTriggers,
    /// Where direct triggers go: the app's (see `trigger_settings_update`).
    pub direct: &'a DirectTriggers,
}

impl<'a, E: Edges> PassContext<'a, E> {
    /// A pass for trying a node out on, not recording, with no controls connected.
    #[cfg(test)]
    pub fn new(
        sequence: u64,
        duration: f64,
        inputs: &'a [Option<&'a Buffer<E>>],
        outputs: &'a mut [Buffer<E>],
    ) -> Self {
        let taken: &'a mut [Option<Buffer<E>>] = Box::leak(Box::new([]));
        let (triggers, direct) = (Box::leak(Box::default()), Box::leak(Box::default()));
        let (node, controls, shared) = (NodeId(0), &[], &());
        let snapshot = Self::trying(false);
        PassContext { sequence, duration, node, inputs, taken, outputs, controls, snapshot, shared, triggers, direct }
    }

    /// A snapshot for trying a node out: running as of now, and recording or not (with nothing prepared).
    #[cfg(test)]
    fn trying(recording: bool) -> &'a Arc<dyn SnapshotLite> {
        let recorded = recording.then(|| super::Recorded::now(Arc::default()));
        let trying: Arc<dyn SnapshotLite> =
            Arc::new(super::snapshot_lite::Trying { started: std::time::Instant::now(), recorded });
        Box::leak(Box::new(trying))
    }

    /// For trying a node out: running as of now, and recording or not (with nothing prepared).
    #[cfg(test)]
    pub fn set_recording(&mut self, recording: bool) {
        self.snapshot = Self::trying(recording);
    }

    /// Whether the graph is recording.
    pub fn recording(&self) -> bool {
        self.snapshot.recorded().is_some()
    }

    /// The snapshot this pass runs against, as nodes can see it (`SnapshotLite`), shared: for a node's own thread, which
    /// carries on between passes, to keep (the newest it's been given, say), and ask how long it's been acquiring and
    /// recording, as of whenever it asks.
    pub fn snapshot(&self) -> &'a Arc<dyn SnapshotLite> {
        self.snapshot
    }

    /// What the node shares with the GUI: its `Node::Shared`, which is `T`.
    pub fn shared<T: 'static>(&self) -> &'a T {
        self.shared.downcast_ref().expect("the node's own Node::Shared")
    }

    /// The node's pass trigger of kind `kind` (see `Node::triggers`), if it's been triggered, taking it: the node then
    /// says how it's getting on (`trigger_progress`) until it's done (`trigger_done`).
    pub fn triggered(&self, kind: TriggerKind<E>) -> Option<E::Trigger> {
        self.triggers.take_trigger(self.node, kind)
    }

    /// Whether the node's trigger of kind `kind`, taken, has been stopped since: the node should abandon what it's doing
    /// for it (it's then done).
    pub fn trigger_cancelled(&self, kind: TriggerKind<E>) -> bool {
        self.triggers.take_cancel(self.node, kind.into())
    }

    /// How far along the node's trigger of kind `kind` is, from 0 to 1, for the GUI to show.
    pub fn trigger_progress(&self, kind: TriggerKind<E>, progress: f32) {
        self.triggers.set_trigger_progress(self.node, kind.into(), Some(progress.clamp(0.0, 1.0)));
    }

    /// The node's trigger of kind `kind` is done.
    pub fn trigger_done(&self, kind: TriggerKind<E>) {
        self.triggers.set_trigger_progress(self.node, kind.into(), None);
    }

    /// Triggers an update of the node's setting `setting` (by index) to `value`, which the app applies as an edit
    /// (unless it's recording): passes can't change the graph themselves. See `crate::triggers`.
    pub fn trigger_settings_update(&self, setting: usize, value: E::Setting) {
        self.direct.trigger_settings_update(self.node, setting, Box::new(value));
    }

    /// Has the app start (`on`) recording, as the toolbar's Record does, or stop it, if it isn't already: whatever's
    /// asked last, by any node, is what happens. See `crate::triggers`.
    pub fn trigger_recording(&self, on: bool) {
        self.direct.trigger_recording(self.node, on);
    }

    /// An input's data, taken (see `take_input`) or not.
    pub fn input(&self, index: usize) -> Option<&Buffer<E>> {
        match self.taken.get(index) {
            Some(Some(buffer)) => Some(buffer),
            _ => self.inputs.get(index).copied().flatten(),
        }
    }

    /// The buffer of an input the node takes (see `BufferUsage::takes`), if it's connected: to swap for another just as big
    /// (of the same data stream), which the pass carries on with. The data's the node's to do as it likes with.
    pub fn take_input(&mut self, index: usize) -> Option<&mut Buffer<E>> {
        self.taken.get_mut(index)?.as_mut()
    }

    /// The base directory the recording's in (see `crate::base_directory`), while recording, if there is one.
    pub fn base_directory(&self) -> Option<&crate::base_directory::Place> {
        self.snapshot.recorded()?.recording.base_directory()
    }

    /// What the node prepared to record (see `Node::prepare_to_record`), which is `T`, if it did: given once, to the
    /// first pass that asks while recording.
    pub fn take_prepared<T: 'static>(&self) -> Option<T> {
        let prepared = self.snapshot.recorded()?.recording.take(self.node)?;
        Some(*prepared.downcast().expect("what the node's own prepare_to_record made"))
    }
}
