//! What a run's passes and the GUI share, without either waiting on the other.
//!
//! Each run has an `Exchange`. In it, each node can keep things of any type its passes and its window (or the dev
//! tools) both reach: `Pass::shared` finds or makes one, and `UiContext::shared` (or `ThreadContext::shared`) finds it.
//! What they are, and how they're used, is up to the node: the exchange only keeps them for the run, and doesn't
//! make or move anything per pass. The usual one is a `standard_components::pool::Pool` of buffers, which the passes
//! fill and the window reads the newest of. Anything kept can say where its buffers are (`Shared::pool`), for
//! the dev tools to show. Once a run's stopped, its windows still find what's in its exchange, so
//! what they show stays, until the next run, or until the graph changes in a way that would start a new epoch.
//!
//! The other way, the GUI can send a node pass triggers (see `crate::core::triggers`): "auto detect", say. A pass of the
//! node takes it (`Pass::triggered`), says how it's getting on (`Pass::trigger_progress`) and when it's done. And the
//! passes, which can't change the graph, can trigger an update of one of the node's settings
//! (`Pass::trigger_settings_update`): a direct trigger (see `crate::core::triggers`), which doesn't wait for a pass.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::engine::{NodeId, PoolStatus};
use super::triggers::{AnyTrigger, DirectTriggers, Trigger, TriggerKind};

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

/// Where a run's nodes leave things for their windows, and the GUI triggers their pass triggers.
#[derive(Default)]
pub struct Exchange {
    /// What each node shares, by type.
    shared: Mutex<HashMap<(NodeId, TypeId), Entry>>,
    /// Pass triggers triggered, until they're done: how far along each is (0 until a pass takes it).
    triggers: Mutex<HashMap<(NodeId, TriggerKind), PassTrigger>>,
    /// Where direct triggers go: the app's.
    direct: Arc<DirectTriggers>,
}

/// A pass trigger triggered: waiting for a pass to take it, or taken, and how far along (0 to 1).
enum PassTrigger {
    Triggered(AnyTrigger),
    Running(f32),
    /// Stopped by the GUI, until a pass sees it has been.
    Cancelled,
}

impl Exchange {
    /// An exchange whose direct triggers go to `direct`.
    pub fn new(direct: Arc<DirectTriggers>) -> Self {
        Exchange { direct, ..Default::default() }
    }

    /// Where direct triggers go.
    pub fn direct_triggers(&self) -> &DirectTriggers {
        &self.direct
    }

    /// Triggers node `node`'s pass trigger `trigger`: its next pass takes it.
    pub fn trigger(&self, node: NodeId, trigger: AnyTrigger) {
        self.triggers.lock().unwrap().insert((node, trigger.kind()), PassTrigger::Triggered(trigger));
    }

    /// How far along node `node`'s trigger of kind `kind` is (0 to 1), if it's been triggered and isn't done (or
    /// stopped).
    pub fn trigger_progress(&self, node: NodeId, kind: TriggerKind) -> Option<f32> {
        self.triggers.lock().unwrap().get(&(node, kind)).and_then(|t| match t {
            PassTrigger::Triggered(_) => Some(0.0),
            PassTrigger::Running(progress) => Some(*progress),
            PassTrigger::Cancelled => None,
        })
    }

    /// Stops node `node`'s trigger of kind `kind`, if it's been triggered: its passes abandon what they were doing.
    pub fn cancel(&self, node: NodeId, kind: TriggerKind) {
        let mut triggers = self.triggers.lock().unwrap();
        match triggers.get(&(node, kind)) {
            // Not taken yet: as if it hadn't been triggered.
            Some(PassTrigger::Triggered(_)) => {
                triggers.remove(&(node, kind));
            }
            Some(PassTrigger::Running(_)) => {
                triggers.insert((node, kind), PassTrigger::Cancelled);
            }
            _ => {}
        }
    }

    /// For `Pass`: whether node `node`'s trigger of kind `kind` has been stopped, forgetting it if so.
    pub(crate) fn take_cancel(&self, node: NodeId, kind: TriggerKind) -> bool {
        let mut triggers = self.triggers.lock().unwrap();
        let cancelled = matches!(triggers.get(&(node, kind)), Some(PassTrigger::Cancelled));
        if cancelled {
            triggers.remove(&(node, kind));
        }
        cancelled
    }

    /// For `Pass`: node `node`'s trigger `T`, if it's been triggered, taking it.
    pub(crate) fn take_trigger<T: Trigger>(&self, node: NodeId) -> Option<T> {
        let mut triggers = self.triggers.lock().unwrap();
        let state = triggers.get_mut(&(node, TriggerKind::of::<T>()))?;
        if !matches!(state, PassTrigger::Triggered(_)) {
            return None;
        }
        let PassTrigger::Triggered(trigger) = std::mem::replace(state, PassTrigger::Running(0.0)) else { return None };
        trigger.take::<T>().ok()
    }

    /// For `Pass`: how far along a trigger taken is, or (`None`) that it's done.
    pub(crate) fn set_trigger_progress(&self, node: NodeId, kind: TriggerKind, progress: Option<f32>) {
        let mut triggers = self.triggers.lock().unwrap();
        match progress {
            // Not over a stop the pass hasn't seen yet.
            Some(_) if matches!(triggers.get(&(node, kind)), Some(PassTrigger::Cancelled)) => {}
            Some(progress) => {
                triggers.insert((node, kind), PassTrigger::Running(progress));
            }
            None => {
                triggers.remove(&(node, kind));
            }
        }
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

    #[derive(Debug, PartialEq)]
    struct Level(f32);
    impl Trigger for Level {
        const NAME: &'static str = "level";
        const DELIVERY: crate::core::triggers::Delivery = crate::core::triggers::Delivery::Pass;
    }

    #[test]
    fn a_pass_trigger_is_taken_with_what_it_carries_once() {
        let exchange = Exchange::default();
        let node = NodeId(1);
        exchange.trigger(node, AnyTrigger::new(Level(0.5)));
        assert_eq!(exchange.trigger_progress(node, Level::KIND), Some(0.0), "waiting");
        assert_eq!(exchange.take_trigger::<Level>(NodeId(2)), None, "another node's");
        assert_eq!(exchange.take_trigger::<Level>(node), Some(Level(0.5)));
        assert_eq!(exchange.take_trigger::<Level>(node), None, "taken");
        assert_eq!(exchange.trigger_progress(node, Level::KIND), Some(0.0), "running, till it's done");
        exchange.set_trigger_progress(node, Level::KIND, None);
        assert_eq!(exchange.trigger_progress(node, Level::KIND), None);
    }

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
