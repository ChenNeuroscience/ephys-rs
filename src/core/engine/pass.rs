//! What a node works on for one pass (see `run`): its inputs, buffers for its outputs, its controls' settings, whether
//! it's recording, what it shares with the GUI, and its triggers.

use std::any::Any;

use super::{Buffer, Edges, NodeId, Recording, TriggerKind};
use crate::core::pass_triggers::PassTriggers;
use crate::core::triggers::DirectTriggers;

/// The length of time each pass covers, in seconds.
pub const PASS_DURATION: f64 = 0.02;

/// What a node works on for one pass.
pub struct Pass<'a, E: Edges> {
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
    /// Whether the graph is recording.
    pub recording: bool,
    /// The recording, with what nodes prepared for it.
    pub(super) prepared: Option<&'a Recording>,
    /// What the node shares with the GUI (its `Node::Shared`): see `shared`.
    pub shared: &'a dyn Any,
    /// The run's pass triggers (see `triggered`).
    pub triggers: &'a PassTriggers,
    /// Where direct triggers go: the app's (see `trigger_settings_update`).
    pub direct: &'a DirectTriggers,
}

impl<'a, E: Edges> Pass<'a, E> {
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
        let (node, controls, recording, prepared, shared) = (NodeId(0), &[], false, None, &());
        Pass {
            sequence,
            duration,
            node,
            inputs,
            taken,
            outputs,
            controls,
            recording,
            prepared,
            shared,
            triggers,
            direct,
        }
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
    /// (unless it's recording): passes can't change the graph themselves. See `crate::core::triggers`.
    pub fn trigger_settings_update(&self, setting: usize, value: E::Setting) {
        self.direct.trigger_settings_update(self.node, setting, Box::new(value));
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

    /// The base directory the recording's in (see `crate::core::base_directory`), while recording, if there is one.
    pub fn base_directory(&self) -> Option<&crate::core::base_directory::Place> {
        self.prepared?.base_directory()
    }

    /// What the node prepared to record (see `Node::prepare_to_record`), if it did and it's of type `T`: given once,
    /// to the first pass that asks while recording.
    pub fn take_prepared<T: 'static>(&self) -> Option<T> {
        let prepared = self.prepared?.take(self.node)?;
        prepared.downcast().ok().map(|b| *b)
    }
}
