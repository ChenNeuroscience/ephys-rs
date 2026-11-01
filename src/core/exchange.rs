//! Handing things from a node's passes to its window, without either waiting on the other.
//!
//! Each run has an `Exchange`, where each node can leave the latest of something for its window: a frame to show,
//! say. The engine doesn't make or move these; a node that wants to keeps a `Latest` in its state, and sends to
//! it from its passes. Its window gets the latest sent (as `Context::display`), for as long as the run lasts.
//!
//! Sending fills a buffer of the sender's own, then swaps it for the one the window reads from, unless the window
//! is reading just then: then that pass's copy is skipped, so a pass never waits on the GUI. The buffers are
//! swapped rather than replaced, so once they've grown to size, nothing is allocated. A node whose state is only
//! working memory has a `Latest` on each worker (see `PassState::CARRIED_OVER`): they share the node's slot, and a
//! copy from an earlier pass than the one there is skipped.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::engine::{Edges, NodeId, Pass};

/// One node's latest, for its window.
trait Slot: Send + Sync {
    /// Runs `f` on the latest sent, if anything's been sent yet.
    fn with(&self, f: &mut dyn FnMut(&(dyn Any + Send)));
}

struct TypedSlot<T> {
    /// The pass the latest is from (`None` until something's sent), and the latest.
    latest: Mutex<(Option<u64>, T)>,
}

impl<T: Send + 'static> Slot for TypedSlot<T> {
    fn with(&self, f: &mut dyn FnMut(&(dyn Any + Send))) {
        let latest = self.latest.lock().unwrap();
        if latest.0.is_some() {
            f(&latest.1);
        }
    }
}

/// A node's slot, as itself (for its senders to find) and as a `Slot` (for its window).
struct Entry {
    typed: Arc<dyn Any + Send + Sync>,
    slot: Arc<dyn Slot>,
}

/// Where a run's nodes leave things for their windows.
#[derive(Default)]
pub struct Exchange {
    slots: Mutex<HashMap<NodeId, Entry>>,
}

impl Exchange {
    /// Runs `f` on the latest that node `id` has sent, if it's sent anything. Passes sending meanwhile skip
    /// their copy rather than wait.
    pub fn with_latest<R>(&self, id: NodeId, f: impl FnOnce(&(dyn Any + Send)) -> R) -> Option<R> {
        let slot = self.slots.lock().unwrap().get(&id).map(|e| e.slot.clone())?;
        let mut f = Some(f);
        let mut result = None;
        slot.with(&mut |latest| result = f.take().map(|f| f(latest)));
        result
    }
}

/// The sending end, kept in a node's state: `send` from its passes. Starts unconnected, and connects to the node's
/// slot in the run's exchange on the first send.
pub struct Latest<T> {
    slot: Option<Arc<TypedSlot<T>>>,
    /// The buffer filled before it's swapped in.
    spare: T,
}

impl<T: Default> Default for Latest<T> {
    fn default() -> Self {
        Latest { slot: None, spare: T::default() }
    }
}

impl<T: Default + Send + 'static> Latest<T> {
    /// Fills a buffer with `fill` and makes it the latest for the node's window, unless the window is reading
    /// the latest just then. `fill` gets back an earlier send's buffer, to overwrite.
    pub fn send<E: Edges>(&mut self, pass: &Pass<E>, fill: impl FnOnce(&mut T)) {
        let slot = self.slot.get_or_insert_with(|| {
            let mut slots = pass.exchange.slots.lock().unwrap();
            let existing = slots.get(&pass.node).and_then(|e| e.typed.clone().downcast::<TypedSlot<T>>().ok());
            existing.unwrap_or_else(|| {
                let slot = Arc::new(TypedSlot { latest: Mutex::new((None, T::default())) });
                slots.insert(pass.node, Entry { typed: slot.clone(), slot: slot.clone() });
                slot
            })
        });
        fill(&mut self.spare);
        let Ok(mut latest) = slot.latest.try_lock() else { return };
        if latest.0.is_none_or(|sent| sent < pass.sequence) {
            std::mem::swap(&mut latest.1, &mut self.spare);
            latest.0 = Some(pass.sequence);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edges as ProgramEdges;

    #[test]
    fn the_window_gets_the_latest_sent() {
        let mut pass: Pass<ProgramEdges> = Pass::new(0, 0.02, &[], &mut []);
        pass.node = NodeId(7);
        let exchange = pass.exchange;
        let read = || exchange.with_latest(NodeId(7), |l| *l.downcast_ref::<u32>().unwrap());
        assert_eq!(read(), None, "nothing until something's sent");
        let mut latest = Latest::<u32>::default();
        latest.send(&pass, |n| *n = 1);
        pass.sequence = 1;
        latest.send(&pass, |n| *n = 2);
        assert_eq!(read(), Some(2));
        // Another sender (another worker's) shares the slot; an earlier pass's copy doesn't replace a later one.
        let mut other = Latest::<u32>::default();
        pass.sequence = 5;
        other.send(&pass, |n| *n = 5);
        pass.sequence = 4;
        latest.send(&pass, |n| *n = 4);
        assert_eq!(read(), Some(5));
    }
}
