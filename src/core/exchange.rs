//! What a run's passes and the GUI share, without either waiting on the other.
//!
//! Each run has an `Exchange`. In it, each node can keep things of any type its passes and its window (or the dev
//! tools) both reach: `Pass::shared` finds or makes one, and `Context::shared` (or `ThreadContext::shared`) finds it.
//! What they are, and how they're used, is up to the node: the exchange only keeps them for the run, and doesn't
//! make or move anything per pass. The usual one is a `standard_components::pool::Pool` of buffers, which the passes
//! fill and the window reads the newest of. Anything kept can say where its buffers are (`Shared::pool`), for
//! the dev tools to show.
//!
//! The other way, the GUI can fire a node's triggers (see `Node::triggers`): "auto detect", say. A pass of the node
//! takes it (`Pass::triggered`), says how it's getting on (`Pass::trigger_progress`) and when it's done. And the
//! passes, which can't change the graph, can propose a new value for one of the node's settings (`Pass::propose`),
//! which the GUI applies (as an edit, unless recording).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::engine::{NodeId, PoolStatus};

/// Something a node's passes and its window share (see `Exchange`).
pub trait Shared: Any + Send + Sync {
    /// What it is, for the dev tools: "pool", say.
    fn name(&self) -> &'static str {
        "shared"
    }

    /// Where its buffers are, if it's a pool of them, for the dev tools.
    fn pool(&self) -> Option<PoolStatus> {
        None
    }
}

/// A node's shared thing, as itself (to find by type) and as a `Shared` (for the dev tools).
struct Entry {
    typed: Arc<dyn Any + Send + Sync>,
    shared: Arc<dyn Shared>,
}

/// Where a run's nodes leave things for their windows, and the GUI fires their triggers.
#[derive(Default)]
pub struct Exchange {
    /// What each node shares, by type.
    shared: Mutex<HashMap<(NodeId, TypeId), Entry>>,
    /// Triggers fired, until they're done: how far along each is (0 until a pass takes it).
    triggers: Mutex<HashMap<(NodeId, &'static str), Trigger>>,
    /// Settings' values proposed by passes, for the GUI to apply: the node, which setting, and its value.
    proposals: Mutex<Vec<Proposal>>,
}

/// A setting's value proposed by a pass: the node, which of its settings, and the value.
pub type Proposal = (NodeId, usize, Box<dyn Any + Send>);

/// A trigger fired: waiting for a pass to take it, or taken, and how far along (0 to 1).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Trigger {
    Fired,
    Running(f32),
    /// Stopped by the GUI, until a pass sees it has been.
    Cancelled,
}

impl Exchange {
    /// Fires node `node`'s trigger `name`: its next pass takes it.
    pub fn fire(&self, node: NodeId, name: &'static str) {
        self.triggers.lock().unwrap().insert((node, name), Trigger::Fired);
    }

    /// How far along node `node`'s trigger `name` is (0 to 1), if it's been fired and isn't done (or stopped).
    pub fn trigger_progress(&self, node: NodeId, name: &'static str) -> Option<f32> {
        self.triggers.lock().unwrap().get(&(node, name)).and_then(|t| match t {
            Trigger::Fired => Some(0.0),
            Trigger::Running(progress) => Some(*progress),
            Trigger::Cancelled => None,
        })
    }

    /// Stops node `node`'s trigger `name`, if it's been fired: its passes abandon what they were doing.
    pub fn cancel(&self, node: NodeId, name: &'static str) {
        let mut triggers = self.triggers.lock().unwrap();
        match triggers.get(&(node, name)) {
            // Not taken yet: as if it hadn't been fired.
            Some(Trigger::Fired) => {
                triggers.remove(&(node, name));
            }
            Some(Trigger::Running(_)) => {
                triggers.insert((node, name), Trigger::Cancelled);
            }
            _ => {}
        }
    }

    /// For `Pass`: whether node `node`'s trigger `name` has been stopped, forgetting it if so.
    pub(crate) fn take_cancel(&self, node: NodeId, name: &'static str) -> bool {
        let mut triggers = self.triggers.lock().unwrap();
        let cancelled = triggers.get(&(node, name)) == Some(&Trigger::Cancelled);
        if cancelled {
            triggers.remove(&(node, name));
        }
        cancelled
    }

    /// The settings' values passes have proposed since last time: the node, which setting, and the value.
    pub fn take_proposals(&self) -> Vec<Proposal> {
        std::mem::take(&mut *self.proposals.lock().unwrap())
    }

    /// For `Pass`: whether node `node`'s trigger `name` has been fired, taking it if so.
    pub(crate) fn take_trigger(&self, node: NodeId, name: &'static str) -> bool {
        let mut triggers = self.triggers.lock().unwrap();
        match triggers.get_mut(&(node, name)) {
            Some(trigger @ Trigger::Fired) => {
                *trigger = Trigger::Running(0.0);
                true
            }
            _ => false,
        }
    }

    /// For `Pass`: how far along a trigger taken is, or (`None`) that it's done.
    pub(crate) fn set_trigger_progress(&self, node: NodeId, name: &'static str, progress: Option<f32>) {
        let mut triggers = self.triggers.lock().unwrap();
        match progress {
            // Not over a stop the pass hasn't seen yet.
            Some(_) if triggers.get(&(node, name)) == Some(&Trigger::Cancelled) => {}
            Some(progress) => {
                triggers.insert((node, name), Trigger::Running(progress));
            }
            None => {
                triggers.remove(&(node, name));
            }
        }
    }

    /// For `Pass`: a proposed value for a node's setting.
    pub(crate) fn propose(&self, node: NodeId, setting: usize, value: Box<dyn Any + Send>) {
        self.proposals.lock().unwrap().push((node, setting, value));
    }

    /// Node `node`'s shared `T`, made with `make` if it hasn't one yet (see `Pass::shared`).
    pub fn shared<T: Shared>(&self, node: NodeId, make: impl FnOnce() -> T) -> Arc<T> {
        let mut shared = self.shared.lock().unwrap();
        let entry = shared.entry((node, TypeId::of::<T>())).or_insert_with(|| {
            let made = Arc::new(make());
            Entry { typed: made.clone(), shared: made }
        });
        entry.typed.clone().downcast().expect("kept by its type")
    }

    /// Node `node`'s shared `T`, if it has one.
    pub fn get<T: Shared>(&self, node: NodeId) -> Option<Arc<T>> {
        let shared = self.shared.lock().unwrap();
        shared.get(&(node, TypeId::of::<T>())).and_then(|e| e.typed.clone().downcast().ok())
    }

    /// The pools node `node` shares, and what each is, for the dev tools.
    pub fn pools(&self, node: NodeId) -> Vec<(&'static str, PoolStatus)> {
        let shared: Vec<Arc<dyn Shared>> = self
            .shared
            .lock()
            .unwrap()
            .iter()
            .filter(|((n, _), _)| *n == node)
            .map(|(_, e)| e.shared.clone())
            .collect();
        let mut pools: Vec<_> = shared.iter().filter_map(|s| Some((s.name(), s.pool()?))).collect();
        pools.sort_by_key(|(name, _)| *name);
        pools
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counter(std::sync::atomic::AtomicU32);

    impl Shared for Counter {}

    #[test]
    fn nodes_share_things_by_type() {
        let exchange = Exchange::default();
        assert!(exchange.get::<Counter>(NodeId(7)).is_none(), "nothing until it's made");
        let made = exchange.shared(NodeId(7), Counter::default);
        made.0.store(3, std::sync::atomic::Ordering::Relaxed);
        // The same one, found again; another node's is its own.
        let found = exchange.get::<Counter>(NodeId(7)).unwrap();
        assert_eq!(found.0.load(std::sync::atomic::Ordering::Relaxed), 3);
        assert_eq!(exchange.shared(NodeId(8), Counter::default).0.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(exchange.pools(NodeId(7)).is_empty());
    }
}
