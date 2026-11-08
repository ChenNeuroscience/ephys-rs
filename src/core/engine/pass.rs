//! What a node works on for one pass (see `run`): its inputs, buffers for its outputs, its controls' settings, whether
//! it's recording, and the run's exchange.

use std::sync::Arc;

use crate::core::exchange::Exchange;

use super::{Buffer, Edges, NodeId, Recording};

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
    /// For each input, its buffer, if the node's taking it (see `Node::takes`).
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
    /// What the node's passes and window share (see `crate::core::exchange`).
    pub exchange: &'a Exchange,
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
        let exchange: &'static Exchange = Box::leak(Box::default());
        let (node, controls, recording, prepared) = (NodeId(0), &[], false, None);
        Pass { sequence, duration, node, inputs, taken, outputs, controls, recording, prepared, exchange }
    }

    /// The node's shared `T` in the run's exchange, made with `make` the first time (see `crate::core::exchange`):
    /// keep it in the node's state, rather than finding it every pass.
    pub fn shared<T: crate::core::exchange::Shared>(&self, make: impl FnOnce() -> T) -> Arc<T> {
        self.exchange.shared(self.node, make)
    }

    /// Whether the node's trigger `name` (see `Node::triggers`) has been triggered, taking it if so: the node then says
    /// how it's getting on (`trigger_progress`) until it's done (`trigger_done`).
    pub fn triggered(&self, name: &'static str) -> bool {
        self.exchange.take_trigger(self.node, name)
    }

    /// Whether the node's trigger `name`, taken, has been stopped since: the node should abandon what it's doing
    /// for it (it's then done).
    pub fn trigger_cancelled(&self, name: &'static str) -> bool {
        self.exchange.take_cancel(self.node, name)
    }

    /// How far along the node's trigger `name` is, from 0 to 1, for the GUI to show.
    pub fn trigger_progress(&self, name: &'static str, progress: f32) {
        self.exchange.set_trigger_progress(self.node, name, Some(progress.clamp(0.0, 1.0)));
    }

    /// The node's trigger `name` is done.
    pub fn trigger_done(&self, name: &'static str) {
        self.exchange.set_trigger_progress(self.node, name, None);
    }

    /// Triggers an update of the node's setting `setting` (by index) to `value`, which the app applies as an edit
    /// (unless it's recording): passes can't change the graph themselves. See `crate::core::triggers`.
    pub fn trigger_settings_update(&self, setting: usize, value: E::Setting) {
        self.exchange.direct_triggers().trigger_settings_update(self.node, setting, Box::new(value));
    }

    /// An input's data, taken (see `take_input`) or not.
    pub fn input(&self, index: usize) -> Option<&Buffer<E>> {
        match self.taken.get(index) {
            Some(Some(buffer)) => Some(buffer),
            _ => self.inputs.get(index).copied().flatten(),
        }
    }

    /// The buffer of an input the node takes (see `Node::takes`), if it's connected: to swap for another just as big
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
